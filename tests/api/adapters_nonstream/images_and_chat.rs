
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
    assert_eq!(
        upstream["models"],
        json!(["openai/gpt-5-mini", "anthropic/claude-3.7-sonnet"])
    );
    assert_eq!(upstream["route"], json!("fallback"));
    assert_eq!(
        upstream["provider"],
        json!({ "order": ["openai", "anthropic"], "allow_fallbacks": true })
    );
    assert_eq!(
        upstream["plugins"],
        json!([{ "id": "web", "enabled": true }])
    );
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
async fn messages_anthropic_custom_and_builtin_tools_are_forwarded_as_native_descriptors() {
    let ctx = setup().await;
    let (status, body) = json_post(
        &ctx,
        "/v1/messages",
        json!({
            "model": "gpt-5-mini-msg",
            "max_tokens": 64,
            "messages": [{ "role": "user", "content": [{ "type": "text", "text": "use native tools" }] }],
            "tools": [
                {
                    "type": "custom",
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
                },
                {
                    "type": "computer_20251124",
                    "name": "computer",
                    "display_width_px": 1280,
                    "display_height_px": 720,
                    "display_number": 1,
                    "enable_zoom": true
                },
                {
                    "type": "web_search_20260209",
                    "name": "web_search",
                    "max_uses": 3,
                    "allowed_domains": ["example.com"],
                    "user_location": {
                        "type": "approximate",
                        "country": "US",
                        "region": "CA",
                        "city": "San Francisco"
                    }
                },
                {
                    "type": "mcp_toolset",
                    "mcp_server_name": "docs",
                    "default_config": { "enabled": true },
                    "configs": {
                        "search": { "enabled": true, "defer_loading": true }
                    },
                    "cache_control": { "type": "ephemeral" }
                }
            ]
        }),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{body}");

    let upstream = last_captured_body(&ctx, "messages");
    let tools = upstream["tools"]
        .as_array()
        .expect("messages upstream tools");
    assert_eq!(tools.len(), 4, "{upstream}");
    assert!(
        tools
            .iter()
            .all(|tool| tool.get("function").is_none() && tool.get("custom").is_none()),
        "Anthropic tools must stay flat descriptors: {upstream}"
    );

    assert_eq!(tools[0]["type"], json!("custom"));
    assert_eq!(tools[0]["name"], json!("structured_lookup"));
    assert_eq!(tools[0]["description"], json!("Structured lookup"));
    assert_eq!(tools[0]["strict"], json!(true));
    assert_eq!(tools[0]["cache_control"], json!({ "type": "ephemeral" }));
    assert_eq!(tools[0]["defer_loading"], json!(true));
    assert_eq!(
        tools[0]["allowed_callers"],
        json!(["code_execution_20260120"])
    );
    assert_eq!(tools[0]["input_examples"], json!([{ "query": "docs" }]));
    assert_eq!(tools[0]["eager_input_streaming"], json!(true));
    assert_eq!(
        tools[0]["input_schema"]["properties"]["query"]["type"],
        json!("string")
    );
    assert!(tools[0]["input_schema"].get("cache_control").is_none());
    assert!(tools[0]["input_schema"].get("allowed_callers").is_none());
    assert!(tools[0]["input_schema"].get("input_examples").is_none());
    assert!(
        tools[0]["input_schema"]
            .get("eager_input_streaming")
            .is_none()
    );

    assert_eq!(tools[1]["type"], json!("computer_20251124"));
    assert_eq!(tools[1]["name"], json!("computer"));
    assert_eq!(tools[1]["display_width_px"], json!(1280));
    assert_eq!(tools[1]["display_height_px"], json!(720));
    assert_eq!(tools[1]["display_number"], json!(1));
    assert_eq!(tools[1]["enable_zoom"], json!(true));

    assert_eq!(tools[2]["type"], json!("web_search_20260209"));
    assert_eq!(tools[2]["name"], json!("web_search"));
    assert_eq!(tools[2]["max_uses"], json!(3));
    assert_eq!(tools[2]["allowed_domains"], json!(["example.com"]));
    assert_eq!(tools[2]["user_location"]["city"], json!("San Francisco"));

    assert_eq!(tools[3]["type"], json!("mcp_toolset"));
    assert_eq!(tools[3]["mcp_server_name"], json!("docs"));
    assert_eq!(tools[3]["default_config"], json!({ "enabled": true }));
    assert_eq!(tools[3]["configs"]["search"]["defer_loading"], json!(true));
    assert_eq!(tools[3]["cache_control"], json!({ "type": "ephemeral" }));
}

#[tokio::test]
async fn responses_tool_definition_function_extras_preserved_for_responses_upstream() {
    let ctx = setup().await;
    let (status, body) = json_post(
        &ctx,
        "/v1/responses",
        json!({
            "model": "gpt-5-mini",
            "input": [{ "type": "message", "role": "user", "content": [{ "type": "input_text", "text": "function extras" }] }],
            "tools": [{
                "type": "function",
                "name": "structured_lookup",
                "description": "Structured lookup",
                "parameters": {
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

    let upstream = last_captured_body(&ctx, "responses");
    let tools = upstream["tools"]
        .as_array()
        .expect("responses upstream tools");
    assert_eq!(tools.len(), 1, "{upstream}");
    let tool = &tools[0];
    assert_eq!(tool["type"], json!("function"));
    assert_eq!(tool["name"], json!("structured_lookup"));
    assert_eq!(tool["description"], json!("Structured lookup"));
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
async fn responses_tool_definition_function_extras_map_to_chat_upstream() {
    let ctx = setup().await;
    let (status, body) = json_post(
        &ctx,
        "/v1/responses",
        json!({
            "model": "gpt-5-mini-chat",
            "input": [{ "type": "message", "role": "user", "content": [{ "type": "input_text", "text": "function extras to chat" }] }],
            "tools": [{
                "type": "function",
                "name": "structured_lookup",
                "parameters": { "type": "object", "properties": { "query": { "type": "string" } } },
                "strict": true,
                "defer_loading": true,
                "cache_control": { "type": "ephemeral" }
            }]
        }),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{body}");

    let upstream = last_captured_body(&ctx, "chat");
    let tools = upstream["tools"].as_array().expect("chat upstream tools");
    assert_eq!(tools.len(), 1, "{upstream}");
    let tool = &tools[0];
    assert_eq!(tool["type"], json!("function"));
    assert_eq!(tool["function"]["name"], json!("structured_lookup"));
    assert_eq!(tool["function"]["strict"], json!(true));
    assert_eq!(
        tool["function"]["parameters"]["properties"]["query"]["type"],
        json!("string")
    );
    assert_eq!(tool["defer_loading"], json!(true));
    assert_eq!(tool["cache_control"], json!({ "type": "ephemeral" }));
    assert!(tool.get("parameters").is_none());
    assert!(tool.get("input_schema").is_none());
}
