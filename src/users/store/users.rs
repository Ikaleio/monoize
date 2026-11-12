use super::*;

/// Fields to update on one user. An absent field leaves its stored value unchanged.
#[derive(Default)]
pub struct UpdateUserInput<'a> {
    pub username: Option<&'a str>,
    pub password: Option<&'a str>,
    pub role: Option<UserRole>,
    pub enabled: Option<bool>,
    pub balance_nano_usd: Option<&'a str>,
    pub balance_unlimited: Option<bool>,
    pub email: Option<Option<&'a str>>,
    pub group_id: Option<&'a str>,
}

impl UserStore {
    pub fn hash_password(password: &str) -> Result<String, String> {
        let salt = SaltString::generate(&mut OsRng);
        let argon2 = Argon2::default();
        argon2
            .hash_password(password.as_bytes(), &salt)
            .map(|h| h.to_string())
            .map_err(|e| e.to_string())
    }

    pub fn verify_password(password: &str, hash: &str) -> Result<bool, String> {
        let parsed_hash = PasswordHash::new(hash).map_err(|e| e.to_string())?;
        Ok(Argon2::default()
            .verify_password(password.as_bytes(), &parsed_hash)
            .is_ok())
    }

    /// Runs Argon2 outside the Tokio worker pool so request futures remain schedulable.
    pub async fn hash_password_async(password: &str) -> Result<String, String> {
        let password = password.to_string();
        tokio::task::spawn_blocking(move || Self::hash_password(&password))
            .await
            .map_err(|error| format!("password hashing task failed: {error}"))?
    }

    pub async fn verify_password_async(password: &str, hash: &str) -> Result<bool, String> {
        let password = password.to_string();
        let hash = hash.to_string();
        tokio::task::spawn_blocking(move || Self::verify_password(&password, &hash))
            .await
            .map_err(|error| format!("password verification task failed: {error}"))?
    }

    pub async fn user_count(&self) -> Result<i64, String> {
        let row = self.db.read()
            .query_one(self.db.stmt(
                "SELECT COUNT(*) as count FROM users WHERE substr(lower(username), 1, 9) != '_monoize_'",
                vec![],
            ))
            .await
            .map_err(|e| e.to_string())?;
        let row = row.ok_or_else(|| "no count row".to_string())?;
        row.try_get::<i64>("", "count").map_err(|e| e.to_string())
    }

    /// Create a user with the given group id, or the default group when `None`
    /// (`user-billing-and-model-metadata.spec.md` U3). A provided id must
    /// reference an existing registry row (GR-C3).
    pub async fn create_user(
        &self,
        username: &str,
        password: &str,
        role: UserRole,
        group_id: Option<&str>,
    ) -> Result<User, String> {
        let id = uuid::Uuid::new_v4().to_string();
        let password_hash = Self::hash_password_async(password).await?;
        let now = Utc::now();
        let group_id = match group_id.map(str::trim).filter(|value| !value.is_empty()) {
            Some(value) => {
                if self.get_group_by_id(value).await?.is_none() {
                    return Err(format!("unknown group id: {value}"));
                }
                value.to_string()
            }
            None => self.default_group_id().await?,
        };

        self.db.write().await
            .execute(self.db.stmt(
                r#"INSERT INTO users (id, username, password_hash, role, created_at, updated_at, enabled, balance_nano_usd, balance_unlimited, group_id)
                   VALUES ($1, $2, $3, $4, $5, $6, 1, '0', 0, $7)"#,
                vec![
                    id.clone().into(),
                    username.into(),
                    password_hash.clone().into(),
                    role.as_str().into(),
                    now.to_rfc3339().into(),
                    now.to_rfc3339().into(),
                    group_id.clone().into(),
                ],
            ))
            .await
            .map_err(|e| e.to_string())?;

        Ok(User {
            id,
            username: username.to_string(),
            password_hash,
            role,
            created_at: now,
            updated_at: now,
            last_login_at: None,
            enabled: true,
            balance_nano_usd: "0".to_string(),
            balance_unlimited: false,
            email: None,
            group_id,
        })
    }

    pub async fn register_user_atomic(
        &self,
        username: &str,
        password: &str,
        registration_enabled: bool,
    ) -> Result<User, RegisterUserError> {
        let _registration_guard = self.registration_lock.lock().await;
        let user_count = self
            .user_count()
            .await
            .map_err(RegisterUserError::Storage)?;
        if user_count != 0 && !registration_enabled {
            return Err(RegisterUserError::RegistrationDisabled);
        }
        if self
            .get_user_by_username(username)
            .await
            .map_err(RegisterUserError::Storage)?
            .is_some()
        {
            return Err(RegisterUserError::UsernameExists);
        }
        let role = if user_count == 0 {
            UserRole::SuperAdmin
        } else {
            UserRole::User
        };
        self.create_user(username, password, role, None)
            .await
            .map_err(RegisterUserError::Storage)
    }

