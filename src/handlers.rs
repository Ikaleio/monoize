use crate::app::AppState;
use crate::config::{
    ProviderAuthConfig, ProviderAuthType, ProviderConfig, ProviderType, UnknownFieldPolicy,
};
use crate::error::{AppError, AppResult};
use crate::model_registry_store::ModelPricing;
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
use eventsource_stream::Eventsource;
use futures_util::StreamExt;
use serde_json::{Map, Value, json};
use std::collections::{HashMap, HashSet};
use std::sync::Arc;
use tokio::sync::{Mutex, mpsc};

pub async fn metrics(State(state): State<AppState>) -> impl IntoResponse {
    state.metrics.render()
}

pub async fn list_models(State(state): State<AppState>, headers: HeaderMap) -> AppResult<Response> {
    let _auth = auth_tenant(&headers, &state).await?;
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
    let (known, extra) = split_body(&state, body, &URP_KNOWN_RESPONSE_FIELDS)?;
    let req = decode_urp_request(DownstreamProtocol::Responses, known, extra)?;
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
    let (known, extra) = split_body(&state, body, &URP_KNOWN_CHAT_FIELDS)?;
    let req = decode_urp_request(DownstreamProtocol::ChatCompletions, known, extra)?;
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
    let (known, extra) = split_body(&state, body, &URP_KNOWN_MESSAGES_FIELDS)?;
    let req = decode_urp_request(DownstreamProtocol::AnthropicMessages, known, extra)?;
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
    let attempts = build_monoize_attempts(&state, &routing_stub).await?;
    insert_pending_request_log(
        &state,
        &auth,
        &logical_model,
        false,
        request_id.as_deref(),
        request_ip.as_deref(),
    )
    .await;
    let mut last_failed_attempt: Option<MonoizeAttempt> = None;
    let mut tried_providers: Vec<TriedProvider> = Vec::new();

    for attempt in attempts {
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
            state.monoize_runtime.request_timeout_ms,
            &[],
        )
        .await;

        match result {
            Ok(mut value) => {
                mark_channel_success(&state, &attempt.channel_id).await;
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
                    tried_providers,
                );

                return Ok(Json(value).into_response());
            }
            Err(err) => {
                let non_retryable = is_non_retryable_client_error(&err);
                let retryable = is_retryable_error(&err);
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
                        provider_id: attempt.provider_id.clone(),
                        channel_id: attempt.channel_id.clone(),
                        error: app_err.message.clone(),
                    });
                    mark_channel_retryable_failure(&state, &attempt.channel_id).await;
                    last_failed_attempt = Some(attempt.clone());
                    continue;
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
    let final_err = AppError::new(
        StatusCode::BAD_GATEWAY,
        "upstream_error",
        format!(
            "No available upstream provider for model: {}",
            logical_model
        ),
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
struct UrpRequest {
    model: String,
    max_multiplier: Option<f64>,
}

#[derive(Clone, Debug)]
struct MonoizeAttempt {
    provider_id: String,
    provider_type: ProviderType,
    channel_id: String,
    base_url: String,
    api_key: String,
    upstream_model: String,
    model_multiplier: f64,
    provider_transforms: Vec<TransformRuleConfig>,
}

#[derive(Clone, Debug, serde::Serialize)]
struct TriedProvider {
    provider_id: String,
    channel_id: String,
    error: String,
}

#[derive(Clone, Copy)]
enum DownstreamProtocol {
    Responses,
    ChatCompletions,
    AnthropicMessages,
}

#[derive(Default)]
struct StreamRuntimeMetrics {
    ttfb_ms: Option<u64>,
    usage: Option<urp::Usage>,
}

async fn mark_stream_ttfb_if_needed(
    started_at: Option<std::time::Instant>,
    runtime_metrics: &Option<Arc<Mutex<StreamRuntimeMetrics>>>,
) {
    let Some(started_at) = started_at else {
        return;
    };
    let Some(runtime_metrics) = runtime_metrics.as_ref() else {
        return;
    };
    let mut guard = runtime_metrics.lock().await;
    if guard.ttfb_ms.is_none() {
        guard.ttfb_ms = Some(started_at.elapsed().as_millis() as u64);
    }
}

async fn record_stream_usage_if_present(
    runtime_metrics: &Option<Arc<Mutex<StreamRuntimeMetrics>>>,
    usage: Option<urp::Usage>,
) {
    let Some(usage) = usage else {
        return;
    };
    let Some(runtime_metrics) = runtime_metrics.as_ref() else {
        return;
    };
    let mut guard = runtime_metrics.lock().await;
    let new_total = usage.prompt_tokens.saturating_add(usage.completion_tokens);
    let replace = match guard.usage.as_ref() {
        Some(existing) => {
            let existing_total = existing
                .prompt_tokens
                .saturating_add(existing.completion_tokens);
            new_total >= existing_total
        }
        None => true,
    };
    if replace {
        guard.usage = Some(usage);
    }
}

fn parse_usage_from_chat_object(obj: &Value) -> Option<urp::Usage> {
    let usage = obj.get("usage")?.as_object()?;
    let prompt_tokens = usage.get("prompt_tokens")?.as_u64()?;
    let completion_tokens = usage.get("completion_tokens")?.as_u64()?;
    let cached_tokens = usage
        .get("prompt_tokens_details")
        .and_then(|v| v.get("cached_tokens"))
        .and_then(|v| v.as_u64());
    let reasoning_tokens = usage
        .get("completion_tokens_details")
        .and_then(|v| v.get("reasoning_tokens"))
        .and_then(|v| v.as_u64());
    let mut extra_body = HashMap::new();
    if let Some(v) = usage.get("prompt_tokens_details") {
        extra_body.insert("prompt_tokens_details".to_string(), v.clone());
    }
    if let Some(v) = usage.get("completion_tokens_details") {
        extra_body.insert("completion_tokens_details".to_string(), v.clone());
    }
    if let Some(v) = usage.get("total_tokens") {
        extra_body.insert("total_tokens".to_string(), v.clone());
    }
    Some(urp::Usage {
        prompt_tokens,
        completion_tokens,
        reasoning_tokens,
        cached_tokens,
        extra_body,
    })
}

fn parse_usage_from_responses_object(obj: &Value) -> Option<urp::Usage> {
    let usage = obj
        .get("usage")
        .or_else(|| obj.get("response").and_then(|v| v.get("usage")))?;
    let prompt_tokens = usage
        .get("input_tokens")
        .or_else(|| usage.get("prompt_tokens"))
        .and_then(|v| v.as_u64())?;
    let completion_tokens = usage
        .get("output_tokens")
        .or_else(|| usage.get("completion_tokens"))
        .and_then(|v| v.as_u64())?;
    let cached_tokens = usage
        .get("input_tokens_details")
        .and_then(|v| v.get("cached_tokens"))
        .or_else(|| {
            usage
                .get("prompt_tokens_details")
                .and_then(|v| v.get("cached_tokens"))
        })
        .and_then(|v| v.as_u64());
    let reasoning_tokens = usage
        .get("output_tokens_details")
        .and_then(|v| v.get("reasoning_tokens"))
        .or_else(|| {
            usage
                .get("completion_tokens_details")
                .and_then(|v| v.get("reasoning_tokens"))
        })
        .and_then(|v| v.as_u64());
    let mut extra_body = HashMap::new();
    if let Some(v) = usage.get("input_tokens_details") {
        extra_body.insert("input_tokens_details".to_string(), v.clone());
    }
    if let Some(v) = usage.get("output_tokens_details") {
        extra_body.insert("output_tokens_details".to_string(), v.clone());
    }
    if let Some(v) = usage.get("prompt_tokens_details") {
        extra_body.insert("prompt_tokens_details".to_string(), v.clone());
    }
    if let Some(v) = usage.get("completion_tokens_details") {
        extra_body.insert("completion_tokens_details".to_string(), v.clone());
    }
    if let Some(v) = usage.get("total_tokens") {
        extra_body.insert("total_tokens".to_string(), v.clone());
    }
    Some(urp::Usage {
        prompt_tokens,
        completion_tokens,
        reasoning_tokens,
        cached_tokens,
        extra_body,
    })
}

fn parse_usage_from_messages_object(obj: &Value) -> Option<urp::Usage> {
    let usage = obj
        .get("usage")
        .or_else(|| obj.get("message").and_then(|v| v.get("usage")))?
        .as_object()?;
    let prompt_tokens = usage.get("input_tokens")?.as_u64()?;
    let completion_tokens = usage.get("output_tokens")?.as_u64()?;
    let cached_tokens = usage
        .get("cache_read_input_tokens")
        .and_then(|v| v.as_u64());
    let mut extra_body = HashMap::new();
    if let Some(v) = usage.get("cache_creation_input_tokens") {
        extra_body.insert("cache_creation_input_tokens".to_string(), v.clone());
    }
    if let Some(v) = usage.get("cache_read_input_tokens") {
        extra_body.insert("cache_read_input_tokens".to_string(), v.clone());
    }
    Some(urp::Usage {
        prompt_tokens,
        completion_tokens,
        reasoning_tokens: None,
        cached_tokens,
        extra_body,
    })
}

fn parse_usage_from_gemini_object(obj: &Value) -> Option<urp::Usage> {
    let usage = obj.get("usageMetadata")?.as_object()?;
    let prompt_tokens = usage
        .get("promptTokenCount")
        .or_else(|| usage.get("prompt_token_count"))
        .and_then(|v| v.as_u64())?;
    let completion_tokens = usage
        .get("candidatesTokenCount")
        .or_else(|| usage.get("candidates_token_count"))
        .and_then(|v| v.as_u64())?;
    let cached_tokens = usage
        .get("cachedContentTokenCount")
        .or_else(|| usage.get("cached_content_token_count"))
        .and_then(|v| v.as_u64());
    let reasoning_tokens = usage
        .get("thoughtsTokenCount")
        .or_else(|| usage.get("thoughts_token_count"))
        .and_then(|v| v.as_u64());
    let mut extra_body = HashMap::new();
    if let Some(v) = usage.get("cachedContentTokenCount") {
        extra_body.insert("cachedContentTokenCount".to_string(), v.clone());
    }
    if let Some(v) = usage.get("cached_content_token_count") {
        extra_body.insert("cached_content_token_count".to_string(), v.clone());
    }
    if let Some(v) = usage.get("thoughtsTokenCount") {
        extra_body.insert("thoughtsTokenCount".to_string(), v.clone());
    }
    if let Some(v) = usage.get("thoughts_token_count") {
        extra_body.insert("thoughts_token_count".to_string(), v.clone());
    }
    if let Some(v) = usage.get("totalTokenCount") {
        extra_body.insert("totalTokenCount".to_string(), v.clone());
    }
    if let Some(v) = usage.get("total_token_count") {
        extra_body.insert("total_token_count".to_string(), v.clone());
    }
    Some(urp::Usage {
        prompt_tokens,
        completion_tokens,
        reasoning_tokens,
        cached_tokens,
        extra_body,
    })
}

fn parse_usage_from_embeddings_object(obj: &Value) -> Option<urp::Usage> {
    let usage = obj.get("usage")?.as_object()?;
    let prompt_tokens = usage.get("prompt_tokens")?.as_u64()?;
    let total_tokens = usage.get("total_tokens")?.as_u64()?;
    let mut extra_body = HashMap::new();
    extra_body.insert("total_tokens".to_string(), Value::from(total_tokens));
    Some(urp::Usage {
        prompt_tokens,
        completion_tokens: 0,
        reasoning_tokens: None,
        cached_tokens: None,
        extra_body,
    })
}

fn decode_urp_request(
    protocol: DownstreamProtocol,
    known: Value,
    extra: Map<String, Value>,
) -> AppResult<urp::UrpRequest> {
    let merged = merge_known_and_extra(known, extra);
    let decoded = match protocol {
        DownstreamProtocol::Responses => urp::decode::openai_responses::decode_request(&merged),
        DownstreamProtocol::ChatCompletions => urp::decode::openai_chat::decode_request(&merged),
        DownstreamProtocol::AnthropicMessages => urp::decode::anthropic::decode_request(&merged),
    };
    decoded.map_err(|e| AppError::new(StatusCode::BAD_REQUEST, "invalid_request", e))
}

fn merge_known_and_extra(known: Value, extra: Map<String, Value>) -> Value {
    let mut obj = known.as_object().cloned().unwrap_or_default();
    for (k, v) in extra {
        obj.insert(k, v);
    }
    Value::Object(obj)
}

fn resolve_max_multiplier(
    req: &urp::UrpRequest,
    headers: &HeaderMap,
    auth: &crate::auth::AuthResult,
) -> Option<f64> {
    let ceiling = auth.max_multiplier;
    let requested =
        read_max_multiplier_from_extra(req).or_else(|| parse_max_multiplier_header(headers));

    match (ceiling, requested) {
        (Some(c), Some(r)) => Some(r.min(c)),
        (Some(c), None) => Some(c),
        (None, Some(r)) => Some(r),
        (None, None) => None,
    }
}

fn extract_client_ip(headers: &HeaderMap) -> Option<String> {
    headers
        .get("x-forwarded-for")
        .and_then(|v| v.to_str().ok())
        .and_then(|v| v.split(',').next())
        .map(|s| s.trim().to_string())
        .or_else(|| {
            headers
                .get("x-real-ip")
                .and_then(|v| v.to_str().ok())
                .map(|s| s.trim().to_string())
        })
}

fn extract_request_id(headers: &HeaderMap) -> Option<String> {
    headers
        .get("x-request-id")
        .and_then(|v| v.to_str().ok())
        .map(|s| s.to_string())
}

fn read_max_multiplier_from_extra(req: &urp::UrpRequest) -> Option<f64> {
    req.extra_body
        .get("max_multiplier")
        .and_then(|v| {
            v.as_f64().or_else(|| {
                v.as_str()
                    .and_then(|s| s.parse::<f64>().ok())
                    .filter(|n| n.is_finite())
            })
        })
        .filter(|n| *n > 0.0)
}

fn apply_transform_rules_request(
    state: &AppState,
    req: &mut urp::UrpRequest,
    rules: &[TransformRuleConfig],
) -> AppResult<()> {
    if rules.is_empty() {
        return Ok(());
    }
    let mut states = transforms::build_states_for_rules(rules, state.transform_registry.as_ref())
        .map_err(|e| {
        AppError::new(
            StatusCode::INTERNAL_SERVER_ERROR,
            "transform_init_failed",
            e.to_string(),
        )
    })?;
    let model = req.model.clone();
    transforms::apply_transforms(
        transforms::UrpData::Request(req),
        rules,
        &mut states,
        &model,
        Phase::Request,
        state.transform_registry.as_ref(),
    )
    .map_err(|e| {
        AppError::new(
            StatusCode::INTERNAL_SERVER_ERROR,
            "transform_apply_failed",
            e.to_string(),
        )
    })
}

fn apply_transform_rules_response(
    state: &AppState,
    resp: &mut urp::UrpResponse,
    rules: &[TransformRuleConfig],
    model: &str,
) -> AppResult<()> {
    if rules.is_empty() {
        return Ok(());
    }
    let mut states = transforms::build_states_for_rules(rules, state.transform_registry.as_ref())
        .map_err(|e| {
        AppError::new(
            StatusCode::INTERNAL_SERVER_ERROR,
            "transform_init_failed",
            e.to_string(),
        )
    })?;
    transforms::apply_transforms(
        transforms::UrpData::Response(resp),
        rules,
        &mut states,
        model,
        Phase::Response,
        state.transform_registry.as_ref(),
    )
    .map_err(|e| {
        AppError::new(
            StatusCode::INTERNAL_SERVER_ERROR,
            "transform_apply_failed",
            e.to_string(),
        )
    })
}

fn typed_request_to_legacy(
    req: &urp::UrpRequest,
    max_multiplier: Option<f64>,
) -> AppResult<UrpRequest> {
    let encoded = urp::encode::openai_responses::encode_request(req, &req.model);
    let mut extra = Map::new();
    if let Some(limit) = max_multiplier {
        extra.insert("max_multiplier".to_string(), Value::from(limit));
    }
    parse_urp_request(&encoded, extra)
}

fn build_routing_stub(req: &urp::UrpRequest, max_multiplier: Option<f64>) -> UrpRequest {
    UrpRequest {
        model: req.model.clone(),
        max_multiplier,
    }
}

fn build_embeddings_routing_stub(model: &str, max_multiplier: Option<f64>) -> UrpRequest {
    UrpRequest {
        model: model.to_string(),
        max_multiplier,
    }
}

fn is_valid_embeddings_input(input: &Value) -> bool {
    if input.as_str().is_some() {
        return true;
    }
    input
        .as_array()
        .is_some_and(|arr| arr.iter().all(|item| item.as_str().is_some()))
}

fn read_max_multiplier_from_embeddings_body(body: &Value) -> Option<f64> {
    body.as_object()
        .and_then(|obj| obj.get("max_multiplier"))
        .and_then(|v| {
            v.as_f64().or_else(|| {
                v.as_str()
                    .and_then(|s| s.parse::<f64>().ok())
                    .filter(|n| n.is_finite())
            })
        })
        .filter(|n| *n > 0.0)
}

fn resolve_max_multiplier_for_embeddings(
    body: &Value,
    headers: &HeaderMap,
    auth: &crate::auth::AuthResult,
) -> Option<f64> {
    let ceiling = auth.max_multiplier;
    let requested = read_max_multiplier_from_embeddings_body(body)
        .or_else(|| parse_max_multiplier_header(headers));

    match (ceiling, requested) {
        (Some(c), Some(r)) => Some(r.min(c)),
        (Some(c), None) => Some(c),
        (None, Some(r)) => Some(r),
        (None, None) => None,
    }
}

fn has_enabled_response_rules(rules: &[TransformRuleConfig], model: &str) -> bool {
    rules
        .iter()
        .filter(|rule| rule.enabled && rule.phase == Phase::Response)
        .any(|rule| match &rule.models {
            None => true,
            Some(patterns) => patterns
                .iter()
                .any(|pattern| model_glob_match(pattern, model)),
        })
}

fn model_glob_match(pattern: &str, model: &str) -> bool {
    if pattern == "*" {
        return true;
    }
    let mut regex = String::from("^");
    for ch in pattern.chars() {
        match ch {
            '*' => regex.push_str(".*"),
            '?' => regex.push('.'),
            other => regex.push_str(&regex::escape(&other.to_string())),
        }
    }
    regex.push('$');
    regex::Regex::new(&regex)
        .map(|re| re.is_match(model))
        .unwrap_or(false)
}

async fn execute_nonstream_typed(
    state: &AppState,
    auth: &crate::auth::AuthResult,
    mut req: urp::UrpRequest,
    max_multiplier: Option<f64>,
    request_id: Option<String>,
    request_ip: Option<String>,
) -> AppResult<(urp::UrpResponse, String)> {
    let started_at = std::time::Instant::now();
    apply_transform_rules_request(state, &mut req, &auth.transforms)?;
    resolve_model_suffix(state, &mut req).await;
    let routing_stub = build_routing_stub(&req, max_multiplier);
    let attempts = build_monoize_attempts(state, &routing_stub).await?;
    insert_pending_request_log(
        state,
        auth,
        &req.model,
        false,
        request_id.as_deref(),
        request_ip.as_deref(),
    )
    .await;
    let mut last_failed_attempt: Option<MonoizeAttempt> = None;
    let mut tried_providers: Vec<TriedProvider> = Vec::new();
    for attempt in attempts {
        let mut req_attempt = req.clone();
        req_attempt.model = attempt.upstream_model.clone();
        apply_transform_rules_request(state, &mut req_attempt, &attempt.provider_transforms)?;

        let upstream_body = encode_request_for_provider(&req_attempt, &attempt)?;
        let provider = build_channel_provider_config(&attempt);
        let path = upstream_path_for_model(
            attempt.provider_type,
            &req_attempt.model,
            req_attempt.stream.unwrap_or(false),
        );
        let call = upstream::call_upstream_with_timeout_and_headers(
            client_http(state),
            &provider,
            &attempt.api_key,
            &path,
            &upstream_body,
            state.monoize_runtime.request_timeout_ms,
            provider_extra_headers(attempt.provider_type),
        )
        .await;
        match call {
            Ok(value) => {
                mark_channel_success(state, &attempt.channel_id).await;
                let mut resp = decode_response_from_provider(attempt.provider_type, &value)?;
                apply_transform_rules_response(
                    state,
                    &mut resp,
                    &attempt.provider_transforms,
                    &req.model,
                )?;
                apply_transform_rules_response(state, &mut resp, &auth.transforms, &req.model)?;
                let charge =
                    maybe_charge_response(state, auth, &attempt, &req.model, &resp).await?;
                spawn_request_log(
                    state,
                    auth,
                    &attempt,
                    &req.model,
                    resp.usage.clone(),
                    charge.charge_nano_usd,
                    charge.billing_breakdown,
                    false,
                    started_at,
                    request_id.clone(),
                    request_ip.clone(),
                    attempt.channel_id.clone(),
                    None,
                    req.reasoning.as_ref().and_then(|r| r.effort.clone()),
                    tried_providers,
                );
                return Ok((resp, req.model.clone()));
            }
            Err(err) => {
                let non_retryable = is_non_retryable_client_error(&err);
                let retryable = is_retryable_error(&err);
                let app_err = upstream_error_to_app(err);
                if non_retryable {
                    spawn_request_log_error(
                        state,
                        auth,
                        &attempt,
                        &req.model,
                        false,
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
                    mark_channel_retryable_failure(state, &attempt.channel_id).await;
                    last_failed_attempt = Some(attempt.clone());
                    continue;
                }
                spawn_request_log_error(
                    state,
                    auth,
                    &attempt,
                    &req.model,
                    false,
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
        format!("No available upstream provider for model: {}", req.model),
    );
    if let Some(attempt) = last_failed_attempt {
        spawn_request_log_error(
            state,
            auth,
            &attempt,
            &req.model,
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
            &req.model,
            false,
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

async fn forward_nonstream_typed(
    state: &AppState,
    auth: &crate::auth::AuthResult,
    req: urp::UrpRequest,
    max_multiplier: Option<f64>,
    downstream: DownstreamProtocol,
    request_id: Option<String>,
    request_ip: Option<String>,
) -> AppResult<Value> {
    let (resp, logical_model) =
        execute_nonstream_typed(state, auth, req, max_multiplier, request_id, request_ip).await?;
    Ok(encode_response_for_downstream(
        downstream,
        &resp,
        &logical_model,
    ))
}

fn encode_request_for_provider(
    req: &urp::UrpRequest,
    attempt: &MonoizeAttempt,
) -> AppResult<Value> {
    let value = match attempt.provider_type {
        ProviderType::Responses => urp::encode::openai_responses::encode_request(req, &req.model),
        ProviderType::ChatCompletion => urp::encode::openai_chat::encode_request(req, &req.model),
        ProviderType::Messages => urp::encode::anthropic::encode_request(req, &req.model),
        ProviderType::Gemini => urp::encode::gemini::encode_request(req, &req.model),
        ProviderType::Grok => urp::encode::grok::encode_request(req, &req.model),
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

fn decode_response_from_provider(
    provider_type: ProviderType,
    value: &Value,
) -> AppResult<urp::UrpResponse> {
    let decoded = match provider_type {
        ProviderType::Responses => urp::decode::openai_responses::decode_response(value),
        ProviderType::ChatCompletion => urp::decode::openai_chat::decode_response(value),
        ProviderType::Messages => urp::decode::anthropic::decode_response(value),
        ProviderType::Gemini => urp::decode::gemini::decode_response(value),
        ProviderType::Grok => urp::decode::grok::decode_response(value),
        ProviderType::Group => Err("provider_type group is virtual".to_string()),
    };
    decoded.map_err(|e| AppError::new(StatusCode::BAD_GATEWAY, "invalid_upstream_response", e))
}

fn encode_response_for_downstream(
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

#[derive(Debug, Clone)]
struct ChargeComponents {
    prompt_tokens: i128,
    completion_tokens: i128,
    cached_tokens: i128,
    reasoning_tokens: i128,
    billed_uncached_prompt_tokens: i128,
    billed_cached_prompt_tokens: i128,
    billed_non_reasoning_completion_tokens: i128,
    billed_reasoning_completion_tokens: i128,
    uncached_prompt_charge: i128,
    cached_prompt_charge: i128,
    non_reasoning_completion_charge: i128,
    reasoning_completion_charge: i128,
    prompt_charge: i128,
    completion_charge: i128,
    base_charge: i128,
    final_charge: i128,
}

#[derive(Debug, Clone, Default)]
struct ChargeComputation {
    charge_nano_usd: Option<i128>,
    billing_breakdown: Option<Value>,
}

fn non_negative_i128_to_u64(value: i128) -> u64 {
    if value <= 0 {
        0
    } else {
        u64::try_from(value).unwrap_or(u64::MAX)
    }
}

fn parse_u64_value(value: &Value) -> Option<u64> {
    value
        .as_u64()
        .or_else(|| value.as_i64().and_then(|v| u64::try_from(v).ok()))
        .or_else(|| value.as_str().and_then(|s| s.parse::<u64>().ok()))
}

fn map_get_u64(map: &Map<String, Value>, key: &str) -> Option<u64> {
    map.get(key).and_then(parse_u64_value)
}

fn calculate_charge_components(
    usage: &urp::Usage,
    pricing: &ModelPricing,
    provider_multiplier: f64,
) -> Option<ChargeComponents> {
    let prompt_tokens = i128::from(usage.prompt_tokens);
    let completion_tokens = i128::from(usage.completion_tokens);
    let cached_tokens = i128::from(usage.cached_tokens.unwrap_or(0));
    let reasoning_tokens = i128::from(usage.reasoning_tokens.unwrap_or(0));

    let uncached_prompt_tokens = (prompt_tokens - cached_tokens).max(0);
    let non_reasoning_completion_tokens = (completion_tokens - reasoning_tokens).max(0);

    let (
        billed_uncached_prompt_tokens,
        billed_cached_prompt_tokens,
        uncached_prompt_charge,
        cached_prompt_charge,
    ) = if let Some(cached_rate) = pricing.cache_read_input_cost_per_token_nano {
        let uncached_charge =
            uncached_prompt_tokens.checked_mul(pricing.input_cost_per_token_nano)?;
        let cached_charge = cached_tokens.max(0).checked_mul(cached_rate)?;
        (
            uncached_prompt_tokens,
            cached_tokens.max(0),
            uncached_charge,
            cached_charge,
        )
    } else {
        (
            prompt_tokens.max(0),
            0,
            prompt_tokens.checked_mul(pricing.input_cost_per_token_nano)?,
            0,
        )
    };
    let prompt_charge = uncached_prompt_charge.checked_add(cached_prompt_charge)?;

    let (
        billed_non_reasoning_completion_tokens,
        billed_reasoning_completion_tokens,
        non_reasoning_completion_charge,
        reasoning_completion_charge,
    ) = if let Some(reasoning_rate) = pricing.output_cost_per_reasoning_token_nano {
        let non_reasoning_charge =
            non_reasoning_completion_tokens.checked_mul(pricing.output_cost_per_token_nano)?;
        let reasoning_charge = reasoning_tokens.max(0).checked_mul(reasoning_rate)?;
        (
            non_reasoning_completion_tokens,
            reasoning_tokens.max(0),
            non_reasoning_charge,
            reasoning_charge,
        )
    } else {
        (
            completion_tokens.max(0),
            0,
            completion_tokens.checked_mul(pricing.output_cost_per_token_nano)?,
            0,
        )
    };
    let completion_charge =
        non_reasoning_completion_charge.checked_add(reasoning_completion_charge)?;

    let base_charge = prompt_charge.checked_add(completion_charge)?;
    let final_charge = scale_charge_with_multiplier(base_charge, provider_multiplier)?;

    Some(ChargeComponents {
        prompt_tokens,
        completion_tokens,
        cached_tokens,
        reasoning_tokens,
        billed_uncached_prompt_tokens,
        billed_cached_prompt_tokens,
        billed_non_reasoning_completion_tokens,
        billed_reasoning_completion_tokens,
        uncached_prompt_charge,
        cached_prompt_charge,
        non_reasoning_completion_charge,
        reasoning_completion_charge,
        prompt_charge,
        completion_charge,
        base_charge,
        final_charge,
    })
}

#[cfg(test)]
fn calculate_charge_nano(
    usage: &urp::Usage,
    pricing: &ModelPricing,
    provider_multiplier: f64,
) -> Option<i128> {
    calculate_charge_components(usage, pricing, provider_multiplier).map(|parts| parts.final_charge)
}

fn build_usage_breakdown(usage: &urp::Usage) -> Value {
    let input_details = usage
        .extra_body
        .get("input_tokens_details")
        .or_else(|| usage.extra_body.get("prompt_tokens_details"))
        .and_then(|v| v.as_object());
    let output_details = usage
        .extra_body
        .get("output_tokens_details")
        .or_else(|| usage.extra_body.get("completion_tokens_details"))
        .and_then(|v| v.as_object());

    let input_cached = usage
        .cached_tokens
        .or_else(|| input_details.and_then(|d| map_get_u64(d, "cached_tokens")))
        .or_else(|| {
            usage
                .extra_body
                .get("cache_read_input_tokens")
                .and_then(parse_u64_value)
        });
    let input_cache_creation = usage
        .extra_body
        .get("cache_creation_input_tokens")
        .and_then(parse_u64_value);
    let input_audio = input_details.and_then(|d| map_get_u64(d, "audio_tokens"));
    let input_image = input_details.and_then(|d| map_get_u64(d, "image_tokens"));
    let input_text = input_details.and_then(|d| map_get_u64(d, "text_tokens"));
    let output_reasoning = usage
        .reasoning_tokens
        .or_else(|| output_details.and_then(|d| map_get_u64(d, "reasoning_tokens")));
    let output_audio = output_details.and_then(|d| map_get_u64(d, "audio_tokens"));
    let output_image = output_details.and_then(|d| map_get_u64(d, "image_tokens"));
    let output_text = output_details.and_then(|d| map_get_u64(d, "text_tokens"));

    json!({
        "version": 1,
        "input": {
            "total_tokens": usage.prompt_tokens,
            "uncached_tokens": usage.prompt_tokens.saturating_sub(input_cached.unwrap_or(0)),
            "text_tokens": input_text,
            "cached_tokens": input_cached,
            "cache_creation_tokens": input_cache_creation,
            "audio_tokens": input_audio,
            "image_tokens": input_image
        },
        "output": {
            "total_tokens": usage.completion_tokens,
            "non_reasoning_tokens": usage.completion_tokens.saturating_sub(output_reasoning.unwrap_or(0)),
            "text_tokens": output_text,
            "reasoning_tokens": output_reasoning,
            "audio_tokens": output_audio,
            "image_tokens": output_image
        },
        "raw_usage_extra": usage.extra_body
    })
}

fn build_billing_breakdown(
    logical_model: &str,
    attempt: &MonoizeAttempt,
    pricing: &ModelPricing,
    components: &ChargeComponents,
) -> Value {
    json!({
        "version": 1,
        "currency": "nano_usd",
        "logical_model": logical_model,
        "upstream_model": attempt.upstream_model,
        "provider_id": attempt.provider_id,
        "provider_multiplier": attempt.model_multiplier,
        "input": {
            "total_tokens": non_negative_i128_to_u64(components.prompt_tokens),
            "cached_tokens": non_negative_i128_to_u64(components.cached_tokens),
            "billed_uncached_tokens": non_negative_i128_to_u64(components.billed_uncached_prompt_tokens),
            "billed_cached_tokens": non_negative_i128_to_u64(components.billed_cached_prompt_tokens),
            "unit_price_nano": pricing.input_cost_per_token_nano.to_string(),
            "cached_unit_price_nano": pricing.cache_read_input_cost_per_token_nano.map(|v| v.to_string()),
            "uncached_charge_nano": components.uncached_prompt_charge.to_string(),
            "cached_charge_nano": components.cached_prompt_charge.to_string(),
            "total_charge_nano": components.prompt_charge.to_string(),
        },
        "output": {
            "total_tokens": non_negative_i128_to_u64(components.completion_tokens),
            "reasoning_tokens": non_negative_i128_to_u64(components.reasoning_tokens),
            "billed_non_reasoning_tokens": non_negative_i128_to_u64(components.billed_non_reasoning_completion_tokens),
            "billed_reasoning_tokens": non_negative_i128_to_u64(components.billed_reasoning_completion_tokens),
            "unit_price_nano": pricing.output_cost_per_token_nano.to_string(),
            "reasoning_unit_price_nano": pricing.output_cost_per_reasoning_token_nano.map(|v| v.to_string()),
            "non_reasoning_charge_nano": components.non_reasoning_completion_charge.to_string(),
            "reasoning_charge_nano": components.reasoning_completion_charge.to_string(),
            "total_charge_nano": components.completion_charge.to_string(),
        },
        "base_charge_nano": components.base_charge.to_string(),
        "final_charge_nano": components.final_charge.to_string(),
    })
}

fn scale_charge_with_multiplier(base_nano: i128, provider_multiplier: f64) -> Option<i128> {
    if !provider_multiplier.is_finite() || provider_multiplier < 0.0 {
        return None;
    }

    const SCALE: i128 = 1_000_000_000;
    let multiplier_repr = format!("{provider_multiplier:.18}");
    let mut parts = multiplier_repr.split('.');
    let whole = parts.next().unwrap_or("0").parse::<i128>().ok()?;
    let frac_raw = parts.next().unwrap_or("0");
    let mut frac_nano = String::with_capacity(9);
    for ch in frac_raw.chars().take(9) {
        frac_nano.push(ch);
    }
    while frac_nano.len() < 9 {
        frac_nano.push('0');
    }
    let frac = frac_nano.parse::<i128>().ok()?;

    let multiplier_nano = whole.checked_mul(SCALE)?.checked_add(frac)?;
    base_nano.checked_mul(multiplier_nano)?.checked_div(SCALE)
}

async fn maybe_charge_usage(
    state: &AppState,
    auth: &crate::auth::AuthResult,
    attempt: &MonoizeAttempt,
    logical_model: &str,
    usage: &urp::Usage,
) -> AppResult<ChargeComputation> {
    let Some(user_id) = auth.user_id.as_deref() else {
        return Ok(ChargeComputation::default());
    };
    let pricing = match state
        .model_registry_store
        .get_model_pricing(&attempt.upstream_model)
        .await
        .map_err(|e| AppError::new(StatusCode::INTERNAL_SERVER_ERROR, "internal_error", e))?
    {
        Some(v) => v,
        None => {
            tracing::warn!(
                "billing skipped: no model pricing for upstream_model={}",
                attempt.upstream_model
            );
            return Ok(ChargeComputation::default());
        }
    };

    let Some(components) = calculate_charge_components(usage, &pricing, attempt.model_multiplier)
    else {
        tracing::warn!(
            "billing skipped: overflow/invalid charge for model={}",
            attempt.upstream_model
        );
        return Ok(ChargeComputation::default());
    };
    let billing_breakdown = build_billing_breakdown(logical_model, attempt, &pricing, &components);
    let charge_nano = components.final_charge;
    if charge_nano <= 0 {
        return Ok(ChargeComputation {
            charge_nano_usd: None,
            billing_breakdown: Some(billing_breakdown),
        });
    }

    let meta = json!({
        "logical_model": logical_model,
        "upstream_model": attempt.upstream_model,
        "provider_id": attempt.provider_id,
        "provider_multiplier": attempt.model_multiplier,
        "prompt_tokens": usage.prompt_tokens,
        "completion_tokens": usage.completion_tokens,
        "cached_tokens": usage.cached_tokens,
        "reasoning_tokens": usage.reasoning_tokens,
        "charge_nano_usd": charge_nano.to_string(),
    });

    match state
        .user_store
        .charge_user_balance_nano(user_id, charge_nano, &meta)
        .await
    {
        Ok(()) => Ok(ChargeComputation {
            charge_nano_usd: Some(charge_nano),
            billing_breakdown: Some(billing_breakdown),
        }),
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

async fn maybe_charge_response(
    state: &AppState,
    auth: &crate::auth::AuthResult,
    attempt: &MonoizeAttempt,
    logical_model: &str,
    response: &urp::UrpResponse,
) -> AppResult<ChargeComputation> {
    let Some(usage) = response.usage.as_ref() else {
        return Ok(ChargeComputation::default());
    };
    maybe_charge_usage(state, auth, attempt, logical_model, usage).await
}

async fn insert_pending_request_log(
    state: &AppState,
    auth: &crate::auth::AuthResult,
    model: &str,
    is_stream: bool,
    request_id: Option<&str>,
    request_ip: Option<&str>,
) {
    let Some(user_id) = auth.user_id.as_deref() else {
        return;
    };
    let Some(request_id) = request_id.map(str::trim).filter(|v| !v.is_empty()) else {
        return;
    };

    if let Err(e) = state
        .user_store
        .insert_request_log_pending(
            request_id,
            user_id,
            auth.api_key_id.as_deref(),
            model,
            is_stream,
            request_ip,
        )
        .await
    {
        tracing::warn!("failed to insert pending request log: {e}");
    }
}

fn spawn_request_log(
    state: &AppState,
    auth: &crate::auth::AuthResult,
    attempt: &MonoizeAttempt,
    model: &str,
    usage: Option<urp::Usage>,
    charge_nano_usd: Option<i128>,
    billing_breakdown_json: Option<Value>,
    is_stream: bool,
    started_at: std::time::Instant,
    request_id: Option<String>,
    request_ip: Option<String>,
    channel_id: String,
    ttfb_ms: Option<u64>,
    reasoning_effort: Option<String>,
    tried_providers: Vec<TriedProvider>,
) {
    let Some(user_id) = auth.user_id.clone() else {
        return;
    };
    let api_key_id = auth.api_key_id.clone();
    let provider_id = attempt.provider_id.clone();
    let upstream_model = attempt.upstream_model.clone();
    let model_multiplier = attempt.model_multiplier;
    let model = model.to_string();
    let duration_ms = started_at.elapsed().as_millis() as u64;
    let user_store = state.user_store.clone();
    let usage_breakdown_json = usage.as_ref().map(build_usage_breakdown);
    let tried_providers_json = if tried_providers.is_empty() {
        None
    } else {
        serde_json::to_value(&tried_providers).ok()
    };

    tokio::spawn(async move {
        let log = InsertRequestLog {
            request_id,
            user_id,
            api_key_id,
            model,
            provider_id: Some(provider_id),
            upstream_model: Some(upstream_model),
            channel_id: Some(channel_id),
            is_stream,
            prompt_tokens: usage.as_ref().map(|u| u.prompt_tokens),
            completion_tokens: usage.as_ref().map(|u| u.completion_tokens),
            cached_tokens: usage.as_ref().and_then(|u| u.cached_tokens),
            reasoning_tokens: usage.as_ref().and_then(|u| u.reasoning_tokens),
            provider_multiplier: Some(model_multiplier),
            charge_nano_usd,
            status: REQUEST_LOG_STATUS_SUCCESS.to_string(),
            usage_breakdown_json,
            billing_breakdown_json,
            error_code: None,
            error_message: None,
            error_http_status: None,
            duration_ms: Some(duration_ms),
            ttfb_ms,
            request_ip,
            reasoning_effort,
            tried_providers_json,
            request_kind: None,
        };
        if let Err(e) = user_store.finalize_request_log(log).await {
            tracing::warn!("failed to finalize request log: {e}");
        }
    });
}

fn spawn_request_log_error(
    state: &AppState,
    auth: &crate::auth::AuthResult,
    attempt: &MonoizeAttempt,
    model: &str,
    is_stream: bool,
    started_at: std::time::Instant,
    request_id: Option<String>,
    request_ip: Option<String>,
    error: &AppError,
    reasoning_effort: Option<String>,
    tried_providers: Vec<TriedProvider>,
) {
    let Some(user_id) = auth.user_id.clone() else {
        return;
    };
    let api_key_id = auth.api_key_id.clone();
    let model = model.to_string();
    let provider_id = attempt.provider_id.clone();
    let upstream_model = attempt.upstream_model.clone();
    let model_multiplier = attempt.model_multiplier;
    let channel_id = attempt.channel_id.clone();
    let duration_ms = started_at.elapsed().as_millis() as u64;
    let user_store = state.user_store.clone();
    let error_code = Some(error.code.clone());
    let error_message = Some(error.message.clone());
    let error_http_status = Some(error.status.as_u16());
    let tried_providers_json = if tried_providers.is_empty() {
        None
    } else {
        serde_json::to_value(&tried_providers).ok()
    };

    tokio::spawn(async move {
        let log = InsertRequestLog {
            request_id,
            user_id,
            api_key_id,
            model,
            provider_id: Some(provider_id),
            upstream_model: Some(upstream_model),
            channel_id: Some(channel_id),
            is_stream,
            prompt_tokens: None,
            completion_tokens: None,
            cached_tokens: None,
            reasoning_tokens: None,
            provider_multiplier: Some(model_multiplier),
            charge_nano_usd: None,
            status: REQUEST_LOG_STATUS_ERROR.to_string(),
            usage_breakdown_json: None,
            billing_breakdown_json: None,
            error_code,
            error_message,
            error_http_status,
            duration_ms: Some(duration_ms),
            ttfb_ms: None,
            request_ip,
            reasoning_effort,
            tried_providers_json,
            request_kind: None,
        };
        if let Err(e) = user_store.finalize_request_log(log).await {
            tracing::warn!("failed to finalize request log: {e}");
        }
    });
}

fn spawn_request_log_error_no_attempt(
    state: &AppState,
    auth: &crate::auth::AuthResult,
    model: &str,
    is_stream: bool,
    started_at: std::time::Instant,
    request_id: Option<String>,
    request_ip: Option<String>,
    error: &AppError,
    reasoning_effort: Option<String>,
    tried_providers: Vec<TriedProvider>,
) {
    let Some(user_id) = auth.user_id.clone() else {
        return;
    };
    let api_key_id = auth.api_key_id.clone();
    let model = model.to_string();
    let duration_ms = started_at.elapsed().as_millis() as u64;
    let user_store = state.user_store.clone();
    let error_code = Some(error.code.clone());
    let error_message = Some(error.message.clone());
    let error_http_status = Some(error.status.as_u16());
    let tried_providers_json = if tried_providers.is_empty() {
        None
    } else {
        serde_json::to_value(&tried_providers).ok()
    };

    tokio::spawn(async move {
        let log = InsertRequestLog {
            request_id,
            user_id,
            api_key_id,
            model,
            provider_id: None,
            upstream_model: None,
            channel_id: None,
            is_stream,
            prompt_tokens: None,
            completion_tokens: None,
            cached_tokens: None,
            reasoning_tokens: None,
            provider_multiplier: None,
            charge_nano_usd: None,
            status: REQUEST_LOG_STATUS_ERROR.to_string(),
            usage_breakdown_json: None,
            billing_breakdown_json: None,
            error_code,
            error_message,
            error_http_status,
            duration_ms: Some(duration_ms),
            ttfb_ms: None,
            request_ip,
            reasoning_effort,
            tried_providers_json,
            request_kind: None,
        };
        if let Err(e) = user_store.finalize_request_log(log).await {
            tracing::warn!("failed to finalize request log: {e}");
        }
    });
}

async fn auth_tenant(headers: &HeaderMap, state: &AppState) -> AppResult<crate::auth::AuthResult> {
    let auth_header = headers
        .get(axum::http::header::AUTHORIZATION)
        .and_then(|value| value.to_str().ok())
        .ok_or_else(|| AppError::new(StatusCode::UNAUTHORIZED, "unauthorized", "missing auth"))?;
    let token = auth_header
        .strip_prefix("Bearer ")
        .ok_or_else(|| AppError::new(StatusCode::UNAUTHORIZED, "unauthorized", "invalid auth"))?;

    let auth_result = state
        .auth
        .authenticate_token(token, Some(&state.user_store))
        .await
        .ok_or_else(|| AppError::new(StatusCode::UNAUTHORIZED, "unauthorized", "invalid token"))?;
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

fn split_body(
    state: &AppState,
    value: Value,
    known_keys: &[&str],
) -> AppResult<(Value, Map<String, Value>)> {
    let policy = state.runtime.unknown_fields;
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

    if !extra.is_empty() {
        match policy {
            UnknownFieldPolicy::Reject => {
                return Err(AppError::new(
                    StatusCode::BAD_REQUEST,
                    "unknown_field",
                    "unknown fields present",
                ));
            }
            UnknownFieldPolicy::Ignore => extra.clear(),
            UnknownFieldPolicy::Preserve => {}
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

async fn forward_stream_typed(
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
    apply_transform_rules_request(&state, &mut req, &auth.transforms)?;
    resolve_model_suffix(&state, &mut req).await;
    let routing_stub = build_routing_stub(&req, max_multiplier);
    let attempts = build_monoize_attempts(&state, &routing_stub).await?;
    insert_pending_request_log(
        &state,
        &auth,
        &req.model,
        true,
        request_id.as_deref(),
        request_ip.as_deref(),
    )
    .await;

    for attempt in attempts {
        let mut req_attempt = req.clone();
        let logical_model = req.model.clone();
        req_attempt.model = attempt.upstream_model.clone();
        apply_transform_rules_request(&state, &mut req_attempt, &attempt.provider_transforms)?;
        let need_response_transform_stream =
            has_enabled_response_rules(&attempt.provider_transforms, &logical_model)
                || has_enabled_response_rules(&auth.transforms, &logical_model);

        if need_response_transform_stream {
            let mut nonstream_req = req_attempt.clone();
            nonstream_req.stream = Some(false);
            let upstream_body = encode_request_for_provider(&nonstream_req, &attempt)?;
            let provider = build_channel_provider_config(&attempt);
            let path = upstream_path_for_model(attempt.provider_type, &req_attempt.model, false);
            let call = upstream::call_upstream_with_timeout_and_headers(
                client_http(&state),
                &provider,
                &attempt.api_key,
                &path,
                &upstream_body,
                state.monoize_runtime.request_timeout_ms,
                provider_extra_headers(attempt.provider_type),
            )
            .await;
            match call {
                Ok(value) => {
                    mark_channel_success(&state, &attempt.channel_id).await;
                    let mut resp = decode_response_from_provider(attempt.provider_type, &value)?;
                    apply_transform_rules_response(
                        &state,
                        &mut resp,
                        &attempt.provider_transforms,
                        &logical_model,
                    )?;
                    apply_transform_rules_response(
                        &state,
                        &mut resp,
                        &auth.transforms,
                        &logical_model,
                    )?;
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
                            tx,
                        )
                        .await
                        {
                            let _ = tx_err.send(Event::default().data("[DONE]")).await;
                            tracing::warn!("synthetic stream failed: {}", err.message);
                        }
                    });
                    return Ok(tokio_stream::wrappers::ReceiverStream::new(rx).map(Ok));
                }
                Err(err) => {
                    let non_retryable = is_non_retryable_client_error(&err);
                    let retryable = is_retryable_error(&err);
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
                        mark_channel_retryable_failure(&state, &attempt.channel_id).await;
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
        let call = upstream::call_upstream_raw_with_timeout_and_headers(
            client_http(&state),
            &provider,
            &attempt.api_key,
            &path,
            &upstream_body,
            state.monoize_runtime.request_timeout_ms,
            provider_extra_headers(attempt.provider_type),
        )
        .await;
        match call {
            Ok(upstream_resp) => {
                mark_channel_success(&state, &attempt.channel_id).await;
                let legacy = typed_request_to_legacy(&req_attempt, max_multiplier)?;
                let provider_type = attempt.provider_type;
                let (tx, rx) = mpsc::channel::<Event>(64);
                let runtime_metrics = Arc::new(Mutex::new(StreamRuntimeMetrics::default()));
                let metrics_for_stream = runtime_metrics.clone();
                let state_for_log = state.clone();
                let auth_for_log = auth.clone();
                let attempt_for_log = attempt.clone();
                let model_for_log = logical_model.clone();
                let request_id_for_log = request_id.clone();
                let request_ip_for_log = request_ip.clone();
                let channel_id_for_log = attempt.channel_id.clone();
                let reasoning_effort_for_log =
                    req.reasoning.as_ref().and_then(|r| r.effort.clone());
                let tried_providers_for_log = tried_providers.clone();
                tokio::spawn(async move {
                    let tx_err = tx.clone();
                    let stream_result = match downstream {
                        DownstreamProtocol::Responses => match provider_type {
                            ProviderType::Responses => {
                                stream_responses_sse_as_responses(
                                    &legacy,
                                    upstream_resp,
                                    tx,
                                    Some(started_at),
                                    Some(metrics_for_stream.clone()),
                                )
                                .await
                            }
                            ProviderType::ChatCompletion => {
                                stream_chat_sse_as_responses(
                                    &legacy,
                                    upstream_resp,
                                    tx,
                                    Some(started_at),
                                    Some(metrics_for_stream.clone()),
                                )
                                .await
                            }
                            ProviderType::Messages => {
                                stream_messages_sse_as_responses(
                                    &legacy,
                                    upstream_resp,
                                    tx,
                                    Some(started_at),
                                    Some(metrics_for_stream.clone()),
                                )
                                .await
                            }
                            ProviderType::Gemini => {
                                stream_gemini_sse_as_responses(
                                    &legacy,
                                    upstream_resp,
                                    tx,
                                    Some(started_at),
                                    Some(metrics_for_stream.clone()),
                                )
                                .await
                            }
                            ProviderType::Grok => {
                                stream_responses_sse_as_responses(
                                    &legacy,
                                    upstream_resp,
                                    tx,
                                    Some(started_at),
                                    Some(metrics_for_stream.clone()),
                                )
                                .await
                            }
                            ProviderType::Group => Err(AppError::new(
                                StatusCode::BAD_REQUEST,
                                "provider_type_not_supported",
                                "group is virtual",
                            )),
                        },
                        DownstreamProtocol::ChatCompletions => match provider_type {
                            ProviderType::Gemini => {
                                stream_gemini_sse_as_chat(
                                    &legacy,
                                    upstream_resp,
                                    tx,
                                    Some(started_at),
                                    Some(metrics_for_stream.clone()),
                                )
                                .await
                            }
                            _ => {
                                stream_any_sse_as_chat(
                                    &legacy,
                                    provider_type,
                                    upstream_resp,
                                    tx,
                                    Some(started_at),
                                    Some(metrics_for_stream.clone()),
                                )
                                .await
                            }
                        },
                        DownstreamProtocol::AnthropicMessages => match provider_type {
                            ProviderType::Gemini => {
                                stream_gemini_sse_as_messages(
                                    &legacy,
                                    upstream_resp,
                                    tx,
                                    Some(started_at),
                                    Some(metrics_for_stream.clone()),
                                )
                                .await
                            }
                            _ => {
                                stream_any_sse_as_messages(
                                    &legacy,
                                    provider_type,
                                    upstream_resp,
                                    tx,
                                    Some(started_at),
                                    Some(metrics_for_stream.clone()),
                                )
                                .await
                            }
                        },
                    };

                    let (ttfb_ms, usage) = {
                        let guard = runtime_metrics.lock().await;
                        (guard.ttfb_ms, guard.usage.clone())
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
                                tracing::warn!(
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
                                    Event::default().event("error").data(error_json.to_string())
                                ).await;
                            }
                            DownstreamProtocol::ChatCompletions => {
                                let _ = tx_err.send(
                                    Event::default().data(error_json.to_string())
                                ).await;
                            }
                            DownstreamProtocol::AnthropicMessages => {
                                let _ = tx_err.send(
                                    Event::default().event("error").data(
                                        json!({"type": "error", "error": {"type": err.code, "message": err.message}}).to_string()
                                    )
                                ).await;
                            }
                        }
                        let _ = tx_err.send(Event::default().data("[DONE]")).await;
                    }
                });
                return Ok(tokio_stream::wrappers::ReceiverStream::new(rx).map(Ok));
            }
            Err(err) => {
                let non_retryable = is_non_retryable_client_error(&err);
                let retryable = is_retryable_error(&err);
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
                    mark_channel_retryable_failure(&state, &attempt.channel_id).await;
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
        format!("No available upstream provider for model: {}", req.model),
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

async fn emit_synthetic_stream_from_urp_response(
    downstream: DownstreamProtocol,
    logical_model: &str,
    resp: &urp::UrpResponse,
    tx: mpsc::Sender<Event>,
) -> AppResult<()> {
    match downstream {
        DownstreamProtocol::Responses => {
            emit_synthetic_responses_stream(logical_model, resp, tx).await
        }
        DownstreamProtocol::ChatCompletions => {
            emit_synthetic_chat_stream(logical_model, resp, tx).await
        }
        DownstreamProtocol::AnthropicMessages => {
            emit_synthetic_messages_stream(logical_model, resp, tx).await
        }
    }
}

async fn emit_synthetic_responses_stream(
    logical_model: &str,
    resp: &urp::UrpResponse,
    tx: mpsc::Sender<Event>,
) -> AppResult<()> {
    let mut seq = 1u64;
    let encoded = urp::encode::openai_responses::encode_response(resp, logical_model);
    let response_id = encoded
        .get("id")
        .and_then(|v| v.as_str())
        .unwrap_or("resp")
        .to_string();
    let created = encoded
        .get("created")
        .and_then(|v| v.as_i64())
        .unwrap_or_else(now_ts);
    let base_response = json!({
        "id": response_id,
        "object": "response",
        "created": created,
        "model": logical_model,
        "status": "in_progress",
        "output": []
    });
    tx.send(wrap_responses_event(
        &mut seq,
        "response.created",
        base_response.clone(),
    ))
    .await
    .map_err(|e| AppError::new(StatusCode::BAD_GATEWAY, "stream_send_failed", e.to_string()))?;
    tx.send(wrap_responses_event(
        &mut seq,
        "response.in_progress",
        base_response,
    ))
    .await
    .map_err(|e| AppError::new(StatusCode::BAD_GATEWAY, "stream_send_failed", e.to_string()))?;

    let mut text = String::new();
    for part in &resp.message.parts {
        match part {
            urp::Part::Reasoning { content, .. } => {
                if !content.is_empty() {
                    tx.send(wrap_responses_event(
                        &mut seq,
                        "response.reasoning_text.delta",
                        json!({ "delta": content }),
                    ))
                    .await
                    .map_err(|e| {
                        AppError::new(StatusCode::BAD_GATEWAY, "stream_send_failed", e.to_string())
                    })?;
                }
            }
            urp::Part::ReasoningEncrypted { data, .. } => {
                let sig = data
                    .as_str()
                    .map(|s| s.to_string())
                    .unwrap_or_else(|| data.to_string());
                if !sig.is_empty() {
                    tx.send(wrap_responses_event(
                        &mut seq,
                        "response.reasoning_signature.delta",
                        json!({ "delta": sig }),
                    ))
                    .await
                    .map_err(|e| {
                        AppError::new(StatusCode::BAD_GATEWAY, "stream_send_failed", e.to_string())
                    })?;
                }
            }
            urp::Part::Text { content, .. } | urp::Part::Refusal { content, .. } => {
                text.push_str(content);
            }
            urp::Part::ToolCall {
                call_id,
                name,
                arguments,
                ..
            } => {
                let item = json!({
                    "type": "function_call",
                    "call_id": call_id,
                    "name": name,
                    "arguments": arguments
                });
                tx.send(wrap_responses_event(
                    &mut seq,
                    "response.output_item.added",
                    item.clone(),
                ))
                .await
                .map_err(|e| {
                    AppError::new(StatusCode::BAD_GATEWAY, "stream_send_failed", e.to_string())
                })?;
                if !arguments.is_empty() {
                    tx.send(wrap_responses_event(
                        &mut seq,
                        "response.function_call_arguments.delta",
                        json!({ "call_id": call_id, "name": name, "delta": arguments }),
                    ))
                    .await
                    .map_err(|e| {
                        AppError::new(StatusCode::BAD_GATEWAY, "stream_send_failed", e.to_string())
                    })?;
                }
                tx.send(wrap_responses_event(
                    &mut seq,
                    "response.output_item.done",
                    item,
                ))
                .await
                .map_err(|e| {
                    AppError::new(StatusCode::BAD_GATEWAY, "stream_send_failed", e.to_string())
                })?;
            }
            _ => {}
        }
    }
    if !text.is_empty() {
        tx.send(wrap_responses_event(
            &mut seq,
            "response.output_text.delta",
            json!({ "text": text }),
        ))
        .await
        .map_err(|e| AppError::new(StatusCode::BAD_GATEWAY, "stream_send_failed", e.to_string()))?;
    }
    tx.send(wrap_responses_event(
        &mut seq,
        "response.output_text.done",
        json!({}),
    ))
    .await
    .map_err(|e| AppError::new(StatusCode::BAD_GATEWAY, "stream_send_failed", e.to_string()))?;
    tx.send(wrap_responses_event(
        &mut seq,
        "response.completed",
        encoded,
    ))
    .await
    .map_err(|e| AppError::new(StatusCode::BAD_GATEWAY, "stream_send_failed", e.to_string()))?;
    Ok(())
}

async fn emit_synthetic_chat_stream(
    logical_model: &str,
    resp: &urp::UrpResponse,
    tx: mpsc::Sender<Event>,
) -> AppResult<()> {
    let id = format!("chatcmpl_{}", uuid::Uuid::new_v4());
    let created = now_ts();
    let mut saw_tool = false;
    let mut tool_idx = 0usize;

    for part in &resp.message.parts {
        match part {
            urp::Part::Reasoning { content, .. } => {
                if !content.is_empty() {
                    let chunk = json!({
                        "id": id,
                        "object": "chat.completion.chunk",
                        "created": created,
                        "model": logical_model,
                        "choices": [{ "index": 0, "delta": chat_reasoning_delta_from_text(content), "finish_reason": Value::Null }]
                    });
                    tx.send(Event::default().data(chunk.to_string()))
                        .await
                        .map_err(|e| {
                            AppError::new(
                                StatusCode::BAD_GATEWAY,
                                "stream_send_failed",
                                e.to_string(),
                            )
                        })?;
                }
            }
            urp::Part::ReasoningEncrypted { data, .. } => {
                let sig = data
                    .as_str()
                    .map(|s| s.to_string())
                    .unwrap_or_else(|| data.to_string());
                if !sig.is_empty() {
                    let chunk = json!({
                        "id": id,
                        "object": "chat.completion.chunk",
                        "created": created,
                        "model": logical_model,
                        "choices": [{ "index": 0, "delta": chat_reasoning_delta_from_signature(&sig), "finish_reason": Value::Null }]
                    });
                    tx.send(Event::default().data(chunk.to_string()))
                        .await
                        .map_err(|e| {
                            AppError::new(
                                StatusCode::BAD_GATEWAY,
                                "stream_send_failed",
                                e.to_string(),
                            )
                        })?;
                }
            }
            urp::Part::ToolCall {
                call_id,
                name,
                arguments,
                ..
            } => {
                saw_tool = true;
                let chunk = json!({
                    "id": id,
                    "object": "chat.completion.chunk",
                    "created": created,
                    "model": logical_model,
                    "choices": [{
                        "index": 0,
                        "delta": {
                            "tool_calls": [{
                                "index": tool_idx,
                                "id": call_id,
                                "type": "function",
                                "function": { "name": name, "arguments": arguments }
                            }]
                        },
                        "finish_reason": Value::Null
                    }]
                });
                tool_idx += 1;
                tx.send(Event::default().data(chunk.to_string()))
                    .await
                    .map_err(|e| {
                        AppError::new(StatusCode::BAD_GATEWAY, "stream_send_failed", e.to_string())
                    })?;
            }
            urp::Part::Text { content, .. } | urp::Part::Refusal { content, .. } => {
                if !content.is_empty() {
                    let chunk = json!({
                        "id": id,
                        "object": "chat.completion.chunk",
                        "created": created,
                        "model": logical_model,
                        "choices": [{ "index": 0, "delta": { "content": content }, "finish_reason": Value::Null }]
                    });
                    tx.send(Event::default().data(chunk.to_string()))
                        .await
                        .map_err(|e| {
                            AppError::new(
                                StatusCode::BAD_GATEWAY,
                                "stream_send_failed",
                                e.to_string(),
                            )
                        })?;
                }
            }
            _ => {}
        }
    }

    let finish_reason = if saw_tool {
        "tool_calls"
    } else {
        finish_reason_to_chat(resp.finish_reason.unwrap_or(urp::FinishReason::Stop))
    };
    let done = json!({
        "id": id,
        "object": "chat.completion.chunk",
        "created": created,
        "model": logical_model,
        "choices": [{ "index": 0, "delta": {}, "finish_reason": finish_reason }]
    });
    tx.send(Event::default().data(done.to_string()))
        .await
        .map_err(|e| AppError::new(StatusCode::BAD_GATEWAY, "stream_send_failed", e.to_string()))?;
    tx.send(Event::default().data("[DONE]"))
        .await
        .map_err(|e| AppError::new(StatusCode::BAD_GATEWAY, "stream_send_failed", e.to_string()))?;
    Ok(())
}

fn finish_reason_to_chat(reason: urp::FinishReason) -> &'static str {
    match reason {
        urp::FinishReason::Stop => "stop",
        urp::FinishReason::Length => "length",
        urp::FinishReason::ToolCalls => "tool_calls",
        urp::FinishReason::ContentFilter => "content_filter",
        urp::FinishReason::Other => "stop",
    }
}

async fn emit_synthetic_messages_stream(
    logical_model: &str,
    resp: &urp::UrpResponse,
    tx: mpsc::Sender<Event>,
) -> AppResult<()> {
    let message_id = format!("msg_{}", uuid::Uuid::new_v4());
    let mut saw_tool_use = false;
    let usage = resp.usage.clone().unwrap_or(urp::Usage {
        prompt_tokens: 0,
        completion_tokens: 0,
        reasoning_tokens: None,
        cached_tokens: None,
        extra_body: HashMap::new(),
    });
    let start = json!({
        "type": "message_start",
        "message": {
            "id": message_id,
            "type": "message",
            "role": "assistant",
            "model": logical_model,
            "content": [],
            "stop_reason": Value::Null,
            "stop_sequence": Value::Null,
            "usage": {
                "input_tokens": usage.prompt_tokens,
                "output_tokens": usage.completion_tokens
            }
        }
    });
    tx.send(Event::default().data(start.to_string()))
        .await
        .map_err(|e| AppError::new(StatusCode::BAD_GATEWAY, "stream_send_failed", e.to_string()))?;

    let mut index = 0u32;
    for part in &resp.message.parts {
        match part {
            urp::Part::Reasoning { content, .. } => {
                let s = json!({ "type": "content_block_start", "index": index, "content_block": { "type": "thinking", "thinking": "", "signature": "" } });
                tx.send(Event::default().data(s.to_string()))
                    .await
                    .map_err(|e| {
                        AppError::new(StatusCode::BAD_GATEWAY, "stream_send_failed", e.to_string())
                    })?;
                if !content.is_empty() {
                    let d = json!({ "type": "content_block_delta", "index": index, "delta": { "type": "thinking_delta", "thinking": content } });
                    tx.send(Event::default().data(d.to_string()))
                        .await
                        .map_err(|e| {
                            AppError::new(
                                StatusCode::BAD_GATEWAY,
                                "stream_send_failed",
                                e.to_string(),
                            )
                        })?;
                }
                let e = json!({ "type": "content_block_stop", "index": index });
                tx.send(Event::default().data(e.to_string()))
                    .await
                    .map_err(|er| {
                        AppError::new(
                            StatusCode::BAD_GATEWAY,
                            "stream_send_failed",
                            er.to_string(),
                        )
                    })?;
                index += 1;
            }
            urp::Part::ReasoningEncrypted { data, .. } => {
                let sig = data
                    .as_str()
                    .map(|s| s.to_string())
                    .unwrap_or_else(|| data.to_string());
                if !sig.is_empty() {
                    let s = json!({ "type": "content_block_start", "index": index, "content_block": { "type": "thinking", "thinking": "", "signature": "" } });
                    tx.send(Event::default().data(s.to_string()))
                        .await
                        .map_err(|e| {
                            AppError::new(
                                StatusCode::BAD_GATEWAY,
                                "stream_send_failed",
                                e.to_string(),
                            )
                        })?;
                    let d = json!({ "type": "content_block_delta", "index": index, "delta": { "type": "signature_delta", "signature": sig } });
                    tx.send(Event::default().data(d.to_string()))
                        .await
                        .map_err(|e| {
                            AppError::new(
                                StatusCode::BAD_GATEWAY,
                                "stream_send_failed",
                                e.to_string(),
                            )
                        })?;
                    let e = json!({ "type": "content_block_stop", "index": index });
                    tx.send(Event::default().data(e.to_string()))
                        .await
                        .map_err(|er| {
                            AppError::new(
                                StatusCode::BAD_GATEWAY,
                                "stream_send_failed",
                                er.to_string(),
                            )
                        })?;
                    index += 1;
                }
            }
            urp::Part::ToolCall {
                call_id,
                name,
                arguments,
                ..
            } => {
                saw_tool_use = true;
                let start_tool = json!({
                    "type": "content_block_start",
                    "index": index,
                    "content_block": { "type": "tool_use", "id": call_id, "name": name, "input": {} }
                });
                tx.send(Event::default().data(start_tool.to_string()))
                    .await
                    .map_err(|e| {
                        AppError::new(StatusCode::BAD_GATEWAY, "stream_send_failed", e.to_string())
                    })?;
                if !arguments.is_empty() {
                    let d = json!({
                        "type": "content_block_delta",
                        "index": index,
                        "delta": { "type": "input_json_delta", "partial_json": arguments }
                    });
                    tx.send(Event::default().data(d.to_string()))
                        .await
                        .map_err(|e| {
                            AppError::new(
                                StatusCode::BAD_GATEWAY,
                                "stream_send_failed",
                                e.to_string(),
                            )
                        })?;
                }
                let stop_tool = json!({ "type": "content_block_stop", "index": index });
                tx.send(Event::default().data(stop_tool.to_string()))
                    .await
                    .map_err(|e| {
                        AppError::new(StatusCode::BAD_GATEWAY, "stream_send_failed", e.to_string())
                    })?;
                index += 1;
            }
            urp::Part::Text { content, .. } | urp::Part::Refusal { content, .. } => {
                if content.is_empty() {
                    continue;
                }
                let s = json!({ "type": "content_block_start", "index": index, "content_block": { "type": "text", "text": "" } });
                tx.send(Event::default().data(s.to_string()))
                    .await
                    .map_err(|e| {
                        AppError::new(StatusCode::BAD_GATEWAY, "stream_send_failed", e.to_string())
                    })?;
                let d = json!({ "type": "content_block_delta", "index": index, "delta": { "type": "text_delta", "text": content } });
                tx.send(Event::default().data(d.to_string()))
                    .await
                    .map_err(|e| {
                        AppError::new(StatusCode::BAD_GATEWAY, "stream_send_failed", e.to_string())
                    })?;
                let e = json!({ "type": "content_block_stop", "index": index });
                tx.send(Event::default().data(e.to_string()))
                    .await
                    .map_err(|er| {
                        AppError::new(
                            StatusCode::BAD_GATEWAY,
                            "stream_send_failed",
                            er.to_string(),
                        )
                    })?;
                index += 1;
            }
            _ => {}
        }
    }

    let message_delta = json!({
        "type": "message_delta",
        "delta": {
            "stop_reason": if saw_tool_use { "tool_use" } else { "end_turn" },
            "stop_sequence": Value::Null
        },
        "usage": {
            "input_tokens": usage.prompt_tokens,
            "output_tokens": usage.completion_tokens
        }
    });
    tx.send(Event::default().data(message_delta.to_string()))
        .await
        .map_err(|e| AppError::new(StatusCode::BAD_GATEWAY, "stream_send_failed", e.to_string()))?;

    tx.send(Event::default().data(json!({ "type": "message_stop" }).to_string()))
        .await
        .map_err(|e| AppError::new(StatusCode::BAD_GATEWAY, "stream_send_failed", e.to_string()))?;
    Ok(())
}

fn extract_reasoning_text_and_signature(item: &Value) -> (String, String) {
    let mut text = item
        .get("text")
        .and_then(|v| v.as_str())
        .unwrap_or("")
        .to_string();
    if text.is_empty() {
        if let Some(summary) = item.get("summary").and_then(|v| v.as_array()) {
            let mut parts = Vec::new();
            for s in summary {
                if s.get("type").and_then(|v| v.as_str()) == Some("summary_text") {
                    if let Some(t) = s.get("text").and_then(|v| v.as_str()) {
                        if !t.is_empty() {
                            parts.push(t);
                        }
                    }
                }
            }
            if !parts.is_empty() {
                text = parts.join("\n");
            }
        }
    }
    let mut signature = item
        .get("signature")
        .and_then(|v| v.as_str())
        .unwrap_or("")
        .to_string();
    if signature.is_empty() {
        signature = item
            .get("id")
            .and_then(|v| v.as_str())
            .unwrap_or("")
            .to_string();
    }
    (text, signature)
}

fn reasoning_text_detail_value(text: &str, signature: Option<&str>) -> Value {
    json!({
        "type": "reasoning.text",
        "text": text,
        "signature": signature,
        "format": "unknown"
    })
}

fn reasoning_encrypted_detail_value(data: Value) -> Value {
    json!({
        "type": "reasoning.encrypted",
        "data": data,
        "format": "unknown"
    })
}

fn extract_chat_reasoning_from_detail(
    detail: &Value,
    text_out: &mut Vec<String>,
    sig_out: &mut Vec<String>,
) {
    let Some(obj) = detail.as_object() else {
        return;
    };
    match obj.get("type").and_then(|v| v.as_str()).unwrap_or("") {
        "reasoning.text" => {
            if let Some(t) = obj.get("text").and_then(|v| v.as_str()) {
                if !t.is_empty() {
                    text_out.push(t.to_string());
                }
            }
            if let Some(sig) = obj.get("signature").and_then(|v| v.as_str()) {
                if !sig.is_empty() {
                    sig_out.push(sig.to_string());
                }
            }
        }
        "reasoning.encrypted" => {
            if let Some(data) = obj.get("data") {
                match data {
                    Value::String(s) if !s.is_empty() => sig_out.push(s.clone()),
                    Value::String(_) | Value::Null => {}
                    other => sig_out.push(other.to_string()),
                }
            }
        }
        "reasoning.summary" => {
            if let Some(summary) = obj.get("summary").and_then(|v| v.as_str()) {
                if !summary.is_empty() {
                    text_out.push(summary.to_string());
                }
            }
        }
        _ => {}
    }
}

fn extract_chat_reasoning_deltas(delta: &Value) -> (Vec<String>, Vec<String>) {
    let mut text_parts = Vec::new();
    let mut sig_parts = Vec::new();

    if let Some(details) = delta.get("reasoning_details").and_then(|v| v.as_array()) {
        for detail in details {
            extract_chat_reasoning_from_detail(detail, &mut text_parts, &mut sig_parts);
        }
    }

    if text_parts.is_empty() {
        if let Some(reasoning) = delta.get("reasoning").and_then(|v| v.as_str()) {
            if !reasoning.is_empty() {
                text_parts.push(reasoning.to_string());
            }
        }
    }

    if let Some(reasoning) = delta.get("reasoning_content").and_then(|v| v.as_str()) {
        if !reasoning.is_empty() {
            text_parts.push(reasoning.to_string());
        }
    }
    if let Some(sig) = delta.get("reasoning_opaque").and_then(|v| v.as_str()) {
        if !sig.is_empty() {
            sig_parts.push(sig.to_string());
        }
    }

    (text_parts, sig_parts)
}

fn chat_reasoning_delta_from_text(text: &str) -> Value {
    json!({
        "reasoning_details": [reasoning_text_detail_value(text, None)]
    })
}

fn chat_reasoning_delta_from_signature(signature: &str) -> Value {
    json!({
        "reasoning_details": [reasoning_encrypted_detail_value(Value::String(signature.to_string()))]
    })
}

fn normalize_chat_reasoning_delta_object(delta: &mut Map<String, Value>) {
    if delta
        .get("reasoning_details")
        .and_then(|v| v.as_array())
        .is_none()
    {
        let mut details = Vec::new();
        if let Some(text) = delta.get("reasoning_content").and_then(|v| v.as_str()) {
            if !text.is_empty() {
                details.push(reasoning_text_detail_value(text, None));
            }
        }
        if let Some(sig) = delta.get("reasoning_opaque").and_then(|v| v.as_str()) {
            if !sig.is_empty() {
                details.push(reasoning_encrypted_detail_value(Value::String(
                    sig.to_string(),
                )));
            }
        }
        if !details.is_empty() {
            delta.insert("reasoning_details".to_string(), Value::Array(details));
        }
    }
    delta.remove("reasoning_content");
    delta.remove("reasoning_opaque");
}

fn extract_responses_message_text(item: &Value) -> String {
    let mut out = String::new();
    if item.get("type").and_then(|v| v.as_str()) != Some("message") {
        return out;
    }
    if let Some(content) = item.get("content").and_then(|v| v.as_array()) {
        for part in content {
            if part.get("type").and_then(|v| v.as_str()) == Some("output_text") {
                if let Some(text) = part.get("text").and_then(|v| v.as_str()) {
                    out.push_str(text);
                }
            }
        }
    }
    out
}

fn now_ts() -> i64 {
    chrono::Utc::now().timestamp()
}

fn client_http(state: &AppState) -> &reqwest::Client {
    &state.http
}

fn upstream_path(provider_type: ProviderType) -> &'static str {
    match provider_type {
        ProviderType::Responses => "/v1/responses",
        ProviderType::ChatCompletion => "/v1/chat/completions",
        ProviderType::Messages => "/v1/messages",
        ProviderType::Gemini => "/v1beta/models",
        ProviderType::Grok => "/v1/responses",
        ProviderType::Group => "/v1/responses",
    }
}

fn upstream_path_for_model(provider_type: ProviderType, model: &str, stream: bool) -> String {
    match provider_type {
        ProviderType::Gemini => {
            let model = model.trim();
            if stream {
                format!("/v1beta/models/{model}:streamGenerateContent?alt=sse")
            } else {
                format!("/v1beta/models/{model}:generateContent")
            }
        }
        _ => upstream_path(provider_type).to_string(),
    }
}

const BUILTIN_EFFORT_SUFFIXES: &[(&str, &str)] = &[
    ("-none", "none"),
    ("-minimum", "minimum"),
    ("-low", "low"),
    ("-medium", "medium"),
    ("-high", "high"),
    ("-xhigh", "xhigh"),
    ("-max", "xhigh"),
];

async fn resolve_model_suffix(state: &AppState, req: &mut urp::UrpRequest) {
    let providers = match state.monoize_store.list_providers().await {
        Ok(p) => p,
        Err(_) => return,
    };

    let model_exists = |model: &str| -> bool {
        providers
            .iter()
            .any(|p| p.enabled && p.models.contains_key(model))
    };
    if model_exists(&req.model) {
        return;
    }

    let settings_map = state
        .settings_store
        .get_reasoning_suffix_map()
        .await
        .unwrap_or_default();

    // Sort by suffix length descending so longer suffixes match first
    // (e.g. "-nothinking" before "-thinking").
    let mut settings_entries: Vec<(&str, &str)> = settings_map
        .iter()
        .map(|(k, v)| (k.as_str(), v.as_str()))
        .collect();
    settings_entries.sort_by(|a, b| b.0.len().cmp(&a.0.len()));

    for (suffix, effort) in settings_entries
        .iter()
        .chain(BUILTIN_EFFORT_SUFFIXES.iter())
    {
        if let Some(base) = req.model.strip_suffix(suffix) {
            if !base.is_empty() && model_exists(base) {
                req.model = base.to_string();
                match req.reasoning.as_mut() {
                    Some(r) => {
                        r.effort = Some(effort.to_string());
                    }
                    None => {
                        req.reasoning = Some(urp::ReasoningConfig {
                            effort: Some(effort.to_string()),
                            extra_body: std::collections::HashMap::new(),
                        });
                    }
                }
                return;
            }
        }
    }
}

async fn build_monoize_attempts(
    state: &AppState,
    urp: &UrpRequest,
) -> AppResult<Vec<MonoizeAttempt>> {
    let providers =
        state.monoize_store.list_providers().await.map_err(|e| {
            AppError::new(StatusCode::INTERNAL_SERVER_ERROR, "provider_store_error", e)
        })?;
    let mut attempts = Vec::new();
    for provider in providers {
        collect_provider_attempts(state, urp, &provider, &mut attempts).await;
    }
    Ok(attempts)
}

async fn collect_provider_attempts(
    state: &AppState,
    urp: &UrpRequest,
    provider: &crate::monoize_routing::MonoizeProvider,
    out: &mut Vec<MonoizeAttempt>,
) {
    if !provider.enabled {
        return;
    }
    let Some(model_entry) = provider.models.get(&urp.model) else {
        return;
    };
    if let Some(max_multiplier) = urp.max_multiplier {
        if model_entry.multiplier > max_multiplier {
            return;
        }
    }
    let channels = filter_eligible_channels(state, &provider.channels).await;
    if channels.is_empty() {
        return;
    }

    let ordered = weighted_shuffle_channels(channels);
    let max_attempts = if provider.max_retries == -1 {
        ordered.len()
    } else {
        let retries = provider.max_retries.max(0) as usize;
        (retries + 1).min(ordered.len())
    };
    let upstream_model = resolve_upstream_model(&urp.model, model_entry);

    for channel in ordered.into_iter().take(max_attempts) {
        out.push(MonoizeAttempt {
            provider_id: provider.id.clone(),
            provider_type: provider.provider_type.to_config_type(),
            channel_id: channel.id.clone(),
            base_url: channel.base_url.clone(),
            api_key: channel.api_key.clone(),
            upstream_model: upstream_model.clone(),
            model_multiplier: model_entry.multiplier,
            provider_transforms: provider.transforms.clone(),
        });
    }
}

fn resolve_upstream_model(
    requested_model: &str,
    model_entry: &crate::monoize_routing::MonoizeModelEntry,
) -> String {
    model_entry
        .redirect
        .as_deref()
        .map(str::trim)
        .filter(|v| !v.is_empty())
        .map(|v| v.to_string())
        .unwrap_or_else(|| requested_model.to_string())
}

async fn filter_eligible_channels(
    state: &AppState,
    channels: &[crate::monoize_routing::MonoizeChannel],
) -> Vec<crate::monoize_routing::MonoizeChannel> {
    let now = now_ts();
    let health = state.channel_health.lock().await;
    let mut out = Vec::new();
    for channel in channels {
        if !channel.enabled || channel.weight <= 0 {
            continue;
        }
        let channel_health = health
            .get(&channel.id)
            .cloned()
            .unwrap_or_else(crate::monoize_routing::ChannelHealthState::new);
        let is_candidate = if channel_health.healthy {
            true
        } else {
            channel_health
                .cooldown_until
                .map(|until| now >= until)
                .unwrap_or(true)
        };
        if is_candidate {
            out.push(channel.clone());
        }
    }
    out
}

fn weighted_shuffle_channels(
    mut channels: Vec<crate::monoize_routing::MonoizeChannel>,
) -> Vec<crate::monoize_routing::MonoizeChannel> {
    let mut ordered = Vec::with_capacity(channels.len());
    while !channels.is_empty() {
        let total_weight: u64 = channels.iter().map(|c| c.weight.max(1) as u64).sum();
        if total_weight == 0 {
            ordered.append(&mut channels);
            break;
        }
        let target = random_u64(total_weight);
        let mut cumulative = 0u64;
        let mut chosen = 0usize;
        for (idx, channel) in channels.iter().enumerate() {
            cumulative += channel.weight.max(1) as u64;
            if target < cumulative {
                chosen = idx;
                break;
            }
        }
        ordered.push(channels.swap_remove(chosen));
    }
    ordered
}

fn random_u64(bound: u64) -> u64 {
    if bound <= 1 {
        return 0;
    }
    let seed = uuid::Uuid::new_v4().as_u128() as u64;
    seed % bound
}

fn build_channel_provider_config(attempt: &MonoizeAttempt) -> ProviderConfig {
    let (auth_type, header_name, query_name) = match attempt.provider_type {
        ProviderType::Gemini => (
            ProviderAuthType::Header,
            Some("x-goog-api-key".to_string()),
            None,
        ),
        _ => (ProviderAuthType::Bearer, None, None),
    };
    ProviderConfig {
        id: format!("{}_{}", attempt.provider_id, attempt.channel_id),
        provider_type: attempt.provider_type,
        base_url: Some(attempt.base_url.clone()),
        auth: Some(ProviderAuthConfig {
            auth_type,
            value: String::new(),
            header_name,
            query_name,
        }),
        model_map: Vec::new(),
        strategy: None,
        members: Vec::new(),
    }
}

fn provider_extra_headers(provider_type: ProviderType) -> &'static [(&'static str, &'static str)] {
    match provider_type {
        ProviderType::Messages => &[("anthropic-version", "2023-06-01")],
        _ => &[],
    }
}

fn is_non_retryable_client_error(err: &UpstreamCallError) -> bool {
    matches!(
        err.status,
        Some(StatusCode::BAD_REQUEST)
            | Some(StatusCode::UNAUTHORIZED)
            | Some(StatusCode::FORBIDDEN)
            | Some(StatusCode::UNPROCESSABLE_ENTITY)
    )
}

fn is_retryable_error(err: &UpstreamCallError) -> bool {
    if matches!(err.kind, UpstreamErrorKind::Network) {
        return true;
    }
    matches!(
        err.status,
        Some(StatusCode::TOO_MANY_REQUESTS)
            | Some(StatusCode::INTERNAL_SERVER_ERROR)
            | Some(StatusCode::BAD_GATEWAY)
            | Some(StatusCode::SERVICE_UNAVAILABLE)
            | Some(StatusCode::GATEWAY_TIMEOUT)
    )
}

async fn mark_channel_success(state: &AppState, channel_id: &str) {
    let now = now_ts();
    let mut health = state.channel_health.lock().await;
    let entry = health
        .entry(channel_id.to_string())
        .or_insert_with(crate::monoize_routing::ChannelHealthState::new);
    let was_unhealthy = !entry.healthy;
    entry.healthy = true;
    entry.failure_count = 0;
    entry.cooldown_until = None;
    entry.last_success_at = Some(now);
    entry.probe_success_count = 0;
    entry.last_probe_at = None;
    if was_unhealthy {
        tracing::info!(channel_id = %channel_id, "channel recovered to healthy after success");
    }
}

async fn mark_channel_retryable_failure(state: &AppState, channel_id: &str) {
    let now = now_ts();
    let mut health = state.channel_health.lock().await;
    let entry = health
        .entry(channel_id.to_string())
        .or_insert_with(crate::monoize_routing::ChannelHealthState::new);
    entry.failure_count = entry.failure_count.saturating_add(1);
    if entry.failure_count >= state.monoize_runtime.passive_failure_threshold {
        entry.healthy = false;
        entry.cooldown_until = Some(now + state.monoize_runtime.passive_cooldown_seconds as i64);
        entry.probe_success_count = 0;
        entry.last_probe_at = None;
        tracing::info!(
            channel_id = %channel_id,
            failure_count = entry.failure_count,
            cooldown_seconds = state.monoize_runtime.passive_cooldown_seconds,
            "channel marked unhealthy after consecutive failures"
        );
    }
}
fn upstream_error_to_app(err: UpstreamCallError) -> AppError {
    let status = err.status.unwrap_or(StatusCode::BAD_GATEWAY);
    AppError::new(status, "upstream_error", err.message)
}

fn error_to_sse_stream(
    err: &AppError,
    downstream: DownstreamProtocol,
) -> impl futures_util::Stream<Item = Result<Event, std::convert::Infallible>> + Send + 'static {
    let error_json = json!({
        "error": {
            "message": err.message,
            "type": err.error_type,
            "code": err.code,
            "param": err.param,
        }
    });
    let mut events: Vec<Event> = Vec::new();
    match downstream {
        DownstreamProtocol::Responses => {
            let mut seq: u64 = 0;
            let payload = json!({ "sequence_number": seq, "data": error_json });
            seq += 1;
            events.push(Event::default().event("error").data(payload.to_string()));
            let _ = seq;
        }
        DownstreamProtocol::ChatCompletions => {
            events.push(Event::default().data(error_json.to_string()));
        }
        DownstreamProtocol::AnthropicMessages => {
            events.push(
                Event::default()
                    .event("error")
                    .data(json!({"type": "error", "error": {"type": err.code, "message": err.message}}).to_string()),
            );
        }
    }
    events.push(Event::default().data("[DONE]"));
    futures_util::stream::iter(events.into_iter().map(Ok))
}

fn wrap_responses_event(seq: &mut u64, name: &str, data: Value) -> Event {
    let payload = json!({ "sequence_number": *seq, "data": data });
    *seq += 1;
    Event::default().event(name).data(payload.to_string())
}

async fn ensure_anthropic_text_block(
    tx: &mpsc::Sender<Event>,
    text_index: &mut Option<u32>,
    next_index: &mut u32,
    started: &mut Vec<u32>,
) -> AppResult<u32> {
    if let Some(i) = *text_index {
        return Ok(i);
    }
    let i = *next_index;
    *next_index += 1;
    *text_index = Some(i);
    started.push(i);
    let block_start = json!({
        "type": "content_block_start",
        "index": i,
        "content_block": { "type": "text", "text": "" }
    });
    tx.send(Event::default().data(block_start.to_string()))
        .await
        .map_err(|e| AppError::new(StatusCode::BAD_GATEWAY, "stream_send_failed", e.to_string()))?;
    Ok(i)
}

async fn ensure_anthropic_thinking_block(
    tx: &mpsc::Sender<Event>,
    thinking_index: &mut Option<u32>,
    next_index: &mut u32,
    started: &mut Vec<u32>,
) -> AppResult<u32> {
    if let Some(i) = *thinking_index {
        return Ok(i);
    }
    let i = *next_index;
    *next_index += 1;
    *thinking_index = Some(i);
    started.push(i);
    let block_start = json!({
        "type": "content_block_start",
        "index": i,
        "content_block": { "type": "thinking", "thinking": "", "signature": "" }
    });
    tx.send(Event::default().data(block_start.to_string()))
        .await
        .map_err(|e| AppError::new(StatusCode::BAD_GATEWAY, "stream_send_failed", e.to_string()))?;
    Ok(i)
}

async fn ensure_anthropic_tool_block(
    tx: &mpsc::Sender<Event>,
    tool_indices: &mut HashMap<String, u32>,
    tool_names: &mut HashMap<String, String>,
    next_index: &mut u32,
    started: &mut Vec<u32>,
    call_id: &str,
    name: &str,
) -> AppResult<u32> {
    if let Some(i) = tool_indices.get(call_id).copied() {
        if !name.is_empty() && !tool_names.contains_key(call_id) {
            tool_names.insert(call_id.to_string(), name.to_string());
        }
        return Ok(i);
    }
    let i = *next_index;
    *next_index += 1;
    tool_indices.insert(call_id.to_string(), i);
    if !name.is_empty() {
        tool_names.insert(call_id.to_string(), name.to_string());
    }
    started.push(i);
    let block_start = json!({
        "type": "content_block_start",
        "index": i,
        "content_block": { "type": "tool_use", "id": call_id, "name": name, "input": {} }
    });
    tx.send(Event::default().data(block_start.to_string()))
        .await
        .map_err(|e| AppError::new(StatusCode::BAD_GATEWAY, "stream_send_failed", e.to_string()))?;
    Ok(i)
}

async fn stream_responses_sse_as_responses(
    urp: &UrpRequest,
    upstream_resp: reqwest::Response,
    tx: mpsc::Sender<Event>,
    started_at: Option<std::time::Instant>,
    runtime_metrics: Option<Arc<Mutex<StreamRuntimeMetrics>>>,
) -> AppResult<()> {
    let mut seq = 1u64;
    let response_id = format!("resp_{}", uuid::Uuid::new_v4());
    let created = now_ts();
    let mut output_text = String::new();
    let mut reasoning_text = String::new();
    let mut reasoning_sig = String::new();
    let mut call_order: Vec<String> = Vec::new();
    let mut calls: HashMap<String, (String, String)> = HashMap::new(); // call_id -> (name, arguments)
    let mut call_ids_by_output_index: HashMap<u64, String> = HashMap::new();
    let mut saw_text_delta = false;

    let base_response = json!({
        "id": response_id,
        "object": "response",
        "created": created,
        "model": urp.model,
        "status": "in_progress",
        "output": []
    });
    let _ = tx
        .send(wrap_responses_event(
            &mut seq,
            "response.created",
            base_response.clone(),
        ))
        .await;
    let _ = tx
        .send(wrap_responses_event(
            &mut seq,
            "response.in_progress",
            base_response.clone(),
        ))
        .await;

    let mut stream = upstream_resp.bytes_stream().eventsource();
    while let Some(ev) = stream.next().await {
        let Ok(ev) = ev else { continue };
        mark_stream_ttfb_if_needed(started_at, &runtime_metrics).await;
        if ev.data.trim() == "[DONE]" {
            break;
        }
        // For responses upstream, we forward event names and data into our wrapper.
        let data_val: Value = serde_json::from_str(&ev.data).unwrap_or(Value::String(ev.data));
        record_stream_usage_if_present(
            &runtime_metrics,
            parse_usage_from_responses_object(&data_val),
        )
        .await;
        // Try to extract text deltas for final output.
        if ev.event == "response.output_text.delta" {
            if let Some(text) = data_val
                .get("text")
                .and_then(|v| v.as_str())
                .or_else(|| data_val.get("delta").and_then(|v| v.as_str()))
            {
                output_text.push_str(text);
                saw_text_delta = true;
            }
        }
        if ev.event == "response.reasoning_text.delta" {
            if let Some(delta) = data_val
                .get("delta")
                .and_then(|v| v.as_str())
                .or_else(|| data_val.get("text").and_then(|v| v.as_str()))
            {
                reasoning_text.push_str(delta);
            }
        }
        if ev.event == "response.reasoning_signature.delta" {
            if let Some(delta) = data_val.get("delta").and_then(|v| v.as_str()) {
                reasoning_sig.push_str(delta);
            }
        }
        if ev.event == "response.output_item.added" {
            let item = data_val.get("item").unwrap_or(&data_val);
            if item.get("type").and_then(|v| v.as_str()) == Some("function_call") {
                if let Some(call_id) = item.get("call_id").and_then(|v| v.as_str()) {
                    if !calls.contains_key(call_id) {
                        call_order.push(call_id.to_string());
                        calls.insert(
                            call_id.to_string(),
                            (
                                item.get("name")
                                    .and_then(|v| v.as_str())
                                    .unwrap_or("")
                                    .to_string(),
                                item.get("arguments")
                                    .and_then(|v| v.as_str())
                                    .unwrap_or("")
                                    .to_string(),
                            ),
                        );
                    }
                    if let Some(idx) = data_val.get("output_index").and_then(|v| v.as_u64()) {
                        call_ids_by_output_index.insert(idx, call_id.to_string());
                    }
                }
            } else if item.get("type").and_then(|v| v.as_str()) == Some("reasoning") {
                let (text, sig) = extract_reasoning_text_and_signature(item);
                if reasoning_text.is_empty() && !text.is_empty() {
                    reasoning_text = text;
                }
                if reasoning_sig.is_empty() && !sig.is_empty() {
                    reasoning_sig = sig;
                }
            }
        }
        if ev.event == "response.function_call_arguments.delta" {
            let call_id_opt = data_val
                .get("call_id")
                .and_then(|v| v.as_str())
                .map(|s| s.to_string())
                .or_else(|| {
                    data_val
                        .get("output_index")
                        .and_then(|v| v.as_u64())
                        .and_then(|idx| call_ids_by_output_index.get(&idx).cloned())
                });
            if let Some(call_id) = call_id_opt {
                let name = data_val.get("name").and_then(|v| v.as_str()).unwrap_or("");
                let delta = data_val.get("delta").and_then(|v| v.as_str()).unwrap_or("");
                if !calls.contains_key(call_id.as_str()) {
                    call_order.push(call_id.clone());
                    calls.insert(call_id.clone(), (name.to_string(), String::new()));
                }
                if let Some(entry) = calls.get_mut(call_id.as_str()) {
                    if entry.0.is_empty() && !name.is_empty() {
                        entry.0 = name.to_string();
                    }
                    entry.1.push_str(delta);
                }
            }
        }
        if ev.event == "response.function_call_arguments.done" {
            let call_id_opt = data_val
                .get("call_id")
                .and_then(|v| v.as_str())
                .map(|s| s.to_string())
                .or_else(|| {
                    data_val
                        .get("output_index")
                        .and_then(|v| v.as_u64())
                        .and_then(|idx| call_ids_by_output_index.get(&idx).cloned())
                });
            if let Some(call_id) = call_id_opt {
                let args = data_val
                    .get("arguments")
                    .and_then(|v| v.as_str())
                    .unwrap_or("");
                if let Some(entry) = calls.get_mut(call_id.as_str()) {
                    if entry.1.is_empty() && !args.is_empty() {
                        entry.1 = args.to_string();
                    }
                }
            }
        }
        if ev.event == "response.output_item.done" {
            let item = data_val.get("item").unwrap_or(&data_val);
            if item.get("type").and_then(|v| v.as_str()) == Some("function_call") {
                if let Some(call_id) = item.get("call_id").and_then(|v| v.as_str()) {
                    let name = item.get("name").and_then(|v| v.as_str()).unwrap_or("");
                    let args = item.get("arguments").and_then(|v| v.as_str()).unwrap_or("");
                    if !calls.contains_key(call_id) {
                        call_order.push(call_id.to_string());
                        calls.insert(call_id.to_string(), (name.to_string(), args.to_string()));
                    } else if let Some(entry) = calls.get_mut(call_id) {
                        if entry.0.is_empty() && !name.is_empty() {
                            entry.0 = name.to_string();
                        }
                        if entry.1.is_empty() && !args.is_empty() {
                            entry.1 = args.to_string();
                        }
                    }
                    if let Some(idx) = data_val.get("output_index").and_then(|v| v.as_u64()) {
                        call_ids_by_output_index.insert(idx, call_id.to_string());
                    }
                }
            } else if item.get("type").and_then(|v| v.as_str()) == Some("reasoning") {
                let (text, sig) = extract_reasoning_text_and_signature(item);
                if reasoning_text.is_empty() && !text.is_empty() {
                    reasoning_text = text;
                }
                if reasoning_sig.is_empty() && !sig.is_empty() {
                    reasoning_sig = sig;
                }
            } else if item.get("type").and_then(|v| v.as_str()) == Some("message") {
                if !saw_text_delta {
                    output_text.push_str(&extract_responses_message_text(item));
                }
            }
        }
        let name = if ev.event.is_empty() {
            "message"
        } else {
            ev.event.as_str()
        };
        let _ = tx
            .send(wrap_responses_event(&mut seq, name, data_val))
            .await;
    }

    // Minimal completion sequence.
    let mut output_items: Vec<Value> = Vec::new();
    if !reasoning_text.is_empty() || !reasoning_sig.is_empty() {
        output_items.push(
            json!({ "type": "reasoning", "text": reasoning_text, "signature": reasoning_sig }),
        );
    }
    for call_id in &call_order {
        if let Some((name, args)) = calls.get(call_id) {
            output_items.push(json!({
                "type": "function_call",
                "call_id": call_id,
                "name": name,
                "arguments": args
            }));
        }
    }
    let output_item = json!({
        "type": "message",
        "role": "assistant",
        "content": [{ "type": "output_text", "text": output_text }]
    });
    output_items.push(output_item.clone());
    let final_response = json!({
        "id": response_id,
        "object": "response",
        "created": created,
        "model": urp.model,
        "status": "completed",
        "output": output_items
    });
    let _ = tx
        .send(wrap_responses_event(
            &mut seq,
            "response.output_item.added",
            output_item.clone(),
        ))
        .await;
    if !saw_text_delta {
        if let Some(text) = output_item
            .get("content")
            .and_then(|v| v.as_array())
            .and_then(|arr| arr.first())
            .and_then(|p| p.get("text"))
            .and_then(|v| v.as_str())
        {
            let _ = tx
                .send(wrap_responses_event(
                    &mut seq,
                    "response.output_text.delta",
                    json!({ "text": text }),
                ))
                .await;
        }
    }
    let _ = tx
        .send(wrap_responses_event(
            &mut seq,
            "response.output_item.done",
            output_item,
        ))
        .await;
    let _ = tx
        .send(wrap_responses_event(
            &mut seq,
            "response.output_text.done",
            json!({}),
        ))
        .await;
    let _ = tx
        .send(wrap_responses_event(
            &mut seq,
            "response.completed",
            final_response,
        ))
        .await;
    Ok(())
}

async fn stream_chat_sse_as_responses(
    urp: &UrpRequest,
    upstream_resp: reqwest::Response,
    tx: mpsc::Sender<Event>,
    started_at: Option<std::time::Instant>,
    runtime_metrics: Option<Arc<Mutex<StreamRuntimeMetrics>>>,
) -> AppResult<()> {
    let mut seq = 1u64;
    let response_id = format!("resp_{}", uuid::Uuid::new_v4());
    let created = now_ts();
    let mut output_text = String::new();
    let mut reasoning_text = String::new();
    let mut reasoning_sig = String::new();
    let mut call_order: Vec<String> = Vec::new();
    let mut calls: HashMap<String, (String, String)> = HashMap::new(); // call_id -> (name, arguments)
    let mut call_id_by_index: HashMap<usize, String> = HashMap::new();

    let base_response = json!({
        "id": response_id,
        "object": "response",
        "created": created,
        "model": urp.model,
        "status": "in_progress",
        "output": []
    });
    let _ = tx
        .send(wrap_responses_event(
            &mut seq,
            "response.created",
            base_response.clone(),
        ))
        .await;
    let _ = tx
        .send(wrap_responses_event(
            &mut seq,
            "response.in_progress",
            base_response.clone(),
        ))
        .await;
    let _ = tx
        .send(wrap_responses_event(
            &mut seq,
            "response.output_item.added",
            json!({"type":"message","role":"assistant","content":[]}),
        ))
        .await;

    let mut stream = upstream_resp.bytes_stream().eventsource();
    while let Some(ev) = stream.next().await {
        let Ok(ev) = ev else { continue };
        mark_stream_ttfb_if_needed(started_at, &runtime_metrics).await;
        if ev.data.trim() == "[DONE]" {
            break;
        }
        let data_val: Value = serde_json::from_str(&ev.data).unwrap_or(Value::Null);
        record_stream_usage_if_present(&runtime_metrics, parse_usage_from_chat_object(&data_val))
            .await;
        let delta = data_val
            .get("choices")
            .and_then(|v| v.as_array())
            .and_then(|arr| arr.first())
            .and_then(|c| c.get("delta"))
            .cloned()
            .unwrap_or(Value::Null);

        if let Some(t) = delta.get("content").and_then(|v| v.as_str()) {
            if !t.is_empty() {
                output_text.push_str(t);
                let _ = tx
                    .send(wrap_responses_event(
                        &mut seq,
                        "response.output_text.delta",
                        json!({ "text": t }),
                    ))
                    .await;
            }
        }

        let (reasoning_text_deltas, reasoning_sig_deltas) = extract_chat_reasoning_deltas(&delta);
        for t in reasoning_text_deltas {
            reasoning_text.push_str(&t);
            let _ = tx
                .send(wrap_responses_event(
                    &mut seq,
                    "response.reasoning_text.delta",
                    json!({ "delta": t }),
                ))
                .await;
        }
        for sig in reasoning_sig_deltas {
            reasoning_sig.push_str(&sig);
            let _ = tx
                .send(wrap_responses_event(
                    &mut seq,
                    "response.reasoning_signature.delta",
                    json!({ "delta": sig }),
                ))
                .await;
        }

        // Tool call deltas (OpenAI chat format).
        if let Some(tool_calls) = delta.get("tool_calls").and_then(|v| v.as_array()) {
            for (tool_call_pos, tc) in tool_calls.iter().enumerate() {
                let tc_index = tc.get("index").and_then(|v| v.as_u64()).map(|v| v as usize);
                let mut call_id = tc
                    .get("id")
                    .or_else(|| tc.get("call_id"))
                    .and_then(|v| v.as_str())
                    .unwrap_or("")
                    .to_string();
                if call_id.is_empty() {
                    if let Some(idx) = tc_index {
                        if let Some(existing) = call_id_by_index.get(&idx) {
                            call_id = existing.clone();
                        }
                    }
                }
                if call_id.is_empty() && tool_calls.len() == 1 {
                    if let Some(last) = call_order.last() {
                        call_id = last.clone();
                    }
                }
                if call_id.is_empty() {
                    if let Some(existing) = call_order.get(tool_call_pos) {
                        call_id = existing.clone();
                    }
                }
                if call_id.is_empty() {
                    continue;
                }
                if let Some(idx) = tc_index {
                    call_id_by_index.insert(idx, call_id.clone());
                }
                let name = tc
                    .get("function")
                    .and_then(|f| f.get("name"))
                    .and_then(|v| v.as_str())
                    .unwrap_or("")
                    .to_string();
                let args_delta = tc
                    .get("function")
                    .and_then(|f| f.get("arguments"))
                    .map(|v| {
                        v.as_str()
                            .map(|s| s.to_string())
                            .unwrap_or_else(|| v.to_string())
                    })
                    .unwrap_or_default();

                if !calls.contains_key(&call_id) {
                    call_order.push(call_id.clone());
                    calls.insert(call_id.clone(), (name.clone(), String::new()));
                    let item = json!({
                        "type": "function_call",
                        "call_id": call_id,
                        "name": name,
                        "arguments": ""
                    });
                    let _ = tx
                        .send(wrap_responses_event(
                            &mut seq,
                            "response.output_item.added",
                            item,
                        ))
                        .await;
                }

                let entry = calls.get_mut(&call_id).unwrap();

                if !name.is_empty() && entry.0.is_empty() {
                    entry.0 = name.clone();
                }
                if !args_delta.is_empty() {
                    entry.1.push_str(&args_delta);
                    let _ = tx
                        .send(wrap_responses_event(
                            &mut seq,
                            "response.function_call_arguments.delta",
                            json!({ "call_id": call_id, "name": entry.0, "delta": args_delta }),
                        ))
                        .await;
                }
            }
        }
    }

    // Finalize any function calls encountered in the chat stream.
    let mut output_items: Vec<Value> = Vec::new();
    if !reasoning_text.is_empty() || !reasoning_sig.is_empty() {
        output_items.push(
            json!({ "type": "reasoning", "text": reasoning_text, "signature": reasoning_sig }),
        );
    }
    for call_id in &call_order {
        if let Some((name, args)) = calls.get(call_id) {
            let item = json!({
                "type": "function_call",
                "call_id": call_id,
                "name": name,
                "arguments": args
            });
            let _ = tx
                .send(wrap_responses_event(
                    &mut seq,
                    "response.output_item.done",
                    item.clone(),
                ))
                .await;
            output_items.push(item);
        }
    }

    let output_item = json!({
        "type": "message",
        "role": "assistant",
        "content": [{ "type": "output_text", "text": output_text }]
    });
    output_items.push(output_item.clone());
    let final_response = json!({
        "id": response_id,
        "object": "response",
        "created": created,
        "model": urp.model,
        "status": "completed",
        "output": output_items
    });
    let _ = tx
        .send(wrap_responses_event(
            &mut seq,
            "response.output_item.done",
            output_item,
        ))
        .await;
    let _ = tx
        .send(wrap_responses_event(
            &mut seq,
            "response.output_text.done",
            json!({}),
        ))
        .await;
    let _ = tx
        .send(wrap_responses_event(
            &mut seq,
            "response.completed",
            final_response,
        ))
        .await;
    Ok(())
}

async fn stream_messages_sse_as_responses(
    urp: &UrpRequest,
    upstream_resp: reqwest::Response,
    tx: mpsc::Sender<Event>,
    started_at: Option<std::time::Instant>,
    runtime_metrics: Option<Arc<Mutex<StreamRuntimeMetrics>>>,
) -> AppResult<()> {
    let mut seq = 1u64;
    let response_id = format!("resp_{}", uuid::Uuid::new_v4());
    let created = now_ts();
    let mut output_text = String::new();
    let mut reasoning_text = String::new();
    let mut reasoning_sig = String::new();
    let mut call_order: Vec<String> = Vec::new();
    let mut calls: HashMap<String, (String, String)> = HashMap::new(); // call_id -> (name, arguments_json)
    let mut current_tool_call_id: Option<String> = None;

    let base_response = json!({
        "id": response_id,
        "object": "response",
        "created": created,
        "model": urp.model,
        "status": "in_progress",
        "output": []
    });
    let _ = tx
        .send(wrap_responses_event(
            &mut seq,
            "response.created",
            base_response.clone(),
        ))
        .await;
    let _ = tx
        .send(wrap_responses_event(
            &mut seq,
            "response.in_progress",
            base_response.clone(),
        ))
        .await;
    let _ = tx
        .send(wrap_responses_event(
            &mut seq,
            "response.output_item.added",
            json!({"type":"message","role":"assistant","content":[]}),
        ))
        .await;

    let mut stream = upstream_resp.bytes_stream().eventsource();
    while let Some(ev) = stream.next().await {
        let Ok(ev) = ev else { continue };
        mark_stream_ttfb_if_needed(started_at, &runtime_metrics).await;
        if ev.data.trim() == "[DONE]" {
            break;
        }
        let data_val: Value = serde_json::from_str(&ev.data).unwrap_or(Value::Null);
        record_stream_usage_if_present(
            &runtime_metrics,
            parse_usage_from_messages_object(&data_val),
        )
        .await;
        let event_type = data_val.get("type").and_then(|v| v.as_str()).unwrap_or("");
        match event_type {
            "content_block_start" => {
                let cb = data_val
                    .get("content_block")
                    .cloned()
                    .unwrap_or(Value::Null);
                let cb_type = cb.get("type").and_then(|v| v.as_str()).unwrap_or("");
                if cb_type == "tool_use" {
                    let call_id = cb
                        .get("id")
                        .and_then(|v| v.as_str())
                        .unwrap_or("")
                        .to_string();
                    let name = cb
                        .get("name")
                        .and_then(|v| v.as_str())
                        .unwrap_or("")
                        .to_string();
                    current_tool_call_id = if call_id.is_empty() {
                        None
                    } else {
                        Some(call_id.clone())
                    };
                    if !call_id.is_empty() && !calls.contains_key(&call_id) {
                        call_order.push(call_id.clone());
                        calls.insert(call_id.clone(), (name.clone(), String::new()));
                        let item = json!({
                            "type": "function_call",
                            "call_id": call_id,
                            "name": name,
                            "arguments": ""
                        });
                        let _ = tx
                            .send(wrap_responses_event(
                                &mut seq,
                                "response.output_item.added",
                                item,
                            ))
                            .await;
                    }
                }
            }
            "content_block_delta" => {
                let delta = data_val.get("delta").cloned().unwrap_or(Value::Null);
                let delta_type = delta.get("type").and_then(|v| v.as_str()).unwrap_or("");
                match delta_type {
                    "text_delta" => {
                        if let Some(text) = delta.get("text").and_then(|v| v.as_str()) {
                            if !text.is_empty() {
                                output_text.push_str(text);
                                let _ = tx
                                    .send(wrap_responses_event(
                                        &mut seq,
                                        "response.output_text.delta",
                                        json!({ "text": text }),
                                    ))
                                    .await;
                            }
                        }
                    }
                    "thinking_delta" => {
                        if let Some(t) = delta.get("thinking").and_then(|v| v.as_str()) {
                            if !t.is_empty() {
                                reasoning_text.push_str(t);
                                let _ = tx
                                    .send(wrap_responses_event(
                                        &mut seq,
                                        "response.reasoning_text.delta",
                                        json!({ "delta": t }),
                                    ))
                                    .await;
                            }
                        }
                    }
                    "signature_delta" => {
                        if let Some(s) = delta.get("signature").and_then(|v| v.as_str()) {
                            if !s.is_empty() {
                                reasoning_sig.push_str(s);
                                let _ = tx
                                    .send(wrap_responses_event(
                                        &mut seq,
                                        "response.reasoning_signature.delta",
                                        json!({ "delta": s }),
                                    ))
                                    .await;
                            }
                        }
                    }
                    "input_json_delta" => {
                        if let Some(partial) = delta.get("partial_json").and_then(|v| v.as_str()) {
                            if let Some(call_id) = current_tool_call_id.clone() {
                                if let Some(entry) = calls.get_mut(&call_id) {
                                    entry.1.push_str(partial);
                                    let _ = tx
                                        .send(wrap_responses_event(
                                            &mut seq,
                                            "response.function_call_arguments.delta",
                                            json!({ "call_id": call_id, "name": entry.0, "delta": partial }),
                                        ))
                                        .await;
                                }
                            }
                        }
                    }
                    _ => {}
                }
            }
            "content_block_stop" => {
                current_tool_call_id = None;
            }
            "message_stop" => break,
            _ => {}
        }
    }

    let mut output_items: Vec<Value> = Vec::new();
    if !reasoning_text.is_empty() || !reasoning_sig.is_empty() {
        output_items.push(
            json!({ "type": "reasoning", "text": reasoning_text, "signature": reasoning_sig }),
        );
    }
    for call_id in &call_order {
        if let Some((name, args)) = calls.get(call_id) {
            let item = json!({
                "type": "function_call",
                "call_id": call_id,
                "name": name,
                "arguments": args
            });
            let _ = tx
                .send(wrap_responses_event(
                    &mut seq,
                    "response.output_item.done",
                    item.clone(),
                ))
                .await;
            output_items.push(item);
        }
    }

    let output_item = json!({
        "type": "message",
        "role": "assistant",
        "content": [{ "type": "output_text", "text": output_text }]
    });
    output_items.push(output_item.clone());
    let final_response = json!({
        "id": response_id,
        "object": "response",
        "created": created,
        "model": urp.model,
        "status": "completed",
        "output": output_items
    });
    let _ = tx
        .send(wrap_responses_event(
            &mut seq,
            "response.output_item.done",
            output_item,
        ))
        .await;
    let _ = tx
        .send(wrap_responses_event(
            &mut seq,
            "response.output_text.done",
            json!({}),
        ))
        .await;
    let _ = tx
        .send(wrap_responses_event(
            &mut seq,
            "response.completed",
            final_response,
        ))
        .await;
    Ok(())
}

async fn stream_gemini_sse_as_responses(
    urp: &UrpRequest,
    upstream_resp: reqwest::Response,
    tx: mpsc::Sender<Event>,
    started_at: Option<std::time::Instant>,
    runtime_metrics: Option<Arc<Mutex<StreamRuntimeMetrics>>>,
) -> AppResult<()> {
    let mut seq = 1u64;
    let response_id = format!("resp_{}", uuid::Uuid::new_v4());
    let created = now_ts();
    let mut output_text = String::new();
    let mut reasoning_text = String::new();
    let mut reasoning_sig = String::new();
    let mut call_order: Vec<String> = Vec::new();
    let mut calls: HashMap<String, (String, String)> = HashMap::new();

    let base_response = json!({
        "id": response_id,
        "object": "response",
        "created": created,
        "model": urp.model,
        "status": "in_progress",
        "output": []
    });
    let _ = tx
        .send(wrap_responses_event(
            &mut seq,
            "response.created",
            base_response.clone(),
        ))
        .await;
    let _ = tx
        .send(wrap_responses_event(
            &mut seq,
            "response.in_progress",
            base_response.clone(),
        ))
        .await;

    let mut stream = upstream_resp.bytes_stream().eventsource();
    while let Some(ev) = stream.next().await {
        let Ok(ev) = ev else { continue };
        mark_stream_ttfb_if_needed(started_at, &runtime_metrics).await;
        if ev.data.trim() == "[DONE]" {
            break;
        }
        let data_val: Value = serde_json::from_str(&ev.data).unwrap_or(Value::Null);
        record_stream_usage_if_present(&runtime_metrics, parse_usage_from_gemini_object(&data_val))
            .await;
        let Some(candidate) = data_val
            .get("candidates")
            .and_then(|v| v.as_array())
            .and_then(|arr| arr.first())
        else {
            continue;
        };
        if let Some(parts) = candidate
            .get("content")
            .and_then(|v| v.get("parts"))
            .and_then(|v| v.as_array())
        {
            for part in parts {
                if let Some(text) = part.get("text").and_then(|v| v.as_str()) {
                    if part.get("thought").and_then(|v| v.as_bool()) == Some(true) {
                        if !text.is_empty() {
                            reasoning_text.push_str(text);
                            let _ = tx
                                .send(wrap_responses_event(
                                    &mut seq,
                                    "response.reasoning_text.delta",
                                    json!({ "delta": text }),
                                ))
                                .await;
                        }
                        if let Some(sig) = part.get("thoughtSignature") {
                            let sig_text = sig
                                .as_str()
                                .map(|s| s.to_string())
                                .unwrap_or_else(|| sig.to_string());
                            if !sig_text.is_empty() {
                                reasoning_sig.push_str(&sig_text);
                                let _ = tx
                                    .send(wrap_responses_event(
                                        &mut seq,
                                        "response.reasoning_signature.delta",
                                        json!({ "delta": sig_text }),
                                    ))
                                    .await;
                            }
                        }
                    } else if !text.is_empty() {
                        output_text.push_str(text);
                        let _ = tx
                            .send(wrap_responses_event(
                                &mut seq,
                                "response.output_text.delta",
                                json!({ "text": text }),
                            ))
                            .await;
                    }
                }

                if let Some(fc) = part.get("functionCall").and_then(|v| v.as_object()) {
                    let call_id = fc
                        .get("id")
                        .or_else(|| fc.get("name"))
                        .and_then(|v| v.as_str())
                        .unwrap_or("")
                        .to_string();
                    let name = fc
                        .get("name")
                        .and_then(|v| v.as_str())
                        .unwrap_or("")
                        .to_string();
                    let arguments =
                        serde_json::to_string(&fc.get("args").cloned().unwrap_or(Value::Null))
                            .unwrap_or_else(|_| "{}".to_string());
                    if !name.is_empty() {
                        let key = if call_id.is_empty() {
                            format!("call_{}", call_order.len() + 1)
                        } else {
                            call_id
                        };
                        if !calls.contains_key(&key) {
                            call_order.push(key.clone());
                            calls.insert(key.clone(), (name.clone(), String::new()));
                            let _ = tx
                                .send(wrap_responses_event(
                                    &mut seq,
                                    "response.output_item.added",
                                    json!({
                                        "type": "function_call",
                                        "call_id": key,
                                        "name": name,
                                        "arguments": ""
                                    }),
                                ))
                                .await;
                        }
                        if !arguments.is_empty() {
                            if let Some(entry) = calls.get_mut(&key) {
                                entry.1.push_str(&arguments);
                            }
                            let _ = tx
                                .send(wrap_responses_event(
                                    &mut seq,
                                    "response.function_call_arguments.delta",
                                    json!({ "call_id": key, "name": name, "delta": arguments }),
                                ))
                                .await;
                        }
                    }
                }
            }
        }
    }

    let mut output_items: Vec<Value> = Vec::new();
    if !reasoning_text.is_empty() || !reasoning_sig.is_empty() {
        output_items.push(
            json!({ "type": "reasoning", "text": reasoning_text, "signature": reasoning_sig }),
        );
    }
    for call_id in &call_order {
        if let Some((name, args)) = calls.get(call_id) {
            let item = json!({
                "type": "function_call",
                "call_id": call_id,
                "name": name,
                "arguments": args
            });
            let _ = tx
                .send(wrap_responses_event(
                    &mut seq,
                    "response.output_item.done",
                    item.clone(),
                ))
                .await;
            output_items.push(item);
        }
    }

    let output_item = json!({
        "type": "message",
        "role": "assistant",
        "content": [{ "type": "output_text", "text": output_text }]
    });
    output_items.push(output_item.clone());
    let final_response = json!({
        "id": response_id,
        "object": "response",
        "created": created,
        "model": urp.model,
        "status": "completed",
        "output": output_items
    });
    let _ = tx
        .send(wrap_responses_event(
            &mut seq,
            "response.output_item.done",
            output_item,
        ))
        .await;
    let _ = tx
        .send(wrap_responses_event(
            &mut seq,
            "response.output_text.done",
            json!({}),
        ))
        .await;
    let _ = tx
        .send(wrap_responses_event(
            &mut seq,
            "response.completed",
            final_response,
        ))
        .await;
    Ok(())
}

async fn stream_gemini_sse_as_chat(
    urp: &UrpRequest,
    upstream_resp: reqwest::Response,
    tx: mpsc::Sender<Event>,
    started_at: Option<std::time::Instant>,
    runtime_metrics: Option<Arc<Mutex<StreamRuntimeMetrics>>>,
) -> AppResult<()> {
    let id = format!("chatcmpl_{}", uuid::Uuid::new_v4());
    let created = now_ts();
    let mut saw_tool_call = false;

    let mut stream = upstream_resp.bytes_stream().eventsource();
    while let Some(ev) = stream.next().await {
        let Ok(ev) = ev else { continue };
        mark_stream_ttfb_if_needed(started_at, &runtime_metrics).await;
        if ev.data.trim() == "[DONE]" {
            break;
        }
        let data_val: Value = serde_json::from_str(&ev.data).unwrap_or(Value::Null);
        record_stream_usage_if_present(&runtime_metrics, parse_usage_from_gemini_object(&data_val))
            .await;
        let Some(candidate) = data_val
            .get("candidates")
            .and_then(|v| v.as_array())
            .and_then(|arr| arr.first())
        else {
            continue;
        };
        let Some(parts) = candidate
            .get("content")
            .and_then(|v| v.get("parts"))
            .and_then(|v| v.as_array())
        else {
            continue;
        };

        for part in parts {
            if let Some(text) = part.get("text").and_then(|v| v.as_str()) {
                if part.get("thought").and_then(|v| v.as_bool()) == Some(true) {
                    let chunk = json!({
                        "id": id,
                        "object": "chat.completion.chunk",
                        "created": created,
                        "model": urp.model,
                        "choices": [{ "index": 0, "delta": chat_reasoning_delta_from_text(text), "finish_reason": Value::Null }]
                    });
                    let _ = tx.send(Event::default().data(chunk.to_string())).await;
                    if let Some(sig) = part.get("thoughtSignature") {
                        let sig_text = sig
                            .as_str()
                            .map(|s| s.to_string())
                            .unwrap_or_else(|| sig.to_string());
                        if !sig_text.is_empty() {
                            let chunk = json!({
                                "id": id,
                                "object": "chat.completion.chunk",
                                "created": created,
                                "model": urp.model,
                                "choices": [{ "index": 0, "delta": chat_reasoning_delta_from_signature(&sig_text), "finish_reason": Value::Null }]
                            });
                            let _ = tx.send(Event::default().data(chunk.to_string())).await;
                        }
                    }
                } else if !text.is_empty() {
                    let chunk = json!({
                        "id": id,
                        "object": "chat.completion.chunk",
                        "created": created,
                        "model": urp.model,
                        "choices": [{ "index": 0, "delta": { "content": text }, "finish_reason": Value::Null }]
                    });
                    let _ = tx.send(Event::default().data(chunk.to_string())).await;
                }
            }

            if let Some(fc) = part.get("functionCall").and_then(|v| v.as_object()) {
                let call_id = fc
                    .get("id")
                    .or_else(|| fc.get("name"))
                    .and_then(|v| v.as_str())
                    .unwrap_or("")
                    .to_string();
                let name = fc
                    .get("name")
                    .and_then(|v| v.as_str())
                    .unwrap_or("")
                    .to_string();
                let args = serde_json::to_string(&fc.get("args").cloned().unwrap_or(Value::Null))
                    .unwrap_or_else(|_| "{}".to_string());
                if !name.is_empty() {
                    saw_tool_call = true;
                    let chunk = json!({
                        "id": id,
                        "object": "chat.completion.chunk",
                        "created": created,
                        "model": urp.model,
                        "choices": [{
                            "index": 0,
                            "delta": {
                                "tool_calls": [{
                                    "index": 0,
                                    "id": call_id,
                                    "type": "function",
                                    "function": { "name": name, "arguments": args }
                                }]
                            },
                            "finish_reason": Value::Null
                        }]
                    });
                    let _ = tx.send(Event::default().data(chunk.to_string())).await;
                }
            }
        }
    }

    let finish_reason = if saw_tool_call { "tool_calls" } else { "stop" };
    let done = json!({
        "id": id,
        "object": "chat.completion.chunk",
        "created": created,
        "model": urp.model,
        "choices": [{ "index": 0, "delta": {}, "finish_reason": finish_reason }]
    });
    let _ = tx.send(Event::default().data(done.to_string())).await;
    let _ = tx.send(Event::default().data("[DONE]")).await;
    Ok(())
}

async fn stream_gemini_sse_as_messages(
    urp: &UrpRequest,
    upstream_resp: reqwest::Response,
    tx: mpsc::Sender<Event>,
    started_at: Option<std::time::Instant>,
    runtime_metrics: Option<Arc<Mutex<StreamRuntimeMetrics>>>,
) -> AppResult<()> {
    let message_id = format!("msg_{}", uuid::Uuid::new_v4());
    let start = json!({
        "type": "message_start",
        "message": {
            "id": message_id,
            "type": "message",
            "role": "assistant",
            "model": urp.model,
            "content": [],
            "stop_reason": Value::Null,
            "stop_sequence": Value::Null,
            "usage": {
                "input_tokens": 0,
                "output_tokens": 0
            }
        }
    });
    let _ = tx.send(Event::default().data(start.to_string())).await;

    let mut next_index: u32 = 0;
    let mut text_index: Option<u32> = None;
    let mut thinking_index: Option<u32> = None;
    let mut tool_indices: HashMap<String, u32> = HashMap::new();
    let mut tool_names: HashMap<String, String> = HashMap::new();
    let mut started: Vec<u32> = Vec::new();
    let mut saw_tool_use = false;

    let mut stream = upstream_resp.bytes_stream().eventsource();
    while let Some(ev) = stream.next().await {
        let Ok(ev) = ev else { continue };
        mark_stream_ttfb_if_needed(started_at, &runtime_metrics).await;
        if ev.data.trim() == "[DONE]" {
            break;
        }
        let data_val: Value = serde_json::from_str(&ev.data).unwrap_or(Value::Null);
        record_stream_usage_if_present(&runtime_metrics, parse_usage_from_gemini_object(&data_val))
            .await;
        let Some(candidate) = data_val
            .get("candidates")
            .and_then(|v| v.as_array())
            .and_then(|arr| arr.first())
        else {
            continue;
        };
        let Some(parts) = candidate
            .get("content")
            .and_then(|v| v.get("parts"))
            .and_then(|v| v.as_array())
        else {
            continue;
        };

        for part in parts {
            if let Some(text) = part.get("text").and_then(|v| v.as_str()) {
                if part.get("thought").and_then(|v| v.as_bool()) == Some(true) {
                    let idx = ensure_anthropic_thinking_block(
                        &tx,
                        &mut thinking_index,
                        &mut next_index,
                        &mut started,
                    )
                    .await?;
                    let d = json!({
                        "type": "content_block_delta",
                        "index": idx,
                        "delta": { "type": "thinking_delta", "thinking": text }
                    });
                    let _ = tx.send(Event::default().data(d.to_string())).await;
                    if let Some(sig) = part.get("thoughtSignature") {
                        let sig_text = sig
                            .as_str()
                            .map(|s| s.to_string())
                            .unwrap_or_else(|| sig.to_string());
                        if !sig_text.is_empty() {
                            let d = json!({
                                "type": "content_block_delta",
                                "index": idx,
                                "delta": { "type": "signature_delta", "signature": sig_text }
                            });
                            let _ = tx.send(Event::default().data(d.to_string())).await;
                        }
                    }
                } else if !text.is_empty() {
                    let idx = ensure_anthropic_text_block(
                        &tx,
                        &mut text_index,
                        &mut next_index,
                        &mut started,
                    )
                    .await?;
                    let d = json!({
                        "type": "content_block_delta",
                        "index": idx,
                        "delta": { "type": "text_delta", "text": text }
                    });
                    let _ = tx.send(Event::default().data(d.to_string())).await;
                }
            }

            if let Some(fc) = part.get("functionCall").and_then(|v| v.as_object()) {
                let call_id = fc
                    .get("id")
                    .or_else(|| fc.get("name"))
                    .and_then(|v| v.as_str())
                    .unwrap_or("");
                let name = fc.get("name").and_then(|v| v.as_str()).unwrap_or("");
                let args = serde_json::to_string(&fc.get("args").cloned().unwrap_or(Value::Null))
                    .unwrap_or_else(|_| "{}".to_string());
                if !name.is_empty() {
                    saw_tool_use = true;
                    let idx = ensure_anthropic_tool_block(
                        &tx,
                        &mut tool_indices,
                        &mut tool_names,
                        &mut next_index,
                        &mut started,
                        call_id,
                        name,
                    )
                    .await?;
                    if !args.is_empty() {
                        let d = json!({
                            "type": "content_block_delta",
                            "index": idx,
                            "delta": { "type": "input_json_delta", "partial_json": args }
                        });
                        let _ = tx.send(Event::default().data(d.to_string())).await;
                    }
                }
            }
        }
    }

    for idx in started.iter().copied() {
        let stop = json!({ "type": "content_block_stop", "index": idx });
        let _ = tx.send(Event::default().data(stop.to_string())).await;
    }
    let message_delta = json!({
        "type": "message_delta",
        "delta": {
            "stop_reason": if saw_tool_use { "tool_use" } else { "end_turn" },
            "stop_sequence": Value::Null
        },
        "usage": {
            "input_tokens": 0,
            "output_tokens": 0
        }
    });
    let _ = tx
        .send(Event::default().data(message_delta.to_string()))
        .await;
    let stop = json!({ "type": "message_stop" });
    let _ = tx.send(Event::default().data(stop.to_string())).await;
    Ok(())
}

async fn stream_any_sse_as_chat(
    urp: &UrpRequest,
    provider_type: ProviderType,
    upstream_resp: reqwest::Response,
    tx: mpsc::Sender<Event>,
    started_at: Option<std::time::Instant>,
    runtime_metrics: Option<Arc<Mutex<StreamRuntimeMetrics>>>,
) -> AppResult<()> {
    let id = format!("chatcmpl_{}", uuid::Uuid::new_v4());
    let created = now_ts();
    let mut out_text = String::new();
    let mut saw_tool_call = false;
    let mut saw_responses_text_delta = false;
    let mut call_order: Vec<String> = Vec::new();
    let mut call_names: HashMap<String, String> = HashMap::new();
    let mut call_ids_by_output_index: HashMap<u64, String> = HashMap::new();

    let mut stream = upstream_resp.bytes_stream().eventsource();
    while let Some(ev) = stream.next().await {
        let Ok(ev) = ev else { continue };
        mark_stream_ttfb_if_needed(started_at, &runtime_metrics).await;
        if ev.data.trim() == "[DONE]" {
            break;
        }
        match provider_type {
            ProviderType::ChatCompletion => {
                let data_val: Value = serde_json::from_str(&ev.data).unwrap_or(Value::Null);
                record_stream_usage_if_present(
                    &runtime_metrics,
                    parse_usage_from_chat_object(&data_val),
                )
                .await;
                let mut chunk = data_val;
                if let Some(obj) = chunk.as_object_mut() {
                    obj.insert("model".to_string(), Value::String(urp.model.clone()));
                    if !obj.contains_key("id") {
                        obj.insert("id".to_string(), Value::String(id.clone()));
                    }
                    if !obj.contains_key("object") {
                        obj.insert(
                            "object".to_string(),
                            Value::String("chat.completion.chunk".to_string()),
                        );
                    }
                    if !obj.contains_key("created") {
                        obj.insert("created".to_string(), Value::Number(created.into()));
                    }
                    if let Some(delta) = obj
                        .get_mut("choices")
                        .and_then(|v| v.as_array_mut())
                        .and_then(|arr| arr.first_mut())
                        .and_then(|v| v.get_mut("delta"))
                        .and_then(|v| v.as_object_mut())
                    {
                        normalize_chat_reasoning_delta_object(delta);
                    }
                }

                if let Some(t) = chunk
                    .get("choices")
                    .and_then(|v| v.as_array())
                    .and_then(|arr| arr.first())
                    .and_then(|c| c.get("delta"))
                    .and_then(|d| d.get("content"))
                    .and_then(|v| v.as_str())
                {
                    out_text.push_str(t);
                }

                let _ = tx.send(Event::default().data(chunk.to_string())).await;
            }
            ProviderType::Responses => {
                let data_val: Value = serde_json::from_str(&ev.data).unwrap_or(Value::Null);
                record_stream_usage_if_present(
                    &runtime_metrics,
                    parse_usage_from_responses_object(&data_val),
                )
                .await;
                match ev.event.as_str() {
                    "response.output_text.delta" => {
                        let t = data_val
                            .get("text")
                            .and_then(|v| v.as_str())
                            .or_else(|| data_val.get("delta").and_then(|v| v.as_str()))
                            .unwrap_or("");
                        if !t.is_empty() {
                            saw_responses_text_delta = true;
                            out_text.push_str(t);
                            let chunk = json!({
                                "id": id,
                                "object": "chat.completion.chunk",
                                "created": created,
                                "model": urp.model,
                                "choices": [{ "index": 0, "delta": { "content": t }, "finish_reason": Value::Null }]
                            });
                            let _ = tx.send(Event::default().data(chunk.to_string())).await;
                        }
                    }
                    "response.reasoning_text.delta" => {
                        let t = data_val
                            .get("delta")
                            .and_then(|v| v.as_str())
                            .or_else(|| data_val.get("text").and_then(|v| v.as_str()))
                            .unwrap_or("");
                        if !t.is_empty() {
                            let chunk = json!({
                                "id": id,
                                "object": "chat.completion.chunk",
                                "created": created,
                                "model": urp.model,
                                "choices": [{ "index": 0, "delta": chat_reasoning_delta_from_text(t), "finish_reason": Value::Null }]
                            });
                            let _ = tx.send(Event::default().data(chunk.to_string())).await;
                        }
                    }
                    "response.reasoning_signature.delta" => {
                        let t = data_val.get("delta").and_then(|v| v.as_str()).unwrap_or("");
                        if !t.is_empty() {
                            let chunk = json!({
                                "id": id,
                                "object": "chat.completion.chunk",
                                "created": created,
                                "model": urp.model,
                                "choices": [{ "index": 0, "delta": chat_reasoning_delta_from_signature(t), "finish_reason": Value::Null }]
                            });
                            let _ = tx.send(Event::default().data(chunk.to_string())).await;
                        }
                    }
                    "response.output_item.added" => {
                        let item = data_val.get("item").unwrap_or(&data_val);
                        if item.get("type").and_then(|v| v.as_str()) == Some("function_call") {
                            let call_id = item
                                .get("call_id")
                                .and_then(|v| v.as_str())
                                .unwrap_or("")
                                .to_string();
                            let name = item
                                .get("name")
                                .and_then(|v| v.as_str())
                                .unwrap_or("")
                                .to_string();
                            if !call_id.is_empty() {
                                if !call_order.contains(&call_id) {
                                    call_order.push(call_id.clone());
                                }
                                if !name.is_empty() {
                                    call_names.insert(call_id.clone(), name.clone());
                                }
                                if let Some(output_index) =
                                    data_val.get("output_index").and_then(|v| v.as_u64())
                                {
                                    call_ids_by_output_index.insert(output_index, call_id.clone());
                                }
                                let idx =
                                    call_order.iter().position(|x| x == &call_id).unwrap_or(0);
                                saw_tool_call = true;
                                let chunk = json!({
                                    "id": id,
                                    "object": "chat.completion.chunk",
                                    "created": created,
                                    "model": urp.model,
                                    "choices": [{
                                        "index": 0,
                                        "delta": {
                                            "tool_calls": [{
                                                "index": idx,
                                                "id": call_id,
                                                "type": "function",
                                                "function": { "name": name, "arguments": "" }
                                            }]
                                        },
                                        "finish_reason": Value::Null
                                    }]
                                });
                                let _ = tx.send(Event::default().data(chunk.to_string())).await;
                            }
                        }
                    }
                    "response.function_call_arguments.delta" => {
                        let call_id = data_val
                            .get("call_id")
                            .and_then(|v| v.as_str())
                            .map(|s| s.to_string())
                            .or_else(|| {
                                data_val
                                    .get("output_index")
                                    .and_then(|v| v.as_u64())
                                    .and_then(|idx| call_ids_by_output_index.get(&idx).cloned())
                            })
                            .unwrap_or_default();
                        if call_id.is_empty() {
                            continue;
                        }
                        if !call_order.contains(&call_id) {
                            call_order.push(call_id.clone());
                        }
                        let idx = call_order.iter().position(|x| x == &call_id).unwrap_or(0);
                        let name = data_val
                            .get("name")
                            .and_then(|v| v.as_str())
                            .map(|s| s.to_string())
                            .or_else(|| call_names.get(&call_id).cloned())
                            .unwrap_or_default();
                        if !name.is_empty() {
                            call_names.insert(call_id.clone(), name.clone());
                        }
                        let delta = data_val.get("delta").and_then(|v| v.as_str()).unwrap_or("");
                        saw_tool_call = true;
                        let chunk = json!({
                            "id": id,
                            "object": "chat.completion.chunk",
                            "created": created,
                            "model": urp.model,
                            "choices": [{
                                "index": 0,
                                "delta": {
                                    "tool_calls": [{
                                        "index": idx,
                                        "id": call_id,
                                        "type": "function",
                                        "function": { "name": name, "arguments": delta }
                                    }]
                                },
                                "finish_reason": Value::Null
                            }]
                        });
                        let _ = tx.send(Event::default().data(chunk.to_string())).await;
                    }
                    "response.output_item.done" => {
                        let item = data_val.get("item").unwrap_or(&data_val);
                        if item.get("type").and_then(|v| v.as_str()) == Some("function_call") {
                            let call_id = item
                                .get("call_id")
                                .and_then(|v| v.as_str())
                                .unwrap_or("")
                                .to_string();
                            if call_id.is_empty() {
                                continue;
                            }
                            let name = item
                                .get("name")
                                .and_then(|v| v.as_str())
                                .map(|s| s.to_string())
                                .or_else(|| call_names.get(&call_id).cloned())
                                .unwrap_or_default();
                            if !call_order.contains(&call_id) {
                                call_order.push(call_id.clone());
                            }
                            if !name.is_empty() {
                                call_names.insert(call_id.clone(), name.clone());
                            }
                            if let Some(output_index) =
                                data_val.get("output_index").and_then(|v| v.as_u64())
                            {
                                call_ids_by_output_index.insert(output_index, call_id.clone());
                            }
                            let idx = call_order.iter().position(|x| x == &call_id).unwrap_or(0);
                            let args = item.get("arguments").and_then(|v| v.as_str()).unwrap_or("");
                            if !args.is_empty() {
                                saw_tool_call = true;
                                let chunk = json!({
                                    "id": id,
                                    "object": "chat.completion.chunk",
                                    "created": created,
                                    "model": urp.model,
                                    "choices": [{
                                        "index": 0,
                                        "delta": {
                                            "tool_calls": [{
                                                "index": idx,
                                                "id": call_id,
                                                "type": "function",
                                                "function": { "name": name, "arguments": args }
                                            }]
                                        },
                                        "finish_reason": Value::Null
                                    }]
                                });
                                let _ = tx.send(Event::default().data(chunk.to_string())).await;
                            }
                        } else if item.get("type").and_then(|v| v.as_str()) == Some("message") {
                            if !saw_responses_text_delta {
                                let text = extract_responses_message_text(item);
                                if !text.is_empty() {
                                    out_text.push_str(&text);
                                    let chunk = json!({
                                        "id": id,
                                        "object": "chat.completion.chunk",
                                        "created": created,
                                        "model": urp.model,
                                        "choices": [{ "index": 0, "delta": { "content": text }, "finish_reason": Value::Null }]
                                    });
                                    let _ = tx.send(Event::default().data(chunk.to_string())).await;
                                }
                            }
                        } else if item.get("type").and_then(|v| v.as_str()) == Some("reasoning") {
                            let (reasoning_text, reasoning_sig) =
                                extract_reasoning_text_and_signature(item);
                            if !reasoning_text.is_empty() {
                                let chunk = json!({
                                    "id": id,
                                    "object": "chat.completion.chunk",
                                    "created": created,
                                    "model": urp.model,
                                    "choices": [{ "index": 0, "delta": chat_reasoning_delta_from_text(&reasoning_text), "finish_reason": Value::Null }]
                                });
                                let _ = tx.send(Event::default().data(chunk.to_string())).await;
                            }
                            if !reasoning_sig.is_empty() {
                                let chunk = json!({
                                    "id": id,
                                    "object": "chat.completion.chunk",
                                    "created": created,
                                    "model": urp.model,
                                    "choices": [{ "index": 0, "delta": chat_reasoning_delta_from_signature(&reasoning_sig), "finish_reason": Value::Null }]
                                });
                                let _ = tx.send(Event::default().data(chunk.to_string())).await;
                            }
                        }
                    }
                    _ => {}
                }
            }
            ProviderType::Messages => {
                let data_val: Value = serde_json::from_str(&ev.data).unwrap_or(Value::Null);
                record_stream_usage_if_present(
                    &runtime_metrics,
                    parse_usage_from_messages_object(&data_val),
                )
                .await;
                let t = data_val.get("type").and_then(|v| v.as_str()).unwrap_or("");
                match t {
                    "content_block_delta" => {
                        let delta = data_val.get("delta").cloned().unwrap_or(Value::Null);
                        let dt = delta.get("type").and_then(|v| v.as_str()).unwrap_or("");
                        match dt {
                            "text_delta" => {
                                let txt = delta.get("text").and_then(|v| v.as_str()).unwrap_or("");
                                if !txt.is_empty() {
                                    out_text.push_str(txt);
                                    let chunk = json!({
                                        "id": id,
                                        "object": "chat.completion.chunk",
                                        "created": created,
                                        "model": urp.model,
                                        "choices": [{ "index": 0, "delta": { "content": txt }, "finish_reason": Value::Null }]
                                    });
                                    let _ = tx.send(Event::default().data(chunk.to_string())).await;
                                }
                            }
                            "thinking_delta" => {
                                let txt =
                                    delta.get("thinking").and_then(|v| v.as_str()).unwrap_or("");
                                if !txt.is_empty() {
                                    let chunk = json!({
                                        "id": id,
                                        "object": "chat.completion.chunk",
                                        "created": created,
                                        "model": urp.model,
                                        "choices": [{ "index": 0, "delta": chat_reasoning_delta_from_text(txt), "finish_reason": Value::Null }]
                                    });
                                    let _ = tx.send(Event::default().data(chunk.to_string())).await;
                                }
                            }
                            "signature_delta" => {
                                let txt = delta
                                    .get("signature")
                                    .and_then(|v| v.as_str())
                                    .unwrap_or("");
                                if !txt.is_empty() {
                                    let chunk = json!({
                                        "id": id,
                                        "object": "chat.completion.chunk",
                                        "created": created,
                                        "model": urp.model,
                                        "choices": [{ "index": 0, "delta": chat_reasoning_delta_from_signature(txt), "finish_reason": Value::Null }]
                                    });
                                    let _ = tx.send(Event::default().data(chunk.to_string())).await;
                                }
                            }
                            "input_json_delta" => {
                                let call_id = call_order.last().cloned().unwrap_or_default();
                                let partial = delta
                                    .get("partial_json")
                                    .and_then(|v| v.as_str())
                                    .unwrap_or("");
                                if !call_id.is_empty() && !partial.is_empty() {
                                    let idx =
                                        call_order.iter().position(|x| x == &call_id).unwrap_or(0);
                                    let name =
                                        call_names.get(&call_id).cloned().unwrap_or_default();
                                    saw_tool_call = true;
                                    let chunk = json!({
                                        "id": id,
                                        "object": "chat.completion.chunk",
                                        "created": created,
                                        "model": urp.model,
                                        "choices": [{
                                            "index": 0,
                                            "delta": {
                                                "tool_calls": [{
                                                    "index": idx,
                                                    "id": call_id,
                                                    "type": "function",
                                                    "function": { "name": name, "arguments": partial }
                                                }]
                                            },
                                            "finish_reason": Value::Null
                                        }]
                                    });
                                    let _ = tx.send(Event::default().data(chunk.to_string())).await;
                                }
                            }
                            _ => {}
                        }
                    }
                    "content_block_start" => {
                        let cb = data_val
                            .get("content_block")
                            .cloned()
                            .unwrap_or(Value::Null);
                        if cb.get("type").and_then(|v| v.as_str()) == Some("tool_use") {
                            let call_id = cb
                                .get("id")
                                .and_then(|v| v.as_str())
                                .unwrap_or("")
                                .to_string();
                            let name = cb
                                .get("name")
                                .and_then(|v| v.as_str())
                                .unwrap_or("")
                                .to_string();
                            if !call_id.is_empty() {
                                if !call_order.contains(&call_id) {
                                    call_order.push(call_id.clone());
                                }
                                if !name.is_empty() {
                                    call_names.insert(call_id.clone(), name.clone());
                                }
                                let idx =
                                    call_order.iter().position(|x| x == &call_id).unwrap_or(0);
                                saw_tool_call = true;
                                let chunk = json!({
                                    "id": id,
                                    "object": "chat.completion.chunk",
                                    "created": created,
                                    "model": urp.model,
                                    "choices": [{
                                        "index": 0,
                                        "delta": {
                                            "tool_calls": [{
                                                "index": idx,
                                                "id": call_id,
                                                "type": "function",
                                                "function": { "name": name, "arguments": "" }
                                            }]
                                        },
                                        "finish_reason": Value::Null
                                    }]
                                });
                                let _ = tx.send(Event::default().data(chunk.to_string())).await;
                            }
                        }
                    }
                    _ => {}
                }
            }
            ProviderType::Gemini => {
                let data_val: Value = serde_json::from_str(&ev.data).unwrap_or(Value::Null);
                record_stream_usage_if_present(
                    &runtime_metrics,
                    parse_usage_from_gemini_object(&data_val),
                )
                .await;
                let Some(candidate) = data_val
                    .get("candidates")
                    .and_then(|v| v.as_array())
                    .and_then(|arr| arr.first())
                else {
                    continue;
                };
                let Some(parts) = candidate
                    .get("content")
                    .and_then(|v| v.get("parts"))
                    .and_then(|v| v.as_array())
                else {
                    continue;
                };
                for part in parts {
                    if let Some(text) = part.get("text").and_then(|v| v.as_str()) {
                        if part.get("thought").and_then(|v| v.as_bool()) == Some(true) {
                            let chunk = json!({
                                "id": id,
                                "object": "chat.completion.chunk",
                                "created": created,
                                "model": urp.model,
                                "choices": [{ "index": 0, "delta": chat_reasoning_delta_from_text(text), "finish_reason": Value::Null }]
                            });
                            let _ = tx.send(Event::default().data(chunk.to_string())).await;
                            if let Some(sig) = part.get("thoughtSignature") {
                                let sig_text = sig
                                    .as_str()
                                    .map(|s| s.to_string())
                                    .unwrap_or_else(|| sig.to_string());
                                if !sig_text.is_empty() {
                                    let chunk = json!({
                                        "id": id,
                                        "object": "chat.completion.chunk",
                                        "created": created,
                                        "model": urp.model,
                                        "choices": [{ "index": 0, "delta": chat_reasoning_delta_from_signature(&sig_text), "finish_reason": Value::Null }]
                                    });
                                    let _ = tx.send(Event::default().data(chunk.to_string())).await;
                                }
                            }
                        } else if !text.is_empty() {
                            out_text.push_str(text);
                            let chunk = json!({
                                "id": id,
                                "object": "chat.completion.chunk",
                                "created": created,
                                "model": urp.model,
                                "choices": [{ "index": 0, "delta": { "content": text }, "finish_reason": Value::Null }]
                            });
                            let _ = tx.send(Event::default().data(chunk.to_string())).await;
                        }
                    }
                    if let Some(fc) = part.get("functionCall").and_then(|v| v.as_object()) {
                        let call_id = fc
                            .get("id")
                            .or_else(|| fc.get("name"))
                            .and_then(|v| v.as_str())
                            .unwrap_or("")
                            .to_string();
                        let name = fc
                            .get("name")
                            .and_then(|v| v.as_str())
                            .unwrap_or("")
                            .to_string();
                        let args =
                            serde_json::to_string(&fc.get("args").cloned().unwrap_or(Value::Null))
                                .unwrap_or_else(|_| "{}".to_string());
                        if !name.is_empty() {
                            if !call_order.contains(&call_id) {
                                call_order.push(call_id.clone());
                            }
                            saw_tool_call = true;
                            let idx = call_order.iter().position(|x| x == &call_id).unwrap_or(0);
                            let chunk = json!({
                                "id": id,
                                "object": "chat.completion.chunk",
                                "created": created,
                                "model": urp.model,
                                "choices": [{
                                    "index": 0,
                                    "delta": {
                                        "tool_calls": [{
                                            "index": idx,
                                            "id": call_id,
                                            "type": "function",
                                            "function": { "name": name, "arguments": args }
                                        }]
                                    },
                                    "finish_reason": Value::Null
                                }]
                            });
                            let _ = tx.send(Event::default().data(chunk.to_string())).await;
                        }
                    }
                }
            }
            ProviderType::Grok => {
                let data_val: Value = serde_json::from_str(&ev.data).unwrap_or(Value::Null);
                record_stream_usage_if_present(
                    &runtime_metrics,
                    parse_usage_from_responses_object(&data_val),
                )
                .await;
                match ev.event.as_str() {
                    "response.output_text.delta" => {
                        let t = data_val
                            .get("text")
                            .and_then(|v| v.as_str())
                            .or_else(|| data_val.get("delta").and_then(|v| v.as_str()))
                            .unwrap_or("");
                        if !t.is_empty() {
                            out_text.push_str(t);
                            let chunk = json!({
                                "id": id,
                                "object": "chat.completion.chunk",
                                "created": created,
                                "model": urp.model,
                                "choices": [{ "index": 0, "delta": { "content": t }, "finish_reason": Value::Null }]
                            });
                            let _ = tx.send(Event::default().data(chunk.to_string())).await;
                        }
                    }
                    "response.output_item.added" => {
                        let item = data_val.get("item").unwrap_or(&data_val);
                        if item.get("type").and_then(|v| v.as_str()) == Some("function_call") {
                            let call_id = item
                                .get("call_id")
                                .and_then(|v| v.as_str())
                                .unwrap_or("")
                                .to_string();
                            let name = item
                                .get("name")
                                .and_then(|v| v.as_str())
                                .unwrap_or("")
                                .to_string();
                            if !call_id.is_empty() {
                                if !call_order.contains(&call_id) {
                                    call_order.push(call_id.clone());
                                }
                                if !name.is_empty() {
                                    call_names.insert(call_id.clone(), name.clone());
                                }
                                if let Some(output_index) =
                                    data_val.get("output_index").and_then(|v| v.as_u64())
                                {
                                    call_ids_by_output_index.insert(output_index, call_id.clone());
                                }
                                let idx =
                                    call_order.iter().position(|x| x == &call_id).unwrap_or(0);
                                saw_tool_call = true;
                                let chunk = json!({
                                    "id": id,
                                    "object": "chat.completion.chunk",
                                    "created": created,
                                    "model": urp.model,
                                    "choices": [{
                                        "index": 0,
                                        "delta": {
                                            "tool_calls": [{
                                                "index": idx,
                                                "id": call_id,
                                                "type": "function",
                                                "function": { "name": name, "arguments": "" }
                                            }]
                                        },
                                        "finish_reason": Value::Null
                                    }]
                                });
                                let _ = tx.send(Event::default().data(chunk.to_string())).await;
                            }
                        }
                    }
                    "response.function_call_arguments.delta" => {
                        let call_id = data_val
                            .get("call_id")
                            .and_then(|v| v.as_str())
                            .map(|s| s.to_string())
                            .or_else(|| {
                                data_val
                                    .get("output_index")
                                    .and_then(|v| v.as_u64())
                                    .and_then(|idx| call_ids_by_output_index.get(&idx).cloned())
                            })
                            .unwrap_or_default();
                        if call_id.is_empty() {
                            continue;
                        }
                        if !call_order.contains(&call_id) {
                            call_order.push(call_id.clone());
                        }
                        let idx = call_order.iter().position(|x| x == &call_id).unwrap_or(0);
                        let name = data_val
                            .get("name")
                            .and_then(|v| v.as_str())
                            .map(|s| s.to_string())
                            .or_else(|| call_names.get(&call_id).cloned())
                            .unwrap_or_default();
                        let delta = data_val.get("delta").and_then(|v| v.as_str()).unwrap_or("");
                        saw_tool_call = true;
                        let chunk = json!({
                            "id": id,
                            "object": "chat.completion.chunk",
                            "created": created,
                            "model": urp.model,
                            "choices": [{
                                "index": 0,
                                "delta": {
                                    "tool_calls": [{
                                        "index": idx,
                                        "id": call_id,
                                        "type": "function",
                                        "function": { "name": name, "arguments": delta }
                                    }]
                                },
                                "finish_reason": Value::Null
                            }]
                        });
                        let _ = tx.send(Event::default().data(chunk.to_string())).await;
                    }
                    "response.reasoning_text.delta" => {
                        let t = data_val
                            .get("delta")
                            .and_then(|v| v.as_str())
                            .or_else(|| data_val.get("text").and_then(|v| v.as_str()))
                            .unwrap_or("");
                        if !t.is_empty() {
                            let chunk = json!({
                                "id": id,
                                "object": "chat.completion.chunk",
                                "created": created,
                                "model": urp.model,
                                "choices": [{ "index": 0, "delta": chat_reasoning_delta_from_text(t), "finish_reason": Value::Null }]
                            });
                            let _ = tx.send(Event::default().data(chunk.to_string())).await;
                        }
                    }
                    "response.reasoning_signature.delta" => {
                        let t = data_val.get("delta").and_then(|v| v.as_str()).unwrap_or("");
                        if !t.is_empty() {
                            let chunk = json!({
                                "id": id,
                                "object": "chat.completion.chunk",
                                "created": created,
                                "model": urp.model,
                                "choices": [{ "index": 0, "delta": chat_reasoning_delta_from_signature(t), "finish_reason": Value::Null }]
                            });
                            let _ = tx.send(Event::default().data(chunk.to_string())).await;
                        }
                    }
                    _ => {}
                }
            }
            ProviderType::Group => {}
        }
    }

    let finish_reason = if saw_tool_call { "tool_calls" } else { "stop" };
    let done = json!({
        "id": id,
        "object": "chat.completion.chunk",
        "created": created,
        "model": urp.model,
        "choices": [{ "index": 0, "delta": {}, "finish_reason": finish_reason }]
    });
    let _ = tx.send(Event::default().data(done.to_string())).await;
    let _ = tx.send(Event::default().data("[DONE]")).await;
    Ok(())
}

async fn stream_any_sse_as_messages(
    urp: &UrpRequest,
    provider_type: ProviderType,
    upstream_resp: reqwest::Response,
    tx: mpsc::Sender<Event>,
    started_at: Option<std::time::Instant>,
    runtime_metrics: Option<Arc<Mutex<StreamRuntimeMetrics>>>,
) -> AppResult<()> {
    let message_id = format!("msg_{}", uuid::Uuid::new_v4());
    // If the upstream is already Anthropic Messages streaming, forward it as-is.
    if provider_type == ProviderType::Messages {
        let mut stream = upstream_resp.bytes_stream().eventsource();
        while let Some(ev) = stream.next().await {
            let Ok(ev) = ev else { continue };
            mark_stream_ttfb_if_needed(started_at, &runtime_metrics).await;
            if ev.data.trim() == "[DONE]" {
                break;
            }
            let data_val: Value = serde_json::from_str(&ev.data).unwrap_or(Value::Null);
            record_stream_usage_if_present(
                &runtime_metrics,
                parse_usage_from_messages_object(&data_val),
            )
            .await;
            let _ = tx.send(Event::default().data(ev.data)).await;
        }
        return Ok(());
    }

    let start = json!({
        "type": "message_start",
        "message": {
            "id": message_id,
            "type": "message",
            "role": "assistant",
            "model": urp.model,
            "content": [],
            "stop_reason": Value::Null,
            "stop_sequence": Value::Null,
            "usage": {
                "input_tokens": 0,
                "output_tokens": 0
            }
        }
    });
    let _ = tx.send(Event::default().data(start.to_string())).await;

    let mut next_index: u32 = 0;
    let mut text_index: Option<u32> = None;
    let mut thinking_index: Option<u32> = None;
    let mut saw_responses_text_delta = false;
    let mut tool_indices: HashMap<String, u32> = HashMap::new();
    let mut tool_names: HashMap<String, String> = HashMap::new();
    let mut call_ids_by_output_index: HashMap<u64, String> = HashMap::new();
    let mut started: Vec<u32> = Vec::new();

    let mut stream = upstream_resp.bytes_stream().eventsource();
    while let Some(ev) = stream.next().await {
        let Ok(ev) = ev else { continue };
        mark_stream_ttfb_if_needed(started_at, &runtime_metrics).await;
        if ev.data.trim() == "[DONE]" {
            break;
        }

        match provider_type {
            ProviderType::ChatCompletion => {
                let data_val: Value = serde_json::from_str(&ev.data).unwrap_or(Value::Null);
                record_stream_usage_if_present(
                    &runtime_metrics,
                    parse_usage_from_chat_object(&data_val),
                )
                .await;
                let delta = data_val
                    .get("choices")
                    .and_then(|v| v.as_array())
                    .and_then(|arr| arr.first())
                    .and_then(|c| c.get("delta"))
                    .cloned()
                    .unwrap_or(Value::Null);

                let (reasoning_text_deltas, reasoning_sig_deltas) =
                    extract_chat_reasoning_deltas(&delta);
                for t in reasoning_text_deltas {
                    let idx = ensure_anthropic_thinking_block(
                        &tx,
                        &mut thinking_index,
                        &mut next_index,
                        &mut started,
                    )
                    .await?;
                    let d = json!({
                        "type": "content_block_delta",
                        "index": idx,
                        "delta": { "type": "thinking_delta", "thinking": t }
                    });
                    let _ = tx.send(Event::default().data(d.to_string())).await;
                }
                for s in reasoning_sig_deltas {
                    let idx = ensure_anthropic_thinking_block(
                        &tx,
                        &mut thinking_index,
                        &mut next_index,
                        &mut started,
                    )
                    .await?;
                    let d = json!({
                        "type": "content_block_delta",
                        "index": idx,
                        "delta": { "type": "signature_delta", "signature": s }
                    });
                    let _ = tx.send(Event::default().data(d.to_string())).await;
                }

                if let Some(t) = delta.get("content").and_then(|v| v.as_str()) {
                    if !t.is_empty() {
                        let idx = ensure_anthropic_text_block(
                            &tx,
                            &mut text_index,
                            &mut next_index,
                            &mut started,
                        )
                        .await?;
                        let d = json!({
                            "type": "content_block_delta",
                            "index": idx,
                            "delta": { "type": "text_delta", "text": t }
                        });
                        let _ = tx.send(Event::default().data(d.to_string())).await;
                    }
                }

                if let Some(tool_calls) = delta.get("tool_calls").and_then(|v| v.as_array()) {
                    for tc in tool_calls {
                        let call_id = tc
                            .get("id")
                            .or_else(|| tc.get("call_id"))
                            .and_then(|v| v.as_str())
                            .unwrap_or("");
                        if call_id.is_empty() {
                            continue;
                        }
                        let name = tc
                            .get("function")
                            .and_then(|f| f.get("name"))
                            .and_then(|v| v.as_str())
                            .unwrap_or("");
                        let args = tc
                            .get("function")
                            .and_then(|f| f.get("arguments"))
                            .and_then(|v| v.as_str())
                            .unwrap_or("");
                        let idx = ensure_anthropic_tool_block(
                            &tx,
                            &mut tool_indices,
                            &mut tool_names,
                            &mut next_index,
                            &mut started,
                            call_id,
                            name,
                        )
                        .await?;
                        if !args.is_empty() {
                            let d = json!({
                                "type": "content_block_delta",
                                "index": idx,
                                "delta": { "type": "input_json_delta", "partial_json": args }
                            });
                            let _ = tx.send(Event::default().data(d.to_string())).await;
                        }
                    }
                }
            }
            ProviderType::Responses | ProviderType::Grok => {
                let data_val: Value = serde_json::from_str(&ev.data).unwrap_or(Value::Null);
                record_stream_usage_if_present(
                    &runtime_metrics,
                    parse_usage_from_responses_object(&data_val),
                )
                .await;
                match ev.event.as_str() {
                    "response.output_text.delta" => {
                        let t = data_val
                            .get("text")
                            .and_then(|v| v.as_str())
                            .or_else(|| data_val.get("delta").and_then(|v| v.as_str()))
                            .unwrap_or("");
                        if !t.is_empty() {
                            saw_responses_text_delta = true;
                            let idx = ensure_anthropic_text_block(
                                &tx,
                                &mut text_index,
                                &mut next_index,
                                &mut started,
                            )
                            .await?;
                            let d = json!({ "type": "content_block_delta", "index": idx, "delta": { "type": "text_delta", "text": t } });
                            let _ = tx.send(Event::default().data(d.to_string())).await;
                        }
                    }
                    "response.reasoning_text.delta" => {
                        let t = data_val
                            .get("delta")
                            .and_then(|v| v.as_str())
                            .or_else(|| data_val.get("text").and_then(|v| v.as_str()))
                            .unwrap_or("");
                        if !t.is_empty() {
                            let idx = ensure_anthropic_thinking_block(
                                &tx,
                                &mut thinking_index,
                                &mut next_index,
                                &mut started,
                            )
                            .await?;
                            let d = json!({ "type": "content_block_delta", "index": idx, "delta": { "type": "thinking_delta", "thinking": t } });
                            let _ = tx.send(Event::default().data(d.to_string())).await;
                        }
                    }
                    "response.reasoning_signature.delta" => {
                        let t = data_val.get("delta").and_then(|v| v.as_str()).unwrap_or("");
                        if !t.is_empty() {
                            let idx = ensure_anthropic_thinking_block(
                                &tx,
                                &mut thinking_index,
                                &mut next_index,
                                &mut started,
                            )
                            .await?;
                            let d = json!({ "type": "content_block_delta", "index": idx, "delta": { "type": "signature_delta", "signature": t } });
                            let _ = tx.send(Event::default().data(d.to_string())).await;
                        }
                    }
                    "response.output_item.added" => {
                        let item = data_val.get("item").unwrap_or(&data_val);
                        if item.get("type").and_then(|v| v.as_str()) == Some("function_call") {
                            let call_id =
                                item.get("call_id").and_then(|v| v.as_str()).unwrap_or("");
                            let name = item.get("name").and_then(|v| v.as_str()).unwrap_or("");
                            if !call_id.is_empty() {
                                if let Some(output_index) =
                                    data_val.get("output_index").and_then(|v| v.as_u64())
                                {
                                    call_ids_by_output_index
                                        .insert(output_index, call_id.to_string());
                                }
                                let _ = ensure_anthropic_tool_block(
                                    &tx,
                                    &mut tool_indices,
                                    &mut tool_names,
                                    &mut next_index,
                                    &mut started,
                                    call_id,
                                    name,
                                )
                                .await?;
                            }
                        }
                    }
                    "response.function_call_arguments.delta" => {
                        let call_id = data_val
                            .get("call_id")
                            .and_then(|v| v.as_str())
                            .or_else(|| {
                                data_val
                                    .get("output_index")
                                    .and_then(|v| v.as_u64())
                                    .and_then(|idx| {
                                        call_ids_by_output_index.get(&idx).map(|s| s.as_str())
                                    })
                            })
                            .unwrap_or("");
                        let name = data_val.get("name").and_then(|v| v.as_str()).unwrap_or("");
                        let delta = data_val.get("delta").and_then(|v| v.as_str()).unwrap_or("");
                        if !call_id.is_empty() {
                            let idx = ensure_anthropic_tool_block(
                                &tx,
                                &mut tool_indices,
                                &mut tool_names,
                                &mut next_index,
                                &mut started,
                                call_id,
                                name,
                            )
                            .await?;
                            if !delta.is_empty() {
                                let d = json!({
                                    "type": "content_block_delta",
                                    "index": idx,
                                    "delta": { "type": "input_json_delta", "partial_json": delta }
                                });
                                let _ = tx.send(Event::default().data(d.to_string())).await;
                            }
                        }
                    }
                    "response.output_item.done" => {
                        let item = data_val.get("item").unwrap_or(&data_val);
                        if item.get("type").and_then(|v| v.as_str()) == Some("function_call") {
                            let call_id =
                                item.get("call_id").and_then(|v| v.as_str()).unwrap_or("");
                            let name = item.get("name").and_then(|v| v.as_str()).unwrap_or("");
                            let args = item.get("arguments").and_then(|v| v.as_str()).unwrap_or("");
                            if !call_id.is_empty() {
                                if let Some(output_index) =
                                    data_val.get("output_index").and_then(|v| v.as_u64())
                                {
                                    call_ids_by_output_index
                                        .insert(output_index, call_id.to_string());
                                }
                                let idx = ensure_anthropic_tool_block(
                                    &tx,
                                    &mut tool_indices,
                                    &mut tool_names,
                                    &mut next_index,
                                    &mut started,
                                    call_id,
                                    name,
                                )
                                .await?;
                                if !args.is_empty() {
                                    let d = json!({
                                        "type": "content_block_delta",
                                        "index": idx,
                                        "delta": { "type": "input_json_delta", "partial_json": args }
                                    });
                                    let _ = tx.send(Event::default().data(d.to_string())).await;
                                }
                            }
                        } else if item.get("type").and_then(|v| v.as_str()) == Some("message") {
                            if !saw_responses_text_delta {
                                let text = extract_responses_message_text(item);
                                if !text.is_empty() {
                                    let idx = ensure_anthropic_text_block(
                                        &tx,
                                        &mut text_index,
                                        &mut next_index,
                                        &mut started,
                                    )
                                    .await?;
                                    let d = json!({
                                        "type": "content_block_delta",
                                        "index": idx,
                                        "delta": { "type": "text_delta", "text": text }
                                    });
                                    let _ = tx.send(Event::default().data(d.to_string())).await;
                                }
                            }
                        } else if item.get("type").and_then(|v| v.as_str()) == Some("reasoning") {
                            let (reasoning_text, reasoning_sig) =
                                extract_reasoning_text_and_signature(item);
                            if !reasoning_text.is_empty() {
                                let idx = ensure_anthropic_thinking_block(
                                    &tx,
                                    &mut thinking_index,
                                    &mut next_index,
                                    &mut started,
                                )
                                .await?;
                                let d = json!({
                                    "type": "content_block_delta",
                                    "index": idx,
                                    "delta": { "type": "thinking_delta", "thinking": reasoning_text }
                                });
                                let _ = tx.send(Event::default().data(d.to_string())).await;
                            }
                            if !reasoning_sig.is_empty() {
                                let idx = ensure_anthropic_thinking_block(
                                    &tx,
                                    &mut thinking_index,
                                    &mut next_index,
                                    &mut started,
                                )
                                .await?;
                                let d = json!({
                                    "type": "content_block_delta",
                                    "index": idx,
                                    "delta": { "type": "signature_delta", "signature": reasoning_sig }
                                });
                                let _ = tx.send(Event::default().data(d.to_string())).await;
                            }
                        }
                    }
                    _ => {}
                }
            }
            ProviderType::Gemini | ProviderType::Group | ProviderType::Messages => {}
        }
    }

    // Close all started blocks in the order they were created.
    for idx in started.iter().copied() {
        let stop = json!({ "type": "content_block_stop", "index": idx });
        let _ = tx.send(Event::default().data(stop.to_string())).await;
    }
    let message_delta = json!({
        "type": "message_delta",
        "delta": {
            "stop_reason": if tool_indices.is_empty() { "end_turn" } else { "tool_use" },
            "stop_sequence": Value::Null
        },
        "usage": {
            "input_tokens": 0,
            "output_tokens": 0
        }
    });
    let _ = tx
        .send(Event::default().data(message_delta.to_string()))
        .await;
    let stop = json!({ "type": "message_stop" });
    let _ = tx.send(Event::default().data(stop.to_string())).await;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::{calculate_charge_nano, resolve_upstream_model, scale_charge_with_multiplier};
    use crate::model_registry_store::ModelPricing;
    use crate::monoize_routing::MonoizeModelEntry;
    use crate::urp;
    use std::collections::HashMap;

    #[test]
    fn calculate_charge_nano_uses_model_price_and_multiplier() {
        let usage = urp::Usage {
            prompt_tokens: 15,
            completion_tokens: 5,
            reasoning_tokens: None,
            cached_tokens: Some(0),
            extra_body: HashMap::new(),
        };
        let pricing = ModelPricing {
            input_cost_per_token_nano: 2500,
            output_cost_per_token_nano: 10000,
            cache_read_input_cost_per_token_nano: None,
            output_cost_per_reasoning_token_nano: None,
        };

        let charged = calculate_charge_nano(&usage, &pricing, 1.234_567_891);

        assert_eq!(charged, Some(108_024));
    }

    #[test]
    fn calculate_charge_nano_handles_cached_and_reasoning_tokens() {
        let usage = urp::Usage {
            prompt_tokens: 100,
            completion_tokens: 80,
            reasoning_tokens: Some(30),
            cached_tokens: Some(60),
            extra_body: HashMap::new(),
        };
        let pricing = ModelPricing {
            input_cost_per_token_nano: 1000,
            output_cost_per_token_nano: 2000,
            cache_read_input_cost_per_token_nano: Some(100),
            output_cost_per_reasoning_token_nano: Some(3000),
        };

        let charged = calculate_charge_nano(&usage, &pricing, 1.0);

        assert_eq!(charged, Some(236_000));
    }

    #[test]
    fn scale_charge_quantizes_multiplier_to_nano_precision() {
        let base = 1_000_000_000i128;
        let charged = scale_charge_with_multiplier(base, 1.000_000_000_9);
        assert_eq!(charged, Some(1_000_000_000));
    }

    #[test]
    fn resolve_upstream_model_prefers_non_empty_redirect() {
        let entry = MonoizeModelEntry {
            redirect: Some("  gpt-5-target  ".to_string()),
            multiplier: 1.0,
        };
        assert_eq!(
            resolve_upstream_model("gpt-5-logical", &entry),
            "gpt-5-target".to_string()
        );
    }

    #[test]
    fn resolve_upstream_model_falls_back_to_requested_when_redirect_blank() {
        let entry = MonoizeModelEntry {
            redirect: Some("   ".to_string()),
            multiplier: 1.0,
        };
        assert_eq!(
            resolve_upstream_model("gpt-5-logical", &entry),
            "gpt-5-logical".to_string()
        );
    }
}
