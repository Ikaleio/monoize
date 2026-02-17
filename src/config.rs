use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Copy, Deserialize, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum UnknownFieldPolicy {
    Reject,
    Ignore,
    Preserve,
}

impl Default for UnknownFieldPolicy {
    fn default() -> Self {
        UnknownFieldPolicy::Preserve
    }
}

#[derive(Debug, Clone, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ProviderConfig {
    pub id: String,
    #[serde(rename = "type")]
    pub provider_type: ProviderType,
    #[serde(default)]
    pub base_url: Option<String>,
    #[serde(default)]
    pub auth: Option<ProviderAuthConfig>,
    #[serde(default)]
    pub model_map: Vec<ModelMapEntry>,
    #[serde(default)]
    pub strategy: Option<GroupStrategyConfig>,
    #[serde(default)]
    pub members: Vec<GroupMemberConfig>,
}

#[derive(Debug, Clone, Copy, Deserialize, Serialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum ProviderType {
    Responses,
    ChatCompletion,
    Messages,
    Gemini,
    Grok,
    Group,
}

#[derive(Debug, Clone, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct GroupStrategyConfig {
    #[serde(rename = "type")]
    pub strategy_type: GroupStrategyType,
    #[serde(default = "default_group_max_attempts")]
    pub max_attempts: usize,
    #[serde(default = "default_group_backoff_ms")]
    pub backoff_ms: Vec<u64>,
    #[serde(default = "default_group_retry_on")]
    pub retry_on: Vec<GroupFailureClass>,
    #[serde(default = "default_group_non_retry_codes")]
    pub non_retry_codes: Vec<String>,
    #[serde(default = "default_group_fallback_on")]
    pub fallback_on: Vec<GroupFailureClass>,
}

impl Default for GroupStrategyConfig {
    fn default() -> Self {
        Self {
            strategy_type: GroupStrategyType::WeightedRoundRobin,
            max_attempts: default_group_max_attempts(),
            backoff_ms: default_group_backoff_ms(),
            retry_on: default_group_retry_on(),
            non_retry_codes: default_group_non_retry_codes(),
            fallback_on: default_group_fallback_on(),
        }
    }
}

#[derive(Debug, Clone, Copy, Deserialize, Serialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum GroupStrategyType {
    WeightedRoundRobin,
    Failover,
}

#[derive(Debug, Clone, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct GroupMemberConfig {
    pub provider_id: String,
    #[serde(default = "default_group_weight")]
    pub weight: u32,
}

#[derive(Debug, Clone, Copy, Deserialize, Serialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum GroupFailureClass {
    Network,
    #[serde(rename = "http_408")]
    Http408,
    #[serde(rename = "http_429")]
    Http429,
    #[serde(rename = "http_5xx")]
    Http5xx,
    RetryExhausted,
    NonRetryable,
}

#[derive(Debug, Clone, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ProviderAuthConfig {
    #[serde(rename = "type")]
    pub auth_type: ProviderAuthType,
    pub value: String,
    #[serde(default)]
    pub header_name: Option<String>,
    #[serde(default)]
    pub query_name: Option<String>,
}

#[derive(Debug, Clone, Copy, Deserialize, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum ProviderAuthType {
    Bearer,
    Header,
    Query,
}

#[derive(Debug, Clone, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ModelMapEntry {
    pub logical_model: String,
    pub upstream_model: String,
}

fn default_group_weight() -> u32 {
    1
}

fn default_group_max_attempts() -> usize {
    2
}

fn default_group_backoff_ms() -> Vec<u64> {
    vec![200, 500]
}

fn default_group_retry_on() -> Vec<GroupFailureClass> {
    vec![
        GroupFailureClass::Network,
        GroupFailureClass::Http5xx,
        GroupFailureClass::Http429,
    ]
}

fn default_group_non_retry_codes() -> Vec<String> {
    vec![
        "insufficient_quota".to_string(),
        "invalid_api_key".to_string(),
        "invalid_request_error".to_string(),
        "model_not_found".to_string(),
        "permission_denied".to_string(),
    ]
}

fn default_group_fallback_on() -> Vec<GroupFailureClass> {
    vec![
        GroupFailureClass::RetryExhausted,
        GroupFailureClass::NonRetryable,
        GroupFailureClass::Network,
        GroupFailureClass::Http5xx,
        GroupFailureClass::Http429,
    ]
}
