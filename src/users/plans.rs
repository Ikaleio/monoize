use super::store::{MAX_GROUP_IDS, parse_group_ids_json, serialize_group_ids_json};
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
    pub price_id: String,
    pub plan_name: String,
    pub plan_description: String,
    pub group_ids: Vec<String>,
    pub multiplier: String,
    pub price_nano_usd: String,
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

    fn max_window_seconds(&self) -> i64 {
        self.values()
            .into_iter()
            .filter_map(|(seconds, limit)| limit.map(|_| seconds))
            .max()
            .unwrap_or(FIVE_HOURS_SECONDS)
    }
}

#[derive(Debug, Clone)]
struct SubscriptionSnapshot {
    id: String,
    user_id: String,
    plan_id: String,
    price_id: String,
    plan_name: String,
    plan_description: String,
    limits: PlanLimits,
    group_ids: Vec<String>,
    multiplier: Multiplier,
    price_nano_usd: i128,
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
    Ok(SubscriptionSnapshot {
        id: row.try_get("", "id").map_err(sql_err)?,
        user_id: row.try_get("", "user_id").map_err(sql_err)?,
        plan_id: row.try_get("", "plan_id").map_err(sql_err)?,
        price_id: row.try_get("", "price_id").map_err(sql_err)?,
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
        price_nano_usd: parse_nano_usd(
            &row.try_get::<String>("", "price_nano_usd")
                .map_err(sql_err)?,
        )
        .map_err(sql_err)?,
        starts_at: parse_time(row, "starts_at")?,
        expires_at: parse_time(row, "expires_at")?,
    })
}

fn is_plan_name_unique_violation(error: &str) -> bool {
    let lower = error.to_ascii_lowercase();
    (lower.contains("unique") || lower.contains("duplicate"))
        && (lower.contains("name") || lower.contains("uq_billing_plans_name_lower"))
}

impl UserStore {
    async fn validate_plan_groups(
        &self,
        group_ids: &[String],
    ) -> Result<Result<(), String>, String> {
        if self.find_unknown_group_id(group_ids).await?.is_some() {
            return Ok(Err("invalid_plan_groups".to_string()));
        }
        Ok(Ok(()))
    }

    async fn load_plan_prices_on<C: ConnectionTrait>(
        &self,
        connection: &C,
        plan_id: &str,
    ) -> Result<Vec<BillingPlanPrice>, String> {
        let rows = connection
            .query_all(self.db.stmt(
                "SELECT id, price_nano_usd, duration_seconds, created_at FROM billing_plan_prices WHERE plan_id = $1 ORDER BY duration_seconds ASC, id ASC",
                vec![plan_id.into()],
            ))
            .await
            .map_err(|error| error.to_string())?;
        rows.iter()
            .map(|row| {
                let price_nano_usd: String = row.try_get("", "price_nano_usd").map_err(sql_err)?;
                let parsed = parse_nano_usd(&price_nano_usd).map_err(sql_err)?;
                if parsed <= 0 || parsed.to_string() != price_nano_usd {
                    return Err(sql_err("invalid stored plan price"));
                }
                let duration_seconds: i64 = row.try_get("", "duration_seconds").map_err(sql_err)?;
                if duration_seconds <= 0 {
                    return Err(sql_err("invalid stored plan duration"));
                }
                Ok(BillingPlanPrice {
                    id: row.try_get("", "id").map_err(sql_err)?,
                    price_nano_usd,
                    duration_seconds,
                    created_at: parse_time(row, "created_at")?,
                })
            })
            .collect()
    }

    async fn list_billing_plans_filtered(
        &self,
        listed_only: bool,
    ) -> Result<Vec<BillingPlan>, String> {
        let where_clause = if listed_only { " WHERE listed = 1" } else { "" };
        let rows = self
            .db
            .read()
            .query_all(self.db.stmt(
                &format!("SELECT {PLAN_COLUMNS} FROM billing_plans{where_clause} ORDER BY created_at ASC, id ASC"),
                vec![],
            ))
            .await
            .map_err(|error| error.to_string())?;
        let mut plans = Vec::with_capacity(rows.len());
        for row in rows {
            let id: String = row.try_get("", "id").map_err(sql_err)?;
            let prices = self.load_plan_prices_on(&*self.db.read(), &id).await?;
            plans.push(row_to_plan(&row, prices)?);
        }
        Ok(plans)
    }

    pub async fn list_billing_plans(&self) -> Result<Vec<BillingPlan>, String> {
        self.list_billing_plans_filtered(false).await
    }

