
#[tokio::test]
async fn responses_upstream_requests_include_encrypted_reasoning_content() {
    let ctx = setup().await;
    let (status, body) = json_post(
        &ctx,
        "/v1/responses",
        json!({
            "model": "gpt-5-mini",
            "input": "ping",
            "include": ["usage.input_tokens_details"]
        }),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{body}");

    let upstream = last_captured_body(&ctx, "responses");
    let include = upstream["include"].as_array().expect("include array");
    assert!(
        include
            .iter()
            .any(|value| value.as_str() == Some("usage.input_tokens_details"))
    );
    assert!(
        include
            .iter()
            .any(|value| value.as_str() == Some("reasoning.encrypted_content"))
    );
    assert_eq!(
        include
            .iter()
            .filter(|value| value.as_str() == Some("reasoning.encrypted_content"))
            .count(),
        1
    );
}

#[tokio::test]
async fn responses_reasoning_input_roundtrips_to_responses_upstream() {
    let ctx = setup().await;
    let (status, body) = json_post(
        &ctx,
        "/v1/responses",
        json!({
            "model": "gpt-5-mini",
            "input": [
                {
                    "type": "reasoning",
                    "id": "rs_original",
                    "status": "completed",
                    "summary": [{ "type": "summary_text", "text": "prior summary" }],
                    "encrypted_content": "prior_encrypted_content",
                    "source": "openai"
                },
                {
                    "type": "message",
                    "status": "completed",
                    "annotations": [],
                    "phase": "analysis",
                    "role": "assistant",
                    "content": [{ "type": "output_text", "text": "prior answer", "annotations": [], "logprobs": [], "phase": "analysis" }]
                },
                {
                    "type": "message",
                    "role": "user",
                    "content": [{ "type": "input_text", "text": "continue" }]
                }
            ]
        }),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{body}");

    let upstream = last_captured_body(&ctx, "responses");
    let input = upstream["input"].as_array().expect("responses input array");
    let reasoning_item = input
        .iter()
        .find(|item| item.get("type").and_then(|v| v.as_str()) == Some("reasoning"))
        .expect("responses upstream request should contain replayed reasoning");
    assert_eq!(reasoning_item["id"].as_str(), Some("rs_original"));
    assert_eq!(
        reasoning_item["encrypted_content"].as_str(),
        Some("prior_encrypted_content")
    );
    assert_eq!(
        reasoning_item["summary"],
        json!([{ "type": "summary_text", "text": "prior summary" }])
    );
    assert!(reasoning_item.get("text").is_none());
    assert!(reasoning_item.get("source").is_none());
    assert!(reasoning_item.get("status").is_none());
    let message_item = input
        .iter()
        .find(|item| {
            item.get("type").and_then(|v| v.as_str()) == Some("message")
                && item.get("role").and_then(|v| v.as_str()) == Some("assistant")
        })
        .expect("assistant message item");
    assert!(message_item.get("status").is_none());
    assert!(message_item.get("annotations").is_none());
    assert!(message_item.get("phase").is_none());
    assert!(message_item["content"][0].get("annotations").is_none());
    assert!(message_item["content"][0].get("logprobs").is_none());
    assert!(message_item["content"][0].get("phase").is_none());
}

#[tokio::test]
async fn responses_reasoning_envelope_roundtrips_for_same_model() {
    let ctx = setup().await;
    let (status, body) = json_post(
        &ctx,
        "/v1/responses",
        json!({
            "model": "gpt-5-mini",
            "input": "show reasoning",
            "reasoning": { "effort": "high" }
        }),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{body}");
    let first: Value = serde_json::from_str(&body).expect("first response json");
    let output = first["output"].as_array().expect("output array").clone();
    let encrypted = output
        .iter()
        .find(|item| item["type"].as_str() == Some("reasoning"))
        .and_then(|item| item["encrypted_content"].as_str())
        .expect("wrapped encrypted reasoning");
    assert!(
        encrypted.starts_with("mz2."),
        "downstream encrypted reasoning must be wrapped: {encrypted}"
    );

    let mut input = output;
    input.push(json!({
        "type": "message",
        "role": "user",
        "content": [{ "type": "input_text", "text": "continue" }]
    }));
    let (status, body) = json_post(
        &ctx,
        "/v1/responses",
        json!({
            "model": "gpt-5-mini",
            "input": input
        }),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{body}");

    let upstream = last_captured_body(&ctx, "responses");
    let input = upstream["input"].as_array().expect("upstream input array");
    let reasoning_item = input
        .iter()
        .find(|item| item["type"].as_str() == Some("reasoning"))
        .expect("same-model reasoning should be forwarded");
    assert_eq!(reasoning_item["id"].as_str(), Some("rs_mock"));
    assert_eq!(
        reasoning_item["encrypted_content"].as_str(),
        Some("mock_sig")
    );
}

#[tokio::test]
async fn responses_reasoning_envelope_drops_mismatched_model() {
    let ctx = setup().await;
    let (status, body) = json_post(
        &ctx,
        "/v1/responses",
        json!({
            "model": "gpt-5-mini",
            "input": "show reasoning",
            "reasoning": { "effort": "high" }
        }),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{body}");
    let first: Value = serde_json::from_str(&body).expect("first response json");
    let mut input = first["output"].as_array().expect("output array").clone();
    input.push(json!({
        "type": "message",
        "role": "user",
        "content": [{ "type": "input_text", "text": "continue on another model" }]
    }));

    let (status, body) = json_post(
        &ctx,
        "/v1/responses",
        json!({
            "model": "grok-4",
            "input": input
        }),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{body}");

    let upstream = last_captured_body(&ctx, "responses");
    let input = upstream["input"].as_array().expect("upstream input array");
    assert!(
        input
            .iter()
            .all(|item| item["type"].as_str() != Some("reasoning")),
        "mismatched wrapped reasoning must be removed before upstream: {upstream}"
    );
}

#[tokio::test]
async fn responses_reasoning_envelope_can_be_disabled_per_api_key() {
    let ctx = setup().await;
    let token = ctx
        .auth_header
        .strip_prefix("Bearer ")
        .expect("bearer token");
    let key = ctx
        .state
        .user_store
        .get_api_key_by_prefix(&token[..12])
        .await
        .expect("load key by prefix")
        .expect("api key exists");
    ctx.state
        .user_store
        .update_api_key(
            &key.id,
            monoize::users::UpdateApiKeyInput {
                name: None,
                enabled: None,
                sub_account_enabled: None,
                model_limits_enabled: None,
                model_limits: None,
                ip_whitelist: None,
                allowed_groups: None,
                max_multiplier: None,
                transforms: None,
                model_redirects: None,
                reasoning_envelope_enabled: Some(false),
                request_capture_mode: None,
                expires_at: None,
            },
            false,
        )
        .await
        .expect("disable reasoning envelope");

    let (status, body) = json_post(
        &ctx,
        "/v1/responses",
        json!({
            "model": "gpt-5-mini",
            "input": "show reasoning",
            "reasoning": { "effort": "high" }
        }),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{body}");
    let response: Value = serde_json::from_str(&body).expect("response json");
    let encrypted = response["output"]
        .as_array()
        .expect("output array")
        .iter()
        .find(|item| item["type"].as_str() == Some("reasoning"))
        .and_then(|item| item["encrypted_content"].as_str())
        .expect("encrypted reasoning");
    assert_eq!(encrypted, "mock_sig");
}

#[tokio::test]
async fn image_generations_collects_streamed_responses_image_output() {
    let ctx = setup().await;
    let (status, body) = json_post(
        &ctx,
        "/v1/images/generations",
        json!({
            "model": "gpt-5-mini",
            "prompt": "draw a cat",
            "stream_mode": "image_generation_completed"
        }),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{body}");
    let v: Value = serde_json::from_str(&body).unwrap();
    let data = v["data"].as_array().expect("data array");
    assert_eq!(data.len(), 1);
    assert_eq!(
        data[0]["b64_json"].as_str(),
        Some(
            "iVBORw0KGgoAAAANSUhEUgAAAAEAAAABCAQAAAC1HAwCAAAAC0lEQVR42mP8/x8AAusB9p4N2VwAAAAASUVORK5CYII="
        )
    );

    let upstream = last_captured_body(&ctx, "responses");
    assert_eq!(upstream["stream"].as_bool(), Some(true));
}

#[tokio::test]
async fn responses_nonstream_collects_completed_snapshot_image_generation_result() {
    let ctx = setup().await;
    let (upstream_addr, _, _) = start_upstream().await;
    let base_url = format!("http://{upstream_addr}");
    let mut models = HashMap::new();
    models.insert(
        "gpt-image-test".to_string(),
        monoize::monoize_routing::MonoizeModelEntry {
            redirect: Some("gpt-5-mini".to_string()),
            multiplier: 1.0,
        },
    );
    ctx.state
        .monoize_store
        .create_provider(monoize::monoize_routing::CreateMonoizeProviderInput {
            name: "fisx-style-image-test".to_string(),
            provider_type: monoize::monoize_routing::MonoizeProviderType::ChatCompletion,
            models,
            api_type_overrides: vec![monoize::monoize_routing::ApiTypeOverride {
                pattern: "gpt-image-test".to_string(),
                api_type: monoize::monoize_routing::MonoizeProviderType::Responses,
            }],
            groups: Vec::new(),
            channels: vec![monoize::monoize_routing::CreateMonoizeChannelInput {
                id: Some("fisx-style-image-test-ch".to_string()),
                name: "fisx-style-image-test-ch".to_string(),
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
            transforms: vec![monoize::transforms::TransformRuleConfig {
                transform: "enable_openai_image_generation_tool".to_string(),
                enabled: true,
                models: Some(vec!["gpt-image-test".to_string()]),
                phase: monoize::transforms::Phase::Request,
                config: json!({
                    "output_format": "webp",
                    "force_stream": true,
                    "force_tool_choice": true
                }),
            }],
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
        .expect("create fisx-style provider");

    let (status, body) = json_post(
        &ctx,
        "/v1/responses",
        json!({
            "model": "gpt-image-test",
            "input": "draw a cat",
            "stream_mode": "image_generation_completed_snapshot_only"
        }),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{body}");
    let v: Value = serde_json::from_str(&body).unwrap();
    let output = v["output"].as_array().expect("output array");
    let image = output
        .iter()
        .find_map(|item| item["content"].as_array())
        .and_then(|content| {
            content
                .iter()
                .find(|part| part["type"].as_str() == Some("output_image"))
        })
        .expect("output image from completed snapshot");
    assert_eq!(image["source"]["media_type"].as_str(), Some("image/webp"));
    assert!(
        image["source"]["data"]
            .as_str()
            .is_some_and(|data| !data.is_empty()),
        "{body}"
    );
}

#[tokio::test]
async fn responses_nonstream_dedupes_streamed_image_generation_item_against_completed_snapshot() {
    let ctx = setup().await;
    let (status, body) = json_post(
        &ctx,
        "/v1/responses",
        json!({
            "model": "gpt-5-mini",
            "input": "draw a cat",
            "tools": [{ "type": "image_generation", "output_format": "webp" }],
            "stream_mode": "image_generation_item_done_and_completed_snapshot"
        }),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{body}");
    let v: Value = serde_json::from_str(&body).unwrap();
    let output = v["output"].as_array().expect("output array");
    let image_count = output
        .iter()
        .flat_map(|item| item["content"].as_array().into_iter().flatten())
        .filter(|part| part["type"].as_str() == Some("output_image"))
        .count();
    let text_count = output
        .iter()
        .flat_map(|item| item["content"].as_array().into_iter().flatten())
        .filter(|part| part["type"].as_str() == Some("output_text"))
        .count();

    assert_eq!(image_count, 1, "{body}");
    assert_eq!(text_count, 0, "{body}");
}

#[tokio::test]
async fn image_edits_forwards_base64_images_as_data_urls_to_responses_upstream() {
    let ctx = setup().await;
    let boundary = "----monoize-edit-test";
    let png = base64::engine::general_purpose::STANDARD
        .decode("iVBORw0KGgoAAAANSUhEUgAAAAEAAAABCAQAAAC1HAwCAAAAC0lEQVR42mP8/x8AAusB9p4N2VwAAAAASUVORK5CYII=")
        .unwrap();
    let mut body = Vec::new();
    body.extend_from_slice(
        format!(
            "--{boundary}\r\nContent-Disposition: form-data; name=\"model\"\r\n\r\ngpt-5-mini\r\n"
        )
        .as_bytes(),
    );
    body.extend_from_slice(
        format!(
            "--{boundary}\r\nContent-Disposition: form-data; name=\"prompt\"\r\n\r\nTurn this into a blue version\r\n"
        )
        .as_bytes(),
    );
    body.extend_from_slice(
        format!(
            "--{boundary}\r\nContent-Disposition: form-data; name=\"image\"; filename=\"one.png\"\r\nContent-Type: image/png\r\n\r\n"
        )
        .as_bytes(),
    );
    body.extend_from_slice(&png);
    body.extend_from_slice(b"\r\n");
    body.extend_from_slice(format!("--{boundary}--\r\n").as_bytes());

    let req = Request::builder()
        .method("POST")
        .uri("/v1/images/edits")
        .header(
            CONTENT_TYPE,
            format!("multipart/form-data; boundary={boundary}"),
        )
        .header(AUTHORIZATION, ctx.auth_header.clone())
        .body(Body::from(body))
        .unwrap();

    let resp = ctx.router.clone().oneshot(req).await.unwrap();
    assert_eq!(resp.status(), StatusCode::BAD_GATEWAY);

    let upstream = last_captured_body(&ctx, "responses");
    let input = upstream["input"].as_array().expect("responses input array");
    let content = input[0]["content"].as_array().expect("content array");
    assert_eq!(content[0]["type"].as_str(), Some("input_text"));
    assert_eq!(content[1]["type"].as_str(), Some("input_image"));
    let image_url = content[1]["image_url"].as_str().expect("data url");
    assert!(
        image_url.starts_with("data:image/png;base64,"),
        "{image_url}"
    );
}

#[tokio::test]
async fn image_generations_to_openai_image_use_json_generations_endpoint() {
    let ctx = setup().await;
    let (upstream_addr, _, captured_bodies) = start_upstream().await;
    let base_url = format!("http://{upstream_addr}");
    create_test_provider(
        &ctx.state,
        "openai-image-generation-test",
        monoize::monoize_routing::MonoizeProviderType::OpenaiImage,
        "gpt-image-route-test",
        &base_url,
        "upstream-key",
    )
    .await;
    seed_test_model_pricing(&ctx.state, &["gpt-image-route-test"]).await;

    let (status, body) = json_post(
        &ctx,
        "/v1/images/generations",
        json!({
            "model": "gpt-image-route-test",
            "prompt": "draw a cat",
            "size": "1024x1024"
        }),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{body}");

    let upstream = captured_bodies
        .lock()
        .expect("captured bodies lock")
        .iter()
        .rev()
        .find(|(name, _)| name == "image_generations")
        .map(|(_, body)| body.clone())
        .expect("image generations upstream body");
    assert_eq!(upstream["model"].as_str(), Some("gpt-image-route-test"));
    assert_eq!(upstream["prompt"].as_str(), Some("draw a cat"));
    assert_eq!(upstream["size"].as_str(), Some("1024x1024"));
    assert!(
        captured_bodies
            .lock()
            .expect("captured bodies lock")
            .iter()
            .all(|(endpoint, _)| endpoint != "image_edits")
    );
}

#[tokio::test]
async fn image_edits_to_openai_image_use_multipart_edits_endpoint_with_image_bytes() {
    let ctx = setup().await;
    let (upstream_addr, _, captured_bodies) = start_upstream().await;
    let base_url = format!("http://{upstream_addr}");
    create_test_provider(
        &ctx.state,
        "openai-image-edit-test",
        monoize::monoize_routing::MonoizeProviderType::OpenaiImage,
        "gpt-image-edit-route-test",
        &base_url,
        "upstream-key",
    )
    .await;
    seed_test_model_pricing(&ctx.state, &["gpt-image-edit-route-test"]).await;

    let boundary = "----monoize-openai-image-edit-test";
    let image_b64 = "iVBORw0KGgoAAAANSUhEUgAAAAEAAAABCAQAAAC1HAwCAAAAC0lEQVR42mP8/x8AAusB9p4N2VwAAAAASUVORK5CYII=";
    let mask_b64 = "iVBORw0KGgoAAAANSUhEUgAAAAEAAAABCAQAAAC1HAwCAAAAC0lEQVR42mP8/x8AAusB9p4N2VwAAAAASUVORK5CYII=";
    let image = base64::engine::general_purpose::STANDARD
        .decode(image_b64)
        .unwrap();
    let mask = base64::engine::general_purpose::STANDARD
        .decode(mask_b64)
        .unwrap();
    let mut req_body = Vec::new();
    req_body.extend_from_slice(format!("--{boundary}\r\nContent-Disposition: form-data; name=\"model\"\r\n\r\ngpt-image-edit-route-test\r\n").as_bytes());
    req_body.extend_from_slice(format!("--{boundary}\r\nContent-Disposition: form-data; name=\"prompt\"\r\n\r\nedit this image\r\n").as_bytes());
    req_body.extend_from_slice(format!("--{boundary}\r\nContent-Disposition: form-data; name=\"size\"\r\n\r\n1024x1024\r\n").as_bytes());
    req_body.extend_from_slice(format!("--{boundary}\r\nContent-Disposition: form-data; name=\"image\"; filename=\"one.png\"\r\nContent-Type: image/png\r\n\r\n").as_bytes());
    req_body.extend_from_slice(&image);
    req_body.extend_from_slice(b"\r\n");
    req_body.extend_from_slice(format!("--{boundary}\r\nContent-Disposition: form-data; name=\"mask\"; filename=\"mask.png\"\r\nContent-Type: image/png\r\n\r\n").as_bytes());
    req_body.extend_from_slice(&mask);
    req_body.extend_from_slice(b"\r\n");
    req_body.extend_from_slice(format!("--{boundary}--\r\n").as_bytes());

    let req = Request::builder()
        .method("POST")
        .uri("/v1/images/edits")
        .header(
            CONTENT_TYPE,
            format!("multipart/form-data; boundary={boundary}"),
        )
        .header(AUTHORIZATION, ctx.auth_header.clone())
        .body(Body::from(req_body))
        .unwrap();

    let resp = ctx.router.clone().oneshot(req).await.unwrap();
    assert_eq!(resp.status(), StatusCode::OK);

    let upstream = captured_bodies
        .lock()
        .expect("captured bodies lock")
        .iter()
        .rev()
        .find(|(name, _)| name == "image_edits")
        .map(|(_, body)| body.clone())
        .expect("image edits upstream body");
    assert_eq!(upstream["model"].as_str(), Some("gpt-image-edit-route-test"));
    assert_eq!(upstream["prompt"].as_str(), Some("edit this image"));
    assert_eq!(upstream["size"].as_str(), Some("1024x1024"));
    assert_eq!(upstream["images"][0]["content_type"].as_str(), Some("image/png"), "{upstream}");
    assert_eq!(upstream["images"][0]["b64"].as_str(), Some(image_b64), "{upstream}");
    assert_eq!(upstream["masks"][0]["content_type"].as_str(), Some("image/png"), "{upstream}");
    assert_eq!(upstream["masks"][0]["b64"].as_str(), Some(mask_b64), "{upstream}");
    assert!(
        captured_bodies
            .lock()
            .expect("captured bodies lock")
            .iter()
            .all(|(endpoint, body)| endpoint != "image_generations"
                || body["model"].as_str() != Some("gpt-image-edit-route-test"))
    );
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
