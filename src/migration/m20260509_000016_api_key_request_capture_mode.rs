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
                    "ALTER TABLE api_keys ADD COLUMN request_capture_mode TEXT NOT NULL DEFAULT 'off'"
                        .to_string(),
                ))
                .await?;
                conn.execute(Statement::from_string(
                    DbBackend::Sqlite,
                    "UPDATE api_keys SET request_capture_mode = CASE WHEN request_capture_enabled = 1 THEN 'capture-all' ELSE 'off' END"
                        .to_string(),
                ))
                .await?;
            }
            DbBackend::Postgres => {
                conn.execute(Statement::from_string(
                    DbBackend::Postgres,
                    "ALTER TABLE api_keys ADD COLUMN IF NOT EXISTS request_capture_mode TEXT NOT NULL DEFAULT 'off'"
                        .to_string(),
                ))
                .await?;
                conn.execute(Statement::from_string(
                    DbBackend::Postgres,
                    "UPDATE api_keys SET request_capture_mode = CASE WHEN request_capture_enabled = 1 THEN 'capture-all' ELSE 'off' END WHERE request_capture_mode = 'off'"
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
                    "ALTER TABLE api_keys DROP COLUMN request_capture_mode".to_string(),
                ))
                .await?;
            }
            DbBackend::Postgres => {
                conn.execute(Statement::from_string(
                    DbBackend::Postgres,
                    "ALTER TABLE api_keys DROP COLUMN IF EXISTS request_capture_mode".to_string(),
                ))
                .await?;
            }
            _ => {}
        }

        Ok(())
    }
}
