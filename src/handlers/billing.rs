use super::*;
use rust_decimal::Decimal;
use serde_json::{Value, json};
use std::str::FromStr;

#[derive(Debug, Clone, Default)]
pub(super) struct ChargeComputation {
    pub(super) charge_nano_usd: Option<i128>,
    pub(super) billing_breakdown: Option<Value>,
}

#[derive(Debug, Clone)]
struct TokenPriceSet {
    input: String,
    output: String,
    cache_read: String,
    cache_write: String,
    cache_write_1h: String,
    reasoning: String,
}

#[derive(Debug, Clone)]
struct TokenQuantities {
    input_uncached: u64,
    cache_read: u64,
    cache_write: u64,
    cache_write_1h: u64,
    output: u64,
    reasoning: u64,
}

#[derive(Debug, Clone)]
struct ChargeParts {
    billing_mode: Option<String>,
    applied_tier_index: Option<usize>,
    token_line_items: Vec<Value>,
    tool_line_items: Vec<Value>,
    unpriced_tool_classes: Vec<String>,
    token_charge: i128,
    tool_charge: i128,
    base_charge: i128,
    final_charge: i128,
    free_reason: Option<&'static str>,
}

fn decimal(raw: &str) -> Result<Decimal, String> {
    Decimal::from_str(raw).map_err(|_| format!("invalid persisted decimal `{raw}`"))
}

fn checked_decimal_product_to_i128(
    quantity: u64,
    raw: &str,
    integer_factor: i128,
) -> Result<i128, String> {
    let value = decimal(raw)?;
    let numerator = i128::from(quantity)
        .checked_mul(value.mantissa())
        .and_then(|value| value.checked_mul(integer_factor))
        .ok_or_else(|| "billing arithmetic overflow".to_string())?;
    let divisor = 10_i128
        .checked_pow(value.scale())
        .ok_or_else(|| "billing arithmetic overflow".to_string())?;
    Ok(numerator / divisor)
}

fn checked_usd_to_nano(raw: &str) -> Result<i128, String> {
    checked_decimal_product_to_i128(1, raw, 1_000_000_000)
}

fn checked_scale_with_two_decimals(
    base: i128,
    first: &str,
    second: &str,
) -> Result<i128, String> {
    let first = decimal(first)?;
    let second = decimal(second)?;
    let coefficient = first
        .mantissa()
        .checked_mul(second.mantissa())
        .ok_or_else(|| "billing multiplier overflow".to_string())?;
    let scale = first
        .scale()
        .checked_add(second.scale())
        .ok_or_else(|| "billing multiplier overflow".to_string())?;
    let divisor = 10_i128
        .checked_pow(scale)
        .ok_or_else(|| "billing multiplier overflow".to_string())?;
    let whole = base / divisor;
    let remainder = base % divisor;
    whole
        .checked_mul(coefficient)
        .and_then(|value| {
            remainder
                .checked_mul(coefficient)
                .map(|remainder| value + remainder / divisor)
        })
        .ok_or_else(|| "billing multiplier overflow".to_string())
}

fn token_quantities(usage: &urp::Usage) -> TokenQuantities {
    let input = usage.input_details.as_ref();
    let output = usage.output_details.as_ref();
    let cache_read = input.map(|details| details.cache_read_tokens).unwrap_or(0);
    let aggregate_write = input
        .map(|details| details.cache_creation_tokens)
        .unwrap_or(0);
    let cache_write_5m = input
        .map(|details| details.cache_creation_5m_tokens)
        .unwrap_or(0);
    let cache_write_1h = input
        .map(|details| details.cache_creation_1h_tokens)
        .unwrap_or(0);
    let unsplit_write = aggregate_write
        .saturating_sub(cache_write_5m.saturating_add(cache_write_1h));
    let reasoning = output.map(|details| details.reasoning_tokens).unwrap_or(0);
    TokenQuantities {
        input_uncached: usage
            .input_tokens
            .saturating_sub(cache_read)
            .saturating_sub(aggregate_write),
        cache_read,
        cache_write: cache_write_5m.saturating_add(unsplit_write),
        cache_write_1h,
        output: usage.output_tokens.saturating_sub(reasoning),
        reasoning,
    }
}

