use crate::app::AppState;
use crate::error::{AppError, AppResult};
use crate::monoize_routing::{
    ChannelHealthState, CreateMonoizeProviderInput, MonoizeChannel, MonoizeProvider,
    ReorderProvidersInput, UpdateMonoizeProviderInput,
};
use crate::transforms::TransformRuleConfig;
use crate::users::{User, UserRole, UserStore, format_nano_to_usd, parse_usd_to_nano};
use chrono::Utc;
use axum::Json;
use axum::extract::{Path, State};
use axum::http::{HeaderMap, StatusCode};
use axum::response::IntoResponse;
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};

fn is_valid_username(username: &str) -> bool {
    (3..=22).contains(&username.len())
        && username
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || c == '_')
}

fn is_reserved_internal_username(username: &str) -> bool {
    UserStore::is_reserved_internal_username(username)
}

#[derive(Debug, Deserialize)]
pub struct RegisterRequest {
    pub username: String,
    pub password: String,
}

#[derive(Debug, Deserialize)]
pub struct LoginRequest {
    pub username: String,
    pub password: String,
}

#[derive(Debug, Serialize)]
pub struct AuthResponse {
    pub token: String,
    pub user: UserResponse,
}

#[derive(Debug, Serialize)]
pub struct UserResponse {
    pub id: String,
    pub username: String,
    pub role: UserRole,
    pub created_at: String,
    pub last_login_at: Option<String>,
    pub enabled: bool,
    pub balance_nano_usd: String,
    pub balance_usd: String,
    pub balance_unlimited: bool,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub email: Option<String>,
}

impl From<User> for UserResponse {
    fn from(u: User) -> Self {
        Self {
            id: u.id,
            username: u.username,
            role: u.role,
            created_at: u.created_at.to_rfc3339(),
            last_login_at: u.last_login_at.map(|d| d.to_rfc3339()),
            enabled: u.enabled,
            balance_usd: format_nano_to_usd(u.balance_nano_usd.parse::<i128>().unwrap_or(0)),
            balance_nano_usd: u.balance_nano_usd,
            balance_unlimited: u.balance_unlimited,
            email: u.email,
        }
    }
}

#[derive(Debug, Deserialize)]
pub struct CreateUserRequest {
    pub username: String,
    pub password: String,
    pub role: Option<String>,
}

#[derive(Debug, Deserialize)]
pub struct UpdateUserRequest {
    pub username: Option<String>,
    pub password: Option<String>,
    pub role: Option<String>,
    pub enabled: Option<bool>,
    pub balance_nano_usd: Option<String>,
    pub balance_usd: Option<String>,
    pub balance_unlimited: Option<bool>,
    pub email: Option<Option<String>>,
}

