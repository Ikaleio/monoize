use super::*;

async fn collect_messages_stream_events(ctx: &TestContext, body: Value) -> Vec<Value> {
    let req = Request::builder()
        .method("POST")
        .uri("/v1/messages")
        .header(CONTENT_TYPE, "application/json")
        .header(AUTHORIZATION, ctx.auth_header.clone())
        .body(Body::from(body.to_string()))
        .unwrap();
    let resp = ctx.router.clone().oneshot(req).await.unwrap();
    assert_eq!(resp.status(), StatusCode::OK);
    let bytes = resp.into_body().collect().await.unwrap().to_bytes();
    let text = String::from_utf8_lossy(&bytes).to_string();
    text.lines()
        .filter(|l| l.starts_with("data: "))
        .filter_map(|l| {
            let payload = l.strip_prefix("data: ").unwrap();
            serde_json::from_str::<Value>(payload).ok()
        })
        .collect()
}

async fn collect_messages_stream_text(ctx: &TestContext, body: Value) -> String {
    let req = Request::builder()
        .method("POST")
        .uri("/v1/messages")
        .header(CONTENT_TYPE, "application/json")
        .header(AUTHORIZATION, ctx.auth_header.clone())
        .body(Body::from(body.to_string()))
        .unwrap();
    let resp = ctx.router.clone().oneshot(req).await.unwrap();
    assert_eq!(resp.status(), StatusCode::OK);
    let bytes = resp.into_body().collect().await.unwrap().to_bytes();
    String::from_utf8_lossy(&bytes).to_string()
}

fn message_block_event_sequence(events: &[Value]) -> Vec<(u64, String)> {
    events
        .iter()
        .filter_map(|event| {
            let index = event.get("index").and_then(Value::as_u64)?;
            let event_type = event.get("type").and_then(Value::as_str)?;
            if !matches!(
                event_type,
                "content_block_start" | "content_block_delta" | "content_block_stop"
            ) {
                return None;
            }
            Some((index, event_type.to_string()))
        })
        .collect()
}

fn assert_non_interleaved_message_blocks(events: &[Value], label: &str) {
    let sequence = message_block_event_sequence(events);
    let mut active_block: Option<u64> = None;
    let mut seen_starts: HashMap<u64, usize> = HashMap::new();
    let mut seen_stops: HashMap<u64, usize> = HashMap::new();

    for (index, event_type) in sequence {
        match event_type.as_str() {
            "content_block_start" => {
                assert!(
                    active_block.is_none(),
                    "{label}: block {index} started while block {active_block:?} was still open"
                );
                *seen_starts.entry(index).or_insert(0) += 1;
                active_block = Some(index);
            }
            "content_block_delta" => {
                assert_eq!(
                    active_block,
                    Some(index),
                    "{label}: delta for block {index} appeared while active block was {active_block:?}"
                );
            }
            "content_block_stop" => {
                assert_eq!(
                    active_block,
                    Some(index),
                    "{label}: stop for block {index} appeared while active block was {active_block:?}"
                );
                *seen_stops.entry(index).or_insert(0) += 1;
                active_block = None;
            }
            _ => unreachable!(),
        }
    }

    assert!(active_block.is_none(), "{label}: final block left open");
    for (index, starts) in seen_starts {
        assert_eq!(starts, 1, "{label}: block {index} started {starts} times");
        assert_eq!(
            seen_stops.get(&index).copied().unwrap_or_default(),
            1,
            "{label}: block {index} must stop exactly once"
        );
    }
}

