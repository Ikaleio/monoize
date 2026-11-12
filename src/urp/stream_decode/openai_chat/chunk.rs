//! One upstream SSE event: the chunk envelope, the terminal state of the choice, and the
//! dispatch of each delta member in a fixed order.

use super::content::{content_blocks, validate_content_media};
use super::state::{ChatStreamState, Flow};
use super::tool_calls::legacy_function_call_as_tool_call;
use crate::error::AppResult;
use crate::handlers::usage::{
    StreamChunkMetrics, parse_usage_from_chat_object, record_stream_chunk_metrics,
    record_stream_terminal_event, response_service_tier,
};
use crate::urp::{
    CHAT_CHOICE_EXTRA_BODY_KEY, CHAT_NATIVE_FINISH_REASON_EXTRA_KEY, FinishReason, is_internal_key,
};
use serde_json::{Map, Value};
use std::collections::HashMap;

static NULL_DELTA: Value = Value::Null;

impl ChatStreamState<'_> {
    pub(super) async fn on_sse_event(
        &mut self,
        event: eventsource_stream::Event,
    ) -> AppResult<Flow> {
        let ttfb_ms = self
            .started_at
            .take()
            .map(|started_at| started_at.elapsed().as_millis() as u64);
        if event.data.trim() == "[DONE]" {
            record_stream_chunk_metrics(
                &self.runtime_metrics,
                StreamChunkMetrics {
                    ttfb_ms,
                    done_sentinel: true,
                    visible_output_bytes: std::mem::take(&mut self.pending_visible_output_bytes),
                    ..StreamChunkMetrics::default()
                },
            )
            .await;
            if !self.terminal_seen {
                self.terminal_error(
                    "upstream_stream_missing_terminal",
                    "upstream Chat Completions stream sent [DONE] before a terminal finish_reason",
                    None,
                    None,
                )
                .await?;
                return Ok(Flow::Stop);
            }
            return Ok(Flow::Done);
        }
        let chunk: Value = match serde_json::from_str(&event.data) {
            Ok(chunk) => chunk,
            Err(error) => {
                record_stream_chunk_metrics(
                    &self.runtime_metrics,
                    StreamChunkMetrics {
                        ttfb_ms,
                        ..StreamChunkMetrics::default()
                    },
                )
                .await;
                self.terminal_error(
                    "invalid_upstream_sse_json",
                    &format!("invalid Chat Completions SSE JSON: {error}"),
                    None,
                    Some(Value::String(event.data)),
                )
                .await?;
                return Ok(Flow::Stop);
            }
        };
        self.on_chunk(&chunk, ttfb_ms).await
    }

    async fn on_chunk(&mut self, chunk: &Value, ttfb_ms: Option<u64>) -> AppResult<Flow> {
        if !self.response_started
            && let Some(id) = chunk.get("id").and_then(Value::as_str)
        {
            self.response_id = id.to_string();
        }
        let usage = parse_usage_from_chat_object(chunk);
        if usage.is_some() {
            self.latest_usage.clone_from(&usage);
        }
        record_stream_chunk_metrics(
            &self.runtime_metrics,
            StreamChunkMetrics {
                ttfb_ms,
                done_sentinel: false,
                response_service_tier: response_service_tier(chunk),
                response_model: chunk.get("model").and_then(Value::as_str),
                usage,
                visible_output_bytes: std::mem::take(&mut self.pending_visible_output_bytes),
            },
        )
        .await;

        let choice = chunk
            .get("choices")
            .and_then(Value::as_array)
            .and_then(|choices| choices.first())
            .and_then(Value::as_object);
        if let Some(error) = chunk_error(chunk, choice) {
            self.terminal_error(
                "upstream_chat_error",
                "upstream Chat Completions error",
                Some(chunk.clone()),
                Some(error.clone()),
            )
            .await?;
            return Ok(Flow::Stop);
        }
        if let Some(choice) = choice
            && let Some(reason) = choice
                .get("finish_reason")
                .and_then(Value::as_str)
                .filter(|reason| !reason.is_empty())
        {
            if reason == "error" {
                self.terminal_error(
                    "upstream_chat_error",
                    "upstream Chat Completions stream terminated with finish_reason=error",
                    Some(chunk.clone()),
                    None,
                )
                .await?;
                return Ok(Flow::Stop);
            }
            self.on_finish_reason(choice, reason).await;
        }

        let delta = choice
            .and_then(|choice| choice.get("delta"))
            .unwrap_or(&NULL_DELTA);
        let message = choice.and_then(|choice| choice.get("message"));
        for content in [
            delta.get("content"),
            message.and_then(|message| message.get("content")),
        ]
        .into_iter()
        .flatten()
        {
            if let Err(message) = validate_content_media(content) {
                self.terminal_error("malformed_media", &message, None, None)
                    .await?;
                return Ok(Flow::Stop);
            }
        }
        self.begin_delta(chunk, choice, delta).await?;
        self.on_delta(choice, delta).await?;
        if let Some(message) = message.and_then(Value::as_object) {
            self.on_terminal_message(message).await?;
        }
        Ok(Flow::Continue)
    }

    /// Records a terminal `finish_reason` other than `error`.
    async fn on_finish_reason(&mut self, choice: &Map<String, Value>, reason: &str) {
        self.terminal_seen = true;
        let finish_reason = parse_finish_reason(reason);
        self.finish_reason = Some(finish_reason);
        if finish_reason == FinishReason::Other {
            self.terminal_extra.insert(
                CHAT_NATIVE_FINISH_REASON_EXTRA_KEY.to_string(),
                Value::String(reason.to_string()),
            );
        }
        let choice_extra = chat_choice_extra(choice);
        if !choice_extra.is_empty() {
            self.terminal_extra.insert(
                CHAT_CHOICE_EXTRA_BODY_KEY.to_string(),
                Value::Object(choice_extra),
            );
        }
        record_stream_terminal_event(&self.runtime_metrics, "chat.completion.chunk", Some(reason))
            .await;
    }

    /// Records the members no typed field owns and starts the response when the chunk opens it.
    async fn begin_delta(
        &mut self,
        chunk: &Value,
        choice: Option<&Map<String, Value>>,
        delta: &Value,
    ) -> AppResult<()> {
        if let Some(delta) = delta.as_object() {
            self.delta_extra
                .extend(unknown_members(delta, KNOWN_DELTA_MEMBERS));
        }
        // The choice members of the terminal chunk belong to `ResponseDone` instead.
        if !self.terminal_seen
            && let Some(choice) = choice
        {
            let choice_extra = chat_choice_extra(choice);
            if !choice_extra.is_empty() {
                self.delta_extra.insert(
                    CHAT_CHOICE_EXTRA_BODY_KEY.to_string(),
                    Value::Object(choice_extra),
                );
            }
        }
        if self.phase.is_none() {
            self.phase = delta
                .get("phase")
                .and_then(Value::as_str)
                .map(str::to_string);
        }
        if !self.response_started {
            if self.created_at.is_none() {
                self.created_at = chunk.get("created").and_then(Value::as_i64);
            }
            if self.chunk_extra.is_none() {
                let chunk_extra = chunk
                    .as_object()
                    .map(|chunk| unknown_members(chunk, KNOWN_CHUNK_MEMBERS))
                    .unwrap_or_default();
                if !chunk_extra.is_empty() {
                    self.chunk_extra = Some(chunk_extra);
                }
            }
        }
        let has_payload = delta_has_payload(delta);
        if delta.get("role").and_then(Value::as_str) == Some("assistant") {
            // A role-only first delta carries its unknown members on `ResponseStart`.
            let start_extra =
                if !self.response_started && !has_payload && !self.delta_extra.is_empty() {
                    self.take_delta_extra()
                } else {
                    HashMap::new()
                };
            self.start_response(start_extra).await?;
        } else if has_payload || choice.is_some_and(|choice| choice.contains_key("message")) {
            self.start_response(HashMap::new()).await?;
        }
        Ok(())
    }

    async fn on_delta(
        &mut self,
        choice: Option<&Map<String, Value>>,
        delta: &Value,
    ) -> AppResult<()> {
        let content_text = delta
            .get("content")
            .and_then(Value::as_str)
            .filter(|text| !text.is_empty());
        let has_tool_calls = delta
            .get("tool_calls")
            .and_then(Value::as_array)
            .is_some_and(|calls| !calls.is_empty())
            || delta
                .get("function_call")
                .is_some_and(|call| !call.is_null());
        // PC7h: nodes are sequential; a switch of output kind completes the previous node.
        if content_text.is_some() || has_tool_calls {
            self.close_reasoning(false).await?;
        }
        // PC8: an empty first `content` must not open a text node ahead of reasoning.
        if let Some(text) = content_text {
            let logprobs = choice_logprobs(choice, "content");
            self.push_text(text, logprobs).await?;
        }
        if let Some(annotations) = delta.get("annotations").and_then(Value::as_array) {
            self.on_annotations(annotations).await?;
        }
        if let Some(refusal) = delta.get("refusal").and_then(Value::as_str) {
            let logprobs = choice_logprobs(choice, "refusal");
            self.push_refusal(refusal, logprobs).await?;
        }
        if let Some(audio) = delta.get("audio").and_then(Value::as_object) {
            self.on_audio(audio).await?;
        }
        if let Some(blocks) = delta.get("content").and_then(content_blocks) {
            self.on_content_blocks(blocks).await?;
        }
        self.on_reasoning_delta(delta).await?;
        if has_tool_calls {
            self.close_reasoning(false).await?;
            self.close_text().await?;
        }
        if let Some(tool_calls) = delta.get("tool_calls").and_then(Value::as_array) {
            for (position, tool_call) in tool_calls.iter().enumerate() {
                self.on_tool_call_delta(tool_call, position, false).await?;
            }
        }
        if let Some(function_call) = delta
            .get("function_call")
            .and_then(legacy_function_call_as_tool_call)
        {
            self.on_tool_call_delta(&function_call, 0, true).await?;
        }
        Ok(())
    }
}

