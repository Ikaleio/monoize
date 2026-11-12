use super::*;

pub(super) fn apply_general_setting(
    settings: &mut SystemSettings,
    row: system_settings::Model,
) -> Result<(), String> {
    match row.key.as_str() {
        "registration_enabled" => {
            settings.registration_enabled = decode_registration_enabled(&row.value)?;
        }
        "captcha_enabled" => {
            settings.captcha_enabled = decode_captcha_enabled(&row.value)?;
        }
        "session_ttl_days" => {
            settings.session_ttl_days = row.value.parse().unwrap_or(7);
        }
        "api_key_max_per_user" => {
            settings.api_key_max_per_user = row.value.parse().unwrap_or(1000);
        }
        "site_name" => {
            settings.site_name = row.value;
        }
        "site_description" => {
            settings.site_description = row.value;
        }
        "api_base_url" => {
            settings.api_base_url = row.value;
        }
        "global_transforms" => {
            if let Ok(mut transforms) = serde_json::from_str::<Vec<TransformRuleConfig>>(&row.value)
            {
                canonicalize_transform_rules(&mut transforms);
                settings.global_transforms = transforms;
            }
        }
        "global_model_redirects" => {
            if let Ok(rules) = serde_json::from_str::<Vec<ModelRedirectRule>>(&row.value)
                && validate_model_redirects(&rules).is_ok()
            {
                settings.global_model_redirects = rules;
            }
        }
        "reasoning_suffix_map" => {
            if let Ok(map) = serde_json::from_str(&row.value) {
                settings.reasoning_suffix_map = map;
            }
        }
        "codex_model_ids" => {
            if let Ok(mut model_ids) = serde_json::from_str::<Vec<String>>(&row.value) {
                canonicalize_codex_model_ids(&mut model_ids);
                settings.codex_model_ids = model_ids;
            }
        }
        "dashboard_performance_group_ids" => {
            if let Ok(mut group_ids) = serde_json::from_str::<Vec<String>>(&row.value) {
                canonicalize_ordered_string_ids(&mut group_ids);
                settings.dashboard_performance_group_ids = group_ids;
            }
        }
        "dashboard_performance_model_ids" => {
            if let Ok(mut model_ids) = serde_json::from_str::<Vec<String>>(&row.value) {
                canonicalize_ordered_string_ids(&mut model_ids);
                settings.dashboard_performance_model_ids = model_ids;
            }
        }
        "allow_free_when_unpriced" => {
            settings.allow_free_when_unpriced = row.value.parse().unwrap_or(false);
        }
        "allow_free_when_missing_usage" => {
            settings.allow_free_when_missing_usage = row.value.parse().unwrap_or(false);
        }
        "tool_prices" => {
            if let Ok(value) = serde_json::from_str::<serde_json::Value>(&row.value)
                && validate_tool_prices(&value).is_ok()
            {
                settings.tool_prices = value;
            }
        }
        "price_sync_new_api_base_url" => {
            settings.price_sync_new_api_base_url = row.value;
        }
        "price_sync_new_api_token" => {
            settings.price_sync_new_api_token = row.value;
        }
        "price_sync_auto_enabled" => {
            settings.price_sync_auto_enabled = row.value.parse().unwrap_or(true);
        }
        "recharge_public_origin" => {
            settings.recharge_public_origin = row.value.trim().to_string();
        }
        _ => {}
    }
    Ok(())
}

