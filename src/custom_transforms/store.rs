//! Persistence and runtime snapshot for custom JS transforms
//! (`custom-js-transforms.spec.md` §4–§6).

use super::frontmatter::{CustomTransformVisibility, parse_frontmatter};
use super::sandbox::{self, SandboxLimits};
use crate::db::DbPool;
use crate::transforms::{CustomTransformSource, DynTransform, Phase, TransformScope};
use chrono::Utc;
use sea_orm::{ConnectionTrait, TransactionTrait};
use serde::Serialize;
use serde_json::{Value, json};
use std::collections::HashMap;
use std::sync::{Arc, RwLock};

const DEFAULT_SOURCE_MAX_BYTES: usize = 262_144;

pub fn source_max_bytes() -> usize {
    std::env::var("MONOIZE_CUSTOM_JS_SOURCE_MAX_BYTES")
        .ok()
        .and_then(|raw| raw.trim().parse::<usize>().ok())
        .filter(|value| *value > 0)
        .unwrap_or(DEFAULT_SOURCE_MAX_BYTES)
}

/// CJS-VAL-4 registry default when no `configSchema` is declared.
pub fn default_config_schema() -> Value {
    json!({"type": "object", "properties": {}})
}

/// One compiled snapshot entry for an enabled custom transform (CJS-RT-1).
pub struct CustomTransformEntry {
    pub id: String,
    pub name: String,
    pub description: String,
    pub author: String,
    pub source: String,
    pub visibility: CustomTransformVisibility,
    pub phases: Vec<Phase>,
    pub scopes: Vec<TransformScope>,
    pub config_schema: Option<Value>,
}

/// Point-in-time map of every enabled custom transform, atomically replaced
/// after each successful mutation.
#[derive(Default)]
pub struct CustomTransformSnapshot {
    entries: HashMap<String, Arc<CustomTransformEntry>>,
}

impl CustomTransformSnapshot {
    pub fn from_entries(entries: HashMap<String, Arc<CustomTransformEntry>>) -> Self {
        Self { entries }
    }

    pub fn get(&self, id: &str) -> Option<&Arc<CustomTransformEntry>> {
        self.entries.get(id)
    }

    pub fn values(&self) -> impl Iterator<Item = &Arc<CustomTransformEntry>> {
        self.entries.values()
    }

    pub fn len(&self) -> usize {
        self.entries.len()
    }

    pub fn is_empty(&self) -> bool {
        self.entries.is_empty()
    }
}

impl CustomTransformSource for CustomTransformSnapshot {
    fn resolve_custom(&self, id: &str) -> Option<Arc<dyn DynTransform>> {
        self.entries
            .get(id)
            .map(|entry| entry.clone() as Arc<dyn DynTransform>)
    }
}

/// Shared, atomically swappable snapshot pointer. The default handle holds an
/// empty snapshot, which resolves no custom transform.
#[derive(Clone, Default)]
pub struct CustomTransformSnapshotHandle {
    inner: Arc<RwLock<Arc<CustomTransformSnapshot>>>,
}

impl CustomTransformSnapshotHandle {
    pub fn get(&self) -> Arc<CustomTransformSnapshot> {
        self.inner
            .read()
            .unwrap_or_else(|error| error.into_inner())
            .clone()
    }

    fn set(&self, next: Arc<CustomTransformSnapshot>) {
        *self
            .inner
            .write()
            .unwrap_or_else(|error| error.into_inner()) = next;
    }
}

/// Full stored row in API shape (CJS-API-1). `config_schema` always carries a
/// concrete object (stored schema or the CJS-VAL-4 default).
#[derive(Debug, Clone, Serialize)]
pub struct CustomTransformRecord {
    pub id: String,
    pub name: String,
    pub description: String,
    pub author: String,
    pub source: String,
    pub enabled: bool,
    pub visibility: CustomTransformVisibility,
    pub phases: Vec<Phase>,
    pub scopes: Vec<TransformScope>,
    pub config_schema: Value,
    pub created_at: String,
    pub updated_at: String,
}

