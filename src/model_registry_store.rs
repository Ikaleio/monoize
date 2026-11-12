mod models_dev;
pub use models_dev::normalize_model_id;
pub(crate) use models_dev::{
    SyncProviderVariant, group_models_dev_variants, has_positive_input_variant, pick_best_variant,
    usd_cost_string,
};

use crate::db::DbPool;
use crate::model_registry::{ModelCapabilities, ModelRecord};
use chrono::{DateTime, Utc};
use sea_orm::{ConnectionTrait, TransactionTrait};
use serde::{Deserialize, Deserializer, Serialize};
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

/// Metadata rows no longer carry prices (`model-pricing.spec.md` MP-Y10);
/// prices live only in `model_prices`.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DbModelMetadataRecord {
    pub model_id: String,
    pub models_dev_provider: Option<String>,
    pub mode: Option<String>,
    pub max_input_tokens: Option<i64>,
    pub max_output_tokens: Option<i64>,
    pub max_tokens: Option<i64>,
    pub raw_json: Value,
    pub source: String,
    pub updated_at: DateTime<Utc>,
}

#[derive(Debug, Clone, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct UpsertModelMetadataInput {
    pub source: Option<String>,
    #[serde(default, deserialize_with = "deserialize_nullable_field")]
    pub models_dev_provider: Option<Option<String>>,
    #[serde(default, deserialize_with = "deserialize_nullable_field")]
    pub mode: Option<Option<String>>,
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
                "SELECT model_id, models_dev_provider, mode, max_input_tokens,
                        max_output_tokens, max_tokens, raw_json, source, updated_at
                 FROM model_metadata_records
                 ORDER BY model_id ASC",
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
                        m.model_id, m.models_dev_provider, m.mode, m.max_input_tokens,
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

    pub async fn get_model_metadata(
        &self,
        model_id: &str,
    ) -> Result<Option<DbModelMetadataRecord>, String> {
        let row = self
            .db
            .read()
            .query_one(self.db.stmt(
                "SELECT model_id, models_dev_provider, mode, max_input_tokens,
                        max_output_tokens, max_tokens, raw_json, source, updated_at
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

    pub async fn upsert_model_metadata(
        &self,
        model_id: &str,
        input: UpsertModelMetadataInput,
    ) -> Result<DbModelMetadataRecord, String> {
        let now = Utc::now().to_rfc3339();
        let source = input.source.as_deref().unwrap_or("manual");
        if !matches!(source, "manual" | "models_dev") {
            return Err("source must be manual or models_dev".to_string());
        }
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
                 (model_id, models_dev_provider, mode,
                  max_input_tokens, max_output_tokens, max_tokens, raw_json, source, updated_at)
                 VALUES ($1, $2, $3, $4, $5, $6, '{}', $7, $8)
                 ON CONFLICT(model_id) DO UPDATE SET
                   models_dev_provider = excluded.models_dev_provider,
                   mode = excluded.mode,
                   max_input_tokens = excluded.max_input_tokens,
                   max_output_tokens = excluded.max_output_tokens,
                   max_tokens = excluded.max_tokens,
                   source = excluded.source,
                   updated_at = excluded.updated_at",
            vec![
                model_id.into(),
                models_dev_provider.into(),
                mode.into(),
                max_input_tokens.into(),
                max_output_tokens.into(),
                max_tokens.into(),
                source.into(),
                now.into(),
            ],
        ))
        .await
        .map_err(|e| e.to_string())?;

        let record = get_model_metadata_with(&self.db, &txn, model_id)
            .await?
            .ok_or_else(|| "upsert succeeded but record not found".to_string())?;
        txn.commit().await.map_err(|e| e.to_string())?;
        Ok(record)
    }

    pub async fn delete_model_metadata(&self, model_id: &str) -> Result<bool, String> {
        let write_guard = self.db.write().await;
        let result = write_guard
            .execute(self.db.stmt(
                "DELETE FROM model_metadata_records WHERE model_id = $1",
                vec![model_id.into()],
            ))
            .await
            .map_err(|e| e.to_string())?;
        Ok(result.rows_affected() > 0)
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
                "SELECT model_id, models_dev_provider, mode, max_input_tokens,
                    max_output_tokens, max_tokens, raw_json, source, updated_at
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
