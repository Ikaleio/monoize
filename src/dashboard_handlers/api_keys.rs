use crate::app::AppState;
use crate::dashboard_handlers::session_helpers::get_current_user;
use crate::error::{AppError, AppResult};
use crate::transforms::TransformRuleConfig;
use axum::Json;
use axum::extract::{Path, State};
use axum::http::{HeaderMap, StatusCode};
use axum::response::IntoResponse;
use serde::{Deserialize, Serialize};
use serde_json::json;

#[derive(Debug, Deserialize)]
pub struct CreateApiKeyRequest {
    pub name: String,
    pub expires_in_days: Option<i64>,
    pub quota: Option<i32>,
    #[serde(default = "default_quota_unlimited")]
    pub quota_unlimited: bool,
    #[serde(default)]
    pub model_limits_enabled: bool,
    #[serde(default)]
    pub model_limits: Vec<String>,
    #[serde(default)]
    pub ip_whitelist: Vec<String>,
    #[serde(default = "default_group")]
    pub group: String,
    #[serde(default)]
    pub max_multiplier: Option<f64>,
    #[serde(default)]
    pub transforms: Vec<TransformRuleConfig>,
}

fn default_quota_unlimited() -> bool {
    true
}

fn default_group() -> String {
    "default".to_string()
}

#[derive(Debug, Serialize)]
pub struct ApiKeyResponse {
    pub id: String,
    pub name: String,
    pub key_prefix: String,
    pub key: String,
    pub created_at: String,
    pub expires_at: Option<String>,
    pub last_used_at: Option<String>,
    pub enabled: bool,
    pub quota_remaining: Option<i32>,
    pub quota_unlimited: bool,
    pub model_limits_enabled: bool,
    pub model_limits: Vec<String>,
    pub ip_whitelist: Vec<String>,
    pub group: String,
    pub max_multiplier: Option<f64>,
    pub transforms: Vec<TransformRuleConfig>,
}

#[derive(Debug, Serialize)]
pub struct ApiKeyCreatedResponse {
    pub id: String,
    pub name: String,
    pub key: String,
    pub key_prefix: String,
    pub created_at: String,
    pub expires_at: Option<String>,
    pub quota_remaining: Option<i32>,
    pub quota_unlimited: bool,
    pub model_limits_enabled: bool,
    pub model_limits: Vec<String>,
    pub ip_whitelist: Vec<String>,
    pub group: String,
    pub max_multiplier: Option<f64>,
    pub transforms: Vec<TransformRuleConfig>,
}

#[derive(Debug, Deserialize)]
pub struct UpdateApiKeyRequest {
    pub name: Option<String>,
    pub enabled: Option<bool>,
    pub quota: Option<i32>,
    pub quota_unlimited: Option<bool>,
    pub model_limits_enabled: Option<bool>,
    pub model_limits: Option<Vec<String>>,
    pub ip_whitelist: Option<Vec<String>>,
    pub group: Option<String>,
    pub max_multiplier: Option<f64>,
    pub transforms: Option<Vec<TransformRuleConfig>>,
    pub expires_at: Option<String>,
}

#[derive(Debug, Deserialize)]
pub struct BatchDeleteApiKeysRequest {
    pub ids: Vec<String>,
}

pub async fn list_my_api_keys(
    State(state): State<AppState>,
    headers: HeaderMap,
) -> AppResult<impl IntoResponse> {
    let user = get_current_user(&headers, &state).await?;

    let user_store = &state.user_store;

    let keys = user_store
        .list_user_api_keys(&user.id)
        .await
        .map_err(|e| AppError::new(StatusCode::INTERNAL_SERVER_ERROR, "internal_error", e))?;

    let responses: Vec<ApiKeyResponse> = keys
        .into_iter()
        .map(|k| ApiKeyResponse {
            id: k.id,
            name: k.name,
            key_prefix: k.key_prefix,
            key: k.key,
            created_at: k.created_at.to_rfc3339(),
            expires_at: k.expires_at.map(|d| d.to_rfc3339()),
            last_used_at: k.last_used_at.map(|d| d.to_rfc3339()),
            enabled: k.enabled,
            quota_remaining: k.quota_remaining,
            quota_unlimited: k.quota_unlimited,
            model_limits_enabled: k.model_limits_enabled,
            model_limits: k.model_limits,
            ip_whitelist: k.ip_whitelist,
            group: k.group,
            max_multiplier: k.max_multiplier,
            transforms: k.transforms,
        })
        .collect();

    Ok(Json(responses))
}

