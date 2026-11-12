//! Output item lifecycle: `response.output_item.added`, `response.output_item.done`, and the
//! PR5b close of an item that never received its done event.

use super::item::CallAccumulator;
use super::merge::merge_nodes;
use super::nodes::{
    content_indices, first_item_node, item_extra, item_nodes, node_extra, part_extra, part_node,
    start_header, string_member, wire_extra,
};
use super::reasoning::reasoning_done_deltas;
use super::state::ResponsesStreamDecoder;
use crate::urp::stream_helpers::extract_responses_message_phase;
use crate::urp::{
    Node, NodeDelta, NodeHeader, OrdinaryRole, RESPONSES_MESSAGE_DONE_CONTROL,
    RESPONSES_TEXT_DONE_PHASE_EXTRA_KEY, ToolCallType, UrpStreamEvent, node_is_empty_text,
};
use serde_json::{Value, json};
use std::collections::HashMap;

pub(super) fn output_index(data: &Value) -> u64 {
    data.get("output_index")
        .and_then(Value::as_u64)
        .unwrap_or(0)
}

pub(super) fn item_type(item: &Value) -> &str {
    item.get("type").and_then(Value::as_str).unwrap_or("")
}

/// Role of an output item; a role URP does not model reads as assistant.
fn item_role(item: &Value) -> OrdinaryRole {
    match item.get("role").and_then(Value::as_str) {
        Some("system") => OrdinaryRole::System,
        Some("developer") => OrdinaryRole::Developer,
        Some("user") => OrdinaryRole::User,
        _ => OrdinaryRole::Assistant,
    }
}

fn is_tool_result(kind: &str) -> bool {
    matches!(kind, "function_call_output" | "custom_tool_call_output")
}

/// `NodeDone.extra_body` of a content part, carrying a native done-event `phase`.
pub(super) fn content_done_extra(
    mut extra_body: HashMap<String, Value>,
    done_phase: Option<String>,
) -> HashMap<String, Value> {
    if let Some(phase) = done_phase {
        extra_body.insert(
            RESPONSES_TEXT_DONE_PHASE_EXTRA_KEY.to_string(),
            Value::String(phase),
        );
    }
    extra_body
}

impl ResponsesStreamDecoder {
    pub(super) fn on_item_added(&mut self, data: &Value) -> Vec<UrpStreamEvent> {
        let Some(item) = data.get("item") else {
            return Vec::new();
        };
        let output_index = output_index(data);
        let kind = item_type(item);
        self.record_item_snapshot(output_index, item, false);
        let pending = if kind == "message" {
            self.pending_request_envelope.take()
        } else {
            None
        };
        let slot = self.item(output_index);
        slot.item_type = Some(kind.to_string());
        slot.role = Some(item_role(item));
        slot.phase = extract_responses_message_phase(item);
        if kind == "message" {
            slot.message_id();
            if let Some(extra) = pending {
                slot.extra = extra;
            }
            slot.added_content = item_nodes(item);
        }
        // Tool items keep their members on the node; an envelope control would shift stream
        // node positions relative to the terminal output.
        if !matches!(kind, "function_call" | "custom_tool_call") && !is_tool_result(kind) {
            for (key, value) in item_extra(item) {
                slot.extra.entry(key).or_insert(value);
            }
        }
        if kind == "message" || slot.started {
            return Vec::new();
        }
        let header = if kind == "image_generation_call" {
            NodeHeader::Image {
                metadata: Default::default(),
                id: string_member(item, "id"),
                role: OrdinaryRole::Assistant,
            }
        } else if let Some(node) = first_item_node(item) {
            start_header(&node)
        } else if kind == "reasoning" {
            // An id-less reasoning item without payload decodes to no node.
            NodeHeader::Reasoning {
                metadata: Default::default(),
                id: None,
            }
        } else {
            return Vec::new();
        };
        let extra_body = if matches!(kind, "reasoning" | "function_call" | "custom_tool_call") {
            part_extra(item)
        } else {
            item_extra(item)
        };
        let mut events = Vec::new();
        self.start_item_node(output_index, header, extra_body, &mut events);
        events
    }

    pub(super) fn on_item_done(&mut self, data: &Value) -> Vec<UrpStreamEvent> {
        let Some(item) = data.get("item") else {
            return Vec::new();
        };
        let output_index = output_index(data);
        self.record_item_snapshot(output_index, item, true);
        self.item(output_index)
            .item_type
            .get_or_insert_with(|| item_type(item).to_string());
        self.close_item(output_index, data, item)
    }

