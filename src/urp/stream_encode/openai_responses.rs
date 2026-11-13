//! Downstream OpenAI Responses SSE encoder (RESP-7, STR3*).
//!
//! `ResponsesStreamEncoder` owns every cross-event state of one downstream stream: the
//! sequence counter, the response envelope, one `NodeState` per announced node, and one
//! `OutputItem` per downstream `output_index`.

mod items;
mod media;
mod nodes;
mod reasoning;
mod terminal;
mod wire;

use crate::error::AppResult;
use crate::handlers::routing::now_ts;
use crate::payload::sse::WireEvent as Event;
use crate::urp::encode::openai_responses::{
    encode_response_checked, encode_usage, synthesized_response_controls,
};
use crate::urp::encode::sanitize_provider_item_wire_body;
use crate::urp::stream_encode::{StreamEncodeOptions, StreamEncoder};
use crate::urp::tool_signature::SignatureProjection;
use crate::urp::{
    RESPONSES_IN_PROGRESS_CONTROL, RESPONSES_MESSAGE_DONE_CONTROL,
    RESPONSES_RESPONSE_SOURCE_EXTRA_KEY, RESPONSES_STREAM_START_SOURCE_EXTRA_KEY, UrpResponse,
    UrpStreamEvent, Usage, is_internal_control, is_internal_key,
};
use nodes::{NodeState, OpenMessage, OutputItem};
use serde_json::{Map, Value, json};
use std::collections::HashMap;
use tokio::sync::mpsc;
use wire::Wire;

/// Members of the start object the encoder owns; an upstream copy never replaces them.
const START_OWNED_FIELDS: [&str; 11] = [
    "id",
    "object",
    "created",
    "created_at",
    "model",
    "status",
    "output",
    "incomplete_details",
    "error",
    "usage",
    "completed_at",
];

pub(crate) struct ResponsesStreamEncoder {
    wire: Wire,
    options: StreamEncodeOptions,
    response_id: String,
    created_at: Option<i64>,
    /// An `error` event ended the downstream stream (SE4); later events are dropped.
    terminated: bool,
    /// PR5e: the start object of a native stream until the upstream in-progress object arrives.
    pending_in_progress: Option<Value>,
    /// CTL-5: wire members of control nodes for the next output item.
    pending_envelope_extra: HashMap<String, Value>,
    nodes: HashMap<u32, NodeState>,
    /// Indexed by downstream `output_index`.
    items: Vec<OutputItem>,
    /// The `message` item the next content part may join (RECON-6).
    open_message: Option<OpenMessage>,
}

impl ResponsesStreamEncoder {
    pub(crate) fn new(tx: mpsc::Sender<Event>, options: StreamEncodeOptions) -> Self {
        Self {
            wire: Wire::new(tx, options.sse_max_frame_length),
            options,
            response_id: "resp".to_string(),
            created_at: None,
            terminated: false,
            pending_in_progress: None,
            pending_envelope_extra: HashMap::new(),
            nodes: HashMap::new(),
            items: Vec::new(),
            open_message: None,
        }
    }

    /// STR3o: the duration a completed reasoning item reports.
    fn reasoning_duration_secs(&self) -> u64 {
        self.options
            .replay_reasoning_duration_secs
            .unwrap_or_else(|| self.options.started_at.elapsed().as_secs())
    }

    /// PR5e: emits the held `response.in_progress` before any later event except an unrelated
    /// provider control. Returns `true` when `event` was the upstream in-progress object.
    async fn release_in_progress(&mut self, event: &UrpStreamEvent) -> AppResult<bool> {
        let source = match event {
            UrpStreamEvent::ProviderControl {
                event_name, data, ..
            } if event_name == RESPONSES_IN_PROGRESS_CONTROL => Some(data),
            UrpStreamEvent::ProviderControl { .. } => return Ok(false),
            _ => None,
        };
        if let Some(payload) = self.pending_in_progress.take() {
            let payload = match source {
                Some(source) => response_in_progress_payload(&payload, source),
                None => payload,
            };
            self.wire.event("response.in_progress", payload).await?;
        }
        Ok(source.is_some())
    }

