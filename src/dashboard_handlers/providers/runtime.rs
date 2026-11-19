use crate::app::AppState;
use crate::handlers::channel_probe::ChannelProbeOutcome;
use crate::handlers::routing::{channel_recovery_mode, health_key, now_ts};
use crate::monoize_routing::{ChannelHealthState, MonoizeChannel, MonoizeProvider};
use serde::Serialize;
use std::collections::{HashMap, HashSet};
use std::sync::atomic::Ordering;

pub(super) async fn record_channel_test_health(
    state: &AppState,
    provider: &MonoizeProvider,
    channel: &MonoizeChannel,
    model_id: &str,
    revision: u64,
    outcome: &ChannelProbeOutcome,
) {
    if !(outcome.ok || (provider.circuit_breaker_enabled && outcome.is_persistent_failure())) {
        return;
    }
    let mut health = state.channel_health.lock().await;
    if state.routing_config_revision.load(Ordering::Acquire) != revision {
        return;
    }
    let key = health_key(
        &channel.id,
        provider.per_model_circuit_break.then_some(model_id),
    );
    if !crate::monoize_routing::prepare_channel_health_insert(&mut health, &key) {
        return;
    }
    if outcome.ok {
        health.entry(key).or_default().mark_success(now_ts());
        return;
    }
    let mode = channel_recovery_mode(state, channel.provider_type);
    let ordinary_wait = {
        let runtime = state.monoize_runtime.read().await;
        mode.ordinary_wait(
            runtime.probe_interval(provider, channel),
            runtime.passive_cooldown(channel),
        )
    };
    let now = now_ts();
    let entry = health.entry(key).or_default();
    if !entry.is_unhealthy(now) {
        entry.trip(
            now,
            mode,
            ordinary_wait,
            None,
            false,
            outcome.retry_after_seconds,
        );
    }
}

fn health_timestamp(ts: Option<i64>) -> Option<String> {
    ts.and_then(|value| chrono::DateTime::<chrono::Utc>::from_timestamp(value, 0))
        .map(|value| value.to_rfc3339())
}

#[derive(Default)]
pub(in crate::dashboard_handlers) struct ChannelHealthSummary {
    pub unhealthy: bool,
    pub next_recovery_at: Option<i64>,
    pub last_success_at: Option<i64>,
    pub last_probe_at: Option<i64>,
    pub probe_success_count: u32,
    pub unhealthy_models: Vec<String>,
}

impl ChannelHealthSummary {
    fn add(&mut self, entry: &ChannelHealthState, model: Option<&str>, now: i64) {
        if entry.is_unhealthy(now) {
            self.unhealthy = true;
            if let Some(next) = entry.next_recovery_at {
                self.next_recovery_at = Some(
                    self.next_recovery_at
                        .map_or(next, |current| current.min(next)),
                );
            }
            if let Some(model) = model {
                self.unhealthy_models.push(model.to_owned());
            }
        }
        self.last_success_at = self.last_success_at.max(entry.last_success_at);
        self.last_probe_at = self.last_probe_at.max(entry.last_probe_at);
        self.probe_success_count = self.probe_success_count.max(entry.probe_success_count);
    }
}

pub(in crate::dashboard_handlers) fn summarize_channel_health(
    channel: &MonoizeChannel,
    circuit_breaker_enabled: bool,
    per_model_circuit_break: bool,
    health: &HashMap<String, ChannelHealthState>,
    now: i64,
) -> ChannelHealthSummary {
    let mut summary = ChannelHealthSummary::default();
    if !circuit_breaker_enabled {
        return summary;
    }
    if per_model_circuit_break {
        for model in channel.models.keys() {
            if let Some(entry) = health.get(&health_key(&channel.id, Some(model))) {
                summary.add(entry, Some(model), now);
            }
        }
        summary.unhealthy_models.sort_unstable();
    } else if let Some(entry) = health.get(&channel.id) {
        summary.add(entry, None, now);
    }
    summary
}

pub(super) fn apply_provider_runtime(
    provider: &mut MonoizeProvider,
    health: &HashMap<String, ChannelHealthState>,
    now: i64,
) {
    for channel in &mut provider.channels {
        let summary = summarize_channel_health(
            channel,
            provider.circuit_breaker_enabled,
            provider.per_model_circuit_break,
            health,
            now,
        );
        channel._healthy = Some(!summary.unhealthy);
        channel._health_status = Some(
            if summary.unhealthy {
                "unhealthy"
            } else {
                "healthy"
            }
            .to_owned(),
        );
        channel._last_success_at = health_timestamp(summary.last_success_at);
        channel._unhealthy_models = Some(summary.unhealthy_models);
        channel._next_recovery_at = health_timestamp(summary.next_recovery_at);
    }
}

pub(super) async fn provider_with_runtime(
    state: &AppState,
    mut provider: MonoizeProvider,
) -> MonoizeProvider {
    let health = state.channel_health.lock().await;
    apply_provider_runtime(&mut provider, &health, chrono::Utc::now().timestamp());
    provider
}

#[derive(Debug, Serialize)]
struct ModelRuntimeChannel {
    channel_id: String,
    channel_name: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    next_recovery_at: Option<String>,
}

#[derive(Debug, Serialize)]
pub(super) struct ProviderModelRuntimeStatus {
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

pub(super) fn build_provider_model_runtime_statuses(
    provider: &MonoizeProvider,
    unpriced_entries: &HashSet<(String, String)>,
    health: &HashMap<String, ChannelHealthState>,
    now: i64,
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
                .filter_map(|channel| {
                    if !provider.circuit_breaker_enabled {
                        return None;
                    }
                    let entry = if provider.per_model_circuit_break {
                        health.get(&health_key(&channel.id, Some(&model)))
                    } else {
                        health.get(&channel.id)
                    }?;
                    entry.is_unhealthy(now).then(|| ModelRuntimeChannel {
                        channel_id: channel.id.clone(),
                        channel_name: channel.name.clone(),
                        next_recovery_at: health_timestamp(entry.next_recovery_at),
                    })
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
                    next_recovery_at: None,
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

pub(super) async fn advance_routing_config_revision(state: &AppState, channel_ids: &[String]) {
    let mut health = state.channel_health.lock().await;
    state.routing_config_revision.fetch_add(1, Ordering::AcqRel);
    if channel_ids.is_empty() {
        return;
    }
    let ids: HashSet<&str> = channel_ids.iter().map(String::as_str).collect();
    health.retain(|channel_key, _| {
        let channel_id = channel_key
            .split_once("::")
            .map(|(channel_id, _)| channel_id)
            .unwrap_or(channel_key.as_str());
        !ids.contains(channel_id)
    });
}

pub(super) async fn prune_provider_channel_affinity(state: &AppState, channel_ids: &[String]) {
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
