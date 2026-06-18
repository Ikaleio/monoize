
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
async fn messages_upstream_auto_cache_openai_prompt_is_noop() {
    let ctx = setup().await;
    enable_auto_cache_openai_prompt(&ctx).await;

    let (status, body) = json_post(
        &ctx,
        "/v1/messages",
        json!({
            "model": "gpt-5-mini-msg",
            "max_tokens": 64,
            "messages": [{ "role": "user", "content": [{ "type": "text", "text": "hello" }] }]
        }),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{body}");

    let upstream = last_captured_body(&ctx, "messages");
    assert!(upstream.get("prompt_cache_key").is_none(), "{upstream}");
    assert!(
        upstream.get("prompt_cache_retention").is_none(),
        "{upstream}"
    );
}

#[tokio::test]
async fn messages_tool_definition_metadata_preserved_same_family() {
    let ctx = setup().await;
    let (status, body) = json_post(
        &ctx,
        "/v1/messages",
        json!({
            "model": "gpt-5-mini-msg",
            "max_tokens": 64,
            "messages": [{ "role": "user", "content": [{ "type": "text", "text": "metadata same family" }] }],
            "tools": [{
                "name": "structured_lookup",
                "description": "Structured lookup",
                "input_schema": {
                    "type": "object",
                    "properties": { "query": { "type": "string" } },
                    "required": ["query"],
                    "additionalProperties": false
                },
                "strict": true,
                "cache_control": { "type": "ephemeral" },
                "defer_loading": true,
                "allowed_callers": ["code_execution_20260120"],
                "input_examples": [{ "query": "docs" }],
                "eager_input_streaming": true
            }]
        }),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{body}");

    let upstream = last_captured_body(&ctx, "messages");
    let tools = upstream["tools"]
        .as_array()
        .expect("messages upstream tools");
    assert_eq!(tools.len(), 1, "{upstream}");
    let tool = &tools[0];
    assert_eq!(tool["name"], json!("structured_lookup"));
    assert_eq!(tool["description"], json!("Structured lookup"));
    assert_eq!(tool["strict"], json!(true));
    assert_eq!(tool["cache_control"], json!({ "type": "ephemeral" }));
    assert_eq!(tool["defer_loading"], json!(true));
    assert_eq!(tool["allowed_callers"], json!(["code_execution_20260120"]));
    assert_eq!(tool["input_examples"], json!([{ "query": "docs" }]));
    assert_eq!(tool["eager_input_streaming"], json!(true));
    assert_eq!(
        tool["input_schema"]["properties"]["query"]["type"],
        json!("string")
    );
    assert!(tool.get("function").is_none());
    assert!(tool.get("parameters").is_none());
    assert!(tool["input_schema"].get("cache_control").is_none());
    assert!(tool["input_schema"].get("strict").is_none());
}

#[tokio::test]
async fn messages_tool_definition_metadata_maps_to_responses_upstream() {
    let ctx = setup().await;
    let (status, body) = json_post(
        &ctx,
        "/v1/messages",
        json!({
            "model": "gpt-5-mini",
            "max_tokens": 64,
            "messages": [{ "role": "user", "content": [{ "type": "text", "text": "metadata to responses" }] }],
            "tools": [{
                "name": "structured_lookup",
                "input_schema": {
                    "type": "object",
                    "properties": { "query": { "type": "string" } },
                    "additionalProperties": false
                },
                "strict": true,
                "cache_control": { "type": "ephemeral" },
                "defer_loading": true,
                "allowed_callers": ["code_execution_20260120"],
                "input_examples": [{ "query": "docs" }],
                "eager_input_streaming": true
            }]
        }),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{body}");

    let upstream = last_captured_body(&ctx, "responses");
    let tools = upstream["tools"]
        .as_array()
        .expect("responses upstream tools");
    assert_eq!(tools.len(), 1, "{upstream}");
    let tool = &tools[0];
    assert_eq!(tool["type"], json!("function"));
    assert_eq!(tool["name"], json!("structured_lookup"));
    assert_eq!(tool["strict"], json!(true));
    assert_eq!(tool["cache_control"], json!({ "type": "ephemeral" }));
    assert_eq!(tool["defer_loading"], json!(true));
    assert_eq!(tool["allowed_callers"], json!(["code_execution_20260120"]));
    assert_eq!(tool["input_examples"], json!([{ "query": "docs" }]));
    assert_eq!(tool["eager_input_streaming"], json!(true));
    assert_eq!(
        tool["parameters"]["properties"]["query"]["type"],
        json!("string")
    );
    assert!(tool.get("function").is_none());
    assert!(tool.get("input_schema").is_none());
}

#[tokio::test]
async fn chat_tool_definition_custom_tool_maps_to_responses_upstream() {
    let ctx = setup().await;
    let (status, body) = json_post(
        &ctx,
        "/v1/chat/completions",
        json!({
            "model": "gpt-5-mini",
            "messages": [{ "role": "user", "content": "custom to responses" }],
            "tools": [{
                "type": "custom",
                "custom": {
                    "name": "freeform_lookup",
                    "description": "Freeform lookup",
                    "format": { "type": "text" },
                    "defer_loading": true
                }
            }]
        }),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{body}");

    let upstream = last_captured_body(&ctx, "responses");
    let tools = upstream["tools"]
        .as_array()
        .expect("responses upstream tools");
    assert_eq!(tools.len(), 1, "{upstream}");
    let tool = &tools[0];
    assert_eq!(tool["type"], json!("custom"));
    assert_eq!(tool["name"], json!("freeform_lookup"));
    assert_eq!(tool["description"], json!("Freeform lookup"));
    assert_eq!(tool["format"], json!({ "type": "text" }));
    assert_eq!(tool["defer_loading"], json!(true));
    assert!(tool.get("custom").is_none());
    assert!(tool.get("function").is_none());
    assert!(tool.get("parameters").is_none());
    assert!(tool.get("input_schema").is_none());
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
    assert!(
        input_message.get("message_local").is_none(),
        "cross-family nested envelope extra must strip: {input_message}"
    );
    assert!(
        input_message["content"][0].get("nested_local").is_none(),
        "cross-family nested node extra must strip: {input_message}"
    );
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
    assert!(
        input[1].get("first_only").is_none(),
        "next_downstream_envelope_extra must apply once: {input:?}"
    );
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
        input
            .iter()
            .filter(|item| item["type"].as_str() == Some("function_call_output"))
            .count(),
        1,
        "tool result must stay distinct: {input:?}"
    );
    assert_eq!(
        input
            .iter()
            .filter(|item| item.get("chat_only_extra").is_some())
            .count(),
        0,
        "cross-family next_downstream_envelope_extra must strip: {input:?}"
    );
    assert!(
        !input.iter().any(|item| {
            item["type"].as_str() == Some("message")
                && item["content"]
                    .as_array()
                    .map(|parts| parts.is_empty())
                    .unwrap_or(false)
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
            "messages": [
                {
                    "role": "assistant",
                    "content": [{
                        "type": "tool_use",
                        "id": "call_x",
                        "name": "tool_x",
                        "input": {},
                        "tool_use_extra": "strip-before-responses"
                    }],
                    "message_extra": "strip-too"
                },
                {
                    "role": "user",
                    "content": [{
                        "type": "tool_result",
                        "tool_use_id": "call_x",
                        "content": "R1",
                        "tool_result_extra": "strip-before-responses"
                    }],
                    "message_extra": "strip-too"
                }
            ]
        }),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{body}");

    let upstream = last_captured_body(&ctx, "responses");
    let input = upstream["input"].as_array().expect("responses input array");
    assert_eq!(input.len(), 2);
    assert_eq!(input[0]["type"], json!("function_call"));
    assert_eq!(input[1]["type"], json!("function_call_output"));
    assert!(input[0].get("message_extra").is_none());
    assert!(input[0].get("tool_use_extra").is_none());
    assert!(input[1].get("message_extra").is_none());
    assert!(input[1].get("tool_result_extra").is_none());
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
    assert!(
        chat_upstream["input"][0]["content"][0]
            .get("phase")
            .is_none(),
        "chat cross-family phase must strip: {chat_upstream}"
    );

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
    assert!(
        messages_upstream["input"][0]["content"][0]
            .get("phase")
            .is_none(),
        "messages cross-family phase must strip: {messages_upstream}"
    );
}
