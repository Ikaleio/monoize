//! USD settlement engine over `model_prices` (`model-pricing.spec.md` §4–§8).
//!
//! This module is pure: it converts a resolved price snapshot, normalized
//! usage, tool prices, and multipliers into a nano-USD charge plus a
//! version-3 billing breakdown. Persistence and balance mutation stay in the
//! request handlers and the active-probe scheduler.

use crate::exact_decimal::Multiplier;
use crate::model_price_store::ModelPriceRecord;
use crate::urp;
use rust_decimal::Decimal;
use rust_decimal::prelude::ToPrimitive;
use serde_json::{Map, Value, json};

/// How the settled usage was obtained (MP-F3, MP-B5).
#[derive(Debug, Clone, Copy)]
pub enum SettledUsage<'a> {
    /// Normalized upstream usage was present.
    Reported(&'a urp::Usage),
    /// Pass-through stream byte estimate (breakdown `estimated = true`).
    Estimated(&'a urp::Usage),
    /// Missing usage settled free under `allow_free_when_missing_usage`
    /// (`free_reason = "missing_usage"`, zero token quantities).
    MissingFree,
}

impl<'a> SettledUsage<'a> {
    pub fn usage(&self) -> Option<&'a urp::Usage> {
        match self {
            Self::Reported(usage) | Self::Estimated(usage) => Some(usage),
            Self::MissingFree => None,
        }
    }
}

/// All inputs of one settlement (MP-R7: resolved at preflight, reused here).
pub struct SettlementInputs<'a> {
    pub usage: SettledUsage<'a>,
    /// Decoded terminal output for provider-native tool item counting (MP-T4).
    pub output: Option<&'a [urp::Node]>,
    /// `None` = unpriced settlement under `allow_free_when_unpriced` (MP-F2).
    pub price: Option<&'a ModelPriceRecord>,
    /// Normalized served upstream model key (MP-R1).
    pub pricing_model_key: &'a str,
    /// The `tool_prices` system setting object (MP-T1).
    pub tool_prices: &'a Value,
    /// Requested server-tool usage classes in request descriptor order.
    pub requested_tool_classes: &'a [String],
    pub service_tier: Option<&'a str>,
    /// MP-G1/MP-G3: `None` for system-originated internal traffic.
    pub billing_group_id: Option<&'a str>,
    pub group_billing_ratio: Multiplier,
    pub channel_multiplier: Multiplier,
}

pub struct SettlementOutcome {
    pub final_charge_nano: i128,
    /// `billing_breakdown_json` version 3 (MP-B1).
    pub breakdown: Value,
}

/// Exact decimal token prices resolved per MP-R5 (or a tier per MP-C9).
struct ResolvedTokenPrices {
    input: PriceField,
    output: PriceField,
    cache_read: PriceField,
    cache_write: PriceField,
    cache_write_1h: PriceField,
    reasoning: PriceField,
}

#[derive(Clone)]
struct PriceField {
    value: Decimal,
    raw: String,
}

fn parse_price(raw: &str, column: &str) -> Result<PriceField, String> {
    let value = Decimal::from_str_exact(raw)
        .map_err(|error| format!("stored price {column}={raw} is not exact decimal: {error}"))?;
    Ok(PriceField {
        value,
        raw: raw.to_string(),
    })
}

fn resolve_price_chain(
    input: &str,
    output: Option<&str>,
    cache_read: Option<&str>,
    cache_write: Option<&str>,
    cache_write_1h: Option<&str>,
    reasoning: Option<&str>,
) -> Result<ResolvedTokenPrices, String> {
    let input = parse_price(input, "input_usd_per_1m")?;
    let output = match output {
        Some(raw) => parse_price(raw, "output_usd_per_1m")?,
        None => input.clone(),
    };
    let cache_read = match cache_read {
        Some(raw) => parse_price(raw, "cache_read_usd_per_1m")?,
        None => input.clone(),
    };
    let cache_write = match cache_write {
        Some(raw) => parse_price(raw, "cache_write_usd_per_1m")?,
        None => input.clone(),
    };
    let cache_write_1h = match cache_write_1h {
        Some(raw) => parse_price(raw, "cache_write_1h_usd_per_1m")?,
        None => cache_write.clone(),
    };
    let reasoning = match reasoning {
        Some(raw) => parse_price(raw, "reasoning_usd_per_1m")?,
        None => output.clone(),
    };
    Ok(ResolvedTokenPrices {
        input,
        output,
        cache_read,
        cache_write,
        cache_write_1h,
        reasoning,
    })
}

