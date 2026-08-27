//! Recharge HTTP surface (`recharge-system.spec.md` §5, §6, §8, §9): order
//! creation, the provider notify webhook, refunds, channel CRUD, and the
//! billing-ledger read endpoint.

use crate::app::AppState;
use crate::dashboard_handlers::session_helpers::{get_current_user, require_admin};
use crate::error::{AppError, AppResult};
use crate::recharge::amount::{
    format_minor_units, parse_canonical_positive_nano, parse_positive_decimal,
    parse_positive_usd_to_nano, pay_minor_units,
};
use crate::recharge::store::{
    ChannelWriteError, LedgerListFilter, OrderListFilter, RechargeChannel, RechargeOrder,
};
use crate::recharge::{AckOutcome, NotifyOutcome, PaymentUrls, Verification, adapter_for};
use crate::users::format_nano_to_usd;
use axum::Json;
use axum::extract::{Path, Query, State};
use axum::http::{HeaderMap, StatusCode};
use axum::response::{IntoResponse, Response};
use chrono::{Duration, Utc};
use serde::Deserialize;
use serde_json::{Value, json};

fn internal(message: impl Into<String>) -> AppError {
    AppError::new(StatusCode::INTERNAL_SERVER_ERROR, "internal_error", message)
}

/// RC-A3 order object. `username` is included only for admin callers.
fn order_json(order: &RechargeOrder, include_username: bool) -> Value {
    let mut object = json!({
        "id": order.id,
        "user_id": order.user_id,
        "payment_channel_id": order.payment_channel_id,
        "channel_type_id": order.channel_type_id,
        "channel_name": order.channel_name,
        "status": order.status,
        "credit_nano_usd": order.credit_nano_usd.to_string(),
        "credit_usd": format_nano_to_usd(order.credit_nano_usd),
        "pay_currency": order.pay_currency,
        "pay_amount": order.pay_amount,
        "usd_rate": order.usd_rate,
        "provider_order_id": order.provider_order_id,
        "error_code": order.error_code,
        "paid_at": order.paid_at,
        "expires_at": order.expires_at,
        "created_at": order.created_at,
    });
    if include_username
        && let Some(map) = object.as_object_mut()
    {
        map.insert("username".to_string(), order.username.clone().into());
    }
    object
}

// ---------------------------------------------------------------------------
// §9.1 RC-A1: enabled channels for any authenticated user
// ---------------------------------------------------------------------------

pub async fn list_recharge_channels(
    State(state): State<AppState>,
    headers: HeaderMap,
) -> AppResult<impl IntoResponse> {
    get_current_user(&headers, &state).await?;
    let channels = state
        .user_store
        .list_payment_channels(true)
        .await
        .map_err(internal)?;
    let channels = channels
        .iter()
        .map(|channel| {
            // Registry membership is enforced at write time (RC-P1).
            let pay_scale = adapter_for(&channel.type_id)
                .and_then(|adapter| adapter.currency_scale(&channel.currency).ok())
                .unwrap_or(2);
            json!({
                "id": channel.id,
                "name": channel.name,
                "type_id": channel.type_id,
                "currency": channel.currency,
                "usd_rate": channel.usd_rate,
                "min_credit_usd": channel.min_credit_usd,
                "max_credit_usd": channel.max_credit_usd,
                "pay_scale": pay_scale,
            })
        })
        .collect::<Vec<_>>();
    Ok(Json(json!({ "channels": channels })))
}

// ---------------------------------------------------------------------------
// §5 order creation
// ---------------------------------------------------------------------------

#[derive(Debug, Deserialize)]
pub struct CreateOrderRequest {
    pub payment_channel_id: String,
    pub credit_nano_usd: Option<String>,
    pub credit_usd: Option<String>,
}