fn assert_messages_stream_invariants(events: &[Value], label: &str) {
    assert!(!events.is_empty(), "{label}: expected at least one event");
    assert_eq!(
        events.first().unwrap()["type"].as_str(),
        Some("message_start"),
        "{label}: first event must be message_start"
    );
    let msg = &events.first().unwrap()["message"];
    assert_eq!(
        msg["type"].as_str(),
        Some("message"),
        "{label}: message_start.message.type"
    );
    assert_eq!(
        msg["role"].as_str(),
        Some("assistant"),
        "{label}: message_start.message.role"
    );

    assert_eq!(
        events.last().unwrap()["type"].as_str(),
        Some("message_stop"),
        "{label}: last event must be message_stop"
    );
    let second_last = &events[events.len() - 2];
    assert_eq!(
        second_last["type"].as_str(),
        Some("message_delta"),
        "{label}: second-to-last event must be message_delta"
    );
    assert!(
        second_last["delta"]["stop_reason"].as_str().is_some(),
        "{label}: message_delta must have stop_reason"
    );

    let starts: Vec<u64> = events
        .iter()
        .filter(|e| e["type"].as_str() == Some("content_block_start"))
        .filter_map(|e| e["index"].as_u64())
        .collect();
    let stops: Vec<u64> = events
        .iter()
        .filter(|e| e["type"].as_str() == Some("content_block_stop"))
        .filter_map(|e| e["index"].as_u64())
        .collect();
    for idx in &starts {
        assert!(
            stops.contains(idx),
            "{label}: content_block_start(index={idx}) has no matching stop"
        );
    }

    for idx in starts {
        let lifecycle: Vec<&str> = events
            .iter()
            .filter(|event| event["index"].as_u64() == Some(idx))
            .filter_map(|event| event["type"].as_str())
            .collect();
        assert!(
            !lifecycle.is_empty(),
            "{label}: expected lifecycle events for block {idx}"
        );
        assert_eq!(
            lifecycle.first().copied(),
            Some("content_block_start"),
            "{label}: block {idx} must start with content_block_start"
        );
        assert_eq!(
            lifecycle.last().copied(),
            Some("content_block_stop"),
            "{label}: block {idx} must end with content_block_stop"
        );
        assert_eq!(
            lifecycle
                .iter()
                .filter(|ty| **ty == "content_block_start")
                .count(),
            1,
            "{label}: block {idx} must have exactly one start"
        );
        assert_eq!(
            lifecycle
                .iter()
                .filter(|ty| **ty == "content_block_stop")
                .count(),
            1,
            "{label}: block {idx} must have exactly one stop"
        );
        let stop_pos = lifecycle
            .iter()
            .position(|ty| *ty == "content_block_stop")
            .expect("stop position");
        assert!(
            lifecycle[..stop_pos]
                .iter()
                .all(|ty| matches!(*ty, "content_block_start" | "content_block_delta")),
            "{label}: block {idx} contains non-delta event before stop"
        );
    }
}

#[tokio::test]
async fn messages_streaming_preserves_upstream_thinking_delta_granularity() {
    let ctx = setup().await;
    let text = collect_messages_stream_text(
        &ctx,
        json!({
            "model": "gpt-5-mini-chat",
            "max_tokens": 64,
            "thinking": { "type": "enabled", "budget_tokens": 2048 },
            "messages": [{ "role": "user", "content": [{ "type": "text", "text": "think stream chat" }] }],
            "stream": true
        }),
    )
    .await;
    let frames = parse_sse_frames(&text);
    let events: Vec<Value> = frames
        .iter()
        .filter_map(|(_, data)| serde_json::from_str::<Value>(data).ok())
        .collect();

    let thinking_deltas: Vec<&str> = events
        .iter()
        .filter(|event| event["delta"]["type"].as_str() == Some("thinking_delta"))
        .filter_map(|event| event["delta"]["thinking"].as_str())
        .collect();
    assert_eq!(
        thinking_deltas,
        vec!["mock_reasoning"],
        "thinking delta should preserve upstream chunking: {text}"
    );
}

