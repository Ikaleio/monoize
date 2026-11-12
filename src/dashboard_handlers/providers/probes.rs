use super::provider_pricing_model;
use super::runtime::reset_channel_health_after_successful_test;
use crate::app::AppState;
use crate::dashboard_handlers::session_helpers::require_admin;
use crate::error::{AppError, AppResult};
use crate::monoize_routing::{MonoizeChannel, MonoizeProvider};
use axum::Json;
use axum::extract::{Path, State};
use axum::http::{HeaderMap, StatusCode};
use axum::response::IntoResponse;
use futures_util::FutureExt;
use serde::Deserialize;
use serde_json::{Value, json};
use std::panic::AssertUnwindSafe;
use std::sync::atomic::Ordering;
use std::time::Duration;
use tokio::time::timeout;

fn discovery_body_error(error: crate::bounded_response::BoundedResponseError) -> AppError {
    let code = if error.is_limit_exceeded() {
        "upstream_discovery_response_too_large"
    } else {
        "upstream_fetch_failed"
    };
    AppError::new(
        StatusCode::BAD_GATEWAY,
        code,
        crate::error::error_chain(&error),
    )
}

async fn parse_discovery_json_response(response: reqwest::Response) -> AppResult<Value> {
    let status = response.status();
    let body = crate::bounded_response::read_upstream_discovery_body(response)
        .await
        .map_err(discovery_body_error)?;
    if !status.is_success() {
        return Err(AppError::new(
            StatusCode::BAD_GATEWAY,
            "upstream_fetch_failed",
            format!(
                "upstream returned {status}: {}",
                String::from_utf8_lossy(&body)
            ),
        ));
    }

    serde_json::from_slice(&body).map_err(|error| {
        AppError::new(
            StatusCode::BAD_GATEWAY,
            "upstream_fetch_failed",
            format!("failed to parse response: {error}"),
        )
    })
}

