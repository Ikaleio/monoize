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
    assert!(
        created.1["response"]
            .as_object()
            .unwrap()
            .contains_key("user"),
        "response.created response object must include user, even when null"
    );
    assert_eq!(created.1["response"]["user"], Value::Null);
    let completed = frames
        .iter()
        .find(|(event, _)| event == "response.completed")
        .expect("response.completed frame");
    assert!(
        completed.1["response"]
            .as_object()
            .unwrap()
            .contains_key("user"),
        "response.completed response object must include user, even when null"
    );
    assert_eq!(completed.1["response"]["user"], Value::Null);
    assert_eq!(
        count_done_sentinels(&text),
        1,
        "responses stream must emit exactly one [DONE]"
    );
}

#[tokio::test]
async fn responses_streaming_upstream_error_is_terminal() {
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
                "stream_mode": "error_then_completed"
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
        !text.contains("event: response.completed"),
        "downstream must not emit response.completed after an upstream error: {text}"
    );
    let failed = frames
        .iter()
        .filter(|(event, _)| event == "response.failed")
        .collect::<Vec<_>>();
    assert_eq!(
        failed.len(),
        1,
        "expected exactly one response.failed: {text}"
    );
    assert_eq!(
        failed[0].1["response"]["status"].as_str(),
        Some("failed"),
        "response.failed payload must carry failed status: {text}"
    );
    assert_eq!(
        failed[0].1["response"]["error"]["code"].as_str(),
        Some("mock_stream_error"),
        "response.failed payload must preserve upstream error code: {text}"
    );
    assert_eq!(
        failed[0].1["response"]["error"]["message"].as_str(),
        Some("mock streaming error"),
        "response.failed payload must preserve upstream error message: {text}"
    );
    assert_eq!(
        count_done_sentinels(&text),
        1,
        "responses error stream must emit exactly one [DONE]: {text}"
    );
}

#[tokio::test]
async fn responses_streaming_prestream_upstream_error_returns_error_stream() {
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
                "force_upstream_error_status": 422,
                "force_upstream_error_code": "forced_daily_limit",
                "force_upstream_error_message": "daily usage limit exceeded"
            })
            .to_string(),
        ))
        .unwrap();

    let resp = ctx.router.clone().oneshot(req).await.unwrap();
    assert_eq!(resp.status(), StatusCode::OK);
    let bytes = resp.into_body().collect().await.unwrap().to_bytes();
    let text = String::from_utf8_lossy(&bytes).to_string();
    assert!(
        text.contains("event: error"),
        "pre-stream error must be converted to SSE: {text}"
    );
    assert!(
        text.contains("[DONE]"),
        "pre-stream error stream must append an SSE sentinel: {text}"
    );
    assert_eq!(
        count_done_sentinels(&text),
        1,
        "responses pre-stream error must emit exactly one [DONE]: {text}"
    );
    let frames = parse_responses_sse_json(&text);
    let error = frames
        .iter()
        .find(|(event, _)| event == "error")
        .map(|(_, payload)| payload)
        .expect("error frame");
    assert_eq!(error["type"].as_str(), Some("error"));
    assert_eq!(error["code"].as_str(), Some("forced_daily_limit"));
    assert!(
        error["message"]
            .as_str()
            .unwrap_or("")
            .contains("daily usage limit exceeded"),
        "error message should expose upstream detail: {text}"
    );
}

#[tokio::test]
async fn responses_streaming_preserves_upstream_response_failure_error() {
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
                "stream_mode": "error_then_failed"
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
        !text.contains("event: response.completed"),
        "downstream must not convert upstream failure into response.completed: {text}"
    );
    let failed = frames
        .iter()
        .filter(|(event, _)| event == "response.failed")
        .collect::<Vec<_>>();
    assert_eq!(
        failed.len(),
        1,
        "expected exactly one response.failed: {text}"
    );
    let error = &failed[0].1["response"]["error"];
    assert_eq!(error["type"].as_str(), Some("invalid_request_error"));
    assert_eq!(error["code"].as_str(), Some("context_length_exceeded"));
    assert_eq!(error["param"].as_str(), Some("input"));
    assert_eq!(
        error["message"].as_str(),
        Some("mock context length exceeded")
    );
    assert_eq!(
        count_done_sentinels(&text),
        1,
        "responses failure stream must emit exactly one [DONE]: {text}"
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