#[tokio::test]
async fn messages_streaming_keeps_signature_in_thinking_block_and_delta_order() {
    let ctx = setup().await;
    let text = collect_messages_stream_text(
        &ctx,
        json!({
            "model": "gpt-5-mini-chat",
            "max_tokens": 64,
            "thinking": { "type": "enabled", "budget_tokens": 2048 },
            "messages": [{ "role": "user", "content": [{ "type": "text", "text": "think stream chat" }] }],
            "stream": true
        }),
    )
    .await;
    let frames = parse_sse_frames(&text);
    let events: Vec<Value> = frames
        .iter()
        .filter_map(|(_, data)| serde_json::from_str::<Value>(data).ok())
        .collect();

    let thinking_start = events
        .iter()
        .find(|event| {
            event["type"].as_str() == Some("content_block_start")
                && event["content_block"]["type"].as_str() == Some("thinking")
        })
        .expect("thinking block start");
    assert_eq!(
        thinking_start["content_block"]["thinking"].as_str(),
        Some("")
    );

    let mut thinking_delta_pos = None;
    let mut signature_delta_pos = None;
    let mut stop_pos = None;
    for (idx, event) in events.iter().enumerate() {
        match event["delta"]["type"].as_str() {
            Some("thinking_delta") if thinking_delta_pos.is_none() => {
                thinking_delta_pos = Some(idx)
            }
            Some("signature_delta") if signature_delta_pos.is_none() => {
                signature_delta_pos = Some(idx)
            }
            _ => {}
        }
        if event["type"].as_str() == Some("content_block_stop") && stop_pos.is_none() {
            stop_pos = Some(idx);
        }
    }
    let thinking_delta_pos = thinking_delta_pos.expect("thinking delta position");
    let signature_delta_pos = signature_delta_pos.expect("signature delta position");
    let stop_pos = stop_pos.expect("stop position");
    assert!(
        thinking_delta_pos < signature_delta_pos,
        "thinking_delta must precede signature_delta: {text}"
    );
    assert!(
        signature_delta_pos < stop_pos,
        "signature_delta must precede content_block_stop: {text}"
    );
}

#[tokio::test]
async fn messages_streaming_maps_tool_use_and_thinking_from_chat_upstream() {
    let ctx = setup().await;
    let req = Request::builder()
        .method("POST")
        .uri("/v1/messages")
        .header(CONTENT_TYPE, "application/json")
        .header(AUTHORIZATION, ctx.auth_header.clone())
        .body(Body::from(
            json!({
                "model":"gpt-5-mini-chat",
                "messages":[{"role":"user","content":[{"type":"text","text":"stream tool"}]}],
                "tools":[{ "name":"tool_a","input_schema":{ "type":"object","additionalProperties":true }}],
                "stream": true
            })
            .to_string(),
        ))
        .unwrap();
    let resp = ctx.router.clone().oneshot(req).await.unwrap();
    assert_eq!(resp.status(), StatusCode::OK);
    let bytes = resp.into_body().collect().await.unwrap().to_bytes();
    let text = String::from_utf8_lossy(&bytes).to_string();
    assert!(text.contains("\"tool_use\""));
    assert!(text.contains("\"thinking_delta\""));
}

#[tokio::test]
async fn messages_streaming_maps_text_from_responses_output_item_done() {
    let ctx = setup().await;
    let req = Request::builder()
        .method("POST")
        .uri("/v1/messages")
        .header(CONTENT_TYPE, "application/json")
        .header(AUTHORIZATION, ctx.auth_header.clone())
        .body(Body::from(
            json!({
                "model":"gpt-5-mini",
                "messages":[{"role":"user","content":[{"type":"text","text":"stream plain"}]}],
                "stream": true,
                "stream_mode": "item_done_only"
            })
            .to_string(),
        ))
        .unwrap();
    let resp = ctx.router.clone().oneshot(req).await.unwrap();
    assert_eq!(resp.status(), StatusCode::OK);
    let bytes = resp.into_body().collect().await.unwrap().to_bytes();
    let text = String::from_utf8_lossy(&bytes).to_string();
    assert!(text.contains("\"text_delta\""));
    assert!(text.contains("\"message_stop\""));
}

#[tokio::test]
async fn messages_streaming_emits_message_delta_before_stop_for_responses_upstream() {
    let ctx = setup().await;
    let req = Request::builder()
        .method("POST")
        .uri("/v1/messages")
        .header(CONTENT_TYPE, "application/json")
        .header(AUTHORIZATION, ctx.auth_header.clone())
        .body(Body::from(
            json!({
                "model":"gpt-5-mini",
                "messages":[{"role":"user","content":[{"type":"text","text":"stream plain"}]}],
                "stream": true,
                "stream_mode": "item_done_only"
            })
            .to_string(),
        ))
        .unwrap();
    let resp = ctx.router.clone().oneshot(req).await.unwrap();
    assert_eq!(resp.status(), StatusCode::OK);
    let bytes = resp.into_body().collect().await.unwrap().to_bytes();
    let text = String::from_utf8_lossy(&bytes).to_string();
    let delta_pos = text.find("\"message_delta\"").unwrap_or(usize::MAX);
    let stop_pos = text.find("\"message_stop\"").unwrap_or(usize::MAX);
    assert!(
        delta_pos != usize::MAX,
        "expected message_delta in stream: {text}"
    );
    assert!(
        stop_pos != usize::MAX,
        "expected message_stop in stream: {text}"
    );
    assert!(
        delta_pos < stop_pos,
        "message_delta must appear before message_stop: {text}"
    );
}

