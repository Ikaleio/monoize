//! Named Messages SSE emission: every frame's SSE event name is its payload `type` (DM7).

use crate::error::{AppError, AppResult};
use crate::urp::encode::anthropic::messages_media_error_body;
use crate::urp::encode::sanitize_provider_item_wire_body;
use crate::urp::stream_helpers::{
    messages_delta_path_partial_json, messages_delta_path_signature, messages_delta_path_text,
    messages_delta_path_thinking, send_messages_delta_string, send_named_sse_json,
};
use crate::urp::{Citation, ProviderProtocol};
use axum::http::StatusCode;
use axum::response::sse::Event;
use serde_json::{Value, json};
use tokio::sync::mpsc;

pub(super) struct Wire {
    tx: mpsc::Sender<Event>,
    max_frame_length: Option<usize>,
}

impl Wire {
    pub(super) fn new(tx: mpsc::Sender<Event>, max_frame_length: Option<usize>) -> Self {
        Self {
            tx,
            max_frame_length,
        }
    }

    pub(super) async fn event(&self, payload: Value) -> AppResult<()> {
        let name = payload
            .get("type")
            .and_then(Value::as_str)
            .map(str::to_string)
            .ok_or_else(|| {
                AppError::new(
                    StatusCode::BAD_GATEWAY,
                    "stream_encode_failed",
                    "messages stream payload missing type field",
                )
            })?;
        send_named_sse_json(&self.tx, &name, payload).await
    }

    pub(super) async fn block_start(&self, index: u32, block: Value) -> AppResult<()> {
        self.event(json!({ "type": "content_block_start", "index": index, "content_block": block }))
            .await
    }

    pub(super) async fn block_stop(&self, index: u32) -> AppResult<()> {
        self.event(json!({ "type": "content_block_stop", "index": index }))
            .await
    }

    /// One `content_block_delta` frame, never split.
    pub(super) async fn delta(&self, index: u32, delta: Value) -> AppResult<()> {
        self.event(json!({ "type": "content_block_delta", "index": index, "delta": delta }))
            .await
    }

    /// A `content_block_delta` whose string member, written by `patch`, is split across frames
    /// under the frame limit.
    async fn split_delta(
        &self,
        index: u32,
        delta: Value,
        patch: fn(&mut Value, &str),
        content: &str,
    ) -> AppResult<()> {
        send_messages_delta_string(
            &self.tx,
            json!({ "type": "content_block_delta", "index": index, "delta": delta }),
            patch,
            content,
            self.max_frame_length,
        )
        .await
    }

    /// Empty text sends nothing.
    pub(super) async fn text(&self, index: u32, text: &str) -> AppResult<()> {
        if text.is_empty() {
            return Ok(());
        }
        let delta = json!({ "type": "text_delta", "text": "" });
        self.split_delta(index, delta, messages_delta_path_text, text)
            .await
    }

    /// `delta` is a `thinking_delta` template; it may carry `estimated_tokens` (PM6e).
    pub(super) async fn thinking(&self, index: u32, delta: Value, thinking: &str) -> AppResult<()> {
        self.split_delta(index, delta, messages_delta_path_thinking, thinking)
            .await
    }

    pub(super) async fn signature(&self, index: u32, signature: &str) -> AppResult<()> {
        let delta = json!({ "type": "signature_delta", "signature": "" });
        self.split_delta(index, delta, messages_delta_path_signature, signature)
            .await
    }

    /// Empty input JSON sends nothing.
    pub(super) async fn partial_json(&self, index: u32, partial_json: &str) -> AppResult<()> {
        if partial_json.is_empty() {
            return Ok(());
        }
        let delta = json!({ "type": "input_json_delta", "partial_json": "" });
        self.split_delta(index, delta, messages_delta_path_partial_json, partial_json)
            .await
    }

    pub(super) async fn citation(&self, index: u32, citation: &Citation) -> AppResult<()> {
        match citation.encode(ProviderProtocol::Messages, 0) {
            Some(citation) => {
                self.delta(
                    index,
                    json!({ "type": "citations_delta", "citation": citation }),
                )
                .await
            }
            None => Ok(()),
        }
    }

    /// One native delta of a Messages provider item. The fragment of an `input_json_delta`
    /// obeys the frame limit; every other delta is one frame.
    pub(super) async fn provider_delta(&self, index: u32, delta: &Value) -> AppResult<()> {
        let mut delta = sanitize_provider_item_wire_body(delta);
        let fragment = delta
            .as_object_mut()
            .filter(|object| object.get("type").and_then(Value::as_str) == Some("input_json_delta"))
            .and_then(|object| match object.get_mut("partial_json") {
                Some(Value::String(fragment)) => Some(std::mem::take(fragment)),
                _ => None,
            });
        match fragment {
            Some(fragment) => {
                self.split_delta(index, delta, messages_delta_path_partial_json, &fragment)
                    .await
            }
            None => self.delta(index, delta).await,
        }
    }

    /// Ends the stream with the native error for output Messages cannot represent. Always
    /// returns `Err`, marked as terminal-sent when the frame reached an open receiver (FP6j).
    pub(super) async fn media_error(&self, message: String) -> AppResult<()> {
        self.event(messages_media_error_body(&message)).await?;
        Err(
            AppError::new(StatusCode::BAD_GATEWAY, "unsupported_output_media", message)
                .with_downstream_stream_terminal_sent(!self.tx.is_closed()),
        )
    }
}