fn resolved_prices_from_fields(
    input: &str,
    output: Option<&str>,
    cache_read: Option<&str>,
    cache_write: Option<&str>,
    cache_write_1h: Option<&str>,
    reasoning: Option<&str>,
) -> TokenPriceSet {
    let output = output.unwrap_or(input);
    let cache_write = cache_write.unwrap_or(input);
    TokenPriceSet {
        input: input.to_string(),
        output: output.to_string(),
        cache_read: cache_read.unwrap_or(input).to_string(),
        cache_write: cache_write.to_string(),
        cache_write_1h: cache_write_1h.unwrap_or(cache_write).to_string(),
        reasoning: reasoning.unwrap_or(output).to_string(),
    }
}

fn row_prices(row: &crate::model_price_store::ModelPriceRecord) -> Result<TokenPriceSet, String> {
    let input = row
        .input_usd_per_1m
        .as_deref()
        .ok_or_else(|| "complete per-token row has no input price".to_string())?;
    Ok(resolved_prices_from_fields(
        input,
        row.output_usd_per_1m.as_deref(),
        row.cache_read_usd_per_1m.as_deref(),
        row.cache_write_usd_per_1m.as_deref(),
        row.cache_write_1h_usd_per_1m.as_deref(),
        row.reasoning_usd_per_1m.as_deref(),
    ))
}

fn tier_prices(
    row: &crate::model_price_store::ModelPriceRecord,
    input_tokens: u64,
) -> Result<(usize, TokenPriceSet), String> {
    let tiers = row
        .billing_expr
        .as_ref()
        .and_then(|expr| expr.get("tiers"))
        .and_then(Value::as_array)
        .ok_or_else(|| "complete tiered row has no tiers".to_string())?;
    let index = tiers
        .iter()
        .position(|tier| {
            tier.get("when_input_tokens_lte")
                .and_then(Value::as_u64)
                .is_none_or(|limit| input_tokens <= limit)
        })
        .unwrap_or(tiers.len() - 1);
    let tier = tiers[index]
        .as_object()
        .ok_or_else(|| "persisted billing tier is not an object".to_string())?;
    let field = |name: &str| tier.get(name).and_then(Value::as_str);
    let input = field("input_usd_per_1m")
        .ok_or_else(|| "persisted billing tier has no input price".to_string())?;
    Ok((
        index,
        resolved_prices_from_fields(
            input,
            field("output_usd_per_1m"),
            field("cache_read_usd_per_1m"),
            field("cache_write_usd_per_1m"),
            field("cache_write_1h_usd_per_1m"),
            field("reasoning_usd_per_1m"),
        ),
    ))
}

fn add_token_line(
    lines: &mut Vec<Value>,
    usage_class: &str,
    quantity: u64,
    price: Option<&str>,
    charge_enabled: bool,
) -> Result<i128, String> {
    if quantity == 0 {
        return Ok(0);
    }
    let charge = match (price, charge_enabled) {
        (Some(price), true) => checked_decimal_product_to_i128(quantity, price, 1000)?,
        _ => 0,
    };
    lines.push(json!({
        "usage_class": usage_class,
        "quantity": quantity,
        "usd_per_1m": price,
        "charge_nano": charge.to_string(),
    }));
    Ok(charge)
}

fn token_lines(
    usage: &urp::Usage,
    prices: Option<&TokenPriceSet>,
    charge_enabled: bool,
) -> Result<(Vec<Value>, i128), String> {
    let quantities = token_quantities(usage);
    let mut lines = Vec::new();
    let entries = [
        ("input_uncached", quantities.input_uncached, prices.map(|p| p.input.as_str())),
        ("cache_read", quantities.cache_read, prices.map(|p| p.cache_read.as_str())),
        ("cache_write", quantities.cache_write, prices.map(|p| p.cache_write.as_str())),
        ("cache_write_1h", quantities.cache_write_1h, prices.map(|p| p.cache_write_1h.as_str())),
        ("output", quantities.output, prices.map(|p| p.output.as_str())),
        ("reasoning_output", quantities.reasoning, prices.map(|p| p.reasoning.as_str())),
    ];
    let mut total = 0i128;
    for (usage_class, quantity, price) in entries {
        total = total
            .checked_add(add_token_line(
                &mut lines,
                usage_class,
                quantity,
                price,
                charge_enabled,
            )?)
            .ok_or_else(|| "token charge overflow".to_string())?;
    }
    Ok((lines, total))
}