pub async fn create_recharge_order(
    State(state): State<AppState>,
    headers: HeaderMap,
    Json(body): Json<CreateOrderRequest>,
) -> AppResult<impl IntoResponse> {
    let user = get_current_user(&headers, &state).await?;

    // RC-O3 step 1: unknown channel.
    let channel = state
        .user_store
        .get_payment_channel(&body.payment_channel_id)
        .await
        .map_err(internal)?
        .ok_or_else(|| {
            AppError::new(StatusCode::NOT_FOUND, "not_found", "payment channel not found")
        })?;
    // RC-O3 step 2: disabled channel.
    if !channel.enabled {
        return Err(AppError::new(
            StatusCode::CONFLICT,
            "channel_disabled",
            "payment channel is disabled",
        ));
    }
    // RC-O3 step 3 / RC-G3: origin unset.
    let origin = state
        .settings_store
        .get("recharge_public_origin")
        .await
        .map_err(internal)?
        .map(|value| value.trim().to_string())
        .unwrap_or_default();
    if origin.is_empty() {
        return Err(AppError::new(
            StatusCode::CONFLICT,
            "recharge_origin_unset",
            "recharge_public_origin is not configured",
        ));
    }
    // RC-O3 step 4 / RC-O2: nano wins when both amounts are present.
    let invalid_amount =
        || AppError::new(StatusCode::BAD_REQUEST, "invalid_amount", "invalid recharge amount");
    let credit_nano_usd = match (&body.credit_nano_usd, &body.credit_usd) {
        (Some(nano), _) => parse_canonical_positive_nano(nano).map_err(|_| invalid_amount())?,
        (None, Some(usd)) => parse_positive_usd_to_nano(usd).map_err(|_| invalid_amount())?,
        (None, None) => return Err(invalid_amount()),
    };
    // RC-O3 step 5: inclusive bounds compared in nano-USD.
    let min_nano = parse_positive_usd_to_nano(&channel.min_credit_usd)
        .map_err(|e| internal(format!("stored min_credit_usd invalid: {e}")))?;
    let max_nano = parse_positive_usd_to_nano(&channel.max_credit_usd)
        .map_err(|e| internal(format!("stored max_credit_usd invalid: {e}")))?;
    if credit_nano_usd < min_nano || credit_nano_usd > max_nano {
        return Err(AppError::new(
            StatusCode::BAD_REQUEST,
            "amount_out_of_range",
            "amount is outside the channel's credit bounds",
        ));
    }
    // RC-O3 step 6: per-user pending cap.
    let pending = state
        .user_store
        .count_pending_orders(&user.id)
        .await
        .map_err(internal)?;
    if pending >= crate::recharge::max_pending_orders() as i64 {
        return Err(AppError::new(
            StatusCode::TOO_MANY_REQUESTS,
            "too_many_pending_orders",
            "too many pending recharge orders",
        ));
    }

    let adapter = adapter_for(&channel.type_id).ok_or_else(|| {
        internal(format!("stored type_id {:?} not in registry", channel.type_id))
    })?;
    let scale = adapter
        .currency_scale(&channel.currency)
        .map_err(internal)?;
    // RC-U6: freeze usd_rate and pay_amount into the order row.
    let rate = parse_positive_decimal(&channel.usd_rate)
        .map_err(|e| internal(format!("stored usd_rate invalid: {e}")))?;
    let pay_units = pay_minor_units(credit_nano_usd, rate, scale).map_err(internal)?;
    let pay_amount = format_minor_units(pay_units, scale);

    let now = Utc::now();
    let ttl = Duration::seconds(crate::recharge::order_ttl_secs() as i64);
    // RC-D3: 32 lowercase hex chars, doubling as the merchant order number.
    let order_id = uuid::Uuid::new_v4().simple().to_string();
    let order = RechargeOrder {
        id: order_id.clone(),
        user_id: user.id.clone(),
        payment_channel_id: channel.id.clone(),
        channel_type_id: channel.type_id.clone(),
        channel_name: channel.name.clone(),
        status: "pending".to_string(),
        credit_nano_usd,
        pay_currency: channel.currency.clone(),
        pay_amount,
        usd_rate: channel.usd_rate.clone(),
        provider_order_id: None,
        error_code: None,
        paid_at: None,
        expires_at: (now + ttl).to_rfc3339(),
        meta_json: Value::Object(Default::default()),
        created_at: now.to_rfc3339(),
        updated_at: now.to_rfc3339(),
        username: Some(user.username.clone()),
    };
    state
        .user_store
        .insert_recharge_order(&order)
        .await
        .map_err(internal)?;

    let urls = PaymentUrls::derive(&origin, &channel.id, &order_id);
    let initiation = match adapter.create_payment(&order, &channel.config, &urls).await {
        Ok(initiation) => initiation,
        Err(error) => {
            // RC-O8: the failed order row remains for audit.
            state
                .user_store
                .mark_order_failed(&order_id, "payment_init_failed")
                .await
                .map_err(internal)?;
            tracing::warn!(order_id, error, "recharge payment initiation failed");
            return Err(AppError::new(
                StatusCode::BAD_GATEWAY,
                "payment_init_failed",
                "payment provider rejected the order",
            ));
        }
    };
    // RC-T1: persist the provider id before the RC-O5 response returns.
    let mut order = order;
    if let Some(provider_order_id) = &initiation.provider_order_id {
        state
            .user_store
            .set_order_provider_id(&order_id, provider_order_id)
            .await
            .map_err(internal)?;
        order.provider_order_id = Some(provider_order_id.clone());
    }

    Ok(Json(json!({
        "order": order_json(&order, false),
        "payment": { "kind": "redirect", "url": initiation.url },
    })))
}

