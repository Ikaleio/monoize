use super::*;
use crate::error::StreamErrorDetails;

/// STRM-2b: an explicit frame `status` wins. Decoders synthesize a status for a
/// frame without one, so well-known retryable classes must not reach the client
/// as a 400, and a credential signal must reach RTA-8b.
pub(super) fn pre_output_error_status(
    explicit: Option<StatusCode>,
    http_status: u16,
    code: &str,
    error_type: Option<&str>,
) -> StatusCode {
    if let Some(status) = explicit {
        return status;
    }
    let details = StreamErrorDetails {
        code: Some(code),
        error_type,
        ..Default::default()
    };
    if details.is_credential_failure() {
        return StatusCode::UNAUTHORIZED;
    }
    if http_status == StatusCode::BAD_REQUEST.as_u16()
        && let Some(status) = details.implied_status()
    {
        return status;
    }
    StatusCode::from_u16(http_status).unwrap_or(StatusCode::BAD_GATEWAY)
}

pub(super) async fn emit_stream_error_if_needed(
    downstream: DownstreamProtocol,
    err: &AppError,
    request_id: Option<&str>,
    mask_sensitive_info: bool,
    tx: &mpsc::Sender<Event>,
    capture_frames: Option<&crate::request_capture::SseFrameCapture>,
) {
    if err.downstream_stream_terminal_sent {
        return;
    }
    // SAN-11: a stream-stage error never passed through SAN-1.
    let masked;
    let err = if mask_sensitive_info {
        let mut clone = err.clone();
        clone.message = crate::error_sanitize::mask_sensitive_text(&err.message);
        masked = clone;
        &masked
    } else {
        err
    };
    let (event_name, body) = match downstream {
        DownstreamProtocol::Responses => (Some("error"), responses_stream_error_json(0, err)),
        DownstreamProtocol::ChatCompletions => (None, openai_error_json(err)),
        DownstreamProtocol::AnthropicMessages => {
            (Some("error"), anthropic_stream_error_json(err, request_id))
        }
        DownstreamProtocol::Gemini => (
            None,
            urp::encode::gemini::error_json(err.status, &err.message),
        ),
    };
    let data = body.to_string();
    let event = Event::json_with_payload(event_name, &body, None)
        .expect("writing JSON into memory cannot fail");
    if let Some(frames) = capture_frames {
        frames
            .record(match event_name {
                Some(name) => format!("event: {name}\ndata: {data}\n\n"),
                None => format!("data: {data}\n\n"),
            })
            .await;
    }
    if tx.send(event).await.is_err() {
        return;
    }
    if matches!(downstream, DownstreamProtocol::ChatCompletions) {
        if let Some(frames) = capture_frames {
            frames.record("data: [DONE]\n\n".to_string()).await;
        }
        let _ = tx.send(Event::plain("[DONE]")).await;
    }
}

/// SE2a: reduces one error to the fields its downstream protocol renders. Raw
/// frames, provider metadata and diagnostics stay in the logs and captures.
fn native_stream_error(
    downstream: DownstreamProtocol,
    code: Option<&str>,
    extra_body: &HashMap<String, Value>,
    request_id: Option<&str>,
    mask_sensitive_info: bool,
    cancelled: bool,
) -> (Option<String>, HashMap<String, Value>) {
    let details = StreamErrorDetails::from_urp(code, extra_body);
    let StreamErrorDetails {
        code,
        error_type,
        param,
        ..
    } = details;
    let status = details.status();
    let credential = details.is_credential_failure();
    let mask =
        |text: &str| crate::error_sanitize::maybe_mask_sensitive_text(text, mask_sensitive_info);
    let param = param.filter(|_| !credential).map(mask);
    let mut native = HashMap::new();
    let native_code = match downstream {
        DownstreamProtocol::Gemini => {
            let status = urp::encode::gemini::error_status(details, cancelled);
            native.insert("status".to_string(), json!(status.as_u16()));
            None
        }
        DownstreamProtocol::AnthropicMessages => {
            let mut err = AppError::new(
                if credential {
                    StatusCode::BAD_GATEWAY
                } else {
                    status
                },
                "upstream_error",
                "",
            );
            err.upstream_type = error_type.filter(|_| !credential).map(str::to_string);
            native.insert(
                "error_type".to_string(),
                Value::String(anthropic_error_type(&err)),
            );
            let request_id = request_id
                .filter(|value| !value.is_empty())
                .map(super::messages_wire_request_id)
                .unwrap_or_else(|| {
                    super::messages_wire_request_id(&uuid::Uuid::new_v4().to_string())
                });
            native.insert("request_id".to_string(), Value::String(request_id));
            None
        }
        DownstreamProtocol::ChatCompletions => {
            let openai_type = if credential {
                "server_error".to_string()
            } else {
                error_type
                    .map(mask)
                    .unwrap_or_else(|| openai_error_type_for_status(status).to_string())
            };
            native.insert("type".to_string(), Value::String(openai_type));
            native.insert(
                "param".to_string(),
                param.map(Value::String).unwrap_or(Value::Null),
            );
            Some(if credential {
                "upstream_error".to_string()
            } else {
                code.map(mask)
                    .unwrap_or_else(|| "upstream_error".to_string())
            })
        }
        DownstreamProtocol::Responses => {
            if let Some(param) = param {
                native.insert("param".to_string(), Value::String(param));
            }
            Some(if credential {
                "server_error".to_string()
            } else {
                code.map(mask)
                    .unwrap_or_else(|| "upstream_error".to_string())
            })
        }
    };
    (native_code, native)
}

