use crate::db::DbPool;
use crate::model_registry::{ModelCapabilities, ModelRecord};
use chrono::{DateTime, Utc};
use rust_decimal::Decimal;
use rust_decimal::prelude::ToPrimitive;
use sea_orm::{ConnectionTrait, DatabaseTransaction, TransactionTrait};
use serde::{Deserialize, Deserializer, Serialize};
use serde_json::Value;
use std::str::FromStr;

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
    #[serde(default, deserialize_with = "deserialize_nullable_field")]
    pub models_dev_provider: Option<Option<String>>,
    #[serde(default, deserialize_with = "deserialize_nullable_field")]
    pub mode: Option<Option<String>>,
    #[serde(default, deserialize_with = "deserialize_nullable_field")]
    pub input_cost_per_token_nano: Option<Option<String>>,
    #[serde(default, deserialize_with = "deserialize_nullable_field")]
    pub output_cost_per_token_nano: Option<Option<String>>,
    #[serde(default, deserialize_with = "deserialize_nullable_field")]
    pub cache_read_input_cost_per_token_nano: Option<Option<String>>,
    #[serde(default, deserialize_with = "deserialize_nullable_field")]
    pub cache_creation_input_cost_per_token_nano: Option<Option<String>>,
    #[serde(default, deserialize_with = "deserialize_nullable_field")]
    pub output_cost_per_reasoning_token_nano: Option<Option<String>>,
    #[serde(default, deserialize_with = "deserialize_nullable_field")]
    pub max_input_tokens: Option<Option<i64>>,
    #[serde(default, deserialize_with = "deserialize_nullable_field")]
    pub max_output_tokens: Option<Option<i64>>,
    #[serde(default, deserialize_with = "deserialize_nullable_field")]
    pub max_tokens: Option<Option<i64>>,
}

