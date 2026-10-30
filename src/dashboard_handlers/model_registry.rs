use crate::app::AppState;
use crate::dashboard_handlers::session_helpers::{get_current_user, require_admin};
use crate::error::{AppError, AppResult};
use crate::users::Group;
use crate::model_registry_store::{
    CreateModelInput, DbModelMetadataRecord, ModelMetadataSyncResult, UpdateModelInput,
    UpsertModelMetadataInput,
};
use axum::Json;
use axum::extract::{Path, State};
use axum::http::{HeaderMap, StatusCode};
use axum::response::IntoResponse;
use rust_decimal::Decimal;
use serde_json::json;
use std::collections::{HashMap, HashSet};

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
        .map_err(|error| {
            if error.contains("fetch_failed") || error.contains("parse_failed") {
                AppError::new(StatusCode::BAD_GATEWAY, "upstream_fetch_failed", error)
            } else {
                AppError::new(StatusCode::INTERNAL_SERVER_ERROR, "internal_error", error)
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
        .map_err(|e| {
            if e.starts_with("invalid_request:") {
                AppError::new(StatusCode::BAD_REQUEST, "invalid_request", e)
            } else {
                AppError::new(StatusCode::INTERNAL_SERVER_ERROR, "internal_error", e)
            }
        })?;
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

/// `model-marketplace.spec.md` §3: login (any role), not admin-only. Admins
/// see every offered model at base price. Other users see only models a
/// Provider serves for one of their reachable groups (own group ∪
/// user-selectable groups), priced × the `billing_ratio` of the group that
/// would bill the row (MM-D4).
pub async fn list_marketplace_models(
    State(state): State<AppState>,
    headers: HeaderMap,
) -> AppResult<impl IntoResponse> {
    let user = get_current_user(&headers, &state).await?;
    let internal = |e: String| AppError::new(StatusCode::INTERNAL_SERVER_ERROR, "internal_error", e);
    let is_admin = user.role.can_manage_users();

    let metadata = state
        .model_registry_store
        .list_marketplace_model_metadata()
        .await
        .map_err(internal)?;
    let prices = state.model_price_store.list().await.map_err(internal)?;
    let price_by_id: HashMap<&str, &crate::model_price_store::ModelPriceRecord> = prices
        .iter()
        .filter(|price| price.enabled)
        .map(|price| (price.model_id.as_str(), price))
        .collect();
    let groups = state.user_store.list_groups().await.map_err(internal)?;
    let own_group = groups
        .iter()
        .find(|group| group.id == user.group_id)
        .or_else(|| groups.iter().find(|group| group.is_default));
    let reachable: HashSet<&str> = groups
        .iter()
        .filter(|group| group.user_selectable || Some(group.id.as_str()) == own_group.map(|g| g.id.as_str()))
        .map(|group| group.id.as_str())
        .collect();
    // MM-D4: the ratio of the group that would bill this row (MP-G1), not a
    // blanket own-group ratio: a model reachable only through a selectable
    // group is never billed at the caller's own group ratio.
    let ratio_of = |group: Option<&Group>| {
        group
            .and_then(|group| Decimal::from_str_exact(&group.billing_ratio).ok())
            .unwrap_or(Decimal::ONE)
    };

    let mut rows: Vec<serde_json::Value> = Vec::with_capacity(metadata.len());
    for record in metadata {
        let providers = state
            .monoize_store
            .list_providers_for_model(&record.model_id)
            .await
            .map_err(internal)?;
        let serving: HashSet<&str> = providers
            .iter()
            .flat_map(|provider| provider.group_ids.iter().map(String::as_str))
            .collect();
        // Registry order, restricted to the groups this caller can route through.
        let model_groups: Vec<&str> = groups
            .iter()
            .filter(|group| serving.contains(group.id.as_str()))
            .filter(|group| is_admin || reachable.contains(group.id.as_str()))
            .map(|group| group.name.as_str())
            .collect();
        if !is_admin && model_groups.is_empty() {
            continue;
        }
        let ratio = if is_admin {
            Decimal::ONE
        } else {
            ratio_of(
                own_group
                    .filter(|group| serving.contains(group.id.as_str()))
                    .or_else(|| {
                        groups.iter().find(|group| {
                            serving.contains(group.id.as_str()) && reachable.contains(group.id.as_str())
                        })
                    }),
            )
        };
        let price = price_by_id.get(record.model_id.as_str()).copied();
        let mut value = serde_json::to_value(&record).unwrap_or_else(|_| json!({}));
        value["input_usd_per_1m"] = json!(scale_price(price.and_then(|p| p.input_usd_per_1m.as_deref()), ratio));
        value["output_usd_per_1m"] = json!(scale_price(price.and_then(|p| p.output_usd_per_1m.as_deref()), ratio));
        value["per_request_usd"] = json!(scale_price(price.and_then(|p| p.per_request_usd.as_deref()), ratio));
        value["billing_mode"] = json!(price.map(|p| p.billing_mode.as_str()));
        value["billing_expr"] = json!(price.and_then(|p| p.billing_expr.as_ref()).map(|expr| scale_billing_expr(expr, ratio)));
        value["groups"] = json!(model_groups);
        value["price_ratio"] = json!(ratio.normalize().to_string());
        rows.push(value);
    }

    Ok(Json(rows))
}

/// Exact `price × ratio`, truncated to the MP-U1 9 fractional digits.
fn scale_price(raw: Option<&str>, ratio: Decimal) -> Option<String> {
    let value = Decimal::from_str_exact(raw?.trim()).ok()?;
    Some(
        (value * ratio)
            .round_dp_with_strategy(9, rust_decimal::RoundingStrategy::ToZero)
            .normalize()
            .to_string(),
    )
}

/// Scales every `*_usd_per_1m` price inside `tiers` and each
/// `service_tiers.<name>.tiers` table; thresholds are unchanged.
fn scale_billing_expr(expr: &serde_json::Value, ratio: Decimal) -> serde_json::Value {
    let scale_tiers = |tiers: &serde_json::Value| -> serde_json::Value {
        let Some(tiers) = tiers.as_array() else {
            return tiers.clone();
        };
        tiers
            .iter()
            .map(|tier| {
                let mut tier = tier.clone();
                if let Some(fields) = tier.as_object_mut() {
                    for (key, value) in fields.iter_mut() {
                        if key.ends_with("_usd_per_1m")
                            && let Some(scaled) = scale_price(value.as_str(), ratio)
                        {
                            *value = json!(scaled);
                        }
                    }
                }
                tier
            })
            .collect()
    };
    let mut scaled = expr.clone();
    if let Some(tiers) = expr.get("tiers") {
        scaled["tiers"] = scale_tiers(tiers);
    }
    if let Some(tables) = expr.get("service_tiers").and_then(|v| v.as_object()) {
        for (name, table) in tables {
            if let Some(tiers) = table.get("tiers") {
                scaled["service_tiers"][name]["tiers"] = scale_tiers(tiers);
            }
        }
    }
    scaled
}
