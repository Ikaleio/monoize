use super::types::AffinityFailbackMode;
use crate::settings::default_reasoning_suffix_map;
use crate::transforms::TransformRuleConfig;
use serde::{Deserialize, Serialize};
use std::collections::{HashMap, VecDeque};
use std::sync::OnceLock;
use std::time::Duration;

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct MonoizeRuntimeConfig {
    pub request_timeout_ms: u64,
    pub stream_idle_timeout_ms: u64,
    pub enable_estimated_billing: bool,
    pub passive_failure_count_threshold: u32,
    pub passive_cooldown_seconds: u64,
    pub passive_window_seconds: u64,
    pub passive_rate_limit_cooldown_seconds: u64,
    pub active_enabled: bool,
    pub active_interval_seconds: u64,
    pub active_success_threshold: u32,
    pub active_method: String,
    pub active_probe_model: Option<String>,
    pub global_transforms: Vec<TransformRuleConfig>,
    pub global_model_redirects: Vec<crate::users::ModelRedirectRule>,
    #[serde(skip)]
    pub(crate) compiled_global_model_redirects: Vec<crate::users::CompiledModelRedirectRule>,
    pub reasoning_suffix_map: HashMap<String, String>,
    pub codex_model_ids: Vec<String>,
    /// MP-F1 (`model-pricing.spec.md`): global free-settlement flags. Provider
    /// overrides take precedence at attempt preflight.
    pub allow_free_when_unpriced: bool,
    pub allow_free_when_missing_usage: bool,
    /// MP-T1: the `tool_prices` system setting object.
    pub tool_prices: serde_json::Value,
    pub extra_fields_whitelist: HashMap<String, Vec<String>>,
    pub strip_cross_protocol_nested_extra: bool,
    pub request_capture_enabled: bool,
    pub request_capture_max_total_bytes: u64,
    pub mask_sensitive_info: bool,
    pub affinity_enabled: bool,
    pub affinity_idle_ttl_seconds: u64,
    pub affinity_failback_mode: AffinityFailbackMode,
    pub affinity_failback_delay_seconds: u64,
}

impl Default for MonoizeRuntimeConfig {
    fn default() -> Self {
        Self {
            request_timeout_ms: 30_000,
            stream_idle_timeout_ms: 120_000,
            enable_estimated_billing: true,
            passive_failure_count_threshold: 3,
            passive_cooldown_seconds: 60,
            passive_window_seconds: 30,
            passive_rate_limit_cooldown_seconds: 15,
            active_enabled: true,
            active_interval_seconds: 30,
            active_success_threshold: 1,
            active_method: "completion".to_string(),
            active_probe_model: None,
            global_transforms: Vec::new(),
            global_model_redirects: Vec::new(),
            compiled_global_model_redirects: Vec::new(),
            reasoning_suffix_map: default_reasoning_suffix_map(),
            codex_model_ids: Vec::new(),
            allow_free_when_unpriced: false,
            allow_free_when_missing_usage: false,
            tool_prices: crate::settings::default_tool_prices(),
            extra_fields_whitelist: HashMap::new(),
            strip_cross_protocol_nested_extra: true,
            request_capture_enabled: false,
            request_capture_max_total_bytes:
                crate::settings::DEFAULT_REQUEST_CAPTURE_MAX_TOTAL_BYTES,
            mask_sensitive_info: true,
            affinity_enabled: true,
            affinity_idle_ttl_seconds: 30 * 60,
            affinity_failback_mode: AffinityFailbackMode::Sticky,
            affinity_failback_delay_seconds: 5 * 60,
        }
    }
}

impl MonoizeRuntimeConfig {
    pub fn set_global_model_redirects(
        &mut self,
        rules: Vec<crate::users::ModelRedirectRule>,
    ) -> Result<(), String> {
        let compiled = crate::users::compile_model_redirects(&rules)?;
        self.global_model_redirects = rules;
        self.compiled_global_model_redirects = compiled;
        Ok(())
    }
}

#[derive(Debug, Clone, Default)]
pub struct ChannelHealthState {
    pub healthy: bool,
    pub last_success_at: Option<i64>,
    pub cooldown_until: Option<i64>,
    pub probe_success_count: u32,
    pub last_probe_at: Option<i64>,
    pub passive_failure_timestamps: VecDeque<i64>,
}

#[derive(Debug, Clone)]
pub struct ChannelAffinityBinding {
    pub provider_id: String,
    pub channel_id: String,
    pub bound_at: i64,
    pub last_used_at: i64,
    pub expires_at: i64,
}

pub const DEFAULT_CHANNEL_AFFINITY_MAX_ENTRIES: usize = 4096;
pub const DEFAULT_CHANNEL_AFFINITY_CLEANUP_INTERVAL_SECONDS: u64 = 60;
pub const DEFAULT_CHANNEL_HEALTH_MAX_ENTRIES: usize = 10_000;
pub const DEFAULT_CHANNEL_PASSIVE_FAILURE_SAMPLE_MAX_ENTRIES: usize = 1024;
pub const DEFAULT_PROVIDER_REORDER_MAX_IDS: usize = 199;
fn parse_positive_entry_limit(raw: Option<&str>, default: usize) -> usize {
    raw.and_then(|value| value.trim().parse::<usize>().ok())
        .filter(|value| *value > 0)
        .unwrap_or(default)
}

