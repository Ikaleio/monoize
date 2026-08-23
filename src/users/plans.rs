use super::store::{parse_allowed_groups_json, serialize_allowed_groups_json};
use crate::users::{UserStore, canonicalize_groups, parse_nano_usd};
use chrono::{DateTime, Utc};
use sea_orm::{ConnectionTrait, TransactionTrait, Value as SeaValue};
use serde::{Deserialize, Serialize};

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
    pub period_seconds: i64,
    /// Canonical group restriction layer; empty = unrestricted.
    pub allowed_groups: Vec<String>,
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
    pub period_seconds: i64,
    #[serde(default)]
    pub allowed_groups: Vec<String>,
    #[serde(default)]
    pub enabled: Option<bool>,
}

struct ValidatedPlan {
    name: String,
    amount: i128,
    period_seconds: i64,
    allowed_groups: Vec<String>,
    enabled: bool,
}

fn validate_plan_input(input: &BillingPlanInput) -> Result<ValidatedPlan, String> {
    let name = input.name.trim();
    if name.is_empty() || name.chars().count() > 100 {
        return Err("invalid_plan_name".to_string());
    }
    if input.period_seconds <= 0 {
        return Err("invalid_period".to_string());
    }
    let amount = if let Some(raw) = input.grant_amount_nano_usd.as_deref() {
        let parsed = parse_nano_usd(raw)?;
        if raw.trim() != parsed.to_string() || parsed < 0 {
            return Err("invalid_grant_amount".to_string());
        }
        parsed
    } else if let Some(raw) = input.grant_amount_usd.as_deref() {
        let parsed = super::utils::parse_usd_to_nano(raw)?;
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
        period_seconds: input.period_seconds,
        allowed_groups: canonicalize_groups(&input.allowed_groups),
        enabled: input.enabled.unwrap_or(true),
    })
}

fn sql_err<E: std::fmt::Display>(error: E) -> String {
    format!("invalid persisted billing plan data: {error}")
}

