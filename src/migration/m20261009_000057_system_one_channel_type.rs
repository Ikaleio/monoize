use sea_orm::{ConnectionTrait, Statement};
use sea_orm_migration::prelude::*;

#[derive(DeriveMigrationName)]
pub struct Migration;

// DR-C4 (`decision-api.spec.md`): v1.13.2 stored the System One Channel type
// as `systemone`. Request logs keep their historical label.
async fn rename_type(manager: &SchemaManager<'_>, from: &str, to: &str) -> Result<(), DbErr> {
    let backend = manager.get_database_backend();
    let conn = manager.get_connection();
    conn.execute(Statement::from_string(
        backend,
        format!("UPDATE monoize_channels SET provider_type = '{to}' WHERE provider_type = '{from}'"),
    ))
    .await?;
    // `api_type_overrides` is compact serde JSON, so the field/value pair has
    // exactly this spelling and cannot match a `pattern` value.
    conn.execute(Statement::from_string(
        backend,
        format!(
            "UPDATE monoize_providers \
             SET api_type_overrides = REPLACE(api_type_overrides, '\"api_type\":\"{from}\"', '\"api_type\":\"{to}\"') \
             WHERE api_type_overrides LIKE '%\"api_type\":\"{from}\"%'"
        ),
    ))
    .await?;
    Ok(())
}

#[async_trait::async_trait]
impl MigrationTrait for Migration {
    async fn up(&self, manager: &SchemaManager) -> Result<(), DbErr> {
        rename_type(manager, "systemone", "system_one").await
    }

    async fn down(&self, manager: &SchemaManager) -> Result<(), DbErr> {
        rename_type(manager, "system_one", "systemone").await
    }
}
