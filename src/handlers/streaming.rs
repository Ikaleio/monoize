use super::*;
use crate::urp::stream_decode::stream_upstream_to_urp_events;
use crate::urp::stream_encode::{emit_synthetic_stream_from_urp_response, encode_urp_stream};

pub(super) async fn forward_stream_typed(
    state: AppState,
    auth: crate::auth::AuthResult,
    mut req: urp::UrpRequest,
    max_multiplier: Option<f64>,
    downstream: DownstreamProtocol,
    request_id: Option<String>,
    request_ip: Option<String>,
) -> AppResult<
    impl futures_util::Stream<Item = Result<Event, std::convert::Infallible>> + Send + 'static,
> {
    let started_at = std::time::Instant::now();
    let mut last_failed_attempt: Option<MonoizeAttempt> = None;
    let mut tried_providers: Vec<TriedProvider> = Vec::new();
    let requested_model = req.model.clone();
    let transform_match_model = normalized_logical_model_for_matching(&state, &requested_model).await;
    inject_monoize_context(&auth, &mut req);
    apply_transform_rules_request(&state, &mut req, &auth.transforms, &transform_match_model).await?;
    strip_monoize_context(&mut req);
    resolve_model_suffix(&state, &mut req).await;
    let logical_model = req.model.clone();
    let routing_stub = build_routing_stub(&req, max_multiplier);
    let attempts = build_monoize_attempts(&state, &routing_stub).await?;
    insert_pending_request_log(
        &state,
        &auth,
        &req.model,
        true,
        request_id.as_deref(),
        request_ip.as_deref(),
        started_at,
    )
    .await;

    for attempt in attempts {
        let mut req_attempt = req.clone();
        req_attempt.model = attempt.upstream_model.clone();
        apply_transform_rules_request(
            &state,
            &mut req_attempt,
            &attempt.provider_transforms,
            &transform_match_model,
        )
        .await?;
        ensure_stream_usage_requested(&mut req_attempt, attempt.provider_type);
        let need_response_transform_stream =
            has_enabled_response_rules(&attempt.provider_transforms, &logical_model)
                || has_enabled_response_rules(&auth.transforms, &logical_model);
        let sse_max_frame_length = effective_sse_max_frame_length(
            &attempt.provider_transforms,
            &auth.transforms,
            &logical_model,
        );

        if need_response_transform_stream {
            let mut nonstream_req = req_attempt.clone();
            nonstream_req.stream = Some(false);
            let upstream_body = encode_request_for_provider(&nonstream_req, &attempt)?;
            let provider = build_channel_provider_config(&attempt);
            let path = upstream_path_for_model(attempt.provider_type, &req_attempt.model, false);
            log_outgoing_request_shape(
                request_id.as_deref(),
                &logical_model,
                &nonstream_req.model,
                attempt.provider_type,
                false,
                &path,
                &upstream_body,
                &nonstream_req,
            );
            let call = upstream::call_upstream_with_timeout_and_headers(
                client_http(&state),
                &provider,
                &attempt.api_key,
                &path,
                &upstream_body,
                attempt.request_timeout_ms,
                provider_extra_headers(attempt.provider_type),
            )
            .await;
            match call {
                Ok(value) => {
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
                    let mut resp = decode_response_from_provider(attempt.provider_type, &value)?;
                    apply_transform_rules_response(
                        &state,
                        &mut resp,
                        &attempt.provider_transforms,
                        &logical_model,
                    )
                    .await?;
                    apply_transform_rules_response(
                        &state,
                        &mut resp,
                        &auth.transforms,
                        &logical_model,
                    )
                    .await?;
                    let charge =
                        maybe_charge_response(&state, &auth, &attempt, &logical_model, &resp)
                            .await?;
                    spawn_request_log(
                        &state,
                        &auth,
                        &attempt,
                        &logical_model,
                        resp.usage.clone(),
                        charge.charge_nano_usd,
                        charge.billing_breakdown,
                        true,
                        started_at,
                        request_id.clone(),
                        request_ip.clone(),
                        attempt.channel_id.clone(),
                        Some(started_at.elapsed().as_millis() as u64),
                        None,
                        req.reasoning.as_ref().and_then(|r| r.effort.clone()),
                        tried_providers,
                    );
                    let (tx, rx) = mpsc::channel::<Event>(64);
                    let logical_model_for_stream = logical_model.clone();
                    tokio::spawn(async move {
                        let tx_err = tx.clone();
                        if let Err(err) = emit_synthetic_stream_from_urp_response(
                            downstream,
                            &logical_model_for_stream,
                            &resp,
                            sse_max_frame_length,
                            tx,
                        )
                        .await
                        {
                            tracing::warn!("synthetic stream failed: {}", err.message);
                        }
                        // Always terminate the SSE stream.  emit_synthetic_chat_stream
                        // already sends [DONE], but the responses and messages variants
                        // do not — the duplicate is harmless.
                        let _ = tx_err.send(Event::default().data("[DONE]")).await;
                    });
                    return Ok(tokio_stream::wrappers::ReceiverStream::new(rx).map(Ok));
                }
                Err(err) => {
                    let non_retryable = is_non_retryable_client_error(&err);
                    let retryable = is_retryable_error(&err);
                    let retryable_failure_class = classify_retryable_failure(&err);
                    let app_err = upstream_error_to_app(err);
                    if non_retryable {
                        spawn_request_log_error(
                            &state,
                            &auth,
                            &attempt,
                            &logical_model,
                            true,
                            started_at,
                            request_id.clone(),
                            request_ip.clone(),
                            &app_err,
                            req.reasoning.as_ref().and_then(|r| r.effort.clone()),
                            tried_providers,
                        );
                        return Err(app_err);
                    }
                    if retryable {
                        tried_providers.push(TriedProvider {
                            provider_id: attempt.provider_id.clone(),
                            channel_id: attempt.channel_id.clone(),
                            error: app_err.message.clone(),
                        });
                        mark_channel_retryable_failure(&state, &attempt, retryable_failure_class)
                            .await;
                        last_failed_attempt = Some(attempt.clone());
                        continue;
                    }
                    spawn_request_log_error(
                        &state,
                        &auth,
                        &attempt,
                        &logical_model,
                        true,
                        started_at,
                        request_id.clone(),
                        request_ip.clone(),
                        &app_err,
                        req.reasoning.as_ref().and_then(|r| r.effort.clone()),
                        tried_providers,
                    );
                    return Err(app_err);
                }
            }
        }

        let upstream_body = encode_request_for_provider(&req_attempt, &attempt)?;
        let provider = build_channel_provider_config(&attempt);
        let path = upstream_path_for_model(attempt.provider_type, &req_attempt.model, true);
        log_outgoing_request_shape(
            request_id.as_deref(),
            &logical_model,
            &req_attempt.model,
            attempt.provider_type,
            true,
            &path,
            &upstream_body,
            &req_attempt,
        );
        let call = upstream::call_upstream_raw_with_timeout_and_headers(
            client_http(&state),
            &provider,
            &attempt.api_key,
            &path,
            &upstream_body,
            attempt.request_timeout_ms,
            provider_extra_headers(attempt.provider_type),
        )
        .await;
        match call {
            Ok(upstream_resp) => {
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
                let legacy = typed_request_to_legacy(&req_attempt, max_multiplier)?;
                let provider_type = attempt.provider_type;
                let (tx, rx) = mpsc::channel::<Event>(64);
                let runtime_metrics = Arc::new(Mutex::new(StreamRuntimeMetrics {
                    ttfb_ms: None,
                    usage: None,
                    terminal: StreamTerminalDiagnostics::default(),
                }));
                let metrics_for_stream = runtime_metrics.clone();
                let state_for_log = state.clone();
                let auth_for_log = auth.clone();
                let attempt_for_log = attempt.clone();
                let model_for_log = logical_model.clone();
                let model_for_encode = logical_model.clone();
                let request_id_for_log = request_id.clone();
                let request_ip_for_log = request_ip.clone();
                let channel_id_for_log = attempt.channel_id.clone();
                let reasoning_effort_for_log =
                    req.reasoning.as_ref().and_then(|r| r.effort.clone());
                let tried_providers_for_log = tried_providers.clone();
                tokio::spawn(async move {
                    let tx_err = tx.clone();
                    let (urp_tx, urp_rx) = mpsc::channel::<crate::urp::UrpStreamEvent>(64);

                    let decode_handle = {
                        let metrics = metrics_for_stream.clone();
                        tokio::spawn(async move {
                            stream_upstream_to_urp_events(
                                &legacy,
                                provider_type,
                                upstream_resp,
                                urp_tx,
                                Some(started_at),
                                Some(metrics),
                            )
                            .await
                        })
                    };

                    let encode_handle = {
                        let tx = tx;
                        tokio::spawn(async move {
                            encode_urp_stream(
                                downstream,
                                urp_rx,
                                tx,
                                &model_for_encode,
                                sse_max_frame_length,
                            )
                            .await
                        })
                    };

                    let (decode_result, encode_result) = tokio::join!(decode_handle, encode_handle);
                    let stream_result = decode_result
                        .unwrap_or_else(|e| {
                            Err(AppError::new(
                                StatusCode::INTERNAL_SERVER_ERROR,
                                "task_panic",
                                e.to_string(),
                            ))
                        })
                        .and(encode_result.unwrap_or_else(|e| {
                            Err(AppError::new(
                                StatusCode::INTERNAL_SERVER_ERROR,
                                "task_panic",
                                e.to_string(),
                            ))
                        }));

                    let (ttfb_ms, usage, terminal_diagnostics) = {
                        let guard = runtime_metrics.lock().await;
                        (guard.ttfb_ms, guard.usage.clone(), guard.terminal.clone())
                    };

                    let charge = match usage.as_ref() {
                        Some(usage_row) => match maybe_charge_usage(
                            &state_for_log,
                            &auth_for_log,
                            &attempt_for_log,
                            &model_for_log,
                            usage_row,
                        )
                        .await
                        {
                            Ok(v) => v,
                            Err(err) => {
                                tracing::error!(
                                    "failed to charge passthrough stream request: {}",
                                    err.message
                                );
                                ChargeComputation::default()
                            }
                        },
                        None => ChargeComputation::default(),
                    };

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
                    );

                    if let Err(err) = stream_result {
                        tracing::warn!("stream passthrough adapter failed: {}", err.message);
                        let error_json = json!({
                            "error": {
                                "message": err.message,
                                "type": err.error_type,
                                "code": err.code,
                                "param": err.param,
                            }
                        });
                        match downstream {
                            DownstreamProtocol::Responses => {
                                let _ = tx_err.send(
                                        Event::default()
                                            .event("error")
                                            .data(error_json.to_string()),
                                ).await;
                            }
                            DownstreamProtocol::ChatCompletions => {
                                let _ = tx_err
                                    .send(Event::default().data(error_json.to_string()))
                                    .await;
                            }
                            DownstreamProtocol::AnthropicMessages => {
                                let _ = tx_err.send(
                                    Event::default().event("error").data(
                                        json!({"type": "error", "error": {"type": err.code, "message": err.message}}).to_string()
                                    )
                                ).await;
                            }
                        }
                    }
                    // Always send [DONE] to terminate the SSE stream, whether the
                    // adapter succeeded or failed.  Several adapter functions
                    // (all *_as_responses and *_as_messages variants) do not emit
                    // [DONE] themselves; the duplicate is harmless for those that do.
                    let _ = tx_err.send(Event::default().data("[DONE]")).await;
                });
                return Ok(tokio_stream::wrappers::ReceiverStream::new(rx).map(Ok));
            }
            Err(err) => {
                let non_retryable = is_non_retryable_client_error(&err);
                let retryable = is_retryable_error(&err);
                let retryable_failure_class = classify_retryable_failure(&err);
                let app_err = upstream_error_to_app(err);
                if non_retryable {
                    spawn_request_log_error(
                        &state,
                        &auth,
                        &attempt,
                        &logical_model,
                        true,
                        started_at,
                        request_id.clone(),
                        request_ip.clone(),
                        &app_err,
                        req.reasoning.as_ref().and_then(|r| r.effort.clone()),
                        tried_providers,
                    );
                    return Err(app_err);
                }
                if retryable {
                    tried_providers.push(TriedProvider {
                        provider_id: attempt.provider_id.clone(),
                        channel_id: attempt.channel_id.clone(),
                        error: app_err.message.clone(),
                    });
                    mark_channel_retryable_failure(&state, &attempt, retryable_failure_class).await;
                    last_failed_attempt = Some(attempt.clone());
                    continue;
                }
                spawn_request_log_error(
                    &state,
                    &auth,
                    &attempt,
                    &logical_model,
                    true,
                    started_at,
                    request_id.clone(),
                    request_ip.clone(),
                    &app_err,
                    req.reasoning.as_ref().and_then(|r| r.effort.clone()),
                    tried_providers,
                );
                return Err(app_err);
            }
        }
    }
    let final_err = AppError::new(
        StatusCode::BAD_GATEWAY,
        "upstream_error",
        build_exhausted_error_message(&req.model, &tried_providers),
    );
    if let Some(attempt) = last_failed_attempt {
        spawn_request_log_error(
            &state,
            &auth,
            &attempt,
            &req.model,
            true,
            started_at,
            request_id,
            request_ip,
            &final_err,
            req.reasoning.as_ref().and_then(|r| r.effort.clone()),
            tried_providers,
        );
    } else {
        spawn_request_log_error_no_attempt(
            &state,
            &auth,
            &req.model,
            true,
            started_at,
            request_id,
            request_ip,
            &final_err,
            req.reasoning.as_ref().and_then(|r| r.effort.clone()),
            tried_providers,
        );
    }
    Err(final_err)
}
