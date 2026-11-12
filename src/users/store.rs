use super::utils::parse_nano_usd;
use super::{
    AdminUpdateUserInput, ApiKey, BalanceMutationError, BillingError, BillingErrorKind,
    CreateApiKeyInput, CreateApiKeyWithLimitError, ModelRedirectRule, PlanChargeAllocation,
    RESERVED_INTERNAL_USER_PREFIX, RegisterUserError, RequestCaptureMode, RequestCaptureRetention,
    Session, UpdateApiKeyInput, User, UserBalance, UserRole, UserStore, canonicalize_group_ids,
    compile_model_redirects, validate_model_redirects,
};
use crate::exact_decimal::Multiplier;
use crate::transforms::{
    TransformRuleConfig, canonical_transform_id, canonicalize_transform_rule,
    canonicalize_transform_rules,
};
use argon2::{
    Argon2,
    password_hash::{PasswordHash, PasswordHasher, PasswordVerifier, SaltString, rand_core::OsRng},
};
use chrono::{DateTime, Utc};
use sea_orm::Value as SeaValue;
use sea_orm::{ConnectionTrait, DatabaseTransaction, QueryResult, TransactionTrait};
use serde::de::DeserializeOwned;
use serde_json::Value;
use std::collections::{BTreeMap, BTreeSet};
use std::net::IpAddr;
use std::sync::{Arc, OnceLock};

const MAX_FORWARDING_API_KEY_BYTES: usize = 512;
const DEFAULT_API_KEY_BATCH_DELETE_MAX_IDS: usize = 400;
const DEFAULT_SESSION_CLEANUP_INTERVAL_SECS: u64 = 3_600;

fn validate_positive_max_multiplier(value: Option<Multiplier>) -> Result<(), String> {
    if value.is_some_and(|multiplier| !multiplier.is_positive()) {
        return Err("max_multiplier must be greater than zero".to_string());
    }
    Ok(())
}

fn parse_positive_limit(raw: Option<&str>, default: usize) -> usize {
    raw.and_then(|value| value.trim().parse::<usize>().ok())
        .filter(|value| *value > 0)
        .unwrap_or(default)
}

fn parse_api_key_batch_delete_limit(raw: Option<&str>) -> usize {
    parse_positive_limit(raw, DEFAULT_API_KEY_BATCH_DELETE_MAX_IDS)
        .min(DEFAULT_API_KEY_BATCH_DELETE_MAX_IDS)
}

pub(crate) fn plan_charge_meta(meta: &Value, allocation: &PlanChargeAllocation) -> Value {
    let mut object = meta.as_object().cloned().unwrap_or_default();
    object.insert(
        "charge_nano_usd".to_string(),
        Value::String(allocation.adjusted_charge_nano_usd.to_string()),
    );
    object.insert(
        "plan_covered_nano_usd".to_string(),
        Value::String(allocation.plan_covered_nano_usd.to_string()),
    );
    object.insert(
        "fallback_nano_usd".to_string(),
        Value::String(allocation.fallback_nano_usd.to_string()),
    );
    object.insert(
        "subscription_id".to_string(),
        allocation
            .subscription_id
            .as_ref()
            .map_or(Value::Null, |value| Value::String(value.clone())),
    );
    object.insert(
        "plan_id".to_string(),
        allocation
            .plan_id
            .as_ref()
            .map_or(Value::Null, |value| Value::String(value.clone())),
    );
    object.insert(
        "plan_multiplier".to_string(),
        allocation
            .multiplier
            .as_ref()
            .map_or(Value::Null, |value| Value::String(value.clone())),
    );
    Value::Object(object)
}

fn api_key_batch_delete_max_ids() -> usize {
    static LIMIT: OnceLock<usize> = OnceLock::new();
    *LIMIT.get_or_init(|| {
        parse_api_key_batch_delete_limit(
            std::env::var("MONOIZE_API_KEY_BATCH_DELETE_MAX_IDS")
                .ok()
                .as_deref(),
        )
    })
}

fn parse_session_cleanup_interval_secs(raw: Option<&str>) -> u64 {
    raw.and_then(|value| value.trim().parse::<u64>().ok())
        .filter(|value| *value > 0)
        .unwrap_or(DEFAULT_SESSION_CLEANUP_INTERVAL_SECS)
}

fn session_cleanup_interval() -> std::time::Duration {
    static INTERVAL: OnceLock<std::time::Duration> = OnceLock::new();
    *INTERVAL.get_or_init(|| {
        std::time::Duration::from_secs(parse_session_cleanup_interval_secs(
            std::env::var("MONOIZE_SESSION_CLEANUP_INTERVAL_SECONDS")
                .ok()
                .as_deref(),
        ))
    })
}

