use super::*;
use crate::settings::BUILTIN_REASONING_EFFORT_SUFFIXES;
use std::sync::atomic::Ordering;

pub(crate) fn now_ts() -> i64 {
    chrono::Utc::now().timestamp()
}

/// PX6/PX7: effective outbound client for one attempt's Channel (custom proxy wins,
/// then node-global, else direct). Construction failure fails closed.
#[allow(clippy::result_large_err)]
pub(super) fn client_http_for_attempt(
    state: &AppState,
    attempt: &MonoizeAttempt,
) -> Result<reqwest::Client, AppError> {
    state
        .http_clients
        .for_channel_proxy(attempt.proxy_url.as_deref())
        .map_err(|detail| {
            AppError::new(
                StatusCode::BAD_GATEWAY,
                "upstream_proxy_config_invalid",
                detail,
            )
        })
}

pub(crate) fn health_key(channel_id: &str, model: Option<&str>) -> String {
    match model {
        Some(m) => format!("{channel_id}::{m}"),
        None => channel_id.to_string(),
    }
}

pub(super) fn channel_origin_key(base_url: &str) -> Option<String> {
    let parsed = reqwest::Url::parse(base_url.trim()).ok()?;
    let scheme = parsed.scheme().to_ascii_lowercase();
    if scheme != "http" && scheme != "https" {
        return None;
    }
    let host = parsed.host_str()?.to_ascii_lowercase();
    let port = parsed.port_or_known_default()?;
    Some(format!("{scheme}://{host}:{port}"))
}

pub(super) fn is_shared_origin_status(status: Option<u16>) -> bool {
    matches!(status, Some(502 | 503 | 524))
}

fn origin_peer_channel_ids(
    channels: &[crate::monoize_routing::MonoizeChannel],
    origin_key: &str,
) -> Vec<String> {
    channels
        .iter()
        .filter(|channel| channel.enabled && channel.weight > 0)
        .filter(|channel| channel_origin_key(&channel.base_url).as_deref() == Some(origin_key))
        .map(|channel| channel.id.clone())
        .collect()
}

pub(super) fn upstream_path(provider_type: ProviderType) -> &'static str {
    match provider_type {
        ProviderType::Responses => "/v1/responses",
        ProviderType::ChatCompletion => "/v1/chat/completions",
        ProviderType::Messages => "/v1/messages",
        ProviderType::Gemini => "/v1beta/models",
        ProviderType::OpenaiImage => "/v1/images/generations",
        ProviderType::Replicate => "/v1/predictions",
        ProviderType::Group => "/v1/responses",
    }
}

pub(super) fn upstream_path_for_model(
    provider_type: ProviderType,
    model: &str,
    stream: bool,
) -> String {
    match provider_type {
        ProviderType::Gemini => {
            let model = model.trim();
            if stream {
                format!("/v1beta/models/{model}:streamGenerateContent?alt=sse")
            } else {
                format!("/v1beta/models/{model}:generateContent")
            }
        }
        ProviderType::Replicate => {
            let model = model.trim();
            if let Some(stripped) = model.strip_prefix("deployment:") {
                format!("/v1/deployments/{stripped}/predictions")
            } else if model.contains(':') {
                "/v1/predictions".to_string()
            } else {
                format!("/v1/models/{model}/predictions")
            }
        }
        _ => upstream_path(provider_type).to_string(),
    }
}

pub(super) async fn resolve_model_suffix(
    state: &AppState,
    req: &mut urp::UrpRequest,
) -> AppResult<String> {
    let requested_model = req.model.clone();
    let settings_map = state
        .monoize_runtime
        .read()
        .await
        .reasoning_suffix_map
        .clone();
    let normalized =
        normalized_logical_model_for_matching_with_map(state, &requested_model, &settings_map)
            .await?;
    if normalized == requested_model {
        return Ok(normalized);
    }
    req.model = normalized.clone();

    let mut settings_entries: Vec<(&str, &str)> = settings_map
        .iter()
        .map(|(k, v)| (k.as_str(), v.as_str()))
        .collect();
    settings_entries.sort_by(|a, b| b.0.len().cmp(&a.0.len()));

    for (suffix, effort) in settings_entries
        .iter()
        .chain(BUILTIN_REASONING_EFFORT_SUFFIXES.iter())
    {
        if let Some(base) = requested_model.strip_suffix(suffix) {
            if !base.is_empty() {
                match req.reasoning.as_mut() {
                    Some(r) => {
                        if r.effort.is_none() {
                            r.effort = Some(effort.to_string());
                        }
                    }
                    None => {
                        req.reasoning = Some(urp::ReasoningConfig {
                            effort: Some(effort.to_string()),
                            extra_body: std::collections::HashMap::new(),
                        });
                    }
                }
                return Ok(normalized);
            }
        }
    }
    Ok(normalized)
}

async fn normalized_logical_model_for_matching_with_map(
    state: &AppState,
    requested_model: &str,
    settings_map: &std::collections::HashMap<String, String>,
) -> AppResult<String> {
    // Sort by suffix length descending so longer suffixes match first
    // (e.g. "-nothinking" before "-thinking").
    let mut settings_entries: Vec<(&str, &str)> = settings_map
        .iter()
        .map(|(k, v)| (k.as_str(), v.as_str()))
        .collect();
    settings_entries.sort_by(|a, b| b.0.len().cmp(&a.0.len()));

    let mut candidates = vec![requested_model.to_string()];
    for (suffix, _effort) in settings_entries
        .iter()
        .chain(BUILTIN_REASONING_EFFORT_SUFFIXES.iter())
    {
        if let Some(base) = requested_model.strip_suffix(suffix) {
            if !base.is_empty() && !candidates.iter().any(|candidate| candidate == base) {
                candidates.push(base.to_string());
            }
        }
    }
    let available = state
        .monoize_store
        .available_model_names(&candidates)
        .await
        .map_err(|error| {
            AppError::new(StatusCode::INTERNAL_SERVER_ERROR, "internal_error", error)
        })?;
    Ok(candidates
        .into_iter()
        .find(|candidate| available.contains(candidate))
        .unwrap_or_else(|| requested_model.to_string()))
}

pub(super) async fn build_monoize_attempts(
    state: &AppState,
    urp: &UrpRequest,
    auth: &crate::auth::AuthResult,
) -> AppResult<Vec<MonoizeAttempt>> {
    build_monoize_attempts_for_provider_type(state, urp, auth, None).await
}