fn deserialize_nullable_field<'de, D, T>(deserializer: D) -> Result<Option<Option<T>>, D::Error>
where
    D: Deserializer<'de>,
    T: Deserialize<'de>,
{
    Option::<T>::deserialize(deserializer).map(Some)
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ModelMetadataSyncResult {
    pub success: bool,
    pub upserted: usize,
    pub skipped: usize,
    pub deleted: u64,
    pub fetched_at: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct BatchDeleteModelMetadataResult {
    pub success: bool,
    pub deleted: usize,
    pub not_found: Vec<String>,
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

            let result = self
                .db
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
            if result.rows_affected() == 0 {
                return Err("model not found".to_string());
            }
        }

        self.get_model(id)
            .await?
            .ok_or_else(|| "model not found after update".to_string())
    }

    pub async fn delete_model(&self, id: &str) -> Result<(), String> {
        let result = self
            .db
            .write()
            .await
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
                 ORDER BY CASE WHEN source = 'manual' THEN 0 ELSE 1 END, model_id ASC",
                vec![],
            ))
            .await
            .map_err(|e| e.to_string())?;

        rows.iter().map(row_to_model_metadata).collect()
    }

    pub async fn list_marketplace_model_metadata(
        &self,
    ) -> Result<Vec<DbModelMetadataRecord>, String> {
        let rows = self
            .db
            .read()
            .query_all(self.db.stmt(
                "SELECT DISTINCT
                        m.model_id, m.models_dev_provider, m.mode,
                        m.input_cost_per_token_nano, m.output_cost_per_token_nano,
                        m.cache_read_input_cost_per_token_nano,
                        m.cache_creation_input_cost_per_token_nano,
                        m.output_cost_per_reasoning_token_nano, m.max_input_tokens,
                        m.max_output_tokens, m.max_tokens, m.raw_json, m.source, m.updated_at
                 FROM model_metadata_records AS m
                 INNER JOIN monoize_channel_models AS cm ON cm.model_name = m.model_id
                 INNER JOIN monoize_channels AS c ON c.id = cm.channel_id
                 INNER JOIN monoize_providers AS p ON p.id = c.provider_id
                 WHERE p.enabled = 1
                   AND c.enabled = 1
                   AND c.weight > 0
                 ORDER BY m.model_id ASC",
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

    pub async fn list_model_metadata_pricing_profiles(
        &self,
        model_ids: &[String],
    ) -> Result<std::collections::HashMap<String, String>, String> {
        if model_ids.is_empty() {
            return Ok(std::collections::HashMap::new());
        }
        let model_ids = model_ids
            .iter()
            .cloned()
            .collect::<std::collections::BTreeSet<_>>()
            .into_iter()
            .collect::<Vec<_>>();
        let mut profiles = std::collections::HashMap::new();
        const LOOKUP_CHUNK_SIZE: usize = 400;
        for chunk in model_ids.chunks(LOOKUP_CHUNK_SIZE) {
            let placeholders = (0..chunk.len())
                .map(|index| format!("${}", index + 1))
                .collect::<Vec<_>>()
                .join(", ");
            let rows = self
                .db
                .read()
                .query_all(self.db.stmt(
                    &format!(
                        "SELECT model_id, models_dev_provider
                         FROM model_metadata_records
                         WHERE model_id IN ({placeholders})
                           AND models_dev_provider IS NOT NULL"
                    ),
                    chunk.iter().cloned().map(Into::into).collect(),
                ))
                .await
                .map_err(|e| e.to_string())?;
            for row in rows {
                let model_id: String = row.try_get("", "model_id").map_err(|e| e.to_string())?;
                let profile: String = row
                    .try_get("", "models_dev_provider")
                    .map_err(|e| e.to_string())?;
                let profile = profile.trim();
                if !profile.is_empty() {
                    profiles.insert(model_id, profile.to_string());
                }
            }
        }
        Ok(profiles)
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
        validate_optional_price(
            "input_cost_per_token_nano",
            input.input_cost_per_token_nano.as_ref(),
        )?;
        validate_optional_price(
            "output_cost_per_token_nano",
            input.output_cost_per_token_nano.as_ref(),
        )?;
        validate_optional_price(
            "cache_read_input_cost_per_token_nano",
            input.cache_read_input_cost_per_token_nano.as_ref(),
        )?;
        validate_optional_price(
            "cache_creation_input_cost_per_token_nano",
            input.cache_creation_input_cost_per_token_nano.as_ref(),
        )?;
        validate_optional_price(
            "output_cost_per_reasoning_token_nano",
            input.output_cost_per_reasoning_token_nano.as_ref(),
        )?;

        let now = Utc::now().to_rfc3339();
        let write_guard = self.db.write().await;
        let txn = write_guard.begin().await.map_err(|e| e.to_string())?;
        if self.db.is_postgres() {
            txn.execute_unprepared("LOCK TABLE model_metadata_records IN SHARE ROW EXCLUSIVE MODE")
                .await
                .map_err(|e| e.to_string())?;
        }
        let existing = get_model_metadata_with(&self.db, &txn, model_id).await?;

        let models_dev_provider = merge_nullable(
            input.models_dev_provider,
            existing
                .as_ref()
                .and_then(|record| record.models_dev_provider.clone()),
        );
        let mode = merge_nullable(
            input.mode,
            existing.as_ref().and_then(|record| record.mode.clone()),
        );
        let input_cost_per_token_nano = merge_nullable(
            input.input_cost_per_token_nano,
            existing
                .as_ref()
                .and_then(|record| record.input_cost_per_token_nano.clone()),
        );
        let output_cost_per_token_nano = merge_nullable(
            input.output_cost_per_token_nano,
            existing
                .as_ref()
                .and_then(|record| record.output_cost_per_token_nano.clone()),
        );
        let cache_read_input_cost_per_token_nano = merge_nullable(
            input.cache_read_input_cost_per_token_nano,
            existing
                .as_ref()
                .and_then(|record| record.cache_read_input_cost_per_token_nano.clone()),
        );
        let cache_creation_input_cost_per_token_nano = merge_nullable(
            input.cache_creation_input_cost_per_token_nano,
            existing
                .as_ref()
                .and_then(|record| record.cache_creation_input_cost_per_token_nano.clone()),
        );
        let output_cost_per_reasoning_token_nano = merge_nullable(
            input.output_cost_per_reasoning_token_nano,
            existing
                .as_ref()
                .and_then(|record| record.output_cost_per_reasoning_token_nano.clone()),
        );
        let max_input_tokens = merge_nullable(
            input.max_input_tokens,
            existing.as_ref().and_then(|record| record.max_input_tokens),
        );
        let max_output_tokens = merge_nullable(
            input.max_output_tokens,
            existing
                .as_ref()
                .and_then(|record| record.max_output_tokens),
        );
        let max_tokens = merge_nullable(
            input.max_tokens,
            existing.as_ref().and_then(|record| record.max_tokens),
        );

        txn.execute(self.db.stmt(
                "INSERT INTO model_metadata_records
                 (model_id, models_dev_provider, mode, input_cost_per_token_nano, output_cost_per_token_nano,
                  cache_read_input_cost_per_token_nano, cache_creation_input_cost_per_token_nano, output_cost_per_reasoning_token_nano,
                  max_input_tokens, max_output_tokens, max_tokens, raw_json, source, updated_at)
                 VALUES ($1, $2, $3, $4, $5, $6, $7, $8, $9, $10, $11, '{}', 'manual', $12)
                 ON CONFLICT(model_id) DO UPDATE SET
                   models_dev_provider = excluded.models_dev_provider,
                   mode = excluded.mode,
                   input_cost_per_token_nano = excluded.input_cost_per_token_nano,
                   output_cost_per_token_nano = excluded.output_cost_per_token_nano,
                   cache_read_input_cost_per_token_nano = excluded.cache_read_input_cost_per_token_nano,
                   cache_creation_input_cost_per_token_nano = excluded.cache_creation_input_cost_per_token_nano,
                   output_cost_per_reasoning_token_nano = excluded.output_cost_per_reasoning_token_nano,
                   max_input_tokens = excluded.max_input_tokens,
                   max_output_tokens = excluded.max_output_tokens,
                   max_tokens = excluded.max_tokens,
                   source = 'manual',
                   updated_at = excluded.updated_at",
                vec![
                    model_id.into(),
                    models_dev_provider.into(),
                    mode.into(),
                    input_cost_per_token_nano.into(),
                    output_cost_per_token_nano.into(),
                    cache_read_input_cost_per_token_nano.into(),
                    cache_creation_input_cost_per_token_nano.into(),
                    output_cost_per_reasoning_token_nano.into(),
                    max_input_tokens.into(),
                    max_output_tokens.into(),
                    max_tokens.into(),
                    now.into(),
                ],
            ))
            .await
            .map_err(|e| e.to_string())?;

        let record = get_model_metadata_with(&self.db, &txn, model_id)
            .await?
            .ok_or_else(|| "upsert succeeded but record not found".to_string())?;
        upsert_model_metadata_billing_rates(&self.db, &txn, &record).await?;
        txn.commit().await.map_err(|e| e.to_string())?;
        Ok(record)
    }

    pub async fn delete_model_metadata(&self, model_id: &str) -> Result<bool, String> {
        let write_guard = self.db.write().await;
        let txn = write_guard.begin().await.map_err(|e| e.to_string())?;
        let result = txn
            .execute(self.db.stmt(
                "DELETE FROM model_metadata_records WHERE model_id = $1",
                vec![model_id.into()],
            ))
            .await
            .map_err(|e| e.to_string())?;
        if result.rows_affected() == 0 {
            txn.rollback().await.map_err(|e| e.to_string())?;
            return Ok(false);
        }
        delete_model_metadata_billing_rates(&self.db, &txn, model_id).await?;
        txn.commit().await.map_err(|e| e.to_string())?;
        Ok(true)
    }

    pub async fn batch_delete_model_metadata(
        &self,
        model_ids: &[String],
    ) -> Result<BatchDeleteModelMetadataResult, String> {
        let mut normalized = Vec::with_capacity(model_ids.len());
        let mut seen = std::collections::HashSet::new();
        for model_id in model_ids {
            let model_id = model_id.trim();
            if !model_id.is_empty() && seen.insert(model_id.to_string()) {
                normalized.push(model_id.to_string());
            }
        }
        if normalized.is_empty() {
            return Err("invalid_request: model_ids must not be empty".to_string());
        }

        const BATCH_CHUNK_SIZE: usize = 250;
        let write_guard = self.db.write().await;
        let txn = write_guard.begin().await.map_err(|e| e.to_string())?;
        let mut deleted = 0usize;
        let mut not_found = Vec::new();

        for chunk in normalized.chunks(BATCH_CHUNK_SIZE) {
            let placeholders = (1..=chunk.len())
                .map(|index| format!("${index}"))
                .collect::<Vec<_>>()
                .join(", ");
            let values = chunk
                .iter()
                .cloned()
                .map(Into::into)
                .collect::<Vec<sea_orm::Value>>();
            let rows = txn
                .query_all(self.db.stmt(
                    &format!(
                        "SELECT model_id FROM model_metadata_records WHERE model_id IN ({placeholders})"
                    ),
                    values,
                ))
                .await
                .map_err(|e| e.to_string())?;
            let found = rows
                .iter()
                .map(|row| {
                    row.try_get::<String>("", "model_id")
                        .map_err(|e| e.to_string())
                })
                .collect::<Result<std::collections::HashSet<_>, _>>()?;
            not_found.extend(
                chunk
                    .iter()
                    .filter(|model_id| !found.contains(*model_id))
                    .cloned(),
            );
            if found.is_empty() {
                continue;
            }

            let found_values = found.iter().cloned().map(Into::into).collect::<Vec<_>>();
            let found_placeholders = (1..=found.len())
                .map(|index| format!("${index}"))
                .collect::<Vec<_>>()
                .join(", ");
            let metadata_result = txn
                .execute(self.db.stmt(
                    &format!(
                        "DELETE FROM model_metadata_records WHERE model_id IN ({found_placeholders})"
                    ),
                    found_values.clone(),
                ))
                .await
                .map_err(|e| e.to_string())?;
            deleted += metadata_result.rows_affected() as usize;
            txn.execute(self.db.stmt(
                &format!(
                    "DELETE FROM billing_rate_records
                     WHERE model_pattern IN ({found_placeholders})
                       AND id LIKE 'model_metadata:%'"
                ),
                found_values,
            ))
            .await
            .map_err(|e| e.to_string())?;
        }

        txn.commit().await.map_err(|e| e.to_string())?;
        Ok(BatchDeleteModelMetadataResult {
            success: true,
            deleted,
            not_found,
        })
    }

    pub async fn sync_from_models_dev(
        &self,
        http: &reqwest::Client,
        base_url: &str,
    ) -> Result<ModelMetadataSyncResult, String> {
        let models_dev_url = models_dev_api_url(base_url)?;
        let resp = http
            .get(models_dev_url)
            .timeout(std::time::Duration::from_secs(15))
            .send()
            .await
            .map_err(|e| format!("fetch_failed: {e}"))?;
        let status = resp.status();
        if !status.is_success() {
            return Err(format!("fetch_failed: status={status}"));
        }
        let body = crate::bounded_response::read_upstream_discovery_body(resp)
            .await
            .map_err(|e| format!("fetch_failed: {e}"))?;
        let root: Value =
            serde_json::from_slice(&body).map_err(|e| format!("parse_failed: {e}"))?;
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
                        raw: models_dev_variant_for_dashboard(model_val),
                    });
            }
        }

        let fetched_at = Utc::now().to_rfc3339();
        let _write_guard = self.db.write().await;
        let txn = _write_guard.begin().await.map_err(|e| e.to_string())?;

        let del_result = txn
            .execute(self.db.stmt(
                "DELETE FROM model_metadata_records WHERE source != 'manual'",
                vec![],
            ))
            .await
            .map_err(|e| e.to_string())?;
        let deleted = del_result.rows_affected();
        txn.execute(self.db.stmt(
            "DELETE FROM billing_rate_records WHERE source = 'models_dev'",
            vec![],
        ))
        .await
        .map_err(|e| e.to_string())?;

        let manual_rows = txn
            .query_all(self.db.stmt(
                "SELECT model_id FROM model_metadata_records WHERE source = 'manual'",
                vec![],
            ))
            .await
            .map_err(|e| e.to_string())?;
        let manual_ids = manual_rows
            .into_iter()
            .map(|row| {
                row.try_get::<String>("", "model_id")
                    .map_err(|e| e.to_string())
            })
            .collect::<Result<std::collections::HashSet<_>, _>>()?;

        let mut metadata_writes = Vec::new();
        let mut rate_writes = Vec::new();
        let mut skipped = 0usize;
        let mut grouped_models = grouped.iter().collect::<Vec<_>>();
        grouped_models.sort_by(|(left, _), (right, _)| left.cmp(right));
        for (model_name, variants) in grouped_models {
            if !has_positive_input_variant(variants) {
                continue;
            }
            if manual_ids.contains(model_name) {
                skipped += 1;
                continue;
            }

            let winner = pick_best_variant(model_name, variants);
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
            metadata_writes.push(ModelsDevMetadataWrite {
                model_name: model_name.clone(),
                provider_id: winner.provider_id.clone(),
                mode,
                input_cost_nano: winner.input_cost_nano.clone(),
                output_cost_nano: winner.output_cost_nano.clone(),
                cache_read_nano: winner.cache_read_nano.clone(),
                cache_write_nano: winner.cache_write_nano.clone(),
                reasoning_nano: winner.reasoning_nano.clone(),
                max_input_tokens: winner.max_input_tokens,
                max_output_tokens: winner.max_output_tokens,
                max_tokens: winner.max_tokens,
                raw_json: serde_json::json!({ "providers": providers_map }).to_string(),
            });
            for (usage_class, price) in [
                ("input_uncached", winner.input_cost_nano.as_ref()),
                ("output", winner.output_cost_nano.as_ref()),
                ("cache_read", winner.cache_read_nano.as_ref()),
                ("cache_write_5m", winner.cache_write_nano.as_ref()),
                ("reasoning_output", winner.reasoning_nano.as_ref()),
            ] {
                let Some(price) = price else {
                    continue;
                };
                rate_writes.push(ModelsDevRateWrite {
                    id: format!(
                        "models_dev:{}:{}:{usage_class}",
                        winner.provider_id, model_name
                    ),
                    pricing_profile: winner.provider_id.clone(),
                    model_name: model_name.clone(),
                    usage_class,
                    price: price.clone(),
                    raw_json: serde_json::json!({
                        "source": "models.dev",
                        "models_dev_provider": winner.provider_id
                    })
                    .to_string(),
                });
            }
        }

        const METADATA_SYNC_CHUNK_SIZE: usize = 30;
        for chunk in metadata_writes.chunks(METADATA_SYNC_CHUNK_SIZE) {
            let mut values: Vec<sea_orm::Value> = Vec::with_capacity(chunk.len() * 13);
            let mut rows = Vec::with_capacity(chunk.len());
            for row in chunk {
                let start = values.len() + 1;
                values.extend([
                    row.model_name.clone().into(),
                    row.provider_id.clone().into(),
                    row.mode.into(),
                    row.input_cost_nano.clone().into(),
                    row.output_cost_nano.clone().into(),
                    row.cache_read_nano.clone().into(),
                    row.cache_write_nano.clone().into(),
                    row.reasoning_nano.clone().into(),
                    row.max_input_tokens.into(),
                    row.max_output_tokens.into(),
                    row.max_tokens.into(),
                    row.raw_json.clone().into(),
                    fetched_at.clone().into(),
                ]);
                let mut placeholders = (start..start + 12)
                    .map(|index| format!("${index}"))
                    .collect::<Vec<_>>();
                placeholders.push("'models_dev'".to_string());
                placeholders.push(format!("${}", start + 12));
                rows.push(format!("({})", placeholders.join(", ")));
            }
            txn.execute(self.db.stmt(
                &format!(
                    "INSERT INTO model_metadata_records
                     (model_id, models_dev_provider, mode, input_cost_per_token_nano,
                      output_cost_per_token_nano, cache_read_input_cost_per_token_nano,
                      cache_creation_input_cost_per_token_nano,
                      output_cost_per_reasoning_token_nano, max_input_tokens,
                      max_output_tokens, max_tokens, raw_json, source, updated_at)
                     VALUES {}
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
                    rows.join(", ")
                ),
                values,
            ))
            .await
            .map_err(|e| e.to_string())?;
        }

        const RATE_SYNC_CHUNK_SIZE: usize = 57;
        for chunk in rate_writes.chunks(RATE_SYNC_CHUNK_SIZE) {
            let mut values: Vec<sea_orm::Value> = Vec::with_capacity(chunk.len() * 7);
            let mut rows = Vec::with_capacity(chunk.len());
            for row in chunk {
                let start = values.len() + 1;
                values.extend([
                    row.id.clone().into(),
                    row.pricing_profile.clone().into(),
                    row.model_name.clone().into(),
                    row.usage_class.into(),
                    row.price.clone().into(),
                    row.raw_json.clone().into(),
                    fetched_at.clone().into(),
                ]);
                rows.push(format!(
                    "(${}, 'models_dev', ${}, ${}, NULL, 'token', ${}, 'token', ${}, '{{}}', 0, 1, ${}, ${})",
                    start,
                    start + 1,
                    start + 2,
                    start + 3,
                    start + 4,
                    start + 5,
                    start + 6
                ));
            }
            txn.execute(self.db.stmt(
                &format!(
                    "INSERT INTO billing_rate_records
                     (id, source, pricing_profile, model_pattern, provider_type, rate_kind,
                      usage_class, unit, unit_price_nano_usd, match_json, priority, enabled,
                      raw_json, updated_at)
                     VALUES {}
                     ON CONFLICT(id) DO UPDATE SET
                       source=excluded.source,
                       pricing_profile=excluded.pricing_profile,
                       model_pattern=excluded.model_pattern,
                       provider_type=excluded.provider_type,
                       rate_kind=excluded.rate_kind,
                       usage_class=excluded.usage_class,
                       unit=excluded.unit,
                       unit_price_nano_usd=excluded.unit_price_nano_usd,
                       match_json=excluded.match_json,
                       priority=excluded.priority,
                       enabled=excluded.enabled,
                       raw_json=excluded.raw_json,
                       updated_at=excluded.updated_at",
                    rows.join(", ")
                ),
                values,
            ))
            .await
            .map_err(|e| e.to_string())?;
        }

        txn.commit().await.map_err(|e| e.to_string())?;
        let upserted = metadata_writes.len();
        Ok(ModelMetadataSyncResult {
            success: true,
            upserted,
            skipped,
            deleted,
            fetched_at,
        })
    }
}