fn resolve_row_prices(price: &ModelPriceRecord) -> Result<ResolvedTokenPrices, String> {
    let input = price
        .input_usd_per_1m
        .as_deref()
        .ok_or_else(|| "per_token row without input_usd_per_1m".to_string())?;
    resolve_price_chain(
        input,
        price.output_usd_per_1m.as_deref(),
        price.cache_read_usd_per_1m.as_deref(),
        price.cache_write_usd_per_1m.as_deref(),
        price.cache_write_1h_usd_per_1m.as_deref(),
        price.reasoning_usd_per_1m.as_deref(),
    )
}

fn tier_price<'a>(tier: &'a Map<String, Value>, field: &str) -> Option<&'a str> {
    tier.get(field).and_then(Value::as_str)
}

fn resolve_tier_prices(tier: &Map<String, Value>) -> Result<ResolvedTokenPrices, String> {
    let input = tier_price(tier, "input_usd_per_1m")
        .ok_or_else(|| "tier without input_usd_per_1m".to_string())?;
    resolve_price_chain(
        input,
        tier_price(tier, "output_usd_per_1m"),
        tier_price(tier, "cache_read_usd_per_1m"),
        tier_price(tier, "cache_write_usd_per_1m"),
        tier_price(tier, "cache_write_1h_usd_per_1m"),
        tier_price(tier, "reasoning_usd_per_1m"),
    )
}

/// MP-C8: the applied tier is the first tier whose bound is `>= input_tokens`,
/// else the last tier.
fn select_tier(expr: &Value, input_tokens: u64) -> Result<(usize, Map<String, Value>), String> {
    let tiers = expr
        .get("tiers")
        .and_then(Value::as_array)
        .filter(|tiers| !tiers.is_empty())
        .ok_or_else(|| "billing_expr.tiers missing or empty".to_string())?;
    for (index, tier) in tiers.iter().enumerate() {
        let tier = tier
            .as_object()
            .ok_or_else(|| format!("billing_expr.tiers[{index}] is not an object"))?;
        match tier.get("when_input_tokens_lte").and_then(Value::as_u64) {
            Some(bound) if bound >= input_tokens => return Ok((index, tier.clone())),
            Some(_) => continue,
            None => return Ok((index, tier.clone())),
        }
    }
    let last = tiers.len() - 1;
    let tier = tiers[last]
        .as_object()
        .ok_or_else(|| format!("billing_expr.tiers[{last}] is not an object"))?;
    Ok((last, tier.clone()))
}

/// MP-C1/MP-C2 token quantities from normalized usage.
struct TokenQuantities {
    input_uncached: u64,
    cache_read: u64,
    /// `cache_w_5m` plus any unsplit aggregate remainder (MP-C2).
    cache_write: u64,
    cache_write_1h: u64,
    output_plain: u64,
    reasoning: u64,
}

fn token_quantities(usage: &urp::Usage) -> TokenQuantities {
    let details = usage.input_details.as_ref();
    let cache_read = details.map(|d| d.cache_read_tokens).unwrap_or(0);
    let cache_w_5m = details.map(|d| d.cache_creation_5m_tokens).unwrap_or(0);
    let cache_w_1h = details.map(|d| d.cache_creation_1h_tokens).unwrap_or(0);
    let cache_w_agg = details.map(|d| d.cache_creation_tokens).unwrap_or(0);
    let unsplit = cache_w_agg.saturating_sub(cache_w_5m.saturating_add(cache_w_1h));
    let input_uncached = usage
        .input_tokens
        .saturating_sub(cache_read)
        .saturating_sub(cache_w_agg);
    let reasoning = usage
        .output_details
        .as_ref()
        .map(|d| d.reasoning_tokens)
        .unwrap_or(0);
    let output_plain = usage.output_tokens.saturating_sub(reasoning);
    TokenQuantities {
        input_uncached,
        cache_read,
        cache_write: cache_w_5m.saturating_add(unsplit),
        cache_write_1h: cache_w_1h,
        output_plain,
        reasoning,
    }
}

/// MP-C3: `trunc(quantity * usd_per_1m * 1000)` per line item.
fn token_line_charge_nano(quantity: u64, usd_per_1m: &Decimal) -> Result<i128, String> {
    Decimal::from(quantity)
        .checked_mul(*usd_per_1m)
        .and_then(|value| value.checked_mul(Decimal::from(1000u32)))
        .map(|value| value.trunc())
        .and_then(|value| value.to_i128())
        .ok_or_else(|| "token charge overflow".to_string())
}