const KNOWN_CHUNK_MEMBERS: &[&str] = &[
    "id", "object", "created", "model", "choices", "usage", "error",
    // OpenRouter's upstream identity is not a response member (DC6a).
    "provider",
];
const KNOWN_CHOICE_MEMBERS: &[&str] = &["index", "delta", "message", "finish_reason", "logprobs"];
const KNOWN_DELTA_MEMBERS: &[&str] = &[
    "role",
    "content",
    "reasoning",
    "reasoning_content",
    "reasoning_details",
    "reasoning_opaque",
    "tool_calls",
    "function_call",
    "refusal",
    "audio",
    "annotations",
    "phase",
];

/// The members of `object` outside `known`, without internal keys.
pub(super) fn unknown_members(object: &Map<String, Value>, known: &[&str]) -> Map<String, Value> {
    object
        .iter()
        .filter(|(key, _)| !is_internal_key(key) && !known.contains(&key.as_str()))
        .map(|(key, value)| (key.clone(), value.clone()))
        .collect()
}

fn chat_choice_extra(choice: &Map<String, Value>) -> Map<String, Value> {
    unknown_members(choice, KNOWN_CHOICE_MEMBERS)
}

/// A native error object of the chunk or of its choice.
fn chunk_error<'v>(chunk: &'v Value, choice: Option<&'v Map<String, Value>>) -> Option<&'v Value> {
    chunk
        .get("error")
        .filter(|error| !error.is_null())
        .or_else(|| {
            choice
                .and_then(|choice| choice.get("error"))
                .filter(|error| !error.is_null())
        })
}

