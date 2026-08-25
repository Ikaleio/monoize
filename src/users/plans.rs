use super::store::{MAX_GROUP_IDS, parse_group_ids_json, serialize_group_ids_json};
use crate::users::{UserStore, canonicalize_group_ids, parse_nano_usd};
use chrono::{DateTime, Utc};
use chrono_tz::Asia::Shanghai;
use cron::Schedule;
use sea_orm::{ConnectionTrait, TransactionTrait, Value as SeaValue};
use serde::{Deserialize, Serialize};
use std::str::FromStr;

const DEFAULT_PLAN_GRANT_TICK_INTERVAL_SECS: u64 = 60;

pub fn plan_grant_tick_interval() -> std::time::Duration {
    static INTERVAL: std::sync::OnceLock<std::time::Duration> = std::sync::OnceLock::new();
    *INTERVAL.get_or_init(|| {
        let parsed = std::env::var("MONOIZE_PLAN_GRANT_TICK_INTERVAL_SECS")
            .ok()
            .as_deref()
            .map(str::trim)
            .and_then(|value| value.parse::<u64>().ok())
            .filter(|value| *value > 0);
        std::time::Duration::from_secs(parsed.unwrap_or(DEFAULT_PLAN_GRANT_TICK_INTERVAL_SECS))
    })
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct BillingPlan {
    pub id: String,
    pub name: String,
    /// Signed integer nano-dollar string; balance resets to this amount each period.
    pub grant_amount_nano_usd: String,
    /// Canonical 5-field Unix cron; evaluated in Asia/Shanghai.
    pub schedule: String,
    /// Group-id restriction layer (`groups-registry.spec.md` §1.1); empty = unrestricted.
    pub group_ids: Vec<String>,
    pub enabled: bool,
    pub created_at: DateTime<Utc>,
    pub updated_at: DateTime<Utc>,
}

#[derive(Debug, Clone, Deserialize)]
pub struct BillingPlanInput {
    pub name: String,
    #[serde(default)]
    pub grant_amount_nano_usd: Option<String>,
    #[serde(default)]
    pub grant_amount_usd: Option<String>,
    pub schedule: String,
    #[serde(default)]
    pub group_ids: Option<Vec<String>>,
    #[serde(default)]
    pub enabled: Option<bool>,
}

struct ValidatedPlan {
    name: String,
    amount: i128,
    schedule: String,
}

pub(crate) fn canonicalize_plan_schedule(raw: &str) -> Result<String, String> {
    let fields: Vec<&str> = raw.split_whitespace().collect();
    if fields.len() != 5 || fields.iter().any(|field| field.is_empty()) {
        return Err("invalid_schedule".to_string());
    }
    Ok(fields.join(" "))
}

fn map_unix_dow_atom(atom: &str) -> String {
    match atom {
        "0" | "7" => "1".to_string(),
        "1" => "2".to_string(),
        "2" => "3".to_string(),
        "3" => "4".to_string(),
        "4" => "5".to_string(),
        "5" => "6".to_string(),
        "6" => "7".to_string(),
        other => other.to_string(),
    }
}

fn map_unix_dow_field(field: &str) -> String {
    field
        .split(',')
        .map(|part| match part.split_once('-') {
            Some((start, end)) => {
                format!("{}-{}", map_unix_dow_atom(start), map_unix_dow_atom(end))
            }
            None => map_unix_dow_atom(part),
        })
        .collect::<Vec<_>>()
        .join(",")
}

fn cron_schedule(canonical: &str) -> Result<Schedule, String> {
    let fields: Vec<&str> = canonical.split_whitespace().collect();
    if fields.len() != 5 {
        return Err("invalid_schedule".to_string());
    }
    let expression = format!(
        "0 {} {} {} {} {}",
        fields[0],
        fields[1],
        fields[2],
        fields[3],
        map_unix_dow_field(fields[4])
    );
    Schedule::from_str(&expression).map_err(|_| "invalid_schedule".to_string())
}

pub(crate) fn next_grant_after(
    canonical: &str,
    from: DateTime<Utc>,
) -> Result<DateTime<Utc>, String> {
    let schedule = cron_schedule(canonical)?;
    let from_local = from.with_timezone(&Shanghai);
    schedule
        .after(&from_local)
        .next()
        .map(|when| when.with_timezone(&Utc))
        .ok_or_else(|| "invalid_schedule".to_string())
}

fn map_grant_amount_error(_: String) -> String {
    "invalid_grant_amount".to_string()
}

fn validate_plan_input(input: &BillingPlanInput) -> Result<ValidatedPlan, String> {
    let name = input.name.trim();
    if name.is_empty() || name.chars().count() > 100 {
        return Err("invalid_plan_name".to_string());
    }
    let schedule = canonicalize_plan_schedule(&input.schedule)?;
    next_grant_after(&schedule, Utc::now())?;
    let amount = if let Some(raw) = input.grant_amount_nano_usd.as_deref() {
        let parsed = parse_nano_usd(raw).map_err(map_grant_amount_error)?;
        if raw.trim() != parsed.to_string() || parsed < 0 {
            return Err("invalid_grant_amount".to_string());
        }
        parsed
    } else if let Some(raw) = input.grant_amount_usd.as_deref() {
        let parsed = super::utils::parse_usd_to_nano(raw).map_err(map_grant_amount_error)?;
        if parsed < 0 {
            return Err("invalid_grant_amount".to_string());
        }
        parsed
    } else {
        return Err("invalid_grant_amount".to_string());
    };

    Ok(ValidatedPlan {
        name: name.to_string(),
        amount,
        schedule,
    })
}

fn is_plan_name_unique_violation(error: &str) -> bool {
    let lower = error.to_ascii_lowercase();
    (lower.contains("unique") || lower.contains("duplicate"))
        && (lower.contains("name") || lower.contains("uq_billing_plans_name_lower"))
}

fn plan_lock_sql(is_postgres: bool) -> &'static str {
    if is_postgres {
        "SELECT id, name, grant_amount_nano_usd, schedule, group_ids, enabled, created_at, updated_at FROM billing_plans WHERE id = $1 FOR UPDATE"
    } else {
        "SELECT id, name, grant_amount_nano_usd, schedule, group_ids, enabled, created_at, updated_at FROM billing_plans WHERE id = $1"
    }
}

fn sql_err<E: std::fmt::Display>(error: E) -> String {
    format!("invalid persisted billing plan data: {error}")
}

