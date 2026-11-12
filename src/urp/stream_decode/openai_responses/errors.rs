//! Upstream event validation and native error envelopes.

use super::nodes::wire_extra;
use crate::handlers::StreamTerminalError;
use crate::urp::decode::openai_responses::{validate_compatible_content, validate_responses_items};
use axum::http::StatusCode;
use serde_json::Value;
use std::collections::HashMap;

/// A rejected upstream event: its terminal error code and message.
pub(super) struct Rejection {
    pub(super) code: &'static str,
    pub(super) message: String,
}

/// Rejects an event whose required members are malformed, or that carries malformed media.
pub(super) fn validate_event(event: &str, data: &Value) -> Result<(), Rejection> {
    validate_shape(event, data).map_err(|message| Rejection {
        code: "responses_invalid_event",
        message,
    })?;
    let media = match event {
        "response.content_part.added" | "response.content_part.done" => {
            data.get("part").map(validate_compatible_content)
        }
        "response.output_item.added" | "response.output_item.done" => {
            data.get("item").map(validate_responses_items)
        }
        "response.completed" | "response.failed" | "response.incomplete" => data
            .get("response")
            .and_then(|response| response.get("output"))
            .map(validate_responses_items),
        _ => None,
    };
    match media {
        Some(Err(message)) => Err(Rejection {
            code: "malformed_media",
            message,
        }),
        _ => Ok(()),
    }
}

fn validate_shape(event: &str, data: &Value) -> Result<(), String> {
    if !data.is_object() {
        return Err(format!("{event} payload must be an object"));
    }
    let required = match event {
        "response.created"
        | "response.in_progress"
        | "response.queued"
        | "response.completed"
        | "response.incomplete"
        | "response.failed"
        | "response.cancelled" => Some("response"),
        "response.output_item.added" | "response.output_item.done" => Some("item"),
        "response.content_part.added" | "response.content_part.done" => Some("part"),
        _ => None,
    };
    if let Some(field) = required {
        let object = data
            .get(field)
            .and_then(Value::as_object)
            .ok_or_else(|| format!("{event}.{field} must be an object"))?;
        if field == "response" {
            if object.get("output").is_some_and(|value| !value.is_array()) {
                return Err(format!("{event}.response.output must be an array"));
            }
            if is_terminal(event)
                && let Some(status) = object.get("status")
                && status.as_str() != event.strip_prefix("response.")
            {
                return Err(format!("{event} conflicts with response.status"));
            }
        }
    }
    if matches!(
        event,
        "response.output_text.delta"
            | "response.refusal.delta"
            | "response.reasoning_text.delta"
            | "response.reasoning_summary_text.delta"
            | "response.function_call_arguments.delta"
            | "response.custom_tool_call_input.delta"
    ) && !data.get("delta").is_some_and(Value::is_string)
    {
        return Err(format!("{event}.delta must be a string"));
    }
    Ok(())
}

pub(super) fn is_terminal(event: &str) -> bool {
    matches!(
        event,
        "response.completed" | "response.incomplete" | "response.failed" | "response.cancelled"
    )
}

/// The canonical `Error` members of a native `error` event or a failed response, and the
/// terminal error recorded for it.
pub(super) struct NativeError {
    pub(super) code: Option<String>,
    pub(super) message: String,
    pub(super) extra_body: HashMap<String, Value>,
    pub(super) terminal: StreamTerminalError,
}

pub(super) fn native_error(event_name: &str, data: &Value) -> NativeError {
    let error = if event_name == "response.failed" {
        data.get("response")
            .and_then(|response| response.get("error"))
            .cloned()
            .unwrap_or(Value::Null)
    } else {
        data.get("error").cloned().unwrap_or_else(|| data.clone())
    };
    let member = |key: &str| {
        error
            .get(key)
            .and_then(Value::as_str)
            .or_else(|| data.get(key).and_then(Value::as_str))
    };
    let code = member("code").map(str::to_string);
    let message = member("message")
        .or_else(|| data.as_str())
        .unwrap_or("upstream error")
        .to_string();
    let error_type = member("type")
        .filter(|kind| *kind != event_name)
        .map(str::to_string);
    let param = member("param").map(str::to_string);
    let http_status = error
        .get("status")
        .and_then(Value::as_u64)
        .or_else(|| data.get("status").and_then(Value::as_u64))
        .filter(|status| (400..=599).contains(status))
        .and_then(|status| u16::try_from(status).ok())
        .unwrap_or(StatusCode::BAD_REQUEST.as_u16());
    let mut extra_body = wire_extra(&error, &["code", "message"]);
    extra_body.extend(wire_extra(
        data,
        &["code", "message", "type", "error", "response"],
    ));
    NativeError {
        terminal: StreamTerminalError {
            code: code
                .clone()
                .unwrap_or_else(|| "upstream_stream_error".to_string()),
            message: message.clone(),
            http_status,
            error_type,
            param,
        },
        code,
        message,
        extra_body,
    }
}
