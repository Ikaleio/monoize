
async fn enable_auto_cache_openai_prompt(ctx: &TestContext) {
    ctx.state.monoize_runtime.write().await.global_transforms =
        vec![monoize::transforms::TransformRuleConfig {
            transform: "auto_cache_openai_prompt".to_string(),
            enabled: true,
            models: None,
            phase: monoize::transforms::Phase::Request,
            config: json!({}),
        }];
}

#[tokio::test]
async fn responses_upstream_auto_cache_openai_prompt_adds_top_level_cache_fields() {
    let ctx = setup().await;
    enable_auto_cache_openai_prompt(&ctx).await;

    let (status, body) = json_post(
        &ctx,
        "/v1/responses",
        json!({
            "model": "gpt-5-mini",
            "instructions": "Keep answers short.",
            "input": "cache me"
        }),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{body}");

    let upstream = last_captured_body(&ctx, "responses");
    assert_eq!(upstream["prompt_cache_retention"], json!("24h"));
    let key = upstream["prompt_cache_key"]
        .as_str()
        .expect("prompt_cache_key");
    assert!(
        key.starts_with("mzpc_") && key.len() == "mzpc_".len() + 32,
        "unexpected prompt_cache_key: {key}"
    );
}

#[tokio::test]
async fn responses_structured_instructions_replay_exactly_without_input_duplication() {
    let ctx = setup().await;
    let instructions = json!([
        {
            "type": "message",
            "role": "system",
            "content": [{ "type": "input_text", "text": "system policy" }],
            "future_instruction_field": { "enabled": true }
        },
        { "type": "input_text", "text": "developer policy", "future_part_field": 7 }
    ]);

    let (status, body) = json_post(
        &ctx,
        "/v1/responses",
        json!({
            "model": "gpt-5-mini",
            "instructions": instructions.clone(),
            "input": "answer"
        }),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{body}");

    let upstream = last_captured_body(&ctx, "responses");
    assert_eq!(upstream["instructions"], instructions);
    let serialized_input = serde_json::to_string(&upstream["input"]).unwrap();
    assert!(serialized_input.contains("answer"), "{upstream}");
    assert!(!serialized_input.contains("system policy"), "{upstream}");
    assert!(!serialized_input.contains("developer policy"), "{upstream}");
    assert!(!upstream.to_string().contains("_monoize_"), "{upstream}");
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
            "max_tokens": 4096,
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

    let upstream = last_captured_body(&ctx, "chat");
    assert_eq!(
        upstream["tool_choice"],
        json!({ "type": "function", "function": { "name": "tool_a" } })
    );
    assert!(
        upstream["tools"][0]
            .get("disable_parallel_tool_use")
            .is_none(),
        "Anthropic tool_choice controls must not move into tool descriptors: {upstream}"
    );
}

#[tokio::test]
async fn messages_tool_choice_any_roundtrips_for_messages_upstream() {
    let ctx = setup().await;
    let (status, body) = json_post(
        &ctx,
        "/v1/messages",
        json!({
            "model": "gpt-5-mini-msg",
            "max_tokens": 64,
            "messages": [{ "role": "user", "content": [{ "type": "text", "text": "use tools" }] }],
            "tools": [
              { "name": "tool_a", "input_schema": { "type": "object", "additionalProperties": true } }
            ],
            "tool_choice": { "type": "any" }
        }),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{body}");

    let upstream = last_captured_body(&ctx, "messages");
    assert_eq!(upstream["tool_choice"], json!({ "type": "any" }));
}

#[tokio::test]
async fn messages_tool_choice_none_roundtrips_for_messages_upstream() {
    let ctx = setup().await;
    let (status, body) = json_post(
        &ctx,
        "/v1/messages",
        json!({
            "model": "gpt-5-mini-msg",
            "max_tokens": 64,
            "messages": [{ "role": "user", "content": [{ "type": "text", "text": "no tools" }] }],
            "tools": [
              { "name": "tool_a", "input_schema": { "type": "object", "additionalProperties": true } }
            ],
            "tool_choice": { "type": "none" }
        }),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{body}");

    let upstream = last_captured_body(&ctx, "messages");
    assert_eq!(upstream["tool_choice"], json!({ "type": "none" }));
}

#[tokio::test]
async fn messages_tool_choice_tool_disable_parallel_roundtrips_for_messages_upstream() {
    let ctx = setup().await;
    let (status, body) = json_post(
        &ctx,
        "/v1/messages",
        json!({
            "model": "gpt-5-mini-msg",
            "max_tokens": 64,
            "messages": [{ "role": "user", "content": [{ "type": "text", "text": "use one tool" }] }],
            "tools": [
              { "name": "tool_a", "input_schema": { "type": "object", "additionalProperties": true } }
            ],
            "tool_choice": { "type": "tool", "name": "tool_a", "disable_parallel_tool_use": true }
        }),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{body}");

    let upstream = last_captured_body(&ctx, "messages");
    assert_eq!(
        upstream["tool_choice"],
        json!({ "type": "tool", "name": "tool_a", "disable_parallel_tool_use": true })
    );
    assert!(
        upstream["tools"][0]
            .get("disable_parallel_tool_use")
            .is_none(),
        "disable_parallel_tool_use belongs to request tool_choice, not the tool descriptor: {upstream}"
    );
}

#[tokio::test]
async fn messages_tool_choice_disable_parallel_maps_to_chat_top_level_parallel_false() {
    let ctx = setup().await;
    let (status, body) = json_post(
        &ctx,
        "/v1/messages",
        json!({
            "model": "gpt-5-mini-chat",
            "max_tokens": 64,
            "messages": [{ "role": "user", "content": [{ "type": "text", "text": "use one tool" }] }],
            "tools": [
              { "name": "tool_a", "input_schema": { "type": "object", "additionalProperties": true } }
            ],
            "tool_choice": { "type": "tool", "name": "tool_a", "disable_parallel_tool_use": true }
        }),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{body}");

    let upstream = last_captured_body(&ctx, "chat");
    assert_eq!(upstream["parallel_tool_calls"], json!(false));
    assert_eq!(
        upstream["tool_choice"],
        json!({ "type": "function", "function": { "name": "tool_a" } })
    );
    assert!(
        upstream["tools"][0].get("parallel_tool_calls").is_none(),
        "parallel_tool_calls must remain top-level for Chat upstream: {upstream}"
    );
}

#[tokio::test]
async fn messages_parallel_false_without_tools_does_not_synthesize_messages_tool_choice() {
    let ctx = setup().await;
    let (status, body) = json_post(
        &ctx,
        "/v1/messages",
        json!({
            "model": "gpt-5-mini-msg",
            "max_tokens": 64,
            "messages": [{ "role": "user", "content": [{ "type": "text", "text": "no tools here" }] }],
            "parallel_tool_calls": false
        }),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{body}");

    let upstream = last_captured_body(&ctx, "messages");
    assert!(
        upstream.get("tool_choice").is_none(),
        "parallel_tool_calls=false without tools must not synthesize Anthropic tool_choice: {upstream}"
    );
}

/// An empty `thinking` value with a non-empty `signature` is Anthropic omitted thinking.
/// Same-family request conversion must retain both fields exactly.
#[tokio::test]
async fn messages_request_roundtrips_omitted_thinking_block() {
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
    let omitted = assistant["content"]
        .as_array()
        .unwrap()
        .iter()
        .find(|block| block.get("type").and_then(|v| v.as_str()) == Some("thinking"))
        .expect("omitted thinking block forwarded");
    assert_eq!(omitted["thinking"].as_str(), Some(""));
    assert_eq!(omitted["signature"].as_str(), Some("sig_orphan"));
    assert!(
        omitted.get("encrypted_thinking").is_none(),
        "encrypted_thinking is not part of the Anthropic wire contract: {omitted}"
    );
}
