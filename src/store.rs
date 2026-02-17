use async_trait::async_trait;
use serde_json::Value;
use sqlx::{sqlite::SqlitePoolOptions, Pool, Row, Sqlite};
use std::collections::HashMap;
use std::sync::Arc;
use tokio::sync::RwLock;
use std::path::PathBuf;

fn ensure_sqlite_file(dsn: &str) -> Result<(), String> {
    let dsn = dsn.trim();
    if !dsn.starts_with("sqlite://") {
        return Ok(());
    }
    if dsn.contains(":memory:") || dsn.contains("mode=memory") {
        return Ok(());
    }
    let path_part = dsn.trim_start_matches("sqlite://");
    let path_part = path_part.split('?').next().unwrap_or("");
    if path_part.is_empty() {
        return Ok(());
    }
    let path = PathBuf::from(path_part);
    if let Some(parent) = path.parent() {
        if !parent.as_os_str().is_empty() {
            std::fs::create_dir_all(parent)
                .map_err(|err| format!("sqlite_dir_create_failed: {err}"))?;
        }
    }
    if !path.exists() {
        std::fs::File::create(&path).map_err(|err| format!("sqlite_file_create_failed: {err}"))?;
    }
    Ok(())
}

#[derive(Debug, Clone)]
pub struct StoredRecord {
    pub id: String,
    pub value: Value,
    pub expires_at: Option<i64>,
}

#[async_trait]
pub trait StateStore: Send + Sync {
    async fn put(
        &self,
        tenant_id: &str,
        kind: &str,
        id: &str,
        value: Value,
        expires_at: Option<i64>,
    ) -> Result<(), String>;
    async fn get(&self, tenant_id: &str, kind: &str, id: &str)
        -> Result<Option<StoredRecord>, String>;
    async fn delete(&self, tenant_id: &str, kind: &str, id: &str) -> Result<(), String>;
    async fn list(&self, tenant_id: &str, kind: &str) -> Result<Vec<StoredRecord>, String>;
}

#[derive(Clone, Default)]
pub struct MemoryStateStore {
    inner: Arc<RwLock<HashMap<(String, String, String), StoredRecord>>>,
}

#[async_trait]
impl StateStore for MemoryStateStore {
    async fn put(
        &self,
        tenant_id: &str,
        kind: &str,
        id: &str,
        value: Value,
        expires_at: Option<i64>,
    ) -> Result<(), String> {
        let mut guard = self.inner.write().await;
        guard.insert(
            (tenant_id.to_string(), kind.to_string(), id.to_string()),
            StoredRecord {
                id: id.to_string(),
                value,
                expires_at,
            },
        );
        Ok(())
    }

    async fn get(
        &self,
        tenant_id: &str,
        kind: &str,
        id: &str,
    ) -> Result<Option<StoredRecord>, String> {
        let guard = self.inner.read().await;
        Ok(guard
            .get(&(tenant_id.to_string(), kind.to_string(), id.to_string()))
            .cloned())
    }

    async fn delete(&self, tenant_id: &str, kind: &str, id: &str) -> Result<(), String> {
        let mut guard = self.inner.write().await;
        guard.remove(&(tenant_id.to_string(), kind.to_string(), id.to_string()));
        Ok(())
    }

    async fn list(&self, tenant_id: &str, kind: &str) -> Result<Vec<StoredRecord>, String> {
        let guard = self.inner.read().await;
        let mut out = Vec::new();
        for ((t, k, _), record) in guard.iter() {
            if t == tenant_id && k == kind {
                out.push(record.clone());
            }
        }
        Ok(out)
    }
}

#[derive(Clone)]
pub struct SqliteStateStore {
    pool: Pool<Sqlite>,
}

impl SqliteStateStore {
    pub async fn new(dsn: &str) -> Result<Self, String> {
        ensure_sqlite_file(dsn)?;
        let pool = SqlitePoolOptions::new()
            .max_connections(5)
            .connect(dsn)
            .await
            .map_err(|err| err.to_string())?;
        sqlx::query(
            "CREATE TABLE IF NOT EXISTS state_records (\
             tenant_id TEXT NOT NULL,\
             kind TEXT NOT NULL,\
             id TEXT NOT NULL,\
             value TEXT NOT NULL,\
             expires_at INTEGER,\
             PRIMARY KEY (tenant_id, kind, id)\
             )",
        )
        .execute(&pool)
        .await
        .map_err(|err| err.to_string())?;
        Ok(Self { pool })
    }
}

#[async_trait]
impl StateStore for SqliteStateStore {
    async fn put(
        &self,
        tenant_id: &str,
        kind: &str,
        id: &str,
        value: Value,
        expires_at: Option<i64>,
    ) -> Result<(), String> {
        let value_text = serde_json::to_string(&value).map_err(|err| err.to_string())?;
        sqlx::query(
            "INSERT INTO state_records (tenant_id, kind, id, value, expires_at)\
             VALUES (?, ?, ?, ?, ?)\
             ON CONFLICT(tenant_id, kind, id) DO UPDATE SET value=excluded.value, expires_at=excluded.expires_at",
        )
        .bind(tenant_id)
        .bind(kind)
        .bind(id)
        .bind(value_text)
        .bind(expires_at)
        .execute(&self.pool)
        .await
        .map_err(|err| err.to_string())?;
        Ok(())
    }

