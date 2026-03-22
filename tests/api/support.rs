use axum::Json;
use axum::Router;
use axum::body::Body;
use axum::http::header::{AUTHORIZATION, CONTENT_TYPE};
use axum::http::{Request, StatusCode};
use axum::response::IntoResponse;
use axum::response::Sse;
use axum::response::sse::Event;
use axum::routing::post;
use chrono::{Duration as ChronoDuration, Utc};
use http_body_util::BodyExt;
use serde_json::{Value, json};
use std::collections::HashMap;
use std::convert::Infallible;
use std::net::SocketAddr;
use std::sync::{Arc, Mutex};
use std::time::Duration;
use tempfile::TempDir;
use tower::ServiceExt;

struct TestContext {
    router: axum::Router,
    auth_header: String,
    state: monoize::app::AppState,
    captured_headers: Arc<Mutex<Vec<(String, String)>>>,
    _temp_dir: TempDir,
}

fn maybe_forced_upstream_error(body: &Value) -> Option<axum::response::Response> {
    let status_u64 = body
        .get("force_upstream_error_status")
        .and_then(|v| v.as_u64())?;
    let status_u16 = u16::try_from(status_u64).ok()?;
    let status = StatusCode::from_u16(status_u16).ok()?;
    let code = body
        .get("force_upstream_error_code")
        .and_then(|v| v.as_str())
        .unwrap_or("forced_upstream_error");
    let message = body
        .get("force_upstream_error_message")
        .and_then(|v| v.as_str())
        .unwrap_or("forced upstream error");
    Some(
        (
            status,
            Json(json!({
                "error": {
                    "code": code,
                    "message": message
                }
            })),
        )
            .into_response(),
    )
}

fn maybe_reasoning_summary_validation_error(body: &Value) -> Option<axum::response::Response> {
    if body
        .get("require_reasoning_input_summary")
        .and_then(|v| v.as_bool())
        != Some(true)
    {
        return None;
    }
    let input = body.get("input").and_then(|v| v.as_array())?;
    let missing_summary_index = input.iter().position(|item| {
        item.get("type").and_then(|v| v.as_str()) == Some("reasoning")
            && item.get("summary").is_none()
    });
    if let Some(missing_index) = missing_summary_index {
        return Some((
            StatusCode::BAD_REQUEST,
            Json(json!({
                "error": {
                    "message": format!("Missing required parameter: 'input[{missing_index}].summary'."),
                    "type": "invalid_request_error",
                    "param": format!("input[{missing_index}].summary"),
                    "code": "missing_required_parameter"
                }
            })),
        )
            .into_response());
    }
    let unknown_source_index = input.iter().position(|item| {
        item.get("type").and_then(|v| v.as_str()) == Some("reasoning")
            && item.get("source").is_some()
    });
    if let Some(unknown_source_index) = unknown_source_index {
        return Some((
            StatusCode::BAD_REQUEST,
            Json(json!({
                "error": {
                    "message": format!("Unknown parameter: 'input[{unknown_source_index}].source'."),
                    "type": "invalid_request_error",
                    "param": format!("input[{unknown_source_index}].source"),
                    "code": "unknown_parameter"
                }
            })),
        )
            .into_response());
    }
    let unknown_text_index = input.iter().position(|item| {
        item.get("type").and_then(|v| v.as_str()) == Some("reasoning") && item.get("text").is_some()
    })?;
    Some(
        (
            StatusCode::BAD_REQUEST,
            Json(json!({
                "error": {
                    "message": format!("Unknown parameter: 'input[{unknown_text_index}].text'."),
                    "type": "invalid_request_error",
                    "param": format!("input[{unknown_text_index}].text"),
                    "code": "unknown_parameter"
                }
            })),
        )
            .into_response(),
    )
}

fn maybe_assistant_output_content_validation_error(
    body: &Value,
) -> Option<axum::response::Response> {
    if body
        .get("require_assistant_output_content_types")
        .and_then(|v| v.as_bool())
        != Some(true)
    {
        return None;
    }

    let input = body.get("input").and_then(|v| v.as_array())?;
    let invalid_index = input.iter().enumerate().find_map(|(index, item)| {
        if item.get("type").and_then(|v| v.as_str()) != Some("message") {
            return None;
        }
        if item.get("role").and_then(|v| v.as_str()) != Some("assistant") {
            return None;
        }
        let content = item.get("content").and_then(|v| v.as_array())?;
        let invalid_part = content.iter().position(|part| {
            !matches!(
                part.get("type").and_then(|v| v.as_str()),
                Some("output_text" | "refusal" | "output_image" | "output_file")
            )
        })?;
        Some((index, invalid_part))
    });

    let (message_index, content_index) = invalid_index?;
    Some((
        StatusCode::BAD_REQUEST,
        Json(json!({
            "error": {
                "message": format!("Invalid value: 'input_text'. Supported values are: 'output_text' and 'refusal'."),
                "type": "invalid_request_error",
                "param": format!("input[{message_index}].content[{content_index}]"),
                "code": "invalid_value"
            }
        })),
    )
        .into_response())
}

async fn maybe_forced_upstream_delay(body: &Value) {
    let Some(delay_ms) = body.get("force_upstream_delay_ms").and_then(|v| v.as_u64()) else {
        return;
    };
    tokio::time::sleep(Duration::from_millis(delay_ms)).await;
}