fn models_dev_api_url(base_url: &str) -> Result<String, String> {
    let trimmed = base_url.trim();
    let mut url = reqwest::Url::parse(trimmed)
        .map_err(|error| format!("invalid_request: models_dev_base_url is invalid: {error}"))?;
    validate_models_dev_url(&url)?;

    let path = url.path().trim_end_matches('/').to_string();
    if path.is_empty() {
        url.set_path("/api.json");
    } else if path != url.path() {
        url.set_path(&path);
    }
    Ok(url.to_string())
}

pub fn validate_models_dev_base_url(base_url: &str) -> Result<(), String> {
    let url = reqwest::Url::parse(base_url.trim())
        .map_err(|error| format!("invalid_request: models_dev_base_url is invalid: {error}"))?;
    validate_models_dev_url(&url)
}

fn validate_models_dev_url(url: &reqwest::Url) -> Result<(), String> {
    if !matches!(url.scheme(), "http" | "https") || url.host_str().is_none() {
        return Err(
            "invalid_request: models_dev_base_url must use http or https and contain a host"
                .to_string(),
        );
    }
    if url.query().is_some() || url.fragment().is_some() {
        return Err(
            "invalid_request: models_dev_base_url must not contain a query or fragment".to_string(),
        );
    }
    Ok(())
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
        provider_id: row.try_get("", "provider_id").map_err(|e| e.to_string())?,
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
        models_dev_provider: row
            .try_get("", "models_dev_provider")
            .map_err(|e| e.to_string())?,
        mode: row.try_get("", "mode").map_err(|e| e.to_string())?,
        input_cost_per_token_nano: row
            .try_get("", "input_cost_per_token_nano")
            .map_err(|e| e.to_string())?,
        output_cost_per_token_nano: row
            .try_get("", "output_cost_per_token_nano")
            .map_err(|e| e.to_string())?,
        cache_read_input_cost_per_token_nano: row
            .try_get("", "cache_read_input_cost_per_token_nano")
            .map_err(|e| e.to_string())?,
        cache_creation_input_cost_per_token_nano: row
            .try_get("", "cache_creation_input_cost_per_token_nano")
            .map_err(|e| e.to_string())?,
        output_cost_per_reasoning_token_nano: row
            .try_get("", "output_cost_per_reasoning_token_nano")
            .map_err(|e| e.to_string())?,
        max_input_tokens: row
            .try_get("", "max_input_tokens")
            .map_err(|e| e.to_string())?,
        max_output_tokens: row
            .try_get("", "max_output_tokens")
            .map_err(|e| e.to_string())?,
        max_tokens: row.try_get("", "max_tokens").map_err(|e| e.to_string())?,
        raw_json,
        source: row.try_get("", "source").map_err(|e| e.to_string())?,
        updated_at,
    })
}

