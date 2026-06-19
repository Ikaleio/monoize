
#[tokio::test]
async fn responses_streaming_response_done_does_not_reindex_terminal_tool_outputs_from_node_positions()
 {
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
            payload["output_index"].as_u64() == Some(2)
                && payload["item"]["type"].as_str() == Some("function_call")
                && payload["item"]["id"].as_str() == Some("fc_mock")
        }),
        "terminal function_call done must preserve the streamed tool output index and id: {text}"
    );
    assert!(
        !output_item_done.iter().any(|payload| {
            payload["output_index"].as_u64() == Some(0)
                && payload["item"]["type"].as_str() == Some("message")
        }),
        "reasoning output_index must not be reused for a terminal message during response.done recovery: {text}"
    );
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