async fn start_upstream() -> (SocketAddr, Arc<Mutex<Vec<(String, String)>>>) {
    let captured_headers: Arc<Mutex<Vec<(String, String)>>> = Arc::new(Mutex::new(Vec::new()));
    async fn responses(
        axum::extract::State(captured_headers): axum::extract::State<
            Arc<Mutex<Vec<(String, String)>>>,
        >,
        headers: axum::http::HeaderMap,
        Json(body): Json<Value>,
    ) -> impl axum::response::IntoResponse {
        if let Some(v) = headers
            .get("anthropic-version")
            .and_then(|h| h.to_str().ok())
        {
            if let Ok(mut lock) = captured_headers.lock() {
                lock.push(("anthropic-version".to_string(), v.to_string()));
            }
        }
        if let Some(v) = headers.get("x-goog-api-key").and_then(|h| h.to_str().ok()) {
            if let Ok(mut lock) = captured_headers.lock() {
                lock.push(("x-goog-api-key".to_string(), v.to_string()));
            }
        }
        if let Some(resp) = maybe_forced_upstream_error(&body) {
            return resp;
        }
        if let Some(resp) = maybe_reasoning_summary_validation_error(&body) {
            return resp;
        }
        if let Some(resp) = maybe_assistant_output_content_validation_error(&body) {
            return resp;
        }
        maybe_forced_upstream_delay(&body).await;
        let model = body.get("model").and_then(|v| v.as_str()).unwrap_or("mock");
        let text = collect_responses_text(body.get("input")) + &echo_suffix(&body);
        let input = body.get("input");
        let tools_present = body
            .get("tools")
            .and_then(|v| v.as_array())
            .map(|a| !a.is_empty())
            .unwrap_or(false);
        let parallel = body
            .get("parallel_tool_calls")
            .and_then(|v| v.as_bool())
            .unwrap_or(false);
        let reasoning_enabled = body
            .get("reasoning")
            .and_then(|v| v.get("effort"))
            .and_then(|v| v.as_str())
            .is_some();
        let emit_usage = body
            .get("emit_usage")
            .and_then(|v| v.as_bool())
            .unwrap_or(false);

        let mut tool_outputs: Vec<String> = Vec::new();
        if let Some(arr) = input.and_then(|v| v.as_array()) {
            for item in arr {
                if item.get("type").and_then(|v| v.as_str()) == Some("function_call_output") {
                    if let Some(output) = item.get("output") {
                        let summary = summarize_multipart_content(output);
                        if !summary.is_empty() {
                            tool_outputs.push(summary);
                        }
                    }
                }
            }
        }

        if body.get("stream").and_then(|v| v.as_bool()) == Some(true) {
            // If tools are present and no tool outputs were provided yet, stream a tool call.
            if tools_present && tool_outputs.is_empty() {
                if body.get("stream_mode").and_then(|v| v.as_str()) == Some("reasoning_text_tool") {
                    let stream = futures_util::stream::iter(vec![
                        Ok::<_, Infallible>(Event::default()
                            .event("response.output_item.added")
                            .data(json!({
                                "type": "response.output_item.added",
                                "output_index": 0,
                                "item": { "type": "reasoning", "id": "rs_mock", "summary": [{ "type": "summary_text", "text": "" }], "text": "" }
                            }).to_string())),
                        Ok::<_, Infallible>(Event::default()
                            .event("response.reasoning_summary_part.added")
                            .data(json!({
                                "type": "response.reasoning_summary_part.added",
                                "output_index": 0,
                                "item_id": "rs_mock",
                                "summary_index": 0,
                                "part": { "type": "summary_text", "text": "" }
                            }).to_string())),
                        Ok::<_, Infallible>(Event::default()
                            .event("response.reasoning_summary_text.delta")
                            .data(json!({
                                "type": "response.reasoning_summary_text.delta",
                                "output_index": 0,
                                "item_id": "rs_mock",
                                "summary_index": 0,
                                "delta": "mock_summary"
                            }).to_string())),
                        Ok::<_, Infallible>(Event::default()
                            .event("response.reasoning_summary_text.done")
                            .data(json!({
                                "type": "response.reasoning_summary_text.done",
                                "output_index": 0,
                                "item_id": "rs_mock",
                                "summary_index": 0,
                                "text": "mock_summary"
                            }).to_string())),
                        Ok::<_, Infallible>(Event::default()
                            .event("response.reasoning_summary_part.done")
                            .data(json!({
                                "type": "response.reasoning_summary_part.done",
                                "output_index": 0,
                                "item_id": "rs_mock",
                                "summary_index": 0,
                                "part": { "type": "summary_text", "text": "mock_summary" }
                            }).to_string())),
                        Ok::<_, Infallible>(Event::default()
                            .event("response.output_item.done")
                            .data(json!({
                                "type": "response.output_item.done",
                                "output_index": 0,
                                "item": { "type": "reasoning", "id": "rs_mock", "summary": [{ "type": "summary_text", "text": "mock_summary" }], "text": "mock_reasoning", "encrypted_content": "mock_sig" }
                            }).to_string())),
                        Ok::<_, Infallible>(Event::default()
                            .event("response.output_item.added")
                            .data(json!({
                                "type": "response.output_item.added",
                                "output_index": 1,
                                "item": { "type": "message", "role": "assistant", "phase": "analysis", "content": [] }
                            }).to_string())),
                        Ok::<_, Infallible>(Event::default()
                            .event("response.content_part.added")
                            .data(json!({
                                "type": "response.content_part.added",
                                "output_index": 1,
                                "content_index": 0,
                                "item_id": "msg_mock",
                                "part": { "type": "output_text", "text": "", "annotations": [] }
                            }).to_string())),
                        Ok::<_, Infallible>(Event::default()
                            .event("response.output_text.delta")
                            .data(json!({
                                "type": "response.output_text.delta",
                                "output_index": 1,
                                "content_index": 0,
                                "item_id": "msg_mock",
                                "logprobs": Value::Null,
                                "delta": "answer"
                            }).to_string())),
                        Ok::<_, Infallible>(Event::default()
                            .event("response.output_text.done")
                            .data(json!({
                                "type": "response.output_text.done",
                                "output_index": 1,
                                "content_index": 0,
                                "item_id": "msg_mock",
                                "logprobs": Value::Null,
                                "text": "answer"
                            }).to_string())),
                        Ok::<_, Infallible>(Event::default()
                            .event("response.content_part.done")
                            .data(json!({
                                "type": "response.content_part.done",
                                "output_index": 1,
                                "content_index": 0,
                                "item_id": "msg_mock",
                                "part": { "type": "output_text", "text": "answer", "annotations": [] }
                            }).to_string())),
                        Ok::<_, Infallible>(Event::default()
                            .event("response.output_item.done")
                            .data(json!({
                                "type": "response.output_item.done",
                                "output_index": 1,
                                "item": {
                                    "type": "message",
                                    "id": "msg_mock",
                                    "role": "assistant",
                                    "phase": "analysis",
                                    "content": [{ "type": "output_text", "text": "answer" }]
                                }
                            }).to_string())),
                        Ok::<_, Infallible>(Event::default()
                            .event("response.output_item.added")
                            .data(json!({
                                "type": "response.output_item.added",
                                "output_index": 2,
                                "item": { "type": "function_call", "call_id": "call_1", "name": "tool_a", "arguments": "" }
                            }).to_string())),
                        Ok::<_, Infallible>(Event::default()
                            .event("response.function_call_arguments.delta")
                            .data(json!({
                                "type": "response.function_call_arguments.delta",
                                "output_index": 2,
                                "delta": "{\"a\":1}"
                            }).to_string())),
                        Ok::<_, Infallible>(Event::default()
                            .event("response.function_call_arguments.done")
                            .data(json!({
                                "type": "response.function_call_arguments.done",
                                "output_index": 2,
                                "item_id": "fc_mock",
                                "call_id": "call_1",
                                "name": "tool_a",
                                "arguments": "{\"a\":1}"
                            }).to_string())),
                        Ok::<_, Infallible>(Event::default()
                            .event("response.output_item.done")
                            .data(json!({
                                "type": "response.output_item.done",
                                "output_index": 2,
                                "item": { "type": "function_call", "id": "fc_mock", "call_id": "call_1", "name": "tool_a", "arguments": "{\"a\":1}" }
                            }).to_string())),
                        Ok::<_, Infallible>(Event::default().data("[DONE]")),
                    ]);
                    return Sse::new(stream).into_response();
                }
                if body.get("stream_mode").and_then(|v| v.as_str()) == Some("completed_only_tool") {
                    let calls = if parallel {
                        vec![
                            json!({ "type": "function_call", "call_id": "call_1", "name": "tool_a", "arguments": "{\"a\":1}" }),
                            json!({ "type": "function_call", "call_id": "call_2", "name": "tool_b", "arguments": "{\"b\":2}" }),
                        ]
                    } else {
                        vec![
                            json!({ "type": "function_call", "call_id": "call_1", "name": "tool_a", "arguments": "{\"a\":1}" }),
                        ]
                    };
                    let stream = futures_util::stream::iter(vec![
                        Ok::<_, Infallible>(
                            Event::default().event("response.completed").data(
                                json!({
                                    "type": "response.completed",
                                    "response": {
                                        "id": "resp_mock",
                                        "object": "response",
                                        "created_at": 0,
                                        "model": model,
                                        "status": "completed",
                                        "output": calls
                                    }
                                })
                                .to_string(),
                            ),
                        ),
                        Ok::<_, Infallible>(Event::default().data("[DONE]")),
                    ]);
                    return Sse::new(stream).into_response();
                }
                if body.get("stream_mode").and_then(|v| v.as_str())
                    == Some("message_then_tool_then_completed")
                {
                    let stream = futures_util::stream::iter(vec![
                        Ok::<_, Infallible>(
                            Event::default().event("response.output_item.added").data(
                                json!({
                                    "type": "response.output_item.added",
                                    "output_index": 0,
                                    "item": {
                                        "type": "message",
                                        "id": "msg_mock",
                                        "role": "assistant",
                                        "phase": "commentary",
                                        "content": []
                                    }
                                })
                                .to_string(),
                            ),
                        ),
                        Ok::<_, Infallible>(
                            Event::default().event("response.content_part.added").data(
                                json!({
                                    "type": "response.content_part.added",
                                    "output_index": 0,
                                    "content_index": 0,
                                    "item_id": "msg_mock",
                                    "part": { "type": "output_text", "text": "", "annotations": [] }
                                })
                                .to_string(),
                            ),
                        ),
                        Ok::<_, Infallible>(
                            Event::default().event("response.output_text.delta").data(
                                json!({
                                    "type": "response.output_text.delta",
                                    "output_index": 0,
                                    "content_index": 0,
                                    "item_id": "msg_mock",
                                    "logprobs": Value::Null,
                                    "delta": "Searching"
                                })
                                .to_string(),
                            ),
                        ),
                        Ok::<_, Infallible>(
                            Event::default().event("response.output_text.done").data(
                                json!({
                                    "type": "response.output_text.done",
                                    "output_index": 0,
                                    "content_index": 0,
                                    "item_id": "msg_mock",
                                    "logprobs": Value::Null,
                                    "text": "Searching"
                                })
                                .to_string(),
                            ),
                        ),
                        Ok::<_, Infallible>(
                            Event::default().event("response.content_part.done").data(
                                json!({
                                    "type": "response.content_part.done",
                                    "output_index": 0,
                                    "content_index": 0,
                                    "item_id": "msg_mock",
                                    "part": {
                                        "type": "output_text",
                                        "text": "Searching",
                                        "annotations": []
                                    }
                                })
                                .to_string(),
                            ),
                        ),
                        Ok::<_, Infallible>(
                            Event::default().event("response.output_item.done").data(
                                json!({
                                    "type": "response.output_item.done",
                                    "output_index": 0,
                                    "item": {
                                        "type": "message",
                                        "id": "msg_mock",
                                        "role": "assistant",
                                        "phase": "commentary",
                                        "content": [{ "type": "output_text", "text": "Searching" }]
                                    }
                                })
                                .to_string(),
                            ),
                        ),
                        Ok::<_, Infallible>(
                            Event::default().event("response.output_item.added").data(
                                json!({
                                    "type": "response.output_item.added",
                                    "output_index": 1,
                                    "item": {
                                        "type": "function_call",
                                        "id": "fc_mock",
                                        "call_id": "call_1",
                                        "name": "tool_a",
                                        "arguments": ""
                                    }
                                })
                                .to_string(),
                            ),
                        ),
                        Ok::<_, Infallible>(
                            Event::default().event("response.function_call_arguments.delta").data(
                                json!({
                                    "type": "response.function_call_arguments.delta",
                                    "output_index": 1,
                                    "item_id": "fc_mock",
                                    "delta": "{\"a\":1}"
                                })
                                .to_string(),
                            ),
                        ),
                        Ok::<_, Infallible>(
                            Event::default().event("response.function_call_arguments.done").data(
                                json!({
                                    "type": "response.function_call_arguments.done",
                                    "output_index": 1,
                                    "item_id": "fc_mock",
                                    "call_id": "call_1",
                                    "name": "tool_a",
                                    "arguments": "{\"a\":1}"
                                })
                                .to_string(),
                            ),
                        ),
                        Ok::<_, Infallible>(
                            Event::default().event("response.output_item.done").data(
                                json!({
                                    "type": "response.output_item.done",
                                    "output_index": 1,
                                    "item": {
                                        "type": "function_call",
                                        "id": "fc_mock",
                                        "call_id": "call_1",
                                        "name": "tool_a",
                                        "arguments": "{\"a\":1}"
                                    }
                                })
                                .to_string(),
                            ),
                        ),
                        Ok::<_, Infallible>(
                            Event::default().event("response.completed").data(
                                json!({
                                    "type": "response.completed",
                                    "response": {
                                        "id": "resp_mock",
                                        "object": "response",
                                        "created_at": 0,
                                        "model": model,
                                        "status": "completed",
                                        "output": [
                                            {
                                                "type": "message",
                                                "id": "msg_mock",
                                                "role": "assistant",
                                                "phase": "commentary",
                                                "content": [{ "type": "output_text", "text": "Searching" }]
                                            },
                                            {
                                                "type": "function_call",
                                                "id": "fc_mock",
                                                "call_id": "call_1",
                                                "name": "tool_a",
                                                "arguments": "{\"a\":1}"
                                            }
                                        ]
                                    }
                                })
                                .to_string(),
                            ),
                        ),
                        Ok::<_, Infallible>(Event::default().data("[DONE]")),
                    ]);
                    return Sse::new(stream).into_response();
                }

                let mut events: Vec<Result<Event, Infallible>> = Vec::new();
                events.push(Ok(Event::default()
                    .event("response.output_item.added")
                    .data(json!({
                        "type": "response.output_item.added",
                        "output_index": 0,
                        "item": { "type": "reasoning", "id": "rs_mock", "summary": [{ "type": "summary_text", "text": "" }], "text": "" }
                    }).to_string())));
                events.push(Ok(Event::default()
                    .event("response.reasoning_summary_part.added")
                    .data(json!({ "type": "response.reasoning_summary_part.added", "item_id": "rs_mock", "output_index": 0, "summary_index": 0, "part": { "type": "summary_text", "text": "" } }).to_string())));
                events.push(Ok(Event::default()
                    .event("response.reasoning_summary_text.delta")
                    .data(json!({ "type": "response.reasoning_summary_text.delta", "item_id": "rs_mock", "output_index": 0, "summary_index": 0, "delta": "mock_summary" }).to_string())));
                events.push(Ok(Event::default()
                    .event("response.reasoning_summary_text.done")
                    .data(json!({ "type": "response.reasoning_summary_text.done", "item_id": "rs_mock", "output_index": 0, "summary_index": 0, "text": "mock_summary" }).to_string())));
                events.push(Ok(Event::default()
                    .event("response.reasoning_summary_part.done")
                    .data(json!({ "type": "response.reasoning_summary_part.done", "item_id": "rs_mock", "output_index": 0, "summary_index": 0, "part": { "type": "summary_text", "text": "mock_summary" } }).to_string())));
                events.push(Ok(Event::default()
                    .event("response.reasoning.delta")
                    .data(json!({ "type": "response.reasoning.delta", "item_id": "rs_mock", "output_index": 0, "delta": "mock_reasoning" }).to_string())));
                events.push(Ok(Event::default()
                    .event("response.reasoning.done")
                    .data(json!({ "type": "response.reasoning.done", "item_id": "rs_mock", "output_index": 0, "text": "mock_reasoning" }).to_string())));
                events.push(Ok(Event::default()
                    .event("response.output_item.done")
                    .data(
                        json!({
                            "type": "response.output_item.done",
                            "output_index": 0,
                            "item": {
                                "type": "reasoning",
                                "id": "rs_mock",
                                "summary": [{ "type": "summary_text", "text": "mock_summary" }],
                                "text": "mock_reasoning",
                                "encrypted_content": "mock_sig"
                            }
                        })
                        .to_string(),
                    )));

                let calls = if parallel {
                    vec![
                        ("call_1", "tool_a", "{\"a\":1}"),
                        ("call_2", "tool_b", "{\"b\":2}"),
                    ]
                } else {
                    vec![("call_1", "tool_a", "{\"a\":1}")]
                };
                for (idx, (call_id, name, args)) in calls.into_iter().enumerate() {
                    events.push(Ok(Event::default()
                        .event("response.output_item.added")
                        .data(json!({
                            "type": "response.output_item.added",
                            "output_index": idx + 1,
                            "item": { "type": "function_call", "call_id": call_id, "name": name, "arguments": "" }
                        }).to_string())));
                    events.push(Ok(Event::default()
                        .event("response.function_call_arguments.delta")
                        .data(
                            json!({
                                "type": "response.function_call_arguments.delta",
                                "output_index": idx + 1,
                                "delta": args
                            })
                            .to_string(),
                        )));
                    events.push(Ok(Event::default()
                        .event("response.function_call_arguments.done")
                        .data(
                            json!({
                                "type": "response.function_call_arguments.done",
                                "output_index": idx + 1,
                                "item_id": format!("fc_{}", idx + 1),
                                "call_id": call_id,
                                "name": name,
                                "arguments": args
                            })
                            .to_string(),
                        )));
                    events.push(Ok(Event::default()
                        .event("response.output_item.done")
                        .data(json!({
                            "type": "response.output_item.done",
                            "output_index": idx + 1,
                            "item": { "type": "function_call", "id": format!("fc_{}", idx + 1), "call_id": call_id, "name": name, "arguments": args }
                        }).to_string())));
                }
                events.push(Ok(Event::default().data("[DONE]")));
                return Sse::new(futures_util::stream::iter(events)).into_response();
            }

            if body.get("stream_mode").and_then(|v| v.as_str()) == Some("item_done_only") {
                let message_phase = body
                    .get("message_phase")
                    .and_then(|v| v.as_str())
                    .map(|s| s.to_string());
                let stream = futures_util::stream::iter(vec![
                    Ok::<_, Infallible>(
                        Event::default().event("response.output_item.added").data(
                            json!({
                                "type": "response.output_item.added",
                                "output_index": 0,
                                "item": {
                                    "type": "message",
                                    "role": "assistant",
                                    "phase": message_phase,
                                    "content": []
                                }
                            })
                            .to_string(),
                        ),
                    ),
                    Ok::<_, Infallible>(
                        Event::default().event("response.output_item.done").data(
                            json!({
                                "type": "response.output_item.done",
                                "output_index": 0,
                                "item": {
                                    "type": "message",
                                    "role": "assistant",
                                    "phase": message_phase,
                                    "content": [{ "type": "output_text", "text": text }]
                                }
                            })
                            .to_string(),
                        ),
                    ),
                    Ok::<_, Infallible>(Event::default().data("[DONE]")),
                ]);
                return Sse::new(stream).into_response();
            }

            let mut events = Vec::new();
            if reasoning_enabled {
                events.push(Ok::<_, Infallible>(
                    Event::default()
                        .event("response.reasoning.delta")
                        .data(json!({ "delta": "mock_reasoning" }).to_string()),
                ));
            }
            events.push(Ok::<_, Infallible>(
                Event::default()
                    .event("response.output_text.delta")
                    .data(json!({ "delta": text }).to_string()),
            ));
            if emit_usage {
                events.push(Ok::<_, Infallible>(
                    Event::default().event("response.completed").data(
                        json!({
                            "type": "response.completed",
                            "response": {
                                "id": "resp_mock",
                                "object": "response",
                                "created_at": 0,
                                "model": model,
                                "status": "completed",
                                "output": [],
                                "usage": {
                                    "input_tokens": 12,
                                    "output_tokens": 8,
                                    "total_tokens": 20,
                                    "input_tokens_details": { "cached_tokens": 0 },
                                    "output_tokens_details": { "reasoning_tokens": 0 }
                                }
                            }
                        })
                        .to_string(),
                    ),
                ));
            }
            events.push(Ok::<_, Infallible>(Event::default().data("[DONE]")));
            return Sse::new(futures_util::stream::iter(events)).into_response();
        }

        if tools_present && tool_outputs.is_empty() {
            let calls = if parallel {
                vec![
                    json!({ "type": "function_call", "call_id": "call_1", "name": "tool_a", "arguments": "{\"a\":1}" }),
                    json!({ "type": "function_call", "call_id": "call_2", "name": "tool_b", "arguments": "{\"b\":2}" }),
                ]
            } else {
                vec![
                    json!({ "type": "function_call", "call_id": "call_1", "name": "tool_a", "arguments": "{\"a\":1}" }),
                ]
            };
            let mut output = vec![
                json!({ "type": "reasoning", "text": "mock_reasoning", "encrypted_content": "mock_sig" }),
            ];
            output.extend(calls);
            return Json(json!({
                "id": "resp_mock",
                "object": "response",
                "created_at": 0,
                "model": model,
                "status": "completed",
                "output": output
            }))
            .into_response();
        }

        if !tool_outputs.is_empty() {
            let joined = tool_outputs.join("|");
            return Json(json!({
                "id": "resp_mock",
                "object": "response",
                "created_at": 0,
                "model": model,
                "status": "completed",
                "output": [{
                    "type": "message",
                    "role": "assistant",
                    "content": [{ "type": "output_text", "text": format!("tool_ok:{joined}") }]
                }]
            }))
            .into_response();
        }

        let mut output = Vec::new();
        if reasoning_enabled {
            output.push(
                json!({ "type": "reasoning", "text": "mock_reasoning", "encrypted_content": "mock_sig" }),
            );
        }
        output.push(json!({
            "type": "message",
            "role": "assistant",
            "content": [{ "type": "output_text", "text": text }]
        }));
        Json(json!({
            "id": "resp_mock",
            "object": "response",
            "created_at": 0,
            "model": model,
            "status": "completed",
            "output": output
        }))
        .into_response()
    }

    async fn chat(
        axum::extract::State(captured_headers): axum::extract::State<
            Arc<Mutex<Vec<(String, String)>>>,
        >,
        headers: axum::http::HeaderMap,
        Json(body): Json<Value>,
    ) -> impl axum::response::IntoResponse {
        if let Some(v) = headers
            .get("anthropic-version")
            .and_then(|h| h.to_str().ok())
        {
            if let Ok(mut lock) = captured_headers.lock() {
                lock.push(("anthropic-version".to_string(), v.to_string()));
            }
        }
        if let Some(v) = headers.get("x-goog-api-key").and_then(|h| h.to_str().ok()) {
            if let Ok(mut lock) = captured_headers.lock() {
                lock.push(("x-goog-api-key".to_string(), v.to_string()));
            }
        }
        if let Some(resp) = maybe_forced_upstream_error(&body) {
            return resp;
        }
        maybe_forced_upstream_delay(&body).await;
        let model = body.get("model").and_then(|v| v.as_str()).unwrap_or("mock");
        let messages = body
            .get("messages")
            .and_then(|v| v.as_array())
            .cloned()
            .unwrap_or_default();
        let text = collect_chat_text(&messages) + &echo_suffix(&body);
        let tools_present = body
            .get("tools")
            .and_then(|v| v.as_array())
            .map(|a| !a.is_empty())
            .unwrap_or(false);
        let parallel = body
            .get("parallel_tool_calls")
            .and_then(|v| v.as_bool())
            .unwrap_or(false);
        let reasoning_enabled = body
            .get("reasoning_effort")
            .and_then(|v| v.as_str())
            .is_some();
        let reasoning_source_override = body
            .get("reasoning_source_override")
            .and_then(|v| v.as_str())
            .filter(|value| !value.is_empty());
        let omit_reasoning_source = body
            .get("omit_reasoning_source")
            .and_then(|v| v.as_bool())
            .unwrap_or(false);
        let reasoning_format = if omit_reasoning_source {
            None
        } else {
            reasoning_source_override.or(Some("openrouter"))
        };
        let reasoning_summary_detail = |summary: &str| {
            let mut detail = json!({
                "type": "reasoning.summary",
                "summary": summary,
            });
            if let Some(format) = reasoning_format {
                detail["format"] = json!(format);
            }
            detail
        };
        let reasoning_text_detail = |text: &str| {
            let mut detail = json!({
                "type": "reasoning.text",
                "text": text,
            });
            if let Some(format) = reasoning_format {
                detail["format"] = json!(format);
            }
            detail
        };
        let reasoning_encrypted_detail = |data: &str| {
            let mut detail = json!({
                "type": "reasoning.encrypted",
                "data": data,
            });
            if let Some(format) = reasoning_format {
                detail["format"] = json!(format);
            }
            detail
        };
        let emit_usage = body
            .get("emit_usage")
            .and_then(|v| v.as_bool())
            .unwrap_or(false)
            || body
                .get("stream_options")
                .and_then(|v| v.get("include_usage"))
                .and_then(|v| v.as_bool())
                .unwrap_or(false);
        let finish_reason = body
            .get("force_finish_reason")
            .and_then(|v| v.as_str())
            .unwrap_or("stop");
        let mut tool_outputs: Vec<String> = Vec::new();
        for m in &messages {
            if m.get("role").and_then(|v| v.as_str()) == Some("tool") {
                if let Some(c) = m.get("content").and_then(|v| v.as_str()) {
                    tool_outputs.push(c.to_string());
                }
            }
        }

        if body.get("stream").and_then(|v| v.as_bool()) == Some(true) {
            if tools_present && tool_outputs.is_empty() {
                if body.get("stream_mode").and_then(|v| v.as_str()) == Some("header_only_tool") {
                    let chunks: Vec<Result<Event, Infallible>> = vec![
                        Ok(Event::default().data(json!({
                            "id": "chatcmpl_mock",
                            "object": "chat.completion.chunk",
                            "created": 0,
                            "model": model,
                            "choices": [{ "index": 0, "delta": { "role": "assistant", "content": Value::Null }, "finish_reason": Value::Null }]
                        }).to_string())),
                        Ok(Event::default().data(json!({
                            "id": "chatcmpl_mock",
                            "object": "chat.completion.chunk",
                            "created": 0,
                            "model": model,
                            "choices": [{
                                "index": 0,
                                "delta": {
                                    "tool_calls": [{
                                        "index": 0,
                                        "id": "call_empty",
                                        "type": "function",
                                        "function": { "name": "tool_empty", "arguments": "" }
                                    }]
                                },
                                "finish_reason": Value::Null
                            }]
                        }).to_string())),
                        Ok(Event::default().data(json!({
                            "id": "chatcmpl_mock",
                            "object": "chat.completion.chunk",
                            "created": 0,
                            "model": model,
                            "choices": [{ "index": 0, "delta": {}, "finish_reason": "stop" }]
                        }).to_string())),
                        Ok(Event::default().data("[DONE]")),
                    ];
                    return Sse::new(futures_util::stream::iter(chunks)).into_response();
                }
                if body.get("stream_mode").and_then(|v| v.as_str()) == Some("reasoning_text_tool") {
                    let mut chunks: Vec<Result<Event, Infallible>> = Vec::new();
                    chunks.push(Ok(Event::default().data(json!({
                        "id": "chatcmpl_mock",
                        "object": "chat.completion.chunk",
                        "created": 0,
                        "model": model,
                        "choices": [{ "index": 0, "delta": { "role": "assistant", "content": Value::Null }, "finish_reason": Value::Null }]
                    }).to_string())));
                    chunks.push(Ok(Event::default().data(json!({
                        "id": "chatcmpl_mock",
                        "object": "chat.completion.chunk",
                        "created": 0,
                        "model": model,
                        "choices": [{ "index": 0, "delta": { "reasoning_details": [reasoning_summary_detail("mock_summary")] }, "finish_reason": Value::Null }]
                    }).to_string())));
                    chunks.push(Ok(Event::default().data(json!({
                        "id": "chatcmpl_mock",
                        "object": "chat.completion.chunk",
                        "created": 0,
                        "model": model,
                        "choices": [{ "index": 0, "delta": { "reasoning_details": [reasoning_text_detail("mock_reasoning"), reasoning_encrypted_detail("mock_sig")] }, "finish_reason": Value::Null }]
                    }).to_string())));
                    chunks.push(Ok(Event::default().data(json!({
                        "id": "chatcmpl_mock",
                        "object": "chat.completion.chunk",
                        "created": 0,
                        "model": model,
                        "choices": [{ "index": 0, "delta": { "content": "answer" }, "finish_reason": Value::Null }]
                    }).to_string())));
                    chunks.push(Ok(Event::default().data(
                        json!({
                            "id": "chatcmpl_mock",
                            "object": "chat.completion.chunk",
                            "created": 0,
                            "model": model,
                            "choices": [{
                                "index": 0,
                                "delta": {
                                    "tool_calls": [{
                                        "index": 0,
                                        "id": "call_1",
                                        "type": "function",
                                        "function": { "name": "tool_a", "arguments": "" }
                                    }]
                                },
                                "finish_reason": Value::Null
                            }]
                        })
                        .to_string(),
                    )));
                    chunks.push(Ok(Event::default().data(
                        json!({
                            "id": "chatcmpl_mock",
                            "object": "chat.completion.chunk",
                            "created": 0,
                            "model": model,
                            "choices": [{
                                "index": 0,
                                "delta": {
                                    "tool_calls": [{
                                        "index": 0,
                                        "function": { "arguments": "{\"a\":1}" }
                                    }]
                                },
                                "finish_reason": Value::Null
                            }]
                        })
                        .to_string(),
                    )));
                    chunks.push(Ok(Event::default().data(
                        json!({
                            "id": "chatcmpl_mock",
                            "object": "chat.completion.chunk",
                            "created": 0,
                            "model": model,
                            "choices": [{ "index": 0, "delta": {}, "finish_reason": "tool_calls" }]
                        })
                        .to_string(),
                    )));
                    chunks.push(Ok(Event::default().data("[DONE]")));
                    return Sse::new(futures_util::stream::iter(chunks)).into_response();
                }
                if body.get("stream_mode").and_then(|v| v.as_str()) == Some("content_array_tool") {
                    let chunks: Vec<Result<Event, Infallible>> = vec![
                        Ok(Event::default().data(json!({
                            "id": "chatcmpl_mock",
                            "object": "chat.completion.chunk",
                            "created": 0,
                            "model": model,
                            "choices": [{ "index": 0, "delta": { "role": "assistant", "content": Value::Null }, "finish_reason": Value::Null }]
                        }).to_string())),
                        Ok(Event::default().data(json!({
                            "id": "chatcmpl_mock",
                            "object": "chat.completion.chunk",
                            "created": 0,
                            "model": model,
                            "choices": [{
                                "index": 0,
                                "delta": {
                                    "content": [
                                        { "type": "text", "text": "answer" },
                                        { "type": "tool_call", "id": "call_1", "name": "tool_a", "arguments": "" }
                                    ]
                                },
                                "finish_reason": Value::Null
                            }]
                        }).to_string())),
                        Ok(Event::default().data(json!({
                            "id": "chatcmpl_mock",
                            "object": "chat.completion.chunk",
                            "created": 0,
                            "model": model,
                            "choices": [{
                                "index": 0,
                                "delta": {
                                    "content": [
                                        { "type": "tool_call", "id": "call_1", "name": "tool_a", "arguments": "{\"a\":1}" }
                                    ]
                                },
                                "finish_reason": Value::Null
                            }]
                        }).to_string())),
                        Ok(Event::default().data(json!({
                            "id": "chatcmpl_mock",
                            "object": "chat.completion.chunk",
                            "created": 0,
                            "model": model,
                            "choices": [{ "index": 0, "delta": {}, "finish_reason": "stop" }]
                        }).to_string())),
                        Ok(Event::default().data("[DONE]")),
                    ];
                    return Sse::new(futures_util::stream::iter(chunks)).into_response();
                }
                if body.get("stream_mode").and_then(|v| v.as_str())
                    == Some("content_array_tool_use")
                {
                    let chunks: Vec<Result<Event, Infallible>> = vec![
                        Ok(Event::default().data(json!({
                            "id": "chatcmpl_mock",
                            "object": "chat.completion.chunk",
                            "created": 0,
                            "model": model,
                            "choices": [{ "index": 0, "delta": { "role": "assistant", "content": Value::Null }, "finish_reason": Value::Null }]
                        }).to_string())),
                        Ok(Event::default().data(json!({
                            "id": "chatcmpl_mock",
                            "object": "chat.completion.chunk",
                            "created": 0,
                            "model": model,
                            "choices": [{
                                "index": 0,
                                "delta": {
                                    "content": [
                                        { "type": "text", "text": "answer" },
                                        { "type": "tool_use", "id": "call_1", "name": "tool_a", "input": {} }
                                    ]
                                },
                                "finish_reason": Value::Null
                            }]
                        }).to_string())),
                        Ok(Event::default().data(json!({
                            "id": "chatcmpl_mock",
                            "object": "chat.completion.chunk",
                            "created": 0,
                            "model": model,
                            "choices": [{
                                "index": 0,
                                "delta": {
                                    "content": [
                                        { "type": "tool_use", "id": "call_1", "name": "tool_a", "input": { "a": 1 } }
                                    ]
                                },
                                "finish_reason": Value::Null
                            }]
                        }).to_string())),
                        Ok(Event::default().data(json!({
                            "id": "chatcmpl_mock",
                            "object": "chat.completion.chunk",
                            "created": 0,
                            "model": model,
                            "choices": [{ "index": 0, "delta": {}, "finish_reason": "stop" }]
                        }).to_string())),
                        Ok(Event::default().data("[DONE]")),
                    ];
                    return Sse::new(futures_util::stream::iter(chunks)).into_response();
                }
                let mut chunks: Vec<Result<Event, Infallible>> = Vec::new();
                // Initial role chunk (matches real OpenAI format)
                chunks.push(Ok(Event::default().data(json!({
                    "id": "chatcmpl_mock",
                    "object": "chat.completion.chunk",
                    "created": 0,
                    "model": model,
                    "choices": [{ "index": 0, "delta": { "role": "assistant", "content": Value::Null }, "finish_reason": Value::Null }]
                }).to_string())));
                chunks.push(Ok(Event::default().data(json!({
                    "id": "chatcmpl_mock",
                    "object": "chat.completion.chunk",
                    "created": 0,
                    "model": model,
                    "choices": [{ "index": 0, "delta": { "reasoning_details": [reasoning_summary_detail("mock_summary")] }, "finish_reason": Value::Null }]
                }).to_string())));
                chunks.push(Ok(Event::default().data(json!({
                    "id": "chatcmpl_mock",
                    "object": "chat.completion.chunk",
                    "created": 0,
                    "model": model,
                    "choices": [{ "index": 0, "delta": { "reasoning_details": [reasoning_text_detail("mock_reasoning"), reasoning_encrypted_detail("mock_sig")] }, "finish_reason": Value::Null }]
                }).to_string())));
                let calls: Vec<(usize, &str, &str, Vec<&str>)> = if parallel {
                    vec![
                        (0, "call_1", "tool_a", vec!["{\"a\"", ":1}"]),
                        (1, "call_2", "tool_b", vec!["{\"b\"", ":2}"]),
                    ]
                } else {
                    vec![(0, "call_1", "tool_a", vec!["{\"a\"", ":1}"])]
                };
                for (tc_idx, call_id, name, arg_fragments) in calls {
                    // Header chunk: has id, type, name, empty arguments (matches real OpenAI)
                    chunks.push(Ok(Event::default().data(
                        json!({
                            "id": "chatcmpl_mock",
                            "object": "chat.completion.chunk",
                            "created": 0,
                            "model": model,
                            "choices": [{
                                "index": 0,
                                "delta": {
                                    "tool_calls": [{
                                        "index": tc_idx,
                                        "id": call_id,
                                        "type": "function",
                                        "function": { "name": name, "arguments": "" }
                                    }]
                                },
                                "finish_reason": Value::Null
                            }]
                        })
                        .to_string(),
                    )));
                    // Continuation chunks: only index + arguments fragment (no id, no type, no name)
                    for frag in arg_fragments {
                        chunks.push(Ok(Event::default().data(
                            json!({
                                "id": "chatcmpl_mock",
                                "object": "chat.completion.chunk",
                                "created": 0,
                                "model": model,
                                "choices": [{
                                    "index": 0,
                                    "delta": {
                                        "tool_calls": [{
                                            "index": tc_idx,
                                            "function": { "arguments": frag }
                                        }]
                                    },
                                    "finish_reason": Value::Null
                                }]
                            })
                            .to_string(),
                        )));
                    }
                }
                // Terminal chunk: empty delta with finish_reason (matches real OpenAI)
                chunks.push(Ok(Event::default().data(
                    json!({
                        "id": "chatcmpl_mock",
                        "object": "chat.completion.chunk",
                        "created": 0,
                        "model": model,
                        "choices": [{ "index": 0, "delta": {}, "finish_reason": "tool_calls" }]
                    })
                    .to_string(),
                )));
                chunks.push(Ok(Event::default().data("[DONE]")));
                return Sse::new(futures_util::stream::iter(chunks)).into_response();
            }

            if !tool_outputs.is_empty() {
                let joined = tool_outputs.join("|");
                let chunk = json!({
                    "id": "chatcmpl_mock",
                    "object": "chat.completion.chunk",
                    "created": 0,
                    "model": model,
                    "choices": [{ "index": 0, "delta": { "content": format!("tool_ok:{joined}") }, "finish_reason": Value::Null }]
                });
                let stream = futures_util::stream::iter(vec![
                    Ok::<_, Infallible>(Event::default().data(chunk.to_string())),
                    Ok::<_, Infallible>(Event::default().data("[DONE]")),
                ]);
                return Sse::new(stream).into_response();
            }

            let chunk = json!({
                "id": "chatcmpl_mock",
                "object": "chat.completion.chunk",
                "created": 0,
                "model": model,
                "choices": [{ "index": 0, "delta": { "content": text }, "finish_reason": Value::Null }]
            });
            let mut chunks = Vec::new();
            if reasoning_enabled {
                chunks.push(Ok::<_, Infallible>(Event::default().data(json!({
                    "id": "chatcmpl_mock",
                    "object": "chat.completion.chunk",
                    "created": 0,
                    "model": model,
                    "choices": [{ "index": 0, "delta": { "reasoning_details": [reasoning_text_detail("mock_reasoning"), reasoning_encrypted_detail("mock_sig")] }, "finish_reason": Value::Null }]
                }).to_string())));
            }
            chunks.push(Ok::<_, Infallible>(
                Event::default().data(chunk.to_string()),
            ));
            // Real OpenAI always emits a terminal chunk with finish_reason
            // before [DONE]; usage is only included when stream_options.include_usage is set.
            let mut terminal = json!({
                "id": "chatcmpl_mock",
                "object": "chat.completion.chunk",
                "created": 0,
                "model": model,
                "choices": [{ "index": 0, "delta": {}, "finish_reason": finish_reason }]
            });
            if emit_usage {
                terminal["usage"] = json!({
                    "prompt_tokens": 12,
                    "completion_tokens": 8,
                    "total_tokens": 20,
                    "prompt_tokens_details": { "cached_tokens": 0 },
                    "completion_tokens_details": { "reasoning_tokens": 0 }
                });
            }
            chunks.push(Ok::<_, Infallible>(
                Event::default().data(terminal.to_string()),
            ));
            chunks.push(Ok::<_, Infallible>(Event::default().data("[DONE]")));
            let stream = futures_util::stream::iter(chunks);
            return Sse::new(stream).into_response();
        }

        if tools_present && tool_outputs.is_empty() {
            let calls = if parallel {
                vec![
                    json!({"id":"call_1","type":"function","function":{"name":"tool_a","arguments":"{\"a\":1}"}}),
                    json!({"id":"call_2","type":"function","function":{"name":"tool_b","arguments":"{\"b\":2}"}}),
                ]
            } else {
                vec![
                    json!({"id":"call_1","type":"function","function":{"name":"tool_a","arguments":"{\"a\":1}"}}),
                ]
            };
            return Json(json!({
                "id": "chatcmpl_mock",
                "object": "chat.completion",
                "created": 0,
                "model": model,
                "choices": [{
                    "index": 0,
                    "message": {
                        "role": "assistant",
                        "content": "",
                        "tool_calls": calls,
                        "reasoning": "mock_reasoning",
                        "reasoning_details": [reasoning_text_detail("mock_reasoning"), reasoning_encrypted_detail("mock_sig")]
                    },
                    "finish_reason": "tool_calls"
                }]
            }))
            .into_response();
        }

        if !tool_outputs.is_empty() {
            let joined = tool_outputs.join("|");
            return Json(json!({
                "id": "chatcmpl_mock",
                "object": "chat.completion",
                "created": 0,
                "model": model,
                "choices": [{
                    "index": 0,
                    "message": { "role": "assistant", "content": format!("tool_ok:{joined}") },
                    "finish_reason": "stop"
                }]
            }))
            .into_response();
        }

        let message = if reasoning_enabled {
            json!({
                "role": "assistant",
                "content": text,
                "reasoning": "mock_reasoning",
                "reasoning_details": [reasoning_text_detail("mock_reasoning"), reasoning_encrypted_detail("mock_sig")]
            })
        } else {
            json!({ "role": "assistant", "content": text })
        };
        Json(json!({
            "id": "chatcmpl_mock",
            "object": "chat.completion",
            "created": 0,
            "model": model,
            "choices": [{
                "index": 0,
                "message": message,
                "finish_reason": "stop"
            }]
        }))
        .into_response()
    }

    async fn messages(
        axum::extract::State(captured_headers): axum::extract::State<
            Arc<Mutex<Vec<(String, String)>>>,
        >,
        headers: axum::http::HeaderMap,
        Json(body): Json<Value>,
    ) -> impl axum::response::IntoResponse {
        if let Some(v) = headers
            .get("anthropic-version")
            .and_then(|h| h.to_str().ok())
        {
            if let Ok(mut lock) = captured_headers.lock() {
                lock.push(("anthropic-version".to_string(), v.to_string()));
            }
        }
        if let Some(v) = headers.get("x-goog-api-key").and_then(|h| h.to_str().ok()) {
            if let Ok(mut lock) = captured_headers.lock() {
                lock.push(("x-goog-api-key".to_string(), v.to_string()));
            }
        }
        if let Some(resp) = maybe_forced_upstream_error(&body) {
            return resp;
        }
        maybe_forced_upstream_delay(&body).await;
        let model = body.get("model").and_then(|v| v.as_str()).unwrap_or("mock");
        let messages = body
            .get("messages")
            .and_then(|v| v.as_array())
            .cloned()
            .unwrap_or_default();
        let text = collect_anthropic_text(&messages) + &echo_suffix(&body);
        let tools_present = body
            .get("tools")
            .and_then(|v| v.as_array())
            .map(|a| !a.is_empty())
            .unwrap_or(false);
        let parallel = body
            .get("parallel_tool_calls")
            .and_then(|v| v.as_bool())
            .unwrap_or(false);
        let thinking_enabled = body
            .get("thinking")
            .and_then(|v| v.get("type"))
            .and_then(|v| v.as_str())
            == Some("enabled");
        let mut tool_results: Vec<String> = Vec::new();
        for m in &messages {
            if m.get("role").and_then(|v| v.as_str()) != Some("user") {
                continue;
            }
            if let Some(arr) = m.get("content").and_then(|v| v.as_array()) {
                for b in arr {
                    if b.get("type").and_then(|v| v.as_str()) == Some("tool_result") {
                        if let Some(content) = b.get("content") {
                            let summary = summarize_multipart_content(content);
                            if !summary.is_empty() {
                                tool_results.push(summary);
                            }
                        }
                    }
                }
            }
        }

        if body.get("stream").and_then(|v| v.as_bool()) == Some(true) {
            if tools_present && tool_results.is_empty() {
                let mut events: Vec<Result<Event, Infallible>> = Vec::new();
                events.push(Ok(Event::default().data(json!({
                    "type": "message_start",
                    "message": { "id": "msg_mock", "type": "message", "role": "assistant", "model": model, "content": [] }
                }).to_string())));
                // thinking block
                events.push(Ok(Event::default().data(
                    json!({
                        "type": "content_block_start",
                        "index": 0,
                        "content_block": { "type": "thinking", "thinking": "", "signature": "" }
                    })
                    .to_string(),
                )));
                events.push(Ok(Event::default().data(
                    json!({
                        "type": "content_block_delta",
                        "index": 0,
                        "delta": { "type": "thinking_delta", "thinking": "mock_reasoning" }
                    })
                    .to_string(),
                )));
                events.push(Ok(Event::default().data(
                    json!({
                        "type": "content_block_delta",
                        "index": 0,
                        "delta": { "type": "signature_delta", "signature": "mock_sig" }
                    })
                    .to_string(),
                )));
                events.push(Ok(Event::default().data(
                    json!({ "type": "content_block_stop", "index": 0 }).to_string(),
                )));

                let calls = if parallel {
                    vec![
                        ("call_1", "tool_a", "{\"a\":1}"),
                        ("call_2", "tool_b", "{\"b\":2}"),
                    ]
                } else {
                    vec![("call_1", "tool_a", "{\"a\":1}")]
                };
                let mut idx = 1;
                for (call_id, name, args) in calls {
                    events.push(Ok(Event::default().data(json!({
                        "type": "content_block_start",
                        "index": idx,
                        "content_block": { "type": "tool_use", "id": call_id, "name": name, "input": {} }
                    }).to_string())));
                    events.push(Ok(Event::default().data(
                        json!({
                            "type": "content_block_delta",
                            "index": idx,
                            "delta": { "type": "input_json_delta", "partial_json": args }
                        })
                        .to_string(),
                    )));
                    events.push(Ok(Event::default().data(
                        json!({ "type": "content_block_stop", "index": idx }).to_string(),
                    )));
                    idx += 1;
                }
                events.push(Ok(
                    Event::default().data(json!({ "type": "message_stop" }).to_string())
                ));
                return Sse::new(futures_util::stream::iter(events)).into_response();
            }

            if !tool_results.is_empty() {
                let joined = tool_results.join("|");
                let stream = futures_util::stream::iter(vec![
                    Ok::<_, Infallible>(Event::default().data(json!({
                        "type": "message_start",
                        "message": { "id": "msg_mock", "type": "message", "role": "assistant", "model": model, "content": [] }
                    }).to_string())),
                    Ok::<_, Infallible>(Event::default().data(json!({
                        "type": "content_block_start",
                        "index": 0,
                        "content_block": { "type": "text", "text": "" }
                    }).to_string())),
                    Ok::<_, Infallible>(Event::default().data(json!({
                        "type": "content_block_delta",
                        "index": 0,
                        "delta": { "type": "text_delta", "text": format!("tool_ok:{joined}") }
                    }).to_string())),
                    Ok::<_, Infallible>(Event::default().data(json!({ "type": "content_block_stop", "index": 0 }).to_string())),
                    Ok::<_, Infallible>(Event::default().data(json!({ "type": "message_stop" }).to_string())),
                ]);
                return Sse::new(stream).into_response();
            }

            let mut events = Vec::new();
            events.push(Ok::<_, Infallible>(Event::default().data(json!({
                "type": "message_start",
                "message": { "id": "msg_mock", "type": "message", "role": "assistant", "model": model, "content": [] }
            }).to_string())));
            if thinking_enabled {
                events.push(Ok::<_, Infallible>(
                    Event::default().data(
                        json!({
                            "type": "content_block_start",
                            "index": 0,
                            "content_block": { "type": "thinking", "thinking": "", "signature": "" }
                        })
                        .to_string(),
                    ),
                ));
                events.push(Ok::<_, Infallible>(
                    Event::default().data(
                        json!({
                            "type": "content_block_delta",
                            "index": 0,
                            "delta": { "type": "thinking_delta", "thinking": "mock_reasoning" }
                        })
                        .to_string(),
                    ),
                ));
                events.push(Ok::<_, Infallible>(
                    Event::default().data(
                        json!({
                            "type": "content_block_delta",
                            "index": 0,
                            "delta": { "type": "signature_delta", "signature": "mock_sig" }
                        })
                        .to_string(),
                    ),
                ));
                events.push(Ok::<_, Infallible>(
                    Event::default().data(
                        json!({
                            "type": "content_block_stop",
                            "index": 0
                        })
                        .to_string(),
                    ),
                ));
                events.push(Ok::<_, Infallible>(
                    Event::default().data(
                        json!({
                            "type": "content_block_start",
                            "index": 1,
                            "content_block": { "type": "text", "text": "" }
                        })
                        .to_string(),
                    ),
                ));
                events.push(Ok::<_, Infallible>(
                    Event::default().data(
                        json!({
                            "type": "content_block_delta",
                            "index": 1,
                            "delta": { "type": "text_delta", "text": text }
                        })
                        .to_string(),
                    ),
                ));
                events.push(Ok::<_, Infallible>(
                    Event::default().data(
                        json!({
                            "type": "content_block_stop",
                            "index": 1
                        })
                        .to_string(),
                    ),
                ));
            } else {
                events.push(Ok::<_, Infallible>(
                    Event::default().data(
                        json!({
                            "type": "content_block_delta",
                            "index": 0,
                            "delta": { "type": "text_delta", "text": text }
                        })
                        .to_string(),
                    ),
                ));
            }
            events.push(Ok::<_, Infallible>(
                Event::default().data(json!({ "type": "message_stop" }).to_string()),
            ));
            let stream = futures_util::stream::iter(events);
            return Sse::new(stream).into_response();
        }

        if tools_present && tool_results.is_empty() {
            let blocks = if parallel {
                vec![
                    json!({ "type": "thinking", "thinking": "mock_reasoning", "signature": "mock_sig" }),
                    json!({ "type": "tool_use", "id": "call_1", "name": "tool_a", "input": { "a": 1 } }),
                    json!({ "type": "tool_use", "id": "call_2", "name": "tool_b", "input": { "b": 2 } }),
                ]
            } else {
                vec![
                    json!({ "type": "thinking", "thinking": "mock_reasoning", "signature": "mock_sig" }),
                    json!({ "type": "tool_use", "id": "call_1", "name": "tool_a", "input": { "a": 1 } }),
                ]
            };
            return Json(json!({
                "id": "msg_mock",
                "type": "message",
                "role": "assistant",
                "model": model,
                "content": blocks
            }))
            .into_response();
        }

        if !tool_results.is_empty() {
            let joined = tool_results.join("|");
            return Json(json!({
                "id": "msg_mock",
                "type": "message",
                "role": "assistant",
                "model": model,
                "content": [{ "type": "text", "text": format!("tool_ok:{joined}") }]
            }))
            .into_response();
        }

        let content = if thinking_enabled {
            json!([
                { "type": "thinking", "thinking": "mock_reasoning", "signature": "mock_sig" },
                { "type": "text", "text": text }
            ])
        } else {
            json!([{ "type": "text", "text": text }])
        };
        Json(json!({
            "id": "msg_mock",
            "type": "message",
            "role": "assistant",
            "model": model,
            "content": content
        }))
        .into_response()
    }

    async fn gemini_dispatch(
        axum::extract::State(captured_headers): axum::extract::State<
            Arc<Mutex<Vec<(String, String)>>>,
        >,
        axum::extract::Path(rest): axum::extract::Path<String>,
        headers: axum::http::HeaderMap,
        Json(body): Json<Value>,
    ) -> impl axum::response::IntoResponse {
        if let Some(v) = headers.get("x-goog-api-key").and_then(|h| h.to_str().ok()) {
            if let Ok(mut lock) = captured_headers.lock() {
                lock.push(("x-goog-api-key".to_string(), v.to_string()));
            }
        }
        if let Some(resp) = maybe_forced_upstream_error(&body) {
            return resp;
        }
        maybe_forced_upstream_delay(&body).await;
        let (model, stream_mode) = if let Some(model) = rest.strip_suffix(":generateContent") {
            (model.to_string(), false)
        } else if let Some(model) = rest.strip_suffix(":streamGenerateContent") {
            (model.to_string(), true)
        } else {
            return StatusCode::NOT_FOUND.into_response();
        };

        let text = body
            .get("contents")
            .and_then(|v| v.as_array())
            .cloned()
            .unwrap_or_default()
            .into_iter()
            .flat_map(|item| {
                item.get("parts")
                    .and_then(|v| v.as_array())
                    .cloned()
                    .unwrap_or_default()
            })
            .filter_map(|part| {
                part.get("text")
                    .and_then(|v| v.as_str())
                    .map(|s| s.to_string())
            })
            .collect::<Vec<_>>()
            .join("");

        if stream_mode {
            let event = json!({
                "candidates": [{
                    "content": {
                        "role": "model",
                        "parts": [{ "text": format!("{text}|gemini_stream") }]
                    },
                    "finishReason": "STOP"
                }],
                "modelVersion": model,
                "usageMetadata": {
                    "promptTokenCount": 1,
                    "candidatesTokenCount": 1,
                    "totalTokenCount": 2
                }
            });
            let stream = futures_util::stream::iter(vec![
                Ok::<_, Infallible>(Event::default().data(event.to_string())),
                Ok::<_, Infallible>(Event::default().data("[DONE]")),
            ]);
            Sse::new(stream).into_response()
        } else {
            Json(json!({
                "responseId": "gemini_mock",
                "modelVersion": model,
                "candidates": [{
                    "content": {
                        "role": "model",
                        "parts": [{ "text": format!("{text}|gemini") }]
                    },
                    "finishReason": "STOP"
                }],
                "usageMetadata": {
                    "promptTokenCount": 1,
                    "candidatesTokenCount": 1,
                    "totalTokenCount": 2
                }
            }))
            .into_response()
        }
    }

    let router = Router::new()
        .route("/v1/responses", post(responses))
        .route("/v1/chat/completions", post(chat))
        .route("/v1/messages", post(messages))
        .route("/v1beta/models/{*rest}", post(gemini_dispatch))
        .with_state(Arc::clone(&captured_headers));

    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();

    tokio::spawn(async move {
        axum::serve(listener, router).await.unwrap();
    });

    (addr, captured_headers)
}