fn row_to_plan(row: &sea_orm::QueryResult) -> Result<BillingPlan, String> {
    let enabled = super::store::decode_required_bool(row, "enabled")?;
    let group_ids_raw: String = row.try_get("", "group_ids").map_err(sql_err)?;
    Ok(BillingPlan {
        id: row.try_get("", "id").map_err(sql_err)?,
        name: row.try_get("", "name").map_err(sql_err)?,
        grant_amount_nano_usd: row.try_get("", "grant_amount_nano_usd").map_err(sql_err)?,
        schedule: row.try_get("", "schedule").map_err(sql_err)?,
        group_ids: parse_group_ids_json(Some(group_ids_raw.as_str()), "billing_plans.group_ids")?,
        enabled,
        created_at: DateTime::parse_from_rfc3339(
            &row.try_get::<String>("", "created_at").map_err(sql_err)?,
        )
        .map(|d| d.with_timezone(&Utc))
        .map_err(sql_err)?,
        updated_at: DateTime::parse_from_rfc3339(
            &row.try_get::<String>("", "updated_at").map_err(sql_err)?,
        )
        .map(|d| d.with_timezone(&Utc))
        .map_err(sql_err)?,
    })
}

impl UserStore {
    /// GR-C2/GR-C3 for the plan ceiling: bounded length and every id registered.
    /// Outer `Err` = storage failure; inner `Err` = client-mappable code.
    async fn validate_plan_group_ids(
        &self,
        group_ids: &[String],
    ) -> Result<Result<(), String>, String> {
        if group_ids.len() > MAX_GROUP_IDS {
            return Ok(Err("invalid_request".to_string()));
        }
        if self.find_unknown_group_id(group_ids).await?.is_some() {
            return Ok(Err("invalid_request".to_string()));
        }
        Ok(Ok(()))
    }

    pub async fn list_billing_plans(&self) -> Result<Vec<BillingPlan>, String> {
        let rows = self
            .db
            .read()
            .query_all(self.db.stmt(
                "SELECT id, name, grant_amount_nano_usd, schedule, group_ids, enabled, created_at, updated_at FROM billing_plans ORDER BY created_at ASC",
                vec![],
            ))
            .await
            .map_err(|e| e.to_string())?;
        rows.iter().map(row_to_plan).collect()
    }

    pub async fn get_billing_plan_by_id(&self, id: &str) -> Result<Option<BillingPlan>, String> {
        let row = self
            .db
            .read()
            .query_one(self.db.stmt(
                "SELECT id, name, grant_amount_nano_usd, schedule, group_ids, enabled, created_at, updated_at FROM billing_plans WHERE id = $1",
                vec![id.into()],
            ))
            .await
            .map_err(|e| e.to_string())?;
        match row {
            Some(row) => Ok(Some(row_to_plan(&row)?)),
            None => Ok(None),
        }
    }

    pub async fn create_billing_plan(
        &self,
        input: BillingPlanInput,
    ) -> Result<Result<BillingPlan, String>, String> {
        let plan = match validate_plan_input(&input) {
            Ok(plan) => plan,
            Err(code) => return Ok(Err(code)),
        };
        let group_ids = canonicalize_group_ids(input.group_ids.as_deref().unwrap_or(&[]));
        if let Err(code) = self.validate_plan_group_ids(&group_ids).await? {
            return Ok(Err(code));
        }
        let enabled = input.enabled.unwrap_or(true);
        let groups_json = serialize_group_ids_json(&group_ids)?;

        let id = uuid::Uuid::new_v4().to_string();
        let now = Utc::now().to_rfc3339();
        {
            let write = self.db.write().await;
            let tx = write.begin().await.map_err(|e| e.to_string())?;
            if self.plan_name_exists_on(&tx, None, &plan.name).await? {
                return Ok(Err("plan_name_exists".to_string()));
            }
            if let Err(error) = tx
                .execute(self.db.stmt(
                    "INSERT INTO billing_plans (id, name, grant_amount_nano_usd, schedule, group_ids, enabled, created_at, updated_at) VALUES ($1, $2, $3, $4, $5, $6, $7, $7)",
                    vec![
                        id.clone().into(),
                        plan.name.into(),
                        plan.amount.to_string().into(),
                        plan.schedule.into(),
                        groups_json.into(),
                        SeaValue::Int(Some(if enabled { 1 } else { 0 })),
                        now.into(),
                    ],
                ))
                .await
            {
                let msg = error.to_string();
                if is_plan_name_unique_violation(&msg) {
                    return Ok(Err("plan_name_exists".to_string()));
                }
                return Err(msg);
            }
            tx.commit().await.map_err(|e| e.to_string())?;
        }

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

        {
            let write = self.db.write().await;
            let tx = write.begin().await.map_err(|e| e.to_string())?;
            let existing_row = tx
                .query_one(
                    self.db
                        .stmt(plan_lock_sql(self.db.is_postgres()), vec![plan_id.into()]),
                )
                .await
                .map_err(|e| e.to_string())?;
            let existing = match existing_row {
                Some(row) => row_to_plan(&row)?,
                None => return Err("not_found".to_string()),
            };
            if self
                .plan_name_exists_on(&tx, Some(plan_id), &plan.name)
                .await?
            {
                return Ok(Err("plan_name_exists".to_string()));
            }

            let group_ids = match input.group_ids.as_deref() {
                Some(raw) => {
                    let group_ids = canonicalize_group_ids(raw);
                    if let Err(code) = self.validate_plan_group_ids(&group_ids).await? {
                        return Ok(Err(code));
                    }
                    group_ids
                }
                None => existing.group_ids,
            };
            let enabled = input.enabled.unwrap_or(existing.enabled);
            let groups_json = serialize_group_ids_json(&group_ids)?;

            // Plan edits affect only future evaluations; existing next_grant_at anchors stay.
            if let Err(error) = tx
                .execute(self.db.stmt(
                    "UPDATE billing_plans SET name = $1, grant_amount_nano_usd = $2, schedule = $3, group_ids = $4, enabled = $5, updated_at = $6 WHERE id = $7",
                    vec![
                        plan.name.into(),
                        plan.amount.to_string().into(),
                        plan.schedule.into(),
                        groups_json.into(),
                        SeaValue::Int(Some(if enabled { 1 } else { 0 })),
                        Utc::now().to_rfc3339().into(),
                        plan_id.into(),
                    ],
                ))
                .await
            {
                let msg = error.to_string();
                if is_plan_name_unique_violation(&msg) {
                    return Ok(Err("plan_name_exists".to_string()));
                }
                return Err(msg);
            }
            tx.commit().await.map_err(|e| e.to_string())?;
        }

        // Cached auth results embed the plan's group restriction layer.
        self.api_key_cache.invalidate_all();
        Ok(Ok(()))
    }

