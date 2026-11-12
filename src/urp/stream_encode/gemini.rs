//! URP stream events to Gemini `streamGenerateContent` SSE data frames (gemini-codec.spec.md
//! GEM-8 through GEM-10). Text and reasoning stream in canonical order; other Parts emit when
//! complete. Gemini cannot amend or retract an emitted Part, so an unrepresentable change ends
//! the stream with one native error frame.

mod nodes;
mod terminal;
mod wire;

use super::{StreamEncodeOptions, StreamEncoder};
use crate::error::{AppError, AppResult};
use crate::urp::encode::gemini::encode_response_checked;
use crate::urp::media::error_body;
use crate::urp::stream_helpers::send_plain_sse_json;
use crate::urp::tool_signature::SignatureProjection;
use crate::urp::{UrpResponse, UrpStreamEvent, is_internal_key};
use axum::http::StatusCode;
use axum::response::sse::Event;
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
                Ok(vec![error_frame(code, message, extra_body)])
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

    async fn send(&self, frames: &[Value]) -> AppResult<()> {
        for frame in frames {
            send_plain_sse_json(&self.tx, frame).await?;
        }
        Ok(())
    }

    /// Ends the stream with one native error frame.
    async fn fail(&mut self, message: String) -> AppResult<()> {
        self.terminal = true;
        send_plain_sse_json(&self.tx, &error_body(&message)).await?;
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
            Ok(frames) => self.send(&frames).await,
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

/// GEM-9: `{"error": {"code": <HTTP status>, "message", "status": <canonical status>}}`. A URP
/// code that is a Google canonical status name selects that status; any other is `INTERNAL`.
fn error_frame(code: Option<String>, message: String, extra_body: HashMap<String, Value>) -> Value {
    let mut error: Map<String, Value> = extra_body
        .into_iter()
        .filter(|(key, _)| {
            !is_internal_key(key) && !matches!(key.as_str(), "code" | "message" | "status")
        })
        .collect();
    let status = code
        .as_deref()
        .filter(|code| google_status_http_code(code).is_some())
        .unwrap_or("INTERNAL");
    error.insert(
        "code".into(),
        json!(google_status_http_code(status).unwrap_or(500)),
    );
    error.insert("message".into(), json!(message));
    error.insert("status".into(), json!(status));
    Value::Object(Map::from_iter([(
        "error".to_string(),
        Value::Object(error),
    )]))
}

fn google_status_http_code(status: &str) -> Option<u16> {
    Some(match status {
        "INVALID_ARGUMENT" | "FAILED_PRECONDITION" | "OUT_OF_RANGE" => 400,
        "UNAUTHENTICATED" => 401,
        "PERMISSION_DENIED" => 403,
        "NOT_FOUND" => 404,
        "ABORTED" | "ALREADY_EXISTS" => 409,
        "RESOURCE_EXHAUSTED" => 429,
        "CANCELLED" => 499,
        "UNKNOWN" | "INTERNAL" | "DATA_LOSS" => 500,
        "UNIMPLEMENTED" => 501,
        "UNAVAILABLE" => 503,
        "DEADLINE_EXCEEDED" => 504,
        _ => return None,
    })
}
