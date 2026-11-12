//! Reasoning item sub-lifecycles: summary parts (STR3i) and the `reasoning_text` part
//! (STR3d), plus reasoning durations (STR3o) and the payload rule of PR2g.

use super::nodes::Streamed;
use super::wire::{Wire, with_delta_wire_fields};
use crate::error::AppResult;
use crate::urp::{Node, NodeDelta, ReasoningMetadata, ReasoningSummaryEvent};
use serde_json::{Value, json};
use std::collections::{BTreeMap, HashMap};

/// The wire coordinates of one reasoning item.
#[derive(Clone, Copy)]
pub(super) struct SummaryTarget<'a> {
    item_id: &'a str,
    output_index: usize,
    source: Option<&'a str>,
}

impl SummaryTarget<'_> {
    fn payload(&self, members: Value) -> Value {
        let mut payload = json!({ "item_id": self.item_id, "output_index": self.output_index });
        if let (Some(payload), Value::Object(members)) = (payload.as_object_mut(), members) {
            payload.extend(members);
        }
        payload
    }

    async fn send_text_done(&self, wire: &mut Wire, index: u64, text: &str) -> AppResult<()> {
        let payload = self.payload(json!({ "summary_index": index, "text": text }));
        wire.event(
            "response.reasoning_summary_text.done",
            insert_reasoning_source(payload, self.source),
        )
        .await
    }

    async fn send_part(
        &self,
        wire: &mut Wire,
        name: &str,
        index: u64,
        part: Value,
    ) -> AppResult<()> {
        let payload = self.payload(json!({ "summary_index": index, "part": part }));
        wire.event(name, payload).await
    }
}

#[derive(Debug, Default)]
struct SummaryPart {
    text: String,
    text_done: bool,
    part_done: bool,
}

/// STR3i: one reasoning item's summary parts, addressed by `summary_index`.
#[derive(Debug, Default)]
pub(super) struct ReasoningSummaryLifecycle {
    parts: BTreeMap<u64, SummaryPart>,
    open: Option<u64>,
    finished: bool,
}

impl ReasoningSummaryLifecycle {
    /// Closes `index`. `final_text` replaces accumulated text in the done events.
    async fn close_part(
        &mut self,
        wire: &mut Wire,
        target: SummaryTarget<'_>,
        index: u64,
        final_text: Option<&str>,
    ) -> AppResult<()> {
        if self.open == Some(index) {
            self.open = None;
        }
        let Some(state) = self.parts.get_mut(&index) else {
            return Ok(());
        };
        let text = final_text.map_or_else(|| state.text.clone(), str::to_string);
        if !std::mem::replace(&mut state.text_done, true) {
            target.send_text_done(wire, index, &text).await?;
        }
        if !std::mem::replace(&mut state.part_done, true) {
            let part = json!({ "type": "summary_text", "text": text });
            target
                .send_part(wire, "response.reasoning_summary_part.done", index, part)
                .await?;
        }
        Ok(())
    }

    async fn ensure_open(
        &mut self,
        wire: &mut Wire,
        target: SummaryTarget<'_>,
        index: u64,
        added_part: Option<&Value>,
    ) -> AppResult<()> {
        if self.open == Some(index) {
            return Ok(());
        }
        if let Some(open) = self.open {
            self.close_part(wire, target, open, None).await?;
        }
        if !self.parts.contains_key(&index) {
            self.parts.insert(index, SummaryPart::default());
            let part = added_part
                .cloned()
                .unwrap_or_else(|| json!({ "type": "summary_text", "text": "" }));
            target
                .send_part(wire, "response.reasoning_summary_part.added", index, part)
                .await?;
        }
        self.open = Some(index);
        Ok(())
    }