    async fn get(
        &self,
        tenant_id: &str,
        kind: &str,
        id: &str,
    ) -> Result<Option<StoredRecord>, String> {
        let row = sqlx::query_as::<_, (String, Option<i64>)>(
            "SELECT value, expires_at FROM state_records WHERE tenant_id=? AND kind=? AND id=?",
        )
        .bind(tenant_id)
        .bind(kind)
        .bind(id)
        .fetch_optional(&self.pool)
        .await
        .map_err(|err| err.to_string())?;
        if let Some((value_text, expires_at)) = row {
            let value: Value = serde_json::from_str(&value_text).map_err(|err| err.to_string())?;
            Ok(Some(StoredRecord {
                id: id.to_string(),
                value,
                expires_at,
            }))
        } else {
            Ok(None)
        }
    }

    async fn delete(&self, tenant_id: &str, kind: &str, id: &str) -> Result<(), String> {
        sqlx::query("DELETE FROM state_records WHERE tenant_id=? AND kind=? AND id=?")
            .bind(tenant_id)
            .bind(kind)
            .bind(id)
            .execute(&self.pool)
            .await
            .map_err(|err| err.to_string())?;
        Ok(())
    }

    async fn list(&self, tenant_id: &str, kind: &str) -> Result<Vec<StoredRecord>, String> {
        let rows = sqlx::query_as::<_, (String, String, Option<i64>)>(
            "SELECT id, value, expires_at FROM state_records WHERE tenant_id=? AND kind=?",
        )
        .bind(tenant_id)
        .bind(kind)
        .fetch_all(&self.pool)
        .await
        .map_err(|err| err.to_string())?;
        let mut out = Vec::new();
        for (id, value_text, expires_at) in rows {
            let value: Value = serde_json::from_str(&value_text).map_err(|err| err.to_string())?;
            out.push(StoredRecord { id, value, expires_at });
        }
        Ok(out)
    }
}

#[async_trait]
pub trait FileStore: Send + Sync {
    async fn put_bytes(
        &self,
        tenant_id: &str,
        file_id: &str,
        bytes: Vec<u8>,
    ) -> Result<(), String>;
    async fn get_bytes(&self, tenant_id: &str, file_id: &str) -> Result<Option<Vec<u8>>, String>;
    async fn delete_bytes(&self, tenant_id: &str, file_id: &str) -> Result<(), String>;
}

#[derive(Clone, Default)]
pub struct MemoryFileStore {
    inner: Arc<RwLock<HashMap<(String, String), Vec<u8>>>>,
}

#[async_trait]
impl FileStore for MemoryFileStore {
    async fn put_bytes(
        &self,
        tenant_id: &str,
        file_id: &str,
        bytes: Vec<u8>,
    ) -> Result<(), String> {
        let mut guard = self.inner.write().await;
        guard.insert((tenant_id.to_string(), file_id.to_string()), bytes);
        Ok(())
    }

    async fn get_bytes(&self, tenant_id: &str, file_id: &str) -> Result<Option<Vec<u8>>, String> {
        let guard = self.inner.read().await;
        Ok(guard
            .get(&(tenant_id.to_string(), file_id.to_string()))
            .cloned())
    }

    async fn delete_bytes(&self, tenant_id: &str, file_id: &str) -> Result<(), String> {
        let mut guard = self.inner.write().await;
        guard.remove(&(tenant_id.to_string(), file_id.to_string()));
        Ok(())
    }
}

#[derive(Clone)]
pub struct SqliteFileStore {
    pool: Pool<Sqlite>,
}

impl SqliteFileStore {
    pub async fn new(dsn: &str) -> Result<Self, String> {
        ensure_sqlite_file(dsn)?;
        let pool = SqlitePoolOptions::new()
            .max_connections(5)
            .connect(dsn)
            .await
            .map_err(|err| err.to_string())?;
        sqlx::query(
            "CREATE TABLE IF NOT EXISTS file_bytes (\
             tenant_id TEXT NOT NULL,\
             file_id TEXT NOT NULL,\
             bytes BLOB NOT NULL,\
             PRIMARY KEY (tenant_id, file_id)\
             )",
        )
        .execute(&pool)
        .await
        .map_err(|err| err.to_string())?;
        Ok(Self { pool })
    }
}

#[async_trait]
impl FileStore for SqliteFileStore {
    async fn put_bytes(
        &self,
        tenant_id: &str,
        file_id: &str,
        bytes: Vec<u8>,
    ) -> Result<(), String> {
        sqlx::query(
            "INSERT INTO file_bytes (tenant_id, file_id, bytes) VALUES (?, ?, ?)\
             ON CONFLICT(tenant_id, file_id) DO UPDATE SET bytes=excluded.bytes",
        )
        .bind(tenant_id)
        .bind(file_id)
        .bind(bytes)
        .execute(&self.pool)
        .await
        .map_err(|err| err.to_string())?;
        Ok(())
    }

    async fn get_bytes(&self, tenant_id: &str, file_id: &str) -> Result<Option<Vec<u8>>, String> {
        let row = sqlx::query("SELECT bytes FROM file_bytes WHERE tenant_id=? AND file_id=?")
            .bind(tenant_id)
            .bind(file_id)
            .fetch_optional(&self.pool)
            .await
            .map_err(|err| err.to_string())?;
        if let Some(row) = row {
            let bytes: Vec<u8> = row.try_get(0).map_err(|err| err.to_string())?;
            Ok(Some(bytes))
        } else {
            Ok(None)
        }
    }

    async fn delete_bytes(&self, tenant_id: &str, file_id: &str) -> Result<(), String> {
        sqlx::query("DELETE FROM file_bytes WHERE tenant_id=? AND file_id=?")
            .bind(tenant_id)
            .bind(file_id)
            .execute(&self.pool)
            .await
            .map_err(|err| err.to_string())?;
        Ok(())
    }
}