#[tokio::test]
async fn messages_streaming_from_responses_includes_message_delta_usage() {
    let ctx = setup().await;
    let events = collect_messages_stream_events(
        &ctx,
        json!({
            "model":"gpt-5-mini",
            "messages":[{"role":"user","content":[{"type":"text","text":"stream usage"}]}],
            "stream": true,
            "emit_usage": true
        }),
    )
    .await;

    let msg_delta = events
        .iter()
        .find(|e| e["type"].as_str() == Some("message_delta"))
        .expect("message_delta");
    assert_eq!(msg_delta["usage"]["input_tokens"].as_u64(), Some(12));
    assert_eq!(msg_delta["usage"]["output_tokens"].as_u64(), Some(8));
}

#[tokio::test]
async fn messages_streaming_emits_named_sse_events() {
    let ctx = setup().await;
    let text = collect_messages_stream_text(
        &ctx,
        json!({
            "model": "gpt-5-mini",
            "max_tokens": 64,
            "messages": [{ "role": "user", "content": [{ "type": "text", "text": "stream text" }] }],
            "stream": true
        }),
    )
    .await;
    let frames = parse_sse_frames(&text);
    let first_json = frames
        .iter()
        .find_map(|(event, data)| {
            if data == "[DONE]" {
                return None;
            }
            Some((
                event
                    .clone()
                    .expect("messages frame should have event name"),
                serde_json::from_str::<Value>(data).expect("messages frame should be json"),
            ))
        })
        .expect("at least one messages frame");
    assert_eq!(first_json.0, "message_start");
    assert_eq!(first_json.1["type"].as_str(), Some("message_start"));
    assert!(text.contains("event: message_start"));
    assert!(text.contains("event: content_block_start"));
    assert!(text.contains("event: content_block_delta"));
    assert!(text.contains("event: message_delta"));
    assert!(text.contains("event: message_stop"));
    assert_eq!(
        count_done_sentinels(&text),
        0,
        "messages stream must not append [DONE]"
    );
}

#[tokio::test]
async fn messages_streaming_does_not_duplicate_text_deltas_or_blocks() {
    let ctx = setup().await;
    let events = collect_messages_stream_events(
        &ctx,
        json!({
            "model": "gpt-5-mini-chat",
            "max_tokens": 64,
            "messages": [{ "role": "user", "content": [{ "type": "text", "text": "stream chat text" }] }],
            "stream": true
        }),
    )
    .await;

    let text_deltas: Vec<String> = events
        .iter()
        .filter(|event| {
            event["type"].as_str() == Some("content_block_delta")
                && event["delta"]["type"].as_str() == Some("text_delta")
        })
        .filter_map(|event| event["delta"]["text"].as_str().map(|text| text.to_string()))
        .collect();
    assert_eq!(
        text_deltas,
        vec!["stream chat text".to_string()],
        "text should stream once without full-content replay"
    );

    let text_block_starts = events
        .iter()
        .filter(|event| {
            event["type"].as_str() == Some("content_block_start")
                && event["content_block"]["type"].as_str() == Some("text")
        })
        .count();
    assert_eq!(text_block_starts, 1, "text block should start exactly once");
    assert_non_interleaved_message_blocks(&events, "chat→msg text stream");
}