pub(super) async fn build_monoize_attempts_for_provider_type(
    state: &AppState,
    urp: &UrpRequest,
    auth: &crate::auth::AuthResult,
    required_provider_type: Option<ProviderType>,
) -> AppResult<Vec<MonoizeAttempt>> {
    let routing_config_revision = state.routing_config_revision.load(Ordering::Acquire);
    let providers = state
        .monoize_store
        .list_providers_for_model(&urp.model)
        .await
        .map_err(|e| AppError::new(StatusCode::INTERNAL_SERVER_ERROR, "provider_store_error", e))?;
    let mut attempts = Vec::new();
    for provider in providers {
        collect_provider_attempts(
            state,
            urp,
            &auth.effective_groups,
            &provider,
            routing_config_revision,
            &mut attempts,
        )
        .await;
    }
    if let Some(required_provider_type) = required_provider_type {
        attempts.retain(|attempt| attempt.provider_type == required_provider_type);
    }
    if attempts.is_empty() {
        return Ok(attempts);
    }

    let pricing_inputs = attempts
        .iter()
        .map(|attempt| (attempt.upstream_model.clone(), attempt.provider_type))
        .collect::<Vec<_>>();
    let pricing_snapshot =
        build_billing_rate_resolution_snapshot(state, &pricing_inputs, &urp.model).await?;

    let mut pricing_cache: std::collections::HashMap<
        (String, String, String),
        Result<Option<BillingRateResolution>, String>,
    > = std::collections::HashMap::new();
    let mut blocked_models: std::collections::BTreeSet<String> = std::collections::BTreeSet::new();
    let mut blocked_meter_errors: std::collections::BTreeSet<String> =
        std::collections::BTreeSet::new();
    let mut allowed_attempts = Vec::with_capacity(attempts.len());

    for mut attempt in attempts {
        let tools_key = urp.server_tool_usage_classes.join(",");
        let cache_key = (
            attempt.upstream_model.clone(),
            urp.model.clone(),
            format!(
                "{}:{tools_key}",
                reasoning_envelope_provider_type(attempt.provider_type)
            ),
        );
        let pricing = if let Some(cached) = pricing_cache.get(&cache_key) {
            cached.clone()
        } else {
            let priced = match pricing_snapshot.resolve(
                &attempt.upstream_model,
                &urp.model,
                attempt.provider_type,
            ) {
                Some(resolution) => {
                    let allowed = billing_rate_matrix_allows_request(
                        &resolution,
                        &urp.server_tool_usage_classes,
                    );
                    match allowed {
                        Ok(true) => Ok(Some(resolution)),
                        Ok(false) => Ok(None),
                        Err(error) => Err(error),
                    }
                }
                None => {
                    if urp.server_tool_usage_classes.is_empty() {
                        Ok(None)
                    } else {
                        Err(format!(
                            "meter rate required for server-native tool usage class: {}",
                            urp.server_tool_usage_classes.join(", ")
                        ))
                    }
                }
            };
            pricing_cache.insert(cache_key, priced.clone());
            priced
        };

        match pricing {
            Err(err) => {
                blocked_meter_errors.insert(err);
            }
            Ok(Some(resolution)) => {
                attempt.billable_pricing_available = true;
                attempt.billing_rate_resolution = Some(resolution);
                allowed_attempts.push(attempt);
            }
            Ok(None) => {
                blocked_models.insert(attempt.upstream_model);
            }
        }
    }

    if !blocked_meter_errors.is_empty() {
        let blocked_list = blocked_meter_errors
            .into_iter()
            .collect::<Vec<_>>()
            .join(", ");
        return Err(AppError::new(
            StatusCode::FORBIDDEN,
            "model_pricing_required",
            blocked_list,
        ));
    }

    if allowed_attempts.is_empty() && !blocked_models.is_empty() {
        let blocked_list = blocked_models.into_iter().collect::<Vec<_>>().join(", ");
        return Err(AppError::new(
            StatusCode::FORBIDDEN,
            "model_pricing_required",
            format!("pricing metadata required for model(s): {blocked_list}"),
        ));
    }
    apply_channel_affinity(state, urp, auth, allowed_attempts).await
}

fn affinity_tenant(auth: &crate::auth::AuthResult) -> Option<String> {
    auth.api_key_id
        .as_ref()
        .map(|id| format!("api_key:{id}"))
        .or_else(|| auth.user_id.as_ref().map(|id| format!("user:{id}")))
}

pub(super) fn affinity_key_for_request(
    urp: &UrpRequest,
    auth: &crate::auth::AuthResult,
) -> Option<(String, String)> {
    let tenant = affinity_tenant(auth)?;
    let source = urp
        .affinity_explicit
        .as_ref()
        .map(|value| format!("explicit:{value}"))
        .unwrap_or_else(|| format!("prefix:{}", urp.affinity_prefix_hash));
    let key = format!("v1|{tenant}|model:{}|{source}", urp.model);
    let key_hash = format!("{:016x}", xxhash_rust::xxh3::xxh3_64(key.as_bytes()));
    Some((key, key_hash))
}

pub(super) fn response_id_affinity_key(
    logical_model: &str,
    response_id: &str,
    auth: &crate::auth::AuthResult,
) -> Option<String> {
    let tenant = affinity_tenant(auth)?;
    Some(format!(
        "v1|{tenant}|model:{logical_model}|explicit:previous_response_id:{response_id}"
    ))
}

