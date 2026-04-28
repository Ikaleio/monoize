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
                    "ALTER TABLE api_keys ADD COLUMN reasoning_envelope_enabled INTEGER NOT NULL DEFAULT 1"
                        .to_string(),
                ))
                .await?;
            }
            DbBackend::Postgres => {
                conn.execute(Statement::from_string(
                    DbBackend::Postgres,
                    "ALTER TABLE api_keys ADD COLUMN IF NOT EXISTS reasoning_envelope_enabled INTEGER NOT NULL DEFAULT 1"
                        .to_string(),
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
                    "ALTER TABLE api_keys DROP COLUMN reasoning_envelope_enabled".to_string(),
                ))
                .await?;
            }
            DbBackend::Postgres => {
                conn.execute(Statement::from_string(
                    DbBackend::Postgres,
                    "ALTER TABLE api_keys DROP COLUMN IF EXISTS reasoning_envelope_enabled"
                        .to_string(),
                ))
                .await?;
            }
            _ => {}
        }

        Ok(())
    }
}
