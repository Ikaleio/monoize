use sea_orm::{ConnectionTrait, DbBackend, Statement};
use sea_orm_migration::prelude::*;

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

#[async_trait::async_trait]
impl MigrationTrait for Migration {
    // ISM4.12p: the backfill is a fixed value by design. It reads neither base_url nor models,
    // so a Channel on any vendor endpoint stays `generic` until an operator selects its dialect.
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
        Ok(())
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
        manager
            .get_connection()
            .execute(Statement::from_string(backend, drop.to_string()))
            .await?;
        Ok(())
    }
}
