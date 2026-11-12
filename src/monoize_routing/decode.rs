use super::types::*;
use crate::transforms::{TransformRuleConfig, canonicalize_transform_rules};
use crate::urp::encode::openai_chat::ChatDialect;
use crate::users::canonicalize_group_ids;
use chrono::{DateTime, Utc};
use sea_orm::QueryResult;
use std::collections::{BTreeMap, HashMap};

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

pub(super) fn serialize_provider_group_ids_json(group_ids: &[String]) -> Result<String, String> {
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

fn decode_extra_headers(raw: Option<String>) -> Result<Option<BTreeMap<String, String>>, String> {
    let Some(text) = raw.filter(|value| !value.trim().is_empty()) else {
        return Ok(None);
    };
    serde_json::from_str(&text)
        .map(Some)
        .map_err(|e| format!("invalid stored extra_headers JSON: {e}"))
}

pub(super) fn decode_channel_model_row(
    row: &QueryResult,
    model: &str,
) -> Result<MonoizeChannel, String> {
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

pub(super) fn decode_channel_row(
    row: &QueryResult,
    models: HashMap<String, MonoizeModelEntry>,
) -> Result<MonoizeChannel, String> {
    let id: String = row.try_get("", "id").map_err(|e| e.to_string())?;
    let provider_type_raw: String = row
        .try_get("", "provider_type")
        .map_err(|e| format!("channel {id} missing provider_type: {e}"))?;
    let provider_type = MonoizeProviderType::from_str(&provider_type_raw)
        .ok_or_else(|| format!("channel {id} invalid provider type: {provider_type_raw}"))?;
    let chat_dialect = match row
        .try_get::<Option<String>>("", "chat_dialect")
        .map_err(|e| e.to_string())?
    {
        Some(raw) => Some(
            ChatDialect::from_str(&raw)
                .ok_or_else(|| format!("channel {id} invalid chat_dialect: {raw}"))?,
        ),
        // CP-INV-16b: a chat row holding NULL reads as the fixed default.
        None if provider_type == MonoizeProviderType::ChatCompletion => Some(ChatDialect::Generic),
        None => None,
    };
    Ok(MonoizeChannel {
        id: id.clone(),
        name: row.try_get("", "name").map_err(|e| e.to_string())?,
        provider_type,
        base_url: row.try_get("", "base_url").map_err(|e| e.to_string())?,
        api_key: row.try_get("", "api_key").map_err(|e| e.to_string())?,
        weight: row.try_get("", "weight").map_err(|e| e.to_string())?,
        enabled: decode_required_bool(row, "channel", &id, "enabled")?,
        passive_failure_count_threshold_override: decode_optional_positive_u32(
            row,
            &id,
            "passive_failure_count_threshold_override",
        )?,
        passive_cooldown_seconds_override: decode_optional_positive_u64(
            row,
            &id,
            "passive_cooldown_seconds_override",
        )?,
        passive_window_seconds_override: decode_optional_positive_u64(
            row,
            &id,
            "passive_window_seconds_override",
        )?,
        passive_rate_limit_cooldown_seconds_override: decode_optional_positive_u64(
            row,
            &id,
            "passive_rate_limit_cooldown_seconds_override",
        )?,
        models,
        active_probe_enabled_override: decode_optional_bool(
            row,
            "channel",
            &id,
            "active_probe_enabled_override",
        )?,
        active_probe_interval_seconds_override: decode_optional_positive_u64(
            row,
            &id,
            "active_probe_interval_seconds_override",
        )?,
        active_probe_success_threshold_override: decode_optional_positive_u32(
            row,
            &id,
            "active_probe_success_threshold_override",
        )?,
        active_probe_model_override: row
            .try_get("", "active_probe_model_override")
            .map_err(|e| e.to_string())?,
        affinity_enabled_override: decode_optional_bool(
            row,
            "channel",
            &id,
            "affinity_enabled_override",
        )?,
        affinity_idle_ttl_seconds_override: decode_optional_positive_u64(
            row,
            &id,
            "affinity_idle_ttl_seconds_override",
        )?,
        affinity_failback_mode_override: row
            .try_get::<Option<String>>("", "affinity_failback_mode_override")
            .map_err(|e| e.to_string())?
            .map(|value| {
                AffinityFailbackMode::parse(&value).ok_or_else(|| {
                    format!("channel {id} invalid affinity_failback_mode_override: {value}")
                })
            })
            .transpose()?,
        affinity_failback_delay_seconds_override: decode_optional_nonnegative_u64(
            row,
            &id,
            "affinity_failback_delay_seconds_override",
        )?,
        proxy_url: row
            .try_get::<Option<String>>("", "proxy_url")
            .map_err(|e| e.to_string())?
            .filter(|value| !value.trim().is_empty()),
        extra_headers: decode_extra_headers(
            row.try_get::<Option<String>>("", "extra_headers")
                .map_err(|e| e.to_string())?,
        )?,
        session_affinity_auto: decode_optional_bool(row, "channel", &id, "session_affinity_auto")?,
        websocket_supported: decode_optional_bool(row, "channel", &id, "websocket_supported")?,
        chat_dialect,
        _healthy: None,
        _last_success_at: None,
        _health_status: None,
        _unhealthy_models: None,
        _probing_models: None,
        _cooldown_until: None,
    })
}

pub(super) fn decode_provider_row(
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
    let created_at = decode_provider_timestamp(row, &id, "created_at")?;
    let updated_at = decode_provider_timestamp(row, &id, "updated_at")?;
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
        circuit_breaker_enabled: decode_required_bool(
            row,
            "provider",
            &id,
            "circuit_breaker_enabled",
        )?,
        per_model_circuit_break: decode_required_bool(
            row,
            "provider",
            &id,
            "per_model_circuit_break",
        )?,
        transforms,
        api_type_overrides,
        active_probe_enabled_override: decode_optional_bool(
            row,
            "provider",
            &id,
            "active_probe_enabled_override",
        )?,
        active_probe_interval_seconds_override: decode_optional_positive_u64(
            row,
            &id,
            "active_probe_interval_seconds_override",
        )?,
        active_probe_success_threshold_override: decode_optional_positive_u32(
            row,
            &id,
            "active_probe_success_threshold_override",
        )?,
        active_probe_model_override: row
            .try_get("", "active_probe_model_override")
            .map_err(|e| e.to_string())?,
        request_timeout_ms_override: decode_optional_positive_u64(
            row,
            &id,
            "request_timeout_ms_override",
        )?,
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
        enabled: decode_required_bool(row, "provider", &id, "enabled")?,
        priority: row.try_get("", "priority").map_err(|e| e.to_string())?,
        created_at,
        updated_at,
    })
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

fn decode_required_bool(
    row: &QueryResult,
    entity: &str,
    id: &str,
    field: &str,
) -> Result<bool, String> {
    let value = row.try_get("", field).map_err(|e| e.to_string())?;
    decode_database_bool(entity, id, field, value)
}

fn decode_optional_bool(
    row: &QueryResult,
    entity: &str,
    id: &str,
    field: &str,
) -> Result<Option<bool>, String> {
    row.try_get::<Option<i32>>("", field)
        .map_err(|e| e.to_string())?
        .map(|value| decode_database_bool(entity, id, field, value))
        .transpose()
}

fn decode_optional_positive_u32(
    row: &QueryResult,
    id: &str,
    field: &str,
) -> Result<Option<u32>, String> {
    row.try_get::<Option<i32>>("", field)
        .map_err(|e| e.to_string())?
        .map(|value| decode_positive_u32(id, field, i64::from(value)))
        .transpose()
}

fn decode_optional_positive_u64(
    row: &QueryResult,
    id: &str,
    field: &str,
) -> Result<Option<u64>, String> {
    row.try_get::<Option<i32>>("", field)
        .map_err(|e| e.to_string())?
        .map(|value| decode_positive_u64(id, field, i64::from(value)))
        .transpose()
}

fn decode_optional_nonnegative_u64(
    row: &QueryResult,
    id: &str,
    field: &str,
) -> Result<Option<u64>, String> {
    row.try_get::<Option<i32>>("", field)
        .map_err(|e| e.to_string())?
        .map(|value| decode_nonnegative_u64(id, field, i64::from(value)))
        .transpose()
}

fn decode_provider_timestamp(
    row: &QueryResult,
    id: &str,
    field: &str,
) -> Result<DateTime<Utc>, String> {
    DateTime::parse_from_rfc3339(
        &row.try_get::<String>("", field)
            .map_err(|e| e.to_string())?,
    )
    .map_err(|e| format!("provider {id} invalid {field} RFC3339: {e}"))
    .map(|timestamp| timestamp.with_timezone(&Utc))
}
