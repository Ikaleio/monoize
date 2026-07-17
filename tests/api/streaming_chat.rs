use super::*;

#[tokio::test]
async fn chat_streaming_preserves_summary_vs_reasoning_in_openrouter_extension() {
    let ctx = setup().await;
    let req = Request::builder()
        .method("POST")
        .uri("/v1/chat/completions")
        .header(CONTENT_TYPE, "application/json")
        .header(AUTHORIZATION, ctx.auth_header.clone())
        .body(Body::from(
            json!({
                "model":"gpt-5-mini",
                "messages":[{"role":"user","content":"stream tool"}],
                "tools":[{ "type":"function","function":{ "name":"tool_a","parameters":{ "type":"object","additionalProperties":true }}}],
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

    assert!(
        text.contains("\"type\":\"reasoning.summary\""),
        "chat stream should expose reasoning summary detail: {text}"
    );
    assert!(
        text.contains("\"summary\":\"mock_summary\""),
        "chat stream should preserve summary field: {text}"
    );
    assert!(
        text.contains("\"type\":\"reasoning.text\""),
        "chat stream should expose reasoning text detail: {text}"
    );
    assert!(
        text.contains("\"text\":\"mock_reasoning\""),
        "chat stream should preserve reasoning text field: {text}"
    );
    assert!(
        !text.contains("\"summary\":\"mock_reasoning\""),
        "chat stream must not reinterpret raw reasoning text as a summary: {text}"
    );
    assert!(
        !text.contains("\"delta\":{\"reasoning\":"),
        "chat stream should keep structured reasoning out of delta.reasoning: {text}"
    );
    assert!(
        !text.contains("\"signature\":"),
        "OpenRouter reasoning.text details should not carry signature: {text}"
    );
}

#[tokio::test]
async fn chat_streaming_preserves_encrypted_reasoning_from_chat_upstream() {
    let ctx = setup().await;
    let req = Request::builder()
        .method("POST")
        .uri("/v1/chat/completions")
        .header(CONTENT_TYPE, "application/json")
        .header(AUTHORIZATION, ctx.auth_header.clone())
        .body(Body::from(
            json!({
                "model":"gpt-5-mini-chat",
                "messages":[{"role":"user","content":"stream encrypted reasoning"}],
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

    assert!(
        text.contains("\"type\":\"reasoning.text\""),
        "chat stream should preserve reasoning text detail from chat upstream: {text}"
    );
    assert!(
        text.contains("\"text\":\"mock_reasoning\""),
        "chat stream should preserve reasoning text payload from chat upstream: {text}"
    );
    assert!(
        text.contains("\"type\":\"reasoning.encrypted\""),
        "chat stream should preserve encrypted reasoning detail from chat upstream: {text}"
    );
    assert!(
        text.contains("\"data\":\"mock_sig\""),
        "same-Chat streaming should replay the native encrypted detail: {text}"
    );
    assert!(
        !text.contains("\"data\":\"mz2."),
        "same-Chat streaming must not replace the native detail with an envelope: {text}"
    );
}

#[tokio::test]
async fn chat_streaming_reasoning_details_preserve_raw_entries_and_arrival_order() {
    let ctx = setup().await;
    let req = Request::builder()
        .method("POST")
        .uri("/v1/chat/completions")
        .header(CONTENT_TYPE, "application/json")
        .header(AUTHORIZATION, ctx.auth_header.clone())
        .body(Body::from(
            json!({
                "model": "gpt-5-mini-chat",
                "messages": [{ "role": "user", "content": "ordered reasoning" }],
                "stream": true,
                "stream_mode": "chat_ordered_reasoning_details"
            })
            .to_string(),
        ))
        .unwrap();
    let resp = ctx.router.clone().oneshot(req).await.unwrap();
    assert_eq!(resp.status(), StatusCode::OK);
    let bytes = resp.into_body().collect().await.unwrap().to_bytes();
    let text = String::from_utf8_lossy(&bytes).to_string();

    let payloads = parse_sse_frames(&text)
        .into_iter()
        .filter_map(|(_, data)| (data != "[DONE]").then_some(data))
        .map(|data| serde_json::from_str::<Value>(&data).expect("Chat SSE JSON"))
        .collect::<Vec<_>>();
    let mut details = Vec::new();
    let mut detail_frame_indices = Vec::new();
    let mut content_frame_index = None;
    for (frame_index, payload) in payloads.iter().enumerate() {
        let choice = &payload["choices"][0];
        if let Some(frame_details) = choice["delta"]["reasoning_details"].as_array() {
            assert_eq!(
                frame_details.len(),
                1,
                "one detail per Chat delta: {payload}"
            );
            assert!(
                choice["finish_reason"].is_null(),
                "reasoning detail delta must be non-terminal: {payload}"
            );
            detail_frame_indices.push(frame_index);
            details.push(frame_details[0].clone());
        }
        if choice["delta"]["content"].as_str() == Some("answer") {
            content_frame_index = Some(frame_index);
        }
    }

    assert_eq!(
        details,
        vec![
            json!({ "type": "reasoning.summary", "summary": "first", "id": "sum_1", "format": "openrouter", "index": 0, "future": "summary" }),
            json!({ "type": "reasoning.text", "text": "second", "signature": "native-signature", "id": "txt_1", "format": "openrouter", "index": 1, "future": { "text": true } }),
            json!({ "type": "reasoning.text", "text": "second", "signature": "native-signature", "id": "txt_1", "format": "openrouter", "index": 1, "future": { "text": true } }),
            json!({ "type": "reasoning.encrypted", "data": "opaque-a", "id": "enc_1", "format": "openrouter", "index": 2, "future": [1] }),
            json!({ "type": "reasoning.server_tool_call", "tool_name": "openrouter:fusion", "arguments": "{\"q\":1}", "result": "{\"ok\":true}", "tool_call_id": "call_srv", "id": "srv_1", "format": "openrouter", "index": 3, "future": { "server": true } }),
            json!({ "type": "reasoning.encrypted", "data": "opaque-a", "id": "enc_2", "format": "openrouter", "index": 4, "future": [2] }),
        ]
    );
    let content_frame_index = content_frame_index.expect("interleaved content frame");
    assert!(
        detail_frame_indices[2] < content_frame_index
            && content_frame_index < detail_frame_indices[3],
        "detail/content arrival order must survive: {text}"
    );
    assert_eq!(count_done_sentinels(&text), 1, "{text}");
    assert!(!text.contains("mz2."), "no synthetic envelope: {text}");
}

#[tokio::test]
async fn chat_streaming_terminal_message_snapshot_emits_only_missing_suffixes_and_native_extras() {
    let ctx = setup().await;
    let req = Request::builder()
        .method("POST")
        .uri("/v1/chat/completions")
        .header(CONTENT_TYPE, "application/json")
        .header(AUTHORIZATION, ctx.auth_header.clone())
        .body(Body::from(
            json!({
                "model": "gpt-5-mini-chat",
                "messages": [{ "role": "user", "content": "terminal snapshot" }],
                "tools": [{
                    "type": "function",
                    "function": {
                        "name": "tool_a",
                        "parameters": { "type": "object", "additionalProperties": true }
                    }
                }],
                "stream": true,
                "stream_mode": "chat_terminal_message_snapshot"
            })
            .to_string(),
        ))
        .unwrap();
    let resp = ctx.router.clone().oneshot(req).await.unwrap();
    assert_eq!(resp.status(), StatusCode::OK);
    let bytes = resp.into_body().collect().await.unwrap().to_bytes();
    let text = String::from_utf8_lossy(&bytes).to_string();

    let payloads = parse_sse_frames(&text)
        .into_iter()
        .filter_map(|(_, data)| (data != "[DONE]").then_some(data))
        .map(|data| serde_json::from_str::<Value>(&data).expect("Chat SSE JSON"))
        .collect::<Vec<_>>();
    let mut content = String::new();
    let mut reasoning = String::new();
    let mut arguments = String::new();
    let mut native_meta = Vec::new();
    let mut terminal_positions = Vec::new();
    let mut last_delta_position = None;

    for (position, payload) in payloads.iter().enumerate() {
        let choice = &payload["choices"][0];
        let delta = &choice["delta"];
        if let Some(value) = delta.get("content").and_then(Value::as_str) {
            content.push_str(value);
            last_delta_position = Some(position);
        }
        if let Some(value) = delta.get("reasoning_content").and_then(Value::as_str) {
            reasoning.push_str(value);
            last_delta_position = Some(position);
        }
        if let Some(details) = delta.get("reasoning_details").and_then(Value::as_array) {
            for detail in details {
                if let Some(value) = detail.get("text").and_then(Value::as_str) {
                    reasoning.push_str(value);
                }
            }
            last_delta_position = Some(position);
        }
        if let Some(calls) = delta.get("tool_calls").and_then(Value::as_array) {
            for call in calls {
                if let Some(value) = call["function"]["arguments"].as_str() {
                    arguments.push_str(value);
                }
            }
            last_delta_position = Some(position);
        }
        if let Some(value) = delta.get("native_meta") {
            native_meta.push(value.clone());
            last_delta_position = Some(position);
        }
        if choice
            .get("finish_reason")
            .is_some_and(|value| !value.is_null())
        {
            terminal_positions.push(position);
        }
    }

    assert_eq!(
        content, "answer",
        "text snapshot suffix must appear once: {text}"
    );
    assert_eq!(
        reasoning, "thinking",
        "reasoning snapshot suffix must appear once: {text}"
    );
    assert_eq!(
        arguments, "{\"a\":1}",
        "tool argument snapshot suffix must appear once: {text}"
    );
    assert_eq!(
        native_meta,
        vec![
            json!({ "origin": "incremental" }),
            json!({ "origin": "terminal" })
        ],
        "each native delta/message extension must survive once: {text}"
    );
    assert_eq!(terminal_positions.len(), 1, "one terminal chunk: {text}");
    assert!(
        last_delta_position.is_some_and(|position| position < terminal_positions[0]),
        "all reconstructed deltas must precede the terminal chunk: {text}"
    );
    assert_eq!(count_done_sentinels(&text), 1, "{text}");
    assert!(
        !text.contains("_monoize_"),
        "internal marker leaked: {text}"
    );
    assert!(
        !text.contains("must-not-leak"),
        "internal value leaked: {text}"
    );
}

#[tokio::test]
async fn chat_streaming_emits_single_plain_done_and_no_named_events() {
    let ctx = setup().await;
    let req = Request::builder()
        .method("POST")
        .uri("/v1/chat/completions")
        .header(CONTENT_TYPE, "application/json")
        .header(AUTHORIZATION, ctx.auth_header.clone())
        .body(Body::from(
            json!({
                "model":"gpt-5-mini-chat",
                "messages":[{"role":"user","content":"hello"}],
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
        "chat stream must emit one [DONE]"
    );
    assert!(
        !text.lines().any(|line| line.starts_with("event: ")),
        "chat completions stream must be data-only SSE: {text}"
    );
}

#[tokio::test]
async fn chat_streaming_maps_tool_calls_and_reasoning_from_responses_upstream() {
    let ctx = setup().await;
    let req = Request::builder()
        .method("POST")
        .uri("/v1/chat/completions")
        .header(CONTENT_TYPE, "application/json")
        .header(AUTHORIZATION, ctx.auth_header.clone())
        .body(Body::from(
            json!({
                "model":"gpt-5-mini",
                "messages":[{"role":"user","content":"stream tool"}],
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
    assert!(text.contains("\"tool_calls\""));
    assert!(text.contains("\"reasoning_details\""));
    assert!(text.contains("\"type\":\"reasoning.encrypted\""));
    assert!(text.contains("\"data\":\"mz2."));
    assert!(text.contains("[DONE]"));
}

#[tokio::test]
async fn chat_streaming_from_responses_uses_node_authoritative_tool_deltas_without_duplication() {
    let ctx = setup().await;
    let req = Request::builder()
        .method("POST")
        .uri("/v1/chat/completions")
        .header(CONTENT_TYPE, "application/json")
        .header(AUTHORIZATION, ctx.auth_header.clone())
        .body(Body::from(
            json!({
                "model":"gpt-5-mini",
                "messages":[{"role":"user","content":"stream tool"}],
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

    let mut tool_header_chunks = 0usize;
    let mut argument_chunks = 0usize;
    let mut finish_reasons: Vec<String> = Vec::new();

    for line in text.lines() {
        let Some(payload) = line.strip_prefix("data: ") else {
            continue;
        };
        if payload == "[DONE]" {
            continue;
        }
        let Ok(v) = serde_json::from_str::<Value>(payload) else {
            continue;
        };
        let Some(choice) = v
            .get("choices")
            .and_then(|v| v.as_array())
            .and_then(|arr| arr.first())
        else {
            continue;
        };
        if let Some(reason) = choice.get("finish_reason").and_then(|v| v.as_str()) {
            if !reason.is_empty() {
                finish_reasons.push(reason.to_string());
            }
        }
        if let Some(tool_calls) = choice
            .get("delta")
            .and_then(|d| d.get("tool_calls"))
            .and_then(|v| v.as_array())
        {
            for tool_call in tool_calls {
                let idx = tool_call.get("index").and_then(|v| v.as_u64());
                let id = tool_call.get("id").and_then(|v| v.as_str());
                let name = tool_call
                    .get("function")
                    .and_then(|f| f.get("name"))
                    .and_then(|v| v.as_str());
                let arguments = tool_call
                    .get("function")
                    .and_then(|f| f.get("arguments"))
                    .and_then(|v| v.as_str())
                    .unwrap_or("");

                if idx == Some(0) && id == Some("call_1") && name == Some("tool_a") {
                    tool_header_chunks += 1;
                }
                if idx == Some(0) && arguments == "{\"a\":1}" {
                    argument_chunks += 1;
                }
            }
        }
    }

    assert_eq!(
        tool_header_chunks, 1,
        "node-owned tool header should emit exactly once even when bridge events coexist: {text}"
    );
    assert_eq!(
        argument_chunks, 1,
        "node-owned tool argument delta should emit exactly once even when bridge events coexist: {text}"
    );
    assert_eq!(
        finish_reasons,
        vec!["tool_calls".to_string()],
        "terminal chunk should still be exactly once and normalized from node-owned tool deltas: {text}"
    );
}

#[tokio::test]
async fn chat_streaming_maps_tool_calls_from_responses_completed_fallback() {
    let ctx = setup().await;
    let req = Request::builder()
        .method("POST")
        .uri("/v1/chat/completions")
        .header(CONTENT_TYPE, "application/json")
        .header(AUTHORIZATION, ctx.auth_header.clone())
        .body(Body::from(
            json!({
                "model":"gpt-5-mini",
                "messages":[{"role":"user","content":"stream tool"}],
                "tools":[{ "type":"function","function":{ "name":"tool_a","parameters":{ "type":"object","additionalProperties":true }}}],
                "stream": true,
                "stream_mode": "completed_only_tool"
            })
            .to_string(),
        ))
        .unwrap();
    let resp = ctx.router.clone().oneshot(req).await.unwrap();
    assert_eq!(resp.status(), StatusCode::OK);
    let bytes = resp.into_body().collect().await.unwrap().to_bytes();
    let text = String::from_utf8_lossy(&bytes).to_string();
    assert!(text.contains("\"tool_calls\""));
    assert!(text.contains("\"finish_reason\":\"tool_calls\""));
    assert!(text.contains("[DONE]"));
}

#[tokio::test]
async fn chat_streaming_keeps_chat_upstream_terminal_tool_calls_reason() {
    let ctx = setup().await;
    let req = Request::builder()
        .method("POST")
        .uri("/v1/chat/completions")
        .header(CONTENT_TYPE, "application/json")
        .header(AUTHORIZATION, ctx.auth_header.clone())
        .body(Body::from(
            json!({
                "model":"gpt-5-mini-chat",
                "messages":[{"role":"user","content":"stream tool"}],
                "tools":[{ "type":"function","function":{ "name":"tool_a","parameters":{ "type":"object","additionalProperties":true }}}],
                "stream": true
            })
            .to_string(),
        ))
        .unwrap();
    let resp = ctx.router.clone().oneshot(req).await.unwrap();
    assert_eq!(resp.status(), StatusCode::OK);
    let bytes = resp.into_body().collect().await.unwrap().to_bytes();
    let text = String::from_utf8_lossy(&bytes).to_string();

    let mut finish_reasons: Vec<String> = Vec::new();
    for line in text.lines() {
        let Some(payload) = line.strip_prefix("data: ") else {
            continue;
        };
        if payload == "[DONE]" {
            continue;
        }
        let Ok(v) = serde_json::from_str::<Value>(payload) else {
            continue;
        };
        if let Some(reason) = v
            .get("choices")
            .and_then(|v| v.as_array())
            .and_then(|arr| arr.first())
            .and_then(|c| c.get("finish_reason"))
            .and_then(|v| v.as_str())
        {
            if !reason.is_empty() {
                finish_reasons.push(reason.to_string());
            }
        }
    }

    assert_eq!(
        finish_reasons,
        vec!["tool_calls".to_string()],
        "chat upstream terminal finish reasons should be preserved without synthetic stop: {text}"
    );
    assert!(text.contains("[DONE]"));
}

#[tokio::test]
async fn chat_streaming_normalizes_chat_upstream_stop_to_tool_calls_when_tools_emitted() {
    let ctx = setup().await;
    let req = Request::builder()
        .method("POST")
        .uri("/v1/chat/completions")
        .header(CONTENT_TYPE, "application/json")
        .header(AUTHORIZATION, ctx.auth_header.clone())
        .body(Body::from(
            json!({
                "model":"gpt-5-mini-chat",
                "messages":[{"role":"user","content":"stream tool"}],
                "tools":[{ "type":"function","function":{ "name":"tool_a","parameters":{ "type":"object","additionalProperties":true }}}],
                "stream": true,
                "force_finish_reason": "stop"
            })
            .to_string(),
        ))
        .unwrap();
    let resp = ctx.router.clone().oneshot(req).await.unwrap();
    assert_eq!(resp.status(), StatusCode::OK);
    let bytes = resp.into_body().collect().await.unwrap().to_bytes();
    let text = String::from_utf8_lossy(&bytes).to_string();

    let mut finish_reasons: Vec<String> = Vec::new();
    for line in text.lines() {
        let Some(payload) = line.strip_prefix("data: ") else {
            continue;
        };
        if payload == "[DONE]" {
            continue;
        }
        let Ok(v) = serde_json::from_str::<Value>(payload) else {
            continue;
        };
        if let Some(reason) = v
            .get("choices")
            .and_then(|v| v.as_array())
            .and_then(|arr| arr.first())
            .and_then(|c| c.get("finish_reason"))
            .and_then(|v| v.as_str())
        {
            if !reason.is_empty() {
                finish_reasons.push(reason.to_string());
            }
        }
    }

    assert_eq!(
        finish_reasons,
        vec!["tool_calls".to_string()],
        "finish_reason should normalize to tool_calls when tool deltas were emitted: {text}"
    );
    assert!(text.contains("[DONE]"));
}

#[tokio::test]
async fn chat_streaming_parallel_tool_calls_from_chat_upstream_reassembles_arguments() {
    let ctx = setup().await;
    let req = Request::builder()
        .method("POST")
        .uri("/v1/chat/completions")
        .header(CONTENT_TYPE, "application/json")
        .header(AUTHORIZATION, ctx.auth_header.clone())
        .body(Body::from(
            json!({
                "model":"gpt-5-mini-chat",
                "messages":[{"role":"user","content":"stream tool"}],
                "tools":[
                    { "type":"function","function":{ "name":"tool_a","parameters":{ "type":"object","additionalProperties":true }}},
                    { "type":"function","function":{ "name":"tool_b","parameters":{ "type":"object","additionalProperties":true }}}
                ],
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

    let mut tool_calls_by_idx: HashMap<u64, (String, String, String)> = HashMap::new();
    let mut finish_reasons: Vec<String> = Vec::new();
    for line in text.lines() {
        let Some(payload) = line.strip_prefix("data: ") else {
            continue;
        };
        if payload == "[DONE]" {
            continue;
        }
        let Ok(v) = serde_json::from_str::<Value>(payload) else {
            continue;
        };
        if let Some(reason) = v
            .get("choices")
            .and_then(|v| v.as_array())
            .and_then(|arr| arr.first())
            .and_then(|c| c.get("finish_reason"))
            .and_then(|v| v.as_str())
        {
            if !reason.is_empty() {
                finish_reasons.push(reason.to_string());
            }
        }
        if let Some(tcs) = v
            .get("choices")
            .and_then(|v| v.as_array())
            .and_then(|arr| arr.first())
            .and_then(|c| c.get("delta"))
            .and_then(|d| d.get("tool_calls"))
            .and_then(|v| v.as_array())
        {
            for tc in tcs {
                let idx = tc.get("index").and_then(|v| v.as_u64()).unwrap_or(0);
                let entry = tool_calls_by_idx
                    .entry(idx)
                    .or_insert_with(|| (String::new(), String::new(), String::new()));
                if let Some(id) = tc.get("id").and_then(|v| v.as_str()) {
                    entry.0 = id.to_string();
                }
                if let Some(name) = tc
                    .get("function")
                    .and_then(|f| f.get("name"))
                    .and_then(|v| v.as_str())
                {
                    entry.1 = name.to_string();
                }
                if let Some(args) = tc
                    .get("function")
                    .and_then(|f| f.get("arguments"))
                    .and_then(|v| v.as_str())
                {
                    entry.2.push_str(args);
                }
            }
        }
    }

    assert_eq!(
        tool_calls_by_idx.len(),
        2,
        "expected 2 parallel tool calls: {text}"
    );
    let tc0 = &tool_calls_by_idx[&0];
    assert_eq!(tc0.0, "call_1");
    assert_eq!(tc0.1, "tool_a");
    assert_eq!(
        tc0.2, "{\"a\":1}",
        "tool_a arguments should be reassembled from fragments"
    );
    let tc1 = &tool_calls_by_idx[&1];
    assert_eq!(tc1.0, "call_2");
    assert_eq!(tc1.1, "tool_b");
    assert_eq!(
        tc1.2, "{\"b\":2}",
        "tool_b arguments should be reassembled from fragments"
    );
    assert_eq!(finish_reasons, vec!["tool_calls".to_string()]);
    assert!(text.contains("[DONE]"));
}

#[tokio::test]
async fn chat_streaming_content_then_tool_call_keeps_finish_reason_terminal() {
    let ctx = setup().await;
    let req = Request::builder()
        .method("POST")
        .uri("/v1/chat/completions")
        .header(CONTENT_TYPE, "application/json")
        .header(AUTHORIZATION, ctx.auth_header.clone())
        .body(Body::from(
            json!({
                "model":"gpt-5-mini-chat",
                "messages":[{"role":"user","content":"stream tool"}],
                "tools":[{ "type":"function","function":{ "name":"tool_a","parameters":{ "type":"object","additionalProperties":true }}}],
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

    let mut saw_content = false;
    let mut saw_tool_calls = false;
    let mut terminal_count = 0usize;
    let mut tool_call_seen_before_terminal = false;

    for line in text.lines() {
        let Some(payload) = line.strip_prefix("data: ") else {
            continue;
        };
        if payload == "[DONE]" {
            continue;
        }
        let Ok(v) = serde_json::from_str::<Value>(payload) else {
            continue;
        };
        let choice = v
            .get("choices")
            .and_then(|v| v.as_array())
            .and_then(|arr| arr.first())
            .cloned()
            .unwrap_or(Value::Null);
        let delta = choice.get("delta").cloned().unwrap_or(Value::Null);

        if delta.get("content").and_then(|v| v.as_str()) == Some("answer") {
            saw_content = true;
        }
        if delta.get("tool_calls").and_then(|v| v.as_array()).is_some() {
            saw_tool_calls = true;
            tool_call_seen_before_terminal = true;
        }
        assert!(
            !(delta.get("content").is_some() && delta.get("tool_calls").is_some()),
            "downstream chunk must not co-pack content and tool_calls: {payload}"
        );

        let finish_reason = choice.get("finish_reason").and_then(|v| v.as_str());
        if let Some(reason) = finish_reason {
            terminal_count += 1;
            assert_eq!(
                reason, "tool_calls",
                "unexpected terminal reason: {payload}"
            );
            assert!(
                tool_call_seen_before_terminal,
                "terminal finish_reason arrived before tool_call delta: {text}"
            );
            assert!(
                delta.as_object().map(|obj| obj.is_empty()).unwrap_or(false),
                "terminal finish_reason must be emitted on an empty delta: {payload}"
            );
        }
    }

    assert!(
        saw_content,
        "expected upstream content delta to survive downstream stream: {text}"
    );
    assert!(
        saw_tool_calls,
        "expected downstream tool_call delta: {text}"
    );
    assert_eq!(
        terminal_count, 1,
        "expected exactly one terminal finish_reason chunk: {text}"
    );
    assert!(text.contains("[DONE]"));
}

#[tokio::test]
async fn chat_streaming_content_array_tool_call_keeps_tool_loop_alive() {
    let ctx = setup().await;
    let req = Request::builder()
        .method("POST")
        .uri("/v1/chat/completions")
        .header(CONTENT_TYPE, "application/json")
        .header(AUTHORIZATION, ctx.auth_header.clone())
        .body(Body::from(
            json!({
                "model":"gpt-5-mini-chat",
                "messages":[{"role":"user","content":"stream tool"}],
                "tools":[{ "type":"function","function":{ "name":"tool_a","parameters":{ "type":"object","additionalProperties":true }}}],
                "stream": true,
                "stream_mode": "content_array_tool"
            })
            .to_string(),
        ))
        .unwrap();
    let resp = ctx.router.clone().oneshot(req).await.unwrap();
    assert_eq!(resp.status(), StatusCode::OK);
    let bytes = resp.into_body().collect().await.unwrap().to_bytes();
    let text = String::from_utf8_lossy(&bytes).to_string();

    let mut saw_content = false;
    let mut tool_name = String::new();
    let mut tool_args = String::new();
    let mut finish_reasons: Vec<String> = Vec::new();

    for line in text.lines() {
        let Some(payload) = line.strip_prefix("data: ") else {
            continue;
        };
        if payload == "[DONE]" {
            continue;
        }
        let Ok(v) = serde_json::from_str::<Value>(payload) else {
            continue;
        };
        let choice = v
            .get("choices")
            .and_then(|v| v.as_array())
            .and_then(|arr| arr.first())
            .cloned()
            .unwrap_or(Value::Null);
        let delta = choice.get("delta").cloned().unwrap_or(Value::Null);

        if delta.get("content").and_then(|v| v.as_str()) == Some("answer") {
            saw_content = true;
        }
        if let Some(reason) = choice.get("finish_reason").and_then(|v| v.as_str()) {
            if !reason.is_empty() {
                finish_reasons.push(reason.to_string());
            }
        }
        if let Some(tool_calls) = delta.get("tool_calls").and_then(|v| v.as_array()) {
            for tool_call in tool_calls {
                if let Some(name) = tool_call
                    .get("function")
                    .and_then(|v| v.get("name"))
                    .and_then(|v| v.as_str())
                {
                    tool_name = name.to_string();
                }
                if let Some(arguments) = tool_call
                    .get("function")
                    .and_then(|v| v.get("arguments"))
                    .and_then(|v| v.as_str())
                {
                    tool_args.push_str(arguments);
                }
            }
        }
    }

    assert!(
        saw_content,
        "expected content delta before tool call: {text}"
    );
    assert_eq!(tool_name, "tool_a", "expected decoded tool name: {text}");
    assert_eq!(
        tool_args, "{\"a\":1}",
        "expected reassembled tool arguments: {text}"
    );
    assert_eq!(
        finish_reasons,
        vec!["tool_calls".to_string()],
        "terminal finish_reason should normalize to tool_calls for content-array tool blocks: {text}"
    );
    assert!(text.contains("[DONE]"));
}

#[tokio::test]
async fn chat_streaming_content_array_tool_use_keeps_tool_loop_alive() {
    let ctx = setup().await;
    let req = Request::builder()
        .method("POST")
        .uri("/v1/chat/completions")
        .header(CONTENT_TYPE, "application/json")
        .header(AUTHORIZATION, ctx.auth_header.clone())
        .body(Body::from(
            json!({
                "model":"gpt-5-mini-chat",
                "messages":[{"role":"user","content":"stream tool"}],
                "tools":[{ "type":"function","function":{ "name":"tool_a","parameters":{ "type":"object","additionalProperties":true }}}],
                "stream": true,
                "stream_mode": "content_array_tool_use"
            })
            .to_string(),
        ))
        .unwrap();
    let resp = ctx.router.clone().oneshot(req).await.unwrap();
    assert_eq!(resp.status(), StatusCode::OK);
    let bytes = resp.into_body().collect().await.unwrap().to_bytes();
    let text = String::from_utf8_lossy(&bytes).to_string();

    let mut saw_content = false;
    let mut tool_name = String::new();
    let mut tool_args = String::new();
    let mut finish_reasons: Vec<String> = Vec::new();

    for line in text.lines() {
        let Some(payload) = line.strip_prefix("data: ") else {
            continue;
        };
        if payload == "[DONE]" {
            continue;
        }
        let Ok(v) = serde_json::from_str::<Value>(payload) else {
            continue;
        };
        let choice = v
            .get("choices")
            .and_then(|v| v.as_array())
            .and_then(|arr| arr.first())
            .cloned()
            .unwrap_or(Value::Null);
        let delta = choice.get("delta").cloned().unwrap_or(Value::Null);

        if delta.get("content").and_then(|v| v.as_str()) == Some("answer") {
            saw_content = true;
        }
        if let Some(reason) = choice.get("finish_reason").and_then(|v| v.as_str()) {
            if !reason.is_empty() {
                finish_reasons.push(reason.to_string());
            }
        }
        if let Some(tool_calls) = delta.get("tool_calls").and_then(|v| v.as_array()) {
            for tool_call in tool_calls {
                if let Some(name) = tool_call
                    .get("function")
                    .and_then(|v| v.get("name"))
                    .and_then(|v| v.as_str())
                {
                    tool_name = name.to_string();
                }
                if let Some(arguments) = tool_call
                    .get("function")
                    .and_then(|v| v.get("arguments"))
                    .and_then(|v| v.as_str())
                {
                    tool_args.push_str(arguments);
                }
            }
        }
    }

    assert!(
        saw_content,
        "expected content delta before tool call: {text}"
    );
    assert_eq!(tool_name, "tool_a", "expected decoded tool name: {text}");
    assert_eq!(
        tool_args, "{\"a\":1}",
        "expected reassembled tool arguments: {text}"
    );
    assert_eq!(
        finish_reasons,
        vec!["tool_calls".to_string()],
        "terminal finish_reason should normalize to tool_calls for content-array tool_use blocks: {text}"
    );
    assert!(text.contains("[DONE]"));
}

#[tokio::test]
async fn chat_streaming_maps_text_from_responses_output_item_done() {
    let ctx = setup().await;
    let req = Request::builder()
        .method("POST")
        .uri("/v1/chat/completions")
        .header(CONTENT_TYPE, "application/json")
        .header(AUTHORIZATION, ctx.auth_header.clone())
        .body(Body::from(
            json!({
                "model":"gpt-5-mini",
                "messages":[{"role":"user","content":"stream plain"}],
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
    assert!(text.contains("\"content\""));
    assert!(text.contains("[DONE]"));
}

#[tokio::test]
async fn chat_streaming_from_responses_includes_terminal_usage() {
    let ctx = setup().await;
    let req = Request::builder()
        .method("POST")
        .uri("/v1/chat/completions")
        .header(CONTENT_TYPE, "application/json")
        .header(AUTHORIZATION, ctx.auth_header.clone())
        .body(Body::from(
            json!({
                "model":"gpt-5-mini",
                "messages":[{"role":"user","content":"stream usage"}],
                "stream": true,
                "emit_usage": true
            })
            .to_string(),
        ))
        .unwrap();
    let resp = ctx.router.clone().oneshot(req).await.unwrap();
    assert_eq!(resp.status(), StatusCode::OK);
    let bytes = resp.into_body().collect().await.unwrap().to_bytes();
    let text = String::from_utf8_lossy(&bytes).to_string();

    let mut usage_chunks = Vec::new();
    for line in text.lines() {
        let Some(payload) = line.strip_prefix("data: ") else {
            continue;
        };
        if payload == "[DONE]" {
            continue;
        }
        let Ok(v) = serde_json::from_str::<Value>(payload) else {
            continue;
        };
        if v.get("usage").is_some() {
            usage_chunks.push(v);
        }
    }

    assert_eq!(usage_chunks.len(), 1, "expected one usage chunk: {text}");
    let usage_chunk = &usage_chunks[0];
    assert_eq!(usage_chunk["choices"], json!([]));
    assert_eq!(usage_chunk["usage"]["prompt_tokens"].as_u64(), Some(12));
    assert_eq!(usage_chunk["usage"]["completion_tokens"].as_u64(), Some(8));
}

#[tokio::test]
async fn chat_streaming_openrouter_final_usage_chunk_shape() {
    let ctx = setup().await;
    let req = Request::builder()
        .method("POST")
        .uri("/v1/chat/completions")
        .header(CONTENT_TYPE, "application/json")
        .header(AUTHORIZATION, ctx.auth_header.clone())
        .body(Body::from(
            json!({
                "model":"gpt-5-mini",
                "messages":[{"role":"user","content":"stream usage"}],
                "stream": true,
                "emit_usage": true
            })
            .to_string(),
        ))
        .unwrap();
    let resp = ctx.router.clone().oneshot(req).await.unwrap();
    assert_eq!(resp.status(), StatusCode::OK);
    let bytes = resp.into_body().collect().await.unwrap().to_bytes();
    let text = String::from_utf8_lossy(&bytes).to_string();

    let payloads = text
        .lines()
        .filter_map(|line| line.strip_prefix("data: "))
        .collect::<Vec<_>>();
    let done_index = payloads
        .iter()
        .position(|payload| *payload == "[DONE]")
        .expect("chat stream done sentinel");
    let chunks = payloads
        .iter()
        .enumerate()
        .filter(|(_, payload)| **payload != "[DONE]")
        .filter_map(|(index, payload)| {
            serde_json::from_str::<Value>(payload)
                .ok()
                .map(|chunk| (index, chunk))
        })
        .collect::<Vec<_>>();
    let finish_chunks = chunks
        .iter()
        .filter(|(_, chunk)| {
            chunk["choices"]
                .as_array()
                .and_then(|choices| choices.first())
                .and_then(|choice| choice["finish_reason"].as_str())
                .is_some()
        })
        .collect::<Vec<_>>();
    let usage_chunks = chunks
        .iter()
        .filter(|(_, chunk)| chunk.get("usage").is_some())
        .collect::<Vec<_>>();

    assert_eq!(
        usage_chunks.len(),
        1,
        "chat stream must emit exactly one usage-bearing terminal chunk before [DONE]: {text}"
    );
    assert_eq!(
        finish_chunks.len(),
        1,
        "chat stream must emit one finish chunk: {text}"
    );
    let finish_index = finish_chunks[0].0;
    let finish_chunk = &finish_chunks[0].1;
    let usage_index = usage_chunks[0].0;
    let usage_chunk = &usage_chunks[0].1;
    assert_eq!(usage_index, finish_index + 1);
    assert_eq!(done_index, usage_index + 1);
    assert_eq!(finish_chunk["usage"], Value::Null);
    assert_eq!(usage_chunk["choices"], json!([]));
    for field in ["id", "object", "created", "model"] {
        assert_eq!(
            usage_chunk[field], finish_chunk[field],
            "field {field}: {text}"
        );
    }
    assert_eq!(usage_chunk["usage"]["prompt_tokens"].as_u64(), Some(12));
    assert_eq!(usage_chunk["usage"]["completion_tokens"].as_u64(), Some(8));
}

#[tokio::test]
async fn chat_streaming_plaintext_reasoning_to_summary_rewrites_reasoning_events() {
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
            name: "mono-transform-summary-chat".to_string(),
            models,
            api_type_overrides: Vec::new(),
            groups: Vec::new(),
            channels: vec![monoize::monoize_routing::CreateMonoizeChannelInput {
                id: Some("mono-transform-summary-chat-ch1".to_string()),
                name: "mono-transform-summary-chat-ch1".to_string(),
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
        .uri("/v1/chat/completions")
        .header(CONTENT_TYPE, "application/json")
        .header(AUTHORIZATION, ctx.auth_header.clone())
        .body(Body::from(
            json!({
                "model":"gpt-5-mini",
                "messages":[{"role":"user","content":"stream with reasoning"}],
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

    assert!(
        text.contains("\"type\":\"reasoning.summary\""),
        "chat stream should expose reasoning summary detail: {text}"
    );
    assert!(
        text.contains("\"summary\":\"mock_reasoning\""),
        "chat stream should move plaintext reasoning into summary: {text}"
    );
    assert!(
        !text.contains("\"type\":\"reasoning.text\""),
        "chat stream should not emit reasoning.text after summary transform: {text}"
    );
}

#[tokio::test]
async fn chat_streaming_plaintext_reasoning_to_summary_preserves_encrypted_reasoning() {
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
            name: "mono-transform-summary-chat-encrypted".to_string(),
            models,
            api_type_overrides: Vec::new(),
            groups: Vec::new(),
            channels: vec![monoize::monoize_routing::CreateMonoizeChannelInput {
                id: Some("mono-transform-summary-chat-encrypted-ch1".to_string()),
                name: "mono-transform-summary-chat-encrypted-ch1".to_string(),
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
        .uri("/v1/chat/completions")
        .header(CONTENT_TYPE, "application/json")
        .header(AUTHORIZATION, ctx.auth_header.clone())
        .body(Body::from(
            json!({
                "model":"gpt-5-mini",
                "messages":[{"role":"user","content":"stream tool"}],
                "tools":[{ "type":"function","function":{ "name":"tool_a","parameters":{ "type":"object","additionalProperties":true }}}],
                "parallel_tool_calls": true,
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

    assert!(
        text.contains("\"type\":\"reasoning.summary\""),
        "chat stream should expose reasoning summary detail: {text}"
    );
    assert!(
        text.contains("\"summary\":\"mock_reasoning\""),
        "chat stream should move plaintext reasoning into summary: {text}"
    );
    assert!(
        !text.contains("\"type\":\"reasoning.text\""),
        "chat stream should not emit reasoning.text after summary transform: {text}"
    );
    assert!(
        text.contains("\"type\":\"reasoning.encrypted\""),
        "chat stream should preserve encrypted reasoning detail: {text}"
    );
    assert!(
        text.contains("\"data\":\"mz2."),
        "chat stream should wrap encrypted reasoning payload: {text}"
    );
}

#[tokio::test]
async fn chat_streaming_content_only_from_chat_upstream_has_terminal_chunk_and_usage() {
    let ctx = setup().await;
    let req = Request::builder()
        .method("POST")
        .uri("/v1/chat/completions")
        .header(CONTENT_TYPE, "application/json")
        .header(AUTHORIZATION, ctx.auth_header.clone())
        .body(Body::from(
            json!({
                "model":"gpt-5-mini-chat",
                "messages":[{"role":"user","content":"hello"}],
                "stream": true
            })
            .to_string(),
        ))
        .unwrap();
    let resp = ctx.router.clone().oneshot(req).await.unwrap();
    assert_eq!(resp.status(), StatusCode::OK);
    let bytes = resp.into_body().collect().await.unwrap().to_bytes();
    let text = String::from_utf8_lossy(&bytes).to_string();

    let mut finish_reasons: Vec<String> = Vec::new();
    let mut has_usage = false;
    let mut has_content = false;
    for line in text.lines() {
        let Some(payload) = line.strip_prefix("data: ") else {
            continue;
        };
        if payload == "[DONE]" {
            continue;
        }
        let Ok(v) = serde_json::from_str::<Value>(payload) else {
            continue;
        };
        if v.get("usage").is_some() {
            has_usage = true;
        }
        let choice = v
            .get("choices")
            .and_then(|v| v.as_array())
            .and_then(|arr| arr.first());
        if let Some(c) = choice {
            if let Some(reason) = c.get("finish_reason").and_then(|v| v.as_str()) {
                if !reason.is_empty() {
                    finish_reasons.push(reason.to_string());
                }
            }
            if c.get("delta")
                .and_then(|d| d.get("content"))
                .and_then(|v| v.as_str())
                .is_some()
            {
                has_content = true;
            }
        }
    }

    assert!(has_content, "should have content deltas: {text}");
    assert_eq!(
        finish_reasons,
        vec!["stop".to_string()],
        "content-only chat stream must have exactly one terminal finish_reason=stop: {text}"
    );
    assert!(
        has_usage,
        "PC9: usage must be present via auto-injected stream_options.include_usage: {text}"
    );
    assert!(text.contains("[DONE]"), "must end with [DONE]: {text}");
}

#[tokio::test]
async fn chat_streaming_header_only_tool_call_still_finishes_as_tool_calls() {
    let ctx = setup().await;
    let req = Request::builder()
        .method("POST")
        .uri("/v1/chat/completions")
        .header(CONTENT_TYPE, "application/json")
        .header(AUTHORIZATION, ctx.auth_header.clone())
        .body(Body::from(
            json!({
                "model":"gpt-5-mini-chat",
                "messages":[{"role":"user","content":"stream tool"}],
                "tools":[{ "type":"function","function":{ "name":"tool_empty","parameters":{ "type":"object","additionalProperties":true }}}],
                "stream": true,
                "stream_mode": "header_only_tool"
            })
            .to_string(),
        ))
        .unwrap();
    let resp = ctx.router.clone().oneshot(req).await.unwrap();
    assert_eq!(resp.status(), StatusCode::OK);
    let bytes = resp.into_body().collect().await.unwrap().to_bytes();
    let text = String::from_utf8_lossy(&bytes).to_string();

    let mut finish_reasons = Vec::new();
    let mut saw_tool_call_header = false;
    for line in text.lines() {
        let Some(payload) = line.strip_prefix("data: ") else {
            continue;
        };
        if payload == "[DONE]" {
            continue;
        }
        let Ok(v) = serde_json::from_str::<Value>(payload) else {
            continue;
        };
        if v.get("choices")
            .and_then(|v| v.as_array())
            .and_then(|arr| arr.first())
            .and_then(|choice| choice.get("delta"))
            .and_then(|delta| delta.get("tool_calls"))
            .and_then(|v| v.as_array())
            .is_some()
        {
            saw_tool_call_header = true;
        }
        if let Some(reason) = v
            .get("choices")
            .and_then(|v| v.as_array())
            .and_then(|arr| arr.first())
            .and_then(|choice| choice.get("finish_reason"))
            .and_then(|v| v.as_str())
        {
            if !reason.is_empty() {
                finish_reasons.push(reason.to_string());
            }
        }
    }

    assert!(
        saw_tool_call_header,
        "expected downstream tool call header chunk: {text}"
    );
    assert_eq!(
        finish_reasons,
        vec!["tool_calls".to_string()],
        "header-only tool call must still normalize terminal finish reason to tool_calls: {text}"
    );
    assert!(text.contains("[DONE]"));
}

#[tokio::test]
async fn chat_streaming_prestream_upstream_error_returns_error_stream() {
    let ctx = setup().await;
    let req = Request::builder()
        .method("POST")
        .uri("/v1/chat/completions")
        .header(CONTENT_TYPE, "application/json")
        .header(AUTHORIZATION, ctx.auth_header.clone())
        .body(Body::from(
            json!({
                "model":"gpt-5-mini-chat",
                "messages":[{"role":"user","content":"blocked"}],
                "stream": true,
                "force_upstream_error_status": 400,
                "force_upstream_error_code": "cyber_policy",
                "force_upstream_error_message": "mock cybersecurity policy block"
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
        "chat pre-stream error must emit exactly one [DONE]: {text}"
    );

    let error = parse_sse_frames(&text)
        .into_iter()
        .filter(|(_, data)| data != "[DONE]")
        .filter_map(|(_, data)| serde_json::from_str::<Value>(&data).ok())
        .find(|payload| payload.get("error").is_some())
        .expect("chat error frame");
    assert_eq!(error["error"]["code"].as_str(), Some("cyber_policy"));
    assert!(
        error["error"]["message"]
            .as_str()
            .unwrap_or("")
            .contains("mock cybersecurity policy block"),
        "error message should expose upstream detail: {text}"
    );
}

async fn chat_stream_for_mode(ctx: &TestContext, stream_mode: &str) -> String {
    let req = Request::builder()
        .method("POST")
        .uri("/v1/chat/completions")
        .header(CONTENT_TYPE, "application/json")
        .header(AUTHORIZATION, ctx.auth_header.clone())
        .body(Body::from(
            json!({
                "model": "gpt-5-mini-chat",
                "messages": [{ "role": "user", "content": "terminal semantics" }],
                "stream": true,
                "stream_mode": stream_mode
            })
            .to_string(),
        ))
        .unwrap();
    let resp = ctx.router.clone().oneshot(req).await.unwrap();
    assert_eq!(resp.status(), StatusCode::OK);
    let bytes = resp.into_body().collect().await.unwrap().to_bytes();
    String::from_utf8_lossy(&bytes).to_string()
}

#[tokio::test]
async fn chat_streaming_openrouter_error_chunks_are_terminal_and_preserved() {
    let ctx = setup().await;
    for (mode, expected_message, expected_code) in [
        ("chat_top_level_error", "openrouter top-level failure", 503),
        ("chat_choice_error", "openrouter choice failure", 502),
    ] {
        let text = chat_stream_for_mode(&ctx, mode).await;
        assert_eq!(count_done_sentinels(&text), 1, "{mode}: {text}");
        assert!(text.contains(expected_message), "{mode}: {text}");
        assert!(
            text.contains(&format!("\"code\":{expected_code}")),
            "{mode}: {text}"
        );
        assert!(
            !text.contains("\"finish_reason\":\"stop\""),
            "error stream must not synthesize success for {mode}: {text}"
        );
        if mode == "chat_choice_error" {
            assert!(
                text.contains("\"native_finish_reason\":\"error\"")
                    && text.contains("\"provider_marker\":\"openrouter\""),
                "choice-local error fields must survive same-family streaming: {text}"
            );
        }
    }

    let metadata_error = chat_stream_for_mode(&ctx, "chat_metadata_error").await;
    assert_eq!(count_done_sentinels(&metadata_error), 1, "{metadata_error}");
    assert!(metadata_error.contains("openrouter metadata failure"));
    assert!(
        metadata_error.contains("\"code\":\"P529\""),
        "{metadata_error}"
    );
    assert!(
        metadata_error.contains("provider_error"),
        "{metadata_error}"
    );
}

#[tokio::test]
async fn chat_streaming_preserves_choice_logprobs_on_nonterminal_frame() {
    let ctx = setup().await;
    let text = chat_stream_for_mode(&ctx, "chat_token_logprobs").await;
    let logprobs_frame = parse_sse_frames(&text)
        .into_iter()
        .filter_map(|(_, data)| serde_json::from_str::<Value>(&data).ok())
        .find(|frame| frame["choices"][0].get("logprobs").is_some())
        .expect("choice-level logprobs frame");

    assert_eq!(
        logprobs_frame["choices"][0]["logprobs"]["content"][0]["token"],
        json!("A")
    );
    assert_eq!(logprobs_frame["choices"][0]["delta"], json!({}));
    assert_eq!(logprobs_frame["choices"][0]["finish_reason"], Value::Null);
    assert_eq!(count_done_sentinels(&text), 1, "{text}");
}

#[tokio::test]
async fn chat_streaming_invalid_or_premature_end_emits_error_not_stop() {
    let ctx = setup().await;
    for mode in [
        "chat_malformed_json",
        "chat_done_before_terminal",
        "chat_eof_before_terminal",
    ] {
        let text = chat_stream_for_mode(&ctx, mode).await;
        assert_eq!(count_done_sentinels(&text), 1, "{mode}: {text}");
        assert!(
            text.contains("\"error\":"),
            "{mode} must emit a canonical Chat error: {text}"
        );
        assert!(
            !text.contains("\"finish_reason\":\"stop\""),
            "{mode} must not synthesize success: {text}"
        );
    }
}

#[tokio::test]
async fn chat_streaming_deepseek_resource_finish_reason_round_trips() {
    let ctx = setup().await;
    let text = chat_stream_for_mode(&ctx, "chat_insufficient_system_resource").await;
    assert_eq!(count_done_sentinels(&text), 1, "{text}");

    let terminal = parse_sse_frames(&text)
        .into_iter()
        .filter(|(_, data)| data != "[DONE]")
        .filter_map(|(_, data)| serde_json::from_str::<Value>(&data).ok())
        .find(|payload| {
            payload["choices"][0]["finish_reason"].as_str() == Some("insufficient_system_resource")
        })
        .expect("resource-exhausted terminal chunk");
    assert_eq!(
        terminal["choices"][0]["native_finish_reason"],
        json!("insufficient_system_resource")
    );
    assert_eq!(terminal["choices"][0]["provider_marker"], json!("deepseek"));
    assert!(terminal.get("error").is_none(), "{text}");
}