    async fn on_response_start(
        &mut self,
        usage: Option<Usage>,
        id: String,
        created_at: Option<i64>,
        extra_body: HashMap<String, Value>,
    ) -> AppResult<()> {
        let created_at = created_at.unwrap_or_else(now_ts);
        self.created_at = Some(created_at);
        let payload = self.start_payload(&id, created_at, usage.as_ref(), &extra_body);
        self.response_id = id;
        self.wire.event("response.created", payload.clone()).await?;
        if extra_body.contains_key(RESPONSES_STREAM_START_SOURCE_EXTRA_KEY) {
            // A later upstream in-progress object may add fields (PR5d).
            self.pending_in_progress = Some(payload);
            Ok(())
        } else {
            self.wire.event("response.in_progress", payload).await
        }
    }

    /// STR3e: the in-progress object of `response.created`. Only an object synthesized for a
    /// non-Responses source carries request controls (STR3t).
    fn start_payload(
        &self,
        id: &str,
        created_at: i64,
        usage: Option<&Usage>,
        extra_body: &HashMap<String, Value>,
    ) -> Value {
        let synthesized = !extra_body.contains_key(RESPONSES_STREAM_START_SOURCE_EXTRA_KEY)
            && !extra_body.contains_key(RESPONSES_RESPONSE_SOURCE_EXTRA_KEY);
        let mut response = Map::new();
        response.insert("id".to_string(), json!(id));
        response.insert("object".to_string(), json!("response"));
        response.insert("created_at".to_string(), json!(created_at));
        response.insert("model".to_string(), json!(self.options.logical_model));
        if synthesized {
            response.insert("completed_at".to_string(), Value::Null);
            response.extend(synthesized_response_controls(
                self.options.responses_request_echo.as_ref(),
            ));
        }
        for (key, value) in extra_body {
            if !is_internal_key(key) && !START_OWNED_FIELDS.contains(&key.as_str()) {
                response.insert(key.clone(), value.clone());
            }
        }
        // A replayed response's own `completed_at` belongs to its terminal event only.
        if extra_body.contains_key("completed_at") {
            response.insert("completed_at".to_string(), Value::Null);
        }
        response.insert("status".to_string(), json!("in_progress"));
        response.insert("output".to_string(), json!([]));
        response.insert("incomplete_details".to_string(), Value::Null);
        response.insert("error".to_string(), Value::Null);
        response.insert("usage".to_string(), usage.map_or(Value::Null, encode_usage));
        json!({ "response": response })
    }

    async fn on_provider_control(
        &mut self,
        protocol: String,
        event_name: String,
        data: Value,
    ) -> AppResult<()> {
        if protocol != "responses" {
            return Ok(());
        }
        if event_name == RESPONSES_MESSAGE_DONE_CONTROL {
            // STR3q: the upstream message item-done owns the item-level fields.
            if let (Some(open), Value::Object(fields)) = (&self.open_message, &data)
                && let Some(item) = self.items[open.output_index].item.as_object_mut()
            {
                item.extend(
                    fields
                        .iter()
                        .filter(|(key, _)| !is_internal_key(key))
                        .map(|(key, value)| (key.clone(), value.clone())),
                );
            }
            return self.close_open_message().await;
        }
        if is_internal_control(&event_name)
            || matches!(
                event_name.as_str(),
                "response.created"
                    | "response.in_progress"
                    | "response.completed"
                    | "response.incomplete"
                    | "response.failed"
                    | "response.cancelled"
            )
        {
            return Ok(());
        }
        self.wire
            .passthrough(&event_name, sanitize_provider_item_wire_body(&data))
            .await
    }

