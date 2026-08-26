use crate::db::DbPool;
use crate::model_registry::{ModelCapabilities, ModelRecord};
use chrono::{DateTime, Utc};
use rust_decimal::Decimal;
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
    ) -> Result<Vec<MarketplaceModelRecord>, String> {
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
                 LEFT JOIN model_prices AS mp ON mp.model_id = m.model_id AND mp.enabled = 1
                 WHERE p.enabled = 1
                   AND c.enabled = 1
                   AND c.weight > 0
                 ORDER BY m.model_id ASC",
                vec![],
            ))
            .await
            .map_err(|e| e.to_string())?;

        rows.iter()
            .map(|row| {
                Ok(MarketplaceModelRecord {
                    metadata: row_to_model_metadata(row)?,
                    billing_mode: row
                        .try_get("", "billing_mode")
                        .map_err(|e| e.to_string())?,
                    input_usd_per_1m: row
                        .try_get("", "input_usd_per_1m")
                        .map_err(|e| e.to_string())?,
                    output_usd_per_1m: row
                        .try_get("", "output_usd_per_1m")
                        .map_err(|e| e.to_string())?,
                })
            })
            .collect()
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
                 VALUES ($1, $2, $3, $4, $5, $6, '{}', 'manual', $7)
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

    /// Fetch models.dev and upsert metadata rows (MP-Y10). Metadata rows no
    /// longer carry prices; `model_prices` is written only by the price sync
    /// engine (`price_sync`).
    pub async fn sync_from_models_dev(
        &self,
        http: &reqwest::Client,
    ) -> Result<ModelMetadataSyncResult, String> {
        let root = crate::price_sync::fetch_models_dev_root(http).await?;
        self.apply_models_dev_metadata(&root).await
    }

    /// Apply one parsed models.dev snapshot to `model_metadata_records`:
    /// delete non-manual rows, then insert one row per canonical model that
    /// has a strictly positive input cost variant. Manual rows are kept and
    /// counted as skipped.
    pub async fn apply_models_dev_metadata(
        &self,
        root: &Value,
    ) -> Result<ModelMetadataSyncResult, String> {
        let grouped = group_models_dev_variants(root)?;

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
        let mut skipped = 0usize;
        for (model_name, variants) in &grouped {
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
                max_input_tokens: winner.max_input_tokens,
                max_output_tokens: winner.max_output_tokens,
                max_tokens: winner.max_tokens,
                raw_json: serde_json::json!({ "providers": providers_map }).to_string(),
            });
        }

        const METADATA_SYNC_CHUNK_SIZE: usize = 100;
        for chunk in metadata_writes.chunks(METADATA_SYNC_CHUNK_SIZE) {
            let mut values: Vec<sea_orm::Value> = Vec::with_capacity(chunk.len() * 8);
            let mut rows = Vec::with_capacity(chunk.len());
            for row in chunk {
                let start = values.len() + 1;
                values.extend([
                    row.model_name.clone().into(),
                    row.provider_id.clone().into(),
                    row.mode.into(),
                    row.max_input_tokens.into(),
                    row.max_output_tokens.into(),
                    row.max_tokens.into(),
                    row.raw_json.clone().into(),
                    fetched_at.clone().into(),
                ]);
                let mut placeholders = (start..start + 7)
                    .map(|index| format!("${index}"))
                    .collect::<Vec<_>>();
                placeholders.push("'models_dev'".to_string());
                placeholders.push(format!("${}", start + 7));
                // Placeholder order: 7 data columns, literal source, updated_at.
                rows.push(format!("({})", placeholders.join(", ")));
            }
            txn.execute(self.db.stmt(
                &format!(
                    "INSERT INTO model_metadata_records
                     (model_id, models_dev_provider, mode, max_input_tokens,
                      max_output_tokens, max_tokens, raw_json, source, updated_at)
                     VALUES {}
                     ON CONFLICT(model_id) DO UPDATE SET
                       models_dev_provider=excluded.models_dev_provider,
                       mode=excluded.mode,
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

/// One models.dev provider variant of a canonical model, with cost values
/// kept as exact USD-per-1M decimal strings (MP-Y5).
pub(crate) struct SyncProviderVariant {
    pub(crate) provider_id: String,
    pub(crate) family: Option<String>,
    pub(crate) cost_input: Option<String>,
    pub(crate) cost_output: Option<String>,
    pub(crate) cost_cache_read: Option<String>,
    pub(crate) cost_cache_write: Option<String>,
    pub(crate) cost_reasoning: Option<String>,
    pub(crate) max_input_tokens: Option<i64>,
    pub(crate) max_output_tokens: Option<i64>,
    pub(crate) max_tokens: Option<i64>,
    pub(crate) raw: Value,
}

struct ModelsDevMetadataWrite {
    model_name: String,
    provider_id: String,
    mode: &'static str,
    max_input_tokens: Option<i64>,
    max_output_tokens: Option<i64>,
    max_tokens: Option<i64>,
    raw_json: String,
}

/// MP-Y4: group models.dev variants by canonical model id (NID1), applying
/// the MP-Y9 skip rules. The result is sorted by canonical id for
/// deterministic write order.
pub(crate) fn group_models_dev_variants(
    root: &Value,
) -> Result<Vec<(String, Vec<SyncProviderVariant>)>, String> {
    let providers = root
        .as_object()
        .ok_or_else(|| "parse_failed: root must be object".to_string())?;
    let mut grouped: std::collections::BTreeMap<String, Vec<SyncProviderVariant>> =
        std::collections::BTreeMap::new();
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
                    cost_input: cost.and_then(|c| c.get("input")).and_then(usd_cost_string),
                    cost_output: cost.and_then(|c| c.get("output")).and_then(usd_cost_string),
                    cost_cache_read: cost
                        .and_then(|c| c.get("cache_read"))
                        .and_then(usd_cost_string),
                    cost_cache_write: cost
                        .and_then(|c| c.get("cache_write"))
                        .and_then(usd_cost_string),
                    cost_reasoning: cost
                        .and_then(|c| c.get("reasoning"))
                        .and_then(usd_cost_string),
                    max_input_tokens: limit.and_then(|l| l.get("input")).and_then(value_to_i64),
                    max_output_tokens: limit.and_then(|l| l.get("output")).and_then(value_to_i64),
                    max_tokens: limit.and_then(|l| l.get("context")).and_then(value_to_i64),
                    raw: models_dev_variant_for_dashboard(model_val),
                });
        }
    }
    Ok(grouped.into_iter().collect())
}

