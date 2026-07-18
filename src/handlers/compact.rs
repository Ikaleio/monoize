use super::*;

pub async fn compact_response(
    State(state): State<AppState>,
    headers: HeaderMap,
    Json(mut body): Json<Value>,
) -> AppResult<Response> {
    let auth = auth_tenant(&headers, &state).await?;
    let raw_input = body.clone();
    let body_obj = body.as_object_mut().ok_or_else(|| {
        AppError::new(
            StatusCode::BAD_REQUEST,
            "invalid_request",
            "body must be object",
        )
    })?;
    if body_obj.get("stream").and_then(Value::as_bool) == Some(true) {
        return Err(AppError::new(
            StatusCode::BAD_REQUEST,
            "invalid_request",
            "responses compact does not support streaming",
        ));
    }
    if !body_obj.contains_key("input") {
        return Err(AppError::new(
            StatusCode::BAD_REQUEST,
            "invalid_request",
            "missing input",
        ));
    }

    let mut logical_model = body_obj
        .get("model")
        .and_then(Value::as_str)
        .map(str::trim)
        .filter(|model| !model.is_empty())
        .ok_or_else(|| AppError::new(StatusCode::BAD_REQUEST, "invalid_request", "missing model"))?
        .to_string();
    apply_model_redirects_to_model(&mut logical_model, &auth.model_redirects);
    ensure_model_allowed(&auth, &logical_model)?;
    body_obj.insert("model".to_string(), Value::String(logical_model.clone()));

    let max_multiplier = resolve_max_multiplier_for_embeddings(&body, &headers, &auth);
    let routing_request = urp::decode::openai_responses::decode_request(&body)
        .map_err(|message| AppError::new(StatusCode::BAD_REQUEST, "invalid_request", message))?;
    let routing_stub = build_routing_stub(&routing_request, max_multiplier);
    let attempts = build_monoize_attempts_for_provider_type(
        &state,
        &routing_stub,
        &auth,
        Some(ProviderType::Responses),
    )
    .await?;
    if attempts.is_empty() {
        return Err(AppError::new(
            StatusCode::BAD_REQUEST,
            "provider_type_not_supported",
            format!("model '{logical_model}' has no Responses provider"),
        ));
    }
    ensure_balance_before_forward_for_attempts(&state, &auth, &attempts).await?;

    let request_id = extract_request_id(&headers);
    let request_ip = extract_client_ip(&headers);
    let started_at = std::time::Instant::now();
    let capture = RequestCaptureContext {
        raw_input,
        session: state
            .request_capture
            .maybe_start_session(
                &state.monoize_runtime,
                &auth,
                request_id.clone(),
                DownstreamProtocol::Responses,
                false,
            )
            .await,
    };
    let _pending_request_log_guard = insert_pending_request_log(
        &state,
        &auth,
        &logical_model,
        false,
        request_id.as_deref(),
        request_ip.as_deref(),
        started_at,
    )
    .await;

    let mut last_failed_attempt: Option<MonoizeAttempt> = None;
    let mut tried_providers = Vec::new();
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
            let mut upstream_body = crate::urp::encode::sanitize_provider_item_wire_body(&body);
            if let Some(obj) = upstream_body.as_object_mut() {
                obj.insert(
                    "model".to_string(),
                    Value::String(attempt.upstream_model.clone()),
                );
                obj.remove("max_multiplier");
            }
            let provider = build_channel_provider_config(&attempt);
            let result = upstream::call_upstream_with_timeout_and_headers(
                client_http(&state),
                &provider,
                &attempt.api_key,
                "/v1/responses/compact",
                &upstream_body,
                attempt.request_timeout_ms,
                provider_extra_headers(attempt.provider_type, &upstream_body),
            )
            .await;

            match result {
                Ok(value) => {
                    if let Some(session) = capture.session.as_ref() {
                        session
                            .push_attempt(crate::request_capture::build_attempt_dump(
                                attempt_number,
                                &attempt.provider_id,
                                Some(&attempt.channel_id),
                                attempt.provider_type,
                                &logical_model,
                                &attempt.upstream_model,
                                "/v1/responses/compact",
                                capture.raw_input.clone(),
                                &routing_request,
                                upstream_body,
                                Some(value.clone()),
                                None,
                                None,
                            ))
                            .await;
                    }
                    update_pending_channel_info(
                        &state,
                        &auth,
                        &attempt,
                        &logical_model,
                        false,
                        request_id.as_deref(),
                        request_ip.as_deref(),
                        started_at,
                    )
                    .await;
                    mark_channel_success(&state, &attempt).await;
                    refresh_channel_affinity(&state, &attempt).await;
                    let usage = parse_usage_from_responses_object(&value);
                    let charge = match usage.as_ref() {
                        Some(usage) => {
                            maybe_charge_usage(&state, &auth, &attempt, &logical_model, usage)
                                .await?
                        }
                        None => ChargeComputation::default(),
                    };
                    spawn_request_log(
                        &state,
                        &auth,
                        &attempt,
                        &logical_model,
                        usage.clone(),
                        charge.charge_nano_usd,
                        charge.billing_breakdown,
                        false,
                        started_at,
                        request_id,
                        request_ip,
                        attempt.channel_id.clone(),
                        None,
                        None,
                        None,
                        None,
                        tried_providers,
                    );
                    if let Some(session) = capture.session.as_ref() {
                        session.persist_with_result(usage.as_ref(), false).await;
                    }
                    return Ok(Json(value).into_response());
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
                                &attempt.upstream_model,
                                "/v1/responses/compact",
                                capture.raw_input.clone(),
                                &routing_request,
                                upstream_body,
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
                    let failure_class = classify_retryable_failure(&err);
                    let app_err = upstream_error_to_app(err);
                    if non_retryable {
                        spawn_request_log_error(
                            &state,
                            &auth,
                            &attempt,
                            &logical_model,
                            false,
                            started_at,
                            request_id,
                            request_ip,
                            &app_err,
                            None,
                            tried_providers,
                        );
                        if let Some(session) = capture.session.as_ref() {
                            session.persist_with_result(None, true).await;
                        }
                        return Err(app_err);
                    }
                    if retryable {
                        clear_channel_affinity(&state, &attempt).await;
                        tried_providers.push(TriedProvider::from_app_error(
                            attempt_number,
                            &attempt,
                            &app_err,
                        ));
                        mark_channel_retryable_failure(&state, &attempt, failure_class).await;
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
                        false,
                        started_at,
                        request_id,
                        request_ip,
                        &app_err,
                        None,
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
            &state,
            &auth,
            &attempt,
            &logical_model,
            false,
            started_at,
            request_id,
            request_ip,
            &final_err,
            None,
            tried_providers,
        );
    } else {
        spawn_request_log_error_no_attempt(
            &state,
            &auth,
            &logical_model,
            false,
            started_at,
            request_id,
            request_ip,
            &final_err,
            None,
            tried_providers,
        );
    }
    if let Some(session) = capture.session.as_ref() {
        session.persist_with_result(None, true).await;
    }
    Err(final_err)
}