// ---------------------------------------------------------------------------
// §9.1 RC-A2..RC-A4: order reads
// ---------------------------------------------------------------------------

const ORDER_STATUSES: [&str; 5] = ["pending", "succeeded", "failed", "expired", "refunded"];

fn default_limit() -> u64 {
    20
}

#[derive(Debug, Deserialize)]
pub struct OrderListQuery {
    #[serde(default = "default_limit")]
    pub limit: u64,
    #[serde(default)]
    pub offset: u64,
    pub status: Option<String>,
    pub username: Option<String>,
}

pub async fn list_recharge_orders(
    State(state): State<AppState>,
    headers: HeaderMap,
    Query(query): Query<OrderListQuery>,
) -> AppResult<impl IntoResponse> {
    let user = get_current_user(&headers, &state).await?;
    let is_admin = user.role.can_manage_users();
    if let Some(status) = &query.status
        && !ORDER_STATUSES.contains(&status.as_str())
    {
        return Err(AppError::new(
            StatusCode::BAD_REQUEST,
            "invalid_request",
            "invalid status filter",
        ));
    }
    let filter = OrderListFilter {
        user_id: (!is_admin).then(|| user.id.clone()),
        status: query.status.clone(),
        // RC-A2: the username filter is ignored for role `user`.
        username: if is_admin { query.username.clone() } else { None },
        limit: query.limit.clamp(1, 100),
        offset: query.offset,
    };
    let (orders, total) = state
        .user_store
        .list_recharge_orders(&filter)
        .await
        .map_err(internal)?;
    let orders = orders
        .iter()
        .map(|order| order_json(order, is_admin))
        .collect::<Vec<_>>();
    Ok(Json(json!({ "orders": orders, "total": total })))
}

pub async fn get_recharge_order(
    State(state): State<AppState>,
    headers: HeaderMap,
    Path(order_id): Path<String>,
) -> AppResult<impl IntoResponse> {
    let user = get_current_user(&headers, &state).await?;
    let is_admin = user.role.can_manage_users();
    let not_found =
        || AppError::new(StatusCode::NOT_FOUND, "not_found", "recharge order not found");
    let order = state
        .user_store
        .get_recharge_order(&order_id)
        .await
        .map_err(internal)?
        .ok_or_else(not_found)?;
    // RC-A4: existence of another user's order is not disclosed.
    if !is_admin && order.user_id != user.id {
        return Err(not_found());
    }
    Ok(Json(json!({ "order": order_json(&order, is_admin) })))
}

// ---------------------------------------------------------------------------
// §9.1 RC-A5: billing-ledger read surface
// ---------------------------------------------------------------------------

#[derive(Debug, Deserialize)]
pub struct LedgerListQuery {
    #[serde(default = "default_limit")]
    pub limit: u64,
    #[serde(default)]
    pub offset: u64,
    pub kinds: Option<String>,
    pub username: Option<String>,
}

