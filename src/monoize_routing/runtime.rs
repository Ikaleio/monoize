use super::types::{AffinityFailbackMode, MonoizeChannel, MonoizeProvider};
use crate::settings::default_reasoning_suffix_map;
use crate::transforms::TransformRuleConfig;
use serde::{Deserialize, Serialize};
use std::collections::{HashMap, VecDeque};
use std::sync::{
    OnceLock,
    atomic::{AtomicU64, Ordering},
};
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
    pub honor_rate_limit: bool,
    pub active_interval_seconds: u64,
    pub active_success_threshold: u32,
    pub active_probe_model: Option<String>,
    pub active_probe_stream: bool,
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
            honor_rate_limit: false,
            active_interval_seconds: 30,
            active_success_threshold: 1,
            active_probe_model: None,
            active_probe_stream: true,
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
    pub fn probe_stream(&self, provider: &MonoizeProvider, channel: &MonoizeChannel) -> bool {
        channel
            .active_probe_stream_override
            .or(provider.active_probe_stream_override)
            .unwrap_or(self.active_probe_stream)
    }

    pub fn rate_limit_wait(
        &self,
        provider: &MonoizeProvider,
        channel: &MonoizeChannel,
    ) -> Option<u64> {
        channel
            .honor_rate_limit_override
            .or(provider.honor_rate_limit_override)
            .unwrap_or(self.honor_rate_limit)
            .then(|| {
                channel
                    .passive_rate_limit_cooldown_seconds_override
                    .unwrap_or(self.passive_rate_limit_cooldown_seconds)
                    .max(1)
            })
    }

    pub fn probe_interval(&self, provider: &MonoizeProvider, channel: &MonoizeChannel) -> u64 {
        channel
            .active_probe_interval_seconds_override
            .or(provider.active_probe_interval_seconds_override)
            .unwrap_or(self.active_interval_seconds)
            .max(1)
    }

    pub fn request_timeout(&self, provider: &MonoizeProvider) -> u64 {
        provider
            .request_timeout_ms_override
            .unwrap_or(self.request_timeout_ms)
            .max(1)
    }

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

#[derive(Debug, Clone)]
pub struct ChannelHealthState {
    pub healthy: bool,
    pub last_success_at: Option<i64>,
    pub probe_success_count: u32,
    pub last_probe_at: Option<i64>,
    pub passive_failure_timestamps: VecDeque<i64>,
    pub trip_id: Option<u64>,
    pub trip_time: Option<i64>,
    pub trip_rate_limited: bool,
    pub trip_retry_after_seconds: Option<u16>,
    pub recovery_mode: Option<RecoveryMode>,
    pub next_recovery_at: Option<i64>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RecoveryMode {
    Probe,
    Timed,
}

pub fn recovery_deadline(now: i64, wait_seconds: u64) -> i64 {
    now.saturating_add(i64::try_from(wait_seconds).unwrap_or(i64::MAX))
}

pub fn recovery_wait(
    ordinary_wait: u64,
    rate_limit_wait: Option<u64>,
    rate_limited: bool,
    retry_after_seconds: Option<u16>,
) -> u64 {
    match rate_limit_wait {
        Some(wait) if rate_limited => wait.max(u64::from(retry_after_seconds.unwrap_or(0))),
        _ => ordinary_wait,
    }
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

impl Default for ChannelHealthState {
    fn default() -> Self {
        Self {
            healthy: true,
            last_success_at: None,
            probe_success_count: 0,
            last_probe_at: None,
            passive_failure_timestamps: VecDeque::new(),
            trip_id: None,
            trip_time: None,
            trip_rate_limited: false,
            trip_retry_after_seconds: None,
            recovery_mode: None,
            next_recovery_at: None,
        }
    }
}

impl ChannelHealthState {
    pub fn is_unhealthy(&self, now: i64) -> bool {
        !self.healthy
            && match self.recovery_mode {
                Some(RecoveryMode::Probe) => true,
                Some(RecoveryMode::Timed) => {
                    self.next_recovery_at.is_some_and(|deadline| now < deadline)
                }
                None => false,
            }
    }

    pub fn mark_success(&mut self, now: i64) {
        self.healthy = true;
        self.last_success_at = Some(now);
        self.probe_success_count = 0;
        self.last_probe_at = None;
        self.trip_id = None;
        self.trip_time = None;
        self.trip_rate_limited = false;
        self.trip_retry_after_seconds = None;
        self.recovery_mode = None;
        self.next_recovery_at = None;
    }

    pub fn trip(
        &mut self,
        now: i64,
        mode: RecoveryMode,
        rate_limited: bool,
        retry_after: Option<u16>,
        wait: u64,
    ) {
        static NEXT_TRIP_ID: AtomicU64 = AtomicU64::new(1);
        let trip_id = NEXT_TRIP_ID.fetch_add(1, Ordering::Relaxed);
        self.healthy = false;
        self.trip_id = Some(trip_id);
        self.trip_time = Some(now);
        self.trip_rate_limited = rate_limited;
        self.trip_retry_after_seconds = retry_after;
        self.recovery_mode = Some(mode);
        self.next_recovery_at = Some(recovery_deadline(now, wait));
        self.probe_success_count = 0;
        self.last_probe_at = None;
    }
}
