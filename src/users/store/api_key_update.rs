use super::*;

impl UserStore {
    /// Update an existing API key with new fields
    pub async fn update_api_key(
        &self,
        key_id: &str,
        input: UpdateApiKeyInput,
        is_admin: bool,
    ) -> Result<ApiKey, BalanceMutationError> {
        let prepared = self
            .prepare_api_key_update(key_id, &input, is_admin)
            .await?;
        let ApiKeyUpdateSql {
            set_clauses,
            mut values,
            idx,
        } = ApiKeyUpdateSql::build(&input, &prepared)?;
        let PreparedApiKeyUpdate {
            canonical_ip_whitelist: _,
            requested_sub_account_balance,
            expected_sub_account_balance,
            existing_key,
            disabling_sub_account,
            group_fields_changed,
            effective_use_user_group,
            effective_group_ids,
        } = prepared;
        if set_clauses.is_empty() {
            return Ok(existing_key);
        }

        if group_fields_changed && !effective_use_user_group {
            let owner_group_id = self
                .get_user_by_id(&existing_key.user_id)
                .await?
                .map(|user| user.group_id)
                .unwrap_or_default();
            self.validate_api_key_group_selection(&owner_group_id, &effective_group_ids, is_admin)
                .await?;
        }

        values.push(key_id.into());

        let query = format!(
            "UPDATE api_keys SET {} WHERE id = ${idx}",
            set_clauses.join(", ")
        );

        if disabling_sub_account || requested_sub_account_balance.is_some() {
            let write = self.db.write().await;
            let tx = write.begin().await.map_err(|e| e.to_string())?;
            let user = self
                .lock_user_balance_tx(&tx, &existing_key.user_id)
                .await
                .map_err(|e| e.message)?;
            let key = self
                .lock_api_key_balance_tx(&tx, key_id, &existing_key.user_id)
                .await
                .map_err(|e| e.message)?;

            self.settle_updated_api_key_balance(
                &tx,
                ApiKeyBalanceUpdate {
                    key_id,
                    user_id: &existing_key.user_id,
                    user: &user,
                    key: &key,
                    requested_sub_account_balance,
                    expected_sub_account_balance,
                    disabling_sub_account,
                },
            )
            .await?;

            tx.execute(self.db.stmt(&query, values))
                .await
                .map_err(|e| e.to_string())?;
            tx.commit().await.map_err(|e| e.to_string())?;
            if disabling_sub_account
                || requested_sub_account_balance.is_some_and(|balance| balance < key.balance)
            {
                self.balance_cache.invalidate(&existing_key.user_id);
            }
        } else {
            self.db
                .write()
                .await
                .execute(self.db.stmt(&query, values))
                .await
                .map_err(|e| e.to_string())?;
        }

        self.sub_account_balance_cache.invalidate(key_id);
        self.api_key_cache.invalidate_by_key_id(key_id);

        self.get_api_key_by_id(key_id)
            .await?
            .ok_or_else(|| "API key not found after update".to_string().into())
    }
}

