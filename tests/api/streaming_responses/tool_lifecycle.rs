
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
        .find(|(event, _)| event == "response.reasoning_text.delta")
        .expect("reasoning delta");
    assert_eq!(reasoning_delta.1["source"].as_str(), Some(reasoning_source));

    let reasoning_done = frames
        .iter()
        .find(|(event, _)| event == "response.reasoning_text.done")
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
        .find(|(event, _)| event == "response.reasoning_text.delta")
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
async fn responses_streaming_completed_snapshot_without_phase_does_not_duplicate_streamed_message()
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
                "input":"stream message",
                "stream": true,
                "stream_mode": "message_completed_snapshot_without_phase"
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
        1,
        "completed snapshot must not create a second added lifecycle: {text}"
    );
    assert_eq!(
        output_item_done.len(),
        1,
        "completed snapshot must not create a second done lifecycle: {text}"
    );
    assert_eq!(output_item_added[0]["output_index"].as_u64(), Some(0));
    assert_eq!(output_item_done[0]["output_index"].as_u64(), Some(0));
    assert_eq!(output_item_done[0]["item"]["id"].as_str(), Some("msg_stream"));

    let completed = frames
        .iter()
        .find(|(event, _)| event == "response.completed")
        .map(|(_, payload)| payload)
        .expect("response.completed frame");
    let output = completed["response"]["output"]
        .as_array()
        .expect("completed output array");
    assert_eq!(
        output.len(),
        1,
        "completed response output must not replay a duplicate message: {text}"
    );
    assert_eq!(output[0]["id"].as_str(), Some("msg_stream"));
    assert_eq!(output[0]["phase"].as_str(), Some("final_answer"));
    assert_eq!(output[0]["content"][0]["text"].as_str(), Some("same text"));
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
