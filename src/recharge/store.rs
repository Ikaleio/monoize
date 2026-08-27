//! Recharge storage layer (`recharge-system.spec.md` §3, §6, §7, §8, §9).
//! Every balance mutation runs on the billing write path
//! (`user-billing-and-model-metadata.spec.md` §6a): the single-connection
//! write pool on SQLite, `SELECT ... FOR UPDATE` row locks on PostgreSQL.

use crate::recharge::amount::decimals_equal;
use crate::recharge::{NotifyResult, VerifiedNotification};
use crate::users::UserStore;
use chrono::Utc;
use sea_orm::{ConnectionTrait, DatabaseTransaction, QueryResult, Value as SeaValue};
use serde_json::Value;

#[derive(Debug, Clone)]
pub struct RechargeChannel {
    pub id: String,
    pub name: String,
    pub type_id: String,
    pub enabled: bool,
    pub currency: String,
    pub usd_rate: String,
    pub min_credit_usd: String,
    pub max_credit_usd: String,
    pub config: Value,
    pub sort_order: i32,
    pub created_at: String,
    pub updated_at: String,
}

#[derive(Debug, Clone)]
pub struct RechargeOrder {
    pub id: String,
    pub user_id: String,
    pub payment_channel_id: String,
    pub channel_type_id: String,
    pub channel_name: String,
    pub status: String,
    pub credit_nano_usd: i128,
    pub pay_currency: String,
    pub pay_amount: String,
    pub usd_rate: String,
    pub provider_order_id: Option<String>,
    pub error_code: Option<String>,
    pub paid_at: Option<String>,
    pub expires_at: String,
    pub meta_json: Value,
    pub created_at: String,
    pub updated_at: String,
    /// Joined `users.username`; `None` after user deletion (RC-A3).
    pub username: Option<String>,
}

#[derive(Debug, Clone)]
pub struct LedgerEntry {
    pub id: String,
    pub user_id: String,
    pub username: Option<String>,
    pub kind: String,
    pub delta_nano_usd: i128,
    pub balance_after_nano_usd: Option<i128>,
    pub meta_json: Value,
    pub created_at: String,
}

/// Outcome of `apply_verified_notification`, mapped by the handler onto the
/// adapter's RC-P2 ack vocabulary.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum NotifyOutcome {
    Credited,
    Duplicate,
    FailedRecorded,
    UnknownOrder,
}

#[derive(Debug, Clone, Default)]
pub struct OrderListFilter {
    pub user_id: Option<String>,
    pub status: Option<String>,
    pub username: Option<String>,
    pub limit: u64,
    pub offset: u64,
}

#[derive(Debug, Clone, Default)]
pub struct LedgerListFilter {
    pub user_id: Option<String>,
    pub kinds: Option<Vec<String>>,
    pub username: Option<String>,
    pub limit: u64,
    pub offset: u64,
}

#[derive(Debug)]
pub enum ChannelWriteError {
    NameExists,
    Internal(String),
}

fn channel_from_row(row: &QueryResult) -> Result<RechargeChannel, String> {
    let config_raw: String = row.try_get("", "config_json").map_err(|e| e.to_string())?;
    Ok(RechargeChannel {
        id: row.try_get("", "id").map_err(|e| e.to_string())?,
        name: row.try_get("", "name").map_err(|e| e.to_string())?,
        type_id: row.try_get("", "type_id").map_err(|e| e.to_string())?,
        enabled: row.try_get::<i32>("", "enabled").map_err(|e| e.to_string())? == 1,
        currency: row.try_get("", "currency").map_err(|e| e.to_string())?,
        usd_rate: row.try_get("", "usd_rate").map_err(|e| e.to_string())?,
        min_credit_usd: row
            .try_get("", "min_credit_usd")
            .map_err(|e| e.to_string())?,
        max_credit_usd: row
            .try_get("", "max_credit_usd")
            .map_err(|e| e.to_string())?,
        config: serde_json::from_str(&config_raw).unwrap_or(Value::Null),
        sort_order: row.try_get("", "sort_order").map_err(|e| e.to_string())?,
        created_at: row.try_get("", "created_at").map_err(|e| e.to_string())?,
        updated_at: row.try_get("", "updated_at").map_err(|e| e.to_string())?,
    })
}

