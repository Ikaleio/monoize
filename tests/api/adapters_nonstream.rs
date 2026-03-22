use super::*;

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
    ctx.state
        .monoize_store
        .create_provider(monoize::monoize_routing::CreateMonoizeProviderInput {
            name: "mono-transform-markdown-images".to_string(),
            provider_type: monoize::monoize_routing::MonoizeProviderType::Responses,
            models,
            api_type_overrides: Vec::new(),
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