    pub async fn delete_billing_plan(&self, plan_id: &str) -> Result<Result<(), String>, String> {
        let write = self.db.write().await;
        let tx = write.begin().await.map_err(|e| e.to_string())?;
        let existing = tx
            .query_one(
                self.db
                    .stmt(plan_lock_sql(self.db.is_postgres()), vec![plan_id.into()]),
            )
            .await
            .map_err(|e| e.to_string())?;
        if existing.is_none() {
            return Err("not_found".to_string());
        }

        let count_row = tx
            .query_one(self.db.stmt(
                "SELECT COUNT(*) AS count FROM users WHERE billing_plan_id = $1",
                vec![plan_id.into()],
            ))
            .await
            .map_err(|e| e.to_string())?
            .ok_or_else(|| "no count row".to_string())?;
        let count: i64 = count_row.try_get("", "count").map_err(|e| e.to_string())?;
        if count > 0 {
            return Ok(Err("plan_in_use".to_string()));
        }

        tx.execute(self.db.stmt(
            "DELETE FROM billing_plans WHERE id = $1",
            vec![plan_id.into()],
        ))
        .await
        .map_err(|e| e.to_string())?;
        tx.commit().await.map_err(|e| e.to_string())?;
        Ok(Ok(()))
    }

    async fn plan_name_exists_on<C: ConnectionTrait>(
        &self,
        conn: &C,
        exclude_id: Option<&str>,
        name: &str,
    ) -> Result<bool, String> {
        let (sql, values) = match exclude_id {
            Some(exclude_id) => (
                "SELECT 1 AS one FROM billing_plans WHERE lower(name) = lower($1) AND id != $2 LIMIT 1",
                vec![name.into(), exclude_id.into()],
            ),
            None => (
                "SELECT 1 AS one FROM billing_plans WHERE lower(name) = lower($1) LIMIT 1",
                vec![name.into()],
            ),
        };
        let row = conn
            .query_one(self.db.stmt(sql, values))
            .await
            .map_err(|e| e.to_string())?;
        Ok(row.is_some())
    }

    pub fn spawn_plan_grant_scheduler(&self) {
        let store = self.clone();
        tokio::spawn(async move {
            loop {
                if let Err(error) = store.run_plan_grant_tick().await {
                    tracing::warn!(%error, "billing plan grant tick failed");
                }
                tokio::time::sleep(super::plans::plan_grant_tick_interval()).await;
            }
        });
    }

    pub async fn run_plan_grant_tick(&self) -> Result<usize, String> {
        let now = Utc::now();
        let due = self
            .db
            .read()
            .query_all(self.db.stmt(
                "SELECT u.id AS user_id FROM users u JOIN billing_plans p ON p.id = u.billing_plan_id WHERE u.billing_plan_id IS NOT NULL AND u.next_grant_at IS NOT NULL AND u.enabled = 1 AND u.balance_unlimited = 0 AND p.enabled = 1 AND (u.next_grant_at <= $1 OR NOT EXISTS (SELECT 1 FROM billing_ledger l WHERE l.user_id = u.id AND l.kind = 'plan_grant'))",
                vec![now.to_rfc3339().into()],
            ))
            .await
            .map_err(|e| e.to_string())?;

        let mut granted = 0usize;
        for row in &due {
            let user_id: String = row.try_get("", "user_id").map_err(sql_err).map_err(|e| e)?;
            match self.grant_user_once(&user_id, false).await {
                Ok(true) => granted += 1,
                Ok(false) => {}
                Err(error) => tracing::warn!(user_id = %user_id, %error, "plan grant failed"),
            }
        }
        Ok(granted)
    }

    pub async fn reset_billing_plan_grants(&self, plan_id: &str) -> Result<usize, String> {
        if self.get_billing_plan_by_id(plan_id).await?.is_none() {
            return Err("not_found".to_string());
        }

        let rows = self
            .db
            .read()
            .query_all(self.db.stmt(
                "SELECT id AS user_id FROM users WHERE billing_plan_id = $1 AND next_grant_at IS NOT NULL AND enabled = 1 AND balance_unlimited = 0",
                vec![plan_id.into()],
            ))
            .await
            .map_err(|e| e.to_string())?;

        let mut reset_count = 0usize;
        for row in &rows {
            let user_id: String = row.try_get("", "user_id").map_err(sql_err)?;
            match self.grant_user_once(&user_id, true).await {
                Ok(true) => reset_count += 1,
                Ok(false) => {}
                Err(error) => {
                    tracing::warn!(user_id = %user_id, plan_id = %plan_id, %error, "admin plan reset grant failed")
                }
            }
        }
        Ok(reset_count)
    }

