mod account_balance;
mod billing;
mod compact;
pub(crate) mod helpers;
pub(crate) mod image_api;
mod nonstream;
#[cfg(test)]
pub(crate) use nonstream::strip_orphaned_tool_calls;
mod request_logging;
mod responses_websocket;
pub(crate) mod routing;
mod streaming;
pub(crate) mod usage;

#[cfg(test)]
mod tests;

use crate::app::AppState;
use crate::config::{ProviderAuthConfig, ProviderAuthType, ProviderConfig, ProviderType};
use crate::error::{AppError, AppResult};
use crate::exact_decimal::Multiplier;
use crate::request_capture::RequestCaptureSession;
use crate::settings::normalize_pricing_model_key;
use crate::transforms::{self, Phase, TransformRuleConfig};
use crate::upstream::{self, UpstreamCallError, UpstreamErrorKind};
use crate::urp;
use crate::users::BillingErrorKind;
use crate::users::{
    InsertRequestLog, REQUEST_LOG_STATUS_CLIENT_GONE, REQUEST_LOG_STATUS_ERROR,
    REQUEST_LOG_STATUS_SUCCESS,
};
use axum::Json;
use axum::extract::State;
use axum::http::{HeaderMap, StatusCode};
use axum::response::sse::{Event, KeepAlive};
use axum::response::{IntoResponse, Response, Sse};
use serde_json::{Map, Value, json};
use std::collections::{HashMap, HashSet};
use std::sync::Arc;
use std::time::{Duration, Instant};
use tokio::sync::{Mutex, mpsc};

use billing::*;
use helpers::*;
use nonstream::*;
use request_logging::*;
use routing::*;
use streaming::*;
use usage::*;

pub use account_balance::{codex_usage, deepseek_user_balance};
pub use compact::compact_response;
pub use responses_websocket::responses_websocket;

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

fn apply_first_model_redirect(
    model: &mut String,
    rules: &[crate::users::CompiledModelRedirectRule],
) -> bool {
    for rule in rules {
        if rule.is_match(model) {
            *model = rule.replace().to_string();
            return true;
        }
    }
    false
}

fn apply_model_redirects_to_model(
    model: &mut String,
    api_key_rules: &[crate::users::CompiledModelRedirectRule],
    global_rules: &[crate::users::CompiledModelRedirectRule],
) {
    if !apply_first_model_redirect(model, api_key_rules) {
        apply_first_model_redirect(model, global_rules);
    }
}

async fn apply_configured_model_redirects_to_model(
    state: &AppState,
    model: &mut String,
    auth: &crate::auth::AuthResult,
) {
    let runtime = state.monoize_runtime.read().await;
    apply_model_redirects_to_model(
        model,
        &auth.model_redirects,
        &runtime.compiled_global_model_redirects,
    );
}

async fn apply_model_redirects(
    state: &AppState,
    req: &mut urp::UrpRequest,
    auth: &crate::auth::AuthResult,
) {
    apply_configured_model_redirects_to_model(state, &mut req.model, auth).await;
}

pub async fn metrics(State(state): State<AppState>) -> impl IntoResponse {
    state.metrics.render()
}

fn api_stream_keep_alive() -> KeepAlive {
    KeepAlive::new()
        .interval(Duration::from_secs(15))
        .text("heartbeat")
}

struct DownstreamGone(std::sync::Arc<AdmittedRequestTaskState>);

impl Drop for DownstreamGone {
    fn drop(&mut self) {
        self.0.mark_client_gone();
    }
}

async fn join_while_client_present<T: Send + 'static>(
    handle: tokio::task::JoinHandle<T>,
    watch: DownstreamGone,
) -> T {
    match handle.await {
        Ok(value) => {
            std::mem::forget(watch);
            value
        }
        Err(err) if err.is_panic() => {
            std::mem::forget(watch);
            std::panic::resume_unwind(err.into_panic())
        }
        Err(_) => {
            std::mem::forget(watch);
            panic!("request worker was cancelled")
        }
    }
}

