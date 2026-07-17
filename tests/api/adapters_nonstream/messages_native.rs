#[tokio::test]
async fn messages_message_envelope_extra_stays_out_of_content_block() {
    let ctx = setup().await;
    let (status, body) = json_post(
        &ctx,
        "/v1/messages",
        json!({
            "model": "gpt-5-mini-msg",
            "max_tokens": 64,
            "messages": [{
                "role": "user",
                "vendor_message": { "trace_id": "trace_1" },
                "content": [{
                    "type": "text",
                    "text": "hello",
                    "cache_control": { "type": "ephemeral" },
                    "citations": [{ "type": "page", "page": 1 }],
                    "caller": { "type": "direct" }
                }]
            }]
        }),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{body}");

    let upstream = last_captured_body(&ctx, "messages");
    let message = &upstream["messages"][0];
    assert_eq!(message["vendor_message"], json!({ "trace_id": "trace_1" }));
    let block = &message["content"][0];
    assert!(
        block.get("vendor_message").is_none(),
        "message envelope fields must not enter content blocks: {upstream}"
    );
    assert_eq!(block["cache_control"], json!({ "type": "ephemeral" }));
    assert_eq!(block["citations"], json!([{ "type": "page", "page": 1 }]));
    assert_eq!(block["caller"], json!({ "type": "direct" }));
}

#[tokio::test]
async fn messages_unknown_system_block_round_trips_same_family() {
    let ctx = setup().await;
    let (status, body) = json_post(
        &ctx,
        "/v1/messages",
        json!({
            "model": "gpt-5-mini-msg",
            "max_tokens": 64,
            "system": [
                { "type": "text", "text": "system text" },
                {
                    "type": "future_system_block",
                    "payload": { "version": 2, "keep": true }
                }
            ],
            "messages": [{ "role": "user", "content": "hello" }]
        }),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{body}");

    let upstream = last_captured_body(&ctx, "messages");
    assert_eq!(
        upstream["system"],
        json!([
            { "type": "text", "text": "system text" },
            {
                "type": "future_system_block",
                "payload": { "version": 2, "keep": true }
            }
        ])
    );
}

#[tokio::test]
async fn messages_nonstream_error_uses_anthropic_envelope_and_request_id() {
    let ctx = setup().await;
    let req = Request::builder()
        .method("POST")
        .uri("/v1/messages")
        .header(CONTENT_TYPE, "application/json")
        .header(AUTHORIZATION, ctx.auth_header.clone())
        .header("x-request-id", "req_messages_error")
        .body(Body::from(
            json!({
                "model": "gpt-5-mini-msg",
                "max_tokens": 64,
                "messages": [{ "role": "user", "content": "fail" }],
                "force_upstream_error_status": 422,
                "force_upstream_error_code": "invalid_request_error",
                "force_upstream_error_message": "invalid request"
            })
            .to_string(),
        ))
        .unwrap();

    let resp = ctx.router.clone().oneshot(req).await.unwrap();
    assert_eq!(resp.status(), StatusCode::UNPROCESSABLE_ENTITY);
    assert_eq!(
        resp.headers()
            .get("request-id")
            .and_then(|value| value.to_str().ok()),
        Some("req_messages_error")
    );
    let body = resp.into_body().collect().await.unwrap().to_bytes();
    let body: Value = serde_json::from_slice(&body).expect("Anthropic error JSON");
    assert_eq!(body["type"], json!("error"));
    assert_eq!(body["error"]["type"], json!("invalid_request_error"));
    assert_eq!(
        body["error"]["message"],
        json!("upstream status 422 Unprocessable Entity: invalid request")
    );
    assert_eq!(
        body["error"]["upstream_code"],
        json!("invalid_request_error")
    );
    assert_eq!(body["request_id"], json!("req_messages_error"));
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
            "messages": [
              {
                "role": "assistant",
                "content": [
                  { "type": "tool_use", "id": "call_multipart", "name": "tool_multipart", "input": {} }
                ]
              },
              {
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
              }
            ]
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
async fn messages_parallel_tool_results_are_encoded_in_one_user_message_for_messages_upstream() {
    let ctx = setup().await;
    let (status, body) = json_post(
        &ctx,
        "/v1/messages",
        json!({
            "model": "gpt-5-mini-msg",
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
    assert!(text.contains("tool_ok:R1|R2"));

    let upstream = last_captured_body(&ctx, "messages");
    let messages = upstream["messages"].as_array().expect("messages array");
    assert_eq!(messages.len(), 2, "unexpected messages shape: {upstream}");
    assert_eq!(messages[0]["role"].as_str(), Some("assistant"));
    assert_eq!(messages[1]["role"].as_str(), Some("user"));
    let tool_results = messages[1]["content"].as_array().expect("content array");
    assert_eq!(
        tool_results
            .iter()
            .filter(|block| block.get("type").and_then(|v| v.as_str()) == Some("tool_result"))
            .count(),
        2,
        "parallel tool results must share one user message: {upstream}"
    );
    assert_eq!(
        tool_results[0]["tool_use_id"].as_str(),
        Some("call_1"),
        "tool_result order must follow input order: {upstream}"
    );
    assert_eq!(tool_results[1]["tool_use_id"].as_str(), Some("call_2"));
}

#[tokio::test]
async fn messages_assistant_empty_text_before_tool_use_is_not_sent_to_messages_upstream() {
    let ctx = setup().await;
    let (status, _body) = json_post(
        &ctx,
        "/v1/messages",
        json!({
            "model": "gpt-5-mini-msg",
            "max_tokens": 64,
            "messages": [{
                "role": "assistant",
                "content": [
                    { "type": "text", "text": "" },
                    { "type": "tool_use", "id": "call_1", "name": "tool_a", "input": {} }
                ]
            }, {
                "role": "user",
                "content": [
                    { "type": "tool_result", "tool_use_id": "call_1", "content": "R1" }
                ]
            }]
        }),
    )
    .await;
    assert_eq!(status, StatusCode::OK);

    let upstream = last_captured_body(&ctx, "messages");
    let messages = upstream["messages"].as_array().expect("messages array");
    assert_eq!(
        messages.len(),
        2,
        "assistant tool use and user tool result should both remain: {upstream}"
    );
    let content = messages[0]["content"].as_array().expect("content array");
    assert_eq!(
        content.len(),
        1,
        "empty assistant text block must not be forwarded: {upstream}"
    );
    assert_eq!(content[0]["type"].as_str(), Some("tool_use"));
    assert_eq!(content[0]["id"].as_str(), Some("call_1"));
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
async fn messages_upstream_forwards_explicit_max_tokens_unchanged() {
    let ctx = setup().await;
    let (status, _body) = json_post(
        &ctx,
        "/v1/messages",
        json!({
            "model": "gpt-5-mini-msg",
            "max_tokens": 1234,
            "messages": [{ "role": "user", "content": [{ "type": "text", "text": "yo" }] }]
        }),
    )
    .await;
    assert_eq!(status, StatusCode::OK);

    let upstream = last_captured_body(&ctx, "messages");
    assert_eq!(upstream["max_tokens"], json!(1234));
}

#[tokio::test]
async fn messages_request_forwards_ordinary_base64_image_to_messages_upstream() {
    let ctx = setup().await;
    let data = base64::engine::general_purpose::STANDARD.encode(b"tiny-image");
    let (status, _body) = json_post(
        &ctx,
        "/v1/messages",
        json!({
            "model": "gpt-5-mini-msg",
            "max_tokens": 64,
            "messages": [{
                "role": "user",
                "content": [
                    { "type": "text", "text": "describe" },
                    {
                        "type": "image",
                        "source": {
                            "type": "base64",
                            "media_type": "image/png",
                            "data": data
                        }
                    }
                ]
            }]
        }),
    )
    .await;
    assert_eq!(status, StatusCode::OK);

    let upstream = last_captured_body(&ctx, "messages");
    let content = upstream["messages"][0]["content"]
        .as_array()
        .expect("message content array");
    assert_eq!(content[0], json!({ "type": "text", "text": "describe" }));
    assert_eq!(
        content[1],
        json!({
            "type": "image",
            "source": {
                "type": "base64",
                "media_type": "image/png",
                "data": data
            }
        })
    );
}

#[tokio::test]
async fn messages_request_forwards_ordinary_url_image_to_messages_upstream() {
    let ctx = setup().await;
    let image_url = "https://example.com/input.png";
    let (status, _body) = json_post(
        &ctx,
        "/v1/messages",
        json!({
            "model": "gpt-5-mini-msg",
            "max_tokens": 64,
            "messages": [{
                "role": "user",
                "content": [
                    { "type": "text", "text": "describe" },
                    { "type": "image", "source": { "type": "url", "url": image_url } }
                ]
            }]
        }),
    )
    .await;
    assert_eq!(status, StatusCode::OK);

    let upstream = last_captured_body(&ctx, "messages");
    let content = upstream["messages"][0]["content"]
        .as_array()
        .expect("message content array");
    assert_eq!(content[0], json!({ "type": "text", "text": "describe" }));
    assert_eq!(
        content[1],
        json!({ "type": "image", "source": { "type": "url", "url": image_url } })
    );
}

#[tokio::test]
async fn messages_file_id_sources_forward_with_files_beta_header() {
    let ctx = setup().await;
    let (status, _) = json_post(
        &ctx,
        "/v1/messages",
        json!({
            "model": "gpt-5-mini-msg",
            "max_tokens": 64,
            "messages": [{
                "role": "user",
                "content": [
                    { "type": "text", "text": "inspect files" },
                    {
                        "type": "image",
                        "source": { "type": "file", "file_id": "file_image_1" }
                    },
                    {
                        "type": "document",
                        "source": { "type": "file", "file_id": "file_document_1" },
                        "title": "Reference"
                    },
                    { "type": "container_upload", "file_id": "file_dataset_1" }
                ]
            }]
        }),
    )
    .await;
    assert_eq!(status, StatusCode::OK);

    let upstream = ctx
        .captured_bodies
        .lock()
        .expect("captured bodies lock")
        .iter()
        .rev()
        .find(|(kind, _)| kind == "messages")
        .map(|(_, body)| body.clone())
        .expect("messages upstream body");
    assert_eq!(
        upstream["messages"][0]["content"],
        json!([
            { "type": "text", "text": "inspect files" },
            {
                "type": "image",
                "source": { "type": "file", "file_id": "file_image_1" }
            },
            {
                "type": "document",
                "source": { "type": "file", "file_id": "file_document_1" },
                "title": "Reference"
            },
            { "type": "container_upload", "file_id": "file_dataset_1" }
        ])
    );
    assert!(
        ctx.captured_headers
            .lock()
            .expect("captured headers lock")
            .iter()
            .any(|(key, value)| { key == "anthropic-beta" && value == "files-api-2025-04-14" }),
        "expected the Files API beta header"
    );
}

#[tokio::test]
async fn messages_tool_result_file_ids_round_trip_with_files_beta_header() {
    let ctx = setup().await;
    let (status, body) = json_post(
        &ctx,
        "/v1/messages",
        json!({
            "model": "gpt-5-mini-msg",
            "max_tokens": 64,
            "messages": [
                {
                    "role": "assistant",
                    "content": [{
                        "type": "tool_use",
                        "id": "call_files",
                        "name": "inspect",
                        "input": {}
                    }]
                },
                {
                    "role": "user",
                    "content": [{
                        "type": "tool_result",
                        "tool_use_id": "call_files",
                        "content": [
                            {
                                "type": "image",
                                "source": { "type": "file", "file_id": "file_image_result" }
                            },
                            {
                                "type": "document",
                                "source": { "type": "file", "file_id": "file_document_result" },
                                "title": "Tool result"
                            }
                        ]
                    }]
                }
            ]
        }),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{body}");

    let upstream = last_captured_body(&ctx, "messages");
    assert_eq!(
        upstream["messages"][1]["content"][0]["content"],
        json!([
            {
                "type": "image",
                "source": { "type": "file", "file_id": "file_image_result" }
            },
            {
                "type": "document",
                "source": { "type": "file", "file_id": "file_document_result" },
                "title": "Tool result"
            }
        ])
    );
    assert!(
        ctx.captured_headers
            .lock()
            .expect("captured headers lock")
            .iter()
            .any(|(key, value)| key == "anthropic-beta" && value == "files-api-2025-04-14"),
        "expected Files API beta header for tool_result file IDs"
    );
}

#[tokio::test]
async fn provider_scoped_file_ids_are_not_cross_family_translated() {
    let to_messages = setup().await;
    let (status, _) = json_post(
        &to_messages,
        "/v1/responses",
        json!({
            "model": "gpt-5-mini-msg",
            "input": [{
                "type": "message",
                "role": "user",
                "content": [
                    { "type": "input_text", "text": "inspect" },
                    { "type": "input_file", "file_id": "file_openai_1" }
                ]
            }]
        }),
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    let messages_upstream = last_captured_body(&to_messages, "messages");
    assert!(
        !messages_upstream.to_string().contains("file_openai_1"),
        "OpenAI file IDs must not be emitted as Anthropic Files IDs"
    );
    assert!(
        !to_messages
            .captured_headers
            .lock()
            .expect("captured headers lock")
            .iter()
            .any(|(key, _)| key == "anthropic-beta"),
        "an omitted OpenAI file ID must not enable the Anthropic Files beta"
    );

    let to_responses = setup().await;
    let (status, _) = json_post(
        &to_responses,
        "/v1/messages",
        json!({
            "model": "gpt-5-mini",
            "max_tokens": 64,
            "messages": [{
                "role": "user",
                "content": [
                    { "type": "text", "text": "inspect" },
                    {
                        "type": "document",
                        "source": { "type": "file", "file_id": "file_anthropic_1" }
                    }
                ]
            }]
        }),
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    let responses_upstream = last_captured_body(&to_responses, "responses");
    assert!(
        !responses_upstream.to_string().contains("file_anthropic_1"),
        "Anthropic file IDs must not be emitted as OpenAI Files IDs"
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
            models,
            api_type_overrides: Vec::new(),
            groups: Vec::new(),
            channels: vec![monoize::monoize_routing::CreateMonoizeChannelInput {
                id: Some("mono-transform-markdown-images-ch1".to_string()),
                name: "mono-transform-markdown-images-ch1".to_string(),
                provider_type: monoize::monoize_routing::MonoizeProviderType::Responses,
                base_url,
                api_key: Some("upstream-key".to_string()),
                weight: 1,
                enabled: true,
                passive_failure_count_threshold_override: None,
                passive_cooldown_seconds_override: None,
                passive_window_seconds_override: None,
                passive_rate_limit_cooldown_seconds_override: None,
                supported_models: vec!["gpt-5-mini".to_string()],
                active_probe_enabled_override: None,
                active_probe_interval_seconds_override: None,
                active_probe_success_threshold_override: None,
                active_probe_model_override: None,
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
