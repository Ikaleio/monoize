//! Scalar Chat reasoning (`reasoning`, `reasoning_content`, `reasoning_opaque`, and reasoning
//! content blocks) and the PC7h completion of reasoning nodes.

use super::details::detail_awaits_signature;
use super::state::ChatStreamState;
use crate::error::AppResult;
use crate::urp::stream_helpers::extract_chat_reasoning_delta_chunks;
use crate::urp::{
    CHAT_REASONING_SURFACE_EXTRA_KEY, CHAT_REASONING_SURFACE_REASONING,
    CHAT_REASONING_SURFACE_REASONING_CONTENT, Node, NodeDelta, NodeHeader, synthetic_reasoning_id,
};
use serde_json::{Map, Value};
use std::collections::HashMap;

/// Scalar reasoning of the message. The accumulators stay cumulative across PC7h segments so a
/// terminal snapshot can extend them by suffix; the offsets mark where the open segment starts.
#[derive(Default)]
pub(super) struct ScalarReasoning {
    pub(super) text: String,
    pub(super) summary: String,
    pub(super) encrypted: String,
    text_start: usize,
    summary_start: usize,
    encrypted_start: usize,
    pub(super) open: Option<u32>,
    /// Latest provider `format` of the open segment.
    source: Option<String>,
    /// Highest-ranked scalar field that fed the open segment.
    surface: Option<&'static str>,
}

impl ScalarReasoning {
    /// Records `format` as the segment source and returns the segment source. OpenRouter's own
    /// `openrouter` format names no upstream source.
    fn resolve_source(&mut self, format: Option<&str>) -> Option<String> {
        if let Some(format) = format.filter(|format| !format.is_empty() && *format != "openrouter")
        {
            self.source = Some(format.to_string());
        }
        self.source.clone()
    }

    /// Records the fragment's scalar surface on the segment and returns the event extras for it.
    fn surface_extra(&mut self, surface: Option<&'static str>) -> HashMap<String, Value> {
        let Some(surface) = surface else {
            return HashMap::new();
        };
        if self
            .surface
            .is_none_or(|current| surface_rank(surface) > surface_rank(current))
        {
            self.surface = Some(surface);
        }
        HashMap::from([(
            CHAT_REASONING_SURFACE_EXTRA_KEY.to_string(),
            Value::String(surface.to_string()),
        )])
    }

    /// A terminal `reasoning_opaque` replaces the opaque payload atomically. Returns the suffix
    /// to stream when the snapshot extends the streamed payload.
    pub(super) fn apply_encrypted_snapshot(&mut self, snapshot: &str) -> Option<String> {
        if let Some(suffix) = snapshot_suffix(&self.encrypted, snapshot) {
            return Some(suffix.to_string());
        }
        if self.encrypted != snapshot {
            self.encrypted.clear();
            self.encrypted.push_str(snapshot);
        }
        None
    }

    /// The open segment as a node; the next segment starts after it.
    pub(super) fn take_segment(&mut self) -> Node {
        let segment = |all: &str, start: usize| {
            Some(all.get(start..).unwrap_or_default())
                .filter(|segment| !segment.is_empty())
                .map(str::to_string)
        };
        let node = Node::Reasoning {
            metadata: Default::default(),
            id: Some(synthetic_reasoning_id()),
            content: segment(&self.text, self.text_start),
            encrypted: segment(&self.encrypted, self.encrypted_start).map(Value::String),
            summary: segment(&self.summary, self.summary_start),
            source: self.source.take(),
            extra_body: self
                .surface
                .take()
                .map(|surface| {
                    HashMap::from([(
                        CHAT_REASONING_SURFACE_EXTRA_KEY.to_string(),
                        Value::String(surface.to_string()),
                    )])
                })
                .unwrap_or_default(),
        };
        self.text_start = self.text.len();
        self.summary_start = self.summary.len();
        self.encrypted_start = self.encrypted.len();
        node
    }
}

