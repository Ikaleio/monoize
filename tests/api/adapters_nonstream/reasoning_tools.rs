
#[tokio::test]
async fn responses_nonstream_preserves_upstream_service_tier() {
    let ctx = setup().await;
    let (status, body) = json_post(
        &ctx,
        "/v1/responses",
        json!({
            "model": "gpt-5-mini",
            "input": [{ "type": "message", "role": "user", "content": [{ "type": "input_text", "text": "ping" }] }],
            "service_tier": "priority"
        }),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{body}");
    let v: Value = serde_json::from_str(&body).unwrap();
    assert_eq!(v["service_tier"].as_str(), Some("priority"));
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
    assert!(
        v["choices"][0]["message"]["reasoning_details"][1]["data"]
            .as_str()
            .is_some_and(|data| data.starts_with("mz2."))
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
    assert!(
        v["choices"][0]["message"]["reasoning_details"][1]["data"]
            .as_str()
            .is_some_and(|data| data.starts_with("mz2."))
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
            && detail["data"]
                .as_str()
                .is_some_and(|data| data.starts_with("mz2."))
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
    let thinking_blocks = blocks
        .iter()
        .filter(|block| block["type"].as_str() == Some("thinking"))
        .collect::<Vec<_>>();
    assert_eq!(thinking_blocks.len(), 1);
    assert_eq!(thinking_blocks[0]["thinking"], json!("mock_reasoning"));
    assert!(
        thinking_blocks[0]["signature"]
            .as_str()
            .is_some_and(|signature| !signature.is_empty()),
        "plaintext and encrypted Chat details must share one Messages thinking block"
    );
}

#[tokio::test]
async fn chat_reasoning_detail_pair_merges_for_messages_upstream_replay() {
    let ctx = setup().await;
    let (status, _) = json_post(
        &ctx,
        "/v1/chat/completions",
        json!({
            "model": "gpt-5-mini-msg",
            "messages": [
                { "role": "user", "content": "start" },
                {
                    "role": "assistant",
                    "content": "",
                    "reasoning_details": [
                        { "type": "reasoning.text", "text": "plain think", "format": "openrouter" },
                        { "type": "reasoning.encrypted", "data": "sig_1", "format": "openrouter" }
                    ]
                },
                { "role": "user", "content": "continue" }
            ]
        }),
    )
    .await;
    assert_eq!(status, StatusCode::OK);

    let upstream = last_captured_body(&ctx, "messages");
    let assistant = upstream["messages"]
        .as_array()
        .expect("messages array")
        .iter()
        .find(|message| message["role"].as_str() == Some("assistant"))
        .expect("assistant history");
    assert_eq!(
        assistant["content"],
        json!([{
            "type": "thinking",
            "thinking": "plain think",
            "signature": "sig_1"
        }])
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

    let upstream = last_captured_body(&ctx, "chat");
    assert_eq!(upstream["parallel_tool_calls"], json!(true));
    let tools = upstream["tools"].as_array().expect("chat upstream tools");
    assert!(
        tools
            .iter()
            .all(|tool| tool.get("parallel_tool_calls").is_none()),
        "parallel_tool_calls must remain a top-level request field: {upstream}"
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
    assert!(
        v["choices"][0]["message"]["reasoning_details"][1]["data"]
            .as_str()
            .is_some_and(|data| data.starts_with("mz2."))
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
            "messages": [
              {
                "role": "assistant",
                "content": [
                  { "type": "tool_use", "id": "call_1", "name": "tool_a", "input": {} },
                  { "type": "tool_use", "id": "call_2", "name": "tool_b", "input": {} }
                ]
              },
              {
                "role": "user",
                "content": [
                  { "type": "tool_result", "tool_use_id": "call_1", "content": "R1" },
                  { "type": "tool_result", "tool_use_id": "call_2", "content": "R2" }
                ]
              }
            ]
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

    let upstream = last_captured_body(&ctx, "responses");
    let input = upstream["input"].as_array().expect("responses input array");
    let input_types: Vec<&str> = input
        .iter()
        .filter_map(|item| item.get("type").and_then(|v| v.as_str()))
        .collect();
    assert_eq!(
        input_types,
        vec![
            "function_call",
            "function_call",
            "function_call_output",
            "function_call_output"
        ],
        "messages->responses tool replay must stay self-contained and ordered: {upstream}"
    );
}

#[tokio::test]
async fn messages_plaintext_thinking_is_not_replayed_to_responses_upstream_during_tool_loop() {
    let ctx = setup().await;
    let (status, _body) = json_post(
        &ctx,
        "/v1/messages",
        json!({
            "model": "gpt-5-mini",
            "max_tokens": 64,
            "messages": [
                {
                    "role": "assistant",
                    "content": [
                        { "type": "thinking", "thinking": "plain transformed summary" },
                        { "type": "tool_use", "id": "call_1", "name": "tool_a", "input": {} },
                        { "type": "tool_use", "id": "call_2", "name": "tool_b", "input": {} }
                    ]
                },
                {
                    "role": "user",
                    "content": [
                        { "type": "tool_result", "tool_use_id": "call_1", "content": "R1" },
                        { "type": "tool_result", "tool_use_id": "call_2", "content": "R2" }
                    ]
                }
            ]
        }),
    )
    .await;
    assert_eq!(status, StatusCode::OK);

    let upstream = last_captured_body(&ctx, "responses");
    let input = upstream["input"].as_array().expect("responses input array");
    let input_types: Vec<&str> = input
        .iter()
        .filter_map(|item| item.get("type").and_then(|v| v.as_str()))
        .collect();
    assert_eq!(
        input_types,
        vec![
            "function_call",
            "function_call",
            "function_call_output",
            "function_call_output"
        ],
        "plaintext-only thinking must be dropped before responses tool replay: {upstream}"
    );
}
