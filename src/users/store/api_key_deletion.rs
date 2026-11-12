use super::*;

type DeleteSettlementRow = (String, String, i128, Option<i128>, String);

impl UserStore {
    pub(super) async fn delete_api_keys_transactional(
        &self,
        ids: &[String],
    ) -> Result<usize, String> {
        if ids.len() > api_key_batch_delete_max_ids() {
            return Err(format!(
                "batch delete accepts at most {} ids",
                api_key_batch_delete_max_ids()
            ));
        }
        if ids.is_empty() {
            return Ok(0);
        }

        let mut key_ids = ids.to_vec();
        key_ids.sort();
        key_ids.dedup();

        let write = self.db.write().await;
        let tx = write.begin().await.map_err(|e| e.to_string())?;

        let placeholders = (0..key_ids.len())
            .map(|index| format!("${}", index + 1))
            .collect::<Vec<_>>()
            .join(", ");
        let key_values = key_ids.iter().cloned().map(Into::into).collect::<Vec<_>>();
        let owner_rows = tx
            .query_all(self.db.stmt(
                &format!(
                    "SELECT id, user_id FROM api_keys WHERE id IN ({placeholders}) ORDER BY user_id, id"
                ),
                key_values.clone(),
            ))
            .await
            .map_err(|e| e.to_string())?;
        let mut expected_owners: BTreeMap<String, String> = BTreeMap::new();
        for row in owner_rows {
            expected_owners.insert(
                row.try_get("", "id").map_err(|e| e.to_string())?,
                row.try_get("", "user_id").map_err(|e| e.to_string())?,
            );
        }

        let user_ids: Vec<String> = expected_owners
            .values()
            .cloned()
            .collect::<BTreeSet<_>>()
            .into_iter()
            .collect();
        let mut user_balances = self.lock_delete_user_balances(&tx, &user_ids).await?;

        let locked_keys = self
            .lock_delete_api_keys(&tx, &placeholders, key_values, &expected_owners)
            .await?;

        let now = Utc::now().to_rfc3339();
        let deleted_key_ids = locked_keys
            .iter()
            .map(|(key_id, _)| key_id.clone())
            .collect::<Vec<_>>();
        let mut affected_user_ids = BTreeSet::new();
        let mut user_updates = BTreeMap::new();
        let mut settlement_rows = Vec::new();
        for (key_id, key) in &locked_keys {
            if key.balance != 0 {
                let user = user_balances
                    .get_mut(&key.user_id)
                    .ok_or_else(|| "locked user balance missing".to_string())?;
                let balance_after = if user.unlimited {
                    None
                } else {
                    let next = user
                        .balance
                        .checked_add(key.balance)
                        .ok_or_else(|| "sub-account delete settlement overflow".to_string())?;
                    user.balance = next;
                    user_updates.insert(key.user_id.clone(), next);
                    Some(next)
                };
                settlement_rows.push((
                    uuid::Uuid::new_v4().to_string(),
                    key.user_id.clone(),
                    key.balance,
                    balance_after,
                    serde_json::json!({ "api_key_id": key_id }).to_string(),
                ));
                affected_user_ids.insert(key.user_id.clone());
            }
        }

        self.write_delete_settlements(&tx, user_updates, &settlement_rows, &now)
            .await?;
        if !deleted_key_ids.is_empty() {
            let delete_placeholders = (0..deleted_key_ids.len())
                .map(|index| format!("${}", index + 1))
                .collect::<Vec<_>>()
                .join(", ");
            tx.execute(self.db.stmt(
                &format!("DELETE FROM api_keys WHERE id IN ({delete_placeholders})"),
                deleted_key_ids.iter().cloned().map(Into::into).collect(),
            ))
            .await
            .map_err(|e| e.to_string())?;
        }

        tx.commit().await.map_err(|e| e.to_string())?;
        self.api_key_cache.invalidate_by_key_ids(&key_ids);
        for key_id in &deleted_key_ids {
            self.sub_account_balance_cache.invalidate(key_id);
        }
        for user_id in affected_user_ids {
            self.balance_cache.invalidate(&user_id);
        }
        Ok(deleted_key_ids.len())
    }

    async fn lock_delete_user_balances(
        &self,
        tx: &DatabaseTransaction,
        user_ids: &[String],
    ) -> Result<BTreeMap<String, LockedUserBalance>, String> {
        let mut user_balances = BTreeMap::new();
        if !user_ids.is_empty() {
            let user_placeholders = (0..user_ids.len())
                .map(|index| format!("${}", index + 1))
                .collect::<Vec<_>>()
                .join(", ");
            let user_lock_suffix = if self.db.is_postgres() {
                " FOR UPDATE"
            } else {
                ""
            };
            let user_rows = tx
                .query_all(self.db.stmt(
                    &format!(
                        "SELECT id, balance_nano_usd, balance_unlimited
                         FROM users WHERE id IN ({user_placeholders})
                         ORDER BY id{user_lock_suffix}"
                    ),
                    user_ids.iter().cloned().map(Into::into).collect(),
                ))
                .await
                .map_err(|e| e.to_string())?;
            for row in user_rows {
                let user_id: String = row.try_get("", "id").map_err(|e| e.to_string())?;
                let raw: String = row
                    .try_get("", "balance_nano_usd")
                    .map_err(|e| e.to_string())?;
                let balance = parse_nano_usd(&raw)
                    .map_err(|e| format!("invalid persisted user balance: {e}"))?;
                let unlimited = row
                    .try_get::<i32>("", "balance_unlimited")
                    .map_err(|e| e.to_string())?
                    == 1;
                user_balances.insert(user_id, LockedUserBalance { balance, unlimited });
            }
            if user_balances.len() != user_ids.len() {
                return Err("api key owner was not found".to_string());
            }
        }

        Ok(user_balances)
    }