pub async fn list_ledger(
    State(state): State<AppState>,
    headers: HeaderMap,
    Query(query): Query<LedgerListQuery>,
) -> AppResult<impl IntoResponse> {
    let user = get_current_user(&headers, &state).await?;
    let is_admin = user.role.can_manage_users();
    let kinds = query
        .kinds
        .as_deref()
        .map(|raw| {
            raw.split(',')
                .map(str::trim)
                .filter(|entry| !entry.is_empty())
                .map(|entry| {
                    let valid = (1..=64).contains(&entry.len())
                        && entry.bytes().all(|byte| byte.is_ascii_lowercase() || byte == b'_');
                    if valid {
                        Ok(entry.to_string())
                    } else {
                        Err(AppError::new(
                            StatusCode::BAD_REQUEST,
                            "invalid_request",
                            "invalid kinds filter",
                        ))
                    }
                })
                .collect::<Result<Vec<_>, _>>()
        })
        .transpose()?;
    let filter = LedgerListFilter {
        user_id: (!is_admin).then(|| user.id.clone()),
        kinds,
        username: if is_admin { query.username.clone() } else { None },
        limit: query.limit.clamp(1, 100),
        offset: query.offset,
    };
    let (entries, total) = state
        .user_store
        .list_billing_ledger(&filter)
        .await
        .map_err(internal)?;
    let entries = entries
        .iter()
        .map(|entry| {
            let mut object = json!({
                "id": entry.id,
                "user_id": entry.user_id,
                "kind": entry.kind,
                "delta_nano_usd": entry.delta_nano_usd.to_string(),
                "delta_usd": format_nano_to_usd(entry.delta_nano_usd),
                "balance_after_nano_usd": entry.balance_after_nano_usd.map(|v| v.to_string()),
                "meta_json": entry.meta_json,
                "created_at": entry.created_at,
            });
            if is_admin
                && let Some(map) = object.as_object_mut()
            {
                map.insert("username".to_string(), entry.username.clone().into());
            }
            object
        })
        .collect::<Vec<_>>();
    Ok(Json(json!({ "entries": entries, "total": total })))
}

// ---------------------------------------------------------------------------
// §9.2 admin payment-channel CRUD
// ---------------------------------------------------------------------------

/// RC-P6: every admin read replaces each stored secret value with "".
fn masked_channel_json(channel: &RechargeChannel) -> Value {
    let mut config = channel.config.clone();
    if let Some(adapter) = adapter_for(&channel.type_id)
        && let Some(object) = config.as_object_mut()
    {
        for field in adapter.secret_fields() {
            if object.contains_key(*field) {
                object.insert((*field).to_string(), Value::String(String::new()));
            }
        }
    }
    json!({
        "id": channel.id,
        "name": channel.name,
        "type_id": channel.type_id,
        "enabled": channel.enabled,
        "currency": channel.currency,
        "usd_rate": channel.usd_rate,
        "min_credit_usd": channel.min_credit_usd,
        "max_credit_usd": channel.max_credit_usd,
        "config": config,
        "sort_order": channel.sort_order,
        "created_at": channel.created_at,
        "updated_at": channel.updated_at,
    })
}

pub async fn list_payment_channels(
    State(state): State<AppState>,
    headers: HeaderMap,
) -> AppResult<impl IntoResponse> {
    require_admin(&headers, &state).await?;
    let channels = state
        .user_store
        .list_payment_channels(false)
        .await
        .map_err(internal)?;
    let channels = channels.iter().map(masked_channel_json).collect::<Vec<_>>();
    Ok(Json(json!({ "channels": channels })))
}

#[derive(Debug, Deserialize)]
pub struct CreateChannelRequest {
    pub name: String,
    pub type_id: String,
    pub currency: String,
    pub usd_rate: String,
    pub min_credit_usd: Option<String>,
    pub max_credit_usd: Option<String>,
    pub enabled: Option<bool>,
    pub sort_order: Option<i32>,
    pub config: Value,
}

fn validate_channel_name(raw: &str) -> AppResult<String> {
    let trimmed = raw.trim();
    if trimmed.is_empty() || trimmed.chars().count() > 100 {
        return Err(AppError::new(
            StatusCode::BAD_REQUEST,
            "invalid_request",
            "name must be 1..100 characters after trimming",
        ));
    }
    Ok(trimmed.to_string())
}