#[tokio::test]
async fn messages_streaming_plaintext_reasoning_to_summary_preserves_thinking_delta() {
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
            name: "mono-transform-summary-messages".to_string(),
            provider_type: monoize::monoize_routing::MonoizeProviderType::Responses,
            models,
            api_type_overrides: Vec::new(),
            channels: vec![monoize::monoize_routing::CreateMonoizeChannelInput {
                id: Some("mono-transform-summary-messages-ch1".to_string()),
                name: "mono-transform-summary-messages-ch1".to_string(),
                base_url,
                api_key: Some("upstream-key".to_string()),
                weight: 1,
                enabled: true,
                groups: Vec::new(),
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
            enabled: true,
            priority: Some(-1),
        })
        .await
        .unwrap();

    let text = collect_messages_stream_text(
        &ctx,
        json!({
            "model": "gpt-5-mini",
            "max_tokens": 64,
            "thinking": { "type": "enabled", "budget_tokens": 2048 },
            "messages": [{ "role": "user", "content": [{ "type": "text", "text": "stream with reasoning" }] }],
            "stream": true
        }),
    )
    .await;
    let events: Vec<Value> = parse_sse_frames(&text)
        .into_iter()
        .filter_map(|(_, data)| serde_json::from_str::<Value>(&data).ok())
        .collect();

    let thinking_deltas: Vec<&str> = events
        .iter()
        .filter(|event| event["delta"]["type"].as_str() == Some("thinking_delta"))
        .filter_map(|event| event["delta"]["thinking"].as_str())
        .collect();
    assert_eq!(
        thinking_deltas,
        vec!["mock_reasoning"],
        "messages stream should preserve the transformed reasoning summary as thinking text: {text}"
    );

    assert!(
        events.iter().any(|event| {
            event["type"].as_str() == Some("content_block_start")
                && event["content_block"]["type"].as_str() == Some("thinking")
        }),
        "expected a thinking block after summary transform: {text}"
    );
}

#[tokio::test]
async fn messages_stream_text_from_responses_upstream_event_sequence() {
    let ctx = setup().await;
    let events = collect_messages_stream_events(
        &ctx,
        json!({
            "model": "gpt-5-mini",
            "max_tokens": 64,
            "messages": [{ "role": "user", "content": [{ "type": "text", "text": "stream text" }] }],
            "stream": true
        }),
    )
    .await;

    assert_messages_stream_invariants(&events, "resp→msg stream");
    let has_text_delta = events
        .iter()
        .any(|e| e.get("delta").and_then(|d| d["type"].as_str()) == Some("text_delta"));
    assert!(has_text_delta, "expected text_delta in stream");
}

#[tokio::test]
async fn messages_stream_text_from_chat_upstream_event_sequence() {
    let ctx = setup().await;
    let events = collect_messages_stream_events(
        &ctx,
        json!({
            "model": "gpt-5-mini-chat",
            "max_tokens": 64,
            "messages": [{ "role": "user", "content": [{ "type": "text", "text": "stream chat text" }] }],
            "stream": true
        }),
    )
    .await;

    assert_messages_stream_invariants(&events, "chat→msg stream");
    let has_text_delta = events
        .iter()
        .any(|e| e.get("delta").and_then(|d| d["type"].as_str()) == Some("text_delta"));
    assert!(has_text_delta, "expected text_delta from chat upstream");
}

#[tokio::test]
async fn messages_stream_text_from_gemini_upstream_event_sequence() {
    let ctx = setup().await;
    let events = collect_messages_stream_events(
        &ctx,
        json!({
            "model": "gemini-2.5-flash",
            "max_tokens": 64,
            "messages": [{ "role": "user", "content": [{ "type": "text", "text": "stream gem text" }] }],
            "stream": true
        }),
    )
    .await;

    assert_messages_stream_invariants(&events, "gemini→msg stream");
    let has_text_delta = events
        .iter()
        .any(|e| e.get("delta").and_then(|d| d["type"].as_str()) == Some("text_delta"));
    assert!(has_text_delta, "expected text_delta from gemini upstream");
}