#[derive(Debug, Deserialize)]
pub struct CreateApiKeyRequest {
    pub name: String,
    pub expires_in_days: Option<i64>,
    pub quota: Option<i64>,
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
    pub quota_remaining: Option<i64>,
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
    pub quota_remaining: Option<i64>,
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
    pub quota: Option<i64>,
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

fn extract_session_token(headers: &HeaderMap) -> Option<String> {
    headers
        .get("authorization")
        .and_then(|v| v.to_str().ok())
        .and_then(|v| v.strip_prefix("Bearer "))
        .map(|s| s.to_string())
}

async fn get_current_user(headers: &HeaderMap, state: &AppState) -> AppResult<User> {
    let token = extract_session_token(headers).ok_or_else(|| {
        AppError::new(
            StatusCode::UNAUTHORIZED,
            "unauthorized",
            "missing authorization header",
        )
    })?;

    let user_store = &state.user_store;

    let session = user_store
        .get_session_by_token(&token)
        .await
        .map_err(|e| AppError::new(StatusCode::INTERNAL_SERVER_ERROR, "internal_error", e))?
        .ok_or_else(|| {
            AppError::new(
                StatusCode::UNAUTHORIZED,
                "unauthorized",
                "invalid or expired session",
            )
        })?;

    let user = user_store
        .get_user_by_id(&session.user_id)
        .await
        .map_err(|e| AppError::new(StatusCode::INTERNAL_SERVER_ERROR, "internal_error", e))?
        .ok_or_else(|| AppError::new(StatusCode::UNAUTHORIZED, "unauthorized", "user not found"))?;

    if !user.enabled {
        return Err(AppError::new(
            StatusCode::FORBIDDEN,
            "forbidden",
            "user account is disabled",
        ));
    }

    Ok(user)
}

async fn require_admin(headers: &HeaderMap, state: &AppState) -> AppResult<User> {
    let user = get_current_user(headers, state).await?;
    if !user.role.can_manage_users() {
        return Err(AppError::new(
            StatusCode::FORBIDDEN,
            "forbidden",
            "admin access required",
        ));
    }
    Ok(user)
}

pub async fn register(
    State(state): State<AppState>,
    Json(body): Json<RegisterRequest>,
) -> AppResult<impl IntoResponse> {
    let user_store = &state.user_store;
    let settings_store = &state.settings_store;

    let user_count = user_store
        .user_count()
        .await
        .map_err(|e| AppError::new(StatusCode::INTERNAL_SERVER_ERROR, "internal_error", e))?;

    let is_first_user = user_count == 0;

    if !is_first_user {
        let registration_enabled = settings_store
            .is_registration_enabled()
            .await
            .map_err(|e| AppError::new(StatusCode::INTERNAL_SERVER_ERROR, "internal_error", e))?;

        if !registration_enabled {
            return Err(AppError::new(
                StatusCode::FORBIDDEN,
                "registration_disabled",
                "user registration is currently disabled",
            ));
        }
    }

    if !is_valid_username(&body.username) {
        return Err(AppError::new(
            StatusCode::BAD_REQUEST,
            "invalid_username",
            "username must be 3-22 characters, only letters, digits and underscores",
        ));
    }

    if is_reserved_internal_username(&body.username) {
        return Err(AppError::new(
            StatusCode::BAD_REQUEST,
            "reserved_username",
            "username prefix _monoize_ is reserved",
        ));
    }

    if body.password.len() < 8 {
        return Err(AppError::new(
            StatusCode::BAD_REQUEST,
            "invalid_password",
            "password must be at least 8 characters",
        ));
    }

    if user_store
        .get_user_by_username(&body.username)
        .await
        .map_err(|e| AppError::new(StatusCode::INTERNAL_SERVER_ERROR, "internal_error", e))?
        .is_some()
    {
        return Err(AppError::new(
            StatusCode::CONFLICT,
            "username_exists",
            "username already exists",
        ));
    }

    let role = if is_first_user {
        UserRole::SuperAdmin
    } else {
        UserRole::User
    };

    let user = user_store
        .create_user(&body.username, &body.password, role)
        .await
        .map_err(|e| AppError::new(StatusCode::INTERNAL_SERVER_ERROR, "internal_error", e))?;

    let session = user_store
        .create_session(&user.id)
        .await
        .map_err(|e| AppError::new(StatusCode::INTERNAL_SERVER_ERROR, "internal_error", e))?;

    Ok(Json(AuthResponse {
        token: session.token,
        user: user.into(),
    }))
}

pub async fn login(
    State(state): State<AppState>,
    Json(body): Json<LoginRequest>,
) -> AppResult<impl IntoResponse> {
    let user_store = &state.user_store;

    if is_reserved_internal_username(&body.username) {
        return Err(AppError::new(
            StatusCode::FORBIDDEN,
            "reserved_username",
            "username prefix _monoize_ is reserved",
        ));
    }

    let user = user_store
        .get_user_by_username(&body.username)
        .await
        .map_err(|e| AppError::new(StatusCode::INTERNAL_SERVER_ERROR, "internal_error", e))?
        .ok_or_else(|| {
            AppError::new(
                StatusCode::UNAUTHORIZED,
                "invalid_credentials",
                "invalid username or password",
            )
        })?;

    if !user.enabled {
        return Err(AppError::new(
            StatusCode::FORBIDDEN,
            "account_disabled",
            "your account has been disabled",
        ));
    }

    let valid = UserStore::verify_password(&body.password, &user.password_hash)
        .map_err(|e| AppError::new(StatusCode::INTERNAL_SERVER_ERROR, "internal_error", e))?;

    if !valid {
        return Err(AppError::new(
            StatusCode::UNAUTHORIZED,
            "invalid_credentials",
            "invalid username or password",
        ));
    }

    user_store.update_last_login(&user.id).await.ok();

    let session = user_store
        .create_session(&user.id)
        .await
        .map_err(|e| AppError::new(StatusCode::INTERNAL_SERVER_ERROR, "internal_error", e))?;

    Ok(Json(AuthResponse {
        token: session.token,
        user: user.into(),
    }))
}

pub async fn logout(
    State(state): State<AppState>,
    headers: HeaderMap,
) -> AppResult<impl IntoResponse> {
    let token = extract_session_token(&headers).ok_or_else(|| {
        AppError::new(
            StatusCode::UNAUTHORIZED,
            "unauthorized",
            "missing authorization header",
        )
    })?;

    let user_store = &state.user_store;

    user_store.delete_session(&token).await.ok();

    Ok(Json(json!({ "success": true })))
}

pub async fn get_me(
    State(state): State<AppState>,
    headers: HeaderMap,
) -> AppResult<impl IntoResponse> {
    let user = get_current_user(&headers, &state).await?;
    Ok(Json(UserResponse::from(user)))
}

#[derive(Debug, Deserialize)]
pub struct UpdateMeRequest {
    pub email: Option<Option<String>>,
}

pub async fn update_me(
    State(state): State<AppState>,
    headers: HeaderMap,
    Json(body): Json<UpdateMeRequest>,
) -> AppResult<impl IntoResponse> {
    let user = get_current_user(&headers, &state).await?;

    let user_store = &state.user_store;

    user_store
        .update_user(
            &user.id,
            None,
            None,
            None,
            None,
            None,
            None,
            body.email.as_ref().map(|e| e.as_deref()),
        )
        .await
        .map_err(|e| AppError::new(StatusCode::INTERNAL_SERVER_ERROR, "internal_error", e))?;

    let updated_user = user_store
        .get_user_by_id(&user.id)
        .await
        .map_err(|e| AppError::new(StatusCode::INTERNAL_SERVER_ERROR, "internal_error", e))?
        .ok_or_else(|| AppError::new(StatusCode::NOT_FOUND, "not_found", "user not found"))?;

    Ok(Json(UserResponse::from(updated_user)))
}

pub async fn list_users(
    State(state): State<AppState>,
    headers: HeaderMap,
) -> AppResult<impl IntoResponse> {
    require_admin(&headers, &state).await?;

    let user_store = &state.user_store;

    let users = user_store
        .list_users()
        .await
        .map_err(|e| AppError::new(StatusCode::INTERNAL_SERVER_ERROR, "internal_error", e))?;

    let responses: Vec<UserResponse> = users.into_iter().map(UserResponse::from).collect();
    Ok(Json(responses))
}

pub async fn get_user(
    State(state): State<AppState>,
    headers: HeaderMap,
    Path(user_id): Path<String>,
) -> AppResult<impl IntoResponse> {
    require_admin(&headers, &state).await?;

    let user_store = &state.user_store;

    let user = user_store
        .get_user_by_id(&user_id)
        .await
        .map_err(|e| AppError::new(StatusCode::INTERNAL_SERVER_ERROR, "internal_error", e))?
        .ok_or_else(|| AppError::new(StatusCode::NOT_FOUND, "not_found", "user not found"))?;

    Ok(Json(UserResponse::from(user)))
}

pub async fn create_user(
    State(state): State<AppState>,
    headers: HeaderMap,
    Json(body): Json<CreateUserRequest>,
) -> AppResult<impl IntoResponse> {
    let current_user = require_admin(&headers, &state).await?;

    let user_store = &state.user_store;

    let role = body
        .role
        .as_ref()
        .and_then(|r| UserRole::from_str(r))
        .unwrap_or(UserRole::User);

    if !current_user.role.can_assign_role(role) {
        return Err(AppError::new(
            StatusCode::FORBIDDEN,
            "forbidden",
            "you cannot assign this role",
        ));
    }

    if !is_valid_username(&body.username) {
        return Err(AppError::new(
            StatusCode::BAD_REQUEST,
            "invalid_username",
            "username must be 3-22 characters, only letters, digits and underscores",
        ));
    }

    if is_reserved_internal_username(&body.username) {
        return Err(AppError::new(
            StatusCode::BAD_REQUEST,
            "reserved_username",
            "username prefix _monoize_ is reserved",
        ));
    }

    if body.password.len() < 8 {
        return Err(AppError::new(
            StatusCode::BAD_REQUEST,
            "invalid_password",
            "password must be at least 8 characters",
        ));
    }

    if user_store
        .get_user_by_username(&body.username)
        .await
        .map_err(|e| AppError::new(StatusCode::INTERNAL_SERVER_ERROR, "internal_error", e))?
        .is_some()
    {
        return Err(AppError::new(
            StatusCode::CONFLICT,
            "username_exists",
            "username already exists",
        ));
    }

    let user = user_store
        .create_user(&body.username, &body.password, role)
        .await
        .map_err(|e| AppError::new(StatusCode::INTERNAL_SERVER_ERROR, "internal_error", e))?;

    Ok((StatusCode::CREATED, Json(UserResponse::from(user))))
}

pub async fn update_user(
    State(state): State<AppState>,
    headers: HeaderMap,
    Path(user_id): Path<String>,
    Json(body): Json<UpdateUserRequest>,
) -> AppResult<impl IntoResponse> {
    let current_user = require_admin(&headers, &state).await?;

    let user_store = &state.user_store;

    let target_user = user_store
        .get_user_by_id(&user_id)
        .await
        .map_err(|e| AppError::new(StatusCode::INTERNAL_SERVER_ERROR, "internal_error", e))?
        .ok_or_else(|| AppError::new(StatusCode::NOT_FOUND, "not_found", "user not found"))?;

    if target_user.role == UserRole::SuperAdmin && current_user.role != UserRole::SuperAdmin {
        return Err(AppError::new(
            StatusCode::FORBIDDEN,
            "forbidden",
            "only super admin can modify super admin accounts",
        ));
    }

    let new_role = body.role.as_ref().and_then(|r| UserRole::from_str(r));
    if let Some(role) = new_role {
        if !current_user.role.can_assign_role(role) {
            return Err(AppError::new(
                StatusCode::FORBIDDEN,
                "forbidden",
                "you cannot assign this role",
            ));
        }
    }

    if let Some(ref username) = body.username {
        if !is_valid_username(username) {
            return Err(AppError::new(
                StatusCode::BAD_REQUEST,
                "invalid_username",
                "username must be 3-22 characters, only letters, digits and underscores",
            ));
        }
        if is_reserved_internal_username(username) {
            return Err(AppError::new(
                StatusCode::BAD_REQUEST,
                "reserved_username",
                "username prefix _monoize_ is reserved",
            ));
        }
    }

    if let Some(ref password) = body.password {
        if password.len() < 8 {
            return Err(AppError::new(
                StatusCode::BAD_REQUEST,
                "invalid_password",
                "password must be at least 8 characters",
            ));
        }
    }

    let balance_nano_override = if let Some(ref raw_nano) = body.balance_nano_usd {
        Some(raw_nano.clone())
    } else if let Some(ref raw_usd) = body.balance_usd {
        Some(
            parse_usd_to_nano(raw_usd)
                .map_err(|_| {
                    AppError::new(
                        StatusCode::BAD_REQUEST,
                        "invalid_balance",
                        "invalid balance_usd",
                    )
                })?
                .to_string(),
        )
    } else {
        None
    };

    user_store
        .update_user(
            &user_id,
            body.username.as_deref(),
            body.password.as_deref(),
            new_role,
            body.enabled,
            None,
            None,
            body.email.as_ref().map(|e| e.as_deref()),
        )
        .await
        .map_err(|e| AppError::new(StatusCode::INTERNAL_SERVER_ERROR, "internal_error", e))?;

    if balance_nano_override.is_some() || body.balance_unlimited.is_some() {
        user_store
            .admin_adjust_user_balance(
                &user_id,
                balance_nano_override,
                body.balance_unlimited,
                &current_user.id,
            )
            .await
            .map_err(|e| {
                if e.contains("not found") {
                    AppError::new(StatusCode::NOT_FOUND, "not_found", e)
                } else if e.contains("invalid") {
                    AppError::new(StatusCode::BAD_REQUEST, "invalid_balance", e)
                } else {
                    AppError::new(StatusCode::INTERNAL_SERVER_ERROR, "internal_error", e)
                }
            })?;
    }

    let updated_user = user_store
        .get_user_by_id(&user_id)
        .await
        .map_err(|e| AppError::new(StatusCode::INTERNAL_SERVER_ERROR, "internal_error", e))?
        .ok_or_else(|| AppError::new(StatusCode::NOT_FOUND, "not_found", "user not found"))?;

    Ok(Json(UserResponse::from(updated_user)))
}

pub async fn delete_user(
    State(state): State<AppState>,
    headers: HeaderMap,
    Path(user_id): Path<String>,
) -> AppResult<impl IntoResponse> {
    let current_user = require_admin(&headers, &state).await?;

    let user_store = &state.user_store;

    let target_user = user_store
        .get_user_by_id(&user_id)
        .await
        .map_err(|e| AppError::new(StatusCode::INTERNAL_SERVER_ERROR, "internal_error", e))?
        .ok_or_else(|| AppError::new(StatusCode::NOT_FOUND, "not_found", "user not found"))?;

    if target_user.role == UserRole::SuperAdmin {
        return Err(AppError::new(
            StatusCode::FORBIDDEN,
            "forbidden",
            "cannot delete super admin account",
        ));
    }

    if current_user.id == user_id {
        return Err(AppError::new(
            StatusCode::FORBIDDEN,
            "forbidden",
            "cannot delete your own account",
        ));
    }

    user_store
        .delete_user(&user_id)
        .await
        .map_err(|e| AppError::new(StatusCode::INTERNAL_SERVER_ERROR, "internal_error", e))?;

    Ok(Json(json!({ "success": true })))
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

    let (api_key, key) = user_store
        .create_api_key_extended(&user.id, input)
        .await
        .map_err(|e| AppError::new(StatusCode::INTERNAL_SERVER_ERROR, "internal_error", e))?;

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

    let updated_key = user_store
        .update_api_key(&key_id, input)
        .await
        .map_err(|e| AppError::new(StatusCode::INTERNAL_SERVER_ERROR, "internal_error", e))?;

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

    Ok(Json(
        json!({ "success": true, "deleted_count": deleted_count }),
    ))
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

    // Refresh the in-memory runtime config so changes take effect immediately
    // without requiring a server restart.
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
        rt.passive_rate_limit_cooldown_seconds =
            updated.monoize_passive_rate_limit_cooldown_seconds.max(1);
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
            "dsn": state.runtime.database_dsn.clone(),
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

fn apply_channel_runtime(channel: &mut MonoizeChannel, health: &ChannelHealthState) {
    let now = chrono::Utc::now().timestamp();
    channel._healthy = Some(health.healthy);
    channel._failure_count = Some(health.failure_count);
    channel._health_status = Some(health.status(now).to_string());
    channel._last_success_at = health
        .last_success_at
        .and_then(|ts| chrono::DateTime::<chrono::Utc>::from_timestamp(ts, 0))
        .map(|t| t.to_rfc3339());
}

async fn provider_with_runtime(state: &AppState, mut provider: MonoizeProvider) -> MonoizeProvider {
    let health = state.channel_health.lock().await;
    for channel in &mut provider.channels {
        let state = health
            .get(&channel.id)
            .cloned()
            .unwrap_or_else(ChannelHealthState::new);
        apply_channel_runtime(channel, &state);
    }
    provider
}

fn provider_pricing_model<'a>(
    logical_model: &'a str,
    model_entry: &'a crate::monoize_routing::MonoizeModelEntry,
) -> &'a str {
    model_entry
        .redirect
        .as_deref()
        .map(str::trim)
        .filter(|v| !v.is_empty())
        .unwrap_or(logical_model)
}

