use crate::app::AppState;
use crate::error::{AppError, AppResult};
use crate::users::{
    BillingPlan, BillingPlanInput, BillingPlanPrice, BillingPlanPriceInput, format_nano_to_usd,
};
use axum::Json;
use axum::extract::{Path, State};
use axum::http::{HeaderMap, StatusCode};
use axum::response::IntoResponse;
use serde::Deserialize;
use serde_json::json;

#[derive(Debug, Deserialize)]
pub struct BillingPlanPriceRequest {
    pub price_nano_usd: Option<String>,
    pub price_usd: Option<String>,
    pub duration_seconds: i64,
}

#[derive(Debug, Deserialize)]
pub struct CreateBillingPlanRequest {
    pub name: String,
    #[serde(default)]
    pub description: String,
    pub limit_5h_nano_usd: Option<String>,
    pub limit_24h_nano_usd: Option<String>,
    pub limit_7d_nano_usd: Option<String>,
    pub limit_30d_nano_usd: Option<String>,
    pub group_ids: Vec<String>,
    #[serde(default = "default_multiplier")]
    pub multiplier: String,
    #[serde(default)]
    pub listed: bool,
    #[serde(default)]
    pub prices: Vec<BillingPlanPriceRequest>,
}

pub type UpdateBillingPlanRequest = CreateBillingPlanRequest;

fn default_multiplier() -> String {
    "1".to_string()
}

#[derive(Debug, serde::Serialize)]
pub struct BillingPlanPriceResponse {
    pub id: String,
    pub price_nano_usd: String,
    pub price_usd: String,
    pub duration_seconds: i64,
    pub created_at: String,
}

#[derive(Debug, serde::Serialize)]
pub struct BillingPlanResponse {
    pub id: String,
    pub name: String,
    pub description: String,
    pub limit_5h_nano_usd: Option<String>,
    pub limit_24h_nano_usd: Option<String>,
    pub limit_7d_nano_usd: Option<String>,
    pub limit_30d_nano_usd: Option<String>,
    pub group_ids: Vec<String>,
    pub multiplier: String,
    pub listed: bool,
    pub prices: Vec<BillingPlanPriceResponse>,
    pub created_at: String,
    pub updated_at: String,
}

impl From<BillingPlanPrice> for BillingPlanPriceResponse {
    fn from(price: BillingPlanPrice) -> Self {
        let nano = price
            .price_nano_usd
            .parse::<i128>()
            .expect("UserStore validates persisted plan prices");
        Self {
            id: price.id,
            price_usd: format_nano_to_usd(nano),
            price_nano_usd: price.price_nano_usd,
            duration_seconds: price.duration_seconds,
            created_at: price.created_at.to_rfc3339(),
        }
    }
}

impl From<BillingPlan> for BillingPlanResponse {
    fn from(plan: BillingPlan) -> Self {
        Self {
            id: plan.id,
            name: plan.name,
            description: plan.description,
            limit_5h_nano_usd: plan.limit_5h_nano_usd,
            limit_24h_nano_usd: plan.limit_24h_nano_usd,
            limit_7d_nano_usd: plan.limit_7d_nano_usd,
            limit_30d_nano_usd: plan.limit_30d_nano_usd,
            group_ids: plan.group_ids,
            multiplier: plan.multiplier,
            listed: plan.listed,
            prices: plan.prices.into_iter().map(Into::into).collect(),
            created_at: plan.created_at.to_rfc3339(),
            updated_at: plan.updated_at.to_rfc3339(),
        }
    }
}

fn plan_input(body: CreateBillingPlanRequest) -> BillingPlanInput {
    BillingPlanInput {
        name: body.name,
        description: body.description,
        limit_5h_nano_usd: body.limit_5h_nano_usd,
        limit_24h_nano_usd: body.limit_24h_nano_usd,
        limit_7d_nano_usd: body.limit_7d_nano_usd,
        limit_30d_nano_usd: body.limit_30d_nano_usd,
        group_ids: body.group_ids,
        multiplier: body.multiplier,
        listed: body.listed,
        prices: body
            .prices
            .into_iter()
            .map(|price| BillingPlanPriceInput {
                price_nano_usd: price.price_nano_usd,
                price_usd: price.price_usd,
                duration_seconds: price.duration_seconds,
            })
            .collect(),
    }
}