pub(super) fn parse_u64_value(value: &Value) -> Option<u64> {
    value
        .as_u64()
        .or_else(|| value.as_i64().and_then(|value| u64::try_from(value).ok()))
        .or_else(|| value.as_str().and_then(|value| value.parse().ok()))
}

fn map_get_u64(map: &serde_json::Map<String, Value>, key: &str) -> Option<u64> {
    map.get(key).and_then(parse_u64_value)
}

pub(super) fn build_usage_breakdown(usage: &urp::Usage) -> Value {
    let input_details = usage.input_details.as_ref();
    let output_details = usage.output_details.as_ref();
    let input_cached = input_details
        .map(|details| details.cache_read_tokens)
        .filter(|quantity| *quantity > 0)
        .or_else(|| usage.extra_body.get("cache_read_input_tokens").and_then(parse_u64_value))
        .or_else(|| {
            usage
                .extra_body
                .get("input_tokens_details")
                .and_then(Value::as_object)
                .and_then(|details| map_get_u64(details, "cached_tokens"))
        })
        .or_else(|| {
            usage
                .extra_body
                .get("prompt_tokens_details")
                .and_then(Value::as_object)
                .and_then(|details| map_get_u64(details, "cached_tokens"))
        });
    let cache_creation = input_details
        .map(|details| details.cache_creation_tokens)
        .filter(|quantity| *quantity > 0)
        .or_else(|| {
            usage
                .extra_body
                .get("cache_creation_input_tokens")
                .and_then(parse_u64_value)
        });
    let reasoning = output_details
        .map(|details| details.reasoning_tokens)
        .filter(|quantity| *quantity > 0)
        .or_else(|| {
            usage
                .extra_body
                .get("output_tokens_details")
                .and_then(Value::as_object)
                .and_then(|details| map_get_u64(details, "reasoning_tokens"))
        })
        .or_else(|| {
            usage
                .extra_body
                .get("completion_tokens_details")
                .and_then(Value::as_object)
                .and_then(|details| map_get_u64(details, "reasoning_tokens"))
        });
    json!({
        "version": 1,
        "input": {
            "total_tokens": usage.input_tokens,
            "uncached_tokens": usage.input_tokens
                .saturating_sub(input_cached.unwrap_or(0))
                .saturating_sub(cache_creation.unwrap_or(0)),
            "cached_tokens": input_cached,
            "cache_creation_tokens": cache_creation,
            "cache_creation_5m_tokens": input_details.map(|d| d.cache_creation_5m_tokens).filter(|v| *v > 0),
            "cache_creation_1h_tokens": input_details.map(|d| d.cache_creation_1h_tokens).filter(|v| *v > 0),
        },
        "output": {
            "total_tokens": usage.output_tokens,
            "non_reasoning_tokens": usage.output_tokens.saturating_sub(reasoning.unwrap_or(0)),
            "reasoning_tokens": reasoning,
        },
        "raw_usage_extra": usage.extra_body,
    })
}

fn authoritative_tool_quantity(usage: &urp::Usage, usage_class: &str) -> Option<u64> {
    let direct_keys = [
        usage_class.to_string(),
        format!("{usage_class}_requests"),
        format!("{usage_class}_calls"),
        format!("{usage_class}_billed_minutes"),
        format!("{usage_class}_minutes"),
        format!("{usage_class}_sessions"),
    ];
    for key in direct_keys {
        if let Some(quantity) = usage.extra_body.get(&key).and_then(parse_u64_value) {
            return Some(quantity);
        }
    }
    for container in ["server_tool_use", "server_side_tool_usage"] {
        let Some(object) = usage.extra_body.get(container).and_then(Value::as_object) else {
            continue;
        };
        for key in [
            usage_class.to_string(),
            format!("{usage_class}_requests"),
            format!("{usage_class}_calls"),
            format!("{usage_class}_billed_minutes"),
            format!("{usage_class}_sessions"),
        ] {
            if let Some(quantity) = object.get(&key).and_then(parse_u64_value) {
                return Some(quantity);
            }
        }
    }
    None
}

