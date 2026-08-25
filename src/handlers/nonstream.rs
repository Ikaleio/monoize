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

fn has_responses_state_reference(req: &urp::UrpRequest) -> bool {
    ["previous_response_id", "conversation"]
        .into_iter()
        .any(|key| {
            req.extra_body
                .get(key)
                .is_some_and(|value| !value.is_null())
        })
}

pub(super) struct AdmittedRequestTaskState {
    started_at: std::time::Instant,
    admitted: std::sync::atomic::AtomicBool,
    is_stream: std::sync::atomic::AtomicBool,
    client_gone: std::sync::atomic::AtomicBool,
    attempt: std::sync::Mutex<Option<MonoizeAttempt>>,
    pending_guard: std::sync::Mutex<Option<PendingRequestLogGuard>>,
}

impl AdmittedRequestTaskState {
    pub(super) fn new(started_at: std::time::Instant) -> Self {
        Self {
            started_at,
            admitted: std::sync::atomic::AtomicBool::new(false),
            is_stream: std::sync::atomic::AtomicBool::new(false),
            client_gone: std::sync::atomic::AtomicBool::new(false),
            attempt: std::sync::Mutex::new(None),
            pending_guard: std::sync::Mutex::new(None),
        }
    }

    pub(super) fn mark_client_gone(&self) {
        self.client_gone
            .store(true, std::sync::atomic::Ordering::Release);
    }

    pub(super) fn client_gone(&self) -> bool {
        self.client_gone.load(std::sync::atomic::Ordering::Acquire)
    }

    pub(super) fn set_stream(&self, is_stream: bool) {
        self.is_stream
            .store(is_stream, std::sync::atomic::Ordering::Release);
    }

    pub(super) fn started_at(&self) -> std::time::Instant {
        self.started_at
    }

    pub(super) fn retain_pending_guard(&self, guard: Option<PendingRequestLogGuard>) {
        let admitted = guard.is_some();
        *self.pending_guard.lock().unwrap_or_else(|e| e.into_inner()) = guard;
        self.admitted
            .store(admitted, std::sync::atomic::Ordering::Release);
    }

    pub(super) fn set_attempt(&self, attempt: &MonoizeAttempt) {
        *self.attempt.lock().unwrap_or_else(|e| e.into_inner()) = Some(attempt.clone());
    }

    pub(super) fn terminal_snapshot(
        &self,
    ) -> Option<(std::time::Instant, bool, Option<MonoizeAttempt>)> {
        if !self.admitted.load(std::sync::atomic::Ordering::Acquire) {
            return None;
        }
        Some((
            self.started_at,
            self.is_stream.load(std::sync::atomic::Ordering::Acquire),
            self.attempt
                .lock()
                .unwrap_or_else(|e| e.into_inner())
                .clone(),
        ))
    }
}

fn client_gone_flag(task_state: Option<&AdmittedRequestTaskState>) -> bool {
    task_state.is_some_and(AdmittedRequestTaskState::client_gone)
}

#[allow(clippy::too_many_arguments)]
async fn finish_nonstream_error(
    state: &AppState,
    auth: &crate::auth::AuthResult,
    attempt: &MonoizeAttempt,
    logical_model: &str,
    started_at: std::time::Instant,
    request_id: &Option<String>,
    request_ip: &Option<String>,
    reasoning_effort: Option<String>,
    tried_providers: Vec<TriedProvider>,
    capture: &RequestCaptureContext,
    capture_upstream_failure: bool,
    error: AppError,
) -> AppError {
    spawn_request_log_error(
        state,
        auth,
        attempt,
        logical_model,
        false,
        started_at,
        request_id.clone(),
        request_ip.clone(),
        &error,
        reasoning_effort,
        tried_providers,
    );
    if let Some(session) = capture.session.as_ref() {
        session
            .persist_with_result(None, capture_upstream_failure)
            .await;
    }
    error
}

