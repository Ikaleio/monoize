//! Numbered Responses SSE emission (STR1, STR2, STR3a).

use crate::error::{AppError, AppResult};
use crate::payload::sse::WireEvent as Event;
use crate::urp::TokenLogprob;
use crate::urp::stream_helpers::{
    send_named_sse_json, send_responses_delta_string, send_responses_event,
};
use axum::http::StatusCode;
use serde_json::{Value, json};
use std::collections::HashMap;
use tokio::sync::mpsc;

pub(super) struct Wire {
    tx: mpsc::Sender<Event>,
    seq: u64,
    max_frame_length: Option<usize>,
}

impl Wire {
    pub(super) fn new(tx: mpsc::Sender<Event>, max_frame_length: Option<usize>) -> Self {
        Self {
            tx,
            seq: 0,
            max_frame_length,
        }
    }

    pub(super) fn max_frame_length(&self) -> Option<usize> {
        self.max_frame_length
    }

    pub(super) async fn event(&mut self, name: &str, data: Value) -> AppResult<()> {
        send_responses_event(&self.tx, &mut self.seq, name, data).await
    }

    /// Sends `content` as the `delta` member of `template`, split across frames under the
    /// frame limit.
    pub(super) async fn delta(
        &mut self,
        name: &str,
        template: Value,
        content: &str,
    ) -> AppResult<()> {
        send_responses_delta_string(
            &self.tx,
            &mut self.seq,
            name,
            template,
            "delta",
            content,
            self.max_frame_length,
        )
        .await
    }

    /// `response.output_text.delta` with token scores; a split frame never repeats a token.
    pub(super) async fn text_delta(
        &mut self,
        mut payload: Value,
        text: &str,
        scores: Option<&[TokenLogprob]>,
    ) -> AppResult<()> {
        const NAME: &str = "response.output_text.delta";
        let Some(scores) = scores else {
            payload["logprobs"] = json!([]);
            return self.delta(NAME, payload, text).await;
        };
        payload["delta"] = json!(text);
        payload["logprobs"] = crate::urp::logprobs::encode_openai(scores);
        if self
            .max_frame_length
            .is_some_and(|limit| payload.to_string().len() + 80 > limit)
        {
            for (text, scores) in crate::urp::logprobs::fragments(scores) {
                let mut part = payload.clone();
                part["delta"] = json!(text);
                part["logprobs"] = crate::urp::logprobs::encode_openai(&scores);
                self.event(NAME, part).await?;
            }
            Ok(())
        } else {
            self.event(NAME, payload).await
        }
    }

    /// STR3q: a provider-control event keeps exactly its upstream fields; only an existing
    /// `sequence_number` is renumbered.
    pub(super) async fn passthrough(&mut self, name: &str, mut data: Value) -> AppResult<()> {
        if let Some(obj) = data.as_object_mut() {
            obj.entry("type".to_string()).or_insert_with(|| json!(name));
            if let Some(number) = obj.get_mut("sequence_number") {
                *number = json!(self.seq);
                self.seq += 1;
            }
        }
        send_named_sse_json(&self.tx, name, data).await
    }

    /// Ends the stream with the native `unsupported_media` error. Always returns `Err`,
    /// marked as terminal-sent when the frame reached an open receiver (FP6j).
    pub(super) async fn media_error(&mut self, message: &str) -> AppResult<()> {
        self.event(
            "error",
            json!({ "code": "unsupported_media", "message": message, "param": null }),
        )
        .await?;
        Err(
            AppError::new(StatusCode::BAD_GATEWAY, "unsupported_media", message)
                .with_downstream_stream_terminal_sent(!self.tx.is_closed()),
        )
    }
}

/// STR3g: copies an upstream `obfuscation` member onto a delta payload.
pub(super) fn with_delta_wire_fields(
    mut payload: Value,
    event_extra: &HashMap<String, Value>,
) -> Value {
    if let Some(obfuscation) = event_extra.get("obfuscation") {
        payload["obfuscation"] = obfuscation.clone();
    }
    payload
}