fn push_token_line(
    line_items: &mut Vec<Value>,
    total: &mut i128,
    usage_class: &str,
    quantity: u64,
    price: &PriceField,
) -> Result<(), String> {
    if quantity == 0 {
        return Ok(());
    }
    let charge = token_line_charge_nano(quantity, &price.value)?;
    line_items.push(json!({
        "usage_class": usage_class,
        "quantity": quantity,
        "usd_per_1m": price.raw,
        "charge_nano": charge.to_string(),
    }));
    *total = total
        .checked_add(charge)
        .ok_or_else(|| "token charge overflow".to_string())?;
    Ok(())
}

fn per_token_lines(
    prices: &ResolvedTokenPrices,
    quantities: &TokenQuantities,
) -> Result<(Vec<Value>, i128), String> {
    let mut line_items = Vec::new();
    let mut total = 0i128;
    push_token_line(
        &mut line_items,
        &mut total,
        "input_uncached",
        quantities.input_uncached,
        &prices.input,
    )?;
    push_token_line(
        &mut line_items,
        &mut total,
        "cache_read",
        quantities.cache_read,
        &prices.cache_read,
    )?;
    push_token_line(
        &mut line_items,
        &mut total,
        "cache_write",
        quantities.cache_write,
        &prices.cache_write,
    )?;
    push_token_line(
        &mut line_items,
        &mut total,
        "cache_write_1h",
        quantities.cache_write_1h,
        &prices.cache_write_1h,
    )?;
    push_token_line(
        &mut line_items,
        &mut total,
        "output",
        quantities.output_plain,
        &prices.output,
    )?;
    push_token_line(
        &mut line_items,
        &mut total,
        "reasoning_output",
        quantities.reasoning,
        &prices.reasoning,
    )?;
    Ok((line_items, total))
}

/// MP-C5: `trunc(per_request_usd * 1_000_000_000)`.
fn per_request_charge_nano(per_request_usd: &str) -> Result<i128, String> {
    let usd = Decimal::from_str_exact(per_request_usd)
        .map_err(|error| format!("stored per_request_usd is not exact decimal: {error}"))?;
    usd.checked_mul(Decimal::from(1_000_000_000u64))
        .map(|value| value.trunc())
        .and_then(|value| value.to_i128())
        .ok_or_else(|| "per-request charge overflow".to_string())
}

// ---------------------------------------------------------------------------
// Tool billing (§6)
// ---------------------------------------------------------------------------

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum ToolUnit {
    KCalls,
    Minute,
    Session,
}

impl ToolUnit {
    fn as_str(self) -> &'static str {
        match self {
            Self::KCalls => "1k_calls",
            Self::Minute => "minute",
            Self::Session => "session",
        }
    }
}

struct ToolPrice {
    usd: Decimal,
    usd_raw: String,
    per: ToolUnit,
    minimum_units: Option<u64>,
}

/// MP-T1..MP-T3: decode one `tool_prices` entry. A malformed persisted entry
/// behaves like a missing entry (fail-open, MP-T8).
fn parse_tool_price(entry: &Value) -> Option<ToolPrice> {
    match entry {
        Value::Number(number) => {
            let raw = number.to_string();
            let usd = Decimal::from_str_exact(&raw).ok()?;
            (!usd.is_sign_negative()).then_some(ToolPrice {
                usd,
                usd_raw: raw,
                per: ToolUnit::KCalls,
                minimum_units: None,
            })
        }
        Value::String(raw) => {
            let usd = Decimal::from_str_exact(raw).ok()?;
            (!usd.is_sign_negative()).then_some(ToolPrice {
                usd,
                usd_raw: raw.clone(),
                per: ToolUnit::KCalls,
                minimum_units: None,
            })
        }
        Value::Object(fields) => {
            let usd_raw = match fields.get("usd")? {
                Value::Number(number) => number.to_string(),
                Value::String(raw) => raw.clone(),
                _ => return None,
            };
            let usd = Decimal::from_str_exact(&usd_raw).ok()?;
            if usd.is_sign_negative() {
                return None;
            }
            let per = match fields.get("per").and_then(Value::as_str)? {
                "1k_calls" => ToolUnit::KCalls,
                "minute" => ToolUnit::Minute,
                "session" => ToolUnit::Session,
                _ => return None,
            };
            let minimum_units = fields.get("minimum_units").and_then(Value::as_u64);
            Some(ToolPrice {
                usd,
                usd_raw,
                per,
                minimum_units,
            })
        }
        _ => None,
    }
}