#[allow(dead_code)]
pub(super) async fn execute_nonstream_typed(
    state: &AppState,
    auth: &crate::auth::AuthResult,
    req: urp::UrpRequest,
    max_multiplier: Option<Multiplier>,
    downstream: DownstreamProtocol,
    request_id: Option<String>,
    request_ip: Option<String>,
    client_session_id: Option<String>,
    capture: RequestCaptureContext,
) -> AppResult<(urp::UrpResponse, String)> {
    execute_nonstream_typed_owned(
        state,
        auth,
        req,
        max_multiplier,
        downstream,
        request_id,
        request_ip,
        client_session_id,
        capture,
        None,
    )
    .await
}

pub(super) async fn execute_nonstream_typed_owned(
    state: &AppState,
    auth: &crate::auth::AuthResult,
    req: urp::UrpRequest,
    max_multiplier: Option<Multiplier>,
    downstream: DownstreamProtocol,
    request_id: Option<String>,
    request_ip: Option<String>,
    client_session_id: Option<String>,
    capture: RequestCaptureContext,
    task_state: Option<&AdmittedRequestTaskState>,
) -> AppResult<(urp::UrpResponse, String)> {
    execute_nonstream_typed_with_validator(
        state,
        auth,
        req,
        max_multiplier,
        downstream,
        request_id,
        request_ip,
        client_session_id,
        capture,
        None,
        task_state,
    )
    .await
}

