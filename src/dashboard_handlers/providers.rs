use crate::app::AppState;
use crate::dashboard_handlers::session_helpers::require_admin;
use crate::error::{AppError, AppResult};
use crate::monoize_routing::{
    ChannelHealthState, CreateMonoizeProviderInput, MonoizeChannel, MonoizeProvider,
    ReorderProvidersInput, UpdateMonoizeProviderInput,
};
use axum::Json;
use axum::extract::{Path, State};
use axum::http::{HeaderMap, StatusCode};
use axum::response::IntoResponse;
use serde::Deserialize;
use serde_json::{Value, json};

fn apply_channel_runtime(channel: &mut MonoizeChannel, health: &ChannelHealthState) {
    let now = chrono::Utc::now().timestamp();
    channel._healthy = Some(health.healthy);
    channel._failure_count = Some(health.failure_count);
    channel._health_status = Some(health.status(now).to_string());
    channel._last_success_at = health
        .last_success_at
        .and_then(|ts| chrono::DateTime::<chrono::Utc>::from_timestamp(ts, 0))
        .map(|t| t.to_rfc3339());
}

async fn provider_with_runtime(state: &AppState, mut provider: MonoizeProvider) -> MonoizeProvider {
    let health = state.channel_health.lock().await;
    for channel in &mut provider.channels {
        let state = health
            .get(&channel.id)
            .cloned()
            .unwrap_or_else(ChannelHealthState::new);
        apply_channel_runtime(channel, &state);
    }
    provider
}

async fn prune_provider_channel_health(state: &AppState, channel_ids: &[String]) {
    if channel_ids.is_empty() {
        return;
    }
    let ids: std::collections::HashSet<&str> = channel_ids.iter().map(String::as_str).collect();
    let mut health = state.channel_health.lock().await;
    health.retain(|channel_id, _| !ids.contains(channel_id.as_str()));
}

pub(super) fn provider_pricing_model<'a>(
    logical_model: &'a str,
    model_entry: &'a crate::monoize_routing::MonoizeModelEntry,
) -> &'a str {
    model_entry
        .redirect
        .as_deref()
        .map(str::trim)
        .filter(|v| !v.is_empty())
        .unwrap_or(logical_model)
}

pub async fn list_providers(
    State(state): State<AppState>,
    headers: HeaderMap,
) -> AppResult<impl IntoResponse> {
    require_admin(&headers, &state).await?;

    let providers = state
        .monoize_store
        .list_providers()
        .await
        .map_err(|e| AppError::new(StatusCode::INTERNAL_SERVER_ERROR, "internal_error", e))?;

    let priced_ids = state
        .model_registry_store
        .list_priced_model_ids()
        .await
        .unwrap_or_default();

    let mut out = Vec::with_capacity(providers.len());
    for provider in providers {
        let unpriced_count = provider
            .models
            .iter()
            .filter(|(logical_model, model_entry)| {
                let target_model = provider_pricing_model(logical_model, model_entry);
                !priced_ids.contains(target_model)
            })
            .count();
        let p = provider_with_runtime(&state, provider).await;
        let val = serde_json::to_value(&p).unwrap_or_default();
        if let Value::Object(mut obj) = val {
            obj.insert(
                "unpriced_model_count".to_string(),
                Value::Number(serde_json::Number::from(unpriced_count)),
            );
            out.push(Value::Object(obj));
        } else {
            out.push(val);
        }
    }

    Ok(Json(out))
}

pub async fn get_provider(
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

    Ok(Json(provider_with_runtime(&state, provider).await))
}

pub async fn create_provider(
    State(state): State<AppState>,
    headers: HeaderMap,
    Json(body): Json<CreateMonoizeProviderInput>,
) -> AppResult<impl IntoResponse> {
    require_admin(&headers, &state).await?;

    let provider = state
        .monoize_store
        .create_provider(body)
        .await
        .map_err(|e| AppError::new(StatusCode::BAD_REQUEST, "invalid_request", e))?;

    Ok((
        StatusCode::CREATED,
        Json(provider_with_runtime(&state, provider).await),
    ))
}

