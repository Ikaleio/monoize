//! Message content: content parts, text and refusal deltas, and annotations.

use super::items::{content_done_extra, output_index};
use super::nodes::{node_extra, part_node, start_header, string_member, wire_extra};
use super::state::ResponsesStreamDecoder;
use crate::urp::{
    Citation, Node, NodeDelta, ProviderProtocol, RESPONSES_CONTENT_PART_SHAPE_KEY, UrpStreamEvent,
};
use serde_json::Value;
use std::collections::HashMap;

pub(super) fn content_index(data: &Value) -> u64 {
    data.get("content_index")
        .or_else(|| data.get("part_index"))
        .and_then(Value::as_u64)
        .unwrap_or(0)
}

fn delta_text(data: &Value) -> &str {
    data.get("delta")
        .and_then(Value::as_str)
        .or_else(|| data.get("text").and_then(Value::as_str))
        .unwrap_or_default()
}

/// Members of a native delta event that the downstream delta event repeats unchanged.
fn delta_wire_members(data: &Value) -> HashMap<String, Value> {
    ["obfuscation", "phase"]
        .into_iter()
        .filter_map(|key| data.get(key).map(|value| (key.to_string(), value.clone())))
        .collect()
}

/// A content part belongs to the item's reasoning node when it is reasoning text or the item
/// is a reasoning item.
fn is_reasoning_part(
    decoder: &mut ResponsesStreamDecoder,
    output_index: u64,
    part: &Value,
) -> bool {
    part.get("type").and_then(Value::as_str) == Some("reasoning_text")
        || decoder.item(output_index).is_reasoning()
}

impl ResponsesStreamDecoder {
    /// The node of one message content part with the message's id, phase, and citations.
    fn message_part_node(&mut self, output_index: u64, content_index: u64, part: &Value) -> Node {
        let slot = self.item(output_index);
        let mut node = part_node(part, slot.role(), Some(slot.message_id()));
        match &mut node {
            Node::ProviderItem { id, extra_body, .. } => {
                *id = string_member(part, "id");
                extra_body.insert(
                    RESPONSES_CONTENT_PART_SHAPE_KEY.to_string(),
                    Value::Bool(true),
                );
            }
            Node::Text {
                phase, citations, ..
            } => {
                phase.clone_from(&slot.phase);
                *citations = slot.citations_for(content_index);
            }
            _ => {}
        }
        node
    }

    pub(super) fn on_part_added(&mut self, data: &Value) -> Vec<UrpStreamEvent> {
        let Some(part) = data.get("part") else {
            return Vec::new();
        };
        let output_index = output_index(data);
        let content_index = content_index(data);
        self.item(output_index)
            .record_part_citations(part, content_index);
        let mut events = Vec::new();
        if is_reasoning_part(self, output_index, part) {
            let slot = self.item(output_index);
            if !slot.started {
                let node = part_node(part, slot.role(), slot.id.clone());
                let extra_body = node_extra(&node).clone();
                self.start_item_node(output_index, start_header(&node), extra_body, &mut events);
            }
            return events;
        }
        let node = self.message_part_node(output_index, content_index, part);
        let slot = self
            .item(output_index)
            .content
            .entry(content_index)
            .or_default();
        let started = slot.node_index.is_some();
        slot.node = Some(node.clone());
        if !started {
            let extra_body = node_extra(&node).clone();
            self.start_content_node(
                output_index,
                content_index,
                start_header(&node),
                extra_body,
                &mut events,
            );
        }
        events
    }