fn canonicalize_ip_whitelist(entries: &[String]) -> Result<Vec<String>, String> {
    let mut canonical = BTreeSet::new();
    for entry in entries {
        let value = entry.trim();
        if value.is_empty() {
            return Err("ip_whitelist entries must not be empty".to_string());
        }
        let normalized = if let Ok(ip) = value.parse::<IpAddr>() {
            ip.to_string()
        } else if let Ok(network) = value.parse::<ipnet::IpNet>() {
            network.to_string()
        } else {
            return Err(format!("invalid ip_whitelist entry: {value}"));
        };
        canonical.insert(normalized);
    }
    Ok(canonical.into_iter().collect())
}

const ALLOWED_API_KEY_REQUEST_TRANSFORMS: &[&str] = &[
    "prompt_inject_system",
    "role_system_to_developer",
    "role_merge_consecutive",
    "prompt_append_empty_user",
    "image_compress_input",
    "image_enable_openai_generation_tool",
    "prompt_strip_anthropic_billing_header",
    "cache_anthropic_system",
    "cache_anthropic_auto",
    "cache_anthropic_tool_use",
    "cache_openai_tool_use",
    "cache_user_id",
    "cache_openai_prompt",
    "field_alias_reserved_tool_names",
    "field_custom_tools_to_function",
];

const ALLOWED_API_KEY_RESPONSE_TRANSFORMS: &[&str] = &[
    "reasoning_strip_output",
    "reasoning_strip_encrypted",
    "reasoning_to_think_xml",
    "reasoning_from_think_xml",
    "stream_split_sse_frames",
    "reasoning_content_to_summary",
    "reasoning_inject_content_field",
    "reasoning_summary_to_raw_cot",
    "reasoning_summary_heading",
    "image_markdown_to_output",
    "image_output_to_markdown",
    "image_compress_output",
    "field_alias_reserved_tool_names",
    "field_custom_tools_to_function",
];

#[derive(Clone, Copy)]
pub(crate) struct LockedUserBalance {
    pub(crate) balance: i128,
    pub(crate) unlimited: bool,
}

struct LockedApiKeyBalance {
    user_id: String,
    balance: i128,
    sub_account_enabled: bool,
}

pub(crate) fn is_allowed_api_key_transform(
    rule: &TransformRuleConfig,
    custom: &crate::custom_transforms::CustomTransformSnapshot,
) -> bool {
    let transform = canonical_transform_id(rule.transform.as_str());
    // CJS-AKV-2: a `js:` rule is allowed only when it resolves in the enabled
    // snapshot to a user-visible transform whose scopes include api_key and
    // whose declared phases include the rule phase.
    if transform.starts_with(crate::transforms::CUSTOM_TRANSFORM_ID_PREFIX) {
        return custom.get(transform).is_some_and(|entry| {
            entry.visibility == crate::custom_transforms::CustomTransformVisibility::User
                && entry
                    .scopes
                    .contains(&crate::transforms::TransformScope::ApiKey)
                && entry.phases.contains(&rule.phase)
        });
    }
    match rule.phase {
        crate::transforms::Phase::Request => {
            ALLOWED_API_KEY_REQUEST_TRANSFORMS.contains(&transform)
        }
        crate::transforms::Phase::Response => {
            ALLOWED_API_KEY_RESPONSE_TRANSFORMS.contains(&transform)
        }
    }
}

pub(crate) fn sanitize_api_key_transforms(
    transforms: Vec<TransformRuleConfig>,
    is_admin: bool,
    custom: &crate::custom_transforms::CustomTransformSnapshot,
) -> Vec<TransformRuleConfig> {
    let transforms: Vec<TransformRuleConfig> = transforms
        .into_iter()
        .map(|mut rule| {
            canonicalize_transform_rule(&mut rule);
            rule
        })
        .collect();
    if is_admin {
        return transforms;
    }
    transforms
        .into_iter()
        .filter(|rule| is_allowed_api_key_transform(rule, custom))
        .collect()
}

pub(crate) fn validate_api_key_transforms(
    transforms: &[TransformRuleConfig],
    is_admin: bool,
    custom: &crate::custom_transforms::CustomTransformSnapshot,
) -> Result<(), String> {
    if is_admin {
        return Ok(());
    }
    for rule in transforms {
        let mut canonical_rule = rule.clone();
        canonicalize_transform_rule(&mut canonical_rule);
        if !is_allowed_api_key_transform(&canonical_rule, custom) {
            return Err(format!(
                "transform '{}' is not allowed for API keys",
                rule.transform
            ));
        }
    }
    Ok(())
}