#[allow(clippy::too_many_arguments)]
pub(super) async fn execute_nonstream_typed_with_validator(
    state: &AppState,
    auth: &crate::auth::AuthResult,
    mut req: urp::UrpRequest,
    max_multiplier: Option<Multiplier>,
    downstream: DownstreamProtocol,
    request_id: Option<String>,
    request_ip: Option<String>,
    client_session_id: Option<String>,
    capture: RequestCaptureContext,
    response_validator: Option<fn(&urp::UrpResponse) -> AppResult<()>>,
    task_state: Option<&AdmittedRequestTaskState>,
) -> AppResult<(urp::UrpResponse, String)> {
    let started_at = task_state
        .map(AdmittedRequestTaskState::started_at)
        .unwrap_or_else(std::time::Instant::now);
    let transform_match_model = resolve_model_suffix(state, &mut req).await?;
    // Preserve the suffix-normalized request so each per-attempt iteration can
    // re-derive the transformed request from a pristine base. This matters
    // because cross-family strip runs BEFORE all transforms per-attempt
    // (auto_cache_* etc. must observe the stripped request so their cache
    // breakpoints actually survive into the upstream encoding).
    let original_req = req.clone();
    let logical_model = req.model.clone();
    let routing_stub = build_routing_stub(&req, max_multiplier);
    let mut attempts = build_monoize_attempts(state, &routing_stub, auth).await?;
    attach_client_session_id(&mut attempts, client_session_id, Some(&req));
    ensure_balance_before_forward_for_attempts(state, auth, &attempts).await?;
    let pending_request_log_guard = insert_pending_request_log(
        state,
        auth,
        &req.model,
        false,
        request_id.as_deref(),
        request_ip.as_deref(),
        started_at,
    )
    .await?;
    let _local_pending_request_log_guard = if let Some(task_state) = task_state {
        task_state.retain_pending_guard(pending_request_log_guard);
        None
    } else {
        pending_request_log_guard
    };
    let mut last_failed_attempt: Option<MonoizeAttempt> = None;
    let mut tried_providers: Vec<TriedProvider> = Vec::new();
    let mut execution_state = AttemptExecutionState::default();
    for mut attempt in attempts {
        if execution_state.should_skip(&attempt) {
            continue;
        }

        let max_channel_attempts = same_channel_attempt_slots(&attempt);
        'channel_attempts: for channel_attempt in 0..max_channel_attempts {
            if execution_state.should_skip(&attempt) {
                break;
            }

            let attempt_number = execution_state.record_upstream_attempt(&attempt);
            if let Some(task_state) = task_state {
                task_state.set_attempt(&attempt);
            }
            // Clone from the pristine original request (pre-transforms) so
            // that the cross-family strip can run BEFORE provider, global,
            // and API-key transforms. This guarantees that transforms which
            // inject upstream-specific part-level metadata (e.g.
            // `cache_anthropic_system`, `cache_anthropic_tool_use`) survive into the
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
            if let Err(err) = apply_transform_rules_request(
                state,
                &mut req_attempt,
                &attempt.provider_transforms,
                &transform_match_model,
                Some(attempt.provider_type),
            )
            .await
            {
                return Err(finish_nonstream_error(
                    state,
                    auth,
                    &attempt,
                    &logical_model,
                    started_at,
                    &request_id,
                    &request_ip,
                    req.reasoning.as_ref().and_then(|r| r.effort.clone()),
                    tried_providers,
                    &capture,
                    false,
                    err,
                )
                .await);
            }
            let global_transforms = state.monoize_runtime.read().await.global_transforms.clone();
            if let Err(err) = apply_transform_rules_request(
                state,
                &mut req_attempt,
                &global_transforms,
                &transform_match_model,
                Some(attempt.provider_type),
            )
            .await
            {
                return Err(finish_nonstream_error(
                    state,
                    auth,
                    &attempt,
                    &logical_model,
                    started_at,
                    &request_id,
                    &request_ip,
                    req.reasoning.as_ref().and_then(|r| r.effort.clone()),
                    tried_providers,
                    &capture,
                    false,
                    err,
                )
                .await);
            }
            if let Err(err) = apply_transform_rules_request(
                state,
                &mut req_attempt,
                &auth.transforms,
                &transform_match_model,
                Some(attempt.provider_type),
            )
            .await
            {
                return Err(finish_nonstream_error(
                    state,
                    auth,
                    &attempt,
                    &logical_model,
                    started_at,
                    &request_id,
                    &request_ip,
                    req.reasoning.as_ref().and_then(|r| r.effort.clone()),
                    tried_providers,
                    &capture,
                    false,
                    err,
                )
                .await);
            }
            strip_monoize_context(&mut req_attempt);
            let capture_transform_chain = crate::request_capture::build_transform_chain(
                &attempt.provider_transforms,
                &global_transforms,
                &auth.transforms,
                &transform_match_model,
            );

            let upstream_body =
                match encode_request_for_provider(&mut req_attempt, &attempt, downstream) {
                    Ok(body) => body,
                    Err(err) => {
                        return Err(finish_nonstream_error(
                            state,
                            auth,
                            &attempt,
                            &logical_model,
                            started_at,
                            &request_id,
                            &request_ip,
                            req.reasoning.as_ref().and_then(|r| r.effort.clone()),
                            tried_providers,
                            &capture,
                            false,
                            err,
                        )
                        .await);
                    }
                };
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
                let http = client_http_for_attempt(state, &attempt)?;
                let extra_headers = attempt_extra_headers(&attempt, &upstream_body);
                attempt.session_affinity_value =
                    resolve_session_affinity_value(&attempt, &upstream_body);
                let call = upstream::call_upstream_raw_with_timeout_and_headers(
                    &http,
                    &provider,
                    &attempt.api_key,
                    &path,
                    &upstream_body,
                    attempt.request_timeout_ms.saturating_mul(10).max(600_000),
                    &extra_headers,
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
                        Err(CollectedUpstreamError::Internal(err)) => {
                            return Err(finish_nonstream_error(
                                state,
                                auth,
                                &attempt,
                                &logical_model,
                                started_at,
                                &request_id,
                                &request_ip,
                                req.reasoning.as_ref().and_then(|r| r.effort.clone()),
                                tried_providers,
                                &capture,
                                false,
                                err,
                            )
                            .await);
                        }
                        Err(CollectedUpstreamError::Upstream(err)) => {
                            let same_channel_retryable = is_same_channel_retryable_app_error(&err);
                            let passive_failure_class = same_channel_retryable
                                .then(|| classify_retryable_app_failure(&err));
                            record_upstream_attempt_failure(
                                state,
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
                                state,
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
                    },
                    Err(err) => Err(err),
                }
            } else if openai_image_edit {
                let form = match urp::encode::openai_image::multipart_form(
                    &req_attempt,
                    &req_attempt.model,
                ) {
                    Ok(form) => form,
                    Err(message) => {
                        let err =
                            AppError::new(StatusCode::BAD_REQUEST, "invalid_request", message);
                        return Err(finish_nonstream_error(
                            state,
                            auth,
                            &attempt,
                            &logical_model,
                            started_at,
                            &request_id,
                            &request_ip,
                            req.reasoning.as_ref().and_then(|r| r.effort.clone()),
                            tried_providers,
                            &capture,
                            false,
                            err,
                        )
                        .await);
                    }
                };
                let http = client_http_for_attempt(state, &attempt)?;
                let extra_headers = attempt_extra_headers(&attempt, &upstream_body);
                attempt.session_affinity_value =
                    resolve_session_affinity_value(&attempt, &upstream_body);
                match upstream::call_upstream_multipart_with_timeout_and_headers(
                    &http,
                    &provider,
                    &attempt.api_key,
                    &path,
                    form,
                    attempt.request_timeout_ms,
                    &extra_headers,
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
                let http = client_http_for_attempt(state, &attempt)?;
                let extra_headers = attempt_extra_headers(&attempt, &upstream_body);
                attempt.session_affinity_value =
                    resolve_session_affinity_value(&attempt, &upstream_body);
                upstream::call_upstream_with_timeout_and_headers(
                    &http,
                    &provider,
                    &attempt.api_key,
                    &path,
                    &upstream_body,
                    attempt.request_timeout_ms,
                    &extra_headers,
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
                                capture.raw_input.as_ref().clone(),
                                &req_attempt,
                                upstream_body.clone(),
                                value.clone(),
                                None,
                                capture_transform_chain.clone(),
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
                    let mut resp = match collected_resp {
                        Some(resp) => resp,
                        None => match value.as_ref() {
                            Some(value) => match decode_response_from_provider(
                                attempt.provider_type,
                                value,
                                &req_attempt.model,
                                state.monoize_runtime.read().await.mask_sensitive_info,
                            ) {
                                Ok(resp) => resp,
                                Err(err) => {
                                    let same_channel_retryable =
                                        is_same_channel_retryable_app_error(&err);
                                    let passive_failure_class = same_channel_retryable
                                        .then(|| classify_retryable_app_failure(&err));
                                    record_upstream_attempt_failure(
                                        state,
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
                                        state,
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
                            },
                            None => {
                                let err = AppError::new(
                                    StatusCode::INTERNAL_SERVER_ERROR,
                                    "internal_error",
                                    "non-stream upstream response value is missing",
                                )
                                .with_type("server_error");
                                return Err(finish_nonstream_error(
                                    state,
                                    auth,
                                    &attempt,
                                    &logical_model,
                                    started_at,
                                    &request_id,
                                    &request_ip,
                                    req.reasoning.as_ref().and_then(|r| r.effort.clone()),
                                    tried_providers,
                                    &capture,
                                    false,
                                    err,
                                )
                                .await);
                            }
                        },
                    };
                    let missing_usage_substituted =
                        substitute_zero_usage_if_allowed(&mut resp.usage, &attempt);
                    if resp.usage.is_none() {
                        let err = AppError::new(
                            StatusCode::BAD_GATEWAY,
                            "upstream_usage_required",
                            "upstream response did not include billable usage",
                        );
                        let same_channel_retryable = is_same_channel_retryable_app_error(&err);
                        let passive_failure_class =
                            same_channel_retryable.then(|| classify_retryable_app_failure(&err));
                        record_upstream_attempt_failure(
                            state,
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
                            state,
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
                    mark_channel_success(state, &attempt).await;
                    refresh_channel_affinity(state, &attempt).await;
                    if attempt.provider_type == ProviderType::Responses {
                        refresh_response_id_affinity(
                            state,
                            auth,
                            &logical_model,
                            &resp.id,
                            &attempt,
                        )
                        .await;
                    }
                    // Wrap newly produced encrypted reasoning payloads in mz2
                    // envelopes BEFORE any response-phase transform observes
                    // the response. Per spec/urp-transform-system.spec.md
                    // PIPE-1 step 12 and PIPE-1d, transforms must only see
                    // encrypted reasoning in `mz2.` envelope form so that
                    // bulk-mutation transforms (e.g. reasoning_strip_encrypted)
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
                        return Err(finish_nonstream_error(
                            state,
                            auth,
                            &attempt,
                            &logical_model,
                            started_at,
                            &request_id,
                            &request_ip,
                            req.reasoning.as_ref().and_then(|r| r.effort.clone()),
                            tried_providers,
                            &capture,
                            false,
                            err,
                        )
                        .await);
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
                        return Err(finish_nonstream_error(
                            state,
                            auth,
                            &attempt,
                            &logical_model,
                            started_at,
                            &request_id,
                            &request_ip,
                            req.reasoning.as_ref().and_then(|r| r.effort.clone()),
                            tried_providers,
                            &capture,
                            false,
                            err,
                        )
                        .await);
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
                        return Err(finish_nonstream_error(
                            state,
                            auth,
                            &attempt,
                            &logical_model,
                            started_at,
                            &request_id,
                            &request_ip,
                            req.reasoning.as_ref().and_then(|r| r.effort.clone()),
                            tried_providers,
                            &capture,
                            false,
                            err,
                        )
                        .await);
                    }
                    if attempt.provider_type == ProviderType::OpenaiImage
                        && !matches!(downstream, DownstreamProtocol::Responses)
                    {
                        convert_assistant_images_to_markdown(&mut resp);
                    }
                    if let Some(validate) = response_validator
                        && let Err(err) = validate(&resp)
                    {
                        return Err(finish_nonstream_error(
                            state,
                            auth,
                            &attempt,
                            &logical_model,
                            started_at,
                            &request_id,
                            &request_ip,
                            req.reasoning.as_ref().and_then(|r| r.effort.clone()),
                            tried_providers,
                            &capture,
                            false,
                            err,
                        )
                        .await);
                    }
                    let charge = match maybe_charge_response(
                        state,
                        auth,
                        &attempt,
                        &logical_model,
                        &resp,
                        missing_usage_substituted,
                        request_id.as_deref(),
                    )
                    .await
                    {
                        Ok(charge) => charge,
                        Err(err) => {
                            return Err(finish_nonstream_error(
                                state,
                                auth,
                                &attempt,
                                &logical_model,
                                started_at,
                                &request_id,
                                &request_ip,
                                req.reasoning.as_ref().and_then(|r| r.effort.clone()),
                                tried_providers,
                                &capture,
                                false,
                                err,
                            )
                            .await);
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
                        req.reasoning.as_ref().and_then(|r| r.effort.clone()),
                        tried_providers,
                        client_gone_flag(task_state),
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
                                capture.raw_input.as_ref().clone(),
                                &req_attempt,
                                upstream_body.clone(),
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
                        state,
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
                        state,
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

enum CollectedUpstreamError {
    Internal(AppError),
    Upstream(AppError),
}

async fn collect_streamed_upstream_response(
    req_attempt: &urp::UrpRequest,
    max_multiplier: Option<Multiplier>,
    provider_type: ProviderType,
    upstream_resp: reqwest::Response,
    started_at: std::time::Instant,
    logical_model: &str,
    stream_idle_timeout_ms: u64,
) -> Result<urp::UrpResponse, CollectedUpstreamError> {
    let legacy = typed_request_to_legacy(req_attempt, max_multiplier)
        .map_err(CollectedUpstreamError::Internal)?;
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
    decode_handle
        .await
        .map_err(|e| {
            CollectedUpstreamError::Internal(AppError::new(
                StatusCode::INTERNAL_SERVER_ERROR,
                "task_panic",
                e.to_string(),
            ))
        })?
        .map_err(CollectedUpstreamError::Upstream)?;
    if let Some(err) = stream_error {
        return Err(CollectedUpstreamError::Upstream(err));
    }
    final_response.ok_or_else(|| {
        CollectedUpstreamError::Upstream(AppError::new(
            StatusCode::BAD_GATEWAY,
            "upstream_stream_error",
            "stream completed without terminal response",
        ))
    })
}

#[allow(dead_code)]
pub(super) async fn forward_nonstream_typed(
    state: &AppState,
    auth: &crate::auth::AuthResult,
    req: urp::UrpRequest,
    max_multiplier: Option<Multiplier>,
    downstream: DownstreamProtocol,
    request_id: Option<String>,
    request_ip: Option<String>,
    client_session_id: Option<String>,
    capture: RequestCaptureContext,
) -> AppResult<Value> {
    forward_nonstream_typed_with_task_state(
        state,
        auth,
        req,
        max_multiplier,
        downstream,
        request_id,
        request_ip,
        client_session_id,
        capture,
        None,
    )
    .await
}

pub(super) async fn forward_nonstream_typed_with_task_state(
    state: &AppState,
    auth: &crate::auth::AuthResult,
    req: urp::UrpRequest,
    max_multiplier: Option<Multiplier>,
    downstream: DownstreamProtocol,
    request_id: Option<String>,
    request_ip: Option<String>,
    client_session_id: Option<String>,
    capture: RequestCaptureContext,
    task_state: Option<&AdmittedRequestTaskState>,
) -> AppResult<Value> {
    let (resp, logical_model) = execute_nonstream_typed_owned(
        state,
        auth,
        req,
        max_multiplier,
        downstream,
        request_id,
        request_ip,
        client_session_id,
        capture,
        task_state,
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
    if matches!(downstream, DownstreamProtocol::Responses)
        && attempt.provider_type != ProviderType::Responses
    {
        req.extra_body.remove("store");
        req.extra_body.remove("conversation");
        req.extra_body.remove("previous_response_id");
    }
    filter_extra_body_for_provider(req, attempt.provider_type, &attempt.extra_fields_whitelist);
    filter_tools_for_provider(req, attempt.provider_type, downstream);
    let stateful_same_responses = matches!(downstream, DownstreamProtocol::Responses)
        && attempt.provider_type == ProviderType::Responses
        && has_responses_state_reference(req);
    if !stateful_same_responses {
        strip_orphaned_tool_calls(req);
    }
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
    mask_sensitive_info: bool,
) -> AppResult<urp::UrpResponse> {
    if provider_type == ProviderType::ChatCompletion
        && let Some(error) = embedded_chat_completion_error(value)
    {
        return Err(embedded_chat_completion_error_to_app(
            error,
            mask_sensitive_info,
        ));
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

fn embedded_chat_completion_error_to_app(error: &Value, mask_sensitive_info: bool) -> AppError {
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

    // SAN-4: the embedded message is upstream-controlled free text — the
    // client sees the masked form while the raw text stays admin-readable
    // via `internal_message`. SAN-CFG5 item 1: masking off disables `MASK`.
    AppError::new(
        StatusCode::BAD_GATEWAY,
        "upstream_chat_error",
        crate::error_sanitize::maybe_mask_sensitive_text(message, mask_sensitive_info),
    )
    .with_internal_message(crate::error_sanitize::truncate_error_detail(message))
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