struct PreparedApiKeyUpdate {
    canonical_ip_whitelist: Option<Vec<String>>,
    requested_sub_account_balance: Option<i128>,
    expected_sub_account_balance: Option<i128>,
    existing_key: ApiKey,
    disabling_sub_account: bool,
    group_fields_changed: bool,
    effective_use_user_group: bool,
    effective_group_ids: Vec<String>,
}
impl UserStore {
    async fn prepare_api_key_update(
        &self,
        key_id: &str,
        input: &UpdateApiKeyInput,
        is_admin: bool,
    ) -> Result<PreparedApiKeyUpdate, BalanceMutationError> {
        validate_positive_max_multiplier(input.max_multiplier)?;
        if let Some(Some(expires_at)) = input.expires_at.as_ref() {
            DateTime::parse_from_rfc3339(expires_at)
                .map_err(|_| "expires_at must be a valid RFC3339 timestamp".to_string())?;
        }
        if let Some(transforms) = &input.transforms {
            validate_api_key_transforms(transforms, is_admin, &self.custom_transforms.get())?;
        }
        if let Some(model_redirects) = &input.model_redirects {
            validate_model_redirects(model_redirects)?;
        }
        let canonical_ip_whitelist = input
            .ip_whitelist
            .as_ref()
            .map(|entries| canonicalize_ip_whitelist(entries))
            .transpose()?;
        if input.sub_account_balance_nano_usd.is_some() && !is_admin {
            return Err("only admins may set a sub-account balance"
                .to_string()
                .into());
        }
        if input.sub_account_enabled == Some(false) && input.sub_account_balance_nano_usd.is_some()
        {
            return Err(
                "sub-account balance cannot be supplied while disabling sub-account billing"
                    .to_string()
                    .into(),
            );
        }
        if input.expected_sub_account_balance_nano_usd.is_some()
            && input.sub_account_balance_nano_usd.is_none()
        {
            return Err(
                "expected sub-account balance requires sub_account_balance_nano_usd"
                    .to_string()
                    .into(),
            );
        }
        let requested_sub_account_balance = input
            .sub_account_balance_nano_usd
            .as_deref()
            .map(parse_nano_usd)
            .transpose()?;
        let expected_sub_account_balance = input
            .expected_sub_account_balance_nano_usd
            .as_deref()
            .map(parse_nano_usd)
            .transpose()?;
        let existing_key = self
            .get_api_key_by_id(key_id)
            .await?
            .ok_or_else(|| "API key not found".to_string())?;
        let resulting_sub_account_enabled = input
            .sub_account_enabled
            .unwrap_or(existing_key.sub_account_enabled);
        if requested_sub_account_balance.is_some_and(|balance| balance != 0)
            && !resulting_sub_account_enabled
        {
            return Err(
                "a non-zero sub-account balance requires sub-account billing to be enabled"
                    .to_string()
                    .into(),
            );
        }
        let disabling_sub_account = input.sub_account_enabled == Some(false);
        let group_fields_changed = input.use_user_group.is_some() || input.group_ids.is_some();
        let effective_use_user_group = input.use_user_group.unwrap_or(existing_key.use_user_group);
        // TM-GRP-4: an inheriting key stores []; an explicit key needs >= 1 group.
        let effective_group_ids = if effective_use_user_group {
            Vec::new()
        } else {
            let group_ids = input
                .group_ids
                .as_deref()
                .map(canonicalize_group_ids)
                .unwrap_or_else(|| existing_key.group_ids.clone());
            if group_fields_changed && group_ids.is_empty() {
                return Err(
                    "group_ids must be non-empty when use_user_group is disabled"
                        .to_string()
                        .into(),
                );
            }
            group_ids
        };
        Ok(PreparedApiKeyUpdate {
            canonical_ip_whitelist,
            requested_sub_account_balance,
            expected_sub_account_balance,
            existing_key,
            disabling_sub_account,
            group_fields_changed,
            effective_use_user_group,
            effective_group_ids,
        })
    }
}

