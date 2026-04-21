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
    let frames = parse_responses_sse_json(&text);

    assert!(
        !text.contains("event: error"),
        "unexpected downstream error stream: {text}"
    );
    assert!(text.contains("event: response.completed"));

    let streamed_message_added = frames
        .iter()
        .find(|(event, payload)| {
            event == "response.output_item.added"
                && payload["item"]["type"].as_str() == Some("message")
        })
        .map(|(_, payload)| payload)
        .expect("streamed message output_item.added");
    let streamed_message_output_index = streamed_message_added["output_index"]
        .as_u64()
        .expect("streamed message output_index");
    let streamed_message_id = streamed_message_added["item"]["id"]
        .as_str()
        .expect("streamed message id")
        .to_string();

    let streamed_text_delta = frames
        .iter()
        .find(|(event, payload)| {
            event == "response.output_text.delta"
                && payload["output_index"].as_u64() == Some(streamed_message_output_index)
        })
        .map(|(_, payload)| payload)
        .expect("streamed text delta");
    assert_eq!(
        streamed_text_delta["item_id"].as_str(),
        Some(streamed_message_id.as_str()),
        "streamed text delta must reuse the streamed message item_id: {text}"
    );

    let completed = frames
        .iter()
        .find(|(event, _)| event == "response.completed")
        .map(|(_, payload)| payload)
        .expect("response.completed frame");
    let completed_output = completed["response"]["output"]
        .as_array()
        .expect("completed response output array");
    let completed_message = completed_output
        .iter()
        .find(|item| item["type"].as_str() == Some("message"))
        .expect("completed message output");

    assert_eq!(
        completed_message["id"].as_str(),
        Some(streamed_message_id.as_str()),
        "terminal replay must preserve the streamed assistant message id in second-step tool-result flows: {text}"
    );

    let output_item_done = frames
        .iter()
        .find(|(event, payload)| {
            event == "response.output_item.done"
                && payload["output_index"].as_u64() == Some(streamed_message_output_index)
                && payload["item"]["type"].as_str() == Some("message")
        })
        .map(|(_, payload)| payload)
        .expect("message output_item.done");
    assert_eq!(
        output_item_done["item"]["id"].as_str(),
        Some(streamed_message_id.as_str()),
        "message output_item.done must preserve the streamed assistant message id: {text}"
    );
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
async fn responses_streaming_completed_preserves_service_tier() {
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
                "stream": true,
                "stream_mode": "message_then_tool_then_completed",
                "tools": [{ "type": "function", "name": "tool_a", "parameters": { "type": "object", "additionalProperties": true } }],
                "service_tier": "priority"
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
        .expect("response.completed frame");
    assert_eq!(
        completed["response"]["service_tier"].as_str(),
        Some("priority")
    );
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
async fn responses_streaming_consumes_next_envelope_extra_exactly_once() {
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
                "input": [
                    {
                        "type": "message",
                        "role": "user",
                        "content": [{ "type": "input_text", "text": "first" }],
                        "first_only": "A"
                    },
                    {
                        "type": "message",
                        "role": "assistant",
                        "content": [{ "type": "output_text", "text": "second" }]
                    }
                ]
            })
            .to_string(),
        ))
        .unwrap();
    let resp = ctx.router.clone().oneshot(req).await.unwrap();
    assert_eq!(resp.status(), StatusCode::OK);
    let bytes = resp.into_body().collect().await.unwrap().to_bytes();
    let text = String::from_utf8_lossy(&bytes).to_string();
    let frames = parse_responses_sse_json(&text);

    let added_items: Vec<&Value> = frames
        .iter()
        .filter(|(event, _)| event == "response.output_item.added")
        .map(|(_, payload)| &payload["item"])
        .collect();
    assert!(
        !added_items.is_empty(),
        "expected at least one visible output item: {text}"
    );
    assert_eq!(
        added_items[0]["first_only"],
        json!("A"),
        "control-node metadata must land on the next output item envelope: {text}"
    );
    for item in added_items.iter().skip(1) {
        assert!(
            item.get("first_only").is_none(),
            "control-node metadata must be consumed exactly once: {text}"
        );
    }
    assert!(
        added_items
            .iter()
            .all(|item| item["type"].as_str() != Some("next_downstream_envelope_extra")),
        "control node must not surface as a visible Responses item: {text}"
    );

    let completed = frames
        .iter()
        .find(|(event, _)| event == "response.completed")
        .map(|(_, payload)| payload)
        .expect("response.completed frame");
    let output = completed["response"]["output"]
        .as_array()
        .expect("completed response output array");
    assert_eq!(output[0]["first_only"], json!("A"));
    for item in output.iter().skip(1) {
        assert!(item.get("first_only").is_none());
    }
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
    assert!(text.contains(
        "\"part\":{\"annotations\":[],\"logprobs\":[],\"text\":\"\",\"type\":\"output_text\"}"
    ));
    assert!(!text.contains("\"part\":{\"text\":\"\",\"type\":\"reasoning\"}"));
    assert!(!text.contains("event: response.content_part.added\ndata: {\"content_index\":2"));
}