fn parse_provider_reorder_limit(raw: Option<&str>) -> usize {
    parse_positive_entry_limit(raw, DEFAULT_PROVIDER_REORDER_MAX_IDS)
        .min(DEFAULT_PROVIDER_REORDER_MAX_IDS)
}

pub(super) fn provider_reorder_max_ids() -> usize {
    static LIMIT: OnceLock<usize> = OnceLock::new();
    *LIMIT.get_or_init(|| {
        parse_provider_reorder_limit(
            std::env::var("MONOIZE_PROVIDER_REORDER_MAX_IDS")
                .ok()
                .as_deref(),
        )
    })
}

pub fn channel_affinity_max_entries() -> usize {
    static LIMIT: OnceLock<usize> = OnceLock::new();
    *LIMIT.get_or_init(|| {
        parse_positive_entry_limit(
            std::env::var("MONOIZE_CHANNEL_AFFINITY_MAX_ENTRIES")
                .ok()
                .as_deref(),
            DEFAULT_CHANNEL_AFFINITY_MAX_ENTRIES,
        )
    })
}

pub fn channel_affinity_cleanup_interval() -> Duration {
    static INTERVAL: OnceLock<Duration> = OnceLock::new();
    *INTERVAL.get_or_init(|| {
        parse_channel_affinity_cleanup_interval(
            std::env::var("MONOIZE_CHANNEL_AFFINITY_CLEANUP_INTERVAL_SECONDS")
                .ok()
                .as_deref(),
        )
    })
}

fn parse_channel_affinity_cleanup_interval(raw: Option<&str>) -> Duration {
    let seconds = raw
        .and_then(|value| value.trim().parse::<u64>().ok())
        .filter(|value| *value > 0)
        .unwrap_or(DEFAULT_CHANNEL_AFFINITY_CLEANUP_INTERVAL_SECONDS);
    Duration::from_secs(seconds)
}

pub fn cleanup_channel_affinity(
    cache: &mut HashMap<String, ChannelAffinityBinding>,
    now_ts: i64,
) -> usize {
    let previous_len = cache.len();
    cache.retain(|_, binding| now_ts < binding.expires_at);
    previous_len - cache.len()
}

pub fn channel_health_max_entries() -> usize {
    static LIMIT: OnceLock<usize> = OnceLock::new();
    *LIMIT.get_or_init(|| {
        parse_positive_entry_limit(
            std::env::var("MONOIZE_CHANNEL_HEALTH_MAX_ENTRIES")
                .ok()
                .as_deref(),
            DEFAULT_CHANNEL_HEALTH_MAX_ENTRIES,
        )
    })
}

pub fn channel_passive_failure_sample_max_entries() -> usize {
    static LIMIT: OnceLock<usize> = OnceLock::new();
    *LIMIT.get_or_init(|| {
        parse_positive_entry_limit(
            std::env::var("MONOIZE_CHANNEL_PASSIVE_FAILURE_SAMPLE_MAX_ENTRIES")
                .ok()
                .as_deref(),
            DEFAULT_CHANNEL_PASSIVE_FAILURE_SAMPLE_MAX_ENTRIES,
        )
    })
}

pub fn effective_passive_failure_threshold(resolved_threshold: u32) -> usize {
    effective_passive_failure_threshold_with_limit(
        resolved_threshold,
        channel_passive_failure_sample_max_entries(),
    )
}

fn effective_passive_failure_threshold_with_limit(resolved_threshold: u32, limit: usize) -> usize {
    (resolved_threshold.max(1) as usize).min(limit.max(1))
}

pub fn prepare_channel_health_insert(
    health: &mut HashMap<String, ChannelHealthState>,
    key: &str,
) -> bool {
    prepare_channel_health_insert_with_limit(health, key, channel_health_max_entries())
}

fn prepare_channel_health_insert_with_limit(
    health: &mut HashMap<String, ChannelHealthState>,
    key: &str,
    limit: usize,
) -> bool {
    health.contains_key(key) || health.len() < limit
}

pub fn missing_channel_health_is_saturated(
    health: &HashMap<String, ChannelHealthState>,
    key: &str,
) -> bool {
    missing_channel_health_is_saturated_with_limit(health, key, channel_health_max_entries())
}

fn missing_channel_health_is_saturated_with_limit(
    health: &HashMap<String, ChannelHealthState>,
    key: &str,
    limit: usize,
) -> bool {
    !health.contains_key(key) && health.len() >= limit
}

impl ChannelHealthState {
    pub fn new() -> Self {
        Self {
            healthy: true,
            last_success_at: None,
            cooldown_until: None,
            probe_success_count: 0,
            last_probe_at: None,
            passive_failure_timestamps: VecDeque::new(),
        }
    }

    pub fn status(&self, now_ts: i64) -> &'static str {
        if self.healthy {
            return "healthy";
        }
        if let Some(until) = self.cooldown_until
            && now_ts < until
        {
            return "unhealthy";
        }
        "probing"
    }
}
