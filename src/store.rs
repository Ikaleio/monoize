use async_trait::async_trait;
use serde_json::Value;
use std::collections::HashMap;
use std::sync::Arc;
use tokio::sync::RwLock;

use crate::db::DbPool;
use sea_orm::{ConnectionTrait, Statement};

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
pub struct DbStateStore {
    db: DbPool,
}

impl DbStateStore {
    pub async fn new(db: DbPool) -> Result<Self, String> {
        Ok(Self { db })
    }
}

#[async_trait]
impl StateStore for DbStateStore {
    async fn put(
        &self,
        tenant_id: &str,
        kind: &str,
        id: &str,
        value: Value,
        expires_at: Option<i64>,
    ) -> Result<(), String> {
        let value_text = serde_json::to_string(&value).map_err(|err| err.to_string())?;
        let backend = self.db.backend();
        let sql = match backend {
            sea_orm::DbBackend::Sqlite => {
                "INSERT INTO state_records (tenant_id, kind, id, value, expires_at) \
                 VALUES ($1, $2, $3, $4, $5) \
                 ON CONFLICT(tenant_id, kind, id) DO UPDATE SET value=excluded.value, expires_at=excluded.expires_at"
            }
            _ => {
                "INSERT INTO state_records (tenant_id, kind, id, value, expires_at) \
                 VALUES ($1, $2, $3, $4, $5) \
                 ON CONFLICT(tenant_id, kind, id) DO UPDATE SET value=EXCLUDED.value, expires_at=EXCLUDED.expires_at"
            }
        };
        self.db
            .write().await
            .execute(Statement::from_sql_and_values(
                backend,
                sql,
                [
                    tenant_id.into(),
                    kind.into(),
                    id.into(),
                    value_text.into(),
                    expires_at.into(),
                ],
            ))
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
        let backend = self.db.backend();
        let row = self
            .db
            .read()
            .query_one(Statement::from_sql_and_values(
                backend,
                "SELECT value, expires_at FROM state_records WHERE tenant_id=$1 AND kind=$2 AND id=$3",
                [tenant_id.into(), kind.into(), id.into()],
            ))
            .await
            .map_err(|err| err.to_string())?;
        match row {
            Some(row) => {
                let value_text: String = row.try_get("", "value").map_err(|e| e.to_string())?;
                let expires_at: Option<i64> =
                    row.try_get("", "expires_at").map_err(|e| e.to_string())?;
                let value: Value =
                    serde_json::from_str(&value_text).map_err(|err| err.to_string())?;
                Ok(Some(StoredRecord {
                    id: id.to_string(),
                    value,
                    expires_at,
                }))
            }
            None => Ok(None),
        }
    }

    async fn delete(&self, tenant_id: &str, kind: &str, id: &str) -> Result<(), String> {
        let backend = self.db.backend();
        self.db
            .write().await
            .execute(Statement::from_sql_and_values(
                backend,
                "DELETE FROM state_records WHERE tenant_id=$1 AND kind=$2 AND id=$3",
                [tenant_id.into(), kind.into(), id.into()],
            ))
            .await
            .map_err(|err| err.to_string())?;
        Ok(())
    }

    async fn list(&self, tenant_id: &str, kind: &str) -> Result<Vec<StoredRecord>, String> {
        let backend = self.db.backend();
        let rows = self
            .db
            .read()
            .query_all(Statement::from_sql_and_values(
                backend,
                "SELECT id, value, expires_at FROM state_records WHERE tenant_id=$1 AND kind=$2",
                [tenant_id.into(), kind.into()],
            ))
            .await
            .map_err(|err| err.to_string())?;
        let mut out = Vec::new();
        for row in rows {
            let id: String = row.try_get("", "id").map_err(|e| e.to_string())?;
            let value_text: String = row.try_get("", "value").map_err(|e| e.to_string())?;
            let expires_at: Option<i64> =
                row.try_get("", "expires_at").map_err(|e| e.to_string())?;
            let value: Value =
                serde_json::from_str(&value_text).map_err(|err| err.to_string())?;
            out.push(StoredRecord {
                id,
                value,
                expires_at,
            });
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
pub struct DbFileStore {
    db: DbPool,
}

impl DbFileStore {
    pub async fn new(db: DbPool) -> Result<Self, String> {
        Ok(Self { db })
    }
}

#[async_trait]
impl FileStore for DbFileStore {
    async fn put_bytes(
        &self,
        tenant_id: &str,
        file_id: &str,
        bytes: Vec<u8>,
    ) -> Result<(), String> {
        let backend = self.db.backend();
        let sql = match backend {
            sea_orm::DbBackend::Sqlite => {
                "INSERT INTO file_bytes (tenant_id, file_id, bytes) VALUES ($1, $2, $3) \
                 ON CONFLICT(tenant_id, file_id) DO UPDATE SET bytes=excluded.bytes"
            }
            _ => {
                "INSERT INTO file_bytes (tenant_id, file_id, bytes) VALUES ($1, $2, $3) \
                 ON CONFLICT(tenant_id, file_id) DO UPDATE SET bytes=EXCLUDED.bytes"
            }
        };
        self.db
            .write().await
            .execute(Statement::from_sql_and_values(
                backend,
                sql,
                [tenant_id.into(), file_id.into(), bytes.into()],
            ))
            .await
            .map_err(|err| err.to_string())?;
        Ok(())
    }

    async fn get_bytes(&self, tenant_id: &str, file_id: &str) -> Result<Option<Vec<u8>>, String> {
        let backend = self.db.backend();
        let row = self
            .db
            .read()
            .query_one(Statement::from_sql_and_values(
                backend,
                "SELECT bytes FROM file_bytes WHERE tenant_id=$1 AND file_id=$2",
                [tenant_id.into(), file_id.into()],
            ))
            .await
            .map_err(|err| err.to_string())?;
        match row {
            Some(row) => {
                let bytes: Vec<u8> = row.try_get("", "bytes").map_err(|e| e.to_string())?;
                Ok(Some(bytes))
            }
            None => Ok(None),
        }
    }

    async fn delete_bytes(&self, tenant_id: &str, file_id: &str) -> Result<(), String> {
        let backend = self.db.backend();
        self.db
            .write().await
            .execute(Statement::from_sql_and_values(
                backend,
                "DELETE FROM file_bytes WHERE tenant_id=$1 AND file_id=$2",
                [tenant_id.into(), file_id.into()],
            ))
            .await
            .map_err(|err| err.to_string())?;
        Ok(())
    }
}