    pub async fn list_marketplace_billing_plans(&self) -> Result<Vec<BillingPlan>, String> {
        self.list_billing_plans_filtered(true).await
    }

    pub async fn get_billing_plan_by_id(&self, id: &str) -> Result<Option<BillingPlan>, String> {
        let read = self.db.read();
        let row = read
            .query_one(self.db.stmt(
                &format!("SELECT {PLAN_COLUMNS} FROM billing_plans WHERE id = $1"),
                vec![id.into()],
            ))
            .await
            .map_err(|error| error.to_string())?;
        let Some(row) = row else {
            return Ok(None);
        };
        let prices = self.load_plan_prices_on(&*read, id).await?;
        Ok(Some(row_to_plan(&row, prices)?))
    }

    async fn plan_name_exists_on<C: ConnectionTrait>(
        &self,
        connection: &C,
        exclude_id: Option<&str>,
        name: &str,
    ) -> Result<bool, String> {
        let (sql, values) = match exclude_id {
            Some(exclude_id) => (
                "SELECT 1 AS one FROM billing_plans WHERE lower(trim(name)) = lower(trim($1)) AND id != $2 LIMIT 1",
                vec![name.into(), exclude_id.into()],
            ),
            None => (
                "SELECT 1 AS one FROM billing_plans WHERE lower(trim(name)) = lower(trim($1)) LIMIT 1",
                vec![name.into()],
            ),
        };
        Ok(connection
            .query_one(self.db.stmt(sql, values))
            .await
            .map_err(|error| error.to_string())?
            .is_some())
    }

    async fn insert_prices_tx(
        &self,
        tx: &DatabaseTransaction,
        plan_id: &str,
        prices: &[(i128, i64)],
        now: &str,
    ) -> Result<(), String> {
        for (amount, duration_seconds) in prices {
            tx.execute(self.db.stmt(
                "INSERT INTO billing_plan_prices (id, plan_id, price_nano_usd, duration_seconds, created_at) VALUES ($1, $2, $3, $4, $5)",
                vec![
                    uuid::Uuid::new_v4().to_string().into(),
                    plan_id.into(),
                    amount.to_string().into(),
                    (*duration_seconds).into(),
                    now.into(),
                ],
            ))
            .await
            .map_err(|error| error.to_string())?;
        }
        Ok(())
    }

    pub async fn create_billing_plan(
        &self,
        input: BillingPlanInput,
    ) -> Result<Result<BillingPlan, String>, String> {
        let plan = match validate_plan_input(&input) {
            Ok(plan) => plan,
            Err(code) => return Ok(Err(code)),
        };
        if let Err(code) = self.validate_plan_groups(&plan.group_ids).await? {
            return Ok(Err(code));
        }
        let id = uuid::Uuid::new_v4().to_string();
        let now = Utc::now().to_rfc3339();
        let groups_json = serialize_group_ids_json(&plan.group_ids)?;
        let write = self.db.write().await;
        let tx = write.begin().await.map_err(|error| error.to_string())?;
        if self.plan_name_exists_on(&tx, None, &plan.name).await? {
            return Ok(Err("plan_name_exists".to_string()));
        }
        let insert = tx
            .execute(self.db.stmt(
                "INSERT INTO billing_plans (id, name, description, limit_5h_nano_usd, limit_24h_nano_usd, limit_7d_nano_usd, limit_30d_nano_usd, group_ids, multiplier, listed, created_at, updated_at) VALUES ($1, $2, $3, $4, $5, $6, $7, $8, $9, $10, $11, $11)",
                vec![
                    id.clone().into(),
                    plan.name.into(),
                    plan.description.into(),
                    plan.limits.five_hour.map(|value| value.to_string()).into(),
                    plan.limits.twenty_four_hour.map(|value| value.to_string()).into(),
                    plan.limits.seven_day.map(|value| value.to_string()).into(),
                    plan.limits.thirty_day.map(|value| value.to_string()).into(),
                    groups_json.into(),
                    plan.multiplier.canonical().into(),
                    SeaValue::Int(Some(if plan.listed { 1 } else { 0 })),
                    now.clone().into(),
                ],
            ))
            .await;
        if let Err(error) = insert {
            let message = error.to_string();
            if is_plan_name_unique_violation(&message) {
                return Ok(Err("plan_name_exists".to_string()));
            }
            return Err(message);
        }
        self.insert_prices_tx(&tx, &id, &plan.prices, &now).await?;
        tx.commit().await.map_err(|error| error.to_string())?;
        Ok(Ok(self
            .get_billing_plan_by_id(&id)
            .await?
            .expect("created plan must exist")))
    }

