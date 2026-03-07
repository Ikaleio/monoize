use crate::db::DbPool;
use crate::model_registry::{ModelCapabilities, ModelRecord};
use chrono::{DateTime, Utc};
use sea_orm::{ConnectionTrait, TransactionTrait};
use serde::{Deserialize, Serialize};
use serde_json::Value;

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DbModelRecord {
    pub id: String,
    pub logical_model: String,
    pub provider_id: String,
    pub upstream_model: String,
    pub capabilities: ModelCapabilities,
    pub enabled: bool,
    pub priority: i32,
    pub created_at: DateTime<Utc>,
    pub updated_at: DateTime<Utc>,
}

impl DbModelRecord {
    pub fn to_model_record(&self) -> ModelRecord {
        ModelRecord {
            logical_model: self.logical_model.clone(),
            provider_id: self.provider_id.clone(),
            upstream_model: self.upstream_model.clone(),
            capabilities: self.capabilities.clone(),
        }
    }
}

#[derive(Debug, Clone, Deserialize)]
pub struct CreateModelInput {
    pub id: Option<String>,
    pub logical_model: String,
    pub provider_id: String,
    pub upstream_model: String,
    pub capabilities: ModelCapabilities,
    #[serde(default = "default_enabled")]
    pub enabled: bool,
    #[serde(default)]
    pub priority: i32,
}

fn default_enabled() -> bool {
    true
}

#[derive(Debug, Clone, Deserialize)]
pub struct UpdateModelInput {
    pub logical_model: Option<String>,
    pub provider_id: Option<String>,
    pub upstream_model: Option<String>,
    pub capabilities: Option<ModelCapabilities>,
    pub enabled: Option<bool>,
    pub priority: Option<i32>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DbModelMetadataRecord {
    pub model_id: String,
    pub models_dev_provider: Option<String>,
    pub mode: Option<String>,
    pub input_cost_per_token_nano: Option<String>,
    pub output_cost_per_token_nano: Option<String>,
    pub cache_read_input_cost_per_token_nano: Option<String>,
    pub cache_creation_input_cost_per_token_nano: Option<String>,
    pub output_cost_per_reasoning_token_nano: Option<String>,
    pub max_input_tokens: Option<i64>,
    pub max_output_tokens: Option<i64>,
    pub max_tokens: Option<i64>,
    pub raw_json: Value,
    pub source: String,
    pub updated_at: DateTime<Utc>,
}

#[derive(Debug, Clone)]
pub struct ModelPricing {
    pub input_cost_per_token_nano: i128,
    pub output_cost_per_token_nano: i128,
    pub cache_read_input_cost_per_token_nano: Option<i128>,
    pub cache_creation_input_cost_per_token_nano: Option<i128>,
    pub output_cost_per_reasoning_token_nano: Option<i128>,
}

#[derive(Debug, Clone, Deserialize)]
pub struct UpsertModelMetadataInput {
    pub models_dev_provider: Option<String>,
    pub mode: Option<String>,
    pub input_cost_per_token_nano: Option<String>,
    pub output_cost_per_token_nano: Option<String>,
    pub cache_read_input_cost_per_token_nano: Option<String>,
    pub cache_creation_input_cost_per_token_nano: Option<String>,
    pub output_cost_per_reasoning_token_nano: Option<String>,
    pub max_input_tokens: Option<i64>,
    pub max_output_tokens: Option<i64>,
    pub max_tokens: Option<i64>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ModelMetadataSyncResult {
    pub success: bool,
    pub upserted: usize,
    pub skipped: usize,
    pub deleted: u64,
    pub fetched_at: String,
}

#[derive(Clone)]
pub struct ModelRegistryStore {
    db: DbPool,
}

impl ModelRegistryStore {
    pub async fn new(db: DbPool) -> Result<Self, String> {
        Ok(Self { db })
    }

    pub async fn list_models(&self) -> Result<Vec<DbModelRecord>, String> {
        let rows = self
            .db
            .read()
            .query_all(self.db.stmt(
                "SELECT id, logical_model, provider_id, upstream_model, capabilities_json,
                        enabled, priority, created_at, updated_at
                 FROM model_registry_records
                 ORDER BY priority DESC, logical_model ASC",
                vec![],
            ))
            .await
            .map_err(|e| e.to_string())?;

        rows.iter().map(row_to_record).collect()
    }

