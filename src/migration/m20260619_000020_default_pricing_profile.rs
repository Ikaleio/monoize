use sea_orm::{ConnectionTrait, DbBackend, Statement};
use sea_orm_migration::prelude::*;

#[derive(DeriveMigrationName)]
pub struct Migration;

#[async_trait::async_trait]
impl MigrationTrait for Migration {
    async fn up(&self, manager: &SchemaManager) -> Result<(), DbErr> {
        rename_pricing_profile(manager, "legacy", "default").await
    }

    async fn down(&self, manager: &SchemaManager) -> Result<(), DbErr> {
        rename_pricing_profile(manager, "default", "legacy").await
    }
}

async fn rename_pricing_profile(
    manager: &SchemaManager<'_>,
    from: &str,
    to: &str,
) -> Result<(), DbErr> {
    let conn = manager.get_connection();
    let backend = manager.get_database_backend();

    conn.execute(Statement::from_string(
        backend,
        format!(
            "UPDATE billing_rate_records SET pricing_profile = '{to}' WHERE pricing_profile = '{from}'"
        ),
    ))
    .await?;

    let compact_from = format!("\"pricing_profile\":\"{from}\"");
    let compact_to = format!("\"pricing_profile\":\"{to}\"");
    let spaced_from = format!("\"pricing_profile\": \"{from}\"");
    let spaced_to = format!("\"pricing_profile\": \"{to}\"");
    let sql = match backend {
        DbBackend::Sqlite | DbBackend::Postgres => format!(
            "UPDATE system_settings
             SET value = REPLACE(REPLACE(value, '{compact_from}', '{compact_to}'), '{spaced_from}', '{spaced_to}')
             WHERE key = 'pricing_profile_model_patterns'"
        ),
        _ => return Ok(()),
    };
    conn.execute(Statement::from_string(backend, sql)).await?;

    Ok(())
}