pub async fn fetch_provider_models(
    State(state): State<AppState>,
    headers: HeaderMap,
    Path(provider_id): Path<String>,
) -> AppResult<impl IntoResponse> {
    require_admin(&headers, &state).await?;

    let provider = state
        .monoize_store
        .get_provider(&provider_id)
        .await
        .map_err(|e| AppError::new(StatusCode::INTERNAL_SERVER_ERROR, "internal_error", e))?
        .ok_or_else(|| AppError::new(StatusCode::NOT_FOUND, "not_found", "provider not found"))?;

    if provider.channels.is_empty() {
        return Err(AppError::new(
            StatusCode::BAD_REQUEST,
            "no_channels",
            "provider has no channels",
        ));
    }

    let channel = provider
        .channels
        .iter()
        .find(|c| c.enabled)
        .unwrap_or(&provider.channels[0]);

    let url =
        if channel.provider_type == crate::monoize_routing::MonoizeProviderType::OpenrouterImage {
            crate::upstream::join_url(&channel.base_url, "/v1/images/models")
        } else {
            build_models_list_url(&channel.base_url)
        };

    let mut request = state
        .http
        .get(&url)
        .timeout(std::time::Duration::from_secs(15));
    request = crate::monoize_routing::apply_provider_api_key(
        request,
        channel.provider_type,
        &channel.api_key,
    );
    if channel.provider_type == crate::monoize_routing::MonoizeProviderType::Messages {
        request = request.header("anthropic-version", "2023-06-01");
    }
    let resp = request.send().await.map_err(|e| {
        AppError::new(
            StatusCode::BAD_GATEWAY,
            "upstream_fetch_failed",
            format!("failed to fetch models: {}", crate::error::error_chain(&e)),
        )
    })?;

    let body = parse_discovery_json_response(resp).await?;

    let list_type =
        if channel.provider_type == crate::monoize_routing::MonoizeProviderType::SystemOne {
            crate::monoize_routing::MonoizeProviderType::SystemOne
        } else {
            crate::monoize_routing::MonoizeProviderType::ChatCompletion
        };
    let models: Vec<String> = extract_model_ids(list_type, &body);
    let (compact_scheme, compact_models) = crate::handlers::classify_openai_compact_scheme(&models);

    Ok(Json(json!({
        "provider_id": provider.id,
        "provider_name": provider.name,
        "models": models,
        "compact_scheme": compact_scheme,
        "compact_models": compact_models
    })))
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct FetchChannelModelsRequest {
    pub provider_type: crate::monoize_routing::MonoizeProviderType,
    pub base_url: String,
    #[serde(default)]
    pub api_key: Option<String>,
    #[serde(default)]
    pub provider_id: Option<String>,
    #[serde(default)]
    pub channel_id: Option<String>,
}

async fn resolve_fetch_channel_api_key(
    state: &AppState,
    body: &FetchChannelModelsRequest,
) -> AppResult<String> {
    if let Some(api_key) = body
        .api_key
        .as_deref()
        .map(str::trim)
        .filter(|value| !value.is_empty())
    {
        return Ok(api_key.to_string());
    }

    let provider_id = body
        .provider_id
        .as_deref()
        .map(str::trim)
        .filter(|value| !value.is_empty());
    let channel_id = body
        .channel_id
        .as_deref()
        .map(str::trim)
        .filter(|value| !value.is_empty());

    let (Some(provider_id), Some(channel_id)) = (provider_id, channel_id) else {
        return Err(AppError::new(
            StatusCode::BAD_REQUEST,
            "invalid_input",
            "api_key is required for unsaved channels",
        ));
    };

    let provider = state
        .monoize_store
        .get_provider(provider_id)
        .await
        .map_err(|e| AppError::new(StatusCode::INTERNAL_SERVER_ERROR, "internal_error", e))?
        .ok_or_else(|| {
            AppError::new(
                StatusCode::BAD_REQUEST,
                "invalid_input",
                "stored channel key could not be resolved",
            )
        })?;

    let channel = provider
        .channels
        .iter()
        .find(|channel| channel.id == channel_id)
        .ok_or_else(|| {
            AppError::new(
                StatusCode::BAD_REQUEST,
                "invalid_input",
                "stored channel key could not be resolved",
            )
        })?;

    let api_key = channel.api_key.trim();
    if api_key.is_empty() {
        return Err(AppError::new(
            StatusCode::BAD_REQUEST,
            "invalid_input",
            "api_key is required for unsaved channels",
        ));
    }

    Ok(api_key.to_string())
}

pub async fn fetch_channel_models(
    State(state): State<AppState>,
    headers: HeaderMap,
    Json(body): Json<FetchChannelModelsRequest>,
) -> AppResult<impl IntoResponse> {
    require_admin(&headers, &state).await?;

    let base_url = body.base_url.trim();
    if base_url.is_empty() {
        return Err(AppError::new(
            StatusCode::BAD_REQUEST,
            "invalid_input",
            "base_url is required",
        ));
    }
    let api_key = resolve_fetch_channel_api_key(&state, &body).await?;

    let url = match body.provider_type {
        crate::monoize_routing::MonoizeProviderType::OpenrouterImage => {
            crate::upstream::join_url(base_url, "/v1/images/models")
        }
        crate::monoize_routing::MonoizeProviderType::Gemini => {
            build_gemini_models_list_url(base_url)
        }
        _ => build_models_list_url(base_url),
    };

    let mut request = state
        .http
        .get(&url)
        .timeout(std::time::Duration::from_secs(15));
    request = crate::monoize_routing::apply_provider_api_key(
        request,
        body.provider_type,
        api_key.as_str(),
    );
    if body.provider_type == crate::monoize_routing::MonoizeProviderType::Messages {
        request = request.header("anthropic-version", "2023-06-01");
    }

    let resp = request.send().await.map_err(|e| {
        AppError::new(
            StatusCode::BAD_GATEWAY,
            "upstream_fetch_failed",
            format!("failed to fetch models: {}", crate::error::error_chain(&e)),
        )
    })?;

    let resp_body = parse_discovery_json_response(resp).await?;

    let models: Vec<String> = extract_model_ids(body.provider_type, &resp_body);
    let (compact_scheme, compact_models) = crate::handlers::classify_openai_compact_scheme(&models);

    Ok(Json(json!({
        "models": models,
        "compact_scheme": compact_scheme,
        "compact_models": compact_models
    })))
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ProbeChannelWebsocketRequest {
    pub provider_type: crate::monoize_routing::MonoizeProviderType,
    pub base_url: String,
    #[serde(default)]
    pub api_key: Option<String>,
    #[serde(default)]
    pub provider_id: Option<String>,
    #[serde(default)]
    pub channel_id: Option<String>,
    #[serde(default)]
    pub model: Option<String>,
}

pub async fn probe_channel_websocket(
    State(state): State<AppState>,
    headers: HeaderMap,
    Json(body): Json<ProbeChannelWebsocketRequest>,
) -> AppResult<impl IntoResponse> {
    require_admin(&headers, &state).await?;
    let fetch_key_body = FetchChannelModelsRequest {
        provider_type: body.provider_type,
        base_url: body.base_url.clone(),
        api_key: body.api_key.clone(),
        provider_id: body.provider_id.clone(),
        channel_id: body.channel_id.clone(),
    };
    let api_key = resolve_fetch_channel_api_key(&state, &fetch_key_body).await?;
    let stored =
        resolve_stored_provider_channel(&state, &body.provider_id, &body.channel_id).await?;
    let stored_provider = stored.as_ref().map(|(provider, _)| provider);
    let stored_channel = stored.as_ref().map(|(_, channel)| channel);
    if body.provider_type != crate::monoize_routing::MonoizeProviderType::Responses {
        return Ok(Json(json!({
            "supported": false,
            "http_status": serde_json::Value::Null,
            "warmup": "skipped",
            "error": "provider_type is not responses"
        })));
    }
    let request_timeout_ms = {
        let runtime = state.monoize_runtime.read().await;
        stored_provider
            .and_then(|provider| provider.request_timeout_ms_override)
            .unwrap_or(runtime.request_timeout_ms)
            .max(1)
    };
    let proxy = stored
        .as_ref()
        .and_then(|(_, channel)| channel.proxy_url.as_deref())
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .or(state.node.upstream_proxy_url.as_deref());
    let ws_headers = websocket_probe_headers(&api_key, stored_channel);
    let started = std::time::Instant::now();
    let remaining_ms = || {
        request_timeout_ms
            .saturating_sub(u64::try_from(started.elapsed().as_millis()).unwrap_or(u64::MAX))
            .max(1)
    };
    match AssertUnwindSafe(crate::upstream_websocket::connect_responses_websocket(
        body.base_url.trim(),
        &ws_headers,
        proxy,
        remaining_ms(),
    ))
    .catch_unwind()
    .await
    {
        Err(_) => Ok(Json(json!({
            "supported": false,
            "http_status": serde_json::Value::Null,
            "warmup": "skipped",
            "error": "upstream WebSocket TLS initialization failed"
        }))),
        Ok(Err(err)) => Ok(Json(json!({
            "supported": false,
            "http_status": err.http_status,
            "warmup": "skipped",
            "error": err.message
        }))),
        Ok(Ok(mut session)) => {
            let model = body
                .model
                .as_deref()
                .map(str::trim)
                .filter(|value| !value.is_empty())
                .map(str::to_string);
            let warmup = if let Some(model) = model {
                let payload = serde_json::json!({
                    "type": "response.create",
                    "model": model,
                    "generate": false
                });
                let warmup_result = timeout(Duration::from_millis(remaining_ms()), async {
                    session.send_text(payload.to_string()).await?;
                    session.next_text().await
                })
                .await;
                match warmup_result {
                    Ok(Ok(Some(_))) => "ok",
                    _ => "protocol_error",
                }
            } else {
                "skipped"
            };
            session.close().await;
            Ok(Json(json!({
                "supported": true,
                "http_status": 101,
                "warmup": warmup,
                "error": serde_json::Value::Null
            })))
        }
    }
}

async fn resolve_stored_provider_channel(
    state: &AppState,
    provider_id: &Option<String>,
    channel_id: &Option<String>,
) -> AppResult<Option<(MonoizeProvider, MonoizeChannel)>> {
    let provider_id = provider_id
        .as_deref()
        .map(str::trim)
        .filter(|value| !value.is_empty());
    let channel_id = channel_id
        .as_deref()
        .map(str::trim)
        .filter(|value| !value.is_empty());
    let (Some(provider_id), Some(channel_id)) = (provider_id, channel_id) else {
        return Ok(None);
    };
    let provider = state
        .monoize_store
        .get_provider(provider_id)
        .await
        .map_err(|e| AppError::new(StatusCode::INTERNAL_SERVER_ERROR, "internal_error", e))?;
    let Some(provider) = provider else {
        return Ok(None);
    };
    let Some(index) = provider
        .channels
        .iter()
        .position(|channel| channel.id == channel_id)
    else {
        return Ok(None);
    };
    let channel = provider.channels[index].clone();
    Ok(Some((provider, channel)))
}

#[derive(Debug, Deserialize)]
pub struct TestChannelRequest {
    pub model: Option<String>,
    pub stream: Option<bool>,
}

pub async fn test_channel(
    State(state): State<AppState>,
    headers: HeaderMap,
    Path((provider_id, channel_id)): Path<(String, String)>,
    body: Option<Json<TestChannelRequest>>,
) -> AppResult<impl IntoResponse> {
    require_admin(&headers, &state).await?;
    let routing_config_revision = state.routing_config_revision.load(Ordering::Acquire);

    let provider = state
        .monoize_store
        .get_provider(&provider_id)
        .await
        .map_err(|e| AppError::new(StatusCode::INTERNAL_SERVER_ERROR, "internal_error", e))?
        .ok_or_else(|| AppError::new(StatusCode::NOT_FOUND, "not_found", "provider not found"))?;

    let channel = provider
        .channels
        .iter()
        .find(|c| c.id == channel_id)
        .ok_or_else(|| AppError::new(StatusCode::NOT_FOUND, "not_found", "channel not found"))?;

    let (requested_model, stream) = body
        .map(|body| (body.model.clone(), body.stream.unwrap_or(true)))
        .unwrap_or((None, true));

    if let Some(model) = requested_model.as_ref()
        && !channel.models.contains_key(model)
    {
        return Err(AppError::new(
            StatusCode::BAD_REQUEST,
            "invalid_request",
            "model is not supported by this channel",
        ));
    }

    if channel.provider_type == crate::monoize_routing::MonoizeProviderType::Replicate {
        return Err(AppError::new(
            StatusCode::BAD_REQUEST,
            "invalid_request",
            "replicate channels do not support completion probe tests",
        ));
    }

    let (global_probe_model, request_timeout_ms) = {
        let runtime = state.monoize_runtime.read().await;
        (
            runtime.active_probe_model.clone(),
            runtime.request_timeout_ms,
        )
    };

    let (model_name, upstream_model) = resolve_test_model(
        &provider,
        channel,
        requested_model,
        global_probe_model,
        stream,
    )?;

    let started_at = std::time::Instant::now();
    // PX6: the connectivity test uses the channel's effective proxy resolution.
    let test_http = state
        .http_clients
        .for_channel_proxy(channel.proxy_url.as_deref())
        .map_err(|detail| {
            AppError::new(
                StatusCode::BAD_REQUEST,
                "invalid_request",
                format!("channel proxy_url cannot be used: {detail}"),
            )
        })?;
    let outcome = crate::monoize_routing::probe_channel_completion(
        &test_http,
        channel,
        request_timeout_ms,
        &upstream_model,
        channel.provider_type,
        &provider.api_type_overrides,
        stream,
    )
    .await;
    let latency_ms = started_at.elapsed().as_millis() as u64;

    if outcome.ok {
        let now = chrono::Utc::now().timestamp();
        let mut health = state.channel_health.lock().await;
        if state.routing_config_revision.load(Ordering::Acquire) == routing_config_revision {
            reset_channel_health_after_successful_test(
                &mut health,
                &channel_id,
                provider.per_model_circuit_break,
                &model_name,
                now,
                crate::monoize_routing::channel_health_max_entries(),
            );
        }
    }

    Ok(Json(json!({
        "success": outcome.ok,
        "latency_ms": latency_ms,
        "model": model_name,
        "stream": stream,
        "http_status": outcome.http_status,
        "error_code": outcome.error_code,
        "error_type": outcome.error_type,
        "error": outcome.error,
    })))
}

fn build_models_list_url(base_url: &str) -> String {
    let base = base_url.trim_end_matches('/');
    if base.ends_with("/v1") {
        format!("{base}/models")
    } else {
        format!("{base}/v1/models")
    }
}

fn build_gemini_models_list_url(base_url: &str) -> String {
    let base = base_url.trim_end_matches('/');
    if base.ends_with("/v1beta") || base.ends_with("/v1") {
        format!("{base}/models")
    } else {
        format!("{base}/v1beta/models")
    }
}

fn extract_model_ids(
    provider_type: crate::monoize_routing::MonoizeProviderType,
    body: &Value,
) -> Vec<String> {
    let mut seen = std::collections::HashSet::new();
    let mut models: Vec<String> = match provider_type {
        crate::monoize_routing::MonoizeProviderType::Gemini => body
            .get("models")
            .and_then(|d| d.as_array())
            .into_iter()
            .flatten()
            .filter_map(|item| {
                item.get("name")
                    .and_then(|id| id.as_str())
                    .map(|value| value.strip_prefix("models/").unwrap_or(value).to_string())
            })
            .filter(|id| seen.insert(id.clone()))
            .collect(),
        // DR-C3: TypeSafe lists models as `models[].name`; other System One
        // hosts use the OpenAI `data[].id` list.
        crate::monoize_routing::MonoizeProviderType::SystemOne => {
            let from_data = string_field_ids(body.get("data"), "id", &mut seen);
            if from_data.is_empty() {
                string_field_ids(body.get("models"), "name", &mut seen)
            } else {
                from_data
            }
        }
        _ => string_field_ids(body.get("data"), "id", &mut seen),
    };
    models.sort();
    models
}

fn string_field_ids(
    items: Option<&Value>,
    field: &str,
    seen: &mut std::collections::HashSet<String>,
) -> Vec<String> {
    items
        .and_then(Value::as_array)
        .into_iter()
        .flatten()
        .filter_map(|item| item.get(field).and_then(Value::as_str).map(String::from))
        .filter(|id| seen.insert(id.clone()))
        .collect()
}

fn websocket_probe_headers(
    api_key: &str,
    stored_channel: Option<&MonoizeChannel>,
) -> Vec<(String, String)> {
    let mut ws_headers = vec![crate::upstream_websocket::responses_authorization_header(
        api_key,
    )];
    if let Some(channel) = stored_channel {
        if let Some(extras) = &channel.extra_headers {
            for (name, value) in extras {
                ws_headers.push((name.clone(), value.clone()));
            }
        }
        let affinity_enabled = channel.session_affinity_auto.unwrap_or_else(|| {
            crate::monoize_routing::default_session_affinity_auto(&channel.base_url)
        });
        if affinity_enabled {
            let has_session = ws_headers.iter().any(|(name, _)| {
                name.eq_ignore_ascii_case("x-session-affinity")
                    || name.eq_ignore_ascii_case("x-opencode-session")
            });
            if !has_session {
                ws_headers.push((
                    "x-session-affinity".to_string(),
                    "mono-ws-probe".to_string(),
                ));
                ws_headers.push((
                    "x-opencode-session".to_string(),
                    "mono-ws-probe".to_string(),
                ));
            }
        }
    }
    ws_headers
}

fn resolve_test_model(
    provider: &MonoizeProvider,
    channel: &MonoizeChannel,
    requested_model: Option<String>,
    global_probe_model: Option<String>,
    stream: bool,
) -> AppResult<(String, String)> {
    let first_supported_model = channel.models.keys().min().cloned();

    let probe_model = requested_model
        .or_else(|| channel.active_probe_model_override.clone())
        .or_else(|| provider.active_probe_model_override.clone())
        .or(global_probe_model)
        .or(first_supported_model);

    let Some(model_name) = probe_model else {
        return Err(AppError::new(
            StatusCode::BAD_REQUEST,
            "invalid_request",
            "no model available for testing; specify a model or add models to this provider",
        ));
    };
    if !channel.models.contains_key(&model_name) {
        return Err(AppError::new(
            StatusCode::BAD_REQUEST,
            "invalid_request",
            "resolved probe model is not supported by this channel",
        ));
    }

    let upstream_model = channel
        .models
        .get(&model_name)
        .map(|entry| provider_pricing_model(&model_name, entry).to_string())
        .unwrap_or_else(|| model_name.clone());

    let effective_type = crate::monoize_routing::resolve_effective_api_type(
        &provider.api_type_overrides,
        channel.provider_type,
        &model_name,
    );
    if stream
        && matches!(
            effective_type,
            crate::monoize_routing::MonoizeProviderType::OpenaiImage
                | crate::monoize_routing::MonoizeProviderType::OpenrouterImage
                | crate::monoize_routing::MonoizeProviderType::Replicate
                | crate::monoize_routing::MonoizeProviderType::SystemOne
                | crate::monoize_routing::MonoizeProviderType::OpenaiDecisions
        )
    {
        return Err(AppError::new(
            StatusCode::BAD_REQUEST,
            "invalid_request",
            "this channel API type does not support streaming liveness tests",
        ));
    }

    Ok((model_name, upstream_model))
}