pub async fn list_providers(
    State(state): State<AppState>,
    headers: HeaderMap,
) -> AppResult<impl IntoResponse> {
    require_admin(&headers, &state).await?;

    let providers = state
        .monoize_store
        .list_providers()
        .await
        .map_err(|e| AppError::new(StatusCode::INTERNAL_SERVER_ERROR, "internal_error", e))?;

    let priced_ids = state
        .model_registry_store
        .list_priced_model_ids()
        .await
        .unwrap_or_default();

    let mut out = Vec::with_capacity(providers.len());
    for provider in providers {
        let unpriced_count = provider
            .models
            .iter()
            .filter(|(logical_model, model_entry)| {
                let target_model = provider_pricing_model(logical_model, model_entry);
                !priced_ids.contains(target_model)
            })
            .count();
        let p = provider_with_runtime(&state, provider).await;
        let val = serde_json::to_value(&p).unwrap_or_default();
        if let Value::Object(mut obj) = val {
            obj.insert(
                "unpriced_model_count".to_string(),
                Value::Number(serde_json::Number::from(unpriced_count)),
            );
            out.push(Value::Object(obj));
        } else {
            out.push(val);
        }
    }

    Ok(Json(out))
}

pub async fn get_provider(
    State(state): State<AppState>,
    headers: HeaderMap,
    Path(provider_id): Path<String>,
) -> AppResult<impl IntoResponse> {
    require_admin(&headers, &state).await?;

    let provider = state
        .monoize_store
        .get_provider(&provider_id)
        .await
        .map_err(|e| AppError::new(StatusCode::INTERNAL_SERVER_ERROR, "internal_error", e))?
        .ok_or_else(|| AppError::new(StatusCode::NOT_FOUND, "not_found", "provider not found"))?;

    Ok(Json(provider_with_runtime(&state, provider).await))
}

