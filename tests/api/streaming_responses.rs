use super::*;
use serde_json::json;

#[tokio::test]
async fn responses_streaming_ai_sdk_second_step_keeps_assistant_output_blocks() {
    let ctx = setup().await;
    let req = Request::builder()
        .method("POST")
        .uri("/v1/responses")
        .header(CONTENT_TYPE, "application/json")
        .header(AUTHORIZATION, ctx.auth_header.clone())
        .body(Body::from(
            json!({
                "model":"gpt-5-mini",
                "stream": true,
                "require_assistant_output_content_types": true,
                "input": [
                    {
                        "type":"message",
                        "role":"user",
                        "content":[{"type":"input_text","text":"Before using tools, say one short sentence about your plan. Then call get_weather for tokyo and get_time for tokyo in parallel. After tool results, answer in one sentence."}]
                    },
                    {
                        "type":"message",
                        "role":"assistant",
                        "content":[{"type":"output_text","text":"I'll fetch Tokyo's weather and local time in parallel."}]
                    },
                    {
                        "type":"function_call",
                        "call_id":"call_1",
                        "name":"get_weather",
                        "arguments":"{\"city\":\"tokyo\"}"
                    },
                    {
                        "type":"function_call",
                        "call_id":"call_2",
                        "name":"get_time",
                        "arguments":"{\"city\":\"tokyo\"}"
                    },
                    {
                        "type":"function_call_output",
                        "call_id":"call_1",
                        "output":"{\"city\":\"tokyo\",\"weather\":\"sunny\",\"tempC\":25}"
                    },
                    {
                        "type":"function_call_output",
                        "call_id":"call_2",
                        "output":"{\"city\":\"tokyo\",\"time\":\"10:00 JST\"}"
                    }
                ],
                "tools": [
                    { "type":"function","name":"get_weather","parameters":{"type":"object","properties":{"city":{"type":"string"}},"required":["city"],"additionalProperties":false}},
                    { "type":"function","name":"get_time","parameters":{"type":"object","properties":{"city":{"type":"string"}},"required":["city"],"additionalProperties":false}}
                ]
            })
            .to_string(),
        ))
        .unwrap();

    let resp = ctx.router.clone().oneshot(req).await.unwrap();
    assert_eq!(resp.status(), StatusCode::OK);
    let bytes = resp.into_body().collect().await.unwrap().to_bytes();
    let text = String::from_utf8_lossy(&bytes).to_string();
    assert!(
        !text.contains("event: error"),
        "unexpected downstream error stream: {text}"
    );
    assert!(text.contains("event: response.completed"));
}

#[tokio::test]
async fn responses_streaming_emits_sse_events() {
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
    assert!(text.contains("event: response.created"));
    assert!(text.contains("event: response.completed"));
    let frames = parse_responses_sse_json(&text);
    assert!(!frames.is_empty(), "expected responses sse frames");
    let created = frames
        .iter()
        .find(|(event, _)| event == "response.created")
        .expect("response.created frame");
    assert_eq!(created.1["type"].as_str(), Some("response.created"));
    assert!(
        created.1.get("data").is_none(),
        "must not wrap payload in data field"
    );
    assert!(created.1["sequence_number"].as_u64().is_some());
    assert_eq!(
        count_done_sentinels(&text),
        1,
        "responses stream must emit exactly one [DONE]"
    );
}

