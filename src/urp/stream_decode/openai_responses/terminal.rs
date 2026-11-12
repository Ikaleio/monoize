//! The terminal response event (PR6c): reconciles `response.output[]` with the stream's
//! reconstruction slots and sends the keyed terminal snapshot (STR-8).
//!
//! Key rule: an entry that continues a node whose `NodeStart` went out carries that node's raw
//! index, which the output order gate maps like every other index. Every other node, including
//! each node of an item that only the terminal carries, gets a fresh index and its STR-14
//! lifecycle right before `ResponseDone`, in terminal order.

use super::item::{ItemClass, OutputItem};
use super::items::item_type;
use super::merge::merge_node_lists;
use super::nodes::{decode_item, node_extra, string_member};
use super::state::{Flow, ResponsesStreamDecoder};
use crate::handlers::usage::parse_usage_from_responses_object;
use crate::urp::decode::openai_responses::{decode_response, incomplete_finish_reason};
use crate::urp::decode::split_extra;
use crate::urp::{
    FinishReason, Node, NodeHeader, RESPONSES_ITEMS_DISCARDED_EXTRA_KEY,
    RESPONSES_RESPONSE_SOURCE_EXTRA_KEY, ResponseOutcome, TerminalNode, UrpStreamEvent,
    is_internal_key,
};
use serde_json::{Map, Value, json};
use std::collections::{BTreeSet, HashMap};

/// One output item at the terminal boundary.
struct Entry {
    output_index: u64,
    /// The entry continues the stream item at `output_index`; otherwise only the terminal
    /// snapshot carries it, at array position `output_index`.
    streamed: bool,
    class: Option<ItemClass>,
    /// Upstream identity keys: the item id and the call id.
    identity: Vec<String>,
    nodes: Vec<Node>,
}

fn is_control(node: &Node) -> bool {
    matches!(node, Node::NextDownstreamEnvelopeExtra { .. })
}

fn identity_keys(id: Option<&str>, call_id: Option<&str>) -> Vec<String> {
    let id = id.filter(|id| !id.is_empty()).map(|id| format!("id:{id}"));
    let call_id = call_id
        .filter(|call_id| !call_id.is_empty())
        .map(|call_id| format!("call_id:{call_id}"));
    id.into_iter().chain(call_id).collect()
}

impl ResponsesStreamDecoder {
    /// Handles `response.completed`, `.incomplete`, `.failed`, and `.cancelled`: releases the
    /// held items, then sends the reconciled `ResponseDone`, or `responses_terminal_conflict`
    /// when the snapshot contradicts the stream.
    pub(super) async fn on_terminal(&mut self, event_name: String, data: &Value) -> Flow {
        let Some(value) = data.get("response") else {
            return Flow::Continue;
        };
        let Some(response) = value.as_object() else {
            return Flow::Continue;
        };
        let terminal_items = response
            .get("output")
            .and_then(Value::as_array)
            .map(Vec::as_slice);
        // PR6c: the output of a non-completed terminal, or of any terminal after a discard
        // request, is the authoritative item set.
        let authoritative = terminal_items.is_some()
            && (self.discard_requested
                || matches!(
                    response.get("status").and_then(Value::as_str),
                    Some("incomplete" | "cancelled" | "failed")
                ));
        let terminal = (0u64..)
            .zip(terminal_items.unwrap_or_default())
            .map(|(position, item)| self.terminal_entry(position, item))
            .collect();
        let (entries, retained) = match self.reconcile(terminal, authoritative) {
            Ok(reconciled) => reconciled,
            Err(reason) => {
                self.release_held(terminal_items, None).await;
                let message = format!(
                    "Responses stream terminal output conflicts with streamed state: {reason}"
                );
                let extra_body =
                    HashMap::from([("conflict_reason".to_string(), Value::String(reason))]);
                return self
                    .protocol_error(
                        "responses_terminal_conflict",
                        message,
                        Some("response.output"),
                        extra_body,
                    )
                    .await;
            }
        };
        self.release_held(terminal_items, retained.as_ref()).await;
        let mut output = self.keyed_output(entries);
        let lifecycles = self.announce_unkeyed(&mut output);
        self.send_all(lifecycles).await;
        let discarded = authoritative && self.discard_requested;
        let done = response_done(value, response, output, discarded);
        self.send_all(vec![done]).await;
        self.terminal_event = Some(event_name);
        Flow::End
    }

