mod billing;
pub(crate) mod helpers;
mod nonstream;
mod request_logging;
pub(crate) mod routing;
mod streaming;
pub(crate) mod usage;

#[cfg(test)]
mod tests;

use crate::app::AppState;
use crate::config::{ProviderAuthConfig, ProviderAuthType, ProviderConfig, ProviderType};
use crate::error::{AppError, AppResult};
use crate::model_registry_store::ModelPricing;
use crate::settings::normalize_pricing_model_key;
use crate::transforms::{self, Phase, TransformRuleConfig};
use crate::upstream::{self, UpstreamCallError, UpstreamErrorKind};
use crate::urp;
use crate::users::BillingErrorKind;
use crate::users::{InsertRequestLog, REQUEST_LOG_STATUS_ERROR, REQUEST_LOG_STATUS_SUCCESS};
use axum::Json;
use axum::extract::State;
use axum::http::{HeaderMap, StatusCode};
use axum::response::sse::Event;
use axum::response::{IntoResponse, Response, Sse};
use futures_util::StreamExt;
use serde_json::{Map, Value, json};
use std::collections::HashSet;
use std::sync::Arc;
use tokio::sync::{Mutex, mpsc};

use billing::*;
use helpers::*;
use nonstream::*;
use request_logging::*;
use routing::*;
use streaming::*;
use usage::*;

#[allow(clippy::result_large_err)]
fn ensure_model_allowed(auth: &crate::auth::AuthResult, logical_model: &str) -> AppResult<()> {
    if !auth.model_limits_enabled || auth.model_limits.is_empty() {
        return Ok(());
    }
    if auth.model_limits.iter().any(|model| model == logical_model) {
        return Ok(());
    }
    Err(AppError::new(
        StatusCode::FORBIDDEN,
        "model_not_allowed",
        format!("model '{logical_model}' is not allowed for this API key"),
    ))
}

pub async fn metrics(State(state): State<AppState>) -> impl IntoResponse {
    state.metrics.render()
}

pub async fn list_models(State(state): State<AppState>, headers: HeaderMap) -> AppResult<Response> {
    let auth = auth_tenant(&headers, &state).await?;
    let providers =
        state.monoize_store.list_providers().await.map_err(|e| {
            AppError::new(StatusCode::INTERNAL_SERVER_ERROR, "provider_store_error", e)
        })?;

    let mut model_ids: Vec<String> = providers
        .into_iter()
        .flat_map(|provider| provider.models.into_keys())
        .collect();
    model_ids.sort();
    model_ids.dedup();

    if auth.model_limits_enabled && !auth.model_limits.is_empty() {
        let allowed: HashSet<&str> = auth.model_limits.iter().map(|s| s.as_str()).collect();
        model_ids.retain(|id| allowed.contains(id.as_str()));
    }

    let data: Vec<Value> = model_ids
        .into_iter()
        .map(|id| {
            json!({
                "id": id,
                "object": "model",
                "created": 0,
                "owned_by": "monoize"
            })
        })
        .collect();

    Ok(Json(json!({ "object": "list", "data": data })).into_response())
}

pub async fn create_response(
    State(state): State<AppState>,
    headers: HeaderMap,
    Json(body): Json<Value>,
) -> AppResult<Response> {
    let auth = auth_tenant(&headers, &state).await?;
    ensure_balance_before_forward(&state, &auth).await?;
    ensure_quota_before_forward(&state, &auth).await?;
    let (known, extra) = split_body(body, &URP_KNOWN_RESPONSE_FIELDS)?;
    let mut req = decode_urp_request(DownstreamProtocol::Responses, known, extra)?;
    // S2/S3: stateful fields must not be forwarded upstream
    req.extra_body.remove("previous_response_id");
    req.extra_body.remove("store");
    req.extra_body.remove("conversation");
    ensure_model_allowed(&auth, &req.model)?;
    let max_multiplier = resolve_max_multiplier(&req, &headers, &auth);
    let request_id = extract_request_id(&headers);
    let request_ip = extract_client_ip(&headers);
    if req
        .extra_body
        .get("background")
        .and_then(|v| v.as_bool())
        .unwrap_or(false)
    {
        return Err(AppError::new(
            StatusCode::BAD_REQUEST,
            "background_not_supported",
            "background not supported",
        ));
    }

    if req.stream.unwrap_or(false) {
        let downstream = DownstreamProtocol::Responses;
        match forward_stream_typed(
            state.clone(),
            auth.clone(),
            req,
            max_multiplier,
            downstream,
            request_id.clone(),
            request_ip.clone(),
        )
        .await
        {
            Ok(stream) => return Ok(Sse::new(stream).into_response()),
            Err(err) => return Ok(Sse::new(error_to_sse_stream(&err, downstream)).into_response()),
        }
    }

    let value = forward_nonstream_typed(
        &state,
        &auth,
        req,
        max_multiplier,
        DownstreamProtocol::Responses,
        request_id,
        request_ip,
    )
    .await?;
    Ok(Json(value).into_response())
}