fn choice_logprobs(
    choice: Option<&Map<String, Value>>,
    field: &str,
) -> Option<Vec<crate::urp::TokenLogprob>> {
    crate::urp::logprobs::decode(
        choice
            .and_then(|choice| choice.get("logprobs"))
            .and_then(|logprobs| logprobs.get(field)),
    )
}

/// Whether `delta` carries output, so its chunk opens the response without `role`.
fn delta_has_payload(delta: &Value) -> bool {
    delta.get("content").is_some_and(|value| !value.is_null())
        || delta
            .get("reasoning_details")
            .and_then(Value::as_array)
            .is_some_and(|details| !details.is_empty())
        || [
            "reasoning",
            "reasoning_content",
            "reasoning_opaque",
            "refusal",
        ]
        .iter()
        .any(|field| {
            delta
                .get(*field)
                .and_then(Value::as_str)
                .is_some_and(|value| !value.is_empty())
        })
        || delta
            .get("tool_calls")
            .and_then(Value::as_array)
            .is_some_and(|calls| !calls.is_empty())
        || delta
            .get("function_call")
            .and_then(Value::as_object)
            .is_some_and(|call| !call.is_empty())
}

fn parse_finish_reason(reason: &str) -> FinishReason {
    match reason {
        "stop" => FinishReason::Stop,
        "length" => FinishReason::Length,
        "tool_calls" | "function_call" => FinishReason::ToolCalls,
        "content_filter" => FinishReason::ContentFilter,
        _ => FinishReason::Other,
    }
}
