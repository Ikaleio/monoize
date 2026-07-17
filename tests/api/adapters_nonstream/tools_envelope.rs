
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
              { "type": "function_call", "id": "fc_1", "call_id": "call_1", "name": "tool_a", "arguments": "{}" },
              { "type": "function_call", "id": "fc_2", "call_id": "call_2", "name": "tool_b", "arguments": "{}" },
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
                "type": "function_call",
                "id": "fc_multipart",
                "call_id": "call_multipart",
                "name": "tool_multipart",
                "arguments": "{}"
              },
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
async fn responses_tool_results_to_messages_upstream_do_not_emit_empty_text_or_split_results() {
    let ctx = setup().await;
    let (status, body) = json_post(
        &ctx,
        "/v1/responses",
        json!({
            "model": "gpt-5-mini-msg",
            "input": [
              {
                "type": "message",
                "role": "assistant",
                "content": [{ "type": "output_text", "text": "" }]
              },
              { "type": "function_call", "id": "fc_1", "call_id": "call_1", "name": "tool_a", "arguments": "{}" },
              { "type": "function_call", "id": "fc_2", "call_id": "call_2", "name": "tool_b", "arguments": "{}" },
              { "type": "function_call_output", "call_id": "call_1", "output": "R1" },
              { "type": "function_call_output", "call_id": "call_2", "output": "R2" }
            ]
        }),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{body}");

    let upstream = last_captured_body(&ctx, "messages");
    let messages = upstream["messages"].as_array().expect("messages array");
    assert_eq!(messages.len(), 2, "unexpected messages shape: {upstream}");
    assert_eq!(messages[0]["role"].as_str(), Some("assistant"));
    assert_eq!(messages[1]["role"].as_str(), Some("user"));

    let assistant_content = messages[0]["content"].as_array().expect("assistant content");
    assert_eq!(
        assistant_content
            .iter()
            .filter(|block| block.get("type").and_then(|v| v.as_str()) == Some("text"))
            .count(),
        0,
        "empty assistant text block must not be sent before tool_use blocks: {upstream}"
    );
    assert_eq!(
        assistant_content
            .iter()
            .filter(|block| block.get("type").and_then(|v| v.as_str()) == Some("tool_use"))
            .count(),
        2,
        "tool calls should remain in the assistant message: {upstream}"
    );

    let user_content = messages[1]["content"].as_array().expect("user content");
    assert_eq!(
        user_content
            .iter()
            .filter(|block| block.get("type").and_then(|v| v.as_str()) == Some("tool_result"))
            .count(),
        2,
        "parallel tool results must share one user message: {upstream}"
    );
    assert_eq!(user_content[0]["tool_use_id"].as_str(), Some("call_1"));
    assert_eq!(user_content[1]["tool_use_id"].as_str(), Some("call_2"));
}

#[tokio::test]
async fn responses_nonstream_image_generation_tool_returns_native_top_level_item() {
    let ctx = setup().await;
    let (status, body) = json_post(
        &ctx,
        "/v1/responses",
        json!({
            "model": "gpt-5-mini",
            "input": [{ "type": "message", "role": "user", "content": [{ "type": "input_text", "text": "generate image" }] }],
            "tools": [{ "type": "image_generation", "output_format": "png" }]
        }),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{body}");
    let v: Value = serde_json::from_str(&body).unwrap();
    let output = v["output"].as_array().expect("output array");
    let image = output
        .iter()
        .find(|item| item["type"].as_str() == Some("image_generation_call"))
        .expect("native image_generation_call output item");
    assert_eq!(image["id"].as_str(), Some("ig_mock"), "{body}");
    assert_eq!(image["output_format"].as_str(), Some("png"), "{body}");
    assert!(
        image["result"].as_str().is_some_and(|data| !data.is_empty()),
        "{body}"
    );
    assert!(!body.contains("output_image"), "{body}");
}

#[tokio::test]
async fn responses_custom_and_builtin_tools_are_forwarded_as_native_descriptors() {
    let ctx = setup().await;
    let (status, body) = json_post(
        &ctx,
        "/v1/responses",
        json!({
            "model": "gpt-5-mini",
            "input": [{ "type": "message", "role": "user", "content": [{ "type": "input_text", "text": "use native tools" }] }],
            "tools": [
                {
                    "type": "custom",
                    "name": "freeform_lookup",
                    "description": "Freeform lookup",
                    "format": { "type": "grammar", "syntax": "lark", "definition": "start: /[a-z]+/" },
                    "defer_loading": true
                },
                { "type": "file_search", "vector_store_ids": ["vs_1", "vs_2"] },
                { "type": "code_interpreter", "container": { "type": "auto", "file_ids": ["file_1"] } },
                {
                    "type": "web_search",
                    "search_context_size": "medium",
                    "user_location": { "type": "approximate", "country": "US" }
                },
                {
                    "type": "mcp",
                    "server_label": "docs",
                    "server_url": "https://mcp.example.test",
                    "allowed_tools": ["search"],
                    "defer_loading": true
                },
                {
                    "type": "namespace",
                    "name": "app_tools",
                    "description": "Application tools",
                    "tools": [{ "name": "fetch_docs", "description": "Fetch docs" }]
                },
                {
                    "type": "tool_search",
                    "description": "Discover tools",
                    "execution": "server",
                    "parameters": { "type": "object", "properties": {} }
                }
            ]
        }),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{body}");

    let upstream = last_captured_body(&ctx, "responses");
    let tools = upstream["tools"]
        .as_array()
        .expect("responses upstream tools");
    assert_eq!(tools.len(), 7, "{upstream}");
    assert!(
        tools.iter().all(|tool| tool.get("function").is_none()),
        "Responses custom and built-ins must not be nested function descriptors: {upstream}"
    );
    assert_eq!(tools[0]["type"], json!("custom"));
    assert_eq!(tools[0]["name"], json!("freeform_lookup"));
    assert_eq!(tools[0]["description"], json!("Freeform lookup"));
    assert_eq!(tools[0]["format"]["type"], json!("grammar"));
    assert_eq!(tools[0]["defer_loading"], json!(true));
    assert!(tools[0].get("custom").is_none());

    assert_eq!(tools[1]["type"], json!("file_search"));
    assert_eq!(tools[1]["vector_store_ids"], json!(["vs_1", "vs_2"]));
    assert_eq!(tools[2]["type"], json!("code_interpreter"));
    assert_eq!(tools[2]["container"]["file_ids"], json!(["file_1"]));
    assert_eq!(tools[3]["type"], json!("web_search"));
    assert_eq!(tools[3]["search_context_size"], json!("medium"));
    assert_eq!(tools[3]["user_location"]["country"], json!("US"));
    assert_eq!(tools[4]["type"], json!("mcp"));
    assert_eq!(tools[4]["server_label"], json!("docs"));
    assert_eq!(tools[4]["allowed_tools"], json!(["search"]));
    assert_eq!(tools[5]["type"], json!("namespace"));
    assert_eq!(tools[5]["name"], json!("app_tools"));
    assert_eq!(tools[5]["tools"][0]["name"], json!("fetch_docs"));
    assert_eq!(tools[6]["type"], json!("tool_search"));
    assert_eq!(tools[6]["description"], json!("Discover tools"));
    assert_eq!(tools[6]["execution"], json!("server"));
}

#[tokio::test]
async fn provider_native_tool_preserved_same_family() {
    let ctx = setup().await;
    let (status, body) = json_post(
        &ctx,
        "/v1/responses",
        json!({
            "model": "gpt-5-mini",
            "input": [{ "type": "message", "role": "user", "content": [{ "type": "input_text", "text": "same family native" }] }],
            "tools": [
                { "type": "function", "name": "lookup", "parameters": { "type": "object", "additionalProperties": true } },
                { "type": "custom", "name": "freeform_lookup", "format": { "type": "text" } },
                { "type": "file_search", "vector_store_ids": ["vs_same"] }
            ]
        }),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{body}");

    let upstream = last_captured_body(&ctx, "responses");
    let tools = upstream["tools"]
        .as_array()
        .expect("responses upstream tools");
    assert_eq!(tools.len(), 3, "{upstream}");
    assert_eq!(tools[0]["type"], json!("function"));
    assert_eq!(tools[0]["name"], json!("lookup"));
    assert_eq!(tools[1]["type"], json!("custom"));
    assert_eq!(tools[1]["name"], json!("freeform_lookup"));
    assert_eq!(tools[2]["type"], json!("file_search"));
    assert_eq!(tools[2]["vector_store_ids"], json!(["vs_same"]));
    assert!(tools[2].get("function").is_none());
    assert!(tools[2].get("custom").is_none());
}

#[tokio::test]
async fn provider_native_tool_filtered_cross_family() {
    let ctx = setup().await;
    let (status, body) = json_post(
        &ctx,
        "/v1/responses",
        json!({
            "model": "gpt-5-mini-chat",
            "input": [{ "type": "message", "role": "user", "content": [{ "type": "input_text", "text": "cross family native" }] }],
            "tools": [
                { "type": "function", "name": "lookup", "parameters": { "type": "object", "additionalProperties": true } },
                { "type": "custom", "name": "freeform_lookup", "format": { "type": "text" } },
                { "type": "file_search", "name": "docs_search", "vector_store_ids": ["vs_cross"] }
            ],
            "tool_choice": { "type": "file_search" }
        }),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{body}");

    let upstream = last_captured_body(&ctx, "chat");
    let tools = upstream["tools"].as_array().expect("chat upstream tools");
    assert_eq!(tools.len(), 2, "{upstream}");
    assert_eq!(tools[0]["type"], json!("function"));
    assert_eq!(tools[0]["function"]["name"], json!("lookup"));
    assert_eq!(tools[1]["type"], json!("custom"));
    assert_eq!(tools[1]["custom"]["name"], json!("freeform_lookup"));
    assert!(
        tools
            .iter()
            .all(|tool| tool.get("type").and_then(Value::as_str) != Some("file_search")),
        "Responses-native file_search must not be emitted to Chat: {upstream}"
    );
    assert!(
        tools.iter().all(|tool| {
            tool.get("function")
                .and_then(|function| function.get("name"))
                .and_then(Value::as_str)
                != Some("file_search")
        }),
        "filtered native tool identity must not be rewritten as a function: {upstream}"
    );
    assert!(
        upstream.get("tool_choice").is_none(),
        "tool_choice selecting a filtered descriptor must be omitted: {upstream}"
    );
}

#[tokio::test]
async fn responses_custom_format_tool_filtered_for_messages_upstream() {
    let ctx = setup().await;
    let (status, body) = json_post(
        &ctx,
        "/v1/responses",
        json!({
            "model": "gpt-5-mini-msg",
            "input": [{ "type": "message", "role": "user", "content": [{ "type": "input_text", "text": "custom to messages" }] }],
            "tools": [
                { "type": "function", "name": "lookup", "parameters": { "type": "object", "additionalProperties": true } },
                {
                    "type": "custom",
                    "name": "freeform_lookup",
                    "description": "OpenAI freeform custom tool",
                    "format": { "type": "grammar", "syntax": "lark", "definition": "start: /[a-z]+/" }
                }
            ],
            "tool_choice": { "type": "custom", "name": "freeform_lookup" }
        }),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{body}");

    let upstream = last_captured_body(&ctx, "messages");
    let tools = upstream["tools"]
        .as_array()
        .expect("messages upstream tools");
    assert_eq!(tools.len(), 1, "{upstream}");
    assert_eq!(tools[0]["name"], json!("lookup"));
    assert_eq!(tools[0]["input_schema"]["type"], json!("object"));
    assert!(
        tools.iter().all(|tool| {
            tool.get("name").and_then(Value::as_str) != Some("freeform_lookup")
                && tool.get("type").and_then(Value::as_str) != Some("custom")
                && tool.get("format").is_none()
        }),
        "Responses format-only custom tools must not be emitted to Messages: {upstream}"
    );
    assert!(
        upstream.get("tool_choice").is_none(),
        "tool_choice selecting a filtered custom descriptor must be omitted: {upstream}"
    );
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
