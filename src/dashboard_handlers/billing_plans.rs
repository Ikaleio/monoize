use crate::app::AppState;
use crate::error::{AppError, AppResult};
use crate::users::{BillingPlan, BillingPlanInput, format_nano_to_usd};
use axum::Json;
use axum::extract::{Path, State};
use axum::http::{HeaderMap, StatusCode};
use axum::response::IntoResponse;
use serde::Deserialize;
use serde_json::json;

#[derive(Debug, Deserialize)]
pub struct CreateBillingPlanRequest {
    pub name: String,
    pub grant_amount_nano_usd: Option<String>,
    pub grant_amount_usd: Option<String>,
    pub period_seconds: i64,
    #[serde(default)]
    pub allowed_groups: Option<Vec<String>>,
    #[serde(default)]
    pub enabled: Option<bool>,
}

#[derive(Debug, Deserialize)]
pub struct UpdateBillingPlanRequest {
    pub name: String,
    pub grant_amount_nano_usd: Option<String>,
    pub grant_amount_usd: Option<String>,
    pub period_seconds: i64,
    #[serde(default)]
    pub allowed_groups: Option<Vec<String>>,
    #[serde(default)]
    pub enabled: Option<bool>,
}

#[derive(Debug, serde::Serialize)]
pub struct BillingPlanResponse {
    pub id: String,
    pub name: String,
    pub grant_amount_nano_usd: String,
    pub grant_amount_usd: String,
    pub period_seconds: i64,
    pub allowed_groups: Vec<String>,
    pub enabled: bool,
    pub created_at: String,
    pub updated_at: String,
}

impl From<BillingPlan> for BillingPlanResponse {
    fn from(plan: BillingPlan) -> Self {
        let nano = plan
            .grant_amount_nano_usd
            .parse::<i128>()
            .expect("UserStore must validate persisted plan amounts");
        Self {
            id: plan.id,
            name: plan.name,
            grant_amount_usd: format_nano_to_usd(nano),
            grant_amount_nano_usd: plan.grant_amount_nano_usd,
            period_seconds: plan.period_seconds,
            allowed_groups: plan.allowed_groups,
            enabled: plan.enabled,
            created_at: plan.created_at.to_rfc3339(),
            updated_at: plan.updated_at.to_rfc3339(),
        }
    }
}

fn plan_input(
    name: String,
    grant_amount_nano_usd: Option<String>,
    grant_amount_usd: Option<String>,
    period_seconds: i64,
    allowed_groups: Option<Vec<String>>,
    enabled: Option<bool>,
) -> BillingPlanInput {
    BillingPlanInput {
        name,
        grant_amount_nano_usd,
        grant_amount_usd,
        period_seconds,
        allowed_groups,
        enabled,
    }
}

fn map_plan_inner_error(error: String) -> AppError {
    match error.as_str() {
        "plan_name_exists" => AppError::new(
            StatusCode::CONFLICT,
            "plan_name_exists",
            "a billing plan with this name already exists",
        ),
        "invalid_period" | "invalid_grant_amount" | "invalid_plan_name" => {
            AppError::new(StatusCode::BAD_REQUEST, error.clone(), error)
        }
        "plan_in_use" => AppError::new(
            StatusCode::CONFLICT,
            "plan_in_use",
            "billing plan is assigned to at least one user",
        ),
        _ => AppError::new(StatusCode::INTERNAL_SERVER_ERROR, "internal_error", error),
    }
}

pub async fn list_billing_plans(
    State(state): State<AppState>,
    headers: HeaderMap,
) -> AppResult<impl IntoResponse> {
    crate::dashboard_handlers::session_helpers::require_admin(&headers, &state).await?;

    let plans = state
        .user_store
        .list_billing_plans()
        .await
        .map_err(|e| AppError::new(StatusCode::INTERNAL_SERVER_ERROR, "internal_error", e))?;

    let responses: Vec<BillingPlanResponse> = plans.into_iter().map(Into::into).collect();
    Ok(Json(responses))
}

pub async fn create_billing_plan(
    State(state): State<AppState>,
    headers: HeaderMap,
    Json(body): Json<CreateBillingPlanRequest>,
) -> AppResult<impl IntoResponse> {
    crate::dashboard_handlers::session_helpers::require_admin(&headers, &state).await?;

    match state
        .user_store
        .create_billing_plan(plan_input(
            body.name,
            body.grant_amount_nano_usd,
            body.grant_amount_usd,
            body.period_seconds,
            body.allowed_groups,
            body.enabled,
        ))
        .await
        .map_err(|e| AppError::new(StatusCode::INTERNAL_SERVER_ERROR, "internal_error", e))?
    {
        Ok(plan) => Ok((StatusCode::CREATED, Json(BillingPlanResponse::from(plan)))),
        Err(error) => Err(map_plan_inner_error(error)),
    }
}

pub async fn update_billing_plan(
    State(state): State<AppState>,
    headers: HeaderMap,
    Path(plan_id): Path<String>,
    Json(body): Json<UpdateBillingPlanRequest>,
) -> AppResult<impl IntoResponse> {
    crate::dashboard_handlers::session_helpers::require_admin(&headers, &state).await?;

    let input = plan_input(
        body.name,
        body.grant_amount_nano_usd,
        body.grant_amount_usd,
        body.period_seconds,
        body.allowed_groups,
        body.enabled,
    );

    match state
        .user_store
        .update_billing_plan(&plan_id, input)
        .await
        .map_err(|e| {
            if e == "not_found" {
                AppError::new(StatusCode::NOT_FOUND, "not_found", "plan not found")
            } else {
                AppError::new(StatusCode::INTERNAL_SERVER_ERROR, "internal_error", e)
            }
        })? {
        Ok(()) => Ok(Json(json!({ "success": true }))),
        Err(error) => Err(map_plan_inner_error(error)),
    }
}

pub async fn delete_billing_plan(
    State(state): State<AppState>,
    headers: HeaderMap,
    Path(plan_id): Path<String>,
) -> AppResult<impl IntoResponse> {
    crate::dashboard_handlers::session_helpers::require_admin(&headers, &state).await?;

    match state
        .user_store
        .delete_billing_plan(&plan_id)
        .await
        .map_err(|e| {
            if e == "not_found" {
                AppError::new(StatusCode::NOT_FOUND, "not_found", "plan not found")
            } else {
                AppError::new(StatusCode::INTERNAL_SERVER_ERROR, "internal_error", e)
            }
        })? {
        Ok(()) => Ok(Json(json!({ "success": true }))),
        Err(error) => Err(map_plan_inner_error(error)),
    }
}
