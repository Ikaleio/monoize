//! Reasoning under one surface policy for every mode (DC5, CHAT-3, CHAT-4, STR-15).

use super::ChatStreamEncoder;
use super::wire::{native_delta_extra, object};
use crate::error::AppResult;
use crate::urp::reasoning::chat_node_details;
use crate::urp::{
    CHAT_REASONING_DETAIL_EXTRA_KEY, CHAT_REASONING_SURFACE_EXTRA_KEY,
    CHAT_REASONING_SURFACE_REASONING_CONTENT, CHAT_REASONING_SURFACE_REASONING_OPAQUE, Node,
    ReasoningMetadata,
};
use serde_json::{Map, Value, json};
use std::borrow::Cow;
use std::collections::HashMap;

/// The emitted part of one reasoning node.
#[derive(Default)]
pub(super) struct ReasoningProgress {
    content: String,
    summary: String,
    encrypted: bool,
    /// A chunk of this node reached the wire.
    surfaced: bool,
    /// DC5: the synthesized `index` of each detail type of this node.
    detail_indexes: Vec<(String, u64)>,
}

impl ReasoningProgress {
    fn record(&mut self, fields: &ReasoningFields<'_>) {
        if let Some(content) = fields.content {
            self.content.push_str(content);
        }
        if let Some(summary) = fields.summary {
            self.summary.push_str(summary);
        }
        self.encrypted |= fields.encrypted.is_some_and(is_present);
    }
}

/// The reasoning payload of one delta or one node.
#[derive(Clone, Copy)]
pub(super) struct ReasoningFields<'a> {
    pub(super) metadata: &'a ReasoningMetadata,
    pub(super) item_id: Option<&'a str>,
    pub(super) content: Option<&'a str>,
    pub(super) summary: Option<&'a str>,
    pub(super) encrypted: Option<&'a Value>,
    pub(super) source: Option<&'a str>,
}

fn is_present(value: &Value) -> bool {
    !value.is_null() && value.as_str() != Some("")
}

fn unsent_suffix<'a>(full: Option<&'a str>, sent: &str) -> Option<&'a str> {
    full?.strip_prefix(sent).filter(|suffix| !suffix.is_empty())
}

fn patch_reasoning_content(chunk: &mut Value, content: &str) {
    chunk["choices"][0]["delta"]["reasoning_content"] = json!(content);
}

fn patch_reasoning_opaque(chunk: &mut Value, content: &str) {
    chunk["choices"][0]["delta"]["reasoning_opaque"] = json!(content);
}

fn patch_detail_text(chunk: &mut Value, content: &str) {
    let delta = &mut chunk["choices"][0]["delta"];
    delta["reasoning"] = json!(content);
    delta["reasoning_details"][0]["text"] = json!(content);
}

fn patch_detail_summary(chunk: &mut Value, content: &str) {
    let delta = &mut chunk["choices"][0]["delta"];
    delta["reasoning"] = json!(content);
    delta["reasoning_details"][0]["summary"] = json!(content);
}

fn patch_detail_data(chunk: &mut Value, content: &str) {
    chunk["choices"][0]["delta"]["reasoning_details"][0]["data"] = json!(content);
}

fn scalar_patch(field: &str) -> fn(&mut Value, &str) {
    if field == "reasoning_content" {
        patch_reasoning_content
    } else {
        patch_reasoning_opaque
    }
}

/// The legacy `reasoning_opaque` text of an encrypted payload; a non-string payload is its JSON.
fn opaque_text(value: &Value) -> Option<Cow<'_, str>> {
    match value {
        Value::Null => None,
        Value::String(value) => Some(Cow::Borrowed(value)),
        value => Some(Cow::Owned(value.to_string())),
    }
}

/// A delta carrying one `reasoning_details` entry.
fn details_delta(detail: Map<String, Value>) -> Value {
    object(
        "reasoning_details",
        Value::Array(vec![Value::Object(detail)]),
    )
}

impl ChatStreamEncoder {
    pub(super) async fn on_reasoning_delta(
        &mut self,
        progress: &mut ReasoningProgress,
        fields: ReasoningFields<'_>,
        event_extra: &HashMap<String, Value>,
    ) -> AppResult<()> {
        let mut native_extra = native_delta_extra(event_extra);
        let sent = self
            .emit_reasoning(progress, fields, event_extra, &mut native_extra)
            .await?;
        progress.record(&fields);
        progress.surfaced |= sent;
        // DC6: the native members of a fragment reach the wire even without a reasoning surface.
        if !native_extra.is_empty() || self.wire.has_pending_delta_extra() {
            let delta = self.wire.take_delta_extra(json!({}), &mut native_extra);
            self.wire.delta(delta).await?;
        }
        Ok(())
    }

    /// STR-15 for a reasoning node: the text and summary suffixes and the encrypted payload that
    /// no delta carried. A node with no emitted chunk goes out whole, as a replay presents it.
    pub(super) async fn complete_reasoning(
        &mut self,
        progress: &mut ReasoningProgress,
        node: &Node,
    ) -> AppResult<()> {
        let Node::Reasoning {
            id,
            metadata,
            content,
            encrypted,
            summary,
            source,
            extra_body,
            ..
        } = node
        else {
            return Ok(());
        };
        let mut fields = ReasoningFields {
            metadata,
            item_id: id.as_deref(),
            content: content.as_deref(),
            summary: summary.as_deref(),
            encrypted: encrypted.as_ref(),
            source: source.as_deref(),
        };
        if progress.surfaced {
            // An opaque payload has no suffix (SACC-5c): only a never-sent one goes out.
            fields.content = unsent_suffix(fields.content, &progress.content);
            fields.summary = unsent_suffix(fields.summary, &progress.summary);
            fields.encrypted = fields
                .encrypted
                .filter(|value| !progress.encrypted && is_present(value));
            if fields.content.is_none() && fields.summary.is_none() && fields.encrypted.is_none() {
                return Ok(());
            }
        }
        let sent = self
            .emit_reasoning(progress, fields, extra_body, &mut Map::new())
            .await?;
        progress.record(&fields);
        progress.surfaced |= sent;
        Ok(())
    }