pub async fn create_provider(
    State(state): State<AppState>,
    headers: HeaderMap,
    Json(body): Json<CreateMonoizeProviderInput>,
) -> AppResult<impl IntoResponse> {
    require_admin(&headers, &state).await?;

    let provider = state
        .monoize_store
        .create_provider(body)
        .await
        .map_err(|e| AppError::new(StatusCode::BAD_REQUEST, "invalid_request", e))?;

    Ok((
        StatusCode::CREATED,
        Json(provider_with_runtime(&state, provider).await),
    ))
}

pub async fn update_provider(
    State(state): State<AppState>,
    headers: HeaderMap,
    Path(provider_id): Path<String>,
    Json(body): Json<UpdateMonoizeProviderInput>,
) -> AppResult<impl IntoResponse> {
    require_admin(&headers, &state).await?;

    let provider = state
        .monoize_store
        .update_provider(&provider_id, body)
        .await
        .map_err(|e| {
            if e.contains("not found") {
                AppError::new(StatusCode::NOT_FOUND, "not_found", e)
            } else {
                AppError::new(StatusCode::BAD_REQUEST, "invalid_request", e)
            }
        })?;

    Ok(Json(provider_with_runtime(&state, provider).await))
}

pub async fn delete_provider(
    State(state): State<AppState>,
    headers: HeaderMap,
    Path(provider_id): Path<String>,
) -> AppResult<impl IntoResponse> {
    require_admin(&headers, &state).await?;

    state
        .monoize_store
        .delete_provider(&provider_id)
        .await
        .map_err(|e| {
            if e.contains("not found") {
                AppError::new(StatusCode::NOT_FOUND, "not_found", e)
            } else {
                AppError::new(StatusCode::INTERNAL_SERVER_ERROR, "internal_error", e)
            }
        })?;

    Ok(Json(json!({ "success": true })))
}