fn decoded_provider_item_count(output: Option<&[urp::Node]>, usage_class: &str) -> u64 {
    output
        .unwrap_or_default()
        .iter()
        .filter(|node| match node {
            urp::Node::ProviderItem { item_type, .. } => match usage_class {
                "web_search" => item_type.contains("web_search"),
                "file_search_tool_call" => item_type.contains("file_search"),
                "x_search" => item_type.contains("x_search"),
                "code_execution"
                | "code_execution_duration"
                | "code_interpreter_duration"
                | "code_interpreter_session" => item_type.contains("code"),
                _ => item_type.contains(usage_class),
            },
            _ => false,
        })
        .count() as u64
}

fn tool_price_entry(value: &Value) -> Result<(String, String, Option<u64>), String> {
    match value {
        Value::String(raw) => Ok((raw.clone(), "1k_calls".to_string(), None)),
        Value::Number(raw) => Ok((raw.to_string(), "1k_calls".to_string(), None)),
        Value::Object(object) => Ok((
            object
                .get("usd")
                .and_then(|value| match value {
                    Value::String(raw) => Some(raw.clone()),
                    Value::Number(raw) => Some(raw.to_string()),
                    _ => None,
                })
                .ok_or_else(|| "tool price object has no usd decimal".to_string())?,
            object
                .get("per")
                .and_then(Value::as_str)
                .ok_or_else(|| "tool price object has no per unit".to_string())?
                .to_string(),
            object.get("minimum_units").and_then(parse_u64_value),
        )),
        _ => Err("invalid persisted tool price".to_string()),
    }
}

fn tool_charges(
    attempt: &MonoizeAttempt,
    usage: &urp::Usage,
    output: Option<&[urp::Node]>,
) -> Result<(Vec<Value>, Vec<String>, i128), String> {
    let settings = attempt
        .tool_prices
        .as_object()
        .ok_or_else(|| "persisted tool_prices is not an object".to_string())?;
    let mut lines = Vec::new();
    let mut unpriced = Vec::new();
    let mut total = 0i128;
    for usage_class in &attempt.server_tool_usage_classes {
        let authoritative = authoritative_tool_quantity(usage, usage_class);
        let decoded = decoded_provider_item_count(output, usage_class);
        if !authoritative.is_some_and(|quantity| quantity > 0) && decoded == 0 {
            continue;
        }
        let Some(entry) = settings.get(usage_class) else {
            unpriced.push(usage_class.clone());
            continue;
        };
        let (usd, per, minimum_units) = tool_price_entry(entry)?;
        let mut quantity = match per.as_str() {
            "1k_calls" => authoritative.unwrap_or(decoded),
            "minute" | "session" => match authoritative {
                Some(quantity) if quantity > 0 => quantity,
                _ => {
                    unpriced.push(usage_class.clone());
                    continue;
                }
            },
            _ => return Err(format!("invalid tool price unit `{per}`")),
        };
        if matches!(per.as_str(), "minute" | "session")
            && let Some(minimum) = minimum_units
        {
            quantity = quantity.max(minimum);
        }
        let charge = match per.as_str() {
            "1k_calls" => checked_decimal_product_to_i128(quantity, &usd, 1_000_000)?,
            "minute" | "session" => {
                checked_decimal_product_to_i128(quantity, &usd, 1_000_000_000)?
            }
            _ => unreachable!(),
        };
        total = total
            .checked_add(charge)
            .ok_or_else(|| "tool charge overflow".to_string())?;
        lines.push(json!({
            "usage_class": usage_class,
            "quantity": quantity,
            "per": per,
            "usd": usd,
            "charge_nano": charge.to_string(),
        }));
    }
    Ok((lines, unpriced, total))
}