    /// STR3i.1: applies one native part event.
    async fn apply_event(
        &mut self,
        wire: &mut Wire,
        target: SummaryTarget<'_>,
        summary_index: Option<u64>,
        event: &ReasoningSummaryEvent,
    ) -> AppResult<()> {
        let index = summary_index.or(self.open).unwrap_or(0);
        match event {
            ReasoningSummaryEvent::PartAdded { part } => {
                self.ensure_open(wire, target, index, Some(part)).await
            }
            ReasoningSummaryEvent::TextDone { text } => {
                self.ensure_open(wire, target, index, None).await?;
                let state = self.parts.entry(index).or_default();
                state.text.clone_from(text);
                if std::mem::replace(&mut state.text_done, true) {
                    return Ok(());
                }
                target.send_text_done(wire, index, text).await
            }
            ReasoningSummaryEvent::PartDone { part } => {
                self.ensure_open(wire, target, index, None).await?;
                let state = self.parts.entry(index).or_default();
                if let Some(text) = part.get("text").and_then(Value::as_str) {
                    state.text = text.to_string();
                }
                let needs_text_done = !std::mem::replace(&mut state.text_done, true);
                state.part_done = true;
                let text = state.text.clone();
                self.open = None;
                if needs_text_done {
                    target.send_text_done(wire, index, &text).await?;
                }
                target
                    .send_part(
                        wire,
                        "response.reasoning_summary_part.done",
                        index,
                        part.clone(),
                    )
                    .await
            }
        }
    }

    async fn delta(
        &mut self,
        wire: &mut Wire,
        target: SummaryTarget<'_>,
        summary_index: Option<u64>,
        delta: &str,
        event_extra: &HashMap<String, Value>,
    ) -> AppResult<()> {
        let index = summary_index.or(self.open).unwrap_or(0);
        self.ensure_open(wire, target, index, None).await?;
        self.parts.entry(index).or_default().text.push_str(delta);
        let payload = with_delta_wire_fields(
            insert_reasoning_source(
                target.payload(json!({ "summary_index": index })),
                target.source,
            ),
            event_extra,
        );
        wire.delta("response.reasoning_summary_text.delta", payload, delta)
            .await
    }

    /// Emits the delta a part never streamed, then closes the part.
    async fn complete_part(
        &mut self,
        wire: &mut Wire,
        target: SummaryTarget<'_>,
        index: u64,
        final_text: Option<&str>,
    ) -> AppResult<()> {
        let needs_delta = final_text.is_some_and(|text| !text.is_empty())
            && self
                .parts
                .get(&index)
                .is_none_or(|state| state.text.is_empty() && !state.text_done);
        if needs_delta && let Some(text) = final_text {
            self.delta(wire, target, Some(index), text, &HashMap::new())
                .await?;
        }
        self.close_part(wire, target, index, final_text).await
    }

    /// STR3i.2: completes every part. `final_parts` are the terminal `summary[]` texts by
    /// position.
    async fn finish(
        &mut self,
        wire: &mut Wire,
        target: SummaryTarget<'_>,
        final_parts: &[String],
    ) -> AppResult<()> {
        if std::mem::replace(&mut self.finished, true) {
            return Ok(());
        }
        // A transform may rewrite the summary into fewer parts than were already streamed.
        let parts_authoritative = final_parts.len() >= self.parts.len();
        if let Some(open) = self.open {
            let final_text = parts_authoritative
                .then(|| final_parts.get(open as usize))
                .flatten()
                .map(String::as_str);
            self.complete_part(wire, target, open, final_text).await?;
        }
        if !parts_authoritative {
            return Ok(());
        }
        for (position, text) in final_parts.iter().enumerate() {
            let index = position as u64;
            let known = self.parts.get(&index);
            if known.is_some_and(|state| state.part_done) || (known.is_none() && text.is_empty()) {
                continue;
            }
            self.complete_part(wire, target, index, Some(text)).await?;
        }
        Ok(())
    }

    /// The streamed summary text, parts concatenated in index order.
    pub(super) fn text(&self) -> String {
        self.parts.values().map(|part| part.text.as_str()).collect()
    }
}

/// The payload members of one `NodeDelta::Reasoning`.
pub(super) struct ReasoningDelta<'a> {
    pub(super) metadata: &'a ReasoningMetadata,
    pub(super) content: Option<&'a str>,
    pub(super) summary: Option<&'a str>,
    pub(super) encrypted: Option<Value>,
    pub(super) source: Option<&'a str>,
}

