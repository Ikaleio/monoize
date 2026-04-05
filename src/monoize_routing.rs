use crate::db::DbPool;
use crate::transforms::TransformRuleConfig;
use crate::users::{canonicalize_groups, parse_groups_json};
use chrono::{DateTime, Utc};
use sea_orm::{ConnectionTrait, QueryResult, Value as SeaValue};
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use std::collections::{HashMap, HashSet, VecDeque};
use std::time::Duration;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum MonoizeProviderType {
    Responses,
    ChatCompletion,
    Messages,
    Gemini,
    OpenaiImage,
}

impl MonoizeProviderType {
    #[allow(clippy::should_implement_trait)]
    pub fn from_str(s: &str) -> Option<Self> {
        match s {
            "responses" => Some(Self::Responses),
            "chat_completion" => Some(Self::ChatCompletion),
            "messages" => Some(Self::Messages),
            "gemini" => Some(Self::Gemini),
            "openai_image" => Some(Self::OpenaiImage),
            _ => None,
        }
    }

    pub fn as_str(&self) -> &'static str {
        match self {
            Self::Responses => "responses",
            Self::ChatCompletion => "chat_completion",
            Self::Messages => "messages",
            Self::Gemini => "gemini",
            Self::OpenaiImage => "openai_image",
        }
    }

    pub fn to_config_type(&self) -> crate::config::ProviderType {
        match self {
            Self::Responses => crate::config::ProviderType::Responses,
            Self::ChatCompletion => crate::config::ProviderType::ChatCompletion,
            Self::Messages => crate::config::ProviderType::Messages,
            Self::Gemini => crate::config::ProviderType::Gemini,
            Self::OpenaiImage => crate::config::ProviderType::OpenaiImage,
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ApiTypeOverride {
    pub pattern: String,
    pub api_type: MonoizeProviderType,
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
    pub passive_failure_count_threshold_override: Option<u32>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub passive_cooldown_seconds_override: Option<u64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub passive_window_seconds_override: Option<u64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub passive_rate_limit_cooldown_seconds_override: Option<u64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub _healthy: Option<bool>,
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
    pub channel_max_retries: i32,
    pub channel_retry_interval_ms: u64,
    pub circuit_breaker_enabled: bool,
    pub per_model_circuit_break: bool,
    #[serde(default)]
    pub transforms: Vec<TransformRuleConfig>,
    #[serde(default)]
    pub api_type_overrides: Vec<ApiTypeOverride>,
    pub active_probe_enabled_override: Option<bool>,
    pub active_probe_interval_seconds_override: Option<u64>,
    pub active_probe_success_threshold_override: Option<u32>,
    pub active_probe_model_override: Option<String>,
    pub request_timeout_ms_override: Option<u64>,
    #[serde(default)]
    pub extra_fields_whitelist: Option<Vec<String>>,
    #[serde(default)]
    pub strip_cross_protocol_nested_extra: Option<bool>,
    #[serde(default)]
    pub groups: Vec<String>,
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
    #[serde(default)]
    pub passive_failure_count_threshold_override: Option<u32>,
    #[serde(default)]
    pub passive_cooldown_seconds_override: Option<u64>,
    #[serde(default)]
    pub passive_window_seconds_override: Option<u64>,
    #[serde(default)]
    pub passive_rate_limit_cooldown_seconds_override: Option<u64>,
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
    pub channel_max_retries: i32,
    #[serde(default)]
    pub channel_retry_interval_ms: u64,
    #[serde(default = "default_enabled")]
    pub circuit_breaker_enabled: bool,
    #[serde(default)]
    pub per_model_circuit_break: bool,
    #[serde(default)]
    pub transforms: Vec<TransformRuleConfig>,
    pub active_probe_enabled_override: Option<bool>,
    #[serde(default)]
    pub api_type_overrides: Vec<ApiTypeOverride>,
    pub active_probe_interval_seconds_override: Option<u64>,
    pub active_probe_success_threshold_override: Option<u32>,
    pub active_probe_model_override: Option<String>,
    pub request_timeout_ms_override: Option<u64>,
    #[serde(default)]
    pub extra_fields_whitelist: Option<Vec<String>>,
    #[serde(default)]
    pub strip_cross_protocol_nested_extra: Option<bool>,
    #[serde(default)]
    pub groups: Vec<String>,
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
    pub channel_max_retries: Option<i32>,
    pub channel_retry_interval_ms: Option<u64>,
    pub circuit_breaker_enabled: Option<bool>,
    pub per_model_circuit_break: Option<bool>,
    pub transforms: Option<Vec<TransformRuleConfig>>,
    pub active_probe_enabled_override: Option<Option<bool>>,
    pub api_type_overrides: Option<Vec<ApiTypeOverride>>,
    pub active_probe_interval_seconds_override: Option<Option<u64>>,
    pub active_probe_success_threshold_override: Option<Option<u32>>,
    pub active_probe_model_override: Option<Option<String>>,
    pub request_timeout_ms_override: Option<Option<u64>>,
    pub extra_fields_whitelist: Option<Option<Vec<String>>>,
    pub strip_cross_protocol_nested_extra: Option<Option<bool>>,
    pub groups: Option<Vec<String>>,
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
    pub enable_estimated_billing: bool,
    pub passive_failure_count_threshold: u32,
    pub passive_cooldown_seconds: u64,
    pub passive_window_seconds: u64,
    pub passive_rate_limit_cooldown_seconds: u64,
    pub active_enabled: bool,
    pub active_interval_seconds: u64,
    pub active_success_threshold: u32,
    pub active_method: String,
    pub active_probe_model: Option<String>,
    pub extra_fields_whitelist: HashMap<String, Vec<String>>,
    pub strip_cross_protocol_nested_extra: bool,
}

impl Default for MonoizeRuntimeConfig {
    fn default() -> Self {
        Self {
            request_timeout_ms: 30_000,
            enable_estimated_billing: true,
            passive_failure_count_threshold: 3,
            passive_cooldown_seconds: 60,
            passive_window_seconds: 30,
            passive_rate_limit_cooldown_seconds: 15,
            active_enabled: true,
            active_interval_seconds: 30,
            active_success_threshold: 1,
            active_method: "completion".to_string(),
            active_probe_model: None,
            extra_fields_whitelist: HashMap::new(),
            strip_cross_protocol_nested_extra: true,
        }
    }
}

#[derive(Debug, Clone)]
pub struct PassiveHealthSample {
    pub at_ts: i64,
    pub failed: bool,
}

#[derive(Debug, Clone, Default)]
pub struct ChannelHealthState {
    pub healthy: bool,
    pub last_success_at: Option<i64>,
    pub cooldown_until: Option<i64>,
    pub probe_success_count: u32,
    pub last_probe_at: Option<i64>,
    pub passive_samples: VecDeque<PassiveHealthSample>,
}

impl ChannelHealthState {
    pub fn new() -> Self {
        Self {
            healthy: true,
            last_success_at: None,
            cooldown_until: None,
            probe_success_count: 0,
            last_probe_at: None,
            passive_samples: VecDeque::new(),
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
    db: DbPool,
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

fn parse_provider_groups_json(raw: &str) -> Vec<String> {
    parse_groups_json(raw)
}

fn serialize_provider_groups_json(groups: &[String]) -> Result<String, String> {
    serde_json::to_string(&canonicalize_groups(groups)).map_err(|e| e.to_string())
}

fn generate_short_id() -> String {
    const CHARSET: &[u8] = b"abcdefghijklmnopqrstuvwxyz0123456789";
    let bytes = uuid::Uuid::new_v4().into_bytes();
    (0..8)
        .map(|i| CHARSET[bytes[i] as usize % CHARSET.len()] as char)
        .collect()
}

impl MonoizeRoutingStore {
    pub async fn new(db: DbPool) -> Result<Self, String> {
        Ok(Self { db })
    }

    pub async fn provider_count(&self) -> Result<i64, String> {
        let row = self
            .db
            .read()
            .query_one(
                self.db
                    .stmt("SELECT COUNT(*) as cnt FROM monoize_providers", vec![]),
            )
            .await
            .map_err(|e| e.to_string())?
            .ok_or_else(|| "count query returned no rows".to_string())?;
        row.try_get("", "cnt").map_err(|e| e.to_string())
    }

    pub async fn list_providers(&self) -> Result<Vec<MonoizeProvider>, String> {
        let rows = self
            .db
            .read()
            .query_all(self.db.stmt(
                r#"SELECT id, name, provider_type, max_retries, channel_max_retries,
                          channel_retry_interval_ms, circuit_breaker_enabled,
                          per_model_circuit_break, transforms, api_type_overrides,
                          active_probe_enabled_override, active_probe_interval_seconds_override,
                          active_probe_success_threshold_override, active_probe_model_override,
                          request_timeout_ms_override, extra_fields_whitelist, groups,
                          enabled, priority, created_at, updated_at
                   FROM monoize_providers
                   ORDER BY priority ASC, created_at ASC"#,
                vec![],
            ))
            .await
            .map_err(|e| e.to_string())?;

        let mut providers = Vec::new();
        for row in &rows {
            providers.push(self.row_to_provider(row).await?);
        }
        Ok(providers)
    }

    pub async fn list_all_provider_groups_json(&self) -> Result<Vec<String>, String> {
        let rows = self
            .db
            .read()
            .query_all(self.db.stmt("SELECT groups FROM monoize_providers", vec![]))
            .await
            .map_err(|e| e.to_string())?;

        rows.into_iter()
            .map(|row| row.try_get("", "groups").map_err(|e| e.to_string()))
            .collect()
    }

    pub async fn get_provider(&self, id: &str) -> Result<Option<MonoizeProvider>, String> {
        let row = self
            .db
            .read()
            .query_one(self.db.stmt(
                r#"SELECT id, name, provider_type, max_retries, channel_max_retries,
                          channel_retry_interval_ms, circuit_breaker_enabled,
                          per_model_circuit_break, transforms, api_type_overrides,
                          active_probe_enabled_override, active_probe_interval_seconds_override,
                          active_probe_success_threshold_override, active_probe_model_override,
                          request_timeout_ms_override, extra_fields_whitelist, groups,
                          enabled, priority, created_at, updated_at
                   FROM monoize_providers
                   WHERE id = $1"#,
                vec![id.into()],
            ))
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
        validate_provider_input(
            &input.name,
            &input.models,
            &input.channels,
            &input.api_type_overrides,
        )?;
        if let Some(v) = input.active_probe_interval_seconds_override {
            if v == 0 {
                return Err("active_probe_interval_seconds_override must be >= 1".to_string());
            }
        }
        if let Some(v) = input.active_probe_success_threshold_override {
            if v == 0 {
                return Err("active_probe_success_threshold_override must be >= 1".to_string());
            }
        }
        if let Some(v) = input.request_timeout_ms_override {
            if v == 0 {
                return Err("request_timeout_ms_override must be >= 1".to_string());
            }
        }

        let id = generate_short_id();
        let now = Utc::now();

        let priority = match input.priority {
            Some(v) => v,
            None => {
                let row = self
                    .db
                    .read()
                    .query_one(self.db.stmt(
                        "SELECT MAX(priority) as max_p FROM monoize_providers",
                        vec![],
                    ))
                    .await
                    .map_err(|e| e.to_string())?;
                let max_priority: Option<i64> = row
                    .and_then(|r| r.try_get("", "max_p").ok())
                    .unwrap_or(None);
                max_priority.unwrap_or(-1) as i32 + 1
            }
        };

        let transforms_json =
            serde_json::to_string(&input.transforms).map_err(|e| e.to_string())?;
        let api_type_overrides_json =
            serde_json::to_string(&input.api_type_overrides).map_err(|e| e.to_string())?;
        let groups_json = serialize_provider_groups_json(&input.groups)?;
        let extra_fields_whitelist_json: Option<String> = input
            .extra_fields_whitelist
            .as_ref()
            .map(|v| serde_json::to_string(v).unwrap_or_else(|_| "[]".to_string()));
        let strip_cross_proto = input.strip_cross_protocol_nested_extra;

        self.db
            .write()
            .await
            .execute(self.db.stmt(
                r#"INSERT INTO monoize_providers (
                        id, name, provider_type, max_retries, channel_max_retries,
                        channel_retry_interval_ms, circuit_breaker_enabled,
                        per_model_circuit_break, transforms, api_type_overrides,
                        active_probe_enabled_override, active_probe_interval_seconds_override,
                        active_probe_success_threshold_override, active_probe_model_override,
                        request_timeout_ms_override, extra_fields_whitelist,
                        strip_cross_protocol_nested_extra, groups,
                        enabled, priority, created_at, updated_at
                   ) VALUES ($1, $2, $3, $4, $5, $6, $7, $8, $9, $10, $11, $12, $13, $14, $15, $16, $17, $18, $19, $20, $21, $22)"#,
                vec![
                        id.clone().into(),
                        input.name.clone().into(),
                        input.provider_type.as_str().into(),
                        SeaValue::Int(Some(input.max_retries)),
                        SeaValue::Int(Some(input.channel_max_retries)),
                        SeaValue::Int(Some(input.channel_retry_interval_ms as i32)),
                        SeaValue::Int(Some(if input.circuit_breaker_enabled { 1 } else { 0 })),
                        SeaValue::Int(Some(if input.per_model_circuit_break { 1 } else { 0 })),
                        transforms_json.into(),
                        api_type_overrides_json.into(),
                        opt_bool_to_value(input.active_probe_enabled_override),
                        opt_u64_to_value(input.active_probe_interval_seconds_override),
                        opt_u64_to_value(
                            input
                                .active_probe_success_threshold_override
                                .map(|v| v as u64),
                        ),
                        input.active_probe_model_override.clone().into(),
                        opt_u64_to_value(input.request_timeout_ms_override),
                        extra_fields_whitelist_json.into(),
                        opt_bool_to_value(strip_cross_proto),
                        groups_json.into(),
                        SeaValue::Int(Some(if input.enabled { 1 } else { 0 })),
                        SeaValue::Int(Some(priority)),
                        now.to_rfc3339().into(),
                        now.to_rfc3339().into(),
                    ],
            ))
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
        if let Some(Some(v)) = input.active_probe_interval_seconds_override {
            if v == 0 {
                return Err("active_probe_interval_seconds_override must be >= 1".to_string());
            }
        }
        if let Some(Some(v)) = input.active_probe_success_threshold_override {
            if v == 0 {
                return Err("active_probe_success_threshold_override must be >= 1".to_string());
            }
        }
        if let Some(Some(v)) = input.request_timeout_ms_override {
            if v == 0 {
                return Err("request_timeout_ms_override must be >= 1".to_string());
            }
        }

        let name = input.name.unwrap_or(existing.name.clone());
        let provider_type = input.provider_type.unwrap_or(existing.provider_type);
        let max_retries = input.max_retries.unwrap_or(existing.max_retries);
        let channel_max_retries = input
            .channel_max_retries
            .unwrap_or(existing.channel_max_retries);
        let channel_retry_interval_ms = input
            .channel_retry_interval_ms
            .unwrap_or(existing.channel_retry_interval_ms);
        let circuit_breaker_enabled = input
            .circuit_breaker_enabled
            .unwrap_or(existing.circuit_breaker_enabled);
        let per_model_circuit_break = input
            .per_model_circuit_break
            .unwrap_or(existing.per_model_circuit_break);
        let transforms = input.transforms.unwrap_or(existing.transforms.clone());
        let api_type_overrides = input
            .api_type_overrides
            .unwrap_or(existing.api_type_overrides.clone());
        validate_api_type_overrides(&api_type_overrides)?;
        let active_probe_enabled_override = input
            .active_probe_enabled_override
            .unwrap_or(existing.active_probe_enabled_override);
        let active_probe_interval_seconds_override = input
            .active_probe_interval_seconds_override
            .unwrap_or(existing.active_probe_interval_seconds_override);
        let active_probe_success_threshold_override = input
            .active_probe_success_threshold_override
            .unwrap_or(existing.active_probe_success_threshold_override);
        let active_probe_model_override = input
            .active_probe_model_override
            .unwrap_or(existing.active_probe_model_override.clone());
        let request_timeout_ms_override = input
            .request_timeout_ms_override
            .unwrap_or(existing.request_timeout_ms_override);
        let extra_fields_whitelist = input
            .extra_fields_whitelist
            .unwrap_or(existing.extra_fields_whitelist.clone());
        let strip_cross_protocol_nested_extra = input
            .strip_cross_protocol_nested_extra
            .unwrap_or(existing.strip_cross_protocol_nested_extra);
        let groups = canonicalize_groups(input.groups.as_deref().unwrap_or(&existing.groups));
        let enabled = input.enabled.unwrap_or(existing.enabled);
        let priority = input.priority.unwrap_or(existing.priority);

        let now = Utc::now();

        let transforms_json = serde_json::to_string(&transforms).map_err(|e| e.to_string())?;
        let api_type_overrides_json =
            serde_json::to_string(&api_type_overrides).map_err(|e| e.to_string())?;
        let groups_json = serialize_provider_groups_json(&groups)?;
        let extra_fields_whitelist_json: Option<String> = extra_fields_whitelist
            .as_ref()
            .map(|v| serde_json::to_string(v).unwrap_or_else(|_| "[]".to_string()));

        let txn = self.db.begin_write().await.map_err(|e| e.to_string())?;

        txn.execute(self.db.stmt(
                r#"UPDATE monoize_providers
                   SET name = $1, provider_type = $2, max_retries = $3,
                       channel_max_retries = $4,
                       channel_retry_interval_ms = $5,
                       circuit_breaker_enabled = $6,
                       per_model_circuit_break = $7,
                       transforms = $8, api_type_overrides = $9,
                       active_probe_enabled_override = $10,
                       active_probe_interval_seconds_override = $11,
                       active_probe_success_threshold_override = $12,
                       active_probe_model_override = $13,
                       request_timeout_ms_override = $14,
                       extra_fields_whitelist = $15,
                       strip_cross_protocol_nested_extra = $16,
                       groups = $17,
                       enabled = $18, priority = $19, updated_at = $20
                   WHERE id = $21"#,
                vec![
                    name.into(),
                    provider_type.as_str().into(),
                    SeaValue::Int(Some(max_retries)),
                    SeaValue::Int(Some(channel_max_retries)),
                    SeaValue::Int(Some(channel_retry_interval_ms as i32)),
                    SeaValue::Int(Some(if circuit_breaker_enabled { 1 } else { 0 })),
                    SeaValue::Int(Some(if per_model_circuit_break { 1 } else { 0 })),
                    transforms_json.into(),
                    api_type_overrides_json.into(),
                    opt_bool_to_value(active_probe_enabled_override),
                    opt_u64_to_value(active_probe_interval_seconds_override),
                    opt_u64_to_value(active_probe_success_threshold_override.map(|v| v as u64)),
                    active_probe_model_override.into(),
                    opt_u64_to_value(request_timeout_ms_override),
                    extra_fields_whitelist_json.into(),
                    opt_bool_to_value(strip_cross_protocol_nested_extra),
                    groups_json.into(),
                    SeaValue::Int(Some(if enabled { 1 } else { 0 })),
                    SeaValue::Int(Some(priority)),
                    now.to_rfc3339().into(),
                    id.into(),
                ],
            ))
            .await
            .map_err(|e| e.to_string())?;

        if let Some(models) = &input.models {
            self.replace_models_on(&*txn, id, models).await?;
        }
        if let Some(channels) = &input.channels {
            self.replace_channels_on(&*txn, id, channels).await?;
        }

        txn.commit().await.map_err(|e| e.to_string())?;

        self.get_provider(id)
            .await?
            .ok_or_else(|| "provider not found after update".to_string())
    }

    pub async fn delete_provider(&self, id: &str) -> Result<(), String> {
        let result = self
            .db
            .write()
            .await
            .execute(self.db.stmt(
                "DELETE FROM monoize_providers WHERE id = $1",
                vec![id.into()],
            ))
            .await
            .map_err(|e| e.to_string())?;

        if result.rows_affected() == 0 {
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
            self.db
                .write()
                .await
                .execute(self.db.stmt(
                    "UPDATE monoize_providers SET priority = $1, updated_at = $2 WHERE id = $3",
                    vec![
                        SeaValue::Int(Some(i as i32)),
                        Utc::now().to_rfc3339().into(),
                        id.as_str().into(),
                    ],
                ))
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
        let w = self.db.write().await;
        self.replace_models_on(&*w, provider_id, models).await
    }

    async fn replace_models_on(
        &self,
        conn: &impl ConnectionTrait,
        provider_id: &str,
        models: &HashMap<String, MonoizeModelEntry>,
    ) -> Result<(), String> {
        conn.execute(self.db.stmt(
                "DELETE FROM monoize_provider_models WHERE provider_id = $1",
                vec![provider_id.into()],
            ))
            .await
            .map_err(|e| e.to_string())?;

        for (model_name, entry) in models {
            conn.execute(self.db.stmt(
                    r#"INSERT INTO monoize_provider_models
                       (id, provider_id, model_name, redirect, multiplier, created_at)
                       VALUES ($1, $2, $3, $4, $5, $6)"#,
                    vec![
                        format!("mono_model_{}", uuid::Uuid::new_v4().simple()).into(),
                        provider_id.into(),
                        model_name.as_str().into(),
                        entry.redirect.clone().into(),
                        SeaValue::Double(Some(entry.multiplier)),
                        Utc::now().to_rfc3339().into(),
                    ],
                ))
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
        let w = self.db.write().await;
        self.replace_channels_on(&*w, provider_id, channels).await
    }

    async fn replace_channels_on(
        &self,
        conn: &impl ConnectionTrait,
        provider_id: &str,
        channels: &[CreateMonoizeChannelInput],
    ) -> Result<(), String> {
        let existing_rows = conn
            .query_all(self.db.stmt(
                "SELECT id, api_key,
                        passive_failure_count_threshold_override,
                        passive_cooldown_seconds_override,
                        passive_window_seconds_override,
                        passive_rate_limit_cooldown_seconds_override
                 FROM monoize_channels
                 WHERE provider_id = $1",
                vec![provider_id.into()],
            ))
            .await
            .map_err(|e| e.to_string())?;

        #[derive(Clone)]
        struct ExistingChannel {
            api_key: String,
            passive_failure_count_threshold_override: Option<u32>,
            passive_cooldown_seconds_override: Option<u64>,
            passive_window_seconds_override: Option<u64>,
            passive_rate_limit_cooldown_seconds_override: Option<u64>,
        }
        let mut existing_channels: HashMap<String, ExistingChannel> = HashMap::new();
        for row in &existing_rows {
            let id: String = row.try_get("", "id").map_err(|e| e.to_string())?;
            existing_channels.insert(
                id,
                ExistingChannel {
                    api_key: row.try_get("", "api_key").map_err(|e| e.to_string())?,
                    passive_failure_count_threshold_override: row
                        .try_get::<Option<i64>>("", "passive_failure_count_threshold_override")
                        .map_err(|e| e.to_string())?
                        .map(|v| v as u32),
                    passive_cooldown_seconds_override: row
                        .try_get::<Option<i64>>("", "passive_cooldown_seconds_override")
                        .map_err(|e| e.to_string())?
                        .map(|v| v as u64),
                    passive_window_seconds_override: row
                        .try_get::<Option<i64>>("", "passive_window_seconds_override")
                        .map_err(|e| e.to_string())?
                        .map(|v| v as u64),
                    passive_rate_limit_cooldown_seconds_override: row
                        .try_get::<Option<i64>>("", "passive_rate_limit_cooldown_seconds_override")
                        .map_err(|e| e.to_string())?
                        .map(|v| v as u64),
                },
            );
        }

        conn.execute(self.db.stmt(
                "DELETE FROM monoize_channels WHERE provider_id = $1",
                vec![provider_id.into()],
            ))
            .await
            .map_err(|e| e.to_string())?;

        for input in channels {
            let id = input
                .id
                .clone()
                .unwrap_or_else(|| format!("mono_ch_{}", uuid::Uuid::new_v4().simple()));

            let api_key = match input.api_key.as_deref() {
                Some(k) if !k.trim().is_empty() => k.to_string(),
                _ => existing_channels
                    .get(&id)
                    .map(|c| c.api_key.clone())
                    .ok_or_else(|| {
                        format!(
                            "channel api_key must not be empty for new channel '{}'",
                            input.name
                        )
                    })?,
            };
            let existing = existing_channels.get(&id);
            let passive_failure_count_threshold_override = input
                .passive_failure_count_threshold_override
                .or_else(|| existing.and_then(|c| c.passive_failure_count_threshold_override));
            let passive_cooldown_seconds_override = input
                .passive_cooldown_seconds_override
                .or_else(|| existing.and_then(|c| c.passive_cooldown_seconds_override));
            let passive_window_seconds_override = input
                .passive_window_seconds_override
                .or_else(|| existing.and_then(|c| c.passive_window_seconds_override));
            let passive_rate_limit_cooldown_seconds_override = input
                .passive_rate_limit_cooldown_seconds_override
                .or_else(|| existing.and_then(|c| c.passive_rate_limit_cooldown_seconds_override));

            let now_str = Utc::now().to_rfc3339();

            conn.execute(self.db.stmt(
                    r#"INSERT INTO monoize_channels
                       (id, provider_id, name, base_url, api_key, weight, enabled,
                          passive_failure_count_threshold_override, passive_cooldown_seconds_override,
                          passive_window_seconds_override, passive_rate_limit_cooldown_seconds_override,
                          created_at, updated_at)
                       VALUES ($1, $2, $3, $4, $5, $6, $7, $8, $9, $10, $11, $12, $13)"#,
                    vec![
                        id.into(),
                        provider_id.into(),
                        input.name.as_str().into(),
                        input.base_url.as_str().into(),
                        api_key.into(),
                        SeaValue::Int(Some(input.weight)),
                        SeaValue::Int(Some(if input.enabled { 1 } else { 0 })),
                        opt_u64_to_value(
                            passive_failure_count_threshold_override.map(|v| v as u64),
                        ),
                        opt_u64_to_value(passive_cooldown_seconds_override),
                        opt_u64_to_value(passive_window_seconds_override),
                        opt_u64_to_value(passive_rate_limit_cooldown_seconds_override),
                        now_str.clone().into(),
                        now_str.into(),
                    ],
                ))
                .await
                .map_err(|e| e.to_string())?;
        }
        Ok(())
    }

    async fn row_to_provider(&self, row: &QueryResult) -> Result<MonoizeProvider, String> {
        let id: String = row.try_get("", "id").map_err(|e| e.to_string())?;
        let provider_type_raw: String = row
            .try_get("", "provider_type")
            .map_err(|e| e.to_string())?;
        let provider_type = MonoizeProviderType::from_str(&provider_type_raw)
            .ok_or_else(|| format!("invalid provider type: {provider_type_raw}"))?;

        let model_rows = self
            .db
            .read()
            .query_all(self.db.stmt(
                r#"SELECT model_name, redirect, multiplier
                   FROM monoize_provider_models
                   WHERE provider_id = $1"#,
                vec![id.clone().into()],
            ))
            .await
            .map_err(|e| e.to_string())?;

        let mut models = HashMap::new();
        for mr in &model_rows {
            let model_name: String = mr.try_get("", "model_name").map_err(|e| e.to_string())?;
            let redirect: Option<String> = mr.try_get("", "redirect").map_err(|e| e.to_string())?;
            let multiplier: f64 = mr.try_get("", "multiplier").map_err(|e| e.to_string())?;
            models.insert(
                model_name,
                MonoizeModelEntry {
                    redirect,
                    multiplier,
                },
            );
        }

        let channel_rows = self
            .db
            .read()
            .query_all(self.db.stmt(
                r#"SELECT id, name, base_url, api_key, weight, enabled,
                          passive_failure_count_threshold_override,
                          passive_cooldown_seconds_override,
                          passive_window_seconds_override,
                          passive_rate_limit_cooldown_seconds_override
                   FROM monoize_channels
                   WHERE provider_id = $1
                   ORDER BY created_at ASC"#,
                vec![id.clone().into()],
            ))
            .await
            .map_err(|e| e.to_string())?;

        let mut channels = Vec::new();
        for cr in &channel_rows {
            channels.push(MonoizeChannel {
                id: cr.try_get("", "id").map_err(|e| e.to_string())?,
                name: cr.try_get("", "name").map_err(|e| e.to_string())?,
                base_url: cr.try_get("", "base_url").map_err(|e| e.to_string())?,
                api_key: cr.try_get("", "api_key").map_err(|e| e.to_string())?,
                weight: cr.try_get("", "weight").map_err(|e| e.to_string())?,
                enabled: cr
                    .try_get::<i32>("", "enabled")
                    .map_err(|e| e.to_string())?
                    == 1,
                passive_failure_count_threshold_override: cr
                    .try_get::<Option<i64>>("", "passive_failure_count_threshold_override")
                    .map_err(|e| e.to_string())?
                    .map(|v| v as u32),
                passive_cooldown_seconds_override: cr
                    .try_get::<Option<i64>>("", "passive_cooldown_seconds_override")
                    .map_err(|e| e.to_string())?
                    .map(|v| v as u64),
                passive_window_seconds_override: cr
                    .try_get::<Option<i64>>("", "passive_window_seconds_override")
                    .map_err(|e| e.to_string())?
                    .map(|v| v as u64),
                passive_rate_limit_cooldown_seconds_override: cr
                    .try_get::<Option<i64>>("", "passive_rate_limit_cooldown_seconds_override")
                    .map_err(|e| e.to_string())?
                    .map(|v| v as u64),
                _healthy: None,
                _last_success_at: None,
                _health_status: None,
            });
        }

        let transforms_raw: String = row
            .try_get("", "transforms")
            .map_err(|e| format!("provider {id} missing transforms column: {e}"))?;
        let transforms: Vec<TransformRuleConfig> = serde_json::from_str(&transforms_raw)
            .map_err(|e| format!("provider {id} invalid transforms JSON: {e}"))?;
        let api_type_overrides_raw: String = row
            .try_get("", "api_type_overrides")
            .map_err(|e| format!("provider {id} missing api_type_overrides column: {e}"))?;
        let api_type_overrides: Vec<ApiTypeOverride> =
            serde_json::from_str(&api_type_overrides_raw)
                .map_err(|e| format!("provider {id} invalid api_type_overrides JSON: {e}"))?;
        let active_probe_enabled_override: Option<bool> = row
            .try_get::<Option<i32>>("", "active_probe_enabled_override")
            .map_err(|e| format!("provider {id} invalid active_probe_enabled_override: {e}"))?
            .map(|v| v != 0);
        let active_probe_interval_seconds_override: Option<u64> = row
            .try_get::<Option<i64>>("", "active_probe_interval_seconds_override")
            .map_err(|e| {
                format!("provider {id} invalid active_probe_interval_seconds_override: {e}")
            })?
            .map(|v| v as u64);
        let active_probe_success_threshold_override: Option<u32> = row
            .try_get::<Option<i64>>("", "active_probe_success_threshold_override")
            .map_err(|e| {
                format!("provider {id} invalid active_probe_success_threshold_override: {e}")
            })?
            .map(|v| v as u32);
        let active_probe_model_override: Option<String> = row
            .try_get("", "active_probe_model_override")
            .map_err(|e| format!("provider {id} invalid active_probe_model_override: {e}"))?;
        let request_timeout_ms_override: Option<u64> = row
            .try_get::<Option<i64>>("", "request_timeout_ms_override")
            .map_err(|e| format!("provider {id} invalid request_timeout_ms_override: {e}"))?
            .map(|v| v as u64);
        let extra_fields_whitelist: Option<Vec<String>> = row
            .try_get::<Option<String>>("", "extra_fields_whitelist")
            .unwrap_or(None)
            .and_then(|raw| serde_json::from_str(&raw).ok());
        let strip_cross_protocol_nested_extra: Option<bool> = row
            .try_get::<Option<i32>>("", "strip_cross_protocol_nested_extra")
            .unwrap_or(None)
            .map(|v| v != 0);
        let groups_raw: String = row
            .try_get("", "groups")
            .unwrap_or_else(|_| "[]".to_string());
        let groups = parse_provider_groups_json(&groups_raw);

        let created_at_str: String = row.try_get("", "created_at").map_err(|e| e.to_string())?;
        let updated_at_str: String = row.try_get("", "updated_at").map_err(|e| e.to_string())?;

        Ok(MonoizeProvider {
            id: id.clone(),
            name: row.try_get("", "name").map_err(|e| e.to_string())?,
            provider_type,
            models,
            channels,
            max_retries: row.try_get("", "max_retries").map_err(|e| e.to_string())?,
            channel_max_retries: row
                .try_get("", "channel_max_retries")
                .map_err(|e| e.to_string())?,
            channel_retry_interval_ms: row
                .try_get::<i32>("", "channel_retry_interval_ms")
                .map_err(|e| e.to_string())? as u64,
            circuit_breaker_enabled: row
                .try_get::<i32>("", "circuit_breaker_enabled")
                .map_err(|e| e.to_string())?
                != 0,
            per_model_circuit_break: row
                .try_get::<i32>("", "per_model_circuit_break")
                .map_err(|e| e.to_string())?
                != 0,
            transforms,
            api_type_overrides,
            active_probe_enabled_override,
            active_probe_interval_seconds_override,
            active_probe_success_threshold_override,
            active_probe_model_override,
            request_timeout_ms_override,
            extra_fields_whitelist,
            strip_cross_protocol_nested_extra,
            groups,
            enabled: row
                .try_get::<i32>("", "enabled")
                .map_err(|e| e.to_string())?
                == 1,
            priority: row.try_get("", "priority").map_err(|e| e.to_string())?,
            created_at: DateTime::parse_from_rfc3339(&created_at_str)
                .map_err(|e| format!("provider {id} invalid created_at RFC3339: {e}"))?
                .with_timezone(&Utc),
            updated_at: DateTime::parse_from_rfc3339(&updated_at_str)
                .map_err(|e| format!("provider {id} invalid updated_at RFC3339: {e}"))?
                .with_timezone(&Utc),
        })
    }
}

fn opt_bool_to_value(v: Option<bool>) -> SeaValue {
    match v {
        Some(b) => SeaValue::Int(Some(if b { 1 } else { 0 })),
        None => SeaValue::Int(None),
    }
}

fn opt_u64_to_value(v: Option<u64>) -> SeaValue {
    match v {
        Some(n) => SeaValue::BigInt(Some(n as i64)),
        None => SeaValue::BigInt(None),
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
        if let Some(v) = c.passive_failure_count_threshold_override {
            if v < 1 {
                return Err(
                    "channel passive_failure_count_threshold_override must be >= 1".to_string(),
                );
            }
        }
        if let Some(v) = c.passive_cooldown_seconds_override {
            if v < 1 {
                return Err("channel passive_cooldown_seconds_override must be >= 1".to_string());
            }
        }
        if let Some(v) = c.passive_window_seconds_override {
            if v < 1 {
                return Err("channel passive_window_seconds_override must be >= 1".to_string());
            }
        }
        if let Some(v) = c.passive_rate_limit_cooldown_seconds_override {
            if v < 1 {
                return Err(
                    "channel passive_rate_limit_cooldown_seconds_override must be >= 1".to_string(),
                );
            }
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
    api_type_overrides: &[ApiTypeOverride],
) -> Result<(), String> {
    if name.trim().is_empty() {
        return Err("provider name must not be empty".to_string());
    }
    validate_models(models)?;
    validate_channels(channels, true)?;
    validate_api_type_overrides(api_type_overrides)?;
    Ok(())
}

fn validate_api_type_overrides(overrides: &[ApiTypeOverride]) -> Result<(), String> {
    for (idx, entry) in overrides.iter().enumerate() {
        if entry.pattern.trim().is_empty() {
            return Err(format!(
                "api_type_overrides[{idx}].pattern must not be empty"
            ));
        }
    }
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

/// Resolves the effective API type for a given model by evaluating api_type_overrides
/// in order. First matching glob pattern wins; falls back to the default provider_type.
pub fn resolve_effective_api_type(
    overrides: &[ApiTypeOverride],
    default_type: MonoizeProviderType,
    model: &str,
) -> MonoizeProviderType {
    for entry in overrides {
        if glob_match(&entry.pattern, model) {
            return entry.api_type;
        }
    }
    default_type
}

fn glob_match(pattern: &str, value: &str) -> bool {
    if pattern == "*" {
        return true;
    }
    let mut regex = String::from("^");
    for ch in pattern.chars() {
        match ch {
            '*' => regex.push_str(".*"),
            '?' => regex.push('.'),
            other => regex.push_str(&regex::escape(&other.to_string())),
        }
    }
    regex.push('$');
    regex::Regex::new(&regex)
        .map(|re| re.is_match(value))
        .unwrap_or(false)
}

pub async fn probe_channel_completion(
    client: &reqwest::Client,
    channel: &MonoizeChannel,
    timeout_ms: u64,
    model: &str,
    provider_type: MonoizeProviderType,
    api_type_overrides: &[ApiTypeOverride],
) -> (bool, Option<Value>) {
    let effective_type = resolve_effective_api_type(api_type_overrides, provider_type, model);
    let base = channel.base_url.trim_end_matches('/');
    let (url, body, extra_headers, use_google_api_key_header) =
        build_probe_request(base, model, effective_type);

    let mut request = client.post(&url).timeout(Duration::from_millis(timeout_ms));
    request = if use_google_api_key_header {
        request.header("x-goog-api-key", &channel.api_key)
    } else {
        request.bearer_auth(&channel.api_key)
    };
    for &(header_name, header_value) in extra_headers {
        request = request.header(header_name, header_value);
    }
    let result = request.json(&body).send().await;

    match result {
        Ok(resp) => {
            if !resp.status().is_success() {
                return (false, None);
            }
            let usage = match resp.json::<Value>().await {
                Ok(value) => extract_probe_usage(&value),
                Err(_) => None,
            };
            (true, usage)
        }
        Err(_) => (false, None),
    }
}

fn build_probe_request(
    base: &str,
    model: &str,
    effective_type: MonoizeProviderType,
) -> (String, Value, &'static [(&'static str, &'static str)], bool) {
    match effective_type {
        MonoizeProviderType::Responses => {
            let url = format!("{base}/v1/responses");
            let body = serde_json::json!({
                "model": model,
                "max_output_tokens": 16,
                "input": [{"type": "message", "role": "user", "content": [{"type": "input_text", "text": "hi"}]}]
            });
            (url, body, &[][..], false)
        }
        MonoizeProviderType::ChatCompletion => {
            let url = format!("{base}/v1/chat/completions");
            let body = serde_json::json!({
                "model": model,
                "max_tokens": 16,
                "messages": [{"role": "user", "content": "hi"}]
            });
            (url, body, &[][..], false)
        }
        MonoizeProviderType::Messages => {
            let url = format!("{base}/v1/messages");
            let body = serde_json::json!({
                "model": model,
                "max_tokens": 16,
                "messages": [{"role": "user", "content": "hi"}]
            });
            (url, body, &[("anthropic-version", "2023-06-01")][..], false)
        }
        MonoizeProviderType::Gemini => {
            let url = format!("{base}/v1beta/models/{model}:generateContent");
            let body = serde_json::json!({
                "contents": [{"role": "user", "parts": [{"text": "hi"}]}],
                "generationConfig": {"maxOutputTokens": 16}
            });
            (url, body, &[][..], true)
        }
        MonoizeProviderType::OpenaiImage => {
            let url = format!("{base}/v1/images/generations");
            let body = serde_json::json!({
                "model": model,
                "prompt": "test",
                "size": "1024x1024",
                "n": 1,
            });
            (url, body, &[][..], false)
        }
    }
}

fn extract_probe_usage(body: &Value) -> Option<Value> {
    if let Some(usage) = body.get("usage") {
        let prompt_tokens = usage
            .get("prompt_tokens")
            .and_then(Value::as_u64)
            .or_else(|| usage.get("input_tokens").and_then(Value::as_u64));
        let completion_tokens = usage
            .get("completion_tokens")
            .and_then(Value::as_u64)
            .or_else(|| usage.get("output_tokens").and_then(Value::as_u64));

        if let (Some(prompt_tokens), Some(completion_tokens)) = (prompt_tokens, completion_tokens) {
            return Some(
                json!({"prompt_tokens": prompt_tokens, "completion_tokens": completion_tokens}),
            );
        }
    }

    let usage = body.get("usageMetadata")?;
    let prompt_tokens = usage
        .get("promptTokenCount")
        .and_then(Value::as_u64)
        .or_else(|| usage.get("input_tokens").and_then(Value::as_u64));
    let completion_tokens = usage
        .get("candidatesTokenCount")
        .and_then(Value::as_u64)
        .or_else(|| usage.get("output_tokens").and_then(Value::as_u64));

    match (prompt_tokens, completion_tokens) {
        (Some(prompt_tokens), Some(completion_tokens)) => {
            Some(json!({"prompt_tokens": prompt_tokens, "completion_tokens": completion_tokens}))
        }
        _ => None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn probe_request_plan_routes_each_api_type() {
        let (resp_url, resp_body, resp_headers, resp_google_auth) = build_probe_request(
            "https://up.example",
            "gpt-5-mini",
            MonoizeProviderType::Responses,
        );
        assert_eq!(resp_url, "https://up.example/v1/responses");
        assert!(!resp_google_auth);
        assert!(resp_headers.is_empty());
        assert_eq!(resp_body["max_output_tokens"].as_u64(), Some(16));
        assert!(resp_body.get("input").is_some());

        let (chat_url, chat_body, chat_headers, chat_google_auth) = build_probe_request(
            "https://up.example",
            "gpt-5-mini",
            MonoizeProviderType::ChatCompletion,
        );
        assert_eq!(chat_url, "https://up.example/v1/chat/completions");
        assert!(!chat_google_auth);
        assert!(chat_headers.is_empty());
        assert_eq!(chat_body["max_tokens"].as_u64(), Some(16));
        assert!(chat_body.get("messages").is_some());

        let (msg_url, msg_body, msg_headers, msg_google_auth) = build_probe_request(
            "https://up.example",
            "claude-3-7-sonnet",
            MonoizeProviderType::Messages,
        );
        assert_eq!(msg_url, "https://up.example/v1/messages");
        assert!(!msg_google_auth);
        assert_eq!(msg_headers, &[("anthropic-version", "2023-06-01")]);
        assert_eq!(msg_body["max_tokens"].as_u64(), Some(16));
        assert!(msg_body.get("messages").is_some());

        let (gem_url, gem_body, gem_headers, gem_google_auth) = build_probe_request(
            "https://up.example",
            "gemini-2.5-flash",
            MonoizeProviderType::Gemini,
        );
        assert_eq!(
            gem_url,
            "https://up.example/v1beta/models/gemini-2.5-flash:generateContent"
        );
        assert!(gem_google_auth);
        assert!(gem_headers.is_empty());
        assert_eq!(
            gem_body["generationConfig"]["maxOutputTokens"].as_u64(),
            Some(16)
        );
        assert!(gem_body.get("contents").is_some());

        let (img_url, img_body, img_headers, img_google_auth) = build_probe_request(
            "https://up.example",
            "gpt-image-1",
            MonoizeProviderType::OpenaiImage,
        );
        assert_eq!(img_url, "https://up.example/v1/images/generations");
        assert!(!img_google_auth);
        assert!(img_headers.is_empty());
        assert_eq!(img_body["model"].as_str(), Some("gpt-image-1"));
        assert_eq!(img_body["prompt"].as_str(), Some("test"));
        assert_eq!(img_body["size"].as_str(), Some("1024x1024"));
        assert_eq!(img_body["n"].as_u64(), Some(1));
    }

    #[test]
    fn extract_probe_usage_supports_gemini_usage_metadata() {
        let usage = extract_probe_usage(&json!({
            "usageMetadata": {
                "promptTokenCount": 12,
                "candidatesTokenCount": 8
            }
        }));
        assert_eq!(
            usage,
            Some(json!({"prompt_tokens": 12, "completion_tokens": 8}))
        );
    }

    #[test]
    fn validate_api_type_overrides_rejects_empty_pattern() {
        let err = validate_api_type_overrides(&[ApiTypeOverride {
            pattern: "   ".to_string(),
            api_type: MonoizeProviderType::ChatCompletion,
        }])
        .expect_err("expected invalid empty override pattern");
        assert!(err.contains("api_type_overrides[0].pattern must not be empty"));
    }

    #[test]
    fn parse_provider_groups_json_is_backward_compatible_for_empty_and_malformed_values() {
        assert!(parse_provider_groups_json("").is_empty());
        assert!(parse_provider_groups_json("not-json").is_empty());
        assert_eq!(
            parse_provider_groups_json(r#"[" Beta ","alpha","ALPHA",""]"#),
            vec!["alpha".to_string(), "beta".to_string()]
        );
    }
}