fn map_plan_error(error: String) -> AppError {
    match error.as_str() {
        "plan_name_exists" => AppError::new(
            StatusCode::CONFLICT,
            "plan_name_exists",
            "a billing plan with this name already exists",
        ),
        "invalid_plan_name"
        | "invalid_plan_description"
        | "invalid_plan_limits"
        | "invalid_plan_groups"
        | "invalid_plan_multiplier"
        | "invalid_plan_prices" => AppError::new(StatusCode::BAD_REQUEST, error.clone(), error),
        "not_found" => AppError::new(StatusCode::NOT_FOUND, "not_found", "plan not found"),
        "plan_not_available" => AppError::new(
            StatusCode::CONFLICT,
            "plan_not_available",
            "billing plan or price is not available",
        ),
        "active_subscription_exists" => AppError::new(
            StatusCode::CONFLICT,
            "active_subscription_exists",
            "an active billing plan subscription already exists",
        ),
        "insufficient_balance" => AppError::new(
            StatusCode::PAYMENT_REQUIRED,
            "insufficient_balance",
            "insufficient prepaid balance",
        ),
        "user_disabled" => {
            AppError::new(StatusCode::FORBIDDEN, "user_disabled", "user is disabled")
        }
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
        .map_err(map_plan_error)?;
    Ok(Json(
        plans
            .into_iter()
            .map(BillingPlanResponse::from)
            .collect::<Vec<_>>(),
    ))
}

pub async fn list_billing_plan_marketplace(
    State(state): State<AppState>,
    headers: HeaderMap,
) -> AppResult<impl IntoResponse> {
    crate::dashboard_handlers::session_helpers::get_current_user(&headers, &state).await?;
    let plans = state
        .user_store
        .list_marketplace_billing_plans()
        .await
        .map_err(map_plan_error)?;
    Ok(Json(
        plans
            .into_iter()
            .map(BillingPlanResponse::from)
            .collect::<Vec<_>>(),
    ))
}

pub async fn get_billing_plan_subscription(
    State(state): State<AppState>,
    headers: HeaderMap,
) -> AppResult<impl IntoResponse> {
    let user =
        crate::dashboard_handlers::session_helpers::get_current_user(&headers, &state).await?;
    let subscription = state
        .user_store
        .get_active_billing_plan_subscription(&user.id)
        .await
        .map_err(map_plan_error)?;
    Ok(Json(subscription))
}

pub async fn create_billing_plan(
    State(state): State<AppState>,
    headers: HeaderMap,
    Json(body): Json<CreateBillingPlanRequest>,
) -> AppResult<impl IntoResponse> {
    crate::dashboard_handlers::session_helpers::require_admin(&headers, &state).await?;
    match state
        .user_store
        .create_billing_plan(plan_input(body))
        .await
        .map_err(map_plan_error)?
    {
        Ok(plan) => Ok((StatusCode::CREATED, Json(BillingPlanResponse::from(plan)))),
        Err(error) => Err(map_plan_error(error)),
    }
}

pub async fn update_billing_plan(
    State(state): State<AppState>,
    headers: HeaderMap,
    Path(plan_id): Path<String>,
    Json(body): Json<UpdateBillingPlanRequest>,
) -> AppResult<impl IntoResponse> {
    crate::dashboard_handlers::session_helpers::require_admin(&headers, &state).await?;
    match state
        .user_store
        .update_billing_plan(&plan_id, plan_input(body))
        .await
        .map_err(map_plan_error)?
    {
        Ok(()) => Ok(Json(json!({ "success": true }))),
        Err(error) => Err(map_plan_error(error)),
    }
}

pub async fn delete_billing_plan(
    State(state): State<AppState>,
    headers: HeaderMap,
    Path(plan_id): Path<String>,
) -> AppResult<impl IntoResponse> {
    crate::dashboard_handlers::session_helpers::require_admin(&headers, &state).await?;
    state
        .user_store
        .delete_billing_plan(&plan_id)
        .await
        .map_err(map_plan_error)?;
    Ok(Json(json!({ "success": true })))
}

#[derive(Debug, Deserialize)]
pub struct PurchaseBillingPlanRequest {
    pub price_id: String,
}

pub async fn purchase_billing_plan(
    State(state): State<AppState>,
    headers: HeaderMap,
    Path(plan_id): Path<String>,
    Json(body): Json<PurchaseBillingPlanRequest>,
) -> AppResult<impl IntoResponse> {
    let user =
        crate::dashboard_handlers::session_helpers::get_current_user(&headers, &state).await?;
    match state
        .user_store
        .purchase_billing_plan(&user.id, &plan_id, &body.price_id)
        .await
        .map_err(map_plan_error)?
    {
        Ok(subscription) => Ok((StatusCode::CREATED, Json(subscription))),
        Err(error) => Err(map_plan_error(error)),
    }
}
