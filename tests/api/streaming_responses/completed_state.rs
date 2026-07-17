
#[tokio::test]
async fn responses_streaming_plaintext_reasoning_to_summary_rewrites_reasoning_events() {
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
            name: "mono-transform-summary".to_string(),
            models,
            api_type_overrides: Vec::new(),
            groups: Vec::new(),
            channels: vec![monoize::monoize_routing::CreateMonoizeChannelInput {
                id: Some("mono-transform-summary-ch1".to_string()),
                name: "mono-transform-summary-ch1".to_string(),
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
                transform: "plaintext_reasoning_to_summary".to_string(),
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
                "model":"gpt-5-mini",
                "input":[{"type":"message","role":"user","content":[{"type":"input_text","text":"stream with reasoning"}]}],
                "tools":[{ "type":"function","name":"tool_a","parameters":{ "type":"object","additionalProperties":true }}],
                "stream": true,
                "stream_mode": "reasoning_text_tool",
                "reasoning": { "effort": "high" }
            })
            .to_string(),
        ))
        .unwrap();
    let resp = ctx.router.clone().oneshot(req).await.unwrap();
    assert_eq!(resp.status(), StatusCode::OK);
    let bytes = resp.into_body().collect().await.unwrap().to_bytes();
    let text = String::from_utf8_lossy(&bytes).to_string();

    assert!(text.contains("event: response.reasoning_summary_text.delta"));
    assert!(!text.contains("event: response.reasoning.delta"));
    assert!(text.contains("event: response.output_text.delta"));
}

#[tokio::test]
async fn responses_streaming_completed_snapshot_merges_reasoning_slot_once() {
    let ctx = setup().await;
    let req = Request::builder()
        .method("POST")
        .uri("/v1/responses")
        .header(CONTENT_TYPE, "application/json")
        .header(AUTHORIZATION, ctx.auth_header.clone())
        .body(Body::from(
            json!({
                "model":"gpt-5-mini",
                "input":[{"type":"message","role":"user","content":[{"type":"input_text","text":"stream tool"}]}],
                "tools":[{ "type":"function","name":"tool_a","parameters":{ "type":"object","additionalProperties":true }}],
                "stream": true,
                "stream_mode": "reasoning_completed_snapshot"
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
        frames
            .iter()
            .any(|(event, _)| event == "response.reasoning_text.delta")
    );
    assert!(
        frames
            .iter()
            .any(|(event, _)| event == "response.reasoning_text.done")
    );
    assert!(!text.contains("event: response.reasoning.delta"));
    assert!(!text.contains("event: response.reasoning.done"));

    let completed = frames
        .iter()
        .find(|(event, _)| event == "response.completed")
        .expect("completed response");
    let output = completed.1["response"]["output"]
        .as_array()
        .expect("completed output array");
    let reasoning_items = output
        .iter()
        .filter(|item| item["type"].as_str() == Some("reasoning"))
        .collect::<Vec<_>>();
    assert_eq!(reasoning_items.len(), 1, "completed output: {output:?}");
    assert!(reasoning_items[0].get("text").is_none());
    assert_eq!(
        reasoning_items[0]["content"],
        json!([{ "type": "reasoning_text", "text": "mock_reasoning" }])
    );
    assert_eq!(
        reasoning_items[0]["summary"],
        json!([{ "type": "summary_text", "text": "mock_summary" }])
    );
    assert!(output.iter().any(|item| item["type"].as_str() == Some("message")));
    assert!(
        output
            .iter()
            .any(|item| item["type"].as_str() == Some("function_call"))
    );
}

#[tokio::test]
async fn responses_streaming_completed_snapshot_conflict_fails_stream() {
    let ctx = setup().await;
    let req = Request::builder()
        .method("POST")
        .uri("/v1/responses")
        .header(CONTENT_TYPE, "application/json")
        .header(AUTHORIZATION, ctx.auth_header.clone())
        .body(Body::from(
            json!({
                "model":"gpt-5-mini",
                "input":[{"type":"message","role":"user","content":[{"type":"input_text","text":"conflict"}]}],
                "tools":[{ "type":"function","name":"tool_a","parameters":{ "type":"object","additionalProperties":true }}],
                "stream": true,
                "stream_mode": "reasoning_completed_conflict"
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
        frames
            .iter()
            .any(|(event, payload)| event == "response.failed"
                && payload["response"]["error"]["code"].as_str()
                    == Some("responses_terminal_conflict")),
        "expected responses_terminal_conflict failure: {text}"
    );
    assert!(
        !frames.iter().any(|(event, _)| event == "response.completed"),
        "conflicting stream must not emit response.completed: {text}"
    );
    assert!(text.trim_end().ends_with("data: [DONE]"));
}

#[tokio::test]
async fn responses_streaming_markdown_image_transforms_emit_image_part_and_appended_markdown() {
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
            name: "mono-transform-streaming-markdown-images".to_string(),
            models,
            api_type_overrides: Vec::new(),
            groups: Vec::new(),
            channels: vec![monoize::monoize_routing::CreateMonoizeChannelInput {
                id: Some("mono-transform-streaming-markdown-images-ch1".to_string()),
                name: "mono-transform-streaming-markdown-images-ch1".to_string(),
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
    let req = Request::builder()
        .method("POST")
        .uri("/v1/responses")
        .header(CONTENT_TYPE, "application/json")
        .header(AUTHORIZATION, ctx.auth_header.clone())
        .body(Body::from(
            json!({
                "model":"gpt-5-mini",
                "input":[{"type":"message","role":"user","content":[{"type":"input_text","text": format!("see {image_markdown}")}]}],
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

    assert!(frames.iter().any(|(event, payload)| {
        event == "response.output_text.delta" && payload["delta"].as_str() == Some("see ")
    }));
    assert!(!frames.iter().any(|(event, payload)| {
        event == "response.output_text.delta"
            && payload["delta"]
                .as_str()
                .is_some_and(|delta| delta.contains("![image](https://example.com/chart.png)"))
    }));
    assert!(frames.iter().any(|(event, payload)| {
        event == "response.content_part.done"
            && payload["part"]["type"].as_str() == Some("output_image")
            && payload["part"]["url"].as_str() == Some("https://example.com/chart.png")
    }));
    assert!(frames.iter().any(|(event, payload)| {
        event == "response.completed"
            && payload["response"]["output"]
                .as_array()
                .is_some_and(|output| {
                    output.iter().any(|item| {
                        item["type"].as_str() == Some("message")
                            && item["content"].as_array().is_some_and(|content| {
                                content.iter().any(|part| {
                                    part["type"].as_str() == Some("output_image")
                                        && part["url"].as_str()
                                            == Some("https://example.com/chart.png")
                                }) && content.iter().any(|part| {
                                    part["type"].as_str() == Some("output_text")
                                        && part["text"].as_str().is_some_and(|text| {
                                            text.contains("![image](https://example.com/chart.png)")
                                        })
                                })
                            })
                    })
                })
    }));
}
