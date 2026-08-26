//! `model_prices` dashboard endpoints (`model-pricing.spec.md` §10).

use crate::app::AppState;
use crate::dashboard_handlers::session_helpers::require_admin;
use crate::error::{AppError, AppResult};
use crate::model_price_store::UpsertModelPriceInput;
use crate::model_price_sync::{PriceSyncError, PriceSyncSource};
use crate::settings::normalize_pricing_model_key;
use axum::Json;
use axum::extract::{Path, Query, State};
use axum::http::{HeaderMap, StatusCode};
use axum::response::IntoResponse;
use serde::Deserialize;
use serde_json::json;
use std::collections::HashMap;

/// MP-A1: list all price rows ordered by `model_id ASC`.
pub async fn list_model_prices(
    State(state): State<AppState>,
    headers: HeaderMap,
) -> AppResult<impl IntoResponse> {
    require_admin(&headers, &state).await?;
    let rows = state
        .model_price_store
        .list()
        .await
        .map_err(|e| AppError::new(StatusCode::INTERNAL_SERVER_ERROR, "internal_error", e))?;
    Ok(Json(rows))
}

/// MP-A2: merge-upsert one price row.
pub async fn upsert_model_price(
    State(state): State<AppState>,
    headers: HeaderMap,
    Path(model_id): Path<String>,
    Json(input): Json<UpsertModelPriceInput>,
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
        .model_price_store
        .upsert(model_id, input)
        .await
        .map_err(|e| AppError::new(StatusCode::BAD_REQUEST, "invalid_request", e))?;
    Ok(Json(record))
}

/// MP-A3: delete one price row.
pub async fn delete_model_price(
    State(state): State<AppState>,
    headers: HeaderMap,
    Path(model_id): Path<String>,
) -> AppResult<impl IntoResponse> {
    require_admin(&headers, &state).await?;
    let model_id = model_id.strip_prefix('/').unwrap_or(&model_id);
    let deleted = state
        .model_price_store
        .delete(model_id)
        .await
        .map_err(|e| AppError::new(StatusCode::INTERNAL_SERVER_ERROR, "internal_error", e))?;
    if !deleted {
        return Err(AppError::new(
            StatusCode::NOT_FOUND,
            "not_found",
            "model price not found",
        ));
    }
    Ok(Json(json!({ "success": true })))
}

/// MP-A4: routable logical models whose pricing key has no applicable price.
pub async fn list_unpriced_models(
    State(state): State<AppState>,
    headers: HeaderMap,
) -> AppResult<impl IntoResponse> {
    require_admin(&headers, &state).await?;
    let available = state
        .monoize_store
        .list_available_model_names()
        .await
        .map_err(|e| AppError::new(StatusCode::INTERNAL_SERVER_ERROR, "internal_error", e))?;
    let rows = state
        .model_price_store
        .list()
        .await
        .map_err(|e| AppError::new(StatusCode::INTERNAL_SERVER_ERROR, "internal_error", e))?;
    let priced: HashMap<&str, bool> = rows
        .iter()
        .map(|row| (row.model_id.as_str(), row.enabled && row.is_complete()))
        .collect();
    let reasoning_suffix_map = state
        .monoize_runtime
        .read()
        .await
        .reasoning_suffix_map
        .clone();
    let mut models: Vec<String> = available
        .into_iter()
        .filter(|model| {
            let key = normalize_pricing_model_key(model, &reasoning_suffix_map);
            !priced.get(key.as_str()).copied().unwrap_or(false)
        })
        .collect();
    models.sort();
    Ok(Json(json!({ "models": models })))
}

#[derive(Debug, Deserialize)]
pub struct PriceSyncRunsQuery {
    pub limit: Option<u64>,
}

/// MP-A5: most recent sync runs, default limit 20, maximum 100.
pub async fn list_price_sync_runs(
    State(state): State<AppState>,
    headers: HeaderMap,
    Query(query): Query<PriceSyncRunsQuery>,
) -> AppResult<impl IntoResponse> {
    require_admin(&headers, &state).await?;
    let runs = state
        .model_price_store
        .list_sync_runs(query.limit.unwrap_or(20))
        .await
        .map_err(|e| AppError::new(StatusCode::INTERNAL_SERVER_ERROR, "internal_error", e))?;
    Ok(Json(runs))
}

fn map_sync_error(error: PriceSyncError) -> AppError {
    match error {
        PriceSyncError::InvalidRequest(message) => {
            AppError::new(StatusCode::BAD_REQUEST, "invalid_request", message)
        }
        PriceSyncError::Upstream(message) => {
            AppError::new(StatusCode::BAD_GATEWAY, "upstream_fetch_failed", message)
        }
        PriceSyncError::Storage(message) => {
            AppError::new(StatusCode::INTERNAL_SERVER_ERROR, "internal_error", message)
        }
    }
}

/// MP-A6: fetch and compare one source without changing price rows.
pub async fn preview_price_sync(
    State(state): State<AppState>,
    headers: HeaderMap,
    Path(source): Path<String>,
) -> AppResult<impl IntoResponse> {
    require_admin(&headers, &state).await?;
    let source = PriceSyncSource::parse(&source).map_err(map_sync_error)?;
    let settings = state
        .settings_store
        .get_all()
        .await
        .map_err(|error| {
            AppError::new(StatusCode::INTERNAL_SERVER_ERROR, "internal_error", error)
        })?;
    let preview = state
        .model_price_store
        .preview_sync(&state.http, source, &settings)
        .await
        .map_err(map_sync_error)?;
    Ok(Json(preview))
}

/// MP-A7: fetch and atomically apply one source.
pub async fn apply_price_sync(
    State(state): State<AppState>,
    headers: HeaderMap,
    Path(source): Path<String>,
) -> AppResult<impl IntoResponse> {
    require_admin(&headers, &state).await?;
    let source = PriceSyncSource::parse(&source).map_err(map_sync_error)?;
    let settings = state
        .settings_store
        .get_all()
        .await
        .map_err(|error| {
            AppError::new(StatusCode::INTERNAL_SERVER_ERROR, "internal_error", error)
        })?;
    let run = state
        .model_price_store
        .apply_sync(&state.http, source, &settings)
        .await
        .map_err(map_sync_error)?;
    Ok(Json(run))
}
