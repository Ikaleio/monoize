use crate::transforms::TransformRuleConfig;
use argon2::{
    Argon2,
    password_hash::{PasswordHash, PasswordHasher, PasswordVerifier, SaltString, rand_core::OsRng},
};
use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use serde_json::Value;
use sqlx::{Pool, QueryBuilder, Row, Sqlite};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum UserRole {
    SuperAdmin,
    Admin,
    User,
}

impl UserRole {
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
    pool: Pool<Sqlite>,
}

impl UserStore {
    pub async fn new(pool: Pool<Sqlite>) -> Result<Self, String> {
        sqlx::query(
            r#"CREATE TABLE IF NOT EXISTS users (
                id TEXT PRIMARY KEY,
                username TEXT NOT NULL UNIQUE,
                password_hash TEXT NOT NULL,
                role TEXT NOT NULL DEFAULT 'user',
                created_at TEXT NOT NULL,
                updated_at TEXT NOT NULL,
                last_login_at TEXT,
                enabled INTEGER NOT NULL DEFAULT 1,
                balance_nano_usd TEXT NOT NULL DEFAULT '0',
                balance_unlimited INTEGER NOT NULL DEFAULT 0
            )"#,
        )
        .execute(&pool)
        .await
        .map_err(|e| e.to_string())?;