fn collect_responses_text(input: Option<&Value>) -> String {
    let Some(input) = input else {
        return String::new();
    };
    if let Some(s) = input.as_str() {
        return s.to_string();
    }
    let Some(arr) = input.as_array() else {
        return String::new();
    };
    let mut out = String::new();
    for item in arr {
        if item.get("type").and_then(|v| v.as_str()) != Some("message") {
            continue;
        }
        if let Some(content) = item.get("content").and_then(|v| v.as_array()) {
            for part in content {
                if let Some(t) = part.get("text").and_then(|v| v.as_str()) {
                    out.push_str(t);
                }
                if let Some(t) = part.get("input_text").and_then(|v| v.as_str()) {
                    out.push_str(t);
                }
            }
        }
    }
    out
}

fn collect_chat_text(messages: &[Value]) -> String {
    let mut out = String::new();
    for msg in messages {
        if let Some(t) = msg.get("content").and_then(|v| v.as_str()) {
            out.push_str(t);
        }
    }
    out
}

fn collect_anthropic_text(messages: &[Value]) -> String {
    let mut out = String::new();
    for msg in messages {
        let Some(content) = msg.get("content").and_then(|v| v.as_array()) else {
            continue;
        };
        for block in content {
            if block.get("type").and_then(|v| v.as_str()) == Some("text") {
                if let Some(t) = block.get("text").and_then(|v| v.as_str()) {
                    out.push_str(t);
                }
            }
        }
    }
    out
}

