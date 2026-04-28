use super::*;
use crate::settings::BUILTIN_REASONING_EFFORT_SUFFIXES;

pub(crate) fn now_ts() -> i64 {
    chrono::Utc::now().timestamp()
}

pub(super) fn client_http(state: &AppState) -> &reqwest::Client {
    &state.http
}

pub(crate) fn health_key(channel_id: &str, model: Option<&str>) -> String {
    match model {
        Some(m) => format!("{channel_id}::{m}"),
        None => channel_id.to_string(),
    }
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

pub(super) async fn resolve_model_suffix(state: &AppState, req: &mut urp::UrpRequest) {
    let requested_model = req.model.clone();
    let normalized = normalized_logical_model_for_matching(state, &requested_model).await;
    if normalized == requested_model {
        return;
    }
    req.model = normalized;

    let settings_map = state
        .settings_store
        .get_reasoning_suffix_map()
        .await
        .unwrap_or_default();

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

pub(super) async fn normalized_logical_model_for_matching(
    state: &AppState,
    requested_model: &str,
) -> String {
    let providers = match state.monoize_store.list_providers().await {
        Ok(p) => p,
        Err(_) => return requested_model.to_string(),
    };

    let model_exists = |model: &str| -> bool {
        providers
            .iter()
            .any(|p| p.enabled && p.models.contains_key(model))
    };
    if model_exists(requested_model) {
        return requested_model.to_string();
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

    for (suffix, _effort) in settings_entries
        .iter()
        .chain(BUILTIN_REASONING_EFFORT_SUFFIXES.iter())
    {
        if let Some(base) = requested_model.strip_suffix(suffix) {
            if !base.is_empty() && model_exists(base) {
                return base.to_string();
            }
        }
    }

    requested_model.to_string()
}

pub(super) async fn build_monoize_attempts(
    state: &AppState,
    urp: &UrpRequest,
    auth: &crate::auth::AuthResult,
) -> AppResult<Vec<MonoizeAttempt>> {
    let providers =
        state.monoize_store.list_providers().await.map_err(|e| {
            AppError::new(StatusCode::INTERNAL_SERVER_ERROR, "provider_store_error", e)
        })?;
    let mut attempts = Vec::new();
    for provider in providers {
        collect_provider_attempts(state, urp, &auth.effective_groups, &provider, &mut attempts)
            .await;
    }
    if attempts.is_empty() {
        return Ok(attempts);
    }

    let mut pricing_cache: std::collections::HashMap<(String, String), bool> =
        std::collections::HashMap::new();
    let mut blocked_models: std::collections::BTreeSet<String> = std::collections::BTreeSet::new();
    let mut allowed_attempts = Vec::with_capacity(attempts.len());
    let allow_unpriced_models = auth.can_bypass_unpriced_models();

    for mut attempt in attempts {
        let cache_key = (attempt.upstream_model.clone(), urp.model.clone());
        let has_pricing = if let Some(cached) = pricing_cache.get(&cache_key) {
            *cached
        } else {
            let priced = resolve_billable_pricing(state, &attempt.upstream_model, &urp.model)
                .await?
                .is_some();
            pricing_cache.insert(cache_key, priced);
            priced
        };

        if has_pricing {
            attempt.billable_pricing_available = true;
            allowed_attempts.push(attempt);
        } else if allow_unpriced_models {
            allowed_attempts.push(attempt);
        } else {
            blocked_models.insert(attempt.upstream_model);
        }
    }

    if allowed_attempts.is_empty() && !blocked_models.is_empty() {
        let blocked_list = blocked_models.into_iter().collect::<Vec<_>>().join(", ");
        return Err(AppError::new(
            StatusCode::FORBIDDEN,
            "model_pricing_required",
            format!("pricing metadata required for model(s): {blocked_list}"),
        ));
    }
    Ok(allowed_attempts)
}

pub(super) async fn collect_provider_attempts(
    state: &AppState,
    urp: &UrpRequest,
    effective_groups: &Option<Vec<String>>,
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
    if !crate::users::is_channel_group_eligible(&provider.groups, effective_groups) {
        return;
    }
    let channels = filter_eligible_channels(
        state,
        &provider.channels,
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
    let upstream_model = resolve_upstream_model(&urp.model, model_entry);
    let effective_provider_type = crate::monoize_routing::resolve_effective_api_type(
        &provider.api_type_overrides,
        provider.provider_type,
        &urp.model,
    );

    let runtime = state.monoize_runtime.read().await;
    for channel in ordered.into_iter().take(max_attempts) {
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
        out.push(MonoizeAttempt {
            provider_id: provider.id.clone(),
            provider_type: effective_provider_type.to_config_type(),
            channel_id: channel.id.clone(),
            base_url: channel.base_url.clone(),
            api_key: channel.api_key.clone(),
            logical_model: urp.model.clone(),
            upstream_model: upstream_model.clone(),
            model_multiplier: model_entry.multiplier,
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
        });
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
) -> &'static [(&'static str, &'static str)] {
    match provider_type {
        ProviderType::Messages => &[("anthropic-version", "2023-06-01")],
        ProviderType::Replicate => &[("prefer", "wait=60")],
        _ => &[],
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

pub(super) fn is_non_retryable_client_error(err: &UpstreamCallError) -> bool {
    matches!(
        err.status,
        Some(StatusCode::BAD_REQUEST)
            | Some(StatusCode::UNAUTHORIZED)
            | Some(StatusCode::FORBIDDEN)
            | Some(StatusCode::UNPROCESSABLE_ENTITY)
    )
}

pub(super) fn is_retryable_error(err: &UpstreamCallError) -> bool {
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

pub(super) fn prune_passive_samples(
    samples: &mut std::collections::VecDeque<crate::monoize_routing::PassiveHealthSample>,
    now_ts: i64,
    window_seconds: u64,
) {
    let cutoff = now_ts.saturating_sub(window_seconds as i64);
    while let Some(front) = samples.front() {
        if front.at_ts < cutoff {
            let _ = samples.pop_front();
        } else {
            break;
        }
    }
}

pub(super) async fn mark_channel_success(state: &AppState, attempt: &MonoizeAttempt) {
    let now = now_ts();
    let mut health = state.channel_health.lock().await;
    let key = health_key(&attempt.channel_id, attempt_health_model(attempt));
    let entry = health
        .entry(key)
        .or_insert_with(crate::monoize_routing::ChannelHealthState::new);
    let was_unhealthy = !entry.healthy;
    entry.healthy = true;
    entry.cooldown_until = None;
    entry.last_success_at = Some(now);
    entry.probe_success_count = 0;
    entry.last_probe_at = None;
    entry
        .passive_samples
        .push_back(crate::monoize_routing::PassiveHealthSample {
            at_ts: now,
            failed: false,
        });
    prune_passive_samples(
        &mut entry.passive_samples,
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
    if !attempt.circuit_breaker_enabled {
        return;
    }
    let now = now_ts();
    let mut health = state.channel_health.lock().await;
    let key = health_key(&attempt.channel_id, attempt_health_model(attempt));
    let entry = health
        .entry(key)
        .or_insert_with(crate::monoize_routing::ChannelHealthState::new);
    entry
        .passive_samples
        .push_back(crate::monoize_routing::PassiveHealthSample {
            at_ts: now,
            failed: true,
        });
    prune_passive_samples(
        &mut entry.passive_samples,
        now,
        attempt.passive_window_seconds,
    );

    let failure_samples = entry.passive_samples.iter().filter(|s| s.failed).count() as u32;
    if failure_samples >= attempt.passive_failure_count_threshold {
        entry.healthy = false;
        let cooldown_seconds = if failure_class == RetryableFailureClass::RateLimited {
            attempt.passive_rate_limit_cooldown_seconds
        } else {
            attempt.passive_cooldown_seconds
        };
        entry.cooldown_until = Some(now + cooldown_seconds as i64);
        entry.probe_success_count = 0;
        entry.last_probe_at = None;
        tracing::info!(
            channel_id = %attempt.channel_id,
            failure_class = ?failure_class,
            failed_samples = failure_samples,
            cooldown_seconds,
            "channel marked unhealthy after passive breaker threshold"
        );
    }
}
pub(super) fn upstream_error_to_app(err: UpstreamCallError) -> AppError {
    let status = err.status.unwrap_or(StatusCode::BAD_GATEWAY);
    tracing::warn!(status = %status, upstream_error = %err.message, "upstream request failed");
    let user_message = format!("upstream status {status}: {}", err.message);
    AppError::new(status, "upstream_error", user_message)
}

pub(super) fn error_to_sse_stream(
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
            let mut seq: u64 = 1;
            let payload =
                crate::urp::stream_helpers::normalize_responses_payload(seq, "error", error_json);
            seq += 1;
            events.push(Event::default().event("error").data(payload.to_string()));
            let _ = seq;
        }
        DownstreamProtocol::ChatCompletions => {
            events.push(Event::default().data(error_json.to_string()));
        }
        DownstreamProtocol::AnthropicMessages => {
            events.push(
                Event::default().event("error").data(
                    json!({"type": "error", "error": {"type": err.code, "message": err.message}})
                        .to_string(),
                ),
            );
        }
    }
    events.push(Event::default().data("[DONE]"));
    futures_util::stream::iter(events.into_iter().map(Ok))
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