    /// Folds an item snapshot into the item's accumulators. `done` marks the complete
    /// `response.output_item.done` snapshot, which owns its non-empty fields (PR6c).
    fn record_item_snapshot(&mut self, output_index: u64, item: &Value, done: bool) {
        let kind = item_type(item);
        let slot = self.item(output_index);
        if let Some(parts) = item.get("content").and_then(Value::as_array) {
            for (content_index, part) in (0u64..).zip(parts) {
                slot.record_part_citations(part, content_index);
            }
        }
        slot.set_id(item.get("id").and_then(Value::as_str));
        match kind {
            "function_call" | "custom_tool_call" => {
                let custom = kind == "custom_tool_call";
                let name = item.get("name").and_then(Value::as_str).unwrap_or("");
                let arguments = item
                    .get(if custom { "input" } else { "arguments" })
                    .and_then(Value::as_str)
                    .unwrap_or("");
                let Some(call_id) = item.get("call_id").and_then(Value::as_str) else {
                    return;
                };
                if let Some(call) = slot.call.as_mut() {
                    if done {
                        call.apply_snapshot(custom, name, arguments);
                    }
                } else {
                    let tool_type = if custom {
                        ToolCallType::Custom
                    } else {
                        ToolCallType::Function
                    };
                    slot.call = Some(CallAccumulator::new(call_id, tool_type, name, arguments));
                }
                if let (Some(call), Some(namespace)) =
                    (slot.call.as_mut(), string_member(item, "namespace"))
                {
                    call.namespace = Some(namespace);
                }
            }
            "reasoning" => {
                slot.reasoning.merge_source(item);
                for (key, value) in part_extra(item) {
                    slot.reasoning.extra_body.entry(key).or_insert(value);
                }
                slot.reasoning.merge_item_snapshot(item, done);
            }
            _ => {}
        }
    }

    /// Completes an item from a done snapshot.
    fn close_item(&mut self, output_index: u64, data: &Value, item: &Value) -> Vec<UrpStreamEvent> {
        match item_type(item) {
            "message" => self.close_message(output_index, item),
            "reasoning" | "function_call" | "custom_tool_call" => {
                self.close_payload_item(output_index, data, item)
            }
            _ => self.close_plain_item(output_index, item),
        }
    }

    /// The done snapshot replaces the accumulated content; an empty snapshot text keeps the
    /// accumulated text. Every part not yet done completes, then the item's own members follow
    /// as a control.
    fn close_message(&mut self, output_index: u64, item: &Value) -> Vec<UrpStreamEvent> {
        let completed: Vec<(u64, Node)> = content_indices(item)
            .into_iter()
            .zip(item_nodes(item))
            .collect();
        let slot = self.item(output_index);
        if !completed.is_empty() {
            let mut previous: HashMap<u64, Node> = slot
                .content
                .iter_mut()
                .filter_map(|(index, content)| content.node.take().map(|node| (*index, node)))
                .collect();
            for (content_index, node) in completed {
                let node = match previous.remove(&content_index) {
                    Some(previous) if node_is_empty_text(&node) => {
                        merge_nodes(&previous, &node).unwrap_or(node)
                    }
                    _ => node,
                };
                slot.content.entry(content_index).or_default().node = Some(node);
            }
        }
        let open: Vec<u64> = slot
            .content
            .iter()
            .filter(|(_, content)| !content.done && content.node.is_some())
            .map(|(content_index, _)| *content_index)
            .collect();
        let mut events = Vec::new();
        for content_index in open {
            self.complete_content(output_index, content_index, &mut events);
        }
        events.push(UrpStreamEvent::ProviderControl {
            protocol: "responses".to_string(),
            event_name: RESPONSES_MESSAGE_DONE_CONTROL.to_string(),
            data: Value::Object(item_extra(item).into_iter().collect()),
            extra_body: HashMap::new(),
        });
        events
    }

    /// Completes one content part from its accumulated node. A part that no event started
    /// starts here, and its text arrives as one delta.
    fn complete_content(
        &mut self,
        output_index: u64,
        content_index: u64,
        events: &mut Vec<UrpStreamEvent>,
    ) {
        let Some(content) = self.item(output_index).content.get(&content_index) else {
            return;
        };
        let (Some(node), started) = (content.node.clone(), content.node_index) else {
            return;
        };
        let node_index = match started {
            Some(node_index) => node_index,
            None => {
                let extra_body = node_extra(&node).clone();
                let node_index = self.start_content_node(
                    output_index,
                    content_index,
                    start_header(&node),
                    extra_body.clone(),
                    events,
                );
                if let Node::Text {
                    content,
                    citations,
                    signature,
                    ..
                } = &node
                    && !content.is_empty()
                {
                    events.push(UrpStreamEvent::NodeDelta {
                        node_index,
                        delta: NodeDelta::Text {
                            logprobs: None,
                            signature: signature.clone(),
                            citations: citations.clone(),
                            content: content.clone(),
                        },
                        usage: None,
                        extra_body,
                    });
                }
                node_index
            }
        };
        let content = self
            .item(output_index)
            .content
            .entry(content_index)
            .or_default();
        content.done = true;
        let extra_body = content_done_extra(HashMap::new(), content.done_phase.take());
        events.push(UrpStreamEvent::NodeDone {
            node_index,
            node,
            usage: None,
            extra_body,
        });
    }