pub async fn reorder_providers(
    State(state): State<AppState>,
    headers: HeaderMap,
    Json(body): Json<ReorderProvidersInput>,
) -> AppResult<impl IntoResponse> {
    require_admin(&headers, &state).await?;

    state
        .monoize_store
        .reorder_providers(body)
        .await
        .map_err(|e| AppError::new(StatusCode::BAD_REQUEST, "invalid_request", e))?;

    Ok(Json(json!({ "success": true })))
}

// Model Registry API endpoints

use crate::model_registry_store::{
    CreateModelInput, DbModelMetadataRecord, ModelMetadataSyncResult, UpdateModelInput,
    UpsertModelMetadataInput,
};

pub async fn list_models(
    State(state): State<AppState>,
    headers: HeaderMap,
) -> AppResult<impl IntoResponse> {
    require_admin(&headers, &state).await?;

    let model_registry_store = &state.model_registry_store;

    let models = model_registry_store
        .list_models()
        .await
        .map_err(|e| AppError::new(StatusCode::INTERNAL_SERVER_ERROR, "internal_error", e))?;

    Ok(Json(models))
}

pub async fn get_model(
    State(state): State<AppState>,
    headers: HeaderMap,
    Path(model_id): Path<String>,
) -> AppResult<impl IntoResponse> {
    require_admin(&headers, &state).await?;

    let model_registry_store = &state.model_registry_store;

    let model = model_registry_store
        .get_model(&model_id)
        .await
        .map_err(|e| AppError::new(StatusCode::INTERNAL_SERVER_ERROR, "internal_error", e))?
        .ok_or_else(|| AppError::new(StatusCode::NOT_FOUND, "not_found", "model not found"))?;

    Ok(Json(model))
}

pub async fn create_model(
    State(state): State<AppState>,
    headers: HeaderMap,
    Json(body): Json<CreateModelInput>,
) -> AppResult<impl IntoResponse> {
    require_admin(&headers, &state).await?;

    let model_registry_store = &state.model_registry_store;

    let model = model_registry_store.create_model(body).await.map_err(|e| {
        if e.contains("model_already_exists") {
            AppError::new(StatusCode::CONFLICT, "model_already_exists", e)
        } else {
            AppError::new(StatusCode::BAD_REQUEST, "invalid_request", e)
        }
    })?;

    // Refresh the in-memory model registry to include the new model
    if model.enabled {
        state
            .model_registry
            .merge_db_records(vec![model.clone()])
            .await;
    }

    Ok((StatusCode::CREATED, Json(model)))
}

pub async fn update_model(
    State(state): State<AppState>,
    headers: HeaderMap,
    Path(model_id): Path<String>,
    Json(body): Json<UpdateModelInput>,
) -> AppResult<impl IntoResponse> {
    require_admin(&headers, &state).await?;

    let model_registry_store = &state.model_registry_store;

    let model = model_registry_store
        .update_model(&model_id, body)
        .await
        .map_err(|e| {
            if e.contains("not found") {
                AppError::new(StatusCode::NOT_FOUND, "not_found", e)
            } else if e.contains("model_already_exists") {
                AppError::new(StatusCode::CONFLICT, "model_already_exists", e)
            } else {
                AppError::new(StatusCode::BAD_REQUEST, "invalid_request", e)
            }
        })?;

    // Refresh the in-memory model registry
    // For simplicity, reload all enabled models from the database
    let all_enabled = model_registry_store
        .list_enabled_models()
        .await
        .map_err(|e| AppError::new(StatusCode::INTERNAL_SERVER_ERROR, "internal_error", e))?;

    state.model_registry.replace_db_records(all_enabled).await;

    Ok(Json(model))
}

pub async fn delete_model(
    State(state): State<AppState>,
    headers: HeaderMap,
    Path(model_id): Path<String>,
) -> AppResult<impl IntoResponse> {
    require_admin(&headers, &state).await?;

    let model_registry_store = &state.model_registry_store;

    model_registry_store
        .delete_model(&model_id)
        .await
        .map_err(|e| {
            if e.contains("not found") {
                AppError::new(StatusCode::NOT_FOUND, "not_found", e)
            } else {
                AppError::new(StatusCode::INTERNAL_SERVER_ERROR, "internal_error", e)
            }
        })?;

    // Refresh the in-memory model registry
    let all_enabled = model_registry_store
        .list_enabled_models()
        .await
        .map_err(|e| AppError::new(StatusCode::INTERNAL_SERVER_ERROR, "internal_error", e))?;

    state.model_registry.replace_db_records(all_enabled).await;

    Ok(Json(json!({ "success": true })))
}

pub async fn list_model_metadata(
    State(state): State<AppState>,
    headers: HeaderMap,
) -> AppResult<impl IntoResponse> {
    require_admin(&headers, &state).await?;
    let rows: Vec<DbModelMetadataRecord> =
        state
            .model_registry_store
            .list_model_metadata()
            .await
            .map_err(|e| AppError::new(StatusCode::INTERNAL_SERVER_ERROR, "internal_error", e))?;
    Ok(Json(rows))
}

pub async fn get_model_metadata(
    State(state): State<AppState>,
    headers: HeaderMap,
    Path(model_id): Path<String>,
) -> AppResult<impl IntoResponse> {
    require_admin(&headers, &state).await?;
    let model_id = model_id.strip_prefix('/').unwrap_or(&model_id);
    let row = state
        .model_registry_store
        .get_model_metadata(model_id)
        .await
        .map_err(|e| AppError::new(StatusCode::INTERNAL_SERVER_ERROR, "internal_error", e))?
        .ok_or_else(|| {
            AppError::new(
                StatusCode::NOT_FOUND,
                "not_found",
                "model metadata not found",
            )
        })?;
    Ok(Json(row))
}

pub async fn sync_model_metadata_models_dev(
    State(state): State<AppState>,
    headers: HeaderMap,
) -> AppResult<impl IntoResponse> {
    require_admin(&headers, &state).await?;
    let result: ModelMetadataSyncResult = state
        .model_registry_store
        .sync_from_models_dev(&state.http)
        .await
        .map_err(|e| {
            if e.contains("fetch_failed") || e.contains("parse_failed") {
                AppError::new(StatusCode::BAD_GATEWAY, "upstream_fetch_failed", e)
            } else {
                AppError::new(StatusCode::INTERNAL_SERVER_ERROR, "internal_error", e)
            }
        })?;
    Ok(Json(result))
}

