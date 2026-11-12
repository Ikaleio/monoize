use super::*;

pub(crate) struct BillingLedgerEntry<'a> {
    pub(crate) user_id: &'a str,
    pub(crate) kind: &'a str,
    pub(crate) delta_nano_usd: i128,
    pub(crate) balance_after_nano_usd: Option<i128>,
    pub(crate) meta: &'a Value,
    pub(crate) created_at_rfc3339: &'a str,
}

impl UserStore {
    pub(crate) async fn lock_user_balance_tx(
        &self,
        tx: &DatabaseTransaction,
        user_id: &str,
    ) -> Result<LockedUserBalance, BillingError> {
        let sql = if self.db.is_postgres() {
            "SELECT balance_nano_usd, balance_unlimited FROM users WHERE id = $1 FOR UPDATE"
        } else {
            "SELECT balance_nano_usd, balance_unlimited FROM users WHERE id = $1"
        };
        let row = tx
            .query_one(self.db.stmt(sql, vec![user_id.into()]))
            .await
            .map_err(|e| BillingError::new(BillingErrorKind::Internal, e.to_string()))?
            .ok_or_else(|| BillingError::new(BillingErrorKind::NotFound, "user not found"))?;
        let raw: String = row
            .try_get("", "balance_nano_usd")
            .map_err(|e| BillingError::new(BillingErrorKind::Internal, e.to_string()))?;
        let balance = parse_nano_usd(&raw)
            .map_err(|e| BillingError::new(BillingErrorKind::InvalidStoredBalance, e))?;
        let unlimited = row
            .try_get::<i32>("", "balance_unlimited")
            .map_err(|e| BillingError::new(BillingErrorKind::Internal, e.to_string()))?
            == 1;
        Ok(LockedUserBalance { balance, unlimited })
    }

    pub(super) async fn lock_api_key_balance_tx(
        &self,
        tx: &DatabaseTransaction,
        api_key_id: &str,
        expected_user_id: &str,
    ) -> Result<LockedApiKeyBalance, BillingError> {
        let sql = if self.db.is_postgres() {
            "SELECT user_id, sub_account_enabled, sub_account_balance_nano FROM api_keys WHERE id = $1 FOR UPDATE"
        } else {
            "SELECT user_id, sub_account_enabled, sub_account_balance_nano FROM api_keys WHERE id = $1"
        };
        let row = tx
            .query_one(self.db.stmt(sql, vec![api_key_id.into()]))
            .await
            .map_err(|e| BillingError::new(BillingErrorKind::Internal, e.to_string()))?
            .ok_or_else(|| BillingError::new(BillingErrorKind::NotFound, "api key not found"))?;
        let user_id: String = row
            .try_get("", "user_id")
            .map_err(|e| BillingError::new(BillingErrorKind::Internal, e.to_string()))?;
        if user_id != expected_user_id {
            return Err(BillingError::new(
                BillingErrorKind::NotFound,
                "api key owner does not match user",
            ));
        }
        let raw: String = row
            .try_get("", "sub_account_balance_nano")
            .map_err(|e| BillingError::new(BillingErrorKind::Internal, e.to_string()))?;
        let balance = parse_nano_usd(&raw)
            .map_err(|e| BillingError::new(BillingErrorKind::InvalidStoredBalance, e))?;
        let sub_account_enabled = row
            .try_get::<i32>("", "sub_account_enabled")
            .map_err(|e| BillingError::new(BillingErrorKind::Internal, e.to_string()))?
            == 1;
        Ok(LockedApiKeyBalance {
            user_id,
            balance,
            sub_account_enabled,
        })
    }

    pub(super) fn user_balance_from_row(row: &QueryResult) -> Result<UserBalance, String> {
        let balance_raw: String = row
            .try_get("", "balance_nano_usd")
            .map_err(|e| e.to_string())?;
        Ok(UserBalance {
            user_id: row.try_get("", "id").map_err(|e| e.to_string())?,
            balance_nano_usd: parse_nano_usd(&balance_raw)?,
            balance_unlimited: row
                .try_get::<i32>("", "balance_unlimited")
                .map_err(|e| e.to_string())?
                == 1,
        })
    }

    pub(super) async fn load_user_balance(
        &self,
        user_id: &str,
    ) -> Result<Option<UserBalance>, String> {
        let row = self
            .db
            .read()
            .query_one(self.db.stmt(
                "SELECT id, balance_nano_usd, balance_unlimited FROM users WHERE id = $1",
                vec![user_id.into()],
            ))
            .await
            .map_err(|e| e.to_string())?;
        row.map(|row| Self::user_balance_from_row(&row)).transpose()
    }