pub(super) async fn delta(
    wire: &mut Wire,
    item_id: &str,
    output_index: usize,
    streamed: &mut Streamed,
    delta: ReasoningDelta<'_>,
    event_extra: &HashMap<String, Value>,
) -> AppResult<()> {
    let target = SummaryTarget {
        item_id,
        output_index,
        source: delta.source,
    };
    if let Some(source) = delta.source.filter(|source| !source.is_empty()) {
        streamed.source = Some(source.to_string());
    }
    if let Some(encrypted) = delta.encrypted.filter(|encrypted| !encrypted.is_null()) {
        streamed.encrypted = Some(encrypted);
    }
    if let Some(event) = &delta.metadata.summary_event {
        streamed
            .summary
            .apply_event(wire, target, delta.metadata.summary_index, event)
            .await?;
    }
    if let Some(summary) = delta.summary.filter(|summary| !summary.is_empty()) {
        streamed
            .summary
            .delta(
                wire,
                target,
                delta.metadata.summary_index,
                summary,
                event_extra,
            )
            .await?;
    }
    let Some(content) = delta.content.filter(|content| !content.is_empty()) else {
        return Ok(());
    };
    ensure_text_part(wire, item_id, output_index, streamed).await?;
    streamed.text.push_str(content);
    let payload = with_delta_wire_fields(
        insert_reasoning_source(
            json!({ "item_id": item_id, "output_index": output_index, "content_index": 0 }),
            delta.source,
        ),
        event_extra,
    );
    wire.delta("response.reasoning_text.delta", payload, content)
        .await
}

/// Completes the summary parts and the `reasoning_text` part of `node` (STR-15).
pub(super) async fn finish(
    wire: &mut Wire,
    item_id: &str,
    output_index: usize,
    streamed: &mut Streamed,
    node: &Node,
) -> AppResult<()> {
    let Node::Reasoning {
        content, source, ..
    } = node
    else {
        return Ok(());
    };
    let source = source.as_deref();
    let target = SummaryTarget {
        item_id,
        output_index,
        source,
    };
    streamed
        .summary
        .finish(wire, target, &summary_parts(node))
        .await?;
    let Some(content) = content.as_deref().filter(|content| !content.is_empty()) else {
        return Ok(());
    };
    ensure_text_part(wire, item_id, output_index, streamed).await?;
    let coordinates =
        json!({ "item_id": item_id, "output_index": output_index, "content_index": 0 });
    if let Some(suffix) = content
        .strip_prefix(streamed.text.as_str())
        .filter(|suffix| !suffix.is_empty())
    {
        let payload = insert_reasoning_source(coordinates.clone(), source);
        wire.delta("response.reasoning_text.delta", payload, suffix)
            .await?;
    }
    let mut done = coordinates.clone();
    done["text"] = json!(content);
    wire.event(
        "response.reasoning_text.done",
        insert_reasoning_source(done, source),
    )
    .await?;
    let mut part_done = coordinates;
    part_done["part"] = json!({ "type": "reasoning_text", "text": content });
    wire.event("response.content_part.done", part_done).await
}

async fn ensure_text_part(
    wire: &mut Wire,
    item_id: &str,
    output_index: usize,
    streamed: &mut Streamed,
) -> AppResult<()> {
    if std::mem::replace(&mut streamed.reasoning_part_added, true) {
        return Ok(());
    }
    let payload = json!({
        "item_id": item_id,
        "output_index": output_index,
        "content_index": 0,
        "part": { "type": "reasoning_text", "text": "" },
    });
    wire.event("response.content_part.added", payload).await
}

/// The terminal `summary[]` texts of a reasoning node by position.
fn summary_parts(node: &Node) -> Vec<String> {
    let Node::Reasoning {
        metadata,
        summary: Some(summary),
        ..
    } = node
    else {
        return Vec::new();
    };
    crate::urp::reasoning::encode_text_parts(
        Some(summary.as_str()),
        metadata.summary_parts.as_deref(),
        "summary_text",
    )
    .as_array()
    .map(|parts| {
        parts
            .iter()
            .map(|part| {
                part.get("text")
                    .and_then(Value::as_str)
                    .unwrap_or_default()
                    .to_string()
            })
            .collect()
    })
    .unwrap_or_default()
}

