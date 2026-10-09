use super::*;

fn invalid_systemone_request(message: impl Into<String>) -> AppError {
    AppError::new(StatusCode::BAD_REQUEST, "invalid_request", message)
}

fn validate_systemone_questions(questions: &Map<String, Value>) -> Result<(), AppError> {
    if questions.is_empty() {
        return Err(invalid_systemone_request(
            "questions must be a non-empty object",
        ));
    }
    for (key, value) in questions {
        let Some(question) = value.as_object() else {
            return Err(invalid_systemone_request(format!(
                "questions.{key} must be an object"
            )));
        };
        let type_ok = question
            .get("type")
            .and_then(Value::as_str)
            .is_some_and(|value| !value.is_empty());
        if !type_ok {
            return Err(invalid_systemone_request(format!(
                "questions.{key}.type must be a non-empty string"
            )));
        }
    }
    Ok(())
}

fn prepare_systemone_upstream_body(body: &Value, upstream_model: &str) -> Value {
    let mut upstream_body = body.clone();
    let Some(obj) = upstream_body.as_object_mut() else {
        return upstream_body;
    };
    obj.insert(
        "model".to_string(),
        Value::String(upstream_model.to_string()),
    );
    obj.remove("max_multiplier");
    obj.retain(|key, _| !key.starts_with("_monoize_"));
    upstream_body
}

pub async fn create_systemone(
    State(state): State<AppState>,
    headers: HeaderMap,
    Json(body): Json<Value>,
) -> AppResult<Response> {
    let auth = auth_tenant(&headers, &state).await?;
    let obj = body
        .as_object()
        .ok_or_else(|| invalid_systemone_request("body must be object"))?;

    let mut logical_model = obj
        .get("model")
        .and_then(Value::as_str)
        .map(str::trim)
        .filter(|model| !model.is_empty())
        .ok_or_else(|| invalid_systemone_request("model must be a non-empty string"))?
        .to_string();
    apply_configured_model_redirects_to_model(&state, &mut logical_model, &auth).await;
    ensure_model_allowed(&auth, &logical_model)?;

    if obj.get("stream").and_then(Value::as_bool) == Some(true) {
        return Err(invalid_systemone_request(
            "system one does not support streaming",
        ));
    }
    if !obj.contains_key("state") {
        return Err(invalid_systemone_request("missing state"));
    }
    let questions = obj
        .get("questions")
        .and_then(Value::as_object)
        .ok_or_else(|| invalid_systemone_request("questions must be a non-empty object"))?;
    validate_systemone_questions(questions)?;

    let max_multiplier = resolve_max_multiplier_for_embeddings(&body, &headers, &auth);
    let request_id = extract_request_id(&headers);
    let request_ip = extract_client_ip(&headers);
    let started_at = std::time::Instant::now();
    let routing_stub = build_embeddings_routing_stub(&logical_model, max_multiplier);
    let mut attempts = build_monoize_attempts_for_provider_type(
        &state,
        &routing_stub,
        &auth,
        Some(ProviderType::Systemone),
    )
    .await?;
    attach_client_session_id(&mut attempts, extract_client_session_id(&headers), None);
    ensure_balance_before_forward_for_attempts(&state, &auth, &attempts).await?;
    let _pending_request_log_guard = insert_pending_request_log(
        &state,
        &auth,
        &logical_model,
        false,
        request_id.as_deref(),
        request_ip.as_deref(),
        started_at,
    )
    .await?;
    let mut last_failed_attempt: Option<MonoizeAttempt> = None;
    let mut tried_providers: Vec<TriedProvider> = Vec::new();
    let mut execution_state = AttemptExecutionState::default();

    for mut attempt in attempts {
        if execution_state.should_skip(&attempt) {
            continue;
        }

        let max_channel_attempts = same_channel_attempt_slots(&attempt);
        for channel_attempt in 0..max_channel_attempts {
            if execution_state.should_skip(&attempt) {
                break;
            }

            let attempt_number = execution_state.record_upstream_attempt(&attempt);
            let upstream_body = prepare_systemone_upstream_body(&body, &attempt.upstream_model);
            let extra_headers = attempt_extra_headers(&attempt, &upstream_body);
            attempt.session_affinity_value =
                resolve_session_affinity_value(&attempt, &upstream_body);
            let request_body = upstream::JsonBody::new(&upstream_body);
            drop(upstream_body);

            let provider = build_channel_provider_config(&attempt);
            let http = client_http_for_attempt(&state, &attempt)?;
            let result = upstream::call_upstream_with_timeout_and_headers(
                &http,
                &provider,
                &attempt.api_key,
                "/v1/systemone",
                request_body,
                attempt.request_timeout_ms,
                &extra_headers,
            )
            .await;

            match result {
                Ok(value) => {
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
                    let usage = parse_usage_from_systemone_object(&value);
                    if usage.is_none() && missing_usage_rejects(&auth, &attempt) {
                        let err = missing_usage_error();
                        spawn_request_log_error(
                            &state,
                            &auth,
                            &attempt,
                            &logical_model,
                            false,
                            started_at,
                            request_id.clone(),
                            request_ip.clone(),
                            &err,
                            None,
                            tried_providers,
                        );
                        return Err(err);
                    }
                    let upstream_response_model = mismatched_upstream_response_model(
                        &attempt.upstream_model,
                        value.get("model").and_then(Value::as_str).unwrap_or(""),
                    );
                    let response_service_tier =
                        usage::response_service_tier(&value).map(str::to_string);
                    mark_channel_success(&state, &attempt).await;
                    refresh_channel_affinity(&state, &attempt).await;
                    let settled_usage = match usage.as_ref() {
                        Some(usage_row) => crate::settlement::SettledUsage::Reported(usage_row),
                        None => crate::settlement::SettledUsage::MissingFree,
                    };
                    let charge = match maybe_charge_settled(
                        &state,
                        &auth,
                        &attempt,
                        &logical_model,
                        settled_usage,
                        None,
                        response_service_tier.as_deref(),
                        request_id.as_deref(),
                    )
                    .await
                    {
                        Ok(charge) => charge,
                        Err(err) => {
                            spawn_request_log_error(
                                &state,
                                &auth,
                                &attempt,
                                &logical_model,
                                false,
                                started_at,
                                request_id.clone(),
                                request_ip.clone(),
                                &err,
                                None,
                                tried_providers,
                            );
                            return Err(err);
                        }
                    };

                    spawn_request_log(
                        &state,
                        &auth,
                        &attempt,
                        &logical_model,
                        usage,
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
                        tried_providers,
                        false,
                        upstream_response_model,
                    );

                    return Ok(Json(value).into_response());
                }
                Err(err) => {
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
    Err(final_err)
}