/// MP-Y8 official family→provider table: a case-insensitive prefix test on
/// the canonical model id, first matching row wins. `o<digit>` is the letter
/// `o` followed by an ASCII digit.
const OFFICIAL_FAMILY_PROVIDERS: &[(&str, &str)] = &[
    ("gpt-", "openai"),
    ("chatgpt-", "openai"),
    ("claude-", "anthropic"),
    ("gemini-", "google"),
    ("gemma-", "google"),
    ("grok-", "xai"),
    ("deepseek-", "deepseek"),
    ("mistral-", "mistral"),
    ("codestral-", "mistral"),
    ("pixtral-", "mistral"),
    ("ministral-", "mistral"),
    ("magistral-", "mistral"),
    ("devstral-", "mistral"),
    ("qwen", "alibaba"),
    ("qwq-", "alibaba"),
    ("qvq-", "alibaba"),
    ("llama-", "llama"),
    ("command-", "cohere"),
    ("kimi-", "moonshotai"),
    ("moonshot-", "moonshotai"),
    ("glm-", "zhipuai"),
    ("minimax-", "minimax"),
    ("step-", "stepfun"),
    ("sonar", "perplexity"),
    ("solar-", "upstage"),
    ("phi-", "azure"),
    ("mimo-", "xiaomi"),
    ("mercury", "inception"),
];

/// Maps a canonical (bare, lowercase) model ID to its official provider ID on
/// models.dev per MP-Y8. Returns `None` for unrecognized families.
pub(crate) fn official_provider_for_model(model_id: &str) -> Option<&'static str> {
    let lower = model_id.to_ascii_lowercase();
    // OpenAI o-series: "o" followed by a digit (o1, o3-pro, o4-mini, etc.)
    if lower.starts_with('o')
        && lower
            .as_bytes()
            .get(1)
            .is_some_and(|b| b.is_ascii_digit())
    {
        return Some("openai");
    }
    OFFICIAL_FAMILY_PROVIDERS
        .iter()
        .find(|(prefix, _)| lower.starts_with(prefix))
        .map(|(_, provider)| *provider)
}