    pub async fn list_enabled_models(&self) -> Result<Vec<DbModelRecord>, String> {
        let rows = self
            .db
            .read()
            .query_all(self.db.stmt(
                "SELECT id, logical_model, provider_id, upstream_model, capabilities_json,
                        enabled, priority, created_at, updated_at
                 FROM model_registry_records
                 WHERE enabled = 1
                 ORDER BY priority DESC, logical_model ASC",
                vec![],
            ))
            .await
            .map_err(|e| e.to_string())?;

        rows.iter().map(row_to_record).collect()
    }

    pub async fn get_model(&self, id: &str) -> Result<Option<DbModelRecord>, String> {
        let row = self
            .db
            .read()
            .query_one(self.db.stmt(
                "SELECT id, logical_model, provider_id, upstream_model, capabilities_json,
                        enabled, priority, created_at, updated_at
                 FROM model_registry_records WHERE id = $1",
                vec![id.into()],
            ))
            .await
            .map_err(|e| e.to_string())?;

        match row {
            Some(r) => Ok(Some(row_to_record(&r)?)),
            None => Ok(None),
        }
    }

    pub async fn get_model_by_logical_and_provider(
        &self,
        logical_model: &str,
        provider_id: &str,
    ) -> Result<Option<DbModelRecord>, String> {
        let row = self
            .db
            .read()
            .query_one(self.db.stmt(
                "SELECT id, logical_model, provider_id, upstream_model, capabilities_json,
                        enabled, priority, created_at, updated_at
                 FROM model_registry_records
                 WHERE logical_model = $1 AND provider_id = $2",
                vec![logical_model.into(), provider_id.into()],
            ))
            .await
            .map_err(|e| e.to_string())?;

        match row {
            Some(r) => Ok(Some(row_to_record(&r)?)),
            None => Ok(None),
        }
    }

    pub async fn find_by_logical_model(
        &self,
        logical_model: &str,
    ) -> Result<Vec<DbModelRecord>, String> {
        let rows = self
            .db
            .read()
            .query_all(self.db.stmt(
                "SELECT id, logical_model, provider_id, upstream_model, capabilities_json,
                        enabled, priority, created_at, updated_at
                 FROM model_registry_records
                 WHERE logical_model = $1 AND enabled = 1
                 ORDER BY priority DESC",
                vec![logical_model.into()],
            ))
            .await
            .map_err(|e| e.to_string())?;

        rows.iter().map(row_to_record).collect()
    }

    pub async fn create_model(&self, input: CreateModelInput) -> Result<DbModelRecord, String> {
        let id = input.id.unwrap_or_else(|| {
            format!(
                "model_{}",
                uuid::Uuid::new_v4().to_string().replace("-", "")
            )
        });
        let now = Utc::now();
        let capabilities_json =
            serde_json::to_string(&input.capabilities).map_err(|e| e.to_string())?;
        let enabled_i: i32 = if input.enabled { 1 } else { 0 };

        self.db
            .write().await
            .execute(self.db.stmt(
                "INSERT INTO model_registry_records
                 (id, logical_model, provider_id, upstream_model, capabilities_json,
                  enabled, priority, created_at, updated_at)
                 VALUES ($1, $2, $3, $4, $5, $6, $7, $8, $9)",
                vec![
                    id.clone().into(),
                    input.logical_model.into(),
                    input.provider_id.into(),
                    input.upstream_model.into(),
                    capabilities_json.into(),
                    enabled_i.into(),
                    input.priority.into(),
                    now.to_rfc3339().into(),
                    now.to_rfc3339().into(),
                ],
            ))
            .await
            .map_err(|e| {
                let msg = e.to_string();
                if msg.contains("UNIQUE") || msg.contains("unique") || msg.contains("duplicate") {
                    "model_already_exists: a model with this logical_model and provider_id already exists".to_string()
                } else {
                    msg
                }
            })?;

        self.get_model(&id)
            .await?
            .ok_or_else(|| "model not found after creation".to_string())
    }

