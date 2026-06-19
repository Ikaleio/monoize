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
                let rows = conn
                    .query_all(Statement::from_string(
                        DbBackend::Sqlite,
                        "PRAGMA table_info(monoize_channels)".to_string(),
                    ))
                    .await?;
                let existing = rows
                    .into_iter()
                    .filter_map(|row| row.try_get::<String>("", "name").ok())
                    .collect::<std::collections::HashSet<_>>();
                for column in [
                    "passive_min_samples_override",
                    "passive_failure_rate_threshold_override",
                    "request_timeout_ms_override",
                ] {
                    if existing.contains(column) {
                        let sql = format!("ALTER TABLE monoize_channels DROP COLUMN {column}");
                        conn.execute(Statement::from_string(DbBackend::Sqlite, sql))
                            .await?;
                    }
                }
            }
            DbBackend::Postgres => {
                for sql in [
                    "ALTER TABLE monoize_channels DROP COLUMN IF EXISTS passive_min_samples_override",
                    "ALTER TABLE monoize_channels DROP COLUMN IF EXISTS passive_failure_rate_threshold_override",
                    "ALTER TABLE monoize_channels DROP COLUMN IF EXISTS request_timeout_ms_override",
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
        let conn = manager.get_connection();
        let backend = manager.get_database_backend();

        match backend {
            DbBackend::Sqlite => {
                for sql in [
                    "ALTER TABLE monoize_channels ADD COLUMN passive_min_samples_override INTEGER",
                    "ALTER TABLE monoize_channels ADD COLUMN passive_failure_rate_threshold_override REAL",
                    "ALTER TABLE monoize_channels ADD COLUMN request_timeout_ms_override INTEGER",
                ] {
                    conn.execute(Statement::from_string(DbBackend::Sqlite, sql.to_string()))
                        .await?;
                }
            }
            DbBackend::Postgres => {
                for sql in [
                    "ALTER TABLE monoize_channels ADD COLUMN IF NOT EXISTS passive_min_samples_override INTEGER",
                    "ALTER TABLE monoize_channels ADD COLUMN IF NOT EXISTS passive_failure_rate_threshold_override DOUBLE PRECISION",
                    "ALTER TABLE monoize_channels ADD COLUMN IF NOT EXISTS request_timeout_ms_override INTEGER",
                ] {
                    conn.execute(Statement::from_string(DbBackend::Postgres, sql.to_string()))
                        .await?;
                }
            }
            _ => {}
        }

        Ok(())
    }
}
