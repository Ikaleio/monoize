//! Admin CRUD endpoints for custom JS transforms
//! (`custom-js-transforms.spec.md` §5).

use crate::app::AppState;
use crate::custom_transforms::CustomTransformError;
use crate::error::{AppError, AppResult};
use axum::Json;
use axum::extract::{Path, State};
use axum::http::{HeaderMap, StatusCode};
use axum::response::IntoResponse;
use serde::Deserialize;
use serde_json::json;

#[derive(Debug, Deserialize)]
pub struct CreateCustomTransformRequest {
    pub source: String,
    #[serde(default)]
    pub enabled: Option<bool>,
}

#[derive(Debug, Deserialize)]
pub struct UpdateCustomTransformRequest {
    #[serde(default)]
    pub source: Option<String>,
    #[serde(default)]
    pub enabled: Option<bool>,
}

fn map_custom_transform_error(error: CustomTransformError) -> AppError {
    match error {
        CustomTransformError::Invalid(detail) => {
            AppError::new(StatusCode::BAD_REQUEST, "invalid_custom_transform", detail)
        }
        CustomTransformError::Exists => AppError::new(
            StatusCode::CONFLICT,
            "custom_transform_exists",
            "a custom transform with this id already exists",
        ),
        CustomTransformError::NotFound => AppError::new(
            StatusCode::NOT_FOUND,
            "not_found",
            "custom transform not found",
        ),
        CustomTransformError::Internal(detail) => {
            AppError::new(StatusCode::INTERNAL_SERVER_ERROR, "internal_error", detail)
        }
    }
}

pub async fn list_custom_transforms(
    State(state): State<AppState>,
    headers: HeaderMap,
) -> AppResult<impl IntoResponse> {
    crate::dashboard_handlers::session_helpers::require_admin(&headers, &state).await?;
    let transforms = state
        .custom_transform_store
        .list()
        .await
        .map_err(|e| AppError::new(StatusCode::INTERNAL_SERVER_ERROR, "internal_error", e))?;
    Ok(Json(json!({ "transforms": transforms })))
}

pub async fn create_custom_transform(
    State(state): State<AppState>,
    headers: HeaderMap,
    Json(body): Json<CreateCustomTransformRequest>,
) -> AppResult<impl IntoResponse> {
    crate::dashboard_handlers::session_helpers::require_admin(&headers, &state).await?;
    let record = state
        .custom_transform_store
        .create(body.source, body.enabled.unwrap_or(true))
        .await
        .map_err(map_custom_transform_error)?;
    Ok((StatusCode::CREATED, Json(record)))
}

pub async fn update_custom_transform(
    State(state): State<AppState>,
    headers: HeaderMap,
    Path(id): Path<String>,
    Json(body): Json<UpdateCustomTransformRequest>,
) -> AppResult<impl IntoResponse> {
    crate::dashboard_handlers::session_helpers::require_admin(&headers, &state).await?;
    let record = state
        .custom_transform_store
        .update(&id, body.source, body.enabled)
        .await
        .map_err(map_custom_transform_error)?;
    Ok(Json(record))
}

pub async fn delete_custom_transform(
    State(state): State<AppState>,
    headers: HeaderMap,
    Path(id): Path<String>,
) -> AppResult<impl IntoResponse> {
    crate::dashboard_handlers::session_helpers::require_admin(&headers, &state).await?;
    state
        .custom_transform_store
        .delete(&id)
        .await
        .map_err(map_custom_transform_error)?;
    Ok(Json(json!({ "success": true })))
}
