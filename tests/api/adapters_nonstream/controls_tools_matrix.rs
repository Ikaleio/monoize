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

fn messages_tool_choice_cases() -> Vec<(&'static str, Value, Value, Value, Option<bool>)> {
    vec![
        (
            "auto-default",
            json!({ "type": "auto" }),
            json!("auto"),
            json!("auto"),
            None,
        ),
        (
            "auto-disabled",
            json!({ "type": "auto", "disable_parallel_tool_use": true }),
            json!("auto"),
            json!("auto"),
            Some(false),
        ),
        (
            "any-default",
            json!({ "type": "any" }),
            json!("required"),
            json!("required"),
            None,
        ),
        (
            "any-enabled",
            json!({ "type": "any", "disable_parallel_tool_use": false }),
            json!("required"),
            json!("required"),
            Some(true),
        ),
        (
            "none",
            json!({ "type": "none" }),
            json!("none"),
            json!("none"),
            None,
        ),
        (
            "named-default",
            json!({ "type": "tool", "name": "tool_a" }),
            json!({ "type": "function", "name": "tool_a" }),
            json!({ "type": "function", "function": { "name": "tool_a" } }),
            None,
        ),
        (
            "named-disabled",
            json!({
                "type": "tool",
                "name": "tool_a",
                "disable_parallel_tool_use": true
            }),
            json!({ "type": "function", "name": "tool_a" }),
            json!({ "type": "function", "function": { "name": "tool_a" } }),
            Some(false),
        ),
    ]
}

#[tokio::test]
async fn messages_tool_choice_variants_map_to_responses_upstream() {
    for (label, tool_choice, expected_choice, _, expected_parallel) in messages_tool_choice_cases()
    {
        let ctx = setup().await;
        let (status, body) = json_post(
            &ctx,
            "/v1/messages",
            json!({
                "model": "gpt-5-mini",
                "max_tokens": 64,
                "messages": [{
                    "role": "user",
                    "content": [{ "type": "text", "text": label }]
                }],
                "tools": [{
                    "name": "tool_a",
                    "input_schema": {
                        "type": "object",
                        "additionalProperties": true
                    }
                }],
                "tool_choice": tool_choice
            }),
        )
        .await;
        assert_eq!(status, StatusCode::OK, "{label}: {body}");

        let upstream = last_captured_body(&ctx, "responses");
        assert_eq!(
            upstream["tool_choice"], expected_choice,
            "{label}: {upstream}"
        );
        match expected_parallel {
            Some(expected) => {
                assert_eq!(upstream["parallel_tool_calls"], json!(expected), "{label}")
            }
            None => assert!(
                upstream.get("parallel_tool_calls").is_none(),
                "{label}: {upstream}"
            ),
        }
        assert!(
            upstream["tools"]
                .as_array()
                .expect("responses tools")
                .iter()
                .all(|tool| tool.get("parallel_tool_calls").is_none()
                    && tool.get("disable_parallel_tool_use").is_none()),
            "{label}: request controls must not enter tool descriptors: {upstream}"
        );
    }
}

#[tokio::test]
async fn messages_tool_choice_variants_map_to_chat_upstream() {
    for (label, tool_choice, _, expected_choice, expected_parallel) in messages_tool_choice_cases()
    {
        let ctx = setup().await;
        let (status, body) = json_post(
            &ctx,
            "/v1/messages",
            json!({
                "model": "gpt-5-mini-chat",
                "max_tokens": 64,
                "messages": [{
                    "role": "user",
                    "content": [{ "type": "text", "text": label }]
                }],
                "tools": [{
                    "name": "tool_a",
                    "input_schema": {
                        "type": "object",
                        "additionalProperties": true
                    }
                }],
                "tool_choice": tool_choice
            }),
        )
        .await;
        assert_eq!(status, StatusCode::OK, "{label}: {body}");

        let upstream = last_captured_body(&ctx, "chat");
        assert_eq!(
            upstream["tool_choice"], expected_choice,
            "{label}: {upstream}"
        );
        match expected_parallel {
            Some(expected) => {
                assert_eq!(upstream["parallel_tool_calls"], json!(expected), "{label}")
            }
            None => assert!(
                upstream.get("parallel_tool_calls").is_none(),
                "{label}: {upstream}"
            ),
        }
        assert!(
            upstream["tools"]
                .as_array()
                .expect("Chat tools")
                .iter()
                .all(|tool| tool.get("parallel_tool_calls").is_none()
                    && tool.get("disable_parallel_tool_use").is_none()),
            "{label}: request controls must not enter tool descriptors: {upstream}"
        );
    }
}

