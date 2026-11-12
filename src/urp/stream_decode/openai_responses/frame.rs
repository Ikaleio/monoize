//! Frame parsing, response start, and dispatch of one upstream event to its handler.

use super::errors::{Rejection, is_terminal, native_error, validate_event};
use super::state::{Flow, ResponsesStreamDecoder};
use crate::handlers::usage::{
    mark_stream_ttfb_if_needed, parse_usage_from_responses_object,
    record_observed_upstream_response_model, record_stream_done_sentinel,
    record_stream_response_id, record_stream_response_service_tier, record_stream_terminal_error,
    record_stream_terminal_event, record_stream_usage_if_present,
};
use crate::urp::decode::openai_responses::parse_usage_from_responses;
use crate::urp::decode::split_extra;
use crate::urp::{
    RESPONSES_DISCARD_REQUESTED_FRAME, RESPONSES_IN_PROGRESS_CONTROL,
    RESPONSES_STREAM_START_SOURCE_EXTRA_KEY, UrpStreamEvent, is_internal_control,
};
use serde_json::{Map, Value, json};
use std::collections::HashMap;

fn is_start(event: &str) -> bool {
    matches!(event, "response.created" | "response.in_progress")
}

fn is_output_event(event: &str) -> bool {
    [
        "response.output_",
        "response.content_part.",
        "response.reasoning_",
        "response.function_call_",
        "response.image_generation",
        "image_generation.",
    ]
    .iter()
    .any(|prefix| event.starts_with(prefix))
}

impl ResponsesStreamDecoder {
    pub(super) async fn on_frame(&mut self, mut event_name: String, data: String) -> Flow {
        if is_internal_control(&event_name) {
            if event_name == RESPONSES_DISCARD_REQUESTED_FRAME {
                self.discard_requested = data.trim() == "true";
            }
            return Flow::Continue;
        }
        mark_stream_ttfb_if_needed(&mut self.started_at, &self.metrics).await;
        if data.trim() == "[DONE]" {
            record_stream_done_sentinel(&self.metrics).await;
            return Flow::End;
        }
        let data: Value = match serde_json::from_str(&data) {
            Ok(value) => value,
            Err(error) => {
                self.release_held(None, None).await;
                let extra_body = HashMap::from([
                    ("event_name".to_string(), json!(event_name)),
                    ("raw_data".to_string(), json!(data)),
                ]);
                let message = format!("invalid JSON in upstream Responses event: {error}");
                return self
                    .protocol_error("responses_invalid_sse_json", message, None, extra_body)
                    .await;
            }
        };
        if (event_name.is_empty() || event_name == "message")
            && let Some(kind) = data
                .get("type")
                .and_then(Value::as_str)
                .filter(|kind| !kind.is_empty())
        {
            event_name = kind.to_string();
        }
        if let Err(Rejection { code, message }) = validate_event(&event_name, &data) {
            let envelope = json!({
                "error": { "code": code, "type": "upstream_protocol_error", "message": message }
            });
            return self.fail_native(code, "error", &envelope).await;
        }
        self.observe(&event_name, &data).await;
        let bare_error = data.get("error").is_some_and(|error| !error.is_null())
            && data.get("response").is_none();
        if event_name == "error" || bare_error {
            return self.fail_native(&event_name, &event_name, &data).await;
        }
        if event_name == "response.failed" {
            let error = native_error(&event_name, &data);
            record_stream_terminal_error(&self.metrics, &event_name, error.terminal).await;
        }
        if is_terminal(&event_name) {
            return self.on_terminal(event_name, &data).await;
        }
        self.on_output_event(&event_name, data).await;
        Flow::Continue
    }

    /// Records response-level metrics and sends the response start before the first event
    /// that implies one.
    async fn observe(&mut self, event_name: &str, data: &Value) {
        record_stream_response_service_tier(&self.metrics, data).await;
        let response = data.get("response").and_then(Value::as_object);
        if let Some(id) = response
            .and_then(|response| response.get("id"))
            .and_then(Value::as_str)
            .filter(|id| !id.is_empty())
        {
            self.response_id = id.to_string();
            record_stream_response_id(&self.metrics, id).await;
        }
        let terminal = is_terminal(event_name);
        if let Some(model) = response
            .and_then(|response| response.get("model"))
            .and_then(Value::as_str)
            .or_else(|| data.get("model").and_then(Value::as_str))
        {
            record_observed_upstream_response_model(&self.metrics, model, terminal).await;
        }
        if self.start_sent
            && event_name == "response.in_progress"
            && let Some(source) = response
        {
            self.send(UrpStreamEvent::ProviderControl {
                protocol: "responses".to_string(),
                event_name: RESPONSES_IN_PROGRESS_CONTROL.to_string(),
                data: Value::Object(source.clone()),
                extra_body: HashMap::new(),
            })
            .await;
        }
        if !self.start_sent && (is_start(event_name) || terminal || is_output_event(event_name)) {
            self.send_start(is_start(event_name), response).await;
        }
        record_stream_usage_if_present(&self.metrics, parse_usage_from_responses_object(data))
            .await;
    }

