use sea_orm::{ConnectionTrait, DbBackend, Statement};
use sea_orm_migration::prelude::*;

#[derive(DeriveMigrationName)]
pub struct Migration;

// CJS-DB-1 (`custom-js-transforms.spec.md`): one row per custom JS transform.
// The metadata columns are derived from the source frontmatter on every save;
// the source column is the single source of truth.
const UP_TABLE: &str = "CREATE TABLE custom_transforms (\
    id TEXT PRIMARY KEY, \
    name TEXT NOT NULL, \
    description TEXT NOT NULL, \
    author TEXT NOT NULL, \
    source TEXT NOT NULL, \
    enabled BOOLEAN NOT NULL DEFAULT TRUE, \
    visibility TEXT NOT NULL, \
    phases TEXT NOT NULL, \
    scopes TEXT NOT NULL, \
    config_schema TEXT, \
    created_at TEXT NOT NULL, \
    updated_at TEXT NOT NULL)";
const DOWN_TABLE: &str = "DROP TABLE IF EXISTS custom_transforms";

#[async_trait::async_trait]
impl MigrationTrait for Migration {
    async fn up(&self, manager: &SchemaManager) -> Result<(), DbErr> {
        let backend = manager.get_database_backend();
        if !matches!(backend, DbBackend::Sqlite | DbBackend::Postgres) {
            return Ok(());
        }
        manager
            .get_connection()
            .execute(Statement::from_string(backend, UP_TABLE.to_string()))
            .await?;
        Ok(())
    }

    async fn down(&self, manager: &SchemaManager) -> Result<(), DbErr> {
        let backend = manager.get_database_backend();
        if !matches!(backend, DbBackend::Sqlite | DbBackend::Postgres) {
            return Ok(());
        }
        manager
            .get_connection()
            .execute(Statement::from_string(backend, DOWN_TABLE.to_string()))
            .await?;
        Ok(())
    }
}
