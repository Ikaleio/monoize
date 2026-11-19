use crate::db::DbPool;
use crate::entity::system_settings;
use crate::monoize_routing::AffinityFailbackMode;
use crate::transforms::{TransformRuleConfig, canonicalize_transform_rules};
use crate::users::{ModelRedirectRule, validate_model_redirects};
use chrono::{DateTime, Utc};
use sea_orm::{ColumnTrait, ConnectionTrait, EntityTrait, QueryFilter, Set, sea_query::OnConflict};
use serde::{Deserialize, Serialize};
use std::collections::{HashMap, HashSet};

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct PublicSettings {
    pub registration_enabled: bool,
    pub captcha_enabled: bool,
    pub site_name: String,
    pub site_description: String,
    pub api_base_url: String,
    pub cap_api_endpoint: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SystemSettings {
    pub registration_enabled: bool,
    pub captcha_enabled: bool,
    pub session_ttl_days: i64,
    pub api_key_max_per_user: i64,
    pub site_name: String,
    pub site_description: String,
    pub api_base_url: String,
    #[serde(default)]
    pub global_transforms: Vec<TransformRuleConfig>,
    #[serde(default)]
    pub global_model_redirects: Vec<ModelRedirectRule>,
    pub reasoning_suffix_map: HashMap<String, String>,
    #[serde(default)]
    pub codex_model_ids: Vec<String>,
    #[serde(default)]
    pub dashboard_performance_group_ids: Vec<String>,
    #[serde(default)]
    pub dashboard_performance_model_ids: Vec<String>,
    pub monoize_honor_rate_limit: bool,
    #[serde(default = "default_failure_classification_rules")]
    pub monoize_failure_classification_rules: Vec<FailureClassificationRule>,
    pub monoize_active_probe_interval_seconds: u64,
    pub monoize_active_probe_success_threshold: u32,
    pub monoize_active_probe_model: Option<String>,
    pub monoize_active_probe_stream: bool,
    pub monoize_passive_failure_threshold: u32,
    pub monoize_passive_cooldown_seconds: u64,
    pub monoize_passive_window_seconds: u64,
    pub monoize_passive_rate_limit_cooldown_seconds: u64,
    pub monoize_request_timeout_ms: u64,
    pub monoize_stream_idle_timeout_ms: u64,
    pub monoize_enable_estimated_billing: bool,
    #[serde(default)]
    pub monoize_extra_fields_whitelist: HashMap<String, Vec<String>>,
    #[serde(default = "default_true")]
    pub monoize_strip_cross_protocol_nested_extra: bool,
    pub monoize_request_capture_enabled: bool,
    pub monoize_request_capture_max_total_bytes: u64,
    #[serde(default = "default_true")]
    pub monoize_mask_sensitive_info: bool,
    pub monoize_affinity_enabled: bool,
    pub monoize_affinity_idle_ttl_seconds: u64,
    pub monoize_affinity_failback_mode: AffinityFailbackMode,
    pub monoize_affinity_failback_delay_seconds: u64,
    /// MP-D13/MP-D14 (`model-pricing.spec.md`): fail-closed free-settlement flags.
    #[serde(default)]
    pub allow_free_when_unpriced: bool,
    #[serde(default)]
    pub allow_free_when_missing_usage: bool,
    /// MP-T1..MP-T3: server-native tool pricing object.
    #[serde(default = "default_tool_prices")]
    pub tool_prices: serde_json::Value,
    /// MP-Y2: `new_api` price-sync source configuration.
    #[serde(default)]
    pub price_sync_new_api_base_url: String,
    #[serde(default)]
    pub price_sync_new_api_token: String,
    /// MP-Y19..MP-Y24: primary-side daily models.dev and OpenRouter sync.
    #[serde(default = "default_true")]
    pub price_sync_auto_enabled: bool,
    /// RC-G1 (`recharge-system.spec.md`): public origin for payment callbacks.
    #[serde(default)]
    pub recharge_public_origin: String,
    pub updated_at: DateTime<Utc>,
}

pub const BUILTIN_REASONING_EFFORT_SUFFIXES: &[(&str, &str)] = &[
    ("-none", "none"),
    ("-minimum", "minimum"),
    ("-low", "low"),
    ("-medium", "medium"),
    ("-high", "high"),
    ("-xhigh", "xhigh"),
    ("-max", "max"),
];

pub(crate) const FAILURE_CLASSIFICATION_RULES_KEY: &str = "monoize_failure_classification_rules";

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum RetryableFailureClass {
    RateLimited,
    Transient,
    Persistent,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct FailureClassificationRule {
    pub status_codes: Vec<u16>,
    pub keyword: String,
    pub class: RetryableFailureClass,
}

/// CFG-4b defaults, in order: key-level relay conditions whose HTTP status
/// would otherwise classify as Transient or RateLimited.
const DEFAULT_PERSISTENT_RULES: [(Option<u16>, &str); 9] = [
    (Some(503), "no available account"),
    (None, "no credits remaining"),
    (Some(503), "no available gemini accounts"),
    (Some(503), "no available openai accounts"),
    (Some(503), "no available grok accounts"),
    (Some(503), "no available compatible accounts"),
    (Some(502), "upstream authentication failed"),
    (Some(502), "upstream access forbidden"),
    (Some(429), "额度已用完"),
];

pub(crate) fn default_failure_classification_rules() -> Vec<FailureClassificationRule> {
    DEFAULT_PERSISTENT_RULES
        .iter()
        .map(|&(status, keyword)| FailureClassificationRule {
            status_codes: status.into_iter().collect(),
            keyword: keyword.to_owned(),
            class: RetryableFailureClass::Persistent,
        })
        .collect()
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct CompiledFailureRules(std::sync::Arc<[FailureClassificationRule]>);

impl Default for CompiledFailureRules {
    fn default() -> Self {
        Self::new(default_failure_classification_rules())
    }
}

impl CompiledFailureRules {
    pub(crate) fn new(mut rules: Vec<FailureClassificationRule>) -> Self {
        for rule in &mut rules {
            if rule.keyword.is_ascii() {
                rule.keyword.make_ascii_lowercase();
            } else {
                rule.keyword = simple_lowercase(&rule.keyword);
            }
        }
        Self(rules.into())
    }

    pub(crate) fn classify(
        &self,
        http_status: Option<u16>,
        fields: [Option<&str>; 3],
    ) -> Option<RetryableFailureClass> {
        let mut lowered_fields = [const { std::cell::OnceCell::new() }; 3];
        self.0.iter().find_map(|rule| {
            (rule.status_codes.is_empty()
                || http_status.is_some_and(|status| rule.status_codes.contains(&status)))
            .then_some(())?;
            fields
                .iter()
                .zip(&mut lowered_fields)
                .any(|(field, lowered)| {
                    let Some(field) = field else {
                        return false;
                    };
                    lowered
                        .get_or_init(|| {
                            if field
                                .bytes()
                                .all(|byte| byte.is_ascii() && !byte.is_ascii_uppercase())
                            {
                                std::borrow::Cow::<str>::Borrowed(field)
                            } else {
                                std::borrow::Cow::Owned(simple_lowercase(field))
                            }
                        })
                        .contains(&rule.keyword)
                })
                .then_some(rule.class)
        })
    }
}

fn simple_lowercase(text: &str) -> String {
    // U+0130 is the only unconditional full lowercase expansion: its simple mapping is 'i'.
    text.chars()
        .map(|c| c.to_lowercase().next().unwrap_or(c))
        .collect()
}

pub(crate) fn parse_failure_classification_rules(
    value: &serde_json::Value,
) -> Result<Vec<FailureClassificationRule>, (String, &'static str)> {
    let key = FAILURE_CLASSIFICATION_RULES_KEY;
    let rules = value
        .as_array()
        .filter(|rules| rules.len() <= 100)
        .ok_or_else(|| (key.to_owned(), "must be an array of at most 100 rules"))?;
    rules
        .iter()
        .enumerate()
        .map(|(index, rule)| {
            let path = || format!("{key}[{index}]");
            let rule = rule
                .as_object()
                .ok_or_else(|| (path(), "must be an object"))?;
            let codes = rule
                .get("status_codes")
                .and_then(serde_json::Value::as_array)
                .ok_or_else(|| (format!("{}.status_codes", path()), "must be an array"))?;
            let mut seen = [false; 500];
            let mut status_codes = Vec::with_capacity(codes.len().min(seen.len()));
            for (status_index, code) in codes.iter().enumerate() {
                let code = code
                    .as_u64()
                    .filter(|code| (100..=599).contains(code))
                    .ok_or_else(|| {
                        (
                            format!("{}.status_codes[{status_index}]", path()),
                            "must be an integer from 100 to 599",
                        )
                    })?;
                if !std::mem::replace(&mut seen[code as usize - 100], true) {
                    status_codes.push(code as u16);
                }
            }
            let keyword = rule
                .get("keyword")
                .and_then(serde_json::Value::as_str)
                .map(str::trim)
                .filter(|keyword| !keyword.is_empty() && keyword.chars().take(201).count() <= 200)
                .ok_or_else(|| {
                    (
                        format!("{}.keyword", path()),
                        "must contain 1 to 200 characters after trimming",
                    )
                })?;
            let class = match rule.get("class").and_then(serde_json::Value::as_str) {
                Some("persistent") => RetryableFailureClass::Persistent,
                Some("rate_limited") => RetryableFailureClass::RateLimited,
                Some("transient") => RetryableFailureClass::Transient,
                _ => {
                    return Err((
                        format!("{}.class", path()),
                        "must be persistent, rate_limited, or transient",
                    ));
                }
            };
            Ok(FailureClassificationRule {
                status_codes,
                keyword: keyword.to_owned(),
                class,
            })
        })
        .collect()
}

/// Compact sibling IDs bill as the conversation model (`model-pricing.spec.md` MP-R1).
pub const OPENAI_COMPACT_PRICING_SUFFIX: &str = "-openai-compact";

pub fn canonicalize_ordered_string_ids(ids: &mut Vec<String>) {
    let mut seen = HashSet::new();
    ids.retain_mut(|id| {
        let trimmed = id.trim().to_string();
        if trimmed.is_empty() || !seen.insert(trimmed.clone()) {
            return false;
        }
        *id = trimmed;
        true
    });
}

pub fn canonicalize_codex_model_ids(model_ids: &mut Vec<String>) {
    canonicalize_ordered_string_ids(model_ids);
}

pub fn normalize_pricing_model_key(
    model_id: &str,
    reasoning_suffix_map: &HashMap<String, String>,
) -> String {
    let trimmed = model_id.trim();
    if trimmed.is_empty() {
        return String::new();
    }

    let mut suffixes: Vec<&str> = reasoning_suffix_map.keys().map(String::as_str).collect();
    suffixes.extend(
        BUILTIN_REASONING_EFFORT_SUFFIXES
            .iter()
            .map(|(suffix, _)| *suffix),
    );
    suffixes.push(OPENAI_COMPACT_PRICING_SUFFIX);
    suffixes.sort_by(|a, b| b.len().cmp(&a.len()).then_with(|| a.cmp(b)));
    suffixes.dedup();

    for suffix in suffixes {
        if let Some(base) = trimmed.strip_suffix(suffix)
            && !base.is_empty()
        {
            return base.to_string();
        }
    }

    trimmed.to_string()
}

fn default_true() -> bool {
    true
}

/// RCD-C4 (`request-capture-dumps.spec.md`): 1 GiB default capture size budget.
pub const DEFAULT_REQUEST_CAPTURE_MAX_TOTAL_BYTES: u64 = 1_073_741_824;
/// RCD-C4: smallest non-zero budget accepted by the settings store.
pub const MIN_REQUEST_CAPTURE_MAX_TOTAL_BYTES: u64 = 1_048_576;

/// RCD-C4: `0` disables the budget; non-zero values below 1 MiB persist 1 MiB.
pub fn clamp_request_capture_max_total_bytes(value: u64) -> u64 {
    if value == 0 {
        0
    } else {
        value.max(MIN_REQUEST_CAPTURE_MAX_TOTAL_BYTES)
    }
}

pub(crate) fn default_reasoning_suffix_map() -> HashMap<String, String> {
    let mut m = HashMap::new();
    m.insert("-thinking".to_string(), "high".to_string());
    m.insert("-reasoning".to_string(), "high".to_string());
    m.insert("-nothinking".to_string(), "none".to_string());
    m
}

/// MP-T10 seed table: USD-denominated defaults mirroring new-api semantics.
pub fn default_tool_prices() -> serde_json::Value {
    serde_json::json!({
        "web_search": "10",
        "x_search": "5",
        "file_search_tool_call": "2.5",
        "code_execution": "5",
        "code_interpreter_duration": { "usd": "0.0015", "per": "minute", "minimum_units": 5 },
        "code_execution_duration": { "usd": "0.000833333", "per": "minute", "minimum_units": 5 },
        "code_interpreter_session": { "usd": "0.03", "per": "session" }
    })
}

/// MP-U1: base-10 decimal string, `>= 0`, at most 12 integer digits and at most
/// 9 fractional digits; exponent notation, leading `+`/`-`, NaN, infinity invalid.
pub fn validate_usd_decimal(raw: &str) -> Result<(), String> {
    let error = || "price must be a non-negative base-10 decimal string".to_string();
    if raw.is_empty()
        || raw.starts_with('+')
        || raw.starts_with('-')
        || !raw
            .bytes()
            .all(|byte| byte.is_ascii_digit() || byte == b'.')
        || raw.bytes().filter(|byte| *byte == b'.').count() > 1
    {
        return Err(error());
    }
    let value = rust_decimal::Decimal::from_str_exact(raw).map_err(|_| error())?;
    if value.is_sign_negative() {
        return Err(error());
    }
    if value.scale() > 9 {
        return Err("price must have at most 9 fractional digits".to_string());
    }
    if value.trunc().to_string().trim_start_matches('-').len() > 12 {
        return Err("price must have at most 12 integer digits".to_string());
    }
    Ok(())
}

const TOOL_PRICE_UNITS: &[&str] = &["1k_calls", "minute", "session"];

/// MP-T1..MP-T3 write-time validation for the `tool_prices` settings object.
///
/// Accepted per class: a JSON number, a decimal string, or an object
/// `{ "usd": <number|string>, "per": <unit>, "minimum_units": <int >= 1> }` where
/// `minimum_units` is valid only for `minute` and `session` units.
pub fn validate_tool_prices(value: &serde_json::Value) -> Result<(), String> {
    let object = value
        .as_object()
        .ok_or_else(|| "tool_prices must be a JSON object".to_string())?;
    for (class, entry) in object {
        if class.trim().is_empty() {
            return Err("tool_prices keys must be non-empty usage classes".to_string());
        }
        let context = |message: String| format!("tool_prices[{class}]: {message}");
        match entry {
            serde_json::Value::Number(number) => {
                validate_usd_decimal(&number.to_string()).map_err(context)?;
            }
            serde_json::Value::String(raw) => {
                validate_usd_decimal(raw).map_err(context)?;
            }
            serde_json::Value::Object(fields) => {
                for key in fields.keys() {
                    if !matches!(key.as_str(), "usd" | "per" | "minimum_units") {
                        return Err(context(format!("unknown field `{key}`")));
                    }
                }
                match fields.get("usd") {
                    Some(serde_json::Value::Number(number)) => {
                        validate_usd_decimal(&number.to_string()).map_err(context)?;
                    }
                    Some(serde_json::Value::String(raw)) => {
                        validate_usd_decimal(raw).map_err(context)?;
                    }
                    _ => return Err(context("`usd` must be a decimal number or string".into())),
                }
                let per = match fields.get("per") {
                    Some(serde_json::Value::String(per))
                        if TOOL_PRICE_UNITS.contains(&per.as_str()) =>
                    {
                        per.as_str()
                    }
                    _ => {
                        return Err(context(
                            "`per` must be one of 1k_calls, minute, session".into(),
                        ));
                    }
                };
                if let Some(minimum) = fields.get("minimum_units") {
                    if per == "1k_calls" {
                        return Err(context(
                            "`minimum_units` is valid only for minute and session units".into(),
                        ));
                    }
                    if !minimum.as_u64().is_some_and(|value| value >= 1) {
                        return Err(context("`minimum_units` must be an integer >= 1".into()));
                    }
                }
            }
            _ => {
                return Err(context(
                    "value must be a decimal, a decimal string, or a price object".into(),
                ));
            }
        }
    }
    Ok(())
}

impl Default for SystemSettings {
    fn default() -> Self {
        Self {
            registration_enabled: true,
            captcha_enabled: true,
            session_ttl_days: 7,
            api_key_max_per_user: 1000,
            site_name: "Monoize Dashboard".to_string(),
            site_description: "Unified Responses Proxy".to_string(),
            api_base_url: String::new(),
            global_transforms: Vec::new(),
            global_model_redirects: Vec::new(),
            reasoning_suffix_map: default_reasoning_suffix_map(),
            codex_model_ids: Vec::new(),
            dashboard_performance_group_ids: Vec::new(),
            dashboard_performance_model_ids: Vec::new(),
            monoize_honor_rate_limit: false,
            monoize_failure_classification_rules: default_failure_classification_rules(),
            monoize_active_probe_interval_seconds: 30,
            monoize_active_probe_success_threshold: 1,
            monoize_active_probe_model: None,
            monoize_active_probe_stream: true,
            monoize_passive_failure_threshold: 3,
            monoize_passive_cooldown_seconds: 60,
            monoize_passive_window_seconds: 30,
            monoize_passive_rate_limit_cooldown_seconds: 15,
            monoize_request_timeout_ms: 30000,
            monoize_stream_idle_timeout_ms: 120000,
            monoize_enable_estimated_billing: true,
            monoize_extra_fields_whitelist: HashMap::new(),
            monoize_strip_cross_protocol_nested_extra: true,
            monoize_request_capture_enabled: false,
            monoize_request_capture_max_total_bytes: DEFAULT_REQUEST_CAPTURE_MAX_TOTAL_BYTES,
            monoize_mask_sensitive_info: true,
            monoize_affinity_enabled: true,
            monoize_affinity_idle_ttl_seconds: 30 * 60,
            monoize_affinity_failback_mode: AffinityFailbackMode::Sticky,
            monoize_affinity_failback_delay_seconds: 5 * 60,
            allow_free_when_unpriced: false,
            allow_free_when_missing_usage: false,
            tool_prices: default_tool_prices(),
            price_sync_new_api_base_url: String::new(),
            price_sync_new_api_token: String::new(),
            price_sync_auto_enabled: true,
            recharge_public_origin: String::new(),
            updated_at: Utc::now(),
        }
    }
}

/// RC-G1: a non-empty `recharge_public_origin` must be an absolute `http` or
/// `https` origin with no path, query, fragment, userinfo, or trailing slash.
pub fn validate_recharge_public_origin(raw: &str) -> Result<(), String> {
    if raw.is_empty() {
        return Ok(());
    }
    let error = || {
        "recharge_public_origin must be an http(s) origin without path or trailing slash"
            .to_string()
    };
    let rest = raw
        .strip_prefix("https://")
        .or_else(|| raw.strip_prefix("http://"))
        .ok_or_else(error)?;
    if rest.is_empty()
        || rest.contains('/')
        || rest.contains('?')
        || rest.contains('#')
        || rest.contains('@')
        || rest.contains(char::is_whitespace)
    {
        return Err(error());
    }
    Ok(())
}

#[derive(Clone)]
pub struct SettingsStore {
    db: DbPool,
}

mod store;

pub(crate) const CONFIG_EPOCH_TENANT: &str = "monoize";
pub(crate) const CONFIG_EPOCH_KIND: &str = "config_epoch";
pub(crate) const CONFIG_EPOCH_ID: &str = "global";

/// E3 read path on a replica: exactly one row, one column.
pub async fn read_config_epoch(db: &DbPool) -> Result<u64, String> {
    let rows = db
        .read()
        .query_all(db.stmt(
            "SELECT value FROM state_records WHERE tenant_id = $1 AND kind = $2 AND id = $3",
            vec![
                CONFIG_EPOCH_TENANT.into(),
                CONFIG_EPOCH_KIND.into(),
                CONFIG_EPOCH_ID.into(),
            ],
        ))
        .await
        .map_err(|e| e.to_string())?;
    match rows.first() {
        None => Ok(0),
        Some(row) => row
            .try_get::<String>("", "value")
            .map_err(|e| format!("invalid config epoch row: {e}"))?
            .trim()
            .parse::<u64>()
            .map_err(|error| format!("invalid config epoch value: {error}")),
    }
}

/// E2 write path on the primary: one statement computing `value + 1` inside the
/// caller's open transaction. A missing row is inserted as epoch 1 (0 + 1).
pub(crate) async fn bump_config_epoch_in_tx(
    db: &DbPool,
    tx: &sea_orm::DatabaseTransaction,
) -> Result<(), String> {
    let sql = if db.is_sqlite() {
        "INSERT INTO state_records (tenant_id, kind, id, value, expires_at) \
         VALUES ($1, $2, $3, '1', NULL) \
         ON CONFLICT (tenant_id, kind, id) \
         DO UPDATE SET value = CAST(CAST(value AS INTEGER) + 1 AS TEXT)"
    } else {
        "INSERT INTO state_records (tenant_id, kind, id, value, expires_at) \
         VALUES ($1, $2, $3, '1', NULL) \
         ON CONFLICT (tenant_id, kind, id) \
         DO UPDATE SET value = CAST(CAST(state_records.value AS BIGINT) + 1 AS TEXT)"
    };
    tx.execute(db.stmt(
        sql,
        vec![
            CONFIG_EPOCH_TENANT.into(),
            CONFIG_EPOCH_KIND.into(),
            CONFIG_EPOCH_ID.into(),
        ],
    ))
    .await
    .map_err(|e| e.to_string())?;
    Ok(())
}

fn decode_registration_enabled(raw: &str) -> Result<bool, String> {
    raw.parse::<bool>()
        .map_err(|error| format!("invalid registration_enabled boolean: {error}"))
}

fn decode_captcha_enabled(raw: &str) -> Result<bool, String> {
    raw.parse::<bool>()
        .map_err(|error| format!("invalid captcha_enabled boolean: {error}"))
}