fn parse_persisted_json_array<T>(raw: &str, column: &str) -> Result<Vec<T>, String>
where
    T: DeserializeOwned,
{
    serde_json::from_str(raw).map_err(|error| format!("invalid persisted {column}: {error}"))
}

/// GR-C4 stored group-id decoding: absent, null, empty string, or a serialized
/// empty array decode as `[]`; any other malformed value fails the read.
pub(crate) fn parse_group_ids_json(raw: Option<&str>, column: &str) -> Result<Vec<String>, String> {
    let Some(raw) = raw else {
        return Ok(Vec::new());
    };
    if raw.trim().is_empty() {
        return Ok(Vec::new());
    }

    let group_ids = serde_json::from_str::<Option<Vec<String>>>(raw)
        .map_err(|error| format!("invalid persisted {column}: {error}"))?
        .unwrap_or_default();
    Ok(canonicalize_group_ids(&group_ids))
}

pub(crate) fn decode_required_bool(row: &QueryResult, column: &str) -> Result<bool, String> {
    let value = row
        .try_get::<i32>("", column)
        .map_err(|error| format!("invalid persisted {column}: {error}"))?;
    match value {
        0 => Ok(false),
        1 => Ok(true),
        _ => Err(format!(
            "invalid persisted {column}: expected integer 0 or 1, got {value}"
        )),
    }
}

fn decode_request_capture_mode(row: &QueryResult) -> Result<RequestCaptureMode, String> {
    let raw = row
        .try_get::<Option<String>>("", "request_capture_mode")
        .map_err(|error| format!("invalid persisted request_capture_mode: {error}"))?;
    match raw.as_deref().map(str::trim) {
        None => Ok(RequestCaptureMode::Off),
        Some("off") => Ok(RequestCaptureMode::Off),
        Some("capture-all") => Ok(RequestCaptureMode::CaptureAll),
        Some("capture-only-abnormal") => Ok(RequestCaptureMode::CaptureOnlyAbnormal),
        Some(value) => Err(format!(
            "invalid persisted request_capture_mode: unsupported value {value:?}"
        )),
    }
}

/// TM-STORAGE-7: strict retention decode; absent/null reads as the `24h`
/// default (RCD-C5b), any other value fails the read.
fn decode_request_capture_retention(row: &QueryResult) -> Result<RequestCaptureRetention, String> {
    let raw = row
        .try_get::<Option<String>>("", "request_capture_retention")
        .map_err(|error| format!("invalid persisted request_capture_retention: {error}"))?;
    match raw.as_deref().map(str::trim) {
        None => Ok(RequestCaptureRetention::OneDay),
        Some("5m") => Ok(RequestCaptureRetention::FiveMinutes),
        Some("1h") => Ok(RequestCaptureRetention::OneHour),
        Some("24h") => Ok(RequestCaptureRetention::OneDay),
        Some("7d") => Ok(RequestCaptureRetention::SevenDays),
        Some(value) => Err(format!(
            "invalid persisted request_capture_retention: unsupported value {value:?}"
        )),
    }
}

impl UserStore {
    pub fn api_key_batch_delete_max_ids() -> usize {
        api_key_batch_delete_max_ids()
    }
}

pub(crate) fn serialize_group_ids_json(group_ids: &[String]) -> Result<String, String> {
    serde_json::to_string(&canonicalize_group_ids(group_ids)).map_err(|e| e.to_string())
}

pub(crate) const MAX_GROUP_IDS: usize = 32;

impl UserStore {
    /// TM-GRP-3/TM-GRP-5 validation for an already canonicalized group-id list:
    /// bounded length, every id registered, and non-admin callers limited to
    /// `user_selectable` groups plus the owner's own current group.
    pub(crate) async fn validate_api_key_group_selection(
        &self,
        owner_group_id: &str,
        group_ids: &[String],
        is_admin: bool,
    ) -> Result<(), String> {
        if group_ids.len() > MAX_GROUP_IDS {
            return Err(format!("at most {MAX_GROUP_IDS} groups can be selected"));
        }
        for id in group_ids {
            let group = self
                .get_group_by_id(id)
                .await?
                .ok_or_else(|| format!("unknown group id: {id}"))?;
            if !is_admin && !group.user_selectable && id != owner_group_id {
                return Err(format!("group is not selectable: {}", group.name));
            }
        }
        Ok(())
    }
}

mod admin;
mod api_key_deletion;
mod api_key_rows;
mod api_key_update;
mod api_keys;
mod balances;
mod lifecycle;
mod migration;
mod sessions;
mod sub_accounts;
mod users;

pub(crate) use balances::BillingLedgerEntry;
pub use users::UpdateUserInput;