#[derive(Debug)]
pub enum CustomTransformError {
    /// HTTP 400 `invalid_custom_transform`.
    Invalid(String),
    /// HTTP 409 `custom_transform_exists`.
    Exists,
    /// HTTP 404.
    NotFound,
    /// HTTP 500.
    Internal(String),
}

impl CustomTransformError {
    fn internal(error: impl ToString) -> Self {
        Self::Internal(error.to_string())
    }
}

#[derive(Clone)]
pub struct CustomTransformStore {
    db: DbPool,
    snapshot: CustomTransformSnapshotHandle,
}

const SELECT_COLUMNS: &str = "id, name, description, author, source, enabled, visibility, \
                              phases, scopes, config_schema, created_at, updated_at";

impl CustomTransformStore {
    pub async fn new(db: DbPool) -> Result<Self, String> {
        let store = Self {
            db,
            snapshot: CustomTransformSnapshotHandle::default(),
        };
        store.reload().await?;
        Ok(store)
    }

    /// The shared handle for read-side consumers (API-key validation).
    pub fn snapshot_handle(&self) -> CustomTransformSnapshotHandle {
        self.snapshot.clone()
    }

    /// Point-in-time snapshot of enabled custom transforms.
    pub fn snapshot(&self) -> Arc<CustomTransformSnapshot> {
        self.snapshot.get()
    }

    /// CJS-RT-1/CJS-RT-7: rebuilds the snapshot from the enabled rows.
    pub async fn reload(&self) -> Result<(), String> {
        let rows = self
            .db
            .read()
            .query_all(self.db.stmt(
                &format!(
                    "SELECT {SELECT_COLUMNS} FROM custom_transforms WHERE enabled = $1 ORDER BY id ASC"
                ),
                vec![sea_orm::Value::from(true)],
            ))
            .await
            .map_err(|e| e.to_string())?;

        let mut entries = HashMap::with_capacity(rows.len());
        for row in &rows {
            let record = row_to_record(row)?;
            entries.insert(
                record.id.clone(),
                Arc::new(CustomTransformEntry {
                    id: record.id,
                    name: record.name,
                    description: record.description,
                    author: record.author,
                    source: record.source,
                    visibility: record.visibility,
                    phases: record.phases,
                    scopes: record.scopes,
                    config_schema: Some(record.config_schema),
                }),
            );
        }
        self.snapshot.set(Arc::new(CustomTransformSnapshot { entries }));
        Ok(())
    }

    /// CJS-API-1 listing: every row including disabled ones, ordered by id.
    pub async fn list(&self) -> Result<Vec<CustomTransformRecord>, String> {
        let rows = self
            .db
            .read()
            .query_all(self.db.stmt(
                &format!("SELECT {SELECT_COLUMNS} FROM custom_transforms ORDER BY id ASC"),
                vec![],
            ))
            .await
            .map_err(|e| e.to_string())?;
        rows.iter().map(row_to_record).collect()
    }

    pub async fn get(&self, id: &str) -> Result<Option<CustomTransformRecord>, String> {
        let row = self
            .db
            .read()
            .query_one(self.db.stmt(
                &format!("SELECT {SELECT_COLUMNS} FROM custom_transforms WHERE id = $1"),
                vec![id.into()],
            ))
            .await
            .map_err(|e| e.to_string())?;
        row.as_ref().map(row_to_record).transpose()
    }

