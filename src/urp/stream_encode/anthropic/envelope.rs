//! The message envelope (PM9 through PM12a): `message_start`, the terminal `message_delta`
//! and `message_stop`, and the terminal `error` frames (DM7c).

use super::MessagesStreamEncoder;
use super::blocks::BlockPayload;
use crate::error::AppResult;
use crate::error_sanitize::maybe_mask_sensitive_text;
use crate::urp::encode::anthropic::{
    anthropic_native_usage_json, messages_finish_reason, messages_response_id,
    messages_stop_reason_for_output,
};
use crate::urp::encode::merge_extra;
use crate::urp::{FinishReason, Node, ResponseOutcome, TerminalNode, Usage, is_internal_key};
use serde_json::{Map, Value, json};
use std::collections::HashMap;

impl MessagesStreamEncoder {
    /// CTL-5: wire members of `ResponseStart` and of control nodes for `message_start`. An
    /// earlier member wins a collision.
    pub(super) fn add_envelope_extra(&mut self, extra_body: &HashMap<String, Value>) {
        if self.message_started {
            return;
        }
        for (key, value) in extra_body {
            if !is_internal_key(key) {
                self.envelope_extra
                    .entry(key.clone())
                    .or_insert_with(|| value.clone());
            }
        }
    }

    /// MSG-3: `message_start` precedes every block.
    pub(super) async fn ensure_message_start(&mut self) -> AppResult<()> {
        if self.message_started {
            return Ok(());
        }
        self.message_started = true;
        let extra = std::mem::take(&mut self.envelope_extra);
        let default_usage = Usage::default();
        let usage = self.usage.as_ref().unwrap_or(&default_usage);
        let id = self
            .message_id
            .get_or_insert_with(|| messages_response_id(""));
        let payload = message_start_payload(id, &self.options.logical_model, usage, &extra);
        self.wire.event(payload).await
    }

    pub(super) async fn on_response_done(
        &mut self,
        outcome: Option<ResponseOutcome>,
        finish_reason: Option<FinishReason>,
        usage: Option<Usage>,
        output: Vec<TerminalNode>,
        extra_body: HashMap<String, Value>,
    ) -> AppResult<()> {
        if let Some(body) = outcome
            .as_ref()
            .and_then(|outcome| outcome.failure_body(true))
        {
            return self.emit_failure(body).await;
        }
        if usage.is_some() {
            self.usage.clone_from(&usage);
        }
        let forms_message = self.replay
            || self.message_started
            || !self.nodes.is_empty()
            || output
                .iter()
                .any(|entry| BlockPayload::from_node(&entry.node).is_some());
        if !forms_message {
            // A live response without content sends nothing; a later terminal may still
            // open the message.
            self.envelope_extra.clear();
            return Ok(());
        }
        self.ensure_message_start().await?;
        self.finish_blocks(&output).await?;
        self.saw_refusal |= output
            .iter()
            .any(|entry| matches!(entry.node, Node::Refusal { .. }));
        let stop_reason = messages_stop_reason(
            &extra_body,
            finish_reason,
            self.saw_tool_use,
            self.saw_refusal,
        );
        let usage = usage.or_else(|| self.usage.clone()).unwrap_or_default();
        self.wire
            .event(message_delta_payload(&extra_body, stop_reason, &usage))
            .await?;
        self.terminated = true;
        self.wire.event(json!({ "type": "message_stop" })).await
    }

    /// SE4: a failed or cancelled response ends the stream with its native error alone.
    pub(super) async fn emit_failure(&mut self, mut body: Value) -> AppResult<()> {
        self.terminated = true;
        let mask = self.options.mask_sensitive_info;
        let masked = body
            .pointer("/error/message")
            .and_then(Value::as_str)
            .map(|message| maybe_mask_sensitive_text(message, mask));
        if let Some(message) = masked
            && let Some(error) = body.get_mut("error").and_then(Value::as_object_mut)
        {
            error.insert("message".to_string(), Value::String(message));
        }
        self.wire.event(body).await
    }

    pub(super) async fn on_error(
        &mut self,
        code: Option<String>,
        message: String,
        extra_body: HashMap<String, Value>,
    ) -> AppResult<()> {
        self.terminated = true;
        // SAN-11 / SAN-CFG5: decoder-origin error text may embed upstream URLs.
        let message = maybe_mask_sensitive_text(&message, self.options.mask_sensitive_info);
        self.wire
            .event(error_payload(code.as_deref(), &message, &extra_body))
            .await
    }
}

