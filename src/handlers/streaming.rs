use super::*;
use crate::urp::stream_decode::stream_upstream_to_urp_events;
use crate::urp::stream_encode::encode_urp_stream;

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
    let transform_match_model =
        normalized_logical_model_for_matching(&state, &requested_model).await;
    inject_monoize_context(&auth, &mut req);
    apply_transform_rules_request(&state, &mut req, &auth.transforms, &transform_match_model)
        .await?;
    strip_monoize_context(&mut req);
    resolve_model_suffix(&state, &mut req).await;
    let logical_model = req.model.clone();
    let routing_stub = build_routing_stub(&req, max_multiplier);
    let attempts =
        build_monoize_attempts(&state, &routing_stub, auth.effective_groups.clone()).await?;
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

    let mut execution_state = AttemptExecutionState::default();

    for attempt in attempts {
        execution_state.enter_provider(&attempt.provider_id);
        if !execution_state.provider_budget_remaining(&attempt) {
            continue;
        }

        let sse_max_frame_length = effective_sse_max_frame_length(
            &attempt.provider_transforms,
            &auth.transforms,
            &logical_model,
        );
        let requires_buffered_stream = requires_buffered_response_stream(
            &attempt.provider_transforms,
            &auth.transforms,
            &logical_model,
            downstream,
        ) || attempt.provider_type == ProviderType::OpenaiImage
            || attempt.provider_type == ProviderType::Replicate;
        let max_channel_attempts = (attempt.channel_max_retries + 1).max(1) as usize;

        for channel_attempt in 0..max_channel_attempts {
            if !execution_state.provider_budget_remaining(&attempt) {
                break;
            }

            let attempt_number = execution_state.record_upstream_attempt();
            let mut req_attempt = req.clone();
            req_attempt.model = attempt.upstream_model.clone();
            if attempt.strip_cross_protocol_nested_extra
                && !downstream.is_same_family(attempt.provider_type)
            {
                urp::strip_nested_extra_body(&mut req_attempt.input);
            }
            apply_transform_rules_request(
                &state,
                &mut req_attempt,
                &attempt.provider_transforms,
                &transform_match_model,
            )
            .await?;

            if requires_buffered_stream {
                let mut nonstream_req = req_attempt.clone();
                nonstream_req.stream = Some(false);
                let upstream_body = encode_request_for_provider(&mut nonstream_req, &attempt)?;
                let provider = build_channel_provider_config(&attempt);
                let path =
                    upstream_path_for_model(attempt.provider_type, &req_attempt.model, false);
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
                        let mut resp = decode_response_from_provider(
                            attempt.provider_type,
                            &value,
                            &nonstream_req.model,
                        )?;
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
                        if attempt.provider_type == ProviderType::OpenaiImage
                            && !matches!(downstream, DownstreamProtocol::Responses)
                        {
                            convert_assistant_images_to_markdown(&mut resp);
                        }
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
                            let stream_result =
                                crate::urp::stream_encode::emit_synthetic_stream_from_urp_response(
                                    downstream,
                                    &logical_model_for_stream,
                                    &resp,
                                    sse_max_frame_length,
                                    tx,
                                )
                                .await;
                            if let Err(err) = stream_result {
                                tracing::warn!("synthetic stream failed: {}", err.message);
                                if matches!(
                                    downstream,
                                    DownstreamProtocol::ChatCompletions
                                        | DownstreamProtocol::Responses
                                ) {
                                    let _ = tx_err.send(Event::default().data("[DONE]")).await;
                                }
                            }
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
                                attempt_number,
                                provider_id: attempt.provider_id.clone(),
                                channel_id: attempt.channel_id.clone(),
                                error: app_err.message.clone(),
                            });
                            mark_channel_retryable_failure(
                                &state,
                                &attempt,
                                retryable_failure_class,
                            )
                            .await;
                            last_failed_attempt = Some(attempt.clone());
                            if !is_attempt_channel_healthy(&state, &attempt).await {
                                break;
                            }
                            if execution_state.provider_budget_remaining(&attempt) {
                                if channel_attempt + 1 < max_channel_attempts {
                                    maybe_sleep_before_channel_retry(&attempt).await;
                                }
                                continue;
                            }
                            break;
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

            let upstream_body = encode_request_for_provider(&mut req_attempt, &attempt)?;
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
                attempt.request_timeout_ms.saturating_mul(10).max(600_000),
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
                    let pending_request_envelope_extra = req.input
                    .clone()
                    .into_iter()
                    .find_map(|node| match node {
                        crate::urp::Node::NextDownstreamEnvelopeExtra { extra_body }
                            if !extra_body.is_empty() =>
                        {
                            Some(extra_body)
                        }
                        _ => None,
                    });
                    let provider_type = attempt.provider_type;
                    let (tx, rx) = mpsc::channel::<Event>(64);
                    let runtime_metrics = Arc::new(Mutex::new(StreamRuntimeMetrics {
                        ttfb_ms: None,
                        usage: None,
                        terminal: StreamTerminalDiagnostics::default(),
                        estimated_output_tokens: 0,
                    }));
                    let metrics_for_stream = runtime_metrics.clone();
                    let state_for_log = state.clone();
                    let auth_for_log = auth.clone();
                    let attempt_for_log = attempt.clone();
                    let model_for_log = logical_model.clone();
                    let model_for_encode = logical_model.clone();
                    let model_for_transform = logical_model.clone();
                    let request_id_for_log = request_id.clone();
                    let request_ip_for_log = request_ip.clone();
                    let channel_id_for_log = attempt.channel_id.clone();
                    let reasoning_effort_for_log =
                        req.reasoning.as_ref().and_then(|r| r.effort.clone());
                    let tried_providers_for_log = tried_providers.clone();
                    let enable_estimated_billing = state
                        .monoize_runtime
                        .read()
                        .await
                        .enable_estimated_billing;
                    let state_for_transform = state.clone();
                    let provider_rules_for_transform = attempt.provider_transforms.clone();
                    let auth_rules_for_transform = auth.transforms.clone();
                    tokio::spawn(async move {
                        let tx_err = tx.clone();
                        let stream_result = {
                            let (decoded_tx, decoded_rx) =
                                mpsc::channel::<crate::urp::UrpStreamEvent>(64);
                            let (transformed_tx, transformed_rx) =
                                mpsc::channel::<crate::urp::UrpStreamEvent>(64);

                            let decode_handle = {
                                let metrics = metrics_for_stream.clone();
                                tokio::spawn(async move {
                                    stream_upstream_to_urp_events(
                                        &legacy,
                                        pending_request_envelope_extra,
                                        provider_type,
                                        upstream_resp,
                                        decoded_tx,
                                        Some(started_at),
                                        Some(metrics),
                                    )
                                    .await
                                })
                            };

                            let transform_handle = tokio::spawn(async move {
                                transform_urp_stream(
                                    &state_for_transform,
                                    decoded_rx,
                                    transformed_tx,
                                    &provider_rules_for_transform,
                                    &auth_rules_for_transform,
                                    &model_for_transform,
                                )
                                .await
                            });

                            let encode_handle = {
                                let tx = tx;
                                tokio::spawn(async move {
                                    encode_urp_stream(
                                        downstream,
                                        transformed_rx,
                                        tx,
                                        &model_for_encode,
                                        sse_max_frame_length,
                                    )
                                    .await
                                })
                            };

                            let (decode_result, transform_result, encode_result) =
                                tokio::join!(decode_handle, transform_handle, encode_handle);
                            decode_result
                                .unwrap_or_else(|e| {
                                    Err(AppError::new(
                                        StatusCode::INTERNAL_SERVER_ERROR,
                                        "task_panic",
                                        e.to_string(),
                                    ))
                                })
                                .and(transform_result.unwrap_or_else(|e| {
                                    Err(AppError::new(
                                        StatusCode::INTERNAL_SERVER_ERROR,
                                        "task_panic",
                                        e.to_string(),
                                    ))
                                }))
                                .and(encode_result.unwrap_or_else(|e| {
                                    Err(AppError::new(
                                        StatusCode::INTERNAL_SERVER_ERROR,
                                        "task_panic",
                                        e.to_string(),
                                    ))
                                }))
                        };

                        let (ttfb_ms, usage, is_estimated, terminal_diagnostics) = {
                            let guard = runtime_metrics.lock().await;
                            let (usage, is_estimated) =
                                match guard.usage.clone() {
                                    Some(u) => (Some(u), false),
                                    None if enable_estimated_billing
                                        && guard.estimated_output_tokens > 0 =>
                                    {
                                        tracing::warn!(
                                            estimated_output_tokens =
                                                guard.estimated_output_tokens,
                                            "upstream stream ended without usage; billing from estimate"
                                        );
                                        (
                                            Some(urp::Usage {
                                                input_tokens: 0,
                                                output_tokens: guard.estimated_output_tokens,
                                                input_details: None,
                                                output_details: None,
                                                extra_body: std::collections::HashMap::new(),
                                            }),
                                            true,
                                        )
                                    }
                                    _ => (None, false),
                                };
                            (guard.ttfb_ms, usage, is_estimated, guard.terminal.clone())
                        };

                        let mut charge = match usage.as_ref() {
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
                        if is_estimated {
                            if let Some(ref mut breakdown) = charge.billing_breakdown {
                                if let Some(obj) = breakdown.as_object_mut() {
                                    obj.insert(
                                        "estimated".to_string(),
                                        serde_json::Value::Bool(true),
                                    );
                                }
                            }
                        }

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

                        let stream_failed = stream_result.is_err();
                        if let Err(ref err) = stream_result {
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
                                    let _ = tx_err
                                        .send(
                                            Event::default()
                                                .event("error")
                                                .data(error_json.to_string()),
                                        )
                                        .await;
                                }
                                DownstreamProtocol::ChatCompletions => {
                                    let _ = tx_err
                                        .send(Event::default().data(error_json.to_string()))
                                        .await;
                                }
                                DownstreamProtocol::AnthropicMessages => {
                                    let _ = tx_err
                                        .send(Event::default().event("error").data(
                                            json!({"type": "error", "error": {"type": err.code, "message": err.message}}).to_string()
                                        ))
                                        .await;
                                }
                            }
                        }
                        if stream_failed
                            && matches!(
                                downstream,
                                DownstreamProtocol::ChatCompletions | DownstreamProtocol::Responses
                            )
                        {
                            let _ = tx_err.send(Event::default().data("[DONE]")).await;
                        }
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
                            attempt_number,
                            provider_id: attempt.provider_id.clone(),
                            channel_id: attempt.channel_id.clone(),
                            error: app_err.message.clone(),
                        });
                        mark_channel_retryable_failure(&state, &attempt, retryable_failure_class)
                            .await;
                        last_failed_attempt = Some(attempt.clone());
                        if !is_attempt_channel_healthy(&state, &attempt).await {
                            break;
                        }
                        if execution_state.provider_budget_remaining(&attempt) {
                            if channel_attempt + 1 < max_channel_attempts {
                                maybe_sleep_before_channel_retry(&attempt).await;
                            }
                            continue;
                        }
                        break;
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
    }
    let final_err = AppError::new(
        StatusCode::BAD_GATEWAY,
        "upstream_error",
        build_exhausted_error_message(&logical_model, &tried_providers),
    );
    if let Some(attempt) = last_failed_attempt {
        spawn_request_log_error(
            &state,
            &auth,
            &attempt,
            &logical_model,
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
    Err(final_err)
}