fn summarize_multipart_content(value: &Value) -> String {
    if let Some(s) = value.as_str() {
        return s.to_string();
    }
    if let Some(obj) = value.as_object() {
        return summarize_content_part(obj);
    }
    if let Some(arr) = value.as_array() {
        return arr
            .iter()
            .filter_map(|item| {
                if let Some(s) = item.as_str() {
                    return Some(s.to_string());
                }
                item.as_object().map(summarize_content_part)
            })
            .filter(|s| !s.is_empty())
            .collect::<Vec<_>>()
            .join("");
    }
    String::new()
}

fn summarize_content_part(obj: &serde_json::Map<String, Value>) -> String {
    match obj.get("type").and_then(|v| v.as_str()).unwrap_or("") {
        "text" | "input_text" | "output_text" => obj
            .get("text")
            .and_then(|v| v.as_str())
            .unwrap_or("")
            .to_string(),
        "image" | "input_image" | "output_image" => {
            let url = obj
                .get("image_url")
                .and_then(|v| v.as_str())
                .or_else(|| obj.get("url").and_then(|v| v.as_str()))
                .or_else(|| {
                    obj.get("source")
                        .and_then(|v| v.get("url"))
                        .and_then(|v| v.as_str())
                });
            match url {
                Some(u) if !u.is_empty() => format!("[image:{u}]"),
                _ => "[image]".to_string(),
            }
        }
        "document" | "file" | "input_file" | "output_file" => {
            let file_ref = obj
                .get("file_url")
                .and_then(|v| v.as_str())
                .or_else(|| obj.get("url").and_then(|v| v.as_str()))
                .or_else(|| {
                    obj.get("source")
                        .and_then(|v| v.get("url"))
                        .and_then(|v| v.as_str())
                })
                .or_else(|| obj.get("file_id").and_then(|v| v.as_str()));
            match file_ref {
                Some(f) if !f.is_empty() => format!("[file:{f}]"),
                _ => "[file]".to_string(),
            }
        }
        _ => String::new(),
    }
}

