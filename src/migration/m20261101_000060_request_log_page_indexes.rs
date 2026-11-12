use sea_orm::{ConnectionTrait, DbBackend, Statement};
use sea_orm_migration::prelude::*;

#[derive(DeriveMigrationName)]
pub struct Migration;

#[async_trait::async_trait]
impl MigrationTrait for Migration {
    async fn up(&self, manager: &SchemaManager) -> Result<(), DbErr> {
        let backend = manager.get_database_backend();
        let nulls_last = if backend == DbBackend::Postgres {
            " NULLS LAST"
        } else {
            ""
        };
        let order = format!("created_at_unix_ms DESC{nulls_last}, created_at DESC, id DESC");
        for sql in [
            format!("CREATE INDEX IF NOT EXISTS idx_request_logs_page ON request_logs ({order})"),
            format!(
                "CREATE INDEX IF NOT EXISTS idx_request_logs_user_page ON request_logs (user_id, {order})"
            ),
            "DROP INDEX IF EXISTS idx_request_logs_created_at".to_string(),
            "DROP INDEX IF EXISTS idx_request_logs_user_created_at".to_string(),
        ] {
            manager
                .get_connection()
                .execute(Statement::from_string(backend, sql))
                .await?;
        }
        Ok(())
    }

    async fn down(&self, manager: &SchemaManager) -> Result<(), DbErr> {
        let backend = manager.get_database_backend();
        for sql in [
            "CREATE INDEX IF NOT EXISTS idx_request_logs_created_at ON request_logs (created_at_unix_ms DESC)",
            "CREATE INDEX IF NOT EXISTS idx_request_logs_user_created_at ON request_logs (user_id, created_at_unix_ms DESC)",
            "DROP INDEX IF EXISTS idx_request_logs_page",
            "DROP INDEX IF EXISTS idx_request_logs_user_page",
        ] {
            manager
                .get_connection()
                .execute(Statement::from_string(backend, sql))
                .await?;
        }
        Ok(())
    }
}