    pub async fn get_user_balance(&self, user_id: &str) -> Result<Option<UserBalance>, String> {
        loop {
            if let Some(cached) = self.balance_cache.get(user_id) {
                return Ok(Some(cached));
            }
            let generation = self.balance_cache.current_generation();
            let Some(balance) = self.load_user_balance(user_id).await? else {
                if self.balance_cache.current_generation() != generation {
                    continue;
                }
                return Ok(None);
            };
            if !self.balance_cache.insert_if_current(
                user_id.to_string(),
                generation,
                balance.clone(),
            ) {
                continue;
            }
            return Ok(Some(balance));
        }
    }

    /// Replica preflight (M7): persisted balance without the 30s dashboard cache.
    pub async fn get_user_balance_uncached(
        &self,
        user_id: &str,
    ) -> Result<Option<UserBalance>, String> {
        self.load_user_balance(user_id).await
    }

    /// DPT-BP1: one read through `cache`. The decision uses the value read even
    /// when a concurrent invalidation prevents publishing it, so a miss costs
    /// exactly one query.
    pub(super) async fn spend_preflight_balance<F, Fut>(
        cache: &crate::db_cache::BalanceCache,
        key: &str,
        load: F,
    ) -> Result<Option<UserBalance>, BillingError>
    where
        F: FnOnce() -> Fut,
        Fut: Future<Output = Result<Option<UserBalance>, String>>,
    {
        if let Some(cached) = cache.get(key) {
            return Ok(Some(cached));
        }
        let generation = cache.current_generation();
        let loaded = load()
            .await
            .map_err(|e| BillingError::new(BillingErrorKind::Internal, e))?;
        if let Some(balance) = &loaded {
            cache.insert_if_current(key.to_string(), generation, balance.clone());
        }
        Ok(loaded)
    }

    pub async fn ensure_user_can_spend(&self, user_id: &str) -> Result<(), BillingError> {
        let balance = Self::spend_preflight_balance(&self.balance_cache, user_id, || {
            self.load_user_balance(user_id)
        })
        .await?
        .ok_or_else(|| BillingError::new(BillingErrorKind::NotFound, "user not found"))?;
        if balance.balance_unlimited || balance.balance_nano_usd > 0 {
            return Ok(());
        }
        Err(BillingError::new(
            BillingErrorKind::InsufficientBalance,
            "insufficient balance",
        ))
    }

    pub async fn charge_user_balance_nano(
        &self,
        user_id: &str,
        amount_nano_usd: i128,
        meta: &Value,
    ) -> Result<PlanChargeAllocation, BillingError> {
        if amount_nano_usd <= 0 {
            return Ok(PlanChargeAllocation {
                adjusted_charge_nano_usd: 0,
                plan_covered_nano_usd: 0,
                fallback_nano_usd: 0,
                subscription_id: None,
                plan_id: None,
                multiplier: None,
            });
        }
        if meta
            .get("request_id")
            .and_then(Value::as_str)
            .is_none_or(|request_id| request_id.trim().is_empty())
        {
            return Err(BillingError::new(
                BillingErrorKind::Internal,
                "request charge metadata is missing request_id",
            ));
        }
        self.charge_user_balance_nano_inner(user_id, amount_nano_usd, meta)
            .await
    }

    pub(super) async fn charge_user_balance_nano_inner(
        &self,
        user_id: &str,
        amount_nano_usd: i128,
        meta: &Value,
    ) -> Result<PlanChargeAllocation, BillingError> {
        let _write_guard = self.db.write().await;
        let tx = _write_guard
            .begin()
            .await
            .map_err(|e| BillingError::new(BillingErrorKind::Internal, e.to_string()))?;
        let user = self.lock_user_balance_tx(&tx, user_id).await?;
        if user.unlimited {
            tx.commit()
                .await
                .map_err(|e| BillingError::new(BillingErrorKind::Internal, e.to_string()))?;
            return Ok(PlanChargeAllocation {
                adjusted_charge_nano_usd: amount_nano_usd,
                plan_covered_nano_usd: 0,
                fallback_nano_usd: 0,
                subscription_id: None,
                plan_id: None,
                multiplier: None,
            });
        }
        let request_id = meta
            .get("request_id")
            .and_then(Value::as_str)
            .expect("validated request_id");
        let api_key_id = meta.get("api_key_id").and_then(Value::as_str);
        let billing_group_id = meta.get("billing_group_id").and_then(Value::as_str);
        let now = Utc::now();
        let allocation = self
            .allocate_plan_charge_tx(
                &tx,
                PlanChargeRequest {
                    user_id,
                    api_key_id,
                    billing_group_id,
                    request_id,
                    settled_charge_nano_usd: amount_nano_usd,
                    now,
                },
            )
            .await?;
        let next_balance = user
            .balance
            .checked_sub(allocation.fallback_nano_usd)
            .ok_or_else(|| {
                BillingError::new(BillingErrorKind::Overflow, "balance subtraction overflow")
            })?;
        let now_raw = now.to_rfc3339();
        if allocation.fallback_nano_usd > 0 {
            tx.execute(self.db.stmt(
                "UPDATE users SET balance_nano_usd = $1, updated_at = $2 WHERE id = $3",
                vec![
                    next_balance.to_string().into(),
                    now_raw.clone().into(),
                    user_id.into(),
                ],
            ))
            .await
            .map_err(|e| BillingError::new(BillingErrorKind::Internal, e.to_string()))?;
        }
        let ledger_meta = plan_charge_meta(meta, &allocation);

        self.insert_billing_ledger_tx(
            &tx,
            BillingLedgerEntry {
                user_id,
                kind: "request_charge",
                delta_nano_usd: -allocation.fallback_nano_usd,
                balance_after_nano_usd: Some(next_balance),
                meta: &ledger_meta,
                created_at_rfc3339: &now_raw,
            },
        )
        .await?;

        tx.commit()
            .await
            .map_err(|e| BillingError::new(BillingErrorKind::Internal, e.to_string()))?;
        self.balance_cache.invalidate(user_id);
        Ok(allocation)
    }

