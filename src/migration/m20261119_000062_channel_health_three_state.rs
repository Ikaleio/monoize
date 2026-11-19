use sea_orm::{ConnectionTrait, DbBackend, Statement};
use sea_orm_migration::prelude::*;

#[derive(DeriveMigrationName)]
pub struct Migration;

const TABLES: [&str; 2] = ["monoize_providers", "monoize_channels"];
const OLD_COLUMN: &str = "active_probe_enabled_override";
const NEW_COLUMNS: &[&str] = &["honor_rate_limit_override", "active_probe_stream_override"];

#[async_trait::async_trait]
impl MigrationTrait for Migration {
    async fn up(&self, manager: &SchemaManager) -> Result<(), DbErr> {
        replace_columns(manager, &[OLD_COLUMN], NEW_COLUMNS).await?;
        manager
            .get_connection()
            .execute(Statement::from_string(
                manager.get_database_backend(),
                "DELETE FROM system_settings WHERE key = 'monoize_active_probe_enabled'",
            ))
            .await?;
        Ok(())
    }

    async fn down(&self, manager: &SchemaManager) -> Result<(), DbErr> {
        replace_columns(manager, NEW_COLUMNS, &[OLD_COLUMN]).await
    }
}

async fn replace_columns(
    manager: &SchemaManager<'_>,
    remove: &[&str],
    add: &[&str],
) -> Result<(), DbErr> {
    let backend = manager.get_database_backend();
    let (if_exists, if_not_exists) = match backend {
        DbBackend::Sqlite => ("", ""),
        DbBackend::Postgres => ("IF EXISTS ", "IF NOT EXISTS "),
        _ => return Ok(()),
    };
    let conn = manager.get_connection();
    for table in TABLES {
        for old in remove {
            if manager.has_column(table, old).await? {
                conn.execute(Statement::from_string(
                    backend,
                    format!("ALTER TABLE {table} DROP COLUMN {if_exists}{old}"),
                ))
                .await?;
            }
        }
        for new in add {
            if !manager.has_column(table, new).await? {
                conn.execute(Statement::from_string(
                    backend,
                    format!("ALTER TABLE {table} ADD COLUMN {if_not_exists}{new} INTEGER"),
                ))
                .await?;
            }
        }
    }
    Ok(())
}