    /// Applies at most one grant for the user (BP-G5 catch-up rule). Returns
    /// false when the locked state no longer satisfies the due conditions.
    /// `force` is the admin-reset path: skip due/plan-enabled checks and always
    /// rewrite `next_grant_at` (BP-A12).
    async fn grant_user_once(&self, user_id: &str, force: bool) -> Result<bool, String> {
        let execution_now = Utc::now();
        let write = self.db.write().await;
        let tx = write.begin().await.map_err(|e| e.to_string())?;

        let lock_sql = if self.db.is_postgres() {
            "SELECT u.balance_unlimited, u.enabled, u.balance_nano_usd, u.next_grant_at, p.id AS plan_id, p.name AS plan_name, p.grant_amount_nano_usd, p.schedule, p.enabled AS plan_enabled FROM users u JOIN billing_plans p ON p.id = u.billing_plan_id WHERE u.id = $1 FOR UPDATE OF u"
        } else {
            "SELECT u.balance_unlimited, u.enabled, u.balance_nano_usd, u.next_grant_at, p.id AS plan_id, p.name AS plan_name, p.grant_amount_nano_usd, p.schedule, p.enabled AS plan_enabled FROM users u JOIN billing_plans p ON p.id = u.billing_plan_id WHERE u.id = $1"
        };
        let row = tx
            .query_one(self.db.stmt(lock_sql, vec![user_id.into()]))
            .await
            .map_err(|e| e.to_string())?;
        let Some(row) = row else {
            return Ok(false);
        };

        let unlimited: i32 = row.try_get("", "balance_unlimited").map_err(sql_err)?;
        let enabled: i32 = row.try_get("", "enabled").map_err(sql_err)?;
        let plan_enabled: i32 = row.try_get("", "plan_enabled").map_err(sql_err)?;
        let raw_balance: String = row.try_get("", "balance_nano_usd").map_err(sql_err)?;
        let old_balance = parse_nano_usd(&raw_balance)?;
        let raw_next_grant_at: Option<String> =
            row.try_get("", "next_grant_at").map_err(sql_err)?;
        let plan_id: String = row.try_get("", "plan_id").map_err(sql_err)?;
        let plan_name: String = row.try_get("", "plan_name").map_err(sql_err)?;
        let raw_amount: String = row.try_get("", "grant_amount_nano_usd").map_err(sql_err)?;
        let amount = parse_nano_usd(&raw_amount)?;
        let schedule: String = row.try_get("", "schedule").map_err(sql_err)?;

        let Some(next_grant_raw) = raw_next_grant_at.as_deref() else {
            return Ok(false);
        };
        let next_grant_at = DateTime::parse_from_rfc3339(next_grant_raw)
            .map_err(sql_err)?
            .with_timezone(&Utc);
        if unlimited != 0 || enabled != 1 {
            return Ok(false);
        }
        if !force && plan_enabled != 1 {
            return Ok(false);
        }
        let due = next_grant_at <= execution_now;
        if !force && !due {
            let prior_grants: i64 = tx
                .query_one(self.db.stmt(
                    "SELECT COUNT(*) AS count FROM billing_ledger WHERE user_id = $1 AND kind = 'plan_grant'",
                    vec![user_id.into()],
                ))
                .await
                .map_err(|e| e.to_string())?
                .ok_or_else(|| "missing plan_grant count row".to_string())?
                .try_get("", "count")
                .map_err(sql_err)?;
            if prior_grants > 0 {
                return Ok(false);
            }
        }

        let new_balance = amount;
        let delta = new_balance
            .checked_sub(old_balance)
            .ok_or("balance overflow")?;
        if force || due {
            let next_anchor = next_grant_after(&schedule, execution_now)?.to_rfc3339();
            tx.execute(self.db.stmt(
                "UPDATE users SET balance_nano_usd = $1, next_grant_at = $2, updated_at = $3 WHERE id = $4",
                vec![
                    new_balance.to_string().into(),
                    next_anchor.into(),
                    execution_now.to_rfc3339().into(),
                    user_id.into(),
                ],
            ))
            .await
            .map_err(|e| e.to_string())?;
        } else {
            tx.execute(self.db.stmt(
                "UPDATE users SET balance_nano_usd = $1, updated_at = $2 WHERE id = $3",
                vec![
                    new_balance.to_string().into(),
                    execution_now.to_rfc3339().into(),
                    user_id.into(),
                ],
            ))
            .await
            .map_err(|e| e.to_string())?;
        }

        self.insert_billing_ledger_tx(
            &tx,
            user_id,
            "plan_grant",
            delta,
            Some(new_balance),
            &if force {
                serde_json::json!({
                    "plan_id": plan_id,
                    "plan_name": plan_name,
                    "source": "admin_reset",
                    "before_balance_nano_usd": old_balance.to_string(),
                    "after_balance_nano_usd": new_balance.to_string(),
                })
            } else {
                serde_json::json!({
                    "plan_id": plan_id,
                    "plan_name": plan_name,
                    "before_balance_nano_usd": old_balance.to_string(),
                    "after_balance_nano_usd": new_balance.to_string(),
                })
            },
            &execution_now.to_rfc3339(),
        )
        .await
        .map_err(|e| e.message)?;

        tx.commit().await.map_err(|e| e.to_string())?;
        self.balance_cache.invalidate(user_id);
        Ok(true)
    }
}

#[cfg(test)]
mod tests {
    use super::{BillingPlanInput, validate_plan_input};
    use crate::db::DbPool;
    use crate::migration::Migrator;
    use crate::users::{AdminUpdateUserInput, UserRole, UserStore, resolve_effective_groups};
    use chrono_tz::Asia::Shanghai;
    use sea_orm::ConnectionTrait;
    use sea_orm_migration::MigratorTrait;

    fn plan_input(name: &str, amount_usd: &str, schedule: &str) -> BillingPlanInput {
        BillingPlanInput {
            name: name.to_string(),
            grant_amount_nano_usd: None,
            grant_amount_usd: Some(amount_usd.to_string()),
            schedule: schedule.to_string(),
            group_ids: None,
            enabled: None,
        }
    }

    async fn make_group(store: &UserStore, name: &str) -> crate::users::Group {
        store
            .create_group(crate::users::CreateGroupInput {
                name: name.to_string(),
                description: String::new(),
                user_selectable: false,
                sort_order: 0,
            })
            .await
            .expect("group creates")
    }

    async fn make_store() -> UserStore {
        let db = DbPool::connect("sqlite::memory:")
            .await
            .expect("db connects");
        {
            let write = db.write().await;
            Migrator::up(&*write, None).await.expect("migrates");
        }
        let (log_tx, _) = tokio::sync::broadcast::channel(1);
        UserStore::new(db, log_tx).await.expect("store creates")
    }

    #[test]
    fn daily_shanghai_midnight_schedule_fires_at_next_local_midnight() {
        use chrono::{DateTime, Datelike, Timelike, Utc};
        let from = DateTime::parse_from_rfc3339("2026-08-23T10:15:00+08:00")
            .expect("parses")
            .with_timezone(&Utc);
        let next = super::next_grant_after("0 0 * * *", from).expect("next fire");
        let local = next.with_timezone(&Shanghai);
        assert_eq!(local.hour(), 0);
        assert_eq!(local.minute(), 0);
        assert_eq!(local.date_naive().to_string(), "2026-08-24");
        let sunday = super::next_grant_after(
            "0 0 * * 0",
            DateTime::parse_from_rfc3339("2026-08-23T10:15:00+08:00")
                .expect("parses")
                .with_timezone(&Utc),
        )
        .expect("unix sunday cron parses");
        assert_eq!(
            sunday.with_timezone(&Shanghai).weekday(),
            chrono::Weekday::Sun,
        );
    }

