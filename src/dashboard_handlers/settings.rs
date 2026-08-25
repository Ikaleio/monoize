use crate::app::AppState;
use crate::dashboard_handlers::auth::user_response_from_store;
use crate::dashboard_handlers::session_helpers::{get_current_user, require_admin};
use crate::error::{AppError, AppResult};
use crate::model_registry_store::validate_models_dev_base_url;
use crate::monoize_routing::AffinityFailbackMode;
use crate::transforms::TransformRuleConfig;
use crate::users::{ModelRedirectRule, validate_model_redirects};
use axum::Json;
use axum::extract::State;
use axum::http::{HeaderMap, StatusCode};
use axum::response::IntoResponse;
use serde::Deserialize;
use serde_json::json;
use std::sync::atomic::Ordering;

#[derive(Debug, Deserialize)]
pub struct UpdateSettingsRequest {
    pub registration_enabled: Option<bool>,
    pub captcha_enabled: Option<bool>,
    pub default_user_role: Option<String>,
    pub session_ttl_days: Option<i64>,
    pub api_key_max_per_user: Option<i64>,
    pub site_name: Option<String>,
    pub site_description: Option<String>,
    pub api_base_url: Option<String>,
    pub models_dev_base_url: Option<String>,
    pub global_transforms: Option<Vec<TransformRuleConfig>>,
    pub global_model_redirects: Option<Vec<ModelRedirectRule>>,
    pub reasoning_suffix_map: Option<std::collections::HashMap<String, String>>,
    pub codex_model_ids: Option<Vec<String>>,
    pub monoize_active_probe_enabled: Option<bool>,
    pub monoize_active_probe_interval_seconds: Option<u64>,
    pub monoize_active_probe_success_threshold: Option<u32>,
    pub monoize_active_probe_model: Option<Option<String>>,
    pub monoize_passive_failure_threshold: Option<u32>,
    pub monoize_passive_cooldown_seconds: Option<u64>,
    pub monoize_passive_window_seconds: Option<u64>,
    pub monoize_passive_min_samples: Option<u32>,
    pub monoize_passive_failure_rate_threshold: Option<f64>,
    pub monoize_passive_rate_limit_cooldown_seconds: Option<u64>,
    pub monoize_request_timeout_ms: Option<u64>,
    pub monoize_stream_idle_timeout_ms: Option<u64>,
    pub monoize_enable_estimated_billing: Option<bool>,
    pub monoize_extra_fields_whitelist: Option<std::collections::HashMap<String, Vec<String>>>,
    pub monoize_strip_cross_protocol_nested_extra: Option<bool>,
    pub monoize_request_capture_enabled: Option<bool>,
    pub monoize_request_capture_retention_days: Option<u64>,
    pub monoize_mask_sensitive_info: Option<bool>,
    pub monoize_affinity_enabled: Option<bool>,
    pub monoize_affinity_idle_ttl_seconds: Option<u64>,
    pub monoize_affinity_failback_mode: Option<AffinityFailbackMode>,
    pub monoize_affinity_failback_delay_seconds: Option<u64>,
}

pub async fn get_settings(
    State(state): State<AppState>,
    headers: HeaderMap,
) -> AppResult<impl IntoResponse> {
    require_admin(&headers, &state).await?;

    let settings_store = &state.settings_store;

    let settings = settings_store
        .get_all()
        .await
        .map_err(|e| AppError::new(StatusCode::INTERNAL_SERVER_ERROR, "internal_error", e))?;

    Ok(Json(settings))
}