#[tokio::test]
async fn responses_tool_choice_variants_use_flat_responses_shapes() {
    let choices = [
        json!({ "type": "function", "name": "lookup" }),
        json!({ "type": "custom", "name": "grammar" }),
        json!({ "type": "file_search" }),
        json!({ "type": "mcp", "server_label": "docs", "name": "search" }),
        json!({
            "type": "allowed_tools",
            "mode": "required",
            "tools": [
                { "type": "function", "name": "lookup" },
                { "type": "custom", "name": "grammar" },
                { "type": "mcp", "server_label": "docs" },
                { "type": "image_generation" }
            ]
        }),
    ];

    for tool_choice in choices {
        let ctx = setup().await;
        let (status, body) = json_post(
            &ctx,
            "/v1/responses",
            json!({
                "model": "gpt-5-mini",
                "input": "responses selector",
                "tools": [
                    {
                        "type": "function",
                        "name": "lookup",
                        "parameters": { "type": "object", "additionalProperties": true }
                    },
                    {
                        "type": "custom",
                        "name": "grammar",
                        "format": { "type": "text" }
                    },
                    { "type": "file_search", "vector_store_ids": ["vs_1"] },
                    {
                        "type": "mcp",
                        "server_label": "docs",
                        "server_url": "https://mcp.example.test"
                    },
                    { "type": "image_generation" }
                ],
                "tool_choice": tool_choice
            }),
        )
        .await;
        assert_eq!(status, StatusCode::OK, "{body}");

        let upstream = last_captured_body(&ctx, "responses");
        assert_eq!(upstream["tool_choice"], tool_choice, "{upstream}");
    }
}