#[tokio::test]
async fn responses_streaming_reconstructs_text_from_output_item_done() {
    let ctx = setup().await;
    let req = Request::builder()
        .method("POST")
        .uri("/v1/responses")
        .header(CONTENT_TYPE, "application/json")
        .header(AUTHORIZATION, ctx.auth_header.clone())
        .body(Body::from(
            json!({
                "model":"gpt-5-mini",
                "input":"stream",
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
    assert!(text.contains("event: response.output_text.delta"));
}

#[tokio::test]
async fn responses_streaming_reconstructs_phase_from_output_item_done() {
    let ctx = setup().await;
    let req = Request::builder()
        .method("POST")
        .uri("/v1/responses")
        .header(CONTENT_TYPE, "application/json")
        .header(AUTHORIZATION, ctx.auth_header.clone())
        .body(Body::from(
            json!({
                "model":"gpt-5-mini",
                "input":"stream",
                "stream": true,
                "stream_mode": "item_done_only",
                "message_phase": "commentary"
            })
            .to_string(),
        ))
        .unwrap();
    let resp = ctx.router.clone().oneshot(req).await.unwrap();
    assert_eq!(resp.status(), StatusCode::OK);
    let bytes = resp.into_body().collect().await.unwrap().to_bytes();
    let text = String::from_utf8_lossy(&bytes).to_string();
    assert!(text.contains("event: response.output_text.delta"));
    assert!(text.contains("\"phase\":\"commentary\""));
}

#[tokio::test]
async fn responses_streaming_includes_tool_calls_and_reasoning_when_upstream_is_chat() {
    let ctx = setup().await;
    let req = Request::builder()
        .method("POST")
        .uri("/v1/responses")
        .header(CONTENT_TYPE, "application/json")
        .header(AUTHORIZATION, ctx.auth_header.clone())
        .body(Body::from(
            json!({
                "model":"gpt-5-mini-chat",
                "input":[{"type":"message","role":"user","content":[{"type":"input_text","text":"stream tool"}]}],
                "tools":[{ "type":"function","function":{ "name":"tool_a","parameters":{ "type":"object","additionalProperties":true }}}],
                "parallel_tool_calls": true,
                "stream": true
            })
            .to_string(),
        ))
        .unwrap();
    let resp = ctx.router.clone().oneshot(req).await.unwrap();
    assert_eq!(resp.status(), StatusCode::OK);
    let bytes = resp.into_body().collect().await.unwrap().to_bytes();
    let text = String::from_utf8_lossy(&bytes).to_string();
    assert!(text.contains("event: response.function_call_arguments.delta"));
    assert!(text.contains("event: response.reasoning_summary_text.delta"));
}

#[tokio::test]
async fn responses_streaming_reencodes_greedy_merged_items_with_canonical_sse_boundaries() {
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
                "stream_mode": "reasoning_text_tool"
            })
            .to_string(),
        ))
        .unwrap();
    let resp = ctx.router.clone().oneshot(req).await.unwrap();
    assert_eq!(resp.status(), StatusCode::OK);
    let bytes = resp.into_body().collect().await.unwrap().to_bytes();
    let text = String::from_utf8_lossy(&bytes).to_string();

    assert!(text.contains("event: response.output_item.added"));
    assert!(text.contains("\"output_index\":0"));
    assert!(text.contains("\"output_index\":1"));
    assert!(text.contains("\"output_index\":2"));
    assert!(text.contains("\"type\":\"reasoning\""));
    assert!(text.contains("\"type\":\"message\""));
    assert!(text.contains("\"type\":\"function_call\""));
    assert!(text.contains("\"phase\":\"analysis\""));
    assert!(text.contains("event: response.content_part.added"));
    assert!(text.contains("\"part\":{\"annotations\":[],\"text\":\"\",\"type\":\"output_text\"}"));
    assert!(!text.contains("\"part\":{\"text\":\"\",\"type\":\"reasoning\"}"));
    assert!(!text.contains("event: response.content_part.added\ndata: {\"content_index\":2"));
}

