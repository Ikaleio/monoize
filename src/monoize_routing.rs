use crate::db::DbPool;
use crate::exact_decimal::Multiplier;
use crate::settings::default_reasoning_suffix_map;
use crate::transforms::{TransformRuleConfig, canonicalize_transform_rules};
use crate::users::canonicalize_group_ids;
use chrono::{DateTime, Utc};
use eventsource_stream::Eventsource;
use futures_util::StreamExt;
use sea_orm::{ConnectionTrait, QueryResult, Value as SeaValue};
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use std::collections::{BTreeMap, HashMap, HashSet, VecDeque};
use std::sync::OnceLock;
use std::time::Duration;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum MonoizeProviderType {
    Responses,
    ChatCompletion,
    Messages,
    Gemini,
    OpenaiImage,
    Replicate,
}

impl MonoizeProviderType {
    #[allow(clippy::should_implement_trait)]
    pub fn from_str(s: &str) -> Option<Self> {
        match s {
            "responses" => Some(Self::Responses),
            "chat_completion" => Some(Self::ChatCompletion),
            "messages" => Some(Self::Messages),
            "gemini" => Some(Self::Gemini),
            "openai_image" => Some(Self::OpenaiImage),
            "replicate" => Some(Self::Replicate),
            _ => None,
        }
    }

    pub fn as_str(&self) -> &'static str {
        match self {
            Self::Responses => "responses",
            Self::ChatCompletion => "chat_completion",
            Self::Messages => "messages",
            Self::Gemini => "gemini",
            Self::OpenaiImage => "openai_image",
            Self::Replicate => "replicate",
        }
    }

    pub fn to_config_type(&self) -> crate::config::ProviderType {
        match self {
            Self::Responses => crate::config::ProviderType::Responses,
            Self::ChatCompletion => crate::config::ProviderType::ChatCompletion,
            Self::Messages => crate::config::ProviderType::Messages,
            Self::Gemini => crate::config::ProviderType::Gemini,
            Self::OpenaiImage => crate::config::ProviderType::OpenaiImage,
            Self::Replicate => crate::config::ProviderType::Replicate,
        }
    }
}

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum AffinityFailbackMode {
    #[default]
    Sticky,
    PreferHigherPriority,
}

impl AffinityFailbackMode {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Sticky => "sticky",
            Self::PreferHigherPriority => "prefer_higher_priority",
        }
    }

    pub fn from_str(value: &str) -> Option<Self> {
        match value {
            "sticky" => Some(Self::Sticky),
            "prefer_higher_priority" => Some(Self::PreferHigherPriority),
            _ => None,
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ApiTypeOverride {
    pub pattern: String,
    pub api_type: MonoizeProviderType,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct MonoizeModelEntry {
    pub redirect: Option<String>,
    pub multiplier: Multiplier,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct MonoizeChannel {
    pub id: String,
    pub name: String,
    pub provider_type: MonoizeProviderType,
    pub base_url: String,
    #[serde(skip_serializing)]
    pub api_key: String,
    #[serde(default = "default_channel_weight")]
    pub weight: i32,
    #[serde(default = "default_enabled")]
    pub enabled: bool,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub passive_failure_count_threshold_override: Option<u32>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub passive_cooldown_seconds_override: Option<u64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub passive_window_seconds_override: Option<u64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub passive_rate_limit_cooldown_seconds_override: Option<u64>,
    #[serde(default)]
    pub models: HashMap<String, MonoizeModelEntry>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub active_probe_enabled_override: Option<bool>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub active_probe_interval_seconds_override: Option<u64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub active_probe_success_threshold_override: Option<u32>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub active_probe_model_override: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub affinity_enabled_override: Option<bool>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub affinity_idle_ttl_seconds_override: Option<u64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub affinity_failback_mode_override: Option<AffinityFailbackMode>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub affinity_failback_delay_seconds_override: Option<u64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub proxy_url: Option<String>,
    /// CP-INV-15: static headers injected into every upstream request for this Channel.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub extra_headers: Option<BTreeMap<String, String>>,
    /// CM-AFF-0: explicit override for URL-based automatic session affinity.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub session_affinity_auto: Option<bool>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub _healthy: Option<bool>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub _last_success_at: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub _health_status: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub _unhealthy_models: Option<Vec<String>>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub _probing_models: Option<Vec<String>>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub _cooldown_until: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct MonoizeProvider {
    pub id: String,
    pub name: String,
    pub channels: Vec<MonoizeChannel>,
    pub max_retries: i32,
    pub channel_max_retries: i32,
    pub channel_retry_interval_ms: i32,
    pub circuit_breaker_enabled: bool,
    pub per_model_circuit_break: bool,
    #[serde(default)]
    pub transforms: Vec<TransformRuleConfig>,
    #[serde(default)]
    pub api_type_overrides: Vec<ApiTypeOverride>,
    pub active_probe_enabled_override: Option<bool>,
    pub active_probe_interval_seconds_override: Option<u64>,
    pub active_probe_success_threshold_override: Option<u32>,
    pub active_probe_model_override: Option<String>,
    pub request_timeout_ms_override: Option<u64>,
    #[serde(default)]
    pub extra_fields_whitelist: Option<Vec<String>>,
    #[serde(default)]
    pub strip_cross_protocol_nested_extra: Option<bool>,
    #[serde(default)]
    pub group_ids: Vec<String>,
    /// MP-D11/MP-D12 (`model-pricing.spec.md`): NULL inherits the global setting.
    pub allow_free_when_unpriced_override: Option<bool>,
    pub allow_free_when_missing_usage_override: Option<bool>,
    pub enabled: bool,
    pub priority: i32,
    pub created_at: DateTime<Utc>,
    pub updated_at: DateTime<Utc>,
}

#[derive(Debug, Clone, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct CreateMonoizeChannelInput {
    pub id: Option<String>,
    pub name: String,
    pub provider_type: MonoizeProviderType,
    pub base_url: String,
    #[serde(default)]
    pub api_key: Option<String>,
    #[serde(default = "default_channel_weight")]
    pub weight: i32,
    #[serde(default = "default_enabled")]
    pub enabled: bool,
    #[serde(default)]
    pub passive_failure_count_threshold_override: Option<u32>,
    #[serde(default)]
    pub passive_cooldown_seconds_override: Option<u64>,
    #[serde(default)]
    pub passive_window_seconds_override: Option<u64>,
    #[serde(default)]
    pub passive_rate_limit_cooldown_seconds_override: Option<u64>,
    #[serde(default)]
    pub models: HashMap<String, MonoizeModelEntry>,
    pub active_probe_enabled_override: Option<bool>,
    pub active_probe_interval_seconds_override: Option<u64>,
    pub active_probe_success_threshold_override: Option<u32>,
    pub active_probe_model_override: Option<String>,
    #[serde(default)]
    pub affinity_enabled_override: Option<bool>,
    #[serde(default)]
    pub affinity_idle_ttl_seconds_override: Option<u64>,
    #[serde(default)]
    pub affinity_failback_mode_override: Option<AffinityFailbackMode>,
    #[serde(default)]
    pub affinity_failback_delay_seconds_override: Option<u64>,
    /// CP-INV-14: None/empty = follow-global; Some(url) = custom http(s) egress proxy.
    #[serde(default)]
    pub proxy_url: Option<String>,
    /// CP-INV-15: static upstream headers; None/empty map = none.
    #[serde(default)]
    pub extra_headers: Option<BTreeMap<String, String>>,
    /// CM-AFF-2: enable derived per-request session affinity.
    #[serde(default)]
    pub session_affinity_auto: Option<bool>,
}

#[derive(Debug, Clone, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct CreateMonoizeProviderInput {
    pub name: String,
    pub channels: Vec<CreateMonoizeChannelInput>,
    #[serde(default = "default_max_retries")]
    pub max_retries: i32,
    #[serde(default)]
    pub channel_max_retries: i32,
    #[serde(default)]
    pub channel_retry_interval_ms: i32,
    #[serde(default = "default_enabled")]
    pub circuit_breaker_enabled: bool,
    #[serde(default)]
    pub per_model_circuit_break: bool,
    #[serde(default)]
    pub transforms: Vec<TransformRuleConfig>,
    pub active_probe_enabled_override: Option<bool>,
    #[serde(default)]
    pub api_type_overrides: Vec<ApiTypeOverride>,
    pub active_probe_interval_seconds_override: Option<u64>,
    pub active_probe_success_threshold_override: Option<u32>,
    pub active_probe_model_override: Option<String>,
    pub request_timeout_ms_override: Option<u64>,
    #[serde(default)]
    pub extra_fields_whitelist: Option<Vec<String>>,
    #[serde(default)]
    pub strip_cross_protocol_nested_extra: Option<bool>,
    #[serde(default)]
    pub group_ids: Vec<String>,
    #[serde(default)]
    pub allow_free_when_unpriced_override: Option<bool>,
    #[serde(default)]
    pub allow_free_when_missing_usage_override: Option<bool>,
    #[serde(default = "default_enabled")]
    pub enabled: bool,
    pub priority: Option<i32>,
}

#[derive(Debug, Clone, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct UpdateMonoizeProviderInput {
    pub name: Option<String>,
    pub channels: Option<Vec<CreateMonoizeChannelInput>>,
    pub max_retries: Option<i32>,
    pub channel_max_retries: Option<i32>,
    pub channel_retry_interval_ms: Option<i32>,
    pub circuit_breaker_enabled: Option<bool>,
    pub per_model_circuit_break: Option<bool>,
    pub transforms: Option<Vec<TransformRuleConfig>>,
    pub active_probe_enabled_override: Option<Option<bool>>,
    pub api_type_overrides: Option<Vec<ApiTypeOverride>>,
    pub active_probe_interval_seconds_override: Option<Option<u64>>,
    pub active_probe_success_threshold_override: Option<Option<u32>>,
    pub active_probe_model_override: Option<Option<String>>,
    pub request_timeout_ms_override: Option<Option<u64>>,
    pub extra_fields_whitelist: Option<Option<Vec<String>>>,
    pub strip_cross_protocol_nested_extra: Option<Option<bool>>,
    pub group_ids: Option<Vec<String>>,
    pub allow_free_when_unpriced_override: Option<Option<bool>>,
    pub allow_free_when_missing_usage_override: Option<Option<bool>>,
    pub enabled: Option<bool>,
    pub priority: Option<i32>,
}

#[derive(Debug, Clone, Deserialize)]
pub struct ReorderProvidersInput {
    pub provider_ids: Vec<String>,
}

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
const TRANSFORM_MIGRATION_BATCH_SIZE: usize = 199;
const TRANSFORM_MIGRATION_MARKER: &str = "migration.provider_transform_rule_ids.v2";
const OBSOLETE_TRANSFORM_MIGRATION_MARKER: &str = "migration.provider_transform_rule_ids.v1";

fn parse_positive_entry_limit(raw: Option<&str>, default: usize) -> usize {
    raw.and_then(|value| value.trim().parse::<usize>().ok())
        .filter(|value| *value > 0)
        .unwrap_or(default)
}

fn parse_provider_reorder_limit(raw: Option<&str>) -> usize {
    parse_positive_entry_limit(raw, DEFAULT_PROVIDER_REORDER_MAX_IDS)
        .min(DEFAULT_PROVIDER_REORDER_MAX_IDS)
}

fn provider_reorder_max_ids() -> usize {
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
        if let Some(until) = self.cooldown_until {
            if now_ts < until {
                return "unhealthy";
            }
        }
        "probing"
    }
}