async fn get_model_metadata_with<C: ConnectionTrait>(
    db: &DbPool,
    conn: &C,
    model_id: &str,
) -> Result<Option<DbModelMetadataRecord>, String> {
    let lock_suffix = if db.is_postgres() { " FOR UPDATE" } else { "" };
    let row = conn
        .query_one(db.stmt(
            &format!(
                "SELECT model_id, models_dev_provider, mode, input_cost_per_token_nano,
                    output_cost_per_token_nano, cache_read_input_cost_per_token_nano,
                    cache_creation_input_cost_per_token_nano,
                    output_cost_per_reasoning_token_nano, max_input_tokens, max_output_tokens,
                    max_tokens, raw_json, source, updated_at
             FROM model_metadata_records
             WHERE model_id = $1{lock_suffix}"
            ),
            vec![model_id.into()],
        ))
        .await
        .map_err(|e| e.to_string())?;
    row.as_ref().map(row_to_model_metadata).transpose()
}

fn merge_nullable<T>(update: Option<Option<T>>, existing: Option<T>) -> Option<T> {
    update.unwrap_or(existing)
}

fn validate_optional_price(field: &str, update: Option<&Option<String>>) -> Result<(), String> {
    let Some(Some(value)) = update else {
        return Ok(());
    };
    if value.is_empty()
        || !value.bytes().all(|byte| byte.is_ascii_digit())
        || (value.len() > 1 && value.starts_with('0'))
        || value.parse::<i128>().is_err()
    {
        return Err(format!(
            "invalid_request: {field} must be a canonical non-negative integer string"
        ));
    }
    Ok(())
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

struct ModelsDevMetadataWrite {
    model_name: String,
    provider_id: String,
    mode: &'static str,
    input_cost_nano: Option<String>,
    output_cost_nano: Option<String>,
    cache_read_nano: Option<String>,
    cache_write_nano: Option<String>,
    reasoning_nano: Option<String>,
    max_input_tokens: Option<i64>,
    max_output_tokens: Option<i64>,
    max_tokens: Option<i64>,
    raw_json: String,
}

struct ModelsDevRateWrite {
    id: String,
    pricing_profile: String,
    model_name: String,
    usage_class: &'static str,
    price: String,
    raw_json: String,
}

async fn upsert_model_metadata_billing_rates(
    db: &DbPool,
    conn: &DatabaseTransaction,
    record: &DbModelMetadataRecord,
) -> Result<(), String> {
    delete_model_metadata_billing_rates(db, conn, &record.model_id).await?;

    let pricing_profile = record
        .models_dev_provider
        .clone()
        .unwrap_or_else(|| "default".to_string());
    for (usage_class, price) in [
        ("input_uncached", record.input_cost_per_token_nano.as_ref()),
        ("output", record.output_cost_per_token_nano.as_ref()),
        (
            "cache_read",
            record.cache_read_input_cost_per_token_nano.as_ref(),
        ),
        (
            "cache_write_5m",
            record.cache_creation_input_cost_per_token_nano.as_ref(),
        ),
        (
            "reasoning_output",
            record.output_cost_per_reasoning_token_nano.as_ref(),
        ),
    ] {
        let Some(price) = price else {
            continue;
        };
        let id = format!("model_metadata:{}:{usage_class}", record.model_id);
        conn.execute(db.stmt(
            "INSERT INTO billing_rate_records
                 (id, source, pricing_profile, model_pattern, provider_type, rate_kind, usage_class,
                  unit, unit_price_nano_usd, match_json, priority, enabled, raw_json, updated_at)
                 VALUES ($1, $2, $3, $4, NULL, 'token', $5, 'token', $6, '{}', 0, 1, $7, $8)
                 ON CONFLICT(id) DO UPDATE SET
                   source = excluded.source,
                   pricing_profile = excluded.pricing_profile,
                   model_pattern = excluded.model_pattern,
                   usage_class = excluded.usage_class,
                   unit_price_nano_usd = excluded.unit_price_nano_usd,
                   raw_json = excluded.raw_json,
                   updated_at = excluded.updated_at",
            vec![
                    id.into(),
                    record.source.clone().into(),
                    pricing_profile.clone().into(),
                    record.model_id.clone().into(),
                    usage_class.into(),
                    price.clone().into(),
                    serde_json::json!({ "source": "model_metadata_records" })
                        .to_string()
                        .into(),
                    record.updated_at.to_rfc3339().into(),
                ],
        ))
        .await
        .map_err(|e| e.to_string())?;
    }
    Ok(())
}

async fn delete_model_metadata_billing_rates(
    db: &DbPool,
    conn: &DatabaseTransaction,
    model_id: &str,
) -> Result<(), String> {
    conn.execute(db.stmt(
        "DELETE FROM billing_rate_records
             WHERE model_pattern = $1 AND id LIKE 'model_metadata:%'",
        vec![model_id.into()],
    ))
    .await
    .map_err(|e| e.to_string())?;
    Ok(())
}

/// Maps a canonical (bare, lowercase) model ID to its official provider ID on models.dev.
/// Returns `None` for models that don't belong to a recognized family.
fn official_provider_for_model(model_id: &str) -> Option<&'static str> {
    if model_id.starts_with("gpt-") {
        return Some("openai");
    }
    // OpenAI o-series: "o" followed by a digit (o1, o3-pro, o4-mini, etc.)
    if model_id.starts_with('o')
        && model_id
            .as_bytes()
            .get(1)
            .is_some_and(|b| b.is_ascii_digit())
    {
        return Some("openai");
    }
    if model_id.starts_with("claude-") {
        return Some("anthropic");
    }
    if model_id.starts_with("gemini-") {
        return Some("google");
    }
    if model_id.starts_with("grok-") {
        return Some("xai");
    }
    if model_id.starts_with("deepseek-") {
        return Some("deepseek");
    }
    if model_id.starts_with("mistral-")
        || model_id.starts_with("codestral-")
        || model_id.starts_with("pixtral-")
        || model_id.starts_with("ministral-")
    {
        return Some("mistral");
    }
    None
}

