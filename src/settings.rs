use crate::db::DbPool;
use crate::entity::system_settings;
use crate::transforms::{TransformRuleConfig, canonicalize_transform_rules};
use chrono::{DateTime, Utc};
use sea_orm::{EntityTrait, Set, sea_query::OnConflict};
use serde::{Deserialize, Serialize};
use std::collections::HashMap;

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct PricingProfilePattern {
    pub pattern: String,
    pub pricing_profile: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SystemSettings {
    pub registration_enabled: bool,
    pub default_user_role: String,
    pub session_ttl_days: i64,
    pub api_key_max_per_user: i64,
    pub site_name: String,
    pub site_description: String,
    pub api_base_url: String,
    #[serde(default)]
    pub global_transforms: Vec<TransformRuleConfig>,
    pub reasoning_suffix_map: HashMap<String, String>,
    #[serde(default = "default_pricing_profile_model_patterns")]
    pub pricing_profile_model_patterns: Vec<PricingProfilePattern>,
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
    pub monoize_stream_idle_timeout_ms: u64,
    pub monoize_enable_estimated_billing: bool,
    #[serde(default)]
    pub monoize_extra_fields_whitelist: HashMap<String, Vec<String>>,
    #[serde(default = "default_true")]
    pub monoize_strip_cross_protocol_nested_extra: bool,
    pub monoize_request_capture_enabled: bool,
    pub monoize_request_capture_retention_days: u64,
    pub updated_at: DateTime<Utc>,
}

pub const BUILTIN_REASONING_EFFORT_SUFFIXES: &[(&str, &str)] = &[
    ("-none", "none"),
    ("-minimum", "minimum"),
    ("-low", "low"),
    ("-medium", "medium"),
    ("-high", "high"),
    ("-xhigh", "xhigh"),
    ("-max", "max"),
];

pub fn normalize_pricing_model_key(
    model_id: &str,
    reasoning_suffix_map: &HashMap<String, String>,
) -> String {
    let trimmed = model_id.trim();
    if trimmed.is_empty() {
        return String::new();
    }

    let mut suffixes: Vec<&str> = reasoning_suffix_map.keys().map(String::as_str).collect();
    suffixes.extend(
        BUILTIN_REASONING_EFFORT_SUFFIXES
            .iter()
            .map(|(suffix, _)| *suffix),
    );
    suffixes.sort_by(|a, b| b.len().cmp(&a.len()).then_with(|| a.cmp(b)));
    suffixes.dedup();

    for suffix in suffixes {
        if let Some(base) = trimmed.strip_suffix(suffix) {
            if !base.is_empty() {
                return base.to_string();
            }
        }
    }

    trimmed.to_string()
}

fn default_true() -> bool {
    true
}

fn default_reasoning_suffix_map() -> HashMap<String, String> {
    let mut m = HashMap::new();
    m.insert("-thinking".to_string(), "high".to_string());
    m.insert("-reasoning".to_string(), "high".to_string());
    m.insert("-nothinking".to_string(), "none".to_string());
    m
}

pub fn default_pricing_profile_model_patterns() -> Vec<PricingProfilePattern> {
    vec![
        PricingProfilePattern {
            pattern: "gpt-image-*".to_string(),
            pricing_profile: "openai".to_string(),
        },
        PricingProfilePattern {
            pattern: "text-embedding-*".to_string(),
            pricing_profile: "openai".to_string(),
        },
        PricingProfilePattern {
            pattern: "gpt-*".to_string(),
            pricing_profile: "openai".to_string(),
        },
        PricingProfilePattern {
            pattern: "o*".to_string(),
            pricing_profile: "openai".to_string(),
        },
        PricingProfilePattern {
            pattern: "claude-*".to_string(),
            pricing_profile: "anthropic".to_string(),
        },
        PricingProfilePattern {
            pattern: "gemini-*".to_string(),
            pricing_profile: "google".to_string(),
        },
        PricingProfilePattern {
            pattern: "grok-*".to_string(),
            pricing_profile: "xai".to_string(),
        },
        PricingProfilePattern {
            pattern: "*".to_string(),
            pricing_profile: "default".to_string(),
        },
    ]
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
            global_transforms: Vec::new(),
            reasoning_suffix_map: default_reasoning_suffix_map(),
            pricing_profile_model_patterns: default_pricing_profile_model_patterns(),
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
            monoize_stream_idle_timeout_ms: 120000,
            monoize_enable_estimated_billing: true,
            monoize_extra_fields_whitelist: HashMap::new(),
            monoize_strip_cross_protocol_nested_extra: true,
            monoize_request_capture_enabled: false,
            monoize_request_capture_retention_days: 1,
            updated_at: Utc::now(),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::default_pricing_profile_model_patterns;
    use crate::billing_rate_store::select_pricing_profile;

    #[test]
    fn default_pricing_profile_patterns_cover_embeddings_and_gemini() {
        let patterns = default_pricing_profile_model_patterns();

        assert_eq!(
            select_pricing_profile(&patterns, "text-embedding-3-small"),
            Some("openai")
        );
        assert_eq!(
            select_pricing_profile(&patterns, "gemini-2.5-pro"),
            Some("google")
        );
    }
}

#[derive(Clone)]
pub struct SettingsStore {
    db: DbPool,
}

impl SettingsStore {
    pub async fn new(db: DbPool) -> Result<Self, String> {
        let store = Self { db };
        store.ensure_defaults().await?;
        store.migrate_transform_rule_ids().await?;
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
            "global_transforms",
            &serde_json::to_string(&defaults.global_transforms)
                .unwrap_or_else(|_| "[]".to_string()),
        )
        .await?;
        self.set_if_not_exists(
            "reasoning_suffix_map",
            &serde_json::to_string(&defaults.reasoning_suffix_map).unwrap(),
        )
        .await?;
        self.set_if_not_exists(
            "pricing_profile_model_patterns",
            &serde_json::to_string(&defaults.pricing_profile_model_patterns)
                .unwrap_or_else(|_| "[]".to_string()),
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
        self.set_if_not_exists(
            "monoize_stream_idle_timeout_ms",
            &defaults.monoize_stream_idle_timeout_ms.to_string(),
        )
        .await?;
        self.set_if_not_exists(
            "monoize_enable_estimated_billing",
            &defaults.monoize_enable_estimated_billing.to_string(),
        )
        .await?;
        self.set_if_not_exists(
            "monoize_extra_fields_whitelist",
            &serde_json::to_string(&defaults.monoize_extra_fields_whitelist)
                .unwrap_or_else(|_| "{}".to_string()),
        )
        .await?;
        self.set_if_not_exists(
            "monoize_strip_cross_protocol_nested_extra",
            &defaults
                .monoize_strip_cross_protocol_nested_extra
                .to_string(),
        )
        .await?;
        self.set_if_not_exists(
            "monoize_request_capture_enabled",
            &defaults.monoize_request_capture_enabled.to_string(),
        )
        .await?;
        self.set_if_not_exists(
            "monoize_request_capture_retention_days",
            &defaults.monoize_request_capture_retention_days.to_string(),
        )
        .await?;
        Ok(())
    }

    async fn set_if_not_exists(&self, key: &str, value: &str) -> Result<(), String> {
        let now = Utc::now().to_rfc3339();
        // INSERT ... ON CONFLICT DO NOTHING — works cross-DB via sea-query
        let insert = system_settings::Entity::insert(system_settings::ActiveModel {
            key: Set(key.to_string()),
            value: Set(value.to_string()),
            updated_at: Set(now),
        })
        .on_conflict(
            OnConflict::column(system_settings::Column::Key)
                .do_nothing()
                .to_owned(),
        )
        .do_nothing();

        let _write_guard = self.db.write().await;
        insert
            .exec(&*_write_guard)
            .await
            .map_err(|e| e.to_string())?;
        Ok(())
    }

    pub async fn get(&self, key: &str) -> Result<Option<String>, String> {
        let row = system_settings::Entity::find_by_id(key.to_string())
            .one(self.db.read())
            .await
            .map_err(|e| e.to_string())?;

        Ok(row.map(|r| r.value))
    }

    pub async fn set(&self, key: &str, value: &str) -> Result<(), String> {
        let now = Utc::now().to_rfc3339();

        let model = system_settings::ActiveModel {
            key: Set(key.to_string()),
            value: Set(value.to_string()),
            updated_at: Set(now),
        };

        let insert = system_settings::Entity::insert(model).on_conflict(
            OnConflict::column(system_settings::Column::Key)
                .update_columns([
                    system_settings::Column::Value,
                    system_settings::Column::UpdatedAt,
                ])
                .to_owned(),
        );

        let _write_guard = self.db.write().await;
        insert
            .exec(&*_write_guard)
            .await
            .map_err(|e| e.to_string())?;
        Ok(())
    }

    pub async fn get_all(&self) -> Result<SystemSettings, String> {
        let rows = system_settings::Entity::find()
            .all(self.db.read())
            .await
            .map_err(|e| e.to_string())?;

        let mut settings = SystemSettings::default();
        let mut latest_update = settings.updated_at;

        for row in rows {
            if let Ok(updated_at) = DateTime::parse_from_rfc3339(&row.updated_at) {
                let updated_at = updated_at.with_timezone(&Utc);
                if updated_at > latest_update {
                    latest_update = updated_at;
                }
            }

            match row.key.as_str() {
                "registration_enabled" => {
                    settings.registration_enabled = row.value.parse().unwrap_or(true);
                }
                "default_user_role" => {
                    settings.default_user_role = row.value;
                }
                "session_ttl_days" => {
                    settings.session_ttl_days = row.value.parse().unwrap_or(7);
                }
                "api_key_max_per_user" => {
                    settings.api_key_max_per_user = row.value.parse().unwrap_or(1000);
                }
                "site_name" => {
                    settings.site_name = row.value;
                }
                "site_description" => {
                    settings.site_description = row.value;
                }
                "api_base_url" => {
                    settings.api_base_url = row.value;
                }
                "global_transforms" => {
                    if let Ok(mut transforms) =
                        serde_json::from_str::<Vec<TransformRuleConfig>>(&row.value)
                    {
                        canonicalize_transform_rules(&mut transforms);
                        settings.global_transforms = transforms;
                    }
                }
                "reasoning_suffix_map" => {
                    if let Ok(map) = serde_json::from_str(&row.value) {
                        settings.reasoning_suffix_map = map;
                    }
                }
                "pricing_profile_model_patterns" => {
                    if let Ok(patterns) = serde_json::from_str(&row.value) {
                        settings.pricing_profile_model_patterns = patterns;
                    }
                }
                "monoize_active_probe_enabled" => {
                    settings.monoize_active_probe_enabled = row.value.parse().unwrap_or(true);
                }
                "monoize_active_probe_interval_seconds" => {
                    settings.monoize_active_probe_interval_seconds =
                        row.value.parse().unwrap_or(30);
                }
                "monoize_active_probe_success_threshold" => {
                    settings.monoize_active_probe_success_threshold =
                        row.value.parse().unwrap_or(1);
                }
                "monoize_active_probe_model" => {
                    let trimmed = row.value.trim();
                    settings.monoize_active_probe_model = if trimmed.is_empty() {
                        None
                    } else {
                        Some(trimmed.to_string())
                    };
                }
                "monoize_passive_failure_threshold" => {
                    settings.monoize_passive_failure_threshold = row.value.parse().unwrap_or(3);
                }
                "monoize_passive_cooldown_seconds" => {
                    settings.monoize_passive_cooldown_seconds = row.value.parse().unwrap_or(60);
                }
                "monoize_passive_window_seconds" => {
                    settings.monoize_passive_window_seconds = row.value.parse().unwrap_or(30);
                }
                "monoize_passive_min_samples" => {
                    settings.monoize_passive_min_samples = row.value.parse().unwrap_or(20);
                }
                "monoize_passive_failure_rate_threshold" => {
                    settings.monoize_passive_failure_rate_threshold =
                        row.value.parse().unwrap_or(0.6);
                }
                "monoize_passive_rate_limit_cooldown_seconds" => {
                    settings.monoize_passive_rate_limit_cooldown_seconds =
                        row.value.parse().unwrap_or(15);
                }
                "monoize_request_timeout_ms" => {
                    settings.monoize_request_timeout_ms = row.value.parse().unwrap_or(30000);
                }
                "monoize_stream_idle_timeout_ms" => {
                    settings.monoize_stream_idle_timeout_ms = row.value.parse().unwrap_or(120000);
                }
                "monoize_enable_estimated_billing" => {
                    settings.monoize_enable_estimated_billing = row.value.parse().unwrap_or(true);
                }
                "monoize_extra_fields_whitelist" => {
                    if let Ok(map) = serde_json::from_str(&row.value) {
                        settings.monoize_extra_fields_whitelist = map;
                    }
                }
                "monoize_strip_cross_protocol_nested_extra" => {
                    settings.monoize_strip_cross_protocol_nested_extra =
                        row.value.parse().unwrap_or(true);
                }
                "monoize_request_capture_enabled" => {
                    settings.monoize_request_capture_enabled = row.value.parse().unwrap_or(false);
                }
                "monoize_request_capture_retention_days" => {
                    settings.monoize_request_capture_retention_days =
                        row.value.parse().unwrap_or(1);
                }
                _ => {}
            }
        }

        settings.updated_at = latest_update;
        Ok(settings)
    }

    pub async fn update_all(&self, settings: &SystemSettings) -> Result<(), String> {
        let mut settings = settings.clone();
        canonicalize_transform_rules(&mut settings.global_transforms);
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
            "global_transforms",
            &serde_json::to_string(&settings.global_transforms)
                .unwrap_or_else(|_| "[]".to_string()),
        )
        .await?;
        self.set(
            "reasoning_suffix_map",
            &serde_json::to_string(&settings.reasoning_suffix_map)
                .unwrap_or_else(|_| "{}".to_string()),
        )
        .await?;
        self.set(
            "pricing_profile_model_patterns",
            &serde_json::to_string(&settings.pricing_profile_model_patterns)
                .unwrap_or_else(|_| "[]".to_string()),
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
            settings.monoize_active_probe_model.as_deref().unwrap_or(""),
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
        self.set(
            "monoize_stream_idle_timeout_ms",
            &settings.monoize_stream_idle_timeout_ms.to_string(),
        )
        .await?;
        self.set(
            "monoize_enable_estimated_billing",
            &settings.monoize_enable_estimated_billing.to_string(),
        )
        .await?;
        self.set(
            "monoize_extra_fields_whitelist",
            &serde_json::to_string(&settings.monoize_extra_fields_whitelist)
                .unwrap_or_else(|_| "{}".to_string()),
        )
        .await?;
        self.set(
            "monoize_strip_cross_protocol_nested_extra",
            &settings
                .monoize_strip_cross_protocol_nested_extra
                .to_string(),
        )
        .await?;
        self.set(
            "monoize_request_capture_enabled",
            &settings.monoize_request_capture_enabled.to_string(),
        )
        .await?;
        self.set(
            "monoize_request_capture_retention_days",
            &settings
                .monoize_request_capture_retention_days
                .max(1)
                .to_string(),
        )
        .await?;
        Ok(())
    }

    async fn migrate_transform_rule_ids(&self) -> Result<(), String> {
        let Some(raw) = self.get("global_transforms").await? else {
            return Ok(());
        };
        let Ok(mut transforms) = serde_json::from_str::<Vec<TransformRuleConfig>>(&raw) else {
            tracing::warn!("skip invalid global_transforms during transform id migration");
            return Ok(());
        };
        if !canonicalize_transform_rules(&mut transforms) {
            return Ok(());
        }
        self.set(
            "global_transforms",
            &serde_json::to_string(&transforms).map_err(|e| e.to_string())?,
        )
        .await
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

    pub async fn get_pricing_profile_model_patterns(
        &self,
    ) -> Result<Vec<PricingProfilePattern>, String> {
        match self.get("pricing_profile_model_patterns").await? {
            Some(json_str) => serde_json::from_str(&json_str)
                .map_err(|e| format!("invalid pricing_profile_model_patterns JSON: {e}")),
            None => Ok(default_pricing_profile_model_patterns()),
        }
    }

    pub async fn set_pricing_profile_model_patterns(
        &self,
        patterns: &[PricingProfilePattern],
    ) -> Result<(), String> {
        self.set(
            "pricing_profile_model_patterns",
            &serde_json::to_string(patterns).map_err(|e| e.to_string())?,
        )
        .await
    }
}