fn positive_cost_input(variant: &SyncProviderVariant) -> Option<Decimal> {
    variant
        .cost_input
        .as_ref()
        .and_then(|raw| Decimal::from_str(raw).ok())
        .filter(|value| value.is_sign_positive() && !value.is_zero())
}

/// MP-Y7 variant selection: official provider with positive input cost,
/// otherwise the highest positive input cost.
pub(crate) fn pick_best_variant<'a>(
    model_id: &str,
    variants: &'a [SyncProviderVariant],
) -> &'a SyncProviderVariant {
    if let Some(official) = official_provider_for_model(model_id) {
        if let Some(v) = variants
            .iter()
            .find(|v| v.provider_id == official && positive_cost_input(v).is_some())
        {
            return v;
        }
    }

    variants
        .iter()
        .max_by(|a, b| {
            let cost_a = positive_cost_input(a).unwrap_or(Decimal::ZERO);
            let cost_b = positive_cost_input(b).unwrap_or(Decimal::ZERO);
            cost_a.cmp(&cost_b)
        })
        .expect("pick_best_variant called with at least one sync variant")
}

pub(crate) fn has_positive_input_variant(variants: &[SyncProviderVariant]) -> bool {
    variants.iter().any(|v| positive_cost_input(v).is_some())
}

pub(crate) fn should_ignore_sync_model(model_id: &str) -> bool {
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

/// Reads one models.dev cost value as an exact USD-per-1M decimal string
/// (MP-Y5): the JSON token is parsed as `Decimal`, never through binary
/// floating point, and canonicalized to at most 9 fractional digits.
pub(crate) fn usd_cost_string(value: &Value) -> Option<String> {
    let raw = match value {
        Value::Number(number) => number.to_string(),
        Value::String(value) => value.clone(),
        _ => return None,
    };
    if raw.contains(['e', 'E']) || raw.starts_with('+') {
        return None;
    }
    let decimal = Decimal::from_str_exact(&raw).ok()?;
    if decimal.is_sign_negative() {
        return None;
    }
    Some(
        decimal
            .round_dp_with_strategy(9, rust_decimal::RoundingStrategy::ToZero)
            .normalize()
            .to_string(),
    )
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
    "openai", "anthropic", "google", "xai", "mistral", "deepseek", "cohere", "meta",
    "minimax", "perplexity", "stepfun", "zhipuai", "nvidia", "moonshotai", "alibaba",
    "amazon-bedrock", "vercel", "openrouter", "azure", "groq", "fireworks", "together",
    "cloudflare", "replicate",
];

fn strip_provider_prefix_once<'a>(segment: &'a str, provider: &str) -> Option<&'a str> {
    segment
        .strip_prefix(&format!("{provider}--"))
        .or_else(|| segment.strip_prefix(&format!("{provider}.")))
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
        ModelRegistryStore, UpsertModelMetadataInput, deserialize_nullable_field,
        models_dev_variant_for_dashboard, usd_cost_string,
    };
    use crate::db::DbPool;
    use crate::migration::Migrator;
    use crate::monoize_routing::{CreateMonoizeProviderInput, MonoizeRoutingStore};
    use sea_orm_migration::MigratorTrait;
    use serde::Deserialize;
    use serde_json::json;

    #[test]
    fn models_dev_decimal_price_conversion_is_exact() {
        assert_eq!(usd_cost_string(&json!(1.001)), Some("1.001".to_string()));
        assert_eq!(usd_cost_string(&json!("0.0009")), Some("0.0009".to_string()));
        assert_eq!(usd_cost_string(&json!(2.50)), Some("2.5".to_string()));
        assert_eq!(usd_cost_string(&json!(-1)), None);
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
                .map(|record| record.metadata.model_id.as_str())
                .collect::<Vec<_>>(),
            vec!["eligible-a", "eligible-z", "shared"]
        );
    }
}