    pub async fn update_billing_plan(
        &self,
        plan_id: &str,
        input: BillingPlanInput,
    ) -> Result<Result<(), String>, String> {
        let plan = match validate_plan_input(&input) {
            Ok(plan) => plan,
            Err(code) => return Ok(Err(code)),
        };
        if let Err(code) = self.validate_plan_groups(&plan.group_ids).await? {
            return Ok(Err(code));
        }
        let write = self.db.write().await;
        let tx = write.begin().await.map_err(|error| error.to_string())?;
        let lock_suffix = if self.db.is_postgres() {
            " FOR UPDATE"
        } else {
            ""
        };
        if tx
            .query_one(self.db.stmt(
                &format!("SELECT id FROM billing_plans WHERE id = $1{lock_suffix}"),
                vec![plan_id.into()],
            ))
            .await
            .map_err(|error| error.to_string())?
            .is_none()
        {
            return Err("not_found".to_string());
        }
        if self
            .plan_name_exists_on(&tx, Some(plan_id), &plan.name)
            .await?
        {
            return Ok(Err("plan_name_exists".to_string()));
        }
        let now = Utc::now().to_rfc3339();
        let groups_json = serialize_group_ids_json(&plan.group_ids)?;
        let update = tx
            .execute(self.db.stmt(
                "UPDATE billing_plans SET name = $1, description = $2, limit_5h_nano_usd = $3, limit_24h_nano_usd = $4, limit_7d_nano_usd = $5, limit_30d_nano_usd = $6, group_ids = $7, multiplier = $8, listed = $9, updated_at = $10 WHERE id = $11",
                vec![
                    plan.name.into(),
                    plan.description.into(),
                    plan.limits.five_hour.map(|value| value.to_string()).into(),
                    plan.limits.twenty_four_hour.map(|value| value.to_string()).into(),
                    plan.limits.seven_day.map(|value| value.to_string()).into(),
                    plan.limits.thirty_day.map(|value| value.to_string()).into(),
                    groups_json.into(),
                    plan.multiplier.canonical().into(),
                    SeaValue::Int(Some(if plan.listed { 1 } else { 0 })),
                    now.clone().into(),
                    plan_id.into(),
                ],
            ))
            .await;
        if let Err(error) = update {
            let message = error.to_string();
            if is_plan_name_unique_violation(&message) {
                return Ok(Err("plan_name_exists".to_string()));
            }
            return Err(message);
        }
        tx.execute(self.db.stmt(
            "DELETE FROM billing_plan_prices WHERE plan_id = $1",
            vec![plan_id.into()],
        ))
        .await
        .map_err(|error| error.to_string())?;
        self.insert_prices_tx(&tx, plan_id, &plan.prices, &now)
            .await?;
        tx.commit().await.map_err(|error| error.to_string())?;
        Ok(Ok(()))
    }

    pub async fn delete_billing_plan(&self, plan_id: &str) -> Result<(), String> {
        let write = self.db.write().await;
        let tx = write.begin().await.map_err(|error| error.to_string())?;
        let deleted = tx
            .execute(self.db.stmt(
                "DELETE FROM billing_plans WHERE id = $1",
                vec![plan_id.into()],
            ))
            .await
            .map_err(|error| error.to_string())?;
        if deleted.rows_affected() == 0 {
            return Err("not_found".to_string());
        }
        tx.execute(self.db.stmt(
            "DELETE FROM billing_plan_prices WHERE plan_id = $1",
            vec![plan_id.into()],
        ))
        .await
        .map_err(|error| error.to_string())?;
        tx.commit().await.map_err(|error| error.to_string())?;
        Ok(())
    }

    async fn active_subscription_on<C: ConnectionTrait>(
        &self,
        connection: &C,
        user_id: &str,
        now: DateTime<Utc>,
        lock: bool,
    ) -> Result<Option<SubscriptionSnapshot>, String> {
        let lock_suffix = if lock && self.db.is_postgres() {
            " FOR UPDATE"
        } else {
            ""
        };
        let row = connection
            .query_one(self.db.stmt(
                &format!("SELECT {SUBSCRIPTION_COLUMNS} FROM billing_plan_subscriptions WHERE user_id = $1 AND starts_at <= $2 AND expires_at > $2 AND revoked_at IS NULL ORDER BY expires_at DESC, id ASC LIMIT 1{lock_suffix}"),
                vec![user_id.into(), now.to_rfc3339().into()],
            ))
            .await
            .map_err(|error| error.to_string())?;
        row.as_ref().map(row_to_subscription).transpose()
    }