fn echo_suffix(body: &Value) -> String {
    if let Some(s) = body.get("extra_echo").and_then(|v| v.as_str()) {
        return format!("|extra_echo={s}");
    }
    if let Some(s) = body.get("unparsed_field").and_then(|v| v.as_str()) {
        return format!("|unparsed_field={s}");
    }
    String::new()
}

async fn create_test_provider(
    state: &monoize::app::AppState,
    name: &str,
    provider_type: monoize::monoize_routing::MonoizeProviderType,
    logical_model: &str,
    base_url: &str,
    api_key: &str,
) {
    let mut models = HashMap::new();
    models.insert(
        logical_model.to_string(),
        monoize::monoize_routing::MonoizeModelEntry {
            redirect: None,
            multiplier: 1.0,
        },
    );
    state
        .monoize_store
        .create_provider(monoize::monoize_routing::CreateMonoizeProviderInput {
            name: name.to_string(),
            provider_type,
            models,
            api_type_overrides: Vec::new(),
            channels: vec![monoize::monoize_routing::CreateMonoizeChannelInput {
                id: None,
                name: format!("{name}-channel"),
                base_url: base_url.to_string(),
                api_key: Some(api_key.to_string()),
                weight: 1,
                enabled: true,
                passive_failure_count_threshold_override: None,
                passive_cooldown_seconds_override: None,
                passive_window_seconds_override: None,
                passive_rate_limit_cooldown_seconds_override: None,
            }],
            max_retries: -1,
            channel_max_retries: 0,
            channel_retry_interval_ms: 0,
            circuit_breaker_enabled: true,
            per_model_circuit_break: false,
            transforms: Vec::new(),
            active_probe_enabled_override: None,
            active_probe_interval_seconds_override: None,
            active_probe_success_threshold_override: None,
            active_probe_model_override: None,
            request_timeout_ms_override: None,
            enabled: true,
            priority: None,
        })
        .await
        .unwrap();
}