    pub(super) fn on_part_done(&mut self, data: &Value) -> Vec<UrpStreamEvent> {
        let Some(part) = data.get("part") else {
            return Vec::new();
        };
        let output_index = output_index(data);
        let content_index = content_index(data);
        self.item(output_index)
            .record_part_citations(part, content_index);
        if is_reasoning_part(self, output_index, part) {
            return Vec::new();
        }
        let node = self.message_part_node(output_index, content_index, part);
        self.item(output_index)
            .content
            .entry(content_index)
            .or_default()
            .node = Some(node.clone());
        let mut events = Vec::new();
        let Some(node_index) =
            self.ensure_content_started(output_index, content_index, &mut events)
        else {
            return events;
        };
        let slot = self
            .item(output_index)
            .content
            .entry(content_index)
            .or_default();
        if slot.done {
            return events;
        }
        slot.done = true;
        let extra_body = content_done_extra(node_extra(&node).clone(), slot.done_phase.take());
        events.push(UrpStreamEvent::NodeDone {
            node_index,
            node,
            usage: None,
            extra_body,
        });
        events
    }

    /// The raw index of a content part, starting the part from its accumulated node when no
    /// event did.
    fn ensure_content_started(
        &mut self,
        output_index: u64,
        content_index: u64,
        events: &mut Vec<UrpStreamEvent>,
    ) -> Option<u32> {
        let slot = self.item(output_index).content.get(&content_index)?;
        if let Some(node_index) = slot.node_index {
            return Some(node_index);
        }
        let node = slot.node.clone()?;
        let extra_body = node_extra(&node).clone();
        Some(self.start_content_node(
            output_index,
            content_index,
            start_header(&node),
            extra_body,
            events,
        ))
    }

    /// Folds one text, refusal, or annotation event into the accumulated node of its part.
    fn accumulate_content(&mut self, event_name: &str, data: &Value) {
        let content_index = content_index(data);
        let refusal = event_name.starts_with("response.refusal.");
        let slot = self.item(output_index(data));
        let citations = slot.citations_for(content_index);
        if slot
            .content
            .get(&content_index)
            .is_none_or(|content| content.node.is_none())
        {
            let id = Some(slot.message_id());
            let node = if refusal {
                Node::Refusal {
                    logprobs: None,
                    id,
                    content: String::new(),
                    extra_body: HashMap::new(),
                }
            } else {
                Node::Text {
                    logprobs: None,
                    citations: Vec::new(),
                    signature: None,
                    id,
                    role: slot.role(),
                    content: String::new(),
                    phase: slot.phase.clone(),
                    cache_breakpoint: None,
                    extra_body: HashMap::new(),
                }
            };
            slot.content.entry(content_index).or_default().node = Some(node);
        }
        let content = slot.content.entry(content_index).or_default();
        if event_name == "response.output_text.done"
            && let Some(phase) = string_member(data, "phase")
        {
            content.done_phase = Some(phase);
        }
        match content.node.as_mut() {
            Some(Node::Text {
                logprobs,
                content,
                citations: stored,
                phase,
                ..
            }) => {
                if event_name == "response.output_text.delta" {
                    content.push_str(delta_text(data));
                    crate::urp::logprobs::append(
                        logprobs,
                        &crate::urp::logprobs::decode(data.get("logprobs")),
                    );
                } else if event_name == "response.output_text.done" {
                    if let Some(text) = data.get("text").and_then(Value::as_str) {
                        *content = text.to_string();
                    }
                    if let Some(scores) = crate::urp::logprobs::decode(data.get("logprobs")) {
                        *logprobs = Some(scores);
                    }
                }
                if phase.is_none() {
                    *phase = string_member(data, "phase");
                }
                *stored = citations;
            }
            Some(Node::Refusal { content, .. }) if refusal => {
                if event_name == "response.refusal.delta" {
                    content.push_str(
                        data.get("delta")
                            .and_then(Value::as_str)
                            .unwrap_or_default(),
                    );
                } else if let Some(text) = data.get("refusal").and_then(Value::as_str) {
                    *content = text.to_string();
                }
            }
            _ => {}
        }
    }

    /// `response.output_text.done` and `response.refusal.done` only complete accumulated
    /// state; the part completes with its `content_part.done` or item done event.
    pub(super) fn on_content_done(&mut self, event_name: &str, data: &Value) {
        self.accumulate_content(event_name, data);
    }

