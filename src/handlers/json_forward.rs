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

struct JsonForwardExecution<'a> {
    context: JsonForward<'a>,
    request_id: Option<String>,
    request_ip: Option<String>,
}

enum JsonAttemptResponse {
    Delivered(Response),
    Invalid(AppError),
}

struct JsonAttemptFailure {
    error: AppError,
    class: Option<RetryableFailureClass>,
}

/// Runs the balance guard, the pending request log, and the attempt loop with
/// same-Channel retries, billing, and request logging.
pub(super) async fn forward_json_attempts(
    ctx: JsonForward<'_>,
    attempts: Vec<MonoizeAttempt>,
    endpoint: &impl JsonForwardEndpoint,
) -> AppResult<Response> {
    let execution = JsonForwardExecution {
        request_id: extract_request_id(ctx.headers),
        request_ip: extract_client_ip(ctx.headers),
        context: ctx,
    };
    let ctx = &execution.context;
    ensure_balance_before_forward_for_attempts(ctx.state, ctx.auth, &attempts).await?;
    let _pending_request_log_guard = insert_pending_request_log(
        ctx.state,
        ctx.auth,
        ctx.logical_model,
        false,
        execution.request_id.as_deref(),
        execution.request_ip.as_deref(),
        ctx.started_at,
    )
    .await?;
    let mut driver = AttemptDriver::new(attempts);
    while let Some(slot) = driver.next() {
        match call_json_attempt(ctx.state, &slot.attempt, endpoint).await? {
            Ok(value) => {
                match execution
                    .accept(&slot.attempt, value, endpoint, &mut driver.tried_providers)
                    .await?
                {
                    JsonAttemptResponse::Delivered(response) => return Ok(response),
                    JsonAttemptResponse::Invalid(error) => {
                        driver
                            .record_terminal_failure(ctx.state, &slot, &error, None)
                            .await;
                    }
                }
            }
            Err(failure) => {
                driver
                    .fail(ctx.state, slot, &failure.error, failure.class)
                    .await;
            }
        }
    }
    let error = build_exhausted_upstream_error(ctx.logical_model, &driver.tried_providers);
    execution.log_error(
        driver.last_failed_attempt.as_ref(),
        &error,
        driver.tried_providers,
    );
    Err(error)
}

async fn call_json_attempt(
    state: &AppState,
    attempt: &MonoizeAttempt,
    endpoint: &impl JsonForwardEndpoint,
) -> AppResult<Result<Value, JsonAttemptFailure>> {
    let (path, upstream_body) = endpoint.upstream_request(attempt)?;
    let request_body = upstream::JsonBody::new(&upstream_body);
    drop(upstream_body);
    let provider = build_channel_provider_config(attempt);
    let http = match client_http_for_attempt(state, attempt) {
        Ok(http) => http,
        Err(error) => return Ok(Err(JsonAttemptFailure { error, class: None })),
    };
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
    Ok(match result {
        Ok(value) => Ok(value),
        Err(error) => {
            let class =
                is_same_channel_retryable_error(&error).then(|| classify_retryable_failure(&error));
            let mask = state.monoize_runtime.read().await.mask_sensitive_info;
            Err(JsonAttemptFailure {
                error: upstream_error_to_app(error, mask),
                class,
            })
        }
    })
}

impl JsonForwardExecution<'_> {
    fn log_context(&self) -> RequestLogContext<'_> {
        RequestLogContext {
            model: self.context.logical_model,
            is_stream: false,
            started_at: self.context.started_at,
            request_id: self.request_id.as_deref(),
            request_ip: self.request_ip.as_deref(),
            reasoning_effort: None,
        }
    }

    async fn accept(
        &self,
        attempt: &MonoizeAttempt,
        value: Value,
        endpoint: &impl JsonForwardEndpoint,
        tried: &mut Vec<TriedProvider>,
    ) -> AppResult<JsonAttemptResponse> {
        let ctx = &self.context;
        update_pending_channel_info(ctx.state, ctx.auth, attempt, self.log_context()).await;
        let usage = endpoint.parse_usage(&value);
        if usage.is_none() && missing_usage_rejects(ctx.auth, attempt) {
            let error = missing_usage_error();
            self.log_error(Some(attempt), &error, std::mem::take(tried));
            return Err(error);
        }
        let upstream_response_model = mismatched_upstream_response_model(
            &attempt.upstream_model,
            value.get("model").and_then(Value::as_str).unwrap_or(""),
        );
        let response_service_tier = usage::response_service_tier(&value).map(str::to_string);
        let body = match endpoint.downstream_body(attempt, value) {
            Ok(body) => body,
            Err(error) => return Ok(JsonAttemptResponse::Invalid(error)),
        };
        mark_channel_success(ctx.state, attempt).await;
        let settled_usage = match usage.as_ref() {
            Some(usage) => crate::settlement::SettledUsage::Reported(usage),
            None => crate::settlement::SettledUsage::MissingFree,
        };
        let charge = match maybe_charge_settled(
            ctx.state,
            ctx.auth,
            attempt,
            ctx.logical_model,
            SettlementRequest {
                usage: settled_usage,
                output: None,
                response_service_tier: response_service_tier.as_deref(),
            },
            self.request_id.as_deref(),
        )
        .await
        {
            Ok(charge) => charge,
            Err(error) => {
                self.log_error(Some(attempt), &error, std::mem::take(tried));
                return Err(error);
            }
        };
        spawn_request_log(
            ctx.state,
            ctx.auth,
            attempt,
            self.log_context(),
            RequestLogSuccess {
                usage,
                charge_nano_usd: charge.charge_nano_usd,
                billing_breakdown_json: charge.billing_breakdown,
                ttfb_ms: None,
                stream_terminal_diagnostics: None,
                tried_providers: std::mem::take(tried),
                client_gone: false,
                upstream_response_model,
            },
        );
        Ok(JsonAttemptResponse::Delivered(Json(body).into_response()))
    }

    fn log_error(
        &self,
        attempt: Option<&MonoizeAttempt>,
        error: &AppError,
        tried: Vec<TriedProvider>,
    ) {
        let ctx = &self.context;
        if let Some(attempt) = attempt {
            spawn_request_log_error(
                ctx.state,
                ctx.auth,
                attempt,
                self.log_context(),
                error,
                tried,
            );
        } else {
            spawn_request_log_error_no_attempt(
                ctx.state,
                ctx.auth,
                self.log_context(),
                error,
                tried,
            );
        }
    }
}
