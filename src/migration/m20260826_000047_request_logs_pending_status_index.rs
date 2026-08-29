use sea_orm::{ConnectionTrait, DbBackend, Statement};
use sea_orm_migration::prelude::*;

#[derive(DeriveMigrationName)]
pub struct Migration;

// RL1f/RL3b (`request-logs.spec.md`) sweep `status = 'pending'` rows on every
// startup and shutdown, but terminal-only inserts (RL1a) mean the predicate
// matches at most legacy rows. The partial index turns that recurring
// full-table scan into an index probe that is nearly free to maintain because
// new rows never satisfy the predicate.
#[async_trait::async_trait]
impl MigrationTrait for Migration {
    async fn up(&self, manager: &SchemaManager) -> Result<(), DbErr> {
        let backend = manager.get_database_backend();
        if matches!(backend, DbBackend::Sqlite | DbBackend::Postgres) {
            manager
                .get_connection()
                .execute(Statement::from_string(
                    backend,
                    "CREATE INDEX IF NOT EXISTS idx_request_logs_status_pending ON request_logs (status) WHERE status = 'pending'".to_string(),
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
                    "DROP INDEX IF EXISTS idx_request_logs_status_pending".to_string(),
                ))
                .await?;
        }
        Ok(())
    }
}