struct ApiKeyUpdateSql {
    set_clauses: Vec<String>,
    values: Vec<SeaValue>,
    idx: usize,
}
impl ApiKeyUpdateSql {
    fn build(input: &UpdateApiKeyInput, prepared: &PreparedApiKeyUpdate) -> Result<Self, String> {
        let PreparedApiKeyUpdate {
            requested_sub_account_balance,
            canonical_ip_whitelist,
            group_fields_changed,
            effective_use_user_group,
            effective_group_ids,
            ..
        } = prepared;
        let mut set_clauses = Vec::new();
        let mut values: Vec<SeaValue> = Vec::new();
        let mut idx = 1usize;

        if let Some(name) = &input.name {
            set_clauses.push(format!("name = ${idx}"));
            values.push(name.clone().into());
            idx += 1;
        }
        if let Some(enabled) = input.enabled {
            set_clauses.push(format!("enabled = ${idx}"));
            values.push(SeaValue::Int(Some(if enabled { 1 } else { 0 })));
            idx += 1;
        }
        if let Some(sub_account_enabled) = input.sub_account_enabled {
            set_clauses.push(format!("sub_account_enabled = ${idx}"));
            values.push(SeaValue::Int(Some(if sub_account_enabled { 1 } else { 0 })));
            idx += 1;
        }
        if let Some(sub_account_balance) = *requested_sub_account_balance {
            set_clauses.push(format!("sub_account_balance_nano = ${idx}"));
            values.push(sub_account_balance.to_string().into());
            idx += 1;
        }
        if let Some(model_limits_enabled) = input.model_limits_enabled {
            set_clauses.push(format!("model_limits_enabled = ${idx}"));
            values.push(SeaValue::Int(Some(if model_limits_enabled {
                1
            } else {
                0
            })));
            idx += 1;
        }
        if let Some(model_limits) = &input.model_limits {
            set_clauses.push(format!("model_limits = ${idx}"));
            values.push(
                serde_json::to_string(model_limits)
                    .map_err(|e| e.to_string())?
                    .into(),
            );
            idx += 1;
        }
        if let Some(ip_whitelist) = canonical_ip_whitelist {
            set_clauses.push(format!("ip_whitelist = ${idx}"));
            values.push(
                serde_json::to_string(ip_whitelist)
                    .map_err(|e| e.to_string())?
                    .into(),
            );
            idx += 1;
        }
        if *group_fields_changed {
            set_clauses.push(format!("use_user_group = ${idx}"));
            values.push(SeaValue::Int(Some(if *effective_use_user_group {
                1
            } else {
                0
            })));
            idx += 1;
            set_clauses.push(format!("group_ids = ${idx}"));
            values.push(serialize_group_ids_json(effective_group_ids)?.into());
            idx += 1;
        }
        let mut sql = Self {
            set_clauses,
            values,
            idx,
        };
        sql.append_policy_fields(input)?;
        Ok(sql)
    }
    fn append_policy_fields(&mut self, input: &UpdateApiKeyInput) -> Result<(), String> {
        let Self {
            set_clauses,
            values,
            idx,
        } = self;
        if let Some(max_multiplier) = input.max_multiplier {
            set_clauses.push(format!("max_multiplier = ${idx}"));
            values.push(max_multiplier.to_string().into());
            *idx += 1;
        }
        if let Some(transforms) = &input.transforms {
            let mut transforms = transforms.clone();
            canonicalize_transform_rules(&mut transforms);
            set_clauses.push(format!("transforms = ${idx}"));
            values.push(
                serde_json::to_string(&transforms)
                    .map_err(|e| e.to_string())?
                    .into(),
            );
            *idx += 1;
        }
        if let Some(model_redirects) = &input.model_redirects {
            set_clauses.push(format!("model_redirects = ${idx}"));
            values.push(
                serde_json::to_string(model_redirects)
                    .map_err(|e| e.to_string())?
                    .into(),
            );
            *idx += 1;
        }
        if let Some(reasoning_envelope_enabled) = input.reasoning_envelope_enabled {
            set_clauses.push(format!("reasoning_envelope_enabled = ${idx}"));
            values.push(SeaValue::Int(Some(if reasoning_envelope_enabled {
                1
            } else {
                0
            })));
            *idx += 1;
        }
        if let Some(request_capture_mode) = input.request_capture_mode {
            set_clauses.push(format!("request_capture_enabled = ${idx}"));
            values.push(SeaValue::Int(Some(
                if request_capture_mode.should_start_capture() {
                    1
                } else {
                    0
                },
            )));
            *idx += 1;
            set_clauses.push(format!("request_capture_mode = ${idx}"));
            values.push(request_capture_mode.as_str().into());
            *idx += 1;
        }
        if let Some(request_capture_retention) = input.request_capture_retention {
            set_clauses.push(format!("request_capture_retention = ${idx}"));
            values.push(request_capture_retention.as_str().into());
            *idx += 1;
        }
        if let Some(expires_at) = &input.expires_at {
            match expires_at {
                Some(expires_at) => {
                    set_clauses.push(format!("expires_at = ${idx}"));
                    values.push(expires_at.clone().into());
                    *idx += 1;
                }
                None => set_clauses.push("expires_at = NULL".to_string()),
            }
        }

        Ok(())
    }
}