    async fn on_error(
        &mut self,
        code: Option<String>,
        message: String,
        extra_body: HashMap<String, Value>,
    ) -> AppResult<()> {
        self.terminated = true;
        // SAN-11 / SAN-CFG5: decoder-origin error text may embed upstream URLs.
        self.wire
            .event(
                "error",
                json!({
                    "code": code.as_deref().unwrap_or("upstream_error"),
                    "message": crate::error_sanitize::maybe_mask_sensitive_text(
                        &message,
                        self.options.mask_sensitive_info,
                    ),
                    "param": extra_body.get("param").cloned().unwrap_or(Value::Null),
                }),
            )
            .await
    }
}

impl StreamEncoder for ResponsesStreamEncoder {
    fn signature_projection() -> Option<SignatureProjection> {
        Some(SignatureProjection::default())
    }

    /// RECON-8: output the Responses schema cannot carry ends the replay with the native
    /// media error before `response.created`.
    async fn begin_replay(&mut self, response: &UrpResponse) -> AppResult<()> {
        match encode_response_checked(response, &self.options.logical_model) {
            Ok(_) => Ok(()),
            Err(error) => self.wire.media_error(&error).await,
        }
    }

    async fn on_event(&mut self, mut event: UrpStreamEvent) -> AppResult<()> {
        if self.terminated || self.release_in_progress(&event).await? {
            return Ok(());
        }
        if let Err(error) = self.validate_media_event(&mut event) {
            return self.wire.media_error(&error).await;
        }
        match event {
            UrpStreamEvent::ResponseStart {
                usage,
                id,
                created_at,
                extra_body,
                ..
            } => {
                self.on_response_start(usage, id, created_at, extra_body)
                    .await
            }
            UrpStreamEvent::NodeStart {
                node_index,
                header,
                extra_body,
            } => self.on_node_start(node_index, header, extra_body).await,
            UrpStreamEvent::NodeDelta {
                node_index,
                delta,
                extra_body,
                ..
            } => self.on_node_delta(node_index, delta, extra_body).await,
            UrpStreamEvent::NodeDone {
                node_index,
                node,
                extra_body,
                ..
            } => self.on_node_done(node_index, &node, &extra_body).await,
            UrpStreamEvent::ResponseDone {
                outcome,
                finish_reason,
                usage,
                output,
                extra_body,
            } => {
                self.on_response_done(outcome, finish_reason, usage, output, extra_body)
                    .await
            }
            UrpStreamEvent::ProviderControl {
                protocol,
                event_name,
                data,
                ..
            } => self.on_provider_control(protocol, event_name, data).await,
            UrpStreamEvent::Error {
                code,
                message,
                extra_body,
            } => self.on_error(code, message, extra_body).await,
        }
    }

    async fn finish(&mut self) -> AppResult<()> {
        match self.pending_in_progress.take() {
            Some(payload) => self.wire.event("response.in_progress", payload).await,
            None => Ok(()),
        }
    }
}

/// PR5e: the upstream `response.in_progress` object owns the downstream event.
fn response_in_progress_payload(start: &Value, source: &Value) -> Value {
    let mut response: Map<String, Value> = source
        .as_object()
        .into_iter()
        .flatten()
        .filter(|(key, _)| !is_internal_key(key) && key.as_str() != "created")
        .map(|(key, value)| (key.clone(), value.clone()))
        .collect();
    let started = &start["response"];
    for key in [
        "id",
        "object",
        "created_at",
        "model",
        "store",
        "previous_response_id",
    ] {
        if let Some(value) = started.get(key) {
            response.insert(key.to_string(), value.clone());
        }
    }
    response.insert("status".to_string(), json!("in_progress"));
    response.insert("output".to_string(), json!([]));
    for key in ["error", "incomplete_details", "usage"] {
        response.entry(key.to_string()).or_insert(Value::Null);
    }
    json!({ "response": response })
}

/// The `id` of a wire item, empty when it has none.
fn item_id(item: &Value) -> String {
    item.get("id")
        .and_then(Value::as_str)
        .unwrap_or_default()
        .to_string()
}
