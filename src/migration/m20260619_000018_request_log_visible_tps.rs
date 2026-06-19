use sea_orm::{ConnectionTrait, DbBackend, Statement};
use sea_orm_migration::prelude::*;

#[derive(DeriveMigrationName)]
pub struct Migration;

#[async_trait::async_trait]
impl MigrationTrait for Migration {
    async fn up(&self, manager: &SchemaManager) -> Result<(), DbErr> {
        let conn = manager.get_connection();
        let backend = manager.get_database_backend();
        for (column, definition) in [
            ("first_visible_output_ms", "INTEGER"),
            ("last_visible_output_ms", "INTEGER"),
            ("visible_generation_ms", "INTEGER"),
            ("visible_output_tokens", "INTEGER"),
            ("tps_mode", "TEXT"),
        ] {
            add_column_if_missing(conn, backend, "request_logs", column, definition).await?;
        }
        Ok(())
    }

    async fn down(&self, _manager: &SchemaManager) -> Result<(), DbErr> {
        Ok(())
    }
}

async fn add_column_if_missing(
    conn: &SchemaManagerConnection<'_>,
    backend: DbBackend,
    table: &str,
    column: &str,
    definition: &str,
) -> Result<(), DbErr> {
    if column_exists(conn, backend, table, column).await? {
        return Ok(());
    }
    let sql = match backend {
        DbBackend::Sqlite => format!("ALTER TABLE {table} ADD COLUMN {column} {definition}"),
        DbBackend::Postgres => {
            format!("ALTER TABLE {table} ADD COLUMN IF NOT EXISTS {column} {definition}")
        }
        _ => return Ok(()),
    };
    conn.execute(Statement::from_string(backend, sql)).await?;
    Ok(())
}

async fn column_exists(
    conn: &SchemaManagerConnection<'_>,
    backend: DbBackend,
    table: &str,
    column: &str,
) -> Result<bool, DbErr> {
    let sql = match backend {
        DbBackend::Sqlite => format!("PRAGMA table_info({table})"),
        DbBackend::Postgres => format!(
            "SELECT column_name AS name FROM information_schema.columns WHERE table_name = '{table}'"
        ),
        _ => return Ok(false),
    };
    let rows = conn.query_all(Statement::from_string(backend, sql)).await?;
    Ok(rows
        .into_iter()
        .filter_map(|row| row.try_get::<String>("", "name").ok())
        .any(|name| name == column))
}