    #[test]
    fn plan_input_rejects_non_positive_period_and_negative_amounts() {
        let mut input = plan_input("p", "5", "not-a-cron");
        assert_eq!(
            validate_plan_input(&input).err(),
            Some("invalid_schedule".to_string())
        );
        input.schedule = "0 0 * *".to_string();
        assert_eq!(
            validate_plan_input(&input).err(),
            Some("invalid_schedule".to_string())
        );
        input.schedule = "0 0 * * *".to_string();
        input.grant_amount_usd = Some("-1".to_string());
        assert_eq!(
            validate_plan_input(&input).err(),
            Some("invalid_grant_amount".to_string())
        );
        input.grant_amount_usd = None;
        input.grant_amount_nano_usd = Some("abc".to_string());
        assert_eq!(
            validate_plan_input(&input).err(),
            Some("invalid_grant_amount".to_string())
        );
        input.grant_amount_nano_usd = None;
        assert_eq!(
            validate_plan_input(&input).err(),
            Some("invalid_grant_amount".to_string())
        );
        assert!(validate_plan_input(&plan_input("zero", "0", "* * * * *")).is_ok());
    }

    #[tokio::test]
    async fn plan_lifecycle_and_assignment_anchor() {
        let store = make_store().await;
        let plan = store
            .create_billing_plan(plan_input("starter", "1", "0 * * * *"))
            .await
            .expect("create succeeds")
            .expect("name is unique");
        assert_eq!(plan.grant_amount_nano_usd, "1000000000");
        assert_eq!(plan.schedule, "0 * * * *");
        assert!(plan.enabled);

        // Duplicate name rejected.
        match store
            .create_billing_plan(plan_input("starter", "2", "* * * * *"))
            .await
            .expect("create runs")
        {
            Ok(_) => panic!("duplicate plan name must be rejected"),
            Err(error) if error == "plan_name_exists" => {}
            Err(other) => panic!("unexpected error: {other}"),
        }

        let user = store
            .create_user("alice", "password", UserRole::User, None)
            .await
            .expect("user creates");
        store
            .admin_update_user_atomic(
                &user.id,
                AdminUpdateUserInput {
                    billing_plan_id: Some(Some(plan.id.clone())),
                    ..Default::default()
                },
                "actor",
            )
            .await
            .expect("assignment succeeds");

        let assigned = store
            .get_user_by_id(&user.id)
            .await
            .expect("user reads")
            .expect("user exists");
        let anchor = assigned.next_grant_at.expect("anchor set");
        assert_eq!(assigned.billing_plan_id.as_deref(), Some(plan.id.as_str()));
        assert_eq!(assigned.balance_nano_usd, "1000000000");
        let expected =
            super::next_grant_after("0 * * * *", assigned.updated_at).expect("hourly next fire");
        let expected_now =
            super::next_grant_after("0 * * * *", chrono::Utc::now()).expect("hourly next fire now");
        assert!(
            anchor == expected || anchor == expected_now,
            "anchor {anchor} must be the next hourly fire"
        );
        assert_eq!(
            store.run_plan_grant_tick().await.expect("tick runs"),
            0,
            "immediate assignment grant must not be repeated on the next tick"
        );

        // In-use plan cannot be deleted (BP-A4).
        match store
            .delete_billing_plan(&plan.id)
            .await
            .expect("delete runs")
        {
            Ok(()) => panic!("in-use plan must not delete"),
            Err(error) if error == "plan_in_use" => {}
            Err(other) => panic!("unexpected error: {other}"),
        }

        // Unassign clears both columns together (BP-S2).
        store
            .admin_update_user_atomic(
                &user.id,
                AdminUpdateUserInput {
                    billing_plan_id: Some(None),
                    ..Default::default()
                },
                "actor",
            )
            .await
            .expect("unassign succeeds");
        let unassigned = store
            .get_user_by_id(&user.id)
            .await
            .expect("user reads")
            .expect("user exists");
        assert!(unassigned.billing_plan_id.is_none());
        assert!(unassigned.next_grant_at.is_none());
        assert_eq!(unassigned.balance_nano_usd, "1000000000");

        // Unknown plan id fails the whole update (BP-S3).
        let error = store
            .admin_update_user_atomic(
                &user.id,
                AdminUpdateUserInput {
                    billing_plan_id: Some(Some("missing-plan".to_string())),
                    ..Default::default()
                },
                "actor",
            )
            .await
            .expect_err("unknown plan must fail");
        assert!(error.contains("billing plan not found"));

        assert!(
            store
                .delete_billing_plan(&plan.id)
                .await
                .expect("delete runs")
                .is_ok()
        );
    }

    #[tokio::test]
    async fn assignment_grants_immediately_only_when_predicates_hold() {
        let store = make_store().await;
        let enabled_plan = store
            .create_billing_plan(plan_input("live", "4", "* * * * *"))
            .await
            .expect("create succeeds")
            .expect("unique");
        let disabled_plan = store
            .create_billing_plan(BillingPlanInput {
                name: "paused".to_string(),
                grant_amount_nano_usd: None,
                grant_amount_usd: Some("9".to_string()),
                schedule: "* * * * *".to_string(),
                group_ids: None,
                enabled: Some(false),
            })
            .await
            .expect("create succeeds")
            .expect("unique");

        let eligible = store
            .create_user("grant_now", "password", UserRole::User, None)
            .await
            .expect("user creates");
        store
            .admin_update_user_atomic(
                &eligible.id,
                AdminUpdateUserInput {
                    billing_plan_id: Some(Some(enabled_plan.id.clone())),
                    ..Default::default()
                },
                "actor",
            )
            .await
            .expect("assigns");
        let eligible = store
            .get_user_by_id(&eligible.id)
            .await
            .expect("reads")
            .expect("exists");
        assert_eq!(eligible.balance_nano_usd, "4000000000");

        let unlimited = store
            .create_user("grant_skip_unlim", "password", UserRole::User, None)
            .await
            .expect("user creates");
        store
            .admin_update_user_atomic(
                &unlimited.id,
                AdminUpdateUserInput {
                    balance_unlimited: Some(true),
                    billing_plan_id: Some(Some(enabled_plan.id.clone())),
                    ..Default::default()
                },
                "actor",
            )
            .await
            .expect("assigns");
        let unlimited = store
            .get_user_by_id(&unlimited.id)
            .await
            .expect("reads")
            .expect("exists");
        assert_eq!(unlimited.balance_nano_usd, "0");
        assert!(unlimited.next_grant_at.is_some());

        let disabled = store
            .create_user("grant_skip_off", "password", UserRole::User, None)
            .await
            .expect("user creates");
        store
            .admin_update_user_atomic(
                &disabled.id,
                AdminUpdateUserInput {
                    enabled: Some(false),
                    billing_plan_id: Some(Some(enabled_plan.id.clone())),
                    ..Default::default()
                },
                "actor",
            )
            .await
            .expect("assigns");
        let disabled = store
            .get_user_by_id(&disabled.id)
            .await
            .expect("reads")
            .expect("exists");
        assert_eq!(disabled.balance_nano_usd, "0");
        assert!(disabled.next_grant_at.is_some());

        let paused = store
            .create_user("grant_skip_paused", "password", UserRole::User, None)
            .await
            .expect("user creates");
        store
            .admin_update_user_atomic(
                &paused.id,
                AdminUpdateUserInput {
                    billing_plan_id: Some(Some(disabled_plan.id.clone())),
                    ..Default::default()
                },
                "actor",
            )
            .await
            .expect("assigns");
        let paused = store
            .get_user_by_id(&paused.id)
            .await
            .expect("reads")
            .expect("exists");
        assert_eq!(paused.balance_nano_usd, "0");
        assert!(paused.next_grant_at.is_some());

        let explicit = store
            .create_user("grant_skip_explicit", "password", UserRole::User, None)
            .await
            .expect("user creates");
        store
            .admin_update_user_atomic(
                &explicit.id,
                AdminUpdateUserInput {
                    balance_nano_usd: Some("123".to_string()),
                    billing_plan_id: Some(Some(enabled_plan.id.clone())),
                    ..Default::default()
                },
                "actor",
            )
            .await
            .expect("assigns");
        let explicit = store
            .get_user_by_id(&explicit.id)
            .await
            .expect("reads")
            .expect("exists");
        assert_eq!(explicit.balance_nano_usd, "123");
    }