    async fn usage_snapshot_on<C: ConnectionTrait>(
        &self,
        connection: &C,
        subscription: &SubscriptionSnapshot,
        now: DateTime<Utc>,
    ) -> Result<PlanUsageSnapshot, String> {
        let oldest = now
            .checked_sub_signed(Duration::seconds(subscription.limits.max_window_seconds()))
            .ok_or_else(|| "plan window timestamp overflow".to_string())?;
        let rows = connection
            .query_all(self.db.stmt(
                "SELECT amount_nano_usd, created_at FROM billing_plan_usage WHERE subscription_id = $1 AND created_at > $2 AND created_at <= $3 ORDER BY created_at ASC",
                vec![
                    subscription.id.clone().into(),
                    oldest.to_rfc3339().into(),
                    now.to_rfc3339().into(),
                ],
            ))
            .await
            .map_err(|error| error.to_string())?;
        let mut usage = PlanUsageSnapshot::default();
        for row in rows {
            let amount = parse_nano_usd(
                &row.try_get::<String>("", "amount_nano_usd")
                    .map_err(sql_err)?,
            )
            .map_err(sql_err)?;
            if amount <= 0 {
                return Err(sql_err("plan usage amount must be positive"));
            }
            let created_at = parse_time(&row, "created_at")?;
            for (index, (seconds, limit)) in subscription.limits.values().into_iter().enumerate() {
                if limit.is_some()
                    && created_at
                        > now
                            .checked_sub_signed(Duration::seconds(seconds))
                            .ok_or_else(|| "plan window timestamp overflow".to_string())?
                {
                    usage.sums[index] = usage.sums[index]
                        .checked_add(amount)
                        .ok_or_else(|| "plan usage sum overflow".to_string())?;
                    let reset_at = created_at
                        .checked_add_signed(Duration::seconds(seconds))
                        .ok_or_else(|| "plan window timestamp overflow".to_string())?;
                    usage.next_reset_at[index] = Some(
                        usage.next_reset_at[index]
                            .map_or(reset_at, |current| current.min(reset_at)),
                    );
                }
            }
        }
        Ok(usage)
    }

    fn remaining_capacity(
        subscription: &SubscriptionSnapshot,
        sums: [i128; 4],
    ) -> Result<i128, String> {
        subscription
            .limits
            .values()
            .into_iter()
            .enumerate()
            .filter_map(|(index, (_, limit))| limit.map(|limit| (index, limit)))
            .map(|(index, limit)| {
                limit
                    .checked_sub(sums[index])
                    .map(|value| value.max(0))
                    .ok_or_else(|| "plan remaining capacity overflow".to_string())
            })
            .collect::<Result<Vec<_>, _>>()?
            .into_iter()
            .min()
            .ok_or_else(|| "subscription has no configured limit".to_string())
    }

    fn window_usage(
        limit: Option<i128>,
        used: i128,
        next_reset_at: Option<DateTime<Utc>>,
    ) -> Result<Option<PlanWindowUsage>, String> {
        limit
            .map(|limit| {
                let remaining = limit
                    .checked_sub(used)
                    .ok_or_else(|| "plan remaining capacity overflow".to_string())?
                    .max(0);
                Ok(PlanWindowUsage {
                    limit_nano_usd: limit.to_string(),
                    used_nano_usd: used.to_string(),
                    remaining_nano_usd: remaining.to_string(),
                    next_reset_at,
                })
            })
            .transpose()
    }

    async fn subscription_response_on<C: ConnectionTrait>(
        &self,
        connection: &C,
        subscription: SubscriptionSnapshot,
        now: DateTime<Utc>,
    ) -> Result<BillingPlanSubscription, String> {
        let usage = self
            .usage_snapshot_on(connection, &subscription, now)
            .await?;
        Ok(BillingPlanSubscription {
            id: subscription.id,
            user_id: subscription.user_id,
            plan_id: subscription.plan_id,
            price_id: subscription.price_id,
            plan_name: subscription.plan_name,
            plan_description: subscription.plan_description,
            group_ids: subscription.group_ids,
            multiplier: subscription.multiplier.canonical(),
            price_nano_usd: subscription.price_nano_usd.to_string(),
            starts_at: subscription.starts_at,
            expires_at: subscription.expires_at,
            windows: BillingPlanUsageWindows {
                five_hour: Self::window_usage(
                    subscription.limits.five_hour,
                    usage.sums[0],
                    usage.next_reset_at[0],
                )?,
                twenty_four_hour: Self::window_usage(
                    subscription.limits.twenty_four_hour,
                    usage.sums[1],
                    usage.next_reset_at[1],
                )?,
                seven_day: Self::window_usage(
                    subscription.limits.seven_day,
                    usage.sums[2],
                    usage.next_reset_at[2],
                )?,
                thirty_day: Self::window_usage(
                    subscription.limits.thirty_day,
                    usage.sums[3],
                    usage.next_reset_at[3],
                )?,
            },
        })
    }

