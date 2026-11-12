use super::*;

impl UserStore {
    pub async fn admin_update_user_atomic(
        &self,
        id: &str,
        input: AdminUpdateUserInput,
        actor_user_id: &str,
    ) -> Result<(), BalanceMutationError> {
        validate_admin_balance_update(&input)?;
        let AdminUpdateUserInput {
            username,
            password,
            role,
            enabled,
            balance_nano_usd,
            balance_delta_nano_usd,
            expected_balance_nano_usd,
            balance_note,
            balance_unlimited,
            email,
            group_id,
        } = input;
        let has_balance_change = balance_nano_usd.is_some()
            || balance_delta_nano_usd.is_some()
            || balance_unlimited.is_some();
        let revokes_sessions = password.is_some() || role.is_some() || enabled == Some(false);
        if username.is_none()
            && password.is_none()
            && role.is_none()
            && enabled.is_none()
            && !has_balance_change
            && email.is_none()
            && group_id.is_none()
        {
            return Ok(());
        }

        let password_hash = match password.as_deref() {
            Some(password) => Some(Self::hash_password_async(password).await?),
            None => None,
        };
        let parsed_balance = parse_optional_balance(balance_nano_usd.as_deref())?;
        let parsed_delta = parse_optional_balance(balance_delta_nano_usd.as_deref())?;
        let parsed_expected = parse_optional_balance(expected_balance_nano_usd.as_deref())?;
        if let Some(group_id) = group_id.as_deref()
            && self.get_group_by_id(group_id).await?.is_none()
        {
            return Err(format!("unknown group id: {group_id}").into());
        }

        let write = self.db.write().await;
        let tx = write.begin().await.map_err(|e| e.to_string())?;
        let current = self
            .lock_user_balance_tx(&tx, id)
            .await
            .map_err(|error| error.message)?;
        if parsed_expected.is_some_and(|expected| expected != current.balance) {
            return Err(BalanceMutationError::BalanceChanged);
        }
        let new_balance = match (parsed_balance, parsed_delta) {
            (Some(balance), _) => balance,
            (None, Some(delta)) => current
                .balance
                .checked_add(delta)
                .ok_or_else(|| "invalid balance: delta overflow".to_string())?,
            (None, None) => current.balance,
        };
        let writes_balance = parsed_balance.is_some() || parsed_delta.is_some();
        let new_unlimited = balance_unlimited.unwrap_or(current.unlimited);
        let now = Utc::now().to_rfc3339();
        self.write_admin_user_assignments(
            &tx,
            id,
            &now,
            AdminUserAssignments {
                username,
                password_hash,
                role,
                enabled,
                writes_balance,
                new_balance,
                balance_unlimited,
                new_unlimited,
                email,
                group_id,
            },
        )
        .await?;

        if revokes_sessions {
            self.delete_user_sessions_tx(&tx, id).await?;
        }

        if has_balance_change {
            self.write_admin_balance_ledger(
                &tx,
                id,
                actor_user_id,
                &now,
                AdminBalanceLedger {
                    current,
                    new_balance,
                    new_unlimited,
                    balance_note,
                },
            )
            .await?;
        }
        tx.commit().await.map_err(|e| e.to_string())?;
        self.api_key_cache.invalidate_by_user_id(id);
        if has_balance_change {
            self.balance_cache.invalidate(id);
        }
        Ok(())
    }
}