pub async fn update_settings(
    State(state): State<AppState>,
    headers: HeaderMap,
    Json(body): Json<UpdateSettingsRequest>,
) -> AppResult<impl IntoResponse> {
    require_admin(&headers, &state).await?;
    let _update_guard = state.settings_update_lock.lock().await;

    let settings_store = &state.settings_store;

    let mut settings = settings_store
        .get_all()
        .await
        .map_err(|e| AppError::new(StatusCode::INTERNAL_SERVER_ERROR, "internal_error", e))?;
    let affinity_settings_before = (
        settings.monoize_affinity_enabled,
        settings.monoize_affinity_idle_ttl_seconds,
        settings.monoize_affinity_failback_mode,
        settings.monoize_affinity_failback_delay_seconds,
    );
    let health_settings_before = (
        settings.monoize_passive_failure_threshold,
        settings.monoize_passive_cooldown_seconds,
        settings.monoize_passive_window_seconds,
        settings.monoize_passive_min_samples,
        settings.monoize_passive_failure_rate_threshold.to_bits(),
        settings.monoize_passive_rate_limit_cooldown_seconds,
        settings.monoize_active_probe_enabled,
        settings.monoize_active_probe_interval_seconds,
        settings.monoize_active_probe_success_threshold,
        settings.monoize_active_probe_model.clone(),
    );

    if let Some(v) = body.registration_enabled {
        settings.registration_enabled = v;
    }
    if let Some(v) = body.captcha_enabled {
        settings.captcha_enabled = v;
    }
    if let Some(v) = body.default_user_role {
        settings.default_user_role = v;
    }
    if let Some(v) = body.session_ttl_days {
        if v <= 0 {
            return Err(AppError::new(
                StatusCode::BAD_REQUEST,
                "invalid_request",
                "session_ttl_days must be positive",
            ));
        }
        settings.session_ttl_days = v;
    }
    if let Some(v) = body.api_key_max_per_user {
        if v <= 0 {
            return Err(AppError::new(
                StatusCode::BAD_REQUEST,
                "invalid_request",
                "api_key_max_per_user must be positive",
            ));
        }
        settings.api_key_max_per_user = v;
    }
    if let Some(v) = body.site_name {
        settings.site_name = v;
    }
    if let Some(v) = body.site_description {
        settings.site_description = v;
    }
    if let Some(v) = body.api_base_url {
        settings.api_base_url = v;
    }
    if let Some(v) = body.models_dev_base_url {
        validate_models_dev_base_url(&v).map_err(|message| {
            AppError::new(StatusCode::BAD_REQUEST, "invalid_request", message)
        })?;
        settings.models_dev_base_url = v.trim().trim_end_matches('/').to_string();
    }
    if let Some(v) = body.global_transforms {
        settings.global_transforms = v;
    }
    if let Some(v) = body.global_model_redirects {
        validate_model_redirects(&v).map_err(|message| {
            AppError::new(StatusCode::BAD_REQUEST, "invalid_request", message)
        })?;
        settings.global_model_redirects = v;
    }
    if let Some(v) = body.reasoning_suffix_map {
        settings.reasoning_suffix_map = v;
    }
    if let Some(v) = body.codex_model_ids {
        settings.codex_model_ids = v;
    }
    if let Some(v) = body.monoize_active_probe_enabled {
        settings.monoize_active_probe_enabled = v;
    }
    if let Some(v) = body.monoize_active_probe_interval_seconds {
        settings.monoize_active_probe_interval_seconds = v.max(1);
    }
    if let Some(v) = body.monoize_active_probe_success_threshold {
        settings.monoize_active_probe_success_threshold = v.max(1);
    }
    if let Some(v) = body.monoize_active_probe_model {
        settings.monoize_active_probe_model = v.and_then(|s| {
            let trimmed = s.trim();
            if trimmed.is_empty() {
                None
            } else {
                Some(trimmed.to_string())
            }
        });
    }
    if let Some(v) = body.monoize_passive_failure_threshold {
        settings.monoize_passive_failure_threshold = v.max(1);
    }
    if let Some(v) = body.monoize_passive_cooldown_seconds {
        settings.monoize_passive_cooldown_seconds = v.max(1);
    }
    if let Some(v) = body.monoize_passive_window_seconds {
        settings.monoize_passive_window_seconds = v.max(1);
    }
    if let Some(v) = body.monoize_passive_min_samples {
        settings.monoize_passive_min_samples = v.max(1);
    }
    if let Some(v) = body.monoize_passive_failure_rate_threshold {
        if v.is_finite() {
            settings.monoize_passive_failure_rate_threshold = v.clamp(0.01, 1.0);
        }
    }
    if let Some(v) = body.monoize_passive_rate_limit_cooldown_seconds {
        settings.monoize_passive_rate_limit_cooldown_seconds = v.max(1);
    }
    if let Some(v) = body.monoize_request_timeout_ms {
        settings.monoize_request_timeout_ms = v.max(1);
    }
    if let Some(v) = body.monoize_stream_idle_timeout_ms {
        settings.monoize_stream_idle_timeout_ms = v.max(1);
    }
    if let Some(v) = body.monoize_enable_estimated_billing {
        settings.monoize_enable_estimated_billing = v;
    }
    if let Some(v) = body.monoize_extra_fields_whitelist {
        settings.monoize_extra_fields_whitelist = v;
    }
    if let Some(v) = body.monoize_strip_cross_protocol_nested_extra {
        settings.monoize_strip_cross_protocol_nested_extra = v;
    }
    if let Some(v) = body.monoize_request_capture_enabled {
        settings.monoize_request_capture_enabled = v;
    }
    if let Some(v) = body.monoize_request_capture_retention_days {
        settings.monoize_request_capture_retention_days = v.max(1);
    }
    if let Some(v) = body.monoize_mask_sensitive_info {
        settings.monoize_mask_sensitive_info = v;
    }
    if let Some(v) = body.monoize_affinity_enabled {
        settings.monoize_affinity_enabled = v;
    }
    if let Some(v) = body.monoize_affinity_idle_ttl_seconds {
        settings.monoize_affinity_idle_ttl_seconds = v.max(1);
    }
    if let Some(v) = body.monoize_affinity_failback_mode {
        settings.monoize_affinity_failback_mode = v;
    }
    if let Some(v) = body.monoize_affinity_failback_delay_seconds {
        settings.monoize_affinity_failback_delay_seconds = v;
    }

    let updated = settings_store
        .update_all(&settings)
        .await
        .map_err(|e| AppError::new(StatusCode::INTERNAL_SERVER_ERROR, "internal_error", e))?;

    {
        let mut rt = state.monoize_runtime.write().await;
        rt.request_timeout_ms = updated.monoize_request_timeout_ms.max(1);
        rt.stream_idle_timeout_ms = updated.monoize_stream_idle_timeout_ms.max(1);
        rt.enable_estimated_billing = updated.monoize_enable_estimated_billing;
        rt.passive_failure_count_threshold = updated.monoize_passive_failure_threshold.max(1);
        rt.passive_cooldown_seconds = updated.monoize_passive_cooldown_seconds.max(1);
        rt.passive_window_seconds = updated.monoize_passive_window_seconds.max(1);
        rt.passive_rate_limit_cooldown_seconds =
            updated.monoize_passive_rate_limit_cooldown_seconds.max(1);
        rt.active_enabled = updated.monoize_active_probe_enabled;
        rt.active_interval_seconds = updated.monoize_active_probe_interval_seconds.max(1);
        rt.active_success_threshold = updated.monoize_active_probe_success_threshold.max(1);
        rt.active_probe_model = updated.monoize_active_probe_model.clone();
        rt.global_transforms = updated.global_transforms.clone();
        rt.set_global_model_redirects(updated.global_model_redirects.clone())
            .expect("validated global model redirects compile");
        rt.reasoning_suffix_map = updated.reasoning_suffix_map.clone();
        rt.pricing_profile_model_patterns = updated.pricing_profile_model_patterns.clone();
        rt.codex_model_ids = updated.codex_model_ids.clone();
        rt.extra_fields_whitelist = updated.monoize_extra_fields_whitelist.clone();
        rt.strip_cross_protocol_nested_extra = updated.monoize_strip_cross_protocol_nested_extra;
        rt.request_capture_enabled = updated.monoize_request_capture_enabled;
        rt.request_capture_retention_days = updated.monoize_request_capture_retention_days.max(1);
        rt.mask_sensitive_info = updated.monoize_mask_sensitive_info;
        rt.affinity_enabled = updated.monoize_affinity_enabled;
        rt.affinity_idle_ttl_seconds = updated.monoize_affinity_idle_ttl_seconds.max(1);
        rt.affinity_failback_mode = updated.monoize_affinity_failback_mode;
        rt.affinity_failback_delay_seconds = updated.monoize_affinity_failback_delay_seconds;
    }

    let affinity_settings_after = (
        updated.monoize_affinity_enabled,
        updated.monoize_affinity_idle_ttl_seconds,
        updated.monoize_affinity_failback_mode,
        updated.monoize_affinity_failback_delay_seconds,
    );
    let health_settings_after = (
        updated.monoize_passive_failure_threshold,
        updated.monoize_passive_cooldown_seconds,
        updated.monoize_passive_window_seconds,
        updated.monoize_passive_min_samples,
        updated.monoize_passive_failure_rate_threshold.to_bits(),
        updated.monoize_passive_rate_limit_cooldown_seconds,
        updated.monoize_active_probe_enabled,
        updated.monoize_active_probe_interval_seconds,
        updated.monoize_active_probe_success_threshold,
        updated.monoize_active_probe_model.clone(),
    );
    let affinity_changed = affinity_settings_before != affinity_settings_after;
    let health_changed = health_settings_before != health_settings_after;
    if affinity_changed || health_changed {
        state.routing_config_revision.fetch_add(1, Ordering::AcqRel);
    }
    if affinity_changed {
        state.channel_affinity.lock().await.clear();
    }
    if health_changed {
        state.channel_health.lock().await.clear();
    }

    Ok(Json(updated))
}