fn parse_u64_value(value: &Value) -> Option<u64> {
    value
        .as_u64()
        .or_else(|| value.as_i64().and_then(|v| u64::try_from(v).ok()))
        .or_else(|| value.as_str().and_then(|s| s.parse::<u64>().ok()))
}

/// MP-T5: authoritative quantity for one usage class from normalized upstream
/// usage `extra_body`. Direct keys first, then provider-shaped containers.
fn authoritative_tool_quantity(usage: &urp::Usage, usage_class: &str) -> Option<u64> {
    let direct_keys = [
        usage_class.to_string(),
        format!("{usage_class}_requests"),
        format!("{usage_class}_calls"),
        format!("{usage_class}_billed_minutes"),
        format!("{usage_class}_minutes"),
        format!("{usage_class}_sessions"),
    ];
    for key in &direct_keys {
        if let Some(value) = usage.extra_body.get(key).and_then(parse_u64_value) {
            return Some(value);
        }
    }
    if let Some(obj) = usage
        .extra_body
        .get("server_tool_use")
        .and_then(Value::as_object)
    {
        let key = match usage_class {
            "web_search" => "web_search_requests",
            "code_execution_duration" => "code_execution_billed_minutes",
            "code_execution" => "code_execution_requests",
            _ => usage_class,
        };
        if let Some(value) = obj.get(key).and_then(parse_u64_value) {
            return Some(value);
        }
    }
    if let Some(obj) = usage
        .extra_body
        .get("server_side_tool_usage")
        .and_then(Value::as_object)
    {
        for key in [
            usage_class,
            &format!("{usage_class}_calls"),
            &format!("{usage_class}_requests"),
        ] {
            if let Some(value) = obj.get(key).and_then(parse_u64_value) {
                return Some(value);
            }
        }
    }
    None
}

/// MP-T4/MP-T5: count matching provider-native tool items in decoded output.
fn decoded_provider_item_count(output: Option<&[urp::Node]>, usage_class: &str) -> u64 {
    let Some(output) = output else {
        return 0;
    };
    output
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
                _ => false,
            },
            _ => false,
        })
        .count() as u64
}

/// MP-T7 charge for one class.
fn tool_charge_nano(price: &ToolPrice, quantity: u64) -> Result<i128, String> {
    let scale = match price.per {
        // usd per 1000 calls: trunc(count * usd * 1e9 / 1000)
        ToolUnit::KCalls => Decimal::from(1_000_000u64),
        ToolUnit::Minute | ToolUnit::Session => Decimal::from(1_000_000_000u64),
    };
    Decimal::from(quantity)
        .checked_mul(price.usd)
        .and_then(|value| value.checked_mul(scale))
        .map(|value| value.trunc())
        .and_then(|value| value.to_i128())
        .ok_or_else(|| "tool charge overflow".to_string())
}

struct ToolSettlement {
    line_items: Vec<Value>,
    unpriced_tool_classes: Vec<String>,
    total: i128,
}

/// MP-T4..MP-T8: settle every actually used class; missing prices and missing
/// authoritative quantities fail open into `unpriced_tool_classes`.
fn settle_tools(
    usage: Option<&urp::Usage>,
    output: Option<&[urp::Node]>,
    tool_prices: &Value,
    requested_tool_classes: &[String],
) -> Result<ToolSettlement, String> {
    let mut line_items = Vec::new();
    let mut unpriced_tool_classes = Vec::new();
    let mut total = 0i128;
    let empty = Map::new();
    let prices = tool_prices.as_object().unwrap_or(&empty);
    for usage_class in requested_tool_classes {
        let authoritative = usage.and_then(|usage| authoritative_tool_quantity(usage, usage_class));
        let decoded_count = decoded_provider_item_count(output, usage_class);
        let actually_used = authoritative.is_some_and(|quantity| quantity > 0) || decoded_count > 0;
        if !actually_used {
            continue;
        }
        let Some(price) = prices.get(usage_class).and_then(parse_tool_price) else {
            unpriced_tool_classes.push(usage_class.clone());
            continue;
        };
        let quantity = match price.per {
            ToolUnit::KCalls => authoritative.unwrap_or(decoded_count),
            ToolUnit::Minute | ToolUnit::Session => match authoritative {
                Some(quantity) if quantity > 0 => quantity,
                // MP-T8: unit kinds requiring an authoritative quantity fail
                // open when upstream usage does not provide one.
                _ => {
                    unpriced_tool_classes.push(usage_class.clone());
                    continue;
                }
            },
        };
        if quantity == 0 {
            continue;
        }
        let billed_quantity = match (price.per, price.minimum_units) {
            (ToolUnit::Minute | ToolUnit::Session, Some(minimum)) => quantity.max(minimum),
            _ => quantity,
        };
        let charge = tool_charge_nano(&price, billed_quantity)?;
        line_items.push(json!({
            "usage_class": usage_class,
            "quantity": billed_quantity,
            "per": price.per.as_str(),
            "usd": price.usd_raw,
            "charge_nano": charge.to_string(),
        }));
        total = total
            .checked_add(charge)
            .ok_or_else(|| "tool charge overflow".to_string())?;
    }
    Ok(ToolSettlement {
        line_items,
        unpriced_tool_classes,
        total,
    })
}