struct AdminUserAssignments {
    username: Option<String>,
    password_hash: Option<String>,
    role: Option<UserRole>,
    enabled: Option<bool>,
    writes_balance: bool,
    new_balance: i128,
    balance_unlimited: Option<bool>,
    new_unlimited: bool,
    email: Option<Option<String>>,
    group_id: Option<String>,
}
impl UserStore {
    async fn write_admin_user_assignments(
        &self,
        tx: &DatabaseTransaction,
        id: &str,
        now: &str,
        assignments: AdminUserAssignments,
    ) -> Result<(), String> {
        let AdminUserAssignments {
            username,
            password_hash,
            role,
            enabled,
            writes_balance,
            new_balance,
            balance_unlimited,
            new_unlimited,
            email,
            group_id,
        } = assignments;
        let mut set_clauses = Vec::new();
        let mut values: Vec<SeaValue> = Vec::new();
        let mut idx = 1usize;

        if let Some(username) = username {
            set_clauses.push(format!("username = ${idx}"));
            values.push(username.into());
            idx += 1;
        }
        if let Some(password_hash) = password_hash {
            set_clauses.push(format!("password_hash = ${idx}"));
            values.push(password_hash.into());
            idx += 1;
        }
        if let Some(role) = role {
            set_clauses.push(format!("role = ${idx}"));
            values.push(role.as_str().into());
            idx += 1;
        }
        if let Some(enabled) = enabled {
            set_clauses.push(format!("enabled = ${idx}"));
            values.push(SeaValue::Int(Some(if enabled { 1 } else { 0 })));
            idx += 1;
        }
        if writes_balance {
            set_clauses.push(format!("balance_nano_usd = ${idx}"));
            values.push(new_balance.to_string().into());
            idx += 1;
        }
        if balance_unlimited.is_some() {
            set_clauses.push(format!("balance_unlimited = ${idx}"));
            values.push(SeaValue::Int(Some(if new_unlimited { 1 } else { 0 })));
            idx += 1;
        }
        if let Some(email) = email {
            match email
                .as_deref()
                .map(str::trim)
                .filter(|value| !value.is_empty())
            {
                Some(email) => {
                    set_clauses.push(format!("email = ${idx}"));
                    values.push(email.into());
                    idx += 1;
                }
                None => set_clauses.push("email = NULL".to_string()),
            }
        }
        if let Some(group_id) = group_id {
            set_clauses.push(format!("group_id = ${idx}"));
            values.push(group_id.into());
            idx += 1;
        }
        set_clauses.push(format!("updated_at = ${idx}"));
        values.push(now.into());
        idx += 1;
        values.push(id.into());
        tx.execute(self.db.stmt(
            &format!(
                "UPDATE users SET {} WHERE id = ${idx}",
                set_clauses.join(", ")
            ),
            values,
        ))
        .await
        .map_err(|e| e.to_string())?;

        Ok(())
    }
}

struct AdminBalanceLedger {
    current: LockedUserBalance,
    new_balance: i128,
    new_unlimited: bool,
    balance_note: Option<String>,
}
impl UserStore {
    async fn write_admin_balance_ledger(
        &self,
        tx: &DatabaseTransaction,
        id: &str,
        actor_user_id: &str,
        now: &str,
        ledger: AdminBalanceLedger,
    ) -> Result<(), BalanceMutationError> {
        let AdminBalanceLedger {
            current,
            new_balance,
            new_unlimited,
            balance_note,
        } = ledger;
        let delta = new_balance
            .checked_sub(current.balance)
            .ok_or_else(|| "balance delta overflow".to_string())?;
        let mut meta = serde_json::json!({
            "actor_user_id": actor_user_id,
            "before_balance_nano_usd": current.balance.to_string(),
            "after_balance_nano_usd": new_balance.to_string(),
            "before_balance_unlimited": current.unlimited,
            "after_balance_unlimited": new_unlimited,
        });
        if let Some(note) = balance_note {
            meta["note"] = Value::String(note);
        }
        self.insert_billing_ledger_tx(
            tx,
            BillingLedgerEntry {
                user_id: id,
                kind: "admin_adjustment",
                delta_nano_usd: delta,
                balance_after_nano_usd: Some(new_balance),
                meta: &meta,
                created_at_rfc3339: now,
            },
        )
        .await
        .map_err(|error| error.message)?;
        Ok(())
    }
}

fn parse_optional_balance(raw: Option<&str>) -> Result<Option<i128>, String> {
    raw.map(parse_nano_usd).transpose()
}

fn validate_admin_balance_update(input: &AdminUpdateUserInput) -> Result<(), BalanceMutationError> {
    if input.balance_nano_usd.is_some() && input.balance_delta_nano_usd.is_some() {
        return Err("balance and balance delta are mutually exclusive"
            .to_string()
            .into());
    }
    if input.expected_balance_nano_usd.is_some() && input.balance_nano_usd.is_none() {
        return Err("expected balance requires an absolute balance"
            .to_string()
            .into());
    }
    Ok(())
}