    pub async fn update_model(
        &self,
        id: &str,
        input: UpdateModelInput,
    ) -> Result<DbModelRecord, String> {
        let _existing = self
            .get_model(id)
            .await?
            .ok_or_else(|| "model not found".to_string())?;

        let now = Utc::now();
        let mut set_clauses = Vec::new();
        let mut values: Vec<sea_orm::Value> = Vec::new();
        let mut idx = 1u32;

        if let Some(logical_model) = &input.logical_model {
            set_clauses.push(format!("logical_model = ${idx}"));
            values.push(logical_model.clone().into());
            idx += 1;
        }
        if let Some(provider_id) = &input.provider_id {
            set_clauses.push(format!("provider_id = ${idx}"));
            values.push(provider_id.clone().into());
            idx += 1;
        }
        if let Some(upstream_model) = &input.upstream_model {
            set_clauses.push(format!("upstream_model = ${idx}"));
            values.push(upstream_model.clone().into());
            idx += 1;
        }
        if let Some(capabilities) = &input.capabilities {
            set_clauses.push(format!("capabilities_json = ${idx}"));
            values.push(
                serde_json::to_string(capabilities)
                    .map_err(|e| e.to_string())?
                    .into(),
            );
            idx += 1;
        }
        if let Some(enabled) = input.enabled {
            let v: i32 = if enabled { 1 } else { 0 };
            set_clauses.push(format!("enabled = ${idx}"));
            values.push(v.into());
            idx += 1;
        }
        if let Some(priority) = input.priority {
            set_clauses.push(format!("priority = ${idx}"));
            values.push(priority.into());
            idx += 1;
        }

        if !set_clauses.is_empty() {
            set_clauses.push(format!("updated_at = ${idx}"));
            values.push(now.to_rfc3339().into());
            idx += 1;

            values.push(id.to_string().into());

            let sql = format!(
                "UPDATE model_registry_records SET {} WHERE id = ${idx}",
                set_clauses.join(", ")
            );

            self.db
                .write().await
                .execute(self.db.stmt(&sql, values))
                .await
                .map_err(|e| {
                    let msg = e.to_string();
                    if msg.contains("UNIQUE")
                        || msg.contains("unique")
                        || msg.contains("duplicate")
                    {
                        "model_already_exists: a model with this logical_model and provider_id already exists".to_string()
                    } else {
                        msg
                    }
                })?;
        }

        self.get_model(id)
            .await?
            .ok_or_else(|| "model not found after update".to_string())
    }

    pub async fn delete_model(&self, id: &str) -> Result<(), String> {
        let result = self
            .db
            .write().await
            .execute(self.db.stmt(
                "DELETE FROM model_registry_records WHERE id = $1",
                vec![id.into()],
            ))
            .await
            .map_err(|e| e.to_string())?;

        if result.rows_affected() == 0 {
            return Err("model not found".to_string());
        }

        Ok(())
    }

    pub async fn list_model_metadata(&self) -> Result<Vec<DbModelMetadataRecord>, String> {
        let rows = self
            .db
            .read()
            .query_all(self.db.stmt(
                "SELECT model_id, models_dev_provider, mode, input_cost_per_token_nano,
                        output_cost_per_token_nano, cache_read_input_cost_per_token_nano,
                        cache_creation_input_cost_per_token_nano,
                        output_cost_per_reasoning_token_nano, max_input_tokens, max_output_tokens,
                        max_tokens, raw_json, source, updated_at
                 FROM model_metadata_records
                 ORDER BY model_id ASC",
                vec![],
            ))
            .await
            .map_err(|e| e.to_string())?;

        rows.iter().map(row_to_model_metadata).collect()
    }

    pub async fn list_priced_model_ids(&self) -> Result<std::collections::HashSet<String>, String> {
        let rows = self
            .db
            .read()
            .query_all(self.db.stmt(
                "SELECT model_id FROM model_metadata_records WHERE input_cost_per_token_nano IS NOT NULL AND output_cost_per_token_nano IS NOT NULL",
                vec![],
            ))
            .await
            .map_err(|e| e.to_string())?;

        let mut set = std::collections::HashSet::new();
        for row in &rows {
            let id: String = row.try_get("", "model_id").map_err(|e| e.to_string())?;
            set.insert(id);
        }
        Ok(set)
    }