    pub async fn get_active_billing_plan_subscription(
        &self,
        user_id: &str,
    ) -> Result<Option<BillingPlanSubscription>, String> {
        let now = Utc::now();
        let read = self.db.read();
        let Some(subscription) = self
            .active_subscription_on(&*read, user_id, now, false)
            .await?
        else {
            return Ok(None);
        };
        self.subscription_response_on(&*read, subscription, now)
            .await
            .map(Some)
    }

    pub async fn purchase_billing_plan(
        &self,
        user_id: &str,
        plan_id: &str,
        price_id: &str,
    ) -> Result<Result<BillingPlanSubscription, String>, String> {
        let now = Utc::now();
        let now_raw = now.to_rfc3339();
        let write = self.db.write().await;
        let tx = write.begin().await.map_err(|error| error.to_string())?;
        let user_lock = if self.db.is_postgres() {
            " FOR UPDATE"
        } else {
            ""
        };
        let user_row = tx
            .query_one(self.db.stmt(
                &format!("SELECT balance_nano_usd, balance_unlimited, enabled FROM users WHERE id = $1{user_lock}"),
                vec![user_id.into()],
            ))
            .await
            .map_err(|error| error.to_string())?
            .ok_or_else(|| "not_found".to_string())?;
        if !super::store::decode_required_bool(&user_row, "enabled")? {
            return Ok(Err("user_disabled".to_string()));
        }
        if self
            .active_subscription_on(&tx, user_id, now, true)
            .await?
            .is_some()
        {
            return Ok(Err("active_subscription_exists".to_string()));
        }
        let plan_row = tx
            .query_one(self.db.stmt(
                &format!("SELECT {PLAN_COLUMNS} FROM billing_plans WHERE id = $1 AND listed = 1"),
                vec![plan_id.into()],
            ))
            .await
            .map_err(|error| error.to_string())?;
        let Some(plan_row) = plan_row else {
            return Ok(Err("plan_not_available".to_string()));
        };
        let price_row = tx
            .query_one(self.db.stmt(
                "SELECT id, price_nano_usd, duration_seconds FROM billing_plan_prices WHERE id = $1 AND plan_id = $2",
                vec![price_id.into(), plan_id.into()],
            ))
            .await
            .map_err(|error| error.to_string())?;
        let Some(price_row) = price_row else {
            return Ok(Err("plan_not_available".to_string()));
        };
        let price = parse_nano_usd(
            &price_row
                .try_get::<String>("", "price_nano_usd")
                .map_err(sql_err)?,
        )
        .map_err(sql_err)?;
        let duration_seconds: i64 = price_row.try_get("", "duration_seconds").map_err(sql_err)?;
        let expires_at = now
            .checked_add_signed(Duration::seconds(duration_seconds))
            .ok_or_else(|| "plan expiry overflow".to_string())?;
        let unlimited = super::store::decode_required_bool(&user_row, "balance_unlimited")?;
        let old_balance = parse_nano_usd(
            &user_row
                .try_get::<String>("", "balance_nano_usd")
                .map_err(sql_err)?,
        )
        .map_err(sql_err)?;
        let new_balance = if unlimited {
            old_balance
        } else {
            let value = old_balance
                .checked_sub(price)
                .ok_or_else(|| "prepaid balance subtraction overflow".to_string())?;
            if value < 0 {
                return Ok(Err("insufficient_balance".to_string()));
            }
            tx.execute(self.db.stmt(
                "UPDATE users SET balance_nano_usd = $1, updated_at = $2 WHERE id = $3",
                vec![
                    value.to_string().into(),
                    now_raw.clone().into(),
                    user_id.into(),
                ],
            ))
            .await
            .map_err(|error| error.to_string())?;
            value
        };
        let subscription_id = uuid::Uuid::new_v4().to_string();
        let plan_name: String = plan_row.try_get("", "name").map_err(sql_err)?;
        let plan_description: String = plan_row.try_get("", "description").map_err(sql_err)?;
        let groups_json: String = plan_row.try_get("", "group_ids").map_err(sql_err)?;
        let multiplier: String = plan_row.try_get("", "multiplier").map_err(sql_err)?;
        tx.execute(self.db.stmt(
            "INSERT INTO billing_plan_subscriptions (id, user_id, plan_id, price_id, plan_name, plan_description, limit_5h_nano_usd, limit_24h_nano_usd, limit_7d_nano_usd, limit_30d_nano_usd, group_ids, multiplier, price_nano_usd, starts_at, expires_at, created_at) VALUES ($1, $2, $3, $4, $5, $6, $7, $8, $9, $10, $11, $12, $13, $14, $15, $14)",
            vec![
                subscription_id.clone().into(),
                user_id.into(),
                plan_id.into(),
                price_id.into(),
                plan_name.clone().into(),
                plan_description.into(),
                plan_row.try_get::<Option<String>>("", "limit_5h_nano_usd").map_err(sql_err)?.into(),
                plan_row.try_get::<Option<String>>("", "limit_24h_nano_usd").map_err(sql_err)?.into(),
                plan_row.try_get::<Option<String>>("", "limit_7d_nano_usd").map_err(sql_err)?.into(),
                plan_row.try_get::<Option<String>>("", "limit_30d_nano_usd").map_err(sql_err)?.into(),
                groups_json.into(),
                multiplier.into(),
                price.to_string().into(),
                now_raw.clone().into(),
                expires_at.to_rfc3339().into(),
            ],
        ))
        .await
        .map_err(|error| error.to_string())?;
        self.insert_billing_ledger_tx(
            &tx,
            user_id,
            "plan_purchase",
            if unlimited { 0 } else { -price },
            Some(new_balance),
            &serde_json::json!({
                "plan_id": plan_id,
                "plan_name": plan_name,
                "price_id": price_id,
                "subscription_id": subscription_id,
                "duration_seconds": duration_seconds,
                "expires_at": expires_at.to_rfc3339(),
            }),
            &now_raw,
        )
        .await
        .map_err(|error| error.message)?;
        tx.commit().await.map_err(|error| error.to_string())?;
        self.balance_cache.invalidate(user_id);
        self.get_active_billing_plan_subscription(user_id)
            .await?
            .ok_or_else(|| "created subscription is not active".to_string())
            .map(Ok)
    }