fn order_from_row(row: &QueryResult, with_username: bool) -> Result<RechargeOrder, String> {
    let credit_raw: String = row
        .try_get("", "credit_nano_usd")
        .map_err(|e| e.to_string())?;
    let meta_raw: String = row.try_get("", "meta_json").map_err(|e| e.to_string())?;
    Ok(RechargeOrder {
        id: row.try_get("", "id").map_err(|e| e.to_string())?,
        user_id: row.try_get("", "user_id").map_err(|e| e.to_string())?,
        payment_channel_id: row
            .try_get("", "payment_channel_id")
            .map_err(|e| e.to_string())?,
        channel_type_id: row
            .try_get("", "channel_type_id")
            .map_err(|e| e.to_string())?,
        channel_name: row.try_get("", "channel_name").map_err(|e| e.to_string())?,
        status: row.try_get("", "status").map_err(|e| e.to_string())?,
        credit_nano_usd: credit_raw
            .parse::<i128>()
            .map_err(|_| "invalid stored credit_nano_usd".to_string())?,
        pay_currency: row.try_get("", "pay_currency").map_err(|e| e.to_string())?,
        pay_amount: row.try_get("", "pay_amount").map_err(|e| e.to_string())?,
        usd_rate: row.try_get("", "usd_rate").map_err(|e| e.to_string())?,
        provider_order_id: row
            .try_get("", "provider_order_id")
            .map_err(|e| e.to_string())?,
        error_code: row.try_get("", "error_code").map_err(|e| e.to_string())?,
        paid_at: row.try_get("", "paid_at").map_err(|e| e.to_string())?,
        expires_at: row.try_get("", "expires_at").map_err(|e| e.to_string())?,
        meta_json: serde_json::from_str(&meta_raw).unwrap_or_else(|_| Value::Object(Default::default())),
        created_at: row.try_get("", "created_at").map_err(|e| e.to_string())?,
        updated_at: row.try_get("", "updated_at").map_err(|e| e.to_string())?,
        username: if with_username {
            row.try_get("", "username").ok()
        } else {
            None
        },
    })
}

const ORDER_COLUMNS: &str = "o.id, o.user_id, o.payment_channel_id, o.channel_type_id, \
    o.channel_name, o.status, o.credit_nano_usd, o.pay_currency, o.pay_amount, o.usd_rate, \
    o.provider_order_id, o.error_code, o.paid_at, o.expires_at, o.meta_json, o.created_at, \
    o.updated_at";

impl UserStore {
    // ------------------------------------------------------------------
    // Payment channels (§3.1, §9.2)
    // ------------------------------------------------------------------

    pub async fn list_payment_channels(
        &self,
        enabled_only: bool,
    ) -> Result<Vec<RechargeChannel>, String> {
        let filter = if enabled_only { "WHERE enabled = 1" } else { "" };
        let rows = self
            .db
            .read()
            .query_all(self.db.stmt(
                &format!(
                    "SELECT * FROM payment_channels {filter} \
                     ORDER BY sort_order ASC, created_at ASC"
                ),
                vec![],
            ))
            .await
            .map_err(|e| e.to_string())?;
        rows.iter().map(channel_from_row).collect()
    }

    pub async fn get_payment_channel(&self, id: &str) -> Result<Option<RechargeChannel>, String> {
        let row = self
            .db
            .read()
            .query_one(self.db.stmt(
                "SELECT * FROM payment_channels WHERE id = $1",
                vec![id.into()],
            ))
            .await
            .map_err(|e| e.to_string())?;
        row.as_ref().map(channel_from_row).transpose()
    }