pub(super) async fn apply_channel_affinity(
    state: &AppState,
    urp: &UrpRequest,
    auth: &crate::auth::AuthResult,
    mut attempts: Vec<MonoizeAttempt>,
) -> AppResult<Vec<MonoizeAttempt>> {
    let Some((key, key_hash)) = affinity_key_for_request(urp, auth) else {
        return Ok(attempts);
    };
    let now = now_ts();
    let binding = {
        let mut guard = state.channel_affinity.lock().await;
        let expired = guard
            .get(&key)
            .is_some_and(|binding| now >= binding.expires_at);
        if expired {
            guard.remove(&key);
            None
        } else {
            guard.get(&key).cloned()
        }
    };
    let had_binding = binding.is_some();

    let mut bound_target = None;
    if let Some(binding) = binding {
        let target = format!("{}/{}", binding.provider_id, binding.channel_id);
        if let Some(pos) = attempts.iter().position(|attempt| {
            attempt.provider_id == binding.provider_id && attempt.channel_id == binding.channel_id
        }) {
            let bound_attempt = &attempts[pos];
            if !bound_attempt.affinity_enabled {
                state.channel_affinity.lock().await.remove(&key);
            } else {
                let failback_delay = i64::try_from(bound_attempt.affinity_failback_delay_seconds)
                    .unwrap_or(i64::MAX);
                let has_earlier_provider = attempts[..pos]
                    .iter()
                    .any(|attempt| attempt.provider_id != binding.provider_id);
                let failback_due = bound_attempt.affinity_failback_mode
                    == crate::monoize_routing::AffinityFailbackMode::PreferHigherPriority
                    && now.saturating_sub(binding.bound_at) >= failback_delay
                    && has_earlier_provider;
                if !failback_due {
                    let mut attempt = attempts.remove(pos);
                    attempt.affinity_key = Some(key.clone());
                    attempt.affinity_key_hash = Some(key_hash.clone());
                    attempt.affinity_hit = Some(true);
                    attempt.affinity_target = Some(target.clone());
                    attempts.insert(0, attempt);
                }
                bound_target = Some(target);
            }
        } else {
            bound_target = Some(target);
        }
    }

    let affinity_should_run =
        had_binding || attempts.iter().any(|attempt| attempt.affinity_enabled);
    if !affinity_should_run {
        return Ok(attempts);
    }
    for attempt in &mut attempts {
        if attempt.affinity_key.is_none() {
            attempt.affinity_key = Some(key.clone());
            attempt.affinity_key_hash = Some(key_hash.clone());
            attempt.affinity_hit = Some(false);
            attempt.affinity_target = bound_target
                .clone()
                .or_else(|| Some(format!("{}/{}", attempt.provider_id, attempt.channel_id)));
        }
    }

    Ok(attempts)
}

fn insert_channel_affinity(
    cache: &mut std::collections::HashMap<String, crate::monoize_routing::ChannelAffinityBinding>,
    key: String,
    binding: crate::monoize_routing::ChannelAffinityBinding,
) {
    insert_channel_affinity_with_limit(
        cache,
        key,
        binding,
        crate::monoize_routing::channel_affinity_max_entries(),
    );
}

pub(super) fn insert_channel_affinity_with_limit(
    cache: &mut std::collections::HashMap<String, crate::monoize_routing::ChannelAffinityBinding>,
    key: String,
    binding: crate::monoize_routing::ChannelAffinityBinding,
    limit: usize,
) {
    if !cache.contains_key(&key) && cache.len() >= limit {
        return;
    }
    cache.insert(key, binding);
}

fn channel_affinity_expires_at(now: i64, idle_ttl_seconds: u64) -> i64 {
    let idle_ttl_seconds = i64::try_from(idle_ttl_seconds).unwrap_or(i64::MAX);
    now.saturating_add(idle_ttl_seconds.max(1))
}

pub(super) async fn refresh_channel_affinity(state: &AppState, attempt: &MonoizeAttempt) {
    let Some(key) = attempt.affinity_key.as_ref() else {
        return;
    };
    let mut guard = state.channel_affinity.lock().await;
    if state.routing_config_revision.load(Ordering::Acquire) != attempt.routing_config_revision {
        return;
    }
    if !attempt.affinity_enabled {
        guard.remove(key);
        return;
    }
    let now = now_ts();
    let bound_at = if attempt.affinity_hit == Some(true) {
        guard
            .get(key)
            .filter(|binding| {
                binding.provider_id == attempt.provider_id
                    && binding.channel_id == attempt.channel_id
            })
            .map(|binding| binding.bound_at)
            .unwrap_or(now)
    } else {
        now
    };
    insert_channel_affinity(
        &mut guard,
        key.clone(),
        crate::monoize_routing::ChannelAffinityBinding {
            provider_id: attempt.provider_id.clone(),
            channel_id: attempt.channel_id.clone(),
            bound_at,
            last_used_at: now,
            expires_at: channel_affinity_expires_at(now, attempt.affinity_idle_ttl_seconds),
        },
    );
}

pub(super) async fn refresh_response_id_affinity(
    state: &AppState,
    auth: &crate::auth::AuthResult,
    logical_model: &str,
    response_id: &str,
    attempt: &MonoizeAttempt,
) {
    if !attempt.affinity_enabled {
        return;
    }
    let response_id = response_id.trim();
    if response_id.is_empty() {
        return;
    }
    let Some(key) = response_id_affinity_key(logical_model, response_id, auth) else {
        return;
    };
    let mut guard = state.channel_affinity.lock().await;
    if state.routing_config_revision.load(Ordering::Acquire) != attempt.routing_config_revision {
        return;
    }
    let now = now_ts();
    let bound_at = if attempt.affinity_hit == Some(true) {
        attempt
            .affinity_key
            .as_ref()
            .and_then(|source_key| guard.get(source_key))
            .filter(|binding| {
                binding.provider_id == attempt.provider_id
                    && binding.channel_id == attempt.channel_id
            })
            .map(|binding| binding.bound_at)
            .unwrap_or(now)
    } else {
        now
    };
    insert_channel_affinity(
        &mut guard,
        key,
        crate::monoize_routing::ChannelAffinityBinding {
            provider_id: attempt.provider_id.clone(),
            channel_id: attempt.channel_id.clone(),
            bound_at,
            last_used_at: now,
            expires_at: channel_affinity_expires_at(now, attempt.affinity_idle_ttl_seconds),
        },
    );
}

pub(super) async fn clear_channel_affinity(state: &AppState, attempt: &MonoizeAttempt) {
    let Some(key) = attempt.affinity_key.as_ref() else {
        return;
    };
    let mut guard = state.channel_affinity.lock().await;
    if state.routing_config_revision.load(Ordering::Acquire) == attempt.routing_config_revision {
        guard.remove(key);
    }
}