#[tokio::test]
async fn responses_streaming_uses_top_level_payload_fields_and_delta_ids() {
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
                "stream_mode": "reasoning_text_tool"
            })
            .to_string(),
        ))
        .unwrap();
    let resp = ctx.router.clone().oneshot(req).await.unwrap();
    assert_eq!(resp.status(), StatusCode::OK);
    let bytes = resp.into_body().collect().await.unwrap().to_bytes();
    let text = String::from_utf8_lossy(&bytes).to_string();
    let frames = parse_responses_sse_json(&text);

    let text_delta = frames
        .iter()
        .find(|(event, _)| event == "response.output_text.delta")
        .expect("output text delta");
    assert_eq!(
        text_delta.1["type"].as_str(),
        Some("response.output_text.delta")
    );
    assert!(
        text_delta.1.get("data").is_none(),
        "must not nest payload under data"
    );
    assert!(
        text_delta.1["item_id"].as_str().is_some(),
        "text delta must include item_id"
    );
    assert_eq!(text_delta.1["logprobs"], Value::Null);

    let reasoning_summary_delta = frames
        .iter()
        .find(|(event, _)| event == "response.reasoning_summary_text.delta")
        .expect("reasoning summary delta");
    assert_eq!(
        reasoning_summary_delta.1["type"].as_str(),
        Some("response.reasoning_summary_text.delta")
    );
    let reasoning_item_id = reasoning_summary_delta.1["item_id"]
        .as_str()
        .expect("reasoning delta item_id");
    assert!(
        !reasoning_item_id.is_empty(),
        "reasoning item_id must be non-empty"
    );

    let reasoning_text_delta = frames
        .iter()
        .find(|(event, _)| event == "response.reasoning.delta")
        .expect("reasoning delta");
    assert_eq!(
        reasoning_text_delta.1["item_id"].as_str(),
        Some(reasoning_item_id)
    );

    let reasoning_done = frames
        .iter()
        .find(|(event, payload)| {
            event == "response.output_item.done"
                && payload
                    .get("item")
                    .and_then(|item| item.get("type"))
                    .and_then(Value::as_str)
                    == Some("reasoning")
        })
        .expect("reasoning output item done");
    assert_eq!(
        reasoning_done.1["item"]["id"].as_str(),
        Some(reasoning_item_id),
        "reasoning delta and done event must use same item id"
    );

    let function_delta = frames
        .iter()
        .find(|(event, _)| event == "response.function_call_arguments.delta")
        .expect("function call delta");
    assert_eq!(
        function_delta.1["type"].as_str(),
        Some("response.function_call_arguments.delta")
    );
    assert!(function_delta.1["item_id"].as_str().is_some());

    let function_done = frames
        .iter()
        .find(|(event, _)| event == "response.function_call_arguments.done")
        .expect("function call done");
    assert_eq!(function_done.1["arguments"].as_str(), Some("{\"a\":1}"));
}

#[tokio::test]
async fn responses_streaming_distinguishes_reasoning_summary_and_content() {
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
                "stream_mode": "reasoning_text_tool"
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
            .any(|(event, _)| event == "response.reasoning_summary_text.delta")
    );
    assert!(
        frames
            .iter()
            .any(|(event, _)| event == "response.reasoning_summary_text.done")
    );
    assert!(
        frames
            .iter()
            .any(|(event, _)| event == "response.reasoning_summary_part.done")
    );
    assert!(
        frames
            .iter()
            .any(|(event, _)| event == "response.reasoning.delta")
    );
    assert!(
        frames
            .iter()
            .any(|(event, _)| event == "response.reasoning.done")
    );

    let summary_delta = frames
        .iter()
        .find(|(event, _)| event == "response.reasoning_summary_text.delta")
        .expect("summary delta");
    assert_eq!(summary_delta.1["delta"].as_str(), Some("mock_summary"));

    let text_delta = frames
        .iter()
        .find(|(event, _)| event == "response.reasoning.delta")
        .expect("text delta");
    assert_eq!(text_delta.1["delta"].as_str(), Some("mock_reasoning"));

    let reasoning_done = frames
        .iter()
        .find(|(event, _)| event == "response.reasoning.done")
        .expect("reasoning done");
    assert_eq!(reasoning_done.1["text"].as_str(), Some("mock_reasoning"));
}

