use crate::app::AppState;
use crate::dashboard_handlers::session_helpers::require_admin;
use crate::error::{AppError, AppResult};
use crate::handlers::routing::health_key;
use crate::monoize_routing::{
    ChannelHealthState, CreateMonoizeProviderInput, MonoizeChannel, MonoizeProvider,
    ReorderProvidersInput, UpdateMonoizeProviderInput,
};
use crate::settings::normalize_pricing_model_key;
use axum::Json;
use axum::extract::{Path, State};
use axum::http::{HeaderMap, StatusCode};
use axum::response::IntoResponse;
use futures_util::FutureExt;
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use std::collections::{HashMap, HashSet};
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

/// MP-UI3: a channel model is priced when an enabled, complete `model_prices`
/// row exists for its normalized upstream key or normalized logical key.
pub(super) fn channel_model_has_model_price(
    priced_keys: &HashSet<String>,
    logical_model: &str,
    model_entry: &crate::monoize_routing::MonoizeModelEntry,
    reasoning_suffix_map: &HashMap<String, String>,
) -> bool {
    let upstream_model = provider_pricing_model(logical_model, model_entry);
    let normalized_upstream_model =
        normalize_pricing_model_key(upstream_model, reasoning_suffix_map);
    if priced_keys.contains(&normalized_upstream_model) {
        return true;
    }
    let normalized_logical_model = normalize_pricing_model_key(logical_model, reasoning_suffix_map);
    normalized_logical_model != normalized_upstream_model
        && priced_keys.contains(&normalized_logical_model)
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
    let live_usage = state
        .user_store
        .get_providers_live_usage()
        .await
        .map_err(|e| AppError::new(StatusCode::INTERNAL_SERVER_ERROR, "internal_error", e))?;

    let reasoning_suffix_map = state
        .monoize_runtime
        .read()
        .await
        .reasoning_suffix_map
        .clone();
    let mut pricing_keys = HashSet::new();
    for provider in &providers {
        for channel in &provider.channels {
            for (logical_model, model_entry) in &channel.models {
                pricing_keys.insert(normalize_pricing_model_key(
                    provider_pricing_model(logical_model, model_entry),
                    &reasoning_suffix_map,
                ));
                pricing_keys.insert(normalize_pricing_model_key(
                    logical_model,
                    &reasoning_suffix_map,
                ));
            }
        }
    }
    let mut pricing_models = pricing_keys.into_iter().collect::<Vec<_>>();
    pricing_models.sort();
    let priced_keys: HashSet<String> = state
        .model_price_store
        .list_by_model_ids(&pricing_models)
        .await
        .map_err(|e| AppError::new(StatusCode::INTERNAL_SERVER_ERROR, "internal_error", e))?
        .into_iter()
        // MP-R2/MP-R4: disabled or incomplete rows count as missing.
        .filter(|row| row.enabled && row.is_complete())
        .map(|row| row.model_id)
        .collect();

    let mut out = Vec::with_capacity(providers.len());
    for provider in providers {
        let mut unpriced_model_ids = Vec::new();
        let mut unpriced_entries = HashSet::new();
        for channel in &provider.channels {
            for (logical_model, model_entry) in &channel.models {
                let has_pricing = channel_model_has_model_price(
                    &priced_keys,
                    logical_model,
                    model_entry,
                    &reasoning_suffix_map,
                );
                if !has_pricing {
                    unpriced_model_ids.push(logical_model.clone());
                    unpriced_entries.insert((channel.id.clone(), logical_model.clone()));
                }
            }
        }
        unpriced_model_ids.sort();
        unpriced_model_ids.dedup();
        let unpriced_count = unpriced_model_ids.len();
        let p = provider_with_runtime(&state, provider).await;
        let model_runtime_statuses = build_provider_model_runtime_statuses(&p, &unpriced_entries);
        let val = serde_json::to_value(&p).unwrap_or_default();
        if let Value::Object(mut obj) = val {
            let usage = live_usage.get(&p.id).copied().unwrap_or_default();
            obj.insert(
                "live_usage".to_string(),
                json!({
                    "window_seconds": crate::users::LIVE_USAGE_WINDOW_SECONDS,
                    "rpm": usage.rpm,
                    "tpm": usage.tpm,
                }),
            );
            obj.insert(
                "unpriced_model_count".to_string(),
                Value::Number(serde_json::Number::from(unpriced_count)),
            );
            obj.insert(
                "unpriced_model_ids".to_string(),
                Value::Array(unpriced_model_ids.into_iter().map(Value::String).collect()),
            );
            obj.insert(
                "model_runtime_statuses".to_string(),
                serde_json::to_value(model_runtime_statuses).unwrap_or_default(),
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

/// CP-INV-14: non-empty channel `proxy_url` must be an absolute http(s) URL.
fn validate_channel_proxy_urls<'a>(
    channels: impl IntoIterator<Item = &'a crate::monoize_routing::CreateMonoizeChannelInput>,
) -> AppResult<()> {
    for channel in channels {
        if let Some(url) = channel
            .proxy_url
            .as_deref()
            .map(str::trim)
            .filter(|v| !v.is_empty())
            && let Err(detail) = crate::node_config::validate_http_proxy_url(url)
        {
            return Err(AppError::new(
                StatusCode::BAD_REQUEST,
                "invalid_request",
                format!("channel '{}' has invalid proxy_url: {detail}", channel.name),
            ));
        }
    }
    Ok(())
}

/// CP-INV-16a: reports the first offending Channel through `param`.
fn validate_channel_chat_dialects(
    channels: &[crate::monoize_routing::CreateMonoizeChannelInput],
) -> AppResult<()> {
    for (index, channel) in channels.iter().enumerate() {
        crate::monoize_routing::channel_chat_dialect(channel).map_err(|message| {
            AppError::new(StatusCode::BAD_REQUEST, "invalid_request", message)
                .with_param(format!("channels[{index}].chat_dialect"))
        })?;
    }
    Ok(())
}

/// CP-INV-17a: reports the first offending override entry through `param`.
fn validate_override_chat_dialects(
    overrides: &[crate::monoize_routing::ApiTypeOverrideInput],
) -> AppResult<()> {
    for (index, entry) in overrides.iter().enumerate() {
        crate::monoize_routing::api_type_override_from_input(index, entry).map_err(|message| {
            AppError::new(StatusCode::BAD_REQUEST, "invalid_request", message)
                .with_param(format!("api_type_overrides[{index}].chat_dialect"))
        })?;
    }
    Ok(())
}

pub async fn create_provider(
    State(state): State<AppState>,
    headers: HeaderMap,
    Json(body): Json<CreateMonoizeProviderInput>,
) -> AppResult<impl IntoResponse> {
    require_admin(&headers, &state).await?;
    validate_channel_proxy_urls(body.channels.iter())?;
    validate_channel_chat_dialects(&body.channels)?;
    validate_override_chat_dialects(&body.api_type_overrides)?;

    let provider = state
        .monoize_store
        .create_provider(body)
        .await
        .map_err(|e| AppError::new(StatusCode::BAD_REQUEST, "invalid_request", e))?;

    advance_routing_config_revision(&state);

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
    if let Some(channels) = body.channels.as_ref() {
        validate_channel_proxy_urls(channels.iter())?;
        validate_channel_chat_dialects(channels)?;
    }
    if let Some(overrides) = body.api_type_overrides.as_ref() {
        validate_override_chat_dialects(overrides)?;
    }

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

    let affected_channel_ids: Vec<String> = prev_provider
        .channels
        .iter()
        .chain(provider.channels.iter())
        .map(|channel| channel.id.clone())
        .collect::<HashSet<_>>()
        .into_iter()
        .collect();
    advance_routing_config_revision(&state);
    prune_provider_channel_health(&state, &affected_channel_ids).await;
    prune_provider_channel_affinity(&state, &affected_channel_ids).await;

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
    advance_routing_config_revision(&state);
    prune_provider_channel_health(&state, &removed_channel_ids).await;
    prune_provider_channel_affinity(&state, &removed_channel_ids).await;

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

    advance_routing_config_revision(&state);
    Ok(Json(json!({ "success": true })))
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct SetChannelEnabledRequest {
    pub enabled: bool,
}

/// CM-CH-EN-1: immediate Channel enable/disable from the Provider list.
pub async fn set_channel_enabled(
    State(state): State<AppState>,
    headers: HeaderMap,
    Path((provider_id, channel_id)): Path<(String, String)>,
    Json(body): Json<SetChannelEnabledRequest>,
) -> AppResult<impl IntoResponse> {
    require_admin(&headers, &state).await?;

    let updated = state
        .monoize_store
        .set_channel_enabled(&provider_id, &channel_id, body.enabled)
        .await
        .map_err(|e| AppError::new(StatusCode::INTERNAL_SERVER_ERROR, "internal_error", e))?;
    if !updated {
        return Err(AppError::new(
            StatusCode::NOT_FOUND,
            "not_found",
            "channel not found",
        ));
    }
    advance_routing_config_revision(&state);

    let provider = state
        .monoize_store
        .get_provider(&provider_id)
        .await
        .map_err(|e| AppError::new(StatusCode::INTERNAL_SERVER_ERROR, "internal_error", e))?
        .ok_or_else(|| AppError::new(StatusCode::NOT_FOUND, "not_found", "provider not found"))?;
    Ok(Json(provider_with_runtime(&state, provider).await))
}

pub async fn get_transform_registry(
    State(state): State<AppState>,
    headers: HeaderMap,
) -> AppResult<impl IntoResponse> {
    require_admin(&headers, &state).await?;

    let mut items: Vec<Value> = state
        .transform_registry
        .values()
        .map(|transform| {
            let mut supported_scopes = transform.supported_scopes().to_vec();
            if !supported_scopes
                .iter()
                .any(|scope| matches!(scope, crate::transforms::TransformScope::Global))
            {
                supported_scopes.push(crate::transforms::TransformScope::Global);
            }
            json!({
                "type_id": transform.type_id(),
                "name": localized_text_object(transform.display_name()),
                "description": localized_text_object(transform.display_description()),
                "supported_phases": transform
                    .supported_phases()
                    .iter()
                    .map(|p| serde_json::to_value(p).unwrap_or(Value::String("request".to_string())))
                    .collect::<Vec<_>>(),
                "supported_scopes": supported_scopes
                    .iter()
                    .map(|scope| serde_json::to_value(scope).unwrap_or(Value::String("provider".to_string())))
                    .collect::<Vec<_>>(),
                "config_schema": transform.config_schema(),
            })
        })
        .collect();

    // CJS-REG-1 + SAC-1: admin session already required; list every enabled
    // custom transform. Visibility is returned (CJS-REG-2) and enforced at
    // API-key validate/sanitize (CJS-AKV-2), not here.
    let snapshot = state.custom_transform_store.snapshot();
    for entry in snapshot.values() {
        items.push(json!({
            "type_id": entry.id,
            "name": { "en": entry.name, "zh": entry.name },
            "description": { "en": entry.description, "zh": entry.description },
            "supported_phases": entry.phases,
            "supported_scopes": entry.scopes,
            "config_schema": entry
                .config_schema
                .clone()
                .unwrap_or_else(crate::custom_transforms::default_config_schema),
            "custom": true,
            "visibility": entry.visibility,
        }));
    }

    items.sort_by(|a, b| a["type_id"].as_str().cmp(&b["type_id"].as_str()));
    Ok(Json(items))
}

fn localized_text_object(entries: crate::transforms::LocalizedText) -> Value {
    let mut map = serde_json::Map::new();
    for (language, text) in entries {
        map.insert((*language).to_string(), Value::String((*text).to_string()));
    }
    Value::Object(map)
}

pub async fn get_provider_presets(
    State(state): State<AppState>,
    headers: HeaderMap,
) -> AppResult<impl IntoResponse> {
    require_admin(&headers, &state).await?;
    Ok(Json(crate::presets::provider_presets()))
}

mod probes;
pub use probes::{
    FetchChannelModelsRequest, TestChannelRequest, fetch_channel_models, fetch_provider_models,
    probe_channel_websocket, test_channel,
};

mod runtime;
use runtime::*;