    pub async fn assign_billing_plan_subscription(
        &self,
        user_id: &str,
        plan_id: &str,
        price_id: &str,
    ) -> Result<Result<BillingPlanSubscription, String>, String> {
        let now = Utc::now();
        let now_raw = now.to_rfc3339();
        let write = self.db.write().await;
        let tx = write.begin().await.map_err(|error| error.to_string())?;
        let user_lock = if self.db.is_postgres() {
            " FOR UPDATE"
        } else {
            ""
        };
        if tx
            .query_one(self.db.stmt(
                &format!("SELECT id FROM users WHERE id = $1{user_lock}"),
                vec![user_id.into()],
            ))
            .await
            .map_err(|error| error.to_string())?
            .is_none()
        {
            return Ok(Err("not_found".to_string()));
        }

        let plan_row = tx
            .query_one(self.db.stmt(
                &format!("SELECT {PLAN_COLUMNS} FROM billing_plans WHERE id = $1"),
                vec![plan_id.into()],
            ))
            .await
            .map_err(|error| error.to_string())?;
        let Some(plan_row) = plan_row else {
            return Ok(Err("plan_not_available".to_string()));
        };
        let price_row = tx
            .query_one(self.db.stmt(
                "SELECT id, price_nano_usd, duration_seconds FROM billing_plan_prices WHERE id = $1 AND plan_id = $2",
                vec![price_id.into(), plan_id.into()],
            ))
            .await
            .map_err(|error| error.to_string())?;
        let Some(price_row) = price_row else {
            return Ok(Err("plan_not_available".to_string()));
        };
        let price_raw: String = price_row.try_get("", "price_nano_usd").map_err(sql_err)?;
        let price = parse_nano_usd(&price_raw).map_err(sql_err)?;
        if price <= 0 || price.to_string() != price_raw {
            return Err(sql_err("invalid stored plan price"));
        }
        let duration_seconds: i64 = price_row.try_get("", "duration_seconds").map_err(sql_err)?;
        if duration_seconds <= 0 {
            return Err(sql_err("invalid stored plan duration"));
        }
        let expires_at = now
            .checked_add_signed(Duration::seconds(duration_seconds))
            .ok_or_else(|| "plan expiry overflow".to_string())?;

        if let Some(active) = self.active_subscription_on(&tx, user_id, now, true).await? {
            tx.execute(self.db.stmt(
                "UPDATE billing_plan_subscriptions SET revoked_at = $1 WHERE id = $2 AND revoked_at IS NULL",
                vec![now_raw.clone().into(), active.id.into()],
            ))
            .await
            .map_err(|error| error.to_string())?;
        }

        let subscription_id = uuid::Uuid::new_v4().to_string();
        tx.execute(self.db.stmt(
            "INSERT INTO billing_plan_subscriptions (id, user_id, plan_id, price_id, plan_name, plan_description, limit_5h_nano_usd, limit_24h_nano_usd, limit_7d_nano_usd, limit_30d_nano_usd, group_ids, multiplier, price_nano_usd, starts_at, expires_at, created_at, revoked_at) VALUES ($1, $2, $3, $4, $5, $6, $7, $8, $9, $10, $11, $12, $13, $14, $15, $14, NULL)",
            vec![
                subscription_id.into(),
                user_id.into(),
                plan_id.into(),
                price_id.into(),
                plan_row.try_get::<String>("", "name").map_err(sql_err)?.into(),
                plan_row.try_get::<String>("", "description").map_err(sql_err)?.into(),
                plan_row.try_get::<Option<String>>("", "limit_5h_nano_usd").map_err(sql_err)?.into(),
                plan_row.try_get::<Option<String>>("", "limit_24h_nano_usd").map_err(sql_err)?.into(),
                plan_row.try_get::<Option<String>>("", "limit_7d_nano_usd").map_err(sql_err)?.into(),
                plan_row.try_get::<Option<String>>("", "limit_30d_nano_usd").map_err(sql_err)?.into(),
                plan_row.try_get::<String>("", "group_ids").map_err(sql_err)?.into(),
                plan_row.try_get::<String>("", "multiplier").map_err(sql_err)?.into(),
                price.to_string().into(),
                now_raw.into(),
                expires_at.to_rfc3339().into(),
            ],
        ))
        .await
        .map_err(|error| error.to_string())?;
        tx.commit().await.map_err(|error| error.to_string())?;

        self.get_active_billing_plan_subscription(user_id)
            .await?
            .ok_or_else(|| "created subscription is not active".to_string())
            .map(Ok)
    }

