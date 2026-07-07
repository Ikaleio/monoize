use crate::app::AppState;
use crate::billing_rate_store::{
    BillingRateSyncResult, DbBillingRateRecord, UpsertBillingRateInput,
};
use crate::dashboard_handlers::session_helpers::require_admin;
use crate::error::{AppError, AppResult};
use crate::settings::PricingProfilePattern;
use axum::Json;
use axum::extract::{Path, State};
use axum::http::{HeaderMap, StatusCode};
use axum::response::IntoResponse;
use serde::{Deserialize, Serialize};
use serde_json::json;

pub async fn list_billing_rates(
    State(state): State<AppState>,
    headers: HeaderMap,
) -> AppResult<impl IntoResponse> {
    require_admin(&headers, &state).await?;
    let rows: Vec<DbBillingRateRecord> = state
        .billing_rate_store
        .list_billing_rates()
        .await
        .map_err(|e| AppError::new(StatusCode::INTERNAL_SERVER_ERROR, "internal_error", e))?;
    Ok(Json(rows))
}

pub async fn upsert_billing_rate(
    State(state): State<AppState>,
    headers: HeaderMap,
    Path(id): Path<String>,
    Json(input): Json<UpsertBillingRateInput>,
) -> AppResult<impl IntoResponse> {
    require_admin(&headers, &state).await?;
    let id = id.strip_prefix('/').unwrap_or(&id);
    let row = state
        .billing_rate_store
        .upsert_billing_rate(id, input)
        .await
        .map_err(|e| AppError::new(StatusCode::BAD_REQUEST, "invalid_request", e))?;
    Ok(Json(row))
}

pub async fn delete_billing_rate(
    State(state): State<AppState>,
    headers: HeaderMap,
    Path(id): Path<String>,
) -> AppResult<impl IntoResponse> {
    require_admin(&headers, &state).await?;
    let id = id.strip_prefix('/').unwrap_or(&id);
    let deleted = state
        .billing_rate_store
        .delete_billing_rate(id)
        .await
        .map_err(|e| AppError::new(StatusCode::INTERNAL_SERVER_ERROR, "internal_error", e))?;
    if !deleted {
        return Err(AppError::new(
            StatusCode::NOT_FOUND,
            "not_found",
            "billing rate not found",
        ));
    }
    Ok(Json(json!({ "success": true })))
}

pub async fn sync_billing_rates_catalog(
    State(state): State<AppState>,
    headers: HeaderMap,
) -> AppResult<impl IntoResponse> {
    require_admin(&headers, &state).await?;
    let result: BillingRateSyncResult = state
        .billing_rate_store
        .sync_catalog()
        .await
        .map_err(|e| AppError::new(StatusCode::INTERNAL_SERVER_ERROR, "internal_error", e))?;
    Ok(Json(result))
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PricingProfilePatternsResponse {
    pub patterns: Vec<PricingProfilePattern>,
}

#[derive(Debug, Clone, Deserialize)]
pub struct UpdatePricingProfilePatternsRequest {
    pub patterns: Vec<PricingProfilePattern>,
}

pub async fn get_pricing_profile_patterns(
    State(state): State<AppState>,
    headers: HeaderMap,
) -> AppResult<impl IntoResponse> {
    require_admin(&headers, &state).await?;
    let patterns = state
        .settings_store
        .get_pricing_profile_model_patterns()
        .await
        .map_err(|e| AppError::new(StatusCode::INTERNAL_SERVER_ERROR, "internal_error", e))?;
    Ok(Json(PricingProfilePatternsResponse { patterns }))
}

pub async fn update_pricing_profile_patterns(
    State(state): State<AppState>,
    headers: HeaderMap,
    Json(body): Json<UpdatePricingProfilePatternsRequest>,
) -> AppResult<impl IntoResponse> {
    require_admin(&headers, &state).await?;
    for pattern in &body.patterns {
        if pattern.pattern.trim().is_empty() || pattern.pricing_profile.trim().is_empty() {
            return Err(AppError::new(
                StatusCode::BAD_REQUEST,
                "invalid_request",
                "pattern and pricing_profile must not be empty",
            ));
        }
    }
    state
        .settings_store
        .set_pricing_profile_model_patterns(&body.patterns)
        .await
        .map_err(|e| AppError::new(StatusCode::INTERNAL_SERVER_ERROR, "internal_error", e))?;
    Ok(Json(PricingProfilePatternsResponse {
        patterns: body.patterns,
    }))
}
