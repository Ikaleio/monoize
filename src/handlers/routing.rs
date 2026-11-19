use super::*;
use crate::monoize_routing::{ChannelHealthState, MonoizeChannel, RecoveryMode};
use crate::settings::BUILTIN_REASONING_EFFORT_SUFFIXES;
use std::sync::atomic::Ordering;

pub(crate) fn now_ts() -> i64 {
    chrono::Utc::now().timestamp()
}

/// PX6/PX7: effective outbound client for one attempt's Channel (custom proxy wins,
/// then node-global, else direct). Construction failure fails closed.
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

pub(crate) fn channel_recovery_mode(
    state: &AppState,
    provider_type: crate::monoize_routing::MonoizeProviderType,
) -> RecoveryMode {
    if state.node.is_replica()
        || provider_type == crate::monoize_routing::MonoizeProviderType::Replicate
    {
        RecoveryMode::Timed
    } else {
        RecoveryMode::Probe
    }
}

pub(super) fn upstream_path(provider_type: ProviderType) -> &'static str {
    match provider_type {
        ProviderType::Responses => "/v1/responses",
        ProviderType::ChatCompletion => "/v1/chat/completions",
        ProviderType::Messages => "/v1/messages",
        ProviderType::Gemini => "/v1beta/models",
        ProviderType::OpenaiImage => "/v1/images/generations",
        ProviderType::OpenrouterImage => "/v1/images",
        ProviderType::Replicate => "/v1/predictions",
        ProviderType::SystemOne => crate::decision::DecisionFormat::SystemOne.upstream_path(),
        ProviderType::OpenaiDecisions => {
            crate::decision::DecisionFormat::OpenaiDecisions.upstream_path()
        }
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
    settings_entries.sort_by_key(|entry| std::cmp::Reverse(entry.0.len()));

    for (suffix, effort) in settings_entries
        .iter()
        .chain(BUILTIN_REASONING_EFFORT_SUFFIXES.iter())
    {
        if let Some(base) = requested_model.strip_suffix(suffix)
            && !base.is_empty()
        {
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
                        ..Default::default()
                    });
                }
            }
            return Ok(normalized);
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
    settings_entries.sort_by_key(|entry| std::cmp::Reverse(entry.0.len()));

    let mut candidates = vec![requested_model.to_string()];
    for (suffix, _effort) in settings_entries
        .iter()
        .chain(BUILTIN_REASONING_EFFORT_SUFFIXES.iter())
    {
        if let Some(base) = requested_model.strip_suffix(suffix)
            && !base.is_empty()
            && !candidates.iter().any(|candidate| candidate == base)
        {
            candidates.push(base.to_string());
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

/// AT-4: the set of effective API types that an endpoint may route to.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) enum RouteSurface {
    /// Every forwarding endpoint other than the decision endpoints.
    Generation,
    /// `/v1/systemone` and `/v1/decisions`.
    Decision,
}

impl RouteSurface {
    fn admits(self, api_type: crate::monoize_routing::MonoizeProviderType) -> bool {
        let is_decision = api_type.to_config_type().decision_format().is_some();
        is_decision == (self == Self::Decision)
    }
}

pub(super) async fn build_monoize_attempts(
    state: &AppState,
    urp: &UrpRequest,
    auth: &crate::auth::AuthResult,
) -> AppResult<Vec<MonoizeAttempt>> {
    build_attempts(state, urp, auth, RouteSurface::Generation, None).await
}

pub(super) async fn build_decision_attempts(
    state: &AppState,
    urp: &UrpRequest,
    auth: &crate::auth::AuthResult,
) -> AppResult<Vec<MonoizeAttempt>> {
    build_attempts(state, urp, auth, RouteSurface::Decision, None).await
}

pub(super) async fn build_monoize_attempts_for_provider_type(
    state: &AppState,
    urp: &UrpRequest,
    auth: &crate::auth::AuthResult,
    required_provider_type: Option<ProviderType>,
) -> AppResult<Vec<MonoizeAttempt>> {
    build_attempts(
        state,
        urp,
        auth,
        RouteSurface::Generation,
        required_provider_type,
    )
    .await
}

async fn build_attempts(
    state: &AppState,
    urp: &UrpRequest,
    auth: &crate::auth::AuthResult,
    surface: RouteSurface,
    required_provider_type: Option<ProviderType>,
) -> AppResult<Vec<MonoizeAttempt>> {
    let routing_config_revision = state.routing_config_revision.load(Ordering::Acquire);
    let providers = state
        .monoize_store
        .list_providers_for_model(&urp.model)
        .await
        .map_err(|e| AppError::new(StatusCode::INTERNAL_SERVER_ERROR, "provider_store_error", e))?;
    // R-GRP-2: rank providers by the position of the first effective group they
    // serve; the stable sort keeps priority order within the same rank.
    let mut ranked_providers = providers.iter().collect::<Vec<_>>();
    ranked_providers.sort_by_key(|provider| {
        crate::users::provider_group_rank(&provider.group_ids, &auth.effective_groups)
    });
    let mut attempts = Vec::new();
    for provider in ranked_providers {
        collect_provider_attempts(
            state,
            urp,
            &auth.effective_groups,
            provider,
            surface,
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

    // MP-R8: one set-based query for all distinct pricing keys.
    let upstream_models = attempts
        .iter()
        .map(|attempt| attempt.upstream_model.clone())
        .collect::<Vec<_>>();
    let pricing_snapshot = build_model_price_snapshot(state, &upstream_models, &urp.model).await?;

    // MP-G2: billing ratios come from the cached group registry (DPT-RC8).
    let group_billing_ratios =
        if attempts
            .iter()
            .any(|attempt| attempt.billing_group_id.is_some())
        {
            Some(state.user_store.group_billing_ratios().await.map_err(|e| {
                AppError::new(StatusCode::INTERNAL_SERVER_ERROR, "internal_error", e)
            })?)
        } else {
            None
        };

    let mut blocked_models: std::collections::BTreeSet<String> = std::collections::BTreeSet::new();
    let mut allowed_attempts = Vec::with_capacity(attempts.len());
    for mut attempt in attempts {
        let (pricing_model_key, model_price) =
            pricing_snapshot.resolve(&attempt.upstream_model, &urp.model);
        // MP-F2: an unpriced attempt is billable only under the effective
        // `allow_free_when_unpriced` flag.
        if model_price.is_none() && !attempt.allow_free_when_unpriced {
            blocked_models.insert(attempt.upstream_model);
            continue;
        }
        if let Some(group_id) = attempt.billing_group_id.as_deref()
            && let Some(ratio) = group_billing_ratios
                .as_deref()
                .and_then(|ratios| ratios.get(group_id))
        {
            attempt.group_billing_ratio = Multiplier::parse(ratio).map_err(|err| {
                AppError::new(
                    StatusCode::INTERNAL_SERVER_ERROR,
                    "internal_error",
                    format!("group {group_id} has invalid billing_ratio: {err}"),
                )
            })?;
        }
        attempt.pricing_model_key = pricing_model_key;
        attempt.model_price = model_price;
        allowed_attempts.push(attempt);
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
    surface: RouteSurface,
    routing_config_revision: u64,
    out: &mut Vec<MonoizeAttempt>,
) {
    if !provider.enabled {
        return;
    }
    if !crate::users::is_provider_group_eligible(&provider.group_ids, effective_groups) {
        return;
    }
    let supporting_channels = provider
        .channels
        .iter()
        .filter(|channel| {
            channel.models.get(&urp.model).is_some_and(|entry| {
                urp.max_multiplier
                    .is_none_or(|maximum| entry.multiplier <= maximum)
            })
        })
        // AT-4: filter before the Provider attempt limit so Channels of the
        // other surface never consume attempt slots.
        .filter(|channel| {
            surface.admits(crate::monoize_routing::resolve_effective_api_type(
                &provider.api_type_overrides,
                channel.provider_type,
                &urp.model,
            ))
        });
    let mut channels = filter_eligible_channels(
        state,
        supporting_channels,
        provider.circuit_breaker_enabled,
        provider
            .per_model_circuit_break
            .then_some(urp.model.as_str()),
    )
    .await;
    if channels.is_empty() {
        return;
    }

    weighted_shuffle_channels(&mut channels);
    let provider_attempt_limit = if provider.max_retries == -1 {
        None
    } else {
        Some(provider.max_retries.max(0) as usize + 1)
    };
    let runtime = state.monoize_runtime.read().await;
    let selected_override = crate::monoize_routing::matching_api_type_override(
        &provider.api_type_overrides,
        &urp.model,
    );
    // MP-G1: the billing group is the group actually used for routing.
    // The provider is group-eligible here, so the rank indexes into
    // `effective_groups`.
    let billing_group_id = effective_groups.as_ref().and_then(|groups| {
        groups
            .get(crate::users::provider_group_rank(
                &provider.group_ids,
                effective_groups,
            ))
            .cloned()
    });
    let mut channel_origins = HashMap::new();
    let mut origin_peers: HashMap<String, Vec<OriginPeer>> = HashMap::new();
    for channel in provider
        .channels
        .iter()
        .filter(|channel| channel.enabled && channel.weight > 0)
    {
        if let Some(origin) = channel_origin_key(&channel.base_url) {
            if provider.circuit_breaker_enabled {
                origin_peers
                    .entry(origin.clone())
                    .or_default()
                    .push(OriginPeer {
                        channel_id: channel.id.clone(),
                        recovery_mode: channel_recovery_mode(state, channel.provider_type),
                        probe_interval: runtime.probe_interval(provider, channel),
                    });
            }
            channel_origins.insert(channel.id.as_str(), origin);
        }
    }
    let origin_peers: HashMap<String, Arc<[OriginPeer]>> = origin_peers
        .into_iter()
        .filter(|(_, peers)| peers.len() > 1)
        .map(|(origin, peers)| (origin, Arc::from(peers)))
        .collect();
    for channel in channels {
        let origin_key = channel_origins.remove(channel.id.as_str());
        let origin_peers = origin_key
            .as_ref()
            .and_then(|origin| origin_peers.get(origin))
            .cloned();
        let model_entry = channel
            .models
            .get(&urp.model)
            .expect("eligible channel must retain its model entry");
        let effective_provider_type =
            selected_override.map_or(channel.provider_type, |entry| entry.api_type);
        out.push(MonoizeAttempt {
            provider_id: provider.id.clone(),
            provider_name: provider.name.clone(),
            provider_type: effective_provider_type.to_config_type(),
            channel_id: channel.id.clone(),
            channel_name: channel.name.clone(),
            base_url: channel.base_url.clone(),
            api_key: channel.api_key.clone(),
            logical_model: urp.model.clone(),
            upstream_model: resolve_upstream_model(&urp.model, model_entry),
            model_multiplier: model_entry.multiplier,
            requested_service_tier: urp.requested_service_tier.clone(),
            server_tool_usage_classes: urp.server_tool_usage_classes.clone(),
            provider_transforms: provider.transforms.clone(),
            passive_failure_count_threshold: channel
                .passive_failure_count_threshold_override
                .unwrap_or(runtime.passive_failure_count_threshold)
                .max(1),
            passive_cooldown_seconds: runtime.passive_cooldown(channel),
            passive_window_seconds: channel
                .passive_window_seconds_override
                .unwrap_or(runtime.passive_window_seconds)
                .max(1),
            rate_limit_wait: runtime.rate_limit_wait(provider, channel),
            probe_interval: runtime.probe_interval(provider, channel),
            recovery_mode: channel_recovery_mode(state, channel.provider_type),
            channel_max_retries: provider.channel_max_retries,
            channel_retry_interval_ms: provider.channel_retry_interval_ms.max(0) as u64,
            failure_classification_rules: runtime.failure_classification_rules.clone(),
            circuit_breaker_enabled: provider.circuit_breaker_enabled,
            per_model_circuit_break: provider.per_model_circuit_break,
            provider_attempt_limit,
            request_timeout_ms: runtime.request_timeout(provider),
            extra_fields_whitelist: merge_extra_fields_whitelist(
                &runtime.extra_fields_whitelist,
                &provider.extra_fields_whitelist,
                effective_provider_type,
            ),
            strip_cross_protocol_nested_extra: provider
                .strip_cross_protocol_nested_extra
                .unwrap_or(runtime.strip_cross_protocol_nested_extra),
            model_price: None,
            pricing_model_key: String::new(),
            allow_free_when_unpriced: provider
                .allow_free_when_unpriced_override
                .unwrap_or(runtime.allow_free_when_unpriced),
            allow_free_when_missing_usage: provider
                .allow_free_when_missing_usage_override
                .unwrap_or(runtime.allow_free_when_missing_usage),
            billing_group_id: billing_group_id.clone(),
            group_billing_ratio: Multiplier::ONE,
            affinity_key: None,
            affinity_key_hash: None,
            affinity_hit: None,
            affinity_target: None,
            affinity_enabled: channel
                .affinity_enabled_override
                .unwrap_or(runtime.affinity_enabled),
            affinity_idle_ttl_seconds: channel
                .affinity_idle_ttl_seconds_override
                .unwrap_or(runtime.affinity_idle_ttl_seconds)
                .max(1),
            affinity_failback_mode: channel
                .affinity_failback_mode_override
                .unwrap_or(runtime.affinity_failback_mode),
            affinity_failback_delay_seconds: channel
                .affinity_failback_delay_seconds_override
                .unwrap_or(runtime.affinity_failback_delay_seconds),
            routing_config_revision,
            proxy_url: {
                crate::error_sanitize::register_configured_url(&channel.base_url);
                if let Some(proxy_url) = channel.proxy_url.as_deref() {
                    crate::error_sanitize::register_configured_url(proxy_url);
                }
                channel.proxy_url.clone()
            },
            extra_headers: channel.extra_headers.clone(),
            session_affinity_auto: effective_session_affinity_auto(
                &channel.base_url,
                channel.session_affinity_auto,
            ),
            websocket_supported: effective_websocket_supported(state, channel),
            chat_dialect: selected_override
                .map_or(channel.chat_dialect, |entry| entry.chat_dialect)
                .unwrap_or(urp::encode::openai_chat::ChatDialect::Generic),
            client_session_id: None,
            derived_session_affinity: None,
            session_affinity_value: None,
            client_anthropic_headers: Default::default(),
            origin_key,
            origin_peers,
        });
    }
}

/// CM-AFF-0: a null setting enables affinity for Cloudflare Workers AI and
/// OpenCode Zen/Go URLs.
pub(super) fn effective_session_affinity_auto(base_url: &str, configured: Option<bool>) -> bool {
    configured.unwrap_or_else(|| crate::monoize_routing::default_session_affinity_auto(base_url))
}

pub(super) fn effective_websocket_supported(
    state: &AppState,
    channel: &crate::monoize_routing::MonoizeChannel,
) -> Option<bool> {
    channel.websocket_supported.or_else(|| {
        state
            .websocket_supported_overlay
            .get(&channel.id)
            .map(|value| *value)
    })
}

pub(super) async fn remember_websocket_supported(
    state: &AppState,
    attempt: &mut MonoizeAttempt,
    supported: bool,
) {
    if attempt.websocket_supported.is_some() {
        return;
    }
    if state.node.is_replica() {
        let remembered = *state
            .websocket_supported_overlay
            .entry(attempt.channel_id.clone())
            .or_insert(supported);
        attempt.websocket_supported = Some(remembered);
        return;
    }
    match state
        .monoize_store
        .remember_channel_websocket_supported(&attempt.channel_id, supported)
        .await
    {
        Ok(true) => {
            let _health = state.channel_health.lock().await;
            let previous = state
                .routing_config_revision
                .fetch_add(1, std::sync::atomic::Ordering::AcqRel);
            if attempt.routing_config_revision == previous {
                attempt.routing_config_revision = previous + 1;
            }
            attempt.websocket_supported = Some(supported);
        }
        Ok(false) => {
            attempt.websocket_supported = Some(supported);
        }
        Err(err) => {
            tracing::warn!(
                channel_id = %attempt.channel_id,
                error = %err,
                "failed to persist channel websocket_supported"
            );
            attempt.websocket_supported = Some(supported);
        }
    }
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
    let anthropic_headers = req
        .map(|req| req.context.anthropic_client_headers.clone())
        .unwrap_or_default();
    for attempt in attempts {
        attempt.client_anthropic_headers = anthropic_headers.clone();
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

async fn filter_eligible_channels<'a>(
    state: &AppState,
    channels: impl Iterator<Item = &'a MonoizeChannel>,
    circuit_breaker_enabled: bool,
    model: Option<&str>,
) -> Vec<&'a MonoizeChannel> {
    let enabled_channels = channels.filter(|channel| channel.enabled && channel.weight > 0);
    if !circuit_breaker_enabled {
        return enabled_channels.collect();
    }
    let health = state.channel_health.lock().await;
    let now = now_ts();
    enabled_channels
        .filter(|channel| {
            if let Some(model) = model {
                channel_health_allows_routing(&health, &health_key(&channel.id, Some(model)), now)
            } else {
                channel_health_allows_routing(&health, &channel.id, now)
            }
        })
        .collect()
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
    let now = now_ts();
    if let Some(model) = attempt_health_model(attempt) {
        channel_health_allows_routing(&health, &health_key(&attempt.channel_id, Some(model)), now)
    } else {
        channel_health_allows_routing(&health, &attempt.channel_id, now)
    }
}

fn channel_health_allows_routing(
    health: &HashMap<String, ChannelHealthState>,
    key: &str,
    now: i64,
) -> bool {
    health.get(key).map_or_else(
        || health.len() < crate::monoize_routing::channel_health_max_entries(),
        |entry| !entry.is_unhealthy(now),
    )
}

fn weighted_shuffle_channels(channels: &mut [&MonoizeChannel]) {
    let mut total_weight = channels.iter().map(|channel| channel.weight as u64).sum();
    for start in 0..channels.len().saturating_sub(1) {
        let mut target = random_u64(total_weight);
        let offset = channels[start..]
            .iter()
            .position(|channel| {
                let weight = channel.weight as u64;
                if target < weight {
                    true
                } else {
                    target -= weight;
                    false
                }
            })
            .expect("positive candidate weights must cover the random target");
        channels.swap(start, start + offset);
        total_weight -= channels[start].weight as u64;
    }
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
    build_upstream_provider_config(
        attempt.provider_type,
        &attempt.provider_id,
        &attempt.channel_id,
        &attempt.base_url,
    )
}

pub(super) fn build_upstream_provider_config(
    provider_type: ProviderType,
    provider_id: &str,
    channel_id: &str,
    base_url: &str,
) -> ProviderConfig {
    let (auth_type, header_name, query_name) = match provider_type {
        ProviderType::Gemini => (
            ProviderAuthType::Header,
            Some("x-goog-api-key".to_string()),
            None,
        ),
        ProviderType::Messages => (
            ProviderAuthType::Header,
            Some("x-api-key".to_string()),
            None,
        ),
        _ => (ProviderAuthType::Bearer, None, None),
    };
    ProviderConfig {
        id: format!("{provider_id}_{channel_id}"),
        provider_type,
        base_url: Some(base_url.to_owned()),
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
) -> Vec<(&'static str, &'static str)> {
    match provider_type {
        ProviderType::Messages => {
            let mut headers = vec![("anthropic-version", "2023-06-01")];
            if messages_body_uses_files_api(body) {
                headers.push(("anthropic-beta", "files-api-2025-04-14"));
            }
            if body.pointer("/thinking/display").and_then(Value::as_str) == Some("updates") {
                headers.push(("anthropic-beta", "thinking-display-updates-2026-08-18"));
            }
            if messages_body_uses_inline_tools(body) {
                headers.push(("anthropic-beta", "inline-tools-2026-09-15"));
            }
            if messages_body_uses_mid_conversation_output_config(body) {
                headers.push((
                    "anthropic-beta",
                    "mid-conversation-output-config-2026-07-01",
                ));
            }
            headers
        }
        ProviderType::Replicate => vec![("prefer", "wait=60")],
        _ => Vec::new(),
    }
}

/// PM8/PM8h: reads the downstream `anthropic-version` and every `anthropic-beta` token.
pub(super) fn anthropic_client_headers(headers: &HeaderMap) -> urp::AnthropicClientHeaders {
    let version = headers
        .get("anthropic-version")
        .and_then(|value| value.to_str().ok())
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .map(str::to_string);
    let mut beta: Vec<String> = Vec::new();
    for value in headers.get_all("anthropic-beta") {
        let Ok(value) = value.to_str() else {
            continue;
        };
        for token in value
            .split(',')
            .map(str::trim)
            .filter(|token| !token.is_empty())
        {
            if !beta.iter().any(|existing| existing == token) {
                beta.push(token.to_string());
            }
        }
    }
    urp::AnthropicClientHeaders { version, beta }
}

const SESSION_CACHE_HEADER_NAMES: &[&str] = &["x-session-affinity", "x-opencode-session"];

fn header_list_contains(headers: &[(String, String)], name: &str) -> bool {
    headers
        .iter()
        .any(|(existing, _)| existing.eq_ignore_ascii_case(name))
}

fn extra_session_cache_header_value(
    extra_headers: Option<&std::collections::BTreeMap<String, String>>,
) -> Option<&str> {
    let headers = extra_headers?;
    for name in SESSION_CACHE_HEADER_NAMES {
        if let Some(value) = headers
            .iter()
            .find_map(|(key, value)| key.eq_ignore_ascii_case(name).then_some(value.as_str()))
        {
            return Some(value);
        }
    }
    None
}

/// CM-HDR-1 + CM-AFF-1/1a/2/5: protocol headers first, then the Channel's
/// static `extra_headers`, then the resolved session-cache identifier as
/// `x-session-affinity` and `x-opencode-session` when automatic session
/// affinity is enabled and those names are not already present.
pub(super) fn attempt_extra_headers(
    attempt: &MonoizeAttempt,
    body: &serde_json::Value,
) -> Vec<(String, String)> {
    build_upstream_headers(
        attempt.provider_type,
        body,
        attempt.extra_headers.as_ref(),
        Some(&attempt.client_anthropic_headers),
        resolve_session_affinity_value(attempt, body).as_deref(),
    )
}

pub(super) fn build_upstream_headers(
    provider_type: ProviderType,
    body: &Value,
    extra_headers: Option<&std::collections::BTreeMap<String, String>>,
    client_anthropic_headers: Option<&urp::AnthropicClientHeaders>,
    session_affinity: Option<&str>,
) -> Vec<(String, String)> {
    let mut out: Vec<(String, String)> = provider_extra_headers(provider_type, body)
        .iter()
        .map(|(name, value)| ((*name).to_string(), (*value).to_string()))
        .collect();
    // PM8/PM8h: downstream anthropic headers reach only Messages upstreams; beta tokens sit
    // between generated and Channel tokens in the merged header.
    if provider_type == ProviderType::Messages
        && let Some(client) = client_anthropic_headers
    {
        if let Some(version) = client.version.as_deref() {
            for (name, value) in &mut out {
                if name.eq_ignore_ascii_case("anthropic-version") {
                    *value = version.to_string();
                }
            }
        }
        if !client.beta.is_empty() {
            out.push(("anthropic-beta".to_string(), client.beta.join(",")));
        }
    }
    if let Some(extras) = extra_headers {
        for (name, value) in extras {
            out.push((name.clone(), value.clone()));
        }
    }
    let mut beta_tokens = Vec::new();
    out.retain(|(name, value)| {
        if provider_type != ProviderType::Messages || !name.eq_ignore_ascii_case("anthropic-beta") {
            return true;
        }
        for token in value
            .split(',')
            .map(str::trim)
            .filter(|token| !token.is_empty())
        {
            if !beta_tokens.iter().any(|existing| existing == token) {
                beta_tokens.push(token.to_string());
            }
        }
        false
    });
    if !beta_tokens.is_empty() {
        out.push(("anthropic-beta".to_string(), beta_tokens.join(",")));
    }
    if let Some(value) = session_affinity {
        for name in SESSION_CACHE_HEADER_NAMES {
            if !header_list_contains(&out, name) {
                out.push(((*name).to_string(), value.to_owned()));
            }
        }
    }
    out
}

/// CM-AFF-1/1a/1b/2 + CM-AFF-4: the single effective session-cache identifier
/// for one attempt. Priority: explicit static `extra_headers` entry
/// (`x-session-affinity` then `x-opencode-session`), then the client header
/// or body conversation identifier, then `prompt_cache_key`, then the
/// decoded-request digest, then encoded-body digest without tools.
pub(super) fn resolve_session_affinity_value(
    attempt: &MonoizeAttempt,
    body: &serde_json::Value,
) -> Option<String> {
    if !attempt.session_affinity_auto {
        return None;
    }
    if let Some(value) = extra_session_cache_header_value(attempt.extra_headers.as_ref()) {
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
    Value::Array(req.input.iter().filter(|node| matches!(node, urp::Node::Text { extra_body, .. } if extra_body.get(urp::RESPONSES_INSTRUCTION_NODE_EXTRA_KEY).and_then(Value::as_bool) == Some(true))).map(canonical_session_head_node).collect())
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

/// `tool_addition` / `tool_removal` are only legal system content when the
/// inline-tools beta is present. Without it Anthropic drops the block and
/// rejects the message as empty system content.
fn messages_body_uses_inline_tools(body: &serde_json::Value) -> bool {
    body.get("messages")
        .and_then(serde_json::Value::as_array)
        .is_some_and(|messages| {
            messages.iter().any(|message| {
                message
                    .get("content")
                    .and_then(serde_json::Value::as_array)
                    .is_some_and(|blocks| {
                        blocks.iter().any(|block| {
                            matches!(
                                block.get("type").and_then(serde_json::Value::as_str),
                                Some("tool_addition" | "tool_removal")
                            )
                        })
                    })
            })
        })
}

/// A mid-conversation system message that only sets `output_config` has an
/// empty `content` array. Anthropic accepts that only with this beta; otherwise
/// it returns "system content must contain at least one block".
fn messages_body_uses_mid_conversation_output_config(body: &serde_json::Value) -> bool {
    body.get("messages")
        .and_then(serde_json::Value::as_array)
        .is_some_and(|messages| {
            messages.iter().any(|message| {
                message.get("role").and_then(serde_json::Value::as_str) == Some("system")
                    && message.get("output_config").is_some()
            })
        })
}

/// SAN-7: the operator-facing internal detail keeps the attempt count and the
/// unmasked internal error of the final attempt for request-log persistence.
pub(super) fn build_exhausted_error_detail(model: &str, tried: &[TriedProvider]) -> String {
    if tried.is_empty() {
        return format!("No available upstream provider for model: {model}");
    }
    let last_error = &tried[tried.len() - 1].error;
    format!(
        "All {n} upstream attempt(s) failed for model: {model}. Last error: {last_error}",
        n = tried.len(),
    )
}

/// RTA-8 `type` for a final attempt that reported no upstream type.
pub(super) fn openai_error_type_for_status(status: StatusCode) -> &'static str {
    match status.as_u16() {
        401 => "authentication_error",
        403 => "permission_error",
        404 => "not_found_error",
        429 => "rate_limit_error",
        400..=499 => "invalid_request_error",
        _ => "server_error",
    }
}

/// RTA-8: the native error of the final failed attempt. SAN-6: its message is
/// the attempt's client text; the wrapper text stays in `internal_message`.
pub(super) fn build_exhausted_upstream_error(model: &str, tried: &[TriedProvider]) -> AppError {
    let Some(last) = tried.last() else {
        let message = build_exhausted_error_detail(model, tried);
        return AppError::new(StatusCode::BAD_GATEWAY, "upstream_error", message.clone())
            .with_type(openai_error_type_for_status(StatusCode::BAD_GATEWAY))
            .with_internal_message(message);
    };
    let code = last
        .upstream_code
        .as_deref()
        .filter(|code| !code.is_empty())
        .unwrap_or("upstream_error");
    if last.target_capability {
        let status = last
            .upstream_status
            .and_then(|status| StatusCode::from_u16(status).ok())
            .unwrap_or(StatusCode::BAD_REQUEST);
        return AppError::new(status, code, last.client_error.clone())
            .with_internal_message(build_exhausted_error_detail(model, tried));
    }
    if let Some(upstream_status @ (401 | 403 | 407)) = last.upstream_status {
        let mut err = AppError::new(
            StatusCode::BAD_GATEWAY,
            "upstream_error",
            last.client_error.clone(),
        )
        .with_internal_message(build_exhausted_error_detail(model, tried))
        .with_type(openai_error_type_for_status(StatusCode::BAD_GATEWAY));
        err.credential_failure = Some(crate::error::CredentialFailure {
            upstream_status,
            upstream_code: last.reported_upstream_code.clone(),
        });
        return err;
    }
    let signature_invalid = code == "thinking_signature_invalid";
    let final_status = last
        .upstream_status
        .and_then(|status| StatusCode::from_u16(status).ok());
    let status = if signature_invalid {
        final_status
            .filter(StatusCode::is_client_error)
            .unwrap_or(StatusCode::BAD_REQUEST)
    } else {
        final_status
            .filter(|status| status.is_client_error() || status.is_server_error())
            .unwrap_or(StatusCode::BAD_GATEWAY)
    };
    let internal_message = if signature_invalid {
        last.error.clone()
    } else {
        build_exhausted_error_detail(model, tried)
    };
    let error_type = last
        .upstream_type
        .as_deref()
        .filter(|error_type| !error_type.is_empty())
        .unwrap_or_else(|| openai_error_type_for_status(status))
        .to_string();
    let mut err = AppError::new(status, code, last.client_error.clone())
        .with_internal_message(internal_message)
        .with_type(error_type);
    err.param = last.upstream_param.clone();
    err.upstream_status = last.upstream_status;
    err.upstream_code = last.upstream_code.clone();
    err.upstream_type = last.upstream_type.clone();
    err.upstream_param = last.upstream_param.clone();
    err
}

fn status_allows_same_channel_retry(status: StatusCode) -> bool {
    status == StatusCode::REQUEST_TIMEOUT
        || status == StatusCode::TOO_MANY_REQUESTS
        || status.is_server_error()
}

pub(super) fn classify_retryable_failure(err: &UpstreamCallError) -> Option<RetryableFailureClass> {
    let network = matches!(err.kind, UpstreamErrorKind::Network);
    (network || err.status.is_some_and(status_allows_same_channel_retry)).then(|| {
        classify_channel_health_failure(
            err.status.map(|status| status.as_u16()),
            err.code.as_deref(),
            err.error_type.as_deref(),
        )
        .unwrap_or(RetryableFailureClass::Transient)
    })
}

pub(super) fn classify_retryable_app_failure(err: &AppError) -> Option<RetryableFailureClass> {
    // Only an upstream status, not the local wrapper status, can authorize another dispatch.
    let status = StatusCode::from_u16(err.upstream_status?).ok()?;
    status_allows_same_channel_retry(status).then(|| {
        classify_channel_health_failure(
            err.upstream_status,
            err.upstream_code.as_deref(),
            err.upstream_type.as_deref(),
        )
        .unwrap_or(RetryableFailureClass::Transient)
    })
}

pub(crate) fn classify_channel_health_failure(
    http_status: Option<u16>,
    error_code: Option<&str>,
    error_type: Option<&str>,
) -> Option<RetryableFailureClass> {
    let signals = [error_code.map(str::trim), error_type.map(str::trim)];
    let has_signal = |expected: &[&str]| {
        signals.iter().flatten().any(|signal| {
            expected
                .iter()
                .any(|value| signal.eq_ignore_ascii_case(value))
        })
    };

    if has_signal(&[
        "insufficient_quota",
        "credit_balance_exhausted",
        "credit_balance_too_low",
        "insufficient_balance",
        "billing_hard_limit_reached",
    ]) {
        return Some(RetryableFailureClass::Persistent);
    }

    if http_status == Some(StatusCode::TOO_MANY_REQUESTS.as_u16()) {
        return Some(RetryableFailureClass::RateLimited);
    }

    if matches!(
        http_status,
        Some(401 | 402 | 403 | 404 | 405 | 407 | 410 | 415 | 426 | 451)
    ) {
        return Some(RetryableFailureClass::Persistent);
    }

    if has_signal(&[
        "rate_limit_error",
        "rate_limit_exceeded",
        "too_many_requests",
    ]) {
        return Some(RetryableFailureClass::RateLimited);
    }

    if has_signal(&[
        "account_deactivated",
        "account_suspended",
        "authentication_error",
        "deployment_not_found",
        "invalid_api_key",
        "model_not_found",
        "model_not_supported",
        "no_available_account",
        "permission_denied",
        "quota_exceeded",
        "unsupported_model",
    ]) {
        return Some(RetryableFailureClass::Persistent);
    }

    if http_status == Some(StatusCode::REQUEST_TIMEOUT.as_u16())
        || http_status.is_some_and(|status| (500..600).contains(&status))
        || has_signal(&[
            "overloaded_error",
            "server_error",
            "service_unavailable",
            "temporarily_unavailable",
        ])
    {
        return Some(RetryableFailureClass::Transient);
    }

    None
}

/// RTA-6/AFF-9: applies one failed upstream attempt to Channel health and the
/// affinity binding. Returns whether the failure is an RTA-6c shared-origin
/// blast, which also marks the origin's peer Channels.
pub(super) async fn record_channel_failure(
    state: &AppState,
    attempt: &MonoizeAttempt,
    failure_class: Option<RetryableFailureClass>,
    blast_status: Option<u16>,
    retry_after_seconds: Option<u16>,
) -> bool {
    let Some(failure_class) = failure_class else {
        return false;
    };
    let shared_origin_blast =
        failure_class == RetryableFailureClass::Transient && is_shared_origin_status(blast_status);
    let unhealthy = record_passive_samples(
        state,
        attempt,
        failure_class,
        retry_after_seconds,
        shared_origin_blast,
    )
    .await;
    if attempt.affinity_hit == Some(true) && !shared_origin_blast && unhealthy {
        clear_channel_affinity(state, attempt).await;
    }
    shared_origin_blast
}

/// STRM-4 + STRM-4a: apply one breaker-relevant terminal failure that occurs
/// after the first downstream byte. Mid-stream failures never blast
/// shared-origin peers. The affinity binding is cleared only when the serving
/// attempt is the bound target and the failure tripped the breaker.
pub(super) async fn record_midstream_terminal_failure(
    state: &AppState,
    attempt: &MonoizeAttempt,
    failure_class: RetryableFailureClass,
) {
    let unhealthy = record_passive_samples(state, attempt, failure_class, None, false).await;
    if attempt.affinity_hit == Some(true) && unhealthy {
        clear_channel_affinity(state, attempt).await;
    }
}

/// STRM-4: only upstream-side adapter failures (idle timeout, malformed
/// upstream stream, connection loss, missing terminal) count as breaker-
/// relevant terminal failures. Internal transform/encode errors do not
/// penalize the Channel.
pub(super) fn is_upstream_adapter_failure(err: &AppError) -> bool {
    err.code.starts_with("upstream_")
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
    let mut health = state.channel_health.lock().await;
    if state.routing_config_revision.load(Ordering::Acquire) != attempt.routing_config_revision {
        return;
    }
    let key = health_key(&attempt.channel_id, attempt_health_model(attempt));
    if !crate::monoize_routing::prepare_channel_health_insert(&mut health, &key) {
        return;
    }
    let entry = health.entry(key).or_default();
    let now = now_ts();
    let was_unhealthy = entry.is_unhealthy(now);
    entry.mark_success(now);
    prune_passive_failure_timestamps(
        &mut entry.passive_failure_timestamps,
        now,
        attempt.passive_window_seconds,
    );
    if was_unhealthy {
        tracing::info!(channel_id = %attempt.channel_id, "channel recovered to healthy after success");
    }
}

struct PassiveSample {
    now: i64,
    failure_class: RetryableFailureClass,
    retry_after_seconds: Option<u16>,
}

async fn record_passive_samples(
    state: &AppState,
    attempt: &MonoizeAttempt,
    failure_class: RetryableFailureClass,
    retry_after_seconds: Option<u16>,
    blast: bool,
) -> bool {
    if !attempt.circuit_breaker_enabled {
        return false;
    }
    let mut health = state.channel_health.lock().await;
    if state.routing_config_revision.load(Ordering::Acquire) != attempt.routing_config_revision {
        return false;
    }
    let sample = PassiveSample {
        now: now_ts(),
        failure_class,
        retry_after_seconds,
    };
    let unhealthy = apply_retryable_failure_to_channel(&mut health, attempt, &sample, None);
    if blast && let Some(peers) = &attempt.origin_peers {
        for peer in peers
            .iter()
            .filter(|peer| peer.channel_id != attempt.channel_id)
        {
            apply_retryable_failure_to_channel(&mut health, attempt, &sample, Some(peer));
        }
        tracing::info!(
            channel_id = %attempt.channel_id,
            origin_key = attempt.origin_key.as_deref().unwrap_or(""),
            peer_count = peers.len() - 1,
            "shared-origin blast applied to same-base-url channels"
        );
    }
    unhealthy
}

fn apply_retryable_failure_to_channel(
    health: &mut HashMap<String, ChannelHealthState>,
    attempt: &MonoizeAttempt,
    sample: &PassiveSample,
    peer: Option<&OriginPeer>,
) -> bool {
    let channel_id = peer.map_or(attempt.channel_id.as_str(), |peer| peer.channel_id.as_str());
    let key = health_key(channel_id, attempt_health_model(attempt));
    if !crate::monoize_routing::prepare_channel_health_insert(health, &key) {
        return false;
    }
    let entry = health.entry(key).or_default();
    prune_passive_failure_timestamps(
        &mut entry.passive_failure_timestamps,
        sample.now,
        attempt.passive_window_seconds,
    );
    let failure_threshold = crate::monoize_routing::effective_passive_failure_threshold(
        attempt.passive_failure_count_threshold,
    );
    let samples = &mut entry.passive_failure_timestamps;
    if samples.len() > failure_threshold {
        samples.drain(..samples.len() - failure_threshold);
    }
    if samples.len() < failure_threshold {
        samples.push_back(sample.now);
    }
    if entry.is_unhealthy(sample.now) {
        return true;
    }
    let failure_samples = entry.passive_failure_timestamps.len();
    if sample.failure_class != RetryableFailureClass::Persistent
        && failure_samples < failure_threshold
    {
        return false;
    }
    let mode = peer.map_or(attempt.recovery_mode, |peer| peer.recovery_mode);
    let wait = entry.trip(
        sample.now,
        mode,
        mode.ordinary_wait(
            peer.map_or(attempt.probe_interval, |peer| peer.probe_interval),
            attempt.passive_cooldown_seconds,
        ),
        attempt.rate_limit_wait,
        sample.failure_class == RetryableFailureClass::RateLimited,
        sample.retry_after_seconds,
    );
    if peer.is_none() {
        tracing::info!(
            channel_id = %channel_id,
            failure_class = ?sample.failure_class,
            failed_samples = failure_samples,
            recovery_wait_seconds = wait,
            "channel marked unhealthy after passive breaker failure"
        );
    }
    true
}
pub(super) fn upstream_error_to_app(
    mut err: UpstreamCallError,
    mask_sensitive_info: bool,
) -> AppError {
    let status = err.status.unwrap_or(StatusCode::BAD_GATEWAY);
    let failure_response = err.failure_response();
    // SAN-3: the raw unmasked upstream detail (transport text with the full
    // upstream URL, raw unparsed error bodies) exists in the server log only.
    tracing::warn!(status = %status, upstream_error = %err.message, "upstream request failed");
    // SAN-1 when masking is enabled; SAN-CFG5 items 1-4 when the admin
    // disabled `monoize_mask_sensitive_info`.
    let client_message = match err.source {
        upstream::UpstreamErrorSource::Transport if mask_sensitive_info => {
            "failed to request upstream".to_string()
        }
        upstream::UpstreamErrorSource::UnparsedBody if mask_sensitive_info => {
            format!("upstream status {status}")
        }
        upstream::UpstreamErrorSource::EmptyBody => format!("upstream status {status}"),
        upstream::UpstreamErrorSource::Transport | upstream::UpstreamErrorSource::UnparsedBody => {
            format!(
                "upstream status {status}: {}",
                crate::error_sanitize::truncate_error_detail(&err.message)
            )
        }
        upstream::UpstreamErrorSource::StructuredBody => {
            crate::error_sanitize::maybe_mask_sensitive_text(&err.message, mask_sensitive_info)
        }
        upstream::UpstreamErrorSource::Internal => {
            format!(
                "upstream status {status}: {}",
                crate::error_sanitize::maybe_mask_sensitive_text(&err.message, mask_sensitive_info)
            )
        }
    };
    // SAN-2: unmasked, TRUNC-bounded detail for request-log persistence.
    // Admins read it verbatim; non-admin dashboard reads mask it at read
    // time (SAN-13/SAN-14).
    let internal_message = format!(
        "upstream status {status}: {}",
        crate::error_sanitize::truncate_error_detail(&err.message)
    );
    let forwarded_headers = err.take_forwarded_headers();
    let mut app_err = AppError::new(status, "upstream_error", client_message)
        .with_internal_message(internal_message)
        .with_upstream_error(
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
    app_err.forwarded_headers = forwarded_headers;
    failure_response.apply(&mut app_err);
    app_err.reported_error = err.reported_error;
    app_err
}

pub(super) fn openai_error_json(err: &AppError) -> Value {
    let mut error = json!({
        "message": err.message,
        "type": err.error_type,
        "code": err.upstream_code.as_ref().unwrap_or(&err.code),
        "param": err.param,
    });
    if let Some(failure) = &err.credential_failure {
        failure.insert_into(&mut error);
    }
    json!({ "error": error })
}

/// SE1c: the native Messages `error.type` of an error.
pub(super) fn anthropic_error_type(err: &AppError) -> String {
    const NATIVE: [&str; 11] = [
        "invalid_request_error",
        "authentication_error",
        "billing_error",
        "permission_error",
        "not_found_error",
        "conflict_error",
        "request_too_large",
        "rate_limit_error",
        "api_error",
        "timeout_error",
        "overloaded_error",
    ];
    // `AppError::new` defaults `error_type` to `invalid_request_error`, which
    // says nothing about a local 401 or 403.
    let local = (err.error_type != "invalid_request_error").then_some(err.error_type.as_str());
    if let Some(error_type) = err
        .upstream_type
        .as_deref()
        .or(local)
        .filter(|error_type| NATIVE.contains(error_type))
    {
        return error_type.to_string();
    }
    match err.status.as_u16() {
        401 => "authentication_error",
        402 => "billing_error",
        403 => "permission_error",
        404 => "not_found_error",
        409 => "conflict_error",
        413 => "request_too_large",
        429 => "rate_limit_error",
        504 => "timeout_error",
        529 => "overloaded_error",
        400..=499 => "invalid_request_error",
        _ => "api_error",
    }
    .to_string()
}

/// SE1, SE1b: the native Messages error body.
pub(super) fn anthropic_error_json(err: &AppError, request_id: &str) -> Value {
    let mut error = json!({
        "type": anthropic_error_type(err),
        "message": err.message,
    });
    if let Some(failure) = &err.credential_failure {
        failure.insert_into(&mut error);
    }
    json!({
        "type": "error",
        "error": error,
        "request_id": request_id,
    })
}

pub(super) fn responses_stream_error_json(seq: u64, err: &AppError) -> Value {
    let status = err.upstream_status.unwrap_or(err.status.as_u16());
    let code = err.upstream_code.as_deref().unwrap_or_else(|| {
        if (500..=599).contains(&status) {
            "server_error"
        } else {
            &err.code
        }
    });
    let mut event = json!({
        "type": "error",
        "sequence_number": seq,
        "code": code,
        "message": err.message,
        "param": err.upstream_param.as_ref().or(err.param.as_ref()),
    });
    if let Some(failure) = &err.credential_failure {
        failure.insert_into(&mut event);
    }
    event
}

pub(super) fn merge_extra_fields_whitelist(
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

#[cfg(test)]
mod messages_beta_header_tests {
    use super::provider_extra_headers;
    use crate::config::ProviderType;

    fn beta_tokens(body: serde_json::Value) -> Vec<&'static str> {
        provider_extra_headers(ProviderType::Messages, &body)
            .into_iter()
            .filter(|(name, _)| *name == "anthropic-beta")
            .map(|(_, value)| value)
            .collect()
    }

    #[test]
    fn inline_tool_change_adds_beta_header() {
        let tokens = beta_tokens(serde_json::json!({
            "model": "claude-opus-5-5",
            "messages": [{
                "role": "system",
                "content": [{
                    "type": "tool_removal",
                    "tool": {"type": "tool_reference", "name": "ops_discover_tools"}
                }]
            }]
        }));
        assert!(tokens.contains(&"inline-tools-2026-09-15"));
        assert!(!tokens.contains(&"mid-conversation-output-config-2026-07-01"));
    }

    #[test]
    fn effort_only_system_message_adds_output_config_beta() {
        let tokens = beta_tokens(serde_json::json!({
            "messages": [{
                "role": "system",
                "content": [],
                "output_config": {"effort": "medium"}
            }],
            "output_config": {"effort": "high"}
        }));
        assert!(tokens.contains(&"mid-conversation-output-config-2026-07-01"));
        assert!(!tokens.contains(&"inline-tools-2026-09-15"));
    }

    #[test]
    fn top_level_output_config_does_not_add_mid_conversation_beta() {
        let tokens = beta_tokens(serde_json::json!({
            "messages": [{"role": "user", "content": "hi"}],
            "output_config": {"effort": "high"}
        }));
        assert!(!tokens.contains(&"mid-conversation-output-config-2026-07-01"));
        assert!(!tokens.contains(&"inline-tools-2026-09-15"));
    }
}
