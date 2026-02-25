use crate::transforms::TransformRuleConfig;
use super::{
    ApiKey, BillingError, BillingErrorKind, CreateApiKeyInput, Session, UpdateApiKeyInput,
    User, UserBalance, UserRole, UserStore, RESERVED_INTERNAL_USER_PREFIX,
};
use super::utils::parse_nano_usd;
use argon2::{
    Argon2,
    password_hash::{PasswordHash, PasswordHasher, PasswordVerifier, SaltString, rand_core::OsRng},
};
use chrono::{DateTime, Utc};
use sea_orm::{ConnectionTrait, DatabaseTransaction, QueryResult, TransactionTrait};
use sea_orm::Value as SeaValue;
use serde_json::Value;

impl UserStore {
    pub fn is_reserved_internal_username(username: &str) -> bool {
        username
            .trim()
            .to_ascii_lowercase()
            .starts_with(RESERVED_INTERNAL_USER_PREFIX)
    }

    pub async fn new(db: crate::db::DbPool) -> Result<Self, String> {
        Ok(Self { db })
    }

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

    pub async fn create_user(
        &self,
        username: &str,
        password: &str,
        role: UserRole,
    ) -> Result<User, String> {
        let id = uuid::Uuid::new_v4().to_string();
        let password_hash = Self::hash_password(password)?;
        let now = Utc::now();

        self.db.write().await
            .execute(self.db.stmt(
                r#"INSERT INTO users (id, username, password_hash, role, created_at, updated_at, enabled, balance_nano_usd, balance_unlimited)
                   VALUES ($1, $2, $3, $4, $5, $6, 1, '0', 0)"#,
                vec![
                    id.clone().into(),
                    username.into(),
                    password_hash.clone().into(),
                    role.as_str().into(),
                    now.to_rfc3339().into(),
                    now.to_rfc3339().into(),
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
        })
    }

    pub async fn get_user_by_id(&self, id: &str) -> Result<Option<User>, String> {
        let row = self.db.read()
            .query_one(self.db.stmt(
                "SELECT id, username, password_hash, role, created_at, updated_at, last_login_at, enabled, balance_nano_usd, balance_unlimited, email FROM users WHERE id = $1",
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
                "SELECT id, username, password_hash, role, created_at, updated_at, last_login_at, enabled, balance_nano_usd, balance_unlimited, email FROM users WHERE username = $1",
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
                "SELECT id, username, password_hash, role, created_at, updated_at, last_login_at, enabled, balance_nano_usd, balance_unlimited, email FROM users WHERE substr(lower(username), 1, 9) != '_monoize_' ORDER BY created_at DESC",
                vec![],
            ))
            .await
            .map_err(|e| e.to_string())?;

        rows.iter().map(|row| self.row_to_user(row)).collect()
    }

    #[allow(clippy::too_many_arguments)]
    pub async fn update_user(
        &self,
        id: &str,
        username: Option<&str>,
        password: Option<&str>,
        role: Option<UserRole>,
        enabled: Option<bool>,
        balance_nano_usd: Option<&str>,
        balance_unlimited: Option<bool>,
        email: Option<Option<&str>>,
    ) -> Result<(), String> {
        let mut set_clauses = Vec::new();
        let mut values: Vec<SeaValue> = Vec::new();
        let mut idx = 1usize;

        if let Some(u) = username {
            set_clauses.push(format!("username = ${idx}"));
            values.push(u.into());
            idx += 1;
        }
        if let Some(p) = password {
            set_clauses.push(format!("password_hash = ${idx}"));
            values.push(Self::hash_password(p)?.into());
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

        if set_clauses.is_empty() {
            return Ok(());
        }

        set_clauses.push(format!("updated_at = ${idx}"));
        values.push(Utc::now().to_rfc3339().into());
        idx += 1;

        values.push(id.into());

        let query = format!("UPDATE users SET {} WHERE id = ${idx}", set_clauses.join(", "));

        self.db.write().await
            .execute(self.db.stmt(&query, values))
            .await
            .map_err(|e| e.to_string())?;
        Ok(())
    }

    pub async fn delete_user(&self, id: &str) -> Result<(), String> {
        self.db.write().await
            .execute(self.db.stmt("DELETE FROM users WHERE id = $1", vec![id.into()]))
            .await
            .map_err(|e| e.to_string())?;
        Ok(())
    }

    pub async fn update_last_login(&self, id: &str) -> Result<(), String> {
        let now = Utc::now();
        self.db.write().await
            .execute(self.db.stmt(
                "UPDATE users SET last_login_at = $1 WHERE id = $2",
                vec![now.to_rfc3339().into(), id.into()],
            ))
            .await
            .map_err(|e| e.to_string())?;
        Ok(())
    }

    pub async fn create_session(&self, user_id: &str) -> Result<Session, String> {
        let id = uuid::Uuid::new_v4().to_string();
        let token = format!(
            "urp_session_{}",
            uuid::Uuid::new_v4().to_string().replace("-", "")
        );
        let now = Utc::now();
        let expires_at = now + chrono::Duration::days(7);

        self.db.write().await
            .execute(self.db.stmt(
                r#"INSERT INTO sessions (id, user_id, token, created_at, expires_at)
                   VALUES ($1, $2, $3, $4, $5)"#,
                vec![
                    id.clone().into(),
                    user_id.into(),
                    token.clone().into(),
                    now.to_rfc3339().into(),
                    expires_at.to_rfc3339().into(),
                ],
            ))
            .await
            .map_err(|e| e.to_string())?;

        Ok(Session {
            id,
            user_id: user_id.to_string(),
            token,
            created_at: now,
            expires_at,
        })
    }

    pub async fn get_session_by_token(&self, token: &str) -> Result<Option<Session>, String> {
        let row = self.db.read()
            .query_one(self.db.stmt(
                "SELECT id, user_id, token, created_at, expires_at FROM sessions WHERE token = $1",
                vec![token.into()],
            ))
            .await
            .map_err(|e| e.to_string())?;

        if let Some(row) = row {
            let expires_at: String = row.try_get("", "expires_at").map_err(|e| e.to_string())?;
            let expires_at = DateTime::parse_from_rfc3339(&expires_at)
                .map_err(|e| e.to_string())?
                .with_timezone(&Utc);

            if expires_at < Utc::now() {
                self.delete_session(token).await?;
                return Ok(None);
            }

            Ok(Some(Session {
                id: row.try_get("", "id").map_err(|e| e.to_string())?,
                user_id: row.try_get("", "user_id").map_err(|e| e.to_string())?,
                token: row.try_get("", "token").map_err(|e| e.to_string())?,
                created_at: DateTime::parse_from_rfc3339(
                    &row.try_get::<String>("", "created_at")
                        .map_err(|e| e.to_string())?,
                )
                .map_err(|e| e.to_string())?
                .with_timezone(&Utc),
                expires_at,
            }))
        } else {
            Ok(None)
        }
    }

    pub async fn delete_session(&self, token: &str) -> Result<(), String> {
        self.db.write().await
            .execute(self.db.stmt("DELETE FROM sessions WHERE token = $1", vec![token.into()]))
            .await
            .map_err(|e| e.to_string())?;
        Ok(())
    }

    pub async fn delete_user_sessions(&self, user_id: &str) -> Result<(), String> {
        self.db.write().await
            .execute(self.db.stmt("DELETE FROM sessions WHERE user_id = $1", vec![user_id.into()]))
            .await
            .map_err(|e| e.to_string())?;
        Ok(())
    }

    pub async fn create_api_key(
        &self,
        user_id: &str,
        name: &str,
        expires_at: Option<DateTime<Utc>>,
    ) -> Result<(ApiKey, String), String> {
        self.create_api_key_extended(
            user_id,
            CreateApiKeyInput {
                name: name.to_string(),
                expires_in_days: expires_at.map(|e| (e - Utc::now()).num_days()),
                quota: None,
                quota_unlimited: true,
                model_limits_enabled: false,
                model_limits: Vec::new(),
                ip_whitelist: Vec::new(),
                group: "default".to_string(),
                max_multiplier: None,
                transforms: Vec::new(),
            },
        )
        .await
    }

    pub async fn create_api_key_extended(
        &self,
        user_id: &str,
        input: CreateApiKeyInput,
    ) -> Result<(ApiKey, String), String> {
        let id = uuid::Uuid::new_v4().to_string();
        let key = format!("sk-{}", uuid::Uuid::new_v4().to_string().replace("-", ""));
        let key_prefix = key[..12].to_string();
        let key_hash = Self::hash_password(&key)?;
        let now = Utc::now();
        let expires_at = input
            .expires_in_days
            .map(|days| now + chrono::Duration::days(days));

        let model_limits_json =
            serde_json::to_string(&input.model_limits).map_err(|e| e.to_string())?;
        let ip_whitelist_json =
            serde_json::to_string(&input.ip_whitelist).map_err(|e| e.to_string())?;

        self.db.write().await
            .execute(self.db.stmt(
                r#"INSERT INTO api_keys (id, user_id, name, key_prefix, key, key_hash, created_at, expires_at, enabled, quota_remaining, quota_unlimited, model_limits_enabled, model_limits, ip_whitelist, token_group, max_multiplier, transforms)
                   VALUES ($1, $2, $3, $4, $5, $6, $7, $8, 1, $9, $10, $11, $12, $13, $14, $15, $16)"#,
                vec![
                    id.clone().into(),
                    user_id.into(),
                    input.name.clone().into(),
                    key_prefix.clone().into(),
                    key.clone().into(),
                    key_hash.clone().into(),
                    now.to_rfc3339().into(),
                    expires_at.map(|e| e.to_rfc3339()).into(),
                    input.quota.map(|v| SeaValue::BigInt(Some(v))).unwrap_or(SeaValue::BigInt(None)),
                    SeaValue::Int(Some(if input.quota_unlimited { 1 } else { 0 })),
                    SeaValue::Int(Some(if input.model_limits_enabled { 1 } else { 0 })),
                    model_limits_json.into(),
                    ip_whitelist_json.into(),
                    input.group.clone().into(),
                    input.max_multiplier.map(|v| SeaValue::Double(Some(v))).unwrap_or(SeaValue::Double(None)),
                    serde_json::to_string(&input.transforms).map_err(|e| e.to_string())?.into(),
                ],
            ))
            .await
            .map_err(|e| e.to_string())?;

        let api_key = ApiKey {
            id,
            user_id: user_id.to_string(),
            name: input.name,
            key_prefix,
            key: key.clone(),
            key_hash,
            created_at: now,
            expires_at,
            last_used_at: None,
            enabled: true,
            quota_remaining: input.quota,
            quota_unlimited: input.quota_unlimited,
            model_limits_enabled: input.model_limits_enabled,
            model_limits: input.model_limits,
            ip_whitelist: input.ip_whitelist,
            group: input.group,
            max_multiplier: input.max_multiplier,
            transforms: input.transforms,
        };

        Ok((api_key, key))
    }

    pub async fn get_api_key_by_prefix(&self, prefix: &str) -> Result<Option<ApiKey>, String> {
        let row = self.db.read()
            .query_one(self.db.stmt(
                "SELECT id, user_id, name, key_prefix, key, key_hash, created_at, expires_at, last_used_at, enabled, quota_remaining, quota_unlimited, model_limits_enabled, model_limits, ip_whitelist, token_group, max_multiplier, transforms FROM api_keys WHERE key_prefix = $1",
                vec![prefix.into()],
            ))
            .await
            .map_err(|e| e.to_string())?;

        if let Some(row) = row {
            Ok(Some(self.row_to_api_key(&row)?))
        } else {
            Ok(None)
        }
    }

    pub async fn list_user_api_keys(&self, user_id: &str) -> Result<Vec<ApiKey>, String> {
        let rows = self.db.read()
            .query_all(self.db.stmt(
                "SELECT id, user_id, name, key_prefix, key, key_hash, created_at, expires_at, last_used_at, enabled, quota_remaining, quota_unlimited, model_limits_enabled, model_limits, ip_whitelist, token_group, max_multiplier, transforms FROM api_keys WHERE user_id = $1 ORDER BY created_at DESC",
                vec![user_id.into()],
            ))
            .await
            .map_err(|e| e.to_string())?;

        rows.iter().map(|row| self.row_to_api_key(row)).collect()
    }

    pub async fn update_api_key_last_used(&self, id: &str) -> Result<(), String> {
        let now = Utc::now();
        self.db.write().await
            .execute(self.db.stmt(
                "UPDATE api_keys SET last_used_at = $1 WHERE id = $2",
                vec![now.to_rfc3339().into(), id.into()],
            ))
            .await
            .map_err(|e| e.to_string())?;
        Ok(())
    }

    pub async fn delete_api_key(&self, id: &str) -> Result<(), String> {
        self.db.write().await
            .execute(self.db.stmt("DELETE FROM api_keys WHERE id = $1", vec![id.into()]))
            .await
            .map_err(|e| e.to_string())?;
        Ok(())
    }

    pub async fn validate_api_key(&self, key: &str) -> Result<Option<(ApiKey, User)>, String> {
        if key.len() < 12 {
            return Ok(None);
        }
        let prefix = &key[..12];

        let api_key = match self.get_api_key_by_prefix(prefix).await? {
            Some(k) => k,
            None => return Ok(None),
        };

        if !api_key.enabled {
            return Ok(None);
        }

        if let Some(expires_at) = api_key.expires_at {
            if expires_at < Utc::now() {
                return Ok(None);
            }
        }

        if !Self::verify_password(key, &api_key.key_hash)? {
            return Ok(None);
        }

        let user = match self.get_user_by_id(&api_key.user_id).await? {
            Some(u) => u,
            None => return Ok(None),
        };

        if !user.enabled {
            return Ok(None);
        }

        self.update_api_key_last_used(&api_key.id).await?;

        Ok(Some((api_key, user)))
    }

    pub(crate) fn row_to_user(&self, row: &QueryResult) -> Result<User, String> {
        let role_str: String = row.try_get("", "role").map_err(|e| e.to_string())?;
        let role = UserRole::from_str(&role_str).ok_or_else(|| "invalid role".to_string())?;

        let last_login_at: Option<String> =
            row.try_get("", "last_login_at").map_err(|e| e.to_string())?;
        let last_login_at = last_login_at
            .map(|s| DateTime::parse_from_rfc3339(&s).map(|d| d.with_timezone(&Utc)))
            .transpose()
            .map_err(|e| e.to_string())?;

        Ok(User {
            id: row.try_get("", "id").map_err(|e| e.to_string())?,
            username: row.try_get("", "username").map_err(|e| e.to_string())?,
            password_hash: row.try_get("", "password_hash").map_err(|e| e.to_string())?,
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
            enabled: row
                .try_get::<i32>("", "enabled")
                .map_err(|e| e.to_string())?
                == 1,
            balance_nano_usd: row
                .try_get("", "balance_nano_usd")
                .unwrap_or_else(|_| "0".to_string()),
            balance_unlimited: row.try_get::<i32>("", "balance_unlimited").unwrap_or(0) == 1,
            email: row.try_get::<Option<String>>("", "email").unwrap_or(None),
        })
    }

    pub(crate) fn row_to_api_key(&self, row: &QueryResult) -> Result<ApiKey, String> {
        let expires_at: Option<String> = row.try_get("", "expires_at").map_err(|e| e.to_string())?;
        let expires_at = expires_at
            .map(|s| DateTime::parse_from_rfc3339(&s).map(|d| d.with_timezone(&Utc)))
            .transpose()
            .map_err(|e| e.to_string())?;

        let last_used_at: Option<String> =
            row.try_get("", "last_used_at").map_err(|e| e.to_string())?;
        let last_used_at = last_used_at
            .map(|s| DateTime::parse_from_rfc3339(&s).map(|d| d.with_timezone(&Utc)))
            .transpose()
            .map_err(|e| e.to_string())?;

        let quota_remaining: Option<i64> = row.try_get("", "quota_remaining").unwrap_or(None);
        let quota_unlimited: i32 = row.try_get("", "quota_unlimited").unwrap_or(1);
        let model_limits_enabled: i32 = row.try_get("", "model_limits_enabled").unwrap_or(0);

        let model_limits_str: String = row
            .try_get("", "model_limits")
            .unwrap_or_else(|_| "[]".to_string());
        let model_limits: Vec<String> = serde_json::from_str(&model_limits_str).unwrap_or_default();

        let ip_whitelist_str: String = row
            .try_get("", "ip_whitelist")
            .unwrap_or_else(|_| "[]".to_string());
        let ip_whitelist: Vec<String> = serde_json::from_str(&ip_whitelist_str).unwrap_or_default();

        let group: String = row
            .try_get("", "token_group")
            .unwrap_or_else(|_| "default".to_string());
        let max_multiplier: Option<f64> = row.try_get("", "max_multiplier").unwrap_or(None);
        let transforms_str: String = row
            .try_get("", "transforms")
            .unwrap_or_else(|_| "[]".to_string());
        let transforms: Vec<TransformRuleConfig> =
            serde_json::from_str(&transforms_str).unwrap_or_default();

        Ok(ApiKey {
            id: row.try_get("", "id").map_err(|e| e.to_string())?,
            user_id: row.try_get("", "user_id").map_err(|e| e.to_string())?,
            name: row.try_get("", "name").map_err(|e| e.to_string())?,
            key_prefix: row.try_get("", "key_prefix").map_err(|e| e.to_string())?,
            key: row.try_get("", "key").unwrap_or_else(|_| String::new()),
            key_hash: row.try_get("", "key_hash").map_err(|e| e.to_string())?,
            created_at: DateTime::parse_from_rfc3339(
                &row.try_get::<String>("", "created_at")
                    .map_err(|e| e.to_string())?,
            )
            .map_err(|e| e.to_string())?
            .with_timezone(&Utc),
            expires_at,
            last_used_at,
            enabled: row
                .try_get::<i32>("", "enabled")
                .map_err(|e| e.to_string())?
                == 1,
            quota_remaining,
            quota_unlimited: quota_unlimited == 1,
            model_limits_enabled: model_limits_enabled == 1,
            model_limits,
            ip_whitelist,
            group,
            max_multiplier,
            transforms,
        })
    }

    /// Update an existing API key with new fields
    pub async fn update_api_key(
        &self,
        key_id: &str,
        input: UpdateApiKeyInput,
    ) -> Result<ApiKey, String> {
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
        if let Some(quota) = input.quota {
            set_clauses.push(format!("quota_remaining = ${idx}"));
            values.push(SeaValue::BigInt(Some(quota)));
            idx += 1;
        }
        if let Some(quota_unlimited) = input.quota_unlimited {
            set_clauses.push(format!("quota_unlimited = ${idx}"));
            values.push(SeaValue::Int(Some(if quota_unlimited { 1 } else { 0 })));
            idx += 1;
        }
        if let Some(model_limits_enabled) = input.model_limits_enabled {
            set_clauses.push(format!("model_limits_enabled = ${idx}"));
            values.push(SeaValue::Int(Some(if model_limits_enabled { 1 } else { 0 })));
            idx += 1;
        }
        if let Some(model_limits) = &input.model_limits {
            set_clauses.push(format!("model_limits = ${idx}"));
            values.push(serde_json::to_string(model_limits).map_err(|e| e.to_string())?.into());
            idx += 1;
        }
        if let Some(ip_whitelist) = &input.ip_whitelist {
            set_clauses.push(format!("ip_whitelist = ${idx}"));
            values.push(serde_json::to_string(ip_whitelist).map_err(|e| e.to_string())?.into());
            idx += 1;
        }
        if let Some(group) = &input.group {
            set_clauses.push(format!("token_group = ${idx}"));
            values.push(group.clone().into());
            idx += 1;
        }
        if let Some(max_multiplier) = input.max_multiplier {
            set_clauses.push(format!("max_multiplier = ${idx}"));
            values.push(SeaValue::Double(Some(max_multiplier)));
            idx += 1;
        }
        if let Some(transforms) = &input.transforms {
            set_clauses.push(format!("transforms = ${idx}"));
            values.push(serde_json::to_string(transforms).map_err(|e| e.to_string())?.into());
            idx += 1;
        }
        if let Some(expires_at) = &input.expires_at {
            set_clauses.push(format!("expires_at = ${idx}"));
            values.push(expires_at.clone().into());
            idx += 1;
        }

        if set_clauses.is_empty() {
            return self
                .get_api_key_by_id(key_id)
                .await?
                .ok_or_else(|| "API key not found".to_string());
        }

        values.push(key_id.into());

        let query = format!("UPDATE api_keys SET {} WHERE id = ${idx}", set_clauses.join(", "));

        self.db.write().await
            .execute(self.db.stmt(&query, values))
            .await
            .map_err(|e| e.to_string())?;

        self.get_api_key_by_id(key_id)
            .await?
            .ok_or_else(|| "API key not found after update".to_string())
    }

    /// Get API key by ID
    pub async fn get_api_key_by_id(&self, id: &str) -> Result<Option<ApiKey>, String> {
        let row = self.db.read()
            .query_one(self.db.stmt(
                "SELECT id, user_id, name, key_prefix, key, key_hash, created_at, expires_at, last_used_at, enabled, quota_remaining, quota_unlimited, model_limits_enabled, model_limits, ip_whitelist, token_group, max_multiplier, transforms FROM api_keys WHERE id = $1",
                vec![id.into()],
            ))
            .await
            .map_err(|e| e.to_string())?;

        if let Some(row) = row {
            Ok(Some(self.row_to_api_key(&row)?))
        } else {
            Ok(None)
        }
    }

    /// Batch delete API keys
    pub async fn batch_delete_api_keys(&self, ids: &[String]) -> Result<usize, String> {
        if ids.is_empty() {
            return Ok(0);
        }

        let mut values: Vec<SeaValue> = Vec::new();
        let placeholders: Vec<String> = ids.iter().enumerate().map(|(i, id)| {
            values.push(id.clone().into());
            format!("${}", i + 1)
        }).collect();
        let query = format!(
            "DELETE FROM api_keys WHERE id IN ({})",
            placeholders.join(", ")
        );

        let result = self.db.write().await
            .execute(self.db.stmt(&query, values))
            .await
            .map_err(|e| e.to_string())?;
        Ok(result.rows_affected() as usize)
    }

    pub async fn get_user_balance(&self, user_id: &str) -> Result<Option<UserBalance>, String> {
        let row = self.db.read()
            .query_one(self.db.stmt(
                "SELECT id, balance_nano_usd, balance_unlimited FROM users WHERE id = $1",
                vec![user_id.into()],
            ))
            .await
            .map_err(|e| e.to_string())?;
        let Some(row) = row else {
            return Ok(None);
        };
        let balance_raw: String = row
            .try_get("", "balance_nano_usd")
            .unwrap_or_else(|_| "0".to_string());
        let balance_nano_usd = parse_nano_usd(&balance_raw)?;
        Ok(Some(UserBalance {
            user_id: row.try_get("", "id").map_err(|e| e.to_string())?,
            balance_nano_usd,
            balance_unlimited: row.try_get::<i32>("", "balance_unlimited").unwrap_or(0) == 1,
        }))
    }

    pub async fn ensure_user_can_spend(&self, user_id: &str) -> Result<(), BillingError> {
        let Some(balance) = self
            .get_user_balance(user_id)
            .await
            .map_err(|e| BillingError::new(BillingErrorKind::Internal, e))?
        else {
            return Err(BillingError::new(
                BillingErrorKind::NotFound,
                "user not found",
            ));
        };

        if balance.balance_unlimited {
            return Ok(());
        }
        if balance.balance_nano_usd <= 0 {
            return Err(BillingError::new(
                BillingErrorKind::InsufficientBalance,
                "insufficient balance",
            ));
        }
        Ok(())
    }

    pub async fn charge_user_balance_nano(
        &self,
        user_id: &str,
        amount_nano_usd: i128,
        meta: &Value,
    ) -> Result<(), BillingError> {
        if amount_nano_usd <= 0 {
            return Ok(());
        }
        self.charge_user_balance_nano_inner(user_id, amount_nano_usd, meta)
            .await
    }

    async fn charge_user_balance_nano_inner(
        &self,
        user_id: &str,
        amount_nano_usd: i128,
        meta: &Value,
    ) -> Result<(), BillingError> {
        let _write_guard = self.db.write().await;
        let tx = _write_guard
            .begin()
            .await
            .map_err(|e| BillingError::new(BillingErrorKind::Internal, e.to_string()))?;
        let select_sql = if self.db.is_postgres() {
            "SELECT balance_nano_usd, balance_unlimited FROM users WHERE id = $1 FOR UPDATE"
        } else {
            "SELECT balance_nano_usd, balance_unlimited FROM users WHERE id = $1"
        };
        let row = tx
            .query_one(self.db.stmt(select_sql, vec![user_id.into()]))
            .await
            .map_err(|e| BillingError::new(BillingErrorKind::Internal, e.to_string()))?;
        let Some(row) = row else {
            return Err(BillingError::new(
                BillingErrorKind::NotFound,
                "user not found",
            ));
        };
        let unlimited = row.try_get::<i32>("", "balance_unlimited").unwrap_or(0) == 1;
        if unlimited {
            tx.commit()
                .await
                .map_err(|e| BillingError::new(BillingErrorKind::Internal, e.to_string()))?;
            return Ok(());
        }

        let balance_raw: String = row
            .try_get("", "balance_nano_usd")
            .unwrap_or_else(|_| "0".to_string());
        let balance = parse_nano_usd(&balance_raw)
            .map_err(|e| BillingError::new(BillingErrorKind::InvalidStoredBalance, e))?;
        let next_balance = balance.checked_sub(amount_nano_usd).ok_or_else(|| {
            BillingError::new(BillingErrorKind::Overflow, "balance subtraction overflow")
        })?;
        if next_balance < 0 {
            return Err(BillingError::new(
                BillingErrorKind::InsufficientBalance,
                "insufficient balance",
            ));
        }

        let now = Utc::now().to_rfc3339();
        tx.execute(self.db.stmt(
            "UPDATE users SET balance_nano_usd = $1, updated_at = $2 WHERE id = $3",
            vec![
                next_balance.to_string().into(),
                now.clone().into(),
                user_id.into(),
            ],
        ))
        .await
        .map_err(|e| BillingError::new(BillingErrorKind::Internal, e.to_string()))?;

        self.insert_billing_ledger_tx(
            &tx,
            user_id,
            "request_charge",
            -amount_nano_usd,
            Some(next_balance),
            meta,
            &now,
        )
        .await?;

        tx.commit()
            .await
            .map_err(|e| BillingError::new(BillingErrorKind::Internal, e.to_string()))?;
        Ok(())
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
        let select_sql = if self.db.is_postgres() {
            "SELECT balance_nano_usd, balance_unlimited FROM users WHERE id = $1 FOR UPDATE"
        } else {
            "SELECT balance_nano_usd, balance_unlimited FROM users WHERE id = $1"
        };
        let row = tx
            .query_one(self.db.stmt(select_sql, vec![user_id.into()]))
            .await
            .map_err(|e| e.to_string())?;
        let Some(row) = row else {
            return Err("user not found".to_string());
        };
        let current_balance_raw: String = row
            .try_get("", "balance_nano_usd")
            .unwrap_or_else(|_| "0".to_string());
        let current_balance = parse_nano_usd(&current_balance_raw)?;
        let current_unlimited = row.try_get::<i32>("", "balance_unlimited").unwrap_or(0) == 1;

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
            user_id,
            "admin_adjustment",
            delta,
            Some(new_balance),
            &meta,
            &now,
        )
        .await
        .map_err(|e| e.message)?;

        tx.commit().await.map_err(|e| e.to_string())?;
        Ok(())
    }

    #[allow(clippy::too_many_arguments)]
    pub(crate) async fn insert_billing_ledger_tx(
        &self,
        tx: &DatabaseTransaction,
        user_id: &str,
        kind: &str,
        delta_nano_usd: i128,
        balance_after_nano_usd: Option<i128>,
        meta: &Value,
        created_at_rfc3339: &str,
    ) -> Result<(), BillingError> {
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
