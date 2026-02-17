use axum::Json;
use axum::Router;
use axum::body::Body;
use axum::http::header::{AUTHORIZATION, CONTENT_TYPE};
use axum::http::{Request, StatusCode};
use axum::response::IntoResponse;
use axum::response::Sse;
use axum::response::sse::Event;
use axum::routing::post;
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
                let mut events: Vec<Result<Event, Infallible>> = Vec::new();
                events.push(Ok(Event::default()
                    .event("response.reasoning_text.delta")
                    .data(json!({ "delta": "mock_reasoning" }).to_string())));
                events.push(Ok(Event::default()
                    .event("response.reasoning_signature.delta")
                    .data(json!({ "delta": "mock_sig" }).to_string())));

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
                                "arguments": args
                            })
                            .to_string(),
                        )));
                    events.push(Ok(Event::default()
                        .event("response.output_item.done")
                        .data(json!({
                            "type": "response.output_item.done",
                            "output_index": idx + 1,
                            "item": { "type": "function_call", "call_id": call_id, "name": name, "arguments": args }
                        }).to_string())));
                }
                events.push(Ok(Event::default().data("[DONE]")));
                return Sse::new(futures_util::stream::iter(events)).into_response();
            }

            if body.get("stream_mode").and_then(|v| v.as_str()) == Some("item_done_only") {
                let stream =
                    futures_util::stream::iter(vec![
                    Ok::<_, Infallible>(Event::default()
                        .event("response.output_item.added")
                        .data(json!({
                            "type": "response.output_item.added",
                            "output_index": 0,
                            "item": { "type": "message", "role": "assistant", "content": [] }
                        }).to_string())),
                    Ok::<_, Infallible>(Event::default()
                        .event("response.output_item.done")
                        .data(json!({
                            "type": "response.output_item.done",
                            "output_index": 0,
                            "item": {
                                "type": "message",
                                "role": "assistant",
                                "content": [{ "type": "output_text", "text": text }]
                            }
                        }).to_string())),
                    Ok::<_, Infallible>(Event::default().data("[DONE]")),
                ]);
                return Sse::new(stream).into_response();
            }

            let mut events = Vec::new();
            if reasoning_enabled {
                events.push(Ok::<_, Infallible>(
                    Event::default()
                        .event("response.reasoning_text.delta")
                        .data(json!({ "delta": "mock_reasoning" }).to_string()),
                ));
            }
            events.push(Ok::<_, Infallible>(
                Event::default()
                    .event("response.output_text.delta")
                    .data(json!({ "delta": text }).to_string()),
            ));
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
                json!({ "type": "reasoning", "text": "mock_reasoning", "signature": "mock_sig" }),
            ];
            output.extend(calls);
            return Json(json!({
                "id": "resp_mock",
                "object": "response",
                "created": 0,
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
                "created": 0,
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
                json!({ "type": "reasoning", "text": "mock_reasoning", "signature": "mock_sig" }),
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
            "created": 0,
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
                let mut chunks: Vec<Result<Event, Infallible>> = Vec::new();
                chunks.push(Ok(Event::default().data(json!({
                    "id": "chatcmpl_mock",
                    "object": "chat.completion.chunk",
                    "created": 0,
                    "model": model,
                    "choices": [{ "index": 0, "delta": { "role": "assistant" }, "finish_reason": Value::Null }]
                }).to_string())));
                chunks.push(Ok(Event::default().data(json!({
                    "id": "chatcmpl_mock",
                    "object": "chat.completion.chunk",
                    "created": 0,
                    "model": model,
                    "choices": [{ "index": 0, "delta": { "reasoning_details": [{ "type": "reasoning.text", "text": "mock_reasoning", "signature": "mock_sig", "format": "unknown" }] }, "finish_reason": Value::Null }]
                }).to_string())));
                let calls = if parallel {
                    vec![
                        ("call_1", "tool_a", "{\"a\":1}"),
                        ("call_2", "tool_b", "{\"b\":2}"),
                    ]
                } else {
                    vec![("call_1", "tool_a", "{\"a\":1}")]
                };
                for (call_id, name, args) in calls {
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
                                        "id": call_id,
                                        "type": "function",
                                        "function": { "name": name, "arguments": args }
                                    }]
                                },
                                "finish_reason": Value::Null
                            }]
                        })
                        .to_string(),
                    )));
                }
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
                    "choices": [{ "index": 0, "delta": { "reasoning_details": [{ "type": "reasoning.text", "text": "mock_reasoning", "signature": "mock_sig", "format": "unknown" }] }, "finish_reason": Value::Null }]
                }).to_string())));
            }
            chunks.push(Ok::<_, Infallible>(
                Event::default().data(chunk.to_string()),
            ));
            if emit_usage {
                chunks.push(Ok::<_, Infallible>(
                    Event::default().data(
                        json!({
                            "id": "chatcmpl_mock",
                            "object": "chat.completion.chunk",
                            "created": 0,
                            "model": model,
                            "choices": [{ "index": 0, "delta": {}, "finish_reason": finish_reason }],
                            "usage": {
                                "prompt_tokens": 12,
                                "completion_tokens": 8,
                                "total_tokens": 20,
                                "prompt_tokens_details": { "cached_tokens": 0 },
                                "completion_tokens_details": { "reasoning_tokens": 0 }
                            }
                        })
                        .to_string(),
                    ),
                ));
            }
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
                        "reasoning_details": [{ "type": "reasoning.text", "text": "mock_reasoning", "signature": "mock_sig", "format": "unknown" }]
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
                "reasoning_details": [{ "type": "reasoning.text", "text": "mock_reasoning", "signature": "mock_sig", "format": "unknown" }]
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
            channels: vec![monoize::monoize_routing::CreateMonoizeChannelInput {
                id: None,
                name: format!("{name}-channel"),
                base_url: base_url.to_string(),
                api_key: Some(api_key.to_string()),
                weight: 1,
                enabled: true,
            }],
            max_retries: -1,
            transforms: Vec::new(),
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

