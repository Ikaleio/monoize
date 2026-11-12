use crate::exact_decimal::Multiplier;
use crate::transforms::TransformRuleConfig;
use crate::urp::encode::openai_chat::ChatDialect;
use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use std::collections::{BTreeMap, HashMap};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum MonoizeProviderType {
    Responses,
    ChatCompletion,
    Messages,
    Gemini,
    OpenaiImage,
    OpenrouterImage,
    Replicate,
    SystemOne,
    OpenaiDecisions,
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
            "openrouter_image" => Some(Self::OpenrouterImage),
            "replicate" => Some(Self::Replicate),
            "system_one" => Some(Self::SystemOne),
            "openai_decisions" => Some(Self::OpenaiDecisions),
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
            Self::OpenrouterImage => "openrouter_image",
            Self::Replicate => "replicate",
            Self::SystemOne => "system_one",
            Self::OpenaiDecisions => "openai_decisions",
        }
    }

    pub fn to_config_type(&self) -> crate::config::ProviderType {
        match self {
            Self::Responses => crate::config::ProviderType::Responses,
            Self::ChatCompletion => crate::config::ProviderType::ChatCompletion,
            Self::Messages => crate::config::ProviderType::Messages,
            Self::Gemini => crate::config::ProviderType::Gemini,
            Self::OpenaiImage => crate::config::ProviderType::OpenaiImage,
            Self::OpenrouterImage => crate::config::ProviderType::OpenrouterImage,
            Self::Replicate => crate::config::ProviderType::Replicate,
            Self::SystemOne => crate::config::ProviderType::SystemOne,
            Self::OpenaiDecisions => crate::config::ProviderType::OpenaiDecisions,
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

    pub fn parse(value: &str) -> Option<Self> {
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
    /// CP-INV-17: present exactly on `chat_completion` entries.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub chat_dialect: Option<ChatDialect>,
}

/// CP-INV-17a: keeps the dialect as raw text so an unknown value reports through `param`.
#[derive(Debug, Clone, Deserialize)]
pub struct ApiTypeOverrideInput {
    pub pattern: String,
    pub api_type: MonoizeProviderType,
    #[serde(default)]
    pub chat_dialect: Option<String>,
}

fn serialize_sorted_models<S: serde::Serializer>(
    models: &HashMap<String, MonoizeModelEntry>,
    serializer: S,
) -> Result<S::Ok, S::Error> {
    use serde::ser::SerializeMap;
    let mut entries: Vec<_> = models.iter().collect();
    entries.sort_unstable_by(|left, right| left.0.cmp(right.0));
    let mut map = serializer.serialize_map(Some(entries.len()))?;
    for (key, value) in entries {
        map.serialize_entry(key, value)?;
    }
    map.end()
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
    /// CM-READ-1: serialized in ascending key order so API reads are stable.
    #[serde(default, serialize_with = "serialize_sorted_models")]
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
    /// WS2e: null = unknown, true = prefer WS, false = HTTP only.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub websocket_supported: Option<bool>,
    /// CP-INV-16: present exactly on `chat_completion` Channels.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub chat_dialect: Option<ChatDialect>,
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
    /// WS2e: explicit Responses upstream WebSocket memory.
    #[serde(default)]
    pub websocket_supported: Option<bool>,
    /// CP-INV-16a: kept as raw text so an unknown value reports through `param`.
    #[serde(default)]
    pub chat_dialect: Option<String>,
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
    pub api_type_overrides: Vec<ApiTypeOverrideInput>,
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
    pub api_type_overrides: Option<Vec<ApiTypeOverrideInput>>,
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

fn default_enabled() -> bool {
    true
}

fn default_max_retries() -> i32 {
    -1
}

fn default_channel_weight() -> i32 {
    1
}
