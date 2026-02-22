use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use sqlx::{Pool, Row, Sqlite};
use std::collections::HashMap;

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SystemSettings {
    pub registration_enabled: bool,
    pub default_user_role: String,
    pub session_ttl_days: i64,
    pub api_key_max_per_user: i64,
    pub site_name: String,
    pub site_description: String,
    pub api_base_url: String,
    pub reasoning_suffix_map: HashMap<String, String>,
    pub monoize_active_probe_enabled: bool,
    pub monoize_active_probe_interval_seconds: u64,
    pub monoize_active_probe_success_threshold: u32,
    pub monoize_active_probe_model: Option<String>,
    pub monoize_passive_failure_threshold: u32,
    pub monoize_passive_cooldown_seconds: u64,
    pub monoize_passive_window_seconds: u64,
    pub monoize_passive_min_samples: u32,
    pub monoize_passive_failure_rate_threshold: f64,
    pub monoize_passive_rate_limit_cooldown_seconds: u64,
    pub monoize_request_timeout_ms: u64,
    pub updated_at: DateTime<Utc>,
}

fn default_reasoning_suffix_map() -> HashMap<String, String> {
    let mut m = HashMap::new();
    m.insert("-thinking".to_string(), "high".to_string());
    m.insert("-reasoning".to_string(), "high".to_string());
    m.insert("-nothinking".to_string(), "none".to_string());
    m
}

impl Default for SystemSettings {
    fn default() -> Self {
        Self {
            registration_enabled: true,
            default_user_role: "user".to_string(),
            session_ttl_days: 7,
            api_key_max_per_user: 1000,
            site_name: "Monoize Dashboard".to_string(),
            site_description: "Unified Responses Proxy".to_string(),
            api_base_url: String::new(),
            reasoning_suffix_map: default_reasoning_suffix_map(),
            monoize_active_probe_enabled: true,
            monoize_active_probe_interval_seconds: 30,
            monoize_active_probe_success_threshold: 1,
            monoize_active_probe_model: None,
            monoize_passive_failure_threshold: 3,
            monoize_passive_cooldown_seconds: 60,
            monoize_passive_window_seconds: 30,
            monoize_passive_min_samples: 20,
            monoize_passive_failure_rate_threshold: 0.6,
            monoize_passive_rate_limit_cooldown_seconds: 15,
            monoize_request_timeout_ms: 30000,
            updated_at: Utc::now(),
        }
    }
}

#[derive(Clone)]
pub struct SettingsStore {
    pool: Pool<Sqlite>,
}

impl SettingsStore {
    pub async fn new(pool: Pool<Sqlite>) -> Result<Self, String> {
        sqlx::query(
            r#"CREATE TABLE IF NOT EXISTS system_settings (
                key TEXT PRIMARY KEY,
                value TEXT NOT NULL,
                updated_at TEXT NOT NULL
            )"#,
        )
        .execute(&pool)
        .await
        .map_err(|e| e.to_string())?;

