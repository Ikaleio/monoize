use super::*;

impl UserStore {
    pub(super) async fn validate_plan_groups(
        &self,
        group_ids: &[String],
    ) -> Result<Result<(), String>, String> {
        if self.find_unknown_group_id(group_ids).await?.is_some() {
            return Ok(Err("invalid_plan_groups".to_string()));
        }
        Ok(Ok(()))
    }

    pub(super) async fn load_plan_prices_on<C: ConnectionTrait>(
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

    pub(super) async fn list_billing_plans_filtered(
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
            let prices = self.load_plan_prices_on(self.db.read(), &id).await?;
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
        let prices = self.load_plan_prices_on(read, id).await?;
        Ok(Some(row_to_plan(&row, prices)?))
    }

    pub(super) async fn plan_name_exists_on<C: ConnectionTrait>(
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

    pub(super) async fn insert_prices_tx(
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
}