fn compute_charge(
    attempt: &MonoizeAttempt,
    usage: &urp::Usage,
    output: Option<&[urp::Node]>,
    missing_usage_substituted: bool,
) -> Result<ChargeParts, String> {
    let free_reason = if attempt.model_price.is_none() {
        Some("unpriced")
    } else if missing_usage_substituted {
        Some("missing_usage")
    } else {
        None
    };
    let mut applied_tier_index = None;
    let mut prices = None;
    let mut per_request_charge = None;
    let billing_mode = attempt
        .model_price
        .as_ref()
        .map(|row| row.billing_mode.clone());
    if free_reason.is_none()
        && let Some(row) = attempt.model_price.as_ref()
    {
        match row.billing_mode.as_str() {
            "per_token" => prices = Some(row_prices(row)?),
            "per_request" => {
                per_request_charge = Some(checked_usd_to_nano(
                    row.per_request_usd
                        .as_deref()
                        .ok_or_else(|| "complete per-request row has no price".to_string())?,
                )?);
            }
            "tiered_expr" => {
                let (index, selected) = tier_prices(row, usage.input_tokens)?;
                applied_tier_index = Some(index);
                prices = Some(selected);
            }
            mode => return Err(format!("invalid persisted billing mode `{mode}`")),
        }
    }
    let token_charge_enabled = free_reason.is_none() && per_request_charge.is_none();
    let (token_line_items, computed_token_charge) =
        token_lines(usage, prices.as_ref(), token_charge_enabled)?;
    let token_charge = per_request_charge.unwrap_or(computed_token_charge);
    let (tool_line_items, unpriced_tool_classes, tool_charge) =
        tool_charges(attempt, usage, output)?;
    let base_charge = token_charge
        .checked_add(tool_charge)
        .ok_or_else(|| "billing charge overflow".to_string())?;
    let final_charge = checked_scale_with_two_decimals(
        base_charge,
        &attempt.model_multiplier.canonical(),
        &attempt.group_billing_ratio,
    )?;
    Ok(ChargeParts {
        billing_mode,
        applied_tier_index,
        token_line_items,
        tool_line_items,
        unpriced_tool_classes,
        token_charge,
        tool_charge,
        base_charge,
        final_charge,
        free_reason,
    })
}

fn build_breakdown(
    attempt: &MonoizeAttempt,
    parts: &ChargeParts,
    response_service_tier: Option<&str>,
) -> Value {
    json!({
        "version": 3,
        "billing_mode": parts.billing_mode,
        "pricing_model_key": attempt.pricing_model_key,
        "price_row_model_id": attempt.model_price.as_ref().map(|row| row.model_id.as_str()),
        "applied_tier_index": parts.applied_tier_index,
        "token_line_items": parts.token_line_items,
        "tool_line_items": parts.tool_line_items,
        "unpriced_tool_classes": parts.unpriced_tool_classes,
        "service_tier": response_service_tier,
        "billing_group_id": attempt.billing_group_id,
        "group_billing_ratio": attempt.group_billing_ratio,
        "channel_multiplier": attempt.model_multiplier.canonical(),
        "token_charge_nano": parts.token_charge.to_string(),
        "tool_charge_nano": parts.tool_charge.to_string(),
        "base_charge_nano": parts.base_charge.to_string(),
        "final_charge_nano": parts.final_charge.to_string(),
        "free_reason": parts.free_reason,
        "estimated": false,
    })
}

