use sea_orm::{ConnectionTrait, DatabaseTransaction, DbBackend, Statement, TransactionTrait};
use sea_orm_migration::prelude::*;

#[derive(DeriveMigrationName)]
pub struct Migration;

/// ISM4.12l / model-pricing.spec.md §12.1: additive step of the model-pricing
/// migration. Creates `model_prices` and `price_sync_runs`, adds
/// `monoize_groups.billing_ratio`, and adds the two nullable Provider
/// free-settlement override columns. It drops or alters nothing.
#[async_trait::async_trait]
impl MigrationTrait for Migration {
    async fn up(&self, manager: &SchemaManager) -> Result<(), DbErr> {
        let backend = manager.get_database_backend();
        if !matches!(backend, DbBackend::Sqlite | DbBackend::Postgres) {
            return Ok(());
        }
        let tx = manager.get_connection().begin().await?;
        if let Err(error) = migrate_up(&tx, backend).await {
            let _ = tx.rollback().await;
            return Err(error);
        }
        tx.commit().await
    }

    async fn down(&self, manager: &SchemaManager) -> Result<(), DbErr> {
        let backend = manager.get_database_backend();
        if !matches!(backend, DbBackend::Sqlite | DbBackend::Postgres) {
            return Ok(());
        }
        let tx = manager.get_connection().begin().await?;
        if let Err(error) = migrate_down(&tx, backend).await {
            let _ = tx.rollback().await;
            return Err(error);
        }
        tx.commit().await
    }
}

async fn migrate_up(tx: &DatabaseTransaction, backend: DbBackend) -> Result<(), DbErr> {
    for sql in [
        "CREATE TABLE model_prices (\
         model_id TEXT NOT NULL PRIMARY KEY, \
         billing_mode TEXT NOT NULL, \
         input_usd_per_1m TEXT NULL, \
         output_usd_per_1m TEXT NULL, \
         cache_read_usd_per_1m TEXT NULL, \
         cache_write_usd_per_1m TEXT NULL, \
         cache_write_1h_usd_per_1m TEXT NULL, \
         reasoning_usd_per_1m TEXT NULL, \
         per_request_usd TEXT NULL, \
         billing_expr TEXT NULL, \
         source TEXT NOT NULL, \
         locked_fields TEXT NOT NULL DEFAULT '[]', \
         raw_json TEXT NOT NULL DEFAULT '{}', \
         enabled INTEGER NOT NULL DEFAULT 1, \
         updated_at TEXT NOT NULL)",
        "CREATE TABLE price_sync_runs (\
         id TEXT NOT NULL PRIMARY KEY, \
         source TEXT NOT NULL, \
         status TEXT NOT NULL, \
         started_at TEXT NOT NULL, \
         finished_at TEXT NULL, \
         inserted INTEGER NOT NULL DEFAULT 0, \
         updated INTEGER NOT NULL DEFAULT 0, \
         skipped INTEGER NOT NULL DEFAULT 0, \
         deleted INTEGER NOT NULL DEFAULT 0, \
         error TEXT NULL, \
         detail_json TEXT NOT NULL DEFAULT '{}')",
        "CREATE INDEX idx_price_sync_runs_started_at ON price_sync_runs (started_at)",
        "ALTER TABLE monoize_groups ADD COLUMN billing_ratio TEXT NOT NULL DEFAULT '1'",
        "ALTER TABLE monoize_providers ADD COLUMN allow_free_when_unpriced_override INTEGER NULL",
        "ALTER TABLE monoize_providers ADD COLUMN allow_free_when_missing_usage_override INTEGER NULL",
    ] {
        tx.execute(Statement::from_string(backend, sql.to_string()))
            .await?;
    }
    Ok(())
}

async fn migrate_down(tx: &DatabaseTransaction, backend: DbBackend) -> Result<(), DbErr> {
    let drop_columns: &[&str] = match backend {
        DbBackend::Postgres => &[
            "ALTER TABLE monoize_providers DROP COLUMN IF EXISTS allow_free_when_missing_usage_override",
            "ALTER TABLE monoize_providers DROP COLUMN IF EXISTS allow_free_when_unpriced_override",
            "ALTER TABLE monoize_groups DROP COLUMN IF EXISTS billing_ratio",
        ],
        _ => &[
            "ALTER TABLE monoize_providers DROP COLUMN allow_free_when_missing_usage_override",
            "ALTER TABLE monoize_providers DROP COLUMN allow_free_when_unpriced_override",
            "ALTER TABLE monoize_groups DROP COLUMN billing_ratio",
        ],
    };
    for sql in drop_columns
        .iter()
        .copied()
        .chain(["DROP TABLE price_sync_runs", "DROP TABLE model_prices"])
    {
        tx.execute(Statement::from_string(backend, sql.to_string()))
            .await?;
    }
    Ok(())
}