    pub async fn get_model_metadata(
        &self,
        model_id: &str,
    ) -> Result<Option<DbModelMetadataRecord>, String> {
        let row = self
            .db
            .read()
            .query_one(self.db.stmt(
                "SELECT model_id, models_dev_provider, mode, input_cost_per_token_nano,
                        output_cost_per_token_nano, cache_read_input_cost_per_token_nano,
                        cache_creation_input_cost_per_token_nano,
                        output_cost_per_reasoning_token_nano, max_input_tokens, max_output_tokens,
                        max_tokens, raw_json, source, updated_at
                 FROM model_metadata_records
                 WHERE model_id = $1",
                vec![model_id.into()],
            ))
            .await
            .map_err(|e| e.to_string())?;

        match row {
            Some(r) => Ok(Some(row_to_model_metadata(&r)?)),
            None => Ok(None),
        }
    }

    pub async fn get_model_pricing(&self, model_id: &str) -> Result<Option<ModelPricing>, String> {
        let row = self.get_model_metadata(model_id).await?;
        let Some(row) = row else {
            return Ok(None);
        };
        let Some(input_raw) = row.input_cost_per_token_nano else {
            return Ok(None);
        };
        let Some(output_raw) = row.output_cost_per_token_nano else {
            return Ok(None);
        };
        let input_cost_per_token_nano = input_raw
            .parse::<i128>()
            .map_err(|_| "invalid input_cost_per_token_nano".to_string())?;
        let output_cost_per_token_nano = output_raw
            .parse::<i128>()
            .map_err(|_| "invalid output_cost_per_token_nano".to_string())?;
        let cache_read_input_cost_per_token_nano = row
            .cache_read_input_cost_per_token_nano
            .map(|v| v.parse::<i128>())
            .transpose()
            .map_err(|_| "invalid cache_read_input_cost_per_token_nano".to_string())?;
        let cache_creation_input_cost_per_token_nano = row
            .cache_creation_input_cost_per_token_nano
            .map(|v| v.parse::<i128>())
            .transpose()
            .map_err(|_| "invalid cache_creation_input_cost_per_token_nano".to_string())?;
        let output_cost_per_reasoning_token_nano = row
            .output_cost_per_reasoning_token_nano
            .map(|v| v.parse::<i128>())
            .transpose()
            .map_err(|_| "invalid output_cost_per_reasoning_token_nano".to_string())?;

        Ok(Some(ModelPricing {
            input_cost_per_token_nano,
            output_cost_per_token_nano,
            cache_read_input_cost_per_token_nano,
            cache_creation_input_cost_per_token_nano,
            output_cost_per_reasoning_token_nano,
        }))
    }

