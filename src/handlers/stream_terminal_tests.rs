use super::*;
use axum::response::IntoResponse;

const TARGETS: [DownstreamProtocol; 3] = [
    DownstreamProtocol::Responses,
    DownstreamProtocol::ChatCompletions,
    DownstreamProtocol::AnthropicMessages,
];

fn media_response() -> urp::UrpResponse {
    urp::decode::openai_responses::decode_response(&json!({
        "id": "resp_media", "object": "response", "model": "test-model", "status": "completed",
        "output": [{"type": "message", "role": "assistant", "content": [
            {"type": "output_image", "url": "https://example.com/image.png"}
        ]}]
    }))
    .unwrap()
}

async fn collect_wire(mut rx: mpsc::Receiver<Event>) -> String {
    let mut frames = Vec::new();
    while let Some(event) = rx.recv().await {
        frames.push(Ok::<_, std::convert::Infallible>(event));
    }
    let body = axum::response::Sse::new(futures_util::stream::iter(frames))
        .into_response()
        .into_body();
    String::from_utf8(
        axum::body::to_bytes(body, usize::MAX)
            .await
            .unwrap()
            .to_vec(),
    )
    .unwrap()
}

fn assert_single_terminal_error(wire: &str, downstream: DownstreamProtocol) {
    let data: Vec<&str> = wire
        .lines()
        .filter_map(|line| line.strip_prefix("data: "))
        .collect();
    let errors: Vec<usize> = data
        .iter()
        .enumerate()
        .filter_map(|(index, value)| {
            let value: Value = serde_json::from_str(value).ok()?;
            (value.get("error").is_some()
                || value["type"] == "error"
                || value["type"] == "response.failed")
                .then_some(index)
        })
        .collect();
    assert_eq!(errors.len(), 1, "{wire}");
    let has_done = !matches!(downstream, DownstreamProtocol::AnthropicMessages);
    assert_eq!(
        data.iter().filter(|data| **data == "[DONE]").count(),
        usize::from(has_done),
        "{wire}"
    );
    assert_eq!(
        errors[0] + 1 + usize::from(has_done),
        data.len(),
        "frames after terminal error: {wire}"
    );
    if has_done {
        assert_eq!(data.last(), Some(&"[DONE]"));
    }
    assert!(
        !wire.contains("response.completed") && !wire.contains("message_stop"),
        "{wire}"
    );
    assert!(!wire.contains("downstream_stream_terminal_sent"));
}

async fn exercise_media_failure(synthetic: bool) {
    for downstream in TARGETS {
        let response = media_response();
        let (tx, rx) = mpsc::channel(1024);
        let result = if synthetic {
            urp::stream_encode::emit_synthetic_stream_from_urp_response(
                downstream,
                "test-model",
                &response,
                None,
                None,
                tx.clone(),
            )
            .await
        } else {
            let (event_tx, event_rx) = mpsc::channel(16);
            event_tx
                .send(urp::UrpStreamEvent::NodeDone {
                    node_index: 0,
                    node: response.output[0].clone(),
                    usage: None,
                    extra_body: HashMap::new(),
                })
                .await
                .unwrap();
            event_tx
                .send(urp::UrpStreamEvent::ResponseDone {
                    finish_reason: response.finish_reason,
                    usage: response.usage.clone(),
                    output: response.output.clone(),
                    extra_body: HashMap::new(),
                })
                .await
                .unwrap();
            drop(event_tx);
            encode_urp_stream(
                downstream,
                event_rx,
                tx.clone(),
                "test-model",
                Instant::now(),
                None,
                false,
            )
            .await
        };
        let err = result.unwrap_err();
        assert!(err.downstream_stream_terminal_sent);
        let original_code = err.code.clone();
        let original_message = err.message.clone();
        let err = combine_stream_stage_results([
            Err(AppError::new(
                StatusCode::BAD_GATEWAY,
                "stream_transform_failed",
                "receiver closed",
            )),
            Ok(()),
            Ok(()),
            Err(err),
        ])
        .unwrap_err();
        assert_eq!(err.code, original_code);
        assert_eq!(err.message, original_message);
        assert!(err.downstream_stream_terminal_sent);
        let capture = crate::request_capture::SseFrameCapture::new();
        let capture_before = format!("{:?}", capture.snapshot().await);
        emit_stream_error_if_needed(downstream, &err, &tx, Some(&capture)).await;
        assert_eq!(format!("{:?}", capture.snapshot().await), capture_before);
        drop(tx);
        assert_single_terminal_error(&collect_wire(rx).await, downstream);
    }
}