pub async fn get_dashboard_stats(
    State(state): State<AppState>,
    headers: HeaderMap,
) -> AppResult<impl IntoResponse> {
    let user = get_current_user(&headers, &state).await?;

    let user_store = &state.user_store;

    let user_count =
        if user.role.can_manage_users() {
            Some(user_store.user_count().await.map_err(|e| {
                AppError::new(StatusCode::INTERNAL_SERVER_ERROR, "internal_error", e)
            })?)
        } else {
            None
        };

    let my_api_keys_count = user_store
        .count_user_api_keys(&user.id)
        .await
        .map_err(|e| AppError::new(StatusCode::INTERNAL_SERVER_ERROR, "internal_error", e))?;

    let providers_count = state
        .monoize_store
        .provider_count()
        .await
        .map_err(|e| AppError::new(StatusCode::INTERNAL_SERVER_ERROR, "internal_error", e))?;

    let current_user = user_response_from_store(user_store, user)
        .await
        .map_err(|e| AppError::new(StatusCode::INTERNAL_SERVER_ERROR, "internal_error", e))?;

    Ok(Json(json!({
        "user_count": user_count,
        "my_api_keys_count": my_api_keys_count,
        "providers_count": providers_count,
        "current_user": current_user,
    })))
}

pub async fn get_config_overview(
    State(state): State<AppState>,
    headers: HeaderMap,
) -> AppResult<impl IntoResponse> {
    require_admin(&headers, &state).await?;
    let providers_count = state
        .monoize_store
        .provider_count()
        .await
        .map_err(|e| AppError::new(StatusCode::INTERNAL_SERVER_ERROR, "internal_error", e))?;

    Ok(Json(json!({
        "server": {
            "listen": state.runtime.listen.clone(),
            "metrics_path": state.runtime.metrics_path.clone(),
        },
        "database": {
            "dsn": redact_dsn(&state.runtime.database_dsn),
        },
        "routing": {
            "providers_count": providers_count,
        }
    })))
}

