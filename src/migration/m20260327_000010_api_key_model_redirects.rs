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
                for sql in
                    ["ALTER TABLE api_keys ADD COLUMN model_redirects TEXT NOT NULL DEFAULT '[]'"]
                {
                    conn.execute(Statement::from_string(DbBackend::Sqlite, sql.to_string()))
                        .await?;
                }
            }
            DbBackend::Postgres => {
                for sql in [
                    "ALTER TABLE api_keys ADD COLUMN IF NOT EXISTS model_redirects TEXT NOT NULL DEFAULT '[]'",
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
                for sql in ["ALTER TABLE api_keys DROP COLUMN model_redirects"] {
                    conn.execute(Statement::from_string(DbBackend::Sqlite, sql.to_string()))
                        .await?;
                }
            }
            DbBackend::Postgres => {
                for sql in ["ALTER TABLE api_keys DROP COLUMN IF EXISTS model_redirects"] {
                    conn.execute(Statement::from_string(DbBackend::Postgres, sql.to_string()))
                        .await?;
                }
            }
            _ => {}
        }

        Ok(())
    }
}