#[derive(Clone)]
pub struct MonoizeRoutingStore {
    db: DbPool,
}

fn default_enabled() -> bool {
    true
}

fn default_max_retries() -> i32 {
    -1
}

fn default_channel_weight() -> i32 {
    1
}

fn decode_provider_group_ids_json(
    provider_id: &str,
    raw: Option<String>,
) -> Result<Vec<String>, String> {
    let Some(raw) = raw else {
        return Ok(Vec::new());
    };
    if raw.trim().is_empty() {
        return Ok(Vec::new());
    }
    let group_ids = serde_json::from_str::<Vec<String>>(&raw)
        .map_err(|error| format!("provider {provider_id} invalid group_ids JSON: {error}"))?;
    Ok(canonicalize_group_ids(&group_ids))
}

fn serialize_provider_group_ids_json(group_ids: &[String]) -> Result<String, String> {
    serde_json::to_string(&canonicalize_group_ids(group_ids)).map_err(|e| e.to_string())
}

fn decode_database_bool(
    entity: &str,
    entity_id: &str,
    field: &str,
    value: i32,
) -> Result<bool, String> {
    match value {
        0 => Ok(false),
        1 => Ok(true),
        _ => Err(format!(
            "{entity} {entity_id} invalid {field} boolean: expected 0 or 1, got {value}"
        )),
    }
}

fn generate_short_id() -> String {
    const CHARSET: &[u8] = b"abcdefghijklmnopqrstuvwxyz0123456789";
    let bytes = uuid::Uuid::new_v4().into_bytes();
    (0..8)
        .map(|i| CHARSET[bytes[i] as usize % CHARSET.len()] as char)
        .collect()
}

/// CP-INV-14: trim and treat empty as NULL (follow-global).
fn normalized_proxy_url(raw: Option<&str>) -> Option<String> {
    raw.map(str::trim)
        .filter(|value| !value.is_empty())
        .map(str::to_string)
}

/// CP-INV-15: reserved header names that must not be overridden by Channel extras.
const EXTRA_HEADERS_RESERVED: &[&str] = &[
    "authorization",
    "host",
    "content-length",
    "content-type",
    "transfer-encoding",
    "connection",
    "keep-alive",
    "upgrade",
    "expect",
    "te",
    "trailer",
];

const EXTRA_HEADERS_MAX_ENTRIES: usize = 16;
const EXTRA_HEADERS_MAX_KEY_LEN: usize = 128;
const EXTRA_HEADERS_MAX_VALUE_LEN: usize = 4096;

fn validate_channel_extra_headers(
    channel_name: &str,
    headers: &BTreeMap<String, String>,
) -> Result<(), String> {
    if headers.len() > EXTRA_HEADERS_MAX_ENTRIES {
        return Err(format!(
            "channel '{channel_name}' extra_headers must contain at most {EXTRA_HEADERS_MAX_ENTRIES} entries"
        ));
    }
    let mut seen_lower: HashSet<String> = HashSet::new();
    for (key, value) in headers {
        let trimmed = key.trim();
        if trimmed.is_empty() {
            return Err(format!(
                "channel '{channel_name}' extra_headers key must not be empty"
            ));
        }
        if trimmed.len() > EXTRA_HEADERS_MAX_KEY_LEN {
            return Err(format!(
                "channel '{channel_name}' extra_headers key exceeds {EXTRA_HEADERS_MAX_KEY_LEN} characters"
            ));
        }
        let valid_token = trimmed.bytes().all(|byte| {
            matches!(byte,
                b'!' | b'#' | b'$' | b'%' | b'&' | b'\'' | b'*' | b'+' | b'-' | b'.'
                | b'^' | b'_' | b'`' | b'|' | b'~'
                | b'0'..=b'9' | b'a'..=b'z' | b'A'..=b'Z')
        });
        if !valid_token {
            return Err(format!(
                "channel '{channel_name}' extra_headers key '{trimmed}' contains invalid characters"
            ));
        }
        let lower = trimmed.to_ascii_lowercase();
        // Case-insensitive duplicate keys would make the effective value ambiguous.
        if !seen_lower.insert(lower.clone()) {
            return Err(format!(
                "channel '{channel_name}' extra_headers contains duplicate key '{trimmed}'"
            ));
        }
        if EXTRA_HEADERS_RESERVED.contains(&lower.as_str()) {
            return Err(format!(
                "channel '{channel_name}' extra_headers key '{trimmed}' is reserved and must not be set"
            ));
        }
        if value.len() > EXTRA_HEADERS_MAX_VALUE_LEN {
            return Err(format!(
                "channel '{channel_name}' extra_headers value for '{trimmed}' exceeds {EXTRA_HEADERS_MAX_VALUE_LEN} characters"
            ));
        }
        if value.contains('\r') || value.contains('\n') {
            return Err(format!(
                "channel '{channel_name}' extra_headers value for '{trimmed}' must not contain CR or LF"
            ));
        }
    }
    Ok(())
}

/// CP-INV-15a: trim keys, drop nothing else, canonical JSON with sorted keys;
/// an empty map persists as NULL.
fn normalized_extra_headers_json(raw: Option<&BTreeMap<String, String>>) -> Option<String> {
    let headers = raw?;
    let mut trimmed: BTreeMap<&str, &String> = BTreeMap::new();
    for (key, value) in headers {
        trimmed.insert(key.trim(), value);
    }
    if trimmed.is_empty() {
        return None;
    }
    serde_json::to_string(&trimmed).ok()
}

fn decode_extra_headers(raw: Option<String>) -> Result<Option<BTreeMap<String, String>>, String> {
    let Some(text) = raw.filter(|value| !value.trim().is_empty()) else {
        return Ok(None);
    };
    serde_json::from_str(&text)
        .map(Some)
        .map_err(|e| format!("invalid stored extra_headers JSON: {e}"))
}

fn decode_channel_model_row(row: &QueryResult, model: &str) -> Result<MonoizeChannel, String> {
    let id: String = row.try_get("", "id").map_err(|e| e.to_string())?;
    let multiplier = row
        .try_get::<String>("", "multiplier")
        .map_err(|e| e.to_string())?
        .parse()
        .map_err(|e: String| format!("channel {id} invalid multiplier: {e}"))?;
    let models = HashMap::from([(
        model.to_string(),
        MonoizeModelEntry {
            redirect: row.try_get("", "redirect").map_err(|e| e.to_string())?,
            multiplier,
        },
    )]);
    decode_channel_row(row, models)
}

fn decode_channel_row(
    row: &QueryResult,
    models: HashMap<String, MonoizeModelEntry>,
) -> Result<MonoizeChannel, String> {
    let id: String = row.try_get("", "id").map_err(|e| e.to_string())?;
    let provider_type_raw: String = row
        .try_get("", "provider_type")
        .map_err(|e| format!("channel {id} missing provider_type: {e}"))?;
    let provider_type = MonoizeProviderType::from_str(&provider_type_raw)
        .ok_or_else(|| format!("channel {id} invalid provider type: {provider_type_raw}"))?;
    Ok(MonoizeChannel {
        id: id.clone(),
        name: row.try_get("", "name").map_err(|e| e.to_string())?,
        provider_type,
        base_url: row.try_get("", "base_url").map_err(|e| e.to_string())?,
        api_key: row.try_get("", "api_key").map_err(|e| e.to_string())?,
        weight: row.try_get("", "weight").map_err(|e| e.to_string())?,
        enabled: decode_database_bool(
            "channel",
            &id,
            "enabled",
            row.try_get::<i32>("", "enabled")
                .map_err(|e| e.to_string())?,
        )?,
        passive_failure_count_threshold_override: row
            .try_get::<Option<i32>>("", "passive_failure_count_threshold_override")
            .map_err(|e| e.to_string())?
            .map(|value| {
                decode_positive_u32(
                    &id,
                    "passive_failure_count_threshold_override",
                    i64::from(value),
                )
            })
            .transpose()?,
        passive_cooldown_seconds_override: row
            .try_get::<Option<i32>>("", "passive_cooldown_seconds_override")
            .map_err(|e| e.to_string())?
            .map(|value| {
                decode_positive_u64(&id, "passive_cooldown_seconds_override", i64::from(value))
            })
            .transpose()?,
        passive_window_seconds_override: row
            .try_get::<Option<i32>>("", "passive_window_seconds_override")
            .map_err(|e| e.to_string())?
            .map(|value| {
                decode_positive_u64(&id, "passive_window_seconds_override", i64::from(value))
            })
            .transpose()?,
        passive_rate_limit_cooldown_seconds_override: row
            .try_get::<Option<i32>>("", "passive_rate_limit_cooldown_seconds_override")
            .map_err(|e| e.to_string())?
            .map(|value| {
                decode_positive_u64(
                    &id,
                    "passive_rate_limit_cooldown_seconds_override",
                    i64::from(value),
                )
            })
            .transpose()?,
        models,
        active_probe_enabled_override: row
            .try_get::<Option<i32>>("", "active_probe_enabled_override")
            .map_err(|e| e.to_string())?
            .map(|value| {
                decode_database_bool("channel", &id, "active_probe_enabled_override", value)
            })
            .transpose()?,
        active_probe_interval_seconds_override: row
            .try_get::<Option<i32>>("", "active_probe_interval_seconds_override")
            .map_err(|e| e.to_string())?
            .map(|value| {
                decode_positive_u64(
                    &id,
                    "active_probe_interval_seconds_override",
                    i64::from(value),
                )
            })
            .transpose()?,
        active_probe_success_threshold_override: row
            .try_get::<Option<i32>>("", "active_probe_success_threshold_override")
            .map_err(|e| e.to_string())?
            .map(|value| {
                decode_positive_u32(
                    &id,
                    "active_probe_success_threshold_override",
                    i64::from(value),
                )
            })
            .transpose()?,
        active_probe_model_override: row
            .try_get("", "active_probe_model_override")
            .map_err(|e| e.to_string())?,
        affinity_enabled_override: row
            .try_get::<Option<i32>>("", "affinity_enabled_override")
            .map_err(|e| e.to_string())?
            .map(|value| decode_database_bool("channel", &id, "affinity_enabled_override", value))
            .transpose()?,
        affinity_idle_ttl_seconds_override: row
            .try_get::<Option<i32>>("", "affinity_idle_ttl_seconds_override")
            .map_err(|e| e.to_string())?
            .map(|value| {
                decode_positive_u64(&id, "affinity_idle_ttl_seconds_override", i64::from(value))
            })
            .transpose()?,
        affinity_failback_mode_override: row
            .try_get::<Option<String>>("", "affinity_failback_mode_override")
            .map_err(|e| e.to_string())?
            .map(|value| {
                AffinityFailbackMode::from_str(&value).ok_or_else(|| {
                    format!("channel {id} invalid affinity_failback_mode_override: {value}")
                })
            })
            .transpose()?,
        affinity_failback_delay_seconds_override: row
            .try_get::<Option<i32>>("", "affinity_failback_delay_seconds_override")
            .map_err(|e| e.to_string())?
            .map(|value| {
                decode_nonnegative_u64(
                    &id,
                    "affinity_failback_delay_seconds_override",
                    i64::from(value),
                )
            })
            .transpose()?,
        proxy_url: row
            .try_get::<Option<String>>("", "proxy_url")
            .map_err(|e| e.to_string())?
            .filter(|value| !value.trim().is_empty()),
        extra_headers: decode_extra_headers(
            row.try_get::<Option<String>>("", "extra_headers")
                .map_err(|e| e.to_string())?,
        )?,
        session_affinity_auto: row
            .try_get::<Option<i32>>("", "session_affinity_auto")
            .map_err(|e| e.to_string())?
            .map(|value| decode_database_bool("channel", &id, "session_affinity_auto", value))
            .transpose()?,
        _healthy: None,
        _last_success_at: None,
        _health_status: None,
        _unhealthy_models: None,
        _probing_models: None,
        _cooldown_until: None,
    })
}