async fn seed_test_model_pricing(state: &monoize::app::AppState, model_ids: &[&str]) {
    for model_id in model_ids {
        state
            .model_registry_store
            .upsert_model_metadata(
                model_id,
                monoize::model_registry_store::UpsertModelMetadataInput {
                    models_dev_provider: Some("test".to_string()),
                    mode: Some("chat".to_string()),
                    input_cost_per_token_nano: Some("1000".to_string()),
                    output_cost_per_token_nano: Some("1000".to_string()),
                    cache_read_input_cost_per_token_nano: None,
                    cache_creation_input_cost_per_token_nano: None,
                    output_cost_per_reasoning_token_nano: None,
                    max_input_tokens: None,
                    max_output_tokens: None,
                    max_tokens: None,
                },
            )
            .await
            .expect("seed model pricing");
    }
}

async fn setup_with_unknown_fields() -> TestContext {
    let (upstream_addr, captured_headers) = start_upstream().await;
    let base_url = format!("http://{upstream_addr}");

    let temp_dir = tempfile::tempdir().unwrap();
    let db_path = temp_dir.path().join("monoize.db");
    let state = monoize::app::load_state_with_runtime(monoize::app::RuntimeConfig {
        listen: "127.0.0.1:0".to_string(),
        metrics_path: "/metrics".to_string(),
        database_dsn: format!("sqlite://{}", db_path.display()),
    })
    .await
    .expect("load state");

    let user = state
        .user_store
        .create_user("tenant-1", "test-password", monoize::users::UserRole::User)
        .await
        .expect("create user");
    state
        .user_store
        .update_user(
            &user.id,
            None,
            None,
            None,
            None,
            Some("1000000000"),
            Some(true),
            None,
        )
        .await
        .expect("update user balance");
    let (_, test_token) = state
        .user_store
        .create_api_key(&user.id, "test-key", None)
        .await
        .expect("create api key");

    create_test_provider(
        &state,
        "up-resp",
        monoize::monoize_routing::MonoizeProviderType::Responses,
        "gpt-5-mini",
        &base_url,
        "upstream-key",
    )
    .await;
    create_test_provider(
        &state,
        "up-chat",
        monoize::monoize_routing::MonoizeProviderType::ChatCompletion,
        "gpt-5-mini-chat",
        &base_url,
        "upstream-key",
    )
    .await;
    create_test_provider(
        &state,
        "up-msg",
        monoize::monoize_routing::MonoizeProviderType::Messages,
        "gpt-5-mini-msg",
        &base_url,
        "upstream-key",
    )
    .await;
    create_test_provider(
        &state,
        "up-gem",
        monoize::monoize_routing::MonoizeProviderType::Gemini,
        "gemini-2.5-flash",
        &base_url,
        "upstream-key-gem",
    )
    .await;
    create_test_provider(
        &state,
        "up-grok",
        monoize::monoize_routing::MonoizeProviderType::Grok,
        "grok-4",
        &base_url,
        "upstream-key-grok",
    )
    .await;

    seed_test_model_pricing(
        &state,
        &[
            "gpt-5-mini",
            "gpt-5-mini-chat",
            "gpt-5-mini-msg",
            "gemini-2.5-flash",
            "grok-4",
        ],
    )
    .await;

    let router = monoize::app::build_app(state.clone());

    TestContext {
        router,
        auth_header: format!("Bearer {test_token}"),
        state,
        captured_headers,
        _temp_dir: temp_dir,
    }
}

