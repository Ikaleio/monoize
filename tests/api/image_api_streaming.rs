use super::*;

const TEST_PNG_B64: &str = "iVBORw0KGgoAAAANSUhEUgAAAAEAAAABCAQAAAC1HAwCAAAAC0lEQVR42mP8/x8AAusB9p4N2VwAAAAASUVORK5CYII=";

async fn sse_post_json(
    ctx: &TestContext,
    path: &str,
    body: Value,
) -> (StatusCode, Option<String>, String) {
    let req = Request::builder()
        .method("POST")
        .uri(path)
        .header(CONTENT_TYPE, "application/json")
        .header(AUTHORIZATION, ctx.auth_header.clone())
        .body(Body::from(body.to_string()))
        .unwrap();
    let resp = ctx.router.clone().oneshot(req).await.unwrap();
    let status = resp.status();
    let content_type = resp
        .headers()
        .get(CONTENT_TYPE)
        .and_then(|v| v.to_str().ok())
        .map(str::to_string);
    let bytes = resp.into_body().collect().await.unwrap().to_bytes();
    (status, content_type, String::from_utf8_lossy(&bytes).to_string())
}

fn edit_multipart_body(boundary: &str, text_fields: &[(&str, &str)], with_mask: bool) -> Vec<u8> {
    let png = base64::engine::general_purpose::STANDARD
        .decode(TEST_PNG_B64)
        .unwrap();
    let mut body = Vec::new();
    for (name, value) in text_fields {
        body.extend_from_slice(
            format!(
                "--{boundary}\r\nContent-Disposition: form-data; name=\"{name}\"\r\n\r\n{value}\r\n"
            )
            .as_bytes(),
        );
    }
    body.extend_from_slice(
        format!(
            "--{boundary}\r\nContent-Disposition: form-data; name=\"image\"; filename=\"one.png\"\r\nContent-Type: image/png\r\n\r\n"
        )
        .as_bytes(),
    );
    body.extend_from_slice(&png);
    body.extend_from_slice(b"\r\n");
    if with_mask {
        body.extend_from_slice(
            format!(
                "--{boundary}\r\nContent-Disposition: form-data; name=\"mask\"; filename=\"mask.png\"\r\nContent-Type: image/png\r\n\r\n"
            )
            .as_bytes(),
        );
        body.extend_from_slice(&png);
        body.extend_from_slice(b"\r\n");
    }
    body.extend_from_slice(format!("--{boundary}--\r\n").as_bytes());
    body
}

async fn sse_post_multipart(
    ctx: &TestContext,
    boundary: &str,
    body: Vec<u8>,
) -> (StatusCode, Option<String>, String) {
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
    let status = resp.status();
    let content_type = resp
        .headers()
        .get(CONTENT_TYPE)
        .and_then(|v| v.to_str().ok())
        .map(str::to_string);
    let bytes = resp.into_body().collect().await.unwrap().to_bytes();
    (status, content_type, String::from_utf8_lossy(&bytes).to_string())
}

fn captured_upstream_body(bodies: &CapturedBodies, endpoint: &str) -> Value {
    bodies
        .lock()
        .expect("captured bodies lock")
        .iter()
        .rev()
        .find(|(name, _)| name == endpoint)
        .map(|(_, body)| body.clone())
        .unwrap_or_else(|| panic!("missing captured upstream body for {endpoint}"))
}

/// IG3: `stream` must be a JSON boolean; IG4: `stream = true` requires n = 1.
#[tokio::test]
async fn image_generations_stream_field_validation_rejects_bad_shapes() {
    let ctx = setup().await;

    let (status, body) = json_post(
        &ctx,
        "/v1/images/generations",
        json!({ "model": "gpt-5-mini", "prompt": "draw", "stream": "yes" }),
    )
    .await;
    assert_eq!(status, StatusCode::BAD_REQUEST, "{body}");
    assert!(body.contains("stream must be a boolean"), "{body}");

    let (status, body) = json_post(
        &ctx,
        "/v1/images/generations",
        json!({ "model": "gpt-5-mini", "prompt": "draw", "stream": true, "n": 2 }),
    )
    .await;
    assert_eq!(status, StatusCode::BAD_REQUEST, "{body}");
    assert!(body.contains("stream=true requires n=1"), "{body}");
}