pub async fn upsert_model_metadata(
    State(state): State<AppState>,
    headers: HeaderMap,
    Path(model_id): Path<String>,
    Json(input): Json<UpsertModelMetadataInput>,
) -> AppResult<impl IntoResponse> {
    require_admin(&headers, &state).await?;
    let model_id = model_id.strip_prefix('/').unwrap_or(&model_id);
    if model_id.is_empty() {
        return Err(AppError::new(
            StatusCode::BAD_REQUEST,
            "invalid_request",
            "model_id must not be empty",
        ));
    }
    let record = state
        .model_registry_store
        .upsert_model_metadata(model_id, input)
        .await
        .map_err(|e| AppError::new(StatusCode::INTERNAL_SERVER_ERROR, "internal_error", e))?;
    Ok(Json(record))
}

pub async fn delete_model_metadata(
    State(state): State<AppState>,
    headers: HeaderMap,
    Path(model_id): Path<String>,
) -> AppResult<impl IntoResponse> {
    require_admin(&headers, &state).await?;
    let model_id = model_id.strip_prefix('/').unwrap_or(&model_id);
    let deleted = state
        .model_registry_store
        .delete_model_metadata(model_id)
        .await
        .map_err(|e| AppError::new(StatusCode::INTERNAL_SERVER_ERROR, "internal_error", e))?;
    if !deleted {
        return Err(AppError::new(
            StatusCode::NOT_FOUND,
            "not_found",
            "model metadata not found",
        ));
    }
    Ok(Json(json!({ "success": true })))
}

pub async fn fetch_provider_models(
    State(state): State<AppState>,
    headers: HeaderMap,
    Path(provider_id): Path<String>,
) -> AppResult<impl IntoResponse> {
    require_admin(&headers, &state).await?;

    let provider = state
        .monoize_store
        .get_provider(&provider_id)
        .await
        .map_err(|e| AppError::new(StatusCode::INTERNAL_SERVER_ERROR, "internal_error", e))?
        .ok_or_else(|| AppError::new(StatusCode::NOT_FOUND, "not_found", "provider not found"))?;

    if provider.channels.is_empty() {
        return Err(AppError::new(
            StatusCode::BAD_REQUEST,
            "no_channels",
            "provider has no channels",
        ));
    }

    let channel = provider
        .channels
        .iter()
        .find(|c| c.enabled)
        .unwrap_or(&provider.channels[0]);

    let url = build_models_list_url(&channel.base_url);

    let resp = state
        .http
        .get(&url)
        .header("Authorization", format!("Bearer {}", channel.api_key))
        .timeout(std::time::Duration::from_secs(15))
        .send()
        .await
        .map_err(|e| {
            AppError::new(
                StatusCode::BAD_GATEWAY,
                "upstream_fetch_failed",
                format!("failed to fetch models: {e}"),
            )
        })?;

    if !resp.status().is_success() {
        let status = resp.status();
        let body = resp.text().await.unwrap_or_default();
        return Err(AppError::new(
            StatusCode::BAD_GATEWAY,
            "upstream_fetch_failed",
            format!("upstream returned {status}: {body}"),
        ));
    }

    let body: Value = resp.json().await.map_err(|e| {
        AppError::new(
            StatusCode::BAD_GATEWAY,
            "upstream_fetch_failed",
            format!("failed to parse response: {e}"),
        )
    })?;

    let models: Vec<String> = body
        .get("data")
        .and_then(|d| d.as_array())
        .map(|arr| {
            let mut seen = std::collections::HashSet::new();
            arr.iter()
                .filter_map(|item| item.get("id").and_then(|id| id.as_str()).map(String::from))
                .filter(|id| seen.insert(id.clone()))
                .collect()
        })
        .unwrap_or_default();

    Ok(Json(json!({
        "provider_id": provider.id,
        "provider_name": provider.name,
        "models": models
    })))
}

#[derive(Debug, Deserialize)]
pub struct FetchChannelModelsRequest {
    pub base_url: String,
    pub api_key: String,
}

pub async fn fetch_channel_models(
    State(state): State<AppState>,
    headers: HeaderMap,
    Json(body): Json<FetchChannelModelsRequest>,
) -> AppResult<impl IntoResponse> {
    require_admin(&headers, &state).await?;

    if body.base_url.trim().is_empty() || body.api_key.trim().is_empty() {
        return Err(AppError::new(
            StatusCode::BAD_REQUEST,
            "invalid_input",
            "base_url and api_key are required",
        ));
    }

    let url = build_models_list_url(&body.base_url);

    let resp = state
        .http
        .get(&url)
        .header("Authorization", format!("Bearer {}", body.api_key))
        .timeout(std::time::Duration::from_secs(15))
        .send()
        .await
        .map_err(|e| {
            AppError::new(
                StatusCode::BAD_GATEWAY,
                "upstream_fetch_failed",
                format!("failed to fetch models: {e}"),
            )
        })?;

    if !resp.status().is_success() {
        let status = resp.status();
        let body = resp.text().await.unwrap_or_default();
        return Err(AppError::new(
            StatusCode::BAD_GATEWAY,
            "upstream_fetch_failed",
            format!("upstream returned {status}: {body}"),
        ));
    }

    let resp_body: Value = resp.json().await.map_err(|e| {
        AppError::new(
            StatusCode::BAD_GATEWAY,
            "upstream_fetch_failed",
            format!("failed to parse response: {e}"),
        )
    })?;

    let models: Vec<String> = resp_body
        .get("data")
        .and_then(|d| d.as_array())
        .map(|arr| {
            let mut seen = std::collections::HashSet::new();
            arr.iter()
                .filter_map(|item| item.get("id").and_then(|id| id.as_str()).map(String::from))
                .filter(|id| seen.insert(id.clone()))
                .collect()
        })
        .unwrap_or_default();

    Ok(Json(json!({ "models": models })))
}

