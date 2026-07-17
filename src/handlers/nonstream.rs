use super::*;
use crate::urp::stream_decode::stream_upstream_to_urp_events;
use std::collections::HashSet;

pub(crate) fn strip_orphaned_tool_calls(req: &mut urp::UrpRequest) {
    let calls: HashSet<String> = req
        .input
        .iter()
        .filter_map(|node| match node {
            urp::Node::ToolCall { call_id, .. } => Some(call_id.clone()),
            _ => None,
        })
        .collect();
    let answered: HashSet<String> = req
        .input
        .iter()
        .filter_map(|node| match node {
            urp::Node::ToolResult { call_id, .. } => Some(call_id.clone()),
            _ => None,
        })
        .collect();
    req.input.retain_mut(|node| match node {
        urp::Node::ToolCall { call_id, .. } => answered.contains(&*call_id),
        urp::Node::ToolResult { call_id, .. } => calls.contains(&*call_id),
        urp::Node::NextDownstreamEnvelopeExtra { .. } => true,
        _ => true,
    });
}

pub(super) async fn execute_nonstream_typed(
    state: &AppState,
    auth: &crate::auth::AuthResult,
    mut req: urp::UrpRequest,
    max_multiplier: Option<f64>,
    downstream: DownstreamProtocol,
    request_id: Option<String>,
    request_ip: Option<String>,
    capture: RequestCaptureContext,
) -> AppResult<(urp::UrpResponse, String)> {
    let started_at = std::time::Instant::now();
    let requested_model = req.model.clone();
    let transform_match_model =
        normalized_logical_model_for_matching(state, &requested_model).await;
    resolve_model_suffix(state, &mut req).await;
    // Preserve the suffix-normalized request so each per-attempt iteration can
    // re-derive the transformed request from a pristine base. This matters
    // because cross-family strip runs BEFORE all transforms per-attempt
    // (auto_cache_* etc. must observe the stripped request so their cache
    // breakpoints actually survive into the upstream encoding).
    let original_req = req.clone();
    let logical_model = req.model.clone();
    let routing_stub = build_routing_stub(&req, max_multiplier);
    let attempts = build_monoize_attempts(state, &routing_stub, auth).await?;
    ensure_balance_before_forward_for_attempts(state, auth, &attempts).await?;
    let _pending_request_log_guard = insert_pending_request_log(
        state,
        auth,
        &req.model,
        false,
        request_id.as_deref(),
        request_ip.as_deref(),
        started_at,
    )
    .await;
    let mut last_failed_attempt: Option<MonoizeAttempt> = None;
    let mut tried_providers: Vec<TriedProvider> = Vec::new();
    let mut execution_state = AttemptExecutionState::default();
    for attempt in attempts {
        execution_state.enter_provider(&attempt.provider_id);
        if !execution_state.provider_budget_remaining(&attempt) {
            continue;
        }

        let max_channel_attempts = (attempt.channel_max_retries + 1).max(1) as usize;
        for channel_attempt in 0..max_channel_attempts {
            if !execution_state.provider_budget_remaining(&attempt) {
                break;
            }

            let attempt_number = execution_state.record_upstream_attempt();
            // Clone from the pristine original request (pre-transforms) so
            // that the cross-family strip can run BEFORE provider, global,
            // and API-key transforms. This guarantees that transforms which
            // inject upstream-specific part-level metadata (e.g.
            // `auto_cache_system`, `auto_cache_tool_use`) survive into the
            // encoded upstream request even when the downstream and upstream
            // protocol families differ.
            let mut req_attempt = original_req.clone();
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
            inject_monoize_context(auth, &mut req_attempt);
            req_attempt.model = attempt.upstream_model.clone();
            // Unwrap mz2 reasoning envelopes BEFORE any request-phase transform
            // observes the request input. Per spec/urp-transform-system.spec.md
            // PIPE-1 step 6 and PIPE-1d, transforms must not see encrypted
            // reasoning replays still in `mz2.` envelope form, and they must not
            // be allowed to mutate the reasoning payload before envelope-bound
            // provider/model checks (PR4c.6) decide whether to keep or drop the
            // replayed reasoning node for this attempt.
            urp::filter_and_unwrap_reasoning_envelopes_for_upstream(
                &mut req_attempt.input,
                reasoning_envelope_provider_type(attempt.provider_type),
                &req_attempt.model,
                auth.reasoning_envelope_enabled,
            );
            apply_transform_rules_request(
                state,
                &mut req_attempt,
                &attempt.provider_transforms,
                &transform_match_model,
                Some(attempt.provider_type),
            )
            .await?;
            let global_transforms = state.monoize_runtime.read().await.global_transforms.clone();
            apply_transform_rules_request(
                state,
                &mut req_attempt,
                &global_transforms,
                &transform_match_model,
                Some(attempt.provider_type),
            )
            .await?;
            apply_transform_rules_request(
                state,
                &mut req_attempt,
                &auth.transforms,
                &transform_match_model,
                Some(attempt.provider_type),
            )
            .await?;
            strip_monoize_context(&mut req_attempt);

            let upstream_body =
                encode_request_for_provider(&mut req_attempt, &attempt, downstream)?;
            let provider = build_channel_provider_config(&attempt);
            let openai_image_edit = attempt.provider_type == ProviderType::OpenaiImage
                && urp::encode::openai_image::has_user_image_input(&req_attempt);
            let path = if openai_image_edit {
                "/v1/images/edits".to_string()
            } else {
                upstream_path_for_model(
                    attempt.provider_type,
                    &req_attempt.model,
                    req_attempt.stream.unwrap_or(false),
                )
            };
            let call_value = if req_attempt.stream == Some(true)
                && supports_nonstream_upstream_stream_collection(attempt.provider_type)
            {
                let stream_idle_timeout_ms = state
                    .monoize_runtime
                    .read()
                    .await
                    .stream_idle_timeout_ms
                    .max(1);
                let call = upstream::call_upstream_raw_with_timeout_and_headers(
                    client_http(state),
                    &provider,
                    &attempt.api_key,
                    &path,
                    &upstream_body,
                    attempt.request_timeout_ms.saturating_mul(10).max(600_000),
                    provider_extra_headers(attempt.provider_type, &upstream_body),
                )
                .await;
                match call {
                    Ok(upstream_resp) => match collect_streamed_upstream_response(
                        &req_attempt,
                        max_multiplier,
                        attempt.provider_type,
                        upstream_resp,
                        started_at,
                        &logical_model,
                        stream_idle_timeout_ms,
                    )
                    .await
                    {
                        Ok(resp) => Ok((None, Some(resp))),
                        Err(err) => return Err(err),
                    },
                    Err(err) => Err(err),
                }
            } else if openai_image_edit {
                let form =
                    urp::encode::openai_image::multipart_form(&req_attempt, &req_attempt.model)
                        .map_err(|e| {
                            AppError::new(StatusCode::BAD_REQUEST, "invalid_request", e)
                        })?;
                match upstream::call_upstream_multipart_with_timeout_and_headers(
                    client_http(state),
                    &provider,
                    &attempt.api_key,
                    &path,
                    form,
                    attempt.request_timeout_ms,
                    provider_extra_headers(attempt.provider_type, &upstream_body),
                )
                .await
                {
                    Ok(resp) => {
                        let status = resp.status();
                        match resp.text().await {
                            Ok(text) => serde_json::from_str::<Value>(&text)
                                .map(|value| (Some(value), None))
                                .map_err(|err| {
                                    upstream::UpstreamCallError::new(
                                        upstream::UpstreamErrorKind::Http,
                                        Some(status),
                                        err.to_string(),
                                    )
                                }),
                            Err(err) => Err(upstream::UpstreamCallError::new(
                                upstream::UpstreamErrorKind::Network,
                                Some(status),
                                err.to_string(),
                            )),
                        }
                    }
                    Err(err) => Err(err),
                }
            } else {
                upstream::call_upstream_with_timeout_and_headers(
                    client_http(state),
                    &provider,
                    &attempt.api_key,
                    &path,
                    &upstream_body,
                    attempt.request_timeout_ms,
                    provider_extra_headers(attempt.provider_type, &upstream_body),
                )
                .await
                .map(|value| (Some(value), None))
            };
            match call_value {
                Ok((value, collected_resp)) => {
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
                                capture.raw_input.clone(),
                                &req_attempt,
                                upstream_body.clone(),
                                value.clone(),
                                None,
                                None,
                            ))
                            .await;
                    }
                    update_pending_channel_info(
                        state,
                        auth,
                        &attempt,
                        &logical_model,
                        false,
                        request_id.as_deref(),
                        request_ip.as_deref(),
                        started_at,
                    )
                    .await;
                    mark_channel_success(state, &attempt).await;
                    refresh_channel_affinity(state, &attempt).await;
                    let mut resp = match collected_resp {
                        Some(resp) => resp,
                        None => match decode_response_from_provider(
                            attempt.provider_type,
                            &value.expect("non-stream upstream value"),
                            &req_attempt.model,
                        ) {
                            Ok(resp) => resp,
                            Err(err) => {
                                if let Some(session) = capture.session.as_ref() {
                                    session.persist_with_result(None, false).await;
                                }
                                return Err(err);
                            }
                        },
                    };
                    // Wrap newly produced encrypted reasoning payloads in mz2
                    // envelopes BEFORE any response-phase transform observes
                    // the response. Per spec/urp-transform-system.spec.md
                    // PIPE-1 step 12 and PIPE-1d, transforms must only see
                    // encrypted reasoning in `mz2.` envelope form so that
                    // bulk-mutation transforms (e.g. strip_encrypted_reasoning)
                    // can reason about that single canonical surface.
                    if auth.reasoning_envelope_enabled {
                        urp::wrap_reasoning_envelopes_in_response(
                            &mut resp,
                            reasoning_envelope_provider_type(attempt.provider_type),
                            &req_attempt.model,
                        );
                    }
                    if let Err(err) = apply_transform_rules_response(
                        state,
                        &mut resp,
                        &attempt.provider_transforms,
                        &req.model,
                        Some(attempt.provider_type),
                    )
                    .await
                    {
                        if let Some(session) = capture.session.as_ref() {
                            session.persist_with_result(None, false).await;
                        }
                        return Err(err);
                    }
                    if let Err(err) = apply_transform_rules_response(
                        state,
                        &mut resp,
                        &global_transforms,
                        &req.model,
                        Some(attempt.provider_type),
                    )
                    .await
                    {
                        if let Some(session) = capture.session.as_ref() {
                            session.persist_with_result(None, false).await;
                        }
                        return Err(err);
                    }
                    if let Err(err) = apply_transform_rules_response(
                        state,
                        &mut resp,
                        &auth.transforms,
                        &req.model,
                        Some(attempt.provider_type),
                    )
                    .await
                    {
                        if let Some(session) = capture.session.as_ref() {
                            session.persist_with_result(None, false).await;
                        }
                        return Err(err);
                    }
                    if attempt.provider_type == ProviderType::OpenaiImage
                        && !matches!(downstream, DownstreamProtocol::Responses)
                    {
                        convert_assistant_images_to_markdown(&mut resp);
                    }
                    let charge =
                        match maybe_charge_response(state, auth, &attempt, &logical_model, &resp)
                            .await
                        {
                            Ok(charge) => charge,
                            Err(err) => {
                                if let Some(session) = capture.session.as_ref() {
                                    session.persist_with_result(None, false).await;
                                }
                                return Err(err);
                            }
                        };
                    spawn_request_log(
                        state,
                        auth,
                        &attempt,
                        &logical_model,
                        resp.usage.clone(),
                        charge.charge_nano_usd,
                        charge.billing_breakdown,
                        false,
                        started_at,
                        request_id.clone(),
                        request_ip.clone(),
                        attempt.channel_id.clone(),
                        None,
                        None,
                        None,
                        req.reasoning.as_ref().and_then(|r| r.effort.clone()),
                        tried_providers,
                    );
                    if let Some(session) = capture.session.as_ref() {
                        session
                            .persist_with_result(resp.usage.as_ref(), false)
                            .await;
                    }
                    return Ok((resp, logical_model.clone()));
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
                                capture.raw_input.clone(),
                                &req_attempt,
                                upstream_body.clone(),
                                None,
                                None,
                                Some(json!({
                                    "message": err.message,
                                    "code": err.code,
                                    "status": err.status.map(|status| status.as_u16()),
                                })),
                            ))
                            .await;
                    }
                    let non_retryable = is_non_retryable_client_error(&err);
                    let retryable = is_retryable_error(&err);
                    let retryable_failure_class = classify_retryable_failure(&err);
                    let app_err = upstream_error_to_app(err);
                    if non_retryable {
                        spawn_request_log_error(
                            state,
                            auth,
                            &attempt,
                            &logical_model,
                            false,
                            started_at,
                            request_id.clone(),
                            request_ip.clone(),
                            &app_err,
                            req.reasoning.as_ref().and_then(|r| r.effort.clone()),
                            tried_providers,
                        );
                        if let Some(session) = capture.session.as_ref() {
                            session.persist_with_result(None, true).await;
                        }
                        return Err(app_err);
                    }
                    if retryable {
                        clear_channel_affinity(state, &attempt).await;
                        tried_providers.push(TriedProvider::from_app_error(
                            attempt_number,
                            &attempt,
                            &app_err,
                        ));
                        mark_channel_retryable_failure(state, &attempt, retryable_failure_class)
                            .await;
                        last_failed_attempt = Some(attempt.clone());
                        if !is_attempt_channel_healthy(state, &attempt).await {
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
                        state,
                        auth,
                        &attempt,
                        &logical_model,
                        false,
                        started_at,
                        request_id.clone(),
                        request_ip.clone(),
                        &app_err,
                        req.reasoning.as_ref().and_then(|r| r.effort.clone()),
                        tried_providers,
                    );
                    if let Some(session) = capture.session.as_ref() {
                        session.persist_with_result(None, true).await;
                    }
                    return Err(app_err);
                }
            }
        }
    }
    let final_err = build_exhausted_upstream_error(&logical_model, &tried_providers);
    if let Some(attempt) = last_failed_attempt {
        spawn_request_log_error(
            state,
            auth,
            &attempt,
            &logical_model,
            false,
            started_at,
            request_id,
            request_ip,
            &final_err,
            req.reasoning.as_ref().and_then(|r| r.effort.clone()),
            tried_providers,
        );
    } else {
        spawn_request_log_error_no_attempt(
            state,
            auth,
            &logical_model,
            false,
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
    Err(final_err)
}

fn supports_nonstream_upstream_stream_collection(provider_type: ProviderType) -> bool {
    matches!(
        provider_type,
        ProviderType::Responses | ProviderType::OpenaiImage
    )
}

async fn collect_streamed_upstream_response(
    req_attempt: &urp::UrpRequest,
    max_multiplier: Option<f64>,
    provider_type: ProviderType,
    upstream_resp: reqwest::Response,
    started_at: std::time::Instant,
    logical_model: &str,
    stream_idle_timeout_ms: u64,
) -> AppResult<urp::UrpResponse> {
    let legacy = typed_request_to_legacy(req_attempt, max_multiplier)?;
    let pending_request_envelope_extra =
        req_attempt
            .input
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
    let (decoded_tx, mut decoded_rx) = mpsc::channel::<crate::urp::UrpStreamEvent>(64);
    let runtime_metrics = Arc::new(Mutex::new(StreamRuntimeMetrics::default()));
    let decode_handle = {
        let runtime_metrics = runtime_metrics.clone();
        tokio::spawn(async move {
            stream_upstream_to_urp_events(
                &legacy,
                pending_request_envelope_extra,
                provider_type,
                upstream_resp,
                decoded_tx,
                Some(started_at),
                Some(runtime_metrics),
                stream_idle_timeout_ms,
            )
            .await
        })
    };

    let mut final_response: Option<urp::UrpResponse> = None;
    let mut stream_error: Option<AppError> = None;
    while let Some(event) = decoded_rx.recv().await {
        match event {
            crate::urp::UrpStreamEvent::ResponseDone {
                finish_reason,
                usage,
                output,
                extra_body,
            } => {
                final_response = Some(urp::UrpResponse {
                    id: extra_body
                        .get("id")
                        .and_then(|value| value.as_str())
                        .unwrap_or("resp_stream_collected")
                        .to_string(),
                    model: extra_body
                        .get("model")
                        .and_then(|value| value.as_str())
                        .unwrap_or(logical_model)
                        .to_string(),
                    created_at: extra_body
                        .get("created_at")
                        .and_then(|value| value.as_i64()),
                    output,
                    finish_reason,
                    usage,
                    extra_body,
                });
            }
            crate::urp::UrpStreamEvent::Error { code, message, .. } => {
                stream_error = Some(AppError::new(
                    StatusCode::BAD_GATEWAY,
                    code.unwrap_or_else(|| "upstream_stream_error".to_string()),
                    message,
                ));
            }
            _ => {}
        }
    }
    decode_handle.await.map_err(|e| {
        AppError::new(
            StatusCode::INTERNAL_SERVER_ERROR,
            "task_panic",
            e.to_string(),
        )
    })??;
    if let Some(err) = stream_error {
        return Err(err);
    }
    final_response.ok_or_else(|| {
        AppError::new(
            StatusCode::BAD_GATEWAY,
            "upstream_stream_error",
            "stream completed without terminal response",
        )
    })
}

pub(super) async fn forward_nonstream_typed(
    state: &AppState,
    auth: &crate::auth::AuthResult,
    req: urp::UrpRequest,
    max_multiplier: Option<f64>,
    downstream: DownstreamProtocol,
    request_id: Option<String>,
    request_ip: Option<String>,
    capture: RequestCaptureContext,
) -> AppResult<Value> {
    let (resp, logical_model) = execute_nonstream_typed(
        state,
        auth,
        req,
        max_multiplier,
        downstream,
        request_id,
        request_ip,
        capture,
    )
    .await?;
    Ok(encode_response_for_downstream(
        downstream,
        &resp,
        &logical_model,
    ))
}

#[allow(clippy::result_large_err)]
pub(super) fn encode_request_for_provider(
    req: &mut urp::UrpRequest,
    attempt: &MonoizeAttempt,
    downstream: DownstreamProtocol,
) -> AppResult<Value> {
    filter_extra_body_for_provider(req, attempt.provider_type, &attempt.extra_fields_whitelist);
    filter_tools_for_provider(req, attempt.provider_type, downstream);
    strip_orphaned_tool_calls(req);
    let model = req.model.clone();
    let value = match attempt.provider_type {
        ProviderType::Responses => urp::encode::openai_responses::encode_request(req, &model),
        ProviderType::ChatCompletion => urp::encode::openai_chat::encode_request(req, &model),
        ProviderType::Messages => urp::encode::anthropic::encode_request_checked(req, &model)
            .map_err(|message| {
                AppError::new(StatusCode::BAD_REQUEST, "invalid_request", message)
            })?,
        ProviderType::Gemini => urp::encode::gemini::encode_request(req, &model),
        ProviderType::OpenaiImage => urp::encode::openai_image::encode_request(req, &model),
        ProviderType::Replicate => urp::encode::replicate::encode_request(req, &model),
        ProviderType::Group => {
            return Err(AppError::new(
                StatusCode::BAD_REQUEST,
                "provider_type_not_supported",
                "group is virtual",
            ));
        }
    };
    Ok(value)
}

#[allow(clippy::result_large_err)]
pub(super) fn decode_response_from_provider(
    provider_type: ProviderType,
    value: &Value,
    model: &str,
) -> AppResult<urp::UrpResponse> {
    if provider_type == ProviderType::ChatCompletion
        && let Some(error) = embedded_chat_completion_error(value)
    {
        return Err(embedded_chat_completion_error_to_app(error));
    }
    if provider_type == ProviderType::ChatCompletion
        && chat_completion_finish_reason_is_error(value)
    {
        return Err(AppError::new(
            StatusCode::BAD_GATEWAY,
            "upstream_chat_error",
            "upstream Chat Completions response terminated with finish_reason=error",
        )
        .with_type("server_error"));
    }
    let decoded = match provider_type {
        ProviderType::Responses => urp::decode::openai_responses::decode_response(value),
        ProviderType::ChatCompletion => urp::decode::openai_chat::decode_response(value),
        ProviderType::Messages => urp::decode::anthropic::decode_response(value),
        ProviderType::Gemini => urp::decode::gemini::decode_response(value),
        ProviderType::OpenaiImage => urp::decode::openai_image::decode_response(value, model),
        ProviderType::Replicate => urp::decode::replicate::decode_response(value),
        ProviderType::Group => Err("provider_type group is virtual".to_string()),
    };
    decoded.map_err(|e| AppError::new(StatusCode::BAD_GATEWAY, "invalid_upstream_response", e))
}

fn embedded_chat_completion_error(value: &Value) -> Option<&Value> {
    value
        .get("error")
        .filter(|error| !error.is_null())
        .or_else(|| {
            value
                .get("choices")
                .and_then(Value::as_array)
                .and_then(|choices| choices.first())
                .and_then(|choice| choice.get("error"))
                .filter(|error| !error.is_null())
        })
}

fn chat_completion_finish_reason_is_error(value: &Value) -> bool {
    value
        .get("choices")
        .and_then(Value::as_array)
        .and_then(|choices| choices.first())
        .and_then(|choice| choice.get("finish_reason"))
        .and_then(Value::as_str)
        == Some("error")
}

fn embedded_chat_completion_error_to_app(error: &Value) -> AppError {
    let message = error
        .get("message")
        .and_then(Value::as_str)
        .or_else(|| error.as_str())
        .filter(|message| !message.is_empty())
        .unwrap_or("upstream Chat Completions response terminated with an error");
    let metadata = error.get("metadata").and_then(Value::as_object);
    let upstream_code = error.get("code").and_then(json_scalar_string).or_else(|| {
        metadata
            .and_then(|metadata| metadata.get("provider_code"))
            .and_then(json_scalar_string)
    });
    let upstream_status = error
        .get("code")
        .and_then(Value::as_u64)
        .filter(|status| (400..=599).contains(status))
        .and_then(|status| StatusCode::from_u16(status as u16).ok());
    let upstream_type = error.get("type").and_then(json_scalar_string).or_else(|| {
        metadata
            .and_then(|metadata| metadata.get("error_type"))
            .and_then(json_scalar_string)
    });
    let upstream_param = error
        .get("param")
        .and_then(Value::as_str)
        .filter(|value| !value.is_empty())
        .map(str::to_string);

    AppError::new(StatusCode::BAD_GATEWAY, "upstream_chat_error", message)
        .with_type("server_error")
        .with_upstream_error(
            upstream_status,
            upstream_code,
            upstream_type,
            upstream_param,
        )
}

fn json_scalar_string(value: &Value) -> Option<String> {
    match value {
        Value::String(value) if !value.is_empty() => Some(value.clone()),
        Value::Number(value) => Some(value.to_string()),
        _ => None,
    }
}

pub(super) fn encode_response_for_downstream(
    downstream: DownstreamProtocol,
    resp: &urp::UrpResponse,
    logical_model: &str,
) -> Value {
    match downstream {
        DownstreamProtocol::Responses => {
            urp::encode::openai_responses::encode_response(resp, logical_model)
        }
        DownstreamProtocol::ChatCompletions => {
            urp::encode::openai_chat::encode_response(resp, logical_model)
        }
        DownstreamProtocol::AnthropicMessages => {
            urp::encode::anthropic::encode_response(resp, logical_model)
        }
    }
}
