use sea_orm::{DbBackend, Statement};
use sea_orm_migration::prelude::*;

#[derive(DeriveMigrationName)]
pub struct Migration;

#[async_trait::async_trait]
impl MigrationTrait for Migration {
    async fn up(&self, manager: &SchemaManager) -> Result<(), DbErr> {
        let conn = manager.get_connection();
        let backend = manager.get_database_backend();

        let sql = match backend {
            DbBackend::Sqlite => {
                "ALTER TABLE monoize_providers ADD COLUMN extra_fields_whitelist TEXT DEFAULT NULL"
            }
            DbBackend::Postgres => {
                "ALTER TABLE monoize_providers ADD COLUMN IF NOT EXISTS extra_fields_whitelist TEXT DEFAULT NULL"
            }
            _ => return Ok(()),
        };

        conn.execute(Statement::from_string(backend, sql.to_string()))
            .await?;
        Ok(())
    }

    async fn down(&self, manager: &SchemaManager) -> Result<(), DbErr> {
        let conn = manager.get_connection();
        let backend = manager.get_database_backend();

        let sql = match backend {
            DbBackend::Sqlite => "ALTER TABLE monoize_providers DROP COLUMN extra_fields_whitelist",
            DbBackend::Postgres => {
                "ALTER TABLE monoize_providers DROP COLUMN IF EXISTS extra_fields_whitelist"
            }
            _ => return Ok(()),
        };

        conn.execute(Statement::from_string(backend, sql.to_string()))
            .await?;
        Ok(())
    }
}