    #[tokio::test]
    async fn grant_tick_recovers_assigned_user_who_never_received_a_plan_grant() {
        let store = make_store().await;
        let plan = store
            .create_billing_plan(plan_input("backfill", "5", "0 0 * * *"))
            .await
            .expect("create succeeds")
            .expect("unique");
        let user = store
            .create_user("stale_sub", "password", UserRole::User, None)
            .await
            .expect("user creates");
        store
            .admin_update_user_atomic(
                &user.id,
                AdminUpdateUserInput {
                    balance_nano_usd: Some("0".to_string()),
                    billing_plan_id: Some(Some(plan.id.clone())),
                    ..Default::default()
                },
                "actor",
            )
            .await
            .expect("old-style assignment without immediate grant");
        let before = store
            .get_user_by_id(&user.id)
            .await
            .expect("reads")
            .expect("exists");
        assert_eq!(before.balance_nano_usd, "0");
        let anchor = before.next_grant_at.expect("future anchor");
        assert!(anchor > chrono::Utc::now());

        let granted = store.run_plan_grant_tick().await.expect("tick runs");
        assert_eq!(granted, 1);
        let after = store
            .get_user_by_id(&user.id)
            .await
            .expect("reads")
            .expect("exists");
        assert_eq!(after.balance_nano_usd, "5000000000");
        let kept = after.next_grant_at.expect("anchor kept");
        assert_eq!(kept, anchor);
        assert_eq!(store.run_plan_grant_tick().await.expect("second tick"), 0);
    }

    #[tokio::test]
    async fn create_plan_maps_validation_errors_as_inner_err() {
        let store = make_store().await;
        match store.create_billing_plan(plan_input("p", "1", "bad")).await {
            Ok(Err(code)) if code == "invalid_schedule" => {}
            other => panic!("expected inner invalid_schedule, got {other:?}"),
        }
        match store
            .create_billing_plan(BillingPlanInput {
                name: "p".to_string(),
                grant_amount_nano_usd: Some("abc".to_string()),
                grant_amount_usd: None,
                schedule: "* * * * *".to_string(),
                group_ids: None,
                enabled: None,
            })
            .await
        {
            Ok(Err(code)) if code == "invalid_grant_amount" => {}
            other => panic!("expected inner invalid_grant_amount, got {other:?}"),
        }
        let zero = store
            .create_billing_plan(plan_input("zero", "0", "* * * * *"))
            .await
            .expect("create runs")
            .expect("zero grant is valid");
        assert_eq!(zero.grant_amount_nano_usd, "0");
    }

    #[tokio::test]
    async fn plan_name_is_unique_case_insensitively() {
        let store = make_store().await;
        store
            .create_billing_plan(plan_input("Starter", "1", "* * * * *"))
            .await
            .expect("create runs")
            .expect("name is unique");
        match store
            .create_billing_plan(plan_input("starter", "2", "* * * * *"))
            .await
            .expect("create runs")
        {
            Ok(_) => panic!("case-insensitive duplicate must be rejected"),
            Err(error) if error == "plan_name_exists" => {}
            Err(other) => panic!("unexpected error: {other}"),
        }
    }

    #[tokio::test]
    async fn update_omits_leave_enabled_and_groups() {
        let store = make_store().await;
        let team_a = make_group(&store, "team-a").await;
        let plan = store
            .create_billing_plan(BillingPlanInput {
                name: "restricted".to_string(),
                grant_amount_nano_usd: None,
                grant_amount_usd: Some("1".to_string()),
                schedule: "* * * * *".to_string(),
                group_ids: Some(vec![team_a.id.clone()]),
                enabled: Some(false),
            })
            .await
            .expect("create runs")
            .expect("unique");
        store
            .update_billing_plan(
                &plan.id,
                BillingPlanInput {
                    name: plan.name.clone(),
                    grant_amount_nano_usd: Some(plan.grant_amount_nano_usd.clone()),
                    grant_amount_usd: None,
                    schedule: plan.schedule,
                    group_ids: None,
                    enabled: None,
                },
            )
            .await
            .expect("update runs")
            .expect("valid");
        let after = store
            .get_billing_plan_by_id(&plan.id)
            .await
            .expect("reads")
            .expect("exists");
        assert!(!after.enabled);
        assert_eq!(after.group_ids, vec![team_a.id.clone()]);

        // GR-C3: an unregistered id is rejected with the invalid_request code.
        match store
            .create_billing_plan(BillingPlanInput {
                name: "bad-groups".to_string(),
                grant_amount_nano_usd: None,
                grant_amount_usd: Some("1".to_string()),
                schedule: "* * * * *".to_string(),
                group_ids: Some(vec!["missing-group".to_string()]),
                enabled: None,
            })
            .await
            .expect("create runs")
        {
            Ok(_) => panic!("unknown group id must be rejected"),
            Err(code) => assert_eq!(code, "invalid_request"),
        }
    }