pub(super) fn insert_reasoning_source(mut payload: Value, source: Option<&str>) -> Value {
    if let Some(source) = source.filter(|source| !source.is_empty())
        && let Some(obj) = payload.as_object_mut()
    {
        obj.insert("source".to_string(), Value::String(source.to_string()));
    }
    payload
}

fn non_empty_value(value: &Value) -> bool {
    match value {
        Value::Null => false,
        Value::String(value) => !value.is_empty(),
        Value::Array(value) => !value.is_empty(),
        Value::Object(value) => !value.is_empty(),
        Value::Bool(_) | Value::Number(_) => true,
    }
}

fn part_has_text(part: &Value) -> bool {
    part.get("text")
        .and_then(Value::as_str)
        .is_some_and(|text| !text.is_empty())
}

fn content_part_has_text(part: &Value) -> bool {
    part.get("text")
        .and_then(Value::as_str)
        .or_else(|| part.get("summary").and_then(Value::as_str))
        .is_some_and(|text| !text.is_empty())
}

fn item_has_text(item: &Value) -> bool {
    item.get("summary")
        .and_then(Value::as_array)
        .is_some_and(|parts| parts.iter().any(part_has_text))
        || item
            .get("content")
            .and_then(Value::as_array)
            .is_some_and(|parts| parts.iter().any(content_part_has_text))
        || item
            .get("text")
            .and_then(Value::as_str)
            .is_some_and(|text| !text.is_empty())
}

/// PR2g: a reasoning stub with a payload is visible at once.
pub(super) fn item_has_payload(item: &Value) -> bool {
    item_has_text(item) || item.get("encrypted_content").is_some_and(non_empty_value)
}

pub(super) fn delta_has_payload(delta: &NodeDelta) -> bool {
    matches!(
        delta,
        NodeDelta::Reasoning {
            metadata,
            content,
            encrypted,
            summary,
            ..
        } if metadata.summary_event.is_some()
            || content.as_ref().is_some_and(|value| !value.is_empty())
            || summary.as_ref().is_some_and(|value| !value.is_empty())
            || encrypted.as_ref().is_some_and(non_empty_value)
    )
}

/// PR2g, PR2g.1: whether a reasoning node owns a downstream item.
pub(super) fn node_has_payload(node: &Node) -> bool {
    matches!(
        node,
        Node::Reasoning {
            metadata,
            content,
            encrypted,
            summary,
            ..
        } if metadata.native_empty
            || content.as_ref().is_some_and(|value| !value.is_empty())
            || summary.as_ref().is_some_and(|value| !value.is_empty())
            || encrypted.as_ref().is_some_and(non_empty_value)
    )
}

pub(super) fn item_with_duration(mut item: Value, duration_secs: Option<u64>) -> Value {
    if item.get("type").and_then(Value::as_str) != Some("reasoning") {
        return item;
    }
    if let Some(duration_secs) = duration_secs
        && let Some(obj) = item.as_object_mut()
    {
        obj.entry("duration".to_string())
            .or_insert_with(|| json!(duration_secs));
    }
    item
}

/// STR3o: an added reasoning item that already states completed reasoning carries a duration.
pub(super) fn added_item_with_duration(item: Value, duration_secs: u64) -> Value {
    if item.get("type").and_then(Value::as_str) != Some("reasoning") {
        return item;
    }
    let completed = item.get("status").and_then(Value::as_str) == Some("completed");
    if completed || item_has_text(&item) {
        item_with_duration(item, Some(duration_secs))
    } else {
        item
    }
}

pub(super) fn response_with_reasoning_durations(mut response: Value, duration_secs: u64) -> Value {
    if let Some(output) = response.get_mut("output").and_then(Value::as_array_mut) {
        for item in output {
            if item.get("type").and_then(Value::as_str) == Some("reasoning")
                && let Some(obj) = item.as_object_mut()
            {
                obj.entry("duration".to_string())
                    .or_insert_with(|| json!(duration_secs));
            }
        }
    }
    response
}