async fn setup_with_unknown_fields(unknown_fields: &str) -> TestContext {
    let (upstream_addr, captured_headers) = start_upstream().await;
    let base_url = format!("http://{upstream_addr}");

    let temp_dir = tempfile::tempdir().unwrap();
    let db_path = temp_dir.path().join("monoize.db");
    let unknown_fields_policy = match unknown_fields {
        "reject" => monoize::config::UnknownFieldPolicy::Reject,
        "ignore" => monoize::config::UnknownFieldPolicy::Ignore,
        _ => monoize::config::UnknownFieldPolicy::Preserve,
    };
    let state = monoize::app::load_state_with_runtime(monoize::app::RuntimeConfig {
        listen: "127.0.0.1:0".to_string(),
        metrics_path: "/metrics".to_string(),
        unknown_fields: unknown_fields_policy,
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
    setup_with_unknown_fields("preserve").await
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
async fn unknown_fields_rejects_when_configured() {
    let ctx = setup_with_unknown_fields("reject").await;
    let (status, _body) = json_post(
        &ctx,
        "/v1/responses",
        json!({"model":"gpt-5-mini","input":"hi","unknown":"x"}),
    )
    .await;
    assert_eq!(status, StatusCode::BAD_REQUEST);
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
    assert!(text.contains("\"sequence_number\":"));
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

    let user = ctx
        .state
        .user_store
        .get_user_by_username("tenant-1")
        .await
        .expect("query user")
        .expect("user exists");

    let mut matched = None;
    for _ in 0..20 {
        let (logs, _) = ctx
            .state
            .user_store
            .list_request_logs_by_user(&user.id, 100, 0, None, None, None, None)
            .await
            .expect("list request logs");
        matched = logs.into_iter().find(|log| {
            log.model == "gpt-5-mini-chat"
                && log.is_stream
                && log.prompt_tokens == Some(12)
                && log.completion_tokens == Some(8)
                && log.charge_nano_usd.as_deref() == Some("20000")
        });
        if matched.is_some() {
            break;
        }
        tokio::time::sleep(Duration::from_millis(50)).await;
    }

    let log = matched.expect("request log should be inserted");
    assert!(log.is_stream);
    assert!(log.ttfb_ms.is_some());
    assert_eq!(log.prompt_tokens, Some(12));
    assert_eq!(log.completion_tokens, Some(8));
    assert_eq!(log.charge_nano_usd.as_deref(), Some("20000"));
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

    let user = ctx
        .state
        .user_store
        .get_user_by_username("tenant-1")
        .await
        .expect("query user")
        .expect("user exists");

    let mut matched = None;
    for _ in 0..20 {
        let (logs, _) = ctx
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
            )
            .await
            .expect("list request logs");
        matched = logs
            .into_iter()
            .find(|log| log.status == "error" && log.error_http_status == Some(422));
        if matched.is_some() {
            break;
        }
        tokio::time::sleep(Duration::from_millis(50)).await;
    }

    let log = matched.expect("error request log should be inserted");
    assert_eq!(log.charge_nano_usd, None);
    assert_eq!(log.error_code.as_deref(), Some("upstream_error"));
    assert!(
        log.error_message
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

    let user = ctx
        .state
        .user_store
        .get_user_by_username("tenant-1")
        .await
        .expect("query user")
        .expect("user exists");

    let mut matched = None;
    for _ in 0..20 {
        let (logs, _) = ctx
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
            )
            .await
            .expect("list request logs");
        matched = logs.into_iter().find(|log| {
            log.model == "gpt-5-mini-chat"
                && log.is_stream
                && log.prompt_tokens == Some(12)
                && log.completion_tokens == Some(8)
                && log.charge_nano_usd.as_deref() == Some("20000")
                && log.status == "success"
        });
        if matched.is_some() {
            break;
        }
        tokio::time::sleep(Duration::from_millis(50)).await;
    }

    let log = matched.expect("stream length request should be billed");
    assert_eq!(log.status, "success");
    assert_eq!(log.charge_nano_usd.as_deref(), Some("20000"));
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
    assert!(text.contains("event: response.reasoning_text.delta"));
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
        channels: vec![monoize::monoize_routing::CreateMonoizeChannelInput {
            id: Some("mono-transform-strip-ch1".to_string()),
            name: "mono-transform-strip-ch1".to_string(),
            base_url,
            api_key: Some("upstream-key".to_string()),
            weight: 1,
            enabled: true,
        }],
        max_retries: -1,
        transforms: vec![monoize::transforms::TransformRuleConfig {
            transform: "strip_reasoning".to_string(),
            enabled: true,
            models: None,
            phase: monoize::transforms::Phase::Response,
            config: json!({}),
        }],
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
            "stream": true
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
    assert!(has_text_delta, "expected text_delta alongside thinking");
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
