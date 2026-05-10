
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