#[tokio::test]
async fn responses_streaming_preserves_explicit_reasoning_source_from_chat_upstream() {
    let ctx = setup().await;
    let reasoning_source = "upstream-custom-reasoner";
    let req = Request::builder()
        .method("POST")
        .uri("/v1/responses")
        .header(CONTENT_TYPE, "application/json")
        .header(AUTHORIZATION, ctx.auth_header.clone())
        .body(Body::from(
            json!({
                "model":"gpt-5-mini-chat",
                "input":[{"type":"message","role":"user","content":[{"type":"input_text","text":"stream tool"}]}],
                "tools":[{ "type":"function","function":{ "name":"tool_a","parameters":{ "type":"object","additionalProperties":true }}}],
                "parallel_tool_calls": true,
                "stream": true,
                "stream_mode": "reasoning_text_tool",
                "reasoning_source_override": reasoning_source
            })
            .to_string(),
        ))
        .unwrap();
    let resp = ctx.router.clone().oneshot(req).await.unwrap();
    assert_eq!(resp.status(), StatusCode::OK);
    let bytes = resp.into_body().collect().await.unwrap().to_bytes();
    let text = String::from_utf8_lossy(&bytes).to_string();
    let frames = parse_responses_sse_json(&text);

    let summary_delta = frames
        .iter()
        .find(|(event, _)| event == "response.reasoning_summary_text.delta")
        .expect("summary delta");
    assert_eq!(summary_delta.1["delta"].as_str(), Some("mock_summary"));
    assert_eq!(summary_delta.1["source"].as_str(), Some(reasoning_source));

    let reasoning_delta = frames
        .iter()
        .find(|(event, _)| event == "response.reasoning.delta")
        .expect("reasoning delta");
    assert_eq!(reasoning_delta.1["source"].as_str(), Some(reasoning_source));

    let reasoning_done = frames
        .iter()
        .find(|(event, _)| event == "response.reasoning.done")
        .expect("reasoning done");
    assert_eq!(reasoning_done.1["source"].as_str(), Some(reasoning_source));

    let completed = frames
        .iter()
        .find(|(event, _)| event == "response.completed")
        .expect("response completed");
    let output = completed.1["response"]["output"]
        .as_array()
        .expect("completed output array");
    let reasoning_item = output
        .iter()
        .find(|item| item["type"].as_str() == Some("reasoning"))
        .expect("reasoning output item");
    assert_eq!(reasoning_item["source"].as_str(), Some(reasoning_source));
}

#[tokio::test]
async fn responses_streaming_omits_reasoning_source_when_chat_upstream_does_not_send_one() {
    let ctx = setup().await;
    let req = Request::builder()
        .method("POST")
        .uri("/v1/responses")
        .header(CONTENT_TYPE, "application/json")
        .header(AUTHORIZATION, ctx.auth_header.clone())
        .body(Body::from(
            json!({
                "model":"gpt-5-mini-chat",
                "input":[{"type":"message","role":"user","content":[{"type":"input_text","text":"stream"}]}],
                "stream": true,
                "reasoning": { "effort": "medium" },
                "omit_reasoning_source": true
            })
            .to_string(),
        ))
        .unwrap();
    let resp = ctx.router.clone().oneshot(req).await.unwrap();
    assert_eq!(resp.status(), StatusCode::OK);
    let bytes = resp.into_body().collect().await.unwrap().to_bytes();
    let text = String::from_utf8_lossy(&bytes).to_string();
    let frames = parse_responses_sse_json(&text);

    let reasoning_delta = frames
        .iter()
        .find(|(event, _)| event == "response.reasoning.delta")
        .expect("reasoning delta");
    assert!(
        reasoning_delta.1.get("source").is_none(),
        "reasoning delta must omit source when upstream omitted it"
    );

    let completed = frames
        .iter()
        .find(|(event, _)| event == "response.completed")
        .expect("response completed");
    let output = completed.1["response"]["output"]
        .as_array()
        .expect("completed output array");
    let reasoning_item = output
        .iter()
        .find(|item| item["type"].as_str() == Some("reasoning"))
        .expect("reasoning output item");
    assert!(
        reasoning_item.get("source").is_none(),
        "completed reasoning item must omit source when upstream omitted it"
    );
}