    /// DC5 for one payload: the scalar surface, the `chat_content` alias, then the detail entries
    /// in summary, text, encrypted order. `marker_extra` holds the surface marker and the native
    /// detail metadata; `event_extra` holds native delta members, which ride on the first chunk.
    /// Returns whether a chunk was sent.
    async fn emit_reasoning(
        &mut self,
        progress: &mut ReasoningProgress,
        fields: ReasoningFields<'_>,
        marker_extra: &HashMap<String, Value>,
        event_extra: &mut Map<String, Value>,
    ) -> AppResult<bool> {
        let ReasoningFields {
            metadata,
            item_id,
            mut content,
            summary,
            mut encrypted,
            source,
        } = fields;
        let mut sent = false;
        let scalar = match marker_extra
            .get(CHAT_REASONING_SURFACE_EXTRA_KEY)
            .and_then(Value::as_str)
        {
            Some(CHAT_REASONING_SURFACE_REASONING_CONTENT) => content
                .take()
                .map(|content| ("reasoning_content", Cow::Borrowed(content))),
            Some(CHAT_REASONING_SURFACE_REASONING_OPAQUE) => encrypted
                .take()
                .and_then(opaque_text)
                .map(|opaque| ("reasoning_opaque", opaque)),
            _ => None,
        };
        if let Some((field, value)) = scalar
            && !value.is_empty()
        {
            let delta = self
                .wire
                .take_delta_extra(object(field, json!("")), event_extra);
            self.wire
                .delta_string(delta, &value, scalar_patch(field))
                .await?;
            sent = true;
        }
        let plain = content
            .filter(|content| !content.is_empty())
            .or(summary.filter(|summary| !summary.is_empty()));
        if let Some(plain) = plain.filter(|_| metadata.chat_content) {
            let delta = self
                .wire
                .take_delta_extra(json!({ "reasoning_content": "" }), event_extra);
            self.wire
                .delta_string(delta, plain, patch_reasoning_content)
                .await?;
            sent = true;
        }
        let native = marker_extra.contains_key(CHAT_REASONING_DETAIL_EXTRA_KEY);
        for mut detail in chat_node_details(
            metadata,
            item_id,
            content,
            summary,
            encrypted,
            source,
            marker_extra,
        ) {
            let kind = detail
                .get("type")
                .and_then(Value::as_str)
                .unwrap_or_default()
                .to_string();
            match detail.get("index").and_then(Value::as_u64) {
                Some(index) => self.next_detail_index = self.next_detail_index.max(index + 1),
                None if !native => {
                    let index = self.detail_index(progress, &kind);
                    detail.insert("index".into(), json!(index));
                }
                None => {}
            }
            type DetailPatch = fn(&mut Value, &str);
            let (field, patch): (&str, Option<DetailPatch>) = match kind.as_str() {
                "reasoning.text" => ("text", Some(patch_detail_text)),
                "reasoning.summary" => ("summary", Some(patch_detail_summary)),
                "reasoning.encrypted" => ("data", Some(patch_detail_data)),
                _ => ("", None),
            };
            let alias = matches!(kind.as_str(), "reasoning.text" | "reasoning.summary");
            // An Anthropic signature is opaque and must not be split across chunks.
            let split = patch.filter(|_| {
                !detail.contains_key("signature") && detail.get(field).is_some_and(Value::is_string)
            });
            if let Some(patch) = split {
                let payload = match detail.insert(field.to_string(), json!("")) {
                    Some(Value::String(payload)) => payload,
                    _ => String::new(),
                };
                let mut delta = details_delta(detail);
                if alias {
                    delta["reasoning"] = json!("");
                }
                let delta = self.wire.take_delta_extra(delta, event_extra);
                self.wire.delta_string(delta, &payload, patch).await?;
            } else {
                let reasoning = detail
                    .get(field)
                    .and_then(Value::as_str)
                    .filter(|payload| alias && !payload.is_empty())
                    .map(str::to_string);
                let mut delta = details_delta(detail);
                if let Some(reasoning) = reasoning {
                    delta["reasoning"] = Value::String(reasoning);
                }
                let delta = self.wire.take_delta_extra(delta, event_extra);
                self.wire.delta(delta).await?;
            }
            sent = true;
        }
        Ok(sent)
    }

    /// DC5: every fragment of one synthesized detail repeats one `index`, above every index the
    /// stream already emitted.
    fn detail_index(&mut self, progress: &mut ReasoningProgress, kind: &str) -> u64 {
        if let Some((_, index)) = progress
            .detail_indexes
            .iter()
            .find(|(seen, _)| seen == kind)
        {
            return *index;
        }
        let index = self.next_detail_index;
        self.next_detail_index += 1;
        progress.detail_indexes.push((kind.to_string(), index));
        index
    }
}