#[tokio::test]
async fn allowed_tools_uses_each_openai_target_family_wrapper() {
    let responses_to_chat = setup().await;
    let (status, body) = json_post(
        &responses_to_chat,
        "/v1/responses",
        json!({
            "model": "gpt-5-mini-chat",
            "input": "responses allowed tools to Chat",
            "tools": [
                {
                    "type": "function",
                    "name": "lookup",
                    "parameters": { "type": "object", "additionalProperties": true }
                },
                { "type": "custom", "name": "grammar", "format": { "type": "text" } }
            ],
            "tool_choice": {
                "type": "allowed_tools",
                "mode": "auto",
                "tools": [
                    { "type": "function", "name": "lookup" },
                    { "type": "custom", "name": "grammar" }
                ]
            }
        }),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{body}");
    let chat = last_captured_body(&responses_to_chat, "chat");
    assert_eq!(
        chat["tool_choice"],
        json!({
            "type": "allowed_tools",
            "allowed_tools": {
                "mode": "auto",
                "tools": [
                    { "type": "function", "function": { "name": "lookup" } },
                    { "type": "custom", "custom": { "name": "grammar" } }
                ]
            }
        })
    );

    let chat_to_responses = setup().await;
    let (status, body) = json_post(
        &chat_to_responses,
        "/v1/chat/completions",
        json!({
            "model": "gpt-5-mini",
            "messages": [{ "role": "user", "content": "Chat allowed tools to Responses" }],
            "tools": [
                {
                    "type": "function",
                    "function": {
                        "name": "lookup",
                        "parameters": { "type": "object", "additionalProperties": true }
                    }
                },
                {
                    "type": "custom",
                    "custom": { "name": "grammar", "format": { "type": "text" } }
                }
            ],
            "tool_choice": {
                "type": "allowed_tools",
                "allowed_tools": {
                    "mode": "required",
                    "tools": [
                        { "type": "function", "function": { "name": "lookup" } },
                        { "type": "custom", "custom": { "name": "grammar" } }
                    ]
                }
            }
        }),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{body}");
    let responses = last_captured_body(&chat_to_responses, "responses");
    assert_eq!(
        responses["tool_choice"],
        json!({
            "type": "allowed_tools",
            "mode": "required",
            "tools": [
                { "type": "function", "name": "lookup" },
                { "type": "custom", "name": "grammar" }
            ]
        })
    );
}

#[tokio::test]
async fn allowed_tools_without_definitions_is_omitted() {
    let ctx = setup().await;
    let (status, body) = json_post(
        &ctx,
        "/v1/responses",
        json!({
            "model": "gpt-5-mini",
            "input": "no tool definitions",
            "tool_choice": {
                "type": "allowed_tools",
                "mode": "required",
                "tools": [{ "type": "function", "name": "missing" }]
            }
        }),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{body}");
    let upstream = last_captured_body(&ctx, "responses");
    assert!(upstream.get("tool_choice").is_none(), "{upstream}");
}

#[tokio::test]
async fn allowed_tools_prunes_only_unavailable_references() {
    let ctx = setup().await;
    let (status, body) = json_post(
        &ctx,
        "/v1/responses",
        json!({
            "model": "gpt-5-mini",
            "input": "prune unavailable allowed tools",
            "tools": [
                {
                    "type": "function",
                    "name": "lookup",
                    "parameters": { "type": "object", "additionalProperties": true }
                },
                {
                    "type": "mcp",
                    "server_label": "docs",
                    "server_url": "https://mcp.example.test"
                }
            ],
            "tool_choice": {
                "type": "allowed_tools",
                "mode": "required",
                "tools": [
                    { "type": "function", "name": "lookup" },
                    { "type": "function", "name": "missing" },
                    { "type": "mcp", "server_label": "docs", "name": "search" },
                    { "type": "mcp", "server_label": "other" }
                ]
            }
        }),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{body}");

    let upstream = last_captured_body(&ctx, "responses");
    assert_eq!(
        upstream["tool_choice"],
        json!({
            "type": "allowed_tools",
            "mode": "required",
            "tools": [
                { "type": "function", "name": "lookup" },
                { "type": "mcp", "server_label": "docs", "name": "search" }
            ]
        }),
        "{upstream}"
    );
}

#[tokio::test]
async fn responses_user_maps_to_messages_while_verbosity_is_omitted() {
    let ctx = setup().await;
    let (status, body) = json_post(
        &ctx,
        "/v1/responses",
        json!({
            "model": "gpt-5-mini-msg",
            "input": "responses controls to messages",
            "text": { "verbosity": "high" },
            "user": "responses-user"
        }),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{body}");

    let upstream = last_captured_body(&ctx, "messages");
    assert_eq!(upstream["metadata"]["user_id"], json!("responses-user"));
    assert!(upstream.get("user").is_none(), "{upstream}");
    assert!(upstream.get("verbosity").is_none(), "{upstream}");
    assert!(upstream.get("text").is_none(), "{upstream}");
}

#[tokio::test]
async fn messages_user_maps_to_responses_while_stop_is_omitted() {
    let ctx = setup().await;
    let (status, body) = json_post(
        &ctx,
        "/v1/messages",
        json!({
            "model": "gpt-5-mini",
            "max_tokens": 64,
            "messages": [{ "role": "user", "content": "messages controls to responses" }],
            "metadata": { "user_id": "messages-user" },
            "stop_sequences": ["FIRST", "SECOND"]
        }),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{body}");

    let upstream = last_captured_body(&ctx, "responses");
    assert_eq!(upstream["user"], json!("messages-user"));
    assert!(upstream.get("stop").is_none(), "{upstream}");
    assert!(upstream.get("stop_sequences").is_none(), "{upstream}");
}

#[tokio::test]
async fn openai_custom_tool_definitions_use_target_family_shape() {
    let chat_to_responses = setup().await;
    let (status, body) = json_post(
        &chat_to_responses,
        "/v1/chat/completions",
        json!({
            "model": "gpt-5-mini",
            "messages": [{ "role": "user", "content": "chat custom" }],
            "tools": [{
                "type": "custom",
                "custom": {
                    "name": "grammar_tool",
                    "description": "Parse grammar input",
                    "format": { "type": "text" },
                    "defer_loading": true
                }
            }]
        }),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{body}");
    let responses = last_captured_body(&chat_to_responses, "responses");
    assert_eq!(
        responses["tools"][0],
        json!({
            "type": "custom",
            "name": "grammar_tool",
            "description": "Parse grammar input",
            "format": { "type": "text" },
            "defer_loading": true
        })
    );

    let responses_to_chat = setup().await;
    let (status, body) = json_post(
        &responses_to_chat,
        "/v1/responses",
        json!({
            "model": "gpt-5-mini-chat",
            "input": "responses custom",
            "tools": [{
                "type": "custom",
                "name": "grammar_tool",
                "description": "Parse grammar input",
                "format": { "type": "text" },
                "defer_loading": true
            }]
        }),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{body}");
    let chat = last_captured_body(&responses_to_chat, "chat");
    assert_eq!(chat["tools"][0]["type"], json!("custom"));
    assert_eq!(
        chat["tools"][0]["custom"],
        json!({
            "name": "grammar_tool",
            "description": "Parse grammar input",
            "format": { "type": "text" },
            "defer_loading": true
        })
    );
    assert!(chat["tools"][0].get("name").is_none(), "{chat}");
    assert!(chat["tools"][0].get("format").is_none(), "{chat}");

    let responses_to_messages = setup().await;
    let (status, body) = json_post(
        &responses_to_messages,
        "/v1/responses",
        json!({
            "model": "gpt-5-mini-msg",
            "input": "responses custom cannot become messages JSON-schema tool",
            "tools": [{
                "type": "custom",
                "name": "grammar_tool",
                "format": { "type": "text" }
            }],
            "tool_choice": { "type": "custom", "name": "grammar_tool" }
        }),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{body}");
    let messages = last_captured_body(&responses_to_messages, "messages");
    assert!(messages.get("tools").is_none(), "{messages}");
    assert!(messages.get("tool_choice").is_none(), "{messages}");
}
