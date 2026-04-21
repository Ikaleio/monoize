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
                "ALTER TABLE monoize_providers ADD COLUMN strip_cross_protocol_nested_extra INTEGER DEFAULT NULL"
            }
            DbBackend::Postgres => {
                "ALTER TABLE monoize_providers ADD COLUMN IF NOT EXISTS strip_cross_protocol_nested_extra INTEGER DEFAULT NULL"
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
            DbBackend::Sqlite => {
                "ALTER TABLE monoize_providers DROP COLUMN strip_cross_protocol_nested_extra"
            }
            DbBackend::Postgres => {
                "ALTER TABLE monoize_providers DROP COLUMN IF EXISTS strip_cross_protocol_nested_extra"
            }
            _ => return Ok(()),
        };

        conn.execute(Statement::from_string(backend, sql.to_string()))
            .await?;
        Ok(())
    }
}