/// RC-A6 rate validation: `usd_rate`, `min_credit_usd`, `max_credit_usd` are
/// RC-U5 decimals with `min <= max`, all mapping to `invalid_rate`.
fn validate_channel_rates(usd_rate: &str, min: &str, max: &str) -> AppResult<()> {
    let invalid = |message: &str| {
        AppError::new(StatusCode::BAD_REQUEST, "invalid_rate", message.to_string())
    };
    parse_positive_decimal(usd_rate).map_err(|_| invalid("malformed usd_rate"))?;
    let min_nano =
        parse_positive_usd_to_nano(min).map_err(|_| invalid("malformed min_credit_usd"))?;
    let max_nano =
        parse_positive_usd_to_nano(max).map_err(|_| invalid("malformed max_credit_usd"))?;
    if min_nano > max_nano {
        return Err(invalid("min_credit_usd exceeds max_credit_usd"));
    }
    Ok(())
}

pub async fn create_payment_channel(
    State(state): State<AppState>,
    headers: HeaderMap,
    Json(body): Json<CreateChannelRequest>,
) -> AppResult<impl IntoResponse> {
    require_admin(&headers, &state).await?;
    let name = validate_channel_name(&body.name)?;
    // RC-P1: unknown type_id.
    let adapter = adapter_for(&body.type_id).ok_or_else(|| {
        AppError::new(
            StatusCode::BAD_REQUEST,
            "invalid_channel_type",
            "unknown payment channel type",
        )
    })?;
    // RC-P4: currency constraint.
    adapter.currency_scale(&body.currency).map_err(|message| {
        AppError::new(StatusCode::BAD_REQUEST, "invalid_currency", message)
    })?;
    let min_credit_usd = body.min_credit_usd.unwrap_or_else(|| "1".to_string());
    let max_credit_usd = body.max_credit_usd.unwrap_or_else(|| "10000".to_string());
    validate_channel_rates(&body.usd_rate, &min_credit_usd, &max_credit_usd)?;
    // RC-P5/RC-P6: on create every secret field must be non-empty.
    adapter.validate_config(&body.config, true).map_err(|message| {
        AppError::new(StatusCode::BAD_REQUEST, "invalid_channel_config", message)
    })?;

    let now = Utc::now().to_rfc3339();
    let channel = RechargeChannel {
        id: uuid::Uuid::new_v4().to_string(),
        name,
        type_id: body.type_id,
        enabled: body.enabled.unwrap_or(true),
        currency: body.currency,
        usd_rate: body.usd_rate,
        min_credit_usd,
        max_credit_usd,
        config: body.config,
        sort_order: body.sort_order.unwrap_or(0),
        created_at: now.clone(),
        updated_at: now,
    };
    state
        .user_store
        .create_payment_channel(&channel)
        .await
        .map_err(map_channel_write_error)?;
    Ok(Json(json!({ "channel": masked_channel_json(&channel) })))
}

fn map_channel_write_error(error: ChannelWriteError) -> AppError {
    match error {
        ChannelWriteError::NameExists => AppError::new(
            StatusCode::CONFLICT,
            "channel_name_exists",
            "a channel with this name already exists",
        ),
        ChannelWriteError::Internal(message) => internal(message),
    }
}

#[derive(Debug, Deserialize)]
pub struct UpdateChannelRequest {
    pub name: Option<String>,
    pub type_id: Option<String>,
    pub currency: Option<String>,
    pub usd_rate: Option<String>,
    pub min_credit_usd: Option<String>,
    pub max_credit_usd: Option<String>,
    pub enabled: Option<bool>,
    pub sort_order: Option<i32>,
    pub config: Option<Value>,
}

