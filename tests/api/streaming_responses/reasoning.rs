
#[tokio::test]
async fn responses_streaming_omits_empty_reasoning_and_compacts_output_indices() {
    let ctx = setup().await;
    let req = Request::builder()
        .method("POST")
        .uri("/v1/responses")
        .header(CONTENT_TYPE, "application/json")
        .header(AUTHORIZATION, ctx.auth_header.clone())
        .body(Body::from(
            json!({
                "model":"gpt-5-mini",
                "input":"stream tool",
                "tools":[{ "type":"function","name":"tool_a","parameters":{ "type":"object","additionalProperties":true }}],
                "stream": true,
                "stream_mode": "reasoning_message_then_tool_completed"
            })
            .to_string(),
        ))
        .unwrap();
    let resp = ctx.router.clone().oneshot(req).await.unwrap();
    assert_eq!(resp.status(), StatusCode::OK);
    let bytes = resp.into_body().collect().await.unwrap().to_bytes();
    let text = String::from_utf8_lossy(&bytes).to_string();
    let frames = parse_responses_sse_json(&text);

    let completed = frames
        .iter()
        .find(|(event, _)| event == "response.completed")
        .map(|(_, payload)| payload)
        .unwrap_or_else(|| panic!("response.completed frame: {text}"));
    let output = completed["response"]["output"]
        .as_array()
        .expect("completed response output array");
    assert_eq!(
        output.len(),
        2,
        "terminal output must not invent an empty reasoning item, and must retain message/function order: {text}"
    );
    assert_eq!(output[0]["type"].as_str(), Some("message"));
    assert_eq!(output[0]["id"].as_str(), Some("msg_mock"));
    assert_eq!(output[1]["type"].as_str(), Some("function_call"));
    assert_eq!(output[1]["id"].as_str(), Some("fc_mock"));
    assert_eq!(output[1]["call_id"].as_str(), Some("call_1"));

    let output_item_done: Vec<&Value> = frames
        .iter()
        .filter(|(event, _)| event == "response.output_item.done")
        .map(|(_, payload)| payload)
        .collect();
    assert!(
        output_item_done.iter().any(|payload| {
            payload["output_index"].as_u64() == Some(1)
                && payload["item"]["type"].as_str() == Some("function_call")
                && payload["item"]["id"].as_str() == Some("fc_mock")
        }),
        "terminal function_call must follow the visible message without an empty-index gap: {text}"
    );
    assert!(
        output_item_done.iter().any(|payload| {
            payload["output_index"].as_u64() == Some(0)
                && payload["item"]["type"].as_str() == Some("message")
        }),
        "the first visible message must use output_index 0: {text}"
    );
    assert!(
        !output_item_done
            .iter()
            .any(|payload| payload["item"]["type"].as_str() == Some("reasoning")),
        "empty reasoning must not emit a terminal lifecycle: {text}"
    );
}

