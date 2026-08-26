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

#[cfg(test)]
mod tests {
    use crate::db::DbPool;
    use crate::migration::Migrator;
    use sea_orm::ConnectionTrait;
    use sea_orm_migration::MigratorTrait;

    #[tokio::test]
    async fn pending_status_partial_index_exists_after_migration() {
        let db = DbPool::connect("sqlite::memory:")
            .await
            .expect("db connects");
        {
            let write = db.write().await;
            Migrator::up(&*write, None).await.expect("migrates");
        }
        let row = db
            .read()
            .query_one(db.stmt(
                "SELECT sql FROM sqlite_master WHERE type = 'index' \
                 AND name = 'idx_request_logs_status_pending' AND tbl_name = 'request_logs'",
                vec![],
            ))
            .await
            .expect("index query succeeds")
            .expect("partial index exists");
        let create_sql: String = row.try_get("", "sql").expect("sql decodes");
        assert!(create_sql.contains("WHERE status = 'pending'"));
    }
}
