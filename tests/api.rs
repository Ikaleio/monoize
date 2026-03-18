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
        item.get("type").and_then(|v| v.as_str()) == Some("reasoning")
            && item.get("text").is_some()
    })?;
    Some((
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
                        "choices": [{ "index": 0, "delta": { "reasoning_details": [{ "type": "reasoning.summary", "summary": "mock_summary", "format": "openrouter" }] }, "finish_reason": Value::Null }]
                    }).to_string())));
                    chunks.push(Ok(Event::default().data(json!({
                        "id": "chatcmpl_mock",
                        "object": "chat.completion.chunk",
                        "created": 0,
                        "model": model,
                        "choices": [{ "index": 0, "delta": { "reasoning_details": [{ "type": "reasoning.text", "text": "mock_reasoning", "format": "openrouter" }, { "type": "reasoning.encrypted", "data": "mock_sig", "format": "openrouter" }] }, "finish_reason": Value::Null }]
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
                if body.get("stream_mode").and_then(|v| v.as_str()) == Some("content_array_tool_use") {
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
                    "choices": [{ "index": 0, "delta": { "reasoning_details": [{ "type": "reasoning.summary", "summary": "mock_summary", "format": "openrouter" }] }, "finish_reason": Value::Null }]
                }).to_string())));
                chunks.push(Ok(Event::default().data(json!({
                    "id": "chatcmpl_mock",
                    "object": "chat.completion.chunk",
                    "created": 0,
                    "model": model,
                    "choices": [{ "index": 0, "delta": { "reasoning_details": [{ "type": "reasoning.text", "text": "mock_reasoning", "format": "openrouter" }, { "type": "reasoning.encrypted", "data": "mock_sig", "format": "openrouter" }] }, "finish_reason": Value::Null }]
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
                    "choices": [{ "index": 0, "delta": { "reasoning_details": [{ "type": "reasoning.text", "text": "mock_reasoning", "format": "openrouter" }, { "type": "reasoning.encrypted", "data": "mock_sig", "format": "openrouter" }] }, "finish_reason": Value::Null }]
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
                        "reasoning_details": [{ "type": "reasoning.text", "text": "mock_reasoning", "format": "openrouter" }, { "type": "reasoning.encrypted", "data": "mock_sig", "format": "openrouter" }]
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
                "reasoning_details": [{ "type": "reasoning.text", "text": "mock_reasoning", "format": "openrouter" }, { "type": "reasoning.encrypted", "data": "mock_sig", "format": "openrouter" }]
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
                passive_failure_threshold_override: None,
                passive_cooldown_seconds_override: None,
                passive_window_seconds_override: None,
                passive_min_samples_override: None,
                passive_failure_rate_threshold_override: None,
                passive_rate_limit_cooldown_seconds_override: None,
            }],
            max_retries: -1,
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

#[tokio::test]
async fn auth_required_for_forwarding_endpoints() {
    let ctx = setup().await;
    let req = Request::builder()
        .method("POST")
        .uri("/v1/responses")
        .header(CONTENT_TYPE, "application/json")
        .body(Body::from(
            json!({"model":"gpt-5-mini","input":"hi"}).to_string(),
        ))
        .unwrap();
    let resp = ctx.router.clone().oneshot(req).await.unwrap();
    assert_eq!(resp.status(), StatusCode::UNAUTHORIZED);

    let req = Request::builder()
        .method("GET")
        .uri("/v1/models")
        .body(Body::empty())
        .unwrap();
    let resp = ctx.router.clone().oneshot(req).await.unwrap();
    assert_eq!(resp.status(), StatusCode::UNAUTHORIZED);
}

#[tokio::test]
async fn models_list_returns_union_sorted_and_unique() {
    let ctx = setup().await;

    create_test_provider(
        &ctx.state,
        "up-dup",
        monoize::monoize_routing::MonoizeProviderType::Responses,
        "gpt-5-mini",
        "http://127.0.0.1:1",
        "upstream-key",
    )
    .await;
    create_test_provider(
        &ctx.state,
        "up-new",
        monoize::monoize_routing::MonoizeProviderType::Responses,
        "zeta-model",
        "http://127.0.0.1:1",
        "upstream-key",
    )
    .await;

    let (status, body) = json_get(&ctx, "/v1/models").await;
    assert_eq!(status, StatusCode::OK);

    let v: Value = serde_json::from_str(&body).unwrap();
    assert_eq!(v["object"], "list");
    let data = v["data"].as_array().expect("data should be an array");

    let ids: Vec<String> = data
        .iter()
        .map(|item| {
            assert_eq!(item["object"], "model");
            assert_eq!(item["created"], 0);
            assert_eq!(item["owned_by"], "monoize");
            item["id"]
                .as_str()
                .expect("id should be string")
                .to_string()
        })
        .collect();

    assert_eq!(
        ids,
        vec![
            "gemini-2.5-flash".to_string(),
            "gpt-5-mini".to_string(),
            "gpt-5-mini-chat".to_string(),
            "gpt-5-mini-msg".to_string(),
            "grok-4".to_string(),
            "zeta-model".to_string(),
        ]
    );
}

#[tokio::test]
async fn models_list_api_alias_works() {
    let ctx = setup().await;
    let (status, body) = json_get(&ctx, "/api/v1/models").await;
    assert_eq!(status, StatusCode::OK);
    let v: Value = serde_json::from_str(&body).unwrap();
    assert_eq!(v["object"], "list");
}

#[tokio::test]
async fn api_alias_works() {
    let ctx = setup().await;
    let (status, body) = json_post(
        &ctx,
        "/api/v1/responses",
        json!({"model":"gpt-5-mini","input":"hello"}),
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    assert!(body.contains("response"));
}

#[tokio::test]
async fn responses_forward_nonstream_and_preserves_unknown_fields() {
    let ctx = setup().await;
    let (status, body) = json_post(
        &ctx,
        "/v1/responses",
        json!({
            "model": "gpt-5-mini",
            "input": [{ "type": "message", "role": "user", "content": [{ "type": "input_text", "text": "ping" }] }],
            "extra_echo": "E1"
        }),
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    let v: Value = serde_json::from_str(&body).unwrap();
    let text = v["output"][0]["content"][0]["text"].as_str().unwrap_or("");
    assert!(text.contains("ping|extra_echo=E1"));
}

#[tokio::test]
async fn chat_to_responses_upstream_reasoning_inputs_always_include_summary() {
    let ctx = setup().await;
    let (status, body) = json_post(
        &ctx,
        "/v1/chat/completions",
        json!({
            "model":"gpt-5-mini",
            "messages":[
                {"role":"user","content":"start"},
                {
                    "role":"assistant",
                    "content":"",
                    "reasoning_details":[
                        {"type":"reasoning.text","text":"plain think","format":"openrouter"},
                        {"type":"reasoning.encrypted","data":"sig_1","format":"openrouter"}
                    ]
                },
                {"role":"user","content":"continue"}
            ],
            "require_reasoning_input_summary": true
        }),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{body}");
    let v: Value = serde_json::from_str(&body).expect("chat response json");
    assert_eq!(v["choices"][0]["message"]["content"], json!("startcontinue"));
}

#[tokio::test]
async fn chat_completions_adapter_nonstream() {
    let ctx = setup().await;
    let (status, body) = json_post(
        &ctx,
        "/v1/chat/completions",
        json!({
            "model": "gpt-5-mini-chat",
            "messages": [{ "role": "user", "content": "hi" }],
            "extra_echo": "E2"
        }),
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    let v: Value = serde_json::from_str(&body).unwrap();
    let text = v["choices"][0]["message"]["content"].as_str().unwrap_or("");
    assert!(text.contains("hi|extra_echo=E2"));
}

#[tokio::test]
async fn messages_adapter_nonstream() {
    let ctx = setup().await;
    let (status, body) = json_post(
        &ctx,
        "/v1/messages",
        json!({
            "model": "gpt-5-mini-msg",
            "max_tokens": 16,
            "messages": [{ "role": "user", "content": [{ "type": "text", "text": "yo" }] }],
            "extra_echo": "E3"
        }),
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    let v: Value = serde_json::from_str(&body).unwrap();
    let text = v["content"][0]["text"].as_str().unwrap_or("");
    assert!(text.contains("yo|extra_echo=E3"));
}

#[tokio::test]
async fn responses_streaming_emits_sse_events() {
    let ctx = setup().await;
    let req = Request::builder()
        .method("POST")
        .uri("/v1/responses")
        .header(CONTENT_TYPE, "application/json")
        .header(AUTHORIZATION, ctx.auth_header.clone())
        .body(Body::from(
            json!({
                "model":"gpt-5-mini",
                "input":[{"type":"message","role":"user","content":[{"type":"input_text","text":"stream"}]}],
                "stream": true
            })
            .to_string(),
        ))
        .unwrap();
    let resp = ctx.router.clone().oneshot(req).await.unwrap();
    assert_eq!(resp.status(), StatusCode::OK);
    let bytes = resp.into_body().collect().await.unwrap().to_bytes();
    let text = String::from_utf8_lossy(&bytes).to_string();
    assert!(text.contains("event: response.created"));
    assert!(text.contains("event: response.completed"));
    let frames = parse_responses_sse_json(&text);
    assert!(!frames.is_empty(), "expected responses sse frames");
    let created = frames
        .iter()
        .find(|(event, _)| event == "response.created")
        .expect("response.created frame");
    assert_eq!(created.1["type"].as_str(), Some("response.created"));
    assert!(
        created.1.get("data").is_none(),
        "must not wrap payload in data field"
    );
    assert!(created.1["sequence_number"].as_u64().is_some());
    assert_eq!(
        count_done_sentinels(&text),
        1,
        "responses stream must emit exactly one [DONE]"
    );
}

#[tokio::test]
async fn responses_streaming_reconstructs_text_from_output_item_done() {
    let ctx = setup().await;
    let req = Request::builder()
        .method("POST")
        .uri("/v1/responses")
        .header(CONTENT_TYPE, "application/json")
        .header(AUTHORIZATION, ctx.auth_header.clone())
        .body(Body::from(
            json!({
                "model":"gpt-5-mini",
                "input":"stream",
                "stream": true,
                "stream_mode": "item_done_only"
            })
            .to_string(),
        ))
        .unwrap();
    let resp = ctx.router.clone().oneshot(req).await.unwrap();
    assert_eq!(resp.status(), StatusCode::OK);
    let bytes = resp.into_body().collect().await.unwrap().to_bytes();
    let text = String::from_utf8_lossy(&bytes).to_string();
    assert!(text.contains("event: response.output_text.delta"));
}

#[tokio::test]
async fn responses_streaming_reconstructs_phase_from_output_item_done() {
    let ctx = setup().await;
    let req = Request::builder()
        .method("POST")
        .uri("/v1/responses")
        .header(CONTENT_TYPE, "application/json")
        .header(AUTHORIZATION, ctx.auth_header.clone())
        .body(Body::from(
            json!({
                "model":"gpt-5-mini",
                "input":"stream",
                "stream": true,
                "stream_mode": "item_done_only",
                "message_phase": "commentary"
            })
            .to_string(),
        ))
        .unwrap();
    let resp = ctx.router.clone().oneshot(req).await.unwrap();
    assert_eq!(resp.status(), StatusCode::OK);
    let bytes = resp.into_body().collect().await.unwrap().to_bytes();
    let text = String::from_utf8_lossy(&bytes).to_string();
    assert!(text.contains("event: response.output_text.delta"));
    assert!(text.contains("\"phase\":\"commentary\""));
}

#[tokio::test]
async fn chat_streaming_records_ttfb_usage_and_charge_in_request_logs() {
    let ctx = setup().await;
    let req = Request::builder()
        .method("POST")
        .uri("/v1/chat/completions")
        .header(CONTENT_TYPE, "application/json")
        .header(AUTHORIZATION, ctx.auth_header.clone())
        .body(Body::from(
            json!({
                "model":"gpt-5-mini-chat",
                "messages":[{"role":"user","content":"stream-log"}],
                "stream": true,
                "emit_usage": true
            })
            .to_string(),
        ))
        .unwrap();

    let resp = ctx.router.clone().oneshot(req).await.unwrap();
    assert_eq!(resp.status(), StatusCode::OK);
    let _ = resp.into_body().collect().await.unwrap().to_bytes();
    ctx.state.user_store.flush_all_batchers().await;

    let user = ctx
        .state
        .user_store
        .get_user_by_username("tenant-1")
        .await
        .expect("query user")
        .expect("user exists");

    let mut matched = None;
    for _ in 0..20 {
        ctx.state.user_store.flush_all_batchers().await;
        let (logs, _, _) = ctx
            .state
            .user_store
            .list_request_logs_by_user(&user.id, 100, 0, None, None, None, None, None, None)
            .await
            .expect("list request logs");
        matched = logs.into_iter().find(|log| {
            log.model == "gpt-5-mini-chat"
                && log.is_stream
                && log.tokens.input == Some(12)
                && log.tokens.output == Some(8)
                && log.billing.charge_nano_usd.as_deref() == Some("20000")
        });
        if matched.is_some() {
            break;
        }
        tokio::time::sleep(Duration::from_millis(50)).await;
    }

    let log = matched.expect("request log should be inserted");
    assert!(log.is_stream);
    assert!(log.timing.ttfb_ms.is_some());
    assert_eq!(log.tokens.input, Some(12));
    assert_eq!(log.tokens.output, Some(8));
    assert_eq!(log.billing.charge_nano_usd.as_deref(), Some("20000"));
}

#[tokio::test]
async fn chat_streaming_requests_upstream_include_usage_by_default() {
    let ctx = setup().await;
    let req = Request::builder()
        .method("POST")
        .uri("/v1/chat/completions")
        .header(CONTENT_TYPE, "application/json")
        .header(AUTHORIZATION, ctx.auth_header.clone())
        .body(Body::from(
            json!({
                "model":"gpt-5-mini-chat",
                "messages":[{"role":"user","content":"stream-log-include-usage"}],
                "stream": true
            })
            .to_string(),
        ))
        .unwrap();

    let resp = ctx.router.clone().oneshot(req).await.unwrap();
    assert_eq!(resp.status(), StatusCode::OK);
    let _ = resp.into_body().collect().await.unwrap().to_bytes();
    ctx.state.user_store.flush_all_batchers().await;

    let user = ctx
        .state
        .user_store
        .get_user_by_username("tenant-1")
        .await
        .expect("query user")
        .expect("user exists");

    let mut matched = None;
    for _ in 0..20 {
        ctx.state.user_store.flush_all_batchers().await;
        let (logs, _, _) = ctx
            .state
            .user_store
            .list_request_logs_by_user(
                &user.id,
                100,
                0,
                Some("gpt-5-mini-chat"),
                Some("success"),
                None,
                None,
                None,
                None,
            )
            .await
            .expect("list request logs");
        matched = logs.into_iter().find(|log| {
            log.model == "gpt-5-mini-chat"
                && log.is_stream
                && log.tokens.input == Some(12)
                && log.tokens.output == Some(8)
                && log.billing.charge_nano_usd.as_deref() == Some("20000")
        });
        if matched.is_some() {
            break;
        }
        tokio::time::sleep(Duration::from_millis(50)).await;
    }

    let log = matched.expect("stream log should include usage without explicit emit_usage");
    assert!(log.timing.ttfb_ms.is_some());
}

#[tokio::test]
async fn request_logs_pending_transitions_to_success_and_charges_once() {
    let ctx = setup().await;

    let user = ctx
        .state
        .user_store
        .get_user_by_username("tenant-1")
        .await
        .expect("query user")
        .expect("user exists");

    ctx.state
        .user_store
        .update_user(
            &user.id,
            None,
            None,
            None,
            None,
            Some("1000000000"),
            Some(false),
            None,
        )
        .await
        .expect("set finite balance");

    let user_before = ctx
        .state
        .user_store
        .get_user_by_username("tenant-1")
        .await
        .expect("query user before")
        .expect("user exists");

    let router = ctx.router.clone();
    let auth_header = ctx.auth_header.clone();
    let request_task = tokio::spawn(async move {
        let req = Request::builder()
            .method("POST")
            .uri("/v1/chat/completions")
            .header(CONTENT_TYPE, "application/json")
            .header(AUTHORIZATION, auth_header)
            .body(Body::from(
                json!({
                    "model":"gpt-5-mini-chat",
                    "messages":[{"role":"user","content":"pending-transition"}],
                    "stream": true,
                    "emit_usage": true,
                    "force_upstream_delay_ms": 800
                })
                .to_string(),
            ))
            .unwrap();

        let resp = router.oneshot(req).await.unwrap();
        assert_eq!(resp.status(), StatusCode::OK);
        let _ = resp.into_body().collect().await.unwrap().to_bytes();
    });

    request_task.await.expect("request task");
    ctx.state.user_store.flush_all_batchers().await;

    let mut model_logs = Vec::new();
    for _ in 0..30 {
        let (logs, _, _) = ctx
            .state
            .user_store
            .list_request_logs_by_user(&user.id, 100, 0, None, None, None, None, None, None)
            .await
            .expect("list request logs");
        model_logs = logs
            .into_iter()
            .filter(|log| log.model == "gpt-5-mini-chat")
            .collect();
        if model_logs.len() == 1 && model_logs[0].status == "success" {
            break;
        }
        tokio::time::sleep(Duration::from_millis(30)).await;
    }

    assert_eq!(
        model_logs.len(),
        1,
        "same request should keep a single lifecycle row"
    );
    let log = &model_logs[0];
    assert_eq!(log.status, "success");
    assert_eq!(log.billing.charge_nano_usd.as_deref(), Some("20000"));

    let user_after = ctx
        .state
        .user_store
        .get_user_by_username("tenant-1")
        .await
        .expect("query user after")
        .expect("user exists");
    let before_nano: i128 = user_before
        .balance_nano_usd
        .parse()
        .expect("parse before balance");
    let after_nano: i128 = user_after
        .balance_nano_usd
        .parse()
        .expect("parse after balance");
    assert_eq!(before_nano - after_nano, 20000);
}

#[tokio::test]
async fn request_logs_pending_usage_can_be_updated_incrementally() {
    // With the batcher pattern, insert_request_log_pending and
    // update_pending_request_log_usage are no-ops. Verify they succeed
    // without error but produce no persisted row.
    let ctx = setup().await;
    let user = ctx
        .state
        .user_store
        .get_user_by_username("tenant-1")
        .await
        .expect("query user")
        .expect("user exists");
    let request_id = "pending-usage-update-test";

    ctx.state
        .user_store
        .insert_request_log_pending(request_id, &user.id, None, "gpt-5-mini-chat", true, None)
        .await
        .expect("insert_request_log_pending should succeed (no-op)");

    ctx.state
        .user_store
        .update_pending_request_log_usage(
            &user.id,
            request_id,
            12,
            8,
            Some(0),
            Some(3),
            Some(2),
            Some(0),
            Some(5),
            Some(1),
            Some(json!({
                "input": { "total_tokens": 12 },
                "output": { "total_tokens": 8 }
            })),
        )
        .await
        .expect("update_pending_request_log_usage should succeed (no-op)");

    // No row should be persisted since both methods are no-ops under batcher pattern
    let (logs, _, _) = ctx
        .state
        .user_store
        .list_request_logs_by_user(
            &user.id,
            100,
            0,
            Some("gpt-5-mini-chat"),
            Some("pending"),
            None,
            Some(request_id),
            None,
            None,
        )
        .await
        .expect("list pending logs");

    assert!(
        logs.is_empty(),
        "no pending row should exist under batcher pattern"
    );
}

#[tokio::test]
async fn request_log_batcher_broadcasts_immediately_before_flush() {
    let ctx = setup().await;
    let user = ctx
        .state
        .user_store
        .get_user_by_username("tenant-1")
        .await
        .expect("query user")
        .expect("user exists");

    let mut receiver = ctx.state.log_broadcast.subscribe();
    let log = monoize::users::InsertRequestLog {
        request_id: Some("immediate-broadcast-request".to_string()),
        user_id: user.id.clone(),
        api_key_id: None,
        model: "gpt-5-mini-chat".to_string(),
        provider_id: None,
        upstream_model: None,
        channel_id: None,
        is_stream: false,
        input_tokens: Some(12),
        output_tokens: Some(8),
        cache_read_tokens: None,
        cache_creation_tokens: None,
        tool_prompt_tokens: None,
        reasoning_tokens: None,
        accepted_prediction_tokens: None,
        rejected_prediction_tokens: None,
        provider_multiplier: None,
        charge_nano_usd: Some(1234),
        status: monoize::users::REQUEST_LOG_STATUS_SUCCESS.to_string(),
        usage_breakdown_json: None,
        billing_breakdown_json: None,
        error_code: None,
        error_message: None,
        error_http_status: None,
        duration_ms: Some(50),
        ttfb_ms: None,
        request_ip: Some("127.0.0.1".to_string()),
        reasoning_effort: None,
        tried_providers_json: None,
        request_kind: None,
        created_at: chrono::Utc::now(),
    };

    ctx.state
        .user_store
        .finalize_request_log(log.clone())
        .await
        .expect("enqueue request log");

    let batch = tokio::time::timeout(Duration::from_millis(200), receiver.recv())
        .await
        .expect("broadcast should not wait for batch flush")
        .expect("broadcast channel should deliver batch");

    assert_eq!(batch.len(), 1);
    assert_eq!(
        batch[0].request_id.as_deref(),
        Some("immediate-broadcast-request")
    );
    assert_eq!(batch[0].status, monoize::users::REQUEST_LOG_STATUS_SUCCESS);

    ctx.state.user_store.flush_all_batchers().await;

    let duplicate = tokio::time::timeout(Duration::from_millis(150), receiver.recv()).await;
    assert!(
        duplicate.is_err(),
        "flushing persisted request logs should not rebroadcast duplicate terminal events"
    );
}

#[tokio::test]
async fn chat_upstream_error_is_logged_and_not_billed() {
    let ctx = setup().await;

    let user_before = ctx
        .state
        .user_store
        .get_user_by_username("tenant-1")
        .await
        .expect("query user before")
        .expect("user exists");

    let (status, _body) = json_post(
        &ctx,
        "/v1/chat/completions",
        json!({
            "model":"gpt-5-mini-chat",
            "messages":[{"role":"user","content":"force error"}],
            "force_upstream_error_status": 422,
            "force_upstream_error_code": "rate_limit"
        }),
    )
    .await;
    assert_eq!(status, StatusCode::UNPROCESSABLE_ENTITY);
    ctx.state.user_store.flush_all_batchers().await;

    let user = ctx
        .state
        .user_store
        .get_user_by_username("tenant-1")
        .await
        .expect("query user")
        .expect("user exists");

    let mut matched = None;
    for _ in 0..20 {
        ctx.state.user_store.flush_all_batchers().await;
        let (logs, _, _) = ctx
            .state
            .user_store
            .list_request_logs_by_user(
                &user.id,
                100,
                0,
                Some("gpt-5-mini-chat"),
                Some("error"),
                None,
                None,
                None,
                None,
            )
            .await
            .expect("list request logs");
        matched = logs
            .into_iter()
            .find(|log| log.status == "error" && log.error.http_status == Some(422));
        if matched.is_some() {
            break;
        }
        tokio::time::sleep(Duration::from_millis(50)).await;
    }

    let log = matched.expect("error request log should be inserted");
    assert_eq!(log.billing.charge_nano_usd, None);
    assert_eq!(log.error.code.as_deref(), Some("upstream_error"));
    assert!(
        log.error
            .message
            .as_deref()
            .unwrap_or("")
            .contains("upstream status 422")
    );

    let user_after = ctx
        .state
        .user_store
        .get_user_by_username("tenant-1")
        .await
        .expect("query user after")
        .expect("user exists");
    assert_eq!(user_before.balance_nano_usd, user_after.balance_nano_usd);
}

#[tokio::test]
async fn request_log_retention_deletes_only_rows_older_than_ninety_days() {
    let ctx = setup().await;
    let user = ctx
        .state
        .user_store
        .get_user_by_username("tenant-1")
        .await
        .expect("query user")
        .expect("user exists");

    let old_created_at = Utc::now() - ChronoDuration::days(91);
    let new_created_at = Utc::now() - ChronoDuration::days(30);

    ctx.state
        .user_store
        .finalize_request_log(monoize::users::InsertRequestLog {
            request_id: Some("retention-old".to_string()),
            user_id: user.id.clone(),
            api_key_id: None,
            model: "retention-model-old".to_string(),
            provider_id: None,
            upstream_model: None,
            channel_id: None,
            is_stream: false,
            input_tokens: Some(1),
            output_tokens: Some(1),
            cache_read_tokens: None,
            cache_creation_tokens: None,
            tool_prompt_tokens: None,
            reasoning_tokens: None,
            accepted_prediction_tokens: None,
            rejected_prediction_tokens: None,
            provider_multiplier: None,
            charge_nano_usd: Some(1),
            status: monoize::users::REQUEST_LOG_STATUS_SUCCESS.to_string(),
            usage_breakdown_json: None,
            billing_breakdown_json: None,
            error_code: None,
            error_message: None,
            error_http_status: None,
            duration_ms: Some(1),
            ttfb_ms: None,
            request_ip: None,
            reasoning_effort: None,
            tried_providers_json: None,
            request_kind: None,
            created_at: old_created_at,
        })
        .await
        .expect("insert old request log");

    ctx.state
        .user_store
        .finalize_request_log(monoize::users::InsertRequestLog {
            request_id: Some("retention-new".to_string()),
            user_id: user.id.clone(),
            api_key_id: None,
            model: "retention-model-new".to_string(),
            provider_id: None,
            upstream_model: None,
            channel_id: None,
            is_stream: false,
            input_tokens: Some(1),
            output_tokens: Some(1),
            cache_read_tokens: None,
            cache_creation_tokens: None,
            tool_prompt_tokens: None,
            reasoning_tokens: None,
            accepted_prediction_tokens: None,
            rejected_prediction_tokens: None,
            provider_multiplier: None,
            charge_nano_usd: Some(1),
            status: monoize::users::REQUEST_LOG_STATUS_SUCCESS.to_string(),
            usage_breakdown_json: None,
            billing_breakdown_json: None,
            error_code: None,
            error_message: None,
            error_http_status: None,
            duration_ms: Some(1),
            ttfb_ms: None,
            request_ip: None,
            reasoning_effort: None,
            tried_providers_json: None,
            request_kind: None,
            created_at: new_created_at,
        })
        .await
        .expect("insert new request log");

    ctx.state.user_store.flush_all_batchers().await;

    let deleted = ctx
        .state
        .user_store
        .cleanup_expired_request_logs()
        .await
        .expect("cleanup expired request logs");
    assert_eq!(deleted, 1, "only logs older than 90 days should be deleted");

    let (logs, _, _) = ctx
        .state
        .user_store
        .list_request_logs_by_user(&user.id, 100, 0, None, None, None, None, None, None)
        .await
        .expect("list request logs after retention cleanup");

    assert!(
        logs.iter()
            .all(|log| log.request_id.as_deref() != Some("retention-old")),
        "expired log should be removed"
    );
    assert!(
        logs.iter()
            .any(|log| log.request_id.as_deref() == Some("retention-new")),
        "recent log should remain"
    );
}

#[tokio::test]
async fn channel_passive_override_threshold_takes_precedence_over_global_defaults() {
    let ctx = setup().await;
    seed_test_model_pricing(&ctx.state, &["override-threshold-model"]).await;

    let providers = ctx
        .state
        .monoize_store
        .list_providers()
        .await
        .expect("list providers");
    let base_url = providers
        .iter()
        .find_map(|p| p.channels.first().map(|c| c.base_url.clone()))
        .expect("at least one existing channel base url");

    let mut models = HashMap::new();
    models.insert(
        "override-threshold-model".to_string(),
        monoize::monoize_routing::MonoizeModelEntry {
            redirect: None,
            multiplier: 1.0,
        },
    );
    let created = ctx
        .state
        .monoize_store
        .create_provider(monoize::monoize_routing::CreateMonoizeProviderInput {
            name: "override-threshold-provider".to_string(),
            provider_type: monoize::monoize_routing::MonoizeProviderType::ChatCompletion,
            models,
            api_type_overrides: Vec::new(),
            channels: vec![monoize::monoize_routing::CreateMonoizeChannelInput {
                id: Some("override-threshold-ch".to_string()),
                name: "override-threshold-ch".to_string(),
                base_url,
                api_key: Some("upstream-key".to_string()),
                weight: 1,
                enabled: true,
                passive_failure_threshold_override: Some(1),
                passive_cooldown_seconds_override: None,
                passive_window_seconds_override: None,
                passive_min_samples_override: None,
                passive_failure_rate_threshold_override: None,
                passive_rate_limit_cooldown_seconds_override: None,
            }],
            max_retries: -1,
            transforms: Vec::new(),
            active_probe_enabled_override: None,
            active_probe_interval_seconds_override: None,
            active_probe_success_threshold_override: None,
            active_probe_model_override: None,
            request_timeout_ms_override: None,
            enabled: true,
            priority: Some(-10),
        })
        .await
        .expect("create provider with channel override");
    let channel_id = created.channels[0].id.clone();

    let (status, _body) = json_post(
        &ctx,
        "/v1/chat/completions",
        json!({
            "model":"override-threshold-model",
            "messages":[{"role":"user","content":"trigger retryable failure"}],
            "force_upstream_error_status": 500,
            "force_upstream_error_code": "forced_500"
        }),
    )
    .await;
    assert_eq!(status, StatusCode::BAD_GATEWAY);

    let health = ctx.state.channel_health.lock().await;
    let state = health
        .get(&channel_id)
        .cloned()
        .expect("channel health state exists");
    assert!(
        !state.healthy,
        "channel should become unhealthy after one transient failure when override threshold=1"
    );
    assert_eq!(
        state.failure_count, 1,
        "consecutive failure count should be tracked per channel"
    );
}

#[tokio::test]
async fn chat_streaming_length_finish_is_still_billed() {
    let ctx = setup().await;
    let req = Request::builder()
        .method("POST")
        .uri("/v1/chat/completions")
        .header(CONTENT_TYPE, "application/json")
        .header(AUTHORIZATION, ctx.auth_header.clone())
        .body(Body::from(
            json!({
                "model":"gpt-5-mini-chat",
                "messages":[{"role":"user","content":"length-billing"}],
                "stream": true,
                "emit_usage": true,
                "force_finish_reason": "length"
            })
            .to_string(),
        ))
        .unwrap();

    let resp = ctx.router.clone().oneshot(req).await.unwrap();
    assert_eq!(resp.status(), StatusCode::OK);
    let _ = resp.into_body().collect().await.unwrap().to_bytes();
    ctx.state.user_store.flush_all_batchers().await;

    let user = ctx
        .state
        .user_store
        .get_user_by_username("tenant-1")
        .await
        .expect("query user")
        .expect("user exists");

    let mut matched = None;
    for _ in 0..20 {
        let (logs, _, _) = ctx
            .state
            .user_store
            .list_request_logs_by_user(
                &user.id,
                100,
                0,
                Some("gpt-5-mini-chat"),
                Some("success"),
                None,
                None,
                None,
                None,
            )
            .await
            .expect("list request logs");
        matched = logs.into_iter().find(|log| {
            log.model == "gpt-5-mini-chat"
                && log.is_stream
                && log.tokens.input == Some(12)
                && log.tokens.output == Some(8)
                && log.billing.charge_nano_usd.as_deref() == Some("20000")
                && log.status == "success"
        });
        if matched.is_some() {
            break;
        }
        tokio::time::sleep(Duration::from_millis(50)).await;
    }

    let log = matched.expect("stream length request should be billed");
    assert_eq!(log.status, "success");
    assert_eq!(log.billing.charge_nano_usd.as_deref(), Some("20000"));
}

#[tokio::test]
async fn responses_tool_call_flow_nonstream_via_chat_upstream_parallel() {
    let ctx = setup().await;
    let (status, body) = json_post(
        &ctx,
        "/v1/responses",
        json!({
            "model": "gpt-5-mini-chat",
            "input": [{ "type": "message", "role": "user", "content": [{ "type": "input_text", "text": "use tools" }] }],
            "tools": [
              { "type": "function", "function": { "name": "tool_a", "parameters": { "type": "object", "additionalProperties": true } } },
              { "type": "function", "function": { "name": "tool_b", "parameters": { "type": "object", "additionalProperties": true } } }
            ],
            "parallel_tool_calls": true
        }),
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    let v: Value = serde_json::from_str(&body).unwrap();
    let out = v["output"].as_array().cloned().unwrap_or_default();
    assert!(
        out.iter()
            .any(|x| x.get("type").and_then(|v| v.as_str()) == Some("reasoning"))
    );
    assert_eq!(
        out.iter()
            .filter(|x| x.get("type").and_then(|v| v.as_str()) == Some("function_call"))
            .count(),
        2
    );

    // Return tool results.
    let (status2, body2) = json_post(
        &ctx,
        "/v1/responses",
        json!({
            "model": "gpt-5-mini-chat",
            "input": [
              { "type": "function_call_output", "call_id": "call_1", "output": "R1" },
              { "type": "function_call_output", "call_id": "call_2", "output": "R2" }
            ]
        }),
    )
    .await;
    assert_eq!(status2, StatusCode::OK);
    let v2: Value = serde_json::from_str(&body2).unwrap();
    let text2 = v2["output"][0]["content"][0]["text"].as_str().unwrap_or("");
    assert!(text2.contains("tool_ok:R1|R2"));
}

#[tokio::test]
async fn responses_tool_result_multipart_roundtrip_via_responses_upstream() {
    let ctx = setup().await;
    let image_url = "https://example.com/tool.png";
    let file_url = "https://example.com/report.pdf";
    let (status, body) = json_post(
        &ctx,
        "/v1/responses",
        json!({
            "model": "gpt-5-mini",
            "input": [
              {
                "type": "function_call_output",
                "call_id": "call_multipart",
                "output": [
                  { "type": "input_text", "text": "R1" },
                  { "type": "input_image", "image_url": image_url },
                  { "type": "input_file", "file_url": file_url }
                ]
              }
            ]
        }),
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    let v: Value = serde_json::from_str(&body).unwrap();
    let text = v["output"][0]["content"][0]["text"].as_str().unwrap_or("");
    assert!(text.contains("tool_ok:R1"));
    assert!(text.contains(&format!("[image:{image_url}]")));
    assert!(text.contains(&format!("[file:{file_url}]")));
}

#[tokio::test]
async fn responses_input_string_maps_to_chat_upstream_messages() {
    let ctx = setup().await;
    let (status, body) = json_post(
        &ctx,
        "/v1/responses",
        json!({
            "model": "gpt-5-mini-chat",
            "input": "hello-string-input"
        }),
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    let v: Value = serde_json::from_str(&body).unwrap();
    let text = v["output"][0]["content"][0]["text"].as_str().unwrap_or("");
    assert!(text.contains("hello-string-input"));
}

#[tokio::test]
async fn responses_reasoning_effort_maps_to_chat_upstream_reasoning() {
    let ctx = setup().await;
    let (status, body) = json_post(
        &ctx,
        "/v1/responses",
        json!({
            "model": "gpt-5-mini-chat",
            "input": "show reasoning",
            "reasoning": { "effort": "high" }
        }),
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    let v: Value = serde_json::from_str(&body).unwrap();
    let out = v["output"].as_array().cloned().unwrap_or_default();
    assert!(
        out.iter()
            .any(|x| x.get("type").and_then(|t| t.as_str()) == Some("reasoning"))
    );
}

#[tokio::test]
async fn chat_reasoning_effort_maps_to_responses_upstream_reasoning() {
    let ctx = setup().await;
    let (status, body) = json_post(
        &ctx,
        "/v1/chat/completions",
        json!({
            "model": "gpt-5-mini",
            "reasoning_effort": "high",
            "messages": [{ "role": "user", "content": "show reasoning" }]
        }),
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    let v: Value = serde_json::from_str(&body).unwrap();
    assert_eq!(
        v["choices"][0]["message"]["reasoning"]
            .as_str()
            .unwrap_or(""),
        "mock_reasoning"
    );
    assert_eq!(
        v["choices"][0]["message"]["reasoning_details"][1]["data"],
        json!("mock_sig")
    );
}

#[tokio::test]
async fn chat_reasoning_effort_maps_to_messages_upstream_thinking() {
    let ctx = setup().await;
    let (status, body) = json_post(
        &ctx,
        "/v1/chat/completions",
        json!({
            "model": "gpt-5-mini-msg",
            "reasoning_effort": "high",
            "messages": [{ "role": "user", "content": "show reasoning" }]
        }),
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    let v: Value = serde_json::from_str(&body).unwrap();
    assert_eq!(
        v["choices"][0]["message"]["reasoning"]
            .as_str()
            .unwrap_or(""),
        "mock_reasoning"
    );
    assert_eq!(
        v["choices"][0]["message"]["reasoning_details"][1]["data"],
        json!("mock_sig")
    );
}

#[tokio::test]
async fn chat_reasoning_effort_maps_to_chat_upstream_encrypted_reasoning() {
    let ctx = setup().await;
    let (status, body) = json_post(
        &ctx,
        "/v1/chat/completions",
        json!({
            "model": "gpt-5-mini-chat",
            "reasoning_effort": "high",
            "messages": [{ "role": "user", "content": "show reasoning" }]
        }),
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    let v: Value = serde_json::from_str(&body).unwrap();
    assert_eq!(
        v["choices"][0]["message"]["reasoning"]
            .as_str()
            .unwrap_or(""),
        "mock_reasoning"
    );
    let details = v["choices"][0]["message"]["reasoning_details"]
        .as_array()
        .cloned()
        .unwrap_or_default();
    assert!(details.iter().any(|detail| {
        detail["type"].as_str() == Some("reasoning.text")
            && detail["text"].as_str() == Some("mock_reasoning")
            && detail["format"].as_str() == Some("openrouter")
    }));
    assert!(details.iter().any(|detail| {
        detail["type"].as_str() == Some("reasoning.encrypted")
            && detail["data"].as_str() == Some("mock_sig")
            && detail["format"].as_str() == Some("openrouter")
    }));
}

#[tokio::test]
async fn messages_thinking_maps_to_chat_upstream_reasoning_effort() {
    let ctx = setup().await;
    let (status, body) = json_post(
        &ctx,
        "/v1/messages",
        json!({
            "model": "gpt-5-mini-chat",
            "max_tokens": 64,
            "thinking": { "type": "enabled", "budget_tokens": 2048 },
            "messages": [{ "role": "user", "content": [{ "type": "text", "text": "show reasoning" }] }]
        }),
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    let v: Value = serde_json::from_str(&body).unwrap();
    let blocks = v["content"].as_array().cloned().unwrap_or_default();
    assert!(
        blocks
            .iter()
            .any(|b| b.get("type").and_then(|t| t.as_str()) == Some("thinking"))
    );
}

#[tokio::test]
async fn responses_response_style_tools_map_to_chat_upstream() {
    let ctx = setup().await;
    let (status, body) = json_post(
        &ctx,
        "/v1/responses",
        json!({
            "model": "gpt-5-mini-chat",
            "input": [{ "type": "message", "role": "user", "content": [{ "type": "input_text", "text": "use tools" }] }],
            "tools": [
              { "type": "function", "name": "tool_a", "parameters": { "type": "object", "additionalProperties": true } },
              { "type": "function", "name": "tool_b", "parameters": { "type": "object", "additionalProperties": true } }
            ],
            "parallel_tool_calls": true
        }),
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    let v: Value = serde_json::from_str(&body).unwrap();
    let out = v["output"].as_array().cloned().unwrap_or_default();
    assert_eq!(
        out.iter()
            .filter(|x| x.get("type").and_then(|v| v.as_str()) == Some("function_call"))
            .count(),
        2
    );
}

#[tokio::test]
async fn chat_tool_call_flow_nonstream_via_responses_upstream_parallel() {
    let ctx = setup().await;
    let (status, body) = json_post(
        &ctx,
        "/v1/chat/completions",
        json!({
            "model": "gpt-5-mini",
            "messages": [{ "role": "user", "content": "use tools" }],
            "tools": [
              { "type": "function", "function": { "name": "tool_a", "parameters": { "type": "object", "additionalProperties": true } } },
              { "type": "function", "function": { "name": "tool_b", "parameters": { "type": "object", "additionalProperties": true } } }
            ],
            "parallel_tool_calls": true
        }),
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    let v: Value = serde_json::from_str(&body).unwrap();
    let tool_calls = v["choices"][0]["message"]["tool_calls"]
        .as_array()
        .cloned()
        .unwrap_or_default();
    assert_eq!(tool_calls.len(), 2);
    assert_eq!(
        v["choices"][0]["message"]["reasoning"]
            .as_str()
            .unwrap_or(""),
        "mock_reasoning"
    );
    assert_eq!(
        v["choices"][0]["message"]["reasoning_details"][1]["data"],
        json!("mock_sig")
    );

    // Send tool results back.
    let (status2, body2) = json_post(
        &ctx,
        "/v1/chat/completions",
        json!({
            "model": "gpt-5-mini",
            "messages": [
              { "role": "assistant", "content": "", "tool_calls": tool_calls },
              { "role": "tool", "tool_call_id": "call_1", "content": "R1" },
              { "role": "tool", "tool_call_id": "call_2", "content": "R2" }
            ]
        }),
    )
    .await;
    assert_eq!(status2, StatusCode::OK);
    let v2: Value = serde_json::from_str(&body2).unwrap();
    let text2 = v2["choices"][0]["message"]["content"]
        .as_str()
        .unwrap_or("");
    assert!(text2.contains("tool_ok:R1|R2"));
}

#[tokio::test]
async fn messages_tool_call_flow_nonstream_via_responses_upstream_parallel() {
    let ctx = setup().await;
    let (status, body) = json_post(
        &ctx,
        "/v1/messages",
        json!({
            "model": "gpt-5-mini",
            "max_tokens": 64,
            "messages": [{ "role": "user", "content": [{ "type": "text", "text": "use tools" }] }],
            "tools": [
              { "name": "tool_a", "input_schema": { "type": "object", "additionalProperties": true } },
              { "name": "tool_b", "input_schema": { "type": "object", "additionalProperties": true } }
            ],
            "parallel_tool_calls": true,
            "tool_choice": { "type": "auto" }
        }),
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    let v: Value = serde_json::from_str(&body).unwrap();
    let blocks = v["content"].as_array().cloned().unwrap_or_default();
    assert!(
        blocks
            .iter()
            .any(|b| b.get("type").and_then(|v| v.as_str()) == Some("thinking"))
    );
    assert_eq!(
        blocks
            .iter()
            .filter(|b| b.get("type").and_then(|v| v.as_str()) == Some("tool_use"))
            .count(),
        2
    );

    // Return tool results.
    let (status2, body2) = json_post(
        &ctx,
        "/v1/messages",
        json!({
            "model": "gpt-5-mini",
            "max_tokens": 64,
            "messages": [{
              "role": "user",
              "content": [
                { "type": "tool_result", "tool_use_id": "call_1", "content": "R1" },
                { "type": "tool_result", "tool_use_id": "call_2", "content": "R2" }
              ]
            }]
        }),
    )
    .await;
    assert_eq!(status2, StatusCode::OK);
    let v2: Value = serde_json::from_str(&body2).unwrap();
    let text2 = v2["content"]
        .as_array()
        .and_then(|arr| {
            arr.iter()
                .find(|b| b.get("type").and_then(|v| v.as_str()) == Some("text"))
        })
        .and_then(|b| b.get("text"))
        .and_then(|v| v.as_str())
        .unwrap_or("");
    assert!(text2.contains("tool_ok:R1|R2"));
}

#[tokio::test]
async fn messages_tool_result_multipart_roundtrip_via_messages_upstream() {
    let ctx = setup().await;
    let image_url = "https://example.com/tool.png";
    let file_url = "https://example.com/report.pdf";
    let (status, body) = json_post(
        &ctx,
        "/v1/messages",
        json!({
            "model": "gpt-5-mini-msg",
            "max_tokens": 64,
            "messages": [{
              "role": "user",
              "content": [{
                "type": "tool_result",
                "tool_use_id": "call_multipart",
                "content": [
                  { "type": "text", "text": "R1" },
                  { "type": "image", "source": { "type": "url", "url": image_url } },
                  { "type": "document", "source": { "type": "url", "url": file_url } }
                ]
              }]
            }]
        }),
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    let v: Value = serde_json::from_str(&body).unwrap();
    let text = v["content"]
        .as_array()
        .and_then(|arr| {
            arr.iter()
                .find(|b| b.get("type").and_then(|v| v.as_str()) == Some("text"))
        })
        .and_then(|b| b.get("text"))
        .and_then(|v| v.as_str())
        .unwrap_or("");
    assert!(text.contains("tool_ok:R1"));
    assert!(text.contains(&format!("[image:{image_url}]")));
    assert!(text.contains(&format!("[file:{file_url}]")));
}

#[tokio::test]
async fn responses_streaming_includes_tool_calls_and_reasoning_when_upstream_is_chat() {
    let ctx = setup().await;
    let req = Request::builder()
        .method("POST")
        .uri("/v1/responses")
        .header(CONTENT_TYPE, "application/json")
        .header(AUTHORIZATION, ctx.auth_header.clone())
        .body(Body::from(
            json!({
                "model":"gpt-5-mini-chat",
                "input":[{"type":"message","role":"user","content":[{"type":"input_text","text":"stream tool"}]}],
                "tools":[{ "type":"function","function":{ "name":"tool_a","parameters":{ "type":"object","additionalProperties":true }}}],
                "parallel_tool_calls": true,
                "stream": true
            })
            .to_string(),
        ))
        .unwrap();
    let resp = ctx.router.clone().oneshot(req).await.unwrap();
    assert_eq!(resp.status(), StatusCode::OK);
    let bytes = resp.into_body().collect().await.unwrap().to_bytes();
    let text = String::from_utf8_lossy(&bytes).to_string();
    assert!(text.contains("event: response.function_call_arguments.delta"));
    assert!(text.contains("event: response.reasoning_summary_text.delta"));
}

#[tokio::test]
async fn responses_streaming_reencodes_greedy_merged_items_with_canonical_sse_boundaries() {
    let ctx = setup().await;
    let req = Request::builder()
        .method("POST")
        .uri("/v1/responses")
        .header(CONTENT_TYPE, "application/json")
        .header(AUTHORIZATION, ctx.auth_header.clone())
        .body(Body::from(
            json!({
                "model":"gpt-5-mini",
                "input":[{"type":"message","role":"user","content":[{"type":"input_text","text":"stream tool"}]}],
                "tools":[{ "type":"function","name":"tool_a","parameters":{ "type":"object","additionalProperties":true }}],
                "stream": true,
                "stream_mode": "reasoning_text_tool"
            })
            .to_string(),
        ))
        .unwrap();
    let resp = ctx.router.clone().oneshot(req).await.unwrap();
    assert_eq!(resp.status(), StatusCode::OK);
    let bytes = resp.into_body().collect().await.unwrap().to_bytes();
    let text = String::from_utf8_lossy(&bytes).to_string();

    assert!(text.contains("event: response.output_item.added"));
    assert!(text.contains("\"output_index\":0"));
    assert!(text.contains("\"output_index\":1"));
    assert!(text.contains("\"output_index\":2"));
    assert!(text.contains("\"type\":\"reasoning\""));
    assert!(text.contains("\"type\":\"message\""));
    assert!(text.contains("\"type\":\"function_call\""));
    assert!(text.contains("\"phase\":\"analysis\""));
    assert!(text.contains("event: response.content_part.added"));
    assert!(text.contains("\"part\":{\"annotations\":[],\"text\":\"\",\"type\":\"output_text\"}"));
    assert!(!text.contains("\"part\":{\"text\":\"\",\"type\":\"reasoning\"}"));
    assert!(!text.contains("event: response.content_part.added\ndata: {\"content_index\":2"));
}

#[tokio::test]
async fn responses_streaming_uses_top_level_payload_fields_and_delta_ids() {
    let ctx = setup().await;
    let req = Request::builder()
        .method("POST")
        .uri("/v1/responses")
        .header(CONTENT_TYPE, "application/json")
        .header(AUTHORIZATION, ctx.auth_header.clone())
        .body(Body::from(
            json!({
                "model":"gpt-5-mini",
                "input":[{"type":"message","role":"user","content":[{"type":"input_text","text":"stream tool"}]}],
                "tools":[{ "type":"function","name":"tool_a","parameters":{ "type":"object","additionalProperties":true }}],
                "stream": true,
                "stream_mode": "reasoning_text_tool"
            })
            .to_string(),
        ))
        .unwrap();
    let resp = ctx.router.clone().oneshot(req).await.unwrap();
    assert_eq!(resp.status(), StatusCode::OK);
    let bytes = resp.into_body().collect().await.unwrap().to_bytes();
    let text = String::from_utf8_lossy(&bytes).to_string();
    let frames = parse_responses_sse_json(&text);

    let text_delta = frames
        .iter()
        .find(|(event, _)| event == "response.output_text.delta")
        .expect("output text delta");
    assert_eq!(
        text_delta.1["type"].as_str(),
        Some("response.output_text.delta")
    );
    assert!(
        text_delta.1.get("data").is_none(),
        "must not nest payload under data"
    );
    assert!(
        text_delta.1["item_id"].as_str().is_some(),
        "text delta must include item_id"
    );
    assert_eq!(text_delta.1["logprobs"], Value::Null);

    let reasoning_summary_delta = frames
        .iter()
        .find(|(event, _)| event == "response.reasoning_summary_text.delta")
        .expect("reasoning summary delta");
    assert_eq!(
        reasoning_summary_delta.1["type"].as_str(),
        Some("response.reasoning_summary_text.delta")
    );
    let reasoning_item_id = reasoning_summary_delta.1["item_id"]
        .as_str()
        .expect("reasoning delta item_id");
    assert!(
        !reasoning_item_id.is_empty(),
        "reasoning item_id must be non-empty"
    );

    let reasoning_text_delta = frames
        .iter()
        .find(|(event, _)| event == "response.reasoning.delta")
        .expect("reasoning delta");
    assert_eq!(
        reasoning_text_delta.1["item_id"].as_str(),
        Some(reasoning_item_id)
    );

    let reasoning_done = frames
        .iter()
        .find(|(event, payload)| {
            event == "response.output_item.done"
                && payload
                    .get("item")
                    .and_then(|item| item.get("type"))
                    .and_then(Value::as_str)
                    == Some("reasoning")
        })
        .expect("reasoning output item done");
    assert_eq!(
        reasoning_done.1["item"]["id"].as_str(),
        Some(reasoning_item_id),
        "reasoning delta and done event must use same item id"
    );

    let function_delta = frames
        .iter()
        .find(|(event, _)| event == "response.function_call_arguments.delta")
        .expect("function call delta");
    assert_eq!(
        function_delta.1["type"].as_str(),
        Some("response.function_call_arguments.delta")
    );
    assert!(function_delta.1["item_id"].as_str().is_some());

    let function_done = frames
        .iter()
        .find(|(event, _)| event == "response.function_call_arguments.done")
        .expect("function call done");
    assert_eq!(function_done.1["arguments"].as_str(), Some("{\"a\":1}"));
}

#[tokio::test]
async fn responses_streaming_distinguishes_reasoning_summary_and_content() {
    let ctx = setup().await;
    let req = Request::builder()
        .method("POST")
        .uri("/v1/responses")
        .header(CONTENT_TYPE, "application/json")
        .header(AUTHORIZATION, ctx.auth_header.clone())
        .body(Body::from(
            json!({
                "model":"gpt-5-mini",
                "input":[{"type":"message","role":"user","content":[{"type":"input_text","text":"stream tool"}]}],
                "tools":[{ "type":"function","name":"tool_a","parameters":{ "type":"object","additionalProperties":true }}],
                "stream": true,
                "stream_mode": "reasoning_text_tool"
            })
            .to_string(),
        ))
        .unwrap();
    let resp = ctx.router.clone().oneshot(req).await.unwrap();
    assert_eq!(resp.status(), StatusCode::OK);
    let bytes = resp.into_body().collect().await.unwrap().to_bytes();
    let text = String::from_utf8_lossy(&bytes).to_string();
    let frames = parse_responses_sse_json(&text);

    assert!(
        frames
            .iter()
            .any(|(event, _)| event == "response.reasoning_summary_text.delta")
    );
    assert!(
        frames
            .iter()
            .any(|(event, _)| event == "response.reasoning_summary_text.done")
    );
    assert!(
        frames
            .iter()
            .any(|(event, _)| event == "response.reasoning_summary_part.done")
    );
    assert!(
        frames
            .iter()
            .any(|(event, _)| event == "response.reasoning.delta")
    );
    assert!(
        frames
            .iter()
            .any(|(event, _)| event == "response.reasoning.done")
    );

    let summary_delta = frames
        .iter()
        .find(|(event, _)| event == "response.reasoning_summary_text.delta")
        .expect("summary delta");
    assert_eq!(summary_delta.1["delta"].as_str(), Some("mock_summary"));

    let text_delta = frames
        .iter()
        .find(|(event, _)| event == "response.reasoning.delta")
        .expect("text delta");
    assert_eq!(text_delta.1["delta"].as_str(), Some("mock_reasoning"));

    let reasoning_done = frames
        .iter()
        .find(|(event, _)| event == "response.reasoning.done")
        .expect("reasoning done");
    assert_eq!(reasoning_done.1["text"].as_str(), Some("mock_reasoning"));
}

#[tokio::test]
async fn responses_streaming_from_responses_upstream_does_not_duplicate_completed_items() {
    let ctx = setup().await;
    let req = Request::builder()
        .method("POST")
        .uri("/v1/responses")
        .header(CONTENT_TYPE, "application/json")
        .header(AUTHORIZATION, ctx.auth_header.clone())
        .body(Body::from(
            json!({
                "model":"gpt-5-mini",
                "input":"stream tool",
                "tools":[{ "type":"function","name":"tool_a","parameters":{ "type":"object","additionalProperties":true }}],
                "stream": true,
                "stream_mode": "message_then_tool_then_completed"
            })
            .to_string(),
        ))
        .unwrap();
    let resp = ctx.router.clone().oneshot(req).await.unwrap();
    assert_eq!(resp.status(), StatusCode::OK);
    let bytes = resp.into_body().collect().await.unwrap().to_bytes();
    let text = String::from_utf8_lossy(&bytes).to_string();
    let frames = parse_responses_sse_json(&text);

    let output_item_added: Vec<&Value> = frames
        .iter()
        .filter(|(event, _)| event == "response.output_item.added")
        .map(|(_, payload)| payload)
        .collect();
    let output_item_done: Vec<&Value> = frames
        .iter()
        .filter(|(event, _)| event == "response.output_item.done")
        .map(|(_, payload)| payload)
        .collect();

    assert_eq!(output_item_added.len(), 2, "must not duplicate added lifecycles: {text}");
    assert_eq!(output_item_done.len(), 2, "must not duplicate done lifecycles: {text}");

    let mut added_types: Vec<&str> = output_item_added
        .iter()
        .filter_map(|payload| payload["item"]["type"].as_str())
        .collect();
    let mut done_types: Vec<&str> = output_item_done
        .iter()
        .filter_map(|payload| payload["item"]["type"].as_str())
        .collect();
    added_types.sort_unstable();
    done_types.sort_unstable();
    assert_eq!(added_types, vec!["function_call", "message"], "unexpected added types: {text}");
    assert_eq!(done_types, vec!["function_call", "message"], "unexpected done types: {text}");

    let completed = frames
        .iter()
        .find(|(event, _)| event == "response.completed")
        .map(|(_, payload)| payload)
        .expect("response.completed frame");
    assert_eq!(
        completed["response"]["output"]
            .as_array()
            .map(Vec::len),
        Some(2),
        "completed output must still contain one message and one function call: {text}"
    );
}

#[tokio::test]
async fn messages_streaming_preserves_upstream_thinking_delta_granularity() {
    let ctx = setup().await;
    let text = collect_messages_stream_text(
        &ctx,
        json!({
            "model": "gpt-5-mini-chat",
            "max_tokens": 64,
            "thinking": { "type": "enabled", "budget_tokens": 2048 },
            "messages": [{ "role": "user", "content": [{ "type": "text", "text": "think stream chat" }] }],
            "stream": true
        }),
    )
    .await;
    let frames = parse_sse_frames(&text);
    let events: Vec<Value> = frames
        .iter()
        .filter_map(|(_, data)| serde_json::from_str::<Value>(data).ok())
        .collect();

    let thinking_deltas: Vec<&str> = events
        .iter()
        .filter(|event| event["delta"]["type"].as_str() == Some("thinking_delta"))
        .filter_map(|event| event["delta"]["thinking"].as_str())
        .collect();
    assert_eq!(
        thinking_deltas,
        vec!["mock_reasoning"],
        "thinking delta should preserve upstream chunking: {text}"
    );
}

#[tokio::test]
async fn chat_streaming_preserves_summary_vs_reasoning_in_openrouter_extension() {
    let ctx = setup().await;
    let req = Request::builder()
        .method("POST")
        .uri("/v1/chat/completions")
        .header(CONTENT_TYPE, "application/json")
        .header(AUTHORIZATION, ctx.auth_header.clone())
        .body(Body::from(
            json!({
                "model":"gpt-5-mini",
                "messages":[{"role":"user","content":"stream tool"}],
                "tools":[{ "type":"function","function":{ "name":"tool_a","parameters":{ "type":"object","additionalProperties":true }}}],
                "stream": true,
                "stream_mode": "reasoning_text_tool"
            })
            .to_string(),
        ))
        .unwrap();
    let resp = ctx.router.clone().oneshot(req).await.unwrap();
    assert_eq!(resp.status(), StatusCode::OK);
    let bytes = resp.into_body().collect().await.unwrap().to_bytes();
    let text = String::from_utf8_lossy(&bytes).to_string();

    assert!(
        text.contains("\"type\":\"reasoning.summary\""),
        "chat stream should expose reasoning summary detail: {text}"
    );
    assert!(
        text.contains("\"summary\":\"mock_summary\""),
        "chat stream should preserve summary field: {text}"
    );
    assert!(
        text.contains("\"type\":\"reasoning.text\""),
        "chat stream should expose reasoning text detail: {text}"
    );
    assert!(
        text.contains("\"text\":\"mock_reasoning\""),
        "chat stream should preserve reasoning text field: {text}"
    );
    assert!(
        !text.contains("\"delta\":{\"reasoning\":"),
        "chat stream should keep structured reasoning out of delta.reasoning: {text}"
    );
    assert!(
        !text.contains("\"signature\":"),
        "OpenRouter reasoning.text details should not carry signature: {text}"
    );
}

#[tokio::test]
async fn chat_streaming_preserves_encrypted_reasoning_from_chat_upstream() {
    let ctx = setup().await;
    let req = Request::builder()
        .method("POST")
        .uri("/v1/chat/completions")
        .header(CONTENT_TYPE, "application/json")
        .header(AUTHORIZATION, ctx.auth_header.clone())
        .body(Body::from(
            json!({
                "model":"gpt-5-mini-chat",
                "messages":[{"role":"user","content":"stream encrypted reasoning"}],
                "reasoning": { "effort": "high" },
                "stream": true
            })
            .to_string(),
        ))
        .unwrap();
    let resp = ctx.router.clone().oneshot(req).await.unwrap();
    assert_eq!(resp.status(), StatusCode::OK);
    let bytes = resp.into_body().collect().await.unwrap().to_bytes();
    let text = String::from_utf8_lossy(&bytes).to_string();

    assert!(
        text.contains("\"type\":\"reasoning.text\""),
        "chat stream should preserve reasoning text detail from chat upstream: {text}"
    );
    assert!(
        text.contains("\"text\":\"mock_reasoning\""),
        "chat stream should preserve reasoning text payload from chat upstream: {text}"
    );
    assert!(
        text.contains("\"type\":\"reasoning.encrypted\""),
        "chat stream should preserve encrypted reasoning detail from chat upstream: {text}"
    );
    assert!(
        text.contains("\"data\":\"mock_sig\""),
        "chat stream should preserve encrypted reasoning payload from chat upstream: {text}"
    );
}

#[tokio::test]
async fn messages_streaming_keeps_signature_in_thinking_block_and_delta_order() {
    let ctx = setup().await;
    let text = collect_messages_stream_text(
        &ctx,
        json!({
            "model": "gpt-5-mini-chat",
            "max_tokens": 64,
            "thinking": { "type": "enabled", "budget_tokens": 2048 },
            "messages": [{ "role": "user", "content": [{ "type": "text", "text": "think stream chat" }] }],
            "stream": true
        }),
    )
    .await;
    let frames = parse_sse_frames(&text);
    let events: Vec<Value> = frames
        .iter()
        .filter_map(|(_, data)| serde_json::from_str::<Value>(data).ok())
        .collect();

    let thinking_start = events
        .iter()
        .find(|event| {
            event["type"].as_str() == Some("content_block_start")
                && event["content_block"]["type"].as_str() == Some("thinking")
        })
        .expect("thinking block start");
    assert_eq!(
        thinking_start["content_block"]["thinking"].as_str(),
        Some("")
    );

    let mut thinking_delta_pos = None;
    let mut signature_delta_pos = None;
    let mut stop_pos = None;
    for (idx, event) in events.iter().enumerate() {
        match event["delta"]["type"].as_str() {
            Some("thinking_delta") if thinking_delta_pos.is_none() => {
                thinking_delta_pos = Some(idx)
            }
            Some("signature_delta") if signature_delta_pos.is_none() => {
                signature_delta_pos = Some(idx)
            }
            _ => {}
        }
        if event["type"].as_str() == Some("content_block_stop") && stop_pos.is_none() {
            stop_pos = Some(idx);
        }
    }
    let thinking_delta_pos = thinking_delta_pos.expect("thinking delta position");
    let signature_delta_pos = signature_delta_pos.expect("signature delta position");
    let stop_pos = stop_pos.expect("stop position");
    assert!(
        thinking_delta_pos < signature_delta_pos,
        "thinking_delta must precede signature_delta: {text}"
    );
    assert!(
        signature_delta_pos < stop_pos,
        "signature_delta must precede content_block_stop: {text}"
    );
}

#[tokio::test]
async fn chat_streaming_emits_single_plain_done_and_no_named_events() {
    let ctx = setup().await;
    let req = Request::builder()
        .method("POST")
        .uri("/v1/chat/completions")
        .header(CONTENT_TYPE, "application/json")
        .header(AUTHORIZATION, ctx.auth_header.clone())
        .body(Body::from(
            json!({
                "model":"gpt-5-mini-chat",
                "messages":[{"role":"user","content":"hello"}],
                "stream": true
            })
            .to_string(),
        ))
        .unwrap();
    let resp = ctx.router.clone().oneshot(req).await.unwrap();
    assert_eq!(resp.status(), StatusCode::OK);
    let bytes = resp.into_body().collect().await.unwrap().to_bytes();
    let text = String::from_utf8_lossy(&bytes).to_string();

    assert_eq!(
        count_done_sentinels(&text),
        1,
        "chat stream must emit one [DONE]"
    );
    assert!(
        !text.lines().any(|line| line.starts_with("event: ")),
        "chat completions stream must be data-only SSE: {text}"
    );
}

#[tokio::test]
async fn responses_streaming_emits_single_plain_done_sentinel() {
    let ctx = setup().await;
    let req = Request::builder()
        .method("POST")
        .uri("/v1/responses")
        .header(CONTENT_TYPE, "application/json")
        .header(AUTHORIZATION, ctx.auth_header.clone())
        .body(Body::from(
            json!({
                "model":"gpt-5-mini",
                "input":[{"type":"message","role":"user","content":[{"type":"input_text","text":"stream"}]}],
                "stream": true
            })
            .to_string(),
        ))
        .unwrap();
    let resp = ctx.router.clone().oneshot(req).await.unwrap();
    assert_eq!(resp.status(), StatusCode::OK);
    let bytes = resp.into_body().collect().await.unwrap().to_bytes();
    let text = String::from_utf8_lossy(&bytes).to_string();

    assert_eq!(
        count_done_sentinels(&text),
        1,
        "responses stream must emit one [DONE]"
    );
    assert!(
        text.contains("\ndata: [DONE]\n"),
        "responses stream must terminate with plain data [DONE]: {text}"
    );
    assert!(
        !text.contains("event: [DONE]") && !text.contains("event: done"),
        "responses [DONE] must not be a named event: {text}"
    );
}

#[tokio::test]
async fn chat_streaming_maps_tool_calls_and_reasoning_from_responses_upstream() {
    let ctx = setup().await;
    let req = Request::builder()
        .method("POST")
        .uri("/v1/chat/completions")
        .header(CONTENT_TYPE, "application/json")
        .header(AUTHORIZATION, ctx.auth_header.clone())
        .body(Body::from(
            json!({
                "model":"gpt-5-mini",
                "messages":[{"role":"user","content":"stream tool"}],
                "tools":[{ "type":"function","function":{ "name":"tool_a","parameters":{ "type":"object","additionalProperties":true }}}],
                "parallel_tool_calls": true,
                "stream": true
            })
            .to_string(),
        ))
        .unwrap();
    let resp = ctx.router.clone().oneshot(req).await.unwrap();
    assert_eq!(resp.status(), StatusCode::OK);
    let bytes = resp.into_body().collect().await.unwrap().to_bytes();
    let text = String::from_utf8_lossy(&bytes).to_string();
    assert!(text.contains("\"tool_calls\""));
    assert!(text.contains("\"reasoning_details\""));
    assert!(text.contains("\"type\":\"reasoning.encrypted\""));
    assert!(text.contains("\"data\":\"mock_sig\""));
    assert!(text.contains("[DONE]"));
}

#[tokio::test]
async fn chat_streaming_maps_tool_calls_from_responses_completed_fallback() {
    let ctx = setup().await;
    let req = Request::builder()
        .method("POST")
        .uri("/v1/chat/completions")
        .header(CONTENT_TYPE, "application/json")
        .header(AUTHORIZATION, ctx.auth_header.clone())
        .body(Body::from(
            json!({
                "model":"gpt-5-mini",
                "messages":[{"role":"user","content":"stream tool"}],
                "tools":[{ "type":"function","function":{ "name":"tool_a","parameters":{ "type":"object","additionalProperties":true }}}],
                "stream": true,
                "stream_mode": "completed_only_tool"
            })
            .to_string(),
        ))
        .unwrap();
    let resp = ctx.router.clone().oneshot(req).await.unwrap();
    assert_eq!(resp.status(), StatusCode::OK);
    let bytes = resp.into_body().collect().await.unwrap().to_bytes();
    let text = String::from_utf8_lossy(&bytes).to_string();
    assert!(text.contains("\"tool_calls\""));
    assert!(text.contains("\"finish_reason\":\"tool_calls\""));
    assert!(text.contains("[DONE]"));
}

#[tokio::test]
async fn chat_streaming_keeps_chat_upstream_terminal_tool_calls_reason() {
    let ctx = setup().await;
    let req = Request::builder()
        .method("POST")
        .uri("/v1/chat/completions")
        .header(CONTENT_TYPE, "application/json")
        .header(AUTHORIZATION, ctx.auth_header.clone())
        .body(Body::from(
            json!({
                "model":"gpt-5-mini-chat",
                "messages":[{"role":"user","content":"stream tool"}],
                "tools":[{ "type":"function","function":{ "name":"tool_a","parameters":{ "type":"object","additionalProperties":true }}}],
                "stream": true
            })
            .to_string(),
        ))
        .unwrap();
    let resp = ctx.router.clone().oneshot(req).await.unwrap();
    assert_eq!(resp.status(), StatusCode::OK);
    let bytes = resp.into_body().collect().await.unwrap().to_bytes();
    let text = String::from_utf8_lossy(&bytes).to_string();

    let mut finish_reasons: Vec<String> = Vec::new();
    for line in text.lines() {
        let Some(payload) = line.strip_prefix("data: ") else {
            continue;
        };
        if payload == "[DONE]" {
            continue;
        }
        let Ok(v) = serde_json::from_str::<Value>(payload) else {
            continue;
        };
        if let Some(reason) = v
            .get("choices")
            .and_then(|v| v.as_array())
            .and_then(|arr| arr.first())
            .and_then(|c| c.get("finish_reason"))
            .and_then(|v| v.as_str())
        {
            if !reason.is_empty() {
                finish_reasons.push(reason.to_string());
            }
        }
    }

    assert_eq!(
        finish_reasons,
        vec!["tool_calls".to_string()],
        "chat upstream terminal finish reasons should be preserved without synthetic stop: {text}"
    );
    assert!(text.contains("[DONE]"));
}

#[tokio::test]
async fn chat_streaming_normalizes_chat_upstream_stop_to_tool_calls_when_tools_emitted() {
    let ctx = setup().await;
    let req = Request::builder()
        .method("POST")
        .uri("/v1/chat/completions")
        .header(CONTENT_TYPE, "application/json")
        .header(AUTHORIZATION, ctx.auth_header.clone())
        .body(Body::from(
            json!({
                "model":"gpt-5-mini-chat",
                "messages":[{"role":"user","content":"stream tool"}],
                "tools":[{ "type":"function","function":{ "name":"tool_a","parameters":{ "type":"object","additionalProperties":true }}}],
                "stream": true,
                "force_finish_reason": "stop"
            })
            .to_string(),
        ))
        .unwrap();
    let resp = ctx.router.clone().oneshot(req).await.unwrap();
    assert_eq!(resp.status(), StatusCode::OK);
    let bytes = resp.into_body().collect().await.unwrap().to_bytes();
    let text = String::from_utf8_lossy(&bytes).to_string();

    let mut finish_reasons: Vec<String> = Vec::new();
    for line in text.lines() {
        let Some(payload) = line.strip_prefix("data: ") else {
            continue;
        };
        if payload == "[DONE]" {
            continue;
        }
        let Ok(v) = serde_json::from_str::<Value>(payload) else {
            continue;
        };
        if let Some(reason) = v
            .get("choices")
            .and_then(|v| v.as_array())
            .and_then(|arr| arr.first())
            .and_then(|c| c.get("finish_reason"))
            .and_then(|v| v.as_str())
        {
            if !reason.is_empty() {
                finish_reasons.push(reason.to_string());
            }
        }
    }

    assert_eq!(
        finish_reasons,
        vec!["tool_calls".to_string()],
        "finish_reason should normalize to tool_calls when tool deltas were emitted: {text}"
    );
    assert!(text.contains("[DONE]"));
}

#[tokio::test]
async fn chat_streaming_parallel_tool_calls_from_chat_upstream_reassembles_arguments() {
    let ctx = setup().await;
    let req = Request::builder()
        .method("POST")
        .uri("/v1/chat/completions")
        .header(CONTENT_TYPE, "application/json")
        .header(AUTHORIZATION, ctx.auth_header.clone())
        .body(Body::from(
            json!({
                "model":"gpt-5-mini-chat",
                "messages":[{"role":"user","content":"stream tool"}],
                "tools":[
                    { "type":"function","function":{ "name":"tool_a","parameters":{ "type":"object","additionalProperties":true }}},
                    { "type":"function","function":{ "name":"tool_b","parameters":{ "type":"object","additionalProperties":true }}}
                ],
                "parallel_tool_calls": true,
                "stream": true
            })
            .to_string(),
        ))
        .unwrap();
    let resp = ctx.router.clone().oneshot(req).await.unwrap();
    assert_eq!(resp.status(), StatusCode::OK);
    let bytes = resp.into_body().collect().await.unwrap().to_bytes();
    let text = String::from_utf8_lossy(&bytes).to_string();

    let mut tool_calls_by_idx: HashMap<u64, (String, String, String)> = HashMap::new();
    let mut finish_reasons: Vec<String> = Vec::new();
    for line in text.lines() {
        let Some(payload) = line.strip_prefix("data: ") else {
            continue;
        };
        if payload == "[DONE]" {
            continue;
        }
        let Ok(v) = serde_json::from_str::<Value>(payload) else {
            continue;
        };
        if let Some(reason) = v
            .get("choices")
            .and_then(|v| v.as_array())
            .and_then(|arr| arr.first())
            .and_then(|c| c.get("finish_reason"))
            .and_then(|v| v.as_str())
        {
            if !reason.is_empty() {
                finish_reasons.push(reason.to_string());
            }
        }
        if let Some(tcs) = v
            .get("choices")
            .and_then(|v| v.as_array())
            .and_then(|arr| arr.first())
            .and_then(|c| c.get("delta"))
            .and_then(|d| d.get("tool_calls"))
            .and_then(|v| v.as_array())
        {
            for tc in tcs {
                let idx = tc.get("index").and_then(|v| v.as_u64()).unwrap_or(0);
                let entry = tool_calls_by_idx
                    .entry(idx)
                    .or_insert_with(|| (String::new(), String::new(), String::new()));
                if let Some(id) = tc.get("id").and_then(|v| v.as_str()) {
                    entry.0 = id.to_string();
                }
                if let Some(name) = tc
                    .get("function")
                    .and_then(|f| f.get("name"))
                    .and_then(|v| v.as_str())
                {
                    entry.1 = name.to_string();
                }
                if let Some(args) = tc
                    .get("function")
                    .and_then(|f| f.get("arguments"))
                    .and_then(|v| v.as_str())
                {
                    entry.2.push_str(args);
                }
            }
        }
    }

    assert_eq!(
        tool_calls_by_idx.len(),
        2,
        "expected 2 parallel tool calls: {text}"
    );
    let tc0 = &tool_calls_by_idx[&0];
    assert_eq!(tc0.0, "call_1");
    assert_eq!(tc0.1, "tool_a");
    assert_eq!(
        tc0.2, "{\"a\":1}",
        "tool_a arguments should be reassembled from fragments"
    );
    let tc1 = &tool_calls_by_idx[&1];
    assert_eq!(tc1.0, "call_2");
    assert_eq!(tc1.1, "tool_b");
    assert_eq!(
        tc1.2, "{\"b\":2}",
        "tool_b arguments should be reassembled from fragments"
    );
    assert_eq!(finish_reasons, vec!["tool_calls".to_string()]);
    assert!(text.contains("[DONE]"));
}

#[tokio::test]
async fn chat_streaming_content_then_tool_call_keeps_finish_reason_terminal() {
    let ctx = setup().await;
    let req = Request::builder()
        .method("POST")
        .uri("/v1/chat/completions")
        .header(CONTENT_TYPE, "application/json")
        .header(AUTHORIZATION, ctx.auth_header.clone())
        .body(Body::from(
            json!({
                "model":"gpt-5-mini-chat",
                "messages":[{"role":"user","content":"stream tool"}],
                "tools":[{ "type":"function","function":{ "name":"tool_a","parameters":{ "type":"object","additionalProperties":true }}}],
                "stream": true,
                "stream_mode": "reasoning_text_tool"
            })
            .to_string(),
        ))
        .unwrap();
    let resp = ctx.router.clone().oneshot(req).await.unwrap();
    assert_eq!(resp.status(), StatusCode::OK);
    let bytes = resp.into_body().collect().await.unwrap().to_bytes();
    let text = String::from_utf8_lossy(&bytes).to_string();

    let mut saw_content = false;
    let mut saw_tool_calls = false;
    let mut terminal_count = 0usize;
    let mut tool_call_seen_before_terminal = false;

    for line in text.lines() {
        let Some(payload) = line.strip_prefix("data: ") else {
            continue;
        };
        if payload == "[DONE]" {
            continue;
        }
        let Ok(v) = serde_json::from_str::<Value>(payload) else {
            continue;
        };
        let choice = v
            .get("choices")
            .and_then(|v| v.as_array())
            .and_then(|arr| arr.first())
            .cloned()
            .unwrap_or(Value::Null);
        let delta = choice.get("delta").cloned().unwrap_or(Value::Null);

        if delta.get("content").and_then(|v| v.as_str()) == Some("answer") {
            saw_content = true;
        }
        if delta.get("tool_calls").and_then(|v| v.as_array()).is_some() {
            saw_tool_calls = true;
            tool_call_seen_before_terminal = true;
        }
        assert!(
            !(delta.get("content").is_some() && delta.get("tool_calls").is_some()),
            "downstream chunk must not co-pack content and tool_calls: {payload}"
        );

        let finish_reason = choice.get("finish_reason").and_then(|v| v.as_str());
        if let Some(reason) = finish_reason {
            terminal_count += 1;
            assert_eq!(
                reason, "tool_calls",
                "unexpected terminal reason: {payload}"
            );
            assert!(
                tool_call_seen_before_terminal,
                "terminal finish_reason arrived before tool_call delta: {text}"
            );
            assert!(
                delta.as_object().map(|obj| obj.is_empty()).unwrap_or(false),
                "terminal finish_reason must be emitted on an empty delta: {payload}"
            );
        }
    }

    assert!(
        saw_content,
        "expected upstream content delta to survive downstream stream: {text}"
    );
    assert!(
        saw_tool_calls,
        "expected downstream tool_call delta: {text}"
    );
    assert_eq!(
        terminal_count, 1,
        "expected exactly one terminal finish_reason chunk: {text}"
    );
    assert!(text.contains("[DONE]"));
}

#[tokio::test]
async fn chat_streaming_content_array_tool_call_keeps_tool_loop_alive() {
    let ctx = setup().await;
    let req = Request::builder()
        .method("POST")
        .uri("/v1/chat/completions")
        .header(CONTENT_TYPE, "application/json")
        .header(AUTHORIZATION, ctx.auth_header.clone())
        .body(Body::from(
            json!({
                "model":"gpt-5-mini-chat",
                "messages":[{"role":"user","content":"stream tool"}],
                "tools":[{ "type":"function","function":{ "name":"tool_a","parameters":{ "type":"object","additionalProperties":true }}}],
                "stream": true,
                "stream_mode": "content_array_tool"
            })
            .to_string(),
        ))
        .unwrap();
    let resp = ctx.router.clone().oneshot(req).await.unwrap();
    assert_eq!(resp.status(), StatusCode::OK);
    let bytes = resp.into_body().collect().await.unwrap().to_bytes();
    let text = String::from_utf8_lossy(&bytes).to_string();

    let mut saw_content = false;
    let mut tool_name = String::new();
    let mut tool_args = String::new();
    let mut finish_reasons: Vec<String> = Vec::new();

    for line in text.lines() {
        let Some(payload) = line.strip_prefix("data: ") else {
            continue;
        };
        if payload == "[DONE]" {
            continue;
        }
        let Ok(v) = serde_json::from_str::<Value>(payload) else {
            continue;
        };
        let choice = v
            .get("choices")
            .and_then(|v| v.as_array())
            .and_then(|arr| arr.first())
            .cloned()
            .unwrap_or(Value::Null);
        let delta = choice.get("delta").cloned().unwrap_or(Value::Null);

        if delta.get("content").and_then(|v| v.as_str()) == Some("answer") {
            saw_content = true;
        }
        if let Some(reason) = choice.get("finish_reason").and_then(|v| v.as_str()) {
            if !reason.is_empty() {
                finish_reasons.push(reason.to_string());
            }
        }
        if let Some(tool_calls) = delta.get("tool_calls").and_then(|v| v.as_array()) {
            for tool_call in tool_calls {
                if let Some(name) = tool_call
                    .get("function")
                    .and_then(|v| v.get("name"))
                    .and_then(|v| v.as_str())
                {
                    tool_name = name.to_string();
                }
                if let Some(arguments) = tool_call
                    .get("function")
                    .and_then(|v| v.get("arguments"))
                    .and_then(|v| v.as_str())
                {
                    tool_args.push_str(arguments);
                }
            }
        }
    }

    assert!(
        saw_content,
        "expected content delta before tool call: {text}"
    );
    assert_eq!(tool_name, "tool_a", "expected decoded tool name: {text}");
    assert_eq!(
        tool_args, "{\"a\":1}",
        "expected reassembled tool arguments: {text}"
    );
    assert_eq!(
        finish_reasons,
        vec!["tool_calls".to_string()],
        "terminal finish_reason should normalize to tool_calls for content-array tool blocks: {text}"
    );
    assert!(text.contains("[DONE]"));
}

#[tokio::test]
async fn chat_streaming_content_array_tool_use_keeps_tool_loop_alive() {
    let ctx = setup().await;
    let req = Request::builder()
        .method("POST")
        .uri("/v1/chat/completions")
        .header(CONTENT_TYPE, "application/json")
        .header(AUTHORIZATION, ctx.auth_header.clone())
        .body(Body::from(
            json!({
                "model":"gpt-5-mini-chat",
                "messages":[{"role":"user","content":"stream tool"}],
                "tools":[{ "type":"function","function":{ "name":"tool_a","parameters":{ "type":"object","additionalProperties":true }}}],
                "stream": true,
                "stream_mode": "content_array_tool_use"
            })
            .to_string(),
        ))
        .unwrap();
    let resp = ctx.router.clone().oneshot(req).await.unwrap();
    assert_eq!(resp.status(), StatusCode::OK);
    let bytes = resp.into_body().collect().await.unwrap().to_bytes();
    let text = String::from_utf8_lossy(&bytes).to_string();

    let mut saw_content = false;
    let mut tool_name = String::new();
    let mut tool_args = String::new();
    let mut finish_reasons: Vec<String> = Vec::new();

    for line in text.lines() {
        let Some(payload) = line.strip_prefix("data: ") else {
            continue;
        };
        if payload == "[DONE]" {
            continue;
        }
        let Ok(v) = serde_json::from_str::<Value>(payload) else {
            continue;
        };
        let choice = v
            .get("choices")
            .and_then(|v| v.as_array())
            .and_then(|arr| arr.first())
            .cloned()
            .unwrap_or(Value::Null);
        let delta = choice.get("delta").cloned().unwrap_or(Value::Null);

        if delta.get("content").and_then(|v| v.as_str()) == Some("answer") {
            saw_content = true;
        }
        if let Some(reason) = choice.get("finish_reason").and_then(|v| v.as_str()) {
            if !reason.is_empty() {
                finish_reasons.push(reason.to_string());
            }
        }
        if let Some(tool_calls) = delta.get("tool_calls").and_then(|v| v.as_array()) {
            for tool_call in tool_calls {
                if let Some(name) = tool_call
                    .get("function")
                    .and_then(|v| v.get("name"))
                    .and_then(|v| v.as_str())
                {
                    tool_name = name.to_string();
                }
                if let Some(arguments) = tool_call
                    .get("function")
                    .and_then(|v| v.get("arguments"))
                    .and_then(|v| v.as_str())
                {
                    tool_args.push_str(arguments);
                }
            }
        }
    }

    assert!(saw_content, "expected content delta before tool call: {text}");
    assert_eq!(tool_name, "tool_a", "expected decoded tool name: {text}");
    assert_eq!(tool_args, "{\"a\":1}", "expected reassembled tool arguments: {text}");
    assert_eq!(
        finish_reasons,
        vec!["tool_calls".to_string()],
        "terminal finish_reason should normalize to tool_calls for content-array tool_use blocks: {text}"
    );
    assert!(text.contains("[DONE]"));
}

#[tokio::test]
async fn chat_streaming_maps_text_from_responses_output_item_done() {
    let ctx = setup().await;
    let req = Request::builder()
        .method("POST")
        .uri("/v1/chat/completions")
        .header(CONTENT_TYPE, "application/json")
        .header(AUTHORIZATION, ctx.auth_header.clone())
        .body(Body::from(
            json!({
                "model":"gpt-5-mini",
                "messages":[{"role":"user","content":"stream plain"}],
                "stream": true,
                "stream_mode": "item_done_only"
            })
            .to_string(),
        ))
        .unwrap();
    let resp = ctx.router.clone().oneshot(req).await.unwrap();
    assert_eq!(resp.status(), StatusCode::OK);
    let bytes = resp.into_body().collect().await.unwrap().to_bytes();
    let text = String::from_utf8_lossy(&bytes).to_string();
    assert!(text.contains("\"content\""));
    assert!(text.contains("[DONE]"));
}

#[tokio::test]
async fn chat_streaming_from_responses_includes_terminal_usage() {
    let ctx = setup().await;
    let req = Request::builder()
        .method("POST")
        .uri("/v1/chat/completions")
        .header(CONTENT_TYPE, "application/json")
        .header(AUTHORIZATION, ctx.auth_header.clone())
        .body(Body::from(
            json!({
                "model":"gpt-5-mini",
                "messages":[{"role":"user","content":"stream usage"}],
                "stream": true,
                "emit_usage": true
            })
            .to_string(),
        ))
        .unwrap();
    let resp = ctx.router.clone().oneshot(req).await.unwrap();
    assert_eq!(resp.status(), StatusCode::OK);
    let bytes = resp.into_body().collect().await.unwrap().to_bytes();
    let text = String::from_utf8_lossy(&bytes).to_string();

    let mut terminal_with_usage: Option<Value> = None;
    for line in text.lines() {
        let Some(payload) = line.strip_prefix("data: ") else {
            continue;
        };
        if payload == "[DONE]" {
            continue;
        }
        let Ok(v) = serde_json::from_str::<Value>(payload) else {
            continue;
        };
        let is_terminal = v
            .get("choices")
            .and_then(|v| v.as_array())
            .and_then(|arr| arr.first())
            .and_then(|c| c.get("finish_reason"))
            .and_then(|v| v.as_str())
            .is_some();
        if is_terminal && v.get("usage").is_some() {
            terminal_with_usage = Some(v);
        }
    }

    let terminal = terminal_with_usage.expect("terminal chat chunk should include usage");
    assert_eq!(terminal["usage"]["prompt_tokens"].as_u64(), Some(12));
    assert_eq!(terminal["usage"]["completion_tokens"].as_u64(), Some(8));
}

#[tokio::test]
async fn messages_streaming_maps_tool_use_and_thinking_from_chat_upstream() {
    let ctx = setup().await;
    let req = Request::builder()
        .method("POST")
        .uri("/v1/messages")
        .header(CONTENT_TYPE, "application/json")
        .header(AUTHORIZATION, ctx.auth_header.clone())
        .body(Body::from(
            json!({
                "model":"gpt-5-mini-chat",
                "messages":[{"role":"user","content":[{"type":"text","text":"stream tool"}]}],
                "tools":[{ "name":"tool_a","input_schema":{ "type":"object","additionalProperties":true }}],
                "stream": true
            })
            .to_string(),
        ))
        .unwrap();
    let resp = ctx.router.clone().oneshot(req).await.unwrap();
    assert_eq!(resp.status(), StatusCode::OK);
    let bytes = resp.into_body().collect().await.unwrap().to_bytes();
    let text = String::from_utf8_lossy(&bytes).to_string();
    assert!(text.contains("\"tool_use\""));
    assert!(text.contains("\"thinking_delta\""));
}

#[tokio::test]
async fn messages_streaming_maps_text_from_responses_output_item_done() {
    let ctx = setup().await;
    let req = Request::builder()
        .method("POST")
        .uri("/v1/messages")
        .header(CONTENT_TYPE, "application/json")
        .header(AUTHORIZATION, ctx.auth_header.clone())
        .body(Body::from(
            json!({
                "model":"gpt-5-mini",
                "messages":[{"role":"user","content":[{"type":"text","text":"stream plain"}]}],
                "stream": true,
                "stream_mode": "item_done_only"
            })
            .to_string(),
        ))
        .unwrap();
    let resp = ctx.router.clone().oneshot(req).await.unwrap();
    assert_eq!(resp.status(), StatusCode::OK);
    let bytes = resp.into_body().collect().await.unwrap().to_bytes();
    let text = String::from_utf8_lossy(&bytes).to_string();
    assert!(text.contains("\"text_delta\""));
    assert!(text.contains("\"message_stop\""));
}

#[tokio::test]
async fn messages_streaming_emits_message_delta_before_stop_for_responses_upstream() {
    let ctx = setup().await;
    let req = Request::builder()
        .method("POST")
        .uri("/v1/messages")
        .header(CONTENT_TYPE, "application/json")
        .header(AUTHORIZATION, ctx.auth_header.clone())
        .body(Body::from(
            json!({
                "model":"gpt-5-mini",
                "messages":[{"role":"user","content":[{"type":"text","text":"stream plain"}]}],
                "stream": true,
                "stream_mode": "item_done_only"
            })
            .to_string(),
        ))
        .unwrap();
    let resp = ctx.router.clone().oneshot(req).await.unwrap();
    assert_eq!(resp.status(), StatusCode::OK);
    let bytes = resp.into_body().collect().await.unwrap().to_bytes();
    let text = String::from_utf8_lossy(&bytes).to_string();
    let delta_pos = text.find("\"message_delta\"").unwrap_or(usize::MAX);
    let stop_pos = text.find("\"message_stop\"").unwrap_or(usize::MAX);
    assert!(
        delta_pos != usize::MAX,
        "expected message_delta in stream: {text}"
    );
    assert!(
        stop_pos != usize::MAX,
        "expected message_stop in stream: {text}"
    );
    assert!(
        delta_pos < stop_pos,
        "message_delta must appear before message_stop: {text}"
    );
}

#[tokio::test]
async fn messages_streaming_from_responses_includes_message_delta_usage() {
    let ctx = setup().await;
    let events = collect_messages_stream_events(
        &ctx,
        json!({
            "model":"gpt-5-mini",
            "messages":[{"role":"user","content":[{"type":"text","text":"stream usage"}]}],
            "stream": true,
            "emit_usage": true
        }),
    )
    .await;

    let msg_delta = events
        .iter()
        .find(|e| e["type"].as_str() == Some("message_delta"))
        .expect("message_delta");
    assert_eq!(msg_delta["usage"]["input_tokens"].as_u64(), Some(12));
    assert_eq!(msg_delta["usage"]["output_tokens"].as_u64(), Some(8));
}

#[tokio::test]
async fn messages_streaming_emits_named_sse_events() {
    let ctx = setup().await;
    let text = collect_messages_stream_text(
        &ctx,
        json!({
            "model": "gpt-5-mini",
            "max_tokens": 64,
            "messages": [{ "role": "user", "content": [{ "type": "text", "text": "stream text" }] }],
            "stream": true
        }),
    )
    .await;
    let frames = parse_sse_frames(&text);
    let first_json = frames
        .iter()
        .find_map(|(event, data)| {
            if data == "[DONE]" {
                return None;
            }
            Some((
                event
                    .clone()
                    .expect("messages frame should have event name"),
                serde_json::from_str::<Value>(data).expect("messages frame should be json"),
            ))
        })
        .expect("at least one messages frame");
    assert_eq!(first_json.0, "message_start");
    assert_eq!(first_json.1["type"].as_str(), Some("message_start"));
    assert!(text.contains("event: message_start"));
    assert!(text.contains("event: content_block_start"));
    assert!(text.contains("event: content_block_delta"));
    assert!(text.contains("event: message_delta"));
    assert!(text.contains("event: message_stop"));
    assert_eq!(
        count_done_sentinels(&text),
        0,
        "messages stream must not append [DONE]"
    );
}

#[tokio::test]
async fn messages_streaming_does_not_duplicate_text_deltas_or_blocks() {
    let ctx = setup().await;
    let events = collect_messages_stream_events(
        &ctx,
        json!({
            "model": "gpt-5-mini-chat",
            "max_tokens": 64,
            "messages": [{ "role": "user", "content": [{ "type": "text", "text": "stream chat text" }] }],
            "stream": true
        }),
    )
    .await;

    let text_deltas: Vec<String> = events
        .iter()
        .filter(|event| {
            event["type"].as_str() == Some("content_block_delta")
                && event["delta"]["type"].as_str() == Some("text_delta")
        })
        .filter_map(|event| event["delta"]["text"].as_str().map(|text| text.to_string()))
        .collect();
    assert_eq!(
        text_deltas,
        vec!["stream chat text".to_string()],
        "text should stream once without full-content replay"
    );

    let text_block_starts = events
        .iter()
        .filter(|event| {
            event["type"].as_str() == Some("content_block_start")
                && event["content_block"]["type"].as_str() == Some("text")
        })
        .count();
    assert_eq!(text_block_starts, 1, "text block should start exactly once");
    assert_non_interleaved_message_blocks(&events, "chat→msg text stream");
}

#[tokio::test]
async fn messages_upstream_sends_anthropic_version_header() {
    let ctx = setup().await;
    let (status, _body) = json_post(
        &ctx,
        "/v1/messages",
        json!({
            "model": "gpt-5-mini-msg",
            "max_tokens": 16,
            "messages": [{ "role": "user", "content": [{ "type": "text", "text": "yo" }] }]
        }),
    )
    .await;
    assert_eq!(status, StatusCode::OK);

    let has_header = ctx
        .captured_headers
        .lock()
        .map(|entries| {
            entries
                .iter()
                .any(|(k, v)| k == "anthropic-version" && v == "2023-06-01")
        })
        .unwrap_or(false);
    assert!(
        has_header,
        "expected anthropic-version header to be forwarded"
    );
}

#[tokio::test]
async fn gemini_native_nonstream_roundtrip_works() {
    let ctx = setup().await;
    let (status, body) = json_post(
        &ctx,
        "/v1/responses",
        json!({
            "model": "gemini-2.5-flash",
            "input": [{ "type": "message", "role": "user", "content": [{ "type": "input_text", "text": "ping" }] }]
        }),
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    let v: Value = serde_json::from_str(&body).unwrap();
    let text = v["output"][0]["content"][0]["text"].as_str().unwrap_or("");
    assert!(
        text.contains("ping|gemini"),
        "unexpected gemini response text: {text}"
    );
}

#[tokio::test]
async fn gemini_native_stream_roundtrip_works() {
    let ctx = setup().await;
    let req = Request::builder()
        .method("POST")
        .uri("/v1/responses")
        .header(CONTENT_TYPE, "application/json")
        .header(AUTHORIZATION, ctx.auth_header.clone())
        .body(Body::from(
            json!({
                "model":"gemini-2.5-flash",
                "input":[{"type":"message","role":"user","content":[{"type":"input_text","text":"stream"}]}],
                "stream": true
            })
            .to_string(),
        ))
        .unwrap();
    let resp = ctx.router.clone().oneshot(req).await.unwrap();
    assert_eq!(resp.status(), StatusCode::OK);
    let bytes = resp.into_body().collect().await.unwrap().to_bytes();
    let text = String::from_utf8_lossy(&bytes).to_string();
    assert!(
        text.contains("event: response.output_text.delta"),
        "missing output delta: {text}"
    );
    assert!(
        text.contains("event: response.completed"),
        "missing completed marker: {text}"
    );

    let has_goog_key = ctx
        .captured_headers
        .lock()
        .map(|entries| entries.iter().any(|(k, _)| k == "x-goog-api-key"))
        .unwrap_or(false);
    assert!(
        has_goog_key,
        "expected x-goog-api-key header for gemini upstream"
    );
}

#[tokio::test]
async fn grok_native_responses_roundtrip_works() {
    let ctx = setup().await;
    let (status, body) = json_post(
        &ctx,
        "/v1/responses",
        json!({
            "model": "grok-4",
            "input": [{ "type": "message", "role": "user", "content": [{ "type": "input_text", "text": "hi grok" }] }]
        }),
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    let v: Value = serde_json::from_str(&body).unwrap();
    let text = v["output"][0]["content"][0]["text"].as_str().unwrap_or("");
    assert!(
        text.contains("hi grok"),
        "unexpected grok response text: {text}"
    );
}

#[tokio::test]
async fn responses_streaming_applies_response_transform_from_provider() {
    let ctx = setup().await;
    let (upstream_addr, _) = start_upstream().await;
    let base_url = format!("http://{upstream_addr}");

    let mut models = HashMap::new();
    models.insert(
        "gpt-5-mini".to_string(),
        monoize::monoize_routing::MonoizeModelEntry {
            redirect: None,
            multiplier: 1.0,
        },
    );
    let create_input = monoize::monoize_routing::CreateMonoizeProviderInput {
        name: "mono-transform-strip".to_string(),
        provider_type: monoize::monoize_routing::MonoizeProviderType::ChatCompletion,
        models,
        api_type_overrides: Vec::new(),
        channels: vec![monoize::monoize_routing::CreateMonoizeChannelInput {
            id: Some("mono-transform-strip-ch1".to_string()),
            name: "mono-transform-strip-ch1".to_string(),
            base_url,
            api_key: Some("upstream-key".to_string()),
            weight: 1,
            enabled: true,
            passive_failure_threshold_override: None,
            passive_cooldown_seconds_override: None,
            passive_window_seconds_override: None,
            passive_min_samples_override: None,
            passive_failure_rate_threshold_override: None,
            passive_rate_limit_cooldown_seconds_override: None,
        }],
        max_retries: -1,
        transforms: vec![monoize::transforms::TransformRuleConfig {
            transform: "strip_reasoning".to_string(),
            enabled: true,
            models: None,
            phase: monoize::transforms::Phase::Response,
            config: json!({}),
        }],
        active_probe_enabled_override: None,
        active_probe_interval_seconds_override: None,
        active_probe_success_threshold_override: None,
        active_probe_model_override: None,
        request_timeout_ms_override: None,
        enabled: true,
        priority: Some(-1),
    };
    ctx.state
        .monoize_store
        .create_provider(create_input)
        .await
        .unwrap();

    let req = Request::builder()
        .method("POST")
        .uri("/v1/responses")
        .header(CONTENT_TYPE, "application/json")
        .header(AUTHORIZATION, ctx.auth_header.clone())
        .body(Body::from(
            json!({
                "model":"gpt-5-mini",
                "input":[{"type":"message","role":"user","content":[{"type":"input_text","text":"stream with reasoning"}]}],
                "stream": true,
                "reasoning": { "effort": "high" }
            })
            .to_string(),
        ))
        .unwrap();
    let resp = ctx.router.clone().oneshot(req).await.unwrap();
    assert_eq!(resp.status(), StatusCode::OK);
    let bytes = resp.into_body().collect().await.unwrap().to_bytes();
    let text = String::from_utf8_lossy(&bytes).to_string();
    assert!(!text.contains("event: response.reasoning_text.delta"));
    assert!(text.contains("event: response.output_text.delta"));
    assert!(text.contains("event: response.completed"));
}

#[tokio::test]
async fn responses_streaming_split_sse_frames_breaks_large_delta_frames() {
    let max_frame_length = 220usize;
    let ctx = setup().await;
    let (upstream_addr, _) = start_upstream().await;
    let base_url = format!("http://{upstream_addr}");

    let mut models = HashMap::new();
    models.insert(
        "gpt-5-mini".to_string(),
        monoize::monoize_routing::MonoizeModelEntry {
            redirect: None,
            multiplier: 1.0,
        },
    );
    let create_input = monoize::monoize_routing::CreateMonoizeProviderInput {
        name: "mono-transform-sse-split".to_string(),
        provider_type: monoize::monoize_routing::MonoizeProviderType::Responses,
        models,
        api_type_overrides: Vec::new(),
        channels: vec![monoize::monoize_routing::CreateMonoizeChannelInput {
            id: Some("mono-transform-sse-split-ch1".to_string()),
            name: "mono-transform-sse-split-ch1".to_string(),
            base_url,
            api_key: Some("upstream-key".to_string()),
            weight: 1,
            enabled: true,
            passive_failure_threshold_override: None,
            passive_cooldown_seconds_override: None,
            passive_window_seconds_override: None,
            passive_min_samples_override: None,
            passive_failure_rate_threshold_override: None,
            passive_rate_limit_cooldown_seconds_override: None,
        }],
        max_retries: -1,
        transforms: vec![monoize::transforms::TransformRuleConfig {
            transform: "split_sse_frames".to_string(),
            enabled: true,
            models: None,
            phase: monoize::transforms::Phase::Response,
            config: json!({ "max_frame_length": max_frame_length }),
        }],
        active_probe_enabled_override: None,
        active_probe_interval_seconds_override: None,
        active_probe_success_threshold_override: None,
        active_probe_model_override: None,
        request_timeout_ms_override: None,
        enabled: true,
        priority: Some(-1),
    };
    ctx.state
        .monoize_store
        .create_provider(create_input)
        .await
        .unwrap();

    let long_input = "abcdefghij".repeat(80);
    let req = Request::builder()
        .method("POST")
        .uri("/v1/responses")
        .header(CONTENT_TYPE, "application/json")
        .header(AUTHORIZATION, ctx.auth_header.clone())
        .body(Body::from(
            json!({
                "model":"gpt-5-mini",
                "input":[{"type":"message","role":"user","content":[{"type":"input_text","text": long_input}]}],
                "stream": true
            })
            .to_string(),
        ))
        .unwrap();
    let resp = ctx.router.clone().oneshot(req).await.unwrap();
    assert_eq!(resp.status(), StatusCode::OK);
    let bytes = resp.into_body().collect().await.unwrap().to_bytes();
    let text = String::from_utf8_lossy(&bytes).to_string();

    let delta_events = text.matches("event: response.output_text.delta").count();
    assert!(
        delta_events >= 2,
        "expected split_sse_frames to emit multiple delta events, body={text}"
    );
    assert!(text.contains("event: response.completed"));

    let mut reconstructed = String::new();
    let mut current_event = String::new();
    for line in text.lines() {
        if let Some(event) = line.strip_prefix("event: ") {
            current_event = event.to_string();
            continue;
        }
        if let Some(payload) = line.strip_prefix("data: ") {
            if current_event == "response.output_text.delta" {
                assert!(
                    payload.len() <= max_frame_length,
                    "expected split output_text delta payloads to respect max_frame_length, len={}, payload={payload}",
                    payload.len()
                );
                let value: Value = serde_json::from_str(payload).expect("delta payload json");
                let piece = value.get("delta").and_then(|v| v.as_str()).unwrap_or("");
                reconstructed.push_str(piece);
            }
        }
    }
    assert_eq!(reconstructed, "abcdefghij".repeat(80));
}

#[tokio::test]
async fn responses_streaming_plaintext_reasoning_to_summary_rewrites_reasoning_events() {
    let ctx = setup().await;
    let (upstream_addr, _) = start_upstream().await;
    let base_url = format!("http://{upstream_addr}");

    let mut models = HashMap::new();
    models.insert(
        "gpt-5-mini".to_string(),
        monoize::monoize_routing::MonoizeModelEntry {
            redirect: None,
            multiplier: 1.0,
        },
    );
    ctx.state
        .monoize_store
        .create_provider(monoize::monoize_routing::CreateMonoizeProviderInput {
            name: "mono-transform-summary".to_string(),
            provider_type: monoize::monoize_routing::MonoizeProviderType::Responses,
            models,
            api_type_overrides: Vec::new(),
            channels: vec![monoize::monoize_routing::CreateMonoizeChannelInput {
                id: Some("mono-transform-summary-ch1".to_string()),
                name: "mono-transform-summary-ch1".to_string(),
                base_url,
                api_key: Some("upstream-key".to_string()),
                weight: 1,
                enabled: true,
                passive_failure_threshold_override: None,
                passive_cooldown_seconds_override: None,
                passive_window_seconds_override: None,
                passive_min_samples_override: None,
                passive_failure_rate_threshold_override: None,
                passive_rate_limit_cooldown_seconds_override: None,
            }],
            max_retries: -1,
            transforms: vec![monoize::transforms::TransformRuleConfig {
                transform: "plaintext_reasoning_to_summary".to_string(),
                enabled: true,
                models: None,
                phase: monoize::transforms::Phase::Response,
                config: json!({}),
            }],
            active_probe_enabled_override: None,
            active_probe_interval_seconds_override: None,
            active_probe_success_threshold_override: None,
            active_probe_model_override: None,
            request_timeout_ms_override: None,
            enabled: true,
            priority: Some(-1),
        })
        .await
        .unwrap();

    let req = Request::builder()
        .method("POST")
        .uri("/v1/responses")
        .header(CONTENT_TYPE, "application/json")
        .header(AUTHORIZATION, ctx.auth_header.clone())
        .body(Body::from(
            json!({
                "model":"gpt-5-mini",
                "input":[{"type":"message","role":"user","content":[{"type":"input_text","text":"stream with reasoning"}]}],
                "stream": true,
                "reasoning": { "effort": "high" }
            })
            .to_string(),
        ))
        .unwrap();
    let resp = ctx.router.clone().oneshot(req).await.unwrap();
    assert_eq!(resp.status(), StatusCode::OK);
    let bytes = resp.into_body().collect().await.unwrap().to_bytes();
    let text = String::from_utf8_lossy(&bytes).to_string();

    assert!(text.contains("event: response.reasoning_summary_text.delta"));
    assert!(!text.contains("event: response.reasoning.delta"));
    assert!(text.contains("event: response.output_text.delta"));
}

#[tokio::test]
async fn chat_streaming_plaintext_reasoning_to_summary_rewrites_reasoning_events() {
    let ctx = setup().await;
    let (upstream_addr, _) = start_upstream().await;
    let base_url = format!("http://{upstream_addr}");

    let mut models = HashMap::new();
    models.insert(
        "gpt-5-mini".to_string(),
        monoize::monoize_routing::MonoizeModelEntry {
            redirect: None,
            multiplier: 1.0,
        },
    );
    ctx.state
        .monoize_store
        .create_provider(monoize::monoize_routing::CreateMonoizeProviderInput {
            name: "mono-transform-summary-chat".to_string(),
            provider_type: monoize::monoize_routing::MonoizeProviderType::Responses,
            models,
            api_type_overrides: Vec::new(),
            channels: vec![monoize::monoize_routing::CreateMonoizeChannelInput {
                id: Some("mono-transform-summary-chat-ch1".to_string()),
                name: "mono-transform-summary-chat-ch1".to_string(),
                base_url,
                api_key: Some("upstream-key".to_string()),
                weight: 1,
                enabled: true,
                passive_failure_threshold_override: None,
                passive_cooldown_seconds_override: None,
                passive_window_seconds_override: None,
                passive_min_samples_override: None,
                passive_failure_rate_threshold_override: None,
                passive_rate_limit_cooldown_seconds_override: None,
            }],
            max_retries: -1,
            transforms: vec![monoize::transforms::TransformRuleConfig {
                transform: "plaintext_reasoning_to_summary".to_string(),
                enabled: true,
                models: None,
                phase: monoize::transforms::Phase::Response,
                config: json!({}),
            }],
            active_probe_enabled_override: None,
            active_probe_interval_seconds_override: None,
            active_probe_success_threshold_override: None,
            active_probe_model_override: None,
            request_timeout_ms_override: None,
            enabled: true,
            priority: Some(-1),
        })
        .await
        .unwrap();

    let req = Request::builder()
        .method("POST")
        .uri("/v1/chat/completions")
        .header(CONTENT_TYPE, "application/json")
        .header(AUTHORIZATION, ctx.auth_header.clone())
        .body(Body::from(
            json!({
                "model":"gpt-5-mini",
                "messages":[{"role":"user","content":"stream with reasoning"}],
                "stream": true,
                "reasoning": { "effort": "high" }
            })
            .to_string(),
        ))
        .unwrap();
    let resp = ctx.router.clone().oneshot(req).await.unwrap();
    assert_eq!(resp.status(), StatusCode::OK);
    let bytes = resp.into_body().collect().await.unwrap().to_bytes();
    let text = String::from_utf8_lossy(&bytes).to_string();

    assert!(
        text.contains("\"type\":\"reasoning.summary\""),
        "chat stream should expose reasoning summary detail: {text}"
    );
    assert!(
        text.contains("\"summary\":\"mock_reasoning\""),
        "chat stream should move plaintext reasoning into summary: {text}"
    );
    assert!(
        !text.contains("\"type\":\"reasoning.text\""),
        "chat stream should not emit reasoning.text after summary transform: {text}"
    );
}

#[tokio::test]
async fn chat_streaming_plaintext_reasoning_to_summary_preserves_encrypted_reasoning() {
    let ctx = setup().await;
    let (upstream_addr, _) = start_upstream().await;
    let base_url = format!("http://{upstream_addr}");

    let mut models = HashMap::new();
    models.insert(
        "gpt-5-mini".to_string(),
        monoize::monoize_routing::MonoizeModelEntry {
            redirect: None,
            multiplier: 1.0,
        },
    );
    ctx.state
        .monoize_store
        .create_provider(monoize::monoize_routing::CreateMonoizeProviderInput {
            name: "mono-transform-summary-chat-encrypted".to_string(),
            provider_type: monoize::monoize_routing::MonoizeProviderType::Responses,
            models,
            api_type_overrides: Vec::new(),
            channels: vec![monoize::monoize_routing::CreateMonoizeChannelInput {
                id: Some("mono-transform-summary-chat-encrypted-ch1".to_string()),
                name: "mono-transform-summary-chat-encrypted-ch1".to_string(),
                base_url,
                api_key: Some("upstream-key".to_string()),
                weight: 1,
                enabled: true,
                passive_failure_threshold_override: None,
                passive_cooldown_seconds_override: None,
                passive_window_seconds_override: None,
                passive_min_samples_override: None,
                passive_failure_rate_threshold_override: None,
                passive_rate_limit_cooldown_seconds_override: None,
            }],
            max_retries: -1,
            transforms: vec![monoize::transforms::TransformRuleConfig {
                transform: "plaintext_reasoning_to_summary".to_string(),
                enabled: true,
                models: None,
                phase: monoize::transforms::Phase::Response,
                config: json!({}),
            }],
            active_probe_enabled_override: None,
            active_probe_interval_seconds_override: None,
            active_probe_success_threshold_override: None,
            active_probe_model_override: None,
            request_timeout_ms_override: None,
            enabled: true,
            priority: Some(-1),
        })
        .await
        .unwrap();

    let req = Request::builder()
        .method("POST")
        .uri("/v1/chat/completions")
        .header(CONTENT_TYPE, "application/json")
        .header(AUTHORIZATION, ctx.auth_header.clone())
        .body(Body::from(
            json!({
                "model":"gpt-5-mini",
                "messages":[{"role":"user","content":"stream tool"}],
                "tools":[{ "type":"function","function":{ "name":"tool_a","parameters":{ "type":"object","additionalProperties":true }}}],
                "parallel_tool_calls": true,
                "stream": true,
                "reasoning": { "effort": "high" }
            })
            .to_string(),
        ))
        .unwrap();
    let resp = ctx.router.clone().oneshot(req).await.unwrap();
    assert_eq!(resp.status(), StatusCode::OK);
    let bytes = resp.into_body().collect().await.unwrap().to_bytes();
    let text = String::from_utf8_lossy(&bytes).to_string();

    assert!(
        text.contains("\"type\":\"reasoning.summary\""),
        "chat stream should expose reasoning summary detail: {text}"
    );
    assert!(
        text.contains("\"summary\":\"mock_reasoning\""),
        "chat stream should move plaintext reasoning into summary: {text}"
    );
    assert!(
        !text.contains("\"type\":\"reasoning.text\""),
        "chat stream should not emit reasoning.text after summary transform: {text}"
    );
    assert!(
        text.contains("\"type\":\"reasoning.encrypted\""),
        "chat stream should preserve encrypted reasoning detail: {text}"
    );
    assert!(
        text.contains("\"data\":\"mock_sig\""),
        "chat stream should preserve encrypted reasoning payload: {text}"
    );
}

#[tokio::test]
async fn messages_streaming_plaintext_reasoning_to_summary_preserves_thinking_delta() {
    let ctx = setup().await;
    let (upstream_addr, _) = start_upstream().await;
    let base_url = format!("http://{upstream_addr}");

    let mut models = HashMap::new();
    models.insert(
        "gpt-5-mini".to_string(),
        monoize::monoize_routing::MonoizeModelEntry {
            redirect: None,
            multiplier: 1.0,
        },
    );
    ctx.state
        .monoize_store
        .create_provider(monoize::monoize_routing::CreateMonoizeProviderInput {
            name: "mono-transform-summary-messages".to_string(),
            provider_type: monoize::monoize_routing::MonoizeProviderType::Responses,
            models,
            api_type_overrides: Vec::new(),
            channels: vec![monoize::monoize_routing::CreateMonoizeChannelInput {
                id: Some("mono-transform-summary-messages-ch1".to_string()),
                name: "mono-transform-summary-messages-ch1".to_string(),
                base_url,
                api_key: Some("upstream-key".to_string()),
                weight: 1,
                enabled: true,
                passive_failure_threshold_override: None,
                passive_cooldown_seconds_override: None,
                passive_window_seconds_override: None,
                passive_min_samples_override: None,
                passive_failure_rate_threshold_override: None,
                passive_rate_limit_cooldown_seconds_override: None,
            }],
            max_retries: -1,
            transforms: vec![monoize::transforms::TransformRuleConfig {
                transform: "plaintext_reasoning_to_summary".to_string(),
                enabled: true,
                models: None,
                phase: monoize::transforms::Phase::Response,
                config: json!({}),
            }],
            active_probe_enabled_override: None,
            active_probe_interval_seconds_override: None,
            active_probe_success_threshold_override: None,
            active_probe_model_override: None,
            request_timeout_ms_override: None,
            enabled: true,
            priority: Some(-1),
        })
        .await
        .unwrap();

    let text = collect_messages_stream_text(
        &ctx,
        json!({
            "model": "gpt-5-mini",
            "max_tokens": 64,
            "thinking": { "type": "enabled", "budget_tokens": 2048 },
            "messages": [{ "role": "user", "content": [{ "type": "text", "text": "stream with reasoning" }] }],
            "stream": true
        }),
    )
    .await;
    let events: Vec<Value> = parse_sse_frames(&text)
        .into_iter()
        .filter_map(|(_, data)| serde_json::from_str::<Value>(&data).ok())
        .collect();

    let thinking_deltas: Vec<&str> = events
        .iter()
        .filter(|event| event["delta"]["type"].as_str() == Some("thinking_delta"))
        .filter_map(|event| event["delta"]["thinking"].as_str())
        .collect();
    assert_eq!(
        thinking_deltas,
        vec!["mock_reasoning"],
        "messages stream should preserve the transformed reasoning summary as thinking text: {text}"
    );

    assert!(
        events.iter().any(|event| {
            event["type"].as_str() == Some("content_block_start")
                && event["content_block"]["type"].as_str() == Some("thinking")
        }),
        "expected a thinking block after summary transform: {text}"
    );
}

#[tokio::test]
async fn responses_nonstream_markdown_image_transforms_extract_and_append_markdown() {
    let ctx = setup().await;
    let (upstream_addr, _) = start_upstream().await;
    let base_url = format!("http://{upstream_addr}");

    let mut models = HashMap::new();
    models.insert(
        "gpt-5-mini".to_string(),
        monoize::monoize_routing::MonoizeModelEntry {
            redirect: None,
            multiplier: 1.0,
        },
    );
    ctx.state
        .monoize_store
        .create_provider(monoize::monoize_routing::CreateMonoizeProviderInput {
            name: "mono-transform-markdown-images".to_string(),
            provider_type: monoize::monoize_routing::MonoizeProviderType::Responses,
            models,
            api_type_overrides: Vec::new(),
            channels: vec![monoize::monoize_routing::CreateMonoizeChannelInput {
                id: Some("mono-transform-markdown-images-ch1".to_string()),
                name: "mono-transform-markdown-images-ch1".to_string(),
                base_url,
                api_key: Some("upstream-key".to_string()),
                weight: 1,
                enabled: true,
                passive_failure_threshold_override: None,
                passive_cooldown_seconds_override: None,
                passive_window_seconds_override: None,
                passive_min_samples_override: None,
                passive_failure_rate_threshold_override: None,
                passive_rate_limit_cooldown_seconds_override: None,
            }],
            max_retries: -1,
            transforms: vec![
                monoize::transforms::TransformRuleConfig {
                    transform: "assistant_markdown_images_to_output".to_string(),
                    enabled: true,
                    models: None,
                    phase: monoize::transforms::Phase::Response,
                    config: json!({}),
                },
                monoize::transforms::TransformRuleConfig {
                    transform: "assistant_output_images_to_markdown".to_string(),
                    enabled: true,
                    models: None,
                    phase: monoize::transforms::Phase::Response,
                    config: json!({}),
                },
            ],
            active_probe_enabled_override: None,
            active_probe_interval_seconds_override: None,
            active_probe_success_threshold_override: None,
            active_probe_model_override: None,
            request_timeout_ms_override: None,
            enabled: true,
            priority: Some(-1),
        })
        .await
        .unwrap();

    let image_markdown = "![chart](https://example.com/chart.png)";
    let default_appended_markdown = "![image](https://example.com/chart.png)";
    let (status, body) = json_post(
        &ctx,
        "/v1/responses",
        json!({
            "model": "gpt-5-mini",
            "input": [{
                "type": "message",
                "role": "user",
                "content": [{ "type": "input_text", "text": format!("see {image_markdown}") }]
            }]
        }),
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    let v: Value = serde_json::from_str(&body).unwrap();
    let output = v["output"].as_array().expect("output array");
    assert_eq!(output.len(), 1);
    let content = output[0]["content"].as_array().expect("content array");
    assert_eq!(content.len(), 2);
    assert_eq!(content[0]["type"].as_str(), Some("output_text"));
    let text = content[0]["text"].as_str().expect("text content");
    assert!(text.contains("see "));
    assert!(text.contains(default_appended_markdown));
    assert_eq!(content[1]["type"].as_str(), Some("output_image"));
    assert_eq!(
        content[1]["url"].as_str(),
        Some("https://example.com/chart.png")
    );
}

#[tokio::test]
async fn responses_streaming_markdown_image_transforms_emit_image_part_and_appended_markdown() {
    let ctx = setup().await;
    let (upstream_addr, _) = start_upstream().await;
    let base_url = format!("http://{upstream_addr}");

    let mut models = HashMap::new();
    models.insert(
        "gpt-5-mini".to_string(),
        monoize::monoize_routing::MonoizeModelEntry {
            redirect: None,
            multiplier: 1.0,
        },
    );
    ctx.state
        .monoize_store
        .create_provider(monoize::monoize_routing::CreateMonoizeProviderInput {
            name: "mono-transform-streaming-markdown-images".to_string(),
            provider_type: monoize::monoize_routing::MonoizeProviderType::Responses,
            models,
            api_type_overrides: Vec::new(),
            channels: vec![monoize::monoize_routing::CreateMonoizeChannelInput {
                id: Some("mono-transform-streaming-markdown-images-ch1".to_string()),
                name: "mono-transform-streaming-markdown-images-ch1".to_string(),
                base_url,
                api_key: Some("upstream-key".to_string()),
                weight: 1,
                enabled: true,
                passive_failure_threshold_override: None,
                passive_cooldown_seconds_override: None,
                passive_window_seconds_override: None,
                passive_min_samples_override: None,
                passive_failure_rate_threshold_override: None,
                passive_rate_limit_cooldown_seconds_override: None,
            }],
            max_retries: -1,
            transforms: vec![
                monoize::transforms::TransformRuleConfig {
                    transform: "assistant_markdown_images_to_output".to_string(),
                    enabled: true,
                    models: None,
                    phase: monoize::transforms::Phase::Response,
                    config: json!({}),
                },
                monoize::transforms::TransformRuleConfig {
                    transform: "assistant_output_images_to_markdown".to_string(),
                    enabled: true,
                    models: None,
                    phase: monoize::transforms::Phase::Response,
                    config: json!({}),
                },
            ],
            active_probe_enabled_override: None,
            active_probe_interval_seconds_override: None,
            active_probe_success_threshold_override: None,
            active_probe_model_override: None,
            request_timeout_ms_override: None,
            enabled: true,
            priority: Some(-1),
        })
        .await
        .unwrap();

    let image_markdown = "![chart](https://example.com/chart.png)";
    let req = Request::builder()
        .method("POST")
        .uri("/v1/responses")
        .header(CONTENT_TYPE, "application/json")
        .header(AUTHORIZATION, ctx.auth_header.clone())
        .body(Body::from(
            json!({
                "model":"gpt-5-mini",
                "input":[{"type":"message","role":"user","content":[{"type":"input_text","text": format!("see {image_markdown}")}]}],
                "stream": true
            })
            .to_string(),
        ))
        .unwrap();
    let resp = ctx.router.clone().oneshot(req).await.unwrap();
    assert_eq!(resp.status(), StatusCode::OK);
    let bytes = resp.into_body().collect().await.unwrap().to_bytes();
    let text = String::from_utf8_lossy(&bytes).to_string();
    let frames = parse_responses_sse_json(&text);

    assert!(frames.iter().any(|(event, payload)| {
        event == "response.output_text.delta"
            && payload["delta"]
                .as_str()
                .is_some_and(|delta| delta.contains("![image](https://example.com/chart.png)"))
    }));
    assert!(frames.iter().any(|(event, payload)| {
        event == "response.completed"
            && payload["response"]["output"]
                .as_array()
                .is_some_and(|output| {
                    output.iter().any(|item| {
                        item["type"].as_str() == Some("message")
                            && item["content"].as_array().is_some_and(|content| {
                                content.iter().any(|part| {
                                    part["type"].as_str() == Some("output_image")
                                        && part["url"].as_str()
                                            == Some("https://example.com/chart.png")
                                })
                            })
                    })
                })
    }));
}

#[tokio::test]
async fn provider_request_transform_matches_normalized_model_before_redirect() {
    let ctx = setup().await;
    seed_test_model_pricing(&ctx.state, &["gpt-5-target"]).await;
    let (upstream_addr, _) = start_upstream().await;
    let base_url = format!("http://{upstream_addr}");

    let mut models = HashMap::new();
    models.insert(
        "normalized-transform-model".to_string(),
        monoize::monoize_routing::MonoizeModelEntry {
            redirect: Some("gpt-5-target".to_string()),
            multiplier: 1.0,
        },
    );

    let create_input = monoize::monoize_routing::CreateMonoizeProviderInput {
        name: "mono-transform-original-model-match".to_string(),
        provider_type: monoize::monoize_routing::MonoizeProviderType::Responses,
        models,
        api_type_overrides: Vec::new(),
        channels: vec![monoize::monoize_routing::CreateMonoizeChannelInput {
            id: Some("mono-transform-original-model-match-ch1".to_string()),
            name: "mono-transform-original-model-match-ch1".to_string(),
            base_url,
            api_key: Some("upstream-key".to_string()),
            weight: 1,
            enabled: true,
            passive_failure_threshold_override: None,
            passive_cooldown_seconds_override: None,
            passive_window_seconds_override: None,
            passive_min_samples_override: None,
            passive_failure_rate_threshold_override: None,
            passive_rate_limit_cooldown_seconds_override: None,
        }],
        max_retries: -1,
        transforms: vec![monoize::transforms::TransformRuleConfig {
            transform: "set_field".to_string(),
            enabled: true,
            models: Some(vec!["normalized-transform-model".to_string()]),
            phase: monoize::transforms::Phase::Request,
            config: json!({
                "path": "extra_echo",
                "value": "matched-original-model"
            }),
        }],
        active_probe_enabled_override: None,
        active_probe_interval_seconds_override: None,
        active_probe_success_threshold_override: None,
        active_probe_model_override: None,
        request_timeout_ms_override: None,
        enabled: true,
        priority: Some(-1),
    };

    ctx.state
        .monoize_store
        .create_provider(create_input)
        .await
        .unwrap();

    let (status, body) = json_post(
        &ctx,
        "/v1/responses",
        json!({
            "model": "normalized-transform-model-high",
            "input": [{ "type": "message", "role": "user", "content": [{ "type": "input_text", "text": "hello" }] }]
        }),
    )
    .await;

    assert_eq!(status, StatusCode::OK);
    let v: Value = serde_json::from_str(&body).unwrap();
    let text = v["output"]
        .as_array()
        .and_then(|items| {
            items
                .iter()
                .find(|item| item["type"].as_str() == Some("message"))
        })
        .and_then(|item| item["content"].as_array())
        .and_then(|content| content.first())
        .and_then(|part| part["text"].as_str())
        .unwrap_or("");
    assert!(
        text.contains("extra_echo=matched-original-model"),
        "expected request transform to match normalized logical model before redirect: text={text}; body={body}"
    );
}

#[tokio::test]
async fn messages_nonstream_from_responses_upstream_text() {
    let ctx = setup().await;
    let (status, body) = json_post(
        &ctx,
        "/v1/messages",
        json!({
            "model": "gpt-5-mini",
            "max_tokens": 64,
            "messages": [{ "role": "user", "content": [{ "type": "text", "text": "hello resp" }] }]
        }),
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    let v: Value = serde_json::from_str(&body).unwrap();
    assert_eq!(v["type"].as_str(), Some("message"));
    assert_eq!(v["role"].as_str(), Some("assistant"));
    let blocks = v["content"].as_array().expect("content array");
    let text_block = blocks
        .iter()
        .find(|b| b["type"].as_str() == Some("text"))
        .expect("text block");
    assert!(
        text_block["text"].as_str().unwrap().contains("hello resp"),
        "expected echoed text"
    );
}

#[tokio::test]
async fn messages_nonstream_from_chat_upstream_text() {
    let ctx = setup().await;
    let (status, body) = json_post(
        &ctx,
        "/v1/messages",
        json!({
            "model": "gpt-5-mini-chat",
            "max_tokens": 64,
            "messages": [{ "role": "user", "content": [{ "type": "text", "text": "hello chat" }] }]
        }),
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    let v: Value = serde_json::from_str(&body).unwrap();
    assert_eq!(v["type"].as_str(), Some("message"));
    assert_eq!(v["role"].as_str(), Some("assistant"));
    let text = v["content"][0]["text"].as_str().unwrap_or("");
    assert!(text.contains("hello chat"));
}

#[tokio::test]
async fn messages_nonstream_from_gemini_upstream_text() {
    let ctx = setup().await;
    let (status, body) = json_post(
        &ctx,
        "/v1/messages",
        json!({
            "model": "gemini-2.5-flash",
            "max_tokens": 64,
            "messages": [{ "role": "user", "content": [{ "type": "text", "text": "hello gem" }] }]
        }),
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    let v: Value = serde_json::from_str(&body).unwrap();
    assert_eq!(v["type"].as_str(), Some("message"));
    assert_eq!(v["role"].as_str(), Some("assistant"));
    let text = v["content"][0]["text"].as_str().unwrap_or("");
    assert!(
        text.contains("hello gem|gemini"),
        "unexpected gemini->messages text: {text}"
    );
}

#[tokio::test]
async fn messages_nonstream_from_grok_upstream_text() {
    let ctx = setup().await;
    let (status, body) = json_post(
        &ctx,
        "/v1/messages",
        json!({
            "model": "grok-4",
            "max_tokens": 64,
            "messages": [{ "role": "user", "content": [{ "type": "text", "text": "hello grok" }] }]
        }),
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    let v: Value = serde_json::from_str(&body).unwrap();
    assert_eq!(v["type"].as_str(), Some("message"));
    assert_eq!(v["role"].as_str(), Some("assistant"));
    let text = v["content"][0]["text"].as_str().unwrap_or("");
    assert!(
        text.contains("hello grok"),
        "unexpected grok->messages text: {text}"
    );
}

#[tokio::test]
async fn messages_nonstream_response_shape_validation() {
    let ctx = setup().await;
    let (status, body) = json_post(
        &ctx,
        "/v1/messages",
        json!({
            "model": "gpt-5-mini",
            "max_tokens": 64,
            "messages": [{ "role": "user", "content": [{ "type": "text", "text": "shape check" }] }]
        }),
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    let v: Value = serde_json::from_str(&body).unwrap();

    assert!(v["id"].as_str().is_some(), "missing id");
    assert_eq!(v["type"].as_str(), Some("message"), "type must be message");
    assert_eq!(
        v["role"].as_str(),
        Some("assistant"),
        "role must be assistant"
    );
    assert!(v["model"].as_str().is_some(), "missing model");
    assert!(v["content"].as_array().is_some(), "missing content array");
    assert!(
        v["stop_reason"].as_str().is_some(),
        "missing stop_reason: {v}"
    );
    assert!(v["usage"].is_object(), "missing usage object: {v}");
    assert!(
        v["usage"]["input_tokens"].is_number(),
        "missing input_tokens"
    );
    assert!(
        v["usage"]["output_tokens"].is_number(),
        "missing output_tokens"
    );
}

#[tokio::test]
async fn messages_nonstream_thinking_from_responses_upstream() {
    let ctx = setup().await;
    let (status, body) = json_post(
        &ctx,
        "/v1/messages",
        json!({
            "model": "gpt-5-mini",
            "max_tokens": 64,
            "thinking": { "type": "enabled", "budget_tokens": 2048 },
            "messages": [{ "role": "user", "content": [{ "type": "text", "text": "show reasoning" }] }]
        }),
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    let v: Value = serde_json::from_str(&body).unwrap();
    let blocks = v["content"].as_array().cloned().unwrap_or_default();
    let thinking = blocks
        .iter()
        .find(|b| b["type"].as_str() == Some("thinking"));
    assert!(thinking.is_some(), "expected thinking block: {v}");
    let thinking = thinking.unwrap();
    assert!(
        thinking["thinking"]
            .as_str()
            .unwrap_or("")
            .contains("mock_reasoning"),
        "expected reasoning text"
    );

    let text = blocks.iter().find(|b| b["type"].as_str() == Some("text"));
    assert!(text.is_some(), "expected text block after thinking");
}

#[tokio::test]
async fn messages_nonstream_thinking_from_messages_upstream() {
    let ctx = setup().await;
    let (status, body) = json_post(
        &ctx,
        "/v1/messages",
        json!({
            "model": "gpt-5-mini-msg",
            "max_tokens": 64,
            "thinking": { "type": "enabled", "budget_tokens": 2048 },
            "messages": [{ "role": "user", "content": [{ "type": "text", "text": "show reasoning" }] }]
        }),
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    let v: Value = serde_json::from_str(&body).unwrap();
    let blocks = v["content"].as_array().cloned().unwrap_or_default();
    assert!(
        blocks
            .iter()
            .any(|b| b["type"].as_str() == Some("thinking")),
        "expected thinking block from messages upstream: {v}"
    );
    assert!(
        blocks.iter().any(|b| b["type"].as_str() == Some("text")),
        "expected text block: {v}"
    );
}

#[tokio::test]
async fn messages_nonstream_stop_reason_tool_use() {
    let ctx = setup().await;
    let (status, body) = json_post(
        &ctx,
        "/v1/messages",
        json!({
            "model": "gpt-5-mini",
            "max_tokens": 64,
            "messages": [{ "role": "user", "content": [{ "type": "text", "text": "use tools" }] }],
            "tools": [{ "name": "tool_a", "input_schema": { "type": "object" } }]
        }),
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    let v: Value = serde_json::from_str(&body).unwrap();
    assert_eq!(
        v["stop_reason"].as_str(),
        Some("tool_use"),
        "stop_reason must be tool_use when tools are returned: {v}"
    );
    let blocks = v["content"].as_array().cloned().unwrap_or_default();
    assert!(
        blocks
            .iter()
            .any(|b| b["type"].as_str() == Some("tool_use")),
        "expected tool_use block"
    );
}

async fn collect_messages_stream_events(ctx: &TestContext, body: Value) -> Vec<Value> {
    let req = Request::builder()
        .method("POST")
        .uri("/v1/messages")
        .header(CONTENT_TYPE, "application/json")
        .header(AUTHORIZATION, ctx.auth_header.clone())
        .body(Body::from(body.to_string()))
        .unwrap();
    let resp = ctx.router.clone().oneshot(req).await.unwrap();
    assert_eq!(resp.status(), StatusCode::OK);
    let bytes = resp.into_body().collect().await.unwrap().to_bytes();
    let text = String::from_utf8_lossy(&bytes).to_string();
    text.lines()
        .filter(|l| l.starts_with("data: "))
        .filter_map(|l| {
            let payload = l.strip_prefix("data: ").unwrap();
            serde_json::from_str::<Value>(payload).ok()
        })
        .collect()
}

async fn collect_messages_stream_text(ctx: &TestContext, body: Value) -> String {
    let req = Request::builder()
        .method("POST")
        .uri("/v1/messages")
        .header(CONTENT_TYPE, "application/json")
        .header(AUTHORIZATION, ctx.auth_header.clone())
        .body(Body::from(body.to_string()))
        .unwrap();
    let resp = ctx.router.clone().oneshot(req).await.unwrap();
    assert_eq!(resp.status(), StatusCode::OK);
    let bytes = resp.into_body().collect().await.unwrap().to_bytes();
    String::from_utf8_lossy(&bytes).to_string()
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

fn message_block_event_sequence(events: &[Value]) -> Vec<(u64, String)> {
    events
        .iter()
        .filter_map(|event| {
            let index = event.get("index").and_then(Value::as_u64)?;
            let event_type = event.get("type").and_then(Value::as_str)?;
            if !matches!(
                event_type,
                "content_block_start" | "content_block_delta" | "content_block_stop"
            ) {
                return None;
            }
            Some((index, event_type.to_string()))
        })
        .collect()
}

fn assert_non_interleaved_message_blocks(events: &[Value], label: &str) {
    let sequence = message_block_event_sequence(events);
    let mut active_block: Option<u64> = None;
    let mut seen_starts: HashMap<u64, usize> = HashMap::new();
    let mut seen_stops: HashMap<u64, usize> = HashMap::new();

    for (index, event_type) in sequence {
        match event_type.as_str() {
            "content_block_start" => {
                assert!(
                    active_block.is_none(),
                    "{label}: block {index} started while block {:?} was still open",
                    active_block
                );
                *seen_starts.entry(index).or_insert(0) += 1;
                active_block = Some(index);
            }
            "content_block_delta" => {
                assert_eq!(
                    active_block,
                    Some(index),
                    "{label}: delta for block {index} appeared while active block was {:?}",
                    active_block
                );
            }
            "content_block_stop" => {
                assert_eq!(
                    active_block,
                    Some(index),
                    "{label}: stop for block {index} appeared while active block was {:?}",
                    active_block
                );
                *seen_stops.entry(index).or_insert(0) += 1;
                active_block = None;
            }
            _ => unreachable!(),
        }
    }

    assert!(active_block.is_none(), "{label}: final block left open");
    for (index, starts) in seen_starts {
        assert_eq!(starts, 1, "{label}: block {index} started {starts} times");
        assert_eq!(
            seen_stops.get(&index).copied().unwrap_or_default(),
            1,
            "{label}: block {index} must stop exactly once"
        );
    }
}

fn assert_messages_stream_invariants(events: &[Value], label: &str) {
    assert!(!events.is_empty(), "{label}: expected at least one event");
    assert_eq!(
        events.first().unwrap()["type"].as_str(),
        Some("message_start"),
        "{label}: first event must be message_start"
    );
    let msg = &events.first().unwrap()["message"];
    assert_eq!(
        msg["type"].as_str(),
        Some("message"),
        "{label}: message_start.message.type"
    );
    assert_eq!(
        msg["role"].as_str(),
        Some("assistant"),
        "{label}: message_start.message.role"
    );

    assert_eq!(
        events.last().unwrap()["type"].as_str(),
        Some("message_stop"),
        "{label}: last event must be message_stop"
    );
    let second_last = &events[events.len() - 2];
    assert_eq!(
        second_last["type"].as_str(),
        Some("message_delta"),
        "{label}: second-to-last event must be message_delta"
    );
    assert!(
        second_last["delta"]["stop_reason"].as_str().is_some(),
        "{label}: message_delta must have stop_reason"
    );

    let starts: Vec<u64> = events
        .iter()
        .filter(|e| e["type"].as_str() == Some("content_block_start"))
        .filter_map(|e| e["index"].as_u64())
        .collect();
    let stops: Vec<u64> = events
        .iter()
        .filter(|e| e["type"].as_str() == Some("content_block_stop"))
        .filter_map(|e| e["index"].as_u64())
        .collect();
    for idx in &starts {
        assert!(
            stops.contains(idx),
            "{label}: content_block_start(index={idx}) has no matching stop"
        );
    }

    for idx in starts {
        let lifecycle: Vec<&str> = events
            .iter()
            .filter(|event| event["index"].as_u64() == Some(idx))
            .filter_map(|event| event["type"].as_str())
            .collect();
        assert!(
            !lifecycle.is_empty(),
            "{label}: expected lifecycle events for block {idx}"
        );
        assert_eq!(
            lifecycle.first().copied(),
            Some("content_block_start"),
            "{label}: block {idx} must start with content_block_start"
        );
        assert_eq!(
            lifecycle.last().copied(),
            Some("content_block_stop"),
            "{label}: block {idx} must end with content_block_stop"
        );
        assert_eq!(
            lifecycle
                .iter()
                .filter(|ty| **ty == "content_block_start")
                .count(),
            1,
            "{label}: block {idx} must have exactly one start"
        );
        assert_eq!(
            lifecycle
                .iter()
                .filter(|ty| **ty == "content_block_stop")
                .count(),
            1,
            "{label}: block {idx} must have exactly one stop"
        );
        let stop_pos = lifecycle
            .iter()
            .position(|ty| *ty == "content_block_stop")
            .expect("stop position");
        assert!(
            lifecycle[..stop_pos]
                .iter()
                .all(|ty| matches!(*ty, "content_block_start" | "content_block_delta")),
            "{label}: block {idx} contains non-delta event before stop"
        );
    }
}

#[tokio::test]
async fn messages_stream_text_from_responses_upstream_event_sequence() {
    let ctx = setup().await;
    let events = collect_messages_stream_events(
        &ctx,
        json!({
            "model": "gpt-5-mini",
            "max_tokens": 64,
            "messages": [{ "role": "user", "content": [{ "type": "text", "text": "stream text" }] }],
            "stream": true
        }),
    )
    .await;

    assert_messages_stream_invariants(&events, "resp→msg stream");
    let has_text_delta = events
        .iter()
        .any(|e| e.get("delta").and_then(|d| d["type"].as_str()) == Some("text_delta"));
    assert!(has_text_delta, "expected text_delta in stream");
}

#[tokio::test]
async fn messages_stream_text_from_chat_upstream_event_sequence() {
    let ctx = setup().await;
    let events = collect_messages_stream_events(
        &ctx,
        json!({
            "model": "gpt-5-mini-chat",
            "max_tokens": 64,
            "messages": [{ "role": "user", "content": [{ "type": "text", "text": "stream chat text" }] }],
            "stream": true
        }),
    )
    .await;

    assert_messages_stream_invariants(&events, "chat→msg stream");
    let has_text_delta = events
        .iter()
        .any(|e| e.get("delta").and_then(|d| d["type"].as_str()) == Some("text_delta"));
    assert!(has_text_delta, "expected text_delta from chat upstream");
}

#[tokio::test]
async fn messages_stream_text_from_gemini_upstream_event_sequence() {
    let ctx = setup().await;
    let events = collect_messages_stream_events(
        &ctx,
        json!({
            "model": "gemini-2.5-flash",
            "max_tokens": 64,
            "messages": [{ "role": "user", "content": [{ "type": "text", "text": "stream gem text" }] }],
            "stream": true
        }),
    )
    .await;

    assert_messages_stream_invariants(&events, "gemini→msg stream");
    let has_text_delta = events
        .iter()
        .any(|e| e.get("delta").and_then(|d| d["type"].as_str()) == Some("text_delta"));
    assert!(has_text_delta, "expected text_delta from gemini upstream");
}

#[tokio::test]
async fn messages_stream_text_from_grok_upstream_event_sequence() {
    let ctx = setup().await;
    let events = collect_messages_stream_events(
        &ctx,
        json!({
            "model": "grok-4",
            "max_tokens": 64,
            "messages": [{ "role": "user", "content": [{ "type": "text", "text": "stream grok text" }] }],
            "stream": true
        }),
    )
    .await;

    assert_messages_stream_invariants(&events, "grok→msg stream");
    let has_text_delta = events
        .iter()
        .any(|e| e.get("delta").and_then(|d| d["type"].as_str()) == Some("text_delta"));
    assert!(has_text_delta, "expected text_delta from grok upstream");
}

#[tokio::test]
async fn messages_stream_passthrough_from_messages_upstream() {
    let ctx = setup().await;
    let events = collect_messages_stream_events(
        &ctx,
        json!({
            "model": "gpt-5-mini-msg",
            "max_tokens": 64,
            "messages": [{ "role": "user", "content": [{ "type": "text", "text": "stream passthrough" }] }],
            "stream": true
        }),
    )
    .await;

    assert!(!events.is_empty(), "expected events from passthrough");
    assert_eq!(
        events.first().unwrap()["type"].as_str(),
        Some("message_start"),
        "passthrough should start with message_start"
    );
}

#[tokio::test]
async fn messages_stream_thinking_from_responses_upstream() {
    let ctx = setup().await;
    let events = collect_messages_stream_events(
        &ctx,
        json!({
            "model": "gpt-5-mini",
            "max_tokens": 64,
            "thinking": { "type": "enabled", "budget_tokens": 2048 },
            "messages": [{ "role": "user", "content": [{ "type": "text", "text": "think stream" }] }],
            "stream": true,
            "stream_mode": "reasoning_text_tool",
            "tools": [{ "name": "tool_a", "input_schema": { "type": "object", "additionalProperties": true } }]
        }),
    )
    .await;

    assert_messages_stream_invariants(&events, "resp→msg thinking stream");

    let has_thinking_delta = events
        .iter()
        .any(|e| e.get("delta").and_then(|d| d["type"].as_str()) == Some("thinking_delta"));
    assert!(
        has_thinking_delta,
        "expected thinking_delta from responses upstream"
    );

    let has_text_delta = events
        .iter()
        .any(|e| e.get("delta").and_then(|d| d["type"].as_str()) == Some("text_delta"));
    let has_tool_use_start = events.iter().any(|e| {
        e["type"].as_str() == Some("content_block_start")
            && e["content_block"]["type"].as_str() == Some("tool_use")
    });
    assert!(
        has_text_delta || has_tool_use_start,
        "expected downstream content or tool_use block alongside thinking"
    );

    let has_signature_delta = events
        .iter()
        .any(|e| e.get("delta").and_then(|d| d["type"].as_str()) == Some("signature_delta"));
    assert!(
        has_signature_delta,
        "expected signature_delta from responses upstream"
    );
}

#[tokio::test]
async fn messages_stream_thinking_from_chat_upstream() {
    let ctx = setup().await;
    let events = collect_messages_stream_events(
        &ctx,
        json!({
            "model": "gpt-5-mini-chat",
            "max_tokens": 64,
            "thinking": { "type": "enabled", "budget_tokens": 2048 },
            "messages": [{ "role": "user", "content": [{ "type": "text", "text": "think stream chat" }] }],
            "stream": true
        }),
    )
    .await;

    assert_messages_stream_invariants(&events, "chat→msg thinking stream");

    let has_thinking_delta = events
        .iter()
        .any(|e| e.get("delta").and_then(|d| d["type"].as_str()) == Some("thinking_delta"));
    assert!(
        has_thinking_delta,
        "expected thinking_delta from chat upstream"
    );

    let has_signature_delta = events
        .iter()
        .any(|e| e.get("delta").and_then(|d| d["type"].as_str()) == Some("signature_delta"));
    assert!(
        has_signature_delta,
        "expected signature_delta from chat upstream"
    );
}

#[tokio::test]
async fn messages_stream_signature_delta_does_not_precede_thinking_delta() {
    let ctx = setup().await;
    let events = collect_messages_stream_events(
        &ctx,
        json!({
            "model": "gpt-5-mini",
            "max_tokens": 64,
            "thinking": { "type": "enabled", "budget_tokens": 2048 },
            "messages": [{ "role": "user", "content": [{ "type": "text", "text": "think stream" }] }],
            "stream": true,
            "stream_mode": "reasoning_text_tool",
            "tools": [{ "name": "tool_a", "input_schema": { "type": "object", "additionalProperties": true } }]
        }),
    )
    .await;

    let thinking_delta_pos = events
        .iter()
        .position(|event| event["delta"]["type"].as_str() == Some("thinking_delta"));
    let signature_delta_pos = events
        .iter()
        .position(|event| event["delta"]["type"].as_str() == Some("signature_delta"));

    let thinking_delta_pos = thinking_delta_pos.expect("thinking delta position");
    let signature_delta_pos = signature_delta_pos.expect("signature delta position");
    assert!(
        thinking_delta_pos < signature_delta_pos,
        "signature_delta must not precede thinking_delta"
    );
}

#[tokio::test]
async fn messages_stream_tool_use_from_responses_upstream() {
    let ctx = setup().await;
    let events = collect_messages_stream_events(
        &ctx,
        json!({
            "model": "gpt-5-mini",
            "max_tokens": 64,
            "messages": [{ "role": "user", "content": [{ "type": "text", "text": "tool stream" }] }],
            "tools": [{ "name": "tool_a", "input_schema": { "type": "object", "additionalProperties": true } }],
            "stream": true
        }),
    )
    .await;

    assert_messages_stream_invariants(&events, "resp→msg tool stream");

    let tool_start = events.iter().find(|e| {
        e["type"].as_str() == Some("content_block_start")
            && e["content_block"]["type"].as_str() == Some("tool_use")
    });
    assert!(
        tool_start.is_some(),
        "expected tool_use content_block_start"
    );
    let tool_start = tool_start.unwrap();
    assert!(
        tool_start["content_block"]["name"].as_str().is_some(),
        "tool_use block must have name"
    );
    assert!(
        tool_start["content_block"]["id"].as_str().is_some(),
        "tool_use block must have id"
    );

    let has_input_json = events
        .iter()
        .any(|e| e.get("delta").and_then(|d| d["type"].as_str()) == Some("input_json_delta"));
    assert!(has_input_json, "expected input_json_delta in tool stream");

    let msg_delta = events
        .iter()
        .find(|e| e["type"].as_str() == Some("message_delta"))
        .expect("message_delta");
    assert_eq!(
        msg_delta["delta"]["stop_reason"].as_str(),
        Some("tool_use"),
        "stop_reason must be tool_use"
    );
}

#[tokio::test]
async fn messages_stream_tool_use_from_responses_completed_fallback() {
    let ctx = setup().await;
    let events = collect_messages_stream_events(
        &ctx,
        json!({
            "model": "gpt-5-mini",
            "max_tokens": 64,
            "messages": [{ "role": "user", "content": [{ "type": "text", "text": "tool stream" }] }],
            "tools": [{ "name": "tool_a", "input_schema": { "type": "object", "additionalProperties": true } }],
            "stream": true,
            "stream_mode": "completed_only_tool"
        }),
    )
    .await;

    assert_messages_stream_invariants(&events, "resp→msg completed fallback");

    let tool_start = events.iter().find(|e| {
        e["type"].as_str() == Some("content_block_start")
            && e["content_block"]["type"].as_str() == Some("tool_use")
    });
    assert!(
        tool_start.is_some(),
        "expected tool_use content_block_start from completed fallback"
    );
    let has_input_json = events
        .iter()
        .any(|e| e.get("delta").and_then(|d| d["type"].as_str()) == Some("input_json_delta"));
    assert!(
        has_input_json,
        "expected input_json_delta from completed fallback"
    );
    let msg_delta = events
        .iter()
        .find(|e| e["type"].as_str() == Some("message_delta"))
        .expect("message_delta");
    assert_eq!(
        msg_delta["delta"]["stop_reason"].as_str(),
        Some("tool_use"),
        "stop_reason must be tool_use"
    );
}

#[tokio::test]
async fn messages_stream_parallel_tool_use_from_chat_upstream() {
    let ctx = setup().await;
    let events = collect_messages_stream_events(
        &ctx,
        json!({
            "model": "gpt-5-mini-chat",
            "max_tokens": 64,
            "messages": [{ "role": "user", "content": [{ "type": "text", "text": "parallel tools" }] }],
            "tools": [
                { "name": "tool_a", "input_schema": { "type": "object", "additionalProperties": true } },
                { "name": "tool_b", "input_schema": { "type": "object", "additionalProperties": true } }
            ],
            "parallel_tool_calls": true,
            "stream": true
        }),
    )
    .await;

    assert_messages_stream_invariants(&events, "chat→msg parallel tool stream");

    let has_thinking = events
        .iter()
        .any(|e| e.get("delta").and_then(|d| d["type"].as_str()) == Some("thinking_delta"));
    assert!(has_thinking, "expected thinking_delta with tool calls");

    let tool_starts: Vec<&Value> = events
        .iter()
        .filter(|e| {
            e["type"].as_str() == Some("content_block_start")
                && e["content_block"]["type"].as_str() == Some("tool_use")
        })
        .collect();
    assert!(
        !tool_starts.is_empty(),
        "expected at least one tool_use block"
    );
    assert_non_interleaved_message_blocks(&events, "chat→msg parallel tool stream");
}

#[tokio::test]
async fn messages_streaming_from_chat_preserves_strict_block_order_in_raw_sse() {
    let ctx = setup().await;
    let text = collect_messages_stream_text(
        &ctx,
        json!({
            "model": "gpt-5-mini-chat",
            "max_tokens": 64,
            "thinking": { "type": "enabled", "budget_tokens": 2048 },
            "messages": [{ "role": "user", "content": [{ "type": "text", "text": "parallel tools" }] }],
            "tools": [
                { "name": "tool_a", "input_schema": { "type": "object", "additionalProperties": true } },
                { "name": "tool_b", "input_schema": { "type": "object", "additionalProperties": true } }
            ],
            "parallel_tool_calls": true,
            "stream": true
        }),
    )
    .await;
    let events: Vec<Value> = parse_sse_frames(&text)
        .into_iter()
        .filter_map(|(_, data)| serde_json::from_str::<Value>(&data).ok())
        .collect();
    assert_non_interleaved_message_blocks(&events, "raw chat→msg mixed stream");
}

#[tokio::test]
async fn messages_streaming_from_responses_preserves_strict_block_order_in_raw_sse() {
    let ctx = setup().await;
    let text = collect_messages_stream_text(
        &ctx,
        json!({
            "model": "gpt-5-mini",
            "max_tokens": 64,
            "thinking": { "type": "enabled", "budget_tokens": 2048 },
            "messages": [{ "role": "user", "content": [{ "type": "text", "text": "think stream" }] }],
            "stream": true
        }),
    )
    .await;
    let events: Vec<Value> = parse_sse_frames(&text)
        .into_iter()
        .filter_map(|(_, data)| serde_json::from_str::<Value>(&data).ok())
        .collect();
    assert_non_interleaved_message_blocks(&events, "raw responses→msg mixed stream");
}

#[tokio::test]
async fn messages_stream_message_start_envelope_fields() {
    let ctx = setup().await;
    let events = collect_messages_stream_events(
        &ctx,
        json!({
            "model": "gpt-5-mini",
            "max_tokens": 64,
            "messages": [{ "role": "user", "content": [{ "type": "text", "text": "envelope check" }] }],
            "stream": true
        }),
    )
    .await;

    let msg_start = events.first().expect("at least one event");
    assert_eq!(msg_start["type"].as_str(), Some("message_start"));
    let msg = &msg_start["message"];
    assert!(msg["id"].as_str().is_some(), "message_start must have id");
    assert_eq!(msg["type"].as_str(), Some("message"));
    assert_eq!(msg["role"].as_str(), Some("assistant"));
    assert!(msg["model"].as_str().is_some(), "must have model");
    assert!(
        msg["content"].as_array().is_some(),
        "must have content array"
    );
    assert!(
        msg["stop_reason"].is_null(),
        "stop_reason should be null at start"
    );
    assert!(
        msg["stop_sequence"].is_null(),
        "stop_sequence should be null at start"
    );
    assert!(msg["usage"].is_object(), "must have usage at start");
}

#[tokio::test]
async fn messages_tool_choice_tool_normalizes_for_chat_upstream() {
    let ctx = setup().await;
    let (status, body) = json_post(
        &ctx,
        "/v1/messages",
        json!({
            "model": "gpt-5-mini-chat",
            "max_tokens": 64,
            "messages": [{ "role": "user", "content": [{ "type": "text", "text": "use tools" }] }],
            "tools": [
              { "name": "tool_a", "input_schema": { "type": "object", "additionalProperties": true } }
            ],
            "tool_choice": { "type": "tool", "name": "tool_a" }
        }),
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    let v: Value = serde_json::from_str(&body).unwrap();
    let blocks = v["content"].as_array().cloned().unwrap_or_default();
    assert!(
        blocks
            .iter()
            .any(|b| b.get("type").and_then(|x| x.as_str()) == Some("tool_use"))
    );
}

#[tokio::test]
async fn models_list_respects_api_key_model_limits() {
    let ctx = setup().await;

    let (status, body) = json_get(&ctx, "/v1/models").await;
    assert_eq!(status, StatusCode::OK);
    let v: Value = serde_json::from_str(&body).unwrap();
    let all_ids: Vec<String> = v["data"]
        .as_array()
        .unwrap()
        .iter()
        .map(|item| item["id"].as_str().unwrap().to_string())
        .collect();
    assert!(all_ids.len() > 2, "should have multiple models");

    let user = ctx
        .state
        .user_store
        .get_user_by_username("tenant-1")
        .await
        .expect("get user")
        .expect("user exists");
    let (_, restricted_token) = ctx
        .state
        .user_store
        .create_api_key_extended(
            &user.id,
            monoize::users::CreateApiKeyInput {
                name: "restricted-key".to_string(),
                expires_in_days: None,
                quota: None,
                quota_unlimited: true,
                model_limits_enabled: true,
                model_limits: vec!["gpt-5-mini".to_string(), "grok-4".to_string()],
                ip_whitelist: Vec::new(),
                group: "default".to_string(),
                max_multiplier: None,
                transforms: Vec::new(),
            },
            false,
        )
        .await
        .expect("create restricted api key");

    let req = Request::builder()
        .method("GET")
        .uri("/v1/models")
        .header(AUTHORIZATION, format!("Bearer {restricted_token}"))
        .body(Body::empty())
        .unwrap();
    let resp = ctx.router.clone().oneshot(req).await.unwrap();
    assert_eq!(resp.status(), StatusCode::OK);
    let bytes = resp.into_body().collect().await.unwrap().to_bytes();
    let v: Value = serde_json::from_str(&String::from_utf8_lossy(&bytes)).unwrap();
    let restricted_ids: Vec<String> = v["data"]
        .as_array()
        .unwrap()
        .iter()
        .map(|item| item["id"].as_str().unwrap().to_string())
        .collect();

    assert_eq!(restricted_ids, vec!["gpt-5-mini", "grok-4"]);
}

#[tokio::test]
async fn models_list_model_limits_disabled_shows_all() {
    let ctx = setup().await;

    let user = ctx
        .state
        .user_store
        .get_user_by_username("tenant-1")
        .await
        .expect("get user")
        .expect("user exists");
    let (_, token) = ctx
        .state
        .user_store
        .create_api_key_extended(
            &user.id,
            monoize::users::CreateApiKeyInput {
                name: "disabled-limits-key".to_string(),
                expires_in_days: None,
                quota: None,
                quota_unlimited: true,
                model_limits_enabled: false,
                model_limits: vec!["gpt-5-mini".to_string()],
                ip_whitelist: Vec::new(),
                group: "default".to_string(),
                max_multiplier: None,
                transforms: Vec::new(),
            },
            false,
        )
        .await
        .expect("create api key with disabled limits");

    let req = Request::builder()
        .method("GET")
        .uri("/v1/models")
        .header(AUTHORIZATION, format!("Bearer {token}"))
        .body(Body::empty())
        .unwrap();
    let resp = ctx.router.clone().oneshot(req).await.unwrap();
    assert_eq!(resp.status(), StatusCode::OK);
    let bytes = resp.into_body().collect().await.unwrap().to_bytes();
    let v: Value = serde_json::from_str(&String::from_utf8_lossy(&bytes)).unwrap();
    let ids: Vec<String> = v["data"]
        .as_array()
        .unwrap()
        .iter()
        .map(|item| item["id"].as_str().unwrap().to_string())
        .collect();

    assert!(
        ids.len() > 1,
        "should return all models when limits disabled"
    );
}

#[tokio::test]
async fn create_api_key_rejects_disallowed_transform() {
    let ctx = setup().await;
    let cookie = dashboard_session_cookie(&ctx, "tenant-1", "test-password").await;

    let req = Request::builder()
        .method("POST")
        .uri("/api/dashboard/tokens")
        .header(CONTENT_TYPE, "application/json")
        .header("cookie", cookie)
        .body(Body::from(
            json!({
                "name": "unsafe-transform-key",
                "transforms": [
                    {
                        "transform": "set_field",
                        "enabled": true,
                        "models": ["gpt-5.4-fast"],
                        "phase": "request",
                        "config": {
                            "path": "service_tier",
                            "value": "priority"
                        }
                    }
                ]
            })
            .to_string(),
        ))
        .unwrap();

    let resp = ctx.router.clone().oneshot(req).await.unwrap();
    assert_eq!(resp.status(), StatusCode::BAD_REQUEST);
    let bytes = resp.into_body().collect().await.unwrap().to_bytes();
    let v: Value = serde_json::from_slice(&bytes).unwrap();
    assert_eq!(v["error"]["code"].as_str(), Some("invalid_request"));
}

#[tokio::test]
async fn create_api_key_allows_new_response_transforms() {
    let ctx = setup().await;
    let cookie = dashboard_session_cookie(&ctx, "tenant-1", "test-password").await;

    let req = Request::builder()
        .method("POST")
        .uri("/api/dashboard/tokens")
        .header(CONTENT_TYPE, "application/json")
        .header("cookie", cookie)
        .body(Body::from(
            json!({
                "name": "safe-transform-key",
                "transforms": [
                    {
                        "transform": "plaintext_reasoning_to_summary",
                        "enabled": true,
                        "phase": "response",
                        "config": {}
                    },
                    {
                        "transform": "assistant_markdown_images_to_output",
                        "enabled": true,
                        "phase": "response",
                        "config": {}
                    },
                    {
                        "transform": "assistant_output_images_to_markdown",
                        "enabled": true,
                        "phase": "response",
                        "config": { "template": "![preview]({{src}})" }
                    }
                ]
            })
            .to_string(),
        ))
        .unwrap();

    let resp = ctx.router.clone().oneshot(req).await.unwrap();
    assert_eq!(resp.status(), StatusCode::CREATED);
    let bytes = resp.into_body().collect().await.unwrap().to_bytes();
    let v: Value = serde_json::from_slice(&bytes).unwrap();
    let transforms = v["transforms"].as_array().expect("transforms array");
    assert_eq!(transforms.len(), 3);
}

#[tokio::test]
async fn forwarding_rejects_models_outside_api_key_model_limits() {
    let ctx = setup().await;
    let user = ctx
        .state
        .user_store
        .get_user_by_username("tenant-1")
        .await
        .unwrap()
        .unwrap();

    let (_, token) = ctx
        .state
        .user_store
        .create_api_key_extended(
            &user.id,
            monoize::users::CreateApiKeyInput {
                name: "restricted-forward-key".to_string(),
                expires_in_days: None,
                quota: None,
                quota_unlimited: true,
                model_limits_enabled: true,
                model_limits: vec!["gpt-5-mini".to_string()],
                ip_whitelist: vec![],
                group: "default".to_string(),
                max_multiplier: None,
                transforms: vec![],
            },
            false,
        )
        .await
        .unwrap();

    let req = Request::builder()
        .method("POST")
        .uri("/v1/responses")
        .header(CONTENT_TYPE, "application/json")
        .header(AUTHORIZATION, format!("Bearer {token}"))
        .body(Body::from(
            json!({
                "model": "grok-4",
                "input": [{ "type": "message", "role": "user", "content": [{ "type": "input_text", "text": "hi" }] }]
            })
            .to_string(),
        ))
        .unwrap();

    let resp = ctx.router.clone().oneshot(req).await.unwrap();
    assert_eq!(resp.status(), StatusCode::FORBIDDEN);
    let bytes = resp.into_body().collect().await.unwrap().to_bytes();
    let v: Value = serde_json::from_slice(&bytes).unwrap();
    assert_eq!(v["error"]["code"].as_str(), Some("model_not_allowed"));
}

#[tokio::test]
async fn auth_missing_authorization_header() {
    let ctx = setup().await;
    let req = Request::builder()
        .method("POST")
        .uri("/v1/responses")
        .header(CONTENT_TYPE, "application/json")
        .body(Body::from(
            json!({"model":"gpt-5-mini","input":"hi"}).to_string(),
        ))
        .unwrap();
    let resp = ctx.router.clone().oneshot(req).await.unwrap();
    assert_eq!(resp.status(), StatusCode::UNAUTHORIZED);
    let bytes = resp.into_body().collect().await.unwrap().to_bytes();
    let v: Value = serde_json::from_str(&String::from_utf8_lossy(&bytes)).unwrap();
    assert_eq!(v["error"]["code"].as_str(), Some("unauthorized"));
    assert_eq!(v["error"]["message"].as_str(), Some("missing auth"));
}

#[tokio::test]
async fn auth_accepts_x_api_key_header() {
    let ctx = setup().await;
    let token = ctx
        .auth_header
        .strip_prefix("Bearer ")
        .expect("bearer token");
    let req = Request::builder()
        .method("POST")
        .uri("/v1/responses")
        .header(CONTENT_TYPE, "application/json")
        .header("x-api-key", token)
        .body(Body::from(
            json!({"model":"gpt-5-mini","input":"hi"}).to_string(),
        ))
        .unwrap();
    let resp = ctx.router.clone().oneshot(req).await.unwrap();
    assert_eq!(resp.status(), StatusCode::OK);
    let bytes = resp.into_body().collect().await.unwrap().to_bytes();
    let v: Value = serde_json::from_slice(&bytes).unwrap();
    assert_eq!(v["output"][0]["content"][0]["text"].as_str(), Some("hi"));
}

#[tokio::test]
async fn auth_no_bearer_prefix() {
    let ctx = setup().await;
    let req = Request::builder()
        .method("POST")
        .uri("/v1/responses")
        .header(CONTENT_TYPE, "application/json")
        .header(AUTHORIZATION, "Token sk-test123456")
        .body(Body::from(
            json!({"model":"gpt-5-mini","input":"hi"}).to_string(),
        ))
        .unwrap();
    let resp = ctx.router.clone().oneshot(req).await.unwrap();
    assert_eq!(resp.status(), StatusCode::UNAUTHORIZED);
    let bytes = resp.into_body().collect().await.unwrap().to_bytes();
    let v: Value = serde_json::from_str(&String::from_utf8_lossy(&bytes)).unwrap();
    assert_eq!(v["error"]["code"].as_str(), Some("unauthorized"));
    assert_eq!(v["error"]["message"].as_str(), Some("invalid auth"));
}

#[tokio::test]
async fn auth_short_token() {
    let ctx = setup().await;
    let req = Request::builder()
        .method("POST")
        .uri("/v1/responses")
        .header(CONTENT_TYPE, "application/json")
        .header(AUTHORIZATION, "Bearer sk-short")
        .body(Body::from(
            json!({"model":"gpt-5-mini","input":"hi"}).to_string(),
        ))
        .unwrap();
    let resp = ctx.router.clone().oneshot(req).await.unwrap();
    assert_eq!(resp.status(), StatusCode::UNAUTHORIZED);
    let bytes = resp.into_body().collect().await.unwrap().to_bytes();
    let v: Value = serde_json::from_str(&String::from_utf8_lossy(&bytes)).unwrap();
    assert_eq!(v["error"]["code"].as_str(), Some("unauthorized"));
    assert_eq!(v["error"]["message"].as_str(), Some("invalid token"));
}

#[tokio::test]
async fn auth_invalid_token_format() {
    let ctx = setup().await;
    let req = Request::builder()
        .method("POST")
        .uri("/v1/responses")
        .header(CONTENT_TYPE, "application/json")
        .header(AUTHORIZATION, "Bearer not-starting-with-sk-xxxx")
        .body(Body::from(
            json!({"model":"gpt-5-mini","input":"hi"}).to_string(),
        ))
        .unwrap();
    let resp = ctx.router.clone().oneshot(req).await.unwrap();
    assert_eq!(resp.status(), StatusCode::UNAUTHORIZED);
    let bytes = resp.into_body().collect().await.unwrap().to_bytes();
    let v: Value = serde_json::from_str(&String::from_utf8_lossy(&bytes)).unwrap();
    assert_eq!(v["error"]["code"].as_str(), Some("unauthorized"));
    assert_eq!(v["error"]["message"].as_str(), Some("invalid token"));
}

#[tokio::test]
async fn auth_nonexistent_valid_format_token() {
    let ctx = setup().await;
    let req = Request::builder()
        .method("POST")
        .uri("/v1/responses")
        .header(CONTENT_TYPE, "application/json")
        .header(AUTHORIZATION, "Bearer sk-doesnotexistindb")
        .body(Body::from(
            json!({"model":"gpt-5-mini","input":"hi"}).to_string(),
        ))
        .unwrap();
    let resp = ctx.router.clone().oneshot(req).await.unwrap();
    assert_eq!(resp.status(), StatusCode::UNAUTHORIZED);
    let bytes = resp.into_body().collect().await.unwrap().to_bytes();
    let v: Value = serde_json::from_str(&String::from_utf8_lossy(&bytes)).unwrap();
    assert_eq!(v["error"]["code"].as_str(), Some("unauthorized"));
    assert_eq!(v["error"]["message"].as_str(), Some("invalid token"));
}

#[tokio::test]
async fn body_not_json_returns_bad_request() {
    let ctx = setup().await;
    for path in ["/v1/responses", "/v1/chat/completions", "/v1/messages"] {
        let req = Request::builder()
            .method("POST")
            .uri(path)
            .header(CONTENT_TYPE, "application/json")
            .header(AUTHORIZATION, ctx.auth_header.clone())
            .body(Body::from("this-is-not-json"))
            .unwrap();
        let resp = ctx.router.clone().oneshot(req).await.unwrap();
        assert_eq!(resp.status(), StatusCode::BAD_REQUEST);
    }
}

#[tokio::test]
async fn body_json_array_returns_bad_request() {
    let ctx = setup().await;
    for path in ["/v1/responses", "/v1/chat/completions", "/v1/messages"] {
        let req = Request::builder()
            .method("POST")
            .uri(path)
            .header(CONTENT_TYPE, "application/json")
            .header(AUTHORIZATION, ctx.auth_header.clone())
            .body(Body::from("[1,2,3]"))
            .unwrap();
        let resp = ctx.router.clone().oneshot(req).await.unwrap();
        assert_eq!(resp.status(), StatusCode::BAD_REQUEST);
        let bytes = resp.into_body().collect().await.unwrap().to_bytes();
        let v: Value = serde_json::from_str(&String::from_utf8_lossy(&bytes)).unwrap();
        assert_eq!(v["error"]["code"].as_str(), Some("invalid_request"));
        assert_eq!(v["error"]["message"].as_str(), Some("body must be object"));
    }
}

#[tokio::test]
async fn body_missing_model_returns_bad_request() {
    let ctx = setup().await;
    let (status, body) = json_post(&ctx, "/v1/responses", json!({"input":"hi"})).await;
    assert_eq!(status, StatusCode::BAD_REQUEST);
    let v: Value = serde_json::from_str(&body).unwrap();
    assert_eq!(v["error"]["code"].as_str(), Some("invalid_request"));
    assert_eq!(v["error"]["message"].as_str(), Some("missing model"));
}

#[tokio::test]
async fn body_empty_model_returns_bad_request() {
    let ctx = setup().await;
    let (status, body) = json_post(&ctx, "/v1/embeddings", json!({"model":"","input":"hi"})).await;
    assert_eq!(status, StatusCode::BAD_REQUEST);
    let v: Value = serde_json::from_str(&body).unwrap();
    assert_eq!(v["error"]["code"].as_str(), Some("invalid_request"));
    assert_eq!(v["error"]["message"].as_str(), Some("missing model"));
}

#[tokio::test]
async fn body_model_wrong_type_returns_bad_request() {
    let ctx = setup().await;
    let (status, body) = json_post(&ctx, "/v1/responses", json!({"model":123,"input":"hi"})).await;
    assert_eq!(status, StatusCode::BAD_REQUEST);
    let v: Value = serde_json::from_str(&body).unwrap();
    assert_eq!(v["error"]["code"].as_str(), Some("invalid_request"));
    assert_eq!(v["error"]["message"].as_str(), Some("missing model"));
}

#[tokio::test]
async fn billing_injected_usage_field_ignored() {
    let ctx = setup().await;
    let user = ctx
        .state
        .user_store
        .get_user_by_username("tenant-1")
        .await
        .expect("get user")
        .expect("user exists");
    ctx.state
        .user_store
        .update_user(
            &user.id,
            None,
            None,
            None,
            None,
            Some("1000000000"),
            Some(false),
            None,
        )
        .await
        .expect("update user");

    let user_before = ctx
        .state
        .user_store
        .get_user_by_username("tenant-1")
        .await
        .expect("get user")
        .expect("user exists");
    let before: i64 = user_before.balance_nano_usd.parse().unwrap();

    let (status, _body) = json_post(
        &ctx,
        "/v1/chat/completions",
        json!({
            "model":"gpt-5-mini-chat",
            "messages":[{"role":"user","content":"billing usage injection"}],
            "stream": true,
            "emit_usage": true,
            "usage": {"input_tokens": 0, "output_tokens": 0}
        }),
    )
    .await;
    assert_eq!(status, StatusCode::OK);

    ctx.state.user_store.flush_all_batchers().await;
    let user_after = ctx
        .state
        .user_store
        .get_user_by_username("tenant-1")
        .await
        .expect("get user")
        .expect("user exists");
    let after: i64 = user_after.balance_nano_usd.parse().unwrap();

    assert_eq!(before - after, 20000);
}

#[tokio::test]
async fn billing_injected_pricing_field_ignored() {
    let ctx = setup().await;
    let user = ctx
        .state
        .user_store
        .get_user_by_username("tenant-1")
        .await
        .expect("get user")
        .expect("user exists");
    ctx.state
        .user_store
        .update_user(
            &user.id,
            None,
            None,
            None,
            None,
            Some("1000000000"),
            Some(false),
            None,
        )
        .await
        .expect("update user");

    let user_before = ctx
        .state
        .user_store
        .get_user_by_username("tenant-1")
        .await
        .expect("get user")
        .expect("user exists");
    let before: i64 = user_before.balance_nano_usd.parse().unwrap();

    let (status, _body) = json_post(
        &ctx,
        "/v1/chat/completions",
        json!({
            "model":"gpt-5-mini-chat",
            "messages":[{"role":"user","content":"billing pricing injection"}],
            "stream": true,
            "emit_usage": true,
            "pricing": {"input_cost": 0}
        }),
    )
    .await;
    assert_eq!(status, StatusCode::OK);

    ctx.state.user_store.flush_all_batchers().await;
    let user_after = ctx
        .state
        .user_store
        .get_user_by_username("tenant-1")
        .await
        .expect("get user")
        .expect("user exists");
    let after: i64 = user_after.balance_nano_usd.parse().unwrap();

    assert_eq!(before - after, 20000);
}

#[tokio::test]
async fn billing_model_field_does_not_affect_upstream_charge() {
    let ctx = setup().await;

    let providers = ctx
        .state
        .monoize_store
        .list_providers()
        .await
        .expect("list providers");
    let base_url = providers
        .iter()
        .find_map(|p| p.channels.first().map(|c| c.base_url.clone()))
        .expect("base_url");

    let mut models = HashMap::new();
    models.insert(
        "alias-route-model".to_string(),
        monoize::monoize_routing::MonoizeModelEntry {
            redirect: Some("gpt-5-mini".to_string()),
            multiplier: 1.0,
        },
    );
    ctx.state
        .monoize_store
        .create_provider(monoize::monoize_routing::CreateMonoizeProviderInput {
            name: "alias-route-provider".to_string(),
            provider_type: monoize::monoize_routing::MonoizeProviderType::Responses,
            models,
            api_type_overrides: Vec::new(),
            channels: vec![monoize::monoize_routing::CreateMonoizeChannelInput {
                id: Some("alias-route-ch".to_string()),
                name: "alias-route-ch".to_string(),
                base_url,
                api_key: Some("upstream-key".to_string()),
                weight: 1,
                enabled: true,
                passive_failure_threshold_override: None,
                passive_cooldown_seconds_override: None,
                passive_window_seconds_override: None,
                passive_min_samples_override: None,
                passive_failure_rate_threshold_override: None,
                passive_rate_limit_cooldown_seconds_override: None,
            }],
            max_retries: -1,
            transforms: Vec::new(),
            active_probe_enabled_override: None,
            active_probe_interval_seconds_override: None,
            active_probe_success_threshold_override: None,
            active_probe_model_override: None,
            request_timeout_ms_override: None,
            enabled: true,
            priority: Some(-50),
        })
        .await
        .expect("create alias provider");

    ctx.state
        .model_registry_store
        .upsert_model_metadata(
            "alias-route-model",
            monoize::model_registry_store::UpsertModelMetadataInput {
                models_dev_provider: Some("test".to_string()),
                mode: Some("chat".to_string()),
                input_cost_per_token_nano: Some("999999".to_string()),
                output_cost_per_token_nano: Some("999999".to_string()),
                cache_read_input_cost_per_token_nano: None,
                cache_creation_input_cost_per_token_nano: None,
                output_cost_per_reasoning_token_nano: None,
                max_input_tokens: None,
                max_output_tokens: None,
                max_tokens: None,
            },
        )
        .await
        .expect("seed alias model pricing");

    let user = ctx
        .state
        .user_store
        .get_user_by_username("tenant-1")
        .await
        .expect("get user")
        .expect("user exists");
    ctx.state
        .user_store
        .update_user(
            &user.id,
            None,
            None,
            None,
            None,
            Some("1000000000"),
            Some(false),
            None,
        )
        .await
        .expect("update user");

    let user_before = ctx
        .state
        .user_store
        .get_user_by_username("tenant-1")
        .await
        .expect("get user")
        .expect("user exists");
    let before: i64 = user_before.balance_nano_usd.parse().unwrap();

    let (status, _body) = json_post(
        &ctx,
        "/v1/responses",
        json!({
            "model":"alias-route-model",
            "input":"route-charge",
            "stream": true,
            "emit_usage": true
        }),
    )
    .await;
    assert_eq!(status, StatusCode::OK);

    ctx.state.user_store.flush_all_batchers().await;
    let user_after = ctx
        .state
        .user_store
        .get_user_by_username("tenant-1")
        .await
        .expect("get user")
        .expect("user exists");
    let after: i64 = user_after.balance_nano_usd.parse().unwrap();

    assert_eq!(before - after, 20000);
}

#[tokio::test]
async fn balance_zero_returns_payment_required() {
    let ctx = setup().await;
    let user = ctx
        .state
        .user_store
        .get_user_by_username("tenant-1")
        .await
        .expect("get user")
        .expect("user exists");
    ctx.state
        .user_store
        .update_user(
            &user.id,
            None,
            None,
            None,
            None,
            Some("0"),
            Some(false),
            None,
        )
        .await
        .expect("update user");

    let (status, body) = json_post(
        &ctx,
        "/v1/responses",
        json!({"model":"gpt-5-mini","input":"hi"}),
    )
    .await;
    assert_eq!(status, StatusCode::PAYMENT_REQUIRED);
    let v: Value = serde_json::from_str(&body).unwrap();
    assert_eq!(v["error"]["code"].as_str(), Some("insufficient_balance"));
}

#[tokio::test]
async fn balance_exact_covers_request() {
    let ctx = setup().await;
    let user = ctx
        .state
        .user_store
        .get_user_by_username("tenant-1")
        .await
        .expect("get user")
        .expect("user exists");
    ctx.state
        .user_store
        .update_user(
            &user.id,
            None,
            None,
            None,
            None,
            Some("20000"),
            Some(false),
            None,
        )
        .await
        .expect("update user");

    let (status, _body) = json_post(
        &ctx,
        "/v1/chat/completions",
        json!({
            "model":"gpt-5-mini-chat",
            "messages":[{"role":"user","content":"hi"}],
            "stream": true,
            "emit_usage": true
        }),
    )
    .await;
    assert_eq!(status, StatusCode::OK);

    ctx.state.user_store.flush_all_batchers().await;
    let user_after = ctx
        .state
        .user_store
        .get_user_by_username("tenant-1")
        .await
        .expect("get user")
        .expect("user exists");
    let after: i64 = user_after.balance_nano_usd.parse().unwrap();
    assert_eq!(after, 0);
}

#[tokio::test]
async fn balance_insufficient_after_charge() {
    let ctx = setup().await;
    let user = ctx
        .state
        .user_store
        .get_user_by_username("tenant-1")
        .await
        .expect("get user")
        .expect("user exists");
    ctx.state
        .user_store
        .update_user(
            &user.id,
            None,
            None,
            None,
            None,
            Some("10000"),
            Some(false),
            None,
        )
        .await
        .expect("update user");

    let (status, body) = json_post(
        &ctx,
        "/v1/chat/completions",
        json!({
            "model":"gpt-5-mini-chat",
            "messages":[{"role":"user","content":"hi"}],
            "stream": true,
            "emit_usage": true
        }),
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    assert!(body.contains("[DONE]"));

    ctx.state.user_store.flush_all_batchers().await;
    let user_after = ctx
        .state
        .user_store
        .get_user_by_username("tenant-1")
        .await
        .expect("get user")
        .expect("user exists");
    let after: i64 = user_after.balance_nano_usd.parse().unwrap();
    assert_eq!(after, 10000);
}

#[tokio::test]
async fn embeddings_missing_model() {
    let ctx = setup().await;
    let (status, body) = json_post(&ctx, "/v1/embeddings", json!({"input":"hello"})).await;
    assert_eq!(status, StatusCode::BAD_REQUEST);
    let v: Value = serde_json::from_str(&body).unwrap();
    assert_eq!(v["error"]["code"].as_str(), Some("invalid_request"));
    assert_eq!(v["error"]["message"].as_str(), Some("missing model"));
}

#[tokio::test]
async fn embeddings_missing_input() {
    let ctx = setup().await;
    let (status, body) = json_post(&ctx, "/v1/embeddings", json!({"model":"gpt-5-mini"})).await;
    assert_eq!(status, StatusCode::BAD_REQUEST);
    let v: Value = serde_json::from_str(&body).unwrap();
    assert_eq!(v["error"]["code"].as_str(), Some("invalid_request"));
    assert_eq!(v["error"]["message"].as_str(), Some("missing input"));
}

#[tokio::test]
async fn embeddings_invalid_input_type() {
    let ctx = setup().await;
    let (status, body) = json_post(
        &ctx,
        "/v1/embeddings",
        json!({"model":"gpt-5-mini","input":123}),
    )
    .await;
    assert_eq!(status, StatusCode::BAD_REQUEST);
    let v: Value = serde_json::from_str(&body).unwrap();
    assert_eq!(v["error"]["code"].as_str(), Some("invalid_request"));
    assert_eq!(
        v["error"]["message"].as_str(),
        Some("input must be string or array of strings")
    );
}

#[tokio::test]
async fn embeddings_invalid_encoding_format() {
    let ctx = setup().await;
    let (status, body) = json_post(
        &ctx,
        "/v1/embeddings",
        json!({"model":"gpt-5-mini","input":"hi","encoding_format":"xml"}),
    )
    .await;
    assert_eq!(status, StatusCode::BAD_REQUEST);
    let v: Value = serde_json::from_str(&body).unwrap();
    assert_eq!(v["error"]["code"].as_str(), Some("invalid_request"));
    assert_eq!(
        v["error"]["message"].as_str(),
        Some("encoding_format must be 'float' or 'base64'")
    );
}

#[tokio::test]
async fn embeddings_encoding_format_wrong_type() {
    let ctx = setup().await;
    let (status, body) = json_post(
        &ctx,
        "/v1/embeddings",
        json!({"model":"gpt-5-mini","input":"hi","encoding_format":42}),
    )
    .await;
    assert_eq!(status, StatusCode::BAD_REQUEST);
    let v: Value = serde_json::from_str(&body).unwrap();
    assert_eq!(v["error"]["code"].as_str(), Some("invalid_request"));
    assert_eq!(
        v["error"]["message"].as_str(),
        Some("encoding_format must be 'float' or 'base64'")
    );
}

#[tokio::test]
async fn quota_exhausted_returns_429() {
    let ctx = setup().await;
    let user = ctx
        .state
        .user_store
        .get_user_by_username("tenant-1")
        .await
        .expect("get user")
        .expect("user exists");
    let (_, token) = ctx
        .state
        .user_store
        .create_api_key_extended(
            &user.id,
            monoize::users::CreateApiKeyInput {
                name: "quota-zero-key".to_string(),
                expires_in_days: None,
                quota: Some(0),
                quota_unlimited: false,
                model_limits_enabled: false,
                model_limits: vec![],
                ip_whitelist: Vec::new(),
                group: "default".to_string(),
                max_multiplier: None,
                transforms: Vec::new(),
            },
            false,
        )
        .await
        .expect("create quota api key");

    let req = Request::builder()
        .method("POST")
        .uri("/v1/responses")
        .header(CONTENT_TYPE, "application/json")
        .header(AUTHORIZATION, format!("Bearer {token}"))
        .body(Body::from(
            json!({"model":"gpt-5-mini","input":"quota check"}).to_string(),
        ))
        .unwrap();
    let resp = ctx.router.clone().oneshot(req).await.unwrap();
    assert_eq!(resp.status(), StatusCode::TOO_MANY_REQUESTS);
    let bytes = resp.into_body().collect().await.unwrap().to_bytes();
    let v: Value = serde_json::from_str(&String::from_utf8_lossy(&bytes)).unwrap();
    assert_eq!(v["error"]["code"].as_str(), Some("quota_exceeded"));
}

#[tokio::test]
async fn unknown_model_returns_error() {
    let ctx = setup().await;
    let (status, body) = json_post(
        &ctx,
        "/v1/responses",
        json!({"model":"nonexistent-model-xyz","input":"hi"}),
    )
    .await;
    assert_eq!(status, StatusCode::BAD_GATEWAY);
    let v: Value = serde_json::from_str(&body).unwrap();
    assert_eq!(v["error"]["code"].as_str(), Some("upstream_error"));
}

#[tokio::test]
async fn extra_fields_do_not_corrupt_response() {
    let ctx = setup().await;
    let user = ctx
        .state
        .user_store
        .get_user_by_username("tenant-1")
        .await
        .expect("get user")
        .expect("user exists");
    ctx.state
        .user_store
        .update_user(
            &user.id,
            None,
            None,
            None,
            None,
            Some("1000000000"),
            Some(false),
            None,
        )
        .await
        .expect("update user");

    let user_before = ctx
        .state
        .user_store
        .get_user_by_username("tenant-1")
        .await
        .expect("get user")
        .expect("user exists");
    let before: i64 = user_before.balance_nano_usd.parse().unwrap();

    let req = Request::builder()
        .method("POST")
        .uri("/v1/chat/completions")
        .header(CONTENT_TYPE, "application/json")
        .header(AUTHORIZATION, ctx.auth_header.clone())
        .body(Body::from(
            json!({
                "model":"gpt-5-mini-chat",
                "messages":[{"role":"user","content":"edge-extra-fields"}],
                "stream": true,
                "emit_usage": true,
                "hack_model":"free-model",
                "override_billing": true,
                "admin": true,
                "nested": {"inject": [1,2,3], "bypass": "no"}
            })
            .to_string(),
        ))
        .unwrap();
    let resp = ctx.router.clone().oneshot(req).await.unwrap();
    assert_eq!(resp.status(), StatusCode::OK);
    let bytes = resp.into_body().collect().await.unwrap().to_bytes();
    let text = String::from_utf8_lossy(&bytes).to_string();
    assert!(text.contains("edge-extra-fields"));

    ctx.state.user_store.flush_all_batchers().await;
    let user_after = ctx
        .state
        .user_store
        .get_user_by_username("tenant-1")
        .await
        .expect("get user")
        .expect("user exists");
    let after: i64 = user_after.balance_nano_usd.parse().unwrap();
    assert_eq!(before - after, 20000);
}

#[tokio::test]
async fn ip_whitelist_blocks_non_whitelisted() {
    let ctx = setup().await;
    let user = ctx
        .state
        .user_store
        .get_user_by_username("tenant-1")
        .await
        .expect("get user")
        .expect("user exists");
    let (_, token) = ctx
        .state
        .user_store
        .create_api_key_extended(
            &user.id,
            monoize::users::CreateApiKeyInput {
                name: "ip-restricted-key".to_string(),
                expires_in_days: None,
                quota: None,
                quota_unlimited: true,
                model_limits_enabled: false,
                model_limits: vec![],
                ip_whitelist: vec!["192.168.1.1".to_string()],
                group: "default".to_string(),
                max_multiplier: None,
                transforms: Vec::new(),
            },
            false,
        )
        .await
        .expect("create ip restricted api key");

    let req = Request::builder()
        .method("POST")
        .uri("/v1/responses")
        .header(CONTENT_TYPE, "application/json")
        .header(AUTHORIZATION, format!("Bearer {token}"))
        .body(Body::from(
            json!({"model":"gpt-5-mini","input":"ip-check"}).to_string(),
        ))
        .unwrap();
    let resp = ctx.router.clone().oneshot(req).await.unwrap();
    assert_eq!(resp.status(), StatusCode::FORBIDDEN);
    let bytes = resp.into_body().collect().await.unwrap().to_bytes();
    let v: Value = serde_json::from_str(&String::from_utf8_lossy(&bytes)).unwrap();
    assert_eq!(v["error"]["code"].as_str(), Some("ip_not_allowed"));
}

#[tokio::test]
async fn chat_streaming_content_only_from_chat_upstream_has_terminal_chunk_and_usage() {
    let ctx = setup().await;
    let req = Request::builder()
        .method("POST")
        .uri("/v1/chat/completions")
        .header(CONTENT_TYPE, "application/json")
        .header(AUTHORIZATION, ctx.auth_header.clone())
        .body(Body::from(
            json!({
                "model":"gpt-5-mini-chat",
                "messages":[{"role":"user","content":"hello"}],
                "stream": true
            })
            .to_string(),
        ))
        .unwrap();
    let resp = ctx.router.clone().oneshot(req).await.unwrap();
    assert_eq!(resp.status(), StatusCode::OK);
    let bytes = resp.into_body().collect().await.unwrap().to_bytes();
    let text = String::from_utf8_lossy(&bytes).to_string();

    let mut finish_reasons: Vec<String> = Vec::new();
    let mut has_usage = false;
    let mut has_content = false;
    for line in text.lines() {
        let Some(payload) = line.strip_prefix("data: ") else {
            continue;
        };
        if payload == "[DONE]" {
            continue;
        }
        let Ok(v) = serde_json::from_str::<Value>(payload) else {
            continue;
        };
        if v.get("usage").is_some() {
            has_usage = true;
        }
        let choice = v
            .get("choices")
            .and_then(|v| v.as_array())
            .and_then(|arr| arr.first());
        if let Some(c) = choice {
            if let Some(reason) = c.get("finish_reason").and_then(|v| v.as_str()) {
                if !reason.is_empty() {
                    finish_reasons.push(reason.to_string());
                }
            }
            if c.get("delta")
                .and_then(|d| d.get("content"))
                .and_then(|v| v.as_str())
                .is_some()
            {
                has_content = true;
            }
        }
    }

    assert!(has_content, "should have content deltas: {text}");
    assert_eq!(
        finish_reasons,
        vec!["stop".to_string()],
        "content-only chat stream must have exactly one terminal finish_reason=stop: {text}"
    );
    assert!(
        has_usage,
        "PC9: usage must be present via auto-injected stream_options.include_usage: {text}"
    );
    assert!(text.contains("[DONE]"), "must end with [DONE]: {text}");
}

#[tokio::test]
async fn chat_streaming_header_only_tool_call_still_finishes_as_tool_calls() {
    let ctx = setup().await;
    let req = Request::builder()
        .method("POST")
        .uri("/v1/chat/completions")
        .header(CONTENT_TYPE, "application/json")
        .header(AUTHORIZATION, ctx.auth_header.clone())
        .body(Body::from(
            json!({
                "model":"gpt-5-mini-chat",
                "messages":[{"role":"user","content":"stream tool"}],
                "tools":[{ "type":"function","function":{ "name":"tool_empty","parameters":{ "type":"object","additionalProperties":true }}}],
                "stream": true,
                "stream_mode": "header_only_tool"
            })
            .to_string(),
        ))
        .unwrap();
    let resp = ctx.router.clone().oneshot(req).await.unwrap();
    assert_eq!(resp.status(), StatusCode::OK);
    let bytes = resp.into_body().collect().await.unwrap().to_bytes();
    let text = String::from_utf8_lossy(&bytes).to_string();

    let mut finish_reasons = Vec::new();
    let mut saw_tool_call_header = false;
    for line in text.lines() {
        let Some(payload) = line.strip_prefix("data: ") else {
            continue;
        };
        if payload == "[DONE]" {
            continue;
        }
        let Ok(v) = serde_json::from_str::<Value>(payload) else {
            continue;
        };
        if v.get("choices")
            .and_then(|v| v.as_array())
            .and_then(|arr| arr.first())
            .and_then(|choice| choice.get("delta"))
            .and_then(|delta| delta.get("tool_calls"))
            .and_then(|v| v.as_array())
            .is_some()
        {
            saw_tool_call_header = true;
        }
        if let Some(reason) = v
            .get("choices")
            .and_then(|v| v.as_array())
            .and_then(|arr| arr.first())
            .and_then(|choice| choice.get("finish_reason"))
            .and_then(|v| v.as_str())
        {
            if !reason.is_empty() {
                finish_reasons.push(reason.to_string());
            }
        }
    }

    assert!(
        saw_tool_call_header,
        "expected downstream tool call header chunk: {text}"
    );
    assert_eq!(
        finish_reasons,
        vec!["tool_calls".to_string()],
        "header-only tool call must still normalize terminal finish reason to tool_calls: {text}"
    );
    assert!(text.contains("[DONE]"));
}