#[tokio::test]
async fn messages_stream_text_from_grok_upstream_event_sequence() {
    let ctx = setup().await;
    let events = collect_messages_stream_events(
        &ctx,
        json!({
            "model": "grok-4",
            "max_tokens": 64,
            "messages": [{ "role": "user", "content": [{ "type": "text", "text": "stream grok text" }] }],
            "stream": true
        }),
    )
    .await;

    assert_messages_stream_invariants(&events, "grok→msg stream");
    let has_text_delta = events
        .iter()
        .any(|e| e.get("delta").and_then(|d| d["type"].as_str()) == Some("text_delta"));
    assert!(has_text_delta, "expected text_delta from grok upstream");
}

#[tokio::test]
async fn messages_stream_passthrough_from_messages_upstream() {
    let ctx = setup().await;
    let events = collect_messages_stream_events(
        &ctx,
        json!({
            "model": "gpt-5-mini-msg",
            "max_tokens": 64,
            "messages": [{ "role": "user", "content": [{ "type": "text", "text": "stream passthrough" }] }],
            "stream": true
        }),
    )
    .await;

    assert!(!events.is_empty(), "expected events from passthrough");
    assert_eq!(
        events.first().unwrap()["type"].as_str(),
        Some("message_start"),
        "passthrough should start with message_start"
    );
}

#[tokio::test]
async fn messages_stream_thinking_from_responses_upstream() {
    let ctx = setup().await;
    let events = collect_messages_stream_events(
        &ctx,
        json!({
            "model": "gpt-5-mini",
            "max_tokens": 64,
            "thinking": { "type": "enabled", "budget_tokens": 2048 },
            "messages": [{ "role": "user", "content": [{ "type": "text", "text": "think stream" }] }],
            "stream": true,
            "stream_mode": "reasoning_text_tool",
            "tools": [{ "name": "tool_a", "input_schema": { "type": "object", "additionalProperties": true } }]
        }),
    )
    .await;

    assert_messages_stream_invariants(&events, "resp→msg thinking stream");

    let has_thinking_delta = events
        .iter()
        .any(|e| e.get("delta").and_then(|d| d["type"].as_str()) == Some("thinking_delta"));
    assert!(
        has_thinking_delta,
        "expected thinking_delta from responses upstream"
    );

    let has_text_delta = events
        .iter()
        .any(|e| e.get("delta").and_then(|d| d["type"].as_str()) == Some("text_delta"));
    let has_tool_use_start = events.iter().any(|e| {
        e["type"].as_str() == Some("content_block_start")
            && e["content_block"]["type"].as_str() == Some("tool_use")
    });
    assert!(
        has_text_delta || has_tool_use_start,
        "expected downstream content or tool_use block alongside thinking"
    );

    let has_signature_delta = events
        .iter()
        .any(|e| e.get("delta").and_then(|d| d["type"].as_str()) == Some("signature_delta"));
    assert!(
        has_signature_delta,
        "expected signature_delta from responses upstream"
    );
}

#[tokio::test]
async fn messages_stream_thinking_from_chat_upstream() {
    let ctx = setup().await;
    let events = collect_messages_stream_events(
        &ctx,
        json!({
            "model": "gpt-5-mini-chat",
            "max_tokens": 64,
            "thinking": { "type": "enabled", "budget_tokens": 2048 },
            "messages": [{ "role": "user", "content": [{ "type": "text", "text": "think stream chat" }] }],
            "stream": true
        }),
    )
    .await;

    assert_messages_stream_invariants(&events, "chat→msg thinking stream");

    let has_thinking_delta = events
        .iter()
        .any(|e| e.get("delta").and_then(|d| d["type"].as_str()) == Some("thinking_delta"));
    assert!(
        has_thinking_delta,
        "expected thinking_delta from chat upstream"
    );

    let has_signature_delta = events
        .iter()
        .any(|e| e.get("delta").and_then(|d| d["type"].as_str()) == Some("signature_delta"));
    assert!(
        has_signature_delta,
        "expected signature_delta from chat upstream"
    );
}