pub async fn update_provider(
    State(state): State<AppState>,
    headers: HeaderMap,
    Path(provider_id): Path<String>,
    Json(body): Json<UpdateMonoizeProviderInput>,
) -> AppResult<impl IntoResponse> {
    require_admin(&headers, &state).await?;

    let prev_provider = state
        .monoize_store
        .get_provider(&provider_id)
        .await
        .map_err(|e| AppError::new(StatusCode::INTERNAL_SERVER_ERROR, "internal_error", e))?
        .ok_or_else(|| AppError::new(StatusCode::NOT_FOUND, "not_found", "provider not found"))?;

    let provider = state
        .monoize_store
        .update_provider(&provider_id, body)
        .await
        .map_err(|e| {
            if e.contains("not found") {
                AppError::new(StatusCode::NOT_FOUND, "not_found", e)
            } else {
                AppError::new(StatusCode::BAD_REQUEST, "invalid_request", e)
            }
        })?;

    let next_channel_ids: std::collections::HashSet<&str> =
        provider.channels.iter().map(|ch| ch.id.as_str()).collect();
    let removed_channel_ids: Vec<String> = prev_provider
        .channels
        .iter()
        .filter(|ch| !next_channel_ids.contains(ch.id.as_str()))
        .map(|ch| ch.id.clone())
        .collect();
    prune_provider_channel_health(&state, &removed_channel_ids).await;

    Ok(Json(provider_with_runtime(&state, provider).await))
}

pub async fn delete_provider(
    State(state): State<AppState>,
    headers: HeaderMap,
    Path(provider_id): Path<String>,
) -> AppResult<impl IntoResponse> {
    require_admin(&headers, &state).await?;

    let existing_provider = state
        .monoize_store
        .get_provider(&provider_id)
        .await
        .map_err(|e| AppError::new(StatusCode::INTERNAL_SERVER_ERROR, "internal_error", e))?
        .ok_or_else(|| AppError::new(StatusCode::NOT_FOUND, "not_found", "provider not found"))?;

    state
        .monoize_store
        .delete_provider(&provider_id)
        .await
        .map_err(|e| {
            if e.contains("not found") {
                AppError::new(StatusCode::NOT_FOUND, "not_found", e)
            } else {
                AppError::new(StatusCode::INTERNAL_SERVER_ERROR, "internal_error", e)
            }
        })?;

    let removed_channel_ids: Vec<String> = existing_provider
        .channels
        .iter()
        .map(|ch| ch.id.clone())
        .collect();
    prune_provider_channel_health(&state, &removed_channel_ids).await;

    Ok(Json(json!({ "success": true })))
}

pub async fn reorder_providers(
    State(state): State<AppState>,
    headers: HeaderMap,
    Json(body): Json<ReorderProvidersInput>,
) -> AppResult<impl IntoResponse> {
    require_admin(&headers, &state).await?;

    state
        .monoize_store
        .reorder_providers(body)
        .await
        .map_err(|e| AppError::new(StatusCode::BAD_REQUEST, "invalid_request", e))?;

    Ok(Json(json!({ "success": true })))
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

    let url = build_models_list_url(&channel.base_url);

    let resp = state
        .http
        .get(&url)
        .header("Authorization", format!("Bearer {}", channel.api_key))
        .timeout(std::time::Duration::from_secs(15))
        .send()
        .await
        .map_err(|e| {
            AppError::new(
                StatusCode::BAD_GATEWAY,
                "upstream_fetch_failed",
                format!("failed to fetch models: {e}"),
            )
        })?;

    if !resp.status().is_success() {
        let status = resp.status();
        let body = resp.text().await.unwrap_or_default();
        return Err(AppError::new(
            StatusCode::BAD_GATEWAY,
            "upstream_fetch_failed",
            format!("upstream returned {status}: {body}"),
        ));
    }

    let body: Value = resp.json().await.map_err(|e| {
        AppError::new(
            StatusCode::BAD_GATEWAY,
            "upstream_fetch_failed",
            format!("failed to parse response: {e}"),
        )
    })?;

    let models: Vec<String> = body
        .get("data")
        .and_then(|d| d.as_array())
        .map(|arr| {
            let mut seen = std::collections::HashSet::new();
            arr.iter()
                .filter_map(|item| item.get("id").and_then(|id| id.as_str()).map(String::from))
                .filter(|id| seen.insert(id.clone()))
                .collect()
        })
        .unwrap_or_default();

    Ok(Json(json!({
        "provider_id": provider.id,
        "provider_name": provider.name,
        "models": models
    })))
}

#[derive(Debug, Deserialize)]
pub struct FetchChannelModelsRequest {
    pub base_url: String,
    pub api_key: String,
}

