use sea_orm::{ConnectionTrait, DbBackend, Statement};
use sea_orm_migration::prelude::*;

#[derive(DeriveMigrationName)]
pub struct Migration;

// RCD-M1/RCD-M2 (`request-capture-dumps.spec.md`): capture metadata rows let
// the request-log list compute `has_capture` with an indexed EXISTS subquery
// and let the capture detail API resolve dump files without directory scans.
// The table intentionally has no foreign keys (RCD-M5): retention cleanup and
// stale-record cleanup are the only deleters.
const UP_TABLE: &str = "CREATE TABLE request_capture_records (\
    file_name TEXT PRIMARY KEY, \
    request_id TEXT NOT NULL, \
    user_id TEXT NOT NULL, \
    api_key_id TEXT NOT NULL, \
    created_at TEXT NOT NULL, \
    created_at_unix_ms BIGINT NOT NULL, \
    size_bytes BIGINT NOT NULL)";
const UP_INDEX_USER_REQUEST: &str = "CREATE INDEX idx_request_capture_records_user_request \
    ON request_capture_records (user_id, request_id)";
const UP_INDEX_CREATED_AT: &str = "CREATE INDEX idx_request_capture_records_created_at \
    ON request_capture_records (created_at_unix_ms)";
const DOWN_TABLE: &str = "DROP TABLE IF EXISTS request_capture_records";

#[async_trait::async_trait]
impl MigrationTrait for Migration {
    async fn up(&self, manager: &SchemaManager) -> Result<(), DbErr> {
        let backend = manager.get_database_backend();
        if !matches!(backend, DbBackend::Sqlite | DbBackend::Postgres) {
            return Ok(());
        }
        let connection = manager.get_connection();
        for sql in [UP_TABLE, UP_INDEX_USER_REQUEST, UP_INDEX_CREATED_AT] {
            connection
                .execute(Statement::from_string(backend, sql.to_string()))
                .await?;
        }
        Ok(())
    }

    async fn down(&self, manager: &SchemaManager) -> Result<(), DbErr> {
        let backend = manager.get_database_backend();
        if !matches!(backend, DbBackend::Sqlite | DbBackend::Postgres) {
            return Ok(());
        }
        manager
            .get_connection()
            .execute(Statement::from_string(backend, DOWN_TABLE.to_string()))
            .await?;
        Ok(())
    }
}