#[tokio::test]
async fn messages_stream_signature_delta_does_not_precede_thinking_delta() {
    let ctx = setup().await;
    let events = collect_messages_stream_events(
        &ctx,
        json!({
            "model": "gpt-5-mini",
            "max_tokens": 64,
            "thinking": { "type": "enabled", "budget_tokens": 2048 },
            "messages": [{ "role": "user", "content": [{ "type": "text", "text": "think stream" }] }],
            "stream": true,
            "stream_mode": "reasoning_text_tool",
            "tools": [{ "name": "tool_a", "input_schema": { "type": "object", "additionalProperties": true } }]
        }),
    )
    .await;

    let thinking_delta_pos = events
        .iter()
        .position(|event| event["delta"]["type"].as_str() == Some("thinking_delta"));
    let signature_delta_pos = events
        .iter()
        .position(|event| event["delta"]["type"].as_str() == Some("signature_delta"));

    let thinking_delta_pos = thinking_delta_pos.expect("thinking delta position");
    let signature_delta_pos = signature_delta_pos.expect("signature delta position");
    assert!(
        thinking_delta_pos < signature_delta_pos,
        "signature_delta must not precede thinking_delta"
    );
}

#[tokio::test]
async fn messages_stream_tool_use_from_responses_upstream() {
    let ctx = setup().await;
    let events = collect_messages_stream_events(
        &ctx,
        json!({
            "model": "gpt-5-mini",
            "max_tokens": 64,
            "messages": [{ "role": "user", "content": [{ "type": "text", "text": "tool stream" }] }],
            "tools": [{ "name": "tool_a", "input_schema": { "type": "object", "additionalProperties": true } }],
            "stream": true
        }),
    )
    .await;

    assert_messages_stream_invariants(&events, "resp→msg tool stream");

    let tool_start = events.iter().find(|e| {
        e["type"].as_str() == Some("content_block_start")
            && e["content_block"]["type"].as_str() == Some("tool_use")
    });
    assert!(
        tool_start.is_some(),
        "expected tool_use content_block_start"
    );
    let tool_start = tool_start.unwrap();
    assert!(
        tool_start["content_block"]["name"].as_str().is_some(),
        "tool_use block must have name"
    );
    assert!(
        tool_start["content_block"]["id"].as_str().is_some(),
        "tool_use block must have id"
    );

    let has_input_json = events
        .iter()
        .any(|e| e.get("delta").and_then(|d| d["type"].as_str()) == Some("input_json_delta"));
    assert!(has_input_json, "expected input_json_delta in tool stream");

    let msg_delta = events
        .iter()
        .find(|e| e["type"].as_str() == Some("message_delta"))
        .expect("message_delta");
    assert_eq!(
        msg_delta["delta"]["stop_reason"].as_str(),
        Some("tool_use"),
        "stop_reason must be tool_use"
    );
}

#[tokio::test]
async fn messages_stream_tool_use_from_responses_completed_fallback() {
    let ctx = setup().await;
    let events = collect_messages_stream_events(
        &ctx,
        json!({
            "model": "gpt-5-mini",
            "max_tokens": 64,
            "messages": [{ "role": "user", "content": [{ "type": "text", "text": "tool stream" }] }],
            "tools": [{ "name": "tool_a", "input_schema": { "type": "object", "additionalProperties": true } }],
            "stream": true,
            "stream_mode": "completed_only_tool"
        }),
    )
    .await;

    assert_messages_stream_invariants(&events, "resp→msg completed fallback");

    let tool_start = events.iter().find(|e| {
        e["type"].as_str() == Some("content_block_start")
            && e["content_block"]["type"].as_str() == Some("tool_use")
    });
    assert!(
        tool_start.is_some(),
        "expected tool_use content_block_start from completed fallback"
    );
    let has_input_json = events
        .iter()
        .any(|e| e.get("delta").and_then(|d| d["type"].as_str()) == Some("input_json_delta"));
    assert!(
        has_input_json,
        "expected input_json_delta from completed fallback"
    );
    let msg_delta = events
        .iter()
        .find(|e| e["type"].as_str() == Some("message_delta"))
        .expect("message_delta");
    assert_eq!(
        msg_delta["delta"]["stop_reason"].as_str(),
        Some("tool_use"),
        "stop_reason must be tool_use"
    );
}

