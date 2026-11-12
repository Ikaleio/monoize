use super::types::*;
use crate::urp::encode::openai_chat::ChatDialect;
use std::collections::{BTreeMap, HashMap, HashSet};

pub(super) fn generate_short_id() -> String {
    const CHARSET: &[u8] = b"abcdefghijklmnopqrstuvwxyz0123456789";
    let bytes = uuid::Uuid::new_v4().into_bytes();
    (0..8)
        .map(|i| CHARSET[bytes[i] as usize % CHARSET.len()] as char)
        .collect()
}

/// CP-INV-14: trim and treat empty as NULL (follow-global).
pub(super) fn normalized_proxy_url(raw: Option<&str>) -> Option<String> {
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

/// CP-INV-16a: maps one Channel input to its stored dialect, or the rejection message.
pub fn channel_chat_dialect(
    input: &CreateMonoizeChannelInput,
) -> Result<Option<ChatDialect>, String> {
    stored_chat_dialect(
        format_args!("channel '{}'", input.name),
        input.provider_type,
        input.chat_dialect.as_deref(),
    )
}

/// CP-INV-17a: maps one override input to its stored entry, or the rejection message.
pub fn api_type_override_from_input(
    index: usize,
    input: &ApiTypeOverrideInput,
) -> Result<ApiTypeOverride, String> {
    Ok(ApiTypeOverride {
        pattern: input.pattern.clone(),
        api_type: input.api_type,
        chat_dialect: stored_chat_dialect(
            format_args!("api_type_overrides[{index}]"),
            input.api_type,
            input.chat_dialect.as_deref(),
        )?,
    })
}

fn stored_chat_dialect(
    owner: std::fmt::Arguments<'_>,
    api_type: MonoizeProviderType,
    requested: Option<&str>,
) -> Result<Option<ChatDialect>, String> {
    if api_type != MonoizeProviderType::ChatCompletion {
        return match requested {
            None => Ok(None),
            Some(_) => Err(format!(
                "{owner} chat_dialect is only valid for type chat_completion"
            )),
        };
    }
    let Some(requested) = requested else {
        return Ok(Some(ChatDialect::Generic));
    };
    ChatDialect::from_str(requested).map(Some).ok_or_else(|| {
        format!(
            "{owner} has invalid chat_dialect '{requested}'; expected openai, deepseek, openrouter, vercel, or generic"
        )
    })
}

/// CP-INV-15a: trim keys, drop nothing else, canonical JSON with sorted keys;
/// an empty map persists as NULL.
pub(super) fn normalized_extra_headers_json(
    raw: Option<&BTreeMap<String, String>>,
) -> Option<String> {
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

pub(super) fn canonicalize_models(
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

pub(super) fn validate_channels(
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
        if let Some(v) = c.passive_failure_count_threshold_override
            && !(1..=i32::MAX as u32).contains(&v)
        {
            return Err(
                "channel passive_failure_count_threshold_override must be between 1 and 2147483647"
                    .to_string(),
            );
        }
        if let Some(v) = c.passive_cooldown_seconds_override
            && !(1..=i32::MAX as u64).contains(&v)
        {
            return Err(
                "channel passive_cooldown_seconds_override must be between 1 and 2147483647"
                    .to_string(),
            );
        }
        if let Some(v) = c.passive_window_seconds_override
            && !(1..=i32::MAX as u64).contains(&v)
        {
            return Err(
                "channel passive_window_seconds_override must be between 1 and 2147483647"
                    .to_string(),
            );
        }
        if let Some(v) = c.passive_rate_limit_cooldown_seconds_override
            && !(1..=i32::MAX as u64).contains(&v)
        {
            return Err(
                    "channel passive_rate_limit_cooldown_seconds_override must be between 1 and 2147483647".to_string(),
                );
        }
        if let Some(v) = c.active_probe_interval_seconds_override
            && !(1..=i32::MAX as u64).contains(&v)
        {
            return Err(
                "channel active_probe_interval_seconds_override must be between 1 and 2147483647"
                    .to_string(),
            );
        }
        if let Some(v) = c.active_probe_success_threshold_override
            && !(1..=i32::MAX as u32).contains(&v)
        {
            return Err(
                "channel active_probe_success_threshold_override must be between 1 and 2147483647"
                    .to_string(),
            );
        }
        if let Some(v) = c.affinity_idle_ttl_seconds_override
            && !(1..=i32::MAX as u64).contains(&v)
        {
            return Err(
                "channel affinity_idle_ttl_seconds_override must be between 1 and 2147483647"
                    .to_string(),
            );
        }
        if let Some(v) = c.affinity_failback_delay_seconds_override
            && v > i32::MAX as u64
        {
            return Err(
                "channel affinity_failback_delay_seconds_override must be between 0 and 2147483647"
                    .to_string(),
            );
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
        if let Some(id) = &c.id
            && !ids.insert(id.clone())
        {
            return Err("duplicate channel id".to_string());
        }
    }
    Ok(())
}

pub(super) fn validate_provider_input(
    name: &str,
    channels: &[CreateMonoizeChannelInput],
    api_type_overrides: &[ApiTypeOverrideInput],
) -> Result<(), String> {
    if name.trim().is_empty() {
        return Err("provider name must not be empty".to_string());
    }
    validate_channels(channels, true)?;
    validate_api_type_overrides(api_type_overrides)?;
    Ok(())
}

pub(super) fn stored_api_type_overrides(
    overrides: &[ApiTypeOverrideInput],
) -> Result<Vec<ApiTypeOverride>, String> {
    overrides
        .iter()
        .enumerate()
        .map(|(index, input)| api_type_override_from_input(index, input))
        .collect()
}

pub(super) fn validate_api_type_overrides(
    overrides: &[ApiTypeOverrideInput],
) -> Result<(), String> {
    for (idx, entry) in overrides.iter().enumerate() {
        if entry.pattern.trim().is_empty() {
            return Err(format!(
                "api_type_overrides[{idx}].pattern must not be empty"
            ));
        }
    }
    Ok(())
}

/// Resolves the effective API type for a given model by evaluating api_type_overrides
/// in order. First matching glob pattern wins; falls back to the default provider_type.
pub fn resolve_effective_api_type(
    overrides: &[ApiTypeOverride],
    default_type: MonoizeProviderType,
    model: &str,
) -> MonoizeProviderType {
    matching_api_type_override(overrides, model).map_or(default_type, |entry| entry.api_type)
}

/// AT-1: the first entry whose pattern matches the model, in array order.
pub fn matching_api_type_override<'a>(
    overrides: &'a [ApiTypeOverride],
    model: &str,
) -> Option<&'a ApiTypeOverride> {
    overrides
        .iter()
        .find(|entry| glob_match(&entry.pattern, model))
}

fn glob_match(pattern: &str, value: &str) -> bool {
    crate::glob::case_sensitive_glob_match(pattern, value)
}

pub(super) fn validate_create_provider_input(
    input: &CreateMonoizeProviderInput,
) -> Result<(), String> {
    validate_provider_input(&input.name, &input.channels, &input.api_type_overrides)?;
    if let Some(v) = input.active_probe_interval_seconds_override
        && !(1..=i32::MAX as u64).contains(&v)
    {
        return Err(
            "active_probe_interval_seconds_override must be between 1 and 2147483647".to_string(),
        );
    }
    if let Some(v) = input.active_probe_success_threshold_override
        && !(1..=i32::MAX as u32).contains(&v)
    {
        return Err(
            "active_probe_success_threshold_override must be between 1 and 2147483647".to_string(),
        );
    }
    if let Some(v) = input.request_timeout_ms_override
        && !(1..=i32::MAX as u64).contains(&v)
    {
        return Err("request_timeout_ms_override must be between 1 and 2147483647".to_string());
    }
    if input.channel_retry_interval_ms < 0 {
        return Err("channel_retry_interval_ms must be >= 0".to_string());
    }

    Ok(())
}

pub(super) fn validate_update_provider_input(
    input: &UpdateMonoizeProviderInput,
) -> Result<(), String> {
    if let Some(channels) = &input.channels {
        validate_channels(channels, false)?;
    }
    if let Some(Some(v)) = input.active_probe_interval_seconds_override
        && !(1..=i32::MAX as u64).contains(&v)
    {
        return Err(
            "active_probe_interval_seconds_override must be between 1 and 2147483647".to_string(),
        );
    }
    if let Some(Some(v)) = input.active_probe_success_threshold_override
        && !(1..=i32::MAX as u32).contains(&v)
    {
        return Err(
            "active_probe_success_threshold_override must be between 1 and 2147483647".to_string(),
        );
    }
    if let Some(Some(v)) = input.request_timeout_ms_override
        && !(1..=i32::MAX as u64).contains(&v)
    {
        return Err("request_timeout_ms_override must be between 1 and 2147483647".to_string());
    }
    if let Some(v) = input.channel_retry_interval_ms
        && v < 0
    {
        return Err("channel_retry_interval_ms must be >= 0".to_string());
    }

    if let Some(api_type_overrides) = &input.api_type_overrides {
        validate_api_type_overrides(api_type_overrides)?;
    }

    Ok(())
}
