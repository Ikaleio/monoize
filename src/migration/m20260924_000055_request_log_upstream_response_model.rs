use sea_orm::{ConnectionTrait, DbBackend, Statement};
use sea_orm_migration::prelude::*;

#[derive(DeriveMigrationName)]
pub struct Migration;

// RL1l (`request-logs.spec.md`): nullable actual model declared by the
// selected upstream response. NULL means the response omitted it or it
// matched the model sent upstream.
async fn add_column_if_missing(
    conn: &impl ConnectionTrait,
    backend: DbBackend,
    column: &str,
    column_type: &str,
) -> Result<(), DbErr> {
    let table = "request_logs";
    let exists = match backend {
        DbBackend::Postgres => {
            let rows = conn
                .query_all(Statement::from_string(
                    backend,
                    format!(
                        "SELECT 1 FROM information_schema.columns \
                         WHERE table_name = '{table}' AND column_name = '{column}'"
                    ),
                ))
                .await?;
            !rows.is_empty()
        }
        _ => {
            let rows = conn
                .query_all(Statement::from_string(
                    backend,
                    format!("PRAGMA table_info({table})"),
                ))
                .await?;
            rows.iter().any(|row| {
                row.try_get::<String>("", "name")
                    .map(|name| name == column)
                    .unwrap_or(false)
            })
        }
    };
    if exists {
        return Ok(());
    }
    conn.execute(Statement::from_string(
        backend,
        format!("ALTER TABLE {table} ADD COLUMN {column} {column_type}"),
    ))
    .await?;
    Ok(())
}

#[async_trait::async_trait]
impl MigrationTrait for Migration {
    async fn up(&self, manager: &SchemaManager) -> Result<(), DbErr> {
        let backend = manager.get_database_backend();
        if !matches!(backend, DbBackend::Sqlite | DbBackend::Postgres) {
            return Ok(());
        }
        add_column_if_missing(
            manager.get_connection(),
            backend,
            "upstream_response_model",
            "TEXT",
        )
        .await
    }

    async fn down(&self, manager: &SchemaManager) -> Result<(), DbErr> {
        let backend = manager.get_database_backend();
        if !matches!(backend, DbBackend::Sqlite | DbBackend::Postgres) {
            return Ok(());
        }
        let drop = match backend {
            DbBackend::Postgres => {
                "ALTER TABLE request_logs DROP COLUMN IF EXISTS upstream_response_model"
            }
            _ => "ALTER TABLE request_logs DROP COLUMN upstream_response_model",
        };
        manager
            .get_connection()
            .execute(Statement::from_string(backend, drop.to_string()))
            .await?;
        Ok(())
    }
}