fn decode_provider_row(
    row: &QueryResult,
    channels: Vec<MonoizeChannel>,
) -> Result<MonoizeProvider, String> {
    let id: String = row.try_get("", "id").map_err(|e| e.to_string())?;
    let mut transforms: Vec<TransformRuleConfig> = serde_json::from_str(
        &row.try_get::<String>("", "transforms")
            .map_err(|e| format!("provider {id} missing transforms column: {e}"))?,
    )
    .map_err(|e| format!("provider {id} invalid transforms JSON: {e}"))?;
    canonicalize_transform_rules(&mut transforms);
    let api_type_overrides: Vec<ApiTypeOverride> = serde_json::from_str(
        &row.try_get::<String>("", "api_type_overrides")
            .map_err(|e| format!("provider {id} missing api_type_overrides column: {e}"))?,
    )
    .map_err(|e| format!("provider {id} invalid api_type_overrides JSON: {e}"))?;
    let created_at = DateTime::parse_from_rfc3339(
        &row.try_get::<String>("", "created_at")
            .map_err(|e| e.to_string())?,
    )
    .map_err(|e| format!("provider {id} invalid created_at RFC3339: {e}"))?
    .with_timezone(&Utc);
    let updated_at = DateTime::parse_from_rfc3339(
        &row.try_get::<String>("", "updated_at")
            .map_err(|e| e.to_string())?,
    )
    .map_err(|e| format!("provider {id} invalid updated_at RFC3339: {e}"))?
    .with_timezone(&Utc);
    Ok(MonoizeProvider {
        id: id.clone(),
        name: row.try_get("", "name").map_err(|e| e.to_string())?,
        channels,
        max_retries: row.try_get("", "max_retries").map_err(|e| e.to_string())?,
        channel_max_retries: row
            .try_get("", "channel_max_retries")
            .map_err(|e| e.to_string())?,
        channel_retry_interval_ms: row
            .try_get("", "channel_retry_interval_ms")
            .map_err(|e| e.to_string())?,
        circuit_breaker_enabled: decode_database_bool(
            "provider",
            &id,
            "circuit_breaker_enabled",
            row.try_get::<i32>("", "circuit_breaker_enabled")
                .map_err(|e| e.to_string())?,
        )?,
        per_model_circuit_break: decode_database_bool(
            "provider",
            &id,
            "per_model_circuit_break",
            row.try_get::<i32>("", "per_model_circuit_break")
                .map_err(|e| e.to_string())?,
        )?,
        transforms,
        api_type_overrides,
        active_probe_enabled_override: row
            .try_get::<Option<i32>>("", "active_probe_enabled_override")
            .map_err(|e| e.to_string())?
            .map(|value| {
                decode_database_bool("provider", &id, "active_probe_enabled_override", value)
            })
            .transpose()?,
        active_probe_interval_seconds_override: row
            .try_get::<Option<i32>>("", "active_probe_interval_seconds_override")
            .map_err(|e| e.to_string())?
            .map(|value| {
                decode_positive_u64(
                    &id,
                    "active_probe_interval_seconds_override",
                    i64::from(value),
                )
            })
            .transpose()?,
        active_probe_success_threshold_override: row
            .try_get::<Option<i32>>("", "active_probe_success_threshold_override")
            .map_err(|e| e.to_string())?
            .map(|value| {
                decode_positive_u32(
                    &id,
                    "active_probe_success_threshold_override",
                    i64::from(value),
                )
            })
            .transpose()?,
        active_probe_model_override: row
            .try_get("", "active_probe_model_override")
            .map_err(|e| e.to_string())?,
        request_timeout_ms_override: row
            .try_get::<Option<i32>>("", "request_timeout_ms_override")
            .map_err(|e| e.to_string())?
            .map(|value| decode_positive_u64(&id, "request_timeout_ms_override", i64::from(value)))
            .transpose()?,
        extra_fields_whitelist: row
            .try_get::<Option<String>>("", "extra_fields_whitelist")
            .map_err(|e| format!("provider {id} invalid extra_fields_whitelist column: {e}"))?
            .map(|raw| {
                serde_json::from_str::<Vec<String>>(&raw)
                    .map_err(|e| format!("provider {id} invalid extra_fields_whitelist JSON: {e}"))
            })
            .transpose()?,
        strip_cross_protocol_nested_extra: row
            .try_get::<Option<i32>>("", "strip_cross_protocol_nested_extra")
            .map_err(|e| {
                format!("provider {id} invalid strip_cross_protocol_nested_extra column: {e}")
            })?
            .map(|value| {
                decode_database_bool("provider", &id, "strip_cross_protocol_nested_extra", value)
            })
            .transpose()?,
        group_ids: decode_provider_group_ids_json(
            &id,
            row.try_get::<Option<String>>("", "group_ids")
                .map_err(|e| format!("provider {id} invalid group_ids column: {e}"))?,
        )?,
        allow_free_when_unpriced_override: row
            .try_get::<Option<i32>>("", "allow_free_when_unpriced_override")
            .map_err(|e| {
                format!("provider {id} invalid allow_free_when_unpriced_override column: {e}")
            })?
            .map(|value| {
                decode_database_bool("provider", &id, "allow_free_when_unpriced_override", value)
            })
            .transpose()?,
        allow_free_when_missing_usage_override: row
            .try_get::<Option<i32>>("", "allow_free_when_missing_usage_override")
            .map_err(|e| {
                format!("provider {id} invalid allow_free_when_missing_usage_override column: {e}")
            })?
            .map(|value| {
                decode_database_bool(
                    "provider",
                    &id,
                    "allow_free_when_missing_usage_override",
                    value,
                )
            })
            .transpose()?,
        enabled: decode_database_bool(
            "provider",
            &id,
            "enabled",
            row.try_get::<i32>("", "enabled")
                .map_err(|e| e.to_string())?,
        )?,
        priority: row.try_get("", "priority").map_err(|e| e.to_string())?,
        created_at,
        updated_at,
    })
}

impl MonoizeRoutingStore {
    pub async fn new(db: DbPool) -> Result<Self, String> {
        let store = Self { db };
        store.migrate_transform_rule_ids().await?;
        Ok(store)
    }

    /// Replica-side constructor per PRP11: skips canonicalization writes that the
    /// primary already performed on the shared database.
    pub async fn new_read_only(db: DbPool) -> Result<Self, String> {
        Ok(Self { db })
    }

    async fn migrate_transform_rule_ids(&self) -> Result<(), String> {
        let marker = self
            .db
            .read()
            .query_one(self.db.stmt(
                "SELECT value FROM system_settings WHERE key = $1",
                vec![TRANSFORM_MIGRATION_MARKER.into()],
            ))
            .await
            .map_err(|e| e.to_string())?;
        let marker_value = marker
            .map(|row| {
                row.try_get::<String>("", "value")
                    .map_err(|e| e.to_string())
            })
            .transpose()?;
        if marker_value.as_deref() == Some("complete") {
            return Ok(());
        }

        let mut last_id: Option<String> = None;
        loop {
            let tx = self.db.begin_write().await.map_err(|e| e.to_string())?;
            let (sql, values) = match last_id.as_deref() {
                Some(last_id) => (
                    format!(
                        "SELECT id, transforms FROM monoize_providers
                         WHERE id > $1 ORDER BY id ASC LIMIT {TRANSFORM_MIGRATION_BATCH_SIZE}"
                    ),
                    vec![last_id.into()],
                ),
                None => (
                    format!(
                        "SELECT id, transforms FROM monoize_providers
                         ORDER BY id ASC LIMIT {TRANSFORM_MIGRATION_BATCH_SIZE}"
                    ),
                    vec![],
                ),
            };
            let rows = tx
                .query_all(self.db.stmt(&sql, values))
                .await
                .map_err(|e| e.to_string())?;
            if rows.is_empty() {
                tx.commit().await.map_err(|e| e.to_string())?;
                break;
            }
            let batch_len = rows.len();
            let next_last_id: String = rows
                .last()
                .expect("non-empty transform migration batch")
                .try_get("", "id")
                .map_err(|e| e.to_string())?;
            let mut updates = Vec::with_capacity(batch_len);
            for row in rows {
                let id: String = row.try_get("", "id").map_err(|e| e.to_string())?;
                let raw: String = row.try_get("", "transforms").map_err(|e| e.to_string())?;
                let Ok(mut transforms) = serde_json::from_str::<Vec<TransformRuleConfig>>(&raw)
                else {
                    tracing::warn!(provider_id = %id, "skip invalid provider transforms during transform id migration");
                    continue;
                };
                if !canonicalize_transform_rules(&mut transforms) {
                    continue;
                }
                let encoded = serde_json::to_string(&transforms).map_err(|e| e.to_string())?;
                updates.push((id, encoded));
            }

            if !updates.is_empty() {
                let mut values: Vec<sea_orm::Value> = Vec::with_capacity(updates.len() * 2);
                let mut cases = Vec::with_capacity(updates.len());
                let mut ids = Vec::with_capacity(updates.len());
                for (id, transforms) in &updates {
                    let id_index = values.len() + 1;
                    values.push(id.clone().into());
                    ids.push(format!("${id_index}"));
                    let transforms_index = values.len() + 1;
                    values.push(transforms.clone().into());
                    cases.push(format!("WHEN ${id_index} THEN ${transforms_index}"));
                }
                tx.execute(self.db.stmt(
                    &format!(
                        "UPDATE monoize_providers
                         SET transforms = CASE id {} ELSE transforms END
                         WHERE id IN ({})",
                        cases.join(" "),
                        ids.join(", ")
                    ),
                    values,
                ))
                .await
                .map_err(|e| e.to_string())?;
            }
            tx.commit().await.map_err(|e| e.to_string())?;
            last_id = Some(next_last_id);
            if batch_len < TRANSFORM_MIGRATION_BATCH_SIZE {
                break;
            }
        }

        let tx = self.db.begin_write().await.map_err(|e| e.to_string())?;
        tx.execute(self.db.stmt(
            "INSERT INTO system_settings (key, value, updated_at) VALUES ($1, $2, $3)
             ON CONFLICT(key) DO UPDATE SET value = excluded.value, updated_at = excluded.updated_at",
            vec![
                TRANSFORM_MIGRATION_MARKER.into(),
                "complete".into(),
                Utc::now().to_rfc3339().into(),
            ],
        ))
        .await
        .map_err(|e| e.to_string())?;
        tx.execute(self.db.stmt(
            "DELETE FROM system_settings WHERE key = $1",
            vec![OBSOLETE_TRANSFORM_MIGRATION_MARKER.into()],
        ))
        .await
        .map_err(|e| e.to_string())?;
        tx.commit().await.map_err(|e| e.to_string())
    }

    pub async fn provider_count(&self) -> Result<i64, String> {
        let row = self
            .db
            .read()
            .query_one(
                self.db
                    .stmt("SELECT COUNT(*) as cnt FROM monoize_providers", vec![]),
            )
            .await
            .map_err(|e| e.to_string())?
            .ok_or_else(|| "count query returned no rows".to_string())?;
        row.try_get("", "cnt").map_err(|e| e.to_string())
    }

