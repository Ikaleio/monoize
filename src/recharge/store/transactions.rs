use super::*;

struct RechargeLedgerInsert<'a> {
    user_id: &'a str,
    kind: &'a str,
    delta_nano_usd: i128,
    balance_after_nano_usd: i128,
    meta: &'a Value,
    idempotency_key: &'a str,
    created_at: &'a str,
}

impl UserStore {
    // ------------------------------------------------------------------
    // Notification processing (§6)
    // ------------------------------------------------------------------

    /// RC-N4..RC-N11: the exactly-once credit transaction. The caller maps
    /// `NotifyOutcome` onto the adapter ack; an `Err` maps to HTTP 500 with an
    /// empty body so the provider retries (RC-N10).
    pub async fn apply_verified_notification(
        &self,
        channel_id: &str,
        verified: &VerifiedNotification,
    ) -> Result<NotifyOutcome, String> {
        let tx = self.db.begin_write().await.map_err(|e| e.to_string())?;

        let Some(order) = self.lock_recharge_order_tx(&tx, &verified.order_id).await? else {
            tx.rollback().await.map_err(|e| e.to_string())?;
            return Ok(NotifyOutcome::UnknownOrder);
        };
        // RC-N4: the order must belong to the notified channel.
        if order.payment_channel_id != channel_id {
            tx.rollback().await.map_err(|e| e.to_string())?;
            return Ok(NotifyOutcome::UnknownOrder);
        }

        match verified.result {
            NotifyResult::Success => {
                self.apply_success_notification_tx(tx, &order, verified)
                    .await
            }
            NotifyResult::Failure => match order.status.as_str() {
                // RC-N11.
                "pending" => {
                    self.update_order_status_tx(
                        &tx,
                        &order.id,
                        "failed",
                        Some("provider_failure"),
                        None,
                    )
                    .await?;
                    tx.commit().await.map_err(|e| e.to_string())?;
                    Ok(NotifyOutcome::FailedRecorded)
                }
                _ => {
                    tx.rollback().await.map_err(|e| e.to_string())?;
                    Ok(NotifyOutcome::Duplicate)
                }
            },
            NotifyResult::Expired => match order.status.as_str() {
                "pending" => {
                    self.update_order_status_tx(&tx, &order.id, "expired", None, None)
                        .await?;
                    tx.commit().await.map_err(|e| e.to_string())?;
                    Ok(NotifyOutcome::FailedRecorded)
                }
                _ => {
                    tx.rollback().await.map_err(|e| e.to_string())?;
                    Ok(NotifyOutcome::Duplicate)
                }
            },
        }
    }

    async fn apply_success_notification_tx(
        &self,
        tx: crate::db::WriteTransaction,
        order: &RechargeOrder,
        verified: &VerifiedNotification,
    ) -> Result<NotifyOutcome, String> {
        // RC-N5 amount check before the success transition.
        let paid_fields = verified
            .paid_amount
            .as_ref()
            .zip(verified.paid_currency.as_ref());
        if let Some((paid_amount, paid_currency)) = paid_fields {
            let amount_matches = paid_currency == &order.pay_currency
                && decimals_equal(paid_amount, &order.pay_amount);
            if !amount_matches {
                return match order.status.as_str() {
                    "pending" | "expired" => {
                        let mut meta = order.meta_json.clone();
                        if let Some(object) = meta.as_object_mut() {
                            object.insert(
                                "mismatch".to_string(),
                                serde_json::json!({
                                    "paid_amount": paid_amount,
                                    "paid_currency": paid_currency,
                                }),
                            );
                        }
                        self.update_order_status_tx(
                            &tx,
                            &order.id,
                            "failed",
                            Some("amount_mismatch"),
                            Some(&meta),
                        )
                        .await?;
                        tx.commit().await.map_err(|e| e.to_string())?;
                        Ok(NotifyOutcome::FailedRecorded)
                    }
                    _ => {
                        tx.rollback().await.map_err(|e| e.to_string())?;
                        Ok(NotifyOutcome::Duplicate)
                    }
                };
            }
        } else {
            tx.rollback().await.map_err(|e| e.to_string())?;
            return Err("successful notification omitted paid amount or currency".to_string());
        }

        match order.status.as_str() {
            // RC-N6 step 2: provider replays are idempotent.
            "succeeded" | "refunded" => {
                tx.rollback().await.map_err(|e| e.to_string())?;
                Ok(NotifyOutcome::Duplicate)
            }
            // RC-N6 step 3: `failed` is terminal; record for audit.
            "failed" => {
                let mut meta = order.meta_json.clone();
                if let Some(object) = meta.as_object_mut() {
                    object.insert(
                        "late_notification".to_string(),
                        serde_json::json!({
                            "result": "success",
                            "provider_order_id": verified.provider_order_id,
                        }),
                    );
                }
                self.update_order_meta_tx(&tx, &order.id, &meta).await?;
                tx.commit().await.map_err(|e| e.to_string())?;
                Ok(NotifyOutcome::FailedRecorded)
            }
            // RC-N6 step 4: the single credit transaction.
            "pending" | "expired" => self.credit_order_tx(tx, order, verified).await,
            other => {
                tx.rollback().await.map_err(|e| e.to_string())?;
                Err(format!("invalid stored order status {other:?}"))
            }
        }
    }