impl ChatStreamState<'_> {
    pub(super) async fn on_reasoning_delta(&mut self, delta: &Value) -> AppResult<()> {
        let details = delta
            .get("reasoning_details")
            .and_then(Value::as_array)
            .filter(|details| !details.is_empty());
        let (texts, encrypted) = match details {
            Some(details) => {
                self.on_reasoning_details(details).await?;
                extract_chat_reasoning_delta_chunks(&scalar_reasoning_beside_details(
                    delta, details,
                ))
            }
            None => extract_chat_reasoning_delta_chunks(delta),
        };
        if !texts.is_empty() || !encrypted.is_empty() {
            self.close_text().await?;
        }
        for chunk in texts {
            self.push_reasoning_text(&chunk.text, chunk.format.as_deref(), chunk.surface)
                .await?;
        }
        for chunk in encrypted {
            self.push_reasoning_encrypted(
                &Value::String(chunk.text),
                chunk.format.as_deref(),
                chunk.surface,
            )
            .await?;
        }
        Ok(())
    }

    async fn reasoning_node(&mut self, extra_body: HashMap<String, Value>) -> AppResult<u32> {
        if let Some(node_index) = self.reasoning.open {
            return Ok(node_index);
        }
        let header = NodeHeader::Reasoning {
            metadata: Default::default(),
            id: None,
        };
        let node_index = self.start_node(header, extra_body).await?;
        self.reasoning.open = Some(node_index);
        Ok(node_index)
    }

    pub(super) async fn push_reasoning_text(
        &mut self,
        content: &str,
        format: Option<&str>,
        surface: Option<&'static str>,
    ) -> AppResult<()> {
        if content.is_empty() {
            return Ok(());
        }
        let source = self.reasoning.resolve_source(format);
        let surface_extra = self.reasoning.surface_extra(surface);
        let mut event_extra = self.take_delta_extra();
        event_extra.extend(surface_extra.clone());
        let node_index = self.reasoning_node(surface_extra).await?;
        self.reasoning.text.push_str(content);
        let delta = NodeDelta::Reasoning {
            metadata: Default::default(),
            content: Some(content.to_string()),
            encrypted: None,
            summary: None,
            source,
        };
        self.send_delta(node_index, delta, event_extra).await
    }

    pub(super) async fn push_reasoning_encrypted(
        &mut self,
        encrypted: &Value,
        format: Option<&str>,
        surface: Option<&'static str>,
    ) -> AppResult<()> {
        if encrypted.is_null() {
            return Ok(());
        }
        let payload = encrypted
            .as_str()
            .map_or_else(|| encrypted.to_string(), str::to_string);
        if payload.is_empty() {
            return Ok(());
        }
        let source = self.reasoning.resolve_source(format);
        let surface_extra = self.reasoning.surface_extra(surface);
        let mut event_extra = self.take_delta_extra();
        event_extra.extend(surface_extra.clone());
        let node_index = self.reasoning_node(surface_extra).await?;
        self.reasoning.encrypted.push_str(&payload);
        let delta = NodeDelta::Reasoning {
            metadata: Default::default(),
            content: None,
            encrypted: Some(encrypted.clone()),
            summary: None,
            source,
        };
        self.send_delta(node_index, delta, event_extra).await
    }

    pub(super) async fn push_reasoning_summary(
        &mut self,
        summary: &str,
        format: Option<&str>,
    ) -> AppResult<()> {
        if summary.is_empty() {
            return Ok(());
        }
        let source = self.reasoning.resolve_source(format);
        self.reasoning.summary.push_str(summary);
        let event_extra = self.take_delta_extra();
        let node_index = self.reasoning_node(HashMap::new()).await?;
        let delta = NodeDelta::Reasoning {
            metadata: Default::default(),
            content: None,
            encrypted: None,
            summary: Some(summary.to_string()),
            source,
        };
        self.send_delta(node_index, delta, event_extra).await
    }

    /// PC7h: completes the open scalar segment and the open detail nodes, in index order. A
    /// detail that awaits its signature stays open unless `force` (the terminal snapshot) is set.
    pub(super) async fn close_reasoning(&mut self, force: bool) -> AppResult<()> {
        let mut completed = Vec::new();
        if let Some(node_index) = self.reasoning.open.take() {
            let node = self.reasoning.take_segment();
            self.closed.push((node_index, node.clone()));
            completed.push((node_index, node));
        }
        for detail in &mut self.details {
            if !detail.done && (force || !detail_awaits_signature(&detail.node)) {
                detail.done = true;
                completed.push((detail.node_index, detail.node.clone()));
            }
        }
        completed.sort_by_key(|(node_index, _)| *node_index);
        for (node_index, node) in completed {
            self.send_done(node_index, node).await?;
        }
        Ok(())
    }
}

/// The suffix by which `snapshot` extends `streamed`, if any.
pub(super) fn snapshot_suffix<'a>(streamed: &str, snapshot: &'a str) -> Option<&'a str> {
    snapshot
        .strip_prefix(streamed)
        .filter(|suffix| !suffix.is_empty())
}

fn surface_rank(surface: &str) -> u8 {
    match surface {
        CHAT_REASONING_SURFACE_REASONING_CONTENT => 3,
        CHAT_REASONING_SURFACE_REASONING => 2,
        _ => 1,
    }
}

/// CHAT-4b: beside `reasoning_details`, scalar `reasoning` is a view of the chunk's details, and
/// a `reasoning_content` or `reasoning_opaque` equal to a detail payload repeats that detail.
fn scalar_reasoning_beside_details(delta: &Value, details: &[Value]) -> Value {
    let mut scalar = Map::new();
    for (key, detail_type, payload_key) in [
        ("reasoning_content", "reasoning.text", "text"),
        ("reasoning_opaque", "reasoning.encrypted", "data"),
    ] {
        let Some(value) = delta.get(key) else {
            continue;
        };
        let repeated = value.as_str().is_some_and(|value| {
            details.iter().any(|detail| {
                detail["type"].as_str() == Some(detail_type)
                    && detail[payload_key].as_str() == Some(value)
            })
        });
        if !repeated {
            scalar.insert(key.to_string(), value.clone());
        }
    }
    Value::Object(scalar)
}
