use sea_orm::{ConnectionTrait, Statement};
use sea_orm_migration::prelude::*;

#[derive(DeriveMigrationName)]
pub struct Migration;

// SSU-26 (`system-settings-ui.spec.md`): no runtime code read these settings.
#[async_trait::async_trait]
impl MigrationTrait for Migration {
    async fn up(&self, manager: &SchemaManager) -> Result<(), DbErr> {
        manager
            .get_connection()
            .execute(Statement::from_string(
                manager.get_database_backend(),
                "DELETE FROM system_settings WHERE key IN (\
                 'default_user_role', \
                 'monoize_passive_min_samples', \
                 'monoize_passive_failure_rate_threshold')"
                    .to_string(),
            ))
            .await?;
        Ok(())
    }

    // An older binary reseeds missing settings with its defaults on startup.
    async fn down(&self, _manager: &SchemaManager) -> Result<(), DbErr> {
        Ok(())
    }
}
