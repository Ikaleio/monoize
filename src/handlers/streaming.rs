use super::*;
use crate::urp::stream_decode::stream_upstream_to_urp_events;
use crate::urp::stream_encode::encode_urp_stream;
use futures_util::StreamExt;

enum StreamUpstreamSource {
    Http(reqwest::Response),
    ResponsesWebSocket(Box<dyn crate::upstream_websocket::ResponsesWsSession>),
}

type ForwardEventStream = futures_util::stream::Map<
    tokio_stream::wrappers::ReceiverStream<Event>,
    fn(Event) -> Result<Event, std::convert::Infallible>,
>;

fn event_ok(event: Event) -> Result<Event, std::convert::Infallible> {
    Ok(event)
}

fn receiver_event_stream(rx: mpsc::Receiver<Event>) -> ForwardEventStream {
    tokio_stream::wrappers::ReceiverStream::new(rx)
        .map(event_ok as fn(Event) -> Result<Event, std::convert::Infallible>)
}

/// STRM-2a: an upstream stream that produced its first output event. The
/// buffered `ResponseStart` events precede that event in decoder order.
struct CommittedUpstreamStream {
    buffered: Vec<crate::urp::UrpStreamEvent>,
    decoded_rx: mpsc::Receiver<crate::urp::UrpStreamEvent>,
    decode_handle: tokio::task::JoinHandle<AppResult<()>>,
}

/// STRM-2b: an upstream stream that failed before its first output event.
struct PreOutputStreamFailure {
    error: AppError,
    failure_class: Option<RetryableFailureClass>,
}

