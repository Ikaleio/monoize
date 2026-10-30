//! `model_prices` dashboard endpoints (`model-pricing.spec.md` §10).

use crate::app::AppState;
use crate::dashboard_handlers::session_helpers::require_admin;
use crate::error::{AppError, AppResult};
use crate::model_price_store::UpsertModelPriceInput;
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
    // MP-A4a: the enabled Provider/Channel pairs that make each model routable.
    let mut sources = serde_json::Map::new();
    for model in &models {
        let providers = state
            .monoize_store
            .list_providers_for_model(model)
            .await
            .map_err(|e| AppError::new(StatusCode::INTERNAL_SERVER_ERROR, "internal_error", e))?;
        let pairs: Vec<serde_json::Value> = providers
            .iter()
            .flat_map(|provider| {
                provider.channels.iter().map(move |channel| {
                    json!({
                        "provider_id": provider.id,
                        "provider_name": provider.name,
                        "channel_id": channel.id,
                        "channel_name": channel.name,
                    })
                })
            })
            .collect();
        sources.insert(model.clone(), json!(pairs));
    }
    Ok(Json(json!({ "models": models, "sources": sources })))
}

#[derive(Debug, Deserialize)]
pub struct PriceSyncRunsQuery {
    pub limit: Option<u64>,
}

fn parse_sync_source(raw: &str) -> AppResult<crate::price_sync::SyncSource> {
    crate::price_sync::SyncSource::parse(raw).ok_or_else(|| {
        AppError::new(
            StatusCode::BAD_REQUEST,
            "invalid_request",
            "source must be one of models_dev, openrouter, new_api",
        )
    })
}

async fn new_api_config(state: &AppState) -> AppResult<(String, String)> {
    let settings = state
        .settings_store
        .get_all()
        .await
        .map_err(|e| AppError::new(StatusCode::INTERNAL_SERVER_ERROR, "internal_error", e))?;
    Ok((
        settings.price_sync_new_api_base_url,
        settings.price_sync_new_api_token,
    ))
}

fn map_sync_error(error: String) -> AppError {
    if error.starts_with("source_disabled: ") {
        new_api_not_configured()
    } else if error.starts_with("fetch_failed") || error.starts_with("parse_failed") {
        // MP-Y3: fetch and parse failures map to 502 upstream_fetch_failed.
        AppError::new(StatusCode::BAD_GATEWAY, "upstream_fetch_failed", error)
    } else {
        AppError::new(StatusCode::INTERNAL_SERVER_ERROR, "internal_error", error)
    }
}

/// MP-Y2: the `new_api` source is disabled while its base URL is empty.
fn new_api_not_configured() -> AppError {
    AppError::new(
        StatusCode::BAD_REQUEST,
        "price_sync_new_api_not_configured",
        "the new-api base URL is not set",
    )
}

/// MP-A6: fetch the source, store the snapshot for MP-A7, and return the
/// computed diff without writing `model_prices` rows.
pub async fn preview_price_sync(
    State(state): State<AppState>,
    headers: HeaderMap,
    Path(source): Path<String>,
) -> AppResult<impl IntoResponse> {
    require_admin(&headers, &state).await?;
    let source = parse_sync_source(&source)?;
    let (base_url, token) = new_api_config(&state).await?;
    let snapshot =
        crate::price_sync::fetch_source_snapshot(&state.http, source, (&base_url, &token))
            .await
            .map_err(map_sync_error)?;
    let existing = state
        .model_price_store
        .list()
        .await
        .map_err(|e| AppError::new(StatusCode::INTERNAL_SERVER_ERROR, "internal_error", e))?;
    Ok(Json(crate::price_sync::store_preview(
        source, &existing, snapshot,
    )))
}

#[derive(Debug, Deserialize)]
struct ApplyPriceSyncInput {
    preview_id: String,
}

/// MP-A7: apply exactly the snapshot shown by an MP-A6 preview and return the
/// finalized run row.
pub async fn apply_price_sync(
    State(state): State<AppState>,
    headers: HeaderMap,
    Path(source): Path<String>,
    body: axum::body::Bytes,
) -> AppResult<impl IntoResponse> {
    require_admin(&headers, &state).await?;
    let source = parse_sync_source(&source)?;
    let preview = serde_json::from_slice::<ApplyPriceSyncInput>(&body)
        .ok()
        .and_then(|input| crate::price_sync::take_preview(input.preview_id.trim(), source))
        .ok_or_else(|| {
            AppError::new(
                StatusCode::CONFLICT,
                "price_sync_preview_expired",
                "the preview is missing or expired; preview again",
            )
        })?;
    match crate::price_sync::apply_previewed_sync_run(
        &state.model_price_store,
        &state.model_registry_store,
        preview,
    )
    .await
    {
        Ok(run) => Ok(Json(run)),
        Err(error) if error == crate::price_sync::PREVIEW_STALE_ERROR => Err(AppError::new(
            StatusCode::CONFLICT,
            "price_sync_preview_stale",
            "stored prices changed since the preview; preview again",
        )),
        Err(error) => Err(map_sync_error(error)),
    }
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
