use crate::app::AppState;
use crate::billing_rate_store::{DbBillingRateRecord, glob_matches, select_pricing_profile};
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
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use std::collections::{HashMap, HashSet};
use std::sync::atomic::Ordering;

fn discovery_body_error(error: crate::bounded_response::BoundedResponseError) -> AppError {
    let code = if error.is_limit_exceeded() {
        "upstream_discovery_response_too_large"
    } else {
        "upstream_fetch_failed"
    };
    AppError::new(StatusCode::BAD_GATEWAY, code, error.to_string())
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

fn reset_channel_health_after_successful_test<'a>(
    health: &mut HashMap<String, ChannelHealthState>,
    channel_id: &str,
    per_model_circuit_break: bool,
    model_id: &'a str,
    now: i64,
    max_entries: usize,
) {
    fn reset(entry: &mut ChannelHealthState, now: i64) {
        entry.healthy = true;
        entry.cooldown_until = None;
        entry.last_success_at = Some(now);
        entry.probe_success_count = 0;
        entry.last_probe_at = None;
    }

    let key = health_key(channel_id, per_model_circuit_break.then_some(model_id));
    if !health.contains_key(&key) && health.len() >= max_entries {
        return;
    }
    let entry = health.entry(key).or_insert_with(ChannelHealthState::new);
    reset(entry, now);
}

fn health_timestamp(ts: Option<i64>) -> Option<String> {
    ts.and_then(|value| chrono::DateTime::<chrono::Utc>::from_timestamp(value, 0))
        .map(|value| value.to_rfc3339())
}

fn apply_channel_runtime(
    channel: &mut MonoizeChannel,
    health: &ChannelHealthState,
    unhealthy_models: Vec<String>,
    probing_models: Vec<String>,
    cooldown_until: Option<i64>,
    now: i64,
) {
    channel._healthy = Some(health.healthy);
    channel._health_status = Some(health.status(now).to_string());
    channel._last_success_at = health_timestamp(health.last_success_at);
    channel._unhealthy_models = Some(unhealthy_models);
    channel._probing_models = Some(probing_models);
    channel._cooldown_until = health_timestamp(cooldown_until);
}