    /// RC-N6 step 4 body. Consumes the transaction so every exit path either
    /// commits or rolls back exactly once.
    async fn credit_order_tx(
        &self,
        tx: crate::db::WriteTransaction,
        order: &RechargeOrder,
        verified: &VerifiedNotification,
    ) -> Result<NotifyOutcome, String> {
        let now = Utc::now().to_rfc3339();

        let locked_user = self
            .lock_user_balance_tx(&tx, &order.user_id)
            .await
            .map(Some)
            .or_else(|error| {
                if error.kind == crate::users::BillingErrorKind::NotFound {
                    Ok(None)
                } else {
                    Err(error.message)
                }
            })?;
        // RC-N9: deleted user — roll back, then record terminal failure.
        let Some(user) = locked_user else {
            tx.rollback().await.map_err(|e| e.to_string())?;
            self.mark_order_failed(&order.id, "user_deleted").await?;
            return Ok(NotifyOutcome::FailedRecorded);
        };

        // RC-N7: unlimited users still receive the finite credit.
        let new_balance = user
            .balance
            .checked_add(order.credit_nano_usd)
            .ok_or_else(|| "balance addition overflow".to_string())?;

        tx.execute(self.db.stmt(
            "UPDATE recharge_orders SET status = 'succeeded', paid_at = $1, \
             provider_order_id = COALESCE($2, provider_order_id), error_code = NULL, \
             updated_at = $1 WHERE id = $3",
            vec![
                now.clone().into(),
                verified.provider_order_id.clone().into(),
                order.id.clone().into(),
            ],
        ))
        .await
        .map_err(|e| e.to_string())?;

        tx.execute(self.db.stmt(
            "UPDATE users SET balance_nano_usd = $1, updated_at = $2 WHERE id = $3",
            vec![
                new_balance.to_string().into(),
                now.clone().into(),
                order.user_id.clone().into(),
            ],
        ))
        .await
        .map_err(|e| e.to_string())?;

        let meta = ledger_meta(order, verified.provider_order_id.as_deref(), None);
        let inserted = self
            .insert_recharge_ledger_tx(
                &tx,
                RechargeLedgerInsert {
                    user_id: &order.user_id,
                    kind: "recharge",
                    delta_nano_usd: order.credit_nano_usd,
                    balance_after_nano_usd: new_balance,
                    meta: &meta,
                    idempotency_key: &format!("recharge:{}", order.id),
                    created_at: &now,
                },
            )
            .await?;
        // RC-N8: the unique idempotency key is the independent second
        // barrier — a conflict rolls back the entire transaction.
        if !inserted {
            tx.rollback().await.map_err(|e| e.to_string())?;
            return Ok(NotifyOutcome::Duplicate);
        }

        tx.commit().await.map_err(|e| e.to_string())?;
        self.balance_cache.invalidate(&order.user_id);
        Ok(NotifyOutcome::Credited)
    }