    pub async fn create_payment_channel(
        &self,
        channel: &RechargeChannel,
    ) -> Result<(), ChannelWriteError> {
        let write = self.db.write().await;
        let result = write
            .execute(self.db.stmt(
                "INSERT INTO payment_channels (id, name, type_id, enabled, currency, usd_rate, \
                 min_credit_usd, max_credit_usd, config_json, sort_order, created_at, updated_at) \
                 VALUES ($1, $2, $3, $4, $5, $6, $7, $8, $9, $10, $11, $12)",
                vec![
                    channel.id.clone().into(),
                    channel.name.clone().into(),
                    channel.type_id.clone().into(),
                    SeaValue::Int(Some(if channel.enabled { 1 } else { 0 })),
                    channel.currency.clone().into(),
                    channel.usd_rate.clone().into(),
                    channel.min_credit_usd.clone().into(),
                    channel.max_credit_usd.clone().into(),
                    channel.config.to_string().into(),
                    SeaValue::Int(Some(channel.sort_order)),
                    channel.created_at.clone().into(),
                    channel.updated_at.clone().into(),
                ],
            ))
            .await;
        map_channel_write(result.map(|_| ()))
    }

    pub async fn update_payment_channel(
        &self,
        channel: &RechargeChannel,
    ) -> Result<(), ChannelWriteError> {
        let write = self.db.write().await;
        let result = write
            .execute(self.db.stmt(
                "UPDATE payment_channels SET name = $1, enabled = $2, usd_rate = $3, \
                 min_credit_usd = $4, max_credit_usd = $5, config_json = $6, sort_order = $7, \
                 updated_at = $8 WHERE id = $9",
                vec![
                    channel.name.clone().into(),
                    SeaValue::Int(Some(if channel.enabled { 1 } else { 0 })),
                    channel.usd_rate.clone().into(),
                    channel.min_credit_usd.clone().into(),
                    channel.max_credit_usd.clone().into(),
                    channel.config.to_string().into(),
                    SeaValue::Int(Some(channel.sort_order)),
                    channel.updated_at.clone().into(),
                    channel.id.clone().into(),
                ],
            ))
            .await;
        map_channel_write(result.map(|_| ()))
    }

    /// RC-D2: deleting a channel never touches `recharge_orders`.
    pub async fn delete_payment_channel(&self, id: &str) -> Result<bool, String> {
        let write = self.db.write().await;
        let result = write
            .execute(self.db.stmt(
                "DELETE FROM payment_channels WHERE id = $1",
                vec![id.into()],
            ))
            .await
            .map_err(|e| e.to_string())?;
        Ok(result.rows_affected() > 0)
    }

    // ------------------------------------------------------------------
    // Orders (§3.2, §5, §9.1)
    // ------------------------------------------------------------------

    pub async fn count_pending_orders(&self, user_id: &str) -> Result<i64, String> {
        let row = self
            .db
            .read()
            .query_one(self.db.stmt(
                "SELECT COUNT(*) AS pending_count FROM recharge_orders \
                 WHERE user_id = $1 AND status = 'pending'",
                vec![user_id.into()],
            ))
            .await
            .map_err(|e| e.to_string())?
            .ok_or_else(|| "count query returned no row".to_string())?;
        row.try_get::<i64>("", "pending_count")
            .map_err(|e| e.to_string())
    }

    pub async fn insert_recharge_order(&self, order: &RechargeOrder) -> Result<(), String> {
        let write = self.db.write().await;
        write
            .execute(self.db.stmt(
                "INSERT INTO recharge_orders (id, user_id, payment_channel_id, channel_type_id, \
                 channel_name, status, credit_nano_usd, pay_currency, pay_amount, usd_rate, \
                 provider_order_id, error_code, paid_at, expires_at, meta_json, created_at, \
                 updated_at) VALUES ($1, $2, $3, $4, $5, $6, $7, $8, $9, $10, $11, $12, $13, \
                 $14, $15, $16, $17)",
                vec![
                    order.id.clone().into(),
                    order.user_id.clone().into(),
                    order.payment_channel_id.clone().into(),
                    order.channel_type_id.clone().into(),
                    order.channel_name.clone().into(),
                    order.status.clone().into(),
                    order.credit_nano_usd.to_string().into(),
                    order.pay_currency.clone().into(),
                    order.pay_amount.clone().into(),
                    order.usd_rate.clone().into(),
                    order.provider_order_id.clone().into(),
                    order.error_code.clone().into(),
                    order.paid_at.clone().into(),
                    order.expires_at.clone().into(),
                    order.meta_json.to_string().into(),
                    order.created_at.clone().into(),
                    order.updated_at.clone().into(),
                ],
            ))
            .await
            .map_err(|e| e.to_string())?;
        Ok(())
    }

