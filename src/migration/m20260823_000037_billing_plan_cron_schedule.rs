use sea_orm::{ConnectionTrait, DbBackend, Statement};
use sea_orm_migration::prelude::*;

#[derive(DeriveMigrationName)]
pub struct Migration;

const SQLITE_SCHEDULE_CASE: &str = "CASE period_seconds \
    WHEN 60 THEN '* * * * *' \
    WHEN 3600 THEN '0 * * * *' \
    WHEN 86400 THEN '0 0 * * *' \
    WHEN 604800 THEN '0 0 * * 0' \
    ELSE '0 0 * * *' END";

#[async_trait::async_trait]
impl MigrationTrait for Migration {
    async fn up(&self, manager: &SchemaManager) -> Result<(), DbErr> {
        let conn = manager.get_connection();
        let backend = manager.get_database_backend();
        match backend {
            DbBackend::Sqlite => {
                for sql in [
                    "CREATE TABLE billing_plans_new (id TEXT NOT NULL PRIMARY KEY, name TEXT NOT NULL, grant_amount_nano_usd TEXT NOT NULL, schedule TEXT NOT NULL, allowed_groups TEXT NOT NULL DEFAULT '[]', enabled INTEGER NOT NULL DEFAULT 1, created_at TEXT NOT NULL, updated_at TEXT NOT NULL)".to_string(),
                    format!(
                        "INSERT INTO billing_plans_new (id, name, grant_amount_nano_usd, schedule, allowed_groups, enabled, created_at, updated_at) SELECT id, name, grant_amount_nano_usd, {SQLITE_SCHEDULE_CASE}, allowed_groups, enabled, created_at, updated_at FROM billing_plans"
                    ),
                    "DROP INDEX IF EXISTS uq_billing_plans_name_lower".to_string(),
                    "DROP TABLE billing_plans".to_string(),
                    "ALTER TABLE billing_plans_new RENAME TO billing_plans".to_string(),
                    "CREATE UNIQUE INDEX IF NOT EXISTS uq_billing_plans_name_lower ON billing_plans (lower(name))".to_string(),
                ] {
                    conn.execute(Statement::from_string(DbBackend::Sqlite, sql))
                        .await?;
                }
            }
            DbBackend::Postgres => {
                for sql in [
                    "ALTER TABLE billing_plans ADD COLUMN schedule TEXT".to_string(),
                    format!(
                        "UPDATE billing_plans SET schedule = {SQLITE_SCHEDULE_CASE}"
                    ),
                    "ALTER TABLE billing_plans ALTER COLUMN schedule SET NOT NULL".to_string(),
                    "ALTER TABLE billing_plans DROP COLUMN period_seconds".to_string(),
                ] {
                    conn.execute(Statement::from_string(DbBackend::Postgres, sql))
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
        let period_case = "CASE schedule \
            WHEN '* * * * *' THEN 60 \
            WHEN '0 * * * *' THEN 3600 \
            WHEN '0 0 * * 0' THEN 604800 \
            ELSE 86400 END";
        match backend {
            DbBackend::Sqlite => {
                for sql in [
                    "CREATE TABLE billing_plans_old (id TEXT NOT NULL PRIMARY KEY, name TEXT NOT NULL, grant_amount_nano_usd TEXT NOT NULL, period_seconds BIGINT NOT NULL, allowed_groups TEXT NOT NULL DEFAULT '[]', enabled INTEGER NOT NULL DEFAULT 1, created_at TEXT NOT NULL, updated_at TEXT NOT NULL)".to_string(),
                    format!(
                        "INSERT INTO billing_plans_old (id, name, grant_amount_nano_usd, period_seconds, allowed_groups, enabled, created_at, updated_at) SELECT id, name, grant_amount_nano_usd, {period_case}, allowed_groups, enabled, created_at, updated_at FROM billing_plans"
                    ),
                    "DROP INDEX IF EXISTS uq_billing_plans_name_lower".to_string(),
                    "DROP TABLE billing_plans".to_string(),
                    "ALTER TABLE billing_plans_old RENAME TO billing_plans".to_string(),
                    "CREATE UNIQUE INDEX IF NOT EXISTS uq_billing_plans_name_lower ON billing_plans (lower(name))".to_string(),
                ] {
                    conn.execute(Statement::from_string(DbBackend::Sqlite, sql))
                        .await?;
                }
            }
            DbBackend::Postgres => {
                for sql in [
                    "ALTER TABLE billing_plans ADD COLUMN period_seconds BIGINT".to_string(),
                    format!("UPDATE billing_plans SET period_seconds = {period_case}"),
                    "ALTER TABLE billing_plans ALTER COLUMN period_seconds SET NOT NULL"
                        .to_string(),
                    "ALTER TABLE billing_plans DROP COLUMN schedule".to_string(),
                ] {
                    conn.execute(Statement::from_string(DbBackend::Postgres, sql))
                        .await?;
                }
            }
            _ => {}
        }
        Ok(())
    }
}
