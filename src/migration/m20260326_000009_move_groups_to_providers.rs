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
                conn.execute(Statement::from_string(
                    DbBackend::Sqlite,
                    "ALTER TABLE monoize_providers ADD COLUMN groups TEXT NOT NULL DEFAULT '[]'"
                        .to_string(),
                ))
                .await?;
                conn.execute(Statement::from_string(
                    DbBackend::Sqlite,
                    "ALTER TABLE monoize_channels DROP COLUMN groups".to_string(),
                ))
                .await?;
            }
            DbBackend::Postgres => {
                conn.execute(Statement::from_string(
                    DbBackend::Postgres,
                    "ALTER TABLE monoize_providers ADD COLUMN IF NOT EXISTS groups TEXT NOT NULL DEFAULT '[]'"
                        .to_string(),
                ))
                .await?;
                conn.execute(Statement::from_string(
                    DbBackend::Postgres,
                    "ALTER TABLE monoize_channels DROP COLUMN IF EXISTS groups".to_string(),
                ))
                .await?;
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
                conn.execute(Statement::from_string(
                    DbBackend::Sqlite,
                    "ALTER TABLE monoize_channels ADD COLUMN groups TEXT NOT NULL DEFAULT '[]'"
                        .to_string(),
                ))
                .await?;
                conn.execute(Statement::from_string(
                    DbBackend::Sqlite,
                    "ALTER TABLE monoize_providers DROP COLUMN groups".to_string(),
                ))
                .await?;
            }
            DbBackend::Postgres => {
                conn.execute(Statement::from_string(
                    DbBackend::Postgres,
                    "ALTER TABLE monoize_channels ADD COLUMN IF NOT EXISTS groups TEXT NOT NULL DEFAULT '[]'"
                        .to_string(),
                ))
                .await?;
                conn.execute(Statement::from_string(
                    DbBackend::Postgres,
                    "ALTER TABLE monoize_providers DROP COLUMN IF EXISTS groups".to_string(),
                ))
                .await?;
            }
            _ => {}
        }

        Ok(())
    }
}