pub async fn create_chat_completions(
    State(state): State<AppState>,
    headers: HeaderMap,
    Json(body): Json<Value>,
) -> AppResult<Response> {
    let auth = auth_tenant(&headers, &state).await?;
    ensure_balance_before_forward(&state, &auth).await?;
    ensure_quota_before_forward(&state, &auth).await?;
    let (known, extra) = split_body(body, &URP_KNOWN_CHAT_FIELDS)?;
    let req = decode_urp_request(DownstreamProtocol::ChatCompletions, known, extra)?;
    ensure_model_allowed(&auth, &req.model)?;
    let max_multiplier = resolve_max_multiplier(&req, &headers, &auth);
    let request_id = extract_request_id(&headers);
    let request_ip = extract_client_ip(&headers);
    if req.stream.unwrap_or(false) {
        let downstream = DownstreamProtocol::ChatCompletions;
        match forward_stream_typed(
            state.clone(),
            auth.clone(),
            req,
            max_multiplier,
            downstream,
            request_id.clone(),
            request_ip.clone(),
        )
        .await
        {
            Ok(stream) => return Ok(Sse::new(stream).into_response()),
            Err(err) => return Ok(Sse::new(error_to_sse_stream(&err, downstream)).into_response()),
        }
    }
    let value = forward_nonstream_typed(
        &state,
        &auth,
        req,
        max_multiplier,
        DownstreamProtocol::ChatCompletions,
        request_id,
        request_ip,
    )
    .await?;
    Ok(Json(value).into_response())
}

pub async fn create_messages(
    State(state): State<AppState>,
    headers: HeaderMap,
    Json(body): Json<Value>,
) -> AppResult<Response> {
    let auth = auth_tenant(&headers, &state).await?;
    ensure_balance_before_forward(&state, &auth).await?;
    ensure_quota_before_forward(&state, &auth).await?;
    let (known, extra) = split_body(body, &URP_KNOWN_MESSAGES_FIELDS)?;
    let req = decode_urp_request(DownstreamProtocol::AnthropicMessages, known, extra)?;
    ensure_model_allowed(&auth, &req.model)?;
    let max_multiplier = resolve_max_multiplier(&req, &headers, &auth);
    let request_id = extract_request_id(&headers);
    let request_ip = extract_client_ip(&headers);
    if req.stream.unwrap_or(false) {
        let downstream = DownstreamProtocol::AnthropicMessages;
        match forward_stream_typed(
            state.clone(),
            auth.clone(),
            req,
            max_multiplier,
            downstream,
            request_id.clone(),
            request_ip.clone(),
        )
        .await
        {
            Ok(stream) => return Ok(Sse::new(stream).into_response()),
            Err(err) => return Ok(Sse::new(error_to_sse_stream(&err, downstream)).into_response()),
        }
    }
    let value = forward_nonstream_typed(
        &state,
        &auth,
        req,
        max_multiplier,
        DownstreamProtocol::AnthropicMessages,
        request_id,
        request_ip,
    )
    .await?;
    Ok(Json(value).into_response())
}

