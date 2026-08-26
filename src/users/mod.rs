mod groups;
mod plans;
mod request_logs;
mod store;
mod utils;

pub use groups::{CreateGroupInput, Group, GroupStoreError, ReorderGroupsInput, UpdateGroupInput};
pub use plans::{BillingPlan, BillingPlanInput};

use crate::db::DbPool;
use crate::exact_decimal::Multiplier;
use crate::transforms::TransformRuleConfig;
use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use serde_json::Value;

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
    /// Assigned billing plan, if any. Referential integrity is enforced by write paths.
    #[serde(default)]
    pub billing_plan_id: Option<String>,
    /// Scheduled next balance grant time; present iff billing_plan_id is present.
    #[serde(default)]
    pub next_grant_at: Option<DateTime<Utc>>,
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
    ) -> bool {
        match self {
            Self::Off => false,
            Self::CaptureAll => true,
            Self::CaptureOnlyAbnormal => {
                upstream_error_seen
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
    pub balance_unlimited: Option<bool>,
    pub email: Option<Option<String>>,
    pub group_id: Option<String>,
    /// Outer Option = field present in the request; inner Option = target plan (None clears).
    pub billing_plan_id: Option<Option<String>>,
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
/// key's ordered `group_ids`. A present non-empty plan layer filters `base` by
/// membership while preserving `base` order. The result is always a concrete
/// ordered list; `None`-typed unrestricted access exists only for internal
/// system traffic and is never produced here.
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
    let filtered: Vec<String> = match plan_group_ids {
        Some(plan) if !plan.is_empty() => base
            .into_iter()
            .filter(|id| plan.iter().any(|allowed| allowed == id))
            .collect(),
        _ => base,
    };
    canonicalize_group_ids(&filtered)
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
    pub expires_at: Option<String>, // RFC3339 format or null
}

#[derive(Clone)]
pub struct UserStore {
    pub(crate) db: DbPool,
    pub(crate) last_used_batcher: crate::db_cache::LastUsedBatcher,
    pub(crate) request_log_batcher: crate::db_cache::RequestLogBatcher,
    pub(crate) api_key_cache: crate::db_cache::ApiKeyCache,
    pub(crate) balance_cache: crate::db_cache::BalanceCache,
    pub(crate) registration_lock: std::sync::Arc<tokio::sync::Mutex<()>>,
    pub(crate) api_key_creation_lock: std::sync::Arc<tokio::sync::Mutex<()>>,
    /// Enabled custom-transform snapshot used for CJS-AKV-2 rule checks.
    /// The default handle resolves nothing, which rejects every `js:` rule
    /// for non-admin callers.
    pub(crate) custom_transforms: crate::custom_transforms::CustomTransformSnapshotHandle,
}

pub(crate) const RESERVED_INTERNAL_USER_PREFIX: &str = "_monoize_";

#[derive(Debug, Clone, Default)]
pub struct RequestLogNameSnapshots {
    pub username: Option<String>,
    pub api_key_name: Option<String>,
    pub provider_name: Option<String>,
    pub channel_name: Option<String>,
}

#[derive(Debug, Clone)]
pub struct InsertRequestLog {
    pub request_id: Option<String>,
    pub user_id: String,
    pub api_key_id: Option<String>,
    pub model: String,
    pub provider_id: Option<String>,
    pub upstream_model: Option<String>,
    pub channel_id: Option<String>,
    pub names: RequestLogNameSnapshots,
    pub is_stream: bool,
    pub input_tokens: Option<u64>,
    pub output_tokens: Option<u64>,
    pub cache_read_tokens: Option<u64>,
    pub cache_creation_tokens: Option<u64>,
    pub tool_prompt_tokens: Option<u64>,
    pub reasoning_tokens: Option<u64>,
    pub accepted_prediction_tokens: Option<u64>,
    pub rejected_prediction_tokens: Option<u64>,
    pub provider_multiplier: Option<Multiplier>,
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
    pub effective_provider_type: Option<String>,
    pub affinity_hit: Option<bool>,
    pub affinity_key_hash: Option<String>,
    pub affinity_target: Option<String>,
    pub session_affinity_value: Option<String>,
    pub created_at: DateTime<Utc>,
}

pub const REQUEST_LOG_STATUS_PENDING: &str = "pending";
pub const REQUEST_LOG_STATUS_SUCCESS: &str = "success";
pub const REQUEST_LOG_STATUS_ERROR: &str = "error";
pub const REQUEST_LOG_STATUS_CLIENT_GONE: &str = "client_gone";

#[derive(Debug, Serialize)]
pub struct RequestLogProvider {
    pub id: Option<String>,
    pub name: Option<String>,
    pub multiplier: Option<Multiplier>,
}

#[derive(Debug, Serialize)]
pub struct RequestLogChannel {
    pub id: Option<String>,
    pub name: Option<String>,
}

#[derive(Debug, Serialize)]
pub struct RequestLogAffinity {
    pub hit: Option<bool>,
    pub key_hash: Option<String>,
    pub target: Option<String>,
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
    pub effective_provider_type: Option<String>,
    pub request_kind: Option<String>,
    pub reasoning_effort: Option<String>,
    pub request_ip: Option<String>,
    pub tried_providers: Option<Value>,
    pub session_affinity_value: Option<String>,
    pub provider: RequestLogProvider,
    pub channel: RequestLogChannel,
    pub affinity: RequestLogAffinity,
    pub user: RequestLogUser,
    pub api_key: RequestLogApiKey,
    pub tokens: RequestLogTokens,
    pub timing: RequestLogTiming,
    pub billing: RequestLogBilling,
    pub usage: Option<Value>,
    pub error: RequestLogError,
    /// RCV-L1/RCV-L2 (`request-capture-viewer.spec.md`): true iff a
    /// `request_capture_records` row matches this row's `(request_id, user_id)`.
    pub has_capture: bool,
}

impl RequestLogRow {
    /// SAN-14 (`spec/upstream-error-sanitization.spec.md`): replace the
    /// admin-tier error detail with its `MASK`ed form before serializing this
    /// row for a non-admin dashboard viewer. Operates on the in-memory copy
    /// only; the stored row is never modified.
    pub fn mask_error_detail_for_non_admin(&mut self) {
        if let Some(message) = self.error.message.as_deref() {
            self.error.message = Some(crate::error_sanitize::mask_sensitive_text(message));
        }
        let Some(Value::Array(items)) = self.tried_providers.as_mut() else {
            return;
        };
        for item in items {
            let Some(obj) = item.as_object_mut() else {
                continue;
            };
            if let Some(error_text) = obj.get("error").and_then(Value::as_str) {
                let masked = crate::error_sanitize::mask_sensitive_text(error_text);
                obj.insert("error".to_string(), Value::String(masked));
            }
        }
    }
}

impl InsertRequestLog {
    pub fn to_request_log_row(&self) -> RequestLogRow {
        RequestLogRow {
            id: self.request_id.clone().unwrap_or_default(),
            request_id: self.request_id.clone(),
            created_at: self.created_at.to_rfc3339(),
            status: self.status.clone(),
            is_stream: self.is_stream,
            model: self.model.clone(),
            upstream_model: self.upstream_model.clone(),
            effective_provider_type: self.effective_provider_type.clone(),
            request_kind: self.request_kind.clone(),
            reasoning_effort: self.reasoning_effort.clone(),
            request_ip: self.request_ip.clone(),
            tried_providers: self.tried_providers_json.clone(),
            session_affinity_value: self.session_affinity_value.clone(),
            // RCV-L3: SSE snapshots are emitted before capture persistence
            // completes, so live rows always report no capture.
            has_capture: false,
            provider: RequestLogProvider {
                id: self.provider_id.clone(),
                name: self.names.provider_name.clone(),
                multiplier: self.provider_multiplier,
            },
            channel: RequestLogChannel {
                id: self.channel_id.clone(),
                name: self.names.channel_name.clone(),
            },
            affinity: RequestLogAffinity {
                hit: self.affinity_hit,
                key_hash: self.affinity_key_hash.clone(),
                target: self.affinity_target.clone(),
            },
            user: RequestLogUser {
                id: self.user_id.clone(),
                username: self.names.username.clone(),
            },
            api_key: RequestLogApiKey {
                id: self.api_key_id.clone(),
                name: self.names.api_key_name.clone(),
            },
            tokens: RequestLogTokens {
                input: self.input_tokens.and_then(|v| i64::try_from(v).ok()),
                output: self.output_tokens.and_then(|v| i64::try_from(v).ok()),
                cache_read: self.cache_read_tokens.and_then(|v| i64::try_from(v).ok()),
                cache_creation: self
                    .cache_creation_tokens
                    .and_then(|v| i64::try_from(v).ok()),
                tool_prompt: self.tool_prompt_tokens.and_then(|v| i64::try_from(v).ok()),
                reasoning: self.reasoning_tokens.and_then(|v| i64::try_from(v).ok()),
                accepted_prediction: self
                    .accepted_prediction_tokens
                    .and_then(|v| i64::try_from(v).ok()),
                rejected_prediction: self
                    .rejected_prediction_tokens
                    .and_then(|v| i64::try_from(v).ok()),
            },
            timing: RequestLogTiming {
                duration_ms: self.duration_ms.and_then(|v| i64::try_from(v).ok()),
                ttfb_ms: self.ttfb_ms.and_then(|v| i64::try_from(v).ok()),
                duration_ms_alias: self.duration_ms.and_then(|v| i64::try_from(v).ok()),
                elapsed_ms: self.duration_ms.and_then(|v| i64::try_from(v).ok()),
                latency_ms: self.duration_ms.and_then(|v| i64::try_from(v).ok()),
                ttfb_ms_alias: self.ttfb_ms.and_then(|v| i64::try_from(v).ok()),
                first_token_ms: self.ttfb_ms.and_then(|v| i64::try_from(v).ok()),
                first_token_ms_alias: self.ttfb_ms.and_then(|v| i64::try_from(v).ok()),
            },
            billing: RequestLogBilling {
                charge_nano_usd: self.charge_nano_usd.map(|v| v.to_string()),
                breakdown: self.billing_breakdown_json.clone(),
            },
            usage: self.usage_breakdown_json.clone(),
            error: RequestLogError {
                code: self.error_code.clone(),
                message: self.error_message.clone(),
                http_status: self.error_http_status.map(i64::from),
            },
        }
    }
}

pub struct AnalyticsModelBucketRow {
    pub bucket_idx: i64,
    pub model: String,
    pub cost_nano: i128,
    pub call_count: i64,
    pub token_count: i64,
}

pub struct AnalyticsProviderBucketRow {
    pub bucket_idx: i64,
    pub provider_label: String,
    pub call_count: i64,
}

pub struct DashboardAnalyticsRaw {
    pub model_buckets: Vec<AnalyticsModelBucketRow>,
    pub provider_buckets: Vec<AnalyticsProviderBucketRow>,
    pub total_cost_nano_usd: i128,
    pub total_calls: i64,
    pub today_cost_nano_usd: i128,
    pub today_calls: i64,
}

pub struct PerformanceHourBucketRow {
    pub hour_idx: i64,
    pub finished_count: i64,
    pub success_count: i64,
}

pub struct PerformanceTargetRaw {
    pub hour_buckets: Vec<PerformanceHourBucketRow>,
    pub avg_ttft_ms: Option<f64>,
    pub avg_tps: Option<f64>,
}

#[derive(Debug, Clone)]
pub struct UserTodayUsage {
    pub user_id: String,
    pub today_calls: i64,
    pub today_cost_nano_usd: i128,
}

#[derive(Debug, Clone)]
pub struct UserUsageRankingRow {
    pub user_id: String,
    pub username: Option<String>,
    pub call_count: i64,
    pub cost_nano_usd: i128,
}

#[derive(Debug, Clone)]
pub struct ChannelTodayUsage {
    pub channel_id: String,
    pub today_calls: i64,
    pub today_cost_nano_usd: i128,
}

/// Live-usage aggregate window (`user-live-usage.spec.md` LU-4).
pub const LIVE_USAGE_WINDOW_SECONDS: i64 = 60;

/// Rolling 60-second per-user request-log aggregate
/// (`user-live-usage.spec.md` LU-6).
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct UserLiveUsage {
    pub rpm: i64,
    pub input_tokens: i64,
    pub output_tokens: i64,
    pub cache_read_tokens: i64,
}

pub use utils::{format_nano_to_usd, parse_nano_usd, parse_usd_to_nano};

#[cfg(test)]
mod tests {
    use super::{
        MAX_MODEL_REDIRECT_PATTERN_BYTES, ModelRedirectRule, canonicalize_group_ids,
        is_provider_group_eligible, provider_group_rank, resolve_effective_groups,
        validate_model_redirects,
    };

    fn ids(values: &[&str]) -> Vec<String> {
        values.iter().map(|value| (*value).to_string()).collect()
    }

    #[test]
    fn canonicalize_group_ids_trims_dedupes_and_preserves_order() {
        assert_eq!(
            canonicalize_group_ids(&ids(&[" g-2 ", "g-1", "g-2", "", "   ", "g-3"])),
            ids(&["g-2", "g-1", "g-3"])
        );
        // Ids are opaque: casing must survive.
        assert_eq!(canonicalize_group_ids(&ids(&["G-1"])), ids(&["G-1"]));
    }

    #[test]
    fn resolve_effective_groups_follows_akg5() {
        // use_user_group => owner's single group.
        assert_eq!(
            resolve_effective_groups("g-user", true, &ids(&["g-1", "g-2"]), None),
            ids(&["g-user"])
        );
        // Explicit empty group_ids resolves like use_user_group (GR-I3).
        assert_eq!(
            resolve_effective_groups("g-user", false, &[], None),
            ids(&["g-user"])
        );
        // Explicit ordered selection preserves order.
        assert_eq!(
            resolve_effective_groups("g-user", false, &ids(&["g-2", "g-1"]), None),
            ids(&["g-2", "g-1"])
        );
        // Non-empty plan layer filters by membership in base order.
        assert_eq!(
            resolve_effective_groups(
                "g-user",
                false,
                &ids(&["g-2", "g-1", "g-3"]),
                Some(&ids(&["g-3", "g-2"]))
            ),
            ids(&["g-2", "g-3"])
        );
        // Empty plan layer is unrestricted.
        assert_eq!(
            resolve_effective_groups("g-user", true, &[], Some(&[])),
            ids(&["g-user"])
        );
        // Plan ceiling can exclude every base group.
        assert_eq!(
            resolve_effective_groups("g-user", true, &[], Some(&ids(&["g-other"]))),
            Vec::<String>::new()
        );
    }

    #[test]
    fn provider_group_eligibility_and_rank_follow_r_grp_rules() {
        // None = internal traffic: everything eligible at rank 0.
        assert!(is_provider_group_eligible(&ids(&["g-1"]), &None));
        assert_eq!(provider_group_rank(&ids(&["g-1"]), &None), 0);

        // Empty effective groups: nothing eligible (R-GRP-1a).
        assert!(!is_provider_group_eligible(
            &ids(&["g-1"]),
            &Some(Vec::new())
        ));

        let effective = Some(ids(&["g-2", "g-1"]));
        assert!(is_provider_group_eligible(&ids(&["g-1"]), &effective));
        assert!(!is_provider_group_eligible(&ids(&["g-9"]), &effective));

        // Rank = earliest matching index in effective order (R-GRP-2).
        assert_eq!(provider_group_rank(&ids(&["g-1"]), &effective), 1);
        assert_eq!(provider_group_rank(&ids(&["g-1", "g-2"]), &effective), 0);
        assert_eq!(provider_group_rank(&ids(&["g-9"]), &effective), usize::MAX);
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

        assert_eq!(
            validate_model_redirects(&[ModelRedirectRule {
                pattern: "a".repeat(MAX_MODEL_REDIRECT_PATTERN_BYTES + 1),
                replace: "gpt-5.4".to_string(),
            }])
            .unwrap_err(),
            "model redirect pattern exceeds 256 bytes"
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

    // SAN-14: the non-admin read-time mask rewrites `error.message` and every
    // `tried_providers[].error` while leaving all other row fields untouched.
    #[test]
    fn non_admin_mask_rewrites_error_message_and_tried_provider_errors() {
        let raw = "upstream status 502 Bad Gateway: connect to https://api.cloudflare.com/client/v4/accounts/ebb3b05a7371fbcbd62bde8264c86cfe/ai failed via 10.32.4.17";
        let mut row = super::InsertRequestLog {
            request_id: Some("req-mask".to_string()),
            user_id: "user-1".to_string(),
            api_key_id: None,
            model: "gpt-5-mini".to_string(),
            provider_id: None,
            upstream_model: None,
            channel_id: None,
            names: super::RequestLogNameSnapshots::default(),
            is_stream: false,
            input_tokens: None,
            output_tokens: None,
            cache_read_tokens: None,
            cache_creation_tokens: None,
            tool_prompt_tokens: None,
            reasoning_tokens: None,
            accepted_prediction_tokens: None,
            rejected_prediction_tokens: None,
            provider_multiplier: None,
            charge_nano_usd: None,
            status: super::REQUEST_LOG_STATUS_ERROR.to_string(),
            usage_breakdown_json: None,
            billing_breakdown_json: None,
            error_code: Some("upstream_error".to_string()),
            error_message: Some(raw.to_string()),
            error_http_status: Some(502),
            duration_ms: None,
            ttfb_ms: None,
            request_ip: None,
            reasoning_effort: None,
            tried_providers_json: Some(serde_json::json!([
                { "attempt_number": 1, "provider_id": "p1", "channel_id": "c1", "error": raw },
                { "attempt_number": 2, "provider_id": "p2", "channel_id": "c2", "error": "plain failure" }
            ])),
            request_kind: None,
            effective_provider_type: None,
            affinity_hit: None,
            affinity_key_hash: None,
            affinity_target: None,
            session_affinity_value: None,
            created_at: chrono::Utc::now(),
        }
        .to_request_log_row();

        row.mask_error_detail_for_non_admin();

        let masked_message = row.error.message.as_deref().expect("error message");
        assert!(!masked_message.contains("cloudflare"), "{masked_message}");
        assert!(
            !masked_message.contains("ebb3b05a7371fbcbd62bde8264c86cfe"),
            "{masked_message}"
        );
        assert!(!masked_message.contains("10.32.4.17"), "{masked_message}");
        assert!(
            masked_message.contains("https://***.com/***"),
            "{masked_message}"
        );

        let tried = row.tried_providers.as_ref().expect("tried providers");
        let first_error = tried[0]["error"].as_str().expect("first hop error");
        assert!(!first_error.contains("cloudflare"), "{first_error}");
        assert!(first_error.contains("https://***.com/***"), "{first_error}");
        assert_eq!(tried[1]["error"], serde_json::json!("plain failure"));
        assert_eq!(tried[0]["provider_id"], serde_json::json!("p1"));
        assert_eq!(row.error.code.as_deref(), Some("upstream_error"));
        assert_eq!(row.error.http_status, Some(502));
    }
}