/// SE2a for a stream event: terminal `Error` events and failed outcomes.
pub(super) fn normalize_stream_error_event(
    event: &mut urp::UrpStreamEvent,
    downstream: DownstreamProtocol,
    request_id: Option<&str>,
    mask_sensitive_info: bool,
) {
    match event {
        urp::UrpStreamEvent::Error {
            code,
            message,
            extra_body,
        } => {
            let (native_code, native) = native_stream_error(
                downstream,
                code.as_deref(),
                extra_body,
                request_id,
                mask_sensitive_info,
                false,
            );
            *code = native_code;
            *extra_body = native;
            *message =
                crate::error_sanitize::maybe_mask_sensitive_text(message, mask_sensitive_info);
        }
        urp::UrpStreamEvent::ResponseDone { outcome, .. } => {
            normalize_outcome_error(outcome, downstream, request_id, mask_sensitive_info);
        }
        _ => {}
    }
}

/// SE2a for a response outcome, on stream and non-stream paths.
pub(in crate::handlers) fn normalize_outcome_error(
    outcome: &mut Option<urp::ResponseOutcome>,
    downstream: DownstreamProtocol,
    request_id: Option<&str>,
    mask_sensitive_info: bool,
) {
    let Some(outcome) = outcome.as_mut() else {
        return;
    };
    // DG2a rule 1: a native Gemini finish keeps its reason in `code`, which the encoder emits
    // as `finishReason`.
    let native_finish = matches!(downstream, DownstreamProtocol::Gemini)
        && urp::encode::gemini::native_failure_finish(outcome).is_some();
    let cancelled = outcome.status == urp::ResponseStatus::Cancelled;
    let Some(error) = outcome.error.as_mut() else {
        return;
    };
    if !native_finish {
        let (native_code, native) = native_stream_error(
            downstream,
            error.code.as_deref(),
            &error.extra_body,
            request_id,
            mask_sensitive_info,
            cancelled,
        );
        error.code = native_code;
        error.extra_body = native;
    }
    if let Some(message) = &error.message {
        error.message = Some(crate::error_sanitize::maybe_mask_sensitive_text(
            message,
            mask_sensitive_info,
        ));
    }
}

/// SE1b: gateway-generated in-stream errors carry the same `request_id`.
fn anthropic_stream_error_json(err: &AppError, request_id: Option<&str>) -> Value {
    let request_id = request_id
        .filter(|value| !value.is_empty())
        .map(super::messages_wire_request_id)
        .unwrap_or_else(|| super::messages_wire_request_id(&uuid::Uuid::new_v4().to_string()));
    anthropic_error_json(err, &request_id)
}

pub(super) fn combine_stream_stage_results(results: [AppResult<()>; 4]) -> AppResult<()> {
    // An encoder failure can close earlier stages. Keep the error that already ended the wire.
    if let Some(err) = results
        .iter()
        .filter_map(|result| result.as_ref().err())
        .find(|err| err.downstream_stream_terminal_sent)
    {
        return Err(err.clone());
    }
    for result in results {
        result?;
    }
    Ok(())
}

fn stream_error_code(err: &AppError) -> String {
    err.upstream_code.as_ref().unwrap_or(&err.code).to_string()
}

pub(super) fn stream_terminal_error_from_app(err: &AppError) -> StreamTerminalError {
    StreamTerminalError {
        code: stream_error_code(err),
        // SAN-9: the terminal request-log row keeps the internal detail while
        // the downstream frame carries the sanitized client message.
        message: err
            .internal_message
            .clone()
            .unwrap_or_else(|| err.message.clone()),
        http_status: err.upstream_status.unwrap_or(err.status.as_u16()),
        error_type: err
            .upstream_type
            .clone()
            .or_else(|| Some(err.error_type.clone())),
        param: err.upstream_param.clone().or_else(|| err.param.clone()),
    }
}

/// The request-log terminal error of a stream whose billing settlement failed.
pub(super) fn billing_settlement_error(err: &AppError) -> StreamTerminalError {
    StreamTerminalError {
        code: "billing_settlement_failed".to_string(),
        message: format!("{}: {}", err.code, err.message),
        http_status: err.status.as_u16(),
        error_type: Some("billing_error".to_string()),
        param: err.param.clone(),
    }
}