    /// Decodes one terminal item. An item without an id takes the id of the stream item at the
    /// same position when both have the same class; otherwise its nodes carry no id, so that no
    /// synthesized id acts as identity.
    fn terminal_entry(&self, position: u64, item: &Value) -> Entry {
        let class = ItemClass::of_type(item_type(item));
        let own_id = string_member(item, "id").filter(|id| !id.is_empty());
        let borrowed = own_id
            .is_none()
            .then(|| {
                self.items
                    .get(&position)
                    .filter(|slot| {
                        class.is_some()
                            && slot.item_type.as_deref().and_then(ItemClass::of_type) == class
                    })
                    .and_then(|slot| slot.id.clone())
            })
            .flatten();
        let nodes = match (&own_id, &borrowed, item.as_object()) {
            (None, Some(id), Some(object)) => {
                let mut object = object.clone();
                object.insert("id".to_string(), Value::String(id.clone()));
                decode_item(&Value::Object(object))
            }
            (None, _, _) => {
                let mut nodes = decode_item(item);
                for node in &mut nodes {
                    node.set_id(None);
                }
                nodes
            }
            (Some(_), _, _) => decode_item(item),
        };
        let id = own_id.or(borrowed);
        let call_id = item.get("call_id").and_then(Value::as_str);
        Entry {
            output_index: position,
            streamed: false,
            class,
            identity: identity_keys(id.as_deref(), call_id),
            nodes,
        }
    }

    /// Merges every terminal entry into the stream item it continues, or appends it as a
    /// terminal-only item. Returns the entries in `output_index` order and, for an
    /// authoritative terminal, the `output_index` set that survives.
    fn reconcile(
        &self,
        terminal: Vec<Entry>,
        authoritative: bool,
    ) -> Result<(Vec<Entry>, Option<BTreeSet<u64>>), String> {
        let mut entries: Vec<Entry> = self
            .items
            .iter()
            .filter_map(|(output_index, item)| {
                Some(Entry {
                    output_index: *output_index,
                    streamed: true,
                    class: Some(item.class()?),
                    identity: identity_keys(
                        item.id.as_deref(),
                        item.call.as_ref().map(|call| call.call_id.as_str()),
                    ),
                    nodes: item.accumulated_nodes(),
                })
            })
            .collect();
        let streamed = entries.len();
        let mut used = vec![false; streamed];
        for entry in terminal {
            match find_stream_entry(&entries[..streamed], &used, &entry) {
                Some(index) => {
                    entries[index].nodes = merge_node_lists(&entries[index].nodes, &entry.nodes)?;
                    used[index] = true;
                }
                None if !entry.nodes.is_empty() => entries.push(entry),
                None => {}
            }
        }
        let retained = authoritative.then(|| {
            // PR6c: a stream item that the authoritative snapshot omits was discarded upstream.
            let mut position = 0;
            entries.retain(|_| {
                let keep = position >= streamed || used[position];
                position += 1;
                keep
            });
            entries.iter().map(|entry| entry.output_index).collect()
        });
        entries.sort_by_key(|entry| entry.output_index);
        Ok((entries, retained))
    }

    fn keyed_output(&self, entries: Vec<Entry>) -> Vec<TerminalNode> {
        let mut images = self.standalone_images.clone();
        let mut output = Vec::new();
        for entry in entries {
            match self
                .items
                .get(&entry.output_index)
                .filter(|_| entry.streamed)
            {
                Some(item) => output.extend(keyed_item_nodes(item, entry.nodes)),
                None => output.extend(entry.nodes.into_iter().map(|node| TerminalNode {
                    node_index: claim_image(&mut images, &node),
                    node,
                })),
            }
        }
        output
    }

    /// Gives every node that no event announced a fresh index and its STR-14 lifecycle, in
    /// terminal order. An envelope control goes out only with the unannounced node after it;
    /// the control of a message whose nodes already streamed can no longer reach that wire
    /// item and stays unannounced (STR-8b).
    fn announce_unkeyed(&mut self, output: &mut [TerminalNode]) -> Vec<UrpStreamEvent> {
        let mut events = Vec::new();
        for position in 0..output.len() {
            if output[position].node_index.is_some() {
                continue;
            }
            if is_control(&output[position].node)
                && !output
                    .get(position + 1)
                    .is_some_and(|next| next.node_index.is_none() && !is_control(&next.node))
            {
                continue;
            }
            let node_index = self.allocate();
            let entry = &mut output[position];
            entry.node_index = Some(node_index);
            let internal = node_extra(&entry.node)
                .iter()
                .filter(|(key, _)| is_internal_key(key))
                .map(|(key, value)| (key.clone(), value.clone()))
                .collect();
            events.push(UrpStreamEvent::NodeStart {
                node_index,
                header: NodeHeader::from(&entry.node),
                extra_body: internal,
            });
            events.push(UrpStreamEvent::NodeDone {
                node_index,
                node: entry.node.clone(),
                usage: None,
                extra_body: HashMap::new(),
            });
        }
        events
    }
}

