use super::*;

impl UserStore {
    pub async fn charge_sub_account_balance_nano(
        &self,
        api_key_id: &str,
        user_id: &str,
        amount_nano_usd: i128,
        meta: &Value,
    ) -> Result<PlanChargeAllocation, BillingError> {
        if let Some(allocation) = sub_account_charge_preflight(amount_nano_usd, meta)? {
            return Ok(allocation);
        }
        let _write_guard = self.db.write().await;
        let tx = _write_guard
            .begin()
            .await
            .map_err(|e| BillingError::new(BillingErrorKind::Internal, e.to_string()))?;

        let user = self.lock_user_balance_tx(&tx, user_id).await?;
        let key = match self.lock_api_key_balance_tx(&tx, api_key_id, user_id).await {
            Ok(key) => Some(key),
            Err(error) if error.kind == BillingErrorKind::NotFound => None,
            Err(error) => return Err(error),
        };
        if key.as_ref().is_none_or(|key| !key.sub_account_enabled) && user.unlimited {
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
        let billing_group_id = meta.get("billing_group_id").and_then(Value::as_str);
        let now = Utc::now();
        let allocation = self
            .allocate_plan_charge_tx(
                &tx,
                crate::users::plans::PlanChargeRequest {
                    user_id,
                    api_key_id: Some(api_key_id),
                    billing_group_id,
                    request_id,
                    settled_charge_nano_usd: amount_nano_usd,
                    now,
                },
            )
            .await?;
        let now_raw = now.to_rfc3339();
        let ledger_meta = plan_charge_meta(meta, &allocation);
        if key.as_ref().is_none_or(|key| !key.sub_account_enabled) {
            self.charge_sub_account_fallback(
                &tx,
                SubAccountCharge {
                    user_id,
                    user,
                    allocation: &allocation,
                    ledger_meta: &ledger_meta,
                    now_raw: &now_raw,
                },
            )
            .await?;
            tx.commit()
                .await
                .map_err(|e| BillingError::new(BillingErrorKind::Internal, e.to_string()))?;
            self.balance_cache.invalidate(user_id);
            self.api_key_cache.invalidate_by_key_id(api_key_id);
            return Ok(allocation);
        }
        let key = key.expect("enabled sub-account key must be present");
        let next_balance = key
            .balance
            .checked_sub(allocation.fallback_nano_usd)
            .ok_or_else(|| {
                BillingError::new(
                    BillingErrorKind::Overflow,
                    "sub-account balance subtraction overflow",
                )
            })?;
        if allocation.fallback_nano_usd > 0 {
            tx.execute(self.db.stmt(
                "UPDATE api_keys SET sub_account_balance_nano = $1 WHERE id = $2",
                vec![next_balance.to_string().into(), api_key_id.into()],
            ))
            .await
            .map_err(|e| BillingError::new(BillingErrorKind::Internal, e.to_string()))?;
        }

        self.insert_billing_ledger_tx(
            &tx,
            BillingLedgerEntry {
                user_id,
                kind: "api_key_charge",
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
        self.sub_account_balance_cache.invalidate(api_key_id);
        self.api_key_cache.invalidate_by_key_id(api_key_id);
        Ok(allocation)
    }

    pub async fn transfer_to_sub_account(
        &self,
        api_key_id: &str,
        user_id: &str,
        amount_nano_usd: i128,
    ) -> Result<(i128, i128), BillingError> {
        if amount_nano_usd <= 0 {
            return Err(BillingError::new(
                BillingErrorKind::Internal,
                "transfer amount must be positive",
            ));
        }

        let _write_guard = self.db.write().await;
        let tx = _write_guard
            .begin()
            .await
            .map_err(|e| BillingError::new(BillingErrorKind::Internal, e.to_string()))?;

        let user = self.lock_user_balance_tx(&tx, user_id).await?;
        let key = self
            .lock_api_key_balance_tx(&tx, api_key_id, user_id)
            .await?;
        if !key.sub_account_enabled {
            return Err(BillingError::new(
                BillingErrorKind::Internal,
                "sub-account not enabled on this key",
            ));
        }

        let new_user_balance = if user.unlimited {
            user.balance
        } else {
            let next = user.balance.checked_sub(amount_nano_usd).ok_or_else(|| {
                BillingError::new(BillingErrorKind::Overflow, "user balance overflow")
            })?;
            if next < 0 {
                return Err(BillingError::new(
                    BillingErrorKind::InsufficientBalance,
                    "insufficient balance for transfer",
                ));
            }
            tx.execute(self.db.stmt(
                "UPDATE users SET balance_nano_usd = $1 WHERE id = $2",
                vec![next.to_string().into(), user_id.into()],
            ))
            .await
            .map_err(|e| BillingError::new(BillingErrorKind::Internal, e.to_string()))?;
            next
        };

        let new_key_balance = key.balance.checked_add(amount_nano_usd).ok_or_else(|| {
            BillingError::new(BillingErrorKind::Overflow, "sub-account balance overflow")
        })?;

        tx.execute(self.db.stmt(
            "UPDATE api_keys SET sub_account_balance_nano = $1 WHERE id = $2",
            vec![new_key_balance.to_string().into(), api_key_id.into()],
        ))
        .await
        .map_err(|e| BillingError::new(BillingErrorKind::Internal, e.to_string()))?;

        let now = Utc::now().to_rfc3339();
        if !user.unlimited {
            self.insert_billing_ledger_tx(
                &tx,
                BillingLedgerEntry {
                    user_id,
                    kind: "sub_account_transfer_out",
                    delta_nano_usd: -amount_nano_usd,
                    balance_after_nano_usd: Some(new_user_balance),
                    meta: &serde_json::json!({ "api_key_id": api_key_id }),
                    created_at_rfc3339: &now,
                },
            )
            .await?;
        }
        self.insert_billing_ledger_tx(
            &tx,
            BillingLedgerEntry {
                user_id,
                kind: "sub_account_transfer_in",
                delta_nano_usd: amount_nano_usd,
                balance_after_nano_usd: Some(new_key_balance),
                meta: &serde_json::json!({ "api_key_id": api_key_id }),
                created_at_rfc3339: &now,
            },
        )
        .await?;

        tx.commit()
            .await
            .map_err(|e| BillingError::new(BillingErrorKind::Internal, e.to_string()))?;
        self.balance_cache.invalidate(user_id);
        self.sub_account_balance_cache.invalidate(api_key_id);
        self.api_key_cache.invalidate_by_key_id(api_key_id);
        Ok((new_key_balance, new_user_balance))
    }

    pub(super) async fn load_sub_account_balance(
        &self,
        api_key_id: &str,
    ) -> Result<Option<UserBalance>, String> {
        let row = self
            .db
            .read()
            .query_one(self.db.stmt(
                "SELECT user_id, sub_account_balance_nano FROM api_keys WHERE id = $1",
                vec![api_key_id.into()],
            ))
            .await
            .map_err(|e| e.to_string())?;
        row.map(|row| {
            let raw: String = row
                .try_get("", "sub_account_balance_nano")
                .map_err(|e| e.to_string())?;
            Ok(UserBalance {
                user_id: row.try_get("", "user_id").map_err(|e| e.to_string())?,
                balance_nano_usd: parse_nano_usd(&raw)?,
                balance_unlimited: false,
            })
        })
        .transpose()
    }

    /// DPT-BP2: sub-account balance through `sub_account_balance_cache`.
    pub async fn ensure_sub_account_can_spend(&self, api_key_id: &str) -> Result<(), BillingError> {
        let balance =
            Self::spend_preflight_balance(&self.sub_account_balance_cache, api_key_id, || {
                self.load_sub_account_balance(api_key_id)
            })
            .await?
            .ok_or_else(|| BillingError::new(BillingErrorKind::NotFound, "api key not found"))?;
        if balance.balance_nano_usd <= 0 {
            return Err(BillingError::new(
                BillingErrorKind::InsufficientBalance,
                "insufficient balance",
            ));
        }
        Ok(())
    }
}

struct SubAccountCharge<'a> {
    user_id: &'a str,
    user: LockedUserBalance,
    allocation: &'a PlanChargeAllocation,
    ledger_meta: &'a Value,
    now_raw: &'a str,
}
impl UserStore {
    async fn charge_sub_account_fallback(
        &self,
        tx: &DatabaseTransaction,
        charge: SubAccountCharge<'_>,
    ) -> Result<(), BillingError> {
        let SubAccountCharge {
            user_id,
            user,
            allocation,
            ledger_meta,
            now_raw,
        } = charge;
        let next_balance = user
            .balance
            .checked_sub(allocation.fallback_nano_usd)
            .ok_or_else(|| {
                BillingError::new(BillingErrorKind::Overflow, "balance subtraction overflow")
            })?;
        if allocation.fallback_nano_usd > 0 {
            tx.execute(self.db.stmt(
                "UPDATE users SET balance_nano_usd = $1, updated_at = $2 WHERE id = $3",
                vec![
                    next_balance.to_string().into(),
                    now_raw.into(),
                    user_id.into(),
                ],
            ))
            .await
            .map_err(|e| BillingError::new(BillingErrorKind::Internal, e.to_string()))?;
        }
        self.insert_billing_ledger_tx(
            tx,
            BillingLedgerEntry {
                user_id,
                kind: "request_charge",
                delta_nano_usd: -allocation.fallback_nano_usd,
                balance_after_nano_usd: Some(next_balance),
                meta: ledger_meta,
                created_at_rfc3339: now_raw,
            },
        )
        .await?;
        Ok(())
    }
}

fn sub_account_charge_preflight(
    amount_nano_usd: i128,
    meta: &Value,
) -> Result<Option<PlanChargeAllocation>, BillingError> {
    if amount_nano_usd <= 0 {
        return Ok(Some(PlanChargeAllocation {
            adjusted_charge_nano_usd: 0,
            plan_covered_nano_usd: 0,
            fallback_nano_usd: 0,
            subscription_id: None,
            plan_id: None,
            multiplier: None,
        }));
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
    Ok(None)
}
