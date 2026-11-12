//! Terminal errors of a Chat stream: one URP `Error` event plus the runtime error record.

use super::state::ChatStreamState;
use crate::error::AppResult;
use crate::handlers::StreamTerminalError;
use crate::handlers::usage::record_stream_terminal_error;
use crate::urp::{CHAT_ERROR_EVENT_EXTRA_KEY, UrpStreamEvent};
use axum::http::StatusCode;
use serde_json::{Value, json};
use std::collections::HashMap;

impl ChatStreamState<'_> {
    /// Ends the stream with a URP `Error`. Members of the native `error` value override the
    /// fallbacks. `chunk` is the native chunk that carried the error; its semantic error members
    /// move to typed fields, and the rest is kept for a chunk-shaped downstream error.
    pub(super) async fn terminal_error(
        &self,
        fallback_code: &str,
        fallback_message: &str,
        chunk: Option<Value>,
        error: Option<Value>,
    ) -> AppResult<()> {
        let native = error.as_ref();
        let code = native
            .and_then(|error| error_scalar(error, "code", "provider_code"))
            .unwrap_or_else(|| fallback_code.to_string());
        let message = native
            .and_then(|error| error.get("message"))
            .and_then(Value::as_str)
            .or_else(|| native.and_then(Value::as_str))
            .filter(|message| !message.is_empty())
            .unwrap_or(fallback_message)
            .to_string();
        let error_type = native.and_then(|error| error_scalar(error, "type", "error_type"));
        let param = native
            .and_then(|error| error.get("param"))
            .and_then(Value::as_str)
            .filter(|param| !param.is_empty())
            .map(str::to_string);
        let http_status = native
            .and_then(|error| error.get("code"))
            .and_then(Value::as_u64)
            .filter(|status| (400..=599).contains(status))
            .map_or(StatusCode::BAD_GATEWAY.as_u16(), |status| status as u16);

        let mut extra_body = HashMap::new();
        if let Some(mut chunk) = chunk {
            if let Some(root) = chunk.as_object_mut() {
                if let Some(error) = root.get_mut("error") {
                    strip_error_semantics(error);
                }
                if let Some(choices) = root.get_mut("choices").and_then(Value::as_array_mut) {
                    for choice in choices {
                        if let Some(error) = choice.get_mut("error") {
                            strip_error_semantics(error);
                        }
                    }
                }
            }
            crate::urp::decode::remove_untrusted_internal_keys(&mut chunk);
            extra_body.insert(CHAT_ERROR_EVENT_EXTRA_KEY.to_string(), chunk);
        } else if let Some(mut error) = error {
            strip_error_semantics(&mut error);
            extra_body.insert("error".to_string(), error);
        }
        if let Some(error_type) = &error_type {
            extra_body.insert("type".to_string(), Value::String(error_type.clone()));
        }
        if let Some(param) = &param {
            extra_body.insert("param".to_string(), Value::String(param.clone()));
        }

        self.send(UrpStreamEvent::Error {
            code: Some(code.clone()),
            message: message.clone(),
            extra_body,
        })
        .await?;
        record_stream_terminal_error(
            &self.runtime_metrics,
            "chat.completion.error",
            StreamTerminalError {
                code,
                message,
                http_status,
                error_type,
                param,
            },
        )
        .await;
        Ok(())
    }
}

/// A non-empty string or number member `direct_key`, else `metadata.<metadata_key>`.
fn error_scalar(error: &Value, direct_key: &str, metadata_key: &str) -> Option<String> {
    error
        .get(direct_key)
        .and_then(json_scalar_string)
        .or_else(|| {
            error
                .get("metadata")
                .and_then(Value::as_object)
                .and_then(|metadata| metadata.get(metadata_key))
                .and_then(json_scalar_string)
        })
}

fn json_scalar_string(value: &Value) -> Option<String> {
    match value {
        Value::String(value) if !value.is_empty() => Some(value.clone()),
        Value::Number(value) => Some(value.to_string()),
        _ => None,
    }
}

/// Removes the members that the typed `Error` fields own.
fn strip_error_semantics(error: &mut Value) {
    let Some(object) = error.as_object_mut() else {
        *error = json!({});
        return;
    };
    for key in ["message", "code", "type", "param"] {
        object.remove(key);
    }
    if let Some(metadata) = object.get_mut("metadata").and_then(Value::as_object_mut) {
        metadata.remove("provider_code");
        metadata.remove("error_type");
    }
}
