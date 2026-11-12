//! Reasoning items: text and summary deltas, the summary part lifecycle (STR3i.1), and the
//! payload of a done snapshot that no delta carried.

use super::item::OutputItem;
use super::items::output_index;
use super::nodes::wire_extra;
use super::state::ResponsesStreamDecoder;
use crate::urp::{Node, NodeDelta, ReasoningMetadata, ReasoningSummaryEvent, UrpStreamEvent};
use serde_json::{Value, json};
use std::collections::HashMap;

fn summary_index(data: &Value) -> Option<u64> {
    data.get("summary_index").and_then(Value::as_u64)
}

/// The text of a reasoning event: `primary`, else `fallback`.
fn event_text<'a>(data: &'a Value, primary: &str, fallback: &str) -> Option<&'a str> {
    data.get(primary)
        .and_then(Value::as_str)
        .or_else(|| data.get(fallback).and_then(Value::as_str))
}

impl ResponsesStreamDecoder {
    /// Records the item id and source an event carries and returns the item.
    fn reasoning_item(&mut self, data: &Value) -> &mut OutputItem {
        let item = self.item(output_index(data));
        item.set_id(data.get("item_id").and_then(Value::as_str));
        item.reasoning.merge_source(data);
        item
    }

    pub(super) fn on_reasoning_delta(
        &mut self,
        event_name: &str,
        data: &Value,
    ) -> Vec<UrpStreamEvent> {
        let summary = event_name == "response.reasoning_summary_text.delta";
        let text = event_text(data, "delta", "text").unwrap_or_default();
        let item = self.reasoning_item(data);
        if summary {
            item.reasoning_summary_delta_seen = true;
            item.reasoning.append_summary(summary_index(data), text);
        } else {
            item.reasoning_text_delta_seen = true;
            item.reasoning.append_text(text);
        }
        let payload = (!text.is_empty()).then(|| text.to_string());
        let (content, summary_text) = if summary {
            (None, payload)
        } else {
            (payload, None)
        };
        let delta = NodeDelta::Reasoning {
            metadata: ReasoningMetadata {
                item_id: item.id.clone(),
                summary_index: summary.then(|| summary_index(data)).flatten(),
                ..Default::default()
            },
            content,
            encrypted: None,
            summary: summary_text,
            source: item.reasoning.source.clone(),
        };
        vec![UrpStreamEvent::NodeDelta {
            node_index: self.item_node_index(output_index(data)),
            delta,
            usage: None,
            extra_body: wire_extra(
                data,
                &[
                    "type",
                    "sequence_number",
                    "delta",
                    "text",
                    "output_index",
                    "content_index",
                    "part_index",
                    "summary_index",
                ],
            ),
        }]
    }

    pub(super) fn on_reasoning_text_done(&mut self, data: &Value) {
        let text = event_text(data, "text", "delta").unwrap_or_default();
        self.reasoning_item(data).reasoning.complete_text(text);
    }

    /// STR3i.1: forwards one native summary part lifecycle event without summary text. A
    /// non-empty done text is the complete value of its part (PR6c).
    pub(super) fn on_summary_event(
        &mut self,
        event_name: &str,
        data: &Value,
    ) -> Vec<UrpStreamEvent> {
        let part = || {
            data.get("part")
                .cloned()
                .unwrap_or_else(|| json!({ "type": "summary_text" }))
        };
        let (summary_event, done_text) = match event_name {
            "response.reasoning_summary_part.added" => {
                (ReasoningSummaryEvent::PartAdded { part: part() }, None)
            }
            "response.reasoning_summary_text.done" => {
                let text = event_text(data, "text", "delta").unwrap_or_default();
                let summary_event = ReasoningSummaryEvent::TextDone {
                    text: data
                        .get("text")
                        .and_then(Value::as_str)
                        .unwrap_or_default()
                        .to_string(),
                };
                (summary_event, Some(text))
            }
            _ => {
                let text = data
                    .get("part")
                    .and_then(|part| part.get("text"))
                    .and_then(Value::as_str);
                (ReasoningSummaryEvent::PartDone { part: part() }, text)
            }
        };
        let item = self.reasoning_item(data);
        if let Some(text) = done_text {
            item.reasoning.complete_summary(summary_index(data), text);
        }
        item.reasoning_summary_delta_seen = true;
        let delta = NodeDelta::Reasoning {
            metadata: ReasoningMetadata {
                item_id: item.id.clone(),
                summary_index: summary_index(data),
                summary_event: Some(summary_event),
                ..Default::default()
            },
            content: None,
            encrypted: None,
            summary: None,
            source: item.reasoning.source.clone(),
        };
        vec![UrpStreamEvent::NodeDelta {
            node_index: self.item_node_index(output_index(data)),
            delta,
            usage: None,
            extra_body: HashMap::new(),
        }]
    }
}

/// Deltas for the reasoning payload of a done snapshot that no delta carried: text and summary
/// that no delta streamed, and the encrypted snapshot. A multi-part summary goes out per part.
pub(super) fn reasoning_done_deltas(
    item: &OutputItem,
    node_index: u32,
    snapshot: &Value,
    node: &Node,
) -> Vec<UrpStreamEvent> {
    let Node::Reasoning {
        content,
        encrypted,
        summary,
        source,
        ..
    } = node
    else {
        return Vec::new();
    };
    let content = content
        .clone()
        .filter(|content| !item.reasoning_text_delta_seen && !content.is_empty());
    let summary = summary
        .clone()
        .filter(|summary| !item.reasoning_summary_delta_seen && !summary.is_empty());
    if content.is_none() && summary.is_none() && encrypted.is_none() {
        return Vec::new();
    }
    let metadata = ReasoningMetadata {
        item_id: snapshot
            .get("id")
            .and_then(Value::as_str)
            .map(str::to_string)
            .or_else(|| item.id.clone()),
        ..Default::default()
    };
    let parts = snapshot
        .get("summary")
        .and_then(Value::as_array)
        .filter(|parts| parts.len() > 1 && summary.is_some());
    let delta = |metadata: ReasoningMetadata,
                 content: Option<String>,
                 encrypted: Option<Value>,
                 summary: Option<String>| UrpStreamEvent::NodeDelta {
        node_index,
        delta: NodeDelta::Reasoning {
            metadata,
            content,
            encrypted,
            summary,
            source: source.clone(),
        },
        usage: None,
        extra_body: HashMap::new(),
    };
    let whole_summary = if parts.is_some() { None } else { summary };
    let mut events = vec![delta(
        metadata.clone(),
        content,
        encrypted.clone(),
        whole_summary,
    )];
    for (summary_index, part) in (0u64..).zip(parts.into_iter().flatten()) {
        let Some(text) = part
            .get("text")
            .and_then(Value::as_str)
            .filter(|text| !text.is_empty())
        else {
            continue;
        };
        let metadata = ReasoningMetadata {
            summary_index: Some(summary_index),
            ..metadata.clone()
        };
        events.push(delta(metadata, None, None, Some(text.to_string())));
    }
    events
}
