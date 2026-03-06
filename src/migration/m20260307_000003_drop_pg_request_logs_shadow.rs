use sea_orm::{DbBackend, Statement};
use sea_orm_migration::prelude::*;

#[derive(DeriveMigrationName)]
pub struct Migration;

#[async_trait::async_trait]
impl MigrationTrait for Migration {
    async fn up(&self, manager: &SchemaManager) -> Result<(), DbErr> {
        if manager.get_database_backend() != DbBackend::Postgres {
            return Ok(());
        }

        let conn = manager.get_connection();
        for sql in [
            "DROP INDEX IF EXISTS idx_request_logs_created_at_ts",
            "DROP INDEX IF EXISTS idx_request_logs_user_created_at_ts",
            "ALTER TABLE request_logs DROP COLUMN IF EXISTS charge_nano_usd_decimal",
            "ALTER TABLE request_logs DROP COLUMN IF EXISTS is_stream_bool",
            "ALTER TABLE request_logs DROP COLUMN IF EXISTS created_at_ts",
        ] {
            conn.execute(Statement::from_string(DbBackend::Postgres, sql.to_string()))
                .await?;
        }

        Ok(())
    }

    async fn down(&self, manager: &SchemaManager) -> Result<(), DbErr> {
        if manager.get_database_backend() != DbBackend::Postgres {
            return Ok(());
        }

        let conn = manager.get_connection();
        for sql in [
            "ALTER TABLE request_logs ADD COLUMN IF NOT EXISTS created_at_ts TIMESTAMPTZ",
            "ALTER TABLE request_logs ADD COLUMN IF NOT EXISTS is_stream_bool BOOLEAN",
            "ALTER TABLE request_logs ADD COLUMN IF NOT EXISTS charge_nano_usd_decimal NUMERIC(39,0)",
            "CREATE INDEX IF NOT EXISTS idx_request_logs_user_created_at_ts ON request_logs (user_id, created_at_ts DESC)",
            "CREATE INDEX IF NOT EXISTS idx_request_logs_created_at_ts ON request_logs (created_at_ts DESC)",
        ] {
            conn.execute(Statement::from_string(DbBackend::Postgres, sql.to_string()))
                .await?;
        }

        Ok(())
    }
}
