use sea_orm::{ConnectionTrait, DbBackend, Statement};
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
                conn.execute(Statement::from_string(
                    DbBackend::Sqlite,
                    "ALTER TABLE request_logs ADD COLUMN created_at_unix_ms BIGINT".to_string(),
                ))
                .await
                .ok();

                conn.execute(Statement::from_string(
                    DbBackend::Sqlite,
                    "UPDATE request_logs SET created_at_unix_ms = CAST(ROUND((julianday(created_at) - 2440587.5) * 86400000.0) AS INTEGER) WHERE created_at_unix_ms IS NULL AND created_at IS NOT NULL AND trim(created_at) != ''".to_string(),
                ))
                .await?;

                for sql in [
                    "DROP INDEX IF EXISTS idx_request_logs_user_id",
                    "DROP INDEX IF EXISTS idx_request_logs_created_at",
                    "DROP INDEX IF EXISTS idx_request_logs_user_created_at",
                    "CREATE INDEX IF NOT EXISTS idx_request_logs_user_created_at ON request_logs (user_id, created_at_unix_ms DESC)",
                    "CREATE INDEX IF NOT EXISTS idx_request_logs_created_at ON request_logs (created_at_unix_ms DESC)",
                ] {
                    conn.execute(Statement::from_string(DbBackend::Sqlite, sql.to_string()))
                        .await?;
                }
            }
            DbBackend::Postgres => {
                conn.execute(Statement::from_string(
                    DbBackend::Postgres,
                    "ALTER TABLE request_logs ADD COLUMN IF NOT EXISTS created_at_unix_ms BIGINT"
                        .to_string(),
                ))
                .await?;

                conn.execute(Statement::from_string(
                    DbBackend::Postgres,
                    "UPDATE request_logs SET created_at_unix_ms = FLOOR(EXTRACT(EPOCH FROM CAST(created_at AS TIMESTAMPTZ)) * 1000)::BIGINT WHERE created_at_unix_ms IS NULL AND created_at IS NOT NULL AND btrim(created_at) != '' AND created_at ~ '^\\d{4}-\\d{2}-\\d{2}T\\d{2}:\\d{2}:\\d{2}(?:\\.\\d+)?(?:Z|[+-]\\d{2}:\\d{2})$'".to_string(),
                ))
                .await?;

                for sql in [
                    "DROP INDEX IF EXISTS idx_request_logs_user_id",
                    "DROP INDEX IF EXISTS idx_request_logs_created_at",
                    "DROP INDEX IF EXISTS idx_request_logs_user_created_at",
                    "CREATE INDEX IF NOT EXISTS idx_request_logs_user_created_at ON request_logs (user_id, created_at_unix_ms DESC)",
                    "CREATE INDEX IF NOT EXISTS idx_request_logs_created_at ON request_logs (created_at_unix_ms DESC)",
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
                    "DROP INDEX IF EXISTS idx_request_logs_user_created_at",
                    "DROP INDEX IF EXISTS idx_request_logs_created_at",
                    "CREATE INDEX IF NOT EXISTS idx_request_logs_user_id ON request_logs (user_id)",
                    "CREATE INDEX IF NOT EXISTS idx_request_logs_created_at ON request_logs (created_at)",
                ] {
                    conn.execute(Statement::from_string(DbBackend::Sqlite, sql.to_string()))
                        .await?;
                }
            }
            DbBackend::Postgres => {
                for sql in [
                    "DROP INDEX IF EXISTS idx_request_logs_user_created_at",
                    "DROP INDEX IF EXISTS idx_request_logs_created_at",
                    "CREATE INDEX IF NOT EXISTS idx_request_logs_user_id ON request_logs (user_id)",
                    "CREATE INDEX IF NOT EXISTS idx_request_logs_created_at ON request_logs (created_at)",
                    "ALTER TABLE request_logs DROP COLUMN IF EXISTS created_at_unix_ms",
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