    pub async fn revoke_billing_plan_subscription(
        &self,
        user_id: &str,
    ) -> Result<Result<(), String>, String> {
        let now = Utc::now();
        let now_raw = now.to_rfc3339();
        let write = self.db.write().await;
        let tx = write.begin().await.map_err(|error| error.to_string())?;
        let user_lock = if self.db.is_postgres() {
            " FOR UPDATE"
        } else {
            ""
        };
        if tx
            .query_one(self.db.stmt(
                &format!("SELECT id FROM users WHERE id = $1{user_lock}"),
                vec![user_id.into()],
            ))
            .await
            .map_err(|error| error.to_string())?
            .is_none()
        {
            return Ok(Err("not_found".to_string()));
        }
        if let Some(active) = self.active_subscription_on(&tx, user_id, now, true).await? {
            tx.execute(self.db.stmt(
                "UPDATE billing_plan_subscriptions SET revoked_at = $1 WHERE id = $2 AND revoked_at IS NULL",
                vec![now_raw.into(), active.id.into()],
            ))
            .await
            .map_err(|error| error.to_string())?;
        }
        tx.commit().await.map_err(|error| error.to_string())?;
        Ok(Ok(()))
    }

    pub async fn has_plan_capacity_for_groups(
        &self,
        user_id: &str,
        api_key_id: Option<&str>,
        billing_group_ids: &[String],
        pending_nano_usd: i128,
    ) -> Result<bool, String> {
        if api_key_id.is_none() || billing_group_ids.is_empty() {
            return Ok(false);
        }
        let now = Utc::now();
        let read = self.db.read();
        let Some(subscription) = self
            .active_subscription_on(&*read, user_id, now, false)
            .await?
        else {
            return Ok(false);
        };
        if !billing_group_ids
            .iter()
            .any(|group| subscription.group_ids.contains(group))
        {
            return Ok(false);
        }
        let usage = self.usage_snapshot_on(&*read, &subscription, now).await?;
        let remaining = Self::remaining_capacity(&subscription, usage.sums)?;
        let pending = subscription
            .multiplier
            .checked_scale_i128(pending_nano_usd.max(0))
            .ok_or_else(|| "pending plan charge overflow".to_string())?;
        Ok(remaining > pending)
    }

