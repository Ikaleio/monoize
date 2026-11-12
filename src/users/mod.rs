mod groups;
mod plans;
mod request_log_types;
mod request_logs;
mod store;
mod utils;

pub use groups::{CreateGroupInput, Group, GroupStoreError, ReorderGroupsInput, UpdateGroupInput};
pub(crate) use plans::PlanChargeRequest;
pub use plans::{
    BillingPlan, BillingPlanInput, BillingPlanPrice, BillingPlanPriceInput,
    BillingPlanSubscription, BillingPlanUsageWindows, PlanChargeAllocation, PlanWindowUsage,
};
pub use request_log_types::*;
pub use request_logs::{PendingRequestLogUsage, RequestLogFilters};
pub use store::UpdateUserInput;
pub(crate) use store::plan_charge_meta;

use crate::db::DbPool;
use crate::exact_decimal::Multiplier;
use crate::transforms::TransformRuleConfig;
use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};

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
    /// The user's single routing group id (`groups-registry.spec.md` §1.1).
    #[serde(default)]
    pub group_id: String,
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

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct ModelRedirectRule {
    pub pattern: String,
    pub replace: String,
}

#[derive(Debug, Clone)]
pub struct CompiledModelRedirectRule {
    regex: regex::Regex,
    replace: String,
}

impl CompiledModelRedirectRule {
    pub(crate) fn is_match(&self, model: &str) -> bool {
        self.regex.is_match(model)
    }

    pub(crate) fn replace(&self) -> &str {
        &self.replace
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ApiKey {
    pub id: String,
    pub user_id: String,
    pub name: String,
    pub key_prefix: String,
    /// The full API key, stored as plaintext and used for authentication and display.
    pub key: String,
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
    /// When true the key inherits the owner's single group at authentication time.
    #[serde(default = "default_true")]
    pub use_user_group: bool,
    /// Ordered group ids used when `use_user_group` is false; order is routing preference.
    #[serde(default)]
    pub group_ids: Vec<String>,
    /// Maximum accepted multiplier for routing
    #[serde(skip_serializing_if = "Option::is_none")]
    pub max_multiplier: Option<Multiplier>,
    /// Ordered transform rules applied for this API key
    #[serde(default)]
    pub transforms: Vec<TransformRuleConfig>,
    #[serde(default)]
    pub model_redirects: Vec<ModelRedirectRule>,
    #[serde(skip)]
    pub(crate) compiled_model_redirects: Vec<CompiledModelRedirectRule>,
    #[serde(default = "default_true")]
    pub reasoning_envelope_enabled: bool,
    #[serde(default)]
    pub request_capture_mode: RequestCaptureMode,
    #[serde(default)]
    pub request_capture_retention: RequestCaptureRetention,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(rename_all = "kebab-case")]
pub enum RequestCaptureMode {
    #[default]
    Off,
    CaptureAll,
    CaptureOnlyAbnormal,
}

/// Per-key capture TTL (`request-capture-dumps.spec.md` RCD-C5a through RCD-C5c).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
pub enum RequestCaptureRetention {
    #[serde(rename = "5m")]
    FiveMinutes,
    #[serde(rename = "1h")]
    OneHour,
    #[default]
    #[serde(rename = "24h")]
    OneDay,
    #[serde(rename = "7d")]
    SevenDays,
}

impl RequestCaptureRetention {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::FiveMinutes => "5m",
            Self::OneHour => "1h",
            Self::OneDay => "24h",
            Self::SevenDays => "7d",
        }
    }

    /// RCD-C5c duration mapping.
    pub fn duration_seconds(self) -> u64 {
        match self {
            Self::FiveMinutes => 300,
            Self::OneHour => 3_600,
            Self::OneDay => 86_400,
            Self::SevenDays => 604_800,
        }
    }
}

impl RequestCaptureMode {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Off => "off",
            Self::CaptureAll => "capture-all",
            Self::CaptureOnlyAbnormal => "capture-only-abnormal",
        }
    }

    pub fn should_start_capture(self) -> bool {
        !matches!(self, Self::Off)
    }

    pub fn should_persist(
        self,
        upstream_usage: Option<&crate::urp::Usage>,
        upstream_error_seen: bool,
        multiple_upstream_attempts: bool,
    ) -> bool {
        match self {
            Self::Off => false,
            Self::CaptureAll => true,
            Self::CaptureOnlyAbnormal => {
                upstream_error_seen
                    || multiple_upstream_attempts
                    || upstream_usage.is_none()
                    || upstream_usage.is_some_and(|usage| usage.total_tokens() == 0)
            }
        }
    }

    pub fn from_db_value(raw: &str) -> Self {
        match raw.trim() {
            "capture-all" => Self::CaptureAll,
            "capture-only-abnormal" => Self::CaptureOnlyAbnormal,
            _ => Self::Off,
        }
    }
}