pub(crate) fn calculate_active_probe_charge(
    row: &crate::model_price_store::ModelPriceRecord,
    pricing_model_key: &str,
    usage: &urp::Usage,
    channel_multiplier: Multiplier,
) -> Result<(i128, Value), String> {
    let (prices, applied_tier_index, per_request_charge) = match row.billing_mode.as_str() {
        "per_token" => (Some(row_prices(row)?), None, None),
        "per_request" => (
            None,
            None,
            Some(checked_usd_to_nano(
                row.per_request_usd
                    .as_deref()
                    .ok_or_else(|| "complete per-request row has no price".to_string())?,
            )?),
        ),
        "tiered_expr" => {
            let (index, prices) = tier_prices(row, usage.input_tokens)?;
            (Some(prices), Some(index), None)
        }
        mode => return Err(format!("invalid persisted billing mode `{mode}`")),
    };
    let (token_line_items, computed_charge) =
        token_lines(usage, prices.as_ref(), per_request_charge.is_none())?;
    let base_charge = per_request_charge.unwrap_or(computed_charge);
    let final_charge = checked_scale_with_two_decimals(
        base_charge,
        &channel_multiplier.canonical(),
        "1",
    )?;
    let breakdown = json!({
        "version": 3,
        "billing_mode": row.billing_mode,
        "pricing_model_key": pricing_model_key,
        "price_row_model_id": row.model_id,
        "applied_tier_index": applied_tier_index,
        "token_line_items": token_line_items,
        "tool_line_items": [],
        "unpriced_tool_classes": [],
        "service_tier": null,
        "billing_group_id": null,
        "group_billing_ratio": "1",
        "channel_multiplier": channel_multiplier.canonical(),
        "token_charge_nano": base_charge.to_string(),
        "tool_charge_nano": "0",
        "base_charge_nano": base_charge.to_string(),
        "final_charge_nano": final_charge.to_string(),
        "free_reason": null,
        "estimated": false,
    });
    Ok((final_charge, breakdown))
}

async fn settle_charge(
    state: &AppState,
    auth: &crate::auth::AuthResult,
    attempt: &MonoizeAttempt,
    logical_model: &str,
    usage: &urp::Usage,
    output: Option<&[urp::Node]>,
    missing_usage_substituted: bool,
    response_service_tier: Option<&str>,
    request_id: Option<&str>,
) -> AppResult<ChargeComputation> {
    if attempt.model_price.is_none() && !attempt.allow_free_when_unpriced {
        return Err(AppError::new(
            StatusCode::FORBIDDEN,
            "model_pricing_required",
            format!("pricing metadata required for model: {}", attempt.upstream_model),
        ));
    }
    if missing_usage_substituted && !attempt.allow_free_when_missing_usage {
        return Err(AppError::new(
            StatusCode::FORBIDDEN,
            "usage_required",
            "upstream response did not include billable usage",
        ));
    }
    let Some(user_id) = auth.user_id.as_deref() else {
        return Ok(ChargeComputation::default());
    };
    let parts = compute_charge(attempt, usage, output, missing_usage_substituted).map_err(|error| {
        tracing::error!(model = %attempt.upstream_model, %error, "billing calculation failed");
        AppError::new(
            StatusCode::INTERNAL_SERVER_ERROR,
            "billing_overflow",
            error,
        )
    })?;
    let breakdown = build_breakdown(attempt, &parts, response_service_tier);
    let charge_nano = parts.final_charge;
    if charge_nano <= 0 {
        return Ok(ChargeComputation {
            charge_nano_usd: Some(0),
            billing_breakdown: Some(breakdown),
        });
    }
    let meta = json!({
        "logical_model": logical_model,
        "upstream_model": attempt.upstream_model,
        "pricing_model_key": attempt.pricing_model_key,
        "provider_id": attempt.provider_id,
        "channel_multiplier": attempt.model_multiplier.canonical(),
        "billing_group_id": attempt.billing_group_id,
        "group_billing_ratio": attempt.group_billing_ratio,
        "prompt_tokens": usage.input_tokens,
        "completion_tokens": usage.output_tokens,
        "charge_nano_usd": charge_nano.to_string(),
        "api_key_id": auth.api_key_id,
        "request_id": request_id,
    });

    if state.node.is_replica() {
        let (kind, api_key_id) = if auth.sub_account_enabled {
            ("api_key_charge", auth.api_key_id.as_deref())
        } else {
            ("request_charge", None)
        };
        crate::replica::metering::ReplicaMetering::enqueue_balance_delta_for_request(
            state,
            kind,
            user_id,
            api_key_id,
            charge_nano,
            &meta,
        )
        .await
        .map_err(|error| {
            AppError::new(
                StatusCode::INTERNAL_SERVER_ERROR,
                "metering_enqueue_failed",
                error,
            )
        })?;
    } else if auth.sub_account_enabled {
        let api_key_id = auth.api_key_id.as_deref().unwrap_or("");
        state
            .user_store
            .charge_sub_account_balance_nano(api_key_id, user_id, charge_nano, &meta)
            .await
            .map_err(map_settlement_error)?;
    } else {
        state
            .user_store
            .charge_user_balance_nano(user_id, charge_nano, &meta)
            .await
            .map_err(map_settlement_error)?;
    }
    Ok(ChargeComputation {
        charge_nano_usd: Some(charge_nano),
        billing_breakdown: Some(breakdown),
    })
}

