use super::*;

/// STRM-2a: an upstream stream that produced its first output event. The
/// buffered `ResponseStart` and `ProviderControl` events precede that event in
/// decoder order.
pub(super) struct CommittedUpstreamStream {
    pub(super) buffered: Vec<crate::urp::UrpStreamEvent>,
    pub(super) decoded_rx: mpsc::Receiver<crate::urp::UrpStreamEvent>,
    pub(super) decode_handle: tokio::task::JoinHandle<AppResult<()>>,
}

/// STRM-2b: an upstream stream that failed before its first output event.
pub(super) struct PreOutputStreamFailure {
    pub(super) error: AppError,
    pub(super) failure_class: Option<RetryableFailureClass>,
}

/// STRM-2a: upper bound on events held before the first output event.
const PRE_OUTPUT_BUFFER_MAX_EVENTS: usize = 256;

/// Waits for the decoder's first output event. A terminal error or stream end
/// inside the window ends the attempt so the router can fall back (STRM-2b).
/// Provider-control events never close the window and never count as the first
/// WebSocket frame for FP4h, so an upstream rejection after them keeps its
/// native status.
pub(super) async fn probe_stream_first_output(
    mut decoded_rx: mpsc::Receiver<crate::urp::UrpStreamEvent>,
    decode_handle: tokio::task::JoinHandle<AppResult<()>>,
    runtime_metrics: &Arc<Mutex<StreamRuntimeMetrics>>,
    commit: &StreamCommit,
) -> Result<CommittedUpstreamStream, PreOutputStreamFailure> {
    let mut buffered = Vec::new();
    loop {
        match decoded_rx.recv().await {
            Some(
                crate::urp::UrpStreamEvent::ProviderControl { .. }
                | crate::urp::UrpStreamEvent::ResponseStart { .. },
            ) if buffered.len() >= PRE_OUTPUT_BUFFER_MAX_EVENTS => {
                decode_handle.abort();
                return Err(PreOutputStreamFailure {
                    error: AppError::new(
                        StatusCode::BAD_GATEWAY,
                        "upstream_control_overflow",
                        "upstream sent too many control events before its first output",
                    )
                    .with_type("server_error"),
                    failure_class: Some(RetryableFailureClass::Transient),
                });
            }
            Some(event @ crate::urp::UrpStreamEvent::ProviderControl { .. }) => {
                buffered.push(event)
            }
            Some(event @ crate::urp::UrpStreamEvent::ResponseStart { .. }) => {
                commit.accept(Vec::new());
                buffered.push(event)
            }
            Some(crate::urp::UrpStreamEvent::Error {
                code,
                message,
                extra_body,
            }) => {
                let explicit_status = extra_body
                    .get("status")
                    .or_else(|| {
                        extra_body
                            .get("error")
                            .and_then(|error| error.get("status"))
                    })
                    .and_then(Value::as_u64)
                    .filter(|status| (400..=599).contains(status))
                    .and_then(|status| StatusCode::from_u16(status as u16).ok());
                let _ = decode_handle.await;
                let terminal = runtime_metrics.lock().await.terminal.terminal_error.clone();
                let http_status = terminal
                    .as_ref()
                    .map(|terminal| terminal.http_status)
                    .unwrap_or(StatusCode::BAD_GATEWAY.as_u16());
                let error_type = terminal
                    .as_ref()
                    .and_then(|terminal| terminal.error_type.clone());
                let param = terminal.and_then(|terminal| terminal.param);
                let reported_code = code.clone();
                let code = code.unwrap_or_else(|| "upstream_stream_error".to_string());
                let failure_class = midstream_terminal_failure_class(
                    http_status,
                    Some(&code),
                    error_type.as_deref(),
                );
                // `upstream_status` stays unset: the status is synthesized by the
                // decoder and must not trigger an RTA-6c shared-origin blast.
                let status = pre_output_error_status(
                    explicit_status,
                    http_status,
                    &code,
                    error_type.as_deref(),
                );
                let error = AppError::new(status, code, message).with_upstream_error(
                    None,
                    reported_code,
                    error_type,
                    param,
                );
                return Err(PreOutputStreamFailure {
                    error,
                    failure_class,
                });
            }
            Some(event) => {
                commit.accept(Vec::new());
                buffered.push(event);
                return Ok(CommittedUpstreamStream {
                    buffered,
                    decoded_rx,
                    decode_handle,
                });
            }
            None => {
                let error = match decode_handle.await {
                    Ok(Ok(())) => AppError::new(
                        StatusCode::BAD_GATEWAY,
                        "upstream_stream_error",
                        "upstream stream ended before its first output event",
                    ),
                    Ok(Err(err)) => err,
                    Err(err) => AppError::new(
                        StatusCode::INTERNAL_SERVER_ERROR,
                        "task_panic",
                        err.to_string(),
                    ),
                };
                let failure_class =
                    is_upstream_adapter_failure(&error).then_some(RetryableFailureClass::Transient);
                return Err(PreOutputStreamFailure {
                    error,
                    failure_class,
                });
            }
        }
    }
}