    async fn lock_delete_api_keys(
        &self,
        tx: &DatabaseTransaction,
        placeholders: &str,
        key_values: Vec<SeaValue>,
        expected_owners: &BTreeMap<String, String>,
    ) -> Result<Vec<(String, LockedApiKeyBalance)>, String> {
        let lock_suffix = if self.db.is_postgres() {
            " FOR UPDATE"
        } else {
            ""
        };
        let locked_rows = tx
            .query_all(self.db.stmt(
                &format!(
                    "SELECT id, user_id, sub_account_enabled, sub_account_balance_nano
                     FROM api_keys WHERE id IN ({placeholders})
                     ORDER BY user_id, id{lock_suffix}"
                ),
                key_values,
            ))
            .await
            .map_err(|e| e.to_string())?;
        let mut locked_keys = Vec::with_capacity(locked_rows.len());
        for row in locked_rows {
            let key_id: String = row.try_get("", "id").map_err(|e| e.to_string())?;
            let user_id: String = row.try_get("", "user_id").map_err(|e| e.to_string())?;
            if expected_owners.get(&key_id) != Some(&user_id) {
                continue;
            }
            let raw_balance: String = row
                .try_get("", "sub_account_balance_nano")
                .map_err(|e| e.to_string())?;
            let balance = parse_nano_usd(&raw_balance)
                .map_err(|e| format!("invalid persisted sub-account balance: {e}"))?;
            locked_keys.push((
                key_id,
                LockedApiKeyBalance {
                    user_id,
                    balance,
                    sub_account_enabled: row
                        .try_get::<i32>("", "sub_account_enabled")
                        .map_err(|e| e.to_string())?
                        == 1,
                },
            ));
        }

        Ok(locked_keys)
    }

    async fn write_delete_settlements(
        &self,
        tx: &DatabaseTransaction,
        user_updates: BTreeMap<String, i128>,
        settlement_rows: &[DeleteSettlementRow],
        now: &str,
    ) -> Result<(), String> {
        const USER_UPDATE_CHUNK_SIZE: usize = 199;
        let user_updates = user_updates.into_iter().collect::<Vec<_>>();
        for chunk in user_updates.chunks(USER_UPDATE_CHUNK_SIZE) {
            let mut values = Vec::with_capacity(chunk.len() * 2 + 1);
            let mut cases = Vec::with_capacity(chunk.len());
            let mut ids = Vec::with_capacity(chunk.len());
            for (user_id, balance) in chunk {
                let id_index = values.len() + 1;
                values.push(user_id.clone().into());
                ids.push(format!("${id_index}"));
                let balance_index = values.len() + 1;
                values.push(balance.to_string().into());
                cases.push(format!("WHEN ${id_index} THEN ${balance_index}"));
            }
            let updated_at_index = values.len() + 1;
            values.push(now.into());
            tx.execute(self.db.stmt(
                &format!(
                    "UPDATE users
                     SET balance_nano_usd = CASE id {} ELSE balance_nano_usd END,
                         updated_at = ${updated_at_index}
                     WHERE id IN ({})",
                    cases.join(" "),
                    ids.join(", ")
                ),
                values,
            ))
            .await
            .map_err(|e| e.to_string())?;
        }

        const LEDGER_INSERT_CHUNK_SIZE: usize = 57;
        for chunk in settlement_rows.chunks(LEDGER_INSERT_CHUNK_SIZE) {
            let mut values = Vec::with_capacity(chunk.len() * 7);
            let mut rows = Vec::with_capacity(chunk.len());
            for (id, user_id, delta, balance_after, meta_json) in chunk {
                let start = values.len() + 1;
                values.push(id.clone().into());
                values.push(user_id.clone().into());
                values.push("sub_account_delete_settlement".into());
                values.push(delta.to_string().into());
                values.push(balance_after.map(|value| value.to_string()).into());
                values.push(meta_json.clone().into());
                values.push(now.into());
                rows.push(format!(
                    "(${}, ${}, ${}, ${}, ${}, ${}, ${})",
                    start,
                    start + 1,
                    start + 2,
                    start + 3,
                    start + 4,
                    start + 5,
                    start + 6
                ));
            }
            tx.execute(self.db.stmt(
                &format!(
                    "INSERT INTO billing_ledger
                     (id, user_id, kind, delta_nano_usd, balance_after_nano_usd, meta_json, created_at)
                     VALUES {}",
                    rows.join(", ")
                ),
                values,
            ))
            .await
            .map_err(|e| e.to_string())?;
        }
        Ok(())
    }
}