    /// CJS-API-2 create: §3 validation, insert, epoch bump, snapshot reload.
    pub async fn create(
        &self,
        source: String,
        enabled: bool,
    ) -> Result<CustomTransformRecord, CustomTransformError> {
        let validated = validate_source_for_save(&source, None).await?;
        let now = Utc::now().to_rfc3339();

        let write_guard = self.db.write().await;
        let txn = write_guard
            .begin()
            .await
            .map_err(CustomTransformError::internal)?;
        let insert = txn
            .execute(self.db.stmt(
                "INSERT INTO custom_transforms
                 (id, name, description, author, source, enabled, visibility, phases, scopes,
                  config_schema, created_at, updated_at)
                 VALUES ($1, $2, $3, $4, $5, $6, $7, $8, $9, $10, $11, $12)",
                vec![
                    validated.meta.id.clone().into(),
                    validated.meta.name.clone().into(),
                    validated.meta.description.clone().into(),
                    validated.meta.author.clone().into(),
                    source.clone().into(),
                    sea_orm::Value::from(enabled),
                    validated.meta.visibility.as_str().into(),
                    validated.phases_json.clone().into(),
                    validated.scopes_json.clone().into(),
                    validated.schema_json.clone().into(),
                    now.clone().into(),
                    now.into(),
                ],
            ))
            .await;
        if let Err(error) = insert {
            let message = error.to_string();
            if message.contains("UNIQUE") || message.contains("unique") || message.contains("duplicate")
            {
                return Err(CustomTransformError::Exists);
            }
            return Err(CustomTransformError::Internal(message));
        }
        crate::settings::bump_config_epoch_in_tx(&self.db, &txn)
            .await
            .map_err(CustomTransformError::Internal)?;
        txn.commit().await.map_err(CustomTransformError::internal)?;
        drop(write_guard);

        self.reload().await.map_err(CustomTransformError::Internal)?;
        self.get(&validated.meta.id)
            .await
            .map_err(CustomTransformError::Internal)?
            .ok_or_else(|| {
                CustomTransformError::Internal("custom transform missing after create".to_string())
            })
    }

    /// CJS-API-3 update: optional source re-validation (CJS-VAL-5) and/or
    /// enabled toggle, epoch bump, snapshot reload.
    pub async fn update(
        &self,
        id: &str,
        source: Option<String>,
        enabled: Option<bool>,
    ) -> Result<CustomTransformRecord, CustomTransformError> {
        if source.is_none() && enabled.is_none() {
            return Err(CustomTransformError::Invalid(
                "at least one of 'source' or 'enabled' must be present".to_string(),
            ));
        }
        let validated = match &source {
            Some(source) => Some(validate_source_for_save(source, Some(id)).await?),
            None => None,
        };
        let now = Utc::now().to_rfc3339();

        let mut set_clauses = Vec::new();
        let mut values: Vec<sea_orm::Value> = Vec::new();
        let mut idx = 1u32;
        if let (Some(source), Some(validated)) = (&source, &validated) {
            for (column, value) in [
                ("name", sea_orm::Value::from(validated.meta.name.clone())),
                (
                    "description",
                    sea_orm::Value::from(validated.meta.description.clone()),
                ),
                ("author", sea_orm::Value::from(validated.meta.author.clone())),
                ("source", sea_orm::Value::from(source.clone())),
                (
                    "visibility",
                    sea_orm::Value::from(validated.meta.visibility.as_str()),
                ),
                ("phases", sea_orm::Value::from(validated.phases_json.clone())),
                ("scopes", sea_orm::Value::from(validated.scopes_json.clone())),
                (
                    "config_schema",
                    sea_orm::Value::from(validated.schema_json.clone()),
                ),
            ] {
                set_clauses.push(format!("{column} = ${idx}"));
                values.push(value);
                idx += 1;
            }
        }
        if let Some(enabled) = enabled {
            set_clauses.push(format!("enabled = ${idx}"));
            values.push(sea_orm::Value::from(enabled));
            idx += 1;
        }
        set_clauses.push(format!("updated_at = ${idx}"));
        values.push(now.into());
        idx += 1;
        values.push(id.to_string().into());

        let sql = format!(
            "UPDATE custom_transforms SET {} WHERE id = ${idx}",
            set_clauses.join(", ")
        );

        let write_guard = self.db.write().await;
        let txn = write_guard
            .begin()
            .await
            .map_err(CustomTransformError::internal)?;
        let result = txn
            .execute(self.db.stmt(&sql, values))
            .await
            .map_err(CustomTransformError::internal)?;
        if result.rows_affected() == 0 {
            txn.rollback().await.map_err(CustomTransformError::internal)?;
            return Err(CustomTransformError::NotFound);
        }
        crate::settings::bump_config_epoch_in_tx(&self.db, &txn)
            .await
            .map_err(CustomTransformError::Internal)?;
        txn.commit().await.map_err(CustomTransformError::internal)?;
        drop(write_guard);

        self.reload().await.map_err(CustomTransformError::Internal)?;
        self.get(id)
            .await
            .map_err(CustomTransformError::Internal)?
            .ok_or_else(|| {
                CustomTransformError::Internal("custom transform missing after update".to_string())
            })
    }