    async fn load_channels_bulk(
        &self,
        provider_id: Option<&str>,
    ) -> Result<HashMap<String, Vec<MonoizeChannel>>, String> {
        let provider_filter = if provider_id.is_some() {
            " WHERE provider_id = $1"
        } else {
            ""
        };
        let values = provider_id.map(|id| vec![id.into()]).unwrap_or_default();
        let channel_rows = self
            .db
            .read()
            .query_all(self.db.stmt(
                &format!(
                    "SELECT id, provider_id, name, base_url, api_key, weight, enabled,
                            provider_type, passive_failure_count_threshold_override,
                            passive_cooldown_seconds_override, passive_window_seconds_override,
                            passive_rate_limit_cooldown_seconds_override,
                            active_probe_enabled_override, active_probe_interval_seconds_override,
                            active_probe_success_threshold_override, active_probe_model_override,
                            affinity_enabled_override, affinity_idle_ttl_seconds_override,
                            affinity_failback_mode_override, affinity_failback_delay_seconds_override,
                            proxy_url, extra_headers, session_affinity_auto
                     FROM monoize_channels{provider_filter}
                     ORDER BY created_at ASC"
                ),
                values,
            ))
            .await
            .map_err(|e| e.to_string())?;

        let (model_sql, model_values) = if let Some(provider_id) = provider_id {
            (
                "SELECT cm.channel_id, cm.model_name, cm.redirect, cm.multiplier
                 FROM monoize_channel_models cm
                 JOIN monoize_channels c ON c.id = cm.channel_id
                 WHERE c.provider_id = $1
                 ORDER BY cm.channel_id ASC, cm.model_name ASC",
                vec![provider_id.into()],
            )
        } else {
            (
                "SELECT channel_id, model_name, redirect, multiplier
                 FROM monoize_channel_models
                 ORDER BY channel_id ASC, model_name ASC",
                vec![],
            )
        };
        let model_rows = self
            .db
            .read()
            .query_all(self.db.stmt(model_sql, model_values))
            .await
            .map_err(|e| e.to_string())?;
        let mut models_by_channel: HashMap<String, HashMap<String, MonoizeModelEntry>> =
            HashMap::new();
        for row in model_rows {
            let channel_id: String = row.try_get("", "channel_id").map_err(|e| e.to_string())?;
            let model_name: String = row.try_get("", "model_name").map_err(|e| e.to_string())?;
            let multiplier = row
                .try_get::<String>("", "multiplier")
                .map_err(|e| e.to_string())?
                .parse()
                .map_err(|e: String| format!("channel {channel_id} invalid multiplier: {e}"))?;
            models_by_channel.entry(channel_id).or_default().insert(
                model_name,
                MonoizeModelEntry {
                    redirect: row.try_get("", "redirect").map_err(|e| e.to_string())?,
                    multiplier,
                },
            );
        }

        let mut channels_by_provider: HashMap<String, Vec<MonoizeChannel>> = HashMap::new();
        for row in channel_rows {
            let provider_id: String = row.try_get("", "provider_id").map_err(|e| e.to_string())?;
            let channel_id: String = row.try_get("", "id").map_err(|e| e.to_string())?;
            channels_by_provider
                .entry(provider_id)
                .or_default()
                .push(decode_channel_row(
                    &row,
                    models_by_channel.remove(&channel_id).unwrap_or_default(),
                )?);
        }
        Ok(channels_by_provider)
    }

    pub async fn list_providers(&self) -> Result<Vec<MonoizeProvider>, String> {
        let rows = self
            .db
            .read()
            .query_all(self.db.stmt(
                r#"SELECT id, name, max_retries, channel_max_retries,
                          channel_retry_interval_ms, circuit_breaker_enabled,
                          per_model_circuit_break, transforms, api_type_overrides,
                          active_probe_enabled_override, active_probe_interval_seconds_override,
                          active_probe_success_threshold_override, active_probe_model_override,
                          request_timeout_ms_override, extra_fields_whitelist,
                          strip_cross_protocol_nested_extra, group_ids,
                          allow_free_when_unpriced_override,
                          allow_free_when_missing_usage_override,
                          enabled, priority, created_at, updated_at
                   FROM monoize_providers
                   ORDER BY priority ASC, created_at ASC"#,
                vec![],
            ))
            .await
            .map_err(|e| e.to_string())?;

        let mut channels_by_provider = self.load_channels_bulk(None).await?;
        rows.iter()
            .map(|row| {
                let id: String = row.try_get("", "id").map_err(|e| e.to_string())?;
                decode_provider_row(row, channels_by_provider.remove(&id).unwrap_or_default())
            })
            .collect()
    }

    pub async fn available_model_names(
        &self,
        candidates: &[String],
    ) -> Result<HashSet<String>, String> {
        if candidates.is_empty() {
            return Ok(HashSet::new());
        }
        let candidates = candidates
            .iter()
            .cloned()
            .collect::<std::collections::BTreeSet<_>>()
            .into_iter()
            .collect::<Vec<_>>();
        let mut available = HashSet::new();
        const LOOKUP_CHUNK_SIZE: usize = 400;
        for chunk in candidates.chunks(LOOKUP_CHUNK_SIZE) {
            let placeholders = (0..chunk.len())
                .map(|index| format!("${}", index + 1))
                .collect::<Vec<_>>()
                .join(", ");
            let rows = self
                .db
                .read()
                .query_all(self.db.stmt(
                    &format!(
                        "SELECT DISTINCT cm.model_name
                         FROM monoize_channel_models cm
                         JOIN monoize_channels c ON c.id = cm.channel_id
                         JOIN monoize_providers p ON p.id = c.provider_id
                         WHERE p.enabled = 1
                           AND c.enabled = 1
                           AND c.weight > 0
                           AND cm.model_name IN ({placeholders})"
                    ),
                    chunk.iter().cloned().map(Into::into).collect(),
                ))
                .await
                .map_err(|e| e.to_string())?;
            for row in rows {
                available.insert(row.try_get("", "model_name").map_err(|e| e.to_string())?);
            }
        }
        Ok(available)
    }

    pub async fn list_available_model_names(&self) -> Result<Vec<String>, String> {
        let rows = self
            .db
            .read()
            .query_all(self.db.stmt(
                "SELECT DISTINCT cm.model_name
                 FROM monoize_channel_models cm
                 JOIN monoize_channels c ON c.id = cm.channel_id
                 JOIN monoize_providers p ON p.id = c.provider_id
                 WHERE p.enabled = 1 AND c.enabled = 1 AND c.weight > 0
                 ORDER BY cm.model_name ASC",
                vec![],
            ))
            .await
            .map_err(|e| e.to_string())?;
        rows.into_iter()
            .map(|row| row.try_get("", "model_name").map_err(|e| e.to_string()))
            .collect()
    }

    pub async fn list_providers_for_model(
        &self,
        model: &str,
    ) -> Result<Vec<MonoizeProvider>, String> {
        let provider_rows = self
            .db
            .read()
            .query_all(self.db.stmt(
                r#"SELECT DISTINCT p.id, p.name, p.max_retries, p.channel_max_retries,
                          p.channel_retry_interval_ms, p.circuit_breaker_enabled,
                          p.per_model_circuit_break, p.transforms, p.api_type_overrides,
                          p.active_probe_enabled_override, p.active_probe_interval_seconds_override,
                          p.active_probe_success_threshold_override, p.active_probe_model_override,
                          p.request_timeout_ms_override, p.extra_fields_whitelist,
                          p.strip_cross_protocol_nested_extra, p.group_ids,
                          p.allow_free_when_unpriced_override,
                          p.allow_free_when_missing_usage_override,
                          p.enabled, p.priority, p.created_at, p.updated_at
                   FROM monoize_providers p
                   JOIN monoize_channels c ON c.provider_id = p.id
                   JOIN monoize_channel_models cm ON cm.channel_id = c.id
                   WHERE cm.model_name = $1
                     AND p.enabled = 1
                     AND c.enabled = 1
                     AND c.weight > 0
                   ORDER BY p.priority ASC, p.created_at ASC"#,
                vec![model.into()],
            ))
            .await
            .map_err(|e| e.to_string())?;
        if provider_rows.is_empty() {
            return Ok(Vec::new());
        }

        let channel_rows = self
            .db
            .read()
            .query_all(self.db.stmt(
                r#"SELECT c.id, c.provider_id, c.name, c.base_url, c.api_key, c.weight, c.enabled,
                          c.provider_type, c.passive_failure_count_threshold_override,
                          c.passive_cooldown_seconds_override, c.passive_window_seconds_override,
                          c.passive_rate_limit_cooldown_seconds_override,
                          c.active_probe_enabled_override, c.active_probe_interval_seconds_override,
                          c.active_probe_success_threshold_override, c.active_probe_model_override,
                          c.affinity_enabled_override, c.affinity_idle_ttl_seconds_override,
                          c.affinity_failback_mode_override, c.affinity_failback_delay_seconds_override,
                          c.proxy_url,
                          c.extra_headers,
                          c.session_affinity_auto,
                          cm.redirect, cm.multiplier
                   FROM monoize_channels c
                   JOIN monoize_providers p ON p.id = c.provider_id
                   JOIN monoize_channel_models cm ON cm.channel_id = c.id
                   WHERE cm.model_name = $1
                     AND p.enabled = 1
                     AND c.enabled = 1
                     AND c.weight > 0
                   ORDER BY c.created_at ASC"#,
                vec![model.into()],
            ))
            .await
            .map_err(|e| e.to_string())?;

        let mut channels_by_provider: HashMap<String, Vec<MonoizeChannel>> = HashMap::new();
        for row in channel_rows {
            let provider_id: String = row.try_get("", "provider_id").map_err(|e| e.to_string())?;
            channels_by_provider
                .entry(provider_id)
                .or_default()
                .push(decode_channel_model_row(&row, model)?);
        }
        provider_rows
            .iter()
            .map(|row| {
                let id: String = row.try_get("", "id").map_err(|e| e.to_string())?;
                decode_provider_row(row, channels_by_provider.remove(&id).unwrap_or_default())
            })
            .collect()
    }

