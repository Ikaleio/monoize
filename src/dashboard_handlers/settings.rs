use crate::app::AppState;
use crate::dashboard_handlers::auth::UserResponse;
use crate::dashboard_handlers::session_helpers::{get_current_user, require_admin};
use crate::error::{AppError, AppResult};
use axum::Json;
use axum::extract::State;
use axum::http::{HeaderMap, StatusCode};
use axum::response::IntoResponse;
use serde::Deserialize;
use serde_json::json;

#[derive(Debug, Deserialize)]
pub struct UpdateSettingsRequest {
    pub registration_enabled: Option<bool>,
    pub default_user_role: Option<String>,
    pub session_ttl_days: Option<i64>,
    pub api_key_max_per_user: Option<i64>,
    pub site_name: Option<String>,
    pub site_description: Option<String>,
    pub api_base_url: Option<String>,
    pub reasoning_suffix_map: Option<std::collections::HashMap<String, String>>,
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

    let settings_store = &state.settings_store;

    let mut settings = settings_store
        .get_all()
        .await
        .map_err(|e| AppError::new(StatusCode::INTERNAL_SERVER_ERROR, "internal_error", e))?;

    if let Some(v) = body.registration_enabled {
        settings.registration_enabled = v;
    }
    if let Some(v) = body.default_user_role {
        settings.default_user_role = v;
    }
    if let Some(v) = body.session_ttl_days {
        settings.session_ttl_days = v;
    }
    if let Some(v) = body.api_key_max_per_user {
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
    if let Some(v) = body.reasoning_suffix_map {
        settings.reasoning_suffix_map = v;
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

    settings_store
        .update_all(&settings)
        .await
        .map_err(|e| AppError::new(StatusCode::INTERNAL_SERVER_ERROR, "internal_error", e))?;

    let updated = settings_store
        .get_all()
        .await
        .map_err(|e| AppError::new(StatusCode::INTERNAL_SERVER_ERROR, "internal_error", e))?;

    {
        let mut rt = state.monoize_runtime.write().await;
        rt.request_timeout_ms = updated.monoize_request_timeout_ms.max(1);
        rt.passive_failure_threshold = updated.monoize_passive_failure_threshold.max(1);
        rt.passive_cooldown_seconds = updated.monoize_passive_cooldown_seconds.max(1);
        rt.passive_window_seconds = updated.monoize_passive_window_seconds.max(1);
        rt.passive_min_samples = updated.monoize_passive_min_samples.max(1);
        rt.passive_failure_rate_threshold = updated
            .monoize_passive_failure_rate_threshold
            .clamp(0.01, 1.0);
        rt.passive_rate_limit_cooldown_seconds = updated.monoize_passive_rate_limit_cooldown_seconds.max(1);
        rt.active_enabled = updated.monoize_active_probe_enabled;
        rt.active_interval_seconds = updated.monoize_active_probe_interval_seconds.max(1);
        rt.active_success_threshold = updated.monoize_active_probe_success_threshold.max(1);
        rt.active_probe_model = updated.monoize_active_probe_model.clone();
    }

    Ok(Json(updated))
}

pub async fn get_dashboard_stats(
    State(state): State<AppState>,
    headers: HeaderMap,
) -> AppResult<impl IntoResponse> {
    let user = get_current_user(&headers, &state).await?;

    let user_store = &state.user_store;

    let user_count = user_store
        .user_count()
        .await
        .map_err(|e| AppError::new(StatusCode::INTERNAL_SERVER_ERROR, "internal_error", e))?;

    let my_api_keys = user_store
        .list_user_api_keys(&user.id)
        .await
        .map_err(|e| AppError::new(StatusCode::INTERNAL_SERVER_ERROR, "internal_error", e))?;

    let providers_count = state
        .monoize_store
        .provider_count()
        .await
        .map_err(|e| AppError::new(StatusCode::INTERNAL_SERVER_ERROR, "internal_error", e))?;

    Ok(Json(json!({
        "user_count": user_count,
        "my_api_keys_count": my_api_keys.len(),
        "providers_count": providers_count,
        "current_user": UserResponse::from(user),
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
    let settings_store = &state.settings_store;

    let settings = settings_store
        .get_all()
        .await
        .map_err(|e| AppError::new(StatusCode::INTERNAL_SERVER_ERROR, "internal_error", e))?;

    Ok(Json(json!({
        "registration_enabled": settings.registration_enabled,
        "site_name": settings.site_name,
        "site_description": settings.site_description,
        "api_base_url": settings.api_base_url,
    })))
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