    pub async fn admin_adjust_user_balance(
        &self,
        user_id: &str,
        balance_nano_usd: Option<String>,
        balance_unlimited: Option<bool>,
        actor_user_id: &str,
    ) -> Result<(), String> {
        if balance_nano_usd.is_none() && balance_unlimited.is_none() {
            return Ok(());
        }

        let _write_guard = self.db.write().await;
        let tx = _write_guard.begin().await.map_err(|e| e.to_string())?;
        let current = self
            .lock_user_balance_tx(&tx, user_id)
            .await
            .map_err(|e| e.message)?;
        let current_balance = current.balance;
        let current_unlimited = current.unlimited;

        let new_balance = if let Some(balance_raw) = balance_nano_usd {
            parse_nano_usd(&balance_raw)?
        } else {
            current_balance
        };
        let new_unlimited = balance_unlimited.unwrap_or(current_unlimited);

        let now = Utc::now().to_rfc3339();
        tx.execute(self.db.stmt(
            "UPDATE users SET balance_nano_usd = $1, balance_unlimited = $2, updated_at = $3 WHERE id = $4",
            vec![
                new_balance.to_string().into(),
                SeaValue::Int(Some(if new_unlimited { 1 } else { 0 })),
                now.clone().into(),
                user_id.into(),
            ],
        ))
        .await
        .map_err(|e| e.to_string())?;

        let delta = new_balance
            .checked_sub(current_balance)
            .ok_or_else(|| "balance delta overflow".to_string())?;
        let meta = serde_json::json!({
            "actor_user_id": actor_user_id,
            "before_balance_nano_usd": current_balance.to_string(),
            "after_balance_nano_usd": new_balance.to_string(),
            "before_balance_unlimited": current_unlimited,
            "after_balance_unlimited": new_unlimited,
        });

        self.insert_billing_ledger_tx(
            &tx,
            BillingLedgerEntry {
                user_id,
                kind: "admin_adjustment",
                delta_nano_usd: delta,
                balance_after_nano_usd: Some(new_balance),
                meta: &meta,
                created_at_rfc3339: &now,
            },
        )
        .await
        .map_err(|e| e.message)?;

        tx.commit().await.map_err(|e| e.to_string())?;
        self.balance_cache.invalidate(user_id);
        Ok(())
    }

    pub(crate) async fn insert_billing_ledger_tx(
        &self,
        tx: &DatabaseTransaction,
        entry: BillingLedgerEntry<'_>,
    ) -> Result<(), BillingError> {
        let BillingLedgerEntry {
            user_id,
            kind,
            delta_nano_usd,
            balance_after_nano_usd,
            meta,
            created_at_rfc3339,
        } = entry;
        let id = uuid::Uuid::new_v4().to_string();
        tx.execute(self.db.stmt(
            r#"INSERT INTO billing_ledger (id, user_id, kind, delta_nano_usd, balance_after_nano_usd, meta_json, created_at)
               VALUES ($1, $2, $3, $4, $5, $6, $7)"#,
            vec![
                id.into(),
                user_id.into(),
                kind.into(),
                delta_nano_usd.to_string().into(),
                balance_after_nano_usd.map(|v| v.to_string()).into(),
                meta.to_string().into(),
                created_at_rfc3339.into(),
            ],
        ))
        .await
        .map_err(|e| BillingError::new(BillingErrorKind::Internal, e.to_string()))?;
        Ok(())
    }
}