pub async fn create_embeddings(
    State(state): State<AppState>,
    headers: HeaderMap,
    Json(body): Json<Value>,
) -> AppResult<Response> {
    let auth = auth_tenant(&headers, &state).await?;
    ensure_balance_before_forward(&state, &auth).await?;
    ensure_quota_before_forward(&state, &auth).await?;

    let obj = body.as_object().ok_or_else(|| {
        AppError::new(
            StatusCode::BAD_REQUEST,
            "invalid_request",
            "body must be object",
        )
    })?;

    let logical_model = obj
        .get("model")
        .and_then(|v| v.as_str())
        .filter(|s| !s.trim().is_empty())
        .ok_or_else(|| AppError::new(StatusCode::BAD_REQUEST, "invalid_request", "missing model"))?
        .to_string();
    ensure_model_allowed(&auth, &logical_model)?;

    let input = obj.get("input").ok_or_else(|| {
        AppError::new(StatusCode::BAD_REQUEST, "invalid_request", "missing input")
    })?;
    if !is_valid_embeddings_input(input) {
        return Err(AppError::new(
            StatusCode::BAD_REQUEST,
            "invalid_request",
            "input must be string or array of strings",
        ));
    }

    if let Some(encoding_format) = obj.get("encoding_format") {
        let encoding_format = encoding_format.as_str().ok_or_else(|| {
            AppError::new(
                StatusCode::BAD_REQUEST,
                "invalid_request",
                "encoding_format must be 'float' or 'base64'",
            )
        })?;
        if encoding_format != "float" && encoding_format != "base64" {
            return Err(AppError::new(
                StatusCode::BAD_REQUEST,
                "invalid_request",
                "encoding_format must be 'float' or 'base64'",
            ));
        }
    }

    let max_multiplier = resolve_max_multiplier_for_embeddings(&body, &headers, &auth);
    let request_id = extract_request_id(&headers);
    let request_ip = extract_client_ip(&headers);
    let started_at = std::time::Instant::now();
    let routing_stub = build_embeddings_routing_stub(&logical_model, max_multiplier);
    let attempts =
        build_monoize_attempts(&state, &routing_stub, auth.effective_groups.clone()).await?;
    insert_pending_request_log(
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
            let mut upstream_body = body.clone();
            if let Some(upstream_obj) = upstream_body.as_object_mut() {
                upstream_obj.insert(
                    "model".to_string(),
                    Value::String(attempt.upstream_model.clone()),
                );
            }

            let provider = build_channel_provider_config(&attempt);
            let result = upstream::call_upstream_with_timeout_and_headers(
                client_http(&state),
                &provider,
                &attempt.api_key,
                "/v1/embeddings",
                &upstream_body,
                attempt.request_timeout_ms,
                &[],
            )
            .await;

            match result {
                Ok(mut value) => {
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
                    let usage = parse_usage_from_embeddings_object(&value);
                    let charge = match usage.as_ref() {
                        Some(usage_row) => {
                            maybe_charge_usage(&state, &auth, &attempt, &logical_model, usage_row)
                                .await?
                        }
                        None => ChargeComputation::default(),
                    };

                    if let Some(obj) = value.as_object_mut() {
                        obj.insert("model".to_string(), Value::String(logical_model.clone()));
                    }

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
                    );

                    return Ok(Json(value).into_response());
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
                            false,
                            started_at,
                            request_id.clone(),
                            request_ip.clone(),
                            &app_err,
                            None,
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
                        false,
                        started_at,
                        request_id.clone(),
                        request_ip.clone(),
                        &app_err,
                        None,
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

const URP_KNOWN_RESPONSE_FIELDS: [&str; 13] = [
    "model",
    "input",
    "tools",
    "tool_choice",
    "stream",
    "include",
    "store",
    "conversation",
    "previous_response_id",
    "background",
    "max_output_tokens",
    "parallel_tool_calls",
    "max_multiplier",
];

const URP_KNOWN_CHAT_FIELDS: [&str; 8] = [
    "model",
    "messages",
    "tools",
    "tool_choice",
    "stream",
    "max_tokens",
    "parallel_tool_calls",
    "max_multiplier",
];

const URP_KNOWN_MESSAGES_FIELDS: [&str; 8] = [
    "model",
    "messages",
    "max_tokens",
    "stream",
    "tools",
    "tool_choice",
    "parallel_tool_calls",
    "max_multiplier",
];

#[derive(Clone, Debug)]
pub(crate) struct UrpRequest {
    pub(crate) model: String,
    pub(crate) max_multiplier: Option<f64>,
}

#[derive(Clone, Debug)]
struct MonoizeAttempt {
    provider_id: String,
    provider_type: ProviderType,
    channel_id: String,
    base_url: String,
    api_key: String,
    logical_model: String,
    upstream_model: String,
    model_multiplier: f64,
    provider_transforms: Vec<TransformRuleConfig>,
    passive_failure_count_threshold: u32,
    passive_cooldown_seconds: u64,
    passive_window_seconds: u64,
    passive_rate_limit_cooldown_seconds: u64,
    channel_max_retries: i32,
    channel_retry_interval_ms: u64,
    circuit_breaker_enabled: bool,
    per_model_circuit_break: bool,
    provider_attempt_limit: Option<usize>,
    request_timeout_ms: u64,
    extra_fields_whitelist: Option<Vec<String>>,
}

async fn maybe_sleep_before_channel_retry(attempt: &MonoizeAttempt) {
    if attempt.channel_retry_interval_ms == 0 {
        return;
    }
    tokio::time::sleep(std::time::Duration::from_millis(
        attempt.channel_retry_interval_ms,
    ))
    .await;
}

async fn get_model_pricing_or_internal_error(
    state: &AppState,
    model_id: &str,
) -> AppResult<Option<ModelPricing>> {
    state
        .model_registry_store
        .get_model_pricing(model_id)
        .await
        .map_err(|e| AppError::new(StatusCode::INTERNAL_SERVER_ERROR, "internal_error", e))
}

async fn normalized_pricing_model_key(state: &AppState, model_id: &str) -> String {
    let reasoning_suffix_map = state
        .settings_store
        .get_reasoning_suffix_map()
        .await
        .unwrap_or_default();
    normalize_pricing_model_key(model_id, &reasoning_suffix_map)
}

async fn resolve_billable_pricing(
    state: &AppState,
    upstream_model: &str,
    logical_model: &str,
) -> AppResult<Option<ModelPricing>> {
    let normalized_upstream_model = normalized_pricing_model_key(state, upstream_model).await;
    if let Some(pricing) =
        get_model_pricing_or_internal_error(state, &normalized_upstream_model).await?
    {
        return Ok(Some(pricing));
    }
    let normalized_logical_model = normalized_pricing_model_key(state, logical_model).await;
    if normalized_logical_model == normalized_upstream_model {
        return Ok(None);
    }
    get_model_pricing_or_internal_error(state, &normalized_logical_model).await
}

#[derive(Clone, Debug, serde::Serialize)]
struct TriedProvider {
    attempt_number: u32,
    provider_id: String,
    channel_id: String,
    error: String,
}

#[derive(Default)]
struct AttemptExecutionState {
    current_provider_id: Option<String>,
    provider_attempts_used: usize,
    next_attempt_number: u32,
}

impl AttemptExecutionState {
    fn enter_provider(&mut self, provider_id: &str) {
        if self.current_provider_id.as_deref() == Some(provider_id) {
            return;
        }
        self.current_provider_id = Some(provider_id.to_string());
        self.provider_attempts_used = 0;
    }

    fn provider_budget_remaining(&self, attempt: &MonoizeAttempt) -> bool {
        attempt
            .provider_attempt_limit
            .map(|limit| self.provider_attempts_used < limit)
            .unwrap_or(true)
    }

    fn record_upstream_attempt(&mut self) -> u32 {
        self.provider_attempts_used = self.provider_attempts_used.saturating_add(1);
        self.next_attempt_number = self.next_attempt_number.saturating_add(1);
        self.next_attempt_number
    }
}

#[derive(Clone, Copy)]
pub(crate) enum DownstreamProtocol {
    Responses,
    ChatCompletions,
    AnthropicMessages,
}

#[derive(Clone, Debug, Default, serde::Serialize)]
pub(crate) struct StreamTerminalDiagnostics {
    saw_done_sentinel: bool,
    terminal_event: Option<String>,
    terminal_finish_reason: Option<String>,
    synthetic_terminal_emitted: bool,
}

#[derive(Default)]
pub(crate) struct StreamRuntimeMetrics {
    ttfb_ms: Option<u64>,
    usage: Option<urp::Usage>,
    terminal: StreamTerminalDiagnostics,
}

async fn auth_tenant(headers: &HeaderMap, state: &AppState) -> AppResult<crate::auth::AuthResult> {
    let token = if let Some(auth_header) = headers
        .get(axum::http::header::AUTHORIZATION)
        .and_then(|value| value.to_str().ok())
    {
        auth_header.strip_prefix("Bearer ").ok_or_else(|| {
            AppError::new(StatusCode::UNAUTHORIZED, "unauthorized", "invalid auth")
        })?
    } else if let Some(api_key) = headers
        .get("x-api-key")
        .and_then(|value| value.to_str().ok())
        .filter(|value| !value.is_empty())
    {
        api_key
    } else {
        return Err(AppError::new(
            StatusCode::UNAUTHORIZED,
            "unauthorized",
            "missing auth",
        ));
    };

    let auth_result = state
        .auth
        .authenticate_token(token, Some(&state.user_store))
        .await
        .ok_or_else(|| AppError::new(StatusCode::UNAUTHORIZED, "unauthorized", "invalid token"))?;
    check_ip_whitelist(&auth_result, headers)?;
    Ok(auth_result)
}

async fn ensure_balance_before_forward(
    state: &AppState,
    auth: &crate::auth::AuthResult,
) -> AppResult<()> {
    let Some(user_id) = auth.user_id.as_deref() else {
        return Ok(());
    };
    match state.user_store.ensure_user_can_spend(user_id).await {
        Ok(()) => Ok(()),
        Err(err) => match err.kind {
            BillingErrorKind::InsufficientBalance => Err(AppError::new(
                StatusCode::PAYMENT_REQUIRED,
                "insufficient_balance",
                "insufficient balance",
            )),
            BillingErrorKind::NotFound => Err(AppError::new(
                StatusCode::UNAUTHORIZED,
                "unauthorized",
                "user not found",
            )),
            _ => Err(AppError::new(
                StatusCode::INTERNAL_SERVER_ERROR,
                "internal_error",
                err.message,
            )),
        },
    }
}

async fn ensure_quota_before_forward(
    _state: &AppState,
    auth: &crate::auth::AuthResult,
) -> AppResult<()> {
    if auth.quota_unlimited {
        return Ok(());
    }
    if let Some(remaining) = auth.quota_remaining {
        if remaining <= 0 {
            return Err(AppError::new(
                StatusCode::TOO_MANY_REQUESTS,
                "quota_exceeded",
                "API key quota exhausted",
            ));
        }
    }
    Ok(())
}

#[allow(clippy::result_large_err)]
fn split_body(value: Value, known_keys: &[&str]) -> AppResult<(Value, Map<String, Value>)> {
    let known: HashSet<&str> = known_keys.iter().copied().collect();
    let obj = value.as_object().ok_or_else(|| {
        AppError::new(
            StatusCode::BAD_REQUEST,
            "invalid_request",
            "body must be object",
        )
    })?;

    let mut known_obj = Map::new();
    let mut extra = Map::new();
    for (k, v) in obj.iter() {
        if known.contains(k.as_str()) {
            known_obj.insert(k.clone(), v.clone());
        } else {
            extra.insert(k.clone(), v.clone());
        }
    }

    Ok((Value::Object(known_obj), extra))
}

fn parse_max_multiplier_header(headers: &HeaderMap) -> Option<f64> {
    headers
        .get("x-max-multiplier")
        .and_then(|v| v.to_str().ok())
        .and_then(|s| s.parse::<f64>().ok())
        .filter(|v| v.is_finite() && *v > 0.0)
}

#[allow(clippy::result_large_err)]
fn parse_urp_request(known: &Value, extra: Map<String, Value>) -> AppResult<UrpRequest> {
    let merged = merge_known_and_extra(known.clone(), extra);
    let obj = merged.as_object().ok_or_else(|| {
        AppError::new(
            StatusCode::BAD_REQUEST,
            "invalid_request",
            "body must be object",
        )
    })?;
    let model = obj
        .get("model")
        .and_then(|v| v.as_str())
        .ok_or_else(|| AppError::new(StatusCode::BAD_REQUEST, "invalid_request", "missing model"))?
        .to_string();
    let max_multiplier = obj
        .get("max_multiplier")
        .and_then(|v| {
            v.as_f64().or_else(|| {
                v.as_str()
                    .and_then(|s| s.parse::<f64>().ok())
                    .filter(|n| n.is_finite())
            })
        })
        .filter(|n| *n > 0.0);

    Ok(UrpRequest {
        model,
        max_multiplier,
    })
}
