
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

#[tokio::test]
async fn responses_compact_is_native_same_protocol_passthrough() {
    let ctx = setup().await;
    let input = json!([
        { "role": "user", "content": "build a site", "_monoize_drop": true },
        {
            "id": "prog_compact_1",
            "type": "program",
            "call_id": "program_compact_call_1",
            "code": "return 1",
            "fingerprint": "fp_compact"
        },
        {
            "id": "msg_compact_1",
            "type": "message",
            "status": "completed",
            "role": "assistant",
            "content": [{ "type": "output_text", "text": "done" }]
        }
    ]);
    let (status, body) = json_post(
        &ctx,
        "/v1/responses/compact",
        json!({
            "model": "gpt-5-mini",
            "input": input.clone(),
            "max_multiplier": 2.0,
            "future_compact_control": { "preserve": true },
            "_monoize_top": "drop"
        }),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{body}");

    let response: Value = serde_json::from_str(&body).expect("compact response JSON");
    assert_eq!(response["object"], json!("response.compaction"));
    assert_eq!(response["id"], json!("resp_compact_mock"));
    assert_eq!(response["output"][1]["type"], json!("compaction"));
    assert_eq!(
        response["output"][1]["encrypted_content"],
        json!("opaque_compaction_payload")
    );
    assert_eq!(
        response["output"][1]["vendor_compaction"],
        json!({ "preserve": true })
    );
    assert_eq!(response["usage"]["total_tokens"], json!(577));
    assert_eq!(
        response["vendor_response"],
        json!({ "preserve": true })
    );

    let upstream = last_captured_body(&ctx, "responses_compact");
    assert_eq!(upstream["model"], json!("gpt-5-mini"));
    assert_eq!(upstream["input"][1], input[1]);
    assert_eq!(upstream["input"][2], input[2]);
    assert!(upstream["input"][0].get("_monoize_drop").is_none());
    assert!(upstream.get("max_multiplier").is_none());
    assert!(upstream.get("_monoize_top").is_none());
    assert_eq!(
        upstream["future_compact_control"],
        json!({ "preserve": true })
    );
}

#[tokio::test]
async fn responses_compact_rejects_streaming_before_upstream_dispatch() {
    let ctx = setup().await;
    let before = ctx
        .captured_bodies
        .lock()
        .expect("captured bodies lock")
        .iter()
        .filter(|(name, _)| name == "responses_compact")
        .count();
    let (status, body) = json_post(
        &ctx,
        "/v1/responses/compact",
        json!({
            "model": "gpt-5-mini",
            "input": [{ "role": "user", "content": "compact" }],
            "stream": true
        }),
    )
    .await;
    assert_eq!(status, StatusCode::BAD_REQUEST, "{body}");
    let error: Value = serde_json::from_str(&body).expect("compact error JSON");
    assert_eq!(error["error"]["code"], json!("invalid_request"));
    let after = ctx
        .captured_bodies
        .lock()
        .expect("captured bodies lock")
        .iter()
        .filter(|(name, _)| name == "responses_compact")
        .count();
    assert_eq!(after, before, "invalid compact request reached upstream");
}

#[tokio::test]
async fn responses_programmatic_tool_calling_round_trips_and_stateful_result_survives() {
    let ctx = setup().await;
    let (status, body) = json_post(
        &ctx,
        "/v1/responses",
        json!({
            "model": "gpt-5-mini",
            "input": "use programmatic tools",
            "tools": [
                { "type": "programmatic_tool_calling" },
                {
                    "type": "function",
                    "name": "lookup",
                    "description": "Lookup data",
                    "parameters": {
                        "type": "object",
                        "properties": { "query": { "type": "string" } },
                        "required": ["query"]
                    },
                    "allowed_callers": ["programmatic"],
                    "output_schema": {
                        "type": "object",
                        "properties": { "answer": { "type": "string" } },
                        "required": ["answer"]
                    },
                    "defer_loading": true
                }
            ],
            "native_response_mode": "responses_ptc"
        }),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{body}");
    let response: Value = serde_json::from_str(&body).expect("PTC response JSON");
    let output = response["output"].as_array().expect("PTC output");
    assert_eq!(
        output.iter().map(|item| item["type"].as_str().unwrap()).collect::<Vec<_>>(),
        vec!["program", "function_call", "program_output"]
    );
    assert_eq!(
        output[1]["caller"],
        json!({ "type": "programmatic", "caller_id": "prog_1" })
    );
    assert_eq!(output[0]["fingerprint"], json!("fp_ptc_1"));

    let upstream = last_captured_body(&ctx, "responses");
    let tools = upstream["tools"].as_array().expect("PTC tools");
    assert_eq!(tools[0]["type"], json!("programmatic_tool_calling"));
    assert_eq!(tools[1]["allowed_callers"], json!(["programmatic"]));
    assert_eq!(tools[1]["output_schema"]["type"], json!("object"));
    assert_eq!(tools[1]["defer_loading"], json!(true));
    assert!(
        ctx.state
            .channel_affinity
            .lock()
            .await
            .keys()
            .any(|key| key.ends_with("previous_response_id:resp_ptc")),
        "successful Responses ids must be affinity keys"
    );

    let caller = json!({ "type": "programmatic", "caller_id": "prog_1" });
    let (status, body) = json_post(
        &ctx,
        "/v1/responses",
        json!({
            "model": "gpt-5-mini",
            "previous_response_id": "resp_ptc",
            "conversation": "conv_ptc_1",
            "store": true,
            "input": [{
                "type": "function_call_output",
                "call_id": "call_ptc_1",
                "output": "{\"answer\":\"ok\"}",
                "caller": caller.clone()
            }]
        }),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{body}");
    let continuation = last_captured_body(&ctx, "responses");
    assert_eq!(continuation["previous_response_id"], json!("resp_ptc"));
    assert_eq!(continuation["conversation"], json!("conv_ptc_1"));
    assert_eq!(continuation["store"], json!(true));
    let result = continuation["input"]
        .as_array()
        .and_then(|items| {
            items
                .iter()
                .find(|item| item["type"] == json!("function_call_output"))
        })
        .expect("stateful function result must survive");
    assert_eq!(result["call_id"], json!("call_ptc_1"));
    assert_eq!(result["caller"], caller);
}

#[tokio::test]
async fn responses_tool_search_lifecycle_round_trips_same_family() {
    let ctx = setup().await;
    let native_input = json!([
        {
            "type": "tool_search_call",
            "id": "tsc_input_1",
            "call_id": "tool_search_input_call_1",
            "arguments": { "query": "lookup docs" }
        },
        {
            "type": "tool_search_output",
            "id": "tso_input_1",
            "call_id": "tool_search_input_call_1",
            "tools": [{ "type": "function", "name": "lookup_docs" }]
        },
        {
            "type": "additional_tools",
            "id": "at_input_1",
            "tools": [{ "type": "function", "name": "lookup_docs" }]
        }
    ]);
    let (status, body) = json_post(
        &ctx,
        "/v1/responses",
        json!({
            "model": "gpt-5-mini",
            "input": native_input.clone(),
            "tools": [
                {
                    "type": "function",
                    "name": "lookup_docs",
                    "parameters": { "type": "object", "properties": {} },
                    "defer_loading": true
                },
                {
                    "type": "mcp",
                    "server_label": "docs",
                    "server_url": "https://mcp.example.test",
                    "defer_loading": true
                },
                {
                    "type": "namespace",
                    "name": "docs_namespace",
                    "tools": [{ "name": "lookup_docs" }]
                },
                {
                    "type": "tool_search",
                    "description": "Search deferred tools",
                    "execution": "server"
                }
            ],
            "native_response_mode": "responses_tool_search"
        }),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{body}");

    let upstream = last_captured_body(&ctx, "responses");
    assert_eq!(upstream["input"], native_input);
    assert_eq!(upstream["tools"][0]["defer_loading"], json!(true));
    assert_eq!(upstream["tools"][1]["defer_loading"], json!(true));
    assert_eq!(upstream["tools"][2]["type"], json!("namespace"));
    assert_eq!(upstream["tools"][3]["type"], json!("tool_search"));

    let response: Value = serde_json::from_str(&body).expect("tool search response JSON");
    let output = response["output"].as_array().expect("tool search output");
    assert_eq!(
        output.iter().map(|item| item["type"].as_str().unwrap()).collect::<Vec<_>>(),
        vec!["tool_search_call", "tool_search_output", "additional_tools"]
    );
    assert_eq!(output[1]["tools"][0]["name"], json!("lookup_docs"));
}

#[tokio::test]
async fn responses_context_management_and_compaction_item_round_trip_same_family() {
    let ctx = setup().await;
    let context_management = json!([{
        "type": "compaction",
        "compact_threshold": 120000,
        "vendor_policy": { "preserve": true }
    }]);
    let compaction_input = json!({
        "type": "compaction",
        "id": "cmp_input_1",
        "encrypted_content": "opaque_input_compaction",
        "vendor_compaction": { "preserve": true }
    });
    let (status, body) = json_post(
        &ctx,
        "/v1/responses",
        json!({
            "model": "gpt-5-mini",
            "input": [
                { "role": "user", "content": "continue after compaction" },
                compaction_input.clone()
            ],
            "context_management": context_management.clone(),
            "native_response_mode": "responses_compaction_item"
        }),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{body}");

    let upstream = last_captured_body(&ctx, "responses");
    assert_eq!(upstream["context_management"], context_management);
    assert_eq!(upstream["input"][1], compaction_input);

    let response: Value = serde_json::from_str(&body).expect("compaction response JSON");
    assert_eq!(response["output"][0]["type"], json!("compaction"));
    assert_eq!(
        response["output"][0]["encrypted_content"],
        json!("opaque_response_compaction")
    );
    assert_eq!(
        response["output"][0]["vendor_compaction"],
        json!({ "preserve": true })
    );
    assert_eq!(response["output"][1]["type"], json!("message"));
}