    pub async fn upsert_model_metadata(
        &self,
        model_id: &str,
        input: UpsertModelMetadataInput,
    ) -> Result<DbModelMetadataRecord, String> {
        let now = Utc::now().to_rfc3339();
        self.db
            .write().await
            .execute(self.db.stmt(
                "INSERT INTO model_metadata_records
                 (model_id, models_dev_provider, mode, input_cost_per_token_nano, output_cost_per_token_nano,
                  cache_read_input_cost_per_token_nano, cache_creation_input_cost_per_token_nano, output_cost_per_reasoning_token_nano,
                  max_input_tokens, max_output_tokens, max_tokens, raw_json, source, updated_at)
                 VALUES ($1, $2, $3, $4, $5, $6, $7, $8, $9, $10, $11, '{}', 'manual', $12)
                 ON CONFLICT(model_id) DO UPDATE SET
                   models_dev_provider = COALESCE($13, model_metadata_records.models_dev_provider),
                   mode = COALESCE($14, model_metadata_records.mode),
                   input_cost_per_token_nano = $15,
                   output_cost_per_token_nano = $16,
                   cache_read_input_cost_per_token_nano = $17,
                   cache_creation_input_cost_per_token_nano = $18,
                   output_cost_per_reasoning_token_nano = $19,
                   max_input_tokens = COALESCE($20, model_metadata_records.max_input_tokens),
                   max_output_tokens = COALESCE($21, model_metadata_records.max_output_tokens),
                   max_tokens = COALESCE($22, model_metadata_records.max_tokens),
                   source = 'manual',
                   updated_at = $23",
                vec![
                    // INSERT binds
                    model_id.into(),
                    input.models_dev_provider.clone().into(),
                    input.mode.clone().into(),
                    input.input_cost_per_token_nano.clone().into(),
                    input.output_cost_per_token_nano.clone().into(),
                    input.cache_read_input_cost_per_token_nano.clone().into(),
                    input.cache_creation_input_cost_per_token_nano.clone().into(),
                    input.output_cost_per_reasoning_token_nano.clone().into(),
                    input.max_input_tokens.into(),
                    input.max_output_tokens.into(),
                    input.max_tokens.into(),
                    now.clone().into(),
                    // UPDATE binds
                    input.models_dev_provider.into(),
                    input.mode.into(),
                    input.input_cost_per_token_nano.into(),
                    input.output_cost_per_token_nano.into(),
                    input.cache_read_input_cost_per_token_nano.into(),
                    input.cache_creation_input_cost_per_token_nano.into(),
                    input.output_cost_per_reasoning_token_nano.into(),
                    input.max_input_tokens.into(),
                    input.max_output_tokens.into(),
                    input.max_tokens.into(),
                    now.into(),
                ],
            ))
            .await
            .map_err(|e| e.to_string())?;

        self.get_model_metadata(model_id)
            .await?
            .ok_or_else(|| "upsert succeeded but record not found".to_string())
    }

    pub async fn delete_model_metadata(&self, model_id: &str) -> Result<bool, String> {
        let result = self
            .db
            .write().await
            .execute(self.db.stmt(
                "DELETE FROM model_metadata_records WHERE model_id = $1",
                vec![model_id.into()],
            ))
            .await
            .map_err(|e| e.to_string())?;
        Ok(result.rows_affected() > 0)
    }