/// Input for creating a new API key with extended fields
#[derive(Debug, Clone, Deserialize)]
pub struct CreateApiKeyInput {
    pub name: String,
    pub expires_in_days: Option<i64>,
    #[serde(default)]
    pub sub_account_enabled: bool,
    #[serde(default)]
    pub sub_account_balance_nano_usd: Option<String>,
    #[serde(default)]
    pub model_limits_enabled: bool,
    #[serde(default)]
    pub model_limits: Vec<String>,
    #[serde(default)]
    pub ip_whitelist: Vec<String>,
    #[serde(default = "default_true")]
    pub use_user_group: bool,
    #[serde(default)]
    pub group_ids: Vec<String>,
    #[serde(default)]
    pub max_multiplier: Option<Multiplier>,
    #[serde(default)]
    pub transforms: Vec<TransformRuleConfig>,
    #[serde(default)]
    pub model_redirects: Vec<ModelRedirectRule>,
    #[serde(default = "default_true")]
    pub reasoning_envelope_enabled: bool,
    #[serde(default)]
    pub request_capture_mode: RequestCaptureMode,
    #[serde(default)]
    pub request_capture_retention: RequestCaptureRetention,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum RegisterUserError {
    RegistrationDisabled,
    UsernameExists,
    Storage(String),
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum CreateApiKeyWithLimitError {
    LimitReached { limit: i64 },
    InvalidRequest(String),
}

#[derive(Debug, Clone, Default)]
pub struct AdminUpdateUserInput {
    pub username: Option<String>,
    pub password: Option<String>,
    pub role: Option<UserRole>,
    pub enabled: Option<bool>,
    pub balance_nano_usd: Option<String>,
    /// Signed amount added to the locked balance; mutually exclusive with `balance_nano_usd`.
    pub balance_delta_nano_usd: Option<String>,
    /// Compare-and-set guard for `balance_nano_usd`.
    pub expected_balance_nano_usd: Option<String>,
    /// User-visible note stored as `meta.note` on the `admin_adjustment` ledger row.
    pub balance_note: Option<String>,
    pub balance_unlimited: Option<bool>,
    pub email: Option<Option<String>>,
    pub group_id: Option<String>,
}

/// Failure of an admin balance mutation that carries an optimistic-concurrency guard.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum BalanceMutationError {
    /// The locked balance differs from the caller's expected value; nothing was written.
    BalanceChanged,
    Other(String),
}

impl From<String> for BalanceMutationError {
    fn from(error: String) -> Self {
        Self::Other(error)
    }
}

fn default_true() -> bool {
    true
}

pub const MAX_MODEL_REDIRECT_PATTERN_BYTES: usize = 256;

pub fn compile_model_redirects(
    rules: &[ModelRedirectRule],
) -> Result<Vec<CompiledModelRedirectRule>, String> {
    if rules.len() > 32 {
        return Err("too many model redirect rules (max 32)".to_string());
    }

    rules
        .iter()
        .map(|rule| {
            if rule.pattern.trim().is_empty() {
                return Err("model redirect pattern must not be empty".to_string());
            }
            if rule.replace.trim().is_empty() {
                return Err("model redirect replace must not be empty".to_string());
            }
            if rule.pattern.len() > MAX_MODEL_REDIRECT_PATTERN_BYTES {
                return Err(format!(
                    "model redirect pattern exceeds {MAX_MODEL_REDIRECT_PATTERN_BYTES} bytes"
                ));
            }

            let regex = regex::Regex::new(&format!("^(?:{})$", rule.pattern))
                .map_err(|e| format!("invalid model redirect pattern: {e}"))?;
            Ok(CompiledModelRedirectRule {
                regex,
                replace: rule.replace.clone(),
            })
        })
        .collect()
}

pub fn validate_model_redirects(rules: &[ModelRedirectRule]) -> Result<(), String> {
    compile_model_redirects(rules).map(|_| ())
}

/// GR-C1 canonicalization for group-id lists: trim each element, drop empties,
/// deduplicate preserving first-occurrence order. Ids are opaque — never
/// lowercased or sorted, because order is routing preference for API keys.
pub fn canonicalize_group_ids(group_ids: &[String]) -> Vec<String> {
    let mut canonical: Vec<String> = Vec::with_capacity(group_ids.len());
    for id in group_ids {
        let id = id.trim();
        if id.is_empty() || canonical.iter().any(|existing| existing == id) {
            continue;
        }
        canonical.push(id.to_string());
    }
    canonical
}

/// AKG5/AKG6 effective-group resolution for API-key authentication.
///
/// `base = [user_group_id]` when the key inherits the owner's group
/// (`use_user_group` true) or stores no explicit groups (GR-I3); otherwise the
/// key's ordered `group_ids`. An active subscription restricts this list by
/// membership without changing its order or adding groups.
pub fn resolve_effective_groups(
    user_group_id: &str,
    use_user_group: bool,
    key_group_ids: &[String],
    plan_group_ids: Option<&[String]>,
) -> Vec<String> {
    let base: Vec<String> = if use_user_group || key_group_ids.is_empty() {
        vec![user_group_id.to_string()]
    } else {
        key_group_ids.to_vec()
    };
    let mut groups = canonicalize_group_ids(&base);
    if let Some(plan_groups) = plan_group_ids {
        groups.retain(|id| plan_groups.contains(id));
    }
    groups
}

/// R-GRP-1 eligibility: `None` means internal system traffic (all providers
/// eligible); otherwise the provider's group-id set must intersect
/// `effective_groups`.
pub fn is_provider_group_eligible(
    provider_group_ids: &[String],
    effective_groups: &Option<Vec<String>>,
) -> bool {
    match effective_groups {
        None => true,
        Some(groups) => provider_group_ids.iter().any(|id| groups.contains(id)),
    }
}

/// R-GRP-2 group-order priority: the index of the first effective group served
/// by the provider. Callers must only rank group-eligible providers; a
/// non-matching provider ranks last as a defensive fallback.
pub fn provider_group_rank(
    provider_group_ids: &[String],
    effective_groups: &Option<Vec<String>>,
) -> usize {
    match effective_groups {
        None => 0,
        Some(groups) => groups
            .iter()
            .position(|id| provider_group_ids.contains(id))
            .unwrap_or(usize::MAX),
    }
}

/// Input for updating an existing API key
#[derive(Debug, Clone, Deserialize)]
pub struct UpdateApiKeyInput {
    pub name: Option<String>,
    pub enabled: Option<bool>,
    pub sub_account_enabled: Option<bool>,
    pub sub_account_balance_nano_usd: Option<String>,
    /// Compare-and-set guard for `sub_account_balance_nano_usd`.
    pub expected_sub_account_balance_nano_usd: Option<String>,
    pub model_limits_enabled: Option<bool>,
    pub model_limits: Option<Vec<String>>,
    pub ip_whitelist: Option<Vec<String>>,
    pub use_user_group: Option<bool>,
    pub group_ids: Option<Vec<String>>,
    pub max_multiplier: Option<Multiplier>,
    pub transforms: Option<Vec<TransformRuleConfig>>,
    pub model_redirects: Option<Vec<ModelRedirectRule>>,
    pub reasoning_envelope_enabled: Option<bool>,
    pub request_capture_mode: Option<RequestCaptureMode>,
    pub request_capture_retention: Option<RequestCaptureRetention>,
    /// `Some(None)` clears the expiry; `Some(Some(rfc3339))` sets it.
    pub expires_at: Option<Option<String>>,
}

#[derive(Clone)]
pub struct UserStore {
    pub(crate) db: DbPool,
    pub(crate) last_used_batcher: crate::db_cache::LastUsedBatcher,
    pub(crate) request_log_batcher: crate::db_cache::RequestLogBatcher,
    pub(crate) api_key_cache: crate::db_cache::ApiKeyCache,
    pub(crate) balance_cache: crate::db_cache::BalanceCache,
    /// DPT-BP2: sub-account balances keyed by API-key id.
    pub(crate) sub_account_balance_cache: crate::db_cache::BalanceCache,
    /// DPT-RC8: `group_id -> billing_ratio` for every registry row.
    pub(crate) group_ratio_cache:
        crate::db_cache::SnapshotCache<std::collections::HashMap<String, String>>,
    pub(crate) registration_lock: std::sync::Arc<tokio::sync::Mutex<()>>,
    pub(crate) api_key_creation_lock: std::sync::Arc<tokio::sync::Mutex<()>>,
    /// Enabled custom-transform snapshot used for CJS-AKV-2 rule checks.
    /// The default handle resolves nothing, which rejects every `js:` rule
    /// for non-admin callers.
    pub(crate) custom_transforms: crate::custom_transforms::CustomTransformSnapshotHandle,
}

pub(crate) const RESERVED_INTERNAL_USER_PREFIX: &str = "_monoize_";

pub use utils::{format_nano_to_usd, parse_nano_usd, parse_usd_to_nano};
