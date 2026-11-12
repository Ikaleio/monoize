use crate::app::AppState;
use crate::handlers::routing::health_key;
use crate::monoize_routing::{ChannelHealthState, MonoizeChannel, MonoizeProvider};
use serde::Serialize;
use std::collections::{HashMap, HashSet};
use std::sync::atomic::Ordering;

pub(super) fn reset_channel_health_after_successful_test(
    health: &mut HashMap<String, ChannelHealthState>,
    channel_id: &str,
    per_model_circuit_break: bool,
    model_id: &str,
    now: i64,
    max_entries: usize,
) {
    let key = health_key(channel_id, per_model_circuit_break.then_some(model_id));
    if !health.contains_key(&key) && health.len() >= max_entries {
        return;
    }
    // Every field except the passive-failure samples is overwritten, so the
    // `Default` (unhealthy) starting state never leaks out.
    let entry = health.entry(key).or_default();
    entry.healthy = true;
    entry.cooldown_until = None;
    entry.last_success_at = Some(now);
    entry.probe_success_count = 0;
    entry.last_probe_at = None;
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

pub(super) async fn provider_with_runtime(
    state: &AppState,
    mut provider: MonoizeProvider,
) -> MonoizeProvider {
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

pub(super) async fn prune_provider_channel_health(state: &AppState, channel_ids: &[String]) {
    if channel_ids.is_empty() {
        return;
    }
    let ids: HashSet<&str> = channel_ids.iter().map(String::as_str).collect();
    let mut health = state.channel_health.lock().await;
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

pub(super) fn advance_routing_config_revision(state: &AppState) {
    state.routing_config_revision.fetch_add(1, Ordering::AcqRel);
}