    pub async fn list_active_probe_candidates(&self) -> Result<Vec<MonoizeProvider>, String> {
        let provider_rows = self
            .db
            .read()
            .query_all(self.db.stmt(
                r#"SELECT id, name, max_retries, channel_max_retries,
                          channel_retry_interval_ms, circuit_breaker_enabled,
                          per_model_circuit_break, transforms, api_type_overrides,
                          active_probe_enabled_override, active_probe_interval_seconds_override,
                          active_probe_success_threshold_override, active_probe_model_override,
                          request_timeout_ms_override, extra_fields_whitelist,
                          strip_cross_protocol_nested_extra, group_ids,
                          allow_free_when_unpriced_override,
                          allow_free_when_missing_usage_override,
                          enabled, priority, created_at, updated_at
                   FROM monoize_providers
                   WHERE circuit_breaker_enabled = 1
                     AND enabled = 1
                     AND EXISTS (
                         SELECT 1
                         FROM monoize_channels c
                         WHERE c.provider_id = monoize_providers.id
                           AND c.enabled = 1
                           AND c.weight > 0
                     )
                   ORDER BY priority ASC, created_at ASC"#,
                vec![],
            ))
            .await
            .map_err(|e| e.to_string())?;
        if provider_rows.is_empty() {
            return Ok(Vec::new());
        }
        let channel_model_rows = self
            .db
            .read()
            .query_all(self.db.stmt(
                r#"SELECT c.id, c.provider_id, c.name, c.base_url, c.api_key, c.weight, c.enabled,
                          c.provider_type, c.passive_failure_count_threshold_override,
                          c.passive_cooldown_seconds_override, c.passive_window_seconds_override,
                          c.passive_rate_limit_cooldown_seconds_override,
                          c.active_probe_enabled_override, c.active_probe_interval_seconds_override,
                          c.active_probe_success_threshold_override, c.active_probe_model_override,
                          c.affinity_enabled_override, c.affinity_idle_ttl_seconds_override,
                          c.affinity_failback_mode_override, c.affinity_failback_delay_seconds_override,
                          c.proxy_url, c.extra_headers, c.session_affinity_auto,
                          cm.model_name, cm.redirect, cm.multiplier
                   FROM monoize_channels c
                   JOIN monoize_providers p ON p.id = c.provider_id
                   JOIN monoize_channel_models cm ON cm.channel_id = c.id
                   WHERE p.circuit_breaker_enabled = 1
                     AND p.enabled = 1
                     AND c.enabled = 1
                     AND c.weight > 0
                   ORDER BY c.created_at ASC, cm.model_name ASC"#,
                vec![],
            ))
            .await
            .map_err(|e| e.to_string())?;

        let mut channels_by_provider: HashMap<String, Vec<MonoizeChannel>> = HashMap::new();
        let mut channel_positions: HashMap<(String, String), usize> = HashMap::new();
        for row in channel_model_rows {
            let provider_id: String = row.try_get("", "provider_id").map_err(|e| e.to_string())?;
            let channel_id: String = row.try_get("", "id").map_err(|e| e.to_string())?;
            let model_name: String = row.try_get("", "model_name").map_err(|e| e.to_string())?;
            let channels = channels_by_provider.entry(provider_id.clone()).or_default();
            if let Some(index) = channel_positions.get(&(provider_id.clone(), channel_id.clone())) {
                let model_entry = decode_channel_model_row(&row, &model_name)?
                    .models
                    .remove(&model_name)
                    .expect("decoded channel model row must contain its model");
                channels[*index].models.insert(model_name, model_entry);
            } else {
                let index = channels.len();
                channels.push(decode_channel_model_row(&row, &model_name)?);
                channel_positions.insert((provider_id, channel_id), index);
            }
        }
        provider_rows
            .iter()
            .map(|row| {
                let id: String = row.try_get("", "id").map_err(|e| e.to_string())?;
                decode_provider_row(row, channels_by_provider.remove(&id).unwrap_or_default())
            })
            .collect()
    }

    /// GR-I2/GR-C3 for the provider group set: canonicalize, replace an empty
    /// selection with the default group id, and require every id to reference
    /// an existing registry row.
    async fn resolve_provider_group_ids(
        &self,
        group_ids: &[String],
    ) -> Result<Vec<String>, String> {
        let group_ids = canonicalize_group_ids(group_ids);
        if group_ids.len() > 32 {
            return Err("at most 32 groups can be selected".to_string());
        }
        if group_ids.is_empty() {
            let row = self
                .db
                .read()
                .query_one(self.db.stmt(
                    "SELECT id FROM monoize_groups WHERE is_default = 1 LIMIT 1",
                    vec![],
                ))
                .await
                .map_err(|e| e.to_string())?
                .ok_or_else(|| "default group row missing (GR-D2 violated)".to_string())?;
            let default_id: String = row.try_get("", "id").map_err(|e| e.to_string())?;
            return Ok(vec![default_id]);
        }
        for id in &group_ids {
            let row = self
                .db
                .read()
                .query_one(self.db.stmt(
                    "SELECT 1 AS one FROM monoize_groups WHERE id = $1",
                    vec![id.clone().into()],
                ))
                .await
                .map_err(|e| e.to_string())?;
            if row.is_none() {
                return Err(format!("unknown group id: {id}"));
            }
        }
        Ok(group_ids)
    }

    pub async fn get_provider(&self, id: &str) -> Result<Option<MonoizeProvider>, String> {
        let row = self
            .db
            .read()
            .query_one(self.db.stmt(
                r#"SELECT id, name, max_retries, channel_max_retries,
                          channel_retry_interval_ms, circuit_breaker_enabled,
                          per_model_circuit_break, transforms, api_type_overrides,
                          active_probe_enabled_override, active_probe_interval_seconds_override,
                          active_probe_success_threshold_override, active_probe_model_override,
                          request_timeout_ms_override, extra_fields_whitelist,
                          strip_cross_protocol_nested_extra, group_ids,
                          allow_free_when_unpriced_override,
                          allow_free_when_missing_usage_override,
                          enabled, priority, created_at, updated_at
                   FROM monoize_providers
                   WHERE id = $1"#,
                vec![id.into()],
            ))
            .await
            .map_err(|e| e.to_string())?;

        let Some(row) = row else {
            return Ok(None);
        };
        let mut channels_by_provider = self.load_channels_bulk(Some(id)).await?;
        Ok(Some(decode_provider_row(
            &row,
            channels_by_provider.remove(id).unwrap_or_default(),
        )?))
    }

    pub async fn create_provider(
        &self,
        input: CreateMonoizeProviderInput,
    ) -> Result<MonoizeProvider, String> {
        validate_provider_input(&input.name, &input.channels, &input.api_type_overrides)?;
        if let Some(v) = input.active_probe_interval_seconds_override {
            if !(1..=i32::MAX as u64).contains(&v) {
                return Err(
                    "active_probe_interval_seconds_override must be between 1 and 2147483647"
                        .to_string(),
                );
            }
        }
        if let Some(v) = input.active_probe_success_threshold_override {
            if !(1..=i32::MAX as u32).contains(&v) {
                return Err(
                    "active_probe_success_threshold_override must be between 1 and 2147483647"
                        .to_string(),
                );
            }
        }
        if let Some(v) = input.request_timeout_ms_override {
            if !(1..=i32::MAX as u64).contains(&v) {
                return Err(
                    "request_timeout_ms_override must be between 1 and 2147483647".to_string(),
                );
            }
        }
        if input.channel_retry_interval_ms < 0 {
            return Err("channel_retry_interval_ms must be >= 0".to_string());
        }

        let id = generate_short_id();
        let now = Utc::now();
        // Resolve before begin_write: the registry lookup uses the read pool,
        // which on single-connection SQLite would deadlock behind our own
        // write transaction.
        let group_ids_json = serialize_provider_group_ids_json(
            &self.resolve_provider_group_ids(&input.group_ids).await?,
        )?;
        let txn = self.db.begin_write().await.map_err(|e| e.to_string())?;

        let priority = match input.priority {
            Some(v) => v,
            None => {
                if self.db.is_postgres() {
                    txn.execute_unprepared(
                        "LOCK TABLE monoize_providers IN SHARE ROW EXCLUSIVE MODE",
                    )
                    .await
                    .map_err(|e| e.to_string())?;
                }
                let row = txn
                    .query_one(self.db.stmt(
                        "SELECT CAST(MAX(priority) AS BIGINT) AS max_p FROM monoize_providers",
                        vec![],
                    ))
                    .await
                    .map_err(|e| e.to_string())?;
                let max_priority = row
                    .map(|row| {
                        row.try_get::<Option<i64>>("", "max_p")
                            .map_err(|e| e.to_string())
                    })
                    .transpose()?
                    .flatten();
                let next_priority = max_priority
                    .unwrap_or(-1)
                    .checked_add(1)
                    .ok_or_else(|| "provider priority overflow".to_string())?;
                i32::try_from(next_priority)
                    .map_err(|_| "provider priority exceeds signed 32-bit range".to_string())?
            }
        };

        let mut transforms = input.transforms.clone();
        canonicalize_transform_rules(&mut transforms);
        let transforms_json = serde_json::to_string(&transforms).map_err(|e| e.to_string())?;
        let api_type_overrides_json =
            serde_json::to_string(&input.api_type_overrides).map_err(|e| e.to_string())?;
        let extra_fields_whitelist_json: Option<String> = input
            .extra_fields_whitelist
            .as_ref()
            .map(|v| serde_json::to_string(v).unwrap_or_else(|_| "[]".to_string()));
        let strip_cross_proto = input.strip_cross_protocol_nested_extra;

        txn.execute(self.db.stmt(
                r#"INSERT INTO monoize_providers (
                        id, name, max_retries, channel_max_retries,
                        channel_retry_interval_ms, circuit_breaker_enabled,
                        per_model_circuit_break, transforms, api_type_overrides,
                        active_probe_enabled_override, active_probe_interval_seconds_override,
                        active_probe_success_threshold_override, active_probe_model_override,
                        request_timeout_ms_override, extra_fields_whitelist,
                        strip_cross_protocol_nested_extra, group_ids,
                        allow_free_when_unpriced_override,
                        allow_free_when_missing_usage_override,
                        enabled, priority, created_at, updated_at
                   ) VALUES ($1, $2, $3, $4, $5, $6, $7, $8, $9, $10, $11, $12, $13, $14, $15, $16, $17, $18, $19, $20, $21, $22, $23)"#,
                vec![
                        id.clone().into(),
                        input.name.clone().into(),
                        SeaValue::Int(Some(input.max_retries)),
                        SeaValue::Int(Some(input.channel_max_retries)),
                        SeaValue::Int(Some(input.channel_retry_interval_ms)),
                        SeaValue::Int(Some(if input.circuit_breaker_enabled { 1 } else { 0 })),
                        SeaValue::Int(Some(if input.per_model_circuit_break { 1 } else { 0 })),
                        transforms_json.into(),
                        api_type_overrides_json.into(),
                        opt_bool_to_value(input.active_probe_enabled_override),
                        opt_u64_to_value(input.active_probe_interval_seconds_override),
                        opt_u64_to_value(
                            input
                                .active_probe_success_threshold_override
                                .map(|v| v as u64),
                        ),
                        input.active_probe_model_override.clone().into(),
                        opt_u64_to_value(input.request_timeout_ms_override),
                        extra_fields_whitelist_json.into(),
                        opt_bool_to_value(strip_cross_proto),
                        group_ids_json.into(),
                        opt_bool_to_value(input.allow_free_when_unpriced_override),
                        opt_bool_to_value(input.allow_free_when_missing_usage_override),
                        SeaValue::Int(Some(if input.enabled { 1 } else { 0 })),
                        SeaValue::Int(Some(priority)),
                        now.to_rfc3339().into(),
                        now.to_rfc3339().into(),
                    ],
            ))
            .await
            .map_err(|e| e.to_string())?;

        self.replace_channels_on(&*txn, &id, &input.channels)
            .await?;
        txn.commit().await.map_err(|e| e.to_string())?;

