use sea_orm::{ConnectionTrait, DbBackend, Statement};
use sea_orm_migration::prelude::*;

#[derive(DeriveMigrationName)]
pub struct Migration;

const UP_COLUMN: &str = "ALTER TABLE monoize_channels ADD COLUMN allow_unpriced_server_tools INTEGER NOT NULL DEFAULT 0";

#[async_trait::async_trait]
impl MigrationTrait for Migration {
    async fn up(&self, manager: &SchemaManager) -> Result<(), DbErr> {
        let backend = manager.get_database_backend();
        if !matches!(backend, DbBackend::Sqlite | DbBackend::Postgres) {
            return Ok(());
        }
        manager
            .get_connection()
            .execute(Statement::from_string(backend, UP_COLUMN.to_string()))
            .await?;
        Ok(())
    }

    async fn down(&self, manager: &SchemaManager) -> Result<(), DbErr> {
        let backend = manager.get_database_backend();
        if !matches!(backend, DbBackend::Sqlite | DbBackend::Postgres) {
            return Ok(());
        }
        let sql = match backend {
            DbBackend::Postgres => {
                "ALTER TABLE monoize_channels DROP COLUMN IF EXISTS allow_unpriced_server_tools"
            }
            _ => "ALTER TABLE monoize_channels DROP COLUMN allow_unpriced_server_tools",
        };
        manager
            .get_connection()
            .execute(Statement::from_string(backend, sql.to_string()))
            .await?;
        Ok(())
    }
}
