use sea_orm::{ConnectionTrait, DbBackend, Statement};
use sea_orm_migration::prelude::*;
use serde_json::{Map, Value};

#[derive(DeriveMigrationName)]
pub struct Migration;

async fn column_exists(conn: &impl ConnectionTrait, backend: DbBackend) -> Result<bool, DbErr> {
    let exists = match backend {
        DbBackend::Postgres => !conn
            .query_all(Statement::from_string(
                backend,
                "SELECT 1 FROM information_schema.columns \
                 WHERE table_name = 'monoize_channels' AND column_name = 'chat_dialect'"
                    .to_string(),
            ))
            .await?
            .is_empty(),
        _ => conn
            .query_all(Statement::from_string(
                backend,
                "PRAGMA table_info(monoize_channels)".to_string(),
            ))
            .await?
            .iter()
            .any(|row| {
                row.try_get::<String>("", "name")
                    .map(|name| name == "chat_dialect")
                    .unwrap_or(false)
            }),
    };
    Ok(exists)
}

/// Rewrites each provider's override list. Rows are parsed rather than edited with REPLACE so a
/// second run cannot produce a duplicate key.
async fn rewrite_overrides(
    conn: &impl ConnectionTrait,
    backend: DbBackend,
    rewrite: fn(&mut Map<String, Value>),
) -> Result<(), DbErr> {
    let rows = conn
        .query_all(Statement::from_string(
            backend,
            "SELECT id, api_type_overrides FROM monoize_providers".to_string(),
        ))
        .await?;
    for row in rows {
        let id: String = row.try_get("", "id")?;
        let raw: String = row.try_get("", "api_type_overrides")?;
        let mut entries: Vec<Map<String, Value>> = serde_json::from_str(&raw).map_err(|e| {
            DbErr::Custom(format!(
                "provider {id} invalid api_type_overrides JSON: {e}"
            ))
        })?;
        if entries.is_empty() {
            continue;
        }
        entries.iter_mut().for_each(rewrite);
        let updated = serde_json::to_string(&entries).map_err(|e| DbErr::Custom(e.to_string()))?;
        if updated == raw {
            continue;
        }
        let update = match backend {
            DbBackend::Postgres => {
                "UPDATE monoize_providers SET api_type_overrides = $1 WHERE id = $2"
            }
            _ => "UPDATE monoize_providers SET api_type_overrides = ? WHERE id = ?",
        };
        conn.execute(Statement::from_sql_and_values(
            backend,
            update,
            [updated.into(), id.into()],
        ))
        .await?;
    }
    Ok(())
}

fn backfill_override_dialect(entry: &mut Map<String, Value>) {
    if entry.get("api_type").and_then(Value::as_str) == Some("chat_completion") {
        entry.insert("chat_dialect".to_string(), Value::from("generic"));
    }
}

fn remove_override_dialect(entry: &mut Map<String, Value>) {
    entry.remove("chat_dialect");
}

#[async_trait::async_trait]
impl MigrationTrait for Migration {
    // ISM4.12p: the backfill is a fixed value by design. It reads neither base_url nor models,
    // so a Channel or override on any vendor stays `generic` until an operator selects a dialect.
    async fn up(&self, manager: &SchemaManager) -> Result<(), DbErr> {
        let backend = manager.get_database_backend();
        if !matches!(backend, DbBackend::Sqlite | DbBackend::Postgres) {
            return Ok(());
        }
        let conn = manager.get_connection();
        if !column_exists(conn, backend).await? {
            conn.execute(Statement::from_string(
                backend,
                "ALTER TABLE monoize_channels ADD COLUMN chat_dialect TEXT".to_string(),
            ))
            .await?;
        }
        conn.execute(Statement::from_string(
            backend,
            "UPDATE monoize_channels SET chat_dialect = \
             CASE WHEN provider_type = 'chat_completion' THEN 'generic' ELSE NULL END"
                .to_string(),
        ))
        .await?;
        rewrite_overrides(conn, backend, backfill_override_dialect).await
    }

    async fn down(&self, manager: &SchemaManager) -> Result<(), DbErr> {
        let backend = manager.get_database_backend();
        if !matches!(backend, DbBackend::Sqlite | DbBackend::Postgres) {
            return Ok(());
        }
        let drop = match backend {
            DbBackend::Postgres => {
                "ALTER TABLE monoize_channels DROP COLUMN IF EXISTS chat_dialect"
            }
            _ => "ALTER TABLE monoize_channels DROP COLUMN chat_dialect",
        };
        let conn = manager.get_connection();
        conn.execute(Statement::from_string(backend, drop.to_string()))
            .await?;
        rewrite_overrides(conn, backend, remove_override_dialect).await
    }
}