/// Waits for the decoder's first output event. A terminal error or stream end
/// inside the window ends the attempt so the router can fall back (STRM-2b).
async fn probe_stream_first_output(
    mut decoded_rx: mpsc::Receiver<crate::urp::UrpStreamEvent>,
    decode_handle: tokio::task::JoinHandle<AppResult<()>>,
    runtime_metrics: &Arc<Mutex<StreamRuntimeMetrics>>,
) -> Result<CommittedUpstreamStream, PreOutputStreamFailure> {
    let mut buffered = Vec::new();
    loop {
        match decoded_rx.recv().await {
            Some(event @ crate::urp::UrpStreamEvent::ResponseStart { .. }) => buffered.push(event),
            Some(crate::urp::UrpStreamEvent::Error { code, message, .. }) => {
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
                let code = code.unwrap_or_else(|| "upstream_stream_error".to_string());
                let failure_class = midstream_terminal_failure_class(
                    http_status,
                    Some(&code),
                    error_type.as_deref(),
                );
                // `upstream_status` stays unset: the status is synthesized by the
                // decoder and must not trigger an RTA-6c shared-origin blast.
                let error = AppError::new(StatusCode::BAD_GATEWAY, code.clone(), message)
                    .with_upstream_error(None, Some(code), error_type, param);
                return Err(PreOutputStreamFailure {
                    error,
                    failure_class,
                });
            }
            Some(event) => {
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
fn replay_then_forward(
    buffered: Vec<crate::urp::UrpStreamEvent>,
    mut decoded_rx: mpsc::Receiver<crate::urp::UrpStreamEvent>,
) -> mpsc::Receiver<crate::urp::UrpStreamEvent> {
    let (tx, rx) = mpsc::channel(64);
    tokio::spawn(async move {
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

#[cfg(test)]
#[path = "stream_terminal_tests.rs"]
mod stream_terminal_tests;

async fn emit_stream_error_if_needed(
    downstream: DownstreamProtocol,
    err: &AppError,
    tx: &mpsc::Sender<Event>,
    capture_frames: Option<&crate::request_capture::SseFrameCapture>,
) {
    if err.downstream_stream_terminal_sent {
        return;
    }
    let (event_name, body) = match downstream {
        DownstreamProtocol::Responses => (Some("error"), responses_stream_error_json(1, err)),
        DownstreamProtocol::ChatCompletions => (None, openai_error_json(err)),
        DownstreamProtocol::AnthropicMessages => (
            Some("error"),
            json!({"type": "error", "error": {"type": err.code, "message": err.message}}),
        ),
    };
    let data = body.to_string();
    let event = match event_name {
        Some(name) => Event::default().event(name).data(&data),
        None => Event::default().data(&data),
    };
    if let Some(frames) = capture_frames {
        frames
            .record(match event_name {
                Some(name) => format!("event: {name}\ndata: {data}\n\n"),
                None => format!("data: {data}\n\n"),
            })
            .await;
    }
    if tx.send(event).await.is_err() {
        return;
    }
    if matches!(
        downstream,
        DownstreamProtocol::ChatCompletions | DownstreamProtocol::Responses
    ) {
        if let Some(frames) = capture_frames {
            frames.record("data: [DONE]\n\n".to_string()).await;
        }
        let _ = tx.send(Event::default().data("[DONE]")).await;
    }
}

fn combine_stream_stage_results(results: [AppResult<()>; 4]) -> AppResult<()> {
    // An encoder failure can close earlier stages. Keep the error that already ended the wire.
    if let Some(err) = results
        .iter()
        .filter_map(|result| result.as_ref().err())
        .find(|err| err.downstream_stream_terminal_sent)
    {
        return Err(err.clone());
    }
    for result in results {
        result?;
    }
    Ok(())
}

fn estimated_tokens_from_utf8_bytes(bytes: u64) -> u64 {
    bytes.div_ceil(4)
}

fn decoded_visible_output_bytes(output: &[urp::Node]) -> u64 {
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

async fn retain_decoded_terminal_output(
    mut rx: mpsc::Receiver<urp::UrpStreamEvent>,
    tx: mpsc::Sender<urp::UrpStreamEvent>,
    terminal_output: Arc<Mutex<Vec<urp::Node>>>,
    aliases: HashMap<String, urp::ToolIdentity>,
) -> AppResult<()> {
    while let Some(mut event) = rx.recv().await {
        restore_tool_namespace_event(&mut event, &aliases);
        if let urp::UrpStreamEvent::ResponseDone { output, .. } = &event {
            *terminal_output.lock().await = output.clone();
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
pub(super) async fn retain_reconstructed_urp_response(
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
            object.insert("output".to_string(), json!(output));
            for (key, value) in extra_body {
                object.insert(key.clone(), value.clone());
            }
            *reconstructed.lock().await = Some(serde_json::Value::Object(object));
        }
        let _ = tx.send(event).await;
    }
    Ok(())
}

fn stream_error_code(err: &AppError) -> String {
    err.upstream_code.as_ref().unwrap_or(&err.code).to_string()
}

fn stream_terminal_error_from_app(err: &AppError) -> StreamTerminalError {
    StreamTerminalError {
        code: stream_error_code(err),
        // SAN-9: the terminal request-log row keeps the internal detail while
        // the downstream frame carries the sanitized client message.
        message: err
            .internal_message
            .clone()
            .unwrap_or_else(|| err.message.clone()),
        http_status: err.upstream_status.unwrap_or(err.status.as_u16()),
        error_type: err
            .upstream_type
            .clone()
            .or_else(|| Some(err.error_type.clone())),
        param: err.upstream_param.clone().or_else(|| err.param.clone()),
    }
}

#[allow(clippy::too_many_arguments)]
fn spawn_stream_attempt_error(
    state: &AppState,
    auth: &crate::auth::AuthResult,
    attempt: &MonoizeAttempt,
    model: &str,
    started_at: std::time::Instant,
    request_id: Option<String>,
    request_ip: Option<String>,
    ttfb_ms: Option<u64>,
    error: &AppError,
    reasoning_effort: Option<String>,
    tried_providers: Vec<TriedProvider>,
) {
    spawn_request_log_stream_terminal_error(
        state,
        auth,
        attempt,
        model,
        started_at,
        request_id,
        request_ip,
        ttfb_ms,
        stream_terminal_error_from_app(error),
        reasoning_effort,
        tried_providers,
        None,
    );
}

fn prestream_error_stream(downstream: DownstreamProtocol, err: AppError) -> ForwardEventStream {
    let (tx, rx) = mpsc::channel::<Event>(8);
    tokio::spawn(async move {
        match downstream {
            DownstreamProtocol::Responses => {
                let responses_error = responses_stream_error_json(1, &err);
                let _ = tx
                    .send(
                        Event::default()
                            .event("error")
                            .data(responses_error.to_string()),
                    )
                    .await;
                let _ = tx.send(Event::default().data("[DONE]")).await;
            }
            DownstreamProtocol::ChatCompletions => {
                let error_json = openai_error_json(&err);
                let _ = tx.send(Event::default().data(error_json.to_string())).await;
                let _ = tx.send(Event::default().data("[DONE]")).await;
            }
            DownstreamProtocol::AnthropicMessages => {
                let code = stream_error_code(&err);
                let anthropic_error = json!({
                    "type": "error",
                    "error": {
                        "type": code,
                        "message": err.message
                    }
                });
                let _ = tx
                    .send(
                        Event::default()
                            .event("error")
                            .data(anthropic_error.to_string()),
                    )
                    .await;
            }
        }
    });
    receiver_event_stream(rx)
}

pub(super) fn deferred_forward_event_stream<F, S>(
    downstream: DownstreamProtocol,
    forwarding: F,
) -> futures_util::stream::BoxStream<'static, Result<Event, std::convert::Infallible>>
where
    F: std::future::Future<Output = AppResult<S>> + Send + 'static,
    S: futures_util::Stream<Item = Result<Event, std::convert::Infallible>> + Send + 'static,
{
    let (tx, rx) = mpsc::channel::<Event>(64);
    tokio::spawn(async move {
        match forwarding.await {
            Ok(stream) => {
                tokio::pin!(stream);
                while let Some(Ok(event)) = stream.next().await {
                    if tx.send(event).await.is_err() {
                        break;
                    }
                }
            }
            Err(err) => {
                let err_stream = prestream_error_stream(downstream, err);
                tokio::pin!(err_stream);
                while let Some(Ok(event)) = err_stream.next().await {
                    if tx.send(event).await.is_err() {
                        break;
                    }
                }
            }
        }
    });
    receiver_event_stream(rx).boxed()
}

pub(super) async fn forward_stream_typed(
    state: AppState,
    auth: crate::auth::AuthResult,
    mut req: urp::UrpRequest,
    max_multiplier: Option<Multiplier>,
    downstream: DownstreamProtocol,
    request_id: Option<String>,
    request_ip: Option<String>,
    client_session_id: Option<String>,
    capture: RequestCaptureContext,
    prefer_upstream_websocket: bool,
) -> AppResult<
    impl futures_util::Stream<Item = Result<Event, std::convert::Infallible>> + Send + 'static,
> {
    let started_at = std::time::Instant::now();
    let mut last_failed_attempt: Option<MonoizeAttempt> = None;
    let mut tried_providers: Vec<TriedProvider> = Vec::new();
    let transform_match_model = resolve_model_suffix(&state, &mut req).await?;
    // Preserve the suffix-normalized request so each per-attempt iteration can
    // re-derive the transformed request from a pristine base (see the matching
    // comment in `execute_nonstream_typed`).
    let mut original_req = req.clone();
    let logical_model = req.model.clone();
    let routing_stub = build_routing_stub(&req, max_multiplier);
    let mut attempts = build_monoize_attempts(&state, &routing_stub, &auth).await?;
    bind_media_request_routes(&mut original_req, &mut attempts)?;
    let history_context =
        responses_history::HistoryContext::from_request(&state, &original_req, downstream);
    attach_client_session_id(&mut attempts, client_session_id, Some(&req));
    ensure_balance_before_forward_for_attempts(&state, &auth, &attempts).await?;
    let pending_request_log_guard = insert_pending_request_log(
        &state,
        &auth,
        &req.model,
        true,
        request_id.as_deref(),
        request_ip.as_deref(),
        started_at,
    )
    .await?;

    let mut execution_state = AttemptExecutionState::default();

    for mut attempt in attempts {
        if execution_state.should_skip(&attempt) {
            continue;
        }

        let global_transforms = state.monoize_runtime.read().await.global_transforms.clone();

        let sse_max_frame_length = effective_sse_max_frame_length(
            &attempt.provider_transforms,
            &global_transforms,
            &auth.transforms,
            &logical_model,
        );
        let requires_buffered_stream = requires_buffered_response_stream(
            &attempt.provider_transforms,
            &global_transforms,
            &auth.transforms,
            &logical_model,
            downstream,
        ) || attempt.provider_type == ProviderType::Replicate;
        let max_channel_attempts = same_channel_attempt_slots(&attempt);

        'channel_attempts: for channel_attempt in 0..max_channel_attempts {
            if execution_state.should_skip(&attempt) {
                break;
            }

            let attempt_number = execution_state.record_upstream_attempt(&attempt);
            // Clone from the pristine original request (pre-transforms) so
            // that the cross-family strip runs BEFORE provider, global, and
            // API-key transforms; see `execute_nonstream_typed`.
            let mut req_attempt = original_req.clone();
            if matches!(downstream, DownstreamProtocol::Responses) {
                promote_responses_additional_tools(&mut req_attempt, attempt.provider_type);
            }
            if let Some(target_protocol) = provider_type_protocol(attempt.provider_type) {
                urp::retain_provider_items_for_protocol(&mut req_attempt.input, target_protocol);
                if target_protocol == urp::ProviderProtocol::Responses {
                    urp::remove_downstream_only_reasoning_for_responses(&mut req_attempt.input);
                }
            }
            if attempt.strip_cross_protocol_nested_extra
                && !downstream.is_same_family(attempt.provider_type)
            {
                urp::strip_nested_extra_body(&mut req_attempt.input);
            }
            inject_monoize_context(&auth, &mut req_attempt);
            req_attempt.model = attempt.upstream_model.clone();
            // Unwrap mz2 reasoning envelopes BEFORE any request-phase transform
            // observes the request input. See `nonstream.rs` for rationale and
            // spec references (urp-transform-system PIPE-1 step 6, PIPE-1d).
            urp::filter_and_unwrap_reasoning_envelopes_for_upstream(
                &mut req_attempt.input,
                reasoning_envelope_provider_type(attempt.provider_type),
                &req_attempt.model,
                auth.reasoning_envelope_enabled,
            );
            if let Err(err) = apply_transform_rules_request(
                &state,
                &mut req_attempt,
                &attempt.provider_transforms,
                &transform_match_model,
                Some(attempt.provider_type),
            )
            .await
            {
                spawn_stream_attempt_error(
                    &state,
                    &auth,
                    &attempt,
                    &logical_model,
                    started_at,
                    request_id.clone(),
                    request_ip.clone(),
                    None,
                    &err,
                    req.reasoning.as_ref().and_then(|r| r.effort.clone()),
                    tried_providers.clone(),
                );
                return Err(err);
            }
            if let Err(err) = apply_transform_rules_request(
                &state,
                &mut req_attempt,
                &global_transforms,
                &transform_match_model,
                Some(attempt.provider_type),
            )
            .await
            {
                spawn_stream_attempt_error(
                    &state,
                    &auth,
                    &attempt,
                    &logical_model,
                    started_at,
                    request_id.clone(),
                    request_ip.clone(),
                    None,
                    &err,
                    req.reasoning.as_ref().and_then(|r| r.effort.clone()),
                    tried_providers.clone(),
                );
                return Err(err);
            }
            if let Err(err) = apply_transform_rules_request(
                &state,
                &mut req_attempt,
                &auth.transforms,
                &transform_match_model,
                Some(attempt.provider_type),
            )
            .await
            {
                spawn_stream_attempt_error(
                    &state,
                    &auth,
                    &attempt,
                    &logical_model,
                    started_at,
                    request_id.clone(),
                    request_ip.clone(),
                    None,
                    &err,
                    req.reasoning.as_ref().and_then(|r| r.effort.clone()),
                    tried_providers.clone(),
                );
                return Err(err);
            }
            strip_monoize_context(&mut req_attempt);
            let capture_transform_chain = crate::request_capture::build_transform_chain(
                &attempt.provider_transforms,
                &global_transforms,
                &auth.transforms,
                &transform_match_model,
            );

            if requires_buffered_stream {
                let mut nonstream_req = req_attempt.clone();
                nonstream_req.stream = Some(false);
                let upstream_body =
                    match encode_request_for_provider(&mut nonstream_req, &attempt, downstream) {
                        Ok(body) => body,
                        Err(err) => {
                            spawn_stream_attempt_error(
                                &state,
                                &auth,
                                &attempt,
                                &logical_model,
                                started_at,
                                request_id.clone(),
                                request_ip.clone(),
                                None,
                                &err,
                                req.reasoning.as_ref().and_then(|r| r.effort.clone()),
                                tried_providers.clone(),
                            );
                            return Err(err);
                        }
                    };
                attempt.session_affinity_value =
                    resolve_session_affinity_value(&attempt, &upstream_body);
                let provider = build_channel_provider_config(&attempt);
                let path =
                    upstream_path_for_model(attempt.provider_type, &req_attempt.model, false);
                let http = client_http_for_attempt(&state, &attempt)?;
                let call = upstream::call_upstream_with_timeout_and_headers(
                    &http,
                    &provider,
                    &attempt.api_key,
                    &path,
                    &upstream_body,
                    attempt.request_timeout_ms,
                    &attempt_extra_headers(&attempt, &upstream_body),
                )
                .await;
                match call {
                    Ok(value) => {
                        if let Some(session) = capture.session.as_ref() {
                            session
                                .push_attempt(crate::request_capture::build_attempt_dump(
                                    attempt_number,
                                    &attempt.provider_id,
                                    Some(&attempt.channel_id),
                                    attempt.provider_type,
                                    &logical_model,
                                    &nonstream_req.model,
                                    &path,
                                    capture.raw_input.as_ref().clone(),
                                    &nonstream_req,
                                    upstream_body.clone(),
                                    Some(value.clone()),
                                    // RCD-D10b: buffered synthetic streams keep
                                    // the provider payload in downstream_response.
                                    None,
                                    None,
                                    capture_transform_chain.clone(),
                                    None,
                                ))
                                .await;
                        }
                        update_pending_channel_info(
                            &state,
                            &auth,
                            &attempt,
                            &logical_model,
                            true,
                            request_id.as_deref(),
                            request_ip.as_deref(),
                            started_at,
                        )
                        .await;
                        let mut resp = match decode_response_from_provider(
                            attempt.provider_type,
                            &value,
                            &nonstream_req.model,
                            state.monoize_runtime.read().await.mask_sensitive_info,
                            &nonstream_req,
                        ) {
                            Ok(resp) => resp,
                            Err(err) => {
                                let same_channel_retryable =
                                    is_same_channel_retryable_app_error(&err);
                                let passive_failure_class = same_channel_retryable
                                    .then(|| classify_retryable_app_failure(&err));
                                record_upstream_attempt_failure(
                                    &state,
                                    &attempt,
                                    attempt_number,
                                    &err,
                                    passive_failure_class,
                                    &mut tried_providers,
                                    &mut execution_state,
                                )
                                .await;
                                last_failed_attempt = Some(attempt.clone());
                                if allow_same_channel_retry(
                                    &state,
                                    &attempt,
                                    &execution_state,
                                    channel_attempt + 1,
                                    passive_failure_class,
                                )
                                .await
                                {
                                    maybe_sleep_before_channel_retry(&attempt).await;
                                    continue 'channel_attempts;
                                }
                                break 'channel_attempts;
                            }
                        };
                        let upstream_response_model =
                            mismatched_upstream_response_model(&nonstream_req.model, &resp.model);
                        // MP-F3: a fail-closed missing-usage billable success
                        // rejects with 403 before the synthetic stream starts.
                        if resp.usage.is_none() && missing_usage_rejects(&auth, &attempt) {
                            let err = missing_usage_error();
                            if let Some(session) = capture.session.as_ref() {
                                session.persist_with_result(None, false).await;
                            }
                            spawn_stream_attempt_error(
                                &state,
                                &auth,
                                &attempt,
                                &logical_model,
                                started_at,
                                request_id.clone(),
                                request_ip.clone(),
                                None,
                                &err,
                                req.reasoning.as_ref().and_then(|r| r.effort.clone()),
                                tried_providers.clone(),
                            );
                            return Err(err);
                        }
                        mark_channel_success(&state, &attempt).await;
                        refresh_channel_affinity(&state, &attempt).await;
                        // Wrap newly produced encrypted reasoning payloads in
                        // mz2 envelopes BEFORE response-phase transforms run.
                        // See `nonstream.rs` and PIPE-1d in
                        // spec/urp-transform-system.spec.md for rationale.
                        if auth.reasoning_envelope_enabled {
                            urp::wrap_reasoning_envelopes_in_response(
                                &mut resp,
                                reasoning_envelope_provider_type(attempt.provider_type),
                                &nonstream_req.model,
                            );
                        }
                        if let Err(err) = apply_transform_rules_response(
                            &state,
                            &mut resp,
                            &attempt.provider_transforms,
                            &logical_model,
                            Some(attempt.provider_type),
                        )
                        .await
                        {
                            if let Some(session) = capture.session.as_ref() {
                                session.persist_with_result(None, false).await;
                            }
                            spawn_stream_attempt_error(
                                &state,
                                &auth,
                                &attempt,
                                &logical_model,
                                started_at,
                                request_id.clone(),
                                request_ip.clone(),
                                None,
                                &err,
                                req.reasoning.as_ref().and_then(|r| r.effort.clone()),
                                tried_providers.clone(),
                            );
                            return Err(err);
                        }
                        if let Err(err) = apply_transform_rules_response(
                            &state,
                            &mut resp,
                            &global_transforms,
                            &logical_model,
                            Some(attempt.provider_type),
                        )
                        .await
                        {
                            if let Some(session) = capture.session.as_ref() {
                                session.persist_with_result(None, false).await;
                            }
                            spawn_stream_attempt_error(
                                &state,
                                &auth,
                                &attempt,
                                &logical_model,
                                started_at,
                                request_id.clone(),
                                request_ip.clone(),
                                None,
                                &err,
                                req.reasoning.as_ref().and_then(|r| r.effort.clone()),
                                tried_providers.clone(),
                            );
                            return Err(err);
                        }
                        if let Err(err) = apply_transform_rules_response(
                            &state,
                            &mut resp,
                            &auth.transforms,
                            &logical_model,
                            Some(attempt.provider_type),
                        )
                        .await
                        {
                            if let Some(session) = capture.session.as_ref() {
                                session.persist_with_result(None, false).await;
                            }
                            spawn_stream_attempt_error(
                                &state,
                                &auth,
                                &attempt,
                                &logical_model,
                                started_at,
                                request_id.clone(),
                                request_ip.clone(),
                                None,
                                &err,
                                req.reasoning.as_ref().and_then(|r| r.effort.clone()),
                                tried_providers.clone(),
                            );
                            return Err(err);
                        }
                        if matches!(
                            attempt.provider_type,
                            ProviderType::OpenaiImage | ProviderType::OpenrouterImage
                        ) && !matches!(downstream, DownstreamProtocol::Responses)
                        {
                            convert_assistant_images_to_markdown(&mut resp);
                        }
                        let history_for_stream = history_context.clone().map(|history| {
                            history.with_resource_scope(media_resource_scope(&attempt))
                        });
                        if let Some(history) = &history_for_stream {
                            history.decorate_response(&mut resp);
                        }
                        let (tx, rx) = mpsc::channel::<Event>(64);
                        let logical_model_for_stream = logical_model.clone();
                        let state_for_log = state.clone();
                        let auth_for_log = auth.clone();
                        let attempt_for_log = attempt.clone();
                        let request_id_for_log = request_id.clone();
                        let request_ip_for_log = request_ip.clone();
                        let reasoning_effort_for_log =
                            req.reasoning.as_ref().and_then(|r| r.effort.clone());
                        let tried_providers_for_log = tried_providers;
                        let capture_session = capture.session.clone();
                        let pending_request_log_guard_for_stream = pending_request_log_guard;
                        tokio::spawn(async move {
                            let _pending_request_log_guard = pending_request_log_guard_for_stream;
                            let tx_err = tx.clone();
                            let synthetic_reasoning_duration_secs =
                                Some(started_at.elapsed().as_secs());
                            let stream_result =
                                crate::urp::stream_encode::emit_synthetic_stream_from_urp_response(
                                    downstream,
                                    &logical_model_for_stream,
                                    &resp,
                                    synthetic_reasoning_duration_secs,
                                    sse_max_frame_length,
                                    tx,
                                )
                                .await;
                            match stream_result {
                                Ok(()) => {
                                    if let Some(history) = &history_for_stream {
                                        history.retain_response(&resp).await;
                                    }
                                    match maybe_charge_response(
                                        &state_for_log,
                                        &auth_for_log,
                                        &attempt_for_log,
                                        &logical_model_for_stream,
                                        &resp,
                                        request_id_for_log.as_deref(),
                                    )
                                    .await
                                    {
                                        Ok(charge) => spawn_request_log(
                                            &state_for_log,
                                            &auth_for_log,
                                            &attempt_for_log,
                                            &logical_model_for_stream,
                                            resp.usage.clone(),
                                            charge.charge_nano_usd,
                                            charge.billing_breakdown,
                                            true,
                                            started_at,
                                            request_id_for_log,
                                            request_ip_for_log,
                                            attempt_for_log.channel_id.clone(),
                                            Some(started_at.elapsed().as_millis() as u64),
                                            None,
                                            reasoning_effort_for_log,
                                            tried_providers_for_log,
                                            tx_err.is_closed(),
                                            upstream_response_model,
                                        ),
                                        Err(err) => {
                                            tracing::error!(
                                                code = %err.code,
                                                "failed to settle buffered stream billing: {}",
                                                err.message
                                            );
                                            spawn_request_log_stream_terminal_error(
                                                &state_for_log,
                                                &auth_for_log,
                                                &attempt_for_log,
                                                &logical_model_for_stream,
                                                started_at,
                                                request_id_for_log,
                                                request_ip_for_log,
                                                Some(started_at.elapsed().as_millis() as u64),
                                                StreamTerminalError {
                                                    code: "billing_settlement_failed".to_string(),
                                                    message: format!(
                                                        "{}: {}",
                                                        err.code, err.message
                                                    ),
                                                    http_status: err.status.as_u16(),
                                                    error_type: Some("billing_error".to_string()),
                                                    param: err.param.clone(),
                                                },
                                                reasoning_effort_for_log,
                                                tried_providers_for_log,
                                                resp.usage.clone(),
                                            );
                                        }
                                    }
                                    if let Some(session) = capture_session.as_ref() {
                                        session
                                            .persist_with_result(resp.usage.as_ref(), false)
                                            .await;
                                    }
                                }
                                Err(err) => {
                                    tracing::warn!("synthetic stream failed: {}", err.message);
                                    spawn_stream_attempt_error(
                                        &state_for_log,
                                        &auth_for_log,
                                        &attempt_for_log,
                                        &logical_model_for_stream,
                                        started_at,
                                        request_id_for_log,
                                        request_ip_for_log,
                                        Some(started_at.elapsed().as_millis() as u64),
                                        &err,
                                        reasoning_effort_for_log,
                                        tried_providers_for_log,
                                    );
                                    emit_stream_error_if_needed(downstream, &err, &tx_err, None)
                                        .await;
                                    if let Some(session) = capture_session.as_ref() {
                                        session.persist_with_result(None, true).await;
                                    }
                                }
                            }
                        });
                        return Ok(receiver_event_stream(rx));
                    }
                    Err(err) => {
                        if let Some(session) = capture.session.as_ref() {
                            session
                                .push_attempt(crate::request_capture::build_attempt_dump(
                                    attempt_number,
                                    &attempt.provider_id,
                                    Some(&attempt.channel_id),
                                    attempt.provider_type,
                                    &logical_model,
                                    &nonstream_req.model,
                                    &path,
                                    capture.raw_input.as_ref().clone(),
                                    &nonstream_req,
                                    upstream_body.clone(),
                                    None,
                                    None,
                                    None,
                                    capture_transform_chain.clone(),
                                    Some(json!({
                                        "message": err.message,
                                        "code": err.code,
                                        "status": err.status.map(|status| status.as_u16()),
                                    })),
                                ))
                                .await;
                        }
                        let same_channel_retryable = is_same_channel_retryable_error(&err);
                        let passive_failure_class =
                            same_channel_retryable.then(|| classify_retryable_failure(&err));
                        let mask_sensitive_info =
                            state.monoize_runtime.read().await.mask_sensitive_info;
                        let app_err = upstream_error_to_app(err, mask_sensitive_info);
                        record_upstream_attempt_failure(
                            &state,
                            &attempt,
                            attempt_number,
                            &app_err,
                            passive_failure_class,
                            &mut tried_providers,
                            &mut execution_state,
                        )
                        .await;
                        last_failed_attempt = Some(attempt.clone());
                        if allow_same_channel_retry(
                            &state,
                            &attempt,
                            &execution_state,
                            channel_attempt + 1,
                            passive_failure_class,
                        )
                        .await
                        {
                            maybe_sleep_before_channel_retry(&attempt).await;
                            continue;
                        }
                        break;
                    }
                }
            }

            let upstream_body =
                match encode_request_for_provider(&mut req_attempt, &attempt, downstream) {
                    Ok(body) => body,
                    Err(err) => {
                        spawn_stream_attempt_error(
                            &state,
                            &auth,
                            &attempt,
                            &logical_model,
                            started_at,
                            request_id.clone(),
                            request_ip.clone(),
                            None,
                            &err,
                            req.reasoning.as_ref().and_then(|r| r.effort.clone()),
                            tried_providers.clone(),
                        );
                        return Err(err);
                    }
                };
            attempt.session_affinity_value =
                resolve_session_affinity_value(&attempt, &upstream_body);
            let estimated_input_tokens = estimated_tokens_from_utf8_bytes(
                u64::try_from(upstream_body.to_string().len()).unwrap_or(u64::MAX),
            );
            let extra_headers = attempt_extra_headers(&attempt, &upstream_body);
            let mut websocket_source = None;
            let mut websocket_send_error = None;
            if prefer_upstream_websocket
                && attempt.provider_type == ProviderType::Responses
                && attempt.websocket_supported != Some(false)
            {
                let mut ws_headers =
                    vec![crate::upstream_websocket::responses_authorization_header(
                        &attempt.api_key,
                    )];
                ws_headers.extend(extra_headers.iter().cloned());
                let proxy = attempt
                    .proxy_url
                    .as_deref()
                    .map(str::trim)
                    .filter(|value| !value.is_empty())
                    .or(state.node.upstream_proxy_url.as_deref());
                match crate::upstream_websocket::connect_responses_websocket(
                    &attempt.base_url,
                    &ws_headers,
                    proxy,
                    attempt.request_timeout_ms,
                )
                .await
                {
                    Ok(mut ws) => {
                        remember_websocket_supported(&state, &mut attempt, true).await;
                        let payload = crate::upstream_websocket::responses_ws_create_payload(
                            upstream_body.clone(),
                        );
                        match ws.send_text(payload.to_string()).await {
                            Ok(()) => websocket_source = Some(ws),
                            Err(err) => websocket_send_error = Some(err),
                        }
                    }
                    Err(_) => {
                        remember_websocket_supported(&state, &mut attempt, false).await;
                    }
                }
            }
            if let Some(err) = websocket_send_error {
                let app_err = AppError::new(
                    StatusCode::BAD_GATEWAY,
                    "upstream_websocket_send_failed",
                    err,
                );
                spawn_stream_attempt_error(
                    &state,
                    &auth,
                    &attempt,
                    &logical_model,
                    started_at,
                    request_id.clone(),
                    request_ip.clone(),
                    None,
                    &app_err,
                    req.reasoning.as_ref().and_then(|r| r.effort.clone()),
                    tried_providers.clone(),
                );
                let same_channel_retryable = is_same_channel_retryable_app_error(&app_err);
                let passive_failure_class =
                    same_channel_retryable.then(|| classify_retryable_app_failure(&app_err));
                record_upstream_attempt_failure(
                    &state,
                    &attempt,
                    attempt_number,
                    &app_err,
                    passive_failure_class,
                    &mut tried_providers,
                    &mut execution_state,
                )
                .await;
                last_failed_attempt = Some(attempt.clone());
                if allow_same_channel_retry(
                    &state,
                    &attempt,
                    &execution_state,
                    channel_attempt + 1,
                    passive_failure_class,
                )
                .await
                {
                    maybe_sleep_before_channel_retry(&attempt).await;
                    continue;
                }
                break;
            }
            let (path, capture_upstream_request, call) = if let Some(ws) = websocket_source {
                (
                    "/v1/responses".to_string(),
                    upstream_body.clone(),
                    Ok(StreamUpstreamSource::ResponsesWebSocket(ws)),
                )
            } else {
                let http = client_http_for_attempt(&state, &attempt)?;
                // Reference edits use JSON; inline Base64 edits use multipart.
                let stream_call = match call_streaming_image_capable_upstream(
                    &http,
                    &attempt,
                    &req_attempt,
                    &upstream_body,
                    attempt.request_timeout_ms.saturating_mul(10).max(600_000),
                    &extra_headers,
                    capture.session.is_some(),
                )
                .await
                {
                    Ok(stream_call) => stream_call,
                    Err(err) => {
                        spawn_stream_attempt_error(
                            &state,
                            &auth,
                            &attempt,
                            &logical_model,
                            started_at,
                            request_id.clone(),
                            request_ip.clone(),
                            None,
                            &err,
                            req.reasoning.as_ref().and_then(|r| r.effort.clone()),
                            tried_providers.clone(),
                        );
                        return Err(err);
                    }
                };
                let path = stream_call.path;
                // RCD-D6a/OIU-E5g: a multipart edit attempt records the sent form
                // as `upstream_request` instead of the unused JSON encoding.
                let capture_upstream_request = stream_call
                    .capture_multipart_request
                    .unwrap_or_else(|| upstream_body.clone());
                (
                    path,
                    capture_upstream_request,
                    stream_call
                        .result
                        .map(StreamUpstreamSource::Http)
                        .map_err(|err| err),
                )
            };
            match call {
                Ok(upstream_source) => {
                    let legacy = match typed_request_to_legacy(&req_attempt, max_multiplier) {
                        Ok(legacy) => legacy,
                        Err(err) => {
                            spawn_stream_attempt_error(
                                &state,
                                &auth,
                                &attempt,
                                &logical_model,
                                started_at,
                                request_id.clone(),
                                request_ip.clone(),
                                None,
                                &err,
                                req.reasoning.as_ref().and_then(|r| r.effort.clone()),
                                tried_providers.clone(),
                            );
                            return Err(err);
                        }
                    };
                    let pending_request_envelope_extra =
                        req.input.clone().into_iter().find_map(|node| match node {
                            crate::urp::Node::NextDownstreamEnvelopeExtra { extra_body }
                                if !extra_body.is_empty() =>
                            {
                                Some(extra_body)
                            }
                            _ => None,
                        });
                    let provider_type = attempt.provider_type;
                    let runtime_metrics = Arc::new(Mutex::new(StreamRuntimeMetrics {
                        ttfb_ms: None,
                        usage: None,
                        response_id: None,
                        response_service_tier: None,
                        response_model: None,
                        response_model_terminal: false,
                        terminal: StreamTerminalDiagnostics::default(),
                        estimated_output_tokens: 0,
                        visible_output_bytes: 0,
                    }));
                    let (stream_idle_timeout_ms, mask_sensitive_info) = {
                        let runtime = state.monoize_runtime.read().await;
                        (
                            runtime.stream_idle_timeout_ms.max(1),
                            runtime.mask_sensitive_info,
                        )
                    };
                    let (decoded_rx, decode_handle) = {
                        let (decoded_tx, decoded_rx) =
                            mpsc::channel::<crate::urp::UrpStreamEvent>(64);
                        let metrics = runtime_metrics.clone();
                        let handle = tokio::spawn(async move {
                            match upstream_source {
                                StreamUpstreamSource::Http(upstream_resp) => {
                                    stream_upstream_to_urp_events(
                                        &legacy,
                                        pending_request_envelope_extra,
                                        provider_type,
                                        upstream_resp,
                                        decoded_tx,
                                        Some(started_at),
                                        Some(metrics),
                                        stream_idle_timeout_ms,
                                    )
                                    .await
                                }
                                StreamUpstreamSource::ResponsesWebSocket(session) => {
                                    crate::urp::stream_decode::openai_responses::stream_responses_websocket_to_urp_events(
                                        &legacy,
                                        pending_request_envelope_extra,
                                        session,
                                        decoded_tx,
                                        Some(started_at),
                                        Some(metrics),
                                        stream_idle_timeout_ms,
                                    )
                                    .await
                                }
                            }
                        });
                        (decoded_rx, handle)
                    };
                    let committed = match probe_stream_first_output(
                        decoded_rx,
                        decode_handle,
                        &runtime_metrics,
                    )
                    .await
                    {
                        Ok(committed) => committed,
                        Err(failure) => {
                            tracing::warn!(
                                attempt_number,
                                channel_id = %attempt.channel_id,
                                code = %failure.error.code,
                                "upstream stream failed before its first output event: {}",
                                failure.error.message
                            );
                            if let Some(session) = capture.session.as_ref() {
                                session
                                    .push_attempt(crate::request_capture::build_attempt_dump(
                                        attempt_number,
                                        &attempt.provider_id,
                                        Some(&attempt.channel_id),
                                        attempt.provider_type,
                                        &logical_model,
                                        &req_attempt.model,
                                        &path,
                                        capture.raw_input.as_ref().clone(),
                                        &req_attempt,
                                        capture_upstream_request.clone(),
                                        None,
                                        None,
                                        None,
                                        capture_transform_chain.clone(),
                                        Some(json!({
                                            "message": failure.error.message,
                                            "code": failure.error.code,
                                            "status": failure.error.status.as_u16(),
                                        })),
                                    ))
                                    .await;
                            }
                            record_upstream_attempt_failure(
                                &state,
                                &attempt,
                                attempt_number,
                                &failure.error,
                                failure.failure_class,
                                &mut tried_providers,
                                &mut execution_state,
                            )
                            .await;
                            last_failed_attempt = Some(attempt.clone());
                            if allow_same_channel_retry(
                                &state,
                                &attempt,
                                &execution_state,
                                channel_attempt + 1,
                                failure.failure_class,
                            )
                            .await
                            {
                                maybe_sleep_before_channel_retry(&attempt).await;
                                continue 'channel_attempts;
                            }
                            break 'channel_attempts;
                        }
                    };
                    update_pending_channel_info(
                        &state,
                        &auth,
                        &attempt,
                        &logical_model,
                        true,
                        request_id.as_deref(),
                        request_ip.as_deref(),
                        started_at,
                    )
                    .await;
                    mark_channel_success(&state, &attempt).await;
                    let (tx, rx) = mpsc::channel::<Event>(64);
                    let capture_frames = capture
                        .session
                        .as_ref()
                        .map(|_| crate::request_capture::SseFrameCapture::new());
                    let decoded_terminal_output = Arc::new(Mutex::new(Vec::<urp::Node>::new()));
                    let history_for_stream = history_context
                        .clone()
                        .map(|history| history.with_resource_scope(media_resource_scope(&attempt)));
                    let state_for_log = state.clone();
                    let auth_for_log = auth.clone();
                    let attempt_for_log = attempt.clone();
                    let model_for_log = logical_model.clone();
                    let model_for_encode = logical_model.clone();
                    let model_for_transform = logical_model.clone();
                    let request_id_for_log = request_id.clone();
                    let request_ip_for_log = request_ip.clone();
                    let channel_id_for_log = attempt.channel_id.clone();
                    let capture_session = capture.session.clone();
                    let capture_raw_input = capture.raw_input.clone();
                    let capture_transform_chain_for_task = capture_transform_chain.clone();
                    let capture_req_attempt = req_attempt.clone();
                    let capture_upstream_body = capture_upstream_request.clone();
                    let capture_path = path.clone();
                    let capture_provider_id = attempt.provider_id.clone();
                    let capture_channel_id = attempt.channel_id.clone();
                    let capture_provider_type = attempt.provider_type;
                    let transform_provider_type = attempt.provider_type;
                    let capture_upstream_model = req_attempt.model.clone();
                    let capture_logical_model = logical_model.clone();
                    let capture_attempt_number = attempt_number;
                    let capture_frames_for_task = capture_frames.clone();
                    let reasoning_effort_for_log =
                        req.reasoning.as_ref().and_then(|r| r.effort.clone());
                    let tried_providers_for_log = tried_providers.clone();
                    let state_for_transform = state.clone();
                    let provider_rules_for_transform = attempt.provider_transforms.clone();
                    let global_rules_for_transform = global_transforms.clone();
                    let auth_rules_for_transform = auth.transforms.clone();
                    let reasoning_envelope_for_transform =
                        auth.reasoning_envelope_enabled.then(|| {
                            (
                                reasoning_envelope_provider_type(attempt.provider_type).to_string(),
                                req_attempt.model.clone(),
                            )
                        });
                    let namespace_aliases = tool_namespace_aliases(&req_attempt);
                    let pending_request_log_guard_for_stream = pending_request_log_guard;
                    tokio::spawn(async move {
                        let _pending_request_log_guard = pending_request_log_guard_for_stream;
                        let tx_err = tx.clone();
                        // RCD-D10a: the reconstruction slot exists only while a
                        // capture session is active; without one the tap stage
                        // is not inserted at all.
                        let reconstructed_urp_response = capture_session
                            .as_ref()
                            .map(|_| Arc::new(Mutex::new(None::<serde_json::Value>)));
                        let stream_future = async {
                            let decoded_rx =
                                replay_then_forward(committed.buffered, committed.decoded_rx);
                            let decode_handle = committed.decode_handle;
                            let (retained_tx, retained_rx) =
                                mpsc::channel::<crate::urp::UrpStreamEvent>(64);
                            let (transformed_tx, transformed_rx) =
                                mpsc::channel::<crate::urp::UrpStreamEvent>(64);

                            let retain_output_handle = {
                                let terminal_output = decoded_terminal_output.clone();
                                crate::request_capture::spawn_with_sse_capture(async move {
                                    retain_decoded_terminal_output(
                                        decoded_rx,
                                        retained_tx,
                                        terminal_output,
                                        namespace_aliases,
                                    )
                                    .await
                                })
                            };

                            let transform_handle =
                                crate::request_capture::spawn_with_sse_capture(async move {
                                    let reasoning_envelope = reasoning_envelope_for_transform
                                        .as_ref()
                                        .map(|(provider_type, upstream_model)| {
                                            (provider_type.as_str(), upstream_model.as_str())
                                        });
                                    transform_urp_stream(
                                        &state_for_transform,
                                        retained_rx,
                                        transformed_tx,
                                        &provider_rules_for_transform,
                                        &global_rules_for_transform,
                                        &auth_rules_for_transform,
                                        &model_for_transform,
                                        Some(transform_provider_type),
                                        reasoning_envelope,
                                    )
                                    .await
                                });

                            let (encode_input_rx, reconstruct_handle) =
                                match reconstructed_urp_response.clone() {
                                    Some(slot) => {
                                        let (tap_tx, tap_rx) =
                                            mpsc::channel::<crate::urp::UrpStreamEvent>(64);
                                        let handle = crate::request_capture::spawn_with_sse_capture(
                                            async move {
                                                retain_reconstructed_urp_response(
                                                    transformed_rx,
                                                    tap_tx,
                                                    slot,
                                                )
                                                .await
                                            },
                                        );
                                        (tap_rx, Some(handle))
                                    }
                                    None => (transformed_rx, None),
                                };

                            let history_for_commit = history_for_stream.clone();
                            let (encode_input_rx, history_handle) =
                                if let Some(history) = history_for_stream {
                                    let (history_tx, history_rx) = mpsc::channel(64);
                                    let handle = tokio::spawn(
                                        history.forward_stream(encode_input_rx, history_tx),
                                    );
                                    (history_rx, Some(handle))
                                } else {
                                    (encode_input_rx, None)
                                };
                            let encode_handle =
                                crate::request_capture::spawn_with_sse_capture(async move {
                                    encode_urp_stream(
                                        downstream,
                                        encode_input_rx,
                                        tx,
                                        &model_for_encode,
                                        started_at,
                                        sse_max_frame_length,
                                        mask_sensitive_info,
                                    )
                                    .await
                                });

                            let (
                                decode_result,
                                retain_output_result,
                                transform_result,
                                encode_result,
                            ) = tokio::join!(
                                decode_handle,
                                retain_output_handle,
                                transform_handle,
                                encode_handle
                            );
                            // The tap forwards every event, so it ends before
                            // encode does; this await never blocks the stream.
                            if let Some(handle) = reconstruct_handle {
                                let _ = handle.await;
                            }
                            let history_response = match history_handle {
                                Some(handle) => handle.await.ok().flatten(),
                                None => None,
                            };
                            let result = combine_stream_stage_results(
                                [
                                    decode_result,
                                    retain_output_result,
                                    transform_result,
                                    encode_result,
                                ]
                                .map(|result| {
                                    result.unwrap_or_else(|e| {
                                        Err(AppError::new(
                                            StatusCode::INTERNAL_SERVER_ERROR,
                                            "task_panic",
                                            e.to_string(),
                                        ))
                                    })
                                }),
                            );
                            if result.is_ok() {
                                if let (Some(history), Some(response)) =
                                    (history_for_commit, history_response)
                                {
                                    history.retain_response(&response).await;
                                }
                            }
                            result
                        };
                        let stream_result = if let Some(frames) = capture_frames_for_task.clone() {
                            crate::request_capture::with_sse_capture(frames, stream_future).await
                        } else {
                            stream_future.await
                        };
                        let settled_output = decoded_terminal_output.lock().await.clone();
                        let terminal_visible_output_bytes =
                            decoded_visible_output_bytes(&settled_output);
                        // RCD-D10a/RCD-D10b: None when the stream ended without
                        // a terminal response_done event.
                        let reconstructed_urp_response_value =
                            match reconstructed_urp_response.as_ref() {
                                Some(slot) => slot.lock().await.clone(),
                                None => None,
                            };

                        let (
                            ttfb_ms,
                            actual_upstream_usage,
                            usage,
                            is_estimated,
                            terminal_diagnostics,
                            response_service_tier,
                            response_model,
                        ) = {
                            let guard = runtime_metrics.lock().await;
                            let actual_upstream_usage = guard.usage.clone();
                            // MP-F3: a pass-through stream without upstream
                            // usage settles free under the effective
                            // `allow_free_when_missing_usage` flag (or the
                            // MP-F5 unpriced rule); otherwise it settles from
                            // the byte estimate with `estimated = true`.
                            let (usage, is_estimated) = match guard.usage.clone() {
                                Some(u) => (Some(u), false),
                                None if attempt_for_log.allow_free_when_missing_usage
                                    || attempt_for_log.model_price.is_none() =>
                                {
                                    (None, false)
                                }
                                None => {
                                    let visible_output_bytes = guard
                                        .visible_output_bytes
                                        .max(terminal_visible_output_bytes);
                                    let estimated_output_tokens =
                                        estimated_tokens_from_utf8_bytes(visible_output_bytes);
                                    tracing::warn!(
                                        estimated_input_tokens,
                                        estimated_output_tokens,
                                        "upstream stream ended without usage; billing from estimate"
                                    );
                                    (
                                        Some(urp::Usage {
                                            iterations: None,
                                            input_tokens: estimated_input_tokens,
                                            output_tokens: estimated_output_tokens,
                                            input_details: None,
                                            output_details: None,
                                            extra_body: std::collections::HashMap::new(),
                                        }),
                                        true,
                                    )
                                }
                            };
                            (
                                guard.ttfb_ms,
                                actual_upstream_usage,
                                usage,
                                is_estimated,
                                guard.terminal.clone(),
                                guard.response_service_tier.clone(),
                                guard.response_model.clone(),
                            )
                        };

                        if let Some(terminal_error) = terminal_diagnostics.terminal_error.clone() {
                            if let Some(failure_class) = midstream_terminal_failure_class(
                                terminal_error.http_status,
                                Some(&terminal_error.code),
                                terminal_error.error_type.as_deref(),
                            ) {
                                record_midstream_terminal_failure(
                                    &state_for_log,
                                    &attempt_for_log,
                                    failure_class,
                                )
                                .await;
                            }
                            spawn_request_log_stream_terminal_error(
                                &state_for_log,
                                &auth_for_log,
                                &attempt_for_log,
                                &model_for_log,
                                started_at,
                                request_id_for_log,
                                request_ip_for_log,
                                ttfb_ms,
                                terminal_error,
                                reasoning_effort_for_log,
                                tried_providers_for_log,
                                actual_upstream_usage.clone(),
                            );
                            if let Some(session) = capture_session.as_ref() {
                                let frames = if let Some(frames) = capture_frames_for_task.as_ref()
                                {
                                    Some(frames.snapshot().await)
                                } else {
                                    None
                                };
                                let error_json =
                                    terminal_diagnostics.terminal_error.as_ref().map(|err| {
                                        json!({
                                            "message": err.message,
                                            "code": err.code,
                                            "status": err.http_status,
                                        })
                                    });
                                session
                                    .push_attempt(crate::request_capture::build_attempt_dump(
                                        capture_attempt_number,
                                        &capture_provider_id,
                                        Some(&capture_channel_id),
                                        capture_provider_type,
                                        &capture_logical_model,
                                        &capture_upstream_model,
                                        &capture_path,
                                        capture_raw_input.as_ref().clone(),
                                        &capture_req_attempt,
                                        capture_upstream_body,
                                        None,
                                        reconstructed_urp_response_value.clone(),
                                        frames,
                                        capture_transform_chain_for_task,
                                        error_json,
                                    ))
                                    .await;
                                session
                                    .persist_with_result(actual_upstream_usage.as_ref(), true)
                                    .await;
                            }
                            return;
                        }

                        if let Err(ref err) = stream_result {
                            tracing::warn!("stream passthrough adapter failed: {}", err.message);
                            if is_upstream_adapter_failure(err) {
                                record_midstream_terminal_failure(
                                    &state_for_log,
                                    &attempt_for_log,
                                    RetryableFailureClass::Transient,
                                )
                                .await;
                            }
                            spawn_stream_attempt_error(
                                &state_for_log,
                                &auth_for_log,
                                &attempt_for_log,
                                &model_for_log,
                                started_at,
                                request_id_for_log,
                                request_ip_for_log,
                                ttfb_ms,
                                err,
                                reasoning_effort_for_log,
                                tried_providers_for_log,
                            );

                            emit_stream_error_if_needed(
                                downstream,
                                err,
                                &tx_err,
                                capture_frames_for_task.as_ref(),
                            )
                            .await;
                            if let Some(session) = capture_session.as_ref() {
                                let frames = if let Some(frames) = capture_frames_for_task.as_ref()
                                {
                                    Some(frames.snapshot().await)
                                } else {
                                    None
                                };
                                session
                                    .push_attempt(crate::request_capture::build_attempt_dump(
                                        capture_attempt_number,
                                        &capture_provider_id,
                                        Some(&capture_channel_id),
                                        capture_provider_type,
                                        &capture_logical_model,
                                        &capture_upstream_model,
                                        &capture_path,
                                        capture_raw_input.as_ref().clone(),
                                        &capture_req_attempt,
                                        capture_upstream_body,
                                        None,
                                        reconstructed_urp_response_value.clone(),
                                        frames,
                                        capture_transform_chain_for_task,
                                        Some(json!({
                                            "message": err.message,
                                            "code": err.code,
                                            "status": err.status.as_u16(),
                                        })),
                                    ))
                                    .await;
                                session
                                    .persist_with_result(actual_upstream_usage.as_ref(), true)
                                    .await;
                            }
                            return;
                        }

                        let settled_usage = match usage.as_ref() {
                            Some(u) if is_estimated => {
                                crate::settlement::SettledUsage::Estimated(u)
                            }
                            Some(u) => crate::settlement::SettledUsage::Reported(u),
                            None => crate::settlement::SettledUsage::MissingFree,
                        };
                        let charge = match maybe_charge_stream_usage(
                            &state_for_log,
                            &auth_for_log,
                            &attempt_for_log,
                            &model_for_log,
                            settled_usage,
                            &settled_output,
                            response_service_tier.as_deref(),
                            request_id_for_log.as_deref(),
                        )
                        .await
                        {
                            Ok(value) => value,
                            Err(err) => {
                                tracing::error!(
                                    code = %err.code,
                                    "failed to settle passthrough stream billing: {}",
                                    err.message
                                );
                                let terminal_error = StreamTerminalError {
                                    code: "billing_settlement_failed".to_string(),
                                    message: format!("{}: {}", err.code, err.message),
                                    http_status: err.status.as_u16(),
                                    error_type: Some("billing_error".to_string()),
                                    param: err.param.clone(),
                                };
                                spawn_request_log_stream_terminal_error(
                                    &state_for_log,
                                    &auth_for_log,
                                    &attempt_for_log,
                                    &model_for_log,
                                    started_at,
                                    request_id_for_log,
                                    request_ip_for_log,
                                    ttfb_ms,
                                    terminal_error,
                                    reasoning_effort_for_log,
                                    tried_providers_for_log,
                                    usage.clone(),
                                );
                                if let Some(session) = capture_session.as_ref() {
                                    session.persist_with_result(usage.as_ref(), true).await;
                                }
                                return;
                            }
                        };

                        refresh_channel_affinity(&state_for_log, &attempt_for_log).await;
                        spawn_request_log(
                            &state_for_log,
                            &auth_for_log,
                            &attempt_for_log,
                            &model_for_log,
                            usage,
                            charge.charge_nano_usd,
                            charge.billing_breakdown,
                            true,
                            started_at,
                            request_id_for_log,
                            request_ip_for_log,
                            channel_id_for_log,
                            ttfb_ms,
                            Some(terminal_diagnostics),
                            reasoning_effort_for_log,
                            tried_providers_for_log,
                            tx_err.is_closed(),
                            mismatched_upstream_response_model(
                                &capture_upstream_model,
                                response_model.as_deref().unwrap_or(""),
                            ),
                        );

                        if let Some(session) = capture_session.as_ref() {
                            let frames = if let Some(frames) = capture_frames_for_task.as_ref() {
                                Some(frames.snapshot().await)
                            } else {
                                None
                            };
                            session
                                .push_attempt(crate::request_capture::build_attempt_dump(
                                    capture_attempt_number,
                                    &capture_provider_id,
                                    Some(&capture_channel_id),
                                    capture_provider_type,
                                    &capture_logical_model,
                                    &capture_upstream_model,
                                    &capture_path,
                                    capture_raw_input.as_ref().clone(),
                                    &capture_req_attempt,
                                    capture_upstream_body,
                                    None,
                                    reconstructed_urp_response_value.clone(),
                                    frames,
                                    capture_transform_chain_for_task,
                                    None,
                                ))
                                .await;
                            session
                                .persist_with_result(actual_upstream_usage.as_ref(), false)
                                .await;
                        }
                    });
                    return Ok(receiver_event_stream(rx));
                }
                Err(err) => {
                    if let Some(session) = capture.session.as_ref() {
                        session
                            .push_attempt(crate::request_capture::build_attempt_dump(
                                attempt_number,
                                &attempt.provider_id,
                                Some(&attempt.channel_id),
                                attempt.provider_type,
                                &logical_model,
                                &req_attempt.model,
                                &path,
                                capture.raw_input.as_ref().clone(),
                                &req_attempt,
                                capture_upstream_request.clone(),
                                None,
                                None,
                                None,
                                capture_transform_chain.clone(),
                                Some(json!({
                                    "message": err.message,
                                    "code": err.code,
                                    "status": err.status.map(|status| status.as_u16()),
                                })),
                            ))
                            .await;
                    }
                    let same_channel_retryable = is_same_channel_retryable_error(&err);
                    let passive_failure_class =
                        same_channel_retryable.then(|| classify_retryable_failure(&err));
                    let mask_sensitive_info =
                        state.monoize_runtime.read().await.mask_sensitive_info;
                    let app_err = upstream_error_to_app(err, mask_sensitive_info);
                    record_upstream_attempt_failure(
                        &state,
                        &attempt,
                        attempt_number,
                        &app_err,
                        passive_failure_class,
                        &mut tried_providers,
                        &mut execution_state,
                    )
                    .await;
                    last_failed_attempt = Some(attempt.clone());
                    if allow_same_channel_retry(
                        &state,
                        &attempt,
                        &execution_state,
                        channel_attempt + 1,
                        passive_failure_class,
                    )
                    .await
                    {
                        maybe_sleep_before_channel_retry(&attempt).await;
                        continue;
                    }
                    break;
                }
            }
        }
    }
    let final_err = build_exhausted_upstream_error(&logical_model, &tried_providers);
    if let Some(attempt) = last_failed_attempt {
        let terminal_error = stream_terminal_error_from_app(&final_err);
        spawn_request_log_stream_terminal_error(
            &state,
            &auth,
            &attempt,
            &logical_model,
            started_at,
            request_id,
            request_ip,
            None,
            terminal_error,
            req.reasoning.as_ref().and_then(|r| r.effort.clone()),
            tried_providers,
            None,
        );
    } else {
        spawn_request_log_error_no_attempt(
            &state,
            &auth,
            &logical_model,
            true,
            started_at,
            request_id,
            request_ip,
            &final_err,
            req.reasoning.as_ref().and_then(|r| r.effort.clone()),
            tried_providers,
        );
    }
    if let Some(session) = capture.session.as_ref() {
        session.persist_with_result(None, true).await;
    }
    Ok(prestream_error_stream(downstream, final_err))
}
