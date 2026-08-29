use sea_orm::{ConnectionTrait, DbBackend, Statement};
use sea_orm_migration::prelude::*;

#[derive(DeriveMigrationName)]
pub struct Migration;

#[async_trait::async_trait]
impl MigrationTrait for Migration {
    async fn up(&self, manager: &SchemaManager) -> Result<(), DbErr> {
        let backend = manager.get_database_backend();
        if !matches!(backend, DbBackend::Sqlite | DbBackend::Postgres) {
            return Ok(());
        }

        let connection = manager.get_connection();
        connection
            .execute(Statement::from_string(
                backend,
                "CREATE INDEX IF NOT EXISTS idx_api_keys_key ON api_keys(key)".to_string(),
            ))
            .await?;
        connection
            .execute(Statement::from_string(
                backend,
                "DROP INDEX IF EXISTS idx_api_keys_key_hash".to_string(),
            ))
            .await?;

        if column_exists(connection, backend, "api_keys", "key_hash").await? {
            let sql = match backend {
                DbBackend::Postgres => {
                    "ALTER TABLE api_keys DROP COLUMN IF EXISTS key_hash".to_string()
                }
                _ => "ALTER TABLE api_keys DROP COLUMN key_hash".to_string(),
            };
            connection
                .execute(Statement::from_string(backend, sql))
                .await?;
        }

        Ok(())
    }

    async fn down(&self, _manager: &SchemaManager) -> Result<(), DbErr> {
        // The plaintext-only storage decision forbids restoring the removed hash column.
        Ok(())
    }
}

async fn column_exists(
    connection: &SchemaManagerConnection<'_>,
    backend: DbBackend,
    table: &str,
    column: &str,
) -> Result<bool, DbErr> {
    let sql = match backend {
        DbBackend::Sqlite => format!("PRAGMA table_info({table})"),
        DbBackend::Postgres => format!(
            "SELECT column_name AS name FROM information_schema.columns WHERE table_schema = current_schema() AND table_name = '{table}'"
        ),
        _ => return Ok(false),
    };
    let rows = connection
        .query_all(Statement::from_string(backend, sql))
        .await?;
    Ok(rows
        .into_iter()
        .filter_map(|row| row.try_get::<String>("", "name").ok())
        .any(|name| name == column))
}