async fn setup() -> TestContext {
    setup_with_unknown_fields().await
}

async fn json_post(ctx: &TestContext, path: &str, body: Value) -> (StatusCode, String) {
    let req = Request::builder()
        .method("POST")
        .uri(path)
        .header(CONTENT_TYPE, "application/json")
        .header(AUTHORIZATION, ctx.auth_header.clone())
        .body(Body::from(body.to_string()))
        .unwrap();
    let resp = ctx.router.clone().oneshot(req).await.unwrap();
    let status = resp.status();
    let bytes = resp.into_body().collect().await.unwrap().to_bytes();
    (status, String::from_utf8_lossy(&bytes).to_string())
}

async fn json_get(ctx: &TestContext, path: &str) -> (StatusCode, String) {
    let req = Request::builder()
        .method("GET")
        .uri(path)
        .header(AUTHORIZATION, ctx.auth_header.clone())
        .body(Body::empty())
        .unwrap();
    let resp = ctx.router.clone().oneshot(req).await.unwrap();
    let status = resp.status();
    let bytes = resp.into_body().collect().await.unwrap().to_bytes();
    (status, String::from_utf8_lossy(&bytes).to_string())
}

async fn dashboard_session_cookie(ctx: &TestContext, username: &str, password: &str) -> String {
    let req = Request::builder()
        .method("POST")
        .uri("/api/dashboard/auth/login")
        .header(CONTENT_TYPE, "application/json")
        .body(Body::from(
            json!({
                "username": username,
                "password": password,
            })
            .to_string(),
        ))
        .unwrap();
    let resp = ctx.router.clone().oneshot(req).await.unwrap();
    assert_eq!(resp.status(), StatusCode::OK);
    resp.headers()
        .get("set-cookie")
        .and_then(|v| v.to_str().ok())
        .map(|s| s.to_string())
        .expect("set-cookie header")
}
fn parse_sse_frames(text: &str) -> Vec<(Option<String>, String)> {
    text.split("\n\n")
        .filter_map(|frame| {
            let frame = frame.trim();
            if frame.is_empty() {
                return None;
            }
            let mut event_name = None;
            let mut data_lines = Vec::new();
            for line in frame.lines() {
                if let Some(value) = line.strip_prefix("event: ") {
                    event_name = Some(value.to_string());
                } else if let Some(value) = line.strip_prefix("data: ") {
                    data_lines.push(value.to_string());
                }
            }
            if data_lines.is_empty() {
                return None;
            }
            Some((event_name, data_lines.join("\n")))
        })
        .collect()
}

fn parse_responses_sse_json(text: &str) -> Vec<(String, Value)> {
    parse_sse_frames(text)
        .into_iter()
        .filter_map(|(event, data)| {
            if data == "[DONE]" {
                return None;
            }
            Some((
                event.expect("responses frame should have event name"),
                serde_json::from_str::<Value>(&data).expect("responses frame should be json"),
            ))
        })
        .collect()
}

fn count_done_sentinels(text: &str) -> usize {
    text.lines().filter(|line| *line == "data: [DONE]").count()
}
