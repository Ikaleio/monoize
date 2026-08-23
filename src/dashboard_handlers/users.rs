use crate::app::AppState;
use crate::dashboard_handlers::auth::UserResponse;
use crate::dashboard_handlers::session_helpers::{
    is_reserved_internal_username, is_valid_username, require_admin,
};
use crate::error::{AppError, AppResult};
use crate::users::{AdminUpdateUserInput, UserRole, canonicalize_groups, parse_usd_to_nano};
use axum::Json;
use axum::extract::{Path, State};
use axum::http::{HeaderMap, StatusCode};
use axum::response::IntoResponse;
use serde::Deserialize;
use serde_json::json;

#[derive(Debug, Deserialize)]
pub struct CreateUserRequest {
    pub username: String,
    pub password: String,
    pub role: Option<String>,
    #[serde(default)]
    pub allowed_groups: Vec<String>,
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
    pub allowed_groups: Option<Vec<String>>,
    /// Absent = no change; null = unassign; string = assign plan (BP-S1..S4).
    #[serde(default)]
    pub billing_plan_id: Option<Option<String>>,
}

pub(super) fn canonicalize_dashboard_user_allowed_groups(groups: &mut Vec<String>) {
    *groups = canonicalize_groups(groups);
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
    Json(mut body): Json<CreateUserRequest>,
) -> AppResult<impl IntoResponse> {
    let current_user = require_admin(&headers, &state).await?;

    let user_store = &state.user_store;

    canonicalize_dashboard_user_allowed_groups(&mut body.allowed_groups);

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
        .create_user(&body.username, &body.password, role, &body.allowed_groups)
        .await
        .map_err(|e| AppError::new(StatusCode::INTERNAL_SERVER_ERROR, "internal_error", e))?;

    Ok((StatusCode::CREATED, Json(UserResponse::from(user))))
}

pub async fn update_user(
    State(state): State<AppState>,
    headers: HeaderMap,
    Path(user_id): Path<String>,
    Json(mut body): Json<UpdateUserRequest>,
) -> AppResult<impl IntoResponse> {
    let current_user = require_admin(&headers, &state).await?;

    let user_store = &state.user_store;

    if let Some(groups) = body.allowed_groups.as_mut() {
        canonicalize_dashboard_user_allowed_groups(groups);
    }

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
        .admin_update_user_atomic(
            &user_id,
            AdminUpdateUserInput {
                username: body.username,
                password: body.password,
                role: new_role,
                enabled: body.enabled,
                balance_nano_usd: balance_nano_override,
                balance_unlimited: body.balance_unlimited,
                email: body.email,
                allowed_groups: body.allowed_groups,
                billing_plan_id: body.billing_plan_id,
            },
            &current_user.id,
        )
        .await
        .map_err(|e| {
            if e.contains("not found") {
                AppError::new(StatusCode::NOT_FOUND, "not_found", e)
            } else if e.contains("billing plan not found") {
                AppError::new(StatusCode::BAD_REQUEST, "invalid_billing_plan", e)
            } else if e.contains("invalid") {
                AppError::new(StatusCode::BAD_REQUEST, "invalid_balance", e)
            } else {
                AppError::new(StatusCode::INTERNAL_SERVER_ERROR, "internal_error", e)
            }
        })?;

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
