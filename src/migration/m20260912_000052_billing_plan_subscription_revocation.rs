use sea_orm::{ConnectionTrait, DbBackend, Statement};
use sea_orm_migration::prelude::*;

#[derive(DeriveMigrationName)]
pub struct Migration;

#[async_trait::async_trait]
impl MigrationTrait for Migration {
    async fn up(&self, manager: &SchemaManager) -> Result<(), DbErr> {
        let backend = manager.get_database_backend();
        if matches!(backend, DbBackend::Sqlite | DbBackend::Postgres) {
            manager
                .get_connection()
                .execute(Statement::from_string(
                    backend,
                    "ALTER TABLE billing_plan_subscriptions ADD COLUMN revoked_at TEXT NULL"
                        .to_string(),
                ))
                .await?;
        }
        Ok(())
    }

    async fn down(&self, manager: &SchemaManager) -> Result<(), DbErr> {
        let backend = manager.get_database_backend();
        if matches!(backend, DbBackend::Sqlite | DbBackend::Postgres) {
            manager
                .get_connection()
                .execute(Statement::from_string(
                    backend,
                    "ALTER TABLE billing_plan_subscriptions DROP COLUMN revoked_at".to_string(),
                ))
                .await?;
        }
        Ok(())
    }
}