// ---------------------------------------------------------------------------
// Settlement entry point
// ---------------------------------------------------------------------------

/// Settle one attempt into a nano-USD charge and a version-3 breakdown
/// (MP-C11, MP-B1). Errors indicate arithmetic overflow or a corrupted
/// persisted price and map to HTTP 500 at call sites.
pub fn settle(inputs: &SettlementInputs<'_>) -> Result<SettlementOutcome, String> {
    let usage = inputs.usage.usage();
    let estimated = matches!(inputs.usage, SettledUsage::Estimated(_));

    let mut free_reason: Option<&str> = None;
    let mut billing_mode: Option<&str> = None;
    let mut price_row_model_id: Option<String> = None;
    let mut applied_tier_index: Option<usize> = None;
    let mut token_line_items: Vec<Value> = Vec::new();
    let mut token_charge_nano = 0i128;

    match inputs.price {
        // MP-F2/MP-F5: unpriced free settlement takes precedence.
        None => {
            free_reason = Some("unpriced");
        }
        Some(price) => {
            billing_mode = Some(price.billing_mode.as_str());
            price_row_model_id = Some(price.model_id.clone());
            match usage {
                None => {
                    // MP-F3: missing usage settled free; zero token quantities.
                    free_reason = Some("missing_usage");
                }
                Some(usage) => match price.billing_mode.as_str() {
                    "per_token" => {
                        let prices = resolve_row_prices(price)?;
                        let quantities = token_quantities(usage);
                        let (items, total) = per_token_lines(&prices, &quantities)?;
                        token_line_items = items;
                        token_charge_nano = total;
                    }
                    "per_request" => {
                        let per_request_usd = price
                            .per_request_usd
                            .as_deref()
                            .ok_or_else(|| "per_request row without per_request_usd".to_string())?;
                        let charge = per_request_charge_nano(per_request_usd)?;
                        token_line_items.push(json!({
                            "usage_class": "per_request",
                            "quantity": 1,
                            "usd": per_request_usd,
                            "charge_nano": charge.to_string(),
                        }));
                        token_charge_nano = charge;
                    }
                    "tiered_expr" => {
                        let expr = price
                            .billing_expr
                            .as_ref()
                            .ok_or_else(|| "tiered_expr row without billing_expr".to_string())?;
                        let (index, tier) = select_tier(expr, usage.input_tokens)?;
                        applied_tier_index = Some(index);
                        let prices = resolve_tier_prices(&tier)?;
                        let quantities = token_quantities(usage);
                        let (items, total) = per_token_lines(&prices, &quantities)?;
                        token_line_items = items;
                        token_charge_nano = total;
                    }
                    other => return Err(format!("unknown billing_mode `{other}`")),
                },
            }
        }
    }

    // MP-T9: tool charges apply in every billing mode and in free settlements.
    let tools = settle_tools(
        usage,
        inputs.output,
        inputs.tool_prices,
        inputs.requested_tool_classes,
    )?;

    let base_charge_nano = token_charge_nano
        .checked_add(tools.total)
        .ok_or_else(|| "charge overflow".to_string())?;
    // MP-C11: one exact composition, one final truncation toward zero.
    let composed = inputs
        .channel_multiplier
        .compose(inputs.group_billing_ratio)
        .ok_or_else(|| "multiplier composition overflow".to_string())?;
    let final_charge_nano = composed
        .checked_scale_i128(base_charge_nano)
        .ok_or_else(|| "charge overflow".to_string())?;

    let breakdown = json!({
        "version": 3,
        "billing_mode": billing_mode,
        "pricing_model_key": inputs.pricing_model_key,
        "price_row_model_id": price_row_model_id,
        "applied_tier_index": applied_tier_index,
        "token_line_items": token_line_items,
        "tool_line_items": tools.line_items,
        "unpriced_tool_classes": tools.unpriced_tool_classes,
        "service_tier": inputs.service_tier,
        "billing_group_id": inputs.billing_group_id,
        "group_billing_ratio": inputs.group_billing_ratio.canonical(),
        "channel_multiplier": inputs.channel_multiplier.canonical(),
        "base_charge_nano": base_charge_nano.to_string(),
        "final_charge_nano": final_charge_nano.to_string(),
        "free_reason": free_reason,
        "estimated": estimated,
    });

    Ok(SettlementOutcome {
        final_charge_nano,
        breakdown,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use chrono::Utc;

    fn price_row(billing_mode: &str) -> ModelPriceRecord {
        ModelPriceRecord {
            model_id: "test-model".to_string(),
            billing_mode: billing_mode.to_string(),
            input_usd_per_1m: Some("2.5".to_string()),
            output_usd_per_1m: Some("10".to_string()),
            cache_read_usd_per_1m: Some("0.25".to_string()),
            cache_write_usd_per_1m: Some("3.125".to_string()),
            cache_write_1h_usd_per_1m: None,
            reasoning_usd_per_1m: None,
            per_request_usd: None,
            billing_expr: None,
            source: "manual".to_string(),
            locked_fields: Vec::new(),
            raw_json: json!({}),
            enabled: true,
            updated_at: Utc::now(),
        }
    }

    fn usage(input: u64, output: u64) -> urp::Usage {
        urp::Usage {
            input_tokens: input,
            output_tokens: output,
            input_details: None,
            output_details: None,
            extra_body: std::collections::HashMap::new(),
        }
    }

    fn base_inputs<'a>(
        usage: SettledUsage<'a>,
        price: Option<&'a ModelPriceRecord>,
        tool_prices: &'a Value,
    ) -> SettlementInputs<'a> {
        SettlementInputs {
            usage,
            output: None,
            price,
            pricing_model_key: "test-model",
            tool_prices,
            requested_tool_classes: &[],
            service_tier: None,
            billing_group_id: None,
            group_billing_ratio: Multiplier::ONE,
            channel_multiplier: Multiplier::ONE,
        }
    }

    #[test]
    fn per_token_settlement_matches_mp_c3() {
        let price = price_row("per_token");
        let usage = usage(1_200, 100);
        let tool_prices = json!({});
        let outcome = settle(&base_inputs(
            SettledUsage::Reported(&usage),
            Some(&price),
            &tool_prices,
        ))
        .unwrap();
        // input: 1200 * 2.5 * 1000 = 3_000_000; output: 100 * 10 * 1000 = 1_000_000
        assert_eq!(outcome.final_charge_nano, 4_000_000);
        assert_eq!(outcome.breakdown["version"], json!(3));
        assert_eq!(outcome.breakdown["free_reason"], json!(null));
        assert_eq!(outcome.breakdown["billing_mode"], json!("per_token"));
        assert_eq!(
            outcome.breakdown["token_line_items"]
                .as_array()
                .unwrap()
                .len(),
            2
        );
    }

    #[test]
    fn cache_quantities_split_per_mp_c1_and_c2() {
        let price = price_row("per_token");
        let mut u = usage(10_000, 0);
        u.input_details = Some(urp::InputDetails {
            standard_tokens: 0,
            cache_read_tokens: 4_000,
            cache_read_modality_breakdown: None,
            cache_creation_tokens: 3_000,
            cache_creation_5m_tokens: 1_000,
            cache_creation_1h_tokens: 500,
            tool_prompt_tokens: 0,
            modality_breakdown: None,
        });
        let tool_prices = json!({});
        let outcome = settle(&base_inputs(
            SettledUsage::Reported(&u),
            Some(&price),
            &tool_prices,
        ))
        .unwrap();
        let items = outcome.breakdown["token_line_items"].as_array().unwrap();
        let quantity = |class: &str| {
            items
                .iter()
                .find(|item| item["usage_class"] == json!(class))
                .map(|item| item["quantity"].as_u64().unwrap())
        };
        // input_uncached = 10000 - 4000 - 3000 = 3000
        assert_eq!(quantity("input_uncached"), Some(3_000));
        assert_eq!(quantity("cache_read"), Some(4_000));
        // cache_write = 1000 (5m) + 1500 (unsplit remainder)
        assert_eq!(quantity("cache_write"), Some(2_500));
        assert_eq!(quantity("cache_write_1h"), Some(500));
        assert_eq!(quantity("output"), None);
    }

    #[test]
    fn per_request_settlement_matches_mp_c5() {
        let mut price = price_row("per_request");
        price.per_request_usd = Some("0.05".to_string());
        let usage = usage(1_000_000, 1_000_000);
        let tool_prices = json!({});
        let outcome = settle(&base_inputs(
            SettledUsage::Reported(&usage),
            Some(&price),
            &tool_prices,
        ))
        .unwrap();
        assert_eq!(outcome.final_charge_nano, 50_000_000);
    }

    #[test]
    fn tiered_settlement_selects_first_covering_tier() {
        let mut price = price_row("tiered_expr");
        price.billing_expr = Some(json!({
            "tiers": [
                { "when_input_tokens_lte": 200_000, "input_usd_per_1m": "1.25",
                  "output_usd_per_1m": "10" },
                { "input_usd_per_1m": "2.5", "output_usd_per_1m": "15" }
            ]
        }));
        let tool_prices = json!({});

        let small = usage(200_000, 0);
        let outcome = settle(&base_inputs(
            SettledUsage::Reported(&small),
            Some(&price),
            &tool_prices,
        ))
        .unwrap();
        assert_eq!(outcome.breakdown["applied_tier_index"], json!(0));
        // 200_000 * 1.25 * 1000 = 250_000_000
        assert_eq!(outcome.final_charge_nano, 250_000_000);

        let large = usage(200_001, 0);
        let outcome = settle(&base_inputs(
            SettledUsage::Reported(&large),
            Some(&price),
            &tool_prices,
        ))
        .unwrap();
        assert_eq!(outcome.breakdown["applied_tier_index"], json!(1));
        // 200_001 * 2.5 * 1000 = 500_002_500
        assert_eq!(outcome.final_charge_nano, 500_002_500);
    }

    #[test]
    fn final_charge_composes_multiplier_and_ratio_with_single_truncation() {
        let price = price_row("per_token");
        let usage = usage(1, 0);
        let tool_prices = json!({});
        let mut inputs = base_inputs(SettledUsage::Reported(&usage), Some(&price), &tool_prices);
        // base = trunc(1 * 2.5 * 1000) = 2500
        inputs.channel_multiplier = Multiplier::parse("1.5").unwrap();
        inputs.group_billing_ratio = Multiplier::parse("0.3").unwrap();
        // trunc(2500 * 0.45) = 1125 — composing first avoids double truncation.
        let outcome = settle(&inputs).unwrap();
        assert_eq!(outcome.final_charge_nano, 1_125);
    }

    #[test]
    fn zero_multiplier_settles_normal_breakdown_with_zero_charge() {
        let price = price_row("per_token");
        let usage = usage(1_200, 100);
        let tool_prices = json!({});
        let mut inputs = base_inputs(SettledUsage::Reported(&usage), Some(&price), &tool_prices);
        inputs.channel_multiplier = Multiplier::ZERO;
        let outcome = settle(&inputs).unwrap();
        // MP-C12: normal breakdown, final 0, free_reason null.
        assert_eq!(outcome.final_charge_nano, 0);
        assert_eq!(outcome.breakdown["free_reason"], json!(null));
        assert_eq!(outcome.breakdown["base_charge_nano"], json!("4000000"));
        assert_eq!(outcome.breakdown["final_charge_nano"], json!("0"));
    }

    #[test]
    fn unpriced_free_settlement_keeps_tool_charges() {
        let tool_prices = json!({ "web_search": "10" });
        let mut u = usage(100, 100);
        u.extra_body.insert(
            "server_tool_use".to_string(),
            json!({ "web_search_requests": 2 }),
        );
        let classes = vec!["web_search".to_string()];
        let mut inputs = base_inputs(SettledUsage::Reported(&u), None, &tool_prices);
        inputs.requested_tool_classes = &classes;
        let outcome = settle(&inputs).unwrap();
        assert_eq!(outcome.breakdown["free_reason"], json!("unpriced"));
        assert_eq!(outcome.breakdown["price_row_model_id"], json!(null));
        assert!(
            outcome.breakdown["token_line_items"]
                .as_array()
                .unwrap()
                .is_empty()
        );
        // 2 calls * 10 usd/1k calls = trunc(2 * 10 * 1e6) = 20_000_000
        assert_eq!(outcome.final_charge_nano, 20_000_000);
    }

    #[test]
    fn missing_usage_free_settlement_zeroes_tokens() {
        let price = price_row("per_token");
        let tool_prices = json!({});
        let outcome = settle(&base_inputs(
            SettledUsage::MissingFree,
            Some(&price),
            &tool_prices,
        ))
        .unwrap();
        assert_eq!(outcome.final_charge_nano, 0);
        assert_eq!(outcome.breakdown["free_reason"], json!("missing_usage"));
        assert_eq!(outcome.breakdown["price_row_model_id"], json!("test-model"));
    }

    #[test]
    fn tool_minute_pricing_applies_minimum_units_and_fails_open() {
        let tool_prices = json!({
            "code_interpreter_duration": { "usd": "0.0015", "per": "minute", "minimum_units": 5 },
            "web_search": "10"
        });
        let price = price_row("per_token");
        let mut u = usage(0, 0);
        u.extra_body.insert(
            "code_interpreter_duration_billed_minutes".to_string(),
            json!(2),
        );
        let classes = vec![
            "code_interpreter_duration".to_string(),
            "x_search".to_string(),
        ];
        let mut inputs = base_inputs(SettledUsage::Reported(&u), Some(&price), &tool_prices);
        inputs.requested_tool_classes = &classes;
        let outcome = settle(&inputs).unwrap();
        // minutes: max(2, 5) = 5 → trunc(5 * 0.0015 * 1e9) = 7_500_000
        assert_eq!(outcome.final_charge_nano, 7_500_000);
        // x_search not actually used → not unpriced either.
        assert_eq!(
            outcome.breakdown["unpriced_tool_classes"],
            json!(Vec::<String>::new())
        );
    }

    #[test]
    fn unpriced_tool_class_fails_open_into_breakdown_list() {
        let tool_prices = json!({});
        let price = price_row("per_token");
        let mut u = usage(0, 0);
        u.extra_body.insert(
            "server_tool_use".to_string(),
            json!({ "web_search_requests": 3 }),
        );
        let classes = vec!["web_search".to_string()];
        let mut inputs = base_inputs(SettledUsage::Reported(&u), Some(&price), &tool_prices);
        inputs.requested_tool_classes = &classes;
        let outcome = settle(&inputs).unwrap();
        assert_eq!(outcome.final_charge_nano, 0);
        assert_eq!(
            outcome.breakdown["unpriced_tool_classes"],
            json!(["web_search"])
        );
    }

    #[test]
    fn minute_class_without_authoritative_quantity_fails_open() {
        let tool_prices = json!({
            "code_interpreter_duration": { "usd": "0.0015", "per": "minute" }
        });
        let price = price_row("per_token");
        let u = usage(0, 0);
        let output = vec![urp::Node::ProviderItem {
            id: None,
            origin_protocol: urp::ProviderProtocol::Responses,
            role: urp::OrdinaryRole::Assistant,
            item_type: "code_interpreter_call".to_string(),
            body: json!({}),
            extra_body: std::collections::HashMap::new(),
        }];
        let classes = vec!["code_interpreter_duration".to_string()];
        let mut inputs = base_inputs(SettledUsage::Reported(&u), Some(&price), &tool_prices);
        inputs.requested_tool_classes = &classes;
        inputs.output = Some(&output);
        let outcome = settle(&inputs).unwrap();
        assert_eq!(outcome.final_charge_nano, 0);
        assert_eq!(
            outcome.breakdown["unpriced_tool_classes"],
            json!(["code_interpreter_duration"])
        );
    }

    #[test]
    fn estimated_usage_marks_breakdown() {
        let price = price_row("per_token");
        let usage = usage(100, 50);
        let tool_prices = json!({});
        let outcome = settle(&base_inputs(
            SettledUsage::Estimated(&usage),
            Some(&price),
            &tool_prices,
        ))
        .unwrap();
        assert_eq!(outcome.breakdown["estimated"], json!(true));
        assert_eq!(outcome.breakdown["free_reason"], json!(null));
    }
}