pub(super) async fn collect_provider_attempts(
    state: &AppState,
    urp: &UrpRequest,
    effective_groups: &Option<Vec<String>>,
    provider: &crate::monoize_routing::MonoizeProvider,
    routing_config_revision: u64,
    out: &mut Vec<MonoizeAttempt>,
) {
    if !provider.enabled {
        return;
    }
    if !crate::users::is_channel_group_eligible(&provider.groups, effective_groups) {
        return;
    }
    let supporting_channels: Vec<crate::monoize_routing::MonoizeChannel> = provider
        .channels
        .iter()
        .filter(|channel| {
            channel.models.get(&urp.model).is_some_and(|entry| {
                urp.max_multiplier
                    .is_none_or(|maximum| entry.multiplier <= maximum)
            })
        })
        .cloned()
        .collect();
    let channels = filter_eligible_channels(
        state,
        &supporting_channels,
        provider.circuit_breaker_enabled,
        provider
            .per_model_circuit_break
            .then_some(urp.model.as_str()),
    )
    .await;
    if channels.is_empty() {
        return;
    }

    let ordered = weighted_shuffle_channels(channels);
    let provider_attempt_limit = if provider.max_retries == -1 {
        None
    } else {
        Some(provider.max_retries.max(0) as usize + 1)
    };
    let max_attempts = provider_attempt_limit
        .unwrap_or(ordered.len())
        .min(ordered.len());
    let runtime = state.monoize_runtime.read().await;
    for channel in ordered.into_iter().take(max_attempts) {
        let origin_key = channel_origin_key(&channel.base_url);
        let origin_peer_channel_ids = origin_key
            .as_deref()
            .map(|origin| origin_peer_channel_ids(&provider.channels, origin))
            .unwrap_or_default();
        let model_entry = channel
            .models
            .get(&urp.model)
            .expect("eligible channel must retain its model entry");
        let upstream_model = resolve_upstream_model(&urp.model, model_entry);
        let effective_provider_type = crate::monoize_routing::resolve_effective_api_type(
            &provider.api_type_overrides,
            channel.provider_type,
            &urp.model,
        );
        let passive_failure_count_threshold = channel
            .passive_failure_count_threshold_override
            .unwrap_or(runtime.passive_failure_count_threshold)
            .max(1);
        let passive_cooldown_seconds = channel
            .passive_cooldown_seconds_override
            .unwrap_or(runtime.passive_cooldown_seconds)
            .max(1);
        let passive_window_seconds = channel
            .passive_window_seconds_override
            .unwrap_or(runtime.passive_window_seconds)
            .max(1);
        let passive_rate_limit_cooldown_seconds = channel
            .passive_rate_limit_cooldown_seconds_override
            .unwrap_or(runtime.passive_rate_limit_cooldown_seconds)
            .max(1);
        let request_timeout_ms = provider
            .request_timeout_ms_override
            .unwrap_or(runtime.request_timeout_ms)
            .max(1);
        let affinity_enabled = channel
            .affinity_enabled_override
            .unwrap_or(runtime.affinity_enabled);
        let affinity_idle_ttl_seconds = channel
            .affinity_idle_ttl_seconds_override
            .unwrap_or(runtime.affinity_idle_ttl_seconds)
            .max(1);
        let affinity_failback_mode = channel
            .affinity_failback_mode_override
            .unwrap_or(runtime.affinity_failback_mode);
        let affinity_failback_delay_seconds = channel
            .affinity_failback_delay_seconds_override
            .unwrap_or(runtime.affinity_failback_delay_seconds);
        out.push(MonoizeAttempt {
            provider_id: provider.id.clone(),
            provider_name: provider.name.clone(),
            provider_type: effective_provider_type.to_config_type(),
            channel_id: channel.id.clone(),
            channel_name: channel.name.clone(),
            base_url: channel.base_url.clone(),
            api_key: channel.api_key.clone(),
            logical_model: urp.model.clone(),
            upstream_model,
            model_multiplier: model_entry.multiplier,
            server_tool_usage_classes: urp.server_tool_usage_classes.clone(),
            provider_transforms: provider.transforms.clone(),
            passive_failure_count_threshold,
            passive_cooldown_seconds,
            passive_window_seconds,
            passive_rate_limit_cooldown_seconds,
            channel_max_retries: provider.channel_max_retries,
            channel_retry_interval_ms: provider.channel_retry_interval_ms.max(0) as u64,
            circuit_breaker_enabled: provider.circuit_breaker_enabled,
            per_model_circuit_break: provider.per_model_circuit_break,
            provider_attempt_limit,
            request_timeout_ms,
            extra_fields_whitelist: merge_extra_fields_whitelist(
                &runtime.extra_fields_whitelist,
                &provider.extra_fields_whitelist,
                effective_provider_type,
            ),
            strip_cross_protocol_nested_extra: provider
                .strip_cross_protocol_nested_extra
                .unwrap_or(runtime.strip_cross_protocol_nested_extra),
            billable_pricing_available: false,
            billing_rate_resolution: None,
            affinity_key: None,
            affinity_key_hash: None,
            affinity_hit: None,
            affinity_target: None,
            affinity_enabled,
            affinity_idle_ttl_seconds,
            affinity_failback_mode,
            affinity_failback_delay_seconds,
            routing_config_revision,
            proxy_url: channel.proxy_url.clone(),
            extra_headers: channel.extra_headers.clone(),
            session_affinity_auto: effective_session_affinity_auto(
                &channel.base_url,
                channel.session_affinity_auto,
            ),
            client_session_id: None,
            derived_session_affinity: None,
            session_affinity_value: None,
            origin_key,
            origin_peer_channel_ids,
        });
    }
}

/// CM-AFF-0: a null setting enables affinity only for the direct Workers AI URL.
pub(super) fn effective_session_affinity_auto(base_url: &str, configured: Option<bool>) -> bool {
    configured.unwrap_or_else(|| is_direct_cloudflare_workers_ai_url(base_url))
}

fn is_direct_cloudflare_workers_ai_url(base_url: &str) -> bool {
    let Ok(url) = reqwest::Url::parse(base_url.trim()) else {
        return false;
    };
    if url.scheme() != "https"
        || url.host_str() != Some("api.cloudflare.com")
        || url.query().is_some()
        || url.fragment().is_some()
    {
        return false;
    }

    let path = url.path().strip_suffix('/').unwrap_or(url.path());
    let Some(account_and_suffix) = path.strip_prefix("/client/v4/accounts/") else {
        return false;
    };
    let account_id = account_and_suffix
        .strip_suffix("/ai/v1")
        .or_else(|| account_and_suffix.strip_suffix("/ai"));
    account_id.is_some_and(|account_id| !account_id.is_empty() && !account_id.contains('/'))
}