#[derive(Debug, Deserialize)]
pub struct TestChannelRequest {
    pub model: Option<String>,
}

pub async fn test_channel(
    State(state): State<AppState>,
    headers: HeaderMap,
    Path((provider_id, channel_id)): Path<(String, String)>,
    body: Option<Json<TestChannelRequest>>,
) -> AppResult<impl IntoResponse> {
    require_admin(&headers, &state).await?;

    let provider = state
        .monoize_store
        .get_provider(&provider_id)
        .await
        .map_err(|e| AppError::new(StatusCode::INTERNAL_SERVER_ERROR, "internal_error", e))?
        .ok_or_else(|| AppError::new(StatusCode::NOT_FOUND, "not_found", "provider not found"))?;

    let channel = provider
        .channels
        .iter()
        .find(|c| c.id == channel_id)
        .ok_or_else(|| AppError::new(StatusCode::NOT_FOUND, "not_found", "channel not found"))?;

    let requested_model = body.and_then(|b| b.model.clone());

    let settings = state
        .settings_store
        .get_all()
        .await
        .map_err(|e| AppError::new(StatusCode::INTERNAL_SERVER_ERROR, "internal_error", e))?;

    let probe_model = requested_model
        .or_else(|| provider.active_probe_model_override.clone())
        .or_else(|| settings.monoize_active_probe_model.clone())
        .or_else(|| provider.models.keys().next().cloned());

    let Some(model_name) = probe_model else {
        return Err(AppError::new(
            StatusCode::BAD_REQUEST,
            "invalid_request",
            "no model available for testing; specify a model or add models to this provider",
        ));
    };

    let started_at = std::time::Instant::now();
    let (ok, _usage) = crate::monoize_routing::probe_channel_completion(
        &state.http,
        channel,
        state.monoize_runtime.read().await.request_timeout_ms,
        &model_name,
        provider.provider_type,
    )
    .await;
    let latency_ms = started_at.elapsed().as_millis() as u64;

    if ok {
        let now = chrono::Utc::now().timestamp();
        let mut health = state.channel_health.lock().await;
        let entry = health
            .entry(channel_id.clone())
            .or_insert_with(ChannelHealthState::new);
        entry.healthy = true;
        entry.failure_count = 0;
        entry.cooldown_until = None;
        entry.last_success_at = Some(now);
        entry.probe_success_count = 0;
        entry.last_probe_at = None;
    }

    let error_msg = if ok {
        None
    } else {
        Some("upstream returned non-2xx status or connection failed".to_string())
    };

    Ok(Json(json!({
        "success": ok,
        "latency_ms": latency_ms,
        "model": model_name,
        "error": error_msg,
    })))
}

pub async fn get_transform_registry(State(state): State<AppState>) -> AppResult<impl IntoResponse> {
    let mut items: Vec<Value> = state
        .transform_registry
        .values()
        .map(|transform| {
            json!({
                "type_id": transform.type_id(),
                "supported_phases": transform
                    .supported_phases()
                    .iter()
                    .map(|p| serde_json::to_value(p).unwrap_or(Value::String("request".to_string())))
                    .collect::<Vec<_>>(),
                "config_schema": transform.config_schema(),
            })
        })
        .collect();
    items.sort_by(|a, b| a["type_id"].as_str().cmp(&b["type_id"].as_str()));
    Ok(Json(items))
}

pub async fn get_provider_presets() -> AppResult<impl IntoResponse> {
    Ok(Json(crate::presets::provider_presets()))
}

fn build_models_list_url(base_url: &str) -> String {
    let base = base_url.trim_end_matches('/');
    if base.ends_with("/v1") {
        format!("{base}/models")
    } else {
        format!("{base}/v1/models")
    }
}

#[cfg(test)]
mod tests {
    use super::{build_models_list_url, provider_pricing_model};
    use crate::monoize_routing::MonoizeModelEntry;

    #[test]
    fn build_models_list_url_adds_v1_when_missing() {
        assert_eq!(
            build_models_list_url("https://openrouter.ai/api"),
            "https://openrouter.ai/api/v1/models"
        );
    }

    #[test]
    fn build_models_list_url_avoids_duplicate_v1_suffix() {
        assert_eq!(
            build_models_list_url("https://openrouter.ai/api/v1"),
            "https://openrouter.ai/api/v1/models"
        );
        assert_eq!(
            build_models_list_url("https://openrouter.ai/api/v1/"),
            "https://openrouter.ai/api/v1/models"
        );
    }

    #[test]
    fn provider_pricing_model_uses_redirect_when_present() {
        let entry = MonoizeModelEntry {
            redirect: Some("  gpt-5-target  ".to_string()),
            multiplier: 1.0,
        };
        assert_eq!(
            provider_pricing_model("gpt-5-logical", &entry),
            "gpt-5-target"
        );
    }

    #[test]
    fn provider_pricing_model_falls_back_to_logical_when_redirect_blank() {
        let entry = MonoizeModelEntry {
            redirect: Some("   ".to_string()),
            multiplier: 1.0,
        };
        assert_eq!(
            provider_pricing_model("gpt-5-logical", &entry),
            "gpt-5-logical"
        );
    }
}

pub async fn get_apikey_presets() -> AppResult<impl IntoResponse> {
    Ok(Json(crate::presets::apikey_presets()))
}

#[derive(Debug, Deserialize)]
pub struct RequestLogsQuery {
    #[serde(default = "default_logs_limit")]
    pub limit: i64,
    #[serde(default)]
    pub offset: i64,
    #[serde(default)]
    pub model: Option<String>,
    #[serde(default)]
    pub status: Option<String>,
    #[serde(default)]
    pub api_key_id: Option<String>,
    #[serde(default)]
    pub username: Option<String>,
    #[serde(default)]
    pub search: Option<String>,
    #[serde(default)]
    pub time_from: Option<String>,
    #[serde(default)]
    pub time_to: Option<String>,
}