    pub async fn sync_from_models_dev(
        &self,
        http: &reqwest::Client,
    ) -> Result<ModelMetadataSyncResult, String> {
        const MODELS_DEV_URL: &str = "https://models.dev/api.json";
        let resp = http
            .get(MODELS_DEV_URL)
            .send()
            .await
            .map_err(|e| format!("fetch_failed: {e}"))?;
        let status = resp.status();
        if !status.is_success() {
            return Err(format!("fetch_failed: status={status}"));
        }
        let text = resp
            .text()
            .await
            .map_err(|e| format!("fetch_failed: {e}"))?;
        let root: Value = serde_json::from_str(&text).map_err(|e| format!("parse_failed: {e}"))?;
        let providers = root
            .as_object()
            .ok_or_else(|| "parse_failed: root must be object".to_string())?;

        let mut grouped: std::collections::HashMap<String, Vec<SyncProviderVariant>> =
            std::collections::HashMap::new();

        for (provider_id, provider_val) in providers {
            let provider_obj = match provider_val.as_object() {
                Some(v) => v,
                None => continue,
            };
            let models = match provider_obj.get("models").and_then(|m| m.as_object()) {
                Some(v) => v,
                None => continue,
            };
            for (model_name, model_val) in models {
                let model_obj = match model_val.as_object() {
                    Some(v) => v,
                    None => continue,
                };
                let cost = model_obj.get("cost").and_then(|c| c.as_object());
                let limit = model_obj.get("limit").and_then(|l| l.as_object());
                let canonical = normalize_model_id(model_name, Some(provider_id));
                if should_ignore_sync_model(&canonical) {
                    continue;
                }
                grouped
                    .entry(canonical)
                    .or_default()
                    .push(SyncProviderVariant {
                        provider_id: provider_id.clone(),
                        family: model_obj
                            .get("family")
                            .and_then(|f| f.as_str())
                            .map(|s| s.to_string()),
                        input_cost_nano: cost
                            .and_then(|c| c.get("input"))
                            .and_then(cost_per_1m_to_nano_string),
                        output_cost_nano: cost
                            .and_then(|c| c.get("output"))
                            .and_then(cost_per_1m_to_nano_string),
                        cache_read_nano: cost
                            .and_then(|c| c.get("cache_read"))
                            .and_then(cost_per_1m_to_nano_string),
                        cache_write_nano: cost
                            .and_then(|c| c.get("cache_write"))
                            .and_then(cost_per_1m_to_nano_string),
                        reasoning_nano: cost
                            .and_then(|c| c.get("reasoning"))
                            .and_then(cost_per_1m_to_nano_string),
                        max_input_tokens: limit.and_then(|l| l.get("input")).and_then(value_to_i64),
                        max_output_tokens: limit
                            .and_then(|l| l.get("output"))
                            .and_then(value_to_i64),
                        max_tokens: limit.and_then(|l| l.get("context")).and_then(value_to_i64),
                        raw: model_val.clone(),
                    });
            }
        }

        let fetched_at = Utc::now().to_rfc3339();
        let _write_guard = self.db.write().await;
        let txn = _write_guard
            .begin()
            .await
            .map_err(|e| e.to_string())?;

        let del_result = txn
            .execute(self.db.stmt(
                "DELETE FROM model_metadata_records WHERE source != 'manual'",
                vec![],
            ))
            .await
            .map_err(|e| e.to_string())?;
        let deleted = del_result.rows_affected();

        let mut upserted = 0usize;
        let mut skipped = 0usize;

        for (model_name, variants) in &grouped {
            if !has_positive_input_variant(variants) {
                continue;
            }

            let existing_row = txn
                .query_one(self.db.stmt(
                    "SELECT source FROM model_metadata_records WHERE model_id = $1",
                    vec![model_name.clone().into()],
                ))
                .await
                .map_err(|e| e.to_string())?;

            let existing_source: Option<String> = existing_row
                .and_then(|r| r.try_get::<String>("", "source").ok());

            if existing_source.as_deref() == Some("manual") {
                skipped += 1;
                continue;
            }

            let winner = pick_best_variant(variants);
            let mode = if variants
                .iter()
                .any(|v| is_embedding_family(v.family.as_deref()))
            {
                "embedding"
            } else {
                "chat"
            };

            let mut providers_map = serde_json::Map::new();
            for v in variants {
                providers_map.insert(v.provider_id.clone(), v.raw.clone());
            }
            let raw_json = serde_json::json!({ "providers": providers_map });

            txn.execute(self.db.stmt(
                "INSERT INTO model_metadata_records
                 (model_id, models_dev_provider, mode, input_cost_per_token_nano, output_cost_per_token_nano,
                  cache_read_input_cost_per_token_nano, cache_creation_input_cost_per_token_nano, output_cost_per_reasoning_token_nano,
                  max_input_tokens, max_output_tokens, max_tokens, raw_json, source, updated_at)
                 VALUES ($1, $2, $3, $4, $5, $6, $7, $8, $9, $10, $11, $12, 'models_dev', $13)
                 ON CONFLICT(model_id) DO UPDATE SET
                   models_dev_provider=excluded.models_dev_provider,
                   mode=excluded.mode,
                   input_cost_per_token_nano=excluded.input_cost_per_token_nano,
                   output_cost_per_token_nano=excluded.output_cost_per_token_nano,
                   cache_read_input_cost_per_token_nano=excluded.cache_read_input_cost_per_token_nano,
                   cache_creation_input_cost_per_token_nano=excluded.cache_creation_input_cost_per_token_nano,
                   output_cost_per_reasoning_token_nano=excluded.output_cost_per_reasoning_token_nano,
                   max_input_tokens=excluded.max_input_tokens,
                   max_output_tokens=excluded.max_output_tokens,
                   max_tokens=excluded.max_tokens,
                   raw_json=excluded.raw_json,
                   source=excluded.source,
                   updated_at=excluded.updated_at",
                vec![
                    model_name.clone().into(),
                    winner.provider_id.clone().into(),
                    mode.into(),
                    winner.input_cost_nano.clone().into(),
                    winner.output_cost_nano.clone().into(),
                    winner.cache_read_nano.clone().into(),
                    winner.cache_write_nano.clone().into(),
                    winner.reasoning_nano.clone().into(),
                    winner.max_input_tokens.into(),
                    winner.max_output_tokens.into(),
                    winner.max_tokens.into(),
                    raw_json.to_string().into(),
                    fetched_at.clone().into(),
                ],
            ))
            .await
            .map_err(|e| e.to_string())?;
            upserted += 1;
        }

        txn.commit().await.map_err(|e| e.to_string())?;
        Ok(ModelMetadataSyncResult {
            success: true,
            upserted,
            skipped,
            deleted,
            fetched_at,
        })
    }
}

fn row_to_record(row: &sea_orm::QueryResult) -> Result<DbModelRecord, String> {
    let capabilities_json: String = row
        .try_get("", "capabilities_json")
        .map_err(|e| e.to_string())?;
    let capabilities: ModelCapabilities =
        serde_json::from_str(&capabilities_json).map_err(|e| e.to_string())?;

    let created_at_str: String = row.try_get("", "created_at").map_err(|e| e.to_string())?;
    let created_at = DateTime::parse_from_rfc3339(&created_at_str)
        .map_err(|e| e.to_string())?
        .with_timezone(&Utc);

    let updated_at_str: String = row.try_get("", "updated_at").map_err(|e| e.to_string())?;
    let updated_at = DateTime::parse_from_rfc3339(&updated_at_str)
        .map_err(|e| e.to_string())?
        .with_timezone(&Utc);

    let enabled_i: i32 = row.try_get("", "enabled").map_err(|e| e.to_string())?;

    Ok(DbModelRecord {
        id: row.try_get("", "id").map_err(|e| e.to_string())?,
        logical_model: row
            .try_get("", "logical_model")
            .map_err(|e| e.to_string())?,
        provider_id: row
            .try_get("", "provider_id")
            .map_err(|e| e.to_string())?,
        upstream_model: row
            .try_get("", "upstream_model")
            .map_err(|e| e.to_string())?,
        capabilities,
        enabled: enabled_i == 1,
        priority: row.try_get("", "priority").map_err(|e| e.to_string())?,
        created_at,
        updated_at,
    })
}

fn row_to_model_metadata(row: &sea_orm::QueryResult) -> Result<DbModelMetadataRecord, String> {
    let updated_at_str: String = row.try_get("", "updated_at").map_err(|e| e.to_string())?;
    let updated_at = DateTime::parse_from_rfc3339(&updated_at_str)
        .map_err(|e| e.to_string())?
        .with_timezone(&Utc);
    let raw_json_str: String = row.try_get("", "raw_json").map_err(|e| e.to_string())?;
    let raw_json: Value = serde_json::from_str(&raw_json_str).map_err(|e| e.to_string())?;

    Ok(DbModelMetadataRecord {
        model_id: row.try_get("", "model_id").map_err(|e| e.to_string())?,
        models_dev_provider: row.try_get("", "models_dev_provider").unwrap_or(None),
        mode: row.try_get("", "mode").unwrap_or(None),
        input_cost_per_token_nano: row
            .try_get("", "input_cost_per_token_nano")
            .unwrap_or(None),
        output_cost_per_token_nano: row
            .try_get("", "output_cost_per_token_nano")
            .unwrap_or(None),
        cache_read_input_cost_per_token_nano: row
            .try_get("", "cache_read_input_cost_per_token_nano")
            .unwrap_or(None),
        cache_creation_input_cost_per_token_nano: row
            .try_get("", "cache_creation_input_cost_per_token_nano")
            .unwrap_or(None),
        output_cost_per_reasoning_token_nano: row
            .try_get("", "output_cost_per_reasoning_token_nano")
            .unwrap_or(None),
        max_input_tokens: row.try_get("", "max_input_tokens").unwrap_or(None),
        max_output_tokens: row.try_get("", "max_output_tokens").unwrap_or(None),
        max_tokens: row.try_get("", "max_tokens").unwrap_or(None),
        raw_json,
        source: row
            .try_get("", "source")
            .unwrap_or_else(|_| "models_dev".to_string()),
        updated_at,
    })
}

struct SyncProviderVariant {
    provider_id: String,
    family: Option<String>,
    input_cost_nano: Option<String>,
    output_cost_nano: Option<String>,
    cache_read_nano: Option<String>,
    cache_write_nano: Option<String>,
    reasoning_nano: Option<String>,
    max_input_tokens: Option<i64>,
    max_output_tokens: Option<i64>,
    max_tokens: Option<i64>,
    raw: Value,
}

fn pick_best_variant(variants: &[SyncProviderVariant]) -> &SyncProviderVariant {
    variants
        .iter()
        .max_by(|a, b| {
            let cost_a = a
                .input_cost_nano
                .as_ref()
                .and_then(|s| s.parse::<i128>().ok())
                .filter(|&v| v > 0)
                .unwrap_or(0);
            let cost_b = b
                .input_cost_nano
                .as_ref()
                .and_then(|s| s.parse::<i128>().ok())
                .filter(|&v| v > 0)
                .unwrap_or(0);
            cost_a.cmp(&cost_b)
        })
        .expect("pick_best_variant called with at least one sync variant")
}

fn has_positive_input_variant(variants: &[SyncProviderVariant]) -> bool {
    variants.iter().any(|v| {
        v.input_cost_nano
            .as_ref()
            .and_then(|s| s.parse::<i128>().ok())
            .is_some_and(|n| n > 0)
    })
}

fn should_ignore_sync_model(model_id: &str) -> bool {
    model_id == "auto"
        || model_id.ends_with("-thinking")
        || model_id.ends_with(":thinking")
        || model_id.ends_with("-think")
}

fn is_embedding_family(family: Option<&str>) -> bool {
    family
        .map(|s| s.to_ascii_lowercase().contains("embed"))
        .unwrap_or(false)
}

fn cost_per_1m_to_nano_string(value: &Value) -> Option<String> {
    let f = value.as_f64()?;
    if !f.is_finite() {
        return None;
    }
    let nano = (f * 1000.0) as i128;
    Some(nano.to_string())
}

fn value_to_i64(value: &Value) -> Option<i64> {
    value
        .as_i64()
        .or_else(|| value.as_u64().and_then(|v| i64::try_from(v).ok()))
        .or_else(|| value.as_f64().map(|v| v as i64))
}

const KNOWN_PROVIDER_PREFIXES: &[&str] = &[
    "openai",
    "anthropic",
    "google",
    "xai",
    "mistral",
    "deepseek",
    "cohere",
    "meta",
    "minimax",
    "perplexity",
    "stepfun",
    "zhipuai",
    "nvidia",
    "moonshotai",
    "alibaba",
    "amazon-bedrock",
    "vercel",
    "openrouter",
    "azure",
    "groq",
    "fireworks",
    "together",
    "cloudflare",
    "replicate",
];

fn strip_provider_prefix_once<'a>(segment: &'a str, provider: &str) -> Option<&'a str> {
    let mut dd = String::with_capacity(provider.len() + 2);
    dd.push_str(provider);
    dd.push_str("--");
    if let Some(rest) = segment.strip_prefix(&dd) {
        return Some(rest);
    }

    let mut dot = String::with_capacity(provider.len() + 1);
    dot.push_str(provider);
    dot.push('.');
    segment.strip_prefix(&dot)
}

fn is_known_provider_prefix(prefix: &str) -> bool {
    KNOWN_PROVIDER_PREFIXES.contains(&prefix)
}

pub fn normalize_model_id(raw: &str, provider_hint: Option<&str>) -> String {
    let mut segment = raw.rsplit('/').next().unwrap_or(raw).to_ascii_lowercase();

    if let Some(hint) = provider_hint {
        let hint = hint.to_ascii_lowercase();
        if let Some(rest) = strip_provider_prefix_once(&segment, &hint) {
            segment = rest.to_string();
        }
    }

    if let Some((prefix, _)) = segment.split_once("--") {
        if is_known_provider_prefix(prefix) {
            if let Some(rest) = strip_provider_prefix_once(&segment, prefix) {
                segment = rest.to_string();
            }
        }
    }

    if let Some((prefix, _)) = segment.split_once('.') {
        if is_known_provider_prefix(prefix) {
            if let Some(rest) = strip_provider_prefix_once(&segment, prefix) {
                segment = rest.to_string();
            }
        }
    }

    segment
}