        self.get_provider(&id)
            .await?
            .ok_or_else(|| "provider not found after create".to_string())
    }

    pub async fn update_provider(
        &self,
        id: &str,
        input: UpdateMonoizeProviderInput,
    ) -> Result<MonoizeProvider, String> {
        if let Some(channels) = &input.channels {
            validate_channels(channels, false)?;
        }
        if let Some(Some(v)) = input.active_probe_interval_seconds_override {
            if !(1..=i32::MAX as u64).contains(&v) {
                return Err(
                    "active_probe_interval_seconds_override must be between 1 and 2147483647"
                        .to_string(),
                );
            }
        }
        if let Some(Some(v)) = input.active_probe_success_threshold_override {
            if !(1..=i32::MAX as u32).contains(&v) {
                return Err(
                    "active_probe_success_threshold_override must be between 1 and 2147483647"
                        .to_string(),
                );
            }
        }
        if let Some(Some(v)) = input.request_timeout_ms_override {
            if !(1..=i32::MAX as u64).contains(&v) {
                return Err(
                    "request_timeout_ms_override must be between 1 and 2147483647".to_string(),
                );
            }
        }
        if let Some(v) = input.channel_retry_interval_ms {
            if v < 0 {
                return Err("channel_retry_interval_ms must be >= 0".to_string());
            }
        }

        if let Some(api_type_overrides) = &input.api_type_overrides {
            validate_api_type_overrides(api_type_overrides)?;
        }

        let mut set_clauses = Vec::new();
        let mut values: Vec<SeaValue> = Vec::new();
        let mut push_value = |column: &str, value: SeaValue| {
            let index = values.len() + 1;
            set_clauses.push(format!("{column} = ${index}"));
            values.push(value);
        };
        if let Some(value) = &input.name {
            push_value("name", value.clone().into());
        }
        if let Some(value) = input.max_retries {
            push_value("max_retries", SeaValue::Int(Some(value)));
        }
        if let Some(value) = input.channel_max_retries {
            push_value("channel_max_retries", SeaValue::Int(Some(value)));
        }
        if let Some(value) = input.channel_retry_interval_ms {
            push_value("channel_retry_interval_ms", SeaValue::Int(Some(value)));
        }
        if let Some(value) = input.circuit_breaker_enabled {
            push_value(
                "circuit_breaker_enabled",
                SeaValue::Int(Some(if value { 1 } else { 0 })),
            );
        }
        if let Some(value) = input.per_model_circuit_break {
            push_value(
                "per_model_circuit_break",
                SeaValue::Int(Some(if value { 1 } else { 0 })),
            );
        }
        if let Some(mut transforms) = input.transforms.clone() {
            canonicalize_transform_rules(&mut transforms);
            push_value(
                "transforms",
                serde_json::to_string(&transforms)
                    .map_err(|e| e.to_string())?
                    .into(),
            );
        }
        if let Some(value) = &input.api_type_overrides {
            push_value(
                "api_type_overrides",
                serde_json::to_string(value)
                    .map_err(|e| e.to_string())?
                    .into(),
            );
        }
        if let Some(value) = input.active_probe_enabled_override {
            push_value("active_probe_enabled_override", opt_bool_to_value(value));
        }
        if let Some(value) = input.active_probe_interval_seconds_override {
            push_value(
                "active_probe_interval_seconds_override",
                opt_u64_to_value(value),
            );
        }
        if let Some(value) = input.active_probe_success_threshold_override {
            push_value(
                "active_probe_success_threshold_override",
                opt_u64_to_value(value.map(u64::from)),
            );
        }
        if let Some(value) = &input.active_probe_model_override {
            push_value("active_probe_model_override", value.clone().into());
        }
        if let Some(value) = input.request_timeout_ms_override {
            push_value("request_timeout_ms_override", opt_u64_to_value(value));
        }
        if let Some(value) = &input.extra_fields_whitelist {
            let encoded = value
                .as_ref()
                .map(serde_json::to_string)
                .transpose()
                .map_err(|e| e.to_string())?;
            push_value("extra_fields_whitelist", encoded.into());
        }
        if let Some(value) = input.strip_cross_protocol_nested_extra {
            push_value(
                "strip_cross_protocol_nested_extra",
                opt_bool_to_value(value),
            );
        }
        if let Some(value) = &input.group_ids {
            let resolved = self.resolve_provider_group_ids(value).await?;
            push_value(
                "group_ids",
                serialize_provider_group_ids_json(&resolved)?.into(),
            );
        }
        if let Some(value) = input.allow_free_when_unpriced_override {
            push_value(
                "allow_free_when_unpriced_override",
                opt_bool_to_value(value),
            );
        }
        if let Some(value) = input.allow_free_when_missing_usage_override {
            push_value(
                "allow_free_when_missing_usage_override",
                opt_bool_to_value(value),
            );
        }
        if let Some(value) = input.enabled {
            push_value("enabled", SeaValue::Int(Some(if value { 1 } else { 0 })));
        }
        if let Some(value) = input.priority {
            push_value("priority", SeaValue::Int(Some(value)));
        }
        push_value("updated_at", Utc::now().to_rfc3339().into());
        drop(push_value);

        let id_index = values.len() + 1;
        values.push(id.into());
        let txn = self.db.begin_write().await.map_err(|e| e.to_string())?;
        let result = txn
            .execute(self.db.stmt(
                &format!(
                    "UPDATE monoize_providers SET {} WHERE id = ${id_index}",
                    set_clauses.join(", ")
                ),
                values,
            ))
            .await
            .map_err(|e| e.to_string())?;
        if result.rows_affected() == 0 {
            return Err("provider not found".to_string());
        }

        if let Some(channels) = &input.channels {
            self.replace_channels_on(&*txn, id, channels).await?;
        }

        txn.commit().await.map_err(|e| e.to_string())?;

        self.get_provider(id)
            .await?
            .ok_or_else(|| "provider not found after update".to_string())
    }

    pub async fn delete_provider(&self, id: &str) -> Result<(), String> {
        let result = self
            .db
            .write()
            .await
            .execute(self.db.stmt(
                "DELETE FROM monoize_providers WHERE id = $1",
                vec![id.into()],
            ))
            .await
            .map_err(|e| e.to_string())?;

        if result.rows_affected() == 0 {
            return Err("provider not found".to_string());
        }

        Ok(())
    }

    pub async fn reorder_providers(&self, input: ReorderProvidersInput) -> Result<(), String> {
        if input.provider_ids.len() > provider_reorder_max_ids() {
            return Err(format!(
                "provider reorder accepts at most {} ids",
                provider_reorder_max_ids()
            ));
        }
        let mut uniq = HashSet::new();
        for id in &input.provider_ids {
            if !uniq.insert(id.clone()) {
                return Err("provider_ids contains duplicates".to_string());
            }
        }

        let txn = self.db.begin_write().await.map_err(|e| e.to_string())?;
        if self.db.is_postgres() {
            txn.execute_unprepared("LOCK TABLE monoize_providers IN SHARE ROW EXCLUSIVE MODE")
                .await
                .map_err(|e| e.to_string())?;
        }
        let rows = txn
            .query_all(
                self.db
                    .stmt("SELECT id FROM monoize_providers ORDER BY id", vec![]),
            )
            .await
            .map_err(|e| e.to_string())?;
        if rows.len() != input.provider_ids.len() {
            return Err("provider_ids must contain all providers exactly once".to_string());
        }

        let existing_ids: HashSet<String> = rows
            .into_iter()
            .map(|row| row.try_get("", "id").map_err(|e| e.to_string()))
            .collect::<Result<_, _>>()?;
        let input_ids: HashSet<String> = input.provider_ids.iter().cloned().collect();
        if existing_ids != input_ids {
            return Err("provider_ids must contain all providers exactly once".to_string());
        }
        if input.provider_ids.is_empty() {
            return txn.commit().await.map_err(|e| e.to_string());
        }

        let mut values = Vec::with_capacity(input.provider_ids.len() * 2 + 1);
        let mut cases = Vec::with_capacity(input.provider_ids.len());
        for (priority, id) in input.provider_ids.iter().enumerate() {
            let id_index = values.len() + 1;
            values.push(id.clone().into());
            let priority_index = values.len() + 1;
            values.push(SeaValue::Int(Some(priority as i32)));
            cases.push(format!("WHEN ${id_index} THEN ${priority_index}"));
        }
        let updated_at_index = values.len() + 1;
        values.push(Utc::now().to_rfc3339().into());
        txn.execute(self.db.stmt(
            &format!(
                "UPDATE monoize_providers
                 SET priority = CASE id {} END, updated_at = ${updated_at_index}",
                cases.join(" ")
            ),
            values,
        ))
        .await
        .map_err(|e| e.to_string())?;
        txn.commit().await.map_err(|e| e.to_string())
    }

    async fn replace_channels_on(
        &self,
        conn: &impl ConnectionTrait,
        provider_id: &str,
        channels: &[CreateMonoizeChannelInput],
    ) -> Result<(), String> {
        let existing_rows = conn
            .query_all(self.db.stmt(
                "SELECT id, api_key
                 FROM monoize_channels
                 WHERE provider_id = $1",
                vec![provider_id.into()],
            ))
            .await
            .map_err(|e| e.to_string())?;

        #[derive(Clone)]
        struct ExistingChannel {
            api_key: String,
        }
        let mut existing_channels: HashMap<String, ExistingChannel> = HashMap::new();
        for row in &existing_rows {
            let id: String = row.try_get("", "id").map_err(|e| e.to_string())?;
            existing_channels.insert(
                id,
                ExistingChannel {
                    api_key: row.try_get("", "api_key").map_err(|e| e.to_string())?,
                },
            );
        }

        conn.execute(self.db.stmt(
            "DELETE FROM monoize_channels WHERE provider_id = $1",
            vec![provider_id.into()],
        ))
        .await
        .map_err(|e| e.to_string())?;

        struct PreparedChannel<'a> {
            id: String,
            api_key: String,
            input: &'a CreateMonoizeChannelInput,
            models: HashMap<String, MonoizeModelEntry>,
        }

        let mut prepared = Vec::with_capacity(channels.len());
        for input in channels {
            let id = input
                .id
                .clone()
                .unwrap_or_else(|| format!("mono_ch_{}", uuid::Uuid::new_v4().simple()));

            let api_key = match input.api_key.as_deref() {
                Some(k) if !k.trim().is_empty() => k.to_string(),
                _ => existing_channels
                    .get(&id)
                    .map(|c| c.api_key.clone())
                    .ok_or_else(|| {
                        format!(
                            "channel api_key must not be empty for new channel '{}'",
                            input.name
                        )
                    })?,
            };
            prepared.push(PreparedChannel {
                id,
                api_key,
                input,
                models: canonicalize_models(&input.models),
            });
        }

        const CHANNEL_INSERT_CHUNK_SIZE: usize = 18;
        let now = Utc::now().to_rfc3339();
        for chunk in prepared.chunks(CHANNEL_INSERT_CHUNK_SIZE) {
            let mut values: Vec<SeaValue> = Vec::with_capacity(chunk.len() * 25);
            let mut rows = Vec::with_capacity(chunk.len());
            for channel in chunk {
                let start = values.len() + 1;
                let input = channel.input;
                values.extend([
                    channel.id.clone().into(),
                    provider_id.into(),
                    input.name.as_str().into(),
                    input.provider_type.as_str().into(),
                    input.base_url.as_str().into(),
                    channel.api_key.clone().into(),
                    SeaValue::Int(Some(input.weight)),
                    SeaValue::Int(Some(if input.enabled { 1 } else { 0 })),
                    opt_u64_to_value(
                        input
                            .passive_failure_count_threshold_override
                            .map(|value| value as u64),
                    ),
                    opt_u64_to_value(input.passive_cooldown_seconds_override),
                    opt_u64_to_value(input.passive_window_seconds_override),
                    opt_u64_to_value(input.passive_rate_limit_cooldown_seconds_override),
                    opt_bool_to_value(input.active_probe_enabled_override),
                    opt_u64_to_value(input.active_probe_interval_seconds_override),
                    opt_u64_to_value(
                        input
                            .active_probe_success_threshold_override
                            .map(|value| value as u64),
                    ),
                    input.active_probe_model_override.clone().into(),
                    opt_bool_to_value(input.affinity_enabled_override),
                    opt_u64_to_value(input.affinity_idle_ttl_seconds_override),
                    input
                        .affinity_failback_mode_override
                        .map(|mode| mode.as_str().to_string())
                        .into(),
                    opt_u64_to_value(input.affinity_failback_delay_seconds_override),
                    normalized_proxy_url(input.proxy_url.as_deref()).into(),
                    normalized_extra_headers_json(input.extra_headers.as_ref()).into(),
                    opt_bool_to_value(input.session_affinity_auto),
                    now.clone().into(),
                    now.clone().into(),
                ]);
                rows.push(format!(
                    "({})",
                    (start..start + 25)
                        .map(|index| format!("${index}"))
                        .collect::<Vec<_>>()
                        .join(", ")
                ));
            }
            conn.execute(self.db.stmt(
                &format!(
                    "INSERT INTO monoize_channels
                     (id, provider_id, name, provider_type, base_url, api_key, weight, enabled,
                      passive_failure_count_threshold_override, passive_cooldown_seconds_override,
                      passive_window_seconds_override, passive_rate_limit_cooldown_seconds_override,
                      active_probe_enabled_override, active_probe_interval_seconds_override,
                      active_probe_success_threshold_override, active_probe_model_override,
                      affinity_enabled_override, affinity_idle_ttl_seconds_override,
                      affinity_failback_mode_override, affinity_failback_delay_seconds_override,
                      proxy_url,
                      extra_headers,
                      session_affinity_auto,
                      created_at, updated_at)
                     VALUES {}",
                    rows.join(", ")
                ),
                values,
            ))
            .await
            .map_err(|e| e.to_string())?;
        }

        let model_rows = prepared
            .iter()
            .flat_map(|channel| {
                channel
                    .models
                    .iter()
                    .map(|(model, entry)| (channel.id.clone(), model.clone(), entry.clone()))
            })
            .collect::<Vec<_>>();
        const MODEL_INSERT_CHUNK_SIZE: usize = 66;
        for chunk in model_rows.chunks(MODEL_INSERT_CHUNK_SIZE) {
            let mut values: Vec<SeaValue> = Vec::with_capacity(chunk.len() * 6);
            let mut rows = Vec::with_capacity(chunk.len());
            for (channel_id, model, entry) in chunk {
                let start = values.len() + 1;
                values.extend([
                    format!("mono_ch_model_{}", uuid::Uuid::new_v4().simple()).into(),
                    channel_id.clone().into(),
                    model.clone().into(),
                    entry.redirect.clone().into(),
                    entry.multiplier.to_string().into(),
                    now.clone().into(),
                ]);
                rows.push(format!(
                    "({})",
                    (start..start + 6)
                        .map(|index| format!("${index}"))
                        .collect::<Vec<_>>()
                        .join(", ")
                ));
            }
            conn.execute(self.db.stmt(
                &format!(
                    "INSERT INTO monoize_channel_models
                     (id, channel_id, model_name, redirect, multiplier, created_at)
                     VALUES {}",
                    rows.join(", ")
                ),
                values,
            ))
            .await
            .map_err(|e| e.to_string())?;
        }
        Ok(())
    }
}

