use super::*;
use base64::Engine as _;

fn last_captured_body(ctx: &TestContext, endpoint: &str) -> Value {
    ctx.captured_bodies
        .lock()
        .expect("captured bodies lock")
        .iter()
        .rev()
        .find(|(name, _)| name == endpoint)
        .map(|(_, body)| body.clone())
        .unwrap_or_else(|| panic!("missing captured upstream body for {endpoint}"))
}

#[tokio::test]
async fn direct_request_matrix_captures_all_openai_anthropic_routes_nonstream_and_stream() {
    let ctx = setup().await;
    let downstreams = ["responses", "chat", "messages"];
    let upstreams = [
        ("responses", "gpt-5-mini"),
        ("chat", "gpt-5-mini-chat"),
        ("messages", "gpt-5-mini-msg"),
    ];

    for stream in [false, true] {
        for downstream in downstreams {
            for (upstream, model) in upstreams {
                let marker = format!("grid-{downstream}-{upstream}-{stream}");
                let (path, body) = match downstream {
                    "responses" => (
                        "/v1/responses",
                        json!({
                            "model": model,
                            "input": marker.clone(),
                            "max_output_tokens": 64,
                            "stream": stream
                        }),
                    ),
                    "chat" => (
                        "/v1/chat/completions",
                        json!({
                            "model": model,
                            "messages": [{ "role": "user", "content": marker.clone() }],
                            "max_completion_tokens": 64,
                            "stream": stream
                        }),
                    ),
                    "messages" => (
                        "/v1/messages",
                        json!({
                            "model": model,
                            "max_tokens": 64,
                            "messages": [{ "role": "user", "content": marker.clone() }],
                            "stream": stream
                        }),
                    ),
                    _ => unreachable!(),
                };

                let (status, response) = json_post(&ctx, path, body).await;
                assert_eq!(
                    status,
                    StatusCode::OK,
                    "{downstream}->{upstream} stream={stream}: {response}"
                );
                if stream {
                    match downstream {
                        "responses" => {
                            assert!(response.contains("event: response.completed"), "{response}");
                            assert!(response.contains("data: [DONE]"), "{response}");
                        }
                        "chat" => {
                            assert!(response.contains("data: [DONE]"), "{response}");
                            assert!(!response.contains("event:"), "{response}");
                        }
                        "messages" => {
                            assert!(response.contains("event: message_start"), "{response}");
                            assert!(response.contains("event: message_stop"), "{response}");
                            assert!(!response.contains("data: [DONE]"), "{response}");
                        }
                        _ => unreachable!(),
                    }
                } else {
                    let decoded: Value = serde_json::from_str(&response).unwrap();
                    match downstream {
                        "responses" => {
                            assert!(decoded.get("output").and_then(Value::as_array).is_some());
                        }
                        "chat" => {
                            assert!(decoded.get("choices").and_then(Value::as_array).is_some());
                        }
                        "messages" => {
                            assert_eq!(decoded["type"], json!("message"));
                            assert!(decoded.get("content").and_then(Value::as_array).is_some());
                        }
                        _ => unreachable!(),
                    }
                }
                let captured = last_captured_body(&ctx, upstream);
                assert_eq!(captured["stream"], json!(stream));
                assert!(
                    serde_json::to_string(&captured).unwrap().contains(&marker),
                    "{downstream}->{upstream} lost semantic input: {captured}"
                );
                match upstream {
                    "responses" => {
                        assert!(captured.get("input").is_some(), "{captured}");
                        assert!(captured.get("messages").is_none(), "{captured}");
                    }
                    "chat" => {
                        assert!(captured.get("messages").is_some(), "{captured}");
                        assert!(captured.get("input").is_none(), "{captured}");
                    }
                    "messages" => {
                        assert!(captured.get("messages").is_some(), "{captured}");
                        assert!(captured.get("input").is_none(), "{captured}");
                        assert_eq!(captured["max_tokens"], json!(64));
                    }
                    _ => unreachable!(),
                }
            }
        }
    }
}

mod responses_reasoning {
    use super::*;
    include!("adapters_nonstream/responses_reasoning.rs");
}

mod images_and_chat {
    use super::*;
    include!("adapters_nonstream/images_and_chat.rs");
}

mod messages_basic {
    use super::*;
    include!("adapters_nonstream/messages_basic.rs");
}

mod tools_envelope {
    use super::*;
    include!("adapters_nonstream/tools_envelope.rs");
}

mod reasoning_tools {
    use super::*;
    include!("adapters_nonstream/reasoning_tools.rs");
}

mod messages_native {
    use super::*;
    include!("adapters_nonstream/messages_native.rs");
}

mod native_responses {
    use super::*;
    include!("adapters_nonstream/native_responses.rs");
}

mod messages_reasoning {
    use super::*;
    include!("adapters_nonstream/messages_reasoning.rs");
}

mod request_controls {
    use super::*;
    include!("adapters_nonstream/request_controls.rs");
}

mod controls_tools_matrix {
    use super::*;
    include!("adapters_nonstream/controls_tools_matrix.rs");
}

#[tokio::test]
async fn chat_nonstream_usage_preserves_nested_unknown_details() {
    let ctx = setup().await;
    let (status, body) = json_post(
        &ctx,
        "/v1/chat/completions",
        json!({
            "model": "gpt-5-mini-chat",
            "messages": [{ "role": "user", "content": "nested chat usage" }],
            "stream_mode": "nested_usage_details"
        }),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "unexpected response: {body}");

    let response: Value = serde_json::from_str(&body).expect("chat response JSON");
    assert_eq!(
        response["usage"]["prompt_tokens_details"],
        json!({
            "cached_tokens": 0,
            "cache_write_tokens": 0,
            "cache_creation_tokens": 0,
            "tool_prompt_tokens": 0,
            "vendor_prompt_detail": { "kind": "warm" }
        })
    );
    assert_eq!(
        response["usage"]["completion_tokens_details"],
        json!({
            "reasoning_tokens": 0,
            "accepted_prediction_tokens": 0,
            "rejected_prediction_tokens": 0,
            "vendor_completion_detail": [1, 2]
        })
    );
    assert!(response["usage"].get("vendor_prompt_detail").is_none());
    assert!(response["usage"].get("vendor_completion_detail").is_none());
}

#[tokio::test]
async fn responses_nonstream_usage_preserves_nested_unknown_details() {
    let ctx = setup().await;
    let (status, body) = json_post(
        &ctx,
        "/v1/responses",
        json!({
            "model": "gpt-5-mini",
            "input": "nested Responses usage",
            "stream_mode": "nested_usage_details"
        }),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "unexpected response: {body}");

    let response: Value = serde_json::from_str(&body).expect("Responses response JSON");
    assert_eq!(
        response["usage"]["input_tokens_details"],
        json!({
            "cached_tokens": 0,
            "cache_write_tokens": 0,
            "cache_creation_tokens": 0,
            "tool_prompt_tokens": 0,
            "vendor_input_detail": { "kind": "warm" }
        })
    );
    assert_eq!(
        response["usage"]["output_tokens_details"],
        json!({
            "reasoning_tokens": 0,
            "accepted_prediction_tokens": 0,
            "rejected_prediction_tokens": 0,
            "vendor_output_detail": [3, 4]
        })
    );
    assert!(response["usage"].get("vendor_input_detail").is_none());
    assert!(response["usage"].get("vendor_output_detail").is_none());
}