fn pick_best_variant<'a>(
    model_id: &str,
    variants: &'a [SyncProviderVariant],
) -> &'a SyncProviderVariant {
    if let Some(official) = official_provider_for_model(model_id) {
        if let Some(v) = variants.iter().find(|v| {
            v.provider_id == official
                && v.input_cost_nano
                    .as_ref()
                    .and_then(|s| s.parse::<i128>().ok())
                    .is_some_and(|n| n > 0)
        }) {
            return v;
        }
    }

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
    let raw = match value {
        Value::Number(number) => number.to_string(),
        Value::String(value) => value.clone(),
        _ => return None,
    };
    if raw.contains(['e', 'E']) || raw.starts_with('+') {
        return None;
    }
    let decimal = Decimal::from_str(&raw).ok()?;
    if decimal.is_sign_negative() {
        return None;
    }
    let nano = decimal
        .checked_mul(Decimal::from(1000u32))?
        .trunc()
        .to_i128()?;
    Some(nano.to_string())
}

fn models_dev_variant_for_dashboard(value: &Value) -> Value {
    let mut value = value.clone();
    let Some(cost) = value
        .as_object_mut()
        .and_then(|model| model.get_mut("cost"))
        .and_then(Value::as_object_mut)
    else {
        return value;
    };
    for price in cost.values_mut() {
        if let Value::Number(number) = price {
            *price = Value::String(number.to_string());
        }
    }
    value
}