fn opt_bool_to_value(v: Option<bool>) -> SeaValue {
    match v {
        Some(b) => SeaValue::Int(Some(if b { 1 } else { 0 })),
        None => SeaValue::Int(None),
    }
}

fn opt_u64_to_value(v: Option<u64>) -> SeaValue {
    match v {
        Some(n) => SeaValue::Int(Some(n as i32)),
        None => SeaValue::Int(None),
    }
}

fn decode_positive_u32(provider_id: &str, field: &str, value: i64) -> Result<u32, String> {
    u32::try_from(value)
        .ok()
        .filter(|v| *v >= 1)
        .ok_or_else(|| format!("provider {provider_id} invalid {field}: must be >= 1"))
}

fn decode_positive_u64(provider_id: &str, field: &str, value: i64) -> Result<u64, String> {
    u64::try_from(value)
        .ok()
        .filter(|v| *v >= 1)
        .ok_or_else(|| format!("provider {provider_id} invalid {field}: must be >= 1"))
}

fn decode_nonnegative_u64(provider_id: &str, field: &str, value: i64) -> Result<u64, String> {
    u64::try_from(value)
        .map_err(|_| format!("provider {provider_id} invalid {field}: must be >= 0"))
}

fn canonicalize_models(
    models: &HashMap<String, MonoizeModelEntry>,
) -> HashMap<String, MonoizeModelEntry> {
    let mut out = HashMap::new();
    for (model, entry) in models {
        let model = model.trim();
        if model.is_empty() {
            continue;
        }
        out.insert(
            model.to_string(),
            MonoizeModelEntry {
                redirect: entry
                    .redirect
                    .as_deref()
                    .map(str::trim)
                    .filter(|value| !value.is_empty())
                    .map(str::to_string),
                multiplier: entry.multiplier,
            },
        );
    }
    out
}

fn validate_models(models: &HashMap<String, MonoizeModelEntry>) -> Result<(), String> {
    for model in models.keys() {
        if model.trim().is_empty() {
            return Err("model key must not be empty".to_string());
        }
    }
    Ok(())
}

fn validate_channels(
    channels: &[CreateMonoizeChannelInput],
    require_api_key: bool,
) -> Result<(), String> {
    if channels.is_empty() {
        return Err("channels must not be empty".to_string());
    }
    if !channels.iter().any(|channel| !channel.models.is_empty()) {
        return Err("at least one channel must define a model".to_string());
    }
    let mut ids = HashSet::new();
    for c in channels {
        if c.name.trim().is_empty() {
            return Err("channel name must not be empty".to_string());
        }
        if c.base_url.trim().is_empty() {
            return Err("channel base_url must not be empty".to_string());
        }
        if require_api_key {
            let key = c.api_key.as_deref().unwrap_or("");
            if key.trim().is_empty() {
                return Err("channel api_key must not be empty".to_string());
            }
        }
        if c.weight < 0 {
            return Err("channel weight must be >= 0".to_string());
        }
        if let Some(headers) = &c.extra_headers {
            validate_channel_extra_headers(&c.name, headers)?;
        }
        if let Some(v) = c.passive_failure_count_threshold_override {
            if !(1..=i32::MAX as u32).contains(&v) {
                return Err(
                    "channel passive_failure_count_threshold_override must be between 1 and 2147483647".to_string(),
                );
            }
        }
        if let Some(v) = c.passive_cooldown_seconds_override {
            if !(1..=i32::MAX as u64).contains(&v) {
                return Err(
                    "channel passive_cooldown_seconds_override must be between 1 and 2147483647"
                        .to_string(),
                );
            }
        }
        if let Some(v) = c.passive_window_seconds_override {
            if !(1..=i32::MAX as u64).contains(&v) {
                return Err(
                    "channel passive_window_seconds_override must be between 1 and 2147483647"
                        .to_string(),
                );
            }
        }
        if let Some(v) = c.passive_rate_limit_cooldown_seconds_override {
            if !(1..=i32::MAX as u64).contains(&v) {
                return Err(
                    "channel passive_rate_limit_cooldown_seconds_override must be between 1 and 2147483647".to_string(),
                );
            }
        }
        if let Some(v) = c.active_probe_interval_seconds_override {
            if !(1..=i32::MAX as u64).contains(&v) {
                return Err(
                    "channel active_probe_interval_seconds_override must be between 1 and 2147483647".to_string(),
                );
            }
        }
        if let Some(v) = c.active_probe_success_threshold_override {
            if !(1..=i32::MAX as u32).contains(&v) {
                return Err(
                    "channel active_probe_success_threshold_override must be between 1 and 2147483647".to_string(),
                );
            }
        }
        if let Some(v) = c.affinity_idle_ttl_seconds_override {
            if !(1..=i32::MAX as u64).contains(&v) {
                return Err(
                    "channel affinity_idle_ttl_seconds_override must be between 1 and 2147483647"
                        .to_string(),
                );
            }
        }
        if let Some(v) = c.affinity_failback_delay_seconds_override {
            if v > i32::MAX as u64 {
                return Err(
                    "channel affinity_failback_delay_seconds_override must be between 0 and 2147483647"
                        .to_string(),
                );
            }
        }
        validate_models(&c.models)?;
        let mut model_seen = HashSet::new();
        for model in c.models.keys() {
            let model = model.trim();
            if model.is_empty() {
                return Err("channel model keys must not be empty".to_string());
            }
            if !model_seen.insert(model.to_string()) {
                return Err(format!(
                    "channel '{}' has duplicate model '{}'",
                    c.name, model
                ));
            }
        }
        if let Some(id) = &c.id {
            if !ids.insert(id.clone()) {
                return Err("duplicate channel id".to_string());
            }
        }
    }
    Ok(())
}

fn validate_provider_input(
    name: &str,
    channels: &[CreateMonoizeChannelInput],
    api_type_overrides: &[ApiTypeOverride],
) -> Result<(), String> {
    if name.trim().is_empty() {
        return Err("provider name must not be empty".to_string());
    }
    validate_channels(channels, true)?;
    validate_api_type_overrides(api_type_overrides)?;
    Ok(())
}

fn validate_api_type_overrides(overrides: &[ApiTypeOverride]) -> Result<(), String> {
    for (idx, entry) in overrides.iter().enumerate() {
        if entry.pattern.trim().is_empty() {
            return Err(format!(
                "api_type_overrides[{idx}].pattern must not be empty"
            ));
        }
    }
    Ok(())
}

pub async fn probe_channel_list_models(
    client: &reqwest::Client,
    channel: &MonoizeChannel,
    timeout_ms: u64,
) -> bool {
    let base = channel.base_url.trim_end_matches('/');
    let url = format!("{base}/v1/models");

    let result = client
        .get(url)
        .timeout(Duration::from_millis(timeout_ms))
        .bearer_auth(&channel.api_key)
        .send()
        .await;

    match result {
        Ok(resp) => resp.status().is_success(),
        Err(_) => false,
    }
}

/// Resolves the effective API type for a given model by evaluating api_type_overrides
/// in order. First matching glob pattern wins; falls back to the default provider_type.
pub fn resolve_effective_api_type(
    overrides: &[ApiTypeOverride],
    default_type: MonoizeProviderType,
    model: &str,
) -> MonoizeProviderType {
    for entry in overrides {
        if glob_match(&entry.pattern, model) {
            return entry.api_type;
        }
    }
    default_type
}

fn glob_match(pattern: &str, value: &str) -> bool {
    crate::glob::case_sensitive_glob_match(pattern, value)
}

pub struct ChannelProbeOutcome {
    pub ok: bool,
    pub usage: Option<Value>,
    pub http_status: Option<u16>,
    pub error_code: Option<String>,
    pub error_type: Option<String>,
    pub error: Option<String>,
}

const PROBE_ERROR_BODY_MAX_CHARS: usize = 512;

fn truncate_probe_body(body: &str) -> String {
    let body = body.trim();
    if body.chars().count() <= PROBE_ERROR_BODY_MAX_CHARS {
        return body.to_string();
    }
    let truncated: String = body.chars().take(PROBE_ERROR_BODY_MAX_CHARS).collect();
    format!("{truncated}…")
}

pub fn format_probe_http_error(status: reqwest::StatusCode, body: &str) -> String {
    let code = status.as_u16();
    let reason = status.canonical_reason().unwrap_or("");
    let body = truncate_probe_body(body);
    if body.is_empty() {
        if reason.is_empty() {
            format!("upstream returned {code}")
        } else {
            format!("upstream returned {code} {reason}")
        }
    } else if reason.is_empty() {
        format!("upstream returned {code}: {body}")
    } else {
        format!("upstream returned {code} {reason}: {body}")
    }
}