pub async fn update_payment_channel(
    State(state): State<AppState>,
    headers: HeaderMap,
    Path(channel_id): Path<String>,
    Json(body): Json<UpdateChannelRequest>,
) -> AppResult<impl IntoResponse> {
    require_admin(&headers, &state).await?;
    let mut channel = state
        .user_store
        .get_payment_channel(&channel_id)
        .await
        .map_err(internal)?
        .ok_or_else(|| {
            AppError::new(StatusCode::NOT_FOUND, "not_found", "payment channel not found")
        })?;
    // RC-D1: type_id and currency are immutable after create.
    if body.type_id.as_ref().is_some_and(|v| *v != channel.type_id)
        || body.currency.as_ref().is_some_and(|v| *v != channel.currency)
    {
        return Err(AppError::new(
            StatusCode::BAD_REQUEST,
            "invalid_request",
            "type_id and currency are immutable",
        ));
    }
    let adapter = adapter_for(&channel.type_id).ok_or_else(|| {
        internal(format!("stored type_id {:?} not in registry", channel.type_id))
    })?;
    if let Some(name) = &body.name {
        channel.name = validate_channel_name(name)?;
    }
    if let Some(usd_rate) = body.usd_rate {
        channel.usd_rate = usd_rate;
    }
    if let Some(min) = body.min_credit_usd {
        channel.min_credit_usd = min;
    }
    if let Some(max) = body.max_credit_usd {
        channel.max_credit_usd = max;
    }
    validate_channel_rates(&channel.usd_rate, &channel.min_credit_usd, &channel.max_credit_usd)?;
    if let Some(enabled) = body.enabled {
        channel.enabled = enabled;
    }
    if let Some(sort_order) = body.sort_order {
        channel.sort_order = sort_order;
    }
    if let Some(mut config) = body.config {
        // RC-P6: an empty-string secret field keeps the stored value.
        if let Some(object) = config.as_object_mut() {
            for field in adapter.secret_fields() {
                let incoming_empty = object
                    .get(*field)
                    .is_none_or(|value| value.as_str() == Some(""));
                if incoming_empty
                    && let Some(stored) = channel.config.get(*field)
                {
                    object.insert((*field).to_string(), stored.clone());
                }
            }
        }
        adapter.validate_config(&config, true).map_err(|message| {
            AppError::new(StatusCode::BAD_REQUEST, "invalid_channel_config", message)
        })?;
        channel.config = config;
    }
    channel.updated_at = Utc::now().to_rfc3339();
    state
        .user_store
        .update_payment_channel(&channel)
        .await
        .map_err(map_channel_write_error)?;
    Ok(Json(json!({ "channel": masked_channel_json(&channel) })))
}

pub async fn delete_payment_channel(
    State(state): State<AppState>,
    headers: HeaderMap,
    Path(channel_id): Path<String>,
) -> AppResult<impl IntoResponse> {
    require_admin(&headers, &state).await?;
    let deleted = state
        .user_store
        .delete_payment_channel(&channel_id)
        .await
        .map_err(internal)?;
    if !deleted {
        return Err(AppError::new(
            StatusCode::NOT_FOUND,
            "not_found",
            "payment channel not found",
        ));
    }
    Ok(Json(json!({ "ok": true })))
}

// ---------------------------------------------------------------------------
// §8 refunds
// ---------------------------------------------------------------------------

#[derive(Debug, Deserialize, Default)]
pub struct RefundRequest {
    #[serde(default)]
    pub manual: bool,
}

pub async fn refund_recharge_order(
    State(state): State<AppState>,
    headers: HeaderMap,
    Path(order_id): Path<String>,
    body: Option<Json<RefundRequest>>,
) -> AppResult<impl IntoResponse> {
    let admin = require_admin(&headers, &state).await?;
    let manual_requested = body.map(|Json(body)| body.manual).unwrap_or(false);
    let invalid_state = || {
        AppError::new(
            StatusCode::CONFLICT,
            "invalid_order_state",
            "order is not in a refundable state",
        )
    };
    let order = state
        .user_store
        .get_recharge_order(&order_id)
        .await
        .map_err(internal)?
        .ok_or_else(|| {
            AppError::new(StatusCode::NOT_FOUND, "not_found", "recharge order not found")
        })?;
    // RC-R3 precheck; RC-R5 step 1 re-checks under the row lock.
    if order.status != "succeeded" {
        return Err(invalid_state());
    }
    let adapter = adapter_for(&order.channel_type_id).ok_or_else(|| {
        internal(format!(
            "stored channel_type_id {:?} not in registry",
            order.channel_type_id
        ))
    })?;
    let manual = if adapter.supports_refund() {
        // RC-R4: the provider must confirm before the RC-R5 transaction.
        let channel = state
            .user_store
            .get_payment_channel(&order.payment_channel_id)
            .await
            .map_err(internal)?;
        let config = channel.map(|c| c.config).ok_or_else(|| {
            AppError::new(
                StatusCode::BAD_GATEWAY,
                "refund_failed",
                "payment channel no longer exists; provider refund is impossible",
            )
        })?;
        adapter.refund(&order, &config).await.map_err(|error| {
            tracing::warn!(order_id, error, "provider refund failed");
            AppError::new(
                StatusCode::BAD_GATEWAY,
                "refund_failed",
                "payment provider refund failed",
            )
        })?;
        false
    } else {
        // RC-R4: the admin must assert the out-of-band refund.
        if !manual_requested {
            return Err(AppError::new(
                StatusCode::BAD_REQUEST,
                "manual_refund_required",
                "this channel requires manual: true acknowledging an out-of-band refund",
            ));
        }
        true
    };
    let refunded = state
        .user_store
        .refund_recharge_order(&order_id, &admin.id, manual)
        .await
        .map_err(internal)?;
    if !refunded {
        return Err(invalid_state());
    }
    let order = state
        .user_store
        .get_recharge_order(&order_id)
        .await
        .map_err(internal)?
        .ok_or_else(|| internal("refunded order disappeared"))?;
    Ok(Json(json!({ "order": order_json(&order, true) })))
}

