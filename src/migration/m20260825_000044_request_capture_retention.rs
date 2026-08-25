use sea_orm::{DbBackend, Statement};
use sea_orm_migration::prelude::*;

#[derive(DeriveMigrationName)]
pub struct Migration;

// RCD-M6/RCD-M7 (`request-capture-dumps.spec.md`): per-key capture retention
// replaces the global `monoize_request_capture_retention_days` setting.
// Existing keys migrate to the `24h` default; existing capture metadata rows
// backfill the 24-hour expiry so they stay reachable until the same horizon
// the old global default provided.
const UP_SQLITE: &[&str] = &[
    "ALTER TABLE api_keys ADD COLUMN request_capture_retention TEXT NOT NULL DEFAULT '24h'",
    "ALTER TABLE request_capture_records ADD COLUMN expires_at_unix_ms BIGINT NOT NULL DEFAULT 0",
    "UPDATE request_capture_records SET expires_at_unix_ms = created_at_unix_ms + 86400000 \
     WHERE expires_at_unix_ms = 0",
    "CREATE INDEX idx_request_capture_records_expires_at \
     ON request_capture_records (expires_at_unix_ms)",
    "DELETE FROM system_settings WHERE key = 'monoize_request_capture_retention_days'",
];

const UP_POSTGRES: &[&str] = &[
    "ALTER TABLE api_keys ADD COLUMN IF NOT EXISTS request_capture_retention TEXT NOT NULL DEFAULT '24h'",
    "ALTER TABLE request_capture_records ADD COLUMN IF NOT EXISTS expires_at_unix_ms BIGINT NOT NULL DEFAULT 0",
    "UPDATE request_capture_records SET expires_at_unix_ms = created_at_unix_ms + 86400000 \
     WHERE expires_at_unix_ms = 0",
    "CREATE INDEX IF NOT EXISTS idx_request_capture_records_expires_at \
     ON request_capture_records (expires_at_unix_ms)",
    "DELETE FROM system_settings WHERE key = 'monoize_request_capture_retention_days'",
];

const DOWN_SQLITE: &[&str] = &[
    "DROP INDEX IF EXISTS idx_request_capture_records_expires_at",
    "ALTER TABLE request_capture_records DROP COLUMN expires_at_unix_ms",
    "ALTER TABLE api_keys DROP COLUMN request_capture_retention",
];

const DOWN_POSTGRES: &[&str] = &[
    "DROP INDEX IF EXISTS idx_request_capture_records_expires_at",
    "ALTER TABLE request_capture_records DROP COLUMN IF EXISTS expires_at_unix_ms",
    "ALTER TABLE api_keys DROP COLUMN IF EXISTS request_capture_retention",
];

#[async_trait::async_trait]
impl MigrationTrait for Migration {
    async fn up(&self, manager: &SchemaManager) -> Result<(), DbErr> {
        let backend = manager.get_database_backend();
        let statements = match backend {
            DbBackend::Sqlite => UP_SQLITE,
            DbBackend::Postgres => UP_POSTGRES,
            _ => return Ok(()),
        };
        let connection = manager.get_connection();
        for sql in statements {
            connection
                .execute(Statement::from_string(backend, (*sql).to_string()))
                .await?;
        }
        Ok(())
    }

    async fn down(&self, manager: &SchemaManager) -> Result<(), DbErr> {
        let backend = manager.get_database_backend();
        let statements = match backend {
            DbBackend::Sqlite => DOWN_SQLITE,
            DbBackend::Postgres => DOWN_POSTGRES,
            _ => return Ok(()),
        };
        let connection = manager.get_connection();
        for sql in statements {
            connection
                .execute(Statement::from_string(backend, (*sql).to_string()))
                .await?;
        }
        Ok(())
    }
}
