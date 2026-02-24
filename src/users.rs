use crate::transforms::TransformRuleConfig;
use crate::db::DbPool;
use argon2::{
    Argon2,
    password_hash::{PasswordHash, PasswordHasher, PasswordVerifier, SaltString, rand_core::OsRng},
};
use chrono::{DateTime, Utc};
use sea_orm::{ConnectionTrait, DatabaseTransaction, QueryResult, TransactionTrait};
use sea_orm::Value as SeaValue;
use serde::{Deserialize, Serialize};
use serde_json::Value;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum UserRole {
    SuperAdmin,
    Admin,
    User,
}

impl UserRole {
    #[allow(clippy::should_implement_trait)]
    pub fn from_str(s: &str) -> Option<Self> {
        match s {
            "super_admin" => Some(Self::SuperAdmin),
            "admin" => Some(Self::Admin),
            "user" => Some(Self::User),
            _ => None,
        }
    }

    pub fn as_str(&self) -> &'static str {
        match self {
            Self::SuperAdmin => "super_admin",
            Self::Admin => "admin",
            Self::User => "user",
        }
    }

    pub fn can_manage_users(&self) -> bool {
        matches!(self, Self::SuperAdmin | Self::Admin)
    }

    pub fn can_manage_system(&self) -> bool {
        matches!(self, Self::SuperAdmin | Self::Admin)
    }

    pub fn can_assign_role(&self, target_role: UserRole) -> bool {
        match self {
            Self::SuperAdmin => true,
            Self::Admin => matches!(target_role, Self::User),
            Self::User => false,
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct User {
    pub id: String,
    pub username: String,
    #[serde(skip_serializing)]
    pub password_hash: String,
    pub role: UserRole,
    pub created_at: DateTime<Utc>,
    pub updated_at: DateTime<Utc>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub last_login_at: Option<DateTime<Utc>>,
    pub enabled: bool,
    /// Signed nano-dollar string balance.
    pub balance_nano_usd: String,
    /// Unlimited balance bypass flag.
    pub balance_unlimited: bool,
    /// Optional email for Gravatar display.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub email: Option<String>,
}

#[derive(Debug, Clone)]
pub struct UserBalance {
    pub user_id: String,
    pub balance_nano_usd: i128,
    pub balance_unlimited: bool,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum BillingErrorKind {
    NotFound,
    InsufficientBalance,
    InvalidStoredBalance,
    Overflow,
    Internal,
}

#[derive(Debug, Clone)]
pub struct BillingError {
    pub kind: BillingErrorKind,
    pub message: String,
}

impl BillingError {
    fn new(kind: BillingErrorKind, message: impl Into<String>) -> Self {
        Self {
            kind,
            message: message.into(),
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Session {
    pub id: String,
    pub user_id: String,
    pub token: String,
    pub created_at: DateTime<Utc>,
    pub expires_at: DateTime<Utc>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ApiKey {
    pub id: String,
    pub user_id: String,
    pub name: String,
    pub key_prefix: String,
    /// The full API key (stored for display purposes)
    pub key: String,
    #[serde(skip_serializing)]
    pub key_hash: String,
    pub created_at: DateTime<Utc>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub expires_at: Option<DateTime<Utc>>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub last_used_at: Option<DateTime<Utc>>,
    pub enabled: bool,
    /// Remaining quota in credits. None means quota_unlimited applies.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub quota_remaining: Option<i64>,
    /// Whether quota is unlimited
    #[serde(default)]
    pub quota_unlimited: bool,
    /// Whether model restrictions are active
    #[serde(default)]
    pub model_limits_enabled: bool,
    /// List of allowed model IDs (empty = all models when model_limits_enabled is false)
    #[serde(default)]
    pub model_limits: Vec<String>,
    /// List of allowed IP addresses/CIDRs (empty = any IP)
    #[serde(default)]
    pub ip_whitelist: Vec<String>,
    /// Token group identifier for rate limiting/policies
    #[serde(default)]
    pub group: String,
    /// Maximum accepted multiplier for routing
    #[serde(skip_serializing_if = "Option::is_none")]
    pub max_multiplier: Option<f64>,
    /// Ordered transform rules applied for this API key
    #[serde(default)]
    pub transforms: Vec<TransformRuleConfig>,
}

/// Input for creating a new API key with extended fields
#[derive(Debug, Clone, Deserialize)]
pub struct CreateApiKeyInput {
    pub name: String,
    pub expires_in_days: Option<i64>,
    pub quota: Option<i64>,
    #[serde(default = "default_quota_unlimited")]
    pub quota_unlimited: bool,
    #[serde(default)]
    pub model_limits_enabled: bool,
    #[serde(default)]
    pub model_limits: Vec<String>,
    #[serde(default)]
    pub ip_whitelist: Vec<String>,
    #[serde(default = "default_group")]
    pub group: String,
    #[serde(default)]
    pub max_multiplier: Option<f64>,
    #[serde(default)]
    pub transforms: Vec<TransformRuleConfig>,
}

fn default_quota_unlimited() -> bool {
    true
}

fn default_group() -> String {
    "default".to_string()
}

/// Input for updating an existing API key
#[derive(Debug, Clone, Deserialize)]
pub struct UpdateApiKeyInput {
    pub name: Option<String>,
    pub enabled: Option<bool>,
    pub quota: Option<i64>,
    pub quota_unlimited: Option<bool>,
    pub model_limits_enabled: Option<bool>,
    pub model_limits: Option<Vec<String>>,
    pub ip_whitelist: Option<Vec<String>>,
    pub group: Option<String>,
    pub max_multiplier: Option<f64>,
    pub transforms: Option<Vec<TransformRuleConfig>>,
    pub expires_at: Option<String>, // RFC3339 format or null
}

#[derive(Clone)]
pub struct UserStore {
    db: DbPool,
}

const RESERVED_INTERNAL_USER_PREFIX: &str = "_monoize_";

impl UserStore {
    pub fn is_reserved_internal_username(username: &str) -> bool {
        username
            .trim()
            .to_ascii_lowercase()
            .starts_with(RESERVED_INTERNAL_USER_PREFIX)
    }

    pub async fn new(db: DbPool) -> Result<Self, String> {
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

        self.db.write()
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

        self.db.write()
            .execute(self.db.stmt(&query, values))
            .await
            .map_err(|e| e.to_string())?;
        Ok(())
    }

    pub async fn delete_user(&self, id: &str) -> Result<(), String> {
        self.db.write()
            .execute(self.db.stmt("DELETE FROM users WHERE id = $1", vec![id.into()]))
            .await
            .map_err(|e| e.to_string())?;
        Ok(())
    }

    pub async fn update_last_login(&self, id: &str) -> Result<(), String> {
        let now = Utc::now();
        self.db.write()
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

        self.db.write()
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
        self.db.write()
            .execute(self.db.stmt("DELETE FROM sessions WHERE token = $1", vec![token.into()]))
            .await
            .map_err(|e| e.to_string())?;
        Ok(())
    }

    pub async fn delete_user_sessions(&self, user_id: &str) -> Result<(), String> {
        self.db.write()
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

        self.db.write()
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
        self.db.write()
            .execute(self.db.stmt(
                "UPDATE api_keys SET last_used_at = $1 WHERE id = $2",
                vec![now.to_rfc3339().into(), id.into()],
            ))
            .await
            .map_err(|e| e.to_string())?;
        Ok(())
    }

    pub async fn delete_api_key(&self, id: &str) -> Result<(), String> {
        self.db.write()
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

    fn row_to_user(&self, row: &QueryResult) -> Result<User, String> {
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

    fn row_to_api_key(&self, row: &QueryResult) -> Result<ApiKey, String> {
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

        self.db.write()
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

        let result = self.db.write()
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
        let tx = self
            .db
            .write()
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

        let tx = self.db.write().begin().await.map_err(|e| e.to_string())?;
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
    async fn insert_billing_ledger_tx(
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

pub struct InsertRequestLog {
    pub request_id: Option<String>,
    pub user_id: String,
    pub api_key_id: Option<String>,
    pub model: String,
    pub provider_id: Option<String>,
    pub upstream_model: Option<String>,
    pub channel_id: Option<String>,
    pub is_stream: bool,
    pub input_tokens: Option<u64>,
    pub output_tokens: Option<u64>,
    pub cache_read_tokens: Option<u64>,
    pub cache_creation_tokens: Option<u64>,
    pub tool_prompt_tokens: Option<u64>,
    pub reasoning_tokens: Option<u64>,
    pub accepted_prediction_tokens: Option<u64>,
    pub rejected_prediction_tokens: Option<u64>,
    pub provider_multiplier: Option<f64>,
    pub charge_nano_usd: Option<i128>,
    pub status: String,
    pub usage_breakdown_json: Option<Value>,
    pub billing_breakdown_json: Option<Value>,
    pub error_code: Option<String>,
    pub error_message: Option<String>,
    pub error_http_status: Option<u16>,
    pub duration_ms: Option<u64>,
    pub ttfb_ms: Option<u64>,
    pub request_ip: Option<String>,
    pub reasoning_effort: Option<String>,
    pub tried_providers_json: Option<Value>,
    pub request_kind: Option<String>,
}

pub const REQUEST_LOG_STATUS_PENDING: &str = "pending";
pub const REQUEST_LOG_STATUS_SUCCESS: &str = "success";
pub const REQUEST_LOG_STATUS_ERROR: &str = "error";

#[derive(Debug, Serialize)]
pub struct RequestLogRow {
    pub id: String,
    pub request_id: Option<String>,
    pub user_id: String,
    pub api_key_id: Option<String>,
    pub model: String,
    pub provider_id: Option<String>,
    pub upstream_model: Option<String>,
    pub channel_id: Option<String>,
    pub is_stream: bool,
    pub input_tokens: Option<i64>,
    pub output_tokens: Option<i64>,
    pub cache_read_tokens: Option<i64>,
    pub cache_creation_tokens: Option<i64>,
    pub tool_prompt_tokens: Option<i64>,
    pub reasoning_tokens: Option<i64>,
    pub accepted_prediction_tokens: Option<i64>,
    pub rejected_prediction_tokens: Option<i64>,
    pub provider_multiplier: Option<f64>,
    pub charge_nano_usd: Option<String>,
    pub status: String,
    pub usage_breakdown_json: Option<Value>,
    pub billing_breakdown_json: Option<Value>,
    pub error_code: Option<String>,
    pub error_message: Option<String>,
    pub error_http_status: Option<i64>,
    pub duration_ms: Option<i64>,
    pub ttfb_ms: Option<i64>,
    pub request_ip: Option<String>,
    pub reasoning_effort: Option<String>,
    pub tried_providers_json: Option<Value>,
    pub request_kind: Option<String>,
    pub created_at: String,
    pub username: Option<String>,
    pub api_key_name: Option<String>,
    pub channel_name: Option<String>,
    pub provider_name: Option<String>,
}

fn normalize_request_log_filter(value: Option<&str>) -> Option<String> {
    value
        .map(str::trim)
        .filter(|v| !v.is_empty())
        .map(ToOwned::to_owned)
}

fn parse_optional_json_text(value: Option<String>) -> Option<Value> {
    value.and_then(|raw| serde_json::from_str::<Value>(&raw).ok())
}

#[allow(clippy::too_many_arguments)]
fn append_request_log_filters(
    sql: &mut String,
    values: &mut Vec<SeaValue>,
    idx: &mut usize,
    model: Option<&str>,
    status: Option<&str>,
    api_key_id: Option<&str>,
    username: Option<&str>,
    search: Option<&str>,
    time_from: Option<&str>,
    time_to: Option<&str>,
) {
    if let Some(model) = model {
        let models: Vec<&str> = model
            .split(',')
            .map(str::trim)
            .filter(|s| !s.is_empty())
            .collect();
        if models.len() == 1 {
            sql.push_str(&format!(" AND rl.model LIKE '%' || ${} || '%'", *idx));
            values.push(models[0].into());
            *idx += 1;
        } else if !models.is_empty() {
            sql.push_str(" AND (");
            for (i, m) in models.iter().enumerate() {
                if i > 0 {
                    sql.push_str(" OR ");
                }
                sql.push_str(&format!("rl.model LIKE '%' || ${} || '%'", *idx));
                values.push((*m).into());
                *idx += 1;
            }
            sql.push(')');
        }
    }
    if let Some(status) = status {
        sql.push_str(&format!(" AND rl.status = ${}", *idx));
        values.push(status.into());
        *idx += 1;
    }
    if let Some(api_key_id) = api_key_id {
        sql.push_str(&format!(" AND rl.api_key_id = ${}", *idx));
        values.push(api_key_id.into());
        *idx += 1;
    }
    if let Some(username) = username {
        sql.push_str(&format!(" AND (u.username = ${} OR rl.request_kind = 'active_probe_connectivity')", *idx));
        values.push(username.into());
        *idx += 1;
    }
    if let Some(search) = search {
        let search_like = format!("%{search}%");
        sql.push_str(&format!(
            " AND (rl.model LIKE ${i} OR rl.upstream_model LIKE ${j} OR rl.request_id LIKE ${k} OR rl.request_ip LIKE ${l})",
            i = *idx, j = *idx + 1, k = *idx + 2, l = *idx + 3
        ));
        values.push(search_like.clone().into());
        values.push(search_like.clone().into());
        values.push(search_like.clone().into());
        values.push(search_like.into());
        *idx += 4;
    }
    if let Some(time_from) = time_from {
        sql.push_str(&format!(" AND rl.created_at >= ${}", *idx));
        values.push(time_from.into());
        *idx += 1;
    }
    if let Some(time_to) = time_to {
        sql.push_str(&format!(" AND rl.created_at < ${}", *idx));
        values.push(time_to.into());
        *idx += 1;
    }
}

pub struct AnalyticsModelBucketRow {
    pub bucket_idx: i64,
    pub model: String,
    pub cost_nano: i64,
    pub call_count: i64,
}

pub struct AnalyticsProviderBucketRow {
    pub bucket_idx: i64,
    pub provider_label: String,
    pub call_count: i64,
}

pub struct DashboardAnalyticsRaw {
    pub model_buckets: Vec<AnalyticsModelBucketRow>,
    pub provider_buckets: Vec<AnalyticsProviderBucketRow>,
    pub total_cost_nano_usd: i64,
    pub total_calls: i64,
    pub today_cost_nano_usd: i64,
    pub today_calls: i64,
}

impl UserStore {
    pub async fn cleanup_pending_request_logs(&self) -> Result<u64, String> {
        let result = self.db.write()
            .execute(self.db.stmt(
                "UPDATE request_logs SET status = 'error', error_code = 'server_shutdown', error_message = 'interrupted by server restart' WHERE status = 'pending'",
                vec![],
            ))
            .await
            .map_err(|e| e.to_string())?;
        Ok(result.rows_affected())
    }

    pub async fn insert_request_log_pending(
        &self,
        request_id: &str,
        user_id: &str,
        api_key_id: Option<&str>,
        model: &str,
        is_stream: bool,
        request_ip: Option<&str>,
    ) -> Result<(), String> {
        self.insert_request_log(InsertRequestLog {
            request_id: Some(request_id.to_string()),
            user_id: user_id.to_string(),
            api_key_id: api_key_id.map(ToOwned::to_owned),
            model: model.to_string(),
            provider_id: None,
            upstream_model: None,
            channel_id: None,
            is_stream,
            input_tokens: None,
            output_tokens: None,
            cache_read_tokens: None,
            cache_creation_tokens: None,
            tool_prompt_tokens: None,
            reasoning_tokens: None,
            accepted_prediction_tokens: None,
            rejected_prediction_tokens: None,
            provider_multiplier: None,
            charge_nano_usd: None,
            status: REQUEST_LOG_STATUS_PENDING.to_string(),
            usage_breakdown_json: None,
            billing_breakdown_json: None,
            error_code: None,
            error_message: None,
            error_http_status: None,
            duration_ms: None,
            ttfb_ms: None,
            request_ip: request_ip.map(ToOwned::to_owned),
            reasoning_effort: None,
            tried_providers_json: None,
            request_kind: None,
        })
        .await
    }

    pub async fn update_pending_request_log_channel(
        &self,
        user_id: &str,
        request_id: &str,
        provider_id: &str,
        channel_id: &str,
        upstream_model: &str,
        provider_multiplier: f64,
    ) -> Result<(), String> {
        self.db.write()
            .execute(self.db.stmt(
                r#"UPDATE request_logs
                   SET provider_id = $1, channel_id = $2, upstream_model = $3, provider_multiplier = $4
                   WHERE user_id = $5 AND request_id = $6 AND status = 'pending' AND request_kind IS NULL"#,
                vec![
                    provider_id.into(),
                    channel_id.into(),
                    upstream_model.into(),
                    SeaValue::Double(Some(provider_multiplier)),
                    user_id.into(),
                    request_id.into(),
                ],
            ))
            .await
            .map_err(|e| e.to_string())?;
        Ok(())
    }

    #[allow(clippy::too_many_arguments)]
    pub async fn update_pending_request_log_usage(
        &self,
        user_id: &str,
        request_id: &str,
        input_tokens: u64,
        output_tokens: u64,
        cache_read_tokens: Option<u64>,
        cache_creation_tokens: Option<u64>,
        tool_prompt_tokens: Option<u64>,
        reasoning_tokens: Option<u64>,
        accepted_prediction_tokens: Option<u64>,
        rejected_prediction_tokens: Option<u64>,
        usage_breakdown_json: Option<Value>,
    ) -> Result<(), String> {
        self.db.write()
            .execute(self.db.stmt(
                r#"UPDATE request_logs
                   SET input_tokens = $1, output_tokens = $2, cache_read_tokens = $3,
                        cache_creation_tokens = $4, tool_prompt_tokens = $5, reasoning_tokens = $6,
                        accepted_prediction_tokens = $7, rejected_prediction_tokens = $8,
                        usage_breakdown_json = $9
                   WHERE user_id = $10 AND request_id = $11 AND status = 'pending' AND request_kind IS NULL"#,
                vec![
                    SeaValue::BigInt(Some(input_tokens as i64)),
                    SeaValue::BigInt(Some(output_tokens as i64)),
                    cache_read_tokens.map(|v| SeaValue::BigInt(Some(v as i64))).unwrap_or(SeaValue::BigInt(None)),
                    cache_creation_tokens.map(|v| SeaValue::BigInt(Some(v as i64))).unwrap_or(SeaValue::BigInt(None)),
                    tool_prompt_tokens.map(|v| SeaValue::BigInt(Some(v as i64))).unwrap_or(SeaValue::BigInt(None)),
                    reasoning_tokens.map(|v| SeaValue::BigInt(Some(v as i64))).unwrap_or(SeaValue::BigInt(None)),
                    accepted_prediction_tokens
                        .map(|v| SeaValue::BigInt(Some(v as i64)))
                        .unwrap_or(SeaValue::BigInt(None)),
                    rejected_prediction_tokens
                        .map(|v| SeaValue::BigInt(Some(v as i64)))
                        .unwrap_or(SeaValue::BigInt(None)),
                    usage_breakdown_json.map(|v| v.to_string()).into(),
                    user_id.into(),
                    request_id.into(),
                ],
            ))
            .await
            .map_err(|e| e.to_string())?;
        Ok(())
    }

    pub async fn finalize_request_log(&self, log: InsertRequestLog) -> Result<(), String> {
        if let Some(request_id) = log
            .request_id
            .as_deref()
            .map(str::trim)
            .filter(|v| !v.is_empty())
        {
            let updated = self.db.write()
                .execute(self.db.stmt(
                    r#"UPDATE request_logs
                       SET api_key_id = $1, model = $2, provider_id = $3, upstream_model = $4, channel_id = $5,
                            is_stream = $6, input_tokens = $7, output_tokens = $8, cache_read_tokens = $9,
                            cache_creation_tokens = $10, tool_prompt_tokens = $11, reasoning_tokens = $12,
                            accepted_prediction_tokens = $13, rejected_prediction_tokens = $14, provider_multiplier = $15, charge_nano_usd = $16, status = $17,
                            usage_breakdown_json = $18, billing_breakdown_json = $19, error_code = $20,
                            error_message = $21, error_http_status = $22, duration_ms = $23, ttfb_ms = $24,
                            request_ip = $25, reasoning_effort = $26, tried_providers_json = $27, request_kind = $28
                       WHERE user_id = $29 AND request_id = $30 AND status = 'pending' AND request_kind IS NULL"#,
                    vec![
                        log.api_key_id.clone().into(),
                        log.model.clone().into(),
                        log.provider_id.clone().into(),
                        log.upstream_model.clone().into(),
                        log.channel_id.clone().into(),
                        SeaValue::Int(Some(if log.is_stream { 1 } else { 0 })),
                        log.input_tokens.map(|v| SeaValue::BigInt(Some(v as i64))).unwrap_or(SeaValue::BigInt(None)),
                        log.output_tokens.map(|v| SeaValue::BigInt(Some(v as i64))).unwrap_or(SeaValue::BigInt(None)),
                        log.cache_read_tokens.map(|v| SeaValue::BigInt(Some(v as i64))).unwrap_or(SeaValue::BigInt(None)),
                        log.cache_creation_tokens.map(|v| SeaValue::BigInt(Some(v as i64))).unwrap_or(SeaValue::BigInt(None)),
                        log.tool_prompt_tokens.map(|v| SeaValue::BigInt(Some(v as i64))).unwrap_or(SeaValue::BigInt(None)),
                        log.reasoning_tokens.map(|v| SeaValue::BigInt(Some(v as i64))).unwrap_or(SeaValue::BigInt(None)),
                        log.accepted_prediction_tokens.map(|v| SeaValue::BigInt(Some(v as i64))).unwrap_or(SeaValue::BigInt(None)),
                        log.rejected_prediction_tokens.map(|v| SeaValue::BigInt(Some(v as i64))).unwrap_or(SeaValue::BigInt(None)),
                        log.provider_multiplier.map(|v| SeaValue::Double(Some(v))).unwrap_or(SeaValue::Double(None)),
                        log.charge_nano_usd.map(|v| v.to_string()).into(),
                        log.status.clone().into(),
                        log.usage_breakdown_json.as_ref().map(Value::to_string).into(),
                        log.billing_breakdown_json.as_ref().map(Value::to_string).into(),
                        log.error_code.clone().into(),
                        log.error_message.clone().into(),
                        log.error_http_status.map(|v| SeaValue::BigInt(Some(i64::from(v)))).unwrap_or(SeaValue::BigInt(None)),
                        log.duration_ms.map(|v| SeaValue::BigInt(Some(v as i64))).unwrap_or(SeaValue::BigInt(None)),
                        log.ttfb_ms.map(|v| SeaValue::BigInt(Some(v as i64))).unwrap_or(SeaValue::BigInt(None)),
                        log.request_ip.clone().into(),
                        log.reasoning_effort.clone().into(),
                        log.tried_providers_json.as_ref().map(Value::to_string).into(),
                        log.request_kind.clone().into(),
                        log.user_id.clone().into(),
                        request_id.into(),
                    ],
                ))
                .await
                .map_err(|e| e.to_string())?;

            if updated.rows_affected() > 0 {
                return Ok(());
            }
        }

        self.insert_request_log(log).await
    }

    pub async fn insert_request_log(&self, log: InsertRequestLog) -> Result<(), String> {
        let id = uuid::Uuid::new_v4().to_string();
        let now = Utc::now().to_rfc3339();
        self.db.write()
            .execute(self.db.stmt(
                r#"INSERT INTO request_logs
                   (id, request_id, user_id, api_key_id, model, provider_id, upstream_model, channel_id, is_stream,
                    input_tokens, output_tokens, cache_read_tokens, cache_creation_tokens, tool_prompt_tokens, reasoning_tokens,
                    accepted_prediction_tokens, rejected_prediction_tokens,
                    provider_multiplier, charge_nano_usd, status, usage_breakdown_json,
                    billing_breakdown_json, error_code, error_message, error_http_status,
                    duration_ms, ttfb_ms, request_ip, reasoning_effort, tried_providers_json, request_kind, created_at)
                   VALUES ($1, $2, $3, $4, $5, $6, $7, $8, $9, $10, $11, $12, $13, $14, $15, $16, $17, $18, $19, $20, $21, $22, $23, $24, $25, $26, $27, $28, $29, $30, $31, $32)"#,
                vec![
                    id.into(),
                    log.request_id.into(),
                    log.user_id.into(),
                    log.api_key_id.into(),
                    log.model.into(),
                    log.provider_id.into(),
                    log.upstream_model.into(),
                    log.channel_id.into(),
                    SeaValue::Int(Some(if log.is_stream { 1 } else { 0 })),
                    log.input_tokens.map(|v| SeaValue::BigInt(Some(v as i64))).unwrap_or(SeaValue::BigInt(None)),
                    log.output_tokens.map(|v| SeaValue::BigInt(Some(v as i64))).unwrap_or(SeaValue::BigInt(None)),
                    log.cache_read_tokens.map(|v| SeaValue::BigInt(Some(v as i64))).unwrap_or(SeaValue::BigInt(None)),
                    log.cache_creation_tokens.map(|v| SeaValue::BigInt(Some(v as i64))).unwrap_or(SeaValue::BigInt(None)),
                    log.tool_prompt_tokens.map(|v| SeaValue::BigInt(Some(v as i64))).unwrap_or(SeaValue::BigInt(None)),
                    log.reasoning_tokens.map(|v| SeaValue::BigInt(Some(v as i64))).unwrap_or(SeaValue::BigInt(None)),
                    log.accepted_prediction_tokens.map(|v| SeaValue::BigInt(Some(v as i64))).unwrap_or(SeaValue::BigInt(None)),
                    log.rejected_prediction_tokens.map(|v| SeaValue::BigInt(Some(v as i64))).unwrap_or(SeaValue::BigInt(None)),
                    log.provider_multiplier.map(|v| SeaValue::Double(Some(v))).unwrap_or(SeaValue::Double(None)),
                    log.charge_nano_usd.map(|v| v.to_string()).into(),
                    log.status.into(),
                    log.usage_breakdown_json.map(|v| v.to_string()).into(),
                    log.billing_breakdown_json.map(|v| v.to_string()).into(),
                    log.error_code.into(),
                    log.error_message.into(),
                    log.error_http_status.map(|v| SeaValue::BigInt(Some(i64::from(v)))).unwrap_or(SeaValue::BigInt(None)),
                    log.duration_ms.map(|v| SeaValue::BigInt(Some(v as i64))).unwrap_or(SeaValue::BigInt(None)),
                    log.ttfb_ms.map(|v| SeaValue::BigInt(Some(v as i64))).unwrap_or(SeaValue::BigInt(None)),
                    log.request_ip.into(),
                    log.reasoning_effort.into(),
                    log.tried_providers_json.map(|v| v.to_string()).into(),
                    log.request_kind.into(),
                    now.into(),
                ],
            ))
            .await
            .map_err(|e| e.to_string())?;
        Ok(())
    }

    #[allow(clippy::too_many_arguments)]
    pub async fn list_request_logs_by_user(
        &self,
        user_id: &str,
        limit: i64,
        offset: i64,
        model: Option<&str>,
        status: Option<&str>,
        api_key_id: Option<&str>,
        search: Option<&str>,
        time_from: Option<&str>,
        time_to: Option<&str>,
    ) -> Result<(Vec<RequestLogRow>, i64, String), String> {
        let model = normalize_request_log_filter(model);
        let status = normalize_request_log_filter(status);
        let api_key_id = normalize_request_log_filter(api_key_id);
        let search = normalize_request_log_filter(search);

        // Count query
        let mut count_sql = "SELECT COUNT(*) as cnt FROM request_logs rl WHERE rl.user_id = $1".to_string();
        let mut count_values: Vec<SeaValue> = vec![user_id.into()];
        let mut count_idx = 2usize;
        append_request_log_filters(
            &mut count_sql,
            &mut count_values,
            &mut count_idx,
            model.as_deref(),
            status.as_deref(),
            api_key_id.as_deref(),
            None,
            search.as_deref(),
            time_from,
            time_to,
        );
        let count_row = self.db.read()
            .query_one(self.db.stmt(&count_sql, count_values))
            .await
            .map_err(|e| e.to_string())?;
        let total: i64 = count_row
            .ok_or_else(|| "no count row".to_string())?
            .try_get("", "cnt")
            .map_err(|e| e.to_string())?;

        // Sum query
        let mut sum_sql = "SELECT CAST(COALESCE(SUM(CAST(rl.charge_nano_usd AS BIGINT)), 0) AS BIGINT) as total_charge FROM request_logs rl WHERE rl.user_id = $1".to_string();
        let mut sum_values: Vec<SeaValue> = vec![user_id.into()];
        let mut sum_idx = 2usize;
        append_request_log_filters(
            &mut sum_sql,
            &mut sum_values,
            &mut sum_idx,
            model.as_deref(),
            status.as_deref(),
            api_key_id.as_deref(),
            None,
            search.as_deref(),
            time_from,
            time_to,
        );
        let sum_row = self.db.read()
            .query_one(self.db.stmt(&sum_sql, sum_values))
            .await
            .map_err(|e| e.to_string())?;
        let total_charge: i64 = sum_row
            .ok_or_else(|| "no sum row".to_string())?
            .try_get("", "total_charge")
            .map_err(|e| e.to_string())?;
        let total_charge_nano_usd = total_charge.to_string();

        // Rows query
        let mut rows_sql = r#"SELECT rl.id, rl.request_id, rl.user_id, rl.api_key_id, rl.model, rl.provider_id, rl.upstream_model,
                      rl.channel_id, rl.is_stream,
                      rl.input_tokens, rl.output_tokens, rl.cache_read_tokens, rl.cache_creation_tokens,
                      rl.tool_prompt_tokens, rl.reasoning_tokens,
                      rl.accepted_prediction_tokens, rl.rejected_prediction_tokens,
                      rl.provider_multiplier, rl.charge_nano_usd, rl.status,
                      rl.usage_breakdown_json, rl.billing_breakdown_json,
                      rl.error_code, rl.error_message, rl.error_http_status,
                      rl.duration_ms, rl.ttfb_ms, rl.request_ip, rl.reasoning_effort, rl.request_kind, rl.created_at,
                      u.username, ak.name AS api_key_name, ch.name AS channel_name,
                      mp.name AS provider_name
               FROM request_logs rl
               LEFT JOIN users u ON rl.user_id = u.id
               LEFT JOIN api_keys ak ON rl.api_key_id = ak.id
               LEFT JOIN monoize_channels ch ON rl.channel_id = ch.id
               LEFT JOIN monoize_providers mp ON rl.provider_id = mp.id
               WHERE rl.user_id = $1"#.to_string();
        let mut rows_values: Vec<SeaValue> = vec![user_id.into()];
        let mut rows_idx = 2usize;
        append_request_log_filters(
            &mut rows_sql,
            &mut rows_values,
            &mut rows_idx,
            model.as_deref(),
            status.as_deref(),
            api_key_id.as_deref(),
            None,
            search.as_deref(),
            time_from,
            time_to,
        );
        rows_sql.push_str(&format!(" ORDER BY rl.created_at DESC LIMIT ${} OFFSET ${}", rows_idx, rows_idx + 1));
        rows_values.push(SeaValue::BigInt(Some(limit)));
        rows_values.push(SeaValue::BigInt(Some(offset)));

        let rows = self.db.read()
            .query_all(self.db.stmt(&rows_sql, rows_values))
            .await
            .map_err(|e| e.to_string())?;

        let logs = rows
            .into_iter()
            .map(|row| row_to_request_log(&row))
            .collect();

        Ok((logs, total, total_charge_nano_usd))
    }

    #[allow(clippy::too_many_arguments)]
    pub async fn list_all_request_logs(
        &self,
        limit: i64,
        offset: i64,
        model: Option<&str>,
        status: Option<&str>,
        api_key_id: Option<&str>,
        username: Option<&str>,
        search: Option<&str>,
        time_from: Option<&str>,
        time_to: Option<&str>,
    ) -> Result<(Vec<RequestLogRow>, i64, String), String> {
        let model = normalize_request_log_filter(model);
        let status = normalize_request_log_filter(status);
        let api_key_id = normalize_request_log_filter(api_key_id);
        let username = normalize_request_log_filter(username);
        let search = normalize_request_log_filter(search);

        // Count query
        let mut count_sql = r#"SELECT COUNT(*) as cnt FROM request_logs rl
               LEFT JOIN users u ON rl.user_id = u.id
               WHERE 1 = 1"#.to_string();
        let mut count_values: Vec<SeaValue> = Vec::new();
        let mut count_idx = 1usize;
        append_request_log_filters(
            &mut count_sql,
            &mut count_values,
            &mut count_idx,
            model.as_deref(),
            status.as_deref(),
            api_key_id.as_deref(),
            username.as_deref(),
            search.as_deref(),
            time_from,
            time_to,
        );
        let count_row = self.db.read()
            .query_one(self.db.stmt(&count_sql, count_values))
            .await
            .map_err(|e| e.to_string())?;
        let total: i64 = count_row
            .ok_or_else(|| "no count row".to_string())?
            .try_get("", "cnt")
            .map_err(|e| e.to_string())?;

        // Sum query
        let mut sum_sql = r#"SELECT CAST(COALESCE(SUM(CAST(rl.charge_nano_usd AS BIGINT)), 0) AS BIGINT) as total_charge FROM request_logs rl
               LEFT JOIN users u ON rl.user_id = u.id
               WHERE 1 = 1"#.to_string();
        let mut sum_values: Vec<SeaValue> = Vec::new();
        let mut sum_idx = 1usize;
        append_request_log_filters(
            &mut sum_sql,
            &mut sum_values,
            &mut sum_idx,
            model.as_deref(),
            status.as_deref(),
            api_key_id.as_deref(),
            username.as_deref(),
            search.as_deref(),
            time_from,
            time_to,
        );
        let sum_row = self.db.read()
            .query_one(self.db.stmt(&sum_sql, sum_values))
            .await
            .map_err(|e| e.to_string())?;
        let total_charge: i64 = sum_row
            .ok_or_else(|| "no sum row".to_string())?
            .try_get("", "total_charge")
            .map_err(|e| e.to_string())?;
        let total_charge_nano_usd = total_charge.to_string();

        // Rows query
        let mut rows_sql = r#"SELECT rl.id, rl.request_id, rl.user_id, rl.api_key_id, rl.model, rl.provider_id, rl.upstream_model,
                      rl.channel_id, rl.is_stream,
                      rl.input_tokens, rl.output_tokens, rl.cache_read_tokens, rl.cache_creation_tokens,
                      rl.tool_prompt_tokens, rl.reasoning_tokens,
                      rl.accepted_prediction_tokens, rl.rejected_prediction_tokens,
                      rl.provider_multiplier, rl.charge_nano_usd, rl.status,
                      rl.usage_breakdown_json, rl.billing_breakdown_json,
                      rl.error_code, rl.error_message, rl.error_http_status,
                      rl.duration_ms, rl.ttfb_ms, rl.request_ip, rl.reasoning_effort, rl.request_kind, rl.created_at,
                      u.username, ak.name AS api_key_name, ch.name AS channel_name,
                      mp.name AS provider_name
               FROM request_logs rl
               LEFT JOIN users u ON rl.user_id = u.id
               LEFT JOIN api_keys ak ON rl.api_key_id = ak.id
               LEFT JOIN monoize_channels ch ON rl.channel_id = ch.id
               LEFT JOIN monoize_providers mp ON rl.provider_id = mp.id
               WHERE 1 = 1"#.to_string();
        let mut rows_values: Vec<SeaValue> = Vec::new();
        let mut rows_idx = 1usize;
        append_request_log_filters(
            &mut rows_sql,
            &mut rows_values,
            &mut rows_idx,
            model.as_deref(),
            status.as_deref(),
            api_key_id.as_deref(),
            username.as_deref(),
            search.as_deref(),
            time_from,
            time_to,
        );
        rows_sql.push_str(&format!(" ORDER BY rl.created_at DESC LIMIT ${} OFFSET ${}", rows_idx, rows_idx + 1));
        rows_values.push(SeaValue::BigInt(Some(limit)));
        rows_values.push(SeaValue::BigInt(Some(offset)));

        let rows = self.db.read()
            .query_all(self.db.stmt(&rows_sql, rows_values))
            .await
            .map_err(|e| e.to_string())?;

        let logs = rows
            .into_iter()
            .map(|row| row_to_request_log(&row))
            .collect();

        Ok((logs, total, total_charge_nano_usd))
    }

    pub async fn get_dashboard_analytics(
        &self,
        user_id: Option<&str>,
        time_from: &str,
        time_to: &str,
        today_start: &str,
        bucket_count: i64,
        bucket_width_days: f64,
    ) -> Result<DashboardAnalyticsRaw, String> {
        let is_sqlite = self.db.is_sqlite();

        // 1. Model bucketed aggregation (cost + calls)
        let bucket_expr = if is_sqlite {
            "CAST((julianday(rl.created_at) - julianday($1)) / $2 AS INTEGER)".to_string()
        } else {
            "CAST(EXTRACT(EPOCH FROM (CAST(rl.created_at AS TIMESTAMPTZ) - CAST($1 AS TIMESTAMPTZ))) / ($2 * 86400.0) AS INTEGER)".to_string()
        };

        let mut model_sql = format!(
            r#"SELECT
                 {bucket_expr} AS bucket_idx,
                 rl.model,
                 CAST(COALESCE(SUM(CAST(rl.charge_nano_usd AS BIGINT)), 0) AS BIGINT) AS cost_nano,
                 COUNT(*) AS call_count
               FROM request_logs rl
               WHERE rl.created_at >= $3 AND rl.created_at < $4"#
        );
        let mut model_values: Vec<SeaValue> = vec![
            time_from.into(),
            SeaValue::Double(Some(bucket_width_days)),
            time_from.into(),
            time_to.into(),
        ];
        let mut model_idx = 5usize;

        if let Some(uid) = user_id {
            model_sql.push_str(&format!(" AND rl.user_id = ${model_idx}"));
            model_values.push(uid.into());
            model_idx += 1;
        }
        let _ = model_idx;
        model_sql.push_str(" GROUP BY bucket_idx, rl.model");

        let model_rows = self.db.read()
            .query_all(self.db.stmt(&model_sql, model_values))
            .await
            .map_err(|e| e.to_string())?;

        let model_buckets: Vec<AnalyticsModelBucketRow> = model_rows
            .into_iter()
            .map(|row| {
                let idx: i64 = row.try_get("", "bucket_idx").unwrap_or(0);
                AnalyticsModelBucketRow {
                    bucket_idx: idx.clamp(0, bucket_count - 1),
                    model: row.try_get("", "model").unwrap_or_default(),
                    cost_nano: row.try_get("", "cost_nano").unwrap_or(0),
                    call_count: row.try_get("", "call_count").unwrap_or(0),
                }
            })
            .collect();

        // 2. Provider bucketed aggregation (calls only)
        let mut prov_sql = format!(
            r#"SELECT
                 {bucket_expr} AS bucket_idx,
                 COALESCE(mp.name, rl.provider_id, 'unknown') AS provider_label,
                 COUNT(*) AS call_count
               FROM request_logs rl
               LEFT JOIN monoize_providers mp ON rl.provider_id = mp.id
               WHERE rl.created_at >= $3 AND rl.created_at < $4"#
        );
        let mut prov_values: Vec<SeaValue> = vec![
            time_from.into(),
            SeaValue::Double(Some(bucket_width_days)),
            time_from.into(),
            time_to.into(),
        ];
        let mut prov_idx = 5usize;

        if let Some(uid) = user_id {
            prov_sql.push_str(&format!(" AND rl.user_id = ${prov_idx}"));
            prov_values.push(uid.into());
            prov_idx += 1;
        }
        let _ = prov_idx;
        prov_sql.push_str(" GROUP BY bucket_idx, provider_label");

        let prov_rows = self.db.read()
            .query_all(self.db.stmt(&prov_sql, prov_values))
            .await
            .map_err(|e| e.to_string())?;

        let provider_buckets: Vec<AnalyticsProviderBucketRow> = prov_rows
            .into_iter()
            .map(|row| {
                let idx: i64 = row.try_get("", "bucket_idx").unwrap_or(0);
                AnalyticsProviderBucketRow {
                    bucket_idx: idx.clamp(0, bucket_count - 1),
                    provider_label: row.try_get("", "provider_label").unwrap_or_default(),
                    call_count: row.try_get("", "call_count").unwrap_or(0),
                }
            })
            .collect();

        // 3. Total stats for the range
        let mut total_sql = r#"SELECT
                 CAST(COALESCE(SUM(CAST(rl.charge_nano_usd AS BIGINT)), 0) AS BIGINT) AS total_cost,
                 COUNT(*) AS total_calls
               FROM request_logs rl
               WHERE rl.created_at >= $1 AND rl.created_at < $2"#.to_string();
        let mut total_values: Vec<SeaValue> = vec![
            time_from.into(),
            time_to.into(),
        ];
        let mut total_idx = 3usize;

        if let Some(uid) = user_id {
            total_sql.push_str(&format!(" AND rl.user_id = ${total_idx}"));
            total_values.push(uid.into());
            total_idx += 1;
        }
        let _ = total_idx;

        let total_row = self.db.read()
            .query_one(self.db.stmt(&total_sql, total_values))
            .await
            .map_err(|e| e.to_string())?;
        let total_row = total_row.ok_or_else(|| "no total row".to_string())?;

        let total_cost_nano_usd: i64 = total_row.try_get("", "total_cost").unwrap_or(0);
        let total_calls: i64 = total_row.try_get("", "total_calls").unwrap_or(0);

        // 4. Today stats
        let mut today_sql = r#"SELECT
                 CAST(COALESCE(SUM(CAST(rl.charge_nano_usd AS BIGINT)), 0) AS BIGINT) AS today_cost,
                 COUNT(*) AS today_calls
               FROM request_logs rl
               WHERE rl.created_at >= $1"#.to_string();
        let mut today_values: Vec<SeaValue> = vec![
            today_start.into(),
        ];
        let mut today_idx = 2usize;

        if let Some(uid) = user_id {
            today_sql.push_str(&format!(" AND rl.user_id = ${today_idx}"));
            today_values.push(uid.into());
            today_idx += 1;
        }
        let _ = today_idx;

        let today_row = self.db.read()
            .query_one(self.db.stmt(&today_sql, today_values))
            .await
            .map_err(|e| e.to_string())?;
        let today_row = today_row.ok_or_else(|| "no today row".to_string())?;

        let today_cost_nano_usd: i64 = today_row.try_get("", "today_cost").unwrap_or(0);
        let today_calls: i64 = today_row.try_get("", "today_calls").unwrap_or(0);

        Ok(DashboardAnalyticsRaw {
            model_buckets,
            provider_buckets,
            total_cost_nano_usd,
            total_calls,
            today_cost_nano_usd,
            today_calls,
        })
    }
}

fn row_to_request_log(row: &QueryResult) -> RequestLogRow {
    RequestLogRow {
        id: row.try_get("", "id").unwrap_or_default(),
        request_id: row.try_get("", "request_id").unwrap_or(None),
        user_id: row.try_get("", "user_id").unwrap_or_default(),
        api_key_id: row.try_get("", "api_key_id").unwrap_or(None),
        model: row.try_get("", "model").unwrap_or_default(),
        provider_id: row.try_get("", "provider_id").unwrap_or(None),
        upstream_model: row.try_get("", "upstream_model").unwrap_or(None),
        channel_id: row.try_get("", "channel_id").unwrap_or(None),
        is_stream: row.try_get::<i32>("", "is_stream").unwrap_or(0) == 1,
        input_tokens: row.try_get("", "input_tokens").unwrap_or(None),
        output_tokens: row.try_get("", "output_tokens").unwrap_or(None),
        cache_read_tokens: row.try_get("", "cache_read_tokens").unwrap_or(None),
        cache_creation_tokens: row.try_get("", "cache_creation_tokens").unwrap_or(None),
        tool_prompt_tokens: row.try_get("", "tool_prompt_tokens").unwrap_or(None),
        reasoning_tokens: row.try_get("", "reasoning_tokens").unwrap_or(None),
        accepted_prediction_tokens: row
            .try_get("", "accepted_prediction_tokens")
            .unwrap_or(None),
        rejected_prediction_tokens: row
            .try_get("", "rejected_prediction_tokens")
            .unwrap_or(None),
        provider_multiplier: row.try_get("", "provider_multiplier").unwrap_or(None),
        charge_nano_usd: row
            .try_get::<Option<String>>("", "charge_nano_usd")
            .unwrap_or(None),
        status: row
            .try_get("", "status")
            .unwrap_or_else(|_| "unknown".to_string()),
        usage_breakdown_json: parse_optional_json_text(
            row.try_get::<Option<String>>("", "usage_breakdown_json")
                .unwrap_or(None),
        ),
        billing_breakdown_json: parse_optional_json_text(
            row.try_get::<Option<String>>("", "billing_breakdown_json")
                .unwrap_or(None),
        ),
        error_code: row.try_get("", "error_code").unwrap_or(None),
        error_message: row.try_get("", "error_message").unwrap_or(None),
        error_http_status: row.try_get("", "error_http_status").unwrap_or(None),
        duration_ms: row.try_get("", "duration_ms").unwrap_or(None),
        ttfb_ms: row.try_get("", "ttfb_ms").unwrap_or(None),
        request_ip: row.try_get("", "request_ip").unwrap_or(None),
        reasoning_effort: row.try_get("", "reasoning_effort").unwrap_or(None),
        tried_providers_json: parse_optional_json_text(
            row.try_get::<Option<String>>("", "tried_providers_json")
                .unwrap_or(None),
        ),
        request_kind: row.try_get("", "request_kind").unwrap_or(None),
        created_at: row.try_get("", "created_at").unwrap_or_default(),
        username: row.try_get("", "username").unwrap_or(None),
        api_key_name: row.try_get("", "api_key_name").unwrap_or(None),
        channel_name: row.try_get("", "channel_name").unwrap_or(None),
        provider_name: row.try_get("", "provider_name").unwrap_or(None),
    }
}

pub fn parse_nano_usd(value: &str) -> Result<i128, String> {
    value
        .trim()
        .parse::<i128>()
        .map_err(|_| "invalid_nano_usd".to_string())
}

pub fn parse_usd_to_nano(value: &str) -> Result<i128, String> {
    let s = value.trim();
    if s.is_empty() {
        return Err("invalid_usd".to_string());
    }
    let (negative, rest) = if let Some(rem) = s.strip_prefix('-') {
        (true, rem)
    } else if let Some(rem) = s.strip_prefix('+') {
        (false, rem)
    } else {
        (false, s)
    };

    let (whole_raw, frac_raw) = match rest.split_once('.') {
        Some((w, f)) => (w, f),
        None => (rest, ""),
    };
    if whole_raw.is_empty() && frac_raw.is_empty() {
        return Err("invalid_usd".to_string());
    }
    if !whole_raw.chars().all(|c| c.is_ascii_digit()) {
        return Err("invalid_usd".to_string());
    }
    if !frac_raw.chars().all(|c| c.is_ascii_digit()) {
        return Err("invalid_usd".to_string());
    }

    let whole = if whole_raw.is_empty() {
        0i128
    } else {
        whole_raw
            .parse::<i128>()
            .map_err(|_| "invalid_usd".to_string())?
    };
    let mut frac = frac_raw.to_string();
    if frac.len() > 9 {
        frac.truncate(9);
    }
    while frac.len() < 9 {
        frac.push('0');
    }
    let frac_value = if frac.is_empty() {
        0i128
    } else {
        frac.parse::<i128>()
            .map_err(|_| "invalid_usd".to_string())?
    };

    let base = whole
        .checked_mul(1_000_000_000)
        .and_then(|v| v.checked_add(frac_value))
        .ok_or_else(|| "usd_overflow".to_string())?;
    if negative {
        base.checked_neg().ok_or_else(|| "usd_overflow".to_string())
    } else {
        Ok(base)
    }
}

pub fn format_nano_to_usd(nano: i128) -> String {
    let negative = nano < 0;
    let abs = nano.abs();
    let whole = abs / 1_000_000_000;
    let frac = abs % 1_000_000_000;
    if frac == 0 {
        return if negative {
            format!("-{whole}")
        } else {
            whole.to_string()
        };
    }
    let mut frac_str = format!("{frac:09}");
    while frac_str.ends_with('0') {
        frac_str.pop();
    }
    if negative {
        format!("-{whole}.{frac_str}")
    } else {
        format!("{whole}.{frac_str}")
    }
}