pub async fn create_api_key(
    State(state): State<AppState>,
    headers: HeaderMap,
    Json(body): Json<CreateApiKeyRequest>,
) -> AppResult<impl IntoResponse> {
    let user = get_current_user(&headers, &state).await?;

    let user_store = &state.user_store;
    let settings_store = &state.settings_store;

    let settings = settings_store
        .get_all()
        .await
        .map_err(|e| AppError::new(StatusCode::INTERNAL_SERVER_ERROR, "internal_error", e))?;

    let existing_keys = user_store
        .list_user_api_keys(&user.id)
        .await
        .map_err(|e| AppError::new(StatusCode::INTERNAL_SERVER_ERROR, "internal_error", e))?;

    if existing_keys.len() >= settings.api_key_max_per_user as usize {
        return Err(AppError::new(
            StatusCode::FORBIDDEN,
            "max_api_keys_reached",
            format!(
                "maximum of {} API keys allowed per user",
                settings.api_key_max_per_user
            ),
        ));
    }

    use crate::users::CreateApiKeyInput;
    let input = CreateApiKeyInput {
        name: body.name,
        expires_in_days: body.expires_in_days,
        quota: body.quota,
        quota_unlimited: body.quota_unlimited,
        model_limits_enabled: body.model_limits_enabled,
        model_limits: body.model_limits,
        ip_whitelist: body.ip_whitelist,
        group: body.group,
        max_multiplier: body.max_multiplier,
        transforms: body.transforms,
    };

    let is_admin = user.role.can_manage_system();

    let (api_key, key) = user_store
        .create_api_key_extended(&user.id, input, is_admin)
        .await
        .map_err(|e| AppError::new(StatusCode::BAD_REQUEST, "invalid_request", e))?;

    state
        .name_caches
        .api_keys
        .insert(api_key.id.clone(), api_key.name.clone());
    Ok((
        StatusCode::CREATED,
        Json(ApiKeyCreatedResponse {
            id: api_key.id,
            name: api_key.name,
            key,
            key_prefix: api_key.key_prefix,
            created_at: api_key.created_at.to_rfc3339(),
            expires_at: api_key.expires_at.map(|d| d.to_rfc3339()),
            quota_remaining: api_key.quota_remaining,
            quota_unlimited: api_key.quota_unlimited,
            model_limits_enabled: api_key.model_limits_enabled,
            model_limits: api_key.model_limits,
            ip_whitelist: api_key.ip_whitelist,
            group: api_key.group,
            max_multiplier: api_key.max_multiplier,
            transforms: api_key.transforms,
        }),
    ))
}

pub async fn delete_api_key(
    State(state): State<AppState>,
    headers: HeaderMap,
    Path(key_id): Path<String>,
) -> AppResult<impl IntoResponse> {
    let user = get_current_user(&headers, &state).await?;

    let user_store = &state.user_store;

    let keys = user_store
        .list_user_api_keys(&user.id)
        .await
        .map_err(|e| AppError::new(StatusCode::INTERNAL_SERVER_ERROR, "internal_error", e))?;

    if !keys.iter().any(|k| k.id == key_id) {
        return Err(AppError::new(
            StatusCode::NOT_FOUND,
            "not_found",
            "API key not found",
        ));
    }

    user_store
        .delete_api_key(&key_id)
        .await
        .map_err(|e| AppError::new(StatusCode::INTERNAL_SERVER_ERROR, "internal_error", e))?;

    state.name_caches.api_keys.remove(&key_id);
    Ok(Json(json!({ "success": true })))
}

pub async fn get_api_key(
    State(state): State<AppState>,
    headers: HeaderMap,
    Path(key_id): Path<String>,
) -> AppResult<impl IntoResponse> {
    let user = get_current_user(&headers, &state).await?;

    let user_store = &state.user_store;

    let api_key = user_store
        .get_api_key_by_id(&key_id)
        .await
        .map_err(|e| AppError::new(StatusCode::INTERNAL_SERVER_ERROR, "internal_error", e))?;

    let api_key = api_key
        .ok_or_else(|| AppError::new(StatusCode::NOT_FOUND, "not_found", "API key not found"))?;

    if api_key.user_id != user.id {
        return Err(AppError::new(
            StatusCode::NOT_FOUND,
            "not_found",
            "API key not found",
        ));
    }

    Ok(Json(ApiKeyResponse {
        id: api_key.id,
        name: api_key.name,
        key_prefix: api_key.key_prefix,
        key: api_key.key,
        created_at: api_key.created_at.to_rfc3339(),
        expires_at: api_key.expires_at.map(|d| d.to_rfc3339()),
        last_used_at: api_key.last_used_at.map(|d| d.to_rfc3339()),
        enabled: api_key.enabled,
        quota_remaining: api_key.quota_remaining,
        quota_unlimited: api_key.quota_unlimited,
        model_limits_enabled: api_key.model_limits_enabled,
        model_limits: api_key.model_limits,
        ip_whitelist: api_key.ip_whitelist,
        group: api_key.group,
        max_multiplier: api_key.max_multiplier,
        transforms: api_key.transforms,
    }))
}