fn probe_error_metadata(body: &str) -> (Option<String>, Option<String>) {
    let Ok(value) = serde_json::from_str::<Value>(body) else {
        return (None, None);
    };
    let error = value.get("error").unwrap_or(&value);
    let code = error
        .get("code")
        .and_then(Value::as_str)
        .filter(|value| !value.trim().is_empty())
        .map(str::to_string);
    let error_type = error
        .get("type")
        .and_then(Value::as_str)
        .filter(|value| !value.trim().is_empty())
        .map(str::to_string);
    (code, error_type)
}

fn probe_stream_error(
    value: &Value,
    sse_event: &str,
) -> Option<(Option<String>, Option<String>, String)> {
    let event_type = value.get("type").and_then(Value::as_str);
    let error = value.get("error");
    if error.is_none()
        && !matches!(
            event_type,
            Some("error" | "response.failed" | "response.incomplete")
        )
        && !matches!(
            sse_event,
            "error" | "response.failed" | "response.incomplete"
        )
    {
        return None;
    }
    let detail = error.unwrap_or(value);
    let code = detail
        .get("code")
        .and_then(Value::as_str)
        .filter(|value| !value.trim().is_empty())
        .map(str::to_string);
    let error_type = detail
        .get("type")
        .and_then(Value::as_str)
        .or(event_type)
        .or_else(|| (!sse_event.is_empty()).then_some(sse_event))
        .filter(|value| !value.trim().is_empty())
        .map(str::to_string);
    let message = detail
        .get("message")
        .and_then(Value::as_str)
        .filter(|value| !value.trim().is_empty())
        .map(str::to_string)
        .unwrap_or_else(|| truncate_probe_body(&detail.to_string()));
    Some((
        code,
        error_type,
        format!("upstream stream error: {message}"),
    ))
}

async fn read_probe_stream(
    response: reqwest::Response,
    effective_type: MonoizeProviderType,
) -> ChannelProbeOutcome {
    let status = response.status().as_u16();
    let mut usage = None;
    let mut chat_terminal_chunk_seen = false;
    let mut stream = response.bytes_stream().eventsource();

    while let Some(event) = stream.next().await {
        let event = match event {
            Ok(event) => event,
            Err(error) => {
                return ChannelProbeOutcome {
                    ok: false,
                    usage,
                    http_status: Some(status),
                    error_code: Some("upstream_stream_decode_failed".to_string()),
                    error_type: Some("stream_error".to_string()),
                    error: Some(format!("upstream stream decode failed: {error}")),
                };
            }
        };
        let data = event.data.trim();
        if data.is_empty() {
            continue;
        }
        if data == "[DONE]" {
            if effective_type != MonoizeProviderType::ChatCompletion || !chat_terminal_chunk_seen {
                return ChannelProbeOutcome {
                    ok: false,
                    usage,
                    http_status: Some(status),
                    error_code: Some("upstream_stream_missing_terminal".to_string()),
                    error_type: Some("stream_error".to_string()),
                    error: Some(format!(
                        "upstream {} stream sent [DONE] before its terminal event",
                        effective_type.as_str()
                    )),
                };
            }
            return ChannelProbeOutcome {
                ok: true,
                usage,
                http_status: Some(status),
                error_code: None,
                error_type: None,
                error: None,
            };
        }

        let value = match serde_json::from_str::<Value>(data) {
            Ok(value) => value,
            Err(error) => {
                return ChannelProbeOutcome {
                    ok: false,
                    usage,
                    http_status: Some(status),
                    error_code: Some("upstream_stream_decode_failed".to_string()),
                    error_type: Some("stream_error".to_string()),
                    error: Some(format!(
                        "upstream stream event contains invalid JSON: {error}: {}",
                        truncate_probe_body(data)
                    )),
                };
            }
        };
        if let Some(stream_usage) = extract_probe_usage(&value) {
            usage = Some(stream_usage);
        }
        if let Some((error_code, error_type, error)) = probe_stream_error(&value, &event.event) {
            return ChannelProbeOutcome {
                ok: false,
                usage,
                http_status: Some(status),
                error_code,
                error_type,
                error: Some(error),
            };
        }

        let event_type = value.get("type").and_then(Value::as_str);
        match effective_type {
            MonoizeProviderType::Responses => {
                if event.event == "response.completed" || event_type == Some("response.completed") {
                    return ChannelProbeOutcome {
                        ok: true,
                        usage,
                        http_status: Some(status),
                        error_code: None,
                        error_type: None,
                        error: None,
                    };
                }
            }
            MonoizeProviderType::ChatCompletion => {
                chat_terminal_chunk_seen |= value
                    .get("choices")
                    .and_then(Value::as_array)
                    .is_some_and(|choices| {
                        choices.iter().any(|choice| {
                            choice
                                .get("finish_reason")
                                .is_some_and(|reason| !reason.is_null())
                        })
                    });
            }
            MonoizeProviderType::Messages => {
                if event.event == "message_stop" || event_type == Some("message_stop") {
                    return ChannelProbeOutcome {
                        ok: true,
                        usage,
                        http_status: Some(status),
                        error_code: None,
                        error_type: None,
                        error: None,
                    };
                }
            }
            MonoizeProviderType::Gemini => {
                let terminal = value
                    .get("candidates")
                    .and_then(Value::as_array)
                    .is_some_and(|candidates| {
                        candidates.iter().any(|candidate| {
                            candidate
                                .get("finishReason")
                                .is_some_and(|reason| !reason.is_null())
                        })
                    });
                if terminal {
                    return ChannelProbeOutcome {
                        ok: true,
                        usage,
                        http_status: Some(status),
                        error_code: None,
                        error_type: None,
                        error: None,
                    };
                }
            }
            MonoizeProviderType::OpenaiImage | MonoizeProviderType::Replicate => {}
        }
    }

    ChannelProbeOutcome {
        ok: false,
        usage,
        http_status: Some(status),
        error_code: Some("upstream_stream_missing_terminal".to_string()),
        error_type: Some("stream_error".to_string()),
        error: Some(format!(
            "upstream {} stream ended without a terminal event",
            effective_type.as_str()
        )),
    }
}

pub async fn probe_channel_completion(
    client: &reqwest::Client,
    channel: &MonoizeChannel,
    timeout_ms: u64,
    model: &str,
    provider_type: MonoizeProviderType,
    api_type_overrides: &[ApiTypeOverride],
    stream: bool,
) -> ChannelProbeOutcome {
    let effective_type = resolve_effective_api_type(api_type_overrides, provider_type, model);
    let base = channel.base_url.trim_end_matches('/');
    let (url, body, extra_headers, use_google_api_key_header) =
        build_probe_request(base, model, effective_type, stream);

    let mut request = client.post(&url).timeout(Duration::from_millis(timeout_ms));
    request = if use_google_api_key_header {
        request.header("x-goog-api-key", &channel.api_key)
    } else {
        request.bearer_auth(&channel.api_key)
    };
    for &(header_name, header_value) in extra_headers {
        request = request.header(header_name, header_value);
    }
    if let Some(channel_headers) = &channel.extra_headers {
        for (header_name, header_value) in channel_headers {
            request = request.header(header_name, header_value);
        }
    }
    let result = request.json(&body).send().await;

    match result {
        Ok(resp) => {
            let status = resp.status();
            if !status.is_success() {
                let body = resp.text().await.unwrap_or_default();
                let (error_code, error_type) = probe_error_metadata(&body);
                return ChannelProbeOutcome {
                    ok: false,
                    usage: None,
                    http_status: Some(status.as_u16()),
                    error_code,
                    error_type,
                    error: Some(format_probe_http_error(status, &body)),
                };
            }
            if stream {
                return read_probe_stream(resp, effective_type).await;
            }
            let usage = match resp.json::<Value>().await {
                Ok(value) => extract_probe_usage(&value),
                Err(_) => None,
            };
            ChannelProbeOutcome {
                ok: true,
                usage,
                http_status: Some(status.as_u16()),
                error_code: None,
                error_type: None,
                error: None,
            }
        }
        Err(error) => ChannelProbeOutcome {
            ok: false,
            usage: None,
            http_status: None,
            error_code: Some("upstream_connection_failed".to_string()),
            error_type: Some("transport_error".to_string()),
            error: Some(format!("connection failed: {error}")),
        },
    }
}

fn build_probe_request(
    base: &str,
    model: &str,
    effective_type: MonoizeProviderType,
    stream: bool,
) -> (String, Value, &'static [(&'static str, &'static str)], bool) {
    match effective_type {
        MonoizeProviderType::Responses => {
            let url = format!("{base}/v1/responses");
            let body = serde_json::json!({
                "model": model,
                "max_output_tokens": 16,
                "stream": stream,
                "input": [{"type": "message", "role": "user", "content": [{"type": "input_text", "text": "hi"}]}]
            });
            (url, body, &[][..], false)
        }
        MonoizeProviderType::ChatCompletion => {
            let url = format!("{base}/v1/chat/completions");
            let body = serde_json::json!({
                "model": model,
                "max_tokens": 16,
                "stream": stream,
                "messages": [{"role": "user", "content": "hi"}]
            });
            (url, body, &[][..], false)
        }
        MonoizeProviderType::Messages => {
            let url = format!("{base}/v1/messages");
            let body = serde_json::json!({
                "model": model,
                "max_tokens": 16,
                "stream": stream,
                "messages": [{"role": "user", "content": "hi"}]
            });
            (url, body, &[("anthropic-version", "2023-06-01")][..], false)
        }
        MonoizeProviderType::Gemini => {
            let method = if stream {
                "streamGenerateContent?alt=sse"
            } else {
                "generateContent"
            };
            let url = format!("{base}/v1beta/models/{model}:{method}");
            let body = serde_json::json!({
                "contents": [{"role": "user", "parts": [{"text": "hi"}]}],
                "generationConfig": {"maxOutputTokens": 16}
            });
            (url, body, &[][..], true)
        }
        MonoizeProviderType::OpenaiImage => {
            let url = format!("{base}/v1/images/generations");
            let body = serde_json::json!({
                "model": model,
                "prompt": "test",
                "size": "1024x1024",
                "n": 1,
            });
            (url, body, &[][..], false)
        }
        MonoizeProviderType::Replicate => {
            // Replicate providers are excluded from active probing; this is a
            // fallback that should never be reached.
            let url = format!("{base}/v1/predictions");
            let body = serde_json::json!({
                "version": model,
                "input": {}
            });
            (url, body, &[][..], false)
        }
    }
}

fn extract_probe_usage(body: &Value) -> Option<Value> {
    if let Some(usage) = body.get("usage") {
        let prompt_tokens = usage
            .get("prompt_tokens")
            .and_then(Value::as_u64)
            .or_else(|| usage.get("input_tokens").and_then(Value::as_u64));
        let completion_tokens = usage
            .get("completion_tokens")
            .and_then(Value::as_u64)
            .or_else(|| usage.get("output_tokens").and_then(Value::as_u64));

        if let (Some(prompt_tokens), Some(completion_tokens)) = (prompt_tokens, completion_tokens) {
            return Some(
                json!({"prompt_tokens": prompt_tokens, "completion_tokens": completion_tokens}),
            );
        }
    }

    let usage = body.get("usageMetadata")?;
    let prompt_tokens = usage
        .get("promptTokenCount")
        .and_then(Value::as_u64)
        .or_else(|| usage.get("input_tokens").and_then(Value::as_u64));
    let completion_tokens = usage
        .get("candidatesTokenCount")
        .and_then(Value::as_u64)
        .or_else(|| usage.get("output_tokens").and_then(Value::as_u64));

    match (prompt_tokens, completion_tokens) {
        (Some(prompt_tokens), Some(completion_tokens)) => {
            Some(json!({"prompt_tokens": prompt_tokens, "completion_tokens": completion_tokens}))
        }
        _ => None,
    }
}
