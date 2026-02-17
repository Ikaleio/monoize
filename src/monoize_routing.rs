use crate::transforms::TransformRuleConfig;
use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use sqlx::{Pool, Row, Sqlite};
use std::collections::{HashMap, HashSet};
use std::time::Duration;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum MonoizeProviderType {
    Responses,
    ChatCompletion,
    Messages,
    Gemini,
    Grok,
}

impl MonoizeProviderType {
    pub fn from_str(s: &str) -> Option<Self> {
        match s {
            "responses" => Some(Self::Responses),
            "chat_completion" => Some(Self::ChatCompletion),
            "messages" => Some(Self::Messages),
            "gemini" => Some(Self::Gemini),
            "grok" => Some(Self::Grok),
            _ => None,
        }
    }

    pub fn as_str(&self) -> &'static str {
        match self {
            Self::Responses => "responses",
            Self::ChatCompletion => "chat_completion",
            Self::Messages => "messages",
            Self::Gemini => "gemini",
            Self::Grok => "grok",
        }
    }

    pub fn to_config_type(&self) -> crate::config::ProviderType {
        match self {
            Self::Responses => crate::config::ProviderType::Responses,
            Self::ChatCompletion => crate::config::ProviderType::ChatCompletion,
            Self::Messages => crate::config::ProviderType::Messages,
            Self::Gemini => crate::config::ProviderType::Gemini,
            Self::Grok => crate::config::ProviderType::Grok,
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct MonoizeModelEntry {
    pub redirect: Option<String>,
    pub multiplier: f64,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct MonoizeChannel {
    pub id: String,
    pub name: String,
    pub base_url: String,
    #[serde(skip_serializing)]
    pub api_key: String,
    #[serde(default = "default_channel_weight")]
    pub weight: i32,
    #[serde(default = "default_enabled")]
    pub enabled: bool,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub _healthy: Option<bool>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub _failure_count: Option<u32>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub _last_success_at: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub _health_status: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct MonoizeProvider {
    pub id: String,
    pub name: String,
    pub provider_type: MonoizeProviderType,
    pub models: HashMap<String, MonoizeModelEntry>,
    pub channels: Vec<MonoizeChannel>,
    pub max_retries: i32,
    #[serde(default)]
    pub transforms: Vec<TransformRuleConfig>,
    pub enabled: bool,
    pub priority: i32,
    pub created_at: DateTime<Utc>,
    pub updated_at: DateTime<Utc>,
}

#[derive(Debug, Clone, Deserialize)]
pub struct CreateMonoizeChannelInput {
    pub id: Option<String>,
    pub name: String,
    pub base_url: String,
    #[serde(default)]
    pub api_key: Option<String>,
    #[serde(default = "default_channel_weight")]
    pub weight: i32,
    #[serde(default = "default_enabled")]
    pub enabled: bool,
}

#[derive(Debug, Clone, Deserialize)]
pub struct CreateMonoizeProviderInput {
    pub name: String,
    pub provider_type: MonoizeProviderType,
    pub models: HashMap<String, MonoizeModelEntry>,
    pub channels: Vec<CreateMonoizeChannelInput>,
    #[serde(default = "default_max_retries")]
    pub max_retries: i32,
    #[serde(default)]
    pub transforms: Vec<TransformRuleConfig>,
    #[serde(default = "default_enabled")]
    pub enabled: bool,
    pub priority: Option<i32>,
}

#[derive(Debug, Clone, Deserialize)]
pub struct UpdateMonoizeProviderInput {
    pub name: Option<String>,
    pub provider_type: Option<MonoizeProviderType>,
    pub models: Option<HashMap<String, MonoizeModelEntry>>,
    pub channels: Option<Vec<CreateMonoizeChannelInput>>,
    pub max_retries: Option<i32>,
    pub transforms: Option<Vec<TransformRuleConfig>>,
    pub enabled: Option<bool>,
    pub priority: Option<i32>,
}

#[derive(Debug, Clone, Deserialize)]
pub struct ReorderProvidersInput {
    pub provider_ids: Vec<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct MonoizeRuntimeConfig {
    pub request_timeout_ms: u64,
    pub passive_failure_threshold: u32,
    pub passive_cooldown_seconds: u64,
    pub active_enabled: bool,
    pub active_interval_seconds: u64,
    pub active_success_threshold: u32,
    pub active_method: String,
}

impl Default for MonoizeRuntimeConfig {
    fn default() -> Self {
        Self {
            request_timeout_ms: 30_000,
            passive_failure_threshold: 3,
            passive_cooldown_seconds: 60,
            active_enabled: true,
            active_interval_seconds: 30,
            active_success_threshold: 1,
            active_method: "list_models".to_string(),
        }
    }
}

#[derive(Debug, Clone, Default)]
pub struct ChannelHealthState {
    pub healthy: bool,
    pub failure_count: u32,
    pub last_success_at: Option<i64>,
    pub cooldown_until: Option<i64>,
    pub probe_success_count: u32,
}

impl ChannelHealthState {
    pub fn new() -> Self {
        Self {
            healthy: true,
            failure_count: 0,
            last_success_at: None,
            cooldown_until: None,
            probe_success_count: 0,
        }
    }

    pub fn status(&self, now_ts: i64) -> &'static str {
        if self.healthy {
            return "healthy";
        }
        if let Some(until) = self.cooldown_until {
            if now_ts < until {
                return "unhealthy";
            }
        }
        "probing"
    }
}

#[derive(Clone)]
pub struct MonoizeRoutingStore {
    pool: Pool<Sqlite>,
}

fn default_enabled() -> bool {
    true
}

fn default_max_retries() -> i32 {
    -1
}

fn default_channel_weight() -> i32 {
    1
}

fn generate_short_id() -> String {
    const CHARSET: &[u8] = b"abcdefghijklmnopqrstuvwxyz0123456789";
    let bytes = uuid::Uuid::new_v4().into_bytes();
    (0..8)
        .map(|i| CHARSET[bytes[i] as usize % CHARSET.len()] as char)
        .collect()
}

impl MonoizeRoutingStore {
    pub async fn new(pool: Pool<Sqlite>) -> Result<Self, String> {
        sqlx::query(
            r#"CREATE TABLE IF NOT EXISTS monoize_providers (
                id TEXT PRIMARY KEY,
                name TEXT NOT NULL,
                provider_type TEXT NOT NULL CHECK (provider_type IN ('responses', 'chat_completion', 'messages', 'gemini', 'grok')),
                max_retries INTEGER NOT NULL DEFAULT -1,
                transforms TEXT NOT NULL DEFAULT '[]',
                enabled INTEGER NOT NULL DEFAULT 1,
                priority INTEGER NOT NULL DEFAULT 0,
                created_at TEXT NOT NULL,
                updated_at TEXT NOT NULL
            )"#,
        )
        .execute(&pool)
        .await
        .map_err(|e| e.to_string())?;

        sqlx::query(
            r#"CREATE TABLE IF NOT EXISTS monoize_provider_models (
                id TEXT PRIMARY KEY,
                provider_id TEXT NOT NULL REFERENCES monoize_providers(id) ON DELETE CASCADE,
                model_name TEXT NOT NULL,
                redirect TEXT,
                multiplier REAL NOT NULL,
                created_at TEXT NOT NULL,
                UNIQUE (provider_id, model_name)
            )"#,
        )
        .execute(&pool)
        .await
        .map_err(|e| e.to_string())?;

        sqlx::query(
            r#"CREATE TABLE IF NOT EXISTS monoize_channels (
                id TEXT PRIMARY KEY,
                provider_id TEXT NOT NULL REFERENCES monoize_providers(id) ON DELETE CASCADE,
                name TEXT NOT NULL,
                base_url TEXT NOT NULL,
                api_key TEXT NOT NULL,
                weight INTEGER NOT NULL DEFAULT 1,
                enabled INTEGER NOT NULL DEFAULT 1,
                created_at TEXT NOT NULL,
                updated_at TEXT NOT NULL
            )"#,
        )
        .execute(&pool)
        .await
        .map_err(|e| e.to_string())?;

        sqlx::query("CREATE INDEX IF NOT EXISTS idx_monoize_providers_priority ON monoize_providers(priority)")
            .execute(&pool)
            .await
            .map_err(|e| e.to_string())?;

        sqlx::query("CREATE INDEX IF NOT EXISTS idx_monoize_models_provider ON monoize_provider_models(provider_id)")
            .execute(&pool)
            .await
            .map_err(|e| e.to_string())?;

        sqlx::query("CREATE INDEX IF NOT EXISTS idx_monoize_channels_provider ON monoize_channels(provider_id)")
            .execute(&pool)
            .await
            .map_err(|e| e.to_string())?;

        let has_transforms_column: bool = sqlx::query_scalar::<_, i32>(
            "SELECT COUNT(*) FROM pragma_table_info('monoize_providers') WHERE name = 'transforms'",
        )
        .fetch_one(&pool)
        .await
        .map(|c| c > 0)
        .unwrap_or(false);
        if !has_transforms_column {
            sqlx::query(
                "ALTER TABLE monoize_providers ADD COLUMN transforms TEXT NOT NULL DEFAULT '[]'",
            )
            .execute(&pool)
            .await
            .ok();
        }

        Ok(Self { pool })
    }

    pub async fn provider_count(&self) -> Result<i64, String> {
        sqlx::query_scalar("SELECT COUNT(*) FROM monoize_providers")
            .fetch_one(&self.pool)
            .await
            .map_err(|e| e.to_string())
    }

    pub async fn list_providers(&self) -> Result<Vec<MonoizeProvider>, String> {
        let rows = sqlx::query(
            r#"SELECT id, name, provider_type, max_retries, transforms, enabled, priority, created_at, updated_at
               FROM monoize_providers
               ORDER BY priority ASC, created_at ASC"#,
        )
        .fetch_all(&self.pool)
        .await
        .map_err(|e| e.to_string())?;

        let mut providers = Vec::new();
        for row in rows {
            providers.push(self.row_to_provider(&row).await?);
        }
        Ok(providers)
    }

    pub async fn get_provider(&self, id: &str) -> Result<Option<MonoizeProvider>, String> {
        let row = sqlx::query(
            r#"SELECT id, name, provider_type, max_retries, transforms, enabled, priority, created_at, updated_at
               FROM monoize_providers
               WHERE id = ?"#,
        )
        .bind(id)
        .fetch_optional(&self.pool)
        .await
        .map_err(|e| e.to_string())?;

        if let Some(row) = row {
            Ok(Some(self.row_to_provider(&row).await?))
        } else {
            Ok(None)
        }
    }

    pub async fn create_provider(
        &self,
        input: CreateMonoizeProviderInput,
    ) -> Result<MonoizeProvider, String> {
        validate_provider_input(&input.name, &input.models, &input.channels)?;

        let id = generate_short_id();
        let now = Utc::now();

        let priority = match input.priority {
            Some(v) => v,
            None => {
                let max_priority: Option<i64> =
                    sqlx::query_scalar("SELECT MAX(priority) FROM monoize_providers")
                        .fetch_one(&self.pool)
                        .await
                        .map_err(|e| e.to_string())?;
                max_priority.unwrap_or(-1) as i32 + 1
            }
        };

        sqlx::query(
            r#"INSERT INTO monoize_providers (id, name, provider_type, max_retries, transforms, enabled, priority, created_at, updated_at)
               VALUES (?, ?, ?, ?, ?, ?, ?, ?, ?)"#,
        )
        .bind(&id)
        .bind(&input.name)
        .bind(input.provider_type.as_str())
        .bind(input.max_retries)
        .bind(serde_json::to_string(&input.transforms).map_err(|e| e.to_string())?)
        .bind(input.enabled)
        .bind(priority)
        .bind(now.to_rfc3339())
        .bind(now.to_rfc3339())
        .execute(&self.pool)
        .await
        .map_err(|e| e.to_string())?;

        self.replace_models(&id, &input.models).await?;
        self.replace_channels(&id, &input.channels).await?;

        self.get_provider(&id)
            .await?
            .ok_or_else(|| "provider not found after create".to_string())
    }

    pub async fn update_provider(
        &self,
        id: &str,
        input: UpdateMonoizeProviderInput,
    ) -> Result<MonoizeProvider, String> {
        let existing = self
            .get_provider(id)
            .await?
            .ok_or_else(|| "provider not found".to_string())?;

        if let Some(models) = &input.models {
            validate_models(models)?;
        }
        if let Some(channels) = &input.channels {
            validate_channels(channels, false)?;
        }

        let name = input.name.unwrap_or(existing.name.clone());
        let provider_type = input.provider_type.unwrap_or(existing.provider_type);
        let max_retries = input.max_retries.unwrap_or(existing.max_retries);
        let transforms = input.transforms.unwrap_or(existing.transforms.clone());
        let enabled = input.enabled.unwrap_or(existing.enabled);
        let priority = input.priority.unwrap_or(existing.priority);

        let now = Utc::now();

        sqlx::query(
            r#"UPDATE monoize_providers
               SET name = ?, provider_type = ?, max_retries = ?, transforms = ?, enabled = ?, priority = ?, updated_at = ?
               WHERE id = ?"#,
        )
        .bind(&name)
        .bind(provider_type.as_str())
        .bind(max_retries)
        .bind(serde_json::to_string(&transforms).map_err(|e| e.to_string())?)
        .bind(enabled)
        .bind(priority)
        .bind(now.to_rfc3339())
        .bind(id)
        .execute(&self.pool)
        .await
        .map_err(|e| e.to_string())?;

        if let Some(models) = &input.models {
            self.replace_models(id, models).await?;
        }
        if let Some(channels) = &input.channels {
            self.replace_channels(id, channels).await?;
        }

        self.get_provider(id)
            .await?
            .ok_or_else(|| "provider not found after update".to_string())
    }

    pub async fn delete_provider(&self, id: &str) -> Result<(), String> {
        let deleted = sqlx::query("DELETE FROM monoize_providers WHERE id = ?")
            .bind(id)
            .execute(&self.pool)
            .await
            .map_err(|e| e.to_string())?
            .rows_affected();

        if deleted == 0 {
            return Err("provider not found".to_string());
        }

        Ok(())
    }

    pub async fn reorder_providers(&self, input: ReorderProvidersInput) -> Result<(), String> {
        if input.provider_ids.is_empty() {
            return Err("provider_ids must not be empty".to_string());
        }

        let mut uniq = HashSet::new();
        for id in &input.provider_ids {
            if !uniq.insert(id.clone()) {
                return Err("provider_ids contains duplicates".to_string());
            }
        }

        let existing = self.list_providers().await?;
        if existing.len() != input.provider_ids.len() {
            return Err("provider_ids must contain all providers exactly once".to_string());
        }

        let existing_ids: HashSet<String> = existing.into_iter().map(|p| p.id).collect();
        let input_ids: HashSet<String> = input.provider_ids.iter().cloned().collect();
        if existing_ids != input_ids {
            return Err("provider_ids must contain all providers exactly once".to_string());
        }

        for (i, id) in input.provider_ids.iter().enumerate() {
            sqlx::query("UPDATE monoize_providers SET priority = ?, updated_at = ? WHERE id = ?")
                .bind(i as i32)
                .bind(Utc::now().to_rfc3339())
                .bind(id)
                .execute(&self.pool)
                .await
                .map_err(|e| e.to_string())?;
        }

        Ok(())
    }

    async fn replace_models(
        &self,
        provider_id: &str,
        models: &HashMap<String, MonoizeModelEntry>,
    ) -> Result<(), String> {
        sqlx::query("DELETE FROM monoize_provider_models WHERE provider_id = ?")
            .bind(provider_id)
            .execute(&self.pool)
            .await
            .map_err(|e| e.to_string())?;

        for (model_name, entry) in models {
            sqlx::query(
                r#"INSERT INTO monoize_provider_models
                   (id, provider_id, model_name, redirect, multiplier, created_at)
                   VALUES (?, ?, ?, ?, ?, ?)"#,
            )
            .bind(format!("mono_model_{}", uuid::Uuid::new_v4().simple()))
            .bind(provider_id)
            .bind(model_name)
            .bind(entry.redirect.clone())
            .bind(entry.multiplier)
            .bind(Utc::now().to_rfc3339())
            .execute(&self.pool)
            .await
            .map_err(|e| e.to_string())?;
        }
        Ok(())
    }

    async fn replace_channels(
        &self,
        provider_id: &str,
        channels: &[CreateMonoizeChannelInput],
    ) -> Result<(), String> {
        // Fetch existing api_keys keyed by channel id; preserved when input omits api_key.
        let existing_rows =
            sqlx::query("SELECT id, api_key FROM monoize_channels WHERE provider_id = ?")
                .bind(provider_id)
                .fetch_all(&self.pool)
                .await
                .map_err(|e| e.to_string())?;
        let mut existing_keys: HashMap<String, String> = HashMap::new();
        for row in &existing_rows {
            let id: String = row.try_get("id").map_err(|e| e.to_string())?;
            let key: String = row.try_get("api_key").map_err(|e| e.to_string())?;
            existing_keys.insert(id, key);
        }

        sqlx::query("DELETE FROM monoize_channels WHERE provider_id = ?")
            .bind(provider_id)
            .execute(&self.pool)
            .await
            .map_err(|e| e.to_string())?;

        for input in channels {
            let id = input
                .id
                .clone()
                .unwrap_or_else(|| format!("mono_ch_{}", uuid::Uuid::new_v4().simple()));

            let api_key = match input.api_key.as_deref() {
                Some(k) if !k.trim().is_empty() => k.to_string(),
                _ => existing_keys.get(&id).cloned().ok_or_else(|| {
                    format!(
                        "channel api_key must not be empty for new channel '{}'",
                        input.name
                    )
                })?,
            };

            sqlx::query(
                r#"INSERT INTO monoize_channels
                   (id, provider_id, name, base_url, api_key, weight, enabled, created_at, updated_at)
                   VALUES (?, ?, ?, ?, ?, ?, ?, ?, ?)"#,
            )
            .bind(&id)
            .bind(provider_id)
            .bind(&input.name)
            .bind(&input.base_url)
            .bind(&api_key)
            .bind(input.weight)
            .bind(input.enabled)
            .bind(Utc::now().to_rfc3339())
            .bind(Utc::now().to_rfc3339())
            .execute(&self.pool)
            .await
            .map_err(|e| e.to_string())?;
        }
        Ok(())
    }

    async fn row_to_provider(
        &self,
        row: &sqlx::sqlite::SqliteRow,
    ) -> Result<MonoizeProvider, String> {
        let id: String = row.try_get("id").map_err(|e| e.to_string())?;
        let provider_type_raw: String = row.try_get("provider_type").map_err(|e| e.to_string())?;
        let provider_type = MonoizeProviderType::from_str(&provider_type_raw)
            .ok_or_else(|| format!("invalid provider type: {provider_type_raw}"))?;

        let model_rows = sqlx::query(
            r#"SELECT model_name, redirect, multiplier
               FROM monoize_provider_models
               WHERE provider_id = ?"#,
        )
        .bind(&id)
        .fetch_all(&self.pool)
        .await
        .map_err(|e| e.to_string())?;

        let mut models = HashMap::new();
        for mr in model_rows {
            let model_name: String = mr.try_get("model_name").map_err(|e| e.to_string())?;
            let redirect: Option<String> = mr.try_get("redirect").map_err(|e| e.to_string())?;
            let multiplier: f64 = mr.try_get("multiplier").map_err(|e| e.to_string())?;
            models.insert(
                model_name,
                MonoizeModelEntry {
                    redirect,
                    multiplier,
                },
            );
        }

        let channel_rows = sqlx::query(
            r#"SELECT id, name, base_url, api_key, weight, enabled
               FROM monoize_channels
               WHERE provider_id = ?
               ORDER BY created_at ASC"#,
        )
        .bind(&id)
        .fetch_all(&self.pool)
        .await
        .map_err(|e| e.to_string())?;

        let mut channels = Vec::new();
        for cr in channel_rows {
            channels.push(MonoizeChannel {
                id: cr.try_get("id").map_err(|e| e.to_string())?,
                name: cr.try_get("name").map_err(|e| e.to_string())?,
                base_url: cr.try_get("base_url").map_err(|e| e.to_string())?,
                api_key: cr.try_get("api_key").map_err(|e| e.to_string())?,
                weight: cr.try_get("weight").map_err(|e| e.to_string())?,
                enabled: cr.try_get("enabled").map_err(|e| e.to_string())?,
                _healthy: None,
                _failure_count: None,
                _last_success_at: None,
                _health_status: None,
            });
        }

        Ok(MonoizeProvider {
            id,
            name: row.try_get("name").map_err(|e| e.to_string())?,
            provider_type,
            models,
            channels,
            max_retries: row.try_get("max_retries").map_err(|e| e.to_string())?,
            transforms: serde_json::from_str(
                &row.try_get::<String, _>("transforms")
                    .unwrap_or_else(|_| "[]".to_string()),
            )
            .unwrap_or_default(),
            enabled: row.try_get("enabled").map_err(|e| e.to_string())?,
            priority: row.try_get("priority").map_err(|e| e.to_string())?,
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
        })
    }
}

fn validate_models(models: &HashMap<String, MonoizeModelEntry>) -> Result<(), String> {
    if models.is_empty() {
        return Err("models must not be empty".to_string());
    }
    for (model, entry) in models {
        if model.trim().is_empty() {
            return Err("model key must not be empty".to_string());
        }
        if !(entry.multiplier.is_finite() && entry.multiplier >= 0.0) {
            return Err("model multiplier must be >= 0".to_string());
        }
    }
    Ok(())
}

fn validate_channels(
    channels: &[CreateMonoizeChannelInput],
    require_api_key: bool,
) -> Result<(), String> {
    if channels.is_empty() {
        return Err("channels must not be empty".to_string());
    }
    let mut ids = HashSet::new();
    for c in channels {
        if c.name.trim().is_empty() {
            return Err("channel name must not be empty".to_string());
        }
        if c.base_url.trim().is_empty() {
            return Err("channel base_url must not be empty".to_string());
        }
        if require_api_key {
            let key = c.api_key.as_deref().unwrap_or("");
            if key.trim().is_empty() {
                return Err("channel api_key must not be empty".to_string());
            }
        }
        if c.weight < 0 {
            return Err("channel weight must be >= 0".to_string());
        }
        if let Some(id) = &c.id {
            if !ids.insert(id.clone()) {
                return Err("duplicate channel id".to_string());
            }
        }
    }
    Ok(())
}

fn validate_provider_input(
    name: &str,
    models: &HashMap<String, MonoizeModelEntry>,
    channels: &[CreateMonoizeChannelInput],
) -> Result<(), String> {
    if name.trim().is_empty() {
        return Err("provider name must not be empty".to_string());
    }
    validate_models(models)?;
    validate_channels(channels, true)?;
    Ok(())
}

pub async fn probe_channel_list_models(
    client: &reqwest::Client,
    channel: &MonoizeChannel,
    timeout_ms: u64,
) -> bool {
    let base = channel.base_url.trim_end_matches('/');
    let url = format!("{base}/v1/models");

    let result = client
        .get(url)
        .timeout(Duration::from_millis(timeout_ms))
        .bearer_auth(&channel.api_key)
        .send()
        .await;

    match result {
        Ok(resp) => resp.status().is_success(),
        Err(_) => false,
    }
}