/// PR6c pairing: identity (item id or call id) first, then the same position with the same
/// item class, then the first free stream item of that class whose accumulated state merges.
fn find_stream_entry(stream: &[Entry], used: &[bool], terminal: &Entry) -> Option<usize> {
    let free = || (0..stream.len()).filter(move |index| !used[*index]);
    if !terminal.identity.is_empty()
        && let Some(index) = free().find(|index| {
            stream[*index]
                .identity
                .iter()
                .any(|key| terminal.identity.contains(key))
        })
    {
        return Some(index);
    }
    let class = terminal.class?;
    free()
        .find(|index| {
            let entry = &stream[*index];
            entry.output_index == terminal.output_index && entry.class == Some(class)
        })
        .or_else(|| {
            free().find(|index| {
                let entry = &stream[*index];
                entry.class == Some(class)
                    && !entry.nodes.is_empty()
                    && merge_node_lists(&entry.nodes, &terminal.nodes).is_ok()
            })
        })
}

/// Keys the final nodes of a stream item with the raw indices of its announced nodes. A
/// message's control is its envelope control and its typed nodes pair with its content parts in
/// order; another item's typed node is the item's own node.
fn keyed_item_nodes(item: &OutputItem, nodes: Vec<Node>) -> Vec<TerminalNode> {
    let typed = nodes.iter().filter(|node| !is_control(node)).count();
    let (mut control, typed_keys) = if item.is_message() {
        let parts: Vec<Option<u32>> = item
            .content
            .values()
            .filter(|slot| slot.node.is_some())
            .map(|slot| slot.node_index)
            .collect();
        let keys = if parts.len() == typed {
            parts
        } else {
            Vec::new()
        };
        (item.control, keys)
    } else {
        (None, vec![item.node_index.filter(|_| item.started)])
    };
    let mut typed_keys = typed_keys.into_iter();
    nodes
        .into_iter()
        .map(|node| {
            let node_index = if is_control(&node) {
                control.take()
            } else {
                typed_keys.next().flatten()
            };
            TerminalNode { node_index, node }
        })
        .collect()
}

/// A terminal image claims the node of the `image_generation.completed` image with its id.
fn claim_image(images: &mut Vec<(u32, Option<String>)>, node: &Node) -> Option<u32> {
    let Node::Image { id: Some(id), .. } = node else {
        return None;
    };
    let position = images
        .iter()
        .position(|(_, image_id)| image_id.as_ref() == Some(id))?;
    Some(images.swap_remove(position).0)
}

fn response_done(
    value: &Value,
    response: &Map<String, Value>,
    output: Vec<TerminalNode>,
    discarded: bool,
) -> UrpStreamEvent {
    let decoded = decode_response(value).ok();
    let calls = output
        .iter()
        .any(|entry| matches!(entry.node, Node::ToolCall { .. }));
    let decoded_finish = decoded.as_ref().and_then(|decoded| decoded.finish_reason);
    let finish_reason = match response.get("status").and_then(Value::as_str) {
        Some("incomplete") => Some(incomplete_finish_reason(response)),
        Some("failed" | "cancelled") => Some(FinishReason::Other),
        Some("completed") if calls => Some(FinishReason::ToolCalls),
        Some("completed") => decoded_finish.or(Some(FinishReason::Stop)),
        _ => decoded_finish,
    };
    let mut extra_body = split_extra(
        response,
        &[
            "id",
            "object",
            "created",
            "created_at",
            "model",
            "output",
            "usage",
            "status",
            "error",
            "incomplete_details",
        ],
    );
    extra_body.insert(RESPONSES_RESPONSE_SOURCE_EXTRA_KEY.to_string(), json!({}));
    if discarded {
        extra_body.insert(
            RESPONSES_ITEMS_DISCARDED_EXTRA_KEY.to_string(),
            Value::Bool(true),
        );
    }
    UrpStreamEvent::ResponseDone {
        outcome: ResponseOutcome::from_responses(response),
        finish_reason,
        usage: decoded
            .and_then(|decoded| decoded.usage)
            .or_else(|| parse_usage_from_responses_object(value)),
        output,
        extra_body,
    }
}