    #[tokio::test]
    async fn grant_tick_resets_balance_and_schedules_next_period_once() {
        let store = make_store().await;
        let plan = store
            .create_billing_plan(plan_input("daily", "2", "0 0 * * *"))
            .await
            .expect("create succeeds")
            .expect("unique");

        let user = store
            .create_user("bob", "password", UserRole::User, None)
            .await
            .expect("user creates");
        store
            .admin_update_user_atomic(
                &user.id,
                AdminUpdateUserInput {
                    balance_nano_usd: Some("500000000".to_string()),
                    billing_plan_id: Some(Some(plan.id.clone())),
                    ..Default::default()
                },
                "actor",
            )
            .await
            .expect("setup succeeds");

        // Force the anchor into the past so the tick is due immediately.
        let past = (chrono::Utc::now() - chrono::Duration::hours(2)).to_rfc3339();
        {
            let write = store.db.write().await;
            write
                .execute(store.db.stmt(
                    "UPDATE users SET next_grant_at = $1 WHERE id = $2",
                    vec![past.into(), user.id.clone().into()],
                ))
                .await
                .expect("anchor update succeeds");
        }

        let granted = store.run_plan_grant_tick().await.expect("tick runs");
        assert_eq!(granted, 1);

        let after = store
            .get_user_by_id(&user.id)
            .await
            .expect("user reads")
            .expect("user exists");
        assert_eq!(after.balance_nano_usd, "2000000000");
        let next = after.next_grant_at.expect("next anchor exists");
        assert!(next > chrono::Utc::now());

        // Second tick with a future anchor grants nothing (BP-G2).
        assert_eq!(store.run_plan_grant_tick().await.expect("tick runs"), 0);

        // Exactly one ledger row of kind plan_grant was appended (BP-G3).
        let ledger_count: i64 = {
            let read = store.db.read();
            let row = read
                .query_one(store.db.stmt(
                    "SELECT COUNT(*) AS count FROM billing_ledger WHERE user_id = $1 AND kind = 'plan_grant'",
                    vec![user.id.clone().into()],
                ))
                .await
                .expect("ledger query")
                .expect("row");
            row.try_get("", "count").expect("count decodes")
        };
        assert_eq!(ledger_count, 1);
    }

    #[tokio::test]
    async fn grant_tick_skips_unlimited_disabled_and_disabled_plans() {
        let store = make_store().await;
        let plan = store
            .create_billing_plan(plan_input("weekly", "3", "0 0 * * 0"))
            .await
            .expect("create succeeds")
            .expect("unique");

        for username in ["unlimited-user", "disabled-user", "planned"] {
            store
                .create_user(username, "password", UserRole::User, None)
                .await
                .expect("user creates");
        }
        let unlimited_user = store
            .get_user_by_username("unlimited-user")
            .await
            .expect("reads")
            .expect("exists");
        let disabled_user = store
            .get_user_by_username("disabled-user")
            .await
            .expect("reads")
            .expect("exists");
        let planned = store
            .get_user_by_username("planned")
            .await
            .expect("reads")
            .expect("exists");

        for target in [&unlimited_user, &disabled_user, &planned] {
            store
                .admin_update_user_atomic(
                    &target.id,
                    AdminUpdateUserInput {
                        billing_plan_id: Some(Some(plan.id.clone())),
                        ..Default::default()
                    },
                    "actor",
                )
                .await
                .expect("assignment works");
        }
        store
            .admin_update_user_atomic(
                &unlimited_user.id,
                AdminUpdateUserInput {
                    balance_unlimited: Some(true),
                    ..Default::default()
                },
                "actor",
            )
            .await
            .expect("unlimited works");
        store
            .update_user(
                &disabled_user.id,
                None,
                None,
                None,
                Some(false),
                None,
                None,
                None,
                None,
            )
            .await
            .expect("disable works");

        // Pull every anchor into the past.
        {
            let write = store.db.write().await;
            write
                .execute(store.db.stmt(
                    "UPDATE users SET next_grant_at = '2000-01-01T00:00:00+00:00'",
                    vec![],
                ))
                .await
                .expect("anchors updated");
        }

        let granted = store.run_plan_grant_tick().await.expect("tick runs");
        assert_eq!(
            granted, 1,
            "only the eligible planned user receives a grant"
        );

        let refreshed = store
            .get_user_by_id(&planned.id)
            .await
            .expect("reads")
            .expect("exists");
        assert_eq!(refreshed.balance_nano_usd, "3000000000");
    }

    #[tokio::test]
    async fn auth_candidate_applies_enabled_plan_group_layer() {
        let store = make_store().await;
        let team_a = make_group(&store, "team-a").await;
        let plan = store
            .create_billing_plan(BillingPlanInput {
                name: "grouped".to_string(),
                grant_amount_nano_usd: None,
                grant_amount_usd: Some("1".to_string()),
                schedule: "* * * * *".to_string(),
                group_ids: Some(vec![team_a.id.clone()]),
                enabled: None,
            })
            .await
            .expect("creates")
            .expect("unique");
        let user = store
            .create_user("carol", "password", UserRole::User, None)
            .await
            .expect("user creates");
        store
            .admin_update_user_atomic(
                &user.id,
                AdminUpdateUserInput {
                    billing_plan_id: Some(Some(plan.id.clone())),
                    ..Default::default()
                },
                "actor",
            )
            .await
            .expect("assigns");
        let (_, token) = store
            .create_api_key_extended(
                &user.id,
                crate::users::CreateApiKeyInput {
                    name: "k".to_string(),
                    expires_in_days: None,
                    sub_account_enabled: false,
                    sub_account_balance_nano_usd: None,
                    model_limits_enabled: false,
                    model_limits: Vec::new(),
                    ip_whitelist: Vec::new(),
                    use_user_group: true,
                    group_ids: Vec::new(),
                    max_multiplier: None,
                    transforms: Vec::new(),
                    model_redirects: Vec::new(),
                    reasoning_envelope_enabled: true,
                    request_capture_mode: crate::users::RequestCaptureMode::Off,
                    request_capture_retention: crate::users::RequestCaptureRetention::default(),
                },
                false,
            )
            .await
            .expect("key creates");

        let (api_key, owner, plan_groups) = store
            .validate_api_key(&token)
            .await
            .expect("validates")
            .expect("key valid");
        assert_eq!(plan_groups, Some(vec![team_a.id.clone()]));

        // The owner's default group is outside the plan ceiling, so the
        // resolved list is empty; selecting team-a explicitly passes.
        let effective = resolve_effective_groups(
            &owner.group_id,
            api_key.use_user_group,
            &api_key.group_ids,
            plan_groups.as_deref(),
        );
        assert_eq!(effective, Vec::<String>::new());
        let explicit = resolve_effective_groups(
            &owner.group_id,
            false,
            std::slice::from_ref(&team_a.id),
            plan_groups.as_deref(),
        );
        assert_eq!(explicit, vec![team_a.id.clone()]);
    }