    pub async fn get_user_by_id(&self, id: &str) -> Result<Option<User>, String> {
        let row = self.db.read()
            .query_one(self.db.stmt(
                "SELECT id, username, password_hash, role, created_at, updated_at, last_login_at, enabled, balance_nano_usd, balance_unlimited, email, group_id FROM users WHERE id = $1",
                vec![id.into()],
            ))
            .await
            .map_err(|e| e.to_string())?;

        if let Some(row) = row {
            Ok(Some(self.row_to_user(&row)?))
        } else {
            Ok(None)
        }
    }

    pub async fn get_user_by_username(&self, username: &str) -> Result<Option<User>, String> {
        let row = self.db.read()
            .query_one(self.db.stmt(
                "SELECT id, username, password_hash, role, created_at, updated_at, last_login_at, enabled, balance_nano_usd, balance_unlimited, email, group_id FROM users WHERE username = $1",
                vec![username.into()],
            ))
            .await
            .map_err(|e| e.to_string())?;

        if let Some(row) = row {
            Ok(Some(self.row_to_user(&row)?))
        } else {
            Ok(None)
        }
    }

    pub async fn list_users(&self) -> Result<Vec<User>, String> {
        let rows = self.db.read()
            .query_all(self.db.stmt(
                "SELECT id, username, password_hash, role, created_at, updated_at, last_login_at, enabled, balance_nano_usd, balance_unlimited, email, group_id FROM users WHERE substr(lower(username), 1, 9) != '_monoize_' ORDER BY created_at DESC",
                vec![],
            ))
            .await
            .map_err(|e| e.to_string())?;

        rows.iter().map(|row| self.row_to_user(row)).collect()
    }

    pub async fn update_user(&self, id: &str, input: UpdateUserInput<'_>) -> Result<(), String> {
        let UpdateUserInput {
            username,
            password,
            role,
            enabled,
            balance_nano_usd,
            balance_unlimited,
            email,
            group_id,
        } = input;
        let revokes_sessions = password.is_some() || role.is_some() || enabled == Some(false);
        let password_hash = match password {
            Some(password) => Some(Self::hash_password_async(password).await?),
            None => None,
        };
        let mut set_clauses = Vec::new();
        let mut values: Vec<SeaValue> = Vec::new();
        let mut idx = 1usize;

        if let Some(u) = username {
            set_clauses.push(format!("username = ${idx}"));
            values.push(u.into());
            idx += 1;
        }
        if let Some(password_hash) = password_hash {
            set_clauses.push(format!("password_hash = ${idx}"));
            values.push(password_hash.into());
            idx += 1;
        }
        if let Some(r) = role {
            set_clauses.push(format!("role = ${idx}"));
            values.push(r.as_str().into());
            idx += 1;
        }
        if let Some(e) = enabled {
            set_clauses.push(format!("enabled = ${idx}"));
            values.push(SeaValue::Int(Some(if e { 1 } else { 0 })));
            idx += 1;
        }
        if let Some(balance) = balance_nano_usd {
            parse_nano_usd(balance)?;
            set_clauses.push(format!("balance_nano_usd = ${idx}"));
            values.push(balance.into());
            idx += 1;
        }
        if let Some(unlimited) = balance_unlimited {
            set_clauses.push(format!("balance_unlimited = ${idx}"));
            values.push(SeaValue::Int(Some(if unlimited { 1 } else { 0 })));
            idx += 1;
        }
        if let Some(email_opt) = email {
            match email_opt {
                Some(e) if !e.trim().is_empty() => {
                    set_clauses.push(format!("email = ${idx}"));
                    values.push(e.trim().into());
                    idx += 1;
                }
                _ => {
                    set_clauses.push("email = NULL".to_string());
                }
            }
        }
        if let Some(group_id) = group_id {
            if self.get_group_by_id(group_id).await?.is_none() {
                return Err(format!("unknown group id: {group_id}"));
            }
            set_clauses.push(format!("group_id = ${idx}"));
            values.push(group_id.into());
            idx += 1;
        }
        if set_clauses.is_empty() {
            return Ok(());
        }

        set_clauses.push(format!("updated_at = ${idx}"));
        values.push(Utc::now().to_rfc3339().into());
        idx += 1;

        values.push(id.into());

        let query = format!(
            "UPDATE users SET {} WHERE id = ${idx}",
            set_clauses.join(", ")
        );

        let write = self.db.write().await;
        let tx = write.begin().await.map_err(|e| e.to_string())?;
        tx.execute(self.db.stmt(&query, values))
            .await
            .map_err(|e| e.to_string())?;

        if revokes_sessions {
            self.delete_user_sessions_tx(&tx, id).await?;
        }

        tx.commit().await.map_err(|e| e.to_string())?;

        if !set_clauses.is_empty() {
            self.api_key_cache.invalidate_by_user_id(id);
        }
        if balance_nano_usd.is_some() || balance_unlimited.is_some() {
            self.balance_cache.invalidate(id);
        }

        Ok(())
    }

