//! The terminal boundary (STR-8d, STR-9, STR3l, STR3n, STR3p, PR6c).

use super::ResponsesStreamEncoder;
use super::items::complete_item;
use super::nodes::{NodeKind, NodeState, OutputItem};
use super::reasoning::response_with_reasoning_durations;
use crate::error::AppResult;
use crate::handlers::routing::now_ts;
use crate::urp::encode::openai_responses::encode_response;
use crate::urp::grouping::GroupKey;
use crate::urp::stream_helpers::{
    sanitize_responses_completed_for_frame_limit, sanitize_responses_output_item_for_frame_limit,
};
use crate::urp::{
    FinishReason, Node, NodeHeader, ProviderProtocol, RESPONSES_CONTENT_PART_SHAPE_KEY,
    RESPONSES_IMAGE_GENERATION_CALL_EXTRA_KEY, RESPONSES_ITEMS_DISCARDED_EXTRA_KEY,
    RESPONSES_REQUEST_ECHO_EXTRA_KEY, RESPONSES_RESPONSE_SOURCE_EXTRA_KEY, ResponseOutcome,
    ResponseStatus, TerminalNode, UrpResponse, Usage, is_internal_key,
};
use serde_json::{Value, json};
use std::collections::{HashMap, HashSet};

impl ResponsesStreamEncoder {
    pub(super) async fn on_response_done(
        &mut self,
        outcome: Option<ResponseOutcome>,
        finish_reason: Option<FinishReason>,
        usage: Option<Usage>,
        output: Vec<TerminalNode>,
        mut extra_body: HashMap<String, Value>,
    ) -> AppResult<()> {
        if let Some(echo) = &self.options.responses_request_echo
            && !extra_body.contains_key(RESPONSES_RESPONSE_SOURCE_EXTRA_KEY)
        {
            extra_body.insert(RESPONSES_REQUEST_ECHO_EXTRA_KEY.to_string(), echo.clone());
        }
        let status = outcome.as_ref().map_or_else(
            || ResponseOutcome::from_finish(finish_reason).status,
            |outcome| outcome.status,
        );
        // STR3p: an item still open when a non-completed response ends is incomplete.
        let incomplete = status != ResponseStatus::Completed
            || extra_body.contains_key(RESPONSES_ITEMS_DISCARDED_EXTRA_KEY);
        let entry_items = self.close_lifecycles(&output, incomplete).await?;
        let slots = encoded_item_slots(&output, &entry_items);
        let completed_at = extra_body.get("completed_at").cloned();
        let logical_model = self.options.logical_model.clone();
        let response = encode_response(
            &UrpResponse {
                outcome,
                id: self.response_id.clone(),
                model: logical_model.clone(),
                created_at: self.created_at,
                output: TerminalNode::into_nodes(output),
                finish_reason,
                usage,
                extra_body,
            },
            &logical_model,
        );
        let response = response_with_reasoning_durations(response, self.reasoning_duration_secs());
        let mut completed =
            sanitize_responses_completed_for_frame_limit(&response, self.wire.max_frame_length());
        if let Some(completed_at) = completed_at {
            completed["completed_at"] = completed_at;
        }
        let status = completed
            .get("status")
            .and_then(Value::as_str)
            .unwrap_or("completed")
            .to_string();
        reconcile_with_done_items(&mut completed, &slots, &self.items, status == "completed");
        if status == "completed" && completed.get("completed_at").is_none() {
            completed["completed_at"] = json!(now_ts());
        }
        let name = match status.as_str() {
            "incomplete" => "response.incomplete",
            "failed" => "response.failed",
            "cancelled" => "response.cancelled",
            _ => "response.completed",
        };
        self.wire
            .event(name, json!({ "response": completed }))
            .await
    }