    /// RC-T1: persist the provider-assigned id before the create response.
    pub async fn set_order_provider_id(
        &self,
        order_id: &str,
        provider_order_id: &str,
    ) -> Result<(), String> {
        let write = self.db.write().await;
        write
            .execute(self.db.stmt(
                "UPDATE recharge_orders SET provider_order_id = $1, updated_at = $2 WHERE id = $3",
                vec![
                    provider_order_id.into(),
                    Utc::now().to_rfc3339().into(),
                    order_id.into(),
                ],
            ))
            .await
            .map_err(|e| e.to_string())?;
        Ok(())
    }

    /// RC-O8 / RC-N9: transition an order to terminal `failed`.
    pub async fn mark_order_failed(&self, order_id: &str, error_code: &str) -> Result<(), String> {
        let write = self.db.write().await;
        write
            .execute(self.db.stmt(
                "UPDATE recharge_orders SET status = 'failed', error_code = $1, updated_at = $2 \
                 WHERE id = $3",
                vec![
                    error_code.into(),
                    Utc::now().to_rfc3339().into(),
                    order_id.into(),
                ],
            ))
            .await
            .map_err(|e| e.to_string())?;
        Ok(())
    }

    pub async fn get_recharge_order(&self, order_id: &str) -> Result<Option<RechargeOrder>, String> {
        let row = self
            .db
            .read()
            .query_one(self.db.stmt(
                &format!(
                    "SELECT {ORDER_COLUMNS}, u.username FROM recharge_orders o \
                     LEFT JOIN users u ON u.id = o.user_id WHERE o.id = $1"
                ),
                vec![order_id.into()],
            ))
            .await
            .map_err(|e| e.to_string())?;
        row.as_ref().map(|row| order_from_row(row, true)).transpose()
    }