    pub async fn delete_user(&self, id: &str) -> Result<(), String> {
        let write = self.db.write().await;
        let tx = write.begin().await.map_err(|e| e.to_string())?;
        let user_lock_sql = if self.db.is_postgres() {
            "SELECT id FROM users WHERE id = $1 FOR UPDATE"
        } else {
            "SELECT id FROM users WHERE id = $1"
        };
        let user = tx
            .query_one(self.db.stmt(user_lock_sql, vec![id.into()]))
            .await
            .map_err(|e| e.to_string())?;
        if user.is_none() {
            return Err("user not found".to_string());
        }
        let result = tx
            .execute(
                self.db
                    .stmt("DELETE FROM users WHERE id = $1", vec![id.into()]),
            )
            .await
            .map_err(|e| e.to_string())?;
        if result.rows_affected() != 1 {
            return Err("user not found".to_string());
        }
        tx.commit().await.map_err(|e| e.to_string())?;
        self.api_key_cache.invalidate_by_user_id(id);
        self.balance_cache.invalidate(id);
        // DPT-AK15 forbids materializing the deleted user's key ids.
        self.sub_account_balance_cache.invalidate_all();
        Ok(())
    }

    pub async fn update_last_login(&self, id: &str) -> Result<(), String> {
        let now = Utc::now();
        self.db
            .write()
            .await
            .execute(self.db.stmt(
                "UPDATE users SET last_login_at = $1 WHERE id = $2",
                vec![now.to_rfc3339().into(), id.into()],
            ))
            .await
            .map_err(|e| e.to_string())?;
        self.api_key_cache.invalidate_by_user_id(id);
        Ok(())
    }

    pub(crate) fn row_to_user(&self, row: &QueryResult) -> Result<User, String> {
        let role_str: String = row.try_get("", "role").map_err(|e| e.to_string())?;
        let role = UserRole::from_str(&role_str).ok_or_else(|| "invalid role".to_string())?;

        let last_login_at: Option<String> = row
            .try_get("", "last_login_at")
            .map_err(|e| e.to_string())?;
        let last_login_at = last_login_at
            .map(|s| DateTime::parse_from_rfc3339(&s).map(|d| d.with_timezone(&Utc)))
            .transpose()
            .map_err(|e| e.to_string())?;
        let group_id: String = row
            .try_get("", "group_id")
            .map_err(|error| format!("invalid persisted users.group_id: {error}"))?;
        let balance_nano_usd: String = row
            .try_get("", "balance_nano_usd")
            .map_err(|e| e.to_string())?;
        parse_nano_usd(&balance_nano_usd)
            .map_err(|e| format!("invalid persisted user balance: {e}"))?;

        Ok(User {
            id: row.try_get("", "id").map_err(|e| e.to_string())?,
            username: row.try_get("", "username").map_err(|e| e.to_string())?,
            password_hash: row
                .try_get("", "password_hash")
                .map_err(|e| e.to_string())?,
            role,
            created_at: DateTime::parse_from_rfc3339(
                &row.try_get::<String>("", "created_at")
                    .map_err(|e| e.to_string())?,
            )
            .map_err(|e| e.to_string())?
            .with_timezone(&Utc),
            updated_at: DateTime::parse_from_rfc3339(
                &row.try_get::<String>("", "updated_at")
                    .map_err(|e| e.to_string())?,
            )
            .map_err(|e| e.to_string())?
            .with_timezone(&Utc),
            last_login_at,
            enabled: decode_required_bool(row, "enabled")?,
            balance_nano_usd,
            balance_unlimited: row
                .try_get::<i32>("", "balance_unlimited")
                .map_err(|e| e.to_string())?
                == 1,
            email: row
                .try_get::<Option<String>>("", "email")
                .map_err(|e| e.to_string())?,
            group_id,
        })
    }
}