#[tokio::test]
async fn responses_streaming_omits_multiple_reasoning_items_emptied_by_response_transform() {
    let ctx = setup().await;
    let (upstream_addr, _, _) = start_upstream().await;
    let base_url = format!("http://{upstream_addr}");
    let model = "gpt-5.6-sol";

    seed_test_model_pricing(&ctx.state, &[model]).await;

    ctx.state
        .monoize_store
        .create_provider(monoize::monoize_routing::CreateMonoizeProviderInput {
            name: "mono-empty-reasoning-filter".to_string(),
            models: HashMap::from([(
                model.to_string(),
                monoize::monoize_routing::MonoizeModelEntry {
                    redirect: None,
                    multiplier: 1.0,
                },
            )]),
            api_type_overrides: Vec::new(),
            groups: Vec::new(),
            channels: vec![monoize::monoize_routing::CreateMonoizeChannelInput {
                id: Some("mono-empty-reasoning-filter-ch1".to_string()),
                name: "mono-empty-reasoning-filter-ch1".to_string(),
                provider_type: monoize::monoize_routing::MonoizeProviderType::Responses,
                base_url,
                api_key: Some("upstream-key".to_string()),
                weight: 1,
                enabled: true,
                passive_failure_count_threshold_override: None,
                passive_cooldown_seconds_override: None,
                passive_window_seconds_override: None,
                passive_rate_limit_cooldown_seconds_override: None,
                supported_models: vec![model.to_string()],
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
            transforms: vec![monoize::transforms::TransformRuleConfig {
                transform: "strip_encrypted_reasoning".to_string(),
                enabled: true,
                models: None,
                phase: monoize::transforms::Phase::Response,
                config: json!({}),
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
        .unwrap();

    let req = Request::builder()
        .method("POST")
        .uri("/v1/responses")
        .header(CONTENT_TYPE, "application/json")
        .header(AUTHORIZATION, ctx.auth_header.clone())
        .body(Body::from(
            json!({
                "model": model,
                "input": "multiple_encrypted_reasoning_then_message",
                "reasoning": { "effort": "high" },
                "tools": [{
                    "type": "function",
                    "name": "unused",
                    "parameters": { "type": "object", "additionalProperties": false }
                }],
                "stream": true,
                "stream_mode": "multiple_encrypted_reasoning_then_message"
            })
            .to_string(),
        ))
        .unwrap();
    let resp = ctx.router.clone().oneshot(req).await.unwrap();
    assert_eq!(resp.status(), StatusCode::OK);
    let bytes = resp.into_body().collect().await.unwrap().to_bytes();
    let text = String::from_utf8_lossy(&bytes).to_string();
    let frames = parse_responses_sse_json(&text);

    assert!(
        !frames.iter().any(|(event, payload)| {
            matches!(
                event.as_str(),
                "response.output_item.added" | "response.output_item.done"
            ) && payload["item"]["type"].as_str() == Some("reasoning")
        }),
        "empty reasoning lifecycles must be absent: {text}"
    );
    assert!(
        !frames
            .iter()
            .any(|(event, _)| event.starts_with("response.reasoning_")),
        "empty reasoning child lifecycles must be absent: {text}"
    );
    assert!(
        !text.contains("encrypted_payload_"),
        "response transform must remove encrypted payloads: {text}"
    );

    let message_added = frames
        .iter()
        .find(|(event, payload)| {
            event == "response.output_item.added"
                && payload["item"]["type"].as_str() == Some("message")
        })
        .map(|(_, payload)| payload)
        .unwrap_or_else(|| panic!("missing message output item: {text}"));
    assert_eq!(message_added["output_index"], json!(0));

    let completed = frames
        .iter()
        .find(|(event, _)| event == "response.completed")
        .map(|(_, payload)| payload)
        .unwrap_or_else(|| panic!("missing response.completed: {text}"));
    let output = completed["response"]["output"]
        .as_array()
        .expect("completed output array");
    assert_eq!(output.len(), 1, "{text}");
    assert_eq!(output[0]["type"], json!("message"));
    assert_eq!(output[0]["content"][0]["text"], json!("answer"));
}

#[tokio::test]
async fn responses_streaming_emits_single_plain_done_sentinel() {
    let ctx = setup().await;
    let req = Request::builder()
        .method("POST")
        .uri("/v1/responses")
        .header(CONTENT_TYPE, "application/json")
        .header(AUTHORIZATION, ctx.auth_header.clone())
        .body(Body::from(
            json!({
                "model":"gpt-5-mini",
                "input":[{"type":"message","role":"user","content":[{"type":"input_text","text":"stream"}]}],
                "stream": true
            })
            .to_string(),
        ))
        .unwrap();
    let resp = ctx.router.clone().oneshot(req).await.unwrap();
    assert_eq!(resp.status(), StatusCode::OK);
    let bytes = resp.into_body().collect().await.unwrap().to_bytes();
    let text = String::from_utf8_lossy(&bytes).to_string();

    assert_eq!(
        count_done_sentinels(&text),
        1,
        "responses stream must emit one [DONE]"
    );
    assert!(
        text.contains("\ndata: [DONE]\n"),
        "responses stream must terminate with plain data [DONE]: {text}"
    );
    assert!(
        !text.contains("event: [DONE]") && !text.contains("event: done"),
        "responses [DONE] must not be a named event: {text}"
    );
}

#[tokio::test]
async fn gemini_native_stream_roundtrip_works() {
    let ctx = setup().await;
    let req = Request::builder()
        .method("POST")
        .uri("/v1/responses")
        .header(CONTENT_TYPE, "application/json")
        .header(AUTHORIZATION, ctx.auth_header.clone())
        .body(Body::from(
            json!({
                "model":"gemini-2.5-flash",
                "input":[{"type":"message","role":"user","content":[{"type":"input_text","text":"stream"}]}],
                "stream": true
            })
            .to_string(),
        ))
        .unwrap();
    let resp = ctx.router.clone().oneshot(req).await.unwrap();
    assert_eq!(resp.status(), StatusCode::OK);
    let bytes = resp.into_body().collect().await.unwrap().to_bytes();
    let text = String::from_utf8_lossy(&bytes).to_string();
    assert!(
        text.contains("event: response.output_text.delta"),
        "missing output delta: {text}"
    );
    assert!(
        text.contains("event: response.completed"),
        "missing completed marker: {text}"
    );

    let has_goog_key = ctx
        .captured_headers
        .lock()
        .map(|entries| entries.iter().any(|(k, _)| k == "x-goog-api-key"))
        .unwrap_or(false);
    assert!(
        has_goog_key,
        "expected x-goog-api-key header for gemini upstream"
    );
}

#[tokio::test]
async fn responses_streaming_applies_response_transform_from_provider() {
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
    let create_input = monoize::monoize_routing::CreateMonoizeProviderInput {
        name: "mono-transform-strip".to_string(),
        models,
        api_type_overrides: Vec::new(),
        groups: Vec::new(),
        channels: vec![monoize::monoize_routing::CreateMonoizeChannelInput {
            id: Some("mono-transform-strip-ch1".to_string()),
            name: "mono-transform-strip-ch1".to_string(),
            provider_type: monoize::monoize_routing::MonoizeProviderType::ChatCompletion,
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
        transforms: vec![monoize::transforms::TransformRuleConfig {
            transform: "strip_reasoning".to_string(),
            enabled: true,
            models: None,
            phase: monoize::transforms::Phase::Response,
            config: json!({}),
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
    };
    ctx.state
        .monoize_store
        .create_provider(create_input)
        .await
        .unwrap();

    let req = Request::builder()
        .method("POST")
        .uri("/v1/responses")
        .header(CONTENT_TYPE, "application/json")
        .header(AUTHORIZATION, ctx.auth_header.clone())
        .body(Body::from(
            json!({
                "model":"gpt-5-mini",
                "input":[{"type":"message","role":"user","content":[{"type":"input_text","text":"stream with reasoning"}]}],
                "stream": true,
                "reasoning": { "effort": "high" }
            })
            .to_string(),
        ))
        .unwrap();
    let resp = ctx.router.clone().oneshot(req).await.unwrap();
    assert_eq!(resp.status(), StatusCode::OK);
    let bytes = resp.into_body().collect().await.unwrap().to_bytes();
    let text = String::from_utf8_lossy(&bytes).to_string();
    assert!(!text.contains("event: response.reasoning_text.delta"));
    assert!(!text.contains("event: response.reasoning.delta"));
    assert!(text.contains("event: response.output_text.delta"));
    assert!(text.contains("event: response.completed"));
}

#[tokio::test]
async fn responses_streaming_split_sse_frames_breaks_large_delta_frames() {
    let max_frame_length = 220usize;
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
    let create_input = monoize::monoize_routing::CreateMonoizeProviderInput {
        name: "mono-transform-sse-split".to_string(),
        models,
        api_type_overrides: Vec::new(),
        groups: Vec::new(),
        channels: vec![monoize::monoize_routing::CreateMonoizeChannelInput {
            id: Some("mono-transform-sse-split-ch1".to_string()),
            name: "mono-transform-sse-split-ch1".to_string(),
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
        transforms: vec![monoize::transforms::TransformRuleConfig {
            transform: "split_sse_frames".to_string(),
            enabled: true,
            models: None,
            phase: monoize::transforms::Phase::Response,
            config: json!({ "max_frame_length": max_frame_length }),
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
    };
    ctx.state
        .monoize_store
        .create_provider(create_input)
        .await
        .unwrap();

    let long_input = "abcdefghij".repeat(80);
    let req = Request::builder()
        .method("POST")
        .uri("/v1/responses")
        .header(CONTENT_TYPE, "application/json")
        .header(AUTHORIZATION, ctx.auth_header.clone())
        .body(Body::from(
            json!({
                "model":"gpt-5-mini",
                "input":[{"type":"message","role":"user","content":[{"type":"input_text","text": long_input}]}],
                "stream": true
            })
            .to_string(),
        ))
        .unwrap();
    let resp = ctx.router.clone().oneshot(req).await.unwrap();
    assert_eq!(resp.status(), StatusCode::OK);
    let bytes = resp.into_body().collect().await.unwrap().to_bytes();
    let text = String::from_utf8_lossy(&bytes).to_string();

    let delta_events = text.matches("event: response.output_text.delta").count();
    assert!(
        delta_events >= 2,
        "expected split_sse_frames to emit multiple delta events, body={text}"
    );
    assert!(text.contains("event: response.completed"));

    let mut reconstructed = String::new();
    let mut current_event = String::new();
    for line in text.lines() {
        if let Some(event) = line.strip_prefix("event: ") {
            current_event = event.to_string();
            continue;
        }
        if let Some(payload) = line.strip_prefix("data: ") {
            if current_event == "response.output_text.delta" {
                assert!(
                    payload.len() <= max_frame_length,
                    "expected split output_text delta payloads to respect max_frame_length, len={}, payload={payload}",
                    payload.len()
                );
                let value: Value = serde_json::from_str(payload).expect("delta payload json");
                let piece = value.get("delta").and_then(|v| v.as_str()).unwrap_or("");
                reconstructed.push_str(piece);
            }
        }
    }
    assert_eq!(reconstructed, "abcdefghij".repeat(80));
}

#[tokio::test]
async fn responses_streaming_maps_messages_thinking_to_reasoning_summary() {
    let ctx = setup().await;
    let req = Request::builder()
        .method("POST")
        .uri("/v1/responses")
        .header(CONTENT_TYPE, "application/json")
        .header(AUTHORIZATION, ctx.auth_header.clone())
        .body(Body::from(
            json!({
                "model": "gpt-5-mini-msg",
                "input": "show summarized thinking",
                "reasoning": { "effort": "high" },
                "stream": true
            })
            .to_string(),
        ))
        .unwrap();
    let resp = ctx.router.clone().oneshot(req).await.unwrap();
    assert_eq!(resp.status(), StatusCode::OK);
    let bytes = resp.into_body().collect().await.unwrap().to_bytes();
    let text = String::from_utf8_lossy(&bytes).to_string();
    let frames = parse_responses_sse_json(&text);

    let summary_deltas = frames
        .iter()
        .filter(|(event, _)| event == "response.reasoning_summary_text.delta")
        .filter_map(|(_, payload)| payload["delta"].as_str())
        .collect::<Vec<_>>();
    assert_eq!(summary_deltas, vec!["mock_reasoning"], "{text}");
    assert!(
        !frames
            .iter()
            .any(|(event, _)| event == "response.reasoning_text.delta"),
        "Messages thinking is a summary, not raw reasoning content: {text}"
    );

    let reasoning = frames
        .iter()
        .find(|(event, payload)| {
            event == "response.output_item.done"
                && payload["item"]["type"].as_str() == Some("reasoning")
        })
        .map(|(_, payload)| &payload["item"])
        .unwrap_or_else(|| panic!("missing completed reasoning item: {text}"));
    assert_eq!(
        reasoning["summary"],
        json!([{ "type": "summary_text", "text": "mock_reasoning" }])
    );
    assert_eq!(reasoning["content"], json!([]));
}