    // ------------------------------------------------------------------
    // Expiry sweeper (§7)
    // ------------------------------------------------------------------

    /// RC-X2: one sweep tick. Writes no ledger row and mutates no balance.
    pub async fn expire_due_recharge_orders(&self) -> Result<u64, String> {
        let now = Utc::now().to_rfc3339();
        let write = self.db.write().await;
        let result = write
            .execute(self.db.stmt(
                "UPDATE recharge_orders SET status = 'expired', updated_at = $1 \
                 WHERE status = 'pending' AND expires_at <= $1",
                vec![now.into()],
            ))
            .await
            .map_err(|e| e.to_string())?;
        Ok(result.rows_affected())
    }

    // ------------------------------------------------------------------
    // Refunds (§8)
    // ------------------------------------------------------------------

    /// RC-R5: the full-order refund transaction. Returns `Ok(false)` when the
    /// locked order is not `succeeded` (concurrent refund / invalid state).
    pub async fn refund_recharge_order(
        &self,
        order_id: &str,
        actor_user_id: &str,
        manual: bool,
    ) -> Result<bool, String> {
        let tx = self.db.begin_write().await.map_err(|e| e.to_string())?;

        let Some(order) = self.lock_recharge_order_tx(&tx, order_id).await? else {
            tx.rollback().await.map_err(|e| e.to_string())?;
            return Ok(false);
        };
        if order.status != "succeeded" {
            tx.rollback().await.map_err(|e| e.to_string())?;
            return Ok(false);
        }

        let locked_user = self
            .lock_user_balance_tx(&tx, &order.user_id)
            .await
            .map(Some)
            .or_else(|error| {
                if error.kind == crate::users::BillingErrorKind::NotFound {
                    Ok(None)
                } else {
                    Err(error.message)
                }
            })?;
        // RC-R5: a refund of a deleted user's order writes nothing.
        let Some(user) = locked_user else {
            tx.rollback().await.map_err(|e| e.to_string())?;
            return Ok(false);
        };

        // RC-R5 step 3: the result may be negative (debt is representable).
        let new_balance = user
            .balance
            .checked_sub(order.credit_nano_usd)
            .ok_or_else(|| "balance subtraction overflow".to_string())?;

        let now = Utc::now().to_rfc3339();
        tx.execute(self.db.stmt(
            "UPDATE recharge_orders SET status = 'refunded', updated_at = $1 WHERE id = $2",
            vec![now.clone().into(), order.id.clone().into()],
        ))
        .await
        .map_err(|e| e.to_string())?;
        tx.execute(self.db.stmt(
            "UPDATE users SET balance_nano_usd = $1, updated_at = $2 WHERE id = $3",
            vec![
                new_balance.to_string().into(),
                now.clone().into(),
                order.user_id.clone().into(),
            ],
        ))
        .await
        .map_err(|e| e.to_string())?;

        let meta = ledger_meta(
            &order,
            order.provider_order_id.as_deref(),
            Some((actor_user_id, manual)),
        );
        let inserted = self
            .insert_recharge_ledger_tx(
                &tx,
                RechargeLedgerInsert {
                    user_id: &order.user_id,
                    kind: "recharge_refund",
                    delta_nano_usd: order
                        .credit_nano_usd
                        .checked_neg()
                        .ok_or_else(|| "refund delta overflow".to_string())?,
                    balance_after_nano_usd: new_balance,
                    meta: &meta,
                    idempotency_key: &format!("recharge_refund:{}", order.id),
                    created_at: &now,
                },
            )
            .await?;
        if !inserted {
            tx.rollback().await.map_err(|e| e.to_string())?;
            return Ok(false);
        }

        tx.commit().await.map_err(|e| e.to_string())?;
        self.balance_cache.invalidate(&order.user_id);
        Ok(true)
    }

