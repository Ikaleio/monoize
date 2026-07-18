use crate::app::AppState;
use crate::billing_rate_store::DbBillingRateRecord;
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
use serde::Deserialize;
use serde_json::{Value, json};
use std::collections::{HashMap, HashSet};

fn apply_channel_runtime(channel: &mut MonoizeChannel, health: &ChannelHealthState) {
    let now = chrono::Utc::now().timestamp();
    channel._healthy = Some(health.healthy);
    channel._health_status = Some(health.status(now).to_string());
    channel._last_success_at = health
        .last_success_at
        .and_then(|ts| chrono::DateTime::<chrono::Utc>::from_timestamp(ts, 0))
        .map(|t| t.to_rfc3339());
}

async fn provider_with_runtime(state: &AppState, mut provider: MonoizeProvider) -> MonoizeProvider {
    if !provider.circuit_breaker_enabled {
        for channel in &mut provider.channels {
            apply_channel_runtime(channel, &ChannelHealthState::new());
        }
        return provider;
    }
    let health = state.channel_health.lock().await;
    for channel in &mut provider.channels {
        let now = chrono::Utc::now().timestamp();
        let states: Vec<ChannelHealthState> = if provider.per_model_circuit_break {
            channel
                .models
                .keys()
                .map(|model| {
                    health
                        .get(&health_key(&channel.id, Some(model)))
                        .cloned()
                        .unwrap_or_else(ChannelHealthState::new)
                })
                .collect()
        } else {
            vec![
                health
                    .get(&health_key(&channel.id, None))
                    .cloned()
                    .unwrap_or_else(ChannelHealthState::new),
            ]
        };
        let state = states
            .iter()
            .find(|state| state.status(now) == "unhealthy")
            .or_else(|| states.iter().find(|state| state.status(now) == "probing"))
            .or_else(|| states.first())
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
    health.retain(|channel_key, _| {
        !ids.iter().any(|channel_id| {
            channel_key.as_str() == *channel_id
                || channel_key.starts_with(&format!("{channel_id}::"))
        })
    });
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

fn parse_u64_value(value: &Value) -> Option<u64> {
    value
        .as_u64()
        .or_else(|| value.as_i64().and_then(|v| u64::try_from(v).ok()))
        .or_else(|| value.as_str().and_then(|s| s.parse::<u64>().ok()))
}

pub(super) fn provider_dashboard_rate_matrix_is_complete(rates: &[DbBillingRateRecord]) -> bool {
    let has_input = rates
        .iter()
        .any(|r| r.rate_kind == "token" && r.usage_class == "input_uncached");
    let has_output = rates
        .iter()
        .any(|r| r.rate_kind == "token" && r.usage_class == "output");
    if !has_input || !has_output {
        return false;
    }

    let context_tiers: HashSet<&str> = rates
        .iter()
        .filter_map(|r| r.context_tier.as_deref())
        .filter(|tier| *tier != "default")
        .collect();
    if context_tiers.is_empty() {
        return true;
    }

    let has_threshold = rates
        .iter()
        .filter_map(|r| r.match_json.get("context_threshold_tokens"))
        .any(|value| parse_u64_value(value).is_some());
    if !has_threshold {
        return false;
    }

    context_tiers.iter().all(|tier| {
        ["input_uncached", "output"].iter().all(|usage_class| {
            rates.iter().any(|r| {
                r.rate_kind == "token"
                    && r.usage_class == *usage_class
                    && r.context_tier.as_deref() == Some(*tier)
            })
        })
    })
}

async fn dashboard_billing_matrix_available_for_model(
    state: &AppState,
    pricing_patterns: &[crate::settings::PricingProfilePattern],
    cache: &mut HashMap<(String, String), bool>,
    model: &str,
    provider_type: &str,
) -> Result<bool, String> {
    let key = (model.to_string(), provider_type.to_string());
    if let Some(cached) = cache.get(&key) {
        return Ok(*cached);
    }
    let mut candidate_profiles = Vec::new();
    if let Some(pricing_profile) =
        crate::billing_rate_store::select_pricing_profile(pricing_patterns, model)
    {
        candidate_profiles.push(pricing_profile.to_string());
    }
    if let Some(metadata_profile) =
        dashboard_metadata_pricing_profile_for_model(state, model).await?
        && !candidate_profiles
            .iter()
            .any(|candidate| candidate == &metadata_profile)
    {
        candidate_profiles.push(metadata_profile);
    }

    for pricing_profile in candidate_profiles {
        let rates = state
            .billing_rate_store
            .list_matching_rates(&pricing_profile, Some(provider_type), model)
            .await?;
        if provider_dashboard_rate_matrix_is_complete(&rates) {
            cache.insert(key, true);
            return Ok(true);
        }
    }
    cache.insert(key, false);
    Ok(false)
}

async fn dashboard_metadata_pricing_profile_for_model(
    state: &AppState,
    model: &str,
) -> Result<Option<String>, String> {
    Ok(state
        .model_registry_store
        .get_model_metadata(model)
        .await?
        .and_then(|record| record.models_dev_provider)
        .map(|profile| profile.trim().to_string())
        .filter(|profile| !profile.is_empty()))
}

async fn channel_model_has_billable_rate_matrix(
    state: &AppState,
    pricing_patterns: &[crate::settings::PricingProfilePattern],
    cache: &mut HashMap<(String, String), bool>,
    provider: &MonoizeProvider,
    channel: &MonoizeChannel,
    logical_model: &str,
    model_entry: &crate::monoize_routing::MonoizeModelEntry,
    reasoning_suffix_map: &HashMap<String, String>,
) -> Result<bool, String> {
    let upstream_model = provider_pricing_model(logical_model, model_entry);
    let normalized_upstream_model =
        normalize_pricing_model_key(upstream_model, reasoning_suffix_map);
    let normalized_logical_model = normalize_pricing_model_key(logical_model, reasoning_suffix_map);
    let effective_type = crate::monoize_routing::resolve_effective_api_type(
        &provider.api_type_overrides,
        channel.provider_type,
        logical_model,
    );
    for provider_type in [effective_type.as_str()] {
        if dashboard_billing_matrix_available_for_model(
            state,
            pricing_patterns,
            cache,
            &normalized_upstream_model,
            provider_type,
        )
        .await?
        {
            return Ok(true);
        }
        if normalized_upstream_model != normalized_logical_model
            && dashboard_billing_matrix_available_for_model(
                state,
                pricing_patterns,
                cache,
                &normalized_logical_model,
                provider_type,
            )
            .await?
        {
            return Ok(true);
        }
    }

    Ok(false)
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

    let reasoning_suffix_map = state
        .settings_store
        .get_reasoning_suffix_map()
        .await
        .unwrap_or_default();
    let pricing_patterns = state
        .settings_store
        .get_pricing_profile_model_patterns()
        .await
        .map_err(|e| AppError::new(StatusCode::INTERNAL_SERVER_ERROR, "internal_error", e))?;
    let mut rate_matrix_cache: HashMap<(String, String), bool> = HashMap::new();

    let mut out = Vec::with_capacity(providers.len());
    for provider in providers {
        let mut unpriced_model_ids = Vec::new();
        for channel in &provider.channels {
            for (logical_model, model_entry) in &channel.models {
                let has_pricing = channel_model_has_billable_rate_matrix(
                    &state,
                    &pricing_patterns,
                    &mut rate_matrix_cache,
                    &provider,
                    channel,
                    logical_model,
                    model_entry,
                    &reasoning_suffix_map,
                )
                .await
                .map_err(|e| {
                    AppError::new(StatusCode::INTERNAL_SERVER_ERROR, "internal_error", e)
                })?;
                if !has_pricing {
                    unpriced_model_ids.push(logical_model.clone());
                }
            }
        }
        unpriced_model_ids.sort();
        unpriced_model_ids.dedup();
        let unpriced_count = unpriced_model_ids.len();
        let p = provider_with_runtime(&state, provider).await;
        let val = serde_json::to_value(&p).unwrap_or_default();
        if let Value::Object(mut obj) = val {
            obj.insert(
                "unpriced_model_count".to_string(),
                Value::Number(serde_json::Number::from(unpriced_count)),
            );
            obj.insert(
                "unpriced_model_ids".to_string(),
                Value::Array(unpriced_model_ids.into_iter().map(Value::String).collect()),
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

    state
        .name_caches
        .providers
        .insert(provider.id.clone(), provider.name.clone());
    for ch in &provider.channels {
        state
            .name_caches
            .channels
            .insert(ch.id.clone(), ch.name.clone());
    }

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

    state
        .name_caches
        .providers
        .insert(provider.id.clone(), provider.name.clone());
    for ch in &provider.channels {
        state
            .name_caches
            .channels
            .insert(ch.id.clone(), ch.name.clone());
    }

    let next_channel_ids: std::collections::HashSet<&str> =
        provider.channels.iter().map(|ch| ch.id.as_str()).collect();
    let removed_channel_ids: Vec<String> = prev_provider
        .channels
        .iter()
        .filter(|ch| !next_channel_ids.contains(ch.id.as_str()))
        .map(|ch| ch.id.clone())
        .collect();
    prune_provider_channel_health(&state, &removed_channel_ids).await;
    if !provider.circuit_breaker_enabled {
        let all_channel_ids: Vec<String> =
            provider.channels.iter().map(|ch| ch.id.clone()).collect();
        prune_provider_channel_health(&state, &all_channel_ids).await;
    }
    for id in &removed_channel_ids {
        state.name_caches.channels.remove(id);
    }

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

    state.name_caches.providers.remove(&provider_id);

    let removed_channel_ids: Vec<String> = existing_provider
        .channels
        .iter()
        .map(|ch| ch.id.clone())
        .collect();
    prune_provider_channel_health(&state, &removed_channel_ids).await;
    for id in &removed_channel_ids {
        state.name_caches.channels.remove(id);
    }

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
        crate::monoize_routing::MonoizeProviderType::Gemini => {
            build_gemini_models_list_url(base_url)
        }
        _ => build_models_list_url(base_url),
    };

    let mut request = state
        .http
        .get(&url)
        .timeout(std::time::Duration::from_secs(15));
    request = match body.provider_type {
        crate::monoize_routing::MonoizeProviderType::Gemini => {
            request.header("x-goog-api-key", api_key.as_str())
        }
        _ => request.header("Authorization", format!("Bearer {api_key}")),
    };

    let resp = request.send().await.map_err(|e| {
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

    let models: Vec<String> = extract_model_ids(body.provider_type, &resp_body);

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

    let first_supported_model = channel.models.keys().min().cloned();

    let probe_model = requested_model
        .or_else(|| channel.active_probe_model_override.clone())
        .or_else(|| provider.active_probe_model_override.clone())
        .or_else(|| settings.monoize_active_probe_model.clone())
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

    let started_at = std::time::Instant::now();
    let (ok, _usage) = crate::monoize_routing::probe_channel_completion(
        &state.http,
        channel,
        state.monoize_runtime.read().await.request_timeout_ms,
        &upstream_model,
        channel.provider_type,
        &provider.api_type_overrides,
    )
    .await;
    let latency_ms = started_at.elapsed().as_millis() as u64;

    if ok {
        let now = chrono::Utc::now().timestamp();
        let mut health = state.channel_health.lock().await;
        let prefix = format!("{channel_id}::");
        let keys: Vec<String> = health
            .keys()
            .filter(|key| key.as_str() == channel_id || key.starts_with(&prefix))
            .cloned()
            .collect();
        if keys.is_empty() {
            let entry = health
                .entry(health_key(&channel_id, None))
                .or_insert_with(ChannelHealthState::new);
            entry.healthy = true;
            entry.cooldown_until = None;
            entry.last_success_at = Some(now);
            entry.probe_success_count = 0;
            entry.last_probe_at = None;
        } else {
            for key in keys {
                if let Some(entry) = health.get_mut(&key) {
                    entry.healthy = true;
                    entry.cooldown_until = None;
                    entry.last_success_at = Some(now);
                    entry.probe_success_count = 0;
                    entry.last_probe_at = None;
                }
            }
        }
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
            let mut supported_scopes = transform.supported_scopes().to_vec();
            if !supported_scopes
                .iter()
                .any(|scope| matches!(scope, crate::transforms::TransformScope::Global))
            {
                supported_scopes.push(crate::transforms::TransformScope::Global);
            }
            json!({
                "type_id": transform.type_id(),
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

pub(super) fn build_gemini_models_list_url(base_url: &str) -> String {
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
        _ => body
            .get("data")
            .and_then(|d| d.as_array())
            .into_iter()
            .flatten()
            .filter_map(|item| item.get("id").and_then(|id| id.as_str()).map(String::from))
            .filter(|id| seen.insert(id.clone()))
            .collect(),
    };
    models.sort();
    models
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::app::{RuntimeConfig, load_state_with_runtime};
    use crate::monoize_routing::{
        CreateMonoizeChannelInput, CreateMonoizeProviderInput, MonoizeModelEntry,
        MonoizeProviderType,
    };
    use crate::users::UserRole;
    use axum::Json;
    use axum::extract::State;
    use axum::http::{HeaderMap, HeaderValue, StatusCode};
    use axum::response::IntoResponse;
    use axum::routing::get;
    use http_body_util::BodyExt;
    use serde_json::{Value, json};
    use std::collections::HashMap;
    use std::sync::{Arc, Mutex};

    async fn start_models_list_server() -> (String, Arc<Mutex<Vec<String>>>) {
        let captured_auth = Arc::new(Mutex::new(Vec::new()));

        async fn models(
            State(captured_auth): State<Arc<Mutex<Vec<String>>>>,
            headers: HeaderMap,
        ) -> Json<Value> {
            if let Some(auth) = headers
                .get("authorization")
                .and_then(|value| value.to_str().ok())
            {
                captured_auth.lock().unwrap().push(auth.to_string());
            }

            Json(json!({
                "data": [
                    { "id": "zeta-model" },
                    { "id": "alpha-model" },
                    { "id": "alpha-model" }
                ]
            }))
        }

        let router = axum::Router::new()
            .route("/v1/models", get(models))
            .with_state(Arc::clone(&captured_auth));
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0")
            .await
            .expect("bind model list server");
        let addr = listener.local_addr().expect("local addr");

        tokio::spawn(async move {
            axum::serve(listener, router)
                .await
                .expect("model list server");
        });

        (format!("http://{addr}"), captured_auth)
    }

    fn test_provider_input(base_url: String) -> CreateMonoizeProviderInput {
        CreateMonoizeProviderInput {
            name: "provider".to_string(),
            enabled: true,
            priority: Some(0),
            max_retries: -1,
            channel_max_retries: 0,
            channel_retry_interval_ms: 0,
            circuit_breaker_enabled: true,
            per_model_circuit_break: false,
            channels: vec![CreateMonoizeChannelInput {
                id: None,
                name: "channel".to_string(),
                provider_type: MonoizeProviderType::ChatCompletion,
                base_url,
                api_key: Some("stored-secret".to_string()),
                weight: 1,
                enabled: true,
                passive_failure_count_threshold_override: None,
                passive_cooldown_seconds_override: None,
                passive_window_seconds_override: None,
                passive_rate_limit_cooldown_seconds_override: None,
                models: HashMap::from([(
                    "alpha-model".to_string(),
                    MonoizeModelEntry {
                        redirect: None,
                        multiplier: 1.0,
                    },
                )]),
                active_probe_enabled_override: None,
                active_probe_interval_seconds_override: None,
                active_probe_success_threshold_override: None,
                active_probe_model_override: None,
            }],
            groups: Vec::new(),
            transforms: Vec::new(),
            api_type_overrides: Vec::new(),
            active_probe_enabled_override: None,
            active_probe_interval_seconds_override: None,
            active_probe_success_threshold_override: None,
            active_probe_model_override: None,
            request_timeout_ms_override: None,
            extra_fields_whitelist: None,
            strip_cross_protocol_nested_extra: None,
        }
    }

    #[tokio::test]
    async fn fetch_channel_models_uses_stored_key_for_existing_channel() {
        let (base_url, captured_auth) = start_models_list_server().await;
        let state = load_state_with_runtime(RuntimeConfig {
            listen: "127.0.0.1:0".to_string(),
            metrics_path: "/metrics".to_string(),
            database_dsn: "sqlite::memory:".to_string(),
        })
        .await
        .expect("state loads");

        let admin = state
            .user_store
            .create_user("admin_user", "password123", UserRole::Admin, &[])
            .await
            .expect("admin created");
        let session = state
            .user_store
            .create_session(&admin.id, 7)
            .await
            .expect("session created");
        let provider = state
            .monoize_store
            .create_provider(test_provider_input(base_url.clone()))
            .await
            .expect("provider created");
        let channel_id = provider.channels[0].id.clone();

        let mut headers = HeaderMap::new();
        headers.insert(
            "authorization",
            HeaderValue::from_str(&format!("Bearer {}", session.token)).expect("auth header"),
        );
        let response = fetch_channel_models(
            State(state),
            headers,
            Json(FetchChannelModelsRequest {
                provider_type: MonoizeProviderType::ChatCompletion,
                base_url,
                api_key: None,
                provider_id: Some(provider.id),
                channel_id: Some(channel_id),
            }),
        )
        .await
        .expect("fetch succeeds")
        .into_response();

        assert_eq!(response.status(), StatusCode::OK);
        let bytes = response
            .into_body()
            .collect()
            .await
            .expect("response body")
            .to_bytes();
        let body: Value = serde_json::from_slice(&bytes).expect("json body");
        assert_eq!(
            body["models"],
            json!(["alpha-model".to_string(), "zeta-model".to_string()])
        );
        assert_eq!(
            captured_auth.lock().unwrap().as_slice(),
            &["Bearer stored-secret".to_string()]
        );
    }
}