    #[tokio::test]
    async fn admin_reset_grants_eligible_subscribers_and_skips_others() {
        let store = make_store().await;
        let plan_a = store
            .create_billing_plan(plan_input("reset-a", "5", "0 0 * * *"))
            .await
            .expect("create a")
            .expect("unique");
        let plan_b = store
            .create_billing_plan(plan_input("reset-b", "9", "0 0 * * *"))
            .await
            .expect("create b")
            .expect("unique");
        let disabled_plan = store
            .create_billing_plan(BillingPlanInput {
                name: "reset-disabled-plan".to_string(),
                grant_amount_nano_usd: None,
                grant_amount_usd: Some("7".to_string()),
                schedule: "0 0 * * *".to_string(),
                group_ids: None,
                enabled: Some(false),
            })
            .await
            .expect("create disabled plan")
            .expect("unique");

        for username in [
            "reset-eligible",
            "reset-unlimited",
            "reset-disabled-user",
            "reset-other-plan",
            "reset-disabled-plan-user",
        ] {
            store
                .create_user(username, "password", UserRole::User, None)
                .await
                .expect("user creates");
        }
        let eligible = store
            .get_user_by_username("reset-eligible")
            .await
            .expect("reads")
            .expect("exists");
        let unlimited = store
            .get_user_by_username("reset-unlimited")
            .await
            .expect("reads")
            .expect("exists");
        let disabled_user = store
            .get_user_by_username("reset-disabled-user")
            .await
            .expect("reads")
            .expect("exists");
        let other = store
            .get_user_by_username("reset-other-plan")
            .await
            .expect("reads")
            .expect("exists");
        let disabled_plan_user = store
            .get_user_by_username("reset-disabled-plan-user")
            .await
            .expect("reads")
            .expect("exists");

        for (user, plan_id) in [
            (&eligible, &plan_a.id),
            (&other, &plan_b.id),
            (&disabled_plan_user, &disabled_plan.id),
        ] {
            store
                .admin_update_user_atomic(
                    &user.id,
                    AdminUpdateUserInput {
                        billing_plan_id: Some(Some(plan_id.clone())),
                        ..Default::default()
                    },
                    "actor",
                )
                .await
                .expect("assigns");
        }
        store
            .admin_update_user_atomic(
                &unlimited.id,
                AdminUpdateUserInput {
                    billing_plan_id: Some(Some(plan_a.id.clone())),
                    balance_unlimited: Some(true),
                    ..Default::default()
                },
                "actor",
            )
            .await
            .expect("unlimited assign");
        store
            .admin_update_user_atomic(
                &disabled_user.id,
                AdminUpdateUserInput {
                    billing_plan_id: Some(Some(plan_a.id.clone())),
                    enabled: Some(false),
                    ..Default::default()
                },
                "actor",
            )
            .await
            .expect("disabled assign");

        store
            .admin_update_user_atomic(
                &eligible.id,
                AdminUpdateUserInput {
                    balance_nano_usd: Some("1000000000".to_string()),
                    ..Default::default()
                },
                "actor",
            )
            .await
            .expect("spend eligible");
        store
            .admin_update_user_atomic(
                &other.id,
                AdminUpdateUserInput {
                    balance_nano_usd: Some("1000000000".to_string()),
                    ..Default::default()
                },
                "actor",
            )
            .await
            .expect("spend other");

        let missing = store
            .reset_billing_plan_grants("missing-plan")
            .await
            .expect_err("missing plan");
        assert_eq!(missing, "not_found");

        let empty = store
            .create_billing_plan(plan_input("reset-empty", "1", "0 0 * * *"))
            .await
            .expect("create empty")
            .expect("unique");
        assert_eq!(
            store
                .reset_billing_plan_grants(&empty.id)
                .await
                .expect("empty reset"),
            0
        );

        let reset = store
            .reset_billing_plan_grants(&plan_a.id)
            .await
            .expect("reset a");
        assert_eq!(reset, 1, "only the enabled non-unlimited subscriber");

        let eligible_after = store
            .get_user_by_id(&eligible.id)
            .await
            .expect("reads")
            .expect("exists");
        assert_eq!(eligible_after.balance_nano_usd, "5000000000");
        let next = eligible_after.next_grant_at.expect("anchor kept");
        assert!(next > chrono::Utc::now());
        assert_eq!(
            store.run_plan_grant_tick().await.expect("tick after reset"),
            0
        );

        let unlimited_after = store
            .get_user_by_id(&unlimited.id)
            .await
            .expect("reads")
            .expect("exists");
        assert_ne!(unlimited_after.balance_nano_usd, "5000000000");

        let disabled_after = store
            .get_user_by_id(&disabled_user.id)
            .await
            .expect("reads")
            .expect("exists");
        assert_ne!(disabled_after.balance_nano_usd, "5000000000");

        let other_after = store
            .get_user_by_id(&other.id)
            .await
            .expect("reads")
            .expect("exists");
        assert_eq!(other_after.balance_nano_usd, "1000000000");

        let disabled_plan_reset = store
            .reset_billing_plan_grants(&disabled_plan.id)
            .await
            .expect("reset disabled plan");
        assert_eq!(disabled_plan_reset, 1);
        let disabled_plan_user_after = store
            .get_user_by_id(&disabled_plan_user.id)
            .await
            .expect("reads")
            .expect("exists");
        assert_eq!(disabled_plan_user_after.balance_nano_usd, "7000000000");

        let meta: String = {
            let read = store.db.read();
            let row = read
                .query_one(store.db.stmt(
                    "SELECT meta_json FROM billing_ledger WHERE user_id = $1 AND kind = 'plan_grant' ORDER BY created_at DESC LIMIT 1",
                    vec![eligible.id.clone().into()],
                ))
                .await
                .expect("ledger query")
                .expect("row");
            row.try_get("", "meta_json").expect("meta")
        };
        let parsed: serde_json::Value = serde_json::from_str(&meta).expect("json");
        assert_eq!(parsed["source"], serde_json::json!("admin_reset"));
        assert_eq!(parsed["plan_id"], serde_json::json!(plan_a.id));
    }
}