    pub async fn list_recharge_orders(
        &self,
        filter: &OrderListFilter,
    ) -> Result<(Vec<RechargeOrder>, i64), String> {
        let mut conditions: Vec<String> = Vec::new();
        let mut values: Vec<SeaValue> = Vec::new();
        if let Some(user_id) = &filter.user_id {
            values.push(user_id.clone().into());
            conditions.push(format!("o.user_id = ${}", values.len()));
        }
        if let Some(status) = &filter.status {
            values.push(status.clone().into());
            conditions.push(format!("o.status = ${}", values.len()));
        }
        if let Some(username) = &filter.username {
            values.push(username.clone().into());
            conditions.push(format!("u.username = ${}", values.len()));
        }
        let where_clause = if conditions.is_empty() {
            String::new()
        } else {
            format!("WHERE {}", conditions.join(" AND "))
        };

        let total_row = self
            .db
            .read()
            .query_one(self.db.stmt(
                &format!(
                    "SELECT COUNT(*) AS total FROM recharge_orders o \
                     LEFT JOIN users u ON u.id = o.user_id {where_clause}"
                ),
                values.clone(),
            ))
            .await
            .map_err(|e| e.to_string())?
            .ok_or_else(|| "count query returned no row".to_string())?;
        let total: i64 = total_row.try_get("", "total").map_err(|e| e.to_string())?;

        let mut page_values = values;
        page_values.push(SeaValue::BigUnsigned(Some(filter.limit)));
        let limit_index = page_values.len();
        page_values.push(SeaValue::BigUnsigned(Some(filter.offset)));
        let offset_index = page_values.len();
        let rows = self
            .db
            .read()
            .query_all(self.db.stmt(
                &format!(
                    "SELECT {ORDER_COLUMNS}, u.username FROM recharge_orders o \
                     LEFT JOIN users u ON u.id = o.user_id {where_clause} \
                     ORDER BY o.created_at DESC, o.id DESC LIMIT ${limit_index} OFFSET ${offset_index}"
                ),
                page_values,
            ))
            .await
            .map_err(|e| e.to_string())?;
        let orders = rows
            .iter()
            .map(|row| order_from_row(row, true))
            .collect::<Result<Vec<_>, _>>()?;
        Ok((orders, total))
    }

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
                // RC-N5 amount check before the success transition.
                if let (Some(paid_amount), Some(paid_currency)) =
                    (&verified.paid_amount, &verified.paid_currency)
                {
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
                    "pending" | "expired" => {
                        self.credit_order_tx(tx, &order, verified).await
                    }
                    other => {
                        tx.rollback().await.map_err(|e| e.to_string())?;
                        Err(format!("invalid stored order status {other:?}"))
                    }
                }
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
                &order.user_id,
                "recharge",
                order.credit_nano_usd,
                new_balance,
                &meta,
                &format!("recharge:{}", order.id),
                &now,
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
                &order.user_id,
                "recharge_refund",
                order
                    .credit_nano_usd
                    .checked_neg()
                    .ok_or_else(|| "refund delta overflow".to_string())?,
                new_balance,
                &meta,
                &format!("recharge_refund:{}", order.id),
                &now,
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
    // Ledger read surface (§9.1 RC-A5)
    // ------------------------------------------------------------------

    pub async fn list_billing_ledger(
        &self,
        filter: &LedgerListFilter,
    ) -> Result<(Vec<LedgerEntry>, i64), String> {
        let mut conditions: Vec<String> = Vec::new();
        let mut values: Vec<SeaValue> = Vec::new();
        if let Some(user_id) = &filter.user_id {
            values.push(user_id.clone().into());
            conditions.push(format!("l.user_id = ${}", values.len()));
        }
        if let Some(kinds) = &filter.kinds {
            if kinds.is_empty() {
                return Ok((Vec::new(), 0));
            }
            let placeholders = kinds
                .iter()
                .map(|kind| {
                    values.push(kind.clone().into());
                    format!("${}", values.len())
                })
                .collect::<Vec<_>>()
                .join(", ");
            conditions.push(format!("l.kind IN ({placeholders})"));
        }
        if let Some(username) = &filter.username {
            values.push(username.clone().into());
            conditions.push(format!("u.username = ${}", values.len()));
        }
        let where_clause = if conditions.is_empty() {
            String::new()
        } else {
            format!("WHERE {}", conditions.join(" AND "))
        };

        let total_row = self
            .db
            .read()
            .query_one(self.db.stmt(
                &format!(
                    "SELECT COUNT(*) AS total FROM billing_ledger l \
                     LEFT JOIN users u ON u.id = l.user_id {where_clause}"
                ),
                values.clone(),
            ))
            .await
            .map_err(|e| e.to_string())?
            .ok_or_else(|| "count query returned no row".to_string())?;
        let total: i64 = total_row.try_get("", "total").map_err(|e| e.to_string())?;

        let mut page_values = values;
        page_values.push(SeaValue::BigUnsigned(Some(filter.limit)));
        let limit_index = page_values.len();
        page_values.push(SeaValue::BigUnsigned(Some(filter.offset)));
        let offset_index = page_values.len();
        let rows = self
            .db
            .read()
            .query_all(self.db.stmt(
                &format!(
                    "SELECT l.id, l.user_id, l.kind, l.delta_nano_usd, \
                     l.balance_after_nano_usd, l.meta_json, l.created_at, u.username \
                     FROM billing_ledger l LEFT JOIN users u ON u.id = l.user_id \
                     {where_clause} ORDER BY l.created_at DESC, l.id DESC \
                     LIMIT ${limit_index} OFFSET ${offset_index}"
                ),
                page_values,
            ))
            .await
            .map_err(|e| e.to_string())?;
        let entries = rows
            .iter()
            .map(|row| {
                let delta_raw: String = row
                    .try_get("", "delta_nano_usd")
                    .map_err(|e| e.to_string())?;
                let balance_after_raw: Option<String> = row
                    .try_get("", "balance_after_nano_usd")
                    .map_err(|e| e.to_string())?;
                let meta_raw: String = row.try_get("", "meta_json").map_err(|e| e.to_string())?;
                Ok(LedgerEntry {
                    id: row.try_get("", "id").map_err(|e: sea_orm::DbErr| e.to_string())?,
                    user_id: row.try_get("", "user_id").map_err(|e| e.to_string())?,
                    username: row.try_get("", "username").ok(),
                    kind: row.try_get("", "kind").map_err(|e| e.to_string())?,
                    delta_nano_usd: delta_raw
                        .parse::<i128>()
                        .map_err(|_| "invalid stored delta_nano_usd".to_string())?,
                    balance_after_nano_usd: balance_after_raw
                        .map(|raw| raw.parse::<i128>())
                        .transpose()
                        .map_err(|_| "invalid stored balance_after_nano_usd".to_string())?,
                    meta_json: serde_json::from_str(&meta_raw)
                        .unwrap_or_else(|_| Value::Object(Default::default())),
                    created_at: row.try_get("", "created_at").map_err(|e| e.to_string())?,
                })
            })
            .collect::<Result<Vec<_>, String>>()?;
        Ok((entries, total))
    }