    /// Completes a reasoning or call item. Payload that no delta carried arrives as deltas
    /// before the node completes.
    fn close_payload_item(
        &mut self,
        output_index: u64,
        data: &Value,
        item: &Value,
    ) -> Vec<UrpStreamEvent> {
        let slot = self.item(output_index);
        if slot.node_done {
            return Vec::new();
        }
        let node =
            first_item_node(item).unwrap_or_else(|| part_node(item, slot.role(), slot.id.clone()));
        let started = slot.started;
        let mut events = Vec::new();
        let node_index = if started {
            self.item_node_index(output_index)
        } else {
            self.start_item_node(
                output_index,
                start_header(&node),
                part_extra(item),
                &mut events,
            )
        };
        let slot = self.item(output_index);
        match &node {
            Node::Reasoning { .. } => {
                events.extend(reasoning_done_deltas(slot, node_index, item, &node));
            }
            Node::ToolCall { arguments, .. }
                if !slot.arguments_delta_seen && !arguments.is_empty() =>
            {
                events.push(UrpStreamEvent::NodeDelta {
                    node_index,
                    delta: NodeDelta::ToolCallArguments {
                        arguments: arguments.clone(),
                    },
                    usage: None,
                    extra_body: wire_extra(
                        data,
                        &["item", "output_index", "content_index", "part_index"],
                    ),
                });
            }
            _ => {}
        }
        slot.node_done = true;
        events.push(UrpStreamEvent::NodeDone {
            node_index,
            node,
            usage: None,
            extra_body: part_extra(item),
        });
        events
    }

    /// Completes an item kind without an accumulator; its done node is the item's state.
    fn close_plain_item(&mut self, output_index: u64, item: &Value) -> Vec<UrpStreamEvent> {
        if self.item(output_index).node_done {
            return Vec::new();
        }
        let Some(node) = first_item_node(item).filter(|node| !node_is_empty_text(node)) else {
            return Vec::new();
        };
        let extra_body = if is_tool_result(item_type(item)) {
            item_extra(item)
        } else {
            part_extra(item)
        };
        let mut events = Vec::new();
        let node_index = if self.item(output_index).started {
            self.item_node_index(output_index)
        } else {
            let header = start_header(&node);
            self.start_item_node(output_index, header, extra_body.clone(), &mut events)
        };
        self.item(output_index).node_done = true;
        events.push(UrpStreamEvent::NodeDone {
            node_index,
            node,
            usage: None,
            extra_body,
        });
        events
    }

    /// PR5b: closes an item still open while a later item is released, as if its
    /// `response.output_item.done` had arrived, from the item's terminal snapshot when one
    /// exists. Otherwise a message closes from accumulated state, with status `incomplete` on
    /// a failure path (`terminal` is `None`); another item closes from its accumulated node
    /// only at the terminal boundary.
    pub(super) fn close_open_item(
        &mut self,
        output_index: u64,
        terminal: Option<&[Value]>,
    ) -> Vec<UrpStreamEvent> {
        let Some(slot) = self.items.get(&output_index) else {
            return Vec::new();
        };
        if !slot.emitted_any() {
            return Vec::new();
        }
        let kind = slot.item_type.clone().unwrap_or_default();
        let id = slot.id.clone();
        let snapshot = terminal.and_then(|items| {
            items
                .iter()
                .find(|item| {
                    id.is_some() && item.get("id").and_then(Value::as_str) == id.as_deref()
                })
                .or_else(|| {
                    items
                        .get(output_index as usize)
                        .filter(|item| id.is_none() && item_type(item) == kind)
                })
                .cloned()
        });
        if slot.is_message() {
            let item = snapshot.unwrap_or_else(|| {
                let mut item = json!({ "type": "message", "role": slot.role() });
                if let Some(id) = &id {
                    item["id"] = json!(id);
                }
                if terminal.is_none() {
                    item["status"] = json!("incomplete");
                }
                item
            });
            return self.close_message(output_index, &item);
        }
        if slot.node_done {
            return Vec::new();
        }
        if let Some(item) = snapshot {
            let data = json!({
                "type": "response.output_item.done",
                "output_index": output_index,
                "item": item,
            });
            return self.close_item(output_index, &data, &data["item"]);
        }
        if terminal.is_none() || !slot.started {
            return Vec::new();
        }
        let Some(node) = slot.accumulated_node() else {
            return Vec::new();
        };
        let node_index = self.item_node_index(output_index);
        self.item(output_index).node_done = true;
        vec![UrpStreamEvent::NodeDone {
            node_index,
            node,
            usage: None,
            extra_body: HashMap::new(),
        }]
    }
}