pub async fn get_public_settings(State(state): State<AppState>) -> AppResult<impl IntoResponse> {
    let mut settings = state
        .settings_store
        .get_public_settings()
        .await
        .map_err(|e| AppError::new(StatusCode::INTERNAL_SERVER_ERROR, "internal_error", e))?;
    settings.cap_api_endpoint = settings
        .captcha_enabled
        .then(|| state.cap_verifier.public_api_endpoint().to_string());
    Ok(Json(settings))
}

/// Redact credentials from a DSN string.
/// e.g. postgres://user:password@host/db → postgres://***@host/db
fn redact_dsn(dsn: &str) -> String {
    if let Some(at_pos) = dsn.find('@') {
        if let Some(scheme_end) = dsn.find("://") {
            return format!("{}://***@{}", &dsn[..scheme_end], &dsn[at_pos + 1..]);
        }
    }
    if dsn.starts_with("sqlite") {
        return dsn.to_string();
    }
    "***".to_string()
}

#[cfg(test)]
mod tests {
    use super::get_dashboard_stats;
    use crate::app::{RuntimeConfig, load_state_with_runtime};
    use crate::users::UserRole;
    use axum::extract::State;
    use axum::http::{HeaderMap, HeaderValue};
    use axum::response::IntoResponse;
    use http_body_util::BodyExt;
    use serde_json::Value;

    async fn stats_for_session(
        state: crate::app::AppState,
        token: &str,
    ) -> serde_json::Map<String, Value> {
        let mut headers = HeaderMap::new();
        headers.insert(
            "authorization",
            HeaderValue::from_str(&format!("Bearer {token}")).expect("header value"),
        );
        let response = get_dashboard_stats(State(state), headers)
            .await
            .expect("stats handler succeeds")
            .into_response();
        let bytes = response
            .into_body()
            .collect()
            .await
            .expect("stats body collects")
            .to_bytes();
        serde_json::from_slice::<Value>(&bytes)
            .expect("stats body is JSON")
            .as_object()
            .expect("stats body is an object")
            .clone()
    }

    #[tokio::test]
    async fn dashboard_stats_hides_global_user_count_from_non_admins() {
        let state = load_state_with_runtime(RuntimeConfig {
            listen: "127.0.0.1:0".to_string(),
            metrics_path: "/metrics".to_string(),
            database_dsn: "sqlite::memory:".to_string(),
            request_log_spool_dir: None,
            node: crate::node_config::NodeSettings::primary_default(),
        })
        .await
        .expect("state loads");

        let admin = state
            .user_store
            .create_user("stats_admin", "password123", UserRole::Admin, None)
            .await
            .expect("admin created");
        let admin_session = state
            .user_store
            .create_session(&admin.id, 7)
            .await
            .expect("admin session created");
        let user = state
            .user_store
            .create_user("stats_reader", "password123", UserRole::User, None)
            .await
            .expect("user created");
        let user_session = state
            .user_store
            .create_session(&user.id, 7)
            .await
            .expect("user session created");

        let admin_stats = stats_for_session(state.clone(), &admin_session.token).await;
        assert!(admin_stats["user_count"].as_i64().is_some());

        let user_stats = stats_for_session(state, &user_session.token).await;
        assert_eq!(user_stats["user_count"], Value::Null);
    }
}
