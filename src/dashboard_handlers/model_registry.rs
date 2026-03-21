use crate::app::AppState;
use crate::dashboard_handlers::session_helpers::require_admin;
use crate::error::{AppError, AppResult};
use crate::model_registry_store::{
    CreateModelInput, DbModelMetadataRecord, ModelMetadataSyncResult, UpdateModelInput,
    UpsertModelMetadataInput,
};
use axum::Json;
use axum::extract::{Path, State};
use axum::http::{HeaderMap, StatusCode};
use axum::response::IntoResponse;
use serde_json::json;

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
