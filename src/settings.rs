use crate::db::DbPool;
use crate::entity::system_settings;
use crate::monoize_routing::AffinityFailbackMode;
use crate::transforms::{TransformRuleConfig, canonicalize_transform_rules};
use crate::users::{ModelRedirectRule, validate_model_redirects};
use chrono::{DateTime, Utc};
use sea_orm::{ColumnTrait, ConnectionTrait, EntityTrait, QueryFilter, Set, sea_query::OnConflict};
use serde::{Deserialize, Serialize};
use std::collections::{HashMap, HashSet};

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct PricingProfilePattern {
    pub pattern: String,
    pub pricing_profile: String,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct PublicSettings {
    pub registration_enabled: bool,
    pub captcha_enabled: bool,
    pub site_name: String,
    pub site_description: String,
    pub api_base_url: String,
    pub cap_api_endpoint: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SystemSettings {
    pub registration_enabled: bool,
    pub captcha_enabled: bool,
    pub default_user_role: String,
    pub session_ttl_days: i64,
    pub api_key_max_per_user: i64,
    pub site_name: String,
    pub site_description: String,
    pub api_base_url: String,
    pub models_dev_base_url: String,
    #[serde(default)]
    pub global_transforms: Vec<TransformRuleConfig>,
    #[serde(default)]
    pub global_model_redirects: Vec<ModelRedirectRule>,
    pub reasoning_suffix_map: HashMap<String, String>,
    #[serde(default)]
    pub codex_model_ids: Vec<String>,
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
    #[serde(default = "default_true")]
    pub monoize_mask_sensitive_info: bool,
    pub monoize_affinity_enabled: bool,
    pub monoize_affinity_idle_ttl_seconds: u64,
    pub monoize_affinity_failback_mode: AffinityFailbackMode,
    pub monoize_affinity_failback_delay_seconds: u64,
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

pub fn canonicalize_codex_model_ids(model_ids: &mut Vec<String>) {
    let mut seen = HashSet::new();
    model_ids.retain_mut(|model_id| {
        let trimmed = model_id.trim().to_string();
        if trimmed.is_empty() || !seen.insert(trimmed.clone()) {
            return false;
        }
        *model_id = trimmed;
        true
    });
}

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

pub(crate) fn default_reasoning_suffix_map() -> HashMap<String, String> {
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
            captcha_enabled: true,
            default_user_role: "user".to_string(),
            session_ttl_days: 7,
            api_key_max_per_user: 1000,
            site_name: "Monoize Dashboard".to_string(),
            site_description: "Unified Responses Proxy".to_string(),
            api_base_url: String::new(),
            models_dev_base_url: "https://models.dev/api.json".to_string(),
            global_transforms: Vec::new(),
            global_model_redirects: Vec::new(),
            reasoning_suffix_map: default_reasoning_suffix_map(),
            codex_model_ids: Vec::new(),
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
            monoize_mask_sensitive_info: true,
            monoize_affinity_enabled: true,
            monoize_affinity_idle_ttl_seconds: 30 * 60,
            monoize_affinity_failback_mode: AffinityFailbackMode::Sticky,
            monoize_affinity_failback_delay_seconds: 5 * 60,
            updated_at: Utc::now(),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::{SystemSettings, default_pricing_profile_model_patterns};
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

    #[test]
    fn models_dev_base_url_defaults_to_official_host() {
        assert_eq!(
            SystemSettings::default().models_dev_base_url,
            "https://models.dev/api.json"
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

    /// Replica-side constructor per PRP11: performs no writes because defaults and
    /// canonicalization markers are guaranteed to exist once the primary has started.
    pub async fn new_read_only(db: DbPool) -> Result<Self, String> {
        Ok(Self { db })
    }

    async fn ensure_defaults(&self) -> Result<(), String> {
        let defaults = SystemSettings::default();
        self.set_if_not_exists(
            "registration_enabled",
            &serde_json::to_string(&defaults.registration_enabled).unwrap(),
        )
        .await?;
        self.set_if_not_exists("captcha_enabled", &defaults.captcha_enabled.to_string())
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
        self.set_if_not_exists("models_dev_base_url", &defaults.models_dev_base_url)
            .await?;
        self.set_if_not_exists(
            "global_transforms",
            &serde_json::to_string(&defaults.global_transforms)
                .unwrap_or_else(|_| "[]".to_string()),
        )
        .await?;
        self.set_if_not_exists(
            "global_model_redirects",
            &serde_json::to_string(&defaults.global_model_redirects)
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
        self.set_if_not_exists(
            "monoize_mask_sensitive_info",
            &defaults.monoize_mask_sensitive_info.to_string(),
        )
        .await?;
        self.set_if_not_exists(
            "monoize_affinity_enabled",
            &defaults.monoize_affinity_enabled.to_string(),
        )
        .await?;
        self.set_if_not_exists(
            "monoize_affinity_idle_ttl_seconds",
            &defaults.monoize_affinity_idle_ttl_seconds.to_string(),
        )
        .await?;
        self.set_if_not_exists(
            "monoize_affinity_failback_mode",
            defaults.monoize_affinity_failback_mode.as_str(),
        )
        .await?;
        self.set_if_not_exists(
            "monoize_affinity_failback_delay_seconds",
            &defaults.monoize_affinity_failback_delay_seconds.to_string(),
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

    pub async fn get_session_ttl_days(&self) -> Result<i64, String> {
        Ok(self
            .get("session_ttl_days")
            .await?
            .and_then(|value| value.parse().ok())
            .filter(|value| *value > 0)
            .unwrap_or(7))
    }

    pub async fn get_api_key_max_per_user(&self) -> Result<i64, String> {
        Ok(self
            .get("api_key_max_per_user")
            .await?
            .and_then(|value| value.parse().ok())
            .filter(|value| *value > 0)
            .unwrap_or(1000))
    }

    pub async fn get_public_settings(&self) -> Result<PublicSettings, String> {
        const PUBLIC_KEYS: [&str; 5] = [
            "registration_enabled",
            "captcha_enabled",
            "site_name",
            "site_description",
            "api_base_url",
        ];
        let rows = system_settings::Entity::find()
            .filter(system_settings::Column::Key.is_in(PUBLIC_KEYS))
            .all(self.db.read())
            .await
            .map_err(|error| error.to_string())?;
        let defaults = SystemSettings::default();
        let mut public = PublicSettings {
            registration_enabled: defaults.registration_enabled,
            captcha_enabled: defaults.captcha_enabled,
            site_name: defaults.site_name,
            site_description: defaults.site_description,
            api_base_url: defaults.api_base_url,
            cap_api_endpoint: None,
        };
        for row in rows {
            match row.key.as_str() {
                "registration_enabled" => {
                    public.registration_enabled = decode_registration_enabled(&row.value)?;
                }
                "captcha_enabled" => {
                    public.captcha_enabled = decode_captcha_enabled(&row.value)?;
                }
                "site_name" => public.site_name = row.value,
                "site_description" => public.site_description = row.value,
                "api_base_url" => public.api_base_url = row.value,
                _ => {}
            }
        }
        Ok(public)
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
                    settings.registration_enabled = decode_registration_enabled(&row.value)?;
                }
                "captcha_enabled" => {
                    settings.captcha_enabled = decode_captcha_enabled(&row.value)?;
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
                "models_dev_base_url" => {
                    settings.models_dev_base_url = row.value;
                }
                "global_transforms" => {
                    if let Ok(mut transforms) =
                        serde_json::from_str::<Vec<TransformRuleConfig>>(&row.value)
                    {
                        canonicalize_transform_rules(&mut transforms);
                        settings.global_transforms = transforms;
                    }
                }
                "global_model_redirects" => {
                    if let Ok(rules) = serde_json::from_str::<Vec<ModelRedirectRule>>(&row.value)
                        && validate_model_redirects(&rules).is_ok()
                    {
                        settings.global_model_redirects = rules;
                    }
                }
                "reasoning_suffix_map" => {
                    if let Ok(map) = serde_json::from_str(&row.value) {
                        settings.reasoning_suffix_map = map;
                    }
                }
                "codex_model_ids" => {
                    if let Ok(mut model_ids) = serde_json::from_str::<Vec<String>>(&row.value) {
                        canonicalize_codex_model_ids(&mut model_ids);
                        settings.codex_model_ids = model_ids;
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
                "monoize_mask_sensitive_info" => {
                    settings.monoize_mask_sensitive_info = row.value.parse().unwrap_or(true);
                }
                "monoize_affinity_enabled" => {
                    settings.monoize_affinity_enabled = row.value.parse().unwrap_or(true);
                }
                "monoize_affinity_idle_ttl_seconds" => {
                    settings.monoize_affinity_idle_ttl_seconds =
                        row.value.parse::<u64>().unwrap_or(30 * 60).max(1);
                }
                "monoize_affinity_failback_mode" => {
                    settings.monoize_affinity_failback_mode =
                        AffinityFailbackMode::from_str(&row.value).unwrap_or_default();
                }
                "monoize_affinity_failback_delay_seconds" => {
                    settings.monoize_affinity_failback_delay_seconds =
                        row.value.parse().unwrap_or(5 * 60);
                }
                _ => {}
            }
        }

        settings.updated_at = latest_update;
        Ok(settings)
    }

    pub async fn update_all(&self, settings: &SystemSettings) -> Result<SystemSettings, String> {
        let mut settings = settings.clone();
        canonicalize_transform_rules(&mut settings.global_transforms);
        canonicalize_codex_model_ids(&mut settings.codex_model_ids);
        settings.monoize_request_capture_retention_days =
            settings.monoize_request_capture_retention_days.max(1);
        settings.monoize_affinity_idle_ttl_seconds =
            settings.monoize_affinity_idle_ttl_seconds.max(1);
        let values = vec![
            (
                "registration_enabled",
                settings.registration_enabled.to_string(),
            ),
            ("captcha_enabled", settings.captcha_enabled.to_string()),
            ("default_user_role", settings.default_user_role.clone()),
            ("session_ttl_days", settings.session_ttl_days.to_string()),
            (
                "api_key_max_per_user",
                settings.api_key_max_per_user.to_string(),
            ),
            ("site_name", settings.site_name.clone()),
            ("site_description", settings.site_description.clone()),
            ("api_base_url", settings.api_base_url.clone()),
            ("models_dev_base_url", settings.models_dev_base_url.clone()),
            (
                "global_transforms",
                serde_json::to_string(&settings.global_transforms).map_err(|e| e.to_string())?,
            ),
            (
                "global_model_redirects",
                serde_json::to_string(&settings.global_model_redirects)
                    .map_err(|e| e.to_string())?,
            ),
            (
                "reasoning_suffix_map",
                serde_json::to_string(&settings.reasoning_suffix_map).map_err(|e| e.to_string())?,
            ),
            (
                "codex_model_ids",
                serde_json::to_string(&settings.codex_model_ids).map_err(|e| e.to_string())?,
            ),
            (
                "pricing_profile_model_patterns",
                serde_json::to_string(&settings.pricing_profile_model_patterns)
                    .map_err(|e| e.to_string())?,
            ),
            (
                "monoize_active_probe_enabled",
                settings.monoize_active_probe_enabled.to_string(),
            ),
            (
                "monoize_active_probe_interval_seconds",
                settings.monoize_active_probe_interval_seconds.to_string(),
            ),
            (
                "monoize_active_probe_success_threshold",
                settings.monoize_active_probe_success_threshold.to_string(),
            ),
            (
                "monoize_active_probe_model",
                settings
                    .monoize_active_probe_model
                    .clone()
                    .unwrap_or_default(),
            ),
            (
                "monoize_passive_failure_threshold",
                settings.monoize_passive_failure_threshold.to_string(),
            ),
            (
                "monoize_passive_cooldown_seconds",
                settings.monoize_passive_cooldown_seconds.to_string(),
            ),
            (
                "monoize_passive_window_seconds",
                settings.monoize_passive_window_seconds.to_string(),
            ),
            (
                "monoize_passive_min_samples",
                settings.monoize_passive_min_samples.to_string(),
            ),
            (
                "monoize_passive_failure_rate_threshold",
                settings.monoize_passive_failure_rate_threshold.to_string(),
            ),
            (
                "monoize_passive_rate_limit_cooldown_seconds",
                settings
                    .monoize_passive_rate_limit_cooldown_seconds
                    .to_string(),
            ),
            (
                "monoize_request_timeout_ms",
                settings.monoize_request_timeout_ms.to_string(),
            ),
            (
                "monoize_stream_idle_timeout_ms",
                settings.monoize_stream_idle_timeout_ms.to_string(),
            ),
            (
                "monoize_enable_estimated_billing",
                settings.monoize_enable_estimated_billing.to_string(),
            ),
            (
                "monoize_extra_fields_whitelist",
                serde_json::to_string(&settings.monoize_extra_fields_whitelist)
                    .map_err(|e| e.to_string())?,
            ),
            (
                "monoize_strip_cross_protocol_nested_extra",
                settings
                    .monoize_strip_cross_protocol_nested_extra
                    .to_string(),
            ),
            (
                "monoize_request_capture_enabled",
                settings.monoize_request_capture_enabled.to_string(),
            ),
            (
                "monoize_request_capture_retention_days",
                settings
                    .monoize_request_capture_retention_days
                    .max(1)
                    .to_string(),
            ),
            (
                "monoize_mask_sensitive_info",
                settings.monoize_mask_sensitive_info.to_string(),
            ),
            (
                "monoize_affinity_enabled",
                settings.monoize_affinity_enabled.to_string(),
            ),
            (
                "monoize_affinity_idle_ttl_seconds",
                settings
                    .monoize_affinity_idle_ttl_seconds
                    .max(1)
                    .to_string(),
            ),
            (
                "monoize_affinity_failback_mode",
                settings.monoize_affinity_failback_mode.as_str().to_string(),
            ),
            (
                "monoize_affinity_failback_delay_seconds",
                settings.monoize_affinity_failback_delay_seconds.to_string(),
            ),
        ];

        let committed_at = Utc::now();
        let updated_at = committed_at.to_rfc3339();
        let models = values
            .into_iter()
            .map(|(key, value)| system_settings::ActiveModel {
                key: Set(key.to_string()),
                value: Set(value),
                updated_at: Set(updated_at.clone()),
            })
            .collect::<Vec<_>>();
        let transaction = self.db.begin_write().await.map_err(|e| e.to_string())?;
        system_settings::Entity::insert_many(models)
            .on_conflict(
                OnConflict::column(system_settings::Column::Key)
                    .update_columns([
                        system_settings::Column::Value,
                        system_settings::Column::UpdatedAt,
                    ])
                    .to_owned(),
            )
            .exec(&*transaction)
            .await
            .map_err(|e| e.to_string())?;
        bump_config_epoch_in_tx(&self.db, &transaction).await?;
        transaction.commit().await.map_err(|e| e.to_string())?;
        settings.updated_at = committed_at;
        Ok(settings)
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
        match self.get("registration_enabled").await? {
            Some(raw) => decode_registration_enabled(&raw),
            None => Ok(true),
        }
    }

    pub async fn is_captcha_enabled(&self) -> Result<bool, String> {
        match self.get("captcha_enabled").await? {
            Some(raw) => decode_captcha_enabled(&raw),
            None => Ok(true),
        }
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
        // E2: the point mutation and its epoch increment commit in one transaction.
        let transaction = self.db.begin_write().await.map_err(|e| e.to_string())?;
        let now = Utc::now().to_rfc3339();
        let model = system_settings::ActiveModel {
            key: Set("pricing_profile_model_patterns".to_string()),
            value: Set(serde_json::to_string(patterns).map_err(|e| e.to_string())?),
            updated_at: Set(now),
        };
        system_settings::Entity::insert(model)
            .on_conflict(
                OnConflict::column(system_settings::Column::Key)
                    .update_columns([
                        system_settings::Column::Value,
                        system_settings::Column::UpdatedAt,
                    ])
                    .to_owned(),
            )
            .exec(&*transaction)
            .await
            .map_err(|e| e.to_string())?;
        bump_config_epoch_in_tx(&self.db, &transaction).await?;
        transaction.commit().await.map_err(|e| e.to_string())
    }
}

pub(crate) const CONFIG_EPOCH_TENANT: &str = "monoize";
pub(crate) const CONFIG_EPOCH_KIND: &str = "config_epoch";
pub(crate) const CONFIG_EPOCH_ID: &str = "global";

/// E3 read path on a replica: exactly one row, one column.
pub async fn read_config_epoch(db: &DbPool) -> Result<u64, String> {
    let rows = db
        .read()
        .query_all(db.stmt(
            "SELECT value FROM state_records WHERE tenant_id = $1 AND kind = $2 AND id = $3",
            vec![
                CONFIG_EPOCH_TENANT.into(),
                CONFIG_EPOCH_KIND.into(),
                CONFIG_EPOCH_ID.into(),
            ],
        ))
        .await
        .map_err(|e| e.to_string())?;
    match rows.first() {
        None => Ok(0),
        Some(row) => row
            .try_get::<String>("", "value")
            .map_err(|e| format!("invalid config epoch row: {e}"))?
            .trim()
            .parse::<u64>()
            .map_err(|error| format!("invalid config epoch value: {error}")),
    }
}

/// E2 write path on the primary: one statement computing `value + 1` inside the
/// caller's open transaction. A missing row is inserted as epoch 1 (0 + 1).
pub(crate) async fn bump_config_epoch_in_tx(
    db: &DbPool,
    tx: &sea_orm::DatabaseTransaction,
) -> Result<(), String> {
    let sql = if db.is_sqlite() {
        "INSERT INTO state_records (tenant_id, kind, id, value, expires_at) \
         VALUES ($1, $2, $3, '1', NULL) \
         ON CONFLICT (tenant_id, kind, id) \
         DO UPDATE SET value = CAST(CAST(value AS INTEGER) + 1 AS TEXT)"
    } else {
        "INSERT INTO state_records (tenant_id, kind, id, value, expires_at) \
         VALUES ($1, $2, $3, '1', NULL) \
         ON CONFLICT (tenant_id, kind, id) \
         DO UPDATE SET value = CAST(CAST(state_records.value AS BIGINT) + 1 AS TEXT)"
    };
    tx.execute(db.stmt(
        sql,
        vec![
            CONFIG_EPOCH_TENANT.into(),
            CONFIG_EPOCH_KIND.into(),
            CONFIG_EPOCH_ID.into(),
        ],
    ))
    .await
    .map_err(|e| e.to_string())?;
    Ok(())
}

fn decode_registration_enabled(raw: &str) -> Result<bool, String> {
    raw.parse::<bool>()
        .map_err(|error| format!("invalid registration_enabled boolean: {error}"))
}

fn decode_captcha_enabled(raw: &str) -> Result<bool, String> {
    raw.parse::<bool>()
        .map_err(|error| format!("invalid captcha_enabled boolean: {error}"))
}