    // ------------------------------------------------------------------
    // Internal helpers
    // ------------------------------------------------------------------

    async fn lock_recharge_order_tx(
        &self,
        tx: &DatabaseTransaction,
        order_id: &str,
    ) -> Result<Option<RechargeOrder>, String> {
        let lock_suffix = if self.db.is_postgres() { " FOR UPDATE" } else { "" };
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
        row.as_ref().map(|row| order_from_row(row, false)).transpose()
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
    #[allow(clippy::too_many_arguments)]
    async fn insert_recharge_ledger_tx(
        &self,
        tx: &DatabaseTransaction,
        user_id: &str,
        kind: &str,
        delta_nano_usd: i128,
        balance_after_nano_usd: i128,
        meta: &Value,
        idempotency_key: &str,
        created_at: &str,
    ) -> Result<bool, String> {
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

/// RC-L4 ledger meta. `refund` carries `(actor_user_id, manual)` for
/// `recharge_refund` rows.
fn ledger_meta(
    order: &RechargeOrder,
    provider_order_id: Option<&str>,
    refund: Option<(&str, bool)>,
) -> Value {
    let mut meta = serde_json::json!({
        "order_id": order.id,
        "payment_channel_id": order.payment_channel_id,
        "channel_type_id": order.channel_type_id,
        "pay_currency": order.pay_currency,
        "pay_amount": order.pay_amount,
        "usd_rate": order.usd_rate,
        "provider_order_id": provider_order_id.or(order.provider_order_id.as_deref()),
    });
    if let Some((actor_user_id, manual)) = refund
        && let Some(object) = meta.as_object_mut()
    {
        object.insert("actor_user_id".to_string(), actor_user_id.into());
        object.insert("manual".to_string(), manual.into());
    }
    meta
}

fn map_channel_write(result: Result<(), sea_orm::DbErr>) -> Result<(), ChannelWriteError> {
    result.map_err(|error| {
        let message = error.to_string();
        // The unique expression index rejects duplicate lower(trim(name))
        // values that race past the handler pre-check (RC-A6).
        if message.contains("uidx_payment_channels_name")
            || message.to_uppercase().contains("UNIQUE")
        {
            ChannelWriteError::NameExists
        } else {
            ChannelWriteError::Internal(message)
        }
    })
}
