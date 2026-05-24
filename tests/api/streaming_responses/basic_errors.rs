
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
async fn responses_streaming_image_generation_completed_emits_output_image() {
    let ctx = setup().await;
    let req = Request::builder()
        .method("POST")
        .uri("/v1/responses")
        .header(CONTENT_TYPE, "application/json")
        .header(AUTHORIZATION, ctx.auth_header.clone())
        .body(Body::from(
            json!({
                "model":"gpt-5-mini",
                "input":[{"type":"message","role":"user","content":[{"type":"input_text","text":"generate image"}]}],
                "tools":[{"type":"image_generation","output_format":"png"}],
                "stream": true,
                "stream_mode": "image_generation_completed"
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
        frames.iter().any(|(event, payload)| {
            event == "response.content_part.done"
                && payload["part"]["type"].as_str() == Some("output_image")
                && payload["part"]["source"]["media_type"].as_str() == Some("image/png")
        }),
        "{text}"
    );
    assert!(
        frames.iter().any(|(event, payload)| {
            event == "response.completed"
                && payload["response"]["output"]
                    .as_array()
                    .is_some_and(|output| {
                        output.iter().any(|item| {
                            item["type"].as_str() == Some("message")
                                && item["content"].as_array().is_some_and(|content| {
                                    content.iter().any(|part| {
                                        part["type"].as_str() == Some("output_image")
                                            && part["source"]["media_type"].as_str()
                                                == Some("image/png")
                                    })
                                })
                        })
                    })
        }),
        "{text}"
    );
}

#[tokio::test]
async fn responses_streaming_completed_snapshot_image_generation_emits_output_image() {
    let ctx = setup().await;
    let req = Request::builder()
        .method("POST")
        .uri("/v1/responses")
        .header(CONTENT_TYPE, "application/json")
        .header(AUTHORIZATION, ctx.auth_header.clone())
        .body(Body::from(
            json!({
                "model":"gpt-5-mini",
                "input":"generate image",
                "tools":[{"type":"image_generation","output_format":"webp"}],
                "stream": true,
                "stream_mode": "image_generation_completed_snapshot_only"
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
        frames.iter().any(|(event, payload)| {
            event == "response.completed"
                && payload["response"]["output"]
                    .as_array()
                    .is_some_and(|output| {
                        output.iter().any(|item| {
                            item["type"].as_str() == Some("message")
                                && item["content"].as_array().is_some_and(|content| {
                                    content.iter().any(|part| {
                                        part["type"].as_str() == Some("output_image")
                                            && part["source"]["media_type"].as_str()
                                                == Some("image/webp")
                                            && part["source"]["data"]
                                                .as_str()
                                                .is_some_and(|data| !data.is_empty())
                                    })
                                })
                        })
                    })
        }),
        "{text}"
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
    let frames = parse_responses_sse_json(&text);
    let reasoning_added = frames
        .iter()
        .find(|(event, payload)| {
            event == "response.output_item.added"
                && payload["item"]["type"].as_str() == Some("reasoning")
        })
        .expect("reasoning output_item.added");
    assert!(
        reasoning_added.1["item"]["encrypted_content"]
            .as_str()
            .is_some_and(|value| value.starts_with("mz2.")),
        "reasoning output_item.added must wrap encrypted_content for downstream: {text}"
    );
    assert!(text.contains("event: response.content_part.added"));
    assert!(text.contains(
        "\"part\":{\"annotations\":[],\"logprobs\":[],\"text\":\"\",\"type\":\"output_text\"}"
    ));
    assert!(!text.contains("\"part\":{\"text\":\"\",\"type\":\"reasoning\"}"));
    assert!(!text.contains("event: response.content_part.added\ndata: {\"content_index\":2"));
}
