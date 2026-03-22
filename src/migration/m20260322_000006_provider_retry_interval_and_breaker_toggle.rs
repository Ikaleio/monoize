use sea_orm::{DbBackend, Statement};
use sea_orm_migration::prelude::*;

#[derive(DeriveMigrationName)]
pub struct Migration;

#[async_trait::async_trait]
impl MigrationTrait for Migration {
    async fn up(&self, manager: &SchemaManager) -> Result<(), DbErr> {
        let conn = manager.get_connection();
        let backend = manager.get_database_backend();

        match backend {
            DbBackend::Sqlite => {
                for sql in [
                    "ALTER TABLE monoize_providers ADD COLUMN channel_retry_interval_ms INTEGER NOT NULL DEFAULT 0",
                    "ALTER TABLE monoize_providers ADD COLUMN circuit_breaker_enabled INTEGER NOT NULL DEFAULT 1",
                ] {
                    conn.execute(Statement::from_string(DbBackend::Sqlite, sql.to_string()))
                        .await?;
                }
            }
            DbBackend::Postgres => {
                for sql in [
                    "ALTER TABLE monoize_providers ADD COLUMN IF NOT EXISTS channel_retry_interval_ms INTEGER NOT NULL DEFAULT 0",
                    "ALTER TABLE monoize_providers ADD COLUMN IF NOT EXISTS circuit_breaker_enabled INTEGER NOT NULL DEFAULT 1",
                ] {
                    conn.execute(Statement::from_string(DbBackend::Postgres, sql.to_string()))
                        .await?;
                }
            }
            _ => {}
        }

        Ok(())
    }

    async fn down(&self, manager: &SchemaManager) -> Result<(), DbErr> {
        if manager.get_database_backend() != DbBackend::Postgres {
            return Ok(());
        }

        let conn = manager.get_connection();
        for sql in [
            "ALTER TABLE monoize_providers DROP COLUMN IF EXISTS circuit_breaker_enabled",
            "ALTER TABLE monoize_providers DROP COLUMN IF EXISTS channel_retry_interval_ms",
        ] {
            conn.execute(Statement::from_string(DbBackend::Postgres, sql.to_string()))
                .await?;
        }

        Ok(())
    }
}