fn row_to_plan(row: &sea_orm::QueryResult) -> Result<BillingPlan, String> {
    let enabled = super::store::decode_required_bool(row, "enabled")?;
    let allowed_groups_raw: String = row.try_get("", "allowed_groups").map_err(sql_err)?;
    Ok(BillingPlan {
        id: row.try_get("", "id").map_err(sql_err)?,
        name: row.try_get("", "name").map_err(sql_err)?,
        grant_amount_nano_usd: row.try_get("", "grant_amount_nano_usd").map_err(sql_err)?,
        period_seconds: row.try_get("", "period_seconds").map_err(sql_err)?,
        allowed_groups: parse_allowed_groups_json(
            Some(allowed_groups_raw.as_str()),
            "billing_plans.allowed_groups",
        )?,
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
    pub async fn list_billing_plans(&self) -> Result<Vec<BillingPlan>, String> {
        let rows = self
            .db
            .read()
            .query_all(self.db.stmt(
                "SELECT id, name, grant_amount_nano_usd, period_seconds, allowed_groups, enabled, created_at, updated_at FROM billing_plans ORDER BY created_at ASC",
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
                "SELECT id, name, grant_amount_nano_usd, period_seconds, allowed_groups, enabled, created_at, updated_at FROM billing_plans WHERE id = $1",
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
        let plan = validate_plan_input(&input)?;
        if self.plan_name_exists(None, &plan.name).await? {
            return Ok(Err("plan_name_exists".to_string()));
        }

        let id = uuid::Uuid::new_v4().to_string();
        let now = Utc::now().to_rfc3339();
        self.db.write().await.execute(self.db.stmt(
            "INSERT INTO billing_plans (id, name, grant_amount_nano_usd, period_seconds, allowed_groups, enabled, created_at, updated_at) VALUES ($1, $2, $3, $4, $5, $6, $7, $7)",
            vec![
                id.clone().into(),
                plan.name.into(),
                plan.amount.to_string().into(),
                SeaValue::BigInt(Some(plan.period_seconds)),
                serialize_allowed_groups_json(&plan.allowed_groups)?.into(),
                SeaValue::Int(Some(if plan.enabled { 1 } else { 0 })),
                now.into(),
            ],
        ))
        .await
        .map_err(|e| e.to_string())?;

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
        self.get_billing_plan_by_id(plan_id)
            .await?
            .ok_or_else(|| "not_found".to_string())?;
        let plan = validate_plan_input(&input)?;
        if self.plan_name_exists(Some(plan_id), &plan.name).await? {
            return Ok(Err("plan_name_exists".to_string()));
        }

        // Plan edits affect only future evaluations; existing next_grant_at anchors stay.
        self.db.write().await.execute(self.db.stmt(
            "UPDATE billing_plans SET name = $1, grant_amount_nano_usd = $2, period_seconds = $3, allowed_groups = $4, enabled = $5, updated_at = $6 WHERE id = $7",
            vec![
                plan.name.into(),
                plan.amount.to_string().into(),
                SeaValue::BigInt(Some(plan.period_seconds)),
                serialize_allowed_groups_json(&plan.allowed_groups)?.into(),
                SeaValue::Int(Some(if plan.enabled { 1 } else { 0 })),
                Utc::now().to_rfc3339().into(),
                plan_id.into(),
            ],
        ))
        .await
        .map_err(|e| e.to_string())?;

        // Cached auth results embed the plan's group restriction layer.
        self.api_key_cache.invalidate_all();
        Ok(Ok(()))
    }

    pub async fn delete_billing_plan(&self, plan_id: &str) -> Result<Result<(), String>, String> {
        let count = self.count_users_with_plan(plan_id).await?;
        if count > 0 {
            return Ok(Err("plan_in_use".to_string()));
        }

        let result = self
            .db
            .write()
            .await
            .execute(self.db.stmt(
                "DELETE FROM billing_plans WHERE id = $1",
                vec![plan_id.into()],
            ))
            .await
            .map_err(|e| e.to_string())?;
        if result.rows_affected() == 0 {
            return Err("not_found".to_string());
        }
        Ok(Ok(()))
    }

    pub(crate) async fn plan_name_exists(
        &self,
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
        let row = self
            .db
            .read()
            .query_one(self.db.stmt(sql, values))
            .await
            .map_err(|e| e.to_string())?;
        Ok(row.is_some())
    }

    pub(crate) async fn count_users_with_plan(&self, plan_id: &str) -> Result<i64, String> {
        let row = self
            .db
            .read()
            .query_one(self.db.stmt(
                "SELECT COUNT(*) AS count FROM users WHERE billing_plan_id = $1",
                vec![plan_id.into()],
            ))
            .await
            .map_err(|e| e.to_string())?
            .ok_or_else(|| "no count row".to_string())?;
        row.try_get::<i64>("", "count").map_err(|e| e.to_string())
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
                "SELECT u.id AS user_id FROM users u JOIN billing_plans p ON p.id = u.billing_plan_id WHERE u.billing_plan_id IS NOT NULL AND u.next_grant_at IS NOT NULL AND u.enabled = 1 AND u.balance_unlimited = 0 AND p.enabled = 1 AND u.next_grant_at <= $1",
                vec![now.to_rfc3339().into()],
            ))
            .await
            .map_err(|e| e.to_string())?;

        let mut granted = 0usize;
        for row in &due {
            let user_id: String = row.try_get("", "user_id").map_err(sql_err).map_err(|e| e)?;
            match self.grant_user_once(&user_id).await {
                Ok(true) => granted += 1,
                Ok(false) => {}
                Err(error) => tracing::warn!(user_id = %user_id, %error, "plan grant failed"),
            }
        }
        Ok(granted)
    }

    /// Applies at most one grant for the user (BP-G5 catch-up rule). Returns
    /// false when the locked state no longer satisfies the due conditions.
    async fn grant_user_once(&self, user_id: &str) -> Result<bool, String> {
        let execution_now = Utc::now();
        let write = self.db.write().await;
        let tx = write.begin().await.map_err(|e| e.to_string())?;

        let lock_sql = if self.db.is_postgres() {
            "SELECT u.balance_unlimited, u.enabled, u.balance_nano_usd, u.next_grant_at, p.id AS plan_id, p.name AS plan_name, p.grant_amount_nano_usd, p.period_seconds, p.enabled AS plan_enabled FROM users u JOIN billing_plans p ON p.id = u.billing_plan_id WHERE u.id = $1 FOR UPDATE OF u"
        } else {
            "SELECT u.balance_unlimited, u.enabled, u.balance_nano_usd, u.next_grant_at, p.id AS plan_id, p.name AS plan_name, p.grant_amount_nano_usd, p.period_seconds, p.enabled AS plan_enabled FROM users u JOIN billing_plans p ON p.id = u.billing_plan_id WHERE u.id = $1"
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
        let period_seconds: i64 = row.try_get("", "period_seconds").map_err(sql_err)?;

        let Some(next_grant_raw) = raw_next_grant_at.as_deref() else {
            return Ok(false);
        };
        let next_grant_at = DateTime::parse_from_rfc3339(next_grant_raw)
            .map_err(sql_err)?
            .with_timezone(&Utc);
        if unlimited != 0 || enabled != 1 || plan_enabled != 1 || next_grant_at > execution_now {
            return Ok(false);
        }

        // Absolute reset per BP-G3.
        let new_balance = amount;
        let delta = new_balance
            .checked_sub(old_balance)
            .ok_or("balance overflow")?;
        let next_anchor_ts = execution_now
            .timestamp()
            .checked_add(period_seconds)
            .ok_or("next_grant_at overflow")?;
        let next_anchor = DateTime::from_timestamp(next_anchor_ts, 0)
            .ok_or("next_grant_at overflow")?
            .to_rfc3339();

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

        self.insert_billing_ledger_tx(
            &tx,
            user_id,
            "plan_grant",
            delta,
            Some(new_balance),
            &serde_json::json!({
                "plan_id": plan_id,
                "plan_name": plan_name,
                "before_balance_nano_usd": old_balance.to_string(),
                "after_balance_nano_usd": new_balance.to_string(),
            }),
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
    use crate::users::{
        AdminUpdateUserInput, UserRole, UserStore, compute_effective_groups_with_plan,
    };
    use sea_orm::ConnectionTrait;
    use sea_orm_migration::MigratorTrait;

    fn plan_input(name: &str, amount_usd: &str, period_seconds: i64) -> BillingPlanInput {
        BillingPlanInput {
            name: name.to_string(),
            grant_amount_nano_usd: None,
            grant_amount_usd: Some(amount_usd.to_string()),
            period_seconds,
            allowed_groups: Vec::new(),
            enabled: None,
        }
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
    fn plan_input_rejects_non_positive_period_and_negative_amounts() {
        let mut input = plan_input("p", "5", 0);
        assert_eq!(
            validate_plan_input(&input).err(),
            Some("invalid_period".to_string())
        );
        input.period_seconds = -10;
        assert_eq!(
            validate_plan_input(&input).err(),
            Some("invalid_period".to_string())
        );
        input.period_seconds = 86_400;
        input.grant_amount_usd = Some("-1".to_string());
        assert_eq!(
            validate_plan_input(&input).err(),
            Some("invalid_grant_amount".to_string())
        );
    }

    #[test]
    fn effective_groups_intersect_all_restricting_layers() {
        let user = vec!["Team-A".to_string(), "team-b".to_string()];
        let plan = vec!["team-b".to_string(), " team-c ".to_string()];
        let key = vec!["TEAM-C".to_string()];

        assert_eq!(
            compute_effective_groups_with_plan(&user, Some(&plan), &key),
            Some(vec![])
        );

        let key = vec!["team-b".to_string()];
        assert_eq!(
            compute_effective_groups_with_plan(&user, Some(&plan), &key),
            Some(vec!["team-b".to_string()])
        );

        // Unrestricted user and key with no plan stays fully unrestricted.
        assert_eq!(compute_effective_groups_with_plan(&[], None, &[]), None);

        // Plan-only restriction applies when the other layers are unrestricted.
        assert_eq!(
            compute_effective_groups_with_plan(&[], Some(&plan), &[]),
            Some(vec!["team-b".to_string(), "team-c".to_string()])
        );
    }

    #[tokio::test]
    async fn plan_lifecycle_and_assignment_anchor() {
        let store = make_store().await;
        let plan = store
            .create_billing_plan(plan_input("starter", "1", 3_600))
            .await
            .expect("create succeeds")
            .expect("name is unique");
        assert_eq!(plan.grant_amount_nano_usd, "1000000000");
        assert_eq!(plan.period_seconds, 3_600);
        assert!(plan.enabled);

        // Duplicate name rejected.
        match store
            .create_billing_plan(plan_input("starter", "2", 60))
            .await
            .expect("create runs")
        {
            Ok(_) => panic!("duplicate plan name must be rejected"),
            Err(error) if error == "plan_name_exists" => {}
            Err(other) => panic!("unexpected error: {other}"),
        }

        let user = store
            .create_user("alice", "password", UserRole::User, &[])
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
        let expected = chrono::Utc::now() + chrono::Duration::seconds(3_600);
        let skew = (expected - anchor).num_milliseconds().abs();
        assert!(skew < 5_000, "anchor must be ~now+3600s, skew={skew}ms");

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
    async fn grant_tick_resets_balance_and_schedules_next_period_once() {
        let store = make_store().await;
        let plan = store
            .create_billing_plan(plan_input("daily", "2", 86_400))
            .await
            .expect("create succeeds")
            .expect("unique");

        let user = store
            .create_user("bob", "password", UserRole::User, &[])
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
            .create_billing_plan(plan_input("weekly", "3", 604_800))
            .await
            .expect("create succeeds")
            .expect("unique");

        for username in ["unlimited-user", "disabled-user", "planned"] {
            store
                .create_user(username, "password", UserRole::User, &[])
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
        let plan = store
            .create_billing_plan(BillingPlanInput {
                name: "grouped".to_string(),
                grant_amount_nano_usd: None,
                grant_amount_usd: Some("1".to_string()),
                period_seconds: 60,
                allowed_groups: vec!["team-a".to_string()],
                enabled: None,
            })
            .await
            .expect("creates")
            .expect("unique");
        let user = store
            .create_user("carol", "password", UserRole::User, &[])
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
                    allowed_groups: Vec::new(),
                    max_multiplier: None,
                    transforms: Vec::new(),
                    model_redirects: Vec::new(),
                    reasoning_envelope_enabled: true,
                    request_capture_mode: crate::users::RequestCaptureMode::Off,
                },
                false,
            )
            .await
            .expect("key creates");

        let (api_key, _, plan_groups) = store
            .validate_api_key(&token)
            .await
            .expect("validates")
            .expect("key valid");
        assert_eq!(plan_groups, Some(vec!["team-a".to_string()]));

        let effective = compute_effective_groups_with_plan(
            &api_key.allowed_groups,
            plan_groups.as_deref(),
            &api_key.allowed_groups,
        );
        assert_eq!(effective, Some(vec!["team-a".to_string()]));
    }
}