/// IE4: the edits `stream` text field accepts exactly `true` or `false`.
#[tokio::test]
async fn image_edits_stream_field_validation_rejects_bad_shapes() {
    let ctx = setup().await;
    let boundary = "----monoize-edit-stream-validation";

    let body = edit_multipart_body(
        boundary,
        &[
            ("model", "gpt-5-mini"),
            ("prompt", "edit"),
            ("stream", "1"),
        ],
        false,
    );
    let (status, _, text) = sse_post_multipart(&ctx, boundary, body).await;
    assert_eq!(status, StatusCode::BAD_REQUEST, "{text}");
    assert!(text.contains("stream must be true or false"), "{text}");

    let body = edit_multipart_body(
        boundary,
        &[
            ("model", "gpt-5-mini"),
            ("prompt", "edit"),
            ("stream", "true"),
            ("n", "3"),
        ],
        false,
    );
    let (status, _, text) = sse_post_multipart(&ctx, boundary, body).await;
    assert_eq!(status, StatusCode::BAD_REQUEST, "{text}");
    assert!(text.contains("stream=true requires n=1"), "{text}");
}

/// IS1..IS5 + IG5: generations streaming through an `openai_image` upstream
/// forwards `stream: true` and `partial_images` (OIU-E4/OIU-E6) and encodes
/// downstream `image_generation.*` frames with a `[DONE]` terminator.
#[tokio::test]
async fn image_generations_stream_emits_partials_completed_and_done_via_openai_image() {
    let ctx = setup().await;
    let (upstream_addr, _, captured_bodies) = start_upstream().await;
    let base_url = format!("http://{upstream_addr}");
    create_test_provider(
        &ctx.state,
        "openai-image-gen-stream-test",
        monoize::monoize_routing::MonoizeProviderType::OpenaiImage,
        "gpt-image-gen-stream-test",
        &base_url,
        "upstream-key",
    )
    .await;
    seed_test_model_pricing(&ctx.state, &["gpt-image-gen-stream-test"]).await;

    let (status, content_type, text) = sse_post_json(
        &ctx,
        "/v1/images/generations",
        json!({
            "model": "gpt-image-gen-stream-test",
            "prompt": "draw a cat",
            "stream": true,
            "partial_images": 2,
            "size": "256x256"
        }),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{text}");
    assert!(
        content_type
            .as_deref()
            .is_some_and(|value| value.starts_with("text/event-stream")),
        "{content_type:?}"
    );

    let frames = parse_sse_frames(&text);
    assert_eq!(frames.last().map(|(_, data)| data.as_str()), Some("[DONE]"));
    let events: Vec<(String, Value)> = frames
        .iter()
        .filter(|(_, data)| data != "[DONE]")
        .map(|(event, data)| {
            (
                event.clone().expect("image frame should carry event name"),
                serde_json::from_str::<Value>(data).expect("image frame should be JSON"),
            )
        })
        .collect();
    assert_eq!(events.len(), 3, "{text}");
    for (index, (event, data)) in events.iter().take(2).enumerate() {
        assert_eq!(event, "image_generation.partial_image", "{text}");
        assert_eq!(data["type"].as_str(), Some("image_generation.partial_image"));
        assert_eq!(data["b64_json"].as_str(), Some(TEST_PNG_B64));
        assert_eq!(data["partial_image_index"].as_u64(), Some(index as u64));
        assert_eq!(data["size"].as_str(), Some("256x256"));
        assert_eq!(data["output_format"].as_str(), Some("png"));
        assert!(data["created_at"].is_i64() || data["created_at"].is_u64(), "{data}");
    }
    let (completed_event, completed) = &events[2];
    assert_eq!(completed_event, "image_generation.completed", "{text}");
    assert_eq!(completed["type"].as_str(), Some("image_generation.completed"));
    assert_eq!(completed["b64_json"].as_str(), Some(TEST_PNG_B64));
    assert_eq!(completed["usage"]["input_tokens"].as_u64(), Some(1));
    assert_eq!(completed["usage"]["output_tokens"].as_u64(), Some(1));
    assert_eq!(completed["usage"]["total_tokens"].as_u64(), Some(2));

    let upstream = captured_upstream_body(&captured_bodies, "image_generations");
    assert_eq!(upstream["model"].as_str(), Some("gpt-image-gen-stream-test"));
    assert_eq!(upstream["prompt"].as_str(), Some("draw a cat"));
    assert_eq!(upstream["stream"].as_bool(), Some(true));
    assert_eq!(upstream["partial_images"].as_u64(), Some(2));
}

/// IS9/OIU-S7: an edits streaming sub-request routed to `openai_image` is
/// sent as multipart to `/v1/images/edits` with the `stream` text field, the
/// mask under field name `mask`, and decodes `image_edit.*` upstream SSE into
/// downstream `image_edit.*` frames.
#[tokio::test]
async fn image_edits_stream_uses_multipart_with_mask_and_stream_field() {
    let ctx = setup().await;
    let (upstream_addr, captured_headers, captured_bodies) = start_upstream().await;
    let base_url = format!("http://{upstream_addr}");
    create_test_provider(
        &ctx.state,
        "openai-image-edit-stream-test",
        monoize::monoize_routing::MonoizeProviderType::OpenaiImage,
        "gpt-image-edit-stream-test",
        &base_url,
        "upstream-key",
    )
    .await;
    seed_test_model_pricing(&ctx.state, &["gpt-image-edit-stream-test"]).await;

    let boundary = "----monoize-edit-stream-test";
    let body = edit_multipart_body(
        boundary,
        &[
            ("model", "gpt-image-edit-stream-test"),
            ("prompt", "edit this image"),
            ("stream", "true"),
            ("partial_images", "1"),
        ],
        true,
    );
    let (status, content_type, text) = sse_post_multipart(&ctx, boundary, body).await;
    assert_eq!(status, StatusCode::OK, "{text}");
    assert!(
        content_type
            .as_deref()
            .is_some_and(|value| value.starts_with("text/event-stream")),
        "{content_type:?}"
    );

    let frames = parse_sse_frames(&text);
    assert_eq!(frames.last().map(|(_, data)| data.as_str()), Some("[DONE]"));
    let events: Vec<(String, Value)> = frames
        .iter()
        .filter(|(_, data)| data != "[DONE]")
        .map(|(event, data)| {
            (
                event.clone().expect("image frame should carry event name"),
                serde_json::from_str::<Value>(data).expect("image frame should be JSON"),
            )
        })
        .collect();
    assert_eq!(events.len(), 2, "{text}");
    assert_eq!(events[0].0, "image_edit.partial_image", "{text}");
    assert_eq!(events[0].1["type"].as_str(), Some("image_edit.partial_image"));
    assert_eq!(events[0].1["b64_json"].as_str(), Some(TEST_PNG_B64));
    assert_eq!(events[0].1["partial_image_index"].as_u64(), Some(0));
    assert_eq!(events[1].0, "image_edit.completed", "{text}");
    assert_eq!(events[1].1["b64_json"].as_str(), Some(TEST_PNG_B64));
    assert_eq!(events[1].1["usage"]["total_tokens"].as_u64(), Some(2));

    let upstream = captured_upstream_body(&captured_bodies, "image_edits");
    assert_eq!(upstream["model"].as_str(), Some("gpt-image-edit-stream-test"));
    assert_eq!(upstream["prompt"].as_str(), Some("edit this image"));
    assert_eq!(upstream["stream"].as_str(), Some("true"));
    assert_eq!(upstream["partial_images"].as_str(), Some("1"));
    assert_eq!(
        upstream["images"][0]["b64"].as_str(),
        Some(TEST_PNG_B64),
        "{upstream}"
    );
    assert_eq!(
        upstream["masks"][0]["b64"].as_str(),
        Some(TEST_PNG_B64),
        "{upstream}"
    );
    let multipart_sent = captured_headers
        .lock()
        .expect("captured headers lock")
        .iter()
        .any(|(name, value)| {
            name == "image_edits-content-type" && value.starts_with("multipart/form-data")
        });
    assert!(multipart_sent, "upstream edits call must be multipart");
}

/// IS6: a streaming sub-request that fails before any completed frame emits
/// one `error` frame and terminates with `[DONE]` on an HTTP 200 stream.
#[tokio::test]
async fn image_generations_stream_emits_error_frame_when_routing_fails() {
    let ctx = setup().await;
    let (status, content_type, text) = sse_post_json(
        &ctx,
        "/v1/images/generations",
        json!({
            "model": "model-with-no-provider",
            "prompt": "draw a cat",
            "stream": true
        }),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{text}");
    assert!(
        content_type
            .as_deref()
            .is_some_and(|value| value.starts_with("text/event-stream")),
        "{content_type:?}"
    );

    let frames = parse_sse_frames(&text);
    assert_eq!(frames.last().map(|(_, data)| data.as_str()), Some("[DONE]"));
    let error_frames: Vec<&(Option<String>, String)> = frames
        .iter()
        .filter(|(event, _)| event.as_deref() == Some("error"))
        .collect();
    assert_eq!(error_frames.len(), 1, "{text}");
    let data: Value = serde_json::from_str(&error_frames[0].1).expect("error frame JSON");
    assert_eq!(data["type"].as_str(), Some("error"));
    assert!(data["error"]["message"].is_string(), "{data}");
    assert!(data["error"]["code"].is_string(), "{data}");
    assert!(data["error"]["type"].is_string(), "{data}");
}

/// IS3 with a Responses upstream: partial frames decoded from
/// `response.image_generation_call.partial_image` (OIU-S5a inverse path)
/// surface as downstream `image_generation.partial_image` frames.
#[tokio::test]
async fn image_generations_stream_surfaces_partials_from_responses_upstream() {
    let ctx = setup().await;
    let (upstream_addr, _, _) = start_upstream().await;
    let base_url = format!("http://{upstream_addr}");

    let mut models = HashMap::new();
    models.insert(
        "gpt-image-resp-stream-test".to_string(),
        monoize::monoize_routing::MonoizeModelEntry {
            redirect: None,
            multiplier: monoize::exact_decimal::Multiplier::ONE,
        },
    );
    ctx.state
        .monoize_store
        .create_provider(monoize::monoize_routing::CreateMonoizeProviderInput {
            allow_free_when_unpriced_override: None,
            allow_free_when_missing_usage_override: Some(true),
            name: "responses-image-stream-test".to_string(),
            api_type_overrides: Vec::new(),
            group_ids: Vec::new(),
            channels: vec![monoize::monoize_routing::CreateMonoizeChannelInput {
                id: None,
                name: "responses-image-stream-test-ch".to_string(),
                provider_type: monoize::monoize_routing::MonoizeProviderType::Responses,
                base_url,
                api_key: Some("upstream-key".to_string()),
                weight: 1,
                enabled: true,
                passive_failure_count_threshold_override: None,
                passive_cooldown_seconds_override: None,
                passive_window_seconds_override: None,
                passive_rate_limit_cooldown_seconds_override: None,
                models,
                active_probe_enabled_override: None,
                active_probe_interval_seconds_override: None,
                active_probe_success_threshold_override: None,
                active_probe_model_override: None,
                affinity_enabled_override: None,
                affinity_idle_ttl_seconds_override: None,
                affinity_failback_mode_override: None,
                affinity_failback_delay_seconds_override: None,
                proxy_url: None,
                extra_headers: None,
                session_affinity_auto: None,
            }],
            max_retries: -1,
            channel_max_retries: 0,
            channel_retry_interval_ms: 0,
            circuit_breaker_enabled: true,
            per_model_circuit_break: false,
            transforms: vec![monoize::transforms::TransformRuleConfig {
                transform: "image_enable_openai_generation_tool".to_string(),
                enabled: true,
                models: Some(vec!["gpt-image-resp-stream-test".to_string()]),
                phase: monoize::transforms::Phase::Request,
                config: json!({ "force_tool_choice": true }),
            }],
            active_probe_enabled_override: None,
            active_probe_interval_seconds_override: None,
            active_probe_success_threshold_override: None,
            active_probe_model_override: None,
            request_timeout_ms_override: None,
            extra_fields_whitelist: Some(vec!["*".to_string()]),
            strip_cross_protocol_nested_extra: None,
            enabled: true,
            priority: None,
        })
        .await
        .expect("create responses image stream provider");

    let (status, _, text) = sse_post_json(
        &ctx,
        "/v1/images/generations",
        json!({
            "model": "gpt-image-resp-stream-test",
            "prompt": "draw a cat",
            "stream": true,
            "stream_mode": "image_generation_partial"
        }),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{text}");

    let frames = parse_sse_frames(&text);
    assert_eq!(frames.last().map(|(_, data)| data.as_str()), Some("[DONE]"));
    let partial = frames
        .iter()
        .find(|(event, _)| event.as_deref() == Some("image_generation.partial_image"))
        .unwrap_or_else(|| panic!("missing partial frame: {text}"));
    let partial_data: Value = serde_json::from_str(&partial.1).expect("partial frame JSON");
    assert_eq!(partial_data["b64_json"].as_str(), Some("QUJD"));
    assert_eq!(partial_data["partial_image_index"].as_u64(), Some(0));
    let completed = frames
        .iter()
        .find(|(event, _)| event.as_deref() == Some("image_generation.completed"))
        .unwrap_or_else(|| panic!("missing completed frame: {text}"));
    let completed_data: Value = serde_json::from_str(&completed.1).expect("completed frame JSON");
    assert_eq!(completed_data["b64_json"].as_str(), Some(TEST_PNG_B64));
}

/// OIU-S6 + OIU-S7: a non-streaming downstream edit whose transformed request
/// forces `stream = true` collects the upstream `image_edit.*` SSE internally
/// via multipart `/v1/images/edits` and still returns one JSON body.
#[tokio::test]
async fn image_edits_nonstream_collects_forced_upstream_stream_as_multipart() {
    let ctx = setup().await;
    let (upstream_addr, _, captured_bodies) = start_upstream().await;
    let base_url = format!("http://{upstream_addr}");

    let mut models = HashMap::new();
    models.insert(
        "gpt-image-forced-stream-test".to_string(),
        monoize::monoize_routing::MonoizeModelEntry {
            redirect: None,
            multiplier: monoize::exact_decimal::Multiplier::ONE,
        },
    );
    ctx.state
        .monoize_store
        .create_provider(monoize::monoize_routing::CreateMonoizeProviderInput {
            allow_free_when_unpriced_override: None,
            allow_free_when_missing_usage_override: None,
            name: "openai-image-forced-stream-test".to_string(),
            api_type_overrides: Vec::new(),
            group_ids: Vec::new(),
            channels: vec![monoize::monoize_routing::CreateMonoizeChannelInput {
                id: None,
                name: "openai-image-forced-stream-test-ch".to_string(),
                provider_type: monoize::monoize_routing::MonoizeProviderType::OpenaiImage,
                base_url,
                api_key: Some("upstream-key".to_string()),
                weight: 1,
                enabled: true,
                passive_failure_count_threshold_override: None,
                passive_cooldown_seconds_override: None,
                passive_window_seconds_override: None,
                passive_rate_limit_cooldown_seconds_override: None,
                models,
                active_probe_enabled_override: None,
                active_probe_interval_seconds_override: None,
                active_probe_success_threshold_override: None,
                active_probe_model_override: None,
                affinity_enabled_override: None,
                affinity_idle_ttl_seconds_override: None,
                affinity_failback_mode_override: None,
                affinity_failback_delay_seconds_override: None,
                proxy_url: None,
                extra_headers: None,
                session_affinity_auto: None,
            }],
            max_retries: -1,
            channel_max_retries: 0,
            channel_retry_interval_ms: 0,
            circuit_breaker_enabled: true,
            per_model_circuit_break: false,
            transforms: vec![monoize::transforms::TransformRuleConfig {
                transform: "stream_force".to_string(),
                enabled: true,
                models: Some(vec!["gpt-image-forced-stream-test".to_string()]),
                phase: monoize::transforms::Phase::Request,
                config: json!({ "enabled": true }),
            }],
            active_probe_enabled_override: None,
            active_probe_interval_seconds_override: None,
            active_probe_success_threshold_override: None,
            active_probe_model_override: None,
            request_timeout_ms_override: None,
            extra_fields_whitelist: None,
            strip_cross_protocol_nested_extra: None,
            enabled: true,
            priority: None,
        })
        .await
        .expect("create forced-stream openai_image provider");
    seed_test_model_pricing(&ctx.state, &["gpt-image-forced-stream-test"]).await;

    let boundary = "----monoize-edit-forced-stream-test";
    let body = edit_multipart_body(
        boundary,
        &[
            ("model", "gpt-image-forced-stream-test"),
            ("prompt", "edit this image"),
        ],
        true,
    );
    let (status, content_type, text) = sse_post_multipart(&ctx, boundary, body).await;
    assert_eq!(status, StatusCode::OK, "{text}");
    assert!(
        content_type
            .as_deref()
            .is_some_and(|value| value.starts_with("application/json")),
        "{content_type:?}"
    );
    let v: Value = serde_json::from_str(&text).expect("image response JSON");
    assert_eq!(v["data"][0]["b64_json"].as_str(), Some(TEST_PNG_B64), "{text}");

    let upstream = captured_upstream_body(&captured_bodies, "image_edits");
    assert_eq!(upstream["stream"].as_str(), Some("true"), "{upstream}");
    assert_eq!(
        upstream["images"][0]["b64"].as_str(),
        Some(TEST_PNG_B64),
        "{upstream}"
    );
    assert_eq!(
        upstream["masks"][0]["b64"].as_str(),
        Some(TEST_PNG_B64),
        "{upstream}"
    );
}