        sqlx::query(
            r#"CREATE TABLE IF NOT EXISTS sessions (
                id TEXT PRIMARY KEY,
                user_id TEXT NOT NULL,
                token TEXT NOT NULL UNIQUE,
                created_at TEXT NOT NULL,
                expires_at TEXT NOT NULL,
                FOREIGN KEY (user_id) REFERENCES users(id) ON DELETE CASCADE
            )"#,
        )
        .execute(&pool)
        .await
        .map_err(|e| e.to_string())?;

        sqlx::query(
            r#"CREATE TABLE IF NOT EXISTS api_keys (
                id TEXT PRIMARY KEY,
                user_id TEXT NOT NULL,
                name TEXT NOT NULL,
                key_prefix TEXT NOT NULL,
                key TEXT NOT NULL DEFAULT '',
                key_hash TEXT NOT NULL,
                created_at TEXT NOT NULL,
                expires_at TEXT,
                last_used_at TEXT,
                enabled INTEGER NOT NULL DEFAULT 1,
                quota_remaining INTEGER,
                quota_unlimited INTEGER NOT NULL DEFAULT 1,
                model_limits_enabled INTEGER NOT NULL DEFAULT 0,
                model_limits TEXT NOT NULL DEFAULT '[]',
                ip_whitelist TEXT NOT NULL DEFAULT '[]',
                token_group TEXT NOT NULL DEFAULT 'default',
                max_multiplier REAL,
                transforms TEXT NOT NULL DEFAULT '[]',
                FOREIGN KEY (user_id) REFERENCES users(id) ON DELETE CASCADE
            )"#,
        )
        .execute(&pool)
        .await
        .map_err(|e| e.to_string())?;

        sqlx::query(
            r#"CREATE TABLE IF NOT EXISTS billing_ledger (
                id TEXT PRIMARY KEY,
                user_id TEXT NOT NULL,
                kind TEXT NOT NULL,
                delta_nano_usd TEXT NOT NULL,
                balance_after_nano_usd TEXT,
                meta_json TEXT NOT NULL DEFAULT '{}',
                created_at TEXT NOT NULL,
                FOREIGN KEY (user_id) REFERENCES users(id) ON DELETE CASCADE
            )"#,
        )
        .execute(&pool)
        .await
        .map_err(|e| e.to_string())?;

        sqlx::query(
            "CREATE INDEX IF NOT EXISTS idx_billing_ledger_user ON billing_ledger(user_id)",
        )
        .execute(&pool)
        .await
        .map_err(|e| e.to_string())?;

        sqlx::query(
            r#"CREATE TABLE IF NOT EXISTS request_logs (
                id TEXT PRIMARY KEY,
                request_id TEXT,
                user_id TEXT NOT NULL,
                api_key_id TEXT,
                model TEXT NOT NULL,
                provider_id TEXT,
                upstream_model TEXT,
                channel_id TEXT,
                is_stream INTEGER NOT NULL DEFAULT 0,
                prompt_tokens INTEGER,
                completion_tokens INTEGER,
                cached_tokens INTEGER,
                reasoning_tokens INTEGER,
                provider_multiplier REAL,
                charge_nano_usd TEXT,
                status TEXT NOT NULL DEFAULT 'success',
                usage_breakdown_json TEXT,
                billing_breakdown_json TEXT,
                error_code TEXT,
                error_message TEXT,
                error_http_status INTEGER,
                duration_ms INTEGER,
                ttfb_ms INTEGER,
                request_ip TEXT,
                created_at TEXT NOT NULL,
                FOREIGN KEY (user_id) REFERENCES users(id) ON DELETE CASCADE
            )"#,
        )
        .execute(&pool)
        .await
        .map_err(|e| e.to_string())?;

        for col in &[
            "ALTER TABLE request_logs ADD COLUMN request_id TEXT",
            "ALTER TABLE request_logs ADD COLUMN channel_id TEXT",
            "ALTER TABLE request_logs ADD COLUMN ttfb_ms INTEGER",
            "ALTER TABLE request_logs ADD COLUMN request_ip TEXT",
            "ALTER TABLE request_logs ADD COLUMN usage_breakdown_json TEXT",
            "ALTER TABLE request_logs ADD COLUMN billing_breakdown_json TEXT",
            "ALTER TABLE request_logs ADD COLUMN error_code TEXT",
            "ALTER TABLE request_logs ADD COLUMN error_message TEXT",
            "ALTER TABLE request_logs ADD COLUMN error_http_status INTEGER",
        ] {
            let _ = sqlx::query(col).execute(&pool).await;
        }

        sqlx::query(
            "CREATE INDEX IF NOT EXISTS idx_request_logs_user ON request_logs(user_id, created_at DESC)",
        )
        .execute(&pool)
        .await
        .map_err(|e| e.to_string())?;

        sqlx::query("CREATE INDEX IF NOT EXISTS idx_sessions_token ON sessions(token)")
            .execute(&pool)
            .await
            .map_err(|e| e.to_string())?;

        sqlx::query("CREATE INDEX IF NOT EXISTS idx_api_keys_prefix ON api_keys(key_prefix)")
            .execute(&pool)
            .await
            .map_err(|e| e.to_string())?;

        let has_balance_nano_column: bool = sqlx::query_scalar::<_, i32>(
            "SELECT COUNT(*) FROM pragma_table_info('users') WHERE name = 'balance_nano_usd'",
        )
        .fetch_one(&pool)
        .await
        .map(|c| c > 0)
        .unwrap_or(false);
        if !has_balance_nano_column {
            sqlx::query("ALTER TABLE users ADD COLUMN balance_nano_usd TEXT NOT NULL DEFAULT '0'")
                .execute(&pool)
                .await
                .ok();
        }

        let has_balance_unlimited_column: bool = sqlx::query_scalar::<_, i32>(
            "SELECT COUNT(*) FROM pragma_table_info('users') WHERE name = 'balance_unlimited'",
        )
        .fetch_one(&pool)
        .await
        .map(|c| c > 0)
        .unwrap_or(false);
        if !has_balance_unlimited_column {
            sqlx::query(
                "ALTER TABLE users ADD COLUMN balance_unlimited INTEGER NOT NULL DEFAULT 0",
            )
            .execute(&pool)
            .await
            .ok();
        }

        // Migrate existing api_keys table to add new columns if they don't exist
        // SQLite doesn't support IF NOT EXISTS for ALTER TABLE, so we check the schema first
        let has_quota_column: bool = sqlx::query_scalar::<_, i32>(
            "SELECT COUNT(*) FROM pragma_table_info('api_keys') WHERE name = 'quota_remaining'",
        )
        .fetch_one(&pool)
        .await
        .map(|c| c > 0)
        .unwrap_or(false);

        if !has_quota_column {
            // Add new columns for existing databases
            sqlx::query("ALTER TABLE api_keys ADD COLUMN quota_remaining INTEGER")
                .execute(&pool)
                .await
                .ok(); // Ignore error if column already exists
            sqlx::query(
                "ALTER TABLE api_keys ADD COLUMN quota_unlimited INTEGER NOT NULL DEFAULT 1",
            )
            .execute(&pool)
            .await
            .ok();
            sqlx::query(
                "ALTER TABLE api_keys ADD COLUMN model_limits_enabled INTEGER NOT NULL DEFAULT 0",
            )
            .execute(&pool)
            .await
            .ok();
            sqlx::query("ALTER TABLE api_keys ADD COLUMN model_limits TEXT NOT NULL DEFAULT '[]'")
                .execute(&pool)
                .await
                .ok();
            sqlx::query("ALTER TABLE api_keys ADD COLUMN ip_whitelist TEXT NOT NULL DEFAULT '[]'")
                .execute(&pool)
                .await
                .ok();
            sqlx::query(
                "ALTER TABLE api_keys ADD COLUMN token_group TEXT NOT NULL DEFAULT 'default'",
            )
            .execute(&pool)
            .await
            .ok();
            sqlx::query("ALTER TABLE api_keys ADD COLUMN max_multiplier REAL")
                .execute(&pool)
                .await
                .ok();
            sqlx::query("ALTER TABLE api_keys ADD COLUMN transforms TEXT NOT NULL DEFAULT '[]'")
                .execute(&pool)
                .await
                .ok();
        }

        // Migrate to add key column for storing the full API key
        let has_key_column: bool = sqlx::query_scalar::<_, i32>(
            "SELECT COUNT(*) FROM pragma_table_info('api_keys') WHERE name = 'key'",
        )
        .fetch_one(&pool)
        .await
        .map(|c| c > 0)
        .unwrap_or(false);

        if !has_key_column {
            sqlx::query("ALTER TABLE api_keys ADD COLUMN key TEXT NOT NULL DEFAULT ''")
                .execute(&pool)
                .await
                .ok();
        }

        let has_max_multiplier_column: bool = sqlx::query_scalar::<_, i32>(
            "SELECT COUNT(*) FROM pragma_table_info('api_keys') WHERE name = 'max_multiplier'",
        )
        .fetch_one(&pool)
        .await
        .map(|c| c > 0)
        .unwrap_or(false);
        if !has_max_multiplier_column {
            sqlx::query("ALTER TABLE api_keys ADD COLUMN max_multiplier REAL")
                .execute(&pool)
                .await
                .ok();
        }

        let has_transforms_column: bool = sqlx::query_scalar::<_, i32>(
            "SELECT COUNT(*) FROM pragma_table_info('api_keys') WHERE name = 'transforms'",
        )
        .fetch_one(&pool)
        .await
        .map(|c| c > 0)
        .unwrap_or(false);
        if !has_transforms_column {
            sqlx::query("ALTER TABLE api_keys ADD COLUMN transforms TEXT NOT NULL DEFAULT '[]'")
                .execute(&pool)
                .await
                .ok();
        }

        Ok(Self { pool })
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
        let row = sqlx::query("SELECT COUNT(*) as count FROM users")
            .fetch_one(&self.pool)
            .await
            .map_err(|e| e.to_string())?;
        row.try_get::<i64, _>("count").map_err(|e| e.to_string())
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

        sqlx::query(
            r#"INSERT INTO users (id, username, password_hash, role, created_at, updated_at, enabled, balance_nano_usd, balance_unlimited)
               VALUES (?, ?, ?, ?, ?, ?, 1, '0', 0)"#,
        )
        .bind(&id)
        .bind(username)
        .bind(&password_hash)
        .bind(role.as_str())
        .bind(now.to_rfc3339())
        .bind(now.to_rfc3339())
        .execute(&self.pool)
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
        })
    }

    pub async fn get_user_by_id(&self, id: &str) -> Result<Option<User>, String> {
        let row = sqlx::query(
            "SELECT id, username, password_hash, role, created_at, updated_at, last_login_at, enabled, balance_nano_usd, balance_unlimited FROM users WHERE id = ?",
        )
        .bind(id)
        .fetch_optional(&self.pool)
        .await
        .map_err(|e| e.to_string())?;

        if let Some(row) = row {
            Ok(Some(self.row_to_user(&row)?))
        } else {
            Ok(None)
        }
    }

    pub async fn get_user_by_username(&self, username: &str) -> Result<Option<User>, String> {
        let row = sqlx::query(
            "SELECT id, username, password_hash, role, created_at, updated_at, last_login_at, enabled, balance_nano_usd, balance_unlimited FROM users WHERE username = ?",
        )
        .bind(username)
        .fetch_optional(&self.pool)
        .await
        .map_err(|e| e.to_string())?;

        if let Some(row) = row {
            Ok(Some(self.row_to_user(&row)?))
        } else {
            Ok(None)
        }
    }

    pub async fn list_users(&self) -> Result<Vec<User>, String> {
        let rows = sqlx::query(
            "SELECT id, username, password_hash, role, created_at, updated_at, last_login_at, enabled, balance_nano_usd, balance_unlimited FROM users ORDER BY created_at DESC",
        )
        .fetch_all(&self.pool)
        .await
        .map_err(|e| e.to_string())?;

        rows.iter().map(|row| self.row_to_user(row)).collect()
    }

    pub async fn update_user(
        &self,
        id: &str,
        username: Option<&str>,
        password: Option<&str>,
        role: Option<UserRole>,
        enabled: Option<bool>,
        balance_nano_usd: Option<&str>,
        balance_unlimited: Option<bool>,
    ) -> Result<(), String> {
        let mut updates = Vec::new();
        let mut bindings: Vec<String> = Vec::new();

        if let Some(u) = username {
            updates.push("username = ?");
            bindings.push(u.to_string());
        }
        if let Some(p) = password {
            updates.push("password_hash = ?");
            bindings.push(Self::hash_password(p)?);
        }
        if let Some(r) = role {
            updates.push("role = ?");
            bindings.push(r.as_str().to_string());
        }
        if let Some(e) = enabled {
            updates.push("enabled = ?");
            bindings.push(if e { "1" } else { "0" }.to_string());
        }
        if let Some(balance) = balance_nano_usd {
            parse_nano_usd(balance)?;
            updates.push("balance_nano_usd = ?");
            bindings.push(balance.to_string());
        }
        if let Some(unlimited) = balance_unlimited {
            updates.push("balance_unlimited = ?");
            bindings.push(if unlimited { "1" } else { "0" }.to_string());
        }

        if updates.is_empty() {
            return Ok(());
        }

        updates.push("updated_at = ?");
        bindings.push(Utc::now().to_rfc3339());
        bindings.push(id.to_string());

        let query = format!("UPDATE users SET {} WHERE id = ?", updates.join(", "));

        let mut q = sqlx::query(&query);
        for b in &bindings {
            q = q.bind(b);
        }

        q.execute(&self.pool).await.map_err(|e| e.to_string())?;
        Ok(())
    }

    pub async fn delete_user(&self, id: &str) -> Result<(), String> {
        sqlx::query("DELETE FROM users WHERE id = ?")
            .bind(id)
            .execute(&self.pool)
            .await
            .map_err(|e| e.to_string())?;
        Ok(())
    }

    pub async fn update_last_login(&self, id: &str) -> Result<(), String> {
        let now = Utc::now();
        sqlx::query("UPDATE users SET last_login_at = ? WHERE id = ?")
            .bind(now.to_rfc3339())
            .bind(id)
            .execute(&self.pool)
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

        sqlx::query(
            r#"INSERT INTO sessions (id, user_id, token, created_at, expires_at)
               VALUES (?, ?, ?, ?, ?)"#,
        )
        .bind(&id)
        .bind(user_id)
        .bind(&token)
        .bind(now.to_rfc3339())
        .bind(expires_at.to_rfc3339())
        .execute(&self.pool)
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
        let row = sqlx::query(
            "SELECT id, user_id, token, created_at, expires_at FROM sessions WHERE token = ?",
        )
        .bind(token)
        .fetch_optional(&self.pool)
        .await
        .map_err(|e| e.to_string())?;

        if let Some(row) = row {
            let expires_at: String = row.try_get("expires_at").map_err(|e| e.to_string())?;
            let expires_at = DateTime::parse_from_rfc3339(&expires_at)
                .map_err(|e| e.to_string())?
                .with_timezone(&Utc);

            if expires_at < Utc::now() {
                self.delete_session(token).await?;
                return Ok(None);
            }

            Ok(Some(Session {
                id: row.try_get("id").map_err(|e| e.to_string())?,
                user_id: row.try_get("user_id").map_err(|e| e.to_string())?,
                token: row.try_get("token").map_err(|e| e.to_string())?,
                created_at: DateTime::parse_from_rfc3339(
                    &row.try_get::<String, _>("created_at")
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
        sqlx::query("DELETE FROM sessions WHERE token = ?")
            .bind(token)
            .execute(&self.pool)
            .await
            .map_err(|e| e.to_string())?;
        Ok(())
    }

    pub async fn delete_user_sessions(&self, user_id: &str) -> Result<(), String> {
        sqlx::query("DELETE FROM sessions WHERE user_id = ?")
            .bind(user_id)
            .execute(&self.pool)
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

        sqlx::query(
            r#"INSERT INTO api_keys (id, user_id, name, key_prefix, key, key_hash, created_at, expires_at, enabled, quota_remaining, quota_unlimited, model_limits_enabled, model_limits, ip_whitelist, token_group, max_multiplier, transforms)
               VALUES (?, ?, ?, ?, ?, ?, ?, ?, 1, ?, ?, ?, ?, ?, ?, ?, ?)"#,
        )
        .bind(&id)
        .bind(user_id)
        .bind(&input.name)
        .bind(&key_prefix)
        .bind(&key)
        .bind(&key_hash)
        .bind(now.to_rfc3339())
        .bind(expires_at.map(|e| e.to_rfc3339()))
        .bind(input.quota)
        .bind(if input.quota_unlimited { 1 } else { 0 })
        .bind(if input.model_limits_enabled { 1 } else { 0 })
        .bind(&model_limits_json)
        .bind(&ip_whitelist_json)
        .bind(&input.group)
        .bind(input.max_multiplier)
        .bind(serde_json::to_string(&input.transforms).map_err(|e| e.to_string())?)
        .execute(&self.pool)
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
        let row = sqlx::query(
            "SELECT id, user_id, name, key_prefix, key, key_hash, created_at, expires_at, last_used_at, enabled, quota_remaining, quota_unlimited, model_limits_enabled, model_limits, ip_whitelist, token_group, max_multiplier, transforms FROM api_keys WHERE key_prefix = ?",
        )
        .bind(prefix)
        .fetch_optional(&self.pool)
        .await
        .map_err(|e| e.to_string())?;

        if let Some(row) = row {
            Ok(Some(self.row_to_api_key(&row)?))
        } else {
            Ok(None)
        }
    }

    pub async fn list_user_api_keys(&self, user_id: &str) -> Result<Vec<ApiKey>, String> {
        let rows = sqlx::query(
            "SELECT id, user_id, name, key_prefix, key, key_hash, created_at, expires_at, last_used_at, enabled, quota_remaining, quota_unlimited, model_limits_enabled, model_limits, ip_whitelist, token_group, max_multiplier, transforms FROM api_keys WHERE user_id = ? ORDER BY created_at DESC",
        )
        .bind(user_id)
        .fetch_all(&self.pool)
        .await
        .map_err(|e| e.to_string())?;

        rows.iter().map(|row| self.row_to_api_key(row)).collect()
    }

    pub async fn update_api_key_last_used(&self, id: &str) -> Result<(), String> {
        let now = Utc::now();
        sqlx::query("UPDATE api_keys SET last_used_at = ? WHERE id = ?")
            .bind(now.to_rfc3339())
            .bind(id)
            .execute(&self.pool)
            .await
            .map_err(|e| e.to_string())?;
        Ok(())
    }

    pub async fn delete_api_key(&self, id: &str) -> Result<(), String> {
        sqlx::query("DELETE FROM api_keys WHERE id = ?")
            .bind(id)
            .execute(&self.pool)
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

    fn row_to_user(&self, row: &sqlx::sqlite::SqliteRow) -> Result<User, String> {
        let role_str: String = row.try_get("role").map_err(|e| e.to_string())?;
        let role = UserRole::from_str(&role_str).ok_or_else(|| "invalid role".to_string())?;

        let last_login_at: Option<String> =
            row.try_get("last_login_at").map_err(|e| e.to_string())?;
        let last_login_at = last_login_at
            .map(|s| DateTime::parse_from_rfc3339(&s).map(|d| d.with_timezone(&Utc)))
            .transpose()
            .map_err(|e| e.to_string())?;

        Ok(User {
            id: row.try_get("id").map_err(|e| e.to_string())?,
            username: row.try_get("username").map_err(|e| e.to_string())?,
            password_hash: row.try_get("password_hash").map_err(|e| e.to_string())?,
            role,
            created_at: DateTime::parse_from_rfc3339(
                &row.try_get::<String, _>("created_at")
                    .map_err(|e| e.to_string())?,
            )
            .map_err(|e| e.to_string())?
            .with_timezone(&Utc),
            updated_at: DateTime::parse_from_rfc3339(
                &row.try_get::<String, _>("updated_at")
                    .map_err(|e| e.to_string())?,
            )
            .map_err(|e| e.to_string())?
            .with_timezone(&Utc),
            last_login_at,
            enabled: row
                .try_get::<i32, _>("enabled")
                .map_err(|e| e.to_string())?
                == 1,
            balance_nano_usd: row
                .try_get("balance_nano_usd")
                .unwrap_or_else(|_| "0".to_string()),
            balance_unlimited: row.try_get::<i32, _>("balance_unlimited").unwrap_or(0) == 1,
        })
    }

    fn row_to_api_key(&self, row: &sqlx::sqlite::SqliteRow) -> Result<ApiKey, String> {
        let expires_at: Option<String> = row.try_get("expires_at").map_err(|e| e.to_string())?;
        let expires_at = expires_at
            .map(|s| DateTime::parse_from_rfc3339(&s).map(|d| d.with_timezone(&Utc)))
            .transpose()
            .map_err(|e| e.to_string())?;

        let last_used_at: Option<String> =
            row.try_get("last_used_at").map_err(|e| e.to_string())?;
        let last_used_at = last_used_at
            .map(|s| DateTime::parse_from_rfc3339(&s).map(|d| d.with_timezone(&Utc)))
            .transpose()
            .map_err(|e| e.to_string())?;

        let quota_remaining: Option<i64> = row.try_get("quota_remaining").unwrap_or(None);
        let quota_unlimited: i32 = row.try_get("quota_unlimited").unwrap_or(1);
        let model_limits_enabled: i32 = row.try_get("model_limits_enabled").unwrap_or(0);

        let model_limits_str: String = row
            .try_get("model_limits")
            .unwrap_or_else(|_| "[]".to_string());
        let model_limits: Vec<String> = serde_json::from_str(&model_limits_str).unwrap_or_default();

        let ip_whitelist_str: String = row
            .try_get("ip_whitelist")
            .unwrap_or_else(|_| "[]".to_string());
        let ip_whitelist: Vec<String> = serde_json::from_str(&ip_whitelist_str).unwrap_or_default();

        let group: String = row
            .try_get("token_group")
            .unwrap_or_else(|_| "default".to_string());
        let max_multiplier: Option<f64> = row.try_get("max_multiplier").unwrap_or(None);
        let transforms_str: String = row
            .try_get("transforms")
            .unwrap_or_else(|_| "[]".to_string());
        let transforms: Vec<TransformRuleConfig> =
            serde_json::from_str(&transforms_str).unwrap_or_default();

        Ok(ApiKey {
            id: row.try_get("id").map_err(|e| e.to_string())?,
            user_id: row.try_get("user_id").map_err(|e| e.to_string())?,
            name: row.try_get("name").map_err(|e| e.to_string())?,
            key_prefix: row.try_get("key_prefix").map_err(|e| e.to_string())?,
            key: row.try_get("key").unwrap_or_else(|_| String::new()),
            key_hash: row.try_get("key_hash").map_err(|e| e.to_string())?,
            created_at: DateTime::parse_from_rfc3339(
                &row.try_get::<String, _>("created_at")
                    .map_err(|e| e.to_string())?,
            )
            .map_err(|e| e.to_string())?
            .with_timezone(&Utc),
            expires_at,
            last_used_at,
            enabled: row
                .try_get::<i32, _>("enabled")
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
        let mut updates = Vec::new();
        let mut bindings: Vec<String> = Vec::new();

        if let Some(name) = &input.name {
            updates.push("name = ?");
            bindings.push(name.clone());
        }
        if let Some(enabled) = input.enabled {
            updates.push("enabled = ?");
            bindings.push(if enabled { "1" } else { "0" }.to_string());
        }
        if let Some(quota) = input.quota {
            updates.push("quota_remaining = ?");
            bindings.push(quota.to_string());
        }
        if let Some(quota_unlimited) = input.quota_unlimited {
            updates.push("quota_unlimited = ?");
            bindings.push(if quota_unlimited { "1" } else { "0" }.to_string());
        }
        if let Some(model_limits_enabled) = input.model_limits_enabled {
            updates.push("model_limits_enabled = ?");
            bindings.push(if model_limits_enabled { "1" } else { "0" }.to_string());
        }
        if let Some(model_limits) = &input.model_limits {
            updates.push("model_limits = ?");
            bindings.push(serde_json::to_string(model_limits).map_err(|e| e.to_string())?);
        }
        if let Some(ip_whitelist) = &input.ip_whitelist {
            updates.push("ip_whitelist = ?");
            bindings.push(serde_json::to_string(ip_whitelist).map_err(|e| e.to_string())?);
        }
        if let Some(group) = &input.group {
            updates.push("token_group = ?");
            bindings.push(group.clone());
        }
        if let Some(max_multiplier) = input.max_multiplier {
            updates.push("max_multiplier = ?");
            bindings.push(max_multiplier.to_string());
        }
        if let Some(transforms) = &input.transforms {
            updates.push("transforms = ?");
            bindings.push(serde_json::to_string(transforms).map_err(|e| e.to_string())?);
        }
        if let Some(expires_at) = &input.expires_at {
            updates.push("expires_at = ?");
            bindings.push(expires_at.clone());
        }

        if updates.is_empty() {
            return self
                .get_api_key_by_id(key_id)
                .await?
                .ok_or_else(|| "API key not found".to_string());
        }

        bindings.push(key_id.to_string());

        let query = format!("UPDATE api_keys SET {} WHERE id = ?", updates.join(", "));

        let mut q = sqlx::query(&query);
        for b in &bindings {
            q = q.bind(b);
        }

        q.execute(&self.pool).await.map_err(|e| e.to_string())?;

        self.get_api_key_by_id(key_id)
            .await?
            .ok_or_else(|| "API key not found after update".to_string())
    }

    /// Get API key by ID
    pub async fn get_api_key_by_id(&self, id: &str) -> Result<Option<ApiKey>, String> {
        let row = sqlx::query(
            "SELECT id, user_id, name, key_prefix, key, key_hash, created_at, expires_at, last_used_at, enabled, quota_remaining, quota_unlimited, model_limits_enabled, model_limits, ip_whitelist, token_group, max_multiplier, transforms FROM api_keys WHERE id = ?",
        )
        .bind(id)
        .fetch_optional(&self.pool)
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

        let placeholders: Vec<&str> = ids.iter().map(|_| "?").collect();
        let query = format!(
            "DELETE FROM api_keys WHERE id IN ({})",
            placeholders.join(", ")
        );

        let mut q = sqlx::query(&query);
        for id in ids {
            q = q.bind(id);
        }

        let result = q.execute(&self.pool).await.map_err(|e| e.to_string())?;
        Ok(result.rows_affected() as usize)
    }

    pub async fn get_user_balance(&self, user_id: &str) -> Result<Option<UserBalance>, String> {
        let row =
            sqlx::query("SELECT id, balance_nano_usd, balance_unlimited FROM users WHERE id = ?")
                .bind(user_id)
                .fetch_optional(&self.pool)
                .await
                .map_err(|e| e.to_string())?;
        let Some(row) = row else {
            return Ok(None);
        };
        let balance_raw: String = row
            .try_get("balance_nano_usd")
            .unwrap_or_else(|_| "0".to_string());
        let balance_nano_usd = parse_nano_usd(&balance_raw)?;
        Ok(Some(UserBalance {
            user_id: row.try_get("id").map_err(|e| e.to_string())?,
            balance_nano_usd,
            balance_unlimited: row.try_get::<i32, _>("balance_unlimited").unwrap_or(0) == 1,
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
        let mut tx = self
            .pool
            .begin()
            .await
            .map_err(|e| BillingError::new(BillingErrorKind::Internal, e.to_string()))?;
        let row = sqlx::query("SELECT balance_nano_usd, balance_unlimited FROM users WHERE id = ?")
            .bind(user_id)
            .fetch_optional(&mut *tx)
            .await
            .map_err(|e| BillingError::new(BillingErrorKind::Internal, e.to_string()))?;
        let Some(row) = row else {
            return Err(BillingError::new(
                BillingErrorKind::NotFound,
                "user not found",
            ));
        };
        let unlimited = row.try_get::<i32, _>("balance_unlimited").unwrap_or(0) == 1;
        if unlimited {
            tx.commit()
                .await
                .map_err(|e| BillingError::new(BillingErrorKind::Internal, e.to_string()))?;
            return Ok(());
        }

        let balance_raw: String = row
            .try_get("balance_nano_usd")
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
        sqlx::query("UPDATE users SET balance_nano_usd = ?, updated_at = ? WHERE id = ?")
            .bind(next_balance.to_string())
            .bind(now.clone())
            .bind(user_id)
            .execute(&mut *tx)
            .await
            .map_err(|e| BillingError::new(BillingErrorKind::Internal, e.to_string()))?;

        self.insert_billing_ledger_tx(
            &mut tx,
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

        let mut tx = self.pool.begin().await.map_err(|e| e.to_string())?;
        let row = sqlx::query("SELECT balance_nano_usd, balance_unlimited FROM users WHERE id = ?")
            .bind(user_id)
            .fetch_optional(&mut *tx)
            .await
            .map_err(|e| e.to_string())?;
        let Some(row) = row else {
            return Err("user not found".to_string());
        };
        let current_balance_raw: String = row
            .try_get("balance_nano_usd")
            .unwrap_or_else(|_| "0".to_string());
        let current_balance = parse_nano_usd(&current_balance_raw)?;
        let current_unlimited = row.try_get::<i32, _>("balance_unlimited").unwrap_or(0) == 1;

        let new_balance = if let Some(balance_raw) = balance_nano_usd {
            parse_nano_usd(&balance_raw)?
        } else {
            current_balance
        };
        let new_unlimited = balance_unlimited.unwrap_or(current_unlimited);

        let now = Utc::now().to_rfc3339();
        sqlx::query(
            "UPDATE users SET balance_nano_usd = ?, balance_unlimited = ?, updated_at = ? WHERE id = ?",
        )
        .bind(new_balance.to_string())
        .bind(if new_unlimited { 1 } else { 0 })
        .bind(now.clone())
        .bind(user_id)
        .execute(&mut *tx)
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
            &mut tx,
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

    async fn insert_billing_ledger_tx(
        &self,
        tx: &mut sqlx::Transaction<'_, Sqlite>,
        user_id: &str,
        kind: &str,
        delta_nano_usd: i128,
        balance_after_nano_usd: Option<i128>,
        meta: &Value,
        created_at_rfc3339: &str,
    ) -> Result<(), BillingError> {
        let id = uuid::Uuid::new_v4().to_string();
        sqlx::query(
            r#"INSERT INTO billing_ledger (id, user_id, kind, delta_nano_usd, balance_after_nano_usd, meta_json, created_at)
               VALUES (?, ?, ?, ?, ?, ?, ?)"#,
        )
        .bind(id)
        .bind(user_id)
        .bind(kind)
        .bind(delta_nano_usd.to_string())
        .bind(balance_after_nano_usd.map(|v| v.to_string()))
        .bind(meta.to_string())
        .bind(created_at_rfc3339)
        .execute(&mut **tx)
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
    pub prompt_tokens: Option<u64>,
    pub completion_tokens: Option<u64>,
    pub cached_tokens: Option<u64>,
    pub reasoning_tokens: Option<u64>,
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
}

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
    pub prompt_tokens: Option<i64>,
    pub completion_tokens: Option<i64>,
    pub cached_tokens: Option<i64>,
    pub reasoning_tokens: Option<i64>,
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
    pub created_at: String,
    pub username: Option<String>,
    pub api_key_name: Option<String>,
    pub channel_name: Option<String>,
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

fn append_request_log_filters(
    query: &mut QueryBuilder<'_, Sqlite>,
    model: Option<&str>,
    status: Option<&str>,
    api_key_id: Option<&str>,
    username: Option<&str>,
    search: Option<&str>,
) {
    if let Some(model) = model {
        let models: Vec<&str> = model
            .split(',')
            .map(str::trim)
            .filter(|s| !s.is_empty())
            .collect();
        if models.len() == 1 {
            query.push(" AND rl.model LIKE '%' || ");
            query.push_bind(models[0].to_string());
            query.push(" || '%'");
        } else if !models.is_empty() {
            query.push(" AND (");
            for (i, m) in models.iter().enumerate() {
                if i > 0 {
                    query.push(" OR ");
                }
                query.push("rl.model LIKE '%' || ");
                query.push_bind(m.to_string());
                query.push(" || '%'");
            }
            query.push(")");
        }
    }
    if let Some(status) = status {
        query.push(" AND rl.status = ");
        query.push_bind(status.to_string());
    }
    if let Some(api_key_id) = api_key_id {
        query.push(" AND rl.api_key_id = ");
        query.push_bind(api_key_id.to_string());
    }
    if let Some(username) = username {
        query.push(" AND u.username = ");
        query.push_bind(username.to_string());
    }
    if let Some(search) = search {
        let search_like = format!("%{search}%");
        query.push(" AND (rl.model LIKE ");
        query.push_bind(search_like.clone());
        query.push(" OR rl.upstream_model LIKE ");
        query.push_bind(search_like.clone());
        query.push(" OR rl.request_id LIKE ");
        query.push_bind(search_like.clone());
        query.push(" OR rl.request_ip LIKE ");
        query.push_bind(search_like);
        query.push(")");
    }
}

impl UserStore {
    pub async fn insert_request_log(&self, log: InsertRequestLog) -> Result<(), String> {
        let id = uuid::Uuid::new_v4().to_string();
        let now = Utc::now().to_rfc3339();
        sqlx::query(
            r#"INSERT INTO request_logs
               (id, request_id, user_id, api_key_id, model, provider_id, upstream_model, channel_id, is_stream,
                prompt_tokens, completion_tokens, cached_tokens, reasoning_tokens,
                provider_multiplier, charge_nano_usd, status, usage_breakdown_json,
                billing_breakdown_json, error_code, error_message, error_http_status,
                duration_ms, ttfb_ms, request_ip, created_at)
               VALUES (?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?)"#,
        )
        .bind(&id)
        .bind(&log.request_id)
        .bind(&log.user_id)
        .bind(&log.api_key_id)
        .bind(&log.model)
        .bind(&log.provider_id)
        .bind(&log.upstream_model)
        .bind(&log.channel_id)
        .bind(if log.is_stream { 1 } else { 0 })
        .bind(log.prompt_tokens.map(|v| v as i64))
        .bind(log.completion_tokens.map(|v| v as i64))
        .bind(log.cached_tokens.map(|v| v as i64))
        .bind(log.reasoning_tokens.map(|v| v as i64))
        .bind(log.provider_multiplier)
        .bind(log.charge_nano_usd.map(|v| v.to_string()))
        .bind(&log.status)
        .bind(log.usage_breakdown_json.map(|v| v.to_string()))
        .bind(log.billing_breakdown_json.map(|v| v.to_string()))
        .bind(&log.error_code)
        .bind(&log.error_message)
        .bind(log.error_http_status.map(i64::from))
        .bind(log.duration_ms.map(|v| v as i64))
        .bind(log.ttfb_ms.map(|v| v as i64))
        .bind(&log.request_ip)
        .bind(&now)
        .execute(&self.pool)
        .await
        .map_err(|e| e.to_string())?;
        Ok(())
    }

    pub async fn list_request_logs_by_user(
        &self,
        user_id: &str,
        limit: i64,
        offset: i64,
        model: Option<&str>,
        status: Option<&str>,
        api_key_id: Option<&str>,
        search: Option<&str>,
    ) -> Result<(Vec<RequestLogRow>, i64), String> {
        let model = normalize_request_log_filter(model);
        let status = normalize_request_log_filter(status);
        let api_key_id = normalize_request_log_filter(api_key_id);
        let search = normalize_request_log_filter(search);

        let mut count_query =
            QueryBuilder::<Sqlite>::new("SELECT COUNT(*) FROM request_logs rl WHERE rl.user_id = ");
        count_query.push_bind(user_id);
        append_request_log_filters(
            &mut count_query,
            model.as_deref(),
            status.as_deref(),
            api_key_id.as_deref(),
            None,
            search.as_deref(),
        );
        let total: i64 = count_query
            .build_query_scalar()
            .fetch_one(&self.pool)
            .await
            .map_err(|e| e.to_string())?;

        let mut rows_query = QueryBuilder::<Sqlite>::new(
            r#"SELECT rl.id, rl.request_id, rl.user_id, rl.api_key_id, rl.model, rl.provider_id, rl.upstream_model,
                      rl.channel_id, rl.is_stream,
                      rl.prompt_tokens, rl.completion_tokens, rl.cached_tokens, rl.reasoning_tokens,
                      rl.provider_multiplier, rl.charge_nano_usd, rl.status,
                      rl.usage_breakdown_json, rl.billing_breakdown_json,
                      rl.error_code, rl.error_message, rl.error_http_status,
                      rl.duration_ms, rl.ttfb_ms, rl.request_ip, rl.created_at,
                      u.username, ak.name AS api_key_name, ch.name AS channel_name
               FROM request_logs rl
               LEFT JOIN users u ON rl.user_id = u.id
               LEFT JOIN api_keys ak ON rl.api_key_id = ak.id
               LEFT JOIN monoize_channels ch ON rl.channel_id = ch.id
               WHERE rl.user_id = "#,
        );
        rows_query.push_bind(user_id);
        append_request_log_filters(
            &mut rows_query,
            model.as_deref(),
            status.as_deref(),
            api_key_id.as_deref(),
            None,
            search.as_deref(),
        );
        rows_query.push(" ORDER BY rl.created_at DESC LIMIT ");
        rows_query.push_bind(limit);
        rows_query.push(" OFFSET ");
        rows_query.push_bind(offset);

        let rows = rows_query
            .build()
            .fetch_all(&self.pool)
            .await
            .map_err(|e| e.to_string())?;

        let logs = rows
            .into_iter()
            .map(|row| RequestLogRow {
                id: row.try_get("id").unwrap_or_default(),
                request_id: row.try_get("request_id").unwrap_or(None),
                user_id: row.try_get("user_id").unwrap_or_default(),
                api_key_id: row.try_get("api_key_id").unwrap_or(None),
                model: row.try_get("model").unwrap_or_default(),
                provider_id: row.try_get("provider_id").unwrap_or(None),
                upstream_model: row.try_get("upstream_model").unwrap_or(None),
                channel_id: row.try_get("channel_id").unwrap_or(None),
                is_stream: row.try_get::<i32, _>("is_stream").unwrap_or(0) == 1,
                prompt_tokens: row.try_get("prompt_tokens").unwrap_or(None),
                completion_tokens: row.try_get("completion_tokens").unwrap_or(None),
                cached_tokens: row.try_get("cached_tokens").unwrap_or(None),
                reasoning_tokens: row.try_get("reasoning_tokens").unwrap_or(None),
                provider_multiplier: row.try_get("provider_multiplier").unwrap_or(None),
                charge_nano_usd: row
                    .try_get::<Option<String>, _>("charge_nano_usd")
                    .unwrap_or(None),
                status: row
                    .try_get("status")
                    .unwrap_or_else(|_| "unknown".to_string()),
                usage_breakdown_json: parse_optional_json_text(
                    row.try_get::<Option<String>, _>("usage_breakdown_json")
                        .unwrap_or(None),
                ),
                billing_breakdown_json: parse_optional_json_text(
                    row.try_get::<Option<String>, _>("billing_breakdown_json")
                        .unwrap_or(None),
                ),
                error_code: row.try_get("error_code").unwrap_or(None),
                error_message: row.try_get("error_message").unwrap_or(None),
                error_http_status: row.try_get("error_http_status").unwrap_or(None),
                duration_ms: row.try_get("duration_ms").unwrap_or(None),
                ttfb_ms: row.try_get("ttfb_ms").unwrap_or(None),
                request_ip: row.try_get("request_ip").unwrap_or(None),
                created_at: row.try_get("created_at").unwrap_or_default(),
                username: row.try_get("username").unwrap_or(None),
                api_key_name: row.try_get("api_key_name").unwrap_or(None),
                channel_name: row.try_get("channel_name").unwrap_or(None),
            })
            .collect();

        Ok((logs, total))
    }

    pub async fn list_all_request_logs(
        &self,
        limit: i64,
        offset: i64,
        model: Option<&str>,
        status: Option<&str>,
        api_key_id: Option<&str>,
        username: Option<&str>,
        search: Option<&str>,
    ) -> Result<(Vec<RequestLogRow>, i64), String> {
        let model = normalize_request_log_filter(model);
        let status = normalize_request_log_filter(status);
        let api_key_id = normalize_request_log_filter(api_key_id);
        let username = normalize_request_log_filter(username);
        let search = normalize_request_log_filter(search);

        let mut count_query = QueryBuilder::<Sqlite>::new(
            r#"SELECT COUNT(*) FROM request_logs rl
               LEFT JOIN users u ON rl.user_id = u.id
               WHERE 1 = 1"#,
        );
        append_request_log_filters(
            &mut count_query,
            model.as_deref(),
            status.as_deref(),
            api_key_id.as_deref(),
            username.as_deref(),
            search.as_deref(),
        );
        let total: i64 = count_query
            .build_query_scalar()
            .fetch_one(&self.pool)
            .await
            .map_err(|e| e.to_string())?;

        let mut rows_query = QueryBuilder::<Sqlite>::new(
            r#"SELECT rl.id, rl.request_id, rl.user_id, rl.api_key_id, rl.model, rl.provider_id, rl.upstream_model,
                      rl.channel_id, rl.is_stream,
                      rl.prompt_tokens, rl.completion_tokens, rl.cached_tokens, rl.reasoning_tokens,
                      rl.provider_multiplier, rl.charge_nano_usd, rl.status,
                      rl.usage_breakdown_json, rl.billing_breakdown_json,
                      rl.error_code, rl.error_message, rl.error_http_status,
                      rl.duration_ms, rl.ttfb_ms, rl.request_ip, rl.created_at,
                      u.username, ak.name AS api_key_name, ch.name AS channel_name
               FROM request_logs rl
               LEFT JOIN users u ON rl.user_id = u.id
               LEFT JOIN api_keys ak ON rl.api_key_id = ak.id
               LEFT JOIN monoize_channels ch ON rl.channel_id = ch.id
               WHERE 1 = 1"#,
        );
        append_request_log_filters(
            &mut rows_query,
            model.as_deref(),
            status.as_deref(),
            api_key_id.as_deref(),
            username.as_deref(),
            search.as_deref(),
        );
        rows_query.push(" ORDER BY rl.created_at DESC LIMIT ");
        rows_query.push_bind(limit);
        rows_query.push(" OFFSET ");
        rows_query.push_bind(offset);

        let rows = rows_query
            .build()
            .fetch_all(&self.pool)
            .await
            .map_err(|e| e.to_string())?;

        let logs = rows
            .into_iter()
            .map(|row| RequestLogRow {
                id: row.try_get("id").unwrap_or_default(),
                request_id: row.try_get("request_id").unwrap_or(None),
                user_id: row.try_get("user_id").unwrap_or_default(),
                api_key_id: row.try_get("api_key_id").unwrap_or(None),
                model: row.try_get("model").unwrap_or_default(),
                provider_id: row.try_get("provider_id").unwrap_or(None),
                upstream_model: row.try_get("upstream_model").unwrap_or(None),
                channel_id: row.try_get("channel_id").unwrap_or(None),
                is_stream: row.try_get::<i32, _>("is_stream").unwrap_or(0) == 1,
                prompt_tokens: row.try_get("prompt_tokens").unwrap_or(None),
                completion_tokens: row.try_get("completion_tokens").unwrap_or(None),
                cached_tokens: row.try_get("cached_tokens").unwrap_or(None),
                reasoning_tokens: row.try_get("reasoning_tokens").unwrap_or(None),
                provider_multiplier: row.try_get("provider_multiplier").unwrap_or(None),
                charge_nano_usd: row
                    .try_get::<Option<String>, _>("charge_nano_usd")
                    .unwrap_or(None),
                status: row
                    .try_get("status")
                    .unwrap_or_else(|_| "unknown".to_string()),
                usage_breakdown_json: parse_optional_json_text(
                    row.try_get::<Option<String>, _>("usage_breakdown_json")
                        .unwrap_or(None),
                ),
                billing_breakdown_json: parse_optional_json_text(
                    row.try_get::<Option<String>, _>("billing_breakdown_json")
                        .unwrap_or(None),
                ),
                error_code: row.try_get("error_code").unwrap_or(None),
                error_message: row.try_get("error_message").unwrap_or(None),
                error_http_status: row.try_get("error_http_status").unwrap_or(None),
                duration_ms: row.try_get("duration_ms").unwrap_or(None),
                ttfb_ms: row.try_get("ttfb_ms").unwrap_or(None),
                request_ip: row.try_get("request_ip").unwrap_or(None),
                created_at: row.try_get("created_at").unwrap_or_default(),
                username: row.try_get("username").unwrap_or(None),
                api_key_name: row.try_get("api_key_name").unwrap_or(None),
                channel_name: row.try_get("channel_name").unwrap_or(None),
            })
            .collect();

        Ok((logs, total))
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
