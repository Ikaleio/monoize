//! URP stream events to Gemini `streamGenerateContent` SSE data frames (gemini-codec.spec.md
//! GEM-8 through GEM-10). Text and reasoning stream in canonical order; other Parts emit when
//! complete. Gemini cannot amend or retract an emitted Part, so an unrepresentable change ends
//! the stream with one native error frame.

mod nodes;
mod terminal;
mod wire;

use super::{StreamEncodeOptions, StreamEncoder};
use crate::error::{AppError, AppResult, StreamErrorDetails};
use crate::payload::sse::WireEvent as Event;
use crate::urp::encode::gemini::{encode_response_checked, error_json, error_status};
use crate::urp::stream_helpers::{send_plain_sse_json, split_json_value_by_string_patch};
use crate::urp::tool_signature::SignatureProjection;
use crate::urp::{UrpResponse, UrpStreamEvent};
use axum::http::StatusCode;
use nodes::NodeSlot;
use serde_json::{Map, Value, json};
use std::collections::HashMap;
use tokio::sync::mpsc;
use wire::WireState;

pub(crate) struct GeminiStreamEncoder {
    tx: mpsc::Sender<Event>,
    options: StreamEncodeOptions,
    id: String,
    /// Canonical state of every announced node.
    nodes: HashMap<u32, NodeSlot>,
    /// The first node index whose Parts may not all be on the wire yet.
    next_index: u32,
    wire: WireState,
    /// A terminal frame or error frame ended the stream; later events are ignored.
    terminal: bool,
}

impl GeminiStreamEncoder {
    pub(crate) fn new(tx: mpsc::Sender<Event>, options: StreamEncodeOptions) -> Self {
        Self {
            tx,
            options,
            id: String::new(),
            nodes: HashMap::new(),
            next_index: 0,
            wire: WireState::default(),
            terminal: false,
        }
    }

    /// The data frames of one event.
    fn apply(&mut self, event: UrpStreamEvent) -> Result<Vec<Value>, String> {
        match event {
            UrpStreamEvent::ResponseStart { id, .. } => {
                self.id = id;
                Ok(Vec::new())
            }
            UrpStreamEvent::NodeStart {
                node_index,
                header,
                extra_body,
            } => self.on_node_start(node_index, header, extra_body),
            UrpStreamEvent::NodeDelta {
                node_index,
                delta,
                extra_body,
                ..
            } => self.on_node_delta(node_index, delta, extra_body),
            UrpStreamEvent::NodeDone {
                node_index, node, ..
            } => self.on_node_done(node_index, node),
            UrpStreamEvent::ResponseDone {
                outcome,
                finish_reason,
                usage,
                output,
                extra_body,
            } => {
                let frames =
                    self.on_response_done(outcome, finish_reason, usage, output, extra_body)?;
                self.terminal = true;
                Ok(frames)
            }
            UrpStreamEvent::Error {
                code,
                message,
                extra_body,
            } => {
                self.terminal = true;
                let status = error_status(
                    StreamErrorDetails::from_urp(code.as_deref(), &extra_body),
                    false,
                );
                Ok(vec![error_json(status, &message)])
            }
            UrpStreamEvent::ProviderControl { .. } => Ok(Vec::new()),
        }
    }

    /// One content frame of the selected candidate.
    fn frame(&self, content: Value) -> Value {
        let mut candidate = Map::new();
        candidate.insert("index".into(), json!(0));
        candidate.insert("content".into(), content);
        let mut frame = Map::new();
        frame.insert("responseId".into(), json!(self.id));
        frame.insert("modelVersion".into(), json!(self.options.logical_model));
        frame.insert(
            "candidates".into(),
            Value::Array(vec![Value::Object(candidate)]),
        );
        Value::Object(frame)
    }

    /// SSF-7: under a frame limit, the text of a plain text fragment splits across frames.
    async fn send(&self, frames: Vec<Value>) -> AppResult<()> {
        let limit = self.options.sse_max_frame_length;
        for mut frame in frames {
            match limit.and_then(|_| take_plain_fragment_text(&mut frame)) {
                Some(text) => {
                    for chunk in
                        split_json_value_by_string_patch(frame, &text, patch_fragment_text, limit)
                    {
                        send_plain_sse_json(&self.tx, &chunk).await?;
                    }
                }
                None => send_plain_sse_json(&self.tx, &frame).await?,
            }
        }
        Ok(())
    }

    /// Ends the stream with one native error frame.
    async fn fail(&mut self, message: String) -> AppResult<()> {
        self.terminal = true;
        send_plain_sse_json(&self.tx, &error_json(StatusCode::BAD_GATEWAY, &message)).await?;
        Err(
            AppError::new(StatusCode::BAD_GATEWAY, "stream_encode_failed", message)
                .with_downstream_stream_terminal_sent(!self.tx.is_closed()),
        )
    }
}

impl StreamEncoder for GeminiStreamEncoder {
    fn signature_projection() -> Option<SignatureProjection> {
        // Gemini carries call signatures natively.
        None
    }

    async fn begin_replay(&mut self, response: &UrpResponse) -> AppResult<()> {
        // A replay knows the whole response: an unencodable one fails before any Part, and a
        // failed one replays as its native error body alone, as its non-stream body does.
        match encode_response_checked(response, &self.options.logical_model) {
            Err(message) => self.fail(message).await,
            Ok(body) if body.get("error").is_some() => {
                self.terminal = true;
                send_plain_sse_json(&self.tx, &body).await
            }
            Ok(_) => Ok(()),
        }
    }

    async fn on_event(&mut self, event: UrpStreamEvent) -> AppResult<()> {
        if self.terminal {
            return Ok(());
        }
        match self.apply(event) {
            Ok(frames) => self.send(frames).await,
            Err(message) => self.fail(message).await,
        }
    }

    async fn finish(&mut self) -> AppResult<()> {
        // GEM-10: completion requires a canonical terminal event.
        if self.terminal {
            return Ok(());
        }
        self.fail("Gemini stream has no canonical terminal event".into())
            .await
    }
}

/// Takes the text out of a content frame whose Content has no native members and whose only
/// Part is a plain text fragment. Clients merge consecutive such fragments of one kind into one
/// Part (`wire::plain_fragment_kind`), so only this text can split without changing the Parts
/// a client assembles.
fn take_plain_fragment_text(frame: &mut Value) -> Option<String> {
    let content = frame
        .pointer_mut("/candidates/0/content")?
        .as_object_mut()?;
    if content.keys().any(|key| key != "role" && key != "parts") {
        return None;
    }
    let [part] = content.get_mut("parts")?.as_array_mut()?.as_mut_slice() else {
        return None;
    };
    wire::plain_fragment_kind(part)?;
    match part["text"].take() {
        Value::String(text) => Some(text),
        _ => None,
    }
}

fn patch_fragment_text(frame: &mut Value, text: &str) {
    frame["candidates"][0]["content"]["parts"][0]["text"] = json!(text);
}