/// CM-AFF-1a/1b/2: stamp every freshly built attempt with the client header,
/// decoded-body conversation identifier, and `mono-*` fallback digest.
pub(super) fn attach_client_session_id(
    attempts: &mut [MonoizeAttempt],
    client_session_id: Option<String>,
    req: Option<&urp::UrpRequest>,
) {
    let body_id = req.and_then(super::helpers::stable_session_affinity_raw);
    let derived = req.map(derive_session_affinity_from_urp);
    for attempt in attempts {
        attempt.client_session_id = client_session_id.clone().or_else(|| {
            body_id
                .as_deref()
                .map(sanitize_session_affinity)
                .filter(|value| !value.is_empty())
        });
        attempt.derived_session_affinity = derived.clone();
    }
}

pub(super) fn resolve_upstream_model(
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

pub(super) async fn filter_eligible_channels(
    state: &AppState,
    channels: &[crate::monoize_routing::MonoizeChannel],
    circuit_breaker_enabled: bool,
    model: Option<&str>,
) -> Vec<crate::monoize_routing::MonoizeChannel> {
    let now = now_ts();
    let health = state.channel_health.lock().await;
    let mut out = Vec::new();
    for channel in channels {
        if !channel.enabled || channel.weight <= 0 {
            continue;
        }
        if !circuit_breaker_enabled {
            out.push(channel.clone());
            continue;
        }
        let key = health_key(&channel.id, model);
        if crate::monoize_routing::missing_channel_health_is_saturated(&health, &key) {
            continue;
        }
        let channel_health = health
            .get(&key)
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

fn attempt_health_model(attempt: &MonoizeAttempt) -> Option<&str> {
    attempt
        .per_model_circuit_break
        .then_some(attempt.logical_model.as_str())
}

pub(super) async fn is_attempt_channel_healthy(state: &AppState, attempt: &MonoizeAttempt) -> bool {
    if !attempt.circuit_breaker_enabled {
        return true;
    }
    let health = state.channel_health.lock().await;
    let key = health_key(&attempt.channel_id, attempt_health_model(attempt));
    if crate::monoize_routing::missing_channel_health_is_saturated(&health, &key) {
        return false;
    }
    health
        .get(&key)
        .cloned()
        .unwrap_or_else(crate::monoize_routing::ChannelHealthState::new)
        .healthy
}

pub(super) fn weighted_shuffle_channels(
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

pub(super) fn random_u64(bound: u64) -> u64 {
    if bound <= 1 {
        return 0;
    }
    // Rejection sampling to avoid modulo bias
    let limit = u64::MAX - (u64::MAX % bound);
    loop {
        let sample = uuid::Uuid::new_v4().as_u128() as u64;
        if sample < limit {
            return sample % bound;
        }
    }
}

pub(super) fn build_channel_provider_config(attempt: &MonoizeAttempt) -> ProviderConfig {
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

pub(super) fn provider_extra_headers(
    provider_type: ProviderType,
    body: &serde_json::Value,
) -> &'static [(&'static str, &'static str)] {
    match provider_type {
        ProviderType::Messages if messages_body_uses_files_api(body) => &[
            ("anthropic-version", "2023-06-01"),
            ("anthropic-beta", "files-api-2025-04-14"),
        ],
        ProviderType::Messages => &[("anthropic-version", "2023-06-01")],
        ProviderType::Replicate => &[("prefer", "wait=60")],
        _ => &[],
    }
}

/// CM-HDR-1 + CM-AFF-1/1a/2: protocol headers first, then the Channel's static
/// `extra_headers`, then a client-supplied or derived `x-session-affinity`
/// value when the Channel enables automatic session affinity.
pub(super) fn attempt_extra_headers(
    attempt: &MonoizeAttempt,
    body: &serde_json::Value,
) -> Vec<(String, String)> {
    let mut out: Vec<(String, String)> = provider_extra_headers(attempt.provider_type, body)
        .iter()
        .map(|(name, value)| ((*name).to_string(), (*value).to_string()))
        .collect();
    if let Some(extras) = &attempt.extra_headers {
        for (name, value) in extras {
            out.push((name.clone(), value.clone()));
        }
    }
    if !out
        .iter()
        .any(|(name, _)| name.eq_ignore_ascii_case("x-session-affinity"))
        && let Some(value) = resolve_session_affinity_value(attempt, body)
    {
        out.push(("x-session-affinity".to_string(), value));
    }
    out
}

/// CM-AFF-1/1a/1b/2 + CM-AFF-4: the single effective `x-session-affinity`
/// value for one attempt. Priority: explicit static `extra_headers` entry,
/// then the client header or body conversation identifier, then
/// `prompt_cache_key`, then the decoded-request digest, then encoded-body
/// digest without tools.
pub(super) fn resolve_session_affinity_value(
    attempt: &MonoizeAttempt,
    body: &serde_json::Value,
) -> Option<String> {
    if !attempt.session_affinity_auto {
        return None;
    }
    if let Some(value) = attempt.extra_headers.as_ref().and_then(|headers| {
        headers.iter().find_map(|(name, value)| {
            name.eq_ignore_ascii_case("x-session-affinity")
                .then_some(value.as_str())
        })
    }) {
        return Some(value.to_string());
    }
    if let Some(client) = attempt.client_session_id.as_deref() {
        return Some(client.to_string());
    }
    if let Some(key) = encoded_prompt_cache_key(body) {
        return Some(key);
    }
    if let Some(derived) = attempt.derived_session_affinity.as_deref() {
        return Some(derived.to_string());
    }
    derive_session_affinity(body)
}

const SESSION_AFFINITY_MAX_KEY_CHARS: usize = 128;

/// Restrict a raw affinity value to printable ASCII, at most
/// `SESSION_AFFINITY_MAX_KEY_CHARS` characters, after trimming.
pub(super) fn sanitize_session_affinity(raw: &str) -> String {
    raw.trim()
        .chars()
        .filter(|c| ('\u{20}'..='\u{7e}').contains(c))
        .take(SESSION_AFFINITY_MAX_KEY_CHARS)
        .collect()
}

fn encoded_prompt_cache_key(body: &serde_json::Value) -> Option<String> {
    let key = body.get("prompt_cache_key").and_then(Value::as_str)?;
    let sanitized = sanitize_session_affinity(key);
    (!sanitized.is_empty()).then_some(sanitized)
}

fn session_affinity_digest(payload: &Value) -> Option<String> {
    use sha2::Digest;

    let digest = sha2::Sha256::digest(serde_json::to_string(payload).ok()?);
    let prefix = u64::from_be_bytes([
        digest[0], digest[1], digest[2], digest[3], digest[4], digest[5], digest[6], digest[7],
    ]);
    Some(format!("mono-{prefix:016x}"))
}

fn canonical_session_head_node(node: &urp::Node) -> Value {
    match node {
        urp::Node::Text { role, content, .. } => json!({
            "t": "text",
            "role": role,
            "content": content,
        }),
        urp::Node::Image { role, source, .. } => json!({
            "t": "image",
            "role": role,
            "source": source,
        }),
        urp::Node::Audio { role, source, .. } => json!({
            "t": "audio",
            "role": role,
            "source": source,
        }),
        urp::Node::File { role, source, .. } => json!({
            "t": "file",
            "role": role,
            "source": source,
        }),
        urp::Node::Refusal { content, .. } => json!({
            "t": "refusal",
            "content": content,
        }),
        urp::Node::Reasoning { content, .. } => json!({
            "t": "reasoning",
            "content": content,
        }),
        urp::Node::ToolCall {
            name, arguments, ..
        } => json!({
            "t": "tool_call",
            "name": name,
            "arguments": arguments,
        }),
        urp::Node::ToolResult {
            call_id,
            is_error,
            content,
            ..
        } => json!({
            "t": "tool_result",
            "call_id": call_id,
            "is_error": is_error,
            "content": content,
        }),
        urp::Node::ProviderItem {
            role,
            item_type,
            body,
            ..
        } => json!({
            "t": "provider",
            "role": role,
            "item_type": item_type,
            "body": body,
        }),
        urp::Node::NextDownstreamEnvelopeExtra { .. } => json!({ "t": "envelope" }),
    }
}

fn session_affinity_instructions(req: &urp::UrpRequest) -> Value {
    req.extra_body
        .get("instructions")
        .cloned()
        .or_else(|| {
            req.extra_body
                .get(urp::RESPONSES_INSTRUCTIONS_EXTRA_KEY)
                .cloned()
        })
        .unwrap_or(Value::Null)
}

/// CM-AFF-2 rule 2 over the decoded request: instructions plus the first two
/// input nodes, with ids/extra_body/tools omitted.
pub(super) fn derive_session_affinity_from_urp(req: &urp::UrpRequest) -> String {
    let head: Vec<Value> = req
        .input
        .iter()
        .take(2)
        .map(canonical_session_head_node)
        .collect();
    let payload = json!({
        "head": Value::Array(head),
        "instructions": session_affinity_instructions(req),
    });
    session_affinity_digest(&payload).expect("session affinity payload is serializable")
}

/// CM-AFF-2 encoded-body fallback: conversation head without `tools`.
pub(super) fn derive_session_affinity(body: &serde_json::Value) -> Option<String> {
    if let Some(key) = encoded_prompt_cache_key(body) {
        return Some(key);
    }

    let head: Option<Value> = ["messages", "input"].iter().find_map(|field| {
        let items = body.get(*field)?.as_array()?;
        if items.is_empty() {
            return None;
        }
        Some(Value::Array(items.iter().take(2).cloned().collect()))
    });

    let payload = serde_json::json!({
        "head": head.unwrap_or(Value::Null),
        "instructions": body.get("instructions").cloned().unwrap_or(Value::Null),
        "system": body.get("system").cloned().unwrap_or(Value::Null),
    });
    session_affinity_digest(&payload)
}

fn messages_body_uses_files_api(value: &serde_json::Value) -> bool {
    match value {
        serde_json::Value::Object(obj) => {
            let block_type = obj.get("type").and_then(serde_json::Value::as_str);
            let direct_container_upload = block_type == Some("container_upload")
                && obj
                    .get("file_id")
                    .and_then(serde_json::Value::as_str)
                    .is_some_and(|file_id| !file_id.is_empty());
            let nested_file_source = matches!(block_type, Some("image" | "document"))
                && obj
                    .get("source")
                    .and_then(serde_json::Value::as_object)
                    .is_some_and(|source| {
                        source.get("type").and_then(serde_json::Value::as_str) == Some("file")
                            && source
                                .get("file_id")
                                .and_then(serde_json::Value::as_str)
                                .is_some_and(|file_id| !file_id.is_empty())
                    });
            direct_container_upload
                || nested_file_source
                || obj.values().any(messages_body_uses_files_api)
        }
        serde_json::Value::Array(values) => values.iter().any(messages_body_uses_files_api),
        _ => false,
    }
}

pub(super) fn build_exhausted_error_message(model: &str, tried: &[TriedProvider]) -> String {
    if tried.is_empty() {
        return format!("No available upstream provider for model: {model}");
    }
    let last_error = &tried[tried.len() - 1].error;
    format!(
        "All {n} upstream attempt(s) failed for model: {model}. Last error: {last_error}",
        n = tried.len(),
    )
}

pub(super) fn build_exhausted_upstream_error(model: &str, tried: &[TriedProvider]) -> AppError {
    let last = tried.last();
    let code = last
        .and_then(|attempt| attempt.upstream_code.as_deref())
        .filter(|code| !code.is_empty())
        .unwrap_or("upstream_error");
    let signature_invalid = code == "thinking_signature_invalid";
    let status = if signature_invalid {
        last.and_then(|attempt| attempt.upstream_status)
            .and_then(|status| StatusCode::from_u16(status).ok())
            .filter(StatusCode::is_client_error)
            .unwrap_or(StatusCode::BAD_REQUEST)
    } else {
        StatusCode::BAD_GATEWAY
    };
    let message = if signature_invalid {
        last.map(|attempt| attempt.error.clone())
            .unwrap_or_else(|| build_exhausted_error_message(model, tried))
    } else {
        build_exhausted_error_message(model, tried)
    };
    let mut err = AppError::new(status, code, message);
    if let Some(last) = last {
        err.upstream_status = last.upstream_status;
        err.upstream_code = last.upstream_code.clone();
        err.upstream_type = last.upstream_type.clone();
        err.upstream_param = last.upstream_param.clone();
    }
    err
}

fn status_allows_same_channel_retry(status: StatusCode) -> bool {
    status == StatusCode::REQUEST_TIMEOUT
        || status == StatusCode::TOO_MANY_REQUESTS
        || status.is_server_error()
}

pub(super) fn is_same_channel_retryable_error(err: &UpstreamCallError) -> bool {
    if matches!(err.kind, UpstreamErrorKind::Network) {
        return true;
    }
    err.status.is_some_and(status_allows_same_channel_retry)
}

pub(super) fn is_same_channel_retryable_app_error(err: &AppError) -> bool {
    // `status` is Monoize's wrapper status. Only an actual upstream status may
    // authorize another request to the same Channel.
    err.upstream_status
        .and_then(|status| StatusCode::from_u16(status).ok())
        .is_some_and(status_allows_same_channel_retry)
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) enum RetryableFailureClass {
    RateLimited,
    Transient,
}

pub(super) fn classify_retryable_failure(err: &UpstreamCallError) -> RetryableFailureClass {
    if matches!(err.status, Some(StatusCode::TOO_MANY_REQUESTS)) {
        return RetryableFailureClass::RateLimited;
    }
    RetryableFailureClass::Transient
}

pub(super) fn classify_retryable_app_failure(err: &AppError) -> RetryableFailureClass {
    if err.upstream_status == Some(StatusCode::TOO_MANY_REQUESTS.as_u16()) {
        return RetryableFailureClass::RateLimited;
    }
    RetryableFailureClass::Transient
}

pub(super) async fn record_upstream_attempt_failure(
    state: &AppState,
    attempt: &MonoizeAttempt,
    attempt_number: u32,
    app_err: &AppError,
    passive_failure_class: Option<RetryableFailureClass>,
    tried_providers: &mut Vec<TriedProvider>,
    execution_state: &mut AttemptExecutionState,
) {
    tried_providers.push(TriedProvider::from_app_error(
        attempt_number,
        attempt,
        app_err,
        execution_state.last_attempt_duration_ms(),
    ));
    let Some(failure_class) = passive_failure_class else {
        return;
    };
    let shared_origin_blast = failure_class == RetryableFailureClass::Transient
        && is_shared_origin_status(app_err.upstream_status);
    mark_channel_retryable_failure(state, attempt, failure_class).await;
    // AFF-9: clear the binding only when the failed attempt is the bound
    // target itself, the failure is not a shared-origin blast, and this
    // failure tripped the breaker. Sub-threshold transient failures keep the
    // binding so prompt-cache locality survives one-off faults.
    if attempt.affinity_hit == Some(true)
        && !shared_origin_blast
        && !is_attempt_channel_healthy(state, attempt).await
    {
        clear_channel_affinity(state, attempt).await;
    }
    if shared_origin_blast {
        execution_state.mark_shared_origin_skip(attempt);
        mark_shared_origin_peer_failures(state, attempt, failure_class).await;
    }
}

/// STRM-4 + STRM-4a: one passive-failure sample for a retryable terminal
/// failure that occurs after the first downstream byte (in-stream terminal
/// error event or upstream-side adapter failure). Mid-stream failures never
/// blast shared-origin peers. The affinity binding is cleared only when the
/// serving attempt is the bound target and the sample tripped the breaker.
pub(super) async fn record_midstream_terminal_failure(
    state: &AppState,
    attempt: &MonoizeAttempt,
    failure_class: RetryableFailureClass,
) {
    mark_channel_retryable_failure(state, attempt, failure_class).await;
    if attempt.affinity_hit == Some(true) && !is_attempt_channel_healthy(state, attempt).await {
        clear_channel_affinity(state, attempt).await;
    }
}

/// STRM-4: an in-stream terminal error records a health sample only for
/// retryable statuses; `429` cools down as RateLimited, everything else as
/// Transient.
pub(super) fn midstream_terminal_failure_class(
    http_status: u16,
) -> Option<RetryableFailureClass> {
    match http_status {
        429 => Some(RetryableFailureClass::RateLimited),
        408 => Some(RetryableFailureClass::Transient),
        status if (500..600).contains(&status) => Some(RetryableFailureClass::Transient),
        _ => None,
    }
}

/// STRM-4: only upstream-side adapter failures (idle timeout, malformed
/// upstream stream, connection loss, missing terminal) count as retryable
/// terminal failures. Internal transform/encode errors do not penalize the
/// Channel.
pub(super) fn is_upstream_adapter_failure(err: &AppError) -> bool {
    err.code.starts_with("upstream_")
}

/// RTA-4/RTA-4a: per-Channel loop-slot count. The affinity-hit target
/// reserves one extra slot beyond `channel_max_retries + 1`;
/// `allow_same_channel_retry` decides whether that slot is usable.
pub(super) fn same_channel_attempt_slots(attempt: &MonoizeAttempt) -> usize {
    let base = (attempt.channel_max_retries + 1).max(1) as usize;
    if attempt.affinity_hit == Some(true) {
        base + 1
    } else {
        base
    }
}

/// RTA-4/RTA-4a/RTA-5: decide whether the same Channel may serve one more
/// attempt after a failure. `attempts_used` counts attempts already executed
/// on this Channel for this request, including the failed one.
/// `failure_class` is `None` when the failure is not same-Channel retryable.
/// The affinity-hit target earns one attempt beyond `channel_max_retries + 1`
/// for a Transient (non-429) failure so a single transient fault on the bound
/// Channel does not force a switch that discards prompt-cache locality.
pub(super) async fn allow_same_channel_retry(
    state: &AppState,
    attempt: &MonoizeAttempt,
    execution_state: &AttemptExecutionState,
    attempts_used: usize,
    failure_class: Option<RetryableFailureClass>,
) -> bool {
    let Some(failure_class) = failure_class else {
        return false;
    };
    let base_limit = (attempt.channel_max_retries + 1).max(1) as usize;
    let limit = if attempt.affinity_hit == Some(true)
        && failure_class == RetryableFailureClass::Transient
    {
        base_limit + 1
    } else {
        base_limit
    };
    attempts_used < limit
        && !execution_state.should_skip(attempt)
        && is_attempt_channel_healthy(state, attempt).await
}

pub(super) fn prune_passive_failure_timestamps(
    failure_timestamps: &mut std::collections::VecDeque<i64>,
    now_ts: i64,
    window_seconds: u64,
) {
    let window_seconds = i64::try_from(window_seconds).unwrap_or(i64::MAX);
    let cutoff = now_ts.saturating_sub(window_seconds);
    while let Some(front) = failure_timestamps.front() {
        if *front < cutoff {
            let _ = failure_timestamps.pop_front();
        } else {
            break;
        }
    }
}

pub(super) async fn mark_channel_success(state: &AppState, attempt: &MonoizeAttempt) {
    if !attempt.circuit_breaker_enabled {
        return;
    }
    let now = now_ts();
    let mut health = state.channel_health.lock().await;
    if state.routing_config_revision.load(Ordering::Acquire) != attempt.routing_config_revision {
        return;
    }
    let key = health_key(&attempt.channel_id, attempt_health_model(attempt));
    if !health.contains_key(&key)
        && health.len() >= crate::monoize_routing::channel_health_max_entries()
    {
        return;
    }
    let entry = health
        .entry(key)
        .or_insert_with(crate::monoize_routing::ChannelHealthState::new);
    let was_unhealthy = !entry.healthy;
    entry.healthy = true;
    entry.cooldown_until = None;
    entry.last_success_at = Some(now);
    entry.probe_success_count = 0;
    entry.last_probe_at = None;
    prune_passive_failure_timestamps(
        &mut entry.passive_failure_timestamps,
        now,
        attempt.passive_window_seconds,
    );
    if was_unhealthy {
        tracing::info!(channel_id = %attempt.channel_id, "channel recovered to healthy after success");
    }
}

pub(super) async fn mark_channel_retryable_failure(
    state: &AppState,
    attempt: &MonoizeAttempt,
    failure_class: RetryableFailureClass,
) {
    apply_retryable_failure_to_channel(state, attempt, &attempt.channel_id, failure_class, true)
        .await;
}

async fn mark_shared_origin_peer_failures(
    state: &AppState,
    attempt: &MonoizeAttempt,
    failure_class: RetryableFailureClass,
) {
    let mut peer_count = 0usize;
    for peer_id in &attempt.origin_peer_channel_ids {
        if peer_id == &attempt.channel_id {
            continue;
        }
        apply_retryable_failure_to_channel(state, attempt, peer_id, failure_class, false).await;
        peer_count += 1;
    }
    if peer_count > 0 {
        tracing::info!(
            channel_id = %attempt.channel_id,
            origin_key = attempt.origin_key.as_deref().unwrap_or(""),
            peer_count,
            "shared-origin blast applied to same-base-url channels"
        );
    }
}

async fn apply_retryable_failure_to_channel(
    state: &AppState,
    attempt: &MonoizeAttempt,
    channel_id: &str,
    failure_class: RetryableFailureClass,
    log_threshold: bool,
) {
    if !attempt.circuit_breaker_enabled {
        return;
    }
    let now = now_ts();
    let mut health = state.channel_health.lock().await;
    if state.routing_config_revision.load(Ordering::Acquire) != attempt.routing_config_revision {
        return;
    }
    let key = health_key(channel_id, attempt_health_model(attempt));
    if !crate::monoize_routing::prepare_channel_health_insert(&mut health, &key) {
        return;
    }
    let entry = health
        .entry(key)
        .or_insert_with(crate::monoize_routing::ChannelHealthState::new);
    prune_passive_failure_timestamps(
        &mut entry.passive_failure_timestamps,
        now,
        attempt.passive_window_seconds,
    );
    let failure_threshold = crate::monoize_routing::effective_passive_failure_threshold(
        attempt.passive_failure_count_threshold,
    );
    if entry.passive_failure_timestamps.len() < failure_threshold {
        entry.passive_failure_timestamps.push_back(now);
    }

    let failure_samples = entry.passive_failure_timestamps.len();
    if failure_samples >= failure_threshold {
        entry.healthy = false;
        let cooldown_seconds = if failure_class == RetryableFailureClass::RateLimited {
            attempt.passive_rate_limit_cooldown_seconds
        } else {
            attempt.passive_cooldown_seconds
        };
        entry.cooldown_until = Some(now + cooldown_seconds as i64);
        entry.probe_success_count = 0;
        entry.last_probe_at = None;
        if log_threshold {
            tracing::info!(
                channel_id = %channel_id,
                failure_class = ?failure_class,
                failed_samples = failure_samples,
                cooldown_seconds,
                "channel marked unhealthy after passive breaker threshold"
            );
        }
    }
}
pub(super) fn upstream_error_to_app(err: UpstreamCallError) -> AppError {
    let status = err.status.unwrap_or(StatusCode::BAD_GATEWAY);
    tracing::warn!(status = %status, upstream_error = %err.message, "upstream request failed");
    let user_message = format!("upstream status {status}: {}", err.message);
    let mut app_err = AppError::new(status, "upstream_error", user_message).with_upstream_error(
        err.status,
        err.code,
        err.error_type.clone(),
        err.param.clone(),
    );
    if let Some(error_type) = err.error_type {
        app_err = app_err.with_type(error_type);
    }
    if let Some(param) = err.param {
        app_err = app_err.with_param(param);
    }
    app_err
}

pub(super) fn openai_error_json(err: &AppError) -> Value {
    json!({
        "error": {
            "message": err.message,
            "type": err.error_type,
            "code": err.upstream_code.as_ref().unwrap_or(&err.code),
            "param": err.param,
            "upstream_status": err.upstream_status,
            "upstream_code": err.upstream_code,
            "upstream_type": err.upstream_type,
            "upstream_param": err.upstream_param,
        }
    })
}

pub(super) fn responses_stream_error_json(seq: u64, err: &AppError) -> Value {
    json!({
        "type": "error",
        "sequence_number": seq,
        "code": err.upstream_code.as_ref().unwrap_or(&err.code),
        "message": err.message,
        "param": err.upstream_param.as_ref().or(err.param.as_ref()),
    })
}

fn merge_extra_fields_whitelist(
    global: &std::collections::HashMap<String, Vec<String>>,
    provider_override: &Option<Vec<String>>,
    effective_type: crate::monoize_routing::MonoizeProviderType,
) -> Option<Vec<String>> {
    let global_for_type = global.get(effective_type.as_str());
    match (global_for_type, provider_override) {
        (None, None) => None,
        (Some(g), None) => Some(g.clone()),
        (None, Some(p)) => Some(p.clone()),
        (Some(g), Some(p)) => {
            let mut merged = g.clone();
            for field in p {
                if !merged.contains(field) {
                    merged.push(field.clone());
                }
            }
            Some(merged)
        }
    }
}
