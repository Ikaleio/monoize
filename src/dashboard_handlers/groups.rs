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

#[cfg(test)]
mod tests {
    use super::{DashboardGroupsResponse, list_dashboard_groups};
    use crate::app::{AppState, RuntimeConfig, load_state_with_runtime};
    use crate::users::{CreateGroupInput, ReorderGroupsInput, UpdateGroupInput, UserRole};
    use axum::Json;
    use axum::extract::{Path, State};
    use axum::http::{HeaderMap, HeaderValue};

    async fn make_state() -> AppState {
        load_state_with_runtime(RuntimeConfig {
            listen: "127.0.0.1:0".to_string(),
            metrics_path: "/metrics".to_string(),
            database_dsn: "sqlite::memory:".to_string(),
            request_log_spool_dir: None,
            node: crate::node_config::NodeSettings::primary_default(),
        })
        .await
        .expect("state loads")
    }

    async fn session_headers(state: &AppState, username: &str, role: UserRole) -> HeaderMap {
        let user = state
            .user_store
            .create_user(username, "password123", role, None)
            .await
            .expect("user created");
        let session = state
            .user_store
            .create_session(&user.id, 7)
            .await
            .expect("session created");
        let mut headers = HeaderMap::new();
        headers.insert(
            "authorization",
            HeaderValue::from_str(&format!("Bearer {}", session.token)).expect("header value"),
        );
        headers
    }

    #[tokio::test]
    async fn groups_crud_lifecycle_enforces_registry_invariants() {
        let state = make_state().await;
        let admin_headers = session_headers(&state, "group_admin", UserRole::Admin).await;
        let reader_headers = session_headers(&state, "group_reader", UserRole::User).await;

        // Fresh install: exactly one default group (GM-10).
        let Json(DashboardGroupsResponse { groups }) =
            list_dashboard_groups(State(state.clone()), reader_headers.clone())
                .await
                .expect("list succeeds");
        assert_eq!(groups.len(), 1);
        assert!(groups[0].is_default);
        assert_eq!(groups[0].name, "default");

        // Admin creates a group; the reader sees it in canonical order.
        let created = super::create_group(
            State(state.clone()),
            admin_headers.clone(),
            Json(CreateGroupInput {
                name: " Team-A ".to_string(),
                description: " premium routing ".to_string(),
                user_selectable: true,
                sort_order: 5,
                billing_ratio: Some("1.5".to_string()),
            }),
        )
        .await
        .expect("create succeeds");
        let (status, Json(created)) = created;
        assert_eq!(status, axum::http::StatusCode::CREATED);
        assert_eq!(created.name, "Team-A");
        assert_eq!(created.description, "premium routing");
        assert_eq!(created.billing_ratio, "1.5");
        assert!(!created.is_default);

        let default_id = groups[0].id.clone();
        let Json(reorder_body) = super::reorder_groups(
            State(state.clone()),
            admin_headers.clone(),
            Json(ReorderGroupsInput {
                group_ids: vec![created.id.clone(), default_id.clone()],
            }),
        )
        .await
        .expect("reorder succeeds");
        assert_eq!(reorder_body["success"], serde_json::json!(true));
        let Json(DashboardGroupsResponse { groups: reordered }) =
            list_dashboard_groups(State(state.clone()), reader_headers.clone())
                .await
                .expect("reordered list succeeds");
        assert_eq!(reordered[0].id, created.id);
        assert_eq!(reordered[0].sort_order, 0);
        assert_eq!(reordered[1].id, default_id);
        assert_eq!(reordered[1].sort_order, 1);

        let invalid_reorder = super::reorder_groups(
            State(state.clone()),
            admin_headers.clone(),
            Json(ReorderGroupsInput {
                group_ids: vec![created.id.clone()],
            }),
        )
        .await
        .expect_err("partial reorder must fail");
        assert_eq!(invalid_reorder.status, axum::http::StatusCode::BAD_REQUEST);

        // Duplicate name (case-insensitive) is rejected with 409.
        let conflict = super::create_group(
            State(state.clone()),
            admin_headers.clone(),
            Json(CreateGroupInput {
                name: "team-a".to_string(),
                description: String::new(),
                user_selectable: false,
                sort_order: 0,
                billing_ratio: None,
            }),
        )
        .await
        .expect_err("duplicate must fail");
        assert_eq!(conflict.status, axum::http::StatusCode::CONFLICT);

        // Non-admin cannot create groups.
        let forbidden = super::create_group(
            State(state.clone()),
            reader_headers.clone(),
            Json(CreateGroupInput {
                name: "team-b".to_string(),
                description: String::new(),
                user_selectable: false,
                sort_order: 0,
                billing_ratio: None,
            }),
        )
        .await
        .expect_err("non-admin must fail");
        assert_eq!(forbidden.status, axum::http::StatusCode::FORBIDDEN);

        // The default group is renameable but stays default (GR-D3).
        let Json(renamed) = super::update_group(
            State(state.clone()),
            admin_headers.clone(),
            Path(default_id.clone()),
            Json(UpdateGroupInput {
                name: Some("общий".to_string()),
                description: Some("system default".to_string()),
                ..Default::default()
            }),
        )
        .await
        .expect("rename succeeds");
        assert_eq!(renamed.name, "общий");
        assert!(renamed.is_default);

        // The default group cannot be deleted (GR-A7).
        let default_delete = super::delete_group(
            State(state.clone()),
            admin_headers.clone(),
            Path(default_id.clone()),
        )
        .await
        .expect_err("default delete must fail");
        assert_eq!(default_delete.status, axum::http::StatusCode::BAD_REQUEST);

        // Deleting a non-default group cascades and bumps the routing revision.
        let member = state
            .user_store
            .create_user("member", "password123", UserRole::User, Some(&created.id))
            .await
            .expect("member created");
        let revision_before = state
            .routing_config_revision
            .load(std::sync::atomic::Ordering::Acquire);
        let Json(delete_body) = super::delete_group(
            State(state.clone()),
            admin_headers.clone(),
            Path(created.id.clone()),
        )
        .await
        .expect("delete succeeds");
        assert_eq!(delete_body["success"], serde_json::json!(true));
        assert!(
            state
                .routing_config_revision
                .load(std::sync::atomic::Ordering::Acquire)
                > revision_before
        );
        let member_after = state
            .user_store
            .get_user_by_id(&member.id)
            .await
            .expect("reads")
            .expect("exists");
        assert_eq!(member_after.group_id, default_id);

        // Deleting an unknown id is a 404.
        let missing = super::delete_group(
            State(state.clone()),
            admin_headers,
            Path("missing".to_string()),
        )
        .await
        .expect_err("missing delete must fail");
        assert_eq!(missing.status, axum::http::StatusCode::NOT_FOUND);
    }
}