    // ------------------------------------------------------------------
    // Internal helpers
    // ------------------------------------------------------------------

    async fn lock_recharge_order_tx(
        &self,
        tx: &DatabaseTransaction,
        order_id: &str,
    ) -> Result<Option<RechargeOrder>, String> {
        let lock_suffix = if self.db.is_postgres() {
            " FOR UPDATE"
        } else {
            ""
        };
        let row = tx
            .query_one(self.db.stmt(
                &format!(
                    "SELECT {} FROM recharge_orders o WHERE o.id = $1{lock_suffix}",
                    ORDER_COLUMNS
                ),
                vec![order_id.into()],
            ))
            .await
            .map_err(|e| e.to_string())?;
        row.as_ref()
            .map(|row| order_from_row(row, false))
            .transpose()
    }

    async fn update_order_status_tx(
        &self,
        tx: &DatabaseTransaction,
        order_id: &str,
        status: &str,
        error_code: Option<&str>,
        meta: Option<&Value>,
    ) -> Result<(), String> {
        let now = Utc::now().to_rfc3339();
        match meta {
            Some(meta) => {
                tx.execute(self.db.stmt(
                    "UPDATE recharge_orders SET status = $1, error_code = $2, meta_json = $3, \
                     updated_at = $4 WHERE id = $5",
                    vec![
                        status.into(),
                        error_code.map(str::to_string).into(),
                        meta.to_string().into(),
                        now.into(),
                        order_id.into(),
                    ],
                ))
                .await
                .map_err(|e| e.to_string())?;
            }
            None => {
                tx.execute(self.db.stmt(
                    "UPDATE recharge_orders SET status = $1, error_code = $2, updated_at = $3 \
                     WHERE id = $4",
                    vec![
                        status.into(),
                        error_code.map(str::to_string).into(),
                        now.into(),
                        order_id.into(),
                    ],
                ))
                .await
                .map_err(|e| e.to_string())?;
            }
        }
        Ok(())
    }

    async fn update_order_meta_tx(
        &self,
        tx: &DatabaseTransaction,
        order_id: &str,
        meta: &Value,
    ) -> Result<(), String> {
        tx.execute(self.db.stmt(
            "UPDATE recharge_orders SET meta_json = $1, updated_at = $2 WHERE id = $3",
            vec![
                meta.to_string().into(),
                Utc::now().to_rfc3339().into(),
                order_id.into(),
            ],
        ))
        .await
        .map_err(|e| e.to_string())?;
        Ok(())
    }

    /// RC-L3 ledger insert. Returns `false` when the idempotency key already
    /// exists, in which case the caller must roll back the transaction.
    async fn insert_recharge_ledger_tx(
        &self,
        tx: &DatabaseTransaction,
        entry: RechargeLedgerInsert<'_>,
    ) -> Result<bool, String> {
        let RechargeLedgerInsert {
            user_id,
            kind,
            delta_nano_usd,
            balance_after_nano_usd,
            meta,
            idempotency_key,
            created_at,
        } = entry;
        let result = tx
            .execute(self.db.stmt(
                "INSERT INTO billing_ledger (id, user_id, kind, delta_nano_usd, \
                 balance_after_nano_usd, meta_json, created_at, idempotency_key) \
                 VALUES ($1, $2, $3, $4, $5, $6, $7, $8) \
                 ON CONFLICT(idempotency_key) WHERE idempotency_key IS NOT NULL DO NOTHING",
                vec![
                    uuid::Uuid::new_v4().to_string().into(),
                    user_id.into(),
                    kind.into(),
                    delta_nano_usd.to_string().into(),
                    balance_after_nano_usd.to_string().into(),
                    meta.to_string().into(),
                    created_at.into(),
                    idempotency_key.into(),
                ],
            ))
            .await
            .map_err(|e| e.to_string())?;
        Ok(result.rows_affected() > 0)
    }
}