pub async fn update_api_key(
    State(state): State<AppState>,
    headers: HeaderMap,
    Path(key_id): Path<String>,
    Json(body): Json<UpdateApiKeyRequest>,
) -> AppResult<impl IntoResponse> {
    let user = get_current_user(&headers, &state).await?;

    let user_store = &state.user_store;

    let keys = user_store
        .list_user_api_keys(&user.id)
        .await
        .map_err(|e| AppError::new(StatusCode::INTERNAL_SERVER_ERROR, "internal_error", e))?;

    if !keys.iter().any(|k| k.id == key_id) {
        return Err(AppError::new(
            StatusCode::NOT_FOUND,
            "not_found",
            "API key not found",
        ));
    }

    use crate::users::UpdateApiKeyInput;
    let input = UpdateApiKeyInput {
        name: body.name,
        enabled: body.enabled,
        quota: body.quota,
        quota_unlimited: body.quota_unlimited,
        model_limits_enabled: body.model_limits_enabled,
        model_limits: body.model_limits,
        ip_whitelist: body.ip_whitelist,
        group: body.group,
        max_multiplier: body.max_multiplier,
        transforms: body.transforms,
        expires_at: body.expires_at,
    };

    let is_admin = user.role.can_manage_system();

    let updated_key = user_store
        .update_api_key(&key_id, input, is_admin)
        .await
        .map_err(|e| AppError::new(StatusCode::BAD_REQUEST, "invalid_request", e))?;

    state
        .name_caches
        .api_keys
        .insert(updated_key.id.clone(), updated_key.name.clone());
    Ok(Json(ApiKeyResponse {
        id: updated_key.id,
        name: updated_key.name,
        key_prefix: updated_key.key_prefix,
        key: updated_key.key,
        created_at: updated_key.created_at.to_rfc3339(),
        expires_at: updated_key.expires_at.map(|d| d.to_rfc3339()),
        last_used_at: updated_key.last_used_at.map(|d| d.to_rfc3339()),
        enabled: updated_key.enabled,
        quota_remaining: updated_key.quota_remaining,
        quota_unlimited: updated_key.quota_unlimited,
        model_limits_enabled: updated_key.model_limits_enabled,
        model_limits: updated_key.model_limits,
        ip_whitelist: updated_key.ip_whitelist,
        group: updated_key.group,
        max_multiplier: updated_key.max_multiplier,
        transforms: updated_key.transforms,
    }))
}

pub async fn batch_delete_api_keys(
    State(state): State<AppState>,
    headers: HeaderMap,
    Json(body): Json<BatchDeleteApiKeysRequest>,
) -> AppResult<impl IntoResponse> {
    let user = get_current_user(&headers, &state).await?;

    let user_store = &state.user_store;

    let keys = user_store
        .list_user_api_keys(&user.id)
        .await
        .map_err(|e| AppError::new(StatusCode::INTERNAL_SERVER_ERROR, "internal_error", e))?;

    let user_key_ids: std::collections::HashSet<String> =
        keys.iter().map(|k| k.id.clone()).collect();
    let ids_to_delete: Vec<String> = body
        .ids
        .into_iter()
        .filter(|id| user_key_ids.contains(id))
        .collect();

    let deleted_count = user_store
        .batch_delete_api_keys(&ids_to_delete)
        .await
        .map_err(|e| AppError::new(StatusCode::INTERNAL_SERVER_ERROR, "internal_error", e))?;

    for id in &ids_to_delete {
        state.name_caches.api_keys.remove(id);
    }
    Ok(Json(
        json!({ "success": true, "deleted_count": deleted_count }),
    ))
}

pub async fn get_apikey_presets() -> AppResult<impl IntoResponse> {
    Ok(Json(crate::presets::apikey_presets()))
}