        let store = Self { pool };
        store.ensure_defaults().await?;
        Ok(store)
    }

    async fn ensure_defaults(&self) -> Result<(), String> {
        let defaults = SystemSettings::default();
        self.set_if_not_exists(
            "registration_enabled",
            &serde_json::to_string(&defaults.registration_enabled).unwrap(),
        )
        .await?;
        self.set_if_not_exists("default_user_role", &defaults.default_user_role)
            .await?;
        self.set_if_not_exists("session_ttl_days", &defaults.session_ttl_days.to_string())
            .await?;
        self.set_if_not_exists(
            "api_key_max_per_user",
            &defaults.api_key_max_per_user.to_string(),
        )
        .await?;
        self.set_if_not_exists("site_name", &defaults.site_name)
            .await?;
        self.set_if_not_exists("site_description", &defaults.site_description)
            .await?;
        self.set_if_not_exists("api_base_url", &defaults.api_base_url)
            .await?;
        self.set_if_not_exists(
            "reasoning_suffix_map",
            &serde_json::to_string(&defaults.reasoning_suffix_map).unwrap(),
        )
        .await?;
        self.set_if_not_exists(
            "monoize_active_probe_enabled",
            &defaults.monoize_active_probe_enabled.to_string(),
        )
        .await?;
        self.set_if_not_exists(
            "monoize_active_probe_interval_seconds",
            &defaults.monoize_active_probe_interval_seconds.to_string(),
        )
        .await?;
        self.set_if_not_exists(
            "monoize_active_probe_success_threshold",
            &defaults.monoize_active_probe_success_threshold.to_string(),
        )
        .await?;
        self.set_if_not_exists(
            "monoize_active_probe_model",
            &defaults
                .monoize_active_probe_model
                .clone()
                .unwrap_or_default(),
        )
        .await?;
        self.set_if_not_exists(
            "monoize_passive_failure_threshold",
            &defaults.monoize_passive_failure_threshold.to_string(),
        )
        .await?;
        self.set_if_not_exists(
            "monoize_passive_cooldown_seconds",
            &defaults.monoize_passive_cooldown_seconds.to_string(),
        )
        .await?;
        self.set_if_not_exists(
            "monoize_passive_window_seconds",
            &defaults.monoize_passive_window_seconds.to_string(),
        )
        .await?;
        self.set_if_not_exists(
            "monoize_passive_min_samples",
            &defaults.monoize_passive_min_samples.to_string(),
        )
        .await?;
        self.set_if_not_exists(
            "monoize_passive_failure_rate_threshold",
            &defaults.monoize_passive_failure_rate_threshold.to_string(),
        )
        .await?;
        self.set_if_not_exists(
            "monoize_passive_rate_limit_cooldown_seconds",
            &defaults
                .monoize_passive_rate_limit_cooldown_seconds
                .to_string(),
        )
        .await?;
        self.set_if_not_exists(
            "monoize_request_timeout_ms",
            &defaults.monoize_request_timeout_ms.to_string(),
        )
        .await?;
        Ok(())
    }

    async fn set_if_not_exists(&self, key: &str, value: &str) -> Result<(), String> {
        let now = Utc::now().to_rfc3339();
        sqlx::query(
            "INSERT OR IGNORE INTO system_settings (key, value, updated_at) VALUES (?, ?, ?)",
        )
        .bind(key)
        .bind(value)
        .bind(&now)
        .execute(&self.pool)
        .await
        .map_err(|e| e.to_string())?;
        Ok(())
    }

    pub async fn get(&self, key: &str) -> Result<Option<String>, String> {
        let row = sqlx::query("SELECT value FROM system_settings WHERE key = ?")
            .bind(key)
            .fetch_optional(&self.pool)
            .await
            .map_err(|e| e.to_string())?;

        Ok(row.map(|r| r.try_get("value").unwrap_or_default()))
    }

    pub async fn set(&self, key: &str, value: &str) -> Result<(), String> {
        let now = Utc::now().to_rfc3339();
        sqlx::query(
            "INSERT INTO system_settings (key, value, updated_at) VALUES (?, ?, ?)
             ON CONFLICT(key) DO UPDATE SET value = excluded.value, updated_at = excluded.updated_at",
        )
        .bind(key)
        .bind(value)
        .bind(&now)
        .execute(&self.pool)
        .await
        .map_err(|e| e.to_string())?;
        Ok(())
    }

    pub async fn get_all(&self) -> Result<SystemSettings, String> {
        let rows = sqlx::query("SELECT key, value, updated_at FROM system_settings")
            .fetch_all(&self.pool)
            .await
            .map_err(|e| e.to_string())?;

        let mut settings = SystemSettings::default();
        let mut latest_update = settings.updated_at;

        for row in rows {
            let key: String = row.try_get("key").map_err(|e| e.to_string())?;
            let value: String = row.try_get("value").map_err(|e| e.to_string())?;
            let updated_at_str: String = row.try_get("updated_at").map_err(|e| e.to_string())?;

            if let Ok(updated_at) = DateTime::parse_from_rfc3339(&updated_at_str) {
                let updated_at = updated_at.with_timezone(&Utc);
                if updated_at > latest_update {
                    latest_update = updated_at;
                }
            }

            match key.as_str() {
                "registration_enabled" => {
                    settings.registration_enabled = value.parse().unwrap_or(true);
                }
                "default_user_role" => {
                    settings.default_user_role = value;
                }
                "session_ttl_days" => {
                    settings.session_ttl_days = value.parse().unwrap_or(7);
                }
                "api_key_max_per_user" => {
                    settings.api_key_max_per_user = value.parse().unwrap_or(1000);
                }
                "site_name" => {
                    settings.site_name = value;
                }
                "site_description" => {
                    settings.site_description = value;
                }
                "api_base_url" => {
                    settings.api_base_url = value;
                }
                "reasoning_suffix_map" => {
                    if let Ok(map) = serde_json::from_str(&value) {
                        settings.reasoning_suffix_map = map;
                    }
                }
                "monoize_active_probe_enabled" => {
                    settings.monoize_active_probe_enabled = value.parse().unwrap_or(true);
                }
                "monoize_active_probe_interval_seconds" => {
                    settings.monoize_active_probe_interval_seconds = value.parse().unwrap_or(30);
                }
                "monoize_active_probe_success_threshold" => {
                    settings.monoize_active_probe_success_threshold = value.parse().unwrap_or(1);
                }
                "monoize_active_probe_model" => {
                    let trimmed = value.trim();
                    settings.monoize_active_probe_model = if trimmed.is_empty() {
                        None
                    } else {
                        Some(trimmed.to_string())
                    };
                }
                "monoize_passive_failure_threshold" => {
                    settings.monoize_passive_failure_threshold = value.parse().unwrap_or(3);
                }
                "monoize_passive_cooldown_seconds" => {
                    settings.monoize_passive_cooldown_seconds = value.parse().unwrap_or(60);
                }
                "monoize_passive_window_seconds" => {
                    settings.monoize_passive_window_seconds = value.parse().unwrap_or(30);
                }
                "monoize_passive_min_samples" => {
                    settings.monoize_passive_min_samples = value.parse().unwrap_or(20);
                }
                "monoize_passive_failure_rate_threshold" => {
                    settings.monoize_passive_failure_rate_threshold = value.parse().unwrap_or(0.6);
                }
                "monoize_passive_rate_limit_cooldown_seconds" => {
                    settings.monoize_passive_rate_limit_cooldown_seconds =
                        value.parse().unwrap_or(15);
                }
                "monoize_request_timeout_ms" => {
                    settings.monoize_request_timeout_ms = value.parse().unwrap_or(30000);
                }
                _ => {}
            }
        }

        settings.updated_at = latest_update;
        Ok(settings)
    }

    pub async fn update_all(&self, settings: &SystemSettings) -> Result<(), String> {
        self.set(
            "registration_enabled",
            &settings.registration_enabled.to_string(),
        )
        .await?;
        self.set("default_user_role", &settings.default_user_role)
            .await?;
        self.set("session_ttl_days", &settings.session_ttl_days.to_string())
            .await?;
        self.set(
            "api_key_max_per_user",
            &settings.api_key_max_per_user.to_string(),
        )
        .await?;
        self.set("site_name", &settings.site_name).await?;
        self.set("site_description", &settings.site_description)
            .await?;
        self.set("api_base_url", &settings.api_base_url).await?;
        self.set(
            "reasoning_suffix_map",
            &serde_json::to_string(&settings.reasoning_suffix_map)
                .unwrap_or_else(|_| "{}".to_string()),
        )
        .await?;
        self.set(
            "monoize_active_probe_enabled",
            &settings.monoize_active_probe_enabled.to_string(),
        )
        .await?;
        self.set(
            "monoize_active_probe_interval_seconds",
            &settings.monoize_active_probe_interval_seconds.to_string(),
        )
        .await?;
        self.set(
            "monoize_active_probe_success_threshold",
            &settings.monoize_active_probe_success_threshold.to_string(),
        )
        .await?;
        self.set(
            "monoize_active_probe_model",
            settings
                .monoize_active_probe_model
                .as_ref()
                .map(|s| s.as_str())
                .unwrap_or(""),
        )
        .await?;
        self.set(
            "monoize_passive_failure_threshold",
            &settings.monoize_passive_failure_threshold.to_string(),
        )
        .await?;
        self.set(
            "monoize_passive_cooldown_seconds",
            &settings.monoize_passive_cooldown_seconds.to_string(),
        )
        .await?;
        self.set(
            "monoize_passive_window_seconds",
            &settings.monoize_passive_window_seconds.to_string(),
        )
        .await?;
        self.set(
            "monoize_passive_min_samples",
            &settings.monoize_passive_min_samples.to_string(),
        )
        .await?;
        self.set(
            "monoize_passive_failure_rate_threshold",
            &settings.monoize_passive_failure_rate_threshold.to_string(),
        )
        .await?;
        self.set(
            "monoize_passive_rate_limit_cooldown_seconds",
            &settings
                .monoize_passive_rate_limit_cooldown_seconds
                .to_string(),
        )
        .await?;
        self.set(
            "monoize_request_timeout_ms",
            &settings.monoize_request_timeout_ms.to_string(),
        )
        .await?;
        Ok(())
    }

    pub async fn is_registration_enabled(&self) -> Result<bool, String> {
        self.get("registration_enabled")
            .await
            .map(|v| v.map(|s| s.parse().unwrap_or(true)).unwrap_or(true))
    }

    pub async fn get_reasoning_suffix_map(&self) -> Result<HashMap<String, String>, String> {
        match self.get("reasoning_suffix_map").await? {
            Some(json_str) => serde_json::from_str(&json_str)
                .map_err(|e| format!("invalid reasoning_suffix_map JSON: {e}")),
            None => Ok(default_reasoning_suffix_map()),
        }
    }
}
