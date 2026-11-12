use super::*;

pub(super) const DEFAULT_HTTP_BODY_MAX_BYTES: usize = 50 * 1024 * 1024;

#[derive(Debug, Clone)]
pub struct RuntimeConfig {
    pub listen: String,
    pub metrics_path: String,
    pub database_dsn: String,
    pub request_log_spool_dir: Option<std::path::PathBuf>,
    pub node: NodeSettings,
}

impl RuntimeConfig {
    /// Errors carry `(error_code, detail)` and stop startup per PRP1/PRP7/PX2.
    pub fn from_env() -> Result<Self, (&'static str, String)> {
        let listen = std::env::var("MONOIZE_LISTEN")
            .ok()
            .filter(|v| !v.trim().is_empty())
            .unwrap_or_else(|| "0.0.0.0:8080".to_string());
        let metrics_path = std::env::var("MONOIZE_METRICS_PATH")
            .ok()
            .filter(|v| !v.trim().is_empty())
            .unwrap_or_else(|| "/metrics".to_string());
        let database_dsn = resolve_database_dsn();
        let node = NodeSettings::from_env()?;
        Ok(Self {
            listen,
            metrics_path,
            database_dsn,
            request_log_spool_dir: None,
            node,
        })
    }

    /// Programmatic construction with default primary-node settings.
    pub fn with_defaults(listen: &str, metrics_path: &str, database_dsn: String) -> Self {
        Self {
            listen: listen.to_string(),
            metrics_path: metrics_path.to_string(),
            database_dsn,
            request_log_spool_dir: None,
            node: NodeSettings::primary_default(),
        }
    }
}

pub(super) fn http_body_max_bytes() -> usize {
    http_body_max_bytes_from_raw(std::env::var("MONOIZE_HTTP_BODY_MAX_BYTES").ok().as_deref())
}

pub(super) fn http_body_max_bytes_from_raw(raw: Option<&str>) -> usize {
    raw.map(str::trim)
        .filter(|value| !value.is_empty())
        .and_then(|value| value.parse::<usize>().ok())
        .filter(|value| *value > 0)
        .unwrap_or(DEFAULT_HTTP_BODY_MAX_BYTES)
}

fn resolve_database_dsn() -> String {
    std::env::var("MONOIZE_DATABASE_DSN")
        .ok()
        .filter(|v| !v.trim().is_empty())
        .or_else(|| {
            std::env::var("DATABASE_URL")
                .ok()
                .filter(|v| !v.trim().is_empty())
        })
        .unwrap_or_else(|| "sqlite://./data/monoize.db".to_string())
}

/// Shared construction logic for the settings-derived runtime snapshot (E3): the
/// primary publishes it after mutations and the replica rebuilds it on epoch change.
pub(crate) fn runtime_config_from_settings(
    settings_snapshot: &crate::settings::SystemSettings,
) -> MonoizeRuntimeConfig {
    let mut runtime = MonoizeRuntimeConfig {
        passive_failure_count_threshold: settings_snapshot.monoize_passive_failure_threshold.max(1),
        passive_cooldown_seconds: settings_snapshot.monoize_passive_cooldown_seconds.max(1),
        passive_window_seconds: settings_snapshot.monoize_passive_window_seconds.max(1),
        passive_rate_limit_cooldown_seconds: settings_snapshot
            .monoize_passive_rate_limit_cooldown_seconds
            .max(1),
        active_enabled: settings_snapshot.monoize_active_probe_enabled,
        active_interval_seconds: settings_snapshot
            .monoize_active_probe_interval_seconds
            .max(1),
        active_success_threshold: settings_snapshot
            .monoize_active_probe_success_threshold
            .max(1),
        active_probe_model: settings_snapshot.monoize_active_probe_model.clone(),
        global_transforms: settings_snapshot.global_transforms.clone(),
        reasoning_suffix_map: settings_snapshot.reasoning_suffix_map.clone(),
        allow_free_when_unpriced: settings_snapshot.allow_free_when_unpriced,
        allow_free_when_missing_usage: settings_snapshot.allow_free_when_missing_usage,
        tool_prices: settings_snapshot.tool_prices.clone(),
        codex_model_ids: settings_snapshot.codex_model_ids.clone(),
        request_timeout_ms: settings_snapshot.monoize_request_timeout_ms.max(1),
        stream_idle_timeout_ms: settings_snapshot.monoize_stream_idle_timeout_ms.max(1),
        enable_estimated_billing: settings_snapshot.monoize_enable_estimated_billing,
        extra_fields_whitelist: settings_snapshot.monoize_extra_fields_whitelist.clone(),
        strip_cross_protocol_nested_extra: settings_snapshot
            .monoize_strip_cross_protocol_nested_extra,
        request_capture_enabled: settings_snapshot.monoize_request_capture_enabled,
        request_capture_max_total_bytes: settings_snapshot.monoize_request_capture_max_total_bytes,
        mask_sensitive_info: settings_snapshot.monoize_mask_sensitive_info,
        affinity_enabled: settings_snapshot.monoize_affinity_enabled,
        affinity_idle_ttl_seconds: settings_snapshot.monoize_affinity_idle_ttl_seconds.max(1),
        affinity_failback_mode: settings_snapshot.monoize_affinity_failback_mode,
        affinity_failback_delay_seconds: settings_snapshot.monoize_affinity_failback_delay_seconds,
        ..MonoizeRuntimeConfig::default()
    };
    let _ = runtime.set_global_model_redirects(settings_snapshot.global_model_redirects.clone());
    runtime
}