fn map_settlement_error(error: crate::users::BillingError) -> AppError {
    match error.kind {
        BillingErrorKind::InsufficientBalance => AppError::new(
            StatusCode::PAYMENT_REQUIRED,
            "insufficient_balance",
            "insufficient balance",
        ),
        BillingErrorKind::NotFound => AppError::new(
            StatusCode::UNAUTHORIZED,
            "unauthorized",
            "billing subject not found",
        ),
        _ => AppError::new(
            StatusCode::INTERNAL_SERVER_ERROR,
            "internal_error",
            error.message,
        ),
    }
}

pub(super) async fn maybe_charge_usage(
    state: &AppState,
    auth: &crate::auth::AuthResult,
    attempt: &MonoizeAttempt,
    logical_model: &str,
    usage: &urp::Usage,
    missing_usage_substituted: bool,
    response_service_tier: Option<&str>,
    request_id: Option<&str>,
) -> AppResult<ChargeComputation> {
    settle_charge(
        state,
        auth,
        attempt,
        logical_model,
        usage,
        None,
        missing_usage_substituted,
        response_service_tier,
        request_id,
    )
    .await
}

pub(super) async fn maybe_charge_stream_usage(
    state: &AppState,
    auth: &crate::auth::AuthResult,
    attempt: &MonoizeAttempt,
    logical_model: &str,
    usage: &urp::Usage,
    missing_usage_substituted: bool,
    output: &[urp::Node],
    response_service_tier: Option<&str>,
    request_id: Option<&str>,
) -> AppResult<ChargeComputation> {
    settle_charge(
        state,
        auth,
        attempt,
        logical_model,
        usage,
        Some(output),
        missing_usage_substituted,
        response_service_tier,
        request_id,
    )
    .await
}

pub(super) async fn maybe_charge_response(
    state: &AppState,
    auth: &crate::auth::AuthResult,
    attempt: &MonoizeAttempt,
    logical_model: &str,
    response: &urp::UrpResponse,
    missing_usage_substituted: bool,
    request_id: Option<&str>,
) -> AppResult<ChargeComputation> {
    let fallback;
    let usage = match response.usage.as_ref() {
        Some(usage) => usage,
        None => {
            fallback = urp::Usage::default();
            &fallback
        }
    };
    let response_service_tier = response
        .extra_body
        .get("service_tier")
        .and_then(Value::as_str)
        .map(str::trim)
        .filter(|tier| !tier.is_empty());
    settle_charge(
        state,
        auth,
        attempt,
        logical_model,
        usage,
        Some(response.output.as_slice()),
        missing_usage_substituted || response.usage.is_none(),
        response_service_tier,
        request_id,
    )
    .await
}

/// Keep response pipelines deterministic: missing usage is represented by a
/// zero Usage object, while settlement applies MP-F3 using the returned flag.
pub(super) fn substitute_zero_usage_if_allowed(
    usage: &mut Option<urp::Usage>,
    _attempt: &MonoizeAttempt,
) -> bool {
    if usage.is_none() {
        *usage = Some(urp::Usage::default());
        return true;
    }
    false
}

#[cfg(test)]
mod tests {
    use super::{checked_decimal_product_to_i128, checked_scale_with_two_decimals};

    #[test]
    fn exact_line_item_conversion_truncates_toward_zero() {
        assert_eq!(checked_decimal_product_to_i128(3, "0.000000001", 1000).unwrap(), 0);
        assert_eq!(checked_decimal_product_to_i128(2, "2.5", 1000).unwrap(), 5000);
    }

    #[test]
    fn final_scaling_truncates_once_after_both_ratios() {
        assert_eq!(checked_scale_with_two_decimals(5, "0.5", "0.5").unwrap(), 1);
    }
}