    /// PR5d: a native start keeps the upstream start object's unknown members; any other first
    /// event opens a synthesized in-progress envelope.
    async fn send_start(&mut self, native: bool, source: Option<&Map<String, Value>>) {
        let model = source
            .and_then(|source| source.get("model"))
            .and_then(Value::as_str)
            .filter(|model| !model.is_empty())
            .map_or_else(|| self.request_model.clone(), str::to_string);
        let created_at = source.and_then(|source| {
            source
                .get("created_at")
                .or_else(|| source.get("created"))
                .and_then(Value::as_i64)
        });
        let extra_body = if native {
            let mut extra = source.map_or_else(HashMap::new, |source| {
                split_extra(
                    source,
                    &[
                        "id",
                        "model",
                        "created_at",
                        "created",
                        "output",
                        "usage",
                        "status",
                        "incomplete_details",
                        "error",
                    ],
                )
            });
            extra.insert(
                RESPONSES_STREAM_START_SOURCE_EXTRA_KEY.to_string(),
                json!({}),
            );
            extra
        } else {
            HashMap::from([
                ("object".to_string(), json!("response")),
                ("status".to_string(), json!("in_progress")),
                ("output".to_string(), json!([])),
            ])
        };
        self.send(UrpStreamEvent::ResponseStart {
            usage: source
                .and_then(|source| source.get("usage"))
                .and_then(Value::as_object)
                .map(parse_usage_from_responses),
            id: self.response_id.clone(),
            model,
            created_at,
            extra_body,
        })
        .await;
        self.start_sent = true;
    }

    /// Maps one non-terminal event and delivers its events in `output_index` order (PR5b).
    async fn on_output_event(&mut self, event_name: &str, data: Value) {
        let output_index = data.get("output_index").and_then(Value::as_u64);
        if event_name == "response.output_item.added"
            && let Some(output_index) = output_index
        {
            self.announce(output_index);
        }
        let events = self.map_event(event_name, data);
        self.route(output_index, events).await;
        if event_name == "response.output_item.done"
            && let Some(output_index) = output_index
        {
            self.complete(output_index).await;
        }
    }

    fn map_event(&mut self, event_name: &str, data: Value) -> Vec<UrpStreamEvent> {
        match event_name {
            "response.created" | "response.in_progress" => Vec::new(),
            "response.output_item.added" => self.on_item_added(&data),
            "response.output_item.done" => self.on_item_done(&data),
            "response.content_part.added" => self.on_part_added(&data),
            "response.content_part.done" => self.on_part_done(&data),
            "response.output_text.delta" => self.on_text_delta(&data),
            "response.refusal.delta" => self.on_refusal_delta(&data),
            "response.output_text.annotation.added" => self.on_annotation(&data),
            "response.output_text.done" | "response.refusal.done" => {
                self.on_content_done(event_name, &data);
                Vec::new()
            }
            "response.reasoning_text.delta" | "response.reasoning_summary_text.delta" => {
                self.on_reasoning_delta(event_name, &data)
            }
            "response.reasoning_text.done" => {
                self.on_reasoning_text_done(&data);
                Vec::new()
            }
            "response.reasoning_summary_part.added"
            | "response.reasoning_summary_text.done"
            | "response.reasoning_summary_part.done" => self.on_summary_event(event_name, &data),
            "response.function_call_arguments.delta" | "response.custom_tool_call_input.delta" => {
                self.on_arguments_delta(event_name, &data)
            }
            "response.function_call_arguments.done" | "response.custom_tool_call_input.done" => {
                self.on_arguments_done(event_name, &data);
                Vec::new()
            }
            "image_generation.completed" | "response.image_generation.completed" => {
                self.on_image_completed(&data)
            }
            "image_generation.partial_image"
            | "response.image_generation.partial_image"
            | "response.image_generation_call.in_progress"
            | "response.image_generation_call.generating"
            | "response.image_generation_call.partial_image"
            | "response.image_generation_call.completed" => self.on_image_call_event(&data),
            _ => vec![UrpStreamEvent::ProviderControl {
                protocol: "responses".to_string(),
                event_name: event_name.to_string(),
                data,
                extra_body: HashMap::new(),
            }],
        }
    }

    /// PR6c: an upstream stream without a terminal response event ends in an error.
    pub(super) async fn finish(&mut self) {
        if let Some(event) = &self.terminal_event {
            record_stream_terminal_event(&self.metrics, event, None).await;
            return;
        }
        self.release_held(None, None).await;
        let extra_body = HashMap::from([(
            "expected_events".to_string(),
            json!([
                "response.completed",
                "response.incomplete",
                "response.failed",
                "response.cancelled"
            ]),
        )]);
        let message =
            "upstream Responses stream ended without a terminal response event".to_string();
        self.protocol_error(
            "responses_stream_missing_terminal",
            message,
            None,
            extra_body,
        )
        .await;
    }
}