async fn provider_with_runtime(state: &AppState, mut provider: MonoizeProvider) -> MonoizeProvider {
    let now = chrono::Utc::now().timestamp();
    if !provider.circuit_breaker_enabled {
        for channel in &mut provider.channels {
            apply_channel_runtime(
                channel,
                &ChannelHealthState::new(),
                Vec::new(),
                Vec::new(),
                None,
                now,
            );
        }
        return provider;
    }
    let health = state.channel_health.lock().await;
    for channel in &mut provider.channels {
        let mut unhealthy_models = Vec::new();
        let mut probing_models = Vec::new();
        let states: Vec<ChannelHealthState> = if provider.per_model_circuit_break {
            let mut model_ids: Vec<&String> = channel.models.keys().collect();
            model_ids.sort();
            model_ids
                .into_iter()
                .map(|model| {
                    let state = health
                        .get(&health_key(&channel.id, Some(model)))
                        .cloned()
                        .unwrap_or_else(ChannelHealthState::new);
                    match state.status(now) {
                        "unhealthy" => unhealthy_models.push(model.clone()),
                        "probing" => probing_models.push(model.clone()),
                        _ => {}
                    }
                    state
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
        let cooldown_until = states
            .iter()
            .filter(|state| state.status(now) == "unhealthy")
            .filter_map(|state| state.cooldown_until)
            .max();
        apply_channel_runtime(
            channel,
            &state,
            unhealthy_models,
            probing_models,
            cooldown_until,
            now,
        );
    }
    provider
}

#[derive(Debug, Serialize)]
struct ModelRuntimeChannel {
    channel_id: String,
    channel_name: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    cooldown_until: Option<String>,
}

#[derive(Debug, Serialize)]
struct ProviderModelRuntimeStatus {
    model: String,
    availability_status: &'static str,
    eligible_channel_count: usize,
    available_channel_count: usize,
    breaker_channels: Vec<ModelRuntimeChannel>,
    pricing_status: &'static str,
    unpriced_channels: Vec<ModelRuntimeChannel>,
}

fn sort_runtime_channels(channels: &mut [ModelRuntimeChannel]) {
    channels.sort_by(|left, right| {
        left.channel_name
            .cmp(&right.channel_name)
            .then_with(|| left.channel_id.cmp(&right.channel_id))
    });
}

fn build_provider_model_runtime_statuses(
    provider: &MonoizeProvider,
    unpriced_entries: &HashSet<(String, String)>,
) -> Vec<ProviderModelRuntimeStatus> {
    let mut models: Vec<String> = provider
        .channels
        .iter()
        .flat_map(|channel| channel.models.keys().cloned())
        .collect();
    models.sort();
    models.dedup();

    models
        .into_iter()
        .map(|model| {
            let mapped_channels: Vec<&MonoizeChannel> = provider
                .channels
                .iter()
                .filter(|channel| channel.models.contains_key(&model))
                .collect();
            let eligible_channels: Vec<&MonoizeChannel> = mapped_channels
                .iter()
                .copied()
                .filter(|channel| channel.enabled && channel.weight > 0)
                .collect();
            let mut breaker_channels: Vec<ModelRuntimeChannel> = eligible_channels
                .iter()
                .copied()
                .filter(|channel| {
                    provider.circuit_breaker_enabled
                        && if provider.per_model_circuit_break {
                            channel
                                ._unhealthy_models
                                .as_ref()
                                .is_some_and(|models| models.contains(&model))
                        } else {
                            channel._health_status.as_deref() == Some("unhealthy")
                        }
                })
                .map(|channel| ModelRuntimeChannel {
                    channel_id: channel.id.clone(),
                    channel_name: channel.name.clone(),
                    cooldown_until: channel._cooldown_until.clone(),
                })
                .collect();
            sort_runtime_channels(&mut breaker_channels);

            let eligible_channel_count = eligible_channels.len();
            let available_channel_count = eligible_channel_count - breaker_channels.len();
            let availability_status = if eligible_channel_count > 0 && available_channel_count == 0
            {
                "unavailable"
            } else if available_channel_count < eligible_channel_count {
                "degraded"
            } else {
                "healthy"
            };

            let mut unpriced_channels: Vec<ModelRuntimeChannel> = mapped_channels
                .iter()
                .copied()
                .filter(|channel| unpriced_entries.contains(&(channel.id.clone(), model.clone())))
                .map(|channel| ModelRuntimeChannel {
                    channel_id: channel.id.clone(),
                    channel_name: channel.name.clone(),
                    cooldown_until: None,
                })
                .collect();
            sort_runtime_channels(&mut unpriced_channels);
            let pricing_status = if unpriced_channels.is_empty() {
                "complete"
            } else if unpriced_channels.len() == mapped_channels.len() {
                "missing"
            } else {
                "partial"
            };

            ProviderModelRuntimeStatus {
                model,
                availability_status,
                eligible_channel_count,
                available_channel_count,
                breaker_channels,
                pricing_status,
                unpriced_channels,
            }
        })
        .collect()
}

async fn prune_provider_channel_health(state: &AppState, channel_ids: &[String]) {
    if channel_ids.is_empty() {
        return;
    }
    let ids: std::collections::HashSet<&str> = channel_ids.iter().map(String::as_str).collect();
    let mut health = state.channel_health.lock().await;
    health.retain(|channel_key, _| {
        let channel_id = channel_key
            .split_once("::")
            .map(|(channel_id, _)| channel_id)
            .unwrap_or(channel_key.as_str());
        !ids.contains(channel_id)
    });
}

async fn prune_provider_channel_affinity(state: &AppState, channel_ids: &[String]) {
    if channel_ids.is_empty() {
        return;
    }
    let ids: HashSet<&str> = channel_ids.iter().map(String::as_str).collect();
    state
        .channel_affinity
        .lock()
        .await
        .retain(|_, binding| !ids.contains(binding.channel_id.as_str()));
}

fn advance_routing_config_revision(state: &AppState) {
    state.routing_config_revision.fetch_add(1, Ordering::AcqRel);
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

fn dashboard_candidate_profiles(
    pricing_patterns: &[crate::settings::PricingProfilePattern],
    metadata_profiles: &HashMap<String, String>,
    model: &str,
) -> Vec<String> {
    let mut candidate_profiles = Vec::new();
    if let Some(pricing_profile) = select_pricing_profile(pricing_patterns, model) {
        candidate_profiles.push(pricing_profile.to_string());
    }
    if let Some(metadata_profile) = metadata_profiles.get(model)
        && !candidate_profiles
            .iter()
            .any(|candidate| candidate == metadata_profile)
    {
        candidate_profiles.push(metadata_profile.clone());
    }
    candidate_profiles
}

pub(super) fn build_dashboard_rate_matrix_cache(
    pairs: &HashSet<(String, String)>,
    pricing_patterns: &[crate::settings::PricingProfilePattern],
    metadata_profiles: &HashMap<String, String>,
    candidate_rates: &[DbBillingRateRecord],
) -> HashMap<(String, String), bool> {
    let mut cache = HashMap::with_capacity(pairs.len());
    for (model, provider_type) in pairs {
        let available = dashboard_candidate_profiles(pricing_patterns, metadata_profiles, model)
            .into_iter()
            .any(|profile| {
                let rates = candidate_rates
                    .iter()
                    .filter(|rate| {
                        rate.pricing_profile == profile
                            && rate
                                .provider_type
                                .as_deref()
                                .is_none_or(|value| value == provider_type)
                            && rate
                                .model_pattern
                                .as_deref()
                                .is_none_or(|pattern| glob_matches(pattern, model))
                    })
                    .cloned()
                    .collect::<Vec<_>>();
                provider_dashboard_rate_matrix_is_complete(&rates)
            });
        cache.insert((model.clone(), provider_type.clone()), available);
    }
    cache
}

fn channel_model_has_billable_rate_matrix(
    cache: &HashMap<(String, String), bool>,
    provider: &MonoizeProvider,
    channel: &MonoizeChannel,
    logical_model: &str,
    model_entry: &crate::monoize_routing::MonoizeModelEntry,
    reasoning_suffix_map: &HashMap<String, String>,
) -> bool {
    let upstream_model = provider_pricing_model(logical_model, model_entry);
    let normalized_upstream_model =
        normalize_pricing_model_key(upstream_model, reasoning_suffix_map);
    let normalized_logical_model = normalize_pricing_model_key(logical_model, reasoning_suffix_map);
    let effective_type = crate::monoize_routing::resolve_effective_api_type(
        &provider.api_type_overrides,
        channel.provider_type,
        logical_model,
    );
    let provider_type = effective_type.as_str().to_string();
    if cache
        .get(&(normalized_upstream_model.clone(), provider_type.clone()))
        .copied()
        .unwrap_or(false)
    {
        return true;
    }
    normalized_upstream_model != normalized_logical_model
        && cache
            .get(&(normalized_logical_model, provider_type))
            .copied()
            .unwrap_or(false)
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

    let (reasoning_suffix_map, pricing_patterns) = {
        let runtime = state.monoize_runtime.read().await;
        (
            runtime.reasoning_suffix_map.clone(),
            runtime.pricing_profile_model_patterns.clone(),
        )
    };
    let mut pricing_pairs = HashSet::new();
    for provider in &providers {
        for channel in &provider.channels {
            for (logical_model, model_entry) in &channel.models {
                let normalized_upstream_model = normalize_pricing_model_key(
                    provider_pricing_model(logical_model, model_entry),
                    &reasoning_suffix_map,
                );
                let normalized_logical_model =
                    normalize_pricing_model_key(logical_model, &reasoning_suffix_map);
                let provider_type = crate::monoize_routing::resolve_effective_api_type(
                    &provider.api_type_overrides,
                    channel.provider_type,
                    logical_model,
                )
                .as_str()
                .to_string();
                pricing_pairs.insert((normalized_upstream_model.clone(), provider_type.clone()));
                if normalized_upstream_model != normalized_logical_model {
                    pricing_pairs.insert((normalized_logical_model, provider_type));
                }
            }
        }
    }
    let mut pricing_models = pricing_pairs
        .iter()
        .map(|(model, _)| model.clone())
        .collect::<Vec<_>>();
    pricing_models.sort();
    pricing_models.dedup();
    let metadata_profiles = state
        .model_registry_store
        .list_model_metadata_pricing_profiles(&pricing_models)
        .await
        .map_err(|e| AppError::new(StatusCode::INTERNAL_SERVER_ERROR, "internal_error", e))?;
    let mut candidate_profiles = pricing_models
        .iter()
        .flat_map(|model| {
            dashboard_candidate_profiles(&pricing_patterns, &metadata_profiles, model)
        })
        .collect::<Vec<_>>();
    candidate_profiles.sort();
    candidate_profiles.dedup();
    let mut provider_types = pricing_pairs
        .iter()
        .map(|(_, provider_type)| provider_type.clone())
        .collect::<Vec<_>>();
    provider_types.sort();
    provider_types.dedup();
    let candidate_rates = state
        .billing_rate_store
        .list_candidate_rates_for_profiles_and_provider_types(&candidate_profiles, &provider_types)
        .await
        .map_err(|e| AppError::new(StatusCode::INTERNAL_SERVER_ERROR, "internal_error", e))?;
    let rate_matrix_cache = build_dashboard_rate_matrix_cache(
        &pricing_pairs,
        &pricing_patterns,
        &metadata_profiles,
        &candidate_rates,
    );

    let mut out = Vec::with_capacity(providers.len());
    for provider in providers {
        let mut unpriced_model_ids = Vec::new();
        let mut unpriced_entries = HashSet::new();
        for channel in &provider.channels {
            for (logical_model, model_entry) in &channel.models {
                let has_pricing = channel_model_has_billable_rate_matrix(
                    &rate_matrix_cache,
                    &provider,
                    channel,
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
#[allow(clippy::result_large_err)]
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

pub async fn create_provider(
    State(state): State<AppState>,
    headers: HeaderMap,
    Json(body): Json<CreateMonoizeProviderInput>,
) -> AppResult<impl IntoResponse> {
    require_admin(&headers, &state).await?;
    validate_channel_proxy_urls(body.channels.iter())?;

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

    let body = parse_discovery_json_response(resp).await?;

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

    let resp_body = parse_discovery_json_response(resp).await?;

    let models: Vec<String> = extract_model_ids(body.provider_type, &resp_body);

    Ok(Json(json!({ "models": models })))
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
                | crate::monoize_routing::MonoizeProviderType::Replicate
        )
    {
        return Err(AppError::new(
            StatusCode::BAD_REQUEST,
            "invalid_request",
            "this channel API type does not support streaming liveness tests",
        ));
    }

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
                        multiplier: crate::exact_decimal::Multiplier::ONE,
                    },
                )]),
                active_probe_enabled_override: None,
                active_probe_interval_seconds_override: None,
                active_probe_success_threshold_override: None,
                active_probe_model_override: None,
                affinity_enabled_override: None,
                affinity_idle_ttl_seconds_override: None,
                affinity_failback_mode_override: None,
                affinity_failback_delay_seconds_override: None,

                proxy_url: None,
                extra_headers: None,
                session_affinity_auto: None,
            }],
            group_ids: Vec::new(),
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

    fn unhealthy_state(last_success_at: i64) -> ChannelHealthState {
        ChannelHealthState {
            healthy: false,
            last_success_at: Some(last_success_at),
            cooldown_until: Some(last_success_at + 100),
            probe_success_count: 4,
            last_probe_at: Some(last_success_at + 1),
            ..ChannelHealthState::new()
        }
    }

    #[test]
    fn discovery_limit_error_maps_to_stable_dashboard_error_code() {
        let error = discovery_body_error(
            crate::bounded_response::BoundedResponseError::DeclaredLengthExceeded {
                content_length: 9,
                max_bytes: 8,
            },
        );

        assert_eq!(error.status, StatusCode::BAD_GATEWAY);
        assert_eq!(error.code, "upstream_discovery_response_too_large");
        assert!(error.message.contains("8-byte limit"));
    }

    #[test]
    fn successful_model_test_resets_only_selected_model_health_key() {
        let mut health = HashMap::from([
            ("channel-a".to_string(), unhealthy_state(1)),
            ("channel-a::alpha".to_string(), unhealthy_state(2)),
            ("channel-a::beta".to_string(), unhealthy_state(3)),
            ("channel-a::removed".to_string(), unhealthy_state(4)),
            ("unrelated::alpha".to_string(), unhealthy_state(5)),
        ]);

        reset_channel_health_after_successful_test(
            &mut health,
            "channel-a",
            true,
            "alpha",
            100,
            100,
        );

        let state = health
            .get("channel-a::alpha")
            .expect("selected model remains present");
        assert!(state.healthy);
        assert_eq!(state.last_success_at, Some(100));
        assert_eq!(state.cooldown_until, None);
        assert_eq!(state.probe_success_count, 0);
        assert_eq!(state.last_probe_at, None);
        assert!(!health["channel-a"].healthy);
        assert!(!health["channel-a::beta"].healthy);
        assert!(!health["channel-a::removed"].healthy);
        assert_eq!(health["channel-a::removed"].last_success_at, Some(4));
        assert!(!health["unrelated::alpha"].healthy);
        assert_eq!(health["unrelated::alpha"].last_success_at, Some(5));
    }

    #[test]
    fn successful_model_test_inserts_selected_model_when_missing() {
        let mut health = HashMap::from([("channel-a::alpha".to_string(), unhealthy_state(1))]);

        reset_channel_health_after_successful_test(
            &mut health,
            "channel-a",
            true,
            "beta",
            100,
            100,
        );

        assert!(!health.contains_key("channel-a"));
        assert!(!health["channel-a::alpha"].healthy);
        assert!(health["channel-a::beta"].healthy);
        assert_eq!(health["channel-a::beta"].last_success_at, Some(100));
    }

    #[test]
    fn successful_test_without_per_model_breaker_resets_only_base_key() {
        let mut health = HashMap::from([
            ("channel-a".to_string(), unhealthy_state(1)),
            ("channel-a::alpha".to_string(), unhealthy_state(2)),
        ]);

        reset_channel_health_after_successful_test(
            &mut health,
            "channel-a",
            false,
            "alpha",
            100,
            100,
        );

        assert!(health["channel-a"].healthy);
        assert_eq!(health["channel-a"].last_success_at, Some(100));
        assert!(!health["channel-a::alpha"].healthy);
        assert_eq!(health["channel-a::alpha"].last_success_at, Some(2));
    }

    #[test]
    fn successful_model_test_inserts_only_model_key_when_capacity_remains() {
        let mut health = HashMap::from([("unrelated".to_string(), unhealthy_state(1))]);

        reset_channel_health_after_successful_test(&mut health, "channel-a", true, "alpha", 100, 2);

        assert_eq!(health.len(), 2);
        assert!(!health.contains_key("channel-a"));
        assert!(health["channel-a::alpha"].healthy);
        assert_eq!(health["channel-a::alpha"].last_success_at, Some(100));
        assert!(!health.contains_key("channel-a::beta"));
    }

    #[test]
    fn successful_test_does_not_insert_when_health_capacity_is_full() {
        let mut health = HashMap::from([("unrelated".to_string(), unhealthy_state(1))]);

        reset_channel_health_after_successful_test(&mut health, "channel-a", true, "alpha", 100, 1);

        assert_eq!(health.len(), 1);
        assert!(!health.contains_key("channel-a"));
        assert!(!health.contains_key("channel-a::alpha"));
        assert!(!health["unrelated"].healthy);
    }

    #[tokio::test]
    async fn fetch_channel_models_uses_stored_key_for_existing_channel() {
        let (base_url, captured_auth) = start_models_list_server().await;
        let state = load_state_with_runtime(RuntimeConfig {
            listen: "127.0.0.1:0".to_string(),
            metrics_path: "/metrics".to_string(),
            database_dsn: "sqlite::memory:".to_string(),
            request_log_spool_dir: None,
            node: crate::node_config::NodeSettings::primary_default(),
        })
        .await
        .expect("state loads");

        let admin = state
            .user_store
            .create_user("admin_user", "password123", UserRole::Admin, None)
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