    /// CJS-API-4 delete: row removal, epoch bump, snapshot reload. Chains
    /// referencing the id are left untouched (they become no-ops per CJS-RT-3).
    pub async fn delete(&self, id: &str) -> Result<(), CustomTransformError> {
        let write_guard = self.db.write().await;
        let txn = write_guard
            .begin()
            .await
            .map_err(CustomTransformError::internal)?;
        let result = txn
            .execute(self.db.stmt(
                "DELETE FROM custom_transforms WHERE id = $1",
                vec![id.into()],
            ))
            .await
            .map_err(CustomTransformError::internal)?;
        if result.rows_affected() == 0 {
            txn.rollback().await.map_err(CustomTransformError::internal)?;
            return Err(CustomTransformError::NotFound);
        }
        crate::settings::bump_config_epoch_in_tx(&self.db, &txn)
            .await
            .map_err(CustomTransformError::Internal)?;
        txn.commit().await.map_err(CustomTransformError::internal)?;
        drop(write_guard);

        self.reload().await.map_err(CustomTransformError::Internal)
    }
}

struct ValidatedSource {
    meta: super::frontmatter::CustomTransformMeta,
    phases_json: String,
    scopes_json: String,
    schema_json: Option<String>,
}

/// Full §3 save-time validation: size bound, frontmatter, sandbox evaluation,
/// and the CJS-VAL-5 path-id equality check on update.
async fn validate_source_for_save(
    source: &str,
    expected_id: Option<&str>,
) -> Result<ValidatedSource, CustomTransformError> {
    let max_bytes = source_max_bytes();
    if source.len() > max_bytes {
        return Err(CustomTransformError::Invalid(format!(
            "source exceeds {max_bytes} bytes"
        )));
    }
    let meta = parse_frontmatter(source).map_err(CustomTransformError::Invalid)?;
    if let Some(expected_id) = expected_id {
        if meta.id != expected_id {
            return Err(CustomTransformError::Invalid(format!(
                "frontmatter id '{}' must equal the path id '{expected_id}'; \
                 renaming requires delete plus create",
                meta.id
            )));
        }
    }
    let schema = sandbox::validate_source(source.to_string(), SandboxLimits::from_env())
        .await
        .map_err(CustomTransformError::Invalid)?;

    let phases_json =
        serde_json::to_string(&meta.phases).map_err(CustomTransformError::internal)?;
    let scopes_json =
        serde_json::to_string(&meta.scopes).map_err(CustomTransformError::internal)?;
    let schema_json = schema
        .map(|schema| serde_json::to_string(&schema))
        .transpose()
        .map_err(CustomTransformError::internal)?;

    Ok(ValidatedSource {
        meta,
        phases_json,
        scopes_json,
        schema_json,
    })
}

fn decode_enabled(row: &sea_orm::QueryResult) -> Result<bool, String> {
    // CJS-DB-1 stores BOOLEAN. PostgreSQL returns bool; SQLite BOOLEAN is INTEGER 0/1.
    if let Ok(value) = row.try_get::<bool>("", "enabled") {
        return Ok(value);
    }
    match row.try_get::<i32>("", "enabled") {
        Ok(0) => Ok(false),
        Ok(1) => Ok(true),
        Ok(value) => Err(format!(
            "invalid persisted enabled: expected boolean or 0/1, got {value}"
        )),
        Err(error) => Err(format!("invalid persisted enabled: {error}")),
    }
}