    /// Closes every open lifecycle, gives each unannounced entry its whole lifecycle (STR-8b),
    /// and returns the `output_index` of the wire item behind each terminal entry.
    async fn close_lifecycles(
        &mut self,
        output: &[TerminalNode],
        incomplete: bool,
    ) -> AppResult<Vec<Option<usize>>> {
        let announced: HashSet<u32> = self.nodes.keys().copied().collect();
        let resolved: Vec<(Option<u32>, &Node)> =
            TerminalNode::resolve(output, move |index| announced.contains(&index)).collect();
        let claimed: HashMap<u32, &Node> = resolved
            .iter()
            .filter_map(|(index, node)| index.map(|index| (index, *node)))
            .collect();
        let mut open: Vec<u32> = self
            .nodes
            .iter()
            .filter(|(_, state)| !state.closed)
            .map(|(index, _)| *index)
            .collect();
        open.sort_unstable();
        for index in open {
            match claimed.get(&index) {
                // STR-8e: the terminal entry is the final state of the node.
                Some(node) => self.close_node(index, node, &HashMap::new(), false).await?,
                None => self.close_discarded(index, incomplete).await?,
            }
        }
        let mut next_index = self.nodes.keys().max().map_or(0, |max| max + 1);
        let mut entry_items = Vec::with_capacity(resolved.len());
        for (index, node) in resolved {
            let index = match index {
                Some(index) => index,
                None => {
                    let index = next_index;
                    next_index += 1;
                    // The STR-14 start of the node: its internal members select the shape.
                    let start_extra = node_extra(node)
                        .iter()
                        .filter(|(key, _)| is_internal_key(key))
                        .map(|(key, value)| (key.clone(), value.clone()))
                        .collect();
                    self.on_node_start(index, NodeHeader::from(node), start_extra)
                        .await?;
                    self.close_node(index, node, &HashMap::new(), false).await?;
                    index
                }
            };
            entry_items.push(self.nodes.get(&index).and_then(|state| state.output_index));
        }
        if let Some(open) = self.open_message.take() {
            self.emit_message_done(open.output_index, incomplete)
                .await?;
        }
        Ok(entry_items)
    }

    /// Closes a streamed node that the terminal snapshot omits with what the stream carried.
    /// Its lifecycle events stay emitted; the node is not part of the final response (STR-8e).
    async fn close_discarded(&mut self, node_index: u32, incomplete: bool) -> AppResult<()> {
        let Some(state) = self.nodes.get(&node_index) else {
            return Ok(());
        };
        let item = state
            .output_index
            .map_or(&Value::Null, |output_index| &self.items[output_index].item);
        if let Some(node) = discarded_node(state, item) {
            return self
                .close_node(node_index, &node, &HashMap::new(), incomplete)
                .await;
        }
        let Some(state) = self.nodes.get_mut(&node_index) else {
            return Ok(());
        };
        state.closed = true;
        let Some(output_index) = state.output_index.filter(|_| state.kind != NodeKind::Part) else {
            return Ok(());
        };
        let item = &mut self.items[output_index];
        if !item.added || item.done {
            return Ok(());
        }
        let done = sanitize_responses_output_item_for_frame_limit(
            &complete_item(item.item.clone()),
            self.wire.max_frame_length(),
        );
        item.item = done.clone();
        item.done = true;
        let payload = json!({ "output_index": output_index, "item": done });
        self.wire.event("response.output_item.done", payload).await
    }
}

/// The node the stream carried for `state`, or `None` when nothing typed can close it.
fn discarded_node(state: &NodeState, item: &Value) -> Option<Node> {
    let streamed = &state.streamed;
    match &state.header {
        NodeHeader::Text {
            citations,
            signature,
            id,
            role,
            phase,
        } => Some(Node::Text {
            logprobs: None,
            citations: citations.clone(),
            signature: signature.clone(),
            id: id.clone(),
            role: *role,
            content: streamed.text.clone(),
            phase: phase.clone(),
            cache_breakpoint: None,
            extra_body: HashMap::new(),
        }),
        NodeHeader::Refusal { id } => Some(Node::Refusal {
            logprobs: None,
            id: id.clone(),
            content: streamed.text.clone(),
            extra_body: HashMap::new(),
        }),
        NodeHeader::Reasoning { metadata, id } => Some(Node::Reasoning {
            metadata: metadata.clone(),
            id: id.clone(),
            content: Some(streamed.text.clone()).filter(|text| !text.is_empty()),
            encrypted: streamed.encrypted.clone(),
            summary: Some(streamed.summary.text()).filter(|text| !text.is_empty()),
            source: streamed.source.clone(),
            extra_body: state.start_extra.clone(),
        }),
        NodeHeader::ToolCall {
            namespace,
            signature,
            id,
            tool_type,
            call_id,
            name,
        } => Some(Node::ToolCall {
            namespace: namespace.clone(),
            signature: signature.clone(),
            id: id.clone(),
            tool_type: *tool_type,
            call_id: call_id.clone(),
            name: name.clone(),
            arguments: streamed.text.clone(),
            cache_breakpoint: None,
            extra_body: state.start_extra.clone(),
        }),
        NodeHeader::ProviderItem {
            id,
            origin_protocol,
            role,
            item_type,
            body,
        } => {
            let body = if state.kind == NodeKind::Provider {
                item.clone()
            } else {
                body.clone().unwrap_or_else(|| json!({ "type": item_type }))
            };
            Some(Node::ProviderItem {
                id: id.clone(),
                origin_protocol: *origin_protocol,
                role: *role,
                item_type: item_type.clone(),
                body,
                extra_body: state.start_extra.clone(),
            })
        }
        _ => None,
    }
}