fn value_to_i64(value: &Value) -> Option<i64> {
    value
        .as_i64()
        .or_else(|| value.as_u64().and_then(|v| i64::try_from(v).ok()))
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

#[cfg(test)]
mod tests {
    use super::{
        ModelRegistryStore, UpsertModelMetadataInput, cost_per_1m_to_nano_string,
        deserialize_nullable_field, models_dev_api_url, models_dev_variant_for_dashboard,
        validate_models_dev_base_url,
    };
    use crate::db::DbPool;
    use crate::migration::Migrator;
    use crate::monoize_routing::{CreateMonoizeProviderInput, MonoizeRoutingStore};
    use sea_orm::ConnectionTrait;
    use sea_orm_migration::MigratorTrait;
    use serde::Deserialize;
    use serde_json::json;

    #[test]
    fn models_dev_decimal_price_conversion_is_exact() {
        assert_eq!(
            cost_per_1m_to_nano_string(&json!(1.001)),
            Some("1001".to_string())
        );
        assert_eq!(
            cost_per_1m_to_nano_string(&json!("0.0009")),
            Some("0".to_string())
        );
        assert_eq!(cost_per_1m_to_nano_string(&json!(-1)), None);
    }

    #[test]
    fn dashboard_raw_variant_keeps_costs_as_decimal_strings() {
        let variant = models_dev_variant_for_dashboard(&json!({
            "cost": { "input": 1.001, "output": 2 },
            "limit": { "context": 128000 }
        }));
        assert_eq!(variant["cost"]["input"], json!("1.001"));
        assert_eq!(variant["cost"]["output"], json!("2"));
        assert_eq!(variant["limit"]["context"], json!(128000));
    }

    #[derive(Deserialize)]
    struct NullableProbe {
        #[serde(default, deserialize_with = "deserialize_nullable_field")]
        value: Option<Option<String>>,
    }

    #[test]
    fn nullable_fields_distinguish_omitted_and_explicit_null() {
        let omitted: NullableProbe = serde_json::from_value(json!({})).unwrap();
        let cleared: NullableProbe = serde_json::from_value(json!({ "value": null })).unwrap();
        let assigned: NullableProbe = serde_json::from_value(json!({ "value": "1001" })).unwrap();
        assert_eq!(omitted.value, None);
        assert_eq!(cleared.value, Some(None));
        assert_eq!(assigned.value, Some(Some("1001".to_string())));
    }

    #[test]
    fn models_dev_base_url_normalizes_api_path_and_rejects_unsafe_urls() {
        assert_eq!(
            models_dev_api_url(" https://mirror.example/custom-api.json/ ").unwrap(),
            "https://mirror.example/custom-api.json"
        );
        assert_eq!(
            models_dev_api_url("https://models.dev/api.json").unwrap(),
            "https://models.dev/api.json"
        );
        assert!(validate_models_dev_base_url("file:///tmp/models").is_err());
        assert!(validate_models_dev_base_url("https://models.dev?token=secret").is_err());
    }

    #[tokio::test]
    async fn batch_delete_metadata_removes_generated_billing_mirrors() {
        let db = DbPool::connect("sqlite::memory:")
            .await
            .expect("db connects");
        {
            let write = db.write().await;
            Migrator::up(&*write, None).await.expect("migrates");
        }
        let store = ModelRegistryStore::new(db.clone())
            .await
            .expect("store creates");
        store
            .upsert_model_metadata(
                "batch-model",
                serde_json::from_value(json!({
                    "input_cost_per_token_nano": "1000",
                    "output_cost_per_token_nano": "2000"
                }))
                .expect("input parses"),
            )
            .await
            .expect("metadata upserts");

        let result = store
            .batch_delete_model_metadata(&[
                " batch-model ".to_string(),
                "missing-model".to_string(),
                "batch-model".to_string(),
            ])
            .await
            .expect("batch delete succeeds");
        assert_eq!(result.deleted, 1);
        assert_eq!(result.not_found, vec!["missing-model"]);
        assert!(
            store
                .get_model_metadata("batch-model")
                .await
                .unwrap()
                .is_none()
        );
        let mirror_count: i64 = db
            .read()
            .query_one(db.stmt(
                "SELECT COUNT(*) AS count FROM billing_rate_records WHERE id LIKE 'model_metadata:%'",
                vec![],
            ))
            .await
            .expect("count query succeeds")
            .expect("count row exists")
            .try_get("", "count")
            .expect("count decodes");
        assert_eq!(mirror_count, 0);
    }

    #[tokio::test]
    async fn metadata_list_places_manual_rows_before_synced_rows() {
        let db = DbPool::connect("sqlite::memory:")
            .await
            .expect("db connects");
        {
            let write = db.write().await;
            Migrator::up(&*write, None).await.expect("migrates");
            write
                .execute(db.stmt(
                    "INSERT INTO model_metadata_records
                     (model_id, raw_json, source, updated_at)
                     VALUES ($1, '{}', $2, $3), ($4, '{}', $5, $3), ($6, '{}', $7, $3)",
                    vec![
                        "zeta".into(),
                        "models_dev".into(),
                        "2026-01-01T00:00:00Z".into(),
                        "alpha".into(),
                        "manual".into(),
                        "beta".into(),
                        "manual".into(),
                    ],
                ))
                .await
                .expect("metadata rows insert");
        }
        let store = ModelRegistryStore::new(db)
            .await
            .expect("store creates");
        let model_ids = store
            .list_model_metadata()
            .await
            .expect("metadata list succeeds")
            .into_iter()
            .map(|record| record.model_id)
            .collect::<Vec<_>>();
        assert_eq!(model_ids, vec!["alpha", "beta", "zeta"]);
    }

    #[tokio::test]
    async fn marketplace_metadata_join_is_distinct_sorted_and_filters_routing_state() {
        let db = DbPool::connect("sqlite::memory:")
            .await
            .expect("db connects");
        {
            let write = db.write().await;
            Migrator::up(&*write, None).await.expect("migrates");
        }
        let metadata_store = ModelRegistryStore::new(db.clone())
            .await
            .expect("metadata store creates");
        let routing_store = MonoizeRoutingStore::new(db)
            .await
            .expect("routing store creates");

        for model_id in [
            "eligible-a",
            "eligible-z",
            "shared",
            "disabled-channel",
            "zero-weight",
            "disabled-provider",
            "metadata-only",
        ] {
            let input: UpsertModelMetadataInput =
                serde_json::from_value(json!({})).expect("metadata input parses");
            metadata_store
                .upsert_model_metadata(model_id, input)
                .await
                .expect("metadata upserts");
        }

        let enabled: CreateMonoizeProviderInput = serde_json::from_value(json!({
            "name": "enabled",
            "channels": [
                {
                    "name": "active-a",
                    "provider_type": "responses",
                    "base_url": "https://example.com",
                    "api_key": "secret-a",
                    "models": {
                        "eligible-z": { "redirect": null, "multiplier": "1" },
                        "shared": { "redirect": null, "multiplier": "1" }
                    }
                },
                {
                    "name": "active-b",
                    "provider_type": "responses",
                    "base_url": "https://example.com",
                    "api_key": "secret-b",
                    "models": {
                        "eligible-a": { "redirect": null, "multiplier": "1" },
                        "shared": { "redirect": null, "multiplier": "1" }
                    }
                },
                {
                    "name": "disabled",
                    "provider_type": "responses",
                    "base_url": "https://example.com",
                    "api_key": "secret-disabled",
                    "enabled": false,
                    "models": {
                        "disabled-channel": { "redirect": null, "multiplier": "1" }
                    }
                },
                {
                    "name": "zero-weight",
                    "provider_type": "responses",
                    "base_url": "https://example.com",
                    "api_key": "secret-zero",
                    "weight": 0,
                    "models": {
                        "zero-weight": { "redirect": null, "multiplier": "1" }
                    }
                }
            ]
        }))
        .expect("enabled provider input parses");
        routing_store
            .create_provider(enabled)
            .await
            .expect("enabled provider creates");

        let disabled: CreateMonoizeProviderInput = serde_json::from_value(json!({
            "name": "disabled provider",
            "enabled": false,
            "channels": [{
                "name": "active channel",
                "provider_type": "responses",
                "base_url": "https://example.com",
                "api_key": "secret-provider-disabled",
                "models": {
                    "disabled-provider": { "redirect": null, "multiplier": "1" }
                }
            }]
        }))
        .expect("disabled provider input parses");
        routing_store
            .create_provider(disabled)
            .await
            .expect("disabled provider creates");

        let listed = metadata_store
            .list_marketplace_model_metadata()
            .await
            .expect("marketplace metadata lists");
        assert_eq!(
            listed
                .iter()
                .map(|record| record.model_id.as_str())
                .collect::<Vec<_>>(),
            vec!["eligible-a", "eligible-z", "shared"]
        );
    }
}
