use crate::model_registry::{ModelCapabilities, ModelRecord};
use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use serde_json::Value;
use sqlx::{Pool, Row, Sqlite};

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
    /// Convert to the ModelRecord type used by the in-memory registry
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
    pub output_cost_per_reasoning_token_nano: Option<i128>,
}

#[derive(Debug, Clone, Deserialize)]
pub struct UpsertModelMetadataInput {
    pub models_dev_provider: Option<String>,
    pub mode: Option<String>,
    pub input_cost_per_token_nano: Option<String>,
    pub output_cost_per_token_nano: Option<String>,
    pub cache_read_input_cost_per_token_nano: Option<String>,
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
    pool: Pool<Sqlite>,
}

impl ModelRegistryStore {
    pub async fn new(pool: Pool<Sqlite>) -> Result<Self, String> {
        sqlx::query(
            r#"CREATE TABLE IF NOT EXISTS model_registry_records (
                id TEXT PRIMARY KEY,
                logical_model TEXT NOT NULL,
                provider_id TEXT NOT NULL,
                upstream_model TEXT NOT NULL,
                capabilities_json TEXT NOT NULL,
                enabled INTEGER NOT NULL DEFAULT 1,
                priority INTEGER NOT NULL DEFAULT 0,
                created_at TEXT NOT NULL,
                updated_at TEXT NOT NULL,
                UNIQUE (logical_model, provider_id)
            )"#,
        )
        .execute(&pool)
        .await
        .map_err(|e| e.to_string())?;

        sqlx::query(
            "CREATE INDEX IF NOT EXISTS idx_model_registry_logical ON model_registry_records(logical_model)",
        )
        .execute(&pool)
        .await
        .map_err(|e| e.to_string())?;

        sqlx::query(
            "CREATE INDEX IF NOT EXISTS idx_model_registry_provider ON model_registry_records(provider_id)",
        )
        .execute(&pool)
        .await
        .map_err(|e| e.to_string())?;

        sqlx::query(
            "CREATE INDEX IF NOT EXISTS idx_model_registry_enabled ON model_registry_records(enabled)",
        )
        .execute(&pool)
        .await
        .map_err(|e| e.to_string())?;

