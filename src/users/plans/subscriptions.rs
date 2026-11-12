use super::*;

impl UserStore {
    pub(super) async fn active_subscription_on<C: ConnectionTrait>(
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

    pub(super) async fn usage_snapshot_on<C: ConnectionTrait>(
        &self,
        connection: &C,
        subscription: &SubscriptionSnapshot,
        now: DateTime<Utc>,
    ) -> Result<PlanUsageSnapshot, String> {
        // BP-U7: probe 0 reads the C(T) row; probe `index + 1` reads the first row of
        // window `index`. One statement keeps every probe on the same snapshot.
        let windows = subscription.limits.values();
        let mut sql = format!(
            "SELECT 0 AS probe, {USAGE_PROBE_COLUMNS} FROM (SELECT {USAGE_PROBE_COLUMNS} FROM billing_plan_usage WHERE subscription_id = $1 AND created_at <= $2 ORDER BY created_at DESC, id DESC LIMIT 1) AS probe_0"
        );
        let mut values: Vec<SeaValue> =
            vec![subscription.id.clone().into(), now.to_rfc3339().into()];
        for (index, (seconds, limit)) in windows.iter().enumerate() {
            if limit.is_none() {
                continue;
            }
            let start = now
                .checked_sub_signed(Duration::seconds(*seconds))
                .ok_or_else(|| "plan window timestamp overflow".to_string())?;
            values.push(start.to_rfc3339().into());
            let probe = index + 1;
            let start_placeholder = values.len();
            sql.push_str(&format!(
                " UNION ALL SELECT {probe} AS probe, {USAGE_PROBE_COLUMNS} FROM (SELECT {USAGE_PROBE_COLUMNS} FROM billing_plan_usage WHERE subscription_id = $1 AND created_at > ${start_placeholder} AND created_at <= $2 ORDER BY created_at ASC, id ASC LIMIT 1) AS probe_{probe}"
            ));
        }
        let rows = connection
            .query_all(self.db.stmt(&sql, values))
            .await
            .map_err(|error| error.to_string())?;
        let mut probes: [Option<UsageProbe>; 5] = Default::default();
        for row in &rows {
            let probe = row.try_get::<i32>("", "probe").map_err(sql_err)?;
            let slot = usize::try_from(probe)
                .ok()
                .and_then(|probe| probes.get_mut(probe))
                .ok_or_else(|| sql_err("unexpected plan usage probe"))?;
            *slot = Some(UsageProbe::from_row(row)?);
        }
        let [latest, window_probes @ ..] = probes;
        let mut usage = PlanUsageSnapshot::default();
        for (index, ((seconds, _), first)) in windows.into_iter().zip(window_probes).enumerate() {
            let Some(first) = first else {
                continue;
            };
            let latest = latest
                .as_ref()
                .ok_or_else(|| sql_err("plan usage window row without a latest row"))?;
            let used = latest
                .cumulative
                .checked_sub(first.preceding)
                .filter(|used| *used >= first.amount)
                .ok_or_else(|| sql_err("plan usage cumulative sums are inconsistent"))?;
            usage.sums[index] = used;
            usage.next_reset_at[index] = Some(
                first
                    .created_at
                    .checked_add_signed(Duration::seconds(seconds))
                    .ok_or_else(|| "plan window timestamp overflow".to_string())?,
            );
        }
        Ok(usage)
    }

    pub(super) fn remaining_capacity(
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

    pub(super) fn window_usage(
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

    pub(super) async fn subscription_response_on<C: ConnectionTrait>(
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
            price_nano_usd: subscription.price_nano_usd.map(|price| price.to_string()),
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

    /// Return the user's active subscription groups without loading usage.
    /// Missing, future, expired, and revoked subscriptions return `None`.
    /// Storage errors and active subscriptions without groups return an error.
    pub(crate) async fn get_active_billing_plan_group_ids(
        &self,
        user_id: &str,
    ) -> Result<Option<Vec<String>>, String> {
        let read = self.db.read();
        let Some(subscription) = self
            .active_subscription_on(read, user_id, Utc::now(), false)
            .await?
        else {
            return Ok(None);
        };
        if subscription.group_ids.is_empty() {
            return Err(sql_err("active subscription has no routing groups"));
        }
        Ok(Some(subscription.group_ids))
    }

    /// Active subscription `(plan_name, expires_at)` per user for the admin users list.
    /// One set query; when a user has overlapping rows the latest-expiring one wins,
    /// matching `active_subscription_on`.
    pub async fn list_active_plan_summaries(
        &self,
    ) -> Result<std::collections::HashMap<String, (String, DateTime<Utc>)>, String> {
        let now = Utc::now();
        let rows = self
            .db
            .read()
            .query_all(self.db.stmt(
                "SELECT user_id, plan_name, expires_at FROM billing_plan_subscriptions WHERE starts_at <= $1 AND expires_at > $1 AND revoked_at IS NULL ORDER BY user_id ASC, expires_at DESC, id ASC",
                vec![now.to_rfc3339().into()],
            ))
            .await
            .map_err(|error| error.to_string())?;
        let mut summaries = std::collections::HashMap::new();
        for row in rows {
            let user_id: String = row.try_get("", "user_id").map_err(sql_err)?;
            if summaries.contains_key(&user_id) {
                continue;
            }
            let plan_name: String = row.try_get("", "plan_name").map_err(sql_err)?;
            summaries.insert(user_id, (plan_name, parse_time(&row, "expires_at")?));
        }
        Ok(summaries)
    }

    pub async fn get_active_billing_plan_subscription(
        &self,
        user_id: &str,
    ) -> Result<Option<BillingPlanSubscription>, String> {
        let now = Utc::now();
        let read = self.db.read();
        let Some(subscription) = self
            .active_subscription_on(read, user_id, now, false)
            .await?
        else {
            return Ok(None);
        };
        self.subscription_response_on(read, subscription, now)
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
        if !crate::users::store::decode_required_bool(&user_row, "enabled")? {
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
        let (price, duration_seconds, expires_at) = plan_purchase_price(&price_row, now)?;
        let Some((new_balance, unlimited)) = self
            .debit_plan_purchase_tx(&tx, user_id, &user_row, price, &now_raw)
            .await?
        else {
            return Ok(Err("insufficient_balance".to_string()));
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
            BillingLedgerEntry {
                user_id,
                kind: "plan_purchase",
                delta_nano_usd: if unlimited { 0 } else { -price },
                balance_after_nano_usd: Some(new_balance),
                meta: &serde_json::json!({
                    "plan_id": plan_id,
                    "plan_name": plan_name,
                    "price_id": price_id,
                    "subscription_id": subscription_id,
                    "duration_seconds": duration_seconds,
                    "expires_at": expires_at.to_rfc3339(),
                }),
                created_at_rfc3339: &now_raw,
            },
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
        duration_seconds: i64,
    ) -> Result<Result<BillingPlanSubscription, String>, String> {
        let now = Utc::now();
        let Some(duration) = Duration::try_seconds(duration_seconds) else {
            return Ok(Err("invalid_plan_duration".to_string()));
        };
        if duration_seconds <= 0 {
            return Ok(Err("invalid_plan_duration".to_string()));
        }
        let Some(expires_at) = now.checked_add_signed(duration) else {
            return Ok(Err("invalid_plan_duration".to_string()));
        };
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
                Option::<String>::None.into(),
                plan_row.try_get::<String>("", "name").map_err(sql_err)?.into(),
                plan_row.try_get::<String>("", "description").map_err(sql_err)?.into(),
                plan_row.try_get::<Option<String>>("", "limit_5h_nano_usd").map_err(sql_err)?.into(),
                plan_row.try_get::<Option<String>>("", "limit_24h_nano_usd").map_err(sql_err)?.into(),
                plan_row.try_get::<Option<String>>("", "limit_7d_nano_usd").map_err(sql_err)?.into(),
                plan_row.try_get::<Option<String>>("", "limit_30d_nano_usd").map_err(sql_err)?.into(),
                plan_row.try_get::<String>("", "group_ids").map_err(sql_err)?.into(),
                plan_row.try_get::<String>("", "multiplier").map_err(sql_err)?.into(),
                Option::<String>::None.into(),
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

    async fn debit_plan_purchase_tx(
        &self,
        tx: &DatabaseTransaction,
        user_id: &str,
        user_row: &QueryResult,
        price: i128,
        now_raw: &str,
    ) -> Result<Option<(i128, bool)>, String> {
        let unlimited = crate::users::store::decode_required_bool(user_row, "balance_unlimited")?;
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
                return Ok(None);
            }
            tx.execute(self.db.stmt(
                "UPDATE users SET balance_nano_usd = $1, updated_at = $2 WHERE id = $3",
                vec![value.to_string().into(), now_raw.into(), user_id.into()],
            ))
            .await
            .map_err(|error| error.to_string())?;
            value
        };
        Ok(Some((new_balance, unlimited)))
    }
}

fn plan_purchase_price(
    price_row: &QueryResult,
    now: DateTime<Utc>,
) -> Result<(i128, i64, DateTime<Utc>), String> {
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
    Ok((price, duration_seconds, expires_at))
}
