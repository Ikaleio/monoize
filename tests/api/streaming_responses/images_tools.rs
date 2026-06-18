#[tokio::test]
async fn responses_streaming_preserves_image_generation_partial_image_events() {
    let ctx = setup().await;
    let req = Request::builder()
        .method("POST")
        .uri("/v1/responses")
        .header(CONTENT_TYPE, "application/json")
        .header(AUTHORIZATION, ctx.auth_header.clone())
        .body(Body::from(
            json!({
                "model":"gpt-5-mini",
                "input":[{"type":"message","role":"user","content":[{"type":"input_text","text":"draw"}]}],
                "tools":[{ "type":"image_generation", "partial_images": 3 }],
                "stream": true,
                "stream_mode": "image_generation_partial"
            })
            .to_string(),
        ))
        .unwrap();

    let resp = ctx.router.clone().oneshot(req).await.unwrap();
    assert_eq!(resp.status(), StatusCode::OK);
    let bytes = resp.into_body().collect().await.unwrap().to_bytes();
    let text = String::from_utf8_lossy(&bytes).to_string();
    let frames = parse_responses_sse_json(&text);

    let partial = frames
        .iter()
        .find(|(event, _)| event == "response.image_generation_call.partial_image")
        .expect("partial image event");
    assert_eq!(partial.1["item_id"].as_str(), Some("ig_mock"));
    assert_eq!(partial.1["output_index"].as_u64(), Some(0));
    assert_eq!(partial.1["partial_image_index"].as_u64(), Some(0));
    assert_eq!(partial.1["partial_image_b64"].as_str(), Some("QUJD"));
    assert_eq!(partial.1["output_format"].as_str(), Some("png"));
    assert_eq!(
        partial.1["type"].as_str(),
        Some("response.image_generation_call.partial_image")
    );
    assert!(partial.1.get("provider_event_type").is_none());
    assert!(
        frames
            .iter()
            .any(|(event, _)| event == "response.image_generation_call.in_progress")
    );
    assert!(
        frames
            .iter()
            .any(|(event, _)| event == "response.image_generation_call.generating")
    );
    assert!(
        frames
            .iter()
            .any(|(event, _)| event == "response.image_generation_call.completed")
    );
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
        .find(|(event, _)| event == "response.reasoning_text.delta")
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
            .any(|(event, _)| event == "response.reasoning_text.delta")
    );
    assert!(
        frames
            .iter()
            .any(|(event, _)| event == "response.reasoning_text.done")
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
        .find(|(event, _)| event == "response.reasoning_text.delta")
        .expect("text delta");
    assert_eq!(text_delta.1["delta"].as_str(), Some("mock_reasoning"));

    let reasoning_done = frames
        .iter()
        .find(|(event, _)| event == "response.reasoning_text.done")
        .expect("reasoning done");
    assert_eq!(reasoning_done.1["text"].as_str(), Some("mock_reasoning"));

    let output_item_done = frames
        .iter()
        .find(|(event, payload)| {
            event == "response.output_item.done"
                && payload["item"]["type"].as_str() == Some("reasoning")
        })
        .map(|(_, payload)| payload)
        .expect("reasoning output_item.done");
    assert!(
        output_item_done["item"]["duration"].as_u64().is_some(),
        "completed reasoning item must include OpenWebUI-compatible duration: {text}"
    );

    let completed = frames
        .iter()
        .find(|(event, _)| event == "response.completed")
        .map(|(_, payload)| payload)
        .expect("response.completed");
    let completed_reasoning = completed["response"]["output"]
        .as_array()
        .expect("completed output array")
        .iter()
        .find(|item| item["type"].as_str() == Some("reasoning"))
        .expect("completed reasoning output item");
    assert_eq!(
        completed_reasoning["duration"].as_u64(),
        output_item_done["item"]["duration"].as_u64(),
        "response.completed must preserve the same reasoning duration as output_item.done: {text}"
    );
}
