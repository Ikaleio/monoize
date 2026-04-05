mod request_logs;
mod store;
mod utils;

use crate::db::DbPool;
use crate::transforms::TransformRuleConfig;
use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use serde_json::Value;
use std::collections::BTreeSet;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum UserRole {
    SuperAdmin,
    Admin,
    User,
}

impl UserRole {
    #[allow(clippy::should_implement_trait)]
    pub fn from_str(s: &str) -> Option<Self> {
        match s {
            "super_admin" => Some(Self::SuperAdmin),
            "admin" => Some(Self::Admin),
            "user" => Some(Self::User),
            _ => None,
        }
    }

    pub fn as_str(&self) -> &'static str {
        match self {
            Self::SuperAdmin => "super_admin",
            Self::Admin => "admin",
            Self::User => "user",
        }
    }

    pub fn can_manage_users(&self) -> bool {
        matches!(self, Self::SuperAdmin | Self::Admin)
    }

    pub fn can_manage_system(&self) -> bool {
        matches!(self, Self::SuperAdmin | Self::Admin)
    }

    pub fn can_assign_role(&self, target_role: UserRole) -> bool {
        match self {
            Self::SuperAdmin => true,
            Self::Admin => matches!(target_role, Self::User),
            Self::User => false,
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct User {
    pub id: String,
    pub username: String,
    #[serde(skip_serializing)]
    pub password_hash: String,
    pub role: UserRole,
    pub created_at: DateTime<Utc>,
    pub updated_at: DateTime<Utc>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub last_login_at: Option<DateTime<Utc>>,
    pub enabled: bool,
    /// Signed nano-dollar string balance.
    pub balance_nano_usd: String,
    /// Unlimited balance bypass flag.
    pub balance_unlimited: bool,
    /// Optional email for Gravatar display.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub email: Option<String>,
    #[serde(default)]
    pub allowed_groups: Vec<String>,
}

#[derive(Debug, Clone)]
pub struct UserBalance {
    pub user_id: String,
    pub balance_nano_usd: i128,
    pub balance_unlimited: bool,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum BillingErrorKind {
    NotFound,
    InsufficientBalance,
    InvalidStoredBalance,
    Overflow,
    Internal,
}

#[derive(Debug, Clone)]
pub struct BillingError {
    pub kind: BillingErrorKind,
    pub message: String,
}

impl BillingError {
    pub(crate) fn new(kind: BillingErrorKind, message: impl Into<String>) -> Self {
        Self {
            kind,
            message: message.into(),
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Session {
    pub id: String,
    pub user_id: String,
    pub token: String,
    pub created_at: DateTime<Utc>,
    pub expires_at: DateTime<Utc>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ModelRedirectRule {
    pub pattern: String,
    pub replace: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ApiKey {
    pub id: String,
    pub user_id: String,
    pub name: String,
    pub key_prefix: String,
    /// The full API key (stored for display purposes)
    pub key: String,
    #[serde(skip_serializing)]
    pub key_hash: String,
    pub created_at: DateTime<Utc>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub expires_at: Option<DateTime<Utc>>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub last_used_at: Option<DateTime<Utc>>,
    pub enabled: bool,
    #[serde(default)]
    pub sub_account_enabled: bool,
    #[serde(default)]
    pub sub_account_balance_nano: String,
    /// Whether model restrictions are active
    #[serde(default)]
    pub model_limits_enabled: bool,
    /// List of allowed model IDs (empty = all models when model_limits_enabled is false)
    #[serde(default)]
    pub model_limits: Vec<String>,
    /// List of allowed IP addresses/CIDRs (empty = any IP)
    #[serde(default)]
    pub ip_whitelist: Vec<String>,
    #[serde(default)]
    pub allowed_groups: Vec<String>,
    /// Maximum accepted multiplier for routing
    #[serde(skip_serializing_if = "Option::is_none")]
    pub max_multiplier: Option<f64>,
    /// Ordered transform rules applied for this API key
    #[serde(default)]
    pub transforms: Vec<TransformRuleConfig>,
    #[serde(default)]
    pub model_redirects: Vec<ModelRedirectRule>,
}

/// Input for creating a new API key with extended fields
#[derive(Debug, Clone, Deserialize)]
pub struct CreateApiKeyInput {
    pub name: String,
    pub expires_in_days: Option<i64>,
    #[serde(default)]
    pub sub_account_enabled: bool,
    #[serde(default)]
    pub model_limits_enabled: bool,
    #[serde(default)]
    pub model_limits: Vec<String>,
    #[serde(default)]
    pub ip_whitelist: Vec<String>,
    #[serde(default)]
    pub allowed_groups: Vec<String>,
    #[serde(default)]
    pub max_multiplier: Option<f64>,
    #[serde(default)]
    pub transforms: Vec<TransformRuleConfig>,
    #[serde(default)]
    pub model_redirects: Vec<ModelRedirectRule>,
}

pub fn validate_model_redirects(rules: &[ModelRedirectRule]) -> Result<(), String> {
    if rules.len() > 32 {
        return Err("too many model redirect rules (max 32)".to_string());
    }

    for rule in rules {
        if rule.pattern.trim().is_empty() {
            return Err("model redirect pattern must not be empty".to_string());
        }
        if rule.replace.trim().is_empty() {
            return Err("model redirect replace must not be empty".to_string());
        }
        regex::Regex::new(&rule.pattern)
            .map_err(|e| format!("invalid model redirect pattern: {e}"))?;
    }

    Ok(())
}

pub fn canonicalize_groups(groups: &[String]) -> Vec<String> {
    groups
        .iter()
        .map(|group| group.trim().to_lowercase())
        .filter(|group| !group.is_empty())
        .collect::<BTreeSet<_>>()
        .into_iter()
        .collect()
}

pub fn parse_groups_json(raw: &str) -> Vec<String> {
    if raw.trim().is_empty() {
        return Vec::new();
    }

    serde_json::from_str::<Vec<String>>(raw)
        .map(|groups| canonicalize_groups(&groups))
        .unwrap_or_default()
}

pub fn compute_effective_groups(
    user_groups: &[String],
    key_groups: &[String],
) -> Option<Vec<String>> {
    let user_groups = canonicalize_groups(user_groups);
    let key_groups = canonicalize_groups(key_groups);

    if user_groups.is_empty() && key_groups.is_empty() {
        return None;
    }

    if key_groups.is_empty() {
        return if user_groups.is_empty() {
            None
        } else {
            Some(user_groups)
        };
    }

    if user_groups.is_empty() {
        return Some(key_groups);
    }

    let user_groups: BTreeSet<_> = user_groups.into_iter().collect();
    let key_groups: BTreeSet<_> = key_groups.into_iter().collect();

    Some(user_groups.intersection(&key_groups).cloned().collect())
}

/// Exclusive group routing: when effective_groups is non-empty, only providers
/// with explicitly matching groups are eligible — public providers are excluded.
pub fn is_channel_group_eligible(
    channel_groups: &[String],
    effective_groups: &Option<Vec<String>>,
) -> bool {
    let channel_groups = canonicalize_groups(channel_groups);

    let Some(effective_groups) = effective_groups else {
        // Unrestricted caller: all providers eligible
        return true;
    };
    let effective_groups = canonicalize_groups(effective_groups);

    if effective_groups.is_empty() {
        // effective_groups == []: only public providers eligible
        return channel_groups.is_empty();
    }

    // effective_groups is non-empty: public providers are NOT eligible
    if channel_groups.is_empty() {
        return false;
    }

    let effective_set: BTreeSet<_> = effective_groups.into_iter().collect();
    channel_groups
        .into_iter()
        .any(|group| effective_set.contains(&group))
}

/// Input for updating an existing API key
#[derive(Debug, Clone, Deserialize)]
pub struct UpdateApiKeyInput {
    pub name: Option<String>,
    pub enabled: Option<bool>,
    pub sub_account_enabled: Option<bool>,
    pub model_limits_enabled: Option<bool>,
    pub model_limits: Option<Vec<String>>,
    pub ip_whitelist: Option<Vec<String>>,
    pub allowed_groups: Option<Vec<String>>,
    pub max_multiplier: Option<f64>,
    pub transforms: Option<Vec<TransformRuleConfig>>,
    pub model_redirects: Option<Vec<ModelRedirectRule>>,
    pub expires_at: Option<String>, // RFC3339 format or null
}

#[derive(Clone)]
pub struct UserStore {
    pub(crate) db: DbPool,
    pub(crate) last_used_batcher: crate::db_cache::LastUsedBatcher,
    pub(crate) request_log_batcher: crate::db_cache::RequestLogBatcher,
    pub(crate) api_key_cache: crate::db_cache::ApiKeyCache,
    pub(crate) balance_cache: crate::db_cache::BalanceCache,
}

pub(crate) const RESERVED_INTERNAL_USER_PREFIX: &str = "_monoize_";

#[derive(Debug, Clone)]
pub struct InsertRequestLog {
    pub request_id: Option<String>,
    pub user_id: String,
    pub api_key_id: Option<String>,
    pub model: String,
    pub provider_id: Option<String>,
    pub upstream_model: Option<String>,
    pub channel_id: Option<String>,
    pub is_stream: bool,
    pub input_tokens: Option<u64>,
    pub output_tokens: Option<u64>,
    pub cache_read_tokens: Option<u64>,
    pub cache_creation_tokens: Option<u64>,
    pub tool_prompt_tokens: Option<u64>,
    pub reasoning_tokens: Option<u64>,
    pub accepted_prediction_tokens: Option<u64>,
    pub rejected_prediction_tokens: Option<u64>,
    pub provider_multiplier: Option<f64>,
    pub charge_nano_usd: Option<i128>,
    pub status: String,
    pub usage_breakdown_json: Option<Value>,
    pub billing_breakdown_json: Option<Value>,
    pub error_code: Option<String>,
    pub error_message: Option<String>,
    pub error_http_status: Option<u16>,
    pub duration_ms: Option<u64>,
    pub ttfb_ms: Option<u64>,
    pub request_ip: Option<String>,
    pub reasoning_effort: Option<String>,
    pub tried_providers_json: Option<Value>,
    pub request_kind: Option<String>,
    pub created_at: DateTime<Utc>,
}

pub const REQUEST_LOG_STATUS_PENDING: &str = "pending";
pub const REQUEST_LOG_STATUS_SUCCESS: &str = "success";
pub const REQUEST_LOG_STATUS_ERROR: &str = "error";

#[derive(Debug, Serialize)]
pub struct RequestLogProvider {
    pub id: Option<String>,
    pub name: Option<String>,
    pub multiplier: Option<f64>,
}

#[derive(Debug, Serialize)]
pub struct RequestLogChannel {
    pub id: Option<String>,
    pub name: Option<String>,
}

#[derive(Debug, Serialize)]
pub struct RequestLogUser {
    pub id: String,
    pub username: Option<String>,
}

#[derive(Debug, Serialize)]
pub struct RequestLogApiKey {
    pub id: Option<String>,
    pub name: Option<String>,
}

#[derive(Debug, Serialize)]
pub struct RequestLogTokens {
    pub input: Option<i64>,
    pub output: Option<i64>,
    pub cache_read: Option<i64>,
    pub cache_creation: Option<i64>,
    pub tool_prompt: Option<i64>,
    pub reasoning: Option<i64>,
    pub accepted_prediction: Option<i64>,
    pub rejected_prediction: Option<i64>,
}

#[derive(Debug, Serialize)]
pub struct RequestLogTiming {
    pub duration_ms: Option<i64>,
    pub ttfb_ms: Option<i64>,
    #[serde(rename = "durationMs")]
    pub duration_ms_alias: Option<i64>,
    pub elapsed_ms: Option<i64>,
    pub latency_ms: Option<i64>,
    #[serde(rename = "ttfbMs")]
    pub ttfb_ms_alias: Option<i64>,
    pub first_token_ms: Option<i64>,
    #[serde(rename = "firstTokenMs")]
    pub first_token_ms_alias: Option<i64>,
}

#[derive(Debug, Serialize)]
pub struct RequestLogBilling {
    pub charge_nano_usd: Option<String>,
    pub breakdown: Option<Value>,
}

#[derive(Debug, Serialize)]
pub struct RequestLogError {
    pub code: Option<String>,
    pub message: Option<String>,
    pub http_status: Option<i64>,
}

#[derive(Debug, Serialize)]
pub struct RequestLogRow {
    pub id: String,
    pub request_id: Option<String>,
    pub created_at: String,
    pub status: String,
    pub is_stream: bool,
    pub model: String,
    pub upstream_model: Option<String>,
    pub request_kind: Option<String>,
    pub reasoning_effort: Option<String>,
    pub request_ip: Option<String>,
    pub tried_providers: Option<Value>,
    pub provider: RequestLogProvider,
    pub channel: RequestLogChannel,
    pub user: RequestLogUser,
    pub api_key: RequestLogApiKey,
    pub tokens: RequestLogTokens,
    pub timing: RequestLogTiming,
    pub billing: RequestLogBilling,
    pub usage: Option<Value>,
    pub error: RequestLogError,
}

pub struct AnalyticsModelBucketRow {
    pub bucket_idx: i64,
    pub model: String,
    pub cost_nano: i64,
    pub call_count: i64,
}

pub struct AnalyticsProviderBucketRow {
    pub bucket_idx: i64,
    pub provider_label: String,
    pub call_count: i64,
}

pub struct DashboardAnalyticsRaw {
    pub model_buckets: Vec<AnalyticsModelBucketRow>,
    pub provider_buckets: Vec<AnalyticsProviderBucketRow>,
    pub total_cost_nano_usd: i64,
    pub total_calls: i64,
    pub today_cost_nano_usd: i64,
    pub today_calls: i64,
}

pub use utils::{format_nano_to_usd, parse_nano_usd, parse_usd_to_nano};

#[cfg(test)]
mod tests {
    use super::{
        ModelRedirectRule, canonicalize_groups, compute_effective_groups,
        is_channel_group_eligible, parse_groups_json, validate_model_redirects,
    };

    #[test]
    fn canonicalize_groups_trims_lowercases_deduplicates_and_sorts() {
        let groups = vec![
            " Beta ".to_string(),
            "alpha".to_string(),
            "ALPHA".to_string(),
            "   ".to_string(),
            "gamma".to_string(),
        ];

        assert_eq!(
            canonicalize_groups(&groups),
            vec!["alpha".to_string(), "beta".to_string(), "gamma".to_string(),]
        );
    }

    #[test]
    fn compute_effective_groups_distinguishes_unrestricted_from_public_only() {
        assert_eq!(compute_effective_groups(&[], &[]), None);
        assert_eq!(
            compute_effective_groups(&["Team-A".to_string()], &[]),
            Some(vec!["team-a".to_string()])
        );
        assert_eq!(
            compute_effective_groups(&[], &["Team-B".to_string()]),
            Some(vec!["team-b".to_string()])
        );
        assert_eq!(
            compute_effective_groups(
                &["Team-A".to_string()],
                &["team-b".to_string(), "TEAM-A".to_string()]
            ),
            Some(vec!["team-a".to_string()])
        );
        assert_eq!(
            compute_effective_groups(&["team-a".to_string()], &["team-b".to_string()]),
            Some(Vec::new())
        );
    }

    #[test]
    fn parse_groups_json_is_tolerant_and_canonical() {
        assert!(parse_groups_json("").is_empty());
        assert!(parse_groups_json("not-json").is_empty());
        assert_eq!(
            parse_groups_json(r#"[" Beta ","alpha","ALPHA",""]"#),
            vec!["alpha".to_string(), "beta".to_string()]
        );
    }

    #[test]
    fn channel_group_eligibility_respects_public_and_unrestricted_semantics() {
        // Unrestricted (None): all providers eligible
        assert!(is_channel_group_eligible(&["team-a".to_string()], &None));
        assert!(is_channel_group_eligible(&[], &None));

        // effective_groups == []: only public providers eligible
        assert!(is_channel_group_eligible(&[], &Some(Vec::new())));
        assert!(!is_channel_group_eligible(
            &["team-a".to_string()],
            &Some(Vec::new())
        ));

        // effective_groups non-empty: public providers excluded, matching groups eligible
        assert!(!is_channel_group_eligible(
            &[],
            &Some(vec!["team-a".to_string()])
        ));
        assert!(is_channel_group_eligible(
            &["TEAM-A".to_string()],
            &Some(vec!["team-a".to_string()])
        ));
        assert!(!is_channel_group_eligible(
            &["team-a".to_string()],
            &Some(vec!["team-b".to_string()])
        ));
    }

    #[test]
    fn validate_model_redirects_rejects_invalid_rules() {
        let too_many = (0..33)
            .map(|idx| ModelRedirectRule {
                pattern: format!("model-{idx}"),
                replace: "gpt-5.4".to_string(),
            })
            .collect::<Vec<_>>();
        assert_eq!(
            validate_model_redirects(&too_many).unwrap_err(),
            "too many model redirect rules (max 32)"
        );

        assert_eq!(
            validate_model_redirects(&[ModelRedirectRule {
                pattern: "   ".to_string(),
                replace: "gpt-5.4".to_string(),
            }])
            .unwrap_err(),
            "model redirect pattern must not be empty"
        );

        assert_eq!(
            validate_model_redirects(&[ModelRedirectRule {
                pattern: ".*opus.*".to_string(),
                replace: "   ".to_string(),
            }])
            .unwrap_err(),
            "model redirect replace must not be empty"
        );

        let err = validate_model_redirects(&[ModelRedirectRule {
            pattern: "(".to_string(),
            replace: "gpt-5.4".to_string(),
        }])
        .unwrap_err();
        assert!(err.starts_with("invalid model redirect pattern:"));
    }

    #[test]
    fn validate_model_redirects_accepts_valid_rules() {
        validate_model_redirects(&[
            ModelRedirectRule {
                pattern: ".*opus.*".to_string(),
                replace: "gpt-5.4".to_string(),
            },
            ModelRedirectRule {
                pattern: ".*haiku.*".to_string(),
                replace: "gpt-5.4-mini".to_string(),
            },
        ])
        .expect("valid redirects should pass");
    }
}
