use super::*;

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
async fn responses_reject_item_reference_continuations_while_remaining_stateless() {
    let ctx = setup().await;
    let (status, body) = json_post(
        &ctx,
        "/v1/responses",
        json!({
            "model": "gpt-5-mini",
            "input": [
                { "type": "message", "role": "user", "content": [{ "type": "input_text", "text": "ping" }] },
                { "type": "item_reference", "id": "msg_prev" },
                { "type": "function_call_output", "call_id": "call_1", "output": "{\"ok\":true}" }
            ]
        }),
    )
    .await;
    assert_eq!(status, StatusCode::BAD_REQUEST, "{body}");
    let v: Value = serde_json::from_str(&body).expect("error json");
    assert_eq!(v["error"]["code"].as_str(), Some("invalid_request"));
    let message = v["error"]["message"].as_str().unwrap_or("");
    assert!(
        message.contains("item_reference"),
        "error must mention item_reference: {body}"
    );
    assert!(
        message.contains("stateless"),
        "error must explain the stateless boundary: {body}"
    );
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
    assert_eq!(
        v["choices"][0]["message"]["content"],
        json!("startcontinue")
    );
}

#[tokio::test]
async fn chat_nonstream_openrouter_request_extensions_passthrough() {
    let ctx = setup().await;
    let (status, body) = json_post(
        &ctx,
        "/v1/chat/completions",
        json!({
            "model": "gpt-5-mini-chat",
            "messages": [{ "role": "user", "content": "extensions" }],
            "models": ["openai/gpt-5-mini", "anthropic/claude-3.7-sonnet"],
            "route": "fallback",
            "provider": { "order": ["openai", "anthropic"], "allow_fallbacks": true },
            "plugins": [{ "id": "web", "enabled": true }],
            "user": "user-123",
            "debug": { "echo_upstream_body": true }
        }),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{body}");

    let upstream = last_captured_body(&ctx, "chat");
    assert_eq!(upstream["models"], json!(["openai/gpt-5-mini", "anthropic/claude-3.7-sonnet"]));
    assert_eq!(upstream["route"], json!("fallback"));
    assert_eq!(upstream["provider"], json!({ "order": ["openai", "anthropic"], "allow_fallbacks": true }));
    assert_eq!(upstream["plugins"], json!([{ "id": "web", "enabled": true }]));
    assert_eq!(upstream["user"], json!("user-123"));
    assert_eq!(upstream["debug"], json!({ "echo_upstream_body": true }));
}

#[tokio::test]
async fn chat_nonstream_reasoning_details_replay_preserves_order() {
    let ctx = setup().await;
    let (status, body) = json_post(
        &ctx,
        "/v1/chat/completions",
        json!({
            "model": "gpt-5-mini-chat",
            "messages": [
                { "role": "user", "content": "start" },
                {
                    "role": "assistant",
                    "content": "",
                    "reasoning_details": [
                        { "type": "reasoning.summary", "summary": "first", "format": "openrouter" },
                        { "type": "reasoning.text", "text": "second", "format": "openrouter" },
                        { "type": "reasoning.encrypted", "data": "third", "format": "openrouter" }
                    ]
                },
                { "role": "user", "content": "continue" }
            ]
        }),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{body}");

    let upstream = last_captured_body(&ctx, "chat");
    let assistant = upstream["messages"]
        .as_array()
        .expect("chat messages array")
        .iter()
        .find(|message| message["role"].as_str() == Some("assistant"))
        .expect("assistant replay message");
    assert_eq!(
        assistant["reasoning_details"],
        json!([
            { "type": "reasoning.summary", "summary": "first", "format": "openrouter" },
            { "type": "reasoning.text", "text": "second", "format": "openrouter" },
            { "type": "reasoning.encrypted", "data": "third", "format": "openrouter" }
        ])
    );
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
async fn cross_family_top_level_extra_survives_while_nested_extra_is_stripped() {
    let ctx = setup().await;
    let (status, body) = json_post(
        &ctx,
        "/v1/chat/completions",
        json!({
            "model": "gpt-5-mini",
            "messages": [{
                "role": "user",
                "content": [
                    {
                        "type": "text",
                        "text": "hello",
                        "nested_local": "strip-me",
                        "phase": "commentary"
                    }
                ],
                "message_local": "strip-me-too"
            }],
            "extra_echo": "TOP"
        }),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{body}");

    let upstream = last_captured_body(&ctx, "responses");
    assert_eq!(upstream["extra_echo"], json!("TOP"));
    let input_message = &upstream["input"][0];
    assert_eq!(input_message["type"], json!("message"));
    assert!(input_message.get("message_local").is_none(), "cross-family nested envelope extra must strip: {input_message}");
    assert!(input_message["content"][0].get("nested_local").is_none(), "cross-family nested node extra must strip: {input_message}");
}

#[tokio::test]
async fn responses_same_family_next_downstream_envelope_extra_applies_once() {
    let ctx = setup().await;
    let (status, body) = json_post(
        &ctx,
        "/v1/responses",
        json!({
            "model": "gpt-5-mini",
            "input": [
                {
                    "type": "message",
                    "role": "user",
                    "content": [{ "type": "input_text", "text": "first" }],
                    "first_only": "A"
                },
                {
                    "type": "message",
                    "role": "assistant",
                    "content": [{ "type": "output_text", "text": "second" }]
                }
            ]
        }),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{body}");

    let upstream = last_captured_body(&ctx, "responses");
    let input = upstream["input"].as_array().expect("responses input array");
    assert_eq!(input[0]["first_only"], json!("A"));
    assert!(input[1].get("first_only").is_none(), "next_downstream_envelope_extra must apply once: {input:?}");
}

#[tokio::test]
async fn chat_cross_family_strips_next_downstream_envelope_extra() {
    let ctx = setup().await;
    let (status, body) = json_post(
        &ctx,
        "/v1/chat/completions",
        json!({
            "model": "gpt-5-mini",
            "messages": [
                {
                    "role": "assistant",
                    "content": "hidden-control",
                    "tool_calls": [{
                        "id": "call_x",
                        "type": "function",
                        "function": { "name": "tool_a", "arguments": "{}" }
                    }],
                    "chat_only_extra": "strip-before-responses"
                },
                { "role": "tool", "tool_call_id": "call_x", "content": "R1" }
            ]
        }),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{body}");

    let upstream = last_captured_body(&ctx, "responses");
    let input = upstream["input"].as_array().expect("responses input array");
    assert_eq!(
        input.iter().filter(|item| item["type"].as_str() == Some("function_call_output")).count(),
        1,
        "tool result must stay distinct: {input:?}"
    );
    assert_eq!(
        input.iter().filter(|item| item.get("chat_only_extra").is_some()).count(),
        0,
        "cross-family next_downstream_envelope_extra must strip: {input:?}"
    );
    assert!(
        !input.iter().any(|item| {
            item["type"].as_str() == Some("message")
                && item["content"].as_array().map(|parts| parts.is_empty()).unwrap_or(false)
        }),
        "control node must not synthesize an empty envelope: {input:?}"
    );
}

#[tokio::test]
async fn messages_cross_family_strips_next_downstream_envelope_extra() {
    let ctx = setup().await;
    let (status, body) = json_post(
        &ctx,
        "/v1/messages",
        json!({
            "model": "gpt-5-mini",
            "max_tokens": 64,
            "messages": [{
                "role": "user",
                "content": [{
                    "type": "tool_result",
                    "tool_use_id": "call_x",
                    "content": "R1",
                    "tool_result_extra": "strip-before-responses"
                }],
                "message_extra": "strip-too"
            }]
        }),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{body}");

    let upstream = last_captured_body(&ctx, "responses");
    let input = upstream["input"].as_array().expect("responses input array");
    assert_eq!(input.len(), 1);
    assert_eq!(input[0]["type"], json!("function_call_output"));
    assert!(input[0].get("message_extra").is_none());
    assert!(input[0].get("tool_result_extra").is_none());
}

#[tokio::test]
async fn cross_family_phase_is_not_synthesized_for_chat_or_messages() {
    let ctx = setup().await;

    let (chat_status, chat_body) = json_post(
        &ctx,
        "/v1/chat/completions",
        json!({
            "model": "gpt-5-mini",
            "messages": [{ "role": "user", "content": [{ "type": "text", "text": "phase-check", "phase": "analysis" }] }]
        }),
    )
    .await;
    assert_eq!(chat_status, StatusCode::OK, "{chat_body}");
    let chat_upstream = last_captured_body(&ctx, "responses");
    assert!(chat_upstream["input"][0]["content"][0].get("phase").is_none(), "chat cross-family phase must strip: {chat_upstream}");

    let (messages_status, messages_body) = json_post(
        &ctx,
        "/v1/messages",
        json!({
            "model": "gpt-5-mini",
            "max_tokens": 64,
            "messages": [{ "role": "user", "content": [{ "type": "text", "text": "phase-check", "phase": "analysis" }] }]
        }),
    )
    .await;
    assert_eq!(messages_status, StatusCode::OK, "{messages_body}");
    let messages_upstream = last_captured_body(&ctx, "responses");
    assert!(messages_upstream["input"][0]["content"][0].get("phase").is_none(), "messages cross-family phase must strip: {messages_upstream}");
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
async fn messages_upstream_defaults_max_tokens_to_anthropic_max_when_omitted() {
    let ctx = setup().await;
    let (status, _body) = json_post(
        &ctx,
        "/v1/messages",
        json!({
            "model": "gpt-5-mini-msg",
            "messages": [{ "role": "user", "content": [{ "type": "text", "text": "yo" }] }]
        }),
    )
    .await;
    assert_eq!(status, StatusCode::OK);

    let upstream = last_captured_body(&ctx, "messages");
    assert_eq!(upstream["max_tokens"], json!(64_000));
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
async fn responses_nonstream_markdown_image_transforms_extract_and_append_markdown() {
    let ctx = setup().await;
    let (upstream_addr, _, _) = start_upstream().await;
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
            groups: Vec::new(),
            channels: vec![monoize::monoize_routing::CreateMonoizeChannelInput {
                id: Some("mono-transform-markdown-images-ch1".to_string()),
                name: "mono-transform-markdown-images-ch1".to_string(),
                base_url,
                api_key: Some("upstream-key".to_string()),
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
            extra_fields_whitelist: None,
            strip_cross_protocol_nested_extra: None,
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

/// Regression test for the `messages.N.content.0.thinking.thinking: Field required` bug.
///
/// When a downstream `/v1/messages` request echoes back an assistant history block whose
/// `thinking` field is empty but that carries a non-empty `signature`, monoize must NOT
/// re-emit an invalid `{type:"thinking", encrypted_thinking:<sig>}` or an invalid
/// `{type:"thinking", thinking:"", signature:<sig>}` to the upstream. Per DM5.1 case 3,
/// the block must be dropped entirely since the marker for `redacted_thinking` is absent.
#[tokio::test]
async fn messages_request_drops_empty_thinking_without_redacted_marker() {
    let ctx = setup().await;
    let (status, _body) = json_post(
        &ctx,
        "/v1/messages",
        json!({
            "model": "gpt-5-mini-msg",
            "max_tokens": 64,
            "messages": [
                { "role": "user", "content": [{ "type": "text", "text": "hi" }] },
                {
                    "role": "assistant",
                    "content": [
                        { "type": "thinking", "thinking": "", "signature": "sig_orphan" },
                        { "type": "text", "text": "previous turn" }
                    ]
                },
                { "role": "user", "content": [{ "type": "text", "text": "continue" }] }
            ]
        }),
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    let upstream = last_captured_body(&ctx, "messages");
    let assistant = upstream["messages"]
        .as_array()
        .unwrap()
        .iter()
        .find(|m| m.get("role").and_then(|v| v.as_str()) == Some("assistant"))
        .expect("assistant turn forwarded");
    let blocks = assistant["content"].as_array().cloned().unwrap_or_default();
    for block in &blocks {
        assert!(
            block.get("encrypted_thinking").is_none(),
            "encrypted_thinking field is not part of the Anthropic wire contract: {block}"
        );
        if block.get("type").and_then(|v| v.as_str()) == Some("thinking") {
            let thinking = block
                .get("thinking")
                .and_then(|v| v.as_str())
                .unwrap_or_default();
            assert!(
                !thinking.is_empty(),
                "thinking block must have non-empty `thinking` text: {block}"
            );
        }
    }
}

/// When a downstream `/v1/messages` request echoes back an assistant `thinking` block with
/// plaintext content and signature (the common Claude round-trip case), monoize must forward
/// both fields verbatim to the upstream Messages provider. Signature integrity is critical
/// for newer Claude models (Sonnet 4.x and Opus 4.x) where `signature` is the encrypted
/// reasoning payload, not a verifier.
#[tokio::test]
async fn messages_request_preserves_thinking_and_signature_through_messages_upstream() {
    let ctx = setup().await;
    let (status, _body) = json_post(
        &ctx,
        "/v1/messages",
        json!({
            "model": "gpt-5-mini-msg",
            "max_tokens": 64,
            "messages": [
                { "role": "user", "content": [{ "type": "text", "text": "hi" }] },
                {
                    "role": "assistant",
                    "content": [
                        {
                            "type": "thinking",
                            "thinking": "prior reasoning text",
                            "signature": "encrypted_reasoning_blob"
                        },
                        { "type": "text", "text": "prior answer" }
                    ]
                },
                { "role": "user", "content": [{ "type": "text", "text": "continue" }] }
            ]
        }),
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    let upstream = last_captured_body(&ctx, "messages");
    let assistant = upstream["messages"]
        .as_array()
        .unwrap()
        .iter()
        .find(|m| m.get("role").and_then(|v| v.as_str()) == Some("assistant"))
        .expect("assistant turn forwarded");
    let thinking_block = assistant["content"]
        .as_array()
        .unwrap()
        .iter()
        .find(|b| b.get("type").and_then(|v| v.as_str()) == Some("thinking"))
        .expect("thinking block forwarded");
    assert_eq!(thinking_block["thinking"], "prior reasoning text");
    assert_eq!(thinking_block["signature"], "encrypted_reasoning_blob");
}

/// Downstream `/v1/messages` MUST accept `redacted_thinking` content blocks per PM5a and the
/// upstream request MUST re-emit the block with its `data` field preserved verbatim and the
/// block type MUST be `redacted_thinking` (not `thinking`). See DM5.1 case 2.
#[tokio::test]
async fn messages_request_roundtrips_redacted_thinking_block() {
    let ctx = setup().await;
    let (status, _body) = json_post(
        &ctx,
        "/v1/messages",
        json!({
            "model": "gpt-5-mini-msg",
            "max_tokens": 64,
            "messages": [
                { "role": "user", "content": [{ "type": "text", "text": "hi" }] },
                {
                    "role": "assistant",
                    "content": [
                        {
                            "type": "redacted_thinking",
                            "data": "redacted_opaque_blob"
                        },
                        { "type": "text", "text": "answer" }
                    ]
                },
                { "role": "user", "content": [{ "type": "text", "text": "again" }] }
            ]
        }),
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    let upstream = last_captured_body(&ctx, "messages");
    let assistant = upstream["messages"]
        .as_array()
        .unwrap()
        .iter()
        .find(|m| m.get("role").and_then(|v| v.as_str()) == Some("assistant"))
        .expect("assistant turn forwarded");
    let redacted = assistant["content"]
        .as_array()
        .unwrap()
        .iter()
        .find(|b| b.get("type").and_then(|v| v.as_str()) == Some("redacted_thinking"))
        .expect("redacted_thinking block forwarded unchanged");
    assert_eq!(redacted["data"], "redacted_opaque_blob");
    assert!(
        redacted.get("thinking").is_none(),
        "redacted_thinking blocks must not carry a `thinking` field"
    );
}

/// End-to-end round trip for OpenAI Responses `rs_...` item id preservation through a Messages
/// downstream. Simulates the Claude Code -> monoize -> Responses upstream scenario that
/// originally produced `invalid_encrypted_content: Encrypted content item_id did not match`.
///
/// Flow:
/// 1. Client sends a downstream `/v1/messages` request whose assistant history contains a
///    `thinking` block whose `signature` carries the sigil `mz1.rs_original.<sig>`.
/// 2. monoize decodes the block into `Node::Reasoning { id: Some("rs_original"), encrypted: "<sig>" }`.
/// 3. monoize encodes the URP request to a Responses upstream.
/// 4. The upstream Responses request MUST contain a `reasoning` item whose `id` is exactly
///    `rs_original` - not a freshly synthesized `rs_urp_*` - and whose `encrypted_content` is
///    the stripped original signature, not the sigil string.
#[tokio::test]
async fn messages_item_id_roundtrips_to_responses_upstream_item_id() {
    let ctx = setup().await;
    let sigil = "mz1.rs_original_from_upstream.prior_encrypted_content";
    let (status, _body) = json_post(
        &ctx,
        "/v1/messages",
        json!({
            "model": "gpt-5-mini",
            "max_tokens": 64,
            "messages": [
                { "role": "user", "content": [{ "type": "text", "text": "first turn" }] },
                {
                    "role": "assistant",
                    "content": [
                        {
                            "type": "thinking",
                            "thinking": "prior reasoning",
                            "signature": sigil
                        },
                        { "type": "text", "text": "prior answer" }
                    ]
                },
                { "role": "user", "content": [{ "type": "text", "text": "continue" }] }
            ]
        }),
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    let upstream = last_captured_body(&ctx, "responses");
    let input = upstream["input"].as_array().expect("responses input array");
    let reasoning_item = input
        .iter()
        .find(|item| item.get("type").and_then(|v| v.as_str()) == Some("reasoning"))
        .expect("responses upstream request should contain the replayed reasoning item");
    assert_eq!(
        reasoning_item["id"].as_str(),
        Some("rs_original_from_upstream"),
        "Reasoning item id must be extracted from the signature sigil and forwarded so that `encrypted_content` stays cryptographically bound to the original upstream item id"
    );
    assert_eq!(
        reasoning_item["encrypted_content"].as_str(),
        Some("prior_encrypted_content"),
        "encrypted_content must be the original signature, stripped of the sigil prefix"
    );
}

/// When monoize returns a `/v1/messages` response that embeds reasoning originally produced by
/// a Responses upstream, the downstream Anthropic `thinking.signature` MUST carry the sigil
/// `mz1.<item_id>.<original_signature>`. Claude Code and other Anthropic clients strip unknown
/// content-block fields, so we smuggle the item id inside `signature` instead of attaching a
/// custom field.
#[tokio::test]
async fn messages_response_signature_embeds_item_id_sigil_from_responses_upstream() {
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
        .find(|b| b.get("type").and_then(|t| t.as_str()) == Some("thinking"))
        .expect("downstream response should contain a thinking block");
    let signature = thinking
        .get("signature")
        .and_then(|v| v.as_str())
        .unwrap_or("");
    assert_eq!(
        signature, "mz1.rs_mock.mock_sig",
        "thinking.signature must embed the sigil `mz1.<item_id>.<original_signature>` so downstream clients echo it back and monoize can recover the item id"
    );
}

/// When forwarding an assistant reasoning node to a real Anthropic upstream (a Messages-type
/// provider), monoize MUST strip any sigil prefix from `signature` so that the upstream receives
/// only the opaque original payload. Otherwise Anthropic's own signature validation would reject
/// the sigil-prefixed value.
#[tokio::test]
async fn messages_upstream_request_strips_sigil_from_signature() {
    let ctx = setup().await;
    let sigil = "mz1.rs_original.original_anthropic_signature";
    let (status, _body) = json_post(
        &ctx,
        "/v1/messages",
        json!({
            "model": "gpt-5-mini-msg",
            "max_tokens": 64,
            "messages": [
                { "role": "user", "content": [{ "type": "text", "text": "hi" }] },
                {
                    "role": "assistant",
                    "content": [
                        {
                            "type": "thinking",
                            "thinking": "prior reasoning",
                            "signature": sigil
                        },
                        { "type": "text", "text": "prior answer" }
                    ]
                },
                { "role": "user", "content": [{ "type": "text", "text": "continue" }] }
            ]
        }),
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    let upstream = last_captured_body(&ctx, "messages");
    let assistant = upstream["messages"]
        .as_array()
        .unwrap()
        .iter()
        .find(|m| m.get("role").and_then(|v| v.as_str()) == Some("assistant"))
        .expect("assistant turn forwarded");
    let thinking_block = assistant["content"]
        .as_array()
        .unwrap()
        .iter()
        .find(|b| b.get("type").and_then(|v| v.as_str()) == Some("thinking"))
        .expect("thinking block forwarded");
    assert_eq!(
        thinking_block["signature"].as_str(),
        Some("original_anthropic_signature"),
        "Messages upstream must receive a clean signature, stripped of monoize's sigil prefix, so Anthropic's signature validation does not reject it"
    );
}