// ---------------------------------------------------------------------------
// §6 notify webhook
// ---------------------------------------------------------------------------

fn ack_response(ack: crate::recharge::AckResponse) -> Response {
    (
        ack.status,
        [(axum::http::header::CONTENT_TYPE, ack.content_type)],
        ack.body,
    )
        .into_response()
}

pub async fn pay_notify(
    State(state): State<AppState>,
    Path(payment_channel_id): Path<String>,
    request: axum::extract::Request,
) -> Response {
    // RC-N2: unknown channel returns 404 before the body is read.
    let channel = match state.user_store.get_payment_channel(&payment_channel_id).await {
        Ok(Some(channel)) => channel,
        Ok(None) => return StatusCode::NOT_FOUND.into_response(),
        Err(error) => {
            tracing::error!(payment_channel_id, error, "notify channel lookup failed");
            return StatusCode::INTERNAL_SERVER_ERROR.into_response();
        }
    };
    let Some(adapter) = adapter_for(&channel.type_id) else {
        tracing::error!(
            payment_channel_id,
            type_id = channel.type_id,
            "stored type_id not in registry"
        );
        return StatusCode::INTERNAL_SERVER_ERROR.into_response();
    };

    let method = request.method().clone();
    let headers = request.headers().clone();
    let query = request.uri().query().unwrap_or("").to_string();
    let raw_body = match axum::body::to_bytes(request.into_body(), 1024 * 1024).await {
        Ok(bytes) => bytes,
        Err(_) => return StatusCode::BAD_REQUEST.into_response(),
    };

    // RC-N3: verification precedes every order read.
    let verification =
        match adapter.verify_notification(&method, &headers, &raw_body, &query, &channel.config) {
            Ok(verification) => verification,
            Err(_) => return ack_response(adapter.ack(AckOutcome::SignatureError)),
        };
    let verified = match verification {
        Verification::Verified(verified) => verified,
        // RC-T3: out-of-scope events are acknowledged without state access.
        Verification::Ignored => return ack_response(adapter.ack(AckOutcome::Duplicate)),
    };

    match state
        .user_store
        .apply_verified_notification(&channel.id, &verified)
        .await
    {
        Ok(outcome) => {
            let ack = match outcome {
                NotifyOutcome::Credited => AckOutcome::Credited,
                NotifyOutcome::Duplicate => AckOutcome::Duplicate,
                NotifyOutcome::FailedRecorded => AckOutcome::FailedRecorded,
                NotifyOutcome::UnknownOrder => AckOutcome::UnknownOrder,
            };
            ack_response(adapter.ack(ack))
        }
        // RC-N10: storage errors return 500 with an empty body so the
        // provider retries and the credit is not silently lost.
        Err(error) => {
            tracing::error!(
                payment_channel_id,
                order_id = verified.order_id,
                error,
                "notify credit transaction failed"
            );
            StatusCode::INTERNAL_SERVER_ERROR.into_response()
        }
    }
}