pub(super) fn apply_runtime_setting(
    settings: &mut SystemSettings,
    row: system_settings::Model,
) -> Result<(), String> {
    match row.key.as_str() {
        "monoize_active_probe_enabled" => {
            settings.monoize_active_probe_enabled = row.value.parse().unwrap_or(true);
        }
        "monoize_active_probe_interval_seconds" => {
            settings.monoize_active_probe_interval_seconds = row.value.parse().unwrap_or(30);
        }
        "monoize_active_probe_success_threshold" => {
            settings.monoize_active_probe_success_threshold = row.value.parse().unwrap_or(1);
        }
        "monoize_active_probe_model" => {
            let trimmed = row.value.trim();
            settings.monoize_active_probe_model = if trimmed.is_empty() {
                None
            } else {
                Some(trimmed.to_string())
            };
        }
        "monoize_passive_failure_threshold" => {
            settings.monoize_passive_failure_threshold = row.value.parse().unwrap_or(3);
        }
        "monoize_passive_cooldown_seconds" => {
            settings.monoize_passive_cooldown_seconds = row.value.parse().unwrap_or(60);
        }
        "monoize_passive_window_seconds" => {
            settings.monoize_passive_window_seconds = row.value.parse().unwrap_or(30);
        }
        "monoize_passive_rate_limit_cooldown_seconds" => {
            settings.monoize_passive_rate_limit_cooldown_seconds = row.value.parse().unwrap_or(15);
        }
        "monoize_request_timeout_ms" => {
            settings.monoize_request_timeout_ms = row.value.parse().unwrap_or(30000);
        }
        "monoize_stream_idle_timeout_ms" => {
            settings.monoize_stream_idle_timeout_ms = row.value.parse().unwrap_or(120000);
        }
        "monoize_enable_estimated_billing" => {
            settings.monoize_enable_estimated_billing = row.value.parse().unwrap_or(true);
        }
        "monoize_extra_fields_whitelist" => {
            if let Ok(map) = serde_json::from_str(&row.value) {
                settings.monoize_extra_fields_whitelist = map;
            }
        }
        "monoize_strip_cross_protocol_nested_extra" => {
            settings.monoize_strip_cross_protocol_nested_extra = row.value.parse().unwrap_or(true);
        }
        "monoize_request_capture_enabled" => {
            settings.monoize_request_capture_enabled = row.value.parse().unwrap_or(false);
        }
        "monoize_request_capture_max_total_bytes" => {
            settings.monoize_request_capture_max_total_bytes =
                clamp_request_capture_max_total_bytes(
                    row.value
                        .parse()
                        .unwrap_or(DEFAULT_REQUEST_CAPTURE_MAX_TOTAL_BYTES),
                );
        }
        "monoize_mask_sensitive_info" => {
            settings.monoize_mask_sensitive_info = row.value.parse().unwrap_or(true);
        }
        "monoize_affinity_enabled" => {
            settings.monoize_affinity_enabled = row.value.parse().unwrap_or(true);
        }
        "monoize_affinity_idle_ttl_seconds" => {
            settings.monoize_affinity_idle_ttl_seconds =
                row.value.parse::<u64>().unwrap_or(30 * 60).max(1);
        }
        "monoize_affinity_failback_mode" => {
            settings.monoize_affinity_failback_mode =
                AffinityFailbackMode::parse(&row.value).unwrap_or_default();
        }
        "monoize_affinity_failback_delay_seconds" => {
            settings.monoize_affinity_failback_delay_seconds = row.value.parse().unwrap_or(5 * 60);
        }
        _ => {}
    }
    Ok(())
}

pub(super) fn settings_values(
    settings: &SystemSettings,
) -> Result<Vec<(&'static str, String)>, String> {
    let mut values = Vec::with_capacity(40);
    append_core_values(&mut values, settings)?;
    append_capture_and_billing_values(&mut values, settings)?;
    Ok(values)
}