#[tokio::test]
async fn messages_stream_parallel_tool_use_from_chat_upstream() {
    let ctx = setup().await;
    let events = collect_messages_stream_events(
        &ctx,
        json!({
            "model": "gpt-5-mini-chat",
            "max_tokens": 64,
            "messages": [{ "role": "user", "content": [{ "type": "text", "text": "parallel tools" }] }],
            "tools": [
                { "name": "tool_a", "input_schema": { "type": "object", "additionalProperties": true } },
                { "name": "tool_b", "input_schema": { "type": "object", "additionalProperties": true } }
            ],
            "parallel_tool_calls": true,
            "stream": true
        }),
    )
    .await;

    assert_messages_stream_invariants(&events, "chat→msg parallel tool stream");

    let has_thinking = events
        .iter()
        .any(|e| e.get("delta").and_then(|d| d["type"].as_str()) == Some("thinking_delta"));
    assert!(has_thinking, "expected thinking_delta with tool calls");

    let tool_starts: Vec<&Value> = events
        .iter()
        .filter(|e| {
            e["type"].as_str() == Some("content_block_start")
                && e["content_block"]["type"].as_str() == Some("tool_use")
        })
        .collect();
    assert!(
        !tool_starts.is_empty(),
        "expected at least one tool_use block"
    );
    assert_non_interleaved_message_blocks(&events, "chat→msg parallel tool stream");
}

#[tokio::test]
async fn messages_streaming_from_chat_preserves_strict_block_order_in_raw_sse() {
    let ctx = setup().await;
    let text = collect_messages_stream_text(
        &ctx,
        json!({
            "model": "gpt-5-mini-chat",
            "max_tokens": 64,
            "thinking": { "type": "enabled", "budget_tokens": 2048 },
            "messages": [{ "role": "user", "content": [{ "type": "text", "text": "parallel tools" }] }],
            "tools": [
                { "name": "tool_a", "input_schema": { "type": "object", "additionalProperties": true } },
                { "name": "tool_b", "input_schema": { "type": "object", "additionalProperties": true } }
            ],
            "parallel_tool_calls": true,
            "stream": true
        }),
    )
    .await;
    let events: Vec<Value> = parse_sse_frames(&text)
        .into_iter()
        .filter_map(|(_, data)| serde_json::from_str::<Value>(&data).ok())
        .collect();
    assert_non_interleaved_message_blocks(&events, "raw chat→msg mixed stream");
}

#[tokio::test]
async fn messages_streaming_from_responses_preserves_strict_block_order_in_raw_sse() {
    let ctx = setup().await;
    let text = collect_messages_stream_text(
        &ctx,
        json!({
            "model": "gpt-5-mini",
            "max_tokens": 64,
            "thinking": { "type": "enabled", "budget_tokens": 2048 },
            "messages": [{ "role": "user", "content": [{ "type": "text", "text": "think stream" }] }],
            "stream": true
        }),
    )
    .await;
    let events: Vec<Value> = parse_sse_frames(&text)
        .into_iter()
        .filter_map(|(_, data)| serde_json::from_str::<Value>(&data).ok())
        .collect();
    assert_non_interleaved_message_blocks(&events, "raw responses→msg mixed stream");
}

#[tokio::test]
async fn messages_stream_message_start_envelope_fields() {
    let ctx = setup().await;
    let events = collect_messages_stream_events(
        &ctx,
        json!({
            "model": "gpt-5-mini",
            "max_tokens": 64,
            "messages": [{ "role": "user", "content": [{ "type": "text", "text": "envelope check" }] }],
            "stream": true
        }),
    )
    .await;

    let msg_start = events.first().expect("at least one event");
    assert_eq!(msg_start["type"].as_str(), Some("message_start"));
    let msg = &msg_start["message"];
    assert!(msg["id"].as_str().is_some(), "message_start must have id");
    assert_eq!(msg["type"].as_str(), Some("message"));
    assert_eq!(msg["role"].as_str(), Some("assistant"));
    assert!(msg["model"].as_str().is_some(), "must have model");
    assert!(
        msg["content"].as_array().is_some(),
        "must have content array"
    );
    assert!(
        msg["stop_reason"].is_null(),
        "stop_reason should be null at start"
    );
    assert!(
        msg["stop_sequence"].is_null(),
        "stop_sequence should be null at start"
    );
    assert!(msg["usage"].is_object(), "must have usage at start");
}