fn default_logs_limit() -> i64 {
    50
}

pub async fn list_my_request_logs(
    State(state): State<AppState>,
    headers: HeaderMap,
    axum::extract::Query(query): axum::extract::Query<RequestLogsQuery>,
) -> AppResult<impl IntoResponse> {
    let user = get_current_user(&headers, &state).await?;
    let limit = query.limit.clamp(1, 200);
    let offset = query.offset.max(0);
    let (logs, total, total_charge_nano_usd) = if user.role.can_manage_users() {
        state
            .user_store
            .list_all_request_logs(
                limit,
                offset,
                query.model.as_deref(),
                query.status.as_deref(),
                query.api_key_id.as_deref(),
                query.username.as_deref(),
                query.search.as_deref(),
                query.time_from.as_deref(),
                query.time_to.as_deref(),
            )
            .await
    } else {
        state
            .user_store
            .list_request_logs_by_user(
                &user.id,
                limit,
                offset,
                query.model.as_deref(),
                query.status.as_deref(),
                query.api_key_id.as_deref(),
                query.search.as_deref(),
                query.time_from.as_deref(),
                query.time_to.as_deref(),
            )
            .await
    }
    .map_err(|e| AppError::new(StatusCode::INTERNAL_SERVER_ERROR, "internal_error", e))?;
    Ok(Json(json!({
        "data": logs,
        "total": total,
        "total_charge_nano_usd": total_charge_nano_usd,
        "limit": limit,
        "offset": offset,
    })))
}

#[derive(Debug, Deserialize)]
pub struct AnalyticsQuery {
    #[serde(default = "default_analytics_buckets")]
    pub buckets: i64,
    #[serde(default = "default_analytics_range_hours")]
    pub range_hours: i64,
}

fn default_analytics_buckets() -> i64 {
    8
}

fn default_analytics_range_hours() -> i64 {
    24
}

pub async fn get_dashboard_analytics(
    State(state): State<AppState>,
    headers: HeaderMap,
    axum::extract::Query(query): axum::extract::Query<AnalyticsQuery>,
) -> AppResult<impl IntoResponse> {
    let user = get_current_user(&headers, &state).await?;
    let buckets = query.buckets.clamp(1, 48);
    let range_hours = query.range_hours.clamp(1, 720);

    let now = Utc::now();
    let time_to = now.to_rfc3339();
    let time_from = (now - chrono::Duration::hours(range_hours)).to_rfc3339();
    let today_start = now
        .date_naive()
        .and_hms_opt(0, 0, 0)
        .unwrap()
        .and_utc()
        .to_rfc3339();

    let bucket_width_days = (range_hours as f64) / (buckets as f64) / 24.0;

    let user_id_filter: Option<String> = if user.role.can_manage_users() {
        None
    } else {
        Some(user.id.clone())
    };

    let raw = state
        .user_store
        .get_dashboard_analytics(
            user_id_filter.as_deref(),
            &time_from,
            &time_to,
            &today_start,
            buckets,
            bucket_width_days,
        )
        .await
        .map_err(|e| AppError::new(StatusCode::INTERNAL_SERVER_ERROR, "internal_error", e))?;

    let range_ms = (range_hours as f64) * 3600.0 * 1000.0;
    let time_from_ms = now.timestamp_millis() as f64 - range_ms;
    let bucket_width_ms = range_ms / (buckets as f64);

    let mut bucket_labels: Vec<String> = Vec::with_capacity(buckets as usize);
    for i in 0..buckets {
        let ms = time_from_ms + (i as f64) * bucket_width_ms;
        let secs = (ms / 1000.0) as i64;
        let dt = chrono::DateTime::from_timestamp(secs, 0).unwrap_or(now);
        let label = dt.format("%m-%d %H:00").to_string();
        bucket_labels.push(label);
    }

    let mut cost_by_model_buckets: Vec<serde_json::Map<String, Value>> =
        (0..buckets).map(|_| serde_json::Map::new()).collect();
    let mut calls_by_model_buckets: Vec<serde_json::Map<String, Value>> =
        (0..buckets).map(|_| serde_json::Map::new()).collect();
    let mut calls_by_provider_buckets: Vec<serde_json::Map<String, Value>> =
        (0..buckets).map(|_| serde_json::Map::new()).collect();

    for row in &raw.model_buckets {
        let idx = row.bucket_idx.clamp(0, buckets - 1) as usize;
        *cost_by_model_buckets[idx]
            .entry(&row.model)
            .or_insert(json!(0)) = json!(
            cost_by_model_buckets[idx]
                .get(&row.model)
                .and_then(|v| v.as_i64())
                .unwrap_or(0)
                + row.cost_nano
        );
        *calls_by_model_buckets[idx]
            .entry(&row.model)
            .or_insert(json!(0)) = json!(
            calls_by_model_buckets[idx]
                .get(&row.model)
                .and_then(|v| v.as_i64())
                .unwrap_or(0)
                + row.call_count
        );
    }

    for row in &raw.provider_buckets {
        let idx = row.bucket_idx.clamp(0, buckets - 1) as usize;
        *calls_by_provider_buckets[idx]
            .entry(&row.provider_label)
            .or_insert(json!(0)) = json!(
            calls_by_provider_buckets[idx]
                .get(&row.provider_label)
                .and_then(|v| v.as_i64())
                .unwrap_or(0)
                + row.call_count
        );
    }

    let response_buckets: Vec<Value> = (0..buckets as usize)
        .map(|i| {
            json!({
                "label": bucket_labels[i],
                "cost_by_model": cost_by_model_buckets[i],
                "calls_by_model": calls_by_model_buckets[i],
                "calls_by_provider": calls_by_provider_buckets[i],
            })
        })
        .collect();

    Ok(Json(json!({
        "buckets": response_buckets,
        "time_from": time_from,
        "time_to": time_to,
        "total_cost_nano_usd": raw.total_cost_nano_usd,
        "total_calls": raw.total_calls,
        "today_cost_nano_usd": raw.today_cost_nano_usd,
        "today_calls": raw.today_calls,
    })))
}