struct ApiKeyBalanceUpdate<'a> {
    key_id: &'a str,
    user_id: &'a str,
    user: &'a LockedUserBalance,
    key: &'a LockedApiKeyBalance,
    requested_sub_account_balance: Option<i128>,
    expected_sub_account_balance: Option<i128>,
    disabling_sub_account: bool,
}
impl UserStore {
    async fn settle_updated_api_key_balance(
        &self,
        tx: &DatabaseTransaction,
        update: ApiKeyBalanceUpdate<'_>,
    ) -> Result<(), BalanceMutationError> {
        if update.disabling_sub_account
            && (update.key.sub_account_enabled || update.key.balance != 0)
        {
            self.disable_api_key_sub_account(tx, &update).await?;
        } else if let Some(new_balance) = update.requested_sub_account_balance {
            self.adjust_api_key_sub_account(tx, &update, new_balance)
                .await?;
        }
        Ok(())
    }
    async fn disable_api_key_sub_account(
        &self,
        tx: &DatabaseTransaction,
        update: &ApiKeyBalanceUpdate<'_>,
    ) -> Result<(), BalanceMutationError> {
        let ApiKeyBalanceUpdate {
            key_id,
            user_id,
            user,
            key,
            ..
        } = *update;
        let balance_after = if user.unlimited {
            None
        } else {
            let next = user
                .balance
                .checked_add(key.balance)
                .ok_or_else(|| "sub-account disable settlement overflow".to_string())?;
            tx.execute(self.db.stmt(
                "UPDATE users SET balance_nano_usd = $1, updated_at = $2 WHERE id = $3",
                vec![
                    next.to_string().into(),
                    Utc::now().to_rfc3339().into(),
                    user_id.into(),
                ],
            ))
            .await
            .map_err(|e| e.to_string())?;
            Some(next)
        };

        tx.execute(self.db.stmt(
            "UPDATE api_keys SET sub_account_balance_nano = '0' WHERE id = $1",
            vec![key_id.into()],
        ))
        .await
        .map_err(|e| e.to_string())?;

        if key.balance != 0 {
            let now = Utc::now().to_rfc3339();
            let kind = if key.balance > 0 {
                "sub_account_refund"
            } else {
                "sub_account_debt_transfer"
            };
            self.insert_billing_ledger_tx(
                tx,
                BillingLedgerEntry {
                    user_id,
                    kind,
                    delta_nano_usd: key.balance,
                    balance_after_nano_usd: balance_after,
                    meta: &serde_json::json!({ "api_key_id": key_id }),
                    created_at_rfc3339: &now,
                },
            )
            .await
            .map_err(|e| e.message)?;
        }
        Ok(())
    }
    async fn adjust_api_key_sub_account(
        &self,
        tx: &DatabaseTransaction,
        update: &ApiKeyBalanceUpdate<'_>,
        new_balance: i128,
    ) -> Result<(), BalanceMutationError> {
        let ApiKeyBalanceUpdate {
            key_id,
            user_id,
            user,
            key,
            expected_sub_account_balance,
            ..
        } = *update;
        if expected_sub_account_balance.is_some_and(|expected| expected != key.balance) {
            return Err(BalanceMutationError::BalanceChanged);
        }
        let now = Utc::now().to_rfc3339();
        if new_balance < key.balance {
            let refund = key
                .balance
                .checked_sub(new_balance)
                .ok_or_else(|| "sub-account refund overflow".to_string())?;
            let balance_after = if user.unlimited {
                None
            } else {
                let next = user
                    .balance
                    .checked_add(refund)
                    .ok_or_else(|| "sub-account refund overflow".to_string())?;
                tx.execute(self.db.stmt(
                    "UPDATE users SET balance_nano_usd = $1, updated_at = $2 WHERE id = $3",
                    vec![next.to_string().into(), now.clone().into(), user_id.into()],
                ))
                .await
                .map_err(|e| e.to_string())?;
                Some(next)
            };
            self.insert_billing_ledger_tx(
                tx,
                BillingLedgerEntry {
                    user_id,
                    kind: "sub_account_refund",
                    delta_nano_usd: refund,
                    balance_after_nano_usd: balance_after,
                    meta: &serde_json::json!({
                        "api_key_id": key_id,
                        "balance_before_nano_usd": key.balance.to_string(),
                        "balance_after_nano_usd": new_balance.to_string(),
                    }),
                    created_at_rfc3339: &now,
                },
            )
            .await
            .map_err(|e| e.message)?;
        } else if new_balance > key.balance {
            let increase = new_balance
                .checked_sub(key.balance)
                .ok_or_else(|| "sub-account adjustment overflow".to_string())?;
            self.insert_billing_ledger_tx(
                tx,
                BillingLedgerEntry {
                    user_id,
                    kind: "admin_sub_account_adjustment",
                    delta_nano_usd: increase,
                    balance_after_nano_usd: Some(new_balance),
                    meta: &serde_json::json!({
                        "api_key_id": key_id,
                        "balance_before_nano_usd": key.balance.to_string(),
                        "balance_after_nano_usd": new_balance.to_string(),
                    }),
                    created_at_rfc3339: &now,
                },
            )
            .await
            .map_err(|e| e.message)?;
        }
        Ok(())
    }
}