    pub(super) fn on_text_delta(&mut self, data: &Value) -> Vec<UrpStreamEvent> {
        let output_index = output_index(data);
        let content_index = content_index(data);
        if let Some(extra) = self.pending_request_envelope.take() {
            self.item(output_index).extra = extra;
        }
        self.accumulate_content("response.output_text.delta", data);
        let logprobs = crate::urp::logprobs::decode(data.get("logprobs"));
        // Event-only wire members stay off the node start, whose extras reach the item.
        let mut extra_body = wire_extra(
            data,
            &[
                "type",
                "sequence_number",
                "delta",
                "text",
                "output_index",
                "content_index",
                "part_index",
                "item_id",
                "logprobs",
                "phase",
                "obfuscation",
            ],
        );
        let slot = self.item(output_index);
        for (key, value) in &slot.extra {
            extra_body
                .entry(key.clone())
                .or_insert_with(|| value.clone());
        }
        let started = slot
            .content
            .get(&content_index)
            .and_then(|content| content.node_index);
        let mut events = Vec::new();
        let node_index = match started {
            Some(node_index) => node_index,
            None => {
                if slot.extra.is_empty() {
                    slot.extra = extra_body.clone();
                }
                let node = Node::Text {
                    logprobs: logprobs.clone(),
                    citations: Vec::new(),
                    signature: None,
                    id: Some(slot.message_id()),
                    role: slot.role(),
                    content: String::new(),
                    phase: slot.phase.clone().or_else(|| string_member(data, "phase")),
                    cache_breakpoint: None,
                    extra_body: extra_body.clone(),
                };
                self.start_content_node(
                    output_index,
                    content_index,
                    start_header(&node),
                    extra_body.clone(),
                    &mut events,
                )
            }
        };
        extra_body.extend(delta_wire_members(data));
        events.push(UrpStreamEvent::NodeDelta {
            node_index,
            delta: NodeDelta::Text {
                logprobs,
                citations: Vec::new(),
                signature: None,
                content: delta_text(data).to_string(),
            },
            usage: None,
            extra_body,
        });
        events
    }

    pub(super) fn on_refusal_delta(&mut self, data: &Value) -> Vec<UrpStreamEvent> {
        self.accumulate_content("response.refusal.delta", data);
        let mut events = Vec::new();
        let Some(node_index) =
            self.ensure_content_started(output_index(data), content_index(data), &mut events)
        else {
            return events;
        };
        events.push(UrpStreamEvent::NodeDelta {
            node_index,
            delta: NodeDelta::Refusal {
                logprobs: None,
                content: data
                    .get("delta")
                    .and_then(Value::as_str)
                    .unwrap_or_default()
                    .to_string(),
            },
            usage: None,
            extra_body: delta_wire_members(data),
        });
        events
    }

    pub(super) fn on_annotation(&mut self, data: &Value) -> Vec<UrpStreamEvent> {
        let output_index = output_index(data);
        let content_index = content_index(data);
        let annotation = data.get("annotation");
        if let Some(annotation) = annotation {
            let slot = self.item(output_index);
            let annotation_index = data
                .get("annotation_index")
                .and_then(Value::as_u64)
                .unwrap_or_else(|| slot.citations_for(content_index).len() as u64);
            slot.citations.insert(
                (content_index, annotation_index),
                Citation::decode(annotation.clone(), ProviderProtocol::Responses),
            );
        }
        self.accumulate_content("response.output_text.annotation.added", data);
        let mut events = Vec::new();
        let Some(node_index) =
            self.ensure_content_started(output_index, content_index, &mut events)
        else {
            return events;
        };
        events.push(UrpStreamEvent::NodeDelta {
            node_index,
            delta: NodeDelta::Text {
                logprobs: None,
                citations: annotation
                    .map(|annotation| {
                        Citation::decode(annotation.clone(), ProviderProtocol::Responses)
                    })
                    .into_iter()
                    .collect(),
                signature: None,
                content: String::new(),
            },
            usage: None,
            extra_body: HashMap::new(),
        });
        events
    }
}