#[tokio::test]
async fn synthetic_media_failure_wrapper_does_not_repeat_terminal_frames() {
    exercise_media_failure(true).await;
}

#[tokio::test]
async fn live_media_failure_wrapper_preserves_encoder_error_and_terminal_frames() {
    exercise_media_failure(false).await;
}

#[tokio::test]
async fn unmarked_transport_failure_emits_one_target_error_sequence() {
    for downstream in TARGETS {
        let err = AppError::new(
            StatusCode::BAD_GATEWAY,
            "upstream_transport_failed",
            "connection interrupted",
        );
        assert!(!err.downstream_stream_terminal_sent);
        let (tx, rx) = mpsc::channel(16);
        let capture = crate::request_capture::SseFrameCapture::new();
        emit_stream_error_if_needed(downstream, &err, &tx, Some(&capture)).await;
        drop(tx);
        let wire = collect_wire(rx).await;
        assert_single_terminal_error(&wire, downstream);
        assert!(
            wire.contains("upstream_transport_failed") && wire.contains("connection interrupted")
        );
    }
}

#[test]
fn ordinary_stage_errors_keep_the_original_stage_order() {
    let err = combine_stream_stage_results([
        Err(AppError::new(
            StatusCode::BAD_GATEWAY,
            "decode_failed",
            "decode failed",
        )),
        Ok(()),
        Ok(()),
        Err(AppError::new(
            StatusCode::BAD_GATEWAY,
            "encode_failed",
            "encode failed",
        )),
    ])
    .unwrap_err();
    assert_eq!(err.code, "decode_failed");
    assert!(!err.downstream_stream_terminal_sent);
    assert!(combine_stream_stage_results([Ok(()), Ok(()), Ok(()), Ok(())]).is_ok());
}

#[tokio::test]
async fn failed_terminal_send_is_not_marked_as_sent() {
    for downstream in TARGETS {
        let (tx, rx) = mpsc::channel(1);
        drop(rx);
        let err = urp::stream_encode::emit_synthetic_stream_from_urp_response(
            downstream,
            "test-model",
            &media_response(),
            None,
            None,
            tx,
        )
        .await
        .unwrap_err();
        assert!(!err.downstream_stream_terminal_sent);
    }
}

#[tokio::test]
async fn gemini_stream_failures_mark_the_emitted_terminal_error() {
    for conflicting_terminal in [false, true] {
        let (event_tx, event_rx) = mpsc::channel(16);
        if conflicting_terminal {
            event_tx
                .send(urp::UrpStreamEvent::NodeDone {
                    node_index: 0,
                    node: serde_json::from_value(
                        json!({"type":"text","role":"assistant","content":"before"}),
                    )
                    .unwrap(),
                    usage: None,
                    extra_body: HashMap::new(),
                })
                .await
                .unwrap();
            event_tx
                .send(urp::UrpStreamEvent::ResponseDone {
                    finish_reason: Some(urp::FinishReason::Stop),
                    usage: None,
                    output: vec![],
                    extra_body: HashMap::new(),
                })
                .await
                .unwrap();
        }
        drop(event_tx);
        let (tx, rx) = mpsc::channel(16);
        let err =
            urp::stream_encode::gemini::encode_urp_stream_as_gemini(event_rx, tx, "test-model")
                .await
                .unwrap_err();
        assert!(err.downstream_stream_terminal_sent);
        assert_eq!(err.code, "stream_encode_failed");
        assert_eq!(
            err.message,
            if conflicting_terminal {
                "Gemini cannot delete a Part that was already emitted"
            } else {
                "Gemini stream has no canonical terminal event"
            }
        );
        let wire = collect_wire(rx).await;
        let values: Vec<Value> = wire
            .lines()
            .filter_map(|line| line.strip_prefix("data: "))
            .map(|data| serde_json::from_str(data).unwrap())
            .collect();
        assert_eq!(
            values
                .iter()
                .filter(|value| value.get("error").is_some())
                .count(),
            1
        );
        assert!(values.last().unwrap().get("error").is_some());
        assert!(!wire.contains("[DONE]") && !wire.contains("downstream_stream_terminal_sent"));
    }
}