fn node_extra(node: &Node) -> &HashMap<String, Value> {
    match node {
        Node::Text { extra_body, .. }
        | Node::Image { extra_body, .. }
        | Node::Audio { extra_body, .. }
        | Node::File { extra_body, .. }
        | Node::Refusal { extra_body, .. }
        | Node::Reasoning { extra_body, .. }
        | Node::ToolCall { extra_body, .. }
        | Node::ProviderItem { extra_body, .. }
        | Node::ToolResult { extra_body, .. }
        | Node::NextDownstreamEnvelopeExtra { extra_body } => extra_body,
    }
}

/// How the non-stream Responses encoder renders one terminal node.
enum Rendered<'a> {
    /// A `message` content part with its envelope key.
    Part(GroupKey<'a>),
    /// A top-level item of its own.
    Item,
    /// Nothing; the node still ends the open `message` item.
    Nothing,
}

fn rendered(node: &Node) -> Rendered<'_> {
    let part = || GroupKey::of_node(node).map_or(Rendered::Nothing, Rendered::Part);
    match node {
        Node::Text { .. } | Node::Refusal { .. } => part(),
        Node::ProviderItem {
            origin_protocol,
            extra_body,
            ..
        } if *origin_protocol == ProviderProtocol::Responses => {
            if extra_body.contains_key(RESPONSES_CONTENT_PART_SHAPE_KEY) {
                part()
            } else {
                Rendered::Item
            }
        }
        Node::Reasoning { .. } if super::reasoning::node_has_payload(node) => Rendered::Item,
        Node::Image { extra_body, .. }
            if extra_body.contains_key(RESPONSES_IMAGE_GENERATION_CALL_EXTRA_KEY) =>
        {
            Rendered::Item
        }
        Node::ToolCall { .. } | Node::ToolResult { .. } => Rendered::Item,
        _ => Rendered::Nothing,
    }
}

/// The wire item behind each `response.output[]` item of the non-stream encoding of `output`.
/// The walk applies the item boundaries of that encoder (RECON-6, RECON-7) to the entries,
/// so the association follows terminal keys only (STR-8d).
fn encoded_item_slots(
    output: &[TerminalNode],
    entry_items: &[Option<usize>],
) -> Vec<Option<usize>> {
    let mut slots: Vec<Option<usize>> = Vec::new();
    let mut last_part: Option<GroupKey<'_>> = None;
    for (entry, item) in output.iter().zip(entry_items) {
        match rendered(&entry.node) {
            Rendered::Part(key) => {
                let joins = last_part
                    .is_some_and(|last| last.continues_with(key) && last.phase == key.phase);
                match slots.last_mut() {
                    Some(slot) if joins => {
                        if slot.is_none() {
                            *slot = *item;
                        }
                    }
                    _ => slots.push(*item),
                }
                last_part = Some(key);
            }
            Rendered::Item => {
                slots.push(*item);
                last_part = None;
            }
            Rendered::Nothing => last_part = None,
        }
    }
    slots
}

/// STR3l: each terminal item carries the wire `id` of the item that emitted its lifecycle, and
/// a `completed` terminal also takes that item's status.
fn reconcile_with_done_items(
    response: &mut Value,
    slots: &[Option<usize>],
    items: &[OutputItem],
    with_status: bool,
) {
    let Some(output) = response.get_mut("output").and_then(Value::as_array_mut) else {
        return;
    };
    let mut used = HashSet::new();
    for (terminal_item, slot) in output.iter_mut().zip(slots) {
        let Some(done) = slot
            .filter(|index| used.insert(*index))
            .and_then(|index| items.get(index))
            .filter(|item| item.done)
            .map(|item| &item.item)
        else {
            continue;
        };
        if done.get("type") != terminal_item.get("type") {
            continue;
        }
        let Some(obj) = terminal_item.as_object_mut() else {
            continue;
        };
        if let Some(id) = done
            .get("id")
            .filter(|id| id.as_str().is_some_and(|id| !id.is_empty()))
        {
            obj.insert("id".to_string(), id.clone());
        }
        if with_status && let Some(status) = done.get("status") {
            obj.insert("status".to_string(), status.clone());
        }
    }
}
