//! Shared attempt loop for non-streaming JSON pass-through endpoints:
//! `/v1/embeddings`, `/v1/systemone`, and `/v1/decisions`.

use super::*;

pub(super) trait JsonForwardEndpoint {
    /// Upstream path and body for one attempt.
    fn upstream_request(&self, attempt: &MonoizeAttempt) -> AppResult<(&'static str, Value)>;

    /// Billing usage read from the raw upstream body.
    fn parse_usage(&self, upstream: &Value) -> Option<urp::Usage>;

    /// Converts a successful upstream body into the downstream body. An error
    /// counts as a failure of this attempt, and the loop moves to the next one.
    fn downstream_body(&self, attempt: &MonoizeAttempt, upstream: Value) -> AppResult<Value>;
}

pub(super) struct JsonForward<'a> {
    pub state: &'a AppState,
    pub auth: &'a crate::auth::AuthResult,
    pub headers: &'a HeaderMap,
    pub logical_model: &'a str,
    pub started_at: Instant,
}

/// Runs the balance guard, the pending request log, and the attempt loop with
/// same-Channel retries, billing, and request logging.
pub(super) async fn forward_json_attempts(
    ctx: JsonForward<'_>,
    attempts: Vec<MonoizeAttempt>,
    endpoint: &impl JsonForwardEndpoint,
) -> AppResult<Response> {
    let JsonForward {
        state,
        auth,
        headers,
        logical_model,
        started_at,
    } = ctx;
    let request_id = extract_request_id(headers);
    let request_ip = extract_client_ip(headers);
    ensure_balance_before_forward_for_attempts(state, auth, &attempts).await?;
    let _pending_request_log_guard = insert_pending_request_log(
        state,
        auth,
        logical_model,
        false,
        request_id.as_deref(),
        request_ip.as_deref(),
        started_at,
    )
    .await?;
    let mut last_failed_attempt: Option<MonoizeAttempt> = None;
    let mut tried_providers: Vec<TriedProvider> = Vec::new();
    let mut execution_state = AttemptExecutionState::default();

    for attempt in attempts {
        if execution_state.should_skip(&attempt) {
            continue;
        }

        let max_channel_attempts = same_channel_attempt_slots(&attempt);
        for channel_attempt in 0..max_channel_attempts {
            if execution_state.should_skip(&attempt) {
                break;
            }

            let attempt_number = execution_state.record_upstream_attempt(&attempt);
            let (path, upstream_body) = endpoint.upstream_request(&attempt)?;
            let request_body = upstream::JsonBody::new(&upstream_body);
            drop(upstream_body);

            let provider = build_channel_provider_config(&attempt);
            let http = client_http_for_attempt(state, &attempt)?;
            let result = upstream::call_upstream_with_timeout_and_headers(
                &http,
                &provider,
                &attempt.api_key,
                path,
                request_body,
                attempt.request_timeout_ms,
                &[],
            )
            .await;

            match result {
                Ok(value) => {
                    update_pending_channel_info(
                        state,
                        auth,
                        &attempt,
                        logical_model,
                        false,
                        request_id.as_deref(),
                        request_ip.as_deref(),
                        started_at,
                    )
                    .await;
                    let usage = endpoint.parse_usage(&value);
                    // MP-F3: a fail-closed missing-usage billable success
                    // rejects with 403 before response delivery.
                    if usage.is_none() && missing_usage_rejects(auth, &attempt) {
                        let err = missing_usage_error();
                        spawn_request_log_error(
                            state,
                            auth,
                            &attempt,
                            logical_model,
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
                    let body = match endpoint.downstream_body(&attempt, value) {
                        Ok(body) => body,
                        Err(app_err) => {
                            record_upstream_attempt_failure(
                                state,
                                &attempt,
                                attempt_number,
                                &app_err,
                                None,
                                &mut tried_providers,
                                &mut execution_state,
                            )
                            .await;
                            last_failed_attempt = Some(attempt.clone());
                            break;
                        }
                    };
                    mark_channel_success(state, &attempt).await;
                    let settled_usage = match usage.as_ref() {
                        Some(usage_row) => crate::settlement::SettledUsage::Reported(usage_row),
                        None => crate::settlement::SettledUsage::MissingFree,
                    };
                    let charge = match maybe_charge_settled(
                        state,
                        auth,
                        &attempt,
                        logical_model,
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
                                state,
                                auth,
                                &attempt,
                                logical_model,
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
                        state,
                        auth,
                        &attempt,
                        logical_model,
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

                    return Ok(Json(body).into_response());
                }
                Err(err) => {
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
    let final_err = build_exhausted_upstream_error(logical_model, &tried_providers);
    if let Some(attempt) = last_failed_attempt {
        spawn_request_log_error(
            state,
            auth,
            &attempt,
            logical_model,
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
            state,
            auth,
            logical_model,
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
