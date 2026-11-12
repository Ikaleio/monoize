use crate::app::AppState;
use crate::dashboard_handlers::auth::{UserResponse, normalize_email, user_response_from_store};
use crate::dashboard_handlers::session_helpers::{
    is_reserved_internal_username, is_valid_username, require_admin,
};
use crate::error::{AppError, AppResult};
use crate::users::{
    AdminUpdateUserInput, BalanceMutationError, UserRole, UserTodayUsage, parse_usd_to_nano,
};
use axum::Json;
use axum::extract::{Path, State};
use axum::http::{HeaderMap, StatusCode};
use axum::response::IntoResponse;
use chrono::{NaiveTime, Utc};
use serde::Deserialize;
use serde_json::json;
use std::collections::HashMap;

/// Upper bound for the user-visible admin balance note (ledger `meta.note`).
const MAX_BALANCE_NOTE_CHARS: usize = 200;

#[derive(Debug, Deserialize)]
pub struct CreateUserRequest {
    pub username: String,
    pub password: String,
    pub role: Option<String>,
    /// Absent or empty = assign the default group (U3).
    #[serde(default)]
    pub group_id: Option<String>,
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct UpdateUserRequest {
    pub username: Option<String>,
    pub password: Option<String>,
    pub role: Option<String>,
    pub enabled: Option<bool>,
    pub balance_nano_usd: Option<String>,
    pub balance_usd: Option<String>,
    /// Signed nano-USD amount added to the locked current balance.
    pub balance_delta_nano_usd: Option<String>,
    /// Compare-and-set guard for an absolute balance write.
    pub expected_balance_nano_usd: Option<String>,
    /// Optional user-visible note stored on the `admin_adjustment` ledger row.
    pub balance_note: Option<String>,
    pub balance_unlimited: Option<bool>,
    pub email: Option<Option<String>>,
    pub group_id: Option<String>,
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
    let today_start = Utc::now()
        .date_naive()
        .and_time(NaiveTime::MIN)
        .and_utc()
        .to_rfc3339();
    let usage_rows = user_store
        .get_users_today_usage(&today_start)
        .await
        .map_err(|e| AppError::new(StatusCode::INTERNAL_SERVER_ERROR, "internal_error", e))?;

    let usage_by_id: HashMap<_, _> = usage_rows
        .into_iter()
        .map(|row| (row.user_id.clone(), row))
        .collect();
    let zero_usage = UserTodayUsage {
        user_id: String::new(),
        today_calls: 0,
        today_cost_nano_usd: 0,
    };
    let mut plans = user_store
        .list_active_plan_summaries()
        .await
        .map_err(|e| AppError::new(StatusCode::INTERNAL_SERVER_ERROR, "internal_error", e))?;

    let responses: Vec<UserResponse> = users
        .into_iter()
        .map(|user| {
            let today = usage_by_id.get(&user.id).unwrap_or(&zero_usage);
            let plan = plans.remove(&user.id);
            let mut response = UserResponse::from_user(user, Some(today));
            if let Some((plan_name, expires_at)) = plan {
                response.active_plan_name = Some(plan_name);
                response.active_plan_expires_at = Some(expires_at.to_rfc3339());
            }
            response
        })
        .collect();
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

    let response = user_response_from_store(user_store, user)
        .await
        .map_err(|e| AppError::new(StatusCode::INTERNAL_SERVER_ERROR, "internal_error", e))?;
    Ok(Json(response))
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
        )
        .with_param("username"));
    }

    if is_reserved_internal_username(&body.username) {
        return Err(AppError::new(
            StatusCode::BAD_REQUEST,
            "reserved_username",
            "username prefix _monoize_ is reserved",
        )
        .with_param("username"));
    }

    if body.password.len() < 8 {
        return Err(AppError::new(
            StatusCode::BAD_REQUEST,
            "invalid_password",
            "password must be at least 8 characters",
        )
        .with_param("password"));
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
        )
        .with_param("username"));
    }

    let user = user_store
        .create_user(
            &body.username,
            &body.password,
            role,
            body.group_id.as_deref(),
        )
        .await
        .map_err(|e| {
            if e.starts_with("unknown group id") {
                AppError::new(StatusCode::BAD_REQUEST, "invalid_request", e)
            } else {
                AppError::new(StatusCode::INTERNAL_SERVER_ERROR, "internal_error", e)
            }
        })?;

    let response = user_response_from_store(user_store, user)
        .await
        .map_err(|e| AppError::new(StatusCode::INTERNAL_SERVER_ERROR, "internal_error", e))?;
    Ok((StatusCode::CREATED, Json(response)))
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

    let new_role = validate_user_update(&current_user, &target_user, &user_id, &body)?;

    let email = body
        .email
        .as_ref()
        .map(|email| normalize_email(email.as_deref()).map(|email| email.map(str::to_string)))
        .transpose()?;

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
                    .with_param("balance")
                })?
                .to_string(),
        )
    } else {
        None
    };
    if balance_nano_override.is_some() && body.balance_delta_nano_usd.is_some() {
        return Err(AppError::new(
            StatusCode::BAD_REQUEST,
            "invalid_request",
            "balance_delta_nano_usd cannot be combined with an absolute balance",
        ));
    }
    if body.expected_balance_nano_usd.is_some() && balance_nano_override.is_none() {
        return Err(AppError::new(
            StatusCode::BAD_REQUEST,
            "invalid_request",
            "expected_balance_nano_usd requires an absolute balance",
        ));
    }
    let balance_note = body
        .balance_note
        .as_deref()
        .map(str::trim)
        .filter(|note| !note.is_empty())
        .map(str::to_string);
    if balance_note
        .as_ref()
        .is_some_and(|note| note.chars().count() > MAX_BALANCE_NOTE_CHARS)
    {
        return Err(AppError::new(
            StatusCode::BAD_REQUEST,
            "invalid_request",
            format!("balance_note must be at most {MAX_BALANCE_NOTE_CHARS} characters"),
        )
        .with_param("balance_note"));
    }

    user_store
        .admin_update_user_atomic(
            &user_id,
            AdminUpdateUserInput {
                username: body.username,
                password: body.password,
                role: new_role,
                enabled: body.enabled,
                balance_nano_usd: balance_nano_override,
                balance_delta_nano_usd: body.balance_delta_nano_usd,
                expected_balance_nano_usd: body.expected_balance_nano_usd,
                balance_note,
                balance_unlimited: body.balance_unlimited,
                email,
                group_id: body.group_id,
            },
            &current_user.id,
        )
        .await
        .map_err(map_user_balance_error)?;

    let updated_user = user_store
        .get_user_by_id(&user_id)
        .await
        .map_err(|e| AppError::new(StatusCode::INTERNAL_SERVER_ERROR, "internal_error", e))?
        .ok_or_else(|| AppError::new(StatusCode::NOT_FOUND, "not_found", "user not found"))?;

    let response = user_response_from_store(user_store, updated_user)
        .await
        .map_err(|e| AppError::new(StatusCode::INTERNAL_SERVER_ERROR, "internal_error", e))?;
    Ok(Json(response))
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