fn row_to_record(row: &sea_orm::QueryResult) -> Result<CustomTransformRecord, String> {
    let enabled = decode_enabled(row)?;
    let visibility_raw: String = row.try_get("", "visibility").map_err(|e| e.to_string())?;
    let visibility = CustomTransformVisibility::parse(&visibility_raw)
        .ok_or_else(|| format!("invalid persisted visibility: {visibility_raw:?}"))?;
    let phases_json: String = row.try_get("", "phases").map_err(|e| e.to_string())?;
    let phases: Vec<Phase> = serde_json::from_str(&phases_json)
        .map_err(|error| format!("invalid persisted phases: {error}"))?;
    let scopes_json: String = row.try_get("", "scopes").map_err(|e| e.to_string())?;
    let scopes: Vec<TransformScope> = serde_json::from_str(&scopes_json)
        .map_err(|error| format!("invalid persisted scopes: {error}"))?;
    let schema_raw: Option<String> = row
        .try_get("", "config_schema")
        .map_err(|e| e.to_string())?;
    let config_schema = match schema_raw {
        None => default_config_schema(),
        Some(raw) => serde_json::from_str(&raw)
            .map_err(|error| format!("invalid persisted config_schema: {error}"))?,
    };

    Ok(CustomTransformRecord {
        id: row.try_get("", "id").map_err(|e| e.to_string())?,
        name: row.try_get("", "name").map_err(|e| e.to_string())?,
        description: row.try_get("", "description").map_err(|e| e.to_string())?,
        author: row.try_get("", "author").map_err(|e| e.to_string())?,
        source: row.try_get("", "source").map_err(|e| e.to_string())?,
        enabled,
        visibility,
        phases,
        scopes,
        config_schema,
        created_at: row.try_get("", "created_at").map_err(|e| e.to_string())?,
        updated_at: row.try_get("", "updated_at").map_err(|e| e.to_string())?,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::migration::Migrator;
    use crate::settings::read_config_epoch;
    use sea_orm_migration::MigratorTrait;

    const VALID_SOURCE: &str = r#"/**
 * @monoize-transform
 * id: js:test-rewrite
 * name: Test Rewrite
 * description: Rewrites the model field.
 * author: tester
 * phase: request
 * scopes: provider, global, api_key
 * visibility: user
 */
const configSchema = { type: "object", properties: { model: { type: "string" } } };
function transform(ctx) {
  if (ctx.config.model) ctx.data.model = ctx.config.model;
}
"#;

    async fn test_store() -> (DbPool, CustomTransformStore) {
        let db = DbPool::connect("sqlite::memory:").await.expect("db connects");
        {
            let write = db.write().await;
            Migrator::up(&*write, None).await.expect("migrates");
        }
        let store = CustomTransformStore::new(db.clone())
            .await
            .expect("store creates");
        (db, store)
    }

    #[tokio::test]
    async fn create_persists_derived_metadata_and_snapshot() {
        let (db, store) = test_store().await;
        let epoch_before = read_config_epoch(&db).await.expect("epoch reads");

        let record = store
            .create(VALID_SOURCE.to_string(), true)
            .await
            .expect("creates");
        assert_eq!(record.id, "js:test-rewrite");
        assert_eq!(record.name, "Test Rewrite");
        assert_eq!(record.author, "tester");
        assert_eq!(record.phases, vec![Phase::Request]);
        assert_eq!(record.visibility, CustomTransformVisibility::User);
        assert_eq!(
            record.config_schema["properties"]["model"]["type"],
            json!("string")
        );

        let snapshot = store.snapshot();
        assert!(snapshot.get("js:test-rewrite").is_some());

        let epoch_after = read_config_epoch(&db).await.expect("epoch reads");
        assert_eq!(epoch_after, epoch_before + 1);
    }

    #[tokio::test]
    async fn create_rejects_duplicate_id() {
        let (_db, store) = test_store().await;
        store
            .create(VALID_SOURCE.to_string(), true)
            .await
            .expect("creates");
        let error = store
            .create(VALID_SOURCE.to_string(), true)
            .await
            .expect_err("must conflict");
        assert!(matches!(error, CustomTransformError::Exists));
    }

    #[tokio::test]
    async fn create_rejects_invalid_frontmatter_and_bad_scripts() {
        let (_db, store) = test_store().await;
        let no_frontmatter = "function transform(ctx) {}";
        assert!(matches!(
            store.create(no_frontmatter.to_string(), true).await,
            Err(CustomTransformError::Invalid(_))
        ));

        let no_function = "/* @monoize-transform\nid: js:x\nname: N\ndescription: D\nauthor: a */\nvar y = 1;";
        assert!(matches!(
            store.create(no_function.to_string(), true).await,
            Err(CustomTransformError::Invalid(_))
        ));

        let throwing = "/* @monoize-transform\nid: js:x\nname: N\ndescription: D\nauthor: a */\nthrow new Error('nope');";
        assert!(matches!(
            store.create(throwing.to_string(), true).await,
            Err(CustomTransformError::Invalid(_))
        ));
    }

    #[tokio::test]
    async fn disable_removes_from_snapshot_but_keeps_row() {
        let (_db, store) = test_store().await;
        store
            .create(VALID_SOURCE.to_string(), true)
            .await
            .expect("creates");
        let record = store
            .update("js:test-rewrite", None, Some(false))
            .await
            .expect("updates");
        assert!(!record.enabled);
        assert!(store.snapshot().get("js:test-rewrite").is_none());
        assert_eq!(store.list().await.expect("lists").len(), 1);
    }

    #[tokio::test]
    async fn update_rejects_id_mismatch() {
        let (_db, store) = test_store().await;
        store
            .create(VALID_SOURCE.to_string(), true)
            .await
            .expect("creates");
        let renamed = VALID_SOURCE.replace("js:test-rewrite", "js:other-name");
        let error = store
            .update("js:test-rewrite", Some(renamed), None)
            .await
            .expect_err("must reject");
        assert!(matches!(error, CustomTransformError::Invalid(_)));
    }

    #[tokio::test]
    async fn update_requires_at_least_one_field_and_unknown_id_is_not_found() {
        let (_db, store) = test_store().await;
        assert!(matches!(
            store.update("js:missing", None, None).await,
            Err(CustomTransformError::Invalid(_))
        ));
        assert!(matches!(
            store.update("js:missing", None, Some(true)).await,
            Err(CustomTransformError::NotFound)
        ));
    }

    #[tokio::test]
    async fn delete_removes_row_and_snapshot_entry() {
        let (db, store) = test_store().await;
        store
            .create(VALID_SOURCE.to_string(), true)
            .await
            .expect("creates");
        let epoch_before = read_config_epoch(&db).await.expect("epoch reads");
        store.delete("js:test-rewrite").await.expect("deletes");
        assert!(store.snapshot().get("js:test-rewrite").is_none());
        assert!(store.list().await.expect("lists").is_empty());
        let epoch_after = read_config_epoch(&db).await.expect("epoch reads");
        assert_eq!(epoch_after, epoch_before + 1);
        assert!(matches!(
            store.delete("js:test-rewrite").await,
            Err(CustomTransformError::NotFound)
        ));
    }

    #[tokio::test]
    async fn default_schema_is_reported_when_none_declared() {
        let (_db, store) = test_store().await;
        let source = "/* @monoize-transform\nid: js:plain\nname: N\ndescription: D\nauthor: a */\nfunction transform(ctx) {}";
        let record = store
            .create(source.to_string(), true)
            .await
            .expect("creates");
        assert_eq!(record.config_schema, default_config_schema());
    }

    #[tokio::test]
    async fn snapshot_reload_uses_boolean_enabled_predicate() {
        let (_db, store) = test_store().await;
        store
            .create(VALID_SOURCE.to_string(), true)
            .await
            .expect("creates");
        store.reload().await.expect("reloads enabled snapshot");
        assert!(store.snapshot().get("js:test-rewrite").is_some());
        store
            .update("js:test-rewrite", None, Some(false))
            .await
            .expect("disables");
        store.reload().await.expect("reloads after disable");
        assert!(store.snapshot().get("js:test-rewrite").is_none());
    }
}
