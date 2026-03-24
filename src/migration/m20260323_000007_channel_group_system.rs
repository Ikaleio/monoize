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
                    "ALTER TABLE monoize_channels ADD COLUMN groups TEXT NOT NULL DEFAULT '[]'",
                    "ALTER TABLE users ADD COLUMN allowed_groups TEXT NOT NULL DEFAULT '[]'",
                    "ALTER TABLE api_keys ADD COLUMN allowed_groups TEXT NOT NULL DEFAULT '[]'",
                ] {
                    conn.execute(Statement::from_string(DbBackend::Sqlite, sql.to_string()))
                        .await?;
                }
            }
            DbBackend::Postgres => {
                for sql in [
                    "ALTER TABLE monoize_channels ADD COLUMN IF NOT EXISTS groups TEXT NOT NULL DEFAULT '[]'",
                    "ALTER TABLE users ADD COLUMN IF NOT EXISTS allowed_groups TEXT NOT NULL DEFAULT '[]'",
                    "ALTER TABLE api_keys ADD COLUMN IF NOT EXISTS allowed_groups TEXT NOT NULL DEFAULT '[]'",
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
                    "ALTER TABLE api_keys DROP COLUMN allowed_groups",
                    "ALTER TABLE users DROP COLUMN allowed_groups",
                    "ALTER TABLE monoize_channels DROP COLUMN groups",
                ] {
                    conn.execute(Statement::from_string(DbBackend::Sqlite, sql.to_string()))
                        .await?;
                }
            }
            DbBackend::Postgres => {
                for sql in [
                    "ALTER TABLE api_keys DROP COLUMN IF EXISTS allowed_groups",
                    "ALTER TABLE users DROP COLUMN IF EXISTS allowed_groups",
                    "ALTER TABLE monoize_channels DROP COLUMN IF EXISTS groups",
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
