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

        conn.execute(Statement::from_string(
            DbBackend::Postgres,
            "ALTER TABLE request_logs ADD COLUMN IF NOT EXISTS created_at_ts TIMESTAMPTZ".to_string(),
        ))
        .await?;

        conn.execute(Statement::from_string(
            DbBackend::Postgres,
            "ALTER TABLE request_logs ADD COLUMN IF NOT EXISTS is_stream_bool BOOLEAN".to_string(),
        ))
        .await?;

        conn.execute(Statement::from_string(
            DbBackend::Postgres,
            "ALTER TABLE request_logs ADD COLUMN IF NOT EXISTS charge_nano_usd_decimal NUMERIC(39,0)"
                .to_string(),
        ))
        .await?;

        conn.execute(Statement::from_string(
            DbBackend::Postgres,
            "UPDATE request_logs SET created_at_ts = CASE WHEN created_at IS NULL OR btrim(created_at) = '' THEN NULL WHEN created_at ~ '^\\d{4}-\\d{2}-\\d{2}T\\d{2}:\\d{2}:\\d{2}(?:\\.\\d+)?(?:Z|[+-]\\d{2}:\\d{2})$' THEN CAST(created_at AS TIMESTAMPTZ) ELSE NULL END WHERE created_at_ts IS NULL"
                .to_string(),
        ))
        .await?;

        conn.execute(Statement::from_string(
            DbBackend::Postgres,
            "UPDATE request_logs SET is_stream_bool = CASE WHEN is_stream = 1 THEN TRUE WHEN is_stream = 0 THEN FALSE ELSE NULL END WHERE is_stream_bool IS NULL"
                .to_string(),
        ))
        .await?;

        conn.execute(Statement::from_string(
            DbBackend::Postgres,
            "UPDATE request_logs SET charge_nano_usd_decimal = CASE WHEN charge_nano_usd IS NULL OR btrim(charge_nano_usd) = '' THEN NULL WHEN charge_nano_usd ~ '^-?[0-9]+$' THEN CAST(charge_nano_usd AS NUMERIC(39,0)) ELSE NULL END WHERE charge_nano_usd_decimal IS NULL"
                .to_string(),
        ))
        .await?;

        conn.execute(Statement::from_string(
            DbBackend::Postgres,
            "CREATE INDEX IF NOT EXISTS idx_request_logs_user_created_at_ts ON request_logs (user_id, created_at_ts DESC)"
                .to_string(),
        ))
        .await?;

        conn.execute(Statement::from_string(
            DbBackend::Postgres,
            "CREATE INDEX IF NOT EXISTS idx_request_logs_created_at_ts ON request_logs (created_at_ts DESC)"
                .to_string(),
        ))
        .await?;

        Ok(())
    }

    async fn down(&self, manager: &SchemaManager) -> Result<(), DbErr> {
        if manager.get_database_backend() != DbBackend::Postgres {
            return Ok(());
        }

        let conn = manager.get_connection();

        conn.execute(Statement::from_string(
            DbBackend::Postgres,
            "DROP INDEX IF EXISTS idx_request_logs_created_at_ts".to_string(),
        ))
        .await?;

        conn.execute(Statement::from_string(
            DbBackend::Postgres,
            "DROP INDEX IF EXISTS idx_request_logs_user_created_at_ts".to_string(),
        ))
        .await?;

        conn.execute(Statement::from_string(
            DbBackend::Postgres,
            "ALTER TABLE request_logs DROP COLUMN IF EXISTS charge_nano_usd_decimal".to_string(),
        ))
        .await?;

        conn.execute(Statement::from_string(
            DbBackend::Postgres,
            "ALTER TABLE request_logs DROP COLUMN IF EXISTS is_stream_bool".to_string(),
        ))
        .await?;

        conn.execute(Statement::from_string(
            DbBackend::Postgres,
            "ALTER TABLE request_logs DROP COLUMN IF EXISTS created_at_ts".to_string(),
        ))
        .await?;

        Ok(())
    }
}