#[tokio::test]
async fn responses_streaming_from_responses_upstream_does_not_duplicate_completed_items() {
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
                "stream_mode": "message_then_tool_then_completed"
            })
            .to_string(),
        ))
        .unwrap();
    let resp = ctx.router.clone().oneshot(req).await.unwrap();
    assert_eq!(resp.status(), StatusCode::OK);
    let bytes = resp.into_body().collect().await.unwrap().to_bytes();
    let text = String::from_utf8_lossy(&bytes).to_string();
    let frames = parse_responses_sse_json(&text);

    let output_item_added: Vec<&Value> = frames
        .iter()
        .filter(|(event, _)| event == "response.output_item.added")
        .map(|(_, payload)| payload)
        .collect();
    let output_item_done: Vec<&Value> = frames
        .iter()
        .filter(|(event, _)| event == "response.output_item.done")
        .map(|(_, payload)| payload)
        .collect();

    assert_eq!(
        output_item_added.len(),
        2,
        "must not duplicate added lifecycles: {text}"
    );
    assert_eq!(
        output_item_done.len(),
        2,
        "must not duplicate done lifecycles: {text}"
    );

    let mut added_types: Vec<&str> = output_item_added
        .iter()
        .filter_map(|payload| payload["item"]["type"].as_str())
        .collect();
    let mut done_types: Vec<&str> = output_item_done
        .iter()
        .filter_map(|payload| payload["item"]["type"].as_str())
        .collect();
    added_types.sort_unstable();
    done_types.sort_unstable();
    assert_eq!(
        added_types,
        vec!["function_call", "message"],
        "unexpected added types: {text}"
    );
    assert_eq!(
        done_types,
        vec!["function_call", "message"],
        "unexpected done types: {text}"
    );

    let completed = frames
        .iter()
        .find(|(event, _)| event == "response.completed")
        .map(|(_, payload)| payload)
        .expect("response.completed frame");
    assert_eq!(
        completed["response"]["output"].as_array().map(Vec::len),
        Some(2),
        "completed output must still contain one message and one function call: {text}"
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
    let create_input = monoize::monoize_routing::CreateMonoizeProviderInput {
        name: "mono-transform-strip".to_string(),
        provider_type: monoize::monoize_routing::MonoizeProviderType::ChatCompletion,
        models,
        api_type_overrides: Vec::new(),
        channels: vec![monoize::monoize_routing::CreateMonoizeChannelInput {
            id: Some("mono-transform-strip-ch1".to_string()),
            name: "mono-transform-strip-ch1".to_string(),
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
    assert!(text.contains("event: response.output_text.delta"));
    assert!(text.contains("event: response.completed"));
}

#[tokio::test]
async fn responses_streaming_split_sse_frames_breaks_large_delta_frames() {
    let max_frame_length = 220usize;
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
    let create_input = monoize::monoize_routing::CreateMonoizeProviderInput {
        name: "mono-transform-sse-split".to_string(),
        provider_type: monoize::monoize_routing::MonoizeProviderType::Responses,
        models,
        api_type_overrides: Vec::new(),
        channels: vec![monoize::monoize_routing::CreateMonoizeChannelInput {
            id: Some("mono-transform-sse-split-ch1".to_string()),
            name: "mono-transform-sse-split-ch1".to_string(),
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
async fn responses_streaming_plaintext_reasoning_to_summary_rewrites_reasoning_events() {
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
            name: "mono-transform-summary".to_string(),
            provider_type: monoize::monoize_routing::MonoizeProviderType::Responses,
            models,
            api_type_overrides: Vec::new(),
            channels: vec![monoize::monoize_routing::CreateMonoizeChannelInput {
                id: Some("mono-transform-summary-ch1".to_string()),
                name: "mono-transform-summary-ch1".to_string(),
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

    assert!(text.contains("event: response.reasoning_summary_text.delta"));
    assert!(!text.contains("event: response.reasoning.delta"));
    assert!(text.contains("event: response.output_text.delta"));
}

#[tokio::test]
async fn responses_streaming_markdown_image_transforms_emit_image_part_and_appended_markdown() {
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
            name: "mono-transform-streaming-markdown-images".to_string(),
            provider_type: monoize::monoize_routing::MonoizeProviderType::Responses,
            models,
            api_type_overrides: Vec::new(),
            channels: vec![monoize::monoize_routing::CreateMonoizeChannelInput {
                id: Some("mono-transform-streaming-markdown-images-ch1".to_string()),
                name: "mono-transform-streaming-markdown-images-ch1".to_string(),
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
        event == "response.output_text.delta"
            && payload["delta"].as_str() == Some("see ")
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
                                        && part["text"]
                                            .as_str()
                                            .is_some_and(|text| text.contains("![image](https://example.com/chart.png)"))
                                })
                            })
                    })
                })
    }));
}