fn messages_stream_keep_alive() -> KeepAlive {
    KeepAlive::new()
        .interval(Duration::from_secs(15))
        .event(Event::default().event("ping").data(r#"{"type":"ping"}"#))
}

const CODEX_BASE_INSTRUCTIONS: &str = "You are Codex, a coding agent. Follow the user's instructions and repository guidance. Inspect the workspace before editing, preserve unrelated changes, make scoped changes, and verify completed work with the tools provided by the client.";

fn codex_model_descriptor(model_id: &str, priority: usize) -> Value {
    // These conservative values match Codex's unknown-model fallback rather than claiming
    // provider-specific capabilities that Monoize cannot prove from a logical model ID.
    json!({
        "slug": model_id,
        "display_name": model_id,
        "description": "Routed by Monoize.",
        "default_reasoning_level": null,
        "supported_reasoning_levels": [],
        "shell_type": "default",
        "visibility": "list",
        "supported_in_api": true,
        "priority": priority,
        "additional_speed_tiers": [],
        "service_tiers": [],
        "default_service_tier": null,
        "availability_nux": null,
        "upgrade": null,
        "base_instructions": CODEX_BASE_INSTRUCTIONS,
        "model_messages": null,
        "include_skills_usage_instructions": false,
        "supports_reasoning_summary_parameter": true,
        "default_reasoning_summary": "auto",
        "support_verbosity": false,
        "default_verbosity": null,
        "apply_patch_tool_type": null,
        "web_search_tool_type": "text",
        "truncation_policy": {
            "mode": "bytes",
            "limit": 10_000
        },
        "supports_parallel_tool_calls": false,
        "supports_image_detail_original": false,
        "context_window": 272_000,
        "max_context_window": 272_000,
        "auto_compact_token_limit": null,
        "comp_hash": null,
        "effective_context_window_percent": 95,
        "experimental_supported_tools": [],
        "input_modalities": ["text", "image"],
        "supports_search_tool": false,
        "use_responses_lite": false,
        "auto_review_model_override": null,
        "tool_mode": null,
        "multi_agent_version": null
    })
}

pub async fn list_models(State(state): State<AppState>, headers: HeaderMap) -> AppResult<Response> {
    let auth = auth_tenant(&headers, &state).await?;
    let mut model_ids = state
        .monoize_store
        .list_available_model_names()
        .await
        .map_err(|e| AppError::new(StatusCode::INTERNAL_SERVER_ERROR, "provider_store_error", e))?;

    if auth.model_limits_enabled && !auth.model_limits.is_empty() {
        let allowed: HashSet<&str> = auth.model_limits.iter().map(|s| s.as_str()).collect();
        model_ids.retain(|id| allowed.contains(id.as_str()));
    }

    let codex_model_ids = state.monoize_runtime.read().await.codex_model_ids.clone();
    let visible_model_ids: HashSet<&str> = model_ids.iter().map(String::as_str).collect();
    let codex_models: Vec<Value> = codex_model_ids
        .iter()
        .filter(|model_id| visible_model_ids.contains(model_id.as_str()))
        .enumerate()
        .map(|(priority, model_id)| codex_model_descriptor(model_id, priority))
        .collect();

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

    Ok(Json(json!({
        "object": "list",
        "data": data,
        "models": codex_models
    }))
    .into_response())
}

pub async fn create_response(
    State(state): State<AppState>,
    headers: HeaderMap,
    Json(body): Json<Value>,
) -> AppResult<Response> {
    let auth = auth_tenant(&headers, &state).await?;
    let request_id = extract_request_id(&headers);
    // Session starts before decode so the body is deep-cloned only when a
    // capture session exists. The top-level "stream" bool is exactly what the
    // decoder assigns to `req.stream` for this protocol.
    let capture_session = state
        .request_capture
        .maybe_start_session(
            &state.monoize_runtime,
            &auth,
            request_id.clone(),
            DownstreamProtocol::Responses,
            body.get("stream").and_then(Value::as_bool).unwrap_or(false),
        )
        .await;
    let raw_input = Arc::new(if capture_session.is_some() {
        body.clone()
    } else {
        Value::Null
    });
    let (known, extra) = split_body(body, &URP_KNOWN_RESPONSE_FIELDS)?;
    let mut req = decode_urp_request(DownstreamProtocol::Responses, known, extra)?;
    apply_model_redirects(&state, &mut req, &auth).await;
    ensure_model_allowed(&auth, &req.model)?;
    let max_multiplier = resolve_max_multiplier(&req, &headers, &auth);
    let request_ip = extract_client_ip(&headers);
    let capture = RequestCaptureContext {
        raw_input,
        session: capture_session,
    };
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
        let stream = deferred_forward_event_stream(
            downstream,
            forward_stream_typed(
                state.clone(),
                auth.clone(),
                req,
                max_multiplier,
                downstream,
                request_id.clone(),
                request_ip.clone(),
                extract_client_session_id(&headers),
                capture.clone(),
            ),
        );
        return Ok(Sse::new(stream)
            .keep_alive(api_stream_keep_alive())
            .into_response());
    }

    let session_id = extract_client_session_id(&headers);
    let task_state = std::sync::Arc::new(AdmittedRequestTaskState::new(std::time::Instant::now()));
    let watch = DownstreamGone(task_state.clone());
    let handle = tokio::spawn({
        let task_state = task_state.clone();
        async move {
            forward_nonstream_typed_with_task_state(
                &state,
                &auth,
                req,
                max_multiplier,
                DownstreamProtocol::Responses,
                request_id,
                request_ip,
                session_id,
                capture,
                Some(&task_state),
            )
            .await
        }
    });
    let value = join_while_client_present(handle, watch).await?;
    Ok(Json(value).into_response())
}

pub async fn create_chat_completions(
    State(state): State<AppState>,
    headers: HeaderMap,
    Json(body): Json<Value>,
) -> AppResult<Response> {
    let auth = auth_tenant(&headers, &state).await?;
    let request_id = extract_request_id(&headers);
    let capture_session = state
        .request_capture
        .maybe_start_session(
            &state.monoize_runtime,
            &auth,
            request_id.clone(),
            DownstreamProtocol::ChatCompletions,
            body.get("stream").and_then(Value::as_bool).unwrap_or(false),
        )
        .await;
    let raw_input = Arc::new(if capture_session.is_some() {
        body.clone()
    } else {
        Value::Null
    });
    let (known, extra) = split_body(body, &URP_KNOWN_CHAT_FIELDS)?;
    let mut req = decode_urp_request(DownstreamProtocol::ChatCompletions, known, extra)?;
    apply_model_redirects(&state, &mut req, &auth).await;
    ensure_model_allowed(&auth, &req.model)?;
    let max_multiplier = resolve_max_multiplier(&req, &headers, &auth);
    let request_ip = extract_client_ip(&headers);
    let capture = RequestCaptureContext {
        raw_input,
        session: capture_session,
    };
    if req.stream.unwrap_or(false) {
        let downstream = DownstreamProtocol::ChatCompletions;
        let stream = deferred_forward_event_stream(
            downstream,
            forward_stream_typed(
                state.clone(),
                auth.clone(),
                req,
                max_multiplier,
                downstream,
                request_id.clone(),
                request_ip.clone(),
                extract_client_session_id(&headers),
                capture.clone(),
            ),
        );
        return Ok(Sse::new(stream)
            .keep_alive(api_stream_keep_alive())
            .into_response());
    }
    let session_id = extract_client_session_id(&headers);
    let task_state = std::sync::Arc::new(AdmittedRequestTaskState::new(std::time::Instant::now()));
    let watch = DownstreamGone(task_state.clone());
    let handle = tokio::spawn({
        let task_state = task_state.clone();
        async move {
            forward_nonstream_typed_with_task_state(
                &state,
                &auth,
                req,
                max_multiplier,
                DownstreamProtocol::ChatCompletions,
                request_id,
                request_ip,
                session_id,
                capture,
                Some(&task_state),
            )
            .await
        }
    });
    let value = join_while_client_present(handle, watch).await?;
    Ok(Json(value).into_response())
}

pub async fn create_messages(
    State(state): State<AppState>,
    headers: HeaderMap,
    Json(body): Json<Value>,
) -> Response {
    let request_id = extract_request_id(&headers);
    match create_messages_inner(state, headers, body).await {
        Ok(response) => response,
        Err(err) => anthropic_error_response(err, request_id.as_deref()),
    }
}

async fn create_messages_inner(
    state: AppState,
    headers: HeaderMap,
    body: Value,
) -> AppResult<Response> {
    let auth = auth_tenant(&headers, &state).await?;
    let request_id = extract_request_id(&headers);
    let capture_session = state
        .request_capture
        .maybe_start_session(
            &state.monoize_runtime,
            &auth,
            request_id.clone(),
            DownstreamProtocol::AnthropicMessages,
            body.get("stream").and_then(Value::as_bool).unwrap_or(false),
        )
        .await;
    let raw_input = Arc::new(if capture_session.is_some() {
        body.clone()
    } else {
        Value::Null
    });
    let (known, extra) = split_body(body, &URP_KNOWN_MESSAGES_FIELDS)?;
    let mut req = decode_urp_request(DownstreamProtocol::AnthropicMessages, known, extra)?;
    apply_model_redirects(&state, &mut req, &auth).await;
    ensure_model_allowed(&auth, &req.model)?;
    let max_multiplier = resolve_max_multiplier(&req, &headers, &auth);
    let request_ip = extract_client_ip(&headers);
    let capture = RequestCaptureContext {
        raw_input,
        session: capture_session,
    };
    if req.stream.unwrap_or(false) {
        let downstream = DownstreamProtocol::AnthropicMessages;
        let stream = deferred_forward_event_stream(
            downstream,
            forward_stream_typed(
                state.clone(),
                auth.clone(),
                req,
                max_multiplier,
                downstream,
                request_id.clone(),
                request_ip.clone(),
                extract_client_session_id(&headers),
                capture.clone(),
            ),
        );
        return Ok(Sse::new(stream)
            .keep_alive(messages_stream_keep_alive())
            .into_response());
    }
    let session_id = extract_client_session_id(&headers);
    let task_state = std::sync::Arc::new(AdmittedRequestTaskState::new(std::time::Instant::now()));
    let watch = DownstreamGone(task_state.clone());
    let handle = tokio::spawn({
        let task_state = task_state.clone();
        async move {
            forward_nonstream_typed_with_task_state(
                &state,
                &auth,
                req,
                max_multiplier,
                DownstreamProtocol::AnthropicMessages,
                request_id,
                request_ip,
                session_id,
                capture,
                Some(&task_state),
            )
            .await
        }
    });
    let value = join_while_client_present(handle, watch).await?;
    Ok(Json(value).into_response())
}

fn anthropic_error_response(err: AppError, request_id: Option<&str>) -> Response {
    let error_type = err
        .upstream_type
        .as_deref()
        .unwrap_or(&err.error_type)
        .to_string();
    let mut error = json!({
        "type": error_type,
        "message": err.message,
    });
    if let Some(obj) = error.as_object_mut() {
        if let Some(status) = err.upstream_status {
            obj.insert("upstream_status".to_string(), Value::from(status));
        }
        if let Some(code) = err.upstream_code {
            obj.insert("upstream_code".to_string(), Value::String(code));
        }
        if let Some(error_type) = err.upstream_type {
            obj.insert("upstream_type".to_string(), Value::String(error_type));
        }
        if let Some(param) = err.upstream_param {
            obj.insert("upstream_param".to_string(), Value::String(param));
        }
    }
    let mut body = json!({ "type": "error", "error": error });
    if let Some(request_id) = request_id.filter(|value| !value.is_empty()) {
        body.as_object_mut().expect("Anthropic error body").insert(
            "request_id".to_string(),
            Value::String(request_id.to_string()),
        );
    }

    let mut response = (err.status, Json(body)).into_response();
    if let Some(request_id) = request_id
        && let Ok(value) = axum::http::HeaderValue::from_str(request_id)
    {
        response
            .headers_mut()
            .insert(axum::http::HeaderName::from_static("request-id"), value);
    }
    response
}

pub async fn create_embeddings(
    State(state): State<AppState>,
    headers: HeaderMap,
    Json(body): Json<Value>,
) -> AppResult<Response> {
    let auth = auth_tenant(&headers, &state).await?;

    let obj = body.as_object().ok_or_else(|| {
        AppError::new(
            StatusCode::BAD_REQUEST,
            "invalid_request",
            "body must be object",
        )
    })?;

    let mut logical_model = obj
        .get("model")
        .and_then(|v| v.as_str())
        .filter(|s| !s.trim().is_empty())
        .ok_or_else(|| AppError::new(StatusCode::BAD_REQUEST, "invalid_request", "missing model"))?
        .to_string();
    apply_configured_model_redirects_to_model(&state, &mut logical_model, &auth).await;
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
    let mut attempts = build_monoize_attempts(&state, &routing_stub, &auth).await?;
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
            let mut upstream_body = body.clone();
            if let Some(upstream_obj) = upstream_body.as_object_mut() {
                upstream_obj.insert(
                    "model".to_string(),
                    Value::String(attempt.upstream_model.clone()),
                );
            }

            let provider = build_channel_provider_config(&attempt);
            let http = client_http_for_attempt(&state, &attempt)?;
            let result = upstream::call_upstream_with_timeout_and_headers(
                &http,
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
                    let mut usage = parse_usage_from_embeddings_object(&value);
                    substitute_zero_usage_if_allowed(&mut usage, &attempt);
                    let response_service_tier =
                        usage::response_service_tier(&value).map(str::to_string);
                    let charge = match usage.as_ref() {
                        Some(usage_row) => {
                            mark_channel_success(&state, &attempt).await;
                            match maybe_charge_usage(
                                &state,
                                &auth,
                                &attempt,
                                &logical_model,
                                usage_row,
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
                            }
                        }
                        None => {
                            let err = AppError::new(
                                StatusCode::BAD_GATEWAY,
                                "upstream_usage_required",
                                "upstream response did not include billable usage",
                            );
                            let same_channel_retryable = is_same_channel_retryable_app_error(&err);
                            let passive_failure_class = same_channel_retryable
                                .then(|| classify_retryable_app_failure(&err));
                            record_upstream_attempt_failure(
                                &state,
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
                        false,
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
    pub(crate) max_multiplier: Option<Multiplier>,
    pub(crate) server_tool_usage_classes: Vec<String>,
    pub(crate) affinity_explicit: Option<String>,
    pub(crate) affinity_prefix_hash: String,
}

#[derive(Clone, Debug)]
struct MonoizeAttempt {
    provider_id: String,
    provider_name: String,
    provider_type: ProviderType,
    channel_id: String,
    channel_name: String,
    base_url: String,
    api_key: String,
    logical_model: String,
    upstream_model: String,
    model_multiplier: Multiplier,
    server_tool_usage_classes: Vec<String>,
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
    strip_cross_protocol_nested_extra: bool,
    billable_pricing_available: bool,
    billing_rate_resolution: Option<billing::BillingRateResolution>,
    affinity_key: Option<String>,
    affinity_key_hash: Option<String>,
    affinity_hit: Option<bool>,
    affinity_target: Option<String>,
    affinity_enabled: bool,
    affinity_idle_ttl_seconds: u64,
    affinity_failback_mode: crate::monoize_routing::AffinityFailbackMode,
    affinity_failback_delay_seconds: u64,
    routing_config_revision: u64,
    /// PX6: per-Channel egress proxy override (None = follow node-global).
    proxy_url: Option<String>,
    /// CP-INV-15: static upstream headers configured on the Channel.
    extra_headers: Option<std::collections::BTreeMap<String, String>>,
    /// CM-AFF-2: derive per-request session affinity for this Channel.
    session_affinity_auto: bool,
    allow_missing_usage: bool,
    /// CM-AFF-1a/1b: client header or decoded-body conversation identifier.
    client_session_id: Option<String>,
    /// CM-AFF-2 rule 2: `mono-*` digest of instructions plus the first two
    /// input nodes, computed from the decoded request so tool lists cannot
    /// split one conversation across upstream instances.
    derived_session_affinity: Option<String>,
    /// CM-AFF-4: the effective `x-session-affinity` value sent upstream, for
    /// request-log persistence.
    session_affinity_value: Option<String>,
    /// RTA-6c: `scheme://host:port` from Channel `base_url`, when parseable.
    origin_key: Option<String>,
    /// Enabled positive-weight Channels of this Provider that share `origin_key`.
    origin_peer_channel_ids: Vec<String>,
}

fn reasoning_envelope_provider_type(provider_type: ProviderType) -> &'static str {
    match provider_type {
        ProviderType::Responses => "responses",
        ProviderType::ChatCompletion => "chat_completion",
        ProviderType::Messages => "messages",
        ProviderType::Gemini => "gemini",
        ProviderType::OpenaiImage => "openai_image",
        ProviderType::Replicate => "replicate",
        ProviderType::Group => "group",
    }
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

#[derive(Clone, Debug, serde::Serialize)]
struct TriedProvider {
    attempt_number: u32,
    provider_id: String,
    channel_id: String,
    provider_name: String,
    channel_name: String,
    /// SAN-5: unmasked, truncated internal detail persisted to
    /// `tried_providers_json` (RL17). Admins read it verbatim; non-admin
    /// dashboard reads mask it at read time (SAN-14).
    error: String,
    /// SAN-5: client-facing text used by the exhausted downstream error.
    /// Never persisted.
    #[serde(skip)]
    client_error: String,
    upstream_status: Option<u16>,
    upstream_code: Option<String>,
    upstream_type: Option<String>,
    upstream_param: Option<String>,
    duration_ms: Option<u64>,
}

impl TriedProvider {
    fn from_app_error(
        attempt_number: u32,
        attempt: &MonoizeAttempt,
        app_err: &AppError,
        duration_ms: Option<u64>,
        mask_sensitive_info: bool,
    ) -> Self {
        Self {
            attempt_number,
            provider_id: attempt.provider_id.clone(),
            channel_id: attempt.channel_id.clone(),
            provider_name: attempt.provider_name.clone(),
            channel_name: attempt.channel_name.clone(),
            // SAN-5: `error` is the unmasked internal detail. Attempt
            // failures can also come from response-decoding AppErrors that
            // never pass through `upstream_error_to_app`, so `client_error`
            // masks unconditionally when masking is enabled (idempotent)
            // while `error` keeps the raw text for admin-tier request-log
            // reads. SAN-CFG5 item 1: masking off makes `MASK` the identity.
            error: app_err
                .internal_message
                .clone()
                .unwrap_or_else(|| crate::error_sanitize::truncate_error_detail(&app_err.message)),
            client_error: crate::error_sanitize::maybe_mask_sensitive_text(
                &app_err.message,
                mask_sensitive_info,
            ),
            upstream_status: Some(app_err.upstream_status.unwrap_or(app_err.status.as_u16())),
            upstream_code: Some(
                app_err
                    .upstream_code
                    .clone()
                    .unwrap_or_else(|| app_err.code.clone()),
            ),
            upstream_type: app_err.upstream_type.clone(),
            upstream_param: app_err.upstream_param.clone(),
            duration_ms,
        }
    }
}

fn shared_origin_skip_token(provider_id: &str, origin_key: &str) -> String {
    format!("{provider_id}\0{origin_key}")
}

#[derive(Default)]
struct AttemptExecutionState {
    provider_attempts_used: HashMap<String, usize>,
    next_attempt_number: u32,
    shared_origin_skips: HashSet<String>,
    current_attempt_started: Option<Instant>,
}

impl AttemptExecutionState {
    fn provider_budget_remaining(&self, attempt: &MonoizeAttempt) -> bool {
        attempt
            .provider_attempt_limit
            .map(|limit| {
                self.provider_attempts_used
                    .get(&attempt.provider_id)
                    .copied()
                    .unwrap_or(0)
                    < limit
            })
            .unwrap_or(true)
    }

    fn skip_shared_origin(&self, attempt: &MonoizeAttempt) -> bool {
        let Some(origin_key) = attempt.origin_key.as_ref() else {
            return false;
        };
        self.shared_origin_skips
            .contains(&shared_origin_skip_token(&attempt.provider_id, origin_key))
    }

    fn mark_shared_origin_skip(&mut self, attempt: &MonoizeAttempt) {
        let Some(origin_key) = attempt.origin_key.as_ref() else {
            return;
        };
        self.shared_origin_skips
            .insert(shared_origin_skip_token(&attempt.provider_id, origin_key));
    }

    fn should_skip(&self, attempt: &MonoizeAttempt) -> bool {
        !self.provider_budget_remaining(attempt) || self.skip_shared_origin(attempt)
    }

    fn record_upstream_attempt(&mut self, attempt: &MonoizeAttempt) -> u32 {
        self.current_attempt_started = Some(Instant::now());
        let used = self
            .provider_attempts_used
            .entry(attempt.provider_id.clone())
            .or_default();
        *used = used.saturating_add(1);
        self.next_attempt_number = self.next_attempt_number.saturating_add(1);
        self.next_attempt_number
    }

    fn last_attempt_duration_ms(&self) -> Option<u64> {
        self.current_attempt_started
            .map(|started| u64::try_from(started.elapsed().as_millis()).unwrap_or(u64::MAX))
    }
}

#[derive(Clone, Copy)]
pub(crate) enum DownstreamProtocol {
    Responses,
    ChatCompletions,
    AnthropicMessages,
}

/// Capture context for one downstream request. `raw_input` holds the parsed
/// downstream body only when a capture session actually started; otherwise it
/// is `Value::Null` so the normal (capture-off) path never deep-clones the
/// request body. The `Arc` keeps the per-attempt context clones cheap.
#[derive(Clone)]
pub(crate) struct RequestCaptureContext {
    raw_input: Arc<Value>,
    session: Option<RequestCaptureSession>,
}

impl DownstreamProtocol {
    pub(crate) fn is_same_family(self, upstream: ProviderType) -> bool {
        matches!(
            (self, upstream),
            (Self::Responses, ProviderType::Responses)
                | (Self::ChatCompletions, ProviderType::ChatCompletion)
                | (Self::AnthropicMessages, ProviderType::Messages)
        )
    }
}

pub(crate) fn provider_type_protocol(provider_type: ProviderType) -> Option<urp::ProviderProtocol> {
    match provider_type {
        ProviderType::Responses => Some(urp::ProviderProtocol::Responses),
        ProviderType::ChatCompletion => Some(urp::ProviderProtocol::ChatCompletion),
        ProviderType::Messages => Some(urp::ProviderProtocol::Messages),
        ProviderType::Gemini => Some(urp::ProviderProtocol::Gemini),
        ProviderType::OpenaiImage => Some(urp::ProviderProtocol::OpenaiImage),
        ProviderType::Replicate => Some(urp::ProviderProtocol::Replicate),
        ProviderType::Group => None,
    }
}

#[derive(Clone, Debug, Default, serde::Serialize)]
pub(crate) struct StreamTerminalDiagnostics {
    pub(crate) saw_done_sentinel: bool,
    pub(crate) terminal_event: Option<String>,
    pub(crate) terminal_finish_reason: Option<String>,
    pub(crate) synthetic_terminal_emitted: bool,
    pub(crate) terminal_error: Option<StreamTerminalError>,
}

#[derive(Clone, Debug, serde::Serialize)]
pub(crate) struct StreamTerminalError {
    pub(crate) code: String,
    pub(crate) message: String,
    pub(crate) http_status: u16,
    pub(crate) error_type: Option<String>,
    pub(crate) param: Option<String>,
}

#[derive(Default)]
pub(crate) struct StreamRuntimeMetrics {
    ttfb_ms: Option<u64>,
    usage: Option<urp::Usage>,
    response_id: Option<String>,
    response_service_tier: Option<String>,
    terminal: StreamTerminalDiagnostics,
    pub(crate) estimated_output_tokens: u64,
    // Feeds the usage-less billing estimate only; TPS is derived at display
    // time from `output_tokens`, `duration_ms`, and `ttfb_ms` (request-logs
    // spec FL4a).
    visible_output_bytes: u64,
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
    } else if headers
        .get("x-monoize-internal-source")
        .and_then(|value| value.to_str().ok())
        .is_some_and(|value| value == "playground")
    {
        return authenticate_playground_session(headers, state).await;
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

async fn authenticate_playground_session(
    headers: &HeaderMap,
    state: &AppState,
) -> AppResult<crate::auth::AuthResult> {
    let user = crate::dashboard_handlers::session_helpers::get_current_user(headers, state).await?;
    let selected_group = match headers.get("x-monoize-playground-group") {
        Some(value) => Some(
            value
                .to_str()
                .map_err(|_| {
                    AppError::new(
                        StatusCode::BAD_REQUEST,
                        "invalid_playground_group",
                        "playground group header is not valid text",
                    )
                })?
                .trim()
                .to_string(),
        )
        .filter(|value| !value.is_empty()),
        None => None,
    };

    let group_id = selected_group
        .clone()
        .unwrap_or_else(|| user.group_id.clone());
    let group = state
        .user_store
        .get_group_by_id(&group_id)
        .await
        .map_err(|error| AppError::new(StatusCode::INTERNAL_SERVER_ERROR, "internal_error", error))?
        .ok_or_else(|| {
            AppError::new(
                StatusCode::FORBIDDEN,
                "playground_group_forbidden",
                "playground routing group is unavailable",
            )
        })?;

    if selected_group.is_some()
        && !user.role.can_manage_users()
        && !group.user_selectable
        && group.id != user.group_id
    {
        return Err(AppError::new(
            StatusCode::FORBIDDEN,
            "playground_group_forbidden",
            "playground routing group is not selectable",
        ));
    }

    if let Some(plan_id) = user.billing_plan_id.as_deref() {
        let plan = state
            .user_store
            .get_billing_plan_by_id(plan_id)
            .await
            .map_err(|error| {
                AppError::new(StatusCode::INTERNAL_SERVER_ERROR, "internal_error", error)
            })?;
        if let Some(plan) = plan.filter(|plan| plan.enabled && !plan.group_ids.is_empty()) {
            if !plan.group_ids.iter().any(|allowed| allowed == &group_id) {
                return Err(AppError::new(
                    StatusCode::FORBIDDEN,
                    "playground_group_forbidden",
                    "playground routing group is excluded by the billing plan",
                ));
            }
        }
    }

    Ok(crate::auth::AuthResult {
        tenant_id: user.id.clone(),
        user_id: Some(user.id),
        username: Some(user.username),
        user_role: user.role,
        api_key_id: None,
        api_key_name: None,
        internal_source: Some(crate::auth::InternalRequestSource::Playground),
        max_multiplier: None,
        transforms: Vec::new(),
        model_redirects: Vec::new(),
        effective_groups: Some(vec![group_id]),
        model_limits_enabled: false,
        model_limits: Vec::new(),
        ip_whitelist: Vec::new(),
        sub_account_enabled: false,
        sub_account_balance_nano: "0".to_string(),
        reasoning_envelope_enabled: true,
        request_capture_mode: crate::users::RequestCaptureMode::Off,
    })
}

async fn ensure_balance_before_forward(
    state: &AppState,
    auth: &crate::auth::AuthResult,
) -> AppResult<()> {
    if state.node.is_replica() {
        // M7: replica preflight subtracts locally unshipped charges.
        return ensure_replica_can_spend(state, auth).await;
    }
    if auth.sub_account_enabled {
        let Some(api_key_id) = auth.api_key_id.as_deref() else {
            return Ok(());
        };
        return match state
            .user_store
            .ensure_sub_account_can_spend(api_key_id)
            .await
        {
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
                    "api key not found",
                )),
                _ => Err(AppError::new(
                    StatusCode::INTERNAL_SERVER_ERROR,
                    "internal_error",
                    err.message,
                )),
            },
        };
    }
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

fn attempts_require_balance(attempts: &[MonoizeAttempt]) -> bool {
    attempts
        .iter()
        .any(|attempt| attempt.billable_pricing_available)
}

async fn ensure_balance_before_forward_for_attempts(
    state: &AppState,
    auth: &crate::auth::AuthResult,
    attempts: &[MonoizeAttempt],
) -> AppResult<()> {
    if !attempts_require_balance(attempts) {
        return Ok(());
    }
    ensure_balance_before_forward(state, auth).await
}

/// M7: effective-balance preflight for replicas. Mirrors the primary's
/// `ensure_user_can_spend` / `ensure_sub_account_can_spend` semantics while
/// subtracting charges that are still queued for shipment to the primary.
#[allow(clippy::result_large_err)]
async fn ensure_replica_can_spend(
    state: &AppState,
    auth: &crate::auth::AuthResult,
) -> AppResult<()> {
    let Some(metering) = state.metering.as_ref() else {
        return Ok(());
    };
    let outstanding_for = |subject: &str| metering.pending().outstanding(subject);
    if auth.sub_account_enabled {
        if let Some(api_key_id) = auth.api_key_id.as_deref() {
            let key = state
                .user_store
                .get_api_key_by_id(api_key_id)
                .await
                .map_err(|err| {
                    AppError::new(StatusCode::INTERNAL_SERVER_ERROR, "internal_error", err)
                })?
                .ok_or_else(|| {
                    AppError::new(
                        StatusCode::UNAUTHORIZED,
                        "unauthorized",
                        "api key not found",
                    )
                })?;
            if key.sub_account_enabled {
                let stored: i128 = key.sub_account_balance_nano.trim().parse().map_err(|err| {
                    AppError::new(
                        StatusCode::INTERNAL_SERVER_ERROR,
                        "internal_error",
                        format!("invalid stored sub-account balance: {err}"),
                    )
                })?;
                let effective = stored - outstanding_for(api_key_id);
                if effective <= 0 {
                    return Err(AppError::new(
                        StatusCode::PAYMENT_REQUIRED,
                        "insufficient_balance",
                        "insufficient balance",
                    ));
                }
                return Ok(());
            }
            // Not sub-account-enabled: charges fall back to the owning user row.
            return ensure_replica_user_can_spend(state, &key.user_id, outstanding_for).await;
        }
        return Ok(());
    }
    let Some(user_id) = auth.user_id.as_deref() else {
        return Ok(());
    };
    ensure_replica_user_can_spend(state, user_id, outstanding_for).await
}

#[allow(clippy::result_large_err)]
async fn ensure_replica_user_can_spend(
    state: &AppState,
    user_id: &str,
    outstanding_for: impl Fn(&str) -> i128,
) -> AppResult<()> {
    let balance = state
        .user_store
        .get_user_balance_uncached(user_id)
        .await
        .map_err(|err| AppError::new(StatusCode::INTERNAL_SERVER_ERROR, "internal_error", err))?
        .ok_or_else(|| AppError::new(StatusCode::UNAUTHORIZED, "unauthorized", "user not found"))?;
    if balance.balance_unlimited {
        return Ok(());
    }
    let effective = balance.balance_nano_usd - outstanding_for(user_id);
    if effective <= 0 {
        return Err(AppError::new(
            StatusCode::PAYMENT_REQUIRED,
            "insufficient_balance",
            "insufficient balance",
        ));
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

fn parse_max_multiplier_header(headers: &HeaderMap) -> Option<Multiplier> {
    headers
        .get("x-max-multiplier")
        .and_then(|v| v.to_str().ok())
        .and_then(|s| s.parse().ok())
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
        .and_then(Value::as_str)
        .and_then(|value| value.parse().ok());

    Ok(UrpRequest {
        affinity_explicit: None,
        affinity_prefix_hash: crate::handlers::helpers::short_xxh3_hex(&model),
        model,
        max_multiplier,
        server_tool_usage_classes: Vec::new(),
    })
}
