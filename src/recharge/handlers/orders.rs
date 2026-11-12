use super::*;

fn order_credit_amount(body: &CreateOrderRequest, channel: &RechargeChannel) -> AppResult<i128> {
    // RC-O3 step 4 / RC-O2: nano wins when both amounts are present.
    let invalid_amount = || {
        AppError::new(
            StatusCode::BAD_REQUEST,
            "invalid_amount",
            "invalid recharge amount",
        )
    };
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
    Ok(credit_nano_usd)
}

fn pending_recharge_order(
    channel: &RechargeChannel,
    user_id: &str,
    username: &str,
    credit_nano_usd: i128,
    pay_amount: String,
) -> RechargeOrder {
    let now = Utc::now();
    let ttl = Duration::seconds(crate::recharge::order_ttl_secs() as i64);
    // RC-D3: 32 lowercase hex chars, doubling as the merchant order number.
    let order_id = uuid::Uuid::new_v4().simple().to_string();
    RechargeOrder {
        id: order_id,
        user_id: user_id.to_string(),
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
        username: Some(username.to_string()),
    }
}

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
            AppError::new(
                StatusCode::NOT_FOUND,
                "not_found",
                "payment channel not found",
            )
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
    let credit_nano_usd = order_credit_amount(&body, &channel)?;
    let adapter = adapter_for(&channel.type_id).ok_or_else(|| {
        internal(format!(
            "stored type_id {:?} not in registry",
            channel.type_id
        ))
    })?;
    let scale = adapter
        .currency_scale(&channel.currency)
        .map_err(internal)?;
    // RC-U6: freeze usd_rate and pay_amount into the order row.
    let rate = parse_positive_decimal(&channel.usd_rate)
        .map_err(|e| internal(format!("stored usd_rate invalid: {e}")))?;
    let pay_units = pay_minor_units(credit_nano_usd, rate, scale).map_err(internal)?;
    let pay_amount = format_minor_units(pay_units, scale);

    let order = pending_recharge_order(
        &channel,
        &user.id,
        &user.username,
        credit_nano_usd,
        pay_amount,
    );
    let order_id = order.id.clone();
    let inserted = state
        .user_store
        .insert_recharge_order_under_limit(&order, crate::recharge::max_pending_orders() as i64)
        .await
        .map_err(internal)?;
    if !inserted {
        return Err(AppError::new(
            StatusCode::TOO_MANY_REQUESTS,
            "too_many_pending_orders",
            "too many pending recharge orders",
        ));
    }

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