        sqlx::query(
            r#"CREATE TABLE IF NOT EXISTS model_metadata_records (
                model_id TEXT PRIMARY KEY,
                models_dev_provider TEXT,
                mode TEXT,
                input_cost_per_token_nano TEXT,
                output_cost_per_token_nano TEXT,
                cache_read_input_cost_per_token_nano TEXT,
                output_cost_per_reasoning_token_nano TEXT,
                max_input_tokens INTEGER,
                max_output_tokens INTEGER,
                max_tokens INTEGER,
                raw_json TEXT NOT NULL,
                source TEXT NOT NULL,
                updated_at TEXT NOT NULL
            )"#,
        )
        .execute(&pool)
        .await
        .map_err(|e| e.to_string())?;

        migrate_model_metadata_provider_column(&pool).await?;
        migrate_model_metadata_bare_model_id(&pool).await?;

        sqlx::query(
            "CREATE INDEX IF NOT EXISTS idx_model_metadata_provider ON model_metadata_records(models_dev_provider)",
        )
        .execute(&pool)
        .await
        .map_err(|e| e.to_string())?;

        Ok(Self { pool })
    }

    pub async fn list_models(&self) -> Result<Vec<DbModelRecord>, String> {
        let rows = sqlx::query(
            r#"SELECT id, logical_model, provider_id, upstream_model, capabilities_json,
                      enabled, priority, created_at, updated_at
               FROM model_registry_records
               ORDER BY priority DESC, logical_model ASC"#,
        )
        .fetch_all(&self.pool)
        .await
        .map_err(|e| e.to_string())?;

        let mut records = Vec::new();
        for row in rows {
            records.push(self.row_to_record(&row)?);
        }

        Ok(records)
    }

    pub async fn list_enabled_models(&self) -> Result<Vec<DbModelRecord>, String> {
        let rows = sqlx::query(
            r#"SELECT id, logical_model, provider_id, upstream_model, capabilities_json,
                      enabled, priority, created_at, updated_at
               FROM model_registry_records
               WHERE enabled = 1
               ORDER BY priority DESC, logical_model ASC"#,
        )
        .fetch_all(&self.pool)
        .await
        .map_err(|e| e.to_string())?;

        let mut records = Vec::new();
        for row in rows {
            records.push(self.row_to_record(&row)?);
        }

        Ok(records)
    }

    pub async fn get_model(&self, id: &str) -> Result<Option<DbModelRecord>, String> {
        let row = sqlx::query(
            r#"SELECT id, logical_model, provider_id, upstream_model, capabilities_json,
                      enabled, priority, created_at, updated_at
               FROM model_registry_records WHERE id = ?"#,
        )
        .bind(id)
        .fetch_optional(&self.pool)
        .await
        .map_err(|e| e.to_string())?;

        match row {
            Some(row) => Ok(Some(self.row_to_record(&row)?)),
            None => Ok(None),
        }
    }

    pub async fn get_model_by_logical_and_provider(
        &self,
        logical_model: &str,
        provider_id: &str,
    ) -> Result<Option<DbModelRecord>, String> {
        let row = sqlx::query(
            r#"SELECT id, logical_model, provider_id, upstream_model, capabilities_json,
                      enabled, priority, created_at, updated_at
               FROM model_registry_records
               WHERE logical_model = ? AND provider_id = ?"#,
        )
        .bind(logical_model)
        .bind(provider_id)
        .fetch_optional(&self.pool)
        .await
        .map_err(|e| e.to_string())?;

        match row {
            Some(row) => Ok(Some(self.row_to_record(&row)?)),
            None => Ok(None),
        }
    }

    pub async fn find_by_logical_model(
        &self,
        logical_model: &str,
    ) -> Result<Vec<DbModelRecord>, String> {
        let rows = sqlx::query(
            r#"SELECT id, logical_model, provider_id, upstream_model, capabilities_json,
                      enabled, priority, created_at, updated_at
               FROM model_registry_records
               WHERE logical_model = ? AND enabled = 1
               ORDER BY priority DESC"#,
        )
        .bind(logical_model)
        .fetch_all(&self.pool)
        .await
        .map_err(|e| e.to_string())?;

        let mut records = Vec::new();
        for row in rows {
            records.push(self.row_to_record(&row)?);
        }

        Ok(records)
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

        sqlx::query(
            r#"INSERT INTO model_registry_records
               (id, logical_model, provider_id, upstream_model, capabilities_json,
                enabled, priority, created_at, updated_at)
               VALUES (?, ?, ?, ?, ?, ?, ?, ?, ?)"#,
        )
        .bind(&id)
        .bind(&input.logical_model)
        .bind(&input.provider_id)
        .bind(&input.upstream_model)
        .bind(&capabilities_json)
        .bind(input.enabled)
        .bind(input.priority)
        .bind(now.to_rfc3339())
        .bind(now.to_rfc3339())
        .execute(&self.pool)
        .await
        .map_err(|e| {
            if e.to_string().contains("UNIQUE constraint failed") {
                "model_already_exists: a model with this logical_model and provider_id already exists".to_string()
            } else {
                e.to_string()
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
        let mut updates = Vec::new();
        let mut bindings: Vec<String> = Vec::new();

        if let Some(logical_model) = &input.logical_model {
            updates.push("logical_model = ?");
            bindings.push(logical_model.clone());
        }

        if let Some(provider_id) = &input.provider_id {
            updates.push("provider_id = ?");
            bindings.push(provider_id.clone());
        }

        if let Some(upstream_model) = &input.upstream_model {
            updates.push("upstream_model = ?");
            bindings.push(upstream_model.clone());
        }

        if let Some(capabilities) = &input.capabilities {
            updates.push("capabilities_json = ?");
            bindings.push(serde_json::to_string(capabilities).map_err(|e| e.to_string())?);
        }

        if let Some(enabled) = input.enabled {
            updates.push("enabled = ?");
            bindings.push(if enabled { "1" } else { "0" }.to_string());
        }

        if let Some(priority) = input.priority {
            updates.push("priority = ?");
            bindings.push(priority.to_string());
        }

        if !updates.is_empty() {
            updates.push("updated_at = ?");
            bindings.push(now.to_rfc3339());
            bindings.push(id.to_string());

            let query = format!(
                "UPDATE model_registry_records SET {} WHERE id = ?",
                updates.join(", ")
            );

            let mut q = sqlx::query(&query);
            for b in &bindings {
                q = q.bind(b);
            }

            q.execute(&self.pool).await.map_err(|e| {
                if e.to_string().contains("UNIQUE constraint failed") {
                    "model_already_exists: a model with this logical_model and provider_id already exists".to_string()
                } else {
                    e.to_string()
                }
            })?;
        }

        self.get_model(id)
            .await?
            .ok_or_else(|| "model not found after update".to_string())
    }

    pub async fn delete_model(&self, id: &str) -> Result<(), String> {
        let result = sqlx::query("DELETE FROM model_registry_records WHERE id = ?")
            .bind(id)
            .execute(&self.pool)
            .await
            .map_err(|e| e.to_string())?;

        if result.rows_affected() == 0 {
            return Err("model not found".to_string());
        }

        Ok(())
    }

    pub async fn list_model_metadata(&self) -> Result<Vec<DbModelMetadataRecord>, String> {
        let rows = sqlx::query(
            r#"SELECT model_id, models_dev_provider, mode, input_cost_per_token_nano,
                      output_cost_per_token_nano, cache_read_input_cost_per_token_nano,
                      output_cost_per_reasoning_token_nano, max_input_tokens, max_output_tokens,
                      max_tokens, raw_json, source, updated_at
               FROM model_metadata_records
               ORDER BY model_id ASC"#,
        )
        .fetch_all(&self.pool)
        .await
        .map_err(|e| e.to_string())?;

        rows.iter()
            .map(|row| self.row_to_model_metadata(row))
            .collect()
    }

    pub async fn list_priced_model_ids(&self) -> Result<std::collections::HashSet<String>, String> {
        let rows: Vec<(String,)> = sqlx::query_as(
            "SELECT model_id FROM model_metadata_records WHERE input_cost_per_token_nano IS NOT NULL AND output_cost_per_token_nano IS NOT NULL",
        )
        .fetch_all(&self.pool)
        .await
        .map_err(|e| e.to_string())?;
        Ok(rows.into_iter().map(|(id,)| id).collect())
    }

    pub async fn get_model_metadata(
        &self,
        model_id: &str,
    ) -> Result<Option<DbModelMetadataRecord>, String> {
        let row = sqlx::query(
            r#"SELECT model_id, models_dev_provider, mode, input_cost_per_token_nano,
                      output_cost_per_token_nano, cache_read_input_cost_per_token_nano,
                      output_cost_per_reasoning_token_nano, max_input_tokens, max_output_tokens,
                      max_tokens, raw_json, source, updated_at
               FROM model_metadata_records
               WHERE model_id = ?"#,
        )
        .bind(model_id)
        .fetch_optional(&self.pool)
        .await
        .map_err(|e| e.to_string())?;

        match row {
            Some(row) => Ok(Some(self.row_to_model_metadata(&row)?)),
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
        let output_cost_per_reasoning_token_nano = row
            .output_cost_per_reasoning_token_nano
            .map(|v| v.parse::<i128>())
            .transpose()
            .map_err(|_| "invalid output_cost_per_reasoning_token_nano".to_string())?;

        Ok(Some(ModelPricing {
            input_cost_per_token_nano,
            output_cost_per_token_nano,
            cache_read_input_cost_per_token_nano,
            output_cost_per_reasoning_token_nano,
        }))
    }

    pub async fn upsert_model_metadata(
        &self,
        model_id: &str,
        input: UpsertModelMetadataInput,
    ) -> Result<DbModelMetadataRecord, String> {
        let now = Utc::now().to_rfc3339();
        sqlx::query(
            r#"INSERT INTO model_metadata_records
                (model_id, models_dev_provider, mode, input_cost_per_token_nano, output_cost_per_token_nano,
                 cache_read_input_cost_per_token_nano, output_cost_per_reasoning_token_nano,
                 max_input_tokens, max_output_tokens, max_tokens, raw_json, source, updated_at)
               VALUES (?, ?, ?, ?, ?, ?, ?, ?, ?, ?, '{}', 'manual', ?)
               ON CONFLICT(model_id) DO UPDATE SET
                 models_dev_provider = COALESCE(?, models_dev_provider),
                 mode = COALESCE(?, mode),
                 input_cost_per_token_nano = ?,
                 output_cost_per_token_nano = ?,
                 cache_read_input_cost_per_token_nano = ?,
                 output_cost_per_reasoning_token_nano = ?,
                 max_input_tokens = COALESCE(?, max_input_tokens),
                 max_output_tokens = COALESCE(?, max_output_tokens),
                 max_tokens = COALESCE(?, max_tokens),
                 source = 'manual',
                 updated_at = ?"#,
        )
        // INSERT binds
        .bind(model_id)
        .bind(&input.models_dev_provider)
        .bind(&input.mode)
        .bind(&input.input_cost_per_token_nano)
        .bind(&input.output_cost_per_token_nano)
        .bind(&input.cache_read_input_cost_per_token_nano)
        .bind(&input.output_cost_per_reasoning_token_nano)
        .bind(input.max_input_tokens)
        .bind(input.max_output_tokens)
        .bind(input.max_tokens)
        .bind(&now)
        // UPDATE binds
        .bind(&input.models_dev_provider)
        .bind(&input.mode)
        .bind(&input.input_cost_per_token_nano)
        .bind(&input.output_cost_per_token_nano)
        .bind(&input.cache_read_input_cost_per_token_nano)
        .bind(&input.output_cost_per_reasoning_token_nano)
        .bind(input.max_input_tokens)
        .bind(input.max_output_tokens)
        .bind(input.max_tokens)
        .bind(&now)
        .execute(&self.pool)
        .await
        .map_err(|e| e.to_string())?;

        self.get_model_metadata(model_id)
            .await?
            .ok_or_else(|| "upsert succeeded but record not found".to_string())
    }

    pub async fn delete_model_metadata(&self, model_id: &str) -> Result<bool, String> {
        let result = sqlx::query("DELETE FROM model_metadata_records WHERE model_id = ?")
            .bind(model_id)
            .execute(&self.pool)
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
        let mut tx = self.pool.begin().await.map_err(|e| e.to_string())?;

        let deleted: u64 =
            sqlx::query("DELETE FROM model_metadata_records WHERE source != 'manual'")
                .execute(&mut *tx)
                .await
                .map_err(|e| e.to_string())?
                .rows_affected();

        let mut upserted = 0usize;
        let mut skipped = 0usize;

        for (model_name, variants) in &grouped {
            if !has_positive_input_variant(variants) {
                continue;
            }
            let existing_source: Option<String> =
                sqlx::query_scalar("SELECT source FROM model_metadata_records WHERE model_id = ?")
                    .bind(model_name)
                    .fetch_optional(&mut *tx)
                    .await
                    .map_err(|e| e.to_string())?;

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

            sqlx::query(
                r#"INSERT INTO model_metadata_records
                    (model_id, models_dev_provider, mode, input_cost_per_token_nano, output_cost_per_token_nano,
                     cache_read_input_cost_per_token_nano, output_cost_per_reasoning_token_nano,
                     max_input_tokens, max_output_tokens, max_tokens, raw_json, source, updated_at)
                   VALUES (?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, 'models_dev', ?)
                   ON CONFLICT(model_id) DO UPDATE SET
                     models_dev_provider=excluded.models_dev_provider,
                     mode=excluded.mode,
                     input_cost_per_token_nano=excluded.input_cost_per_token_nano,
                     output_cost_per_token_nano=excluded.output_cost_per_token_nano,
                     cache_read_input_cost_per_token_nano=excluded.cache_read_input_cost_per_token_nano,
                     output_cost_per_reasoning_token_nano=excluded.output_cost_per_reasoning_token_nano,
                     max_input_tokens=excluded.max_input_tokens,
                     max_output_tokens=excluded.max_output_tokens,
                     max_tokens=excluded.max_tokens,
                     raw_json=excluded.raw_json,
                     source=excluded.source,
                     updated_at=excluded.updated_at"#,
            )
            .bind(model_name)
            .bind(&winner.provider_id)
            .bind(mode)
            .bind(&winner.input_cost_nano)
            .bind(&winner.output_cost_nano)
            .bind(&winner.cache_read_nano)
            .bind(&winner.reasoning_nano)
            .bind(winner.max_input_tokens)
            .bind(winner.max_output_tokens)
            .bind(winner.max_tokens)
            .bind(raw_json.to_string())
            .bind(&fetched_at)
            .execute(&mut *tx)
            .await
            .map_err(|e| e.to_string())?;
            upserted += 1;
        }
        tx.commit().await.map_err(|e| e.to_string())?;
        Ok(ModelMetadataSyncResult {
            success: true,
            upserted,
            skipped,
            deleted,
            fetched_at,
        })
    }

    fn row_to_record(&self, row: &sqlx::sqlite::SqliteRow) -> Result<DbModelRecord, String> {
        let capabilities_json: String = row
            .try_get("capabilities_json")
            .map_err(|e| e.to_string())?;
        let capabilities: ModelCapabilities =
            serde_json::from_str(&capabilities_json).map_err(|e| e.to_string())?;

        let created_at_str: String = row.try_get("created_at").map_err(|e| e.to_string())?;
        let created_at = DateTime::parse_from_rfc3339(&created_at_str)
            .map_err(|e| e.to_string())?
            .with_timezone(&Utc);

        let updated_at_str: String = row.try_get("updated_at").map_err(|e| e.to_string())?;
        let updated_at = DateTime::parse_from_rfc3339(&updated_at_str)
            .map_err(|e| e.to_string())?
            .with_timezone(&Utc);

        Ok(DbModelRecord {
            id: row.try_get("id").map_err(|e| e.to_string())?,
            logical_model: row.try_get("logical_model").map_err(|e| e.to_string())?,
            provider_id: row.try_get("provider_id").map_err(|e| e.to_string())?,
            upstream_model: row.try_get("upstream_model").map_err(|e| e.to_string())?,
            capabilities,
            enabled: row
                .try_get::<i32, _>("enabled")
                .map_err(|e| e.to_string())?
                == 1,
            priority: row.try_get("priority").map_err(|e| e.to_string())?,
            created_at,
            updated_at,
        })
    }

    fn row_to_model_metadata(
        &self,
        row: &sqlx::sqlite::SqliteRow,
    ) -> Result<DbModelMetadataRecord, String> {
        let updated_at_str: String = row.try_get("updated_at").map_err(|e| e.to_string())?;
        let updated_at = DateTime::parse_from_rfc3339(&updated_at_str)
            .map_err(|e| e.to_string())?
            .with_timezone(&Utc);
        let raw_json_str: String = row.try_get("raw_json").map_err(|e| e.to_string())?;
        let raw_json: Value = serde_json::from_str(&raw_json_str).map_err(|e| e.to_string())?;
        Ok(DbModelMetadataRecord {
            model_id: row.try_get("model_id").map_err(|e| e.to_string())?,
            models_dev_provider: row.try_get("models_dev_provider").unwrap_or(None),
            mode: row.try_get("mode").unwrap_or(None),
            input_cost_per_token_nano: row.try_get("input_cost_per_token_nano").unwrap_or(None),
            output_cost_per_token_nano: row.try_get("output_cost_per_token_nano").unwrap_or(None),
            cache_read_input_cost_per_token_nano: row
                .try_get("cache_read_input_cost_per_token_nano")
                .unwrap_or(None),
            output_cost_per_reasoning_token_nano: row
                .try_get("output_cost_per_reasoning_token_nano")
                .unwrap_or(None),
            max_input_tokens: row.try_get("max_input_tokens").unwrap_or(None),
            max_output_tokens: row.try_get("max_output_tokens").unwrap_or(None),
            max_tokens: row.try_get("max_tokens").unwrap_or(None),
            raw_json,
            source: row
                .try_get("source")
                .unwrap_or_else(|_| "models_dev".to_string()),
            updated_at,
        })
    }
}

async fn migrate_model_metadata_provider_column(pool: &Pool<Sqlite>) -> Result<(), String> {
    let col_rows = sqlx::query("SELECT name FROM pragma_table_info('model_metadata_records')")
        .fetch_all(pool)
        .await
        .map_err(|e| e.to_string())?;
    let columns: Vec<String> = col_rows
        .iter()
        .filter_map(|r| r.try_get::<String, _>("name").ok())
        .collect();

    let has_models_dev_provider = columns.iter().any(|c| c == "models_dev_provider");
    let legacy_provider_column = columns
        .iter()
        .find(|c| c.ends_with("_provider") && c.as_str() != "models_dev_provider")
        .cloned();

    let Some(legacy_provider_column) = legacy_provider_column else {
        return Ok(());
    };

    let mut tx = pool.begin().await.map_err(|e| e.to_string())?;

    sqlx::query("DROP TABLE IF EXISTS model_metadata_records_legacy")
        .execute(&mut *tx)
        .await
        .map_err(|e| e.to_string())?;

    sqlx::query("ALTER TABLE model_metadata_records RENAME TO model_metadata_records_legacy")
        .execute(&mut *tx)
        .await
        .map_err(|e| e.to_string())?;

    sqlx::query(
        r#"CREATE TABLE model_metadata_records (
            model_id TEXT PRIMARY KEY,
            models_dev_provider TEXT,
            mode TEXT,
            input_cost_per_token_nano TEXT,
            output_cost_per_token_nano TEXT,
            cache_read_input_cost_per_token_nano TEXT,
            output_cost_per_reasoning_token_nano TEXT,
            max_input_tokens INTEGER,
            max_output_tokens INTEGER,
            max_tokens INTEGER,
            raw_json TEXT NOT NULL,
            source TEXT NOT NULL,
            updated_at TEXT NOT NULL
        )"#,
    )
    .execute(&mut *tx)
    .await
    .map_err(|e| e.to_string())?;

    let backfill_query = if has_models_dev_provider {
        format!(
            r#"INSERT INTO model_metadata_records
            (model_id, models_dev_provider, mode, input_cost_per_token_nano, output_cost_per_token_nano,
             cache_read_input_cost_per_token_nano, output_cost_per_reasoning_token_nano,
             max_input_tokens, max_output_tokens, max_tokens, raw_json, source, updated_at)
           SELECT model_id, COALESCE(models_dev_provider, {legacy_provider_column}), mode,
                  input_cost_per_token_nano, output_cost_per_token_nano,
                  cache_read_input_cost_per_token_nano, output_cost_per_reasoning_token_nano,
                  max_input_tokens, max_output_tokens, max_tokens, raw_json, source, updated_at
           FROM model_metadata_records_legacy"#
        )
    } else {
        format!(
            r#"INSERT INTO model_metadata_records
            (model_id, models_dev_provider, mode, input_cost_per_token_nano, output_cost_per_token_nano,
             cache_read_input_cost_per_token_nano, output_cost_per_reasoning_token_nano,
             max_input_tokens, max_output_tokens, max_tokens, raw_json, source, updated_at)
           SELECT model_id, {legacy_provider_column}, mode,
                  input_cost_per_token_nano, output_cost_per_token_nano,
                  cache_read_input_cost_per_token_nano, output_cost_per_reasoning_token_nano,
                  max_input_tokens, max_output_tokens, max_tokens, raw_json, source, updated_at
           FROM model_metadata_records_legacy"#
        )
    };

    sqlx::query(&backfill_query)
        .execute(&mut *tx)
        .await
        .map_err(|e| e.to_string())?;

    sqlx::query("DROP TABLE model_metadata_records_legacy")
        .execute(&mut *tx)
        .await
        .map_err(|e| e.to_string())?;

    tx.commit().await.map_err(|e| e.to_string())
}

/// Strip `provider/` prefix from model_metadata_records PKs.
/// Keeps the row with the latest updated_at when duplicates arise.
async fn migrate_model_metadata_bare_model_id(pool: &Pool<Sqlite>) -> Result<(), String> {
    let has_prefixed: bool = sqlx::query_scalar(
        "SELECT EXISTS(SELECT 1 FROM model_metadata_records WHERE model_id LIKE '%/%')",
    )
    .fetch_one(pool)
    .await
    .map_err(|e| e.to_string())?;

    if !has_prefixed {
        return Ok(());
    }

    let rows = sqlx::query(
        "SELECT model_id, models_dev_provider, mode, input_cost_per_token_nano, output_cost_per_token_nano, cache_read_input_cost_per_token_nano, output_cost_per_reasoning_token_nano, max_input_tokens, max_output_tokens, max_tokens, raw_json, source, updated_at FROM model_metadata_records ORDER BY updated_at DESC",
    )
    .fetch_all(pool)
    .await
    .map_err(|e| e.to_string())?;

    let mut seen = std::collections::HashSet::new();
    let mut keepers: Vec<(String, sqlx::sqlite::SqliteRow)> = Vec::new();

    for row in rows {
        let old_id: String = row.try_get("model_id").map_err(|e| e.to_string())?;
        let bare = normalize_model_id(&old_id, None);
        if seen.insert(bare.clone()) {
            keepers.push((bare, row));
        }
    }

    let mut tx = pool.begin().await.map_err(|e| e.to_string())?;

    sqlx::query("DELETE FROM model_metadata_records")
        .execute(&mut *tx)
        .await
        .map_err(|e| e.to_string())?;

    for (bare_id, row) in &keepers {
        sqlx::query(
            r#"INSERT INTO model_metadata_records
                (model_id, models_dev_provider, mode, input_cost_per_token_nano, output_cost_per_token_nano,
                 cache_read_input_cost_per_token_nano, output_cost_per_reasoning_token_nano,
                 max_input_tokens, max_output_tokens, max_tokens, raw_json, source, updated_at)
               VALUES (?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?)"#,
        )
        .bind(bare_id)
        .bind(row.try_get::<Option<String>, _>("models_dev_provider").unwrap_or(None))
        .bind(row.try_get::<Option<String>, _>("mode").unwrap_or(None))
        .bind(row.try_get::<Option<String>, _>("input_cost_per_token_nano").unwrap_or(None))
        .bind(row.try_get::<Option<String>, _>("output_cost_per_token_nano").unwrap_or(None))
        .bind(row.try_get::<Option<String>, _>("cache_read_input_cost_per_token_nano").unwrap_or(None))
        .bind(row.try_get::<Option<String>, _>("output_cost_per_reasoning_token_nano").unwrap_or(None))
        .bind(row.try_get::<Option<i64>, _>("max_input_tokens").unwrap_or(None))
        .bind(row.try_get::<Option<i64>, _>("max_output_tokens").unwrap_or(None))
        .bind(row.try_get::<Option<i64>, _>("max_tokens").unwrap_or(None))
        .bind(row.try_get::<String, _>("raw_json").unwrap_or_else(|_| "{}".to_string()))
        .bind(row.try_get::<String, _>("source").unwrap_or_else(|_| "models_dev".to_string()))
        .bind(row.try_get::<String, _>("updated_at").map_err(|e| e.to_string())?)
        .execute(&mut *tx)
        .await
        .map_err(|e| e.to_string())?;
    }

    tx.commit().await.map_err(|e| e.to_string())
}

struct SyncProviderVariant {
    provider_id: String,
    family: Option<String>,
    input_cost_nano: Option<String>,
    output_cost_nano: Option<String>,
    cache_read_nano: Option<String>,
    reasoning_nano: Option<String>,
    max_input_tokens: Option<i64>,
    max_output_tokens: Option<i64>,
    max_tokens: Option<i64>,
    raw: Value,
}

fn pick_best_variant(variants: &[SyncProviderVariant]) -> &SyncProviderVariant {
    variants
        .iter()
        .min_by(|a, b| {
            let cost_a = a
                .input_cost_nano
                .as_ref()
                .and_then(|s| s.parse::<i128>().ok())
                .filter(|&v| v > 0)
                .unwrap_or(i128::MAX);
            let cost_b = b
                .input_cost_nano
                .as_ref()
                .and_then(|s| s.parse::<i128>().ok())
                .filter(|&v| v > 0)
                .unwrap_or(i128::MAX);
            cost_a.cmp(&cost_b)
        })
        .unwrap()
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

/// Convert a models.dev cost value (USD per 1M tokens) to nano-dollar per token string.
/// Formula: nano_per_token = trunc(cost_per_1m * 1000)
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

/// Normalize a model ID to canonical form.
/// 1) Take the last `/` segment.
/// 2) Strip provider prefixes (`provider--` or `provider.`) only when provider is known.
/// 3) Lowercase.
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