/// PM10: the `message_start` envelope with the latest usage (PM12a).
fn message_start_payload(
    id: &str,
    logical_model: &str,
    usage: &Usage,
    extra: &HashMap<String, Value>,
) -> Value {
    let mut message = Map::new();
    message.insert("id".to_string(), json!(id));
    message.insert("type".to_string(), json!("message"));
    message.insert("role".to_string(), json!("assistant"));
    message.insert("model".to_string(), json!(logical_model));
    message.insert("content".to_string(), json!([]));
    message.insert("stop_reason".to_string(), Value::Null);
    message.insert("stop_sequence".to_string(), Value::Null);
    message.insert("usage".to_string(), anthropic_native_usage_json(usage));
    merge_extra(&mut message, extra);
    json!({ "type": "message_start", "message": message })
}

/// PM11: the preserved native stop reason when it agrees with the typed finish reason, then
/// `tool_use` for an unfinished response with tool use, then the DM4a refusal rule, then the
/// finish-reason mapping.
fn messages_stop_reason(
    extra_body: &HashMap<String, Value>,
    finish_reason: Option<FinishReason>,
    saw_tool_use: bool,
    saw_refusal: bool,
) -> &str {
    if let Some(native) = extra_body
        .get("stop_reason")
        .and_then(Value::as_str)
        .filter(|native| messages_finish_reason(native) == finish_reason)
    {
        return native;
    }
    if finish_reason.is_none() && saw_tool_use {
        return "tool_use";
    }
    if saw_refusal
        && matches!(
            finish_reason,
            None | Some(FinishReason::Stop | FinishReason::Other)
        )
    {
        return "refusal";
    }
    // The refusal rule above already covered the streamed nodes.
    messages_stop_reason_for_output(finish_reason, &[])
}

/// The terminal `message_delta` (PM11a), carrying preserved `stop_details` and `container`
/// (PM6d).
fn message_delta_payload(
    extra_body: &HashMap<String, Value>,
    stop_reason: &str,
    usage: &Usage,
) -> Value {
    let stop_sequence = match stop_reason {
        "stop_sequence" => extra_body
            .get("stop_sequence")
            .cloned()
            .unwrap_or(Value::Null),
        _ => Value::Null,
    };
    let mut delta = Map::new();
    delta.insert("stop_reason".to_string(), json!(stop_reason));
    delta.insert("stop_sequence".to_string(), stop_sequence);
    for key in ["stop_details", "container"] {
        if let Some(value) = extra_body.get(key) {
            delta.insert(key.to_string(), value.clone());
        }
    }
    json!({
        "type": "message_delta",
        "delta": delta,
        "usage": anthropic_native_usage_json(usage)
    })
}

/// DM7c: the provider error type wins over the canonical code; other wire members stay
/// nested in `error`.
fn error_payload(code: Option<&str>, message: &str, extra_body: &HashMap<String, Value>) -> Value {
    let nested_error = extra_body.get("error").and_then(Value::as_object);
    let error_type = extra_body
        .get("error_type")
        .and_then(Value::as_str)
        .or_else(|| extra_body.get("type").and_then(Value::as_str))
        .or_else(|| {
            nested_error
                .and_then(|error| error.get("type"))
                .and_then(Value::as_str)
        })
        .filter(|value| !value.is_empty())
        .or(code.filter(|value| !value.is_empty()))
        .unwrap_or("server_error");
    let mut error = nested_error.cloned().unwrap_or_default();
    error.retain(|key, _| !is_internal_key(key));
    for (key, value) in extra_body {
        if !matches!(key.as_str(), "error" | "error_type" | "type" | "request_id")
            && !is_internal_key(key)
        {
            error.entry(key.clone()).or_insert_with(|| value.clone());
        }
    }
    error.insert("type".to_string(), json!(error_type));
    error.insert("message".to_string(), json!(message));
    let mut payload = Map::new();
    payload.insert("type".to_string(), json!("error"));
    payload.insert("error".to_string(), Value::Object(error));
    if let Some(request_id) = extra_body.get("request_id") {
        payload.insert("request_id".to_string(), request_id.clone());
    }
    Value::Object(payload)
}