#[tokio::test]
async fn responses_streaming_shared_message_output_adds_item_once_before_each_text_part_lifecycle()
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

    let message_added_indices: Vec<usize> = frames
        .iter()
        .enumerate()
        .filter_map(|(index, (event, payload))| {
            (event == "response.output_item.added"
                && payload["item"]["type"].as_str() == Some("message"))
            .then_some(index)
        })
        .collect();
    assert_eq!(
        message_added_indices.len(),
        1,
        "shared message output must emit response.output_item.added exactly once: {text}"
    );

    let message_add_index = message_added_indices[0];
    let message_output_index = frames[message_add_index].1["output_index"]
        .as_u64()
        .expect("message output_index");
    let message_item_id = frames[message_add_index].1["item"]["id"]
        .as_str()
        .expect("message item id")
        .to_string();

    let part_added_events: Vec<(usize, &Value)> = frames
        .iter()
        .enumerate()
        .filter_map(|(index, (event, payload))| {
            (event == "response.content_part.added"
                && payload["output_index"].as_u64() == Some(message_output_index))
            .then_some((index, payload))
        })
        .collect();
    assert!(
        !part_added_events.is_empty(),
        "message output must emit content_part.added events: {text}"
    );

    for (content_index, (part_added_index, payload)) in part_added_events.iter().enumerate() {
        assert_eq!(
            payload["item_id"].as_str(),
            Some(message_item_id.as_str()),
            "message part must reference the single visible message item id: {text}"
        );
        assert!(
            *part_added_index > message_add_index,
            "content_part.added must occur after the single message output_item.added: {text}"
        );

        let expected_content_index = content_index as u64;
        let delta_index = frames
            .iter()
            .enumerate()
            .find_map(|(index, (event, delta_payload))| {
                (event == "response.output_text.delta"
                    && delta_payload["output_index"].as_u64() == Some(message_output_index)
                    && delta_payload["content_index"].as_u64() == Some(expected_content_index))
                .then_some(index)
            })
            .expect("delta for message content part");
        assert!(
            delta_index > *part_added_index,
            "response.output_text.delta must come after response.content_part.added for content_index={expected_content_index}: {text}"
        );

        let done_index = frames
            .iter()
            .enumerate()
            .find_map(|(index, (event, done_payload))| {
                (event == "response.content_part.done"
                    && done_payload["output_index"].as_u64() == Some(message_output_index)
                    && done_payload["content_index"].as_u64() == Some(expected_content_index))
                .then_some(index)
            })
            .expect("done for message content part");
        assert!(
            done_index > delta_index,
            "response.content_part.done must come after response.output_text.delta for content_index={expected_content_index}: {text}"
        );
    }
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
    assert_eq!(text_delta.1["logprobs"], json!([]));

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

    let summary_added_count = frames
        .iter()
        .filter(|(event, _)| event == "response.reasoning_summary_part.added")
        .count();
    assert_eq!(
        summary_added_count, 1,
        "reasoning summary part must be added exactly once per reasoning item: {text}"
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
    assert!(
        frames.iter().any(|(event, payload)| {
            event == "response.output_text.delta"
                && payload["output_index"].as_u64() == Some(0)
                && payload["delta"].as_str() == Some("Searching")
        }),
        "node-authoritative message delta must survive same-family Responses streams: {text}"
    );
    assert!(
        frames.iter().any(|(event, payload)| {
            event == "response.function_call_arguments.delta"
                && payload["output_index"].as_u64() == Some(1)
                && payload["delta"].as_str() == Some("{\"a\":1}")
        }),
        "node-authoritative function-call delta must survive same-family Responses streams: {text}"
    );
}

