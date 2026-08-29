use crate::app::AppState;
use crate::dashboard_handlers::session_helpers::{get_current_user, require_admin};
use crate::error::{AppError, AppResult};
use crate::users::{
    CreateGroupInput, Group, GroupStoreError, ReorderGroupsInput, UpdateGroupInput,
};
use axum::Json;
use axum::extract::{Path, State};
use axum::http::{HeaderMap, StatusCode};
use serde::Serialize;
use serde_json::{Value, json};
use std::sync::atomic::Ordering;

#[derive(Debug, Serialize)]
pub struct DashboardGroupsResponse {
    pub groups: Vec<Group>,
}

fn map_group_error(error: GroupStoreError) -> AppError {
    match error {
        GroupStoreError::NotFound => {
            AppError::new(StatusCode::NOT_FOUND, "not_found", "group not found")
        }
        GroupStoreError::NameExists => AppError::new(
            StatusCode::CONFLICT,
            "group_name_exists",
            "a group with this name already exists",
        ),
        GroupStoreError::InvalidName => AppError::new(
            StatusCode::BAD_REQUEST,
            "invalid_group_name",
            "group name must be 1-64 characters after trimming",
        ),
        GroupStoreError::InvalidDescription => AppError::new(
            StatusCode::BAD_REQUEST,
            "invalid_group_description",
            "group description must be at most 256 characters after trimming",
        ),
        GroupStoreError::InvalidBillingRatio => AppError::new(
            StatusCode::BAD_REQUEST,
            "invalid_request",
            "billing_ratio must be a non-negative base-10 decimal string with at most 9 fractional digits",
        ),
        GroupStoreError::InvalidReorder(message) => {
            AppError::new(StatusCode::BAD_REQUEST, "invalid_request", message)
        }
        GroupStoreError::CannotDeleteDefault => AppError::new(
            StatusCode::BAD_REQUEST,
            "cannot_delete_default_group",
            "the default group cannot be deleted",
        ),
        GroupStoreError::PlanRequiresGroup => AppError::new(
            StatusCode::CONFLICT,
            "group_required_by_plan",
            "a billing plan must retain at least one eligible group",
        ),
        GroupStoreError::Storage(error) => {
            AppError::new(StatusCode::INTERNAL_SERVER_ERROR, "internal_error", error)
        }
    }
}

/// GR-A1: every authenticated session may read the full registry in canonical order.
pub async fn list_dashboard_groups(
    State(state): State<AppState>,
    headers: HeaderMap,
) -> AppResult<Json<DashboardGroupsResponse>> {
    get_current_user(&headers, &state).await?;

    let groups = state
        .user_store
        .list_groups()
        .await
        .map_err(|e| AppError::new(StatusCode::INTERNAL_SERVER_ERROR, "internal_error", e))?;

    Ok(Json(DashboardGroupsResponse { groups }))
}

pub async fn create_group(
    State(state): State<AppState>,
    headers: HeaderMap,
    Json(body): Json<CreateGroupInput>,
) -> AppResult<(StatusCode, Json<Group>)> {
    require_admin(&headers, &state).await?;

    let group = state
        .user_store
        .create_group(body)
        .await
        .map_err(map_group_error)?;
    Ok((StatusCode::CREATED, Json(group)))
}

pub async fn update_group(
    State(state): State<AppState>,
    headers: HeaderMap,
    Path(group_id): Path<String>,
    Json(body): Json<UpdateGroupInput>,
) -> AppResult<Json<Group>> {
    require_admin(&headers, &state).await?;

    let group = state
        .user_store
        .update_group(&group_id, body)
        .await
        .map_err(map_group_error)?;
    Ok(Json(group))
}

pub async fn reorder_groups(
    State(state): State<AppState>,
    headers: HeaderMap,
    Json(body): Json<ReorderGroupsInput>,
) -> AppResult<Json<Value>> {
    require_admin(&headers, &state).await?;

    state
        .user_store
        .reorder_groups(body)
        .await
        .map_err(map_group_error)?;

    Ok(Json(json!({ "success": true })))
}

pub async fn delete_group(
    State(state): State<AppState>,
    headers: HeaderMap,
    Path(group_id): Path<String>,
) -> AppResult<Json<Value>> {
    require_admin(&headers, &state).await?;

    state
        .user_store
        .delete_group(&group_id)
        .await
        .map_err(map_group_error)?;

    // GR-X6: provider group sets may have changed; force re-validation of
    // in-flight affinity bindings and cached routing decisions.
    state.routing_config_revision.fetch_add(1, Ordering::AcqRel);

    Ok(Json(json!({ "success": true })))
}