/// Replays the STRM-2a buffered events, then forwards the live decoder output.
pub(super) fn replay_then_forward(
    buffered: Vec<crate::urp::UrpStreamEvent>,
    mut decoded_rx: mpsc::Receiver<crate::urp::UrpStreamEvent>,
) -> mpsc::Receiver<crate::urp::UrpStreamEvent> {
    let (tx, rx) = mpsc::channel(64);
    crate::payload::spawn(async move {
        for event in buffered {
            if tx.send(event).await.is_err() {
                return;
            }
        }
        while let Some(event) = decoded_rx.recv().await {
            if tx.send(event).await.is_err() {
                return;
            }
        }
    });
    rx
}

pub(super) fn estimated_tokens_from_utf8_bytes(bytes: u64) -> u64 {
    bytes.div_ceil(4)
}

pub(super) fn decoded_visible_output_bytes(output: &[urp::Node]) -> u64 {
    output.iter().fold(0u64, |total, node| {
        let bytes = match node {
            urp::Node::Text { content, .. } | urp::Node::Refusal { content, .. } => {
                content.len() as u64
            }
            _ => 0,
        };
        total.saturating_add(bytes)
    })
}

pub(super) struct RetainedOutputContext {
    pub(super) aliases: HashMap<String, urp::ToolIdentity>,
    pub(super) foreign_target: Option<urp::ProviderProtocol>,
    pub(super) downstream: DownstreamProtocol,
    pub(super) request_id: Option<String>,
    pub(super) mask_sensitive_info: bool,
}

pub(super) async fn retain_decoded_terminal_output(
    mut rx: mpsc::Receiver<urp::UrpStreamEvent>,
    tx: mpsc::Sender<urp::UrpStreamEvent>,
    terminal_output: Arc<Mutex<Vec<urp::Node>>>,
    context: RetainedOutputContext,
) -> AppResult<()> {
    let RetainedOutputContext {
        aliases,
        foreign_target,
        downstream,
        request_id,
        mask_sensitive_info,
    } = context;
    while let Some(mut event) = rx.recv().await {
        restore_tool_namespace_event(&mut event, &aliases);
        // XTRA-5a, before any response transform. Settlement reads the decoder
        // usage from the runtime metrics, so it keeps the unstripped values.
        if let Some(target) = foreign_target
            && !urp::strip_foreign_stream_event(&mut event, target)
        {
            continue;
        }
        normalize_stream_error_event(
            &mut event,
            downstream,
            request_id.as_deref(),
            mask_sensitive_info,
        );
        if let urp::UrpStreamEvent::ResponseDone { output, .. } = &event {
            *terminal_output.lock().await = output.iter().map(|entry| entry.node.clone()).collect();
        }
        let _ = tx.send(event).await;
    }
    Ok(())
}

/// RCD-D10a (`request-capture-dumps.spec.md`): between response transforms
/// and downstream encoding, retain the terminal `response_done` event as the
/// URP non-stream reconstruction `{finish_reason?, usage?, output, ...extra}`.
/// This stage is only inserted when a capture session is active. The Image
/// API stream-collected path (RCD-D10c) reuses it between decode and response
/// transforms instead.
pub(in crate::handlers) async fn retain_reconstructed_urp_response(
    mut rx: mpsc::Receiver<urp::UrpStreamEvent>,
    tx: mpsc::Sender<urp::UrpStreamEvent>,
    reconstructed: Arc<Mutex<Option<serde_json::Value>>>,
) -> AppResult<()> {
    while let Some(event) = rx.recv().await {
        if let urp::UrpStreamEvent::ResponseDone {
            outcome,
            finish_reason,
            usage,
            output,
            extra_body,
        } = &event
        {
            let mut object = serde_json::Map::new();
            if let Some(outcome) = outcome {
                object.insert("outcome".into(), json!(outcome));
            }
            if let Some(finish_reason) = finish_reason {
                object.insert("finish_reason".to_string(), json!(finish_reason));
            }
            if let Some(usage) = usage {
                object.insert("usage".to_string(), json!(usage));
            }
            // RCD-D10a: `output` is `Node[]`; the stream-only `node_index` stays out of dumps.
            let nodes: Vec<&urp::Node> = output.iter().map(|entry| &entry.node).collect();
            object.insert("output".to_string(), json!(nodes));
            for (key, value) in extra_body {
                object.insert(key.clone(), value.clone());
            }
            *reconstructed.lock().await = Some(serde_json::Value::Object(object));
        }
        let _ = tx.send(event).await;
    }
    Ok(())
}