fn validate_user_update(
    current_user: &crate::users::User,
    target_user: &crate::users::User,
    user_id: &str,
    body: &UpdateUserRequest,
) -> AppResult<Option<UserRole>> {
    if target_user.role == UserRole::SuperAdmin && current_user.role != UserRole::SuperAdmin {
        return Err(AppError::new(
            StatusCode::FORBIDDEN,
            "forbidden",
            "only super admin can modify super admin accounts",
        ));
    }
    if target_user.role == UserRole::Admin
        && current_user.id != target_user.id
        && current_user.role != UserRole::SuperAdmin
    {
        return Err(AppError::new(
            StatusCode::FORBIDDEN,
            "forbidden",
            "only super admin can modify other admin accounts",
        ));
    }

    let new_role = body.role.as_ref().and_then(|r| UserRole::from_str(r));
    if let Some(role) = new_role
        && !current_user.role.can_assign_role(role)
    {
        return Err(AppError::new(
            StatusCode::FORBIDDEN,
            "forbidden",
            "you cannot assign this role",
        ));
    }

    if let Some(ref username) = body.username {
        if !is_valid_username(username) {
            return Err(AppError::new(
                StatusCode::BAD_REQUEST,
                "invalid_username",
                "username must be 3-22 characters, only letters, digits and underscores",
            )
            .with_param("username"));
        }
        if is_reserved_internal_username(username) {
            return Err(AppError::new(
                StatusCode::BAD_REQUEST,
                "reserved_username",
                "username prefix _monoize_ is reserved",
            )
            .with_param("username"));
        }
    }

    if let Some(ref password) = body.password
        && password.len() < 8
    {
        return Err(AppError::new(
            StatusCode::BAD_REQUEST,
            "invalid_password",
            "password must be at least 8 characters",
        )
        .with_param("password"));
    }

    if body.enabled == Some(false) && current_user.id == user_id {
        return Err(AppError::new(
            StatusCode::BAD_REQUEST,
            "cannot_disable_self",
            "you cannot disable your own account",
        ));
    }

    Ok(new_role)
}

fn map_user_balance_error(e: BalanceMutationError) -> AppError {
    match e {
        BalanceMutationError::BalanceChanged => AppError::new(
            StatusCode::CONFLICT,
            "balance_changed",
            "balance changed since it was read",
        )
        .with_param("balance"),
        BalanceMutationError::Other(e) => {
            if e.starts_with("unknown group id") {
                AppError::new(StatusCode::BAD_REQUEST, "invalid_request", e)
            } else if e.contains("not found") {
                AppError::new(StatusCode::NOT_FOUND, "not_found", e)
            } else if e.contains("invalid") {
                AppError::new(StatusCode::BAD_REQUEST, "invalid_balance", e).with_param("balance")
            } else {
                AppError::new(StatusCode::INTERNAL_SERVER_ERROR, "internal_error", e)
            }
        }
    }
}