pub async fn fetch_channel_models(
    State(state): State<AppState>,
    headers: HeaderMap,
    Json(body): Json<FetchChannelModelsRequest>,
) -> AppResult<impl IntoResponse> {
    require_admin(&headers, &state).await?;

    if body.base_url.trim().is_empty() || body.api_key.trim().is_empty() {
        return Err(AppError::new(
            StatusCode::BAD_REQUEST,
            "invalid_input",
            "base_url and api_key are required",
        ));
    }

    let url = build_models_list_url(&body.base_url);

    let resp = state
        .http
        .get(&url)
        .header("Authorization", format!("Bearer {}", body.api_key))
        .timeout(std::time::Duration::from_secs(15))
        .send()
        .await
        .map_err(|e| {
            AppError::new(
                StatusCode::BAD_GATEWAY,
                "upstream_fetch_failed",
                format!("failed to fetch models: {e}"),
            )
        })?;

    if !resp.status().is_success() {
        let status = resp.status();
        let body = resp.text().await.unwrap_or_default();
        return Err(AppError::new(
            StatusCode::BAD_GATEWAY,
            "upstream_fetch_failed",
            format!("upstream returned {status}: {body}"),
        ));
    }

    let resp_body: Value = resp.json().await.map_err(|e| {
        AppError::new(
            StatusCode::BAD_GATEWAY,
            "upstream_fetch_failed",
            format!("failed to parse response: {e}"),
        )
    })?;

    let models: Vec<String> = resp_body
        .get("data")
        .and_then(|d| d.as_array())
        .map(|arr| {
            let mut seen = std::collections::HashSet::new();
            arr.iter()
                .filter_map(|item| item.get("id").and_then(|id| id.as_str()).map(String::from))
                .filter(|id| seen.insert(id.clone()))
                .collect()
        })
        .unwrap_or_default();

    Ok(Json(json!({ "models": models })))
}

#[derive(Debug, Deserialize)]
pub struct TestChannelRequest {
    pub model: Option<String>,
}

pub async fn test_channel(
    State(state): State<AppState>,
    headers: HeaderMap,
    Path((provider_id, channel_id)): Path<(String, String)>,
    body: Option<Json<TestChannelRequest>>,
) -> AppResult<impl IntoResponse> {
    require_admin(&headers, &state).await?;

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

    let requested_model = body.and_then(|b| b.model.clone());

    let settings = state
        .settings_store
        .get_all()
        .await
        .map_err(|e| AppError::new(StatusCode::INTERNAL_SERVER_ERROR, "internal_error", e))?;

    let probe_model = requested_model
        .or_else(|| provider.active_probe_model_override.clone())
        .or_else(|| settings.monoize_active_probe_model.clone())
        .or_else(|| provider.models.keys().next().cloned());

    let Some(model_name) = probe_model else {
        return Err(AppError::new(
            StatusCode::BAD_REQUEST,
            "invalid_request",
            "no model available for testing; specify a model or add models to this provider",
        ));
    };

    let upstream_model = provider
        .models
        .get(&model_name)
        .map(|entry| provider_pricing_model(&model_name, entry).to_string())
        .unwrap_or_else(|| model_name.clone());

    let started_at = std::time::Instant::now();
    let (ok, _usage) = crate::monoize_routing::probe_channel_completion(
        &state.http,
        channel,
        state.monoize_runtime.read().await.request_timeout_ms,
        &upstream_model,
        provider.provider_type,
        &provider.api_type_overrides,
    )
    .await;
    let latency_ms = started_at.elapsed().as_millis() as u64;

    if ok {
        let now = chrono::Utc::now().timestamp();
        let mut health = state.channel_health.lock().await;
        let entry = health
            .entry(channel_id.clone())
            .or_insert_with(ChannelHealthState::new);
        entry.healthy = true;
        entry.failure_count = 0;
        entry.cooldown_until = None;
        entry.last_success_at = Some(now);
        entry.probe_success_count = 0;
        entry.last_probe_at = None;
    }

    let error_msg = if ok {
        None
    } else {
        Some("upstream returned non-2xx status or connection failed".to_string())
    };

    Ok(Json(json!({
        "success": ok,
        "latency_ms": latency_ms,
        "model": model_name,
        "error": error_msg,
    })))
}

pub async fn get_transform_registry(State(state): State<AppState>) -> AppResult<impl IntoResponse> {
    let mut items: Vec<Value> = state
        .transform_registry
        .values()
        .map(|transform| {
            json!({
                "type_id": transform.type_id(),
                "supported_phases": transform
                    .supported_phases()
                    .iter()
                    .map(|p| serde_json::to_value(p).unwrap_or(Value::String("request".to_string())))
                    .collect::<Vec<_>>(),
                "config_schema": transform.config_schema(),
            })
        })
        .collect();
    items.sort_by(|a, b| a["type_id"].as_str().cmp(&b["type_id"].as_str()));
    Ok(Json(items))
}

pub async fn get_provider_presets() -> AppResult<impl IntoResponse> {
    Ok(Json(crate::presets::provider_presets()))
}

pub(super) fn build_models_list_url(base_url: &str) -> String {
    let base = base_url.trim_end_matches('/');
    if base.ends_with("/v1") {
        format!("{base}/models")
    } else {
        format!("{base}/v1/models")
    }
}