    pub(crate) async fn allocate_plan_charge_tx(
        &self,
        tx: &DatabaseTransaction,
        user_id: &str,
        api_key_id: Option<&str>,
        billing_group_id: Option<&str>,
        request_id: &str,
        settled_charge_nano_usd: i128,
        now: DateTime<Utc>,
    ) -> Result<PlanChargeAllocation, BillingError> {
        if settled_charge_nano_usd <= 0 {
            return Ok(PlanChargeAllocation {
                adjusted_charge_nano_usd: 0,
                plan_covered_nano_usd: 0,
                fallback_nano_usd: 0,
                subscription_id: None,
                plan_id: None,
                multiplier: None,
            });
        }
        let Some(api_key_id) = api_key_id else {
            return Ok(PlanChargeAllocation {
                adjusted_charge_nano_usd: settled_charge_nano_usd,
                plan_covered_nano_usd: 0,
                fallback_nano_usd: settled_charge_nano_usd,
                subscription_id: None,
                plan_id: None,
                multiplier: None,
            });
        };
        let Some(group_id) = billing_group_id else {
            return Ok(PlanChargeAllocation {
                adjusted_charge_nano_usd: settled_charge_nano_usd,
                plan_covered_nano_usd: 0,
                fallback_nano_usd: settled_charge_nano_usd,
                subscription_id: None,
                plan_id: None,
                multiplier: None,
            });
        };
        let subscription = self
            .active_subscription_on(tx, user_id, now, true)
            .await
            .map_err(|error| BillingError::new(BillingErrorKind::Internal, error))?;
        let Some(subscription) = subscription else {
            return Ok(PlanChargeAllocation {
                adjusted_charge_nano_usd: settled_charge_nano_usd,
                plan_covered_nano_usd: 0,
                fallback_nano_usd: settled_charge_nano_usd,
                subscription_id: None,
                plan_id: None,
                multiplier: None,
            });
        };
        if !subscription.group_ids.iter().any(|id| id == group_id) {
            return Ok(PlanChargeAllocation {
                adjusted_charge_nano_usd: settled_charge_nano_usd,
                plan_covered_nano_usd: 0,
                fallback_nano_usd: settled_charge_nano_usd,
                subscription_id: None,
                plan_id: None,
                multiplier: None,
            });
        }
        let adjusted = subscription
            .multiplier
            .checked_scale_i128(settled_charge_nano_usd)
            .ok_or_else(|| {
                BillingError::new(BillingErrorKind::Overflow, "plan multiplier overflow")
            })?;
        let usage = self
            .usage_snapshot_on(tx, &subscription, now)
            .await
            .map_err(|error| BillingError::new(BillingErrorKind::Internal, error))?;
        let remaining = Self::remaining_capacity(&subscription, usage.sums)
            .map_err(|error| BillingError::new(BillingErrorKind::Internal, error))?;
        let covered = adjusted.min(remaining);
        let fallback = adjusted.checked_sub(covered).ok_or_else(|| {
            BillingError::new(BillingErrorKind::Overflow, "plan allocation overflow")
        })?;
        if covered > 0 {
            tx.execute(self.db.stmt(
                "INSERT INTO billing_plan_usage (id, subscription_id, user_id, api_key_id, request_id, group_id, amount_nano_usd, created_at) VALUES ($1, $2, $3, $4, $5, $6, $7, $8)",
                vec![
                    uuid::Uuid::new_v4().to_string().into(),
                    subscription.id.clone().into(),
                    user_id.into(),
                    api_key_id.into(),
                    request_id.into(),
                    group_id.into(),
                    covered.to_string().into(),
                    now.to_rfc3339().into(),
                ],
            ))
            .await
            .map_err(|error| BillingError::new(BillingErrorKind::Internal, error.to_string()))?;
        }
        Ok(PlanChargeAllocation {
            adjusted_charge_nano_usd: adjusted,
            plan_covered_nano_usd: covered,
            fallback_nano_usd: fallback,
            subscription_id: Some(subscription.id),
            plan_id: Some(subscription.plan_id),
            multiplier: Some(subscription.multiplier.canonical()),
        })
    }
}
