use super::store::{
    BillingLedgerEntry, MAX_GROUP_IDS, parse_group_ids_json, serialize_group_ids_json,
};
use crate::exact_decimal::Multiplier;
use crate::users::{
    BillingError, BillingErrorKind, UserStore, canonicalize_group_ids, parse_nano_usd,
};
use chrono::{DateTime, Duration, Utc};
use sea_orm::{
    ConnectionTrait, DatabaseTransaction, QueryResult, TransactionTrait, Value as SeaValue,
};
use serde::{Deserialize, Serialize};
use std::collections::HashSet;

const FIVE_HOURS_SECONDS: i64 = 18_000;
const TWENTY_FOUR_HOURS_SECONDS: i64 = 86_400;
const SEVEN_DAYS_SECONDS: i64 = 604_800;
const THIRTY_DAYS_SECONDS: i64 = 2_592_000;

const PLAN_COLUMNS: &str = "id, name, description, limit_5h_nano_usd, \
    limit_24h_nano_usd, limit_7d_nano_usd, limit_30d_nano_usd, group_ids, \
    multiplier, listed, created_at, updated_at";

const SUBSCRIPTION_COLUMNS: &str = "id, user_id, plan_id, price_id, plan_name, \
    plan_description, limit_5h_nano_usd, limit_24h_nano_usd, limit_7d_nano_usd, \
    limit_30d_nano_usd, group_ids, multiplier, price_nano_usd, starts_at, \
    expires_at, created_at";

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct BillingPlanPrice {
    pub id: String,
    pub price_nano_usd: String,
    pub duration_seconds: i64,
    pub created_at: DateTime<Utc>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct BillingPlan {
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
    pub prices: Vec<BillingPlanPrice>,
    pub created_at: DateTime<Utc>,
    pub updated_at: DateTime<Utc>,
}

#[derive(Debug, Clone, Deserialize)]
pub struct BillingPlanPriceInput {
    #[serde(default)]
    pub price_nano_usd: Option<String>,
    #[serde(default)]
    pub price_usd: Option<String>,
    pub duration_seconds: i64,
}

#[derive(Debug, Clone, Deserialize)]
pub struct BillingPlanInput {
    pub name: String,
    #[serde(default)]
    pub description: String,
    #[serde(default)]
    pub limit_5h_nano_usd: Option<String>,
    #[serde(default)]
    pub limit_24h_nano_usd: Option<String>,
    #[serde(default)]
    pub limit_7d_nano_usd: Option<String>,
    #[serde(default)]
    pub limit_30d_nano_usd: Option<String>,
    pub group_ids: Vec<String>,
    #[serde(default = "default_multiplier")]
    pub multiplier: String,
    #[serde(default)]
    pub listed: bool,
    #[serde(default)]
    pub prices: Vec<BillingPlanPriceInput>,
}

fn default_multiplier() -> String {
    "1".to_string()
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PlanWindowUsage {
    pub limit_nano_usd: String,
    pub used_nano_usd: String,
    pub remaining_nano_usd: String,
    pub next_reset_at: Option<DateTime<Utc>>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct BillingPlanUsageWindows {
    pub five_hour: Option<PlanWindowUsage>,
    pub twenty_four_hour: Option<PlanWindowUsage>,
    pub seven_day: Option<PlanWindowUsage>,
    pub thirty_day: Option<PlanWindowUsage>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct BillingPlanSubscription {
    pub id: String,
    pub user_id: String,
    pub plan_id: String,
    pub price_id: Option<String>,
    pub plan_name: String,
    pub plan_description: String,
    pub group_ids: Vec<String>,
    pub multiplier: String,
    pub price_nano_usd: Option<String>,
    pub starts_at: DateTime<Utc>,
    pub expires_at: DateTime<Utc>,
    pub windows: BillingPlanUsageWindows,
}

#[derive(Debug, Clone, Serialize)]
pub struct PlanChargeAllocation {
    pub adjusted_charge_nano_usd: i128,
    pub plan_covered_nano_usd: i128,
    pub fallback_nano_usd: i128,
    pub subscription_id: Option<String>,
    pub plan_id: Option<String>,
    pub multiplier: Option<String>,
}

#[derive(Debug, Clone)]
struct PlanLimits {
    five_hour: Option<i128>,
    twenty_four_hour: Option<i128>,
    seven_day: Option<i128>,
    thirty_day: Option<i128>,
}

#[derive(Debug, Default)]
struct PlanUsageSnapshot {
    sums: [i128; 4],
    next_reset_at: [Option<DateTime<Utc>>; 4],
}

impl PlanLimits {
    fn values(&self) -> [(i64, Option<i128>); 4] {
        [
            (FIVE_HOURS_SECONDS, self.five_hour),
            (TWENTY_FOUR_HOURS_SECONDS, self.twenty_four_hour),
            (SEVEN_DAYS_SECONDS, self.seven_day),
            (THIRTY_DAYS_SECONDS, self.thirty_day),
        ]
    }
}

const USAGE_PROBE_COLUMNS: &str = "created_at, amount_nano_usd, cumulative_nano_usd";

/// One `billing_plan_usage` row read by a BP-U7 ordered lookup.
struct UsageProbe {
    created_at: DateTime<Utc>,
    amount: i128,
    cumulative: i128,
    /// `cumulative - amount`: the prefix sum of every earlier row (BP-D9).
    preceding: i128,
}

impl UsageProbe {
    fn from_row(row: &QueryResult) -> Result<Self, String> {
        let amount = parse_nano_usd(
            &row.try_get::<String>("", "amount_nano_usd")
                .map_err(sql_err)?,
        )
        .map_err(sql_err)?;
        if amount <= 0 {
            return Err(sql_err("plan usage amount must be positive"));
        }
        let cumulative = parse_usage_cumulative(row)?;
        let preceding = cumulative
            .checked_sub(amount)
            .filter(|value| *value >= 0)
            .ok_or_else(|| sql_err("plan usage cumulative is smaller than its amount"))?;
        Ok(Self {
            created_at: parse_time(row, "created_at")?,
            amount,
            cumulative,
            preceding,
        })
    }
}

fn parse_usage_cumulative(row: &QueryResult) -> Result<i128, String> {
    parse_nano_usd(
        &row.try_get::<String>("", "cumulative_nano_usd")
            .map_err(sql_err)?,
    )
    .map_err(sql_err)
}

#[derive(Debug, Clone)]
struct SubscriptionSnapshot {
    id: String,
    user_id: String,
    plan_id: String,
    price_id: Option<String>,
    plan_name: String,
    plan_description: String,
    limits: PlanLimits,
    group_ids: Vec<String>,
    multiplier: Multiplier,
    price_nano_usd: Option<i128>,
    starts_at: DateTime<Utc>,
    expires_at: DateTime<Utc>,
}

#[derive(Debug)]
struct ValidatedPlan {
    name: String,
    description: String,
    limits: PlanLimits,
    group_ids: Vec<String>,
    multiplier: Multiplier,
    listed: bool,
    prices: Vec<(i128, i64)>,
}

fn sql_err(error: impl std::fmt::Display) -> String {
    format!("invalid persisted billing plan data: {error}")
}

fn parse_time(row: &QueryResult, column: &str) -> Result<DateTime<Utc>, String> {
    DateTime::parse_from_rfc3339(&row.try_get::<String>("", column).map_err(sql_err)?)
        .map(|value| value.with_timezone(&Utc))
        .map_err(sql_err)
}

fn parse_optional_positive_nano(raw: Option<&str>) -> Result<Option<i128>, String> {
    let Some(raw) = raw else {
        return Ok(None);
    };
    let value = parse_nano_usd(raw).map_err(|_| "invalid_plan_limits".to_string())?;
    if raw.trim() != value.to_string() || value <= 0 {
        return Err("invalid_plan_limits".to_string());
    }
    Ok(Some(value))
}

fn parse_optional_stored_nano(raw: Option<String>) -> Result<Option<i128>, String> {
    raw.map(|value| parse_nano_usd(&value).map_err(sql_err))
        .transpose()
}

fn parse_price(input: &BillingPlanPriceInput) -> Result<(i128, i64), String> {
    if input.duration_seconds <= 0 {
        return Err("invalid_plan_prices".to_string());
    }
    // BP-D4a: the plan editor sells whole days only.
    if input.duration_seconds % TWENTY_FOUR_HOURS_SECONDS != 0 {
        return Err("invalid_plan_price_duration".to_string());
    }
    let amount = if let Some(raw) = input.price_nano_usd.as_deref() {
        let value = parse_nano_usd(raw).map_err(|_| "invalid_plan_prices".to_string())?;
        if raw.trim() != value.to_string() {
            return Err("invalid_plan_prices".to_string());
        }
        value
    } else if let Some(raw) = input.price_usd.as_deref() {
        super::utils::parse_usd_to_nano(raw).map_err(|_| "invalid_plan_prices".to_string())?
    } else {
        return Err("invalid_plan_prices".to_string());
    };
    if amount <= 0 {
        return Err("invalid_plan_prices".to_string());
    }
    Ok((amount, input.duration_seconds))
}

fn validate_plan_input(input: &BillingPlanInput) -> Result<ValidatedPlan, String> {
    let name = input.name.trim();
    if name.is_empty() || name.chars().count() > 100 {
        return Err("invalid_plan_name".to_string());
    }
    let description = input.description.trim();
    if description.chars().count() > 1000 {
        return Err("invalid_plan_description".to_string());
    }
    let limits = PlanLimits {
        five_hour: parse_optional_positive_nano(input.limit_5h_nano_usd.as_deref())?,
        twenty_four_hour: parse_optional_positive_nano(input.limit_24h_nano_usd.as_deref())?,
        seven_day: parse_optional_positive_nano(input.limit_7d_nano_usd.as_deref())?,
        thirty_day: parse_optional_positive_nano(input.limit_30d_nano_usd.as_deref())?,
    };
    if limits.values().iter().all(|(_, value)| value.is_none()) {
        return Err("invalid_plan_limits".to_string());
    }
    let group_ids = canonicalize_group_ids(&input.group_ids);
    if group_ids.is_empty() || group_ids.len() > MAX_GROUP_IDS {
        return Err("invalid_plan_groups".to_string());
    }
    let multiplier = Multiplier::parse(input.multiplier.trim())
        .map_err(|_| "invalid_plan_multiplier".to_string())?;
    if !multiplier.is_positive() {
        return Err("invalid_plan_multiplier".to_string());
    }
    let mut durations = HashSet::new();
    let mut prices = Vec::with_capacity(input.prices.len());
    for price in &input.prices {
        let parsed = parse_price(price)?;
        if !durations.insert(parsed.1) {
            return Err("invalid_plan_prices".to_string());
        }
        prices.push(parsed);
    }
    if input.listed && prices.is_empty() {
        return Err("invalid_plan_prices".to_string());
    }
    Ok(ValidatedPlan {
        name: name.to_string(),
        description: description.to_string(),
        limits,
        group_ids,
        multiplier,
        listed: input.listed,
        prices,
    })
}

fn row_to_plan(row: &QueryResult, prices: Vec<BillingPlanPrice>) -> Result<BillingPlan, String> {
    let parse_stored = |column: &str| -> Result<Option<String>, String> {
        let raw = row.try_get::<Option<String>>("", column).map_err(sql_err)?;
        if let Some(value) = raw.as_deref() {
            let parsed = parse_nano_usd(value).map_err(sql_err)?;
            if parsed <= 0 || value.trim() != parsed.to_string() {
                return Err(sql_err(format!("invalid {column}")));
            }
        }
        Ok(raw)
    };
    let group_ids_raw: String = row.try_get("", "group_ids").map_err(sql_err)?;
    let multiplier_raw: String = row.try_get("", "multiplier").map_err(sql_err)?;
    let multiplier = Multiplier::parse(&multiplier_raw).map_err(sql_err)?;
    if !multiplier.is_positive() {
        return Err(sql_err("plan multiplier must be positive"));
    }
    Ok(BillingPlan {
        id: row.try_get("", "id").map_err(sql_err)?,
        name: row.try_get("", "name").map_err(sql_err)?,
        description: row.try_get("", "description").map_err(sql_err)?,
        limit_5h_nano_usd: parse_stored("limit_5h_nano_usd")?,
        limit_24h_nano_usd: parse_stored("limit_24h_nano_usd")?,
        limit_7d_nano_usd: parse_stored("limit_7d_nano_usd")?,
        limit_30d_nano_usd: parse_stored("limit_30d_nano_usd")?,
        group_ids: parse_group_ids_json(Some(&group_ids_raw), "billing_plans.group_ids")?,
        multiplier: multiplier.canonical(),
        listed: super::store::decode_required_bool(row, "listed")?,
        prices,
        created_at: parse_time(row, "created_at")?,
        updated_at: parse_time(row, "updated_at")?,
    })
}

fn row_to_subscription(row: &QueryResult) -> Result<SubscriptionSnapshot, String> {
    let group_ids_raw: String = row.try_get("", "group_ids").map_err(sql_err)?;
    let multiplier_raw: String = row.try_get("", "multiplier").map_err(sql_err)?;
    let multiplier = Multiplier::parse(&multiplier_raw).map_err(sql_err)?;
    if !multiplier.is_positive() {
        return Err(sql_err("subscription multiplier must be positive"));
    }
    let price_id = row
        .try_get::<Option<String>>("", "price_id")
        .map_err(sql_err)?;
    let price_raw = row
        .try_get::<Option<String>>("", "price_nano_usd")
        .map_err(sql_err)?;
    let price_nano_usd = match (&price_id, price_raw) {
        (Some(_), Some(raw)) => {
            let value = parse_nano_usd(&raw).map_err(sql_err)?;
            if value <= 0 || value.to_string() != raw {
                return Err(sql_err("invalid stored subscription price"));
            }
            Some(value)
        }
        (None, None) => None,
        _ => return Err(sql_err("incomplete stored subscription price snapshot")),
    };
    Ok(SubscriptionSnapshot {
        id: row.try_get("", "id").map_err(sql_err)?,
        user_id: row.try_get("", "user_id").map_err(sql_err)?,
        plan_id: row.try_get("", "plan_id").map_err(sql_err)?,
        price_id,
        plan_name: row.try_get("", "plan_name").map_err(sql_err)?,
        plan_description: row.try_get("", "plan_description").map_err(sql_err)?,
        limits: PlanLimits {
            five_hour: parse_optional_stored_nano(
                row.try_get("", "limit_5h_nano_usd").map_err(sql_err)?,
            )?,
            twenty_four_hour: parse_optional_stored_nano(
                row.try_get("", "limit_24h_nano_usd").map_err(sql_err)?,
            )?,
            seven_day: parse_optional_stored_nano(
                row.try_get("", "limit_7d_nano_usd").map_err(sql_err)?,
            )?,
            thirty_day: parse_optional_stored_nano(
                row.try_get("", "limit_30d_nano_usd").map_err(sql_err)?,
            )?,
        },
        group_ids: parse_group_ids_json(
            Some(&group_ids_raw),
            "billing_plan_subscriptions.group_ids",
        )?,
        multiplier,
        price_nano_usd,
        starts_at: parse_time(row, "starts_at")?,
        expires_at: parse_time(row, "expires_at")?,
    })
}

fn is_plan_name_unique_violation(error: &str) -> bool {
    let lower = error.to_ascii_lowercase();
    (lower.contains("unique") || lower.contains("duplicate"))
        && (lower.contains("name") || lower.contains("uq_billing_plans_name_lower"))
}

pub(crate) struct PlanChargeRequest<'a> {
    pub(crate) user_id: &'a str,
    pub(crate) api_key_id: Option<&'a str>,
    pub(crate) billing_group_id: Option<&'a str>,
    pub(crate) request_id: &'a str,
    pub(crate) settled_charge_nano_usd: i128,
    pub(crate) now: DateTime<Utc>,
}

mod allocation;
mod catalog;
mod subscriptions;
