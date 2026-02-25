use crate::app::AppState;
use crate::dashboard_handlers::session_helpers::{
    get_current_user, is_reserved_internal_username, is_valid_username,
};
use crate::error::{AppError, AppResult};
use crate::users::{User, UserRole, format_nano_to_usd};
use axum::Json;
use axum::extract::State;
use axum::http::{HeaderMap, StatusCode};
use axum::response::IntoResponse;
use serde::{Deserialize, Serialize};
use serde_json::json;

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
pub struct UpdateMeRequest {
    pub email: Option<Option<String>>,
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

    let valid = crate::users::UserStore::verify_password(&body.password, &user.password_hash)
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
    let token = crate::dashboard_handlers::session_helpers::extract_session_token(&headers)
        .ok_or_else(|| {
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