pub(super) fn append_core_values(
    values: &mut Vec<(&'static str, String)>,
    settings: &SystemSettings,
) -> Result<(), String> {
    values.extend([
        (
            "registration_enabled",
            settings.registration_enabled.to_string(),
        ),
        ("captcha_enabled", settings.captcha_enabled.to_string()),
        ("session_ttl_days", settings.session_ttl_days.to_string()),
        (
            "api_key_max_per_user",
            settings.api_key_max_per_user.to_string(),
        ),
        ("site_name", settings.site_name.clone()),
        ("site_description", settings.site_description.clone()),
        ("api_base_url", settings.api_base_url.clone()),
        (
            "global_transforms",
            serde_json::to_string(&settings.global_transforms).map_err(|e| e.to_string())?,
        ),
        (
            "global_model_redirects",
            serde_json::to_string(&settings.global_model_redirects).map_err(|e| e.to_string())?,
        ),
        (
            "reasoning_suffix_map",
            serde_json::to_string(&settings.reasoning_suffix_map).map_err(|e| e.to_string())?,
        ),
        (
            "codex_model_ids",
            serde_json::to_string(&settings.codex_model_ids).map_err(|e| e.to_string())?,
        ),
        (
            "dashboard_performance_group_ids",
            serde_json::to_string(&settings.dashboard_performance_group_ids)
                .map_err(|e| e.to_string())?,
        ),
        (
            "dashboard_performance_model_ids",
            serde_json::to_string(&settings.dashboard_performance_model_ids)
                .map_err(|e| e.to_string())?,
        ),
        (
            "monoize_active_probe_enabled",
            settings.monoize_active_probe_enabled.to_string(),
        ),
        (
            "monoize_active_probe_interval_seconds",
            settings.monoize_active_probe_interval_seconds.to_string(),
        ),
        (
            "monoize_active_probe_success_threshold",
            settings.monoize_active_probe_success_threshold.to_string(),
        ),
        (
            "monoize_active_probe_model",
            settings
                .monoize_active_probe_model
                .clone()
                .unwrap_or_default(),
        ),
        (
            "monoize_passive_failure_threshold",
            settings.monoize_passive_failure_threshold.to_string(),
        ),
        (
            "monoize_passive_cooldown_seconds",
            settings.monoize_passive_cooldown_seconds.to_string(),
        ),
        (
            "monoize_passive_window_seconds",
            settings.monoize_passive_window_seconds.to_string(),
        ),
        (
            "monoize_passive_rate_limit_cooldown_seconds",
            settings
                .monoize_passive_rate_limit_cooldown_seconds
                .to_string(),
        ),
        (
            "monoize_request_timeout_ms",
            settings.monoize_request_timeout_ms.to_string(),
        ),
        (
            "monoize_stream_idle_timeout_ms",
            settings.monoize_stream_idle_timeout_ms.to_string(),
        ),
        (
            "monoize_enable_estimated_billing",
            settings.monoize_enable_estimated_billing.to_string(),
        ),
        (
            "monoize_extra_fields_whitelist",
            serde_json::to_string(&settings.monoize_extra_fields_whitelist)
                .map_err(|e| e.to_string())?,
        ),
        (
            "monoize_strip_cross_protocol_nested_extra",
            settings
                .monoize_strip_cross_protocol_nested_extra
                .to_string(),
        ),
    ]);
    Ok(())
}

pub(super) fn append_capture_and_billing_values(
    values: &mut Vec<(&'static str, String)>,
    settings: &SystemSettings,
) -> Result<(), String> {
    values.extend([
        (
            "monoize_request_capture_enabled",
            settings.monoize_request_capture_enabled.to_string(),
        ),
        (
            "monoize_request_capture_max_total_bytes",
            settings.monoize_request_capture_max_total_bytes.to_string(),
        ),
        (
            "monoize_mask_sensitive_info",
            settings.monoize_mask_sensitive_info.to_string(),
        ),
        (
            "monoize_affinity_enabled",
            settings.monoize_affinity_enabled.to_string(),
        ),
        (
            "monoize_affinity_idle_ttl_seconds",
            settings
                .monoize_affinity_idle_ttl_seconds
                .max(1)
                .to_string(),
        ),
        (
            "monoize_affinity_failback_mode",
            settings.monoize_affinity_failback_mode.as_str().to_string(),
        ),
        (
            "monoize_affinity_failback_delay_seconds",
            settings.monoize_affinity_failback_delay_seconds.to_string(),
        ),
        (
            "allow_free_when_unpriced",
            settings.allow_free_when_unpriced.to_string(),
        ),
        (
            "allow_free_when_missing_usage",
            settings.allow_free_when_missing_usage.to_string(),
        ),
        (
            "tool_prices",
            serde_json::to_string(&settings.tool_prices).map_err(|e| e.to_string())?,
        ),
        (
            "price_sync_new_api_base_url",
            settings.price_sync_new_api_base_url.clone(),
        ),
        (
            "price_sync_new_api_token",
            settings.price_sync_new_api_token.clone(),
        ),
        (
            "price_sync_auto_enabled",
            settings.price_sync_auto_enabled.to_string(),
        ),
        (
            "recharge_public_origin",
            settings.recharge_public_origin.clone(),
        ),
    ]);
    Ok(())
}
