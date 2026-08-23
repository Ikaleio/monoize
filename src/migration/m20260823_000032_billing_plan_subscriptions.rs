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
                for sql in [
                    "CREATE TABLE IF NOT EXISTS billing_plans (id TEXT NOT NULL PRIMARY KEY, name TEXT NOT NULL UNIQUE, grant_amount_nano_usd TEXT NOT NULL, period_seconds BIGINT NOT NULL, allowed_groups TEXT NOT NULL DEFAULT '[]', enabled INTEGER NOT NULL DEFAULT 1, created_at TEXT NOT NULL, updated_at TEXT NOT NULL)",
                    "ALTER TABLE users ADD COLUMN billing_plan_id TEXT",
                    "ALTER TABLE users ADD COLUMN next_grant_at TEXT",
                    "CREATE INDEX IF NOT EXISTS idx_users_billing_plan_id ON users (billing_plan_id)",
                ] {
                    conn.execute(Statement::from_string(DbBackend::Sqlite, sql.to_string()))
                        .await?;
                }
            }
            DbBackend::Postgres => {
                for sql in [
                    "CREATE TABLE IF NOT EXISTS billing_plans (id TEXT NOT NULL PRIMARY KEY, name TEXT NOT NULL UNIQUE, grant_amount_nano_usd TEXT NOT NULL, period_seconds BIGINT NOT NULL, allowed_groups TEXT NOT NULL DEFAULT '[]', enabled INTEGER NOT NULL DEFAULT 1, created_at TEXT NOT NULL, updated_at TEXT NOT NULL)",
                    "ALTER TABLE users ADD COLUMN IF NOT EXISTS billing_plan_id TEXT",
                    "ALTER TABLE users ADD COLUMN IF NOT EXISTS next_grant_at TEXT",
                    "CREATE INDEX IF NOT EXISTS idx_users_billing_plan_id ON users (billing_plan_id)",
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
                    "DROP INDEX IF EXISTS idx_users_billing_plan_id",
                    "UPDATE users SET billing_plan_id = NULL, next_grant_at = NULL WHERE billing_plan_id IS NOT NULL",
                    "DROP TABLE billing_plans",
                ] {
                    // SQLite cannot DROP COLUMN before 3.35 in some builds; leaving the
                    // nullable columns in place is harmless because no code reads them
                    // once the plan table is gone.
                    conn.execute(Statement::from_string(DbBackend::Sqlite, sql.to_string()))
                        .await?;
                }
            }
            DbBackend::Postgres => {
                for sql in [
                    "DROP INDEX IF EXISTS idx_users_billing_plan_id",
                    "ALTER TABLE users DROP COLUMN IF EXISTS billing_plan_id",
                    "ALTER TABLE users DROP COLUMN IF EXISTS next_grant_at",
                    "DROP TABLE IF EXISTS billing_plans",
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