#[tokio::test]
async fn responses_streaming_response_done_output_is_authoritative_terminal_state() {
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

    let completed = frames
        .iter()
        .find(|(event, _)| event == "response.completed")
        .map(|(_, payload)| payload)
        .expect("response.completed frame");
    let output = completed["response"]["output"]
        .as_array()
        .expect("completed response output array");
    assert_eq!(
        output.len(),
        2,
        "terminal output must contain exactly the completed message and tool call: {text}"
    );
    assert_eq!(output[0]["type"].as_str(), Some("message"));
    assert_eq!(output[0]["id"].as_str(), Some("msg_mock"));
    assert_eq!(output[0]["content"][0]["text"].as_str(), Some("Searching"));
    assert_eq!(output[1]["type"].as_str(), Some("function_call"));
    assert_eq!(output[1]["id"].as_str(), Some("fc_mock"));
    assert_eq!(output[1]["call_id"].as_str(), Some("call_1"));
    assert_eq!(output[1]["arguments"].as_str(), Some("{\"a\":1}"));

    let output_item_done: Vec<&Value> = frames
        .iter()
        .filter(|(event, _)| event == "response.output_item.done")
        .map(|(_, payload)| payload)
        .collect();
    assert_eq!(
        output_item_done.len(),
        output.len(),
        "terminal completed output must match the exact visible item lifecycle cardinality: {text}"
    );

    for (index, item) in output.iter().enumerate() {
        let done = output_item_done
            .iter()
            .find(|payload| payload["output_index"].as_u64() == Some(index as u64))
            .expect("matching output_item.done");

        assert_eq!(
            done["item"]["type"], item["type"],
            "response.completed.output must preserve terminal item type for output_index={index}: {text}"
        );
        match item["type"].as_str() {
            Some("message") => {
                assert_eq!(
                    done["item"]["id"], item["id"],
                    "terminal message id must match for output_index={index}: {text}"
                );
                assert_eq!(
                    done["item"]["role"], item["role"],
                    "terminal message role must match for output_index={index}: {text}"
                );
                assert_eq!(
                    done["item"]["content"].as_array().map(Vec::len),
                    item["content"].as_array().map(Vec::len),
                    "terminal message content cardinality must match for output_index={index}: {text}"
                );
                assert_eq!(
                    done["item"]["content"][0]["text"].as_str(),
                    item["content"][0]["text"].as_str(),
                    "terminal message text must match for output_index={index}: {text}"
                );
            }
            Some("function_call") => {
                assert_eq!(
                    done["item"]["id"], item["id"],
                    "terminal tool id must match for output_index={index}: {text}"
                );
                assert_eq!(
                    done["item"]["call_id"], item["call_id"],
                    "terminal tool call_id must match for output_index={index}: {text}"
                );
                assert_eq!(
                    done["item"]["name"], item["name"],
                    "terminal tool name must match for output_index={index}: {text}"
                );
                assert_eq!(
                    done["item"]["arguments"], item["arguments"],
                    "terminal tool arguments must match for output_index={index}: {text}"
                );
            }
            other => panic!("unexpected terminal item type {other:?}: {text}"),
        }
    }
}

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
        .expect("response.completed frame");
    let output = completed["response"]["output"]
        .as_array()
        .expect("completed response output array");
    assert_eq!(
        output.len(),
        3,
        "terminal output must retain reasoning, message, and function call in their streamed order: {text}"
    );
    assert_eq!(output[0]["type"].as_str(), Some("reasoning"));
    assert_eq!(output[0]["id"].as_str(), Some("rs_mock"));
    assert_eq!(output[1]["type"].as_str(), Some("message"));
    assert_eq!(output[1]["id"].as_str(), Some("msg_mock"));
    assert_eq!(output[2]["type"].as_str(), Some("function_call"));
    assert_eq!(output[2]["id"].as_str(), Some("fc_mock"));
    assert_eq!(output[2]["call_id"].as_str(), Some("call_1"));

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
        provider_type: monoize::monoize_routing::MonoizeProviderType::ChatCompletion,
        models,
        api_type_overrides: Vec::new(),
        groups: Vec::new(),
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
        provider_type: monoize::monoize_routing::MonoizeProviderType::Responses,
        models,
        api_type_overrides: Vec::new(),
        groups: Vec::new(),
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
            provider_type: monoize::monoize_routing::MonoizeProviderType::Responses,
            models,
            api_type_overrides: Vec::new(),
            groups: Vec::new(),
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
            provider_type: monoize::monoize_routing::MonoizeProviderType::Responses,
            models,
            api_type_overrides: Vec::new(),
            groups: Vec::new(),
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
