use sea_orm::{DbBackend, Statement};
use sea_orm_migration::prelude::*;

const VISIBLE_TPS_COLUMNS: [&str; 5] = [
    "first_visible_output_ms",
    "last_visible_output_ms",
    "visible_generation_ms",
    "visible_output_tokens",
    "tps_mode",
];

#[derive(DeriveMigrationName)]
pub struct Migration;

#[async_trait::async_trait]
impl MigrationTrait for Migration {
    async fn up(&self, manager: &SchemaManager) -> Result<(), DbErr> {
        let conn = manager.get_connection();
        let backend = manager.get_database_backend();

        match backend {
            DbBackend::Sqlite => {
                let rows = conn
                    .query_all(Statement::from_string(
                        DbBackend::Sqlite,
                        "PRAGMA table_info(request_logs)".to_string(),
                    ))
                    .await?;
                let existing = rows
                    .into_iter()
                    .filter_map(|row| row.try_get::<String>("", "name").ok())
                    .collect::<std::collections::HashSet<_>>();
                for column in VISIBLE_TPS_COLUMNS {
                    if existing.contains(column) {
                        let sql = format!("ALTER TABLE request_logs DROP COLUMN {column}");
                        conn.execute(Statement::from_string(DbBackend::Sqlite, sql))
                            .await?;
                    }
                }
            }
            DbBackend::Postgres => {
                for column in VISIBLE_TPS_COLUMNS {
                    let sql = format!("ALTER TABLE request_logs DROP COLUMN IF EXISTS {column}");
                    conn.execute(Statement::from_string(DbBackend::Postgres, sql))
                        .await?;
                }
            }
            _ => {}
        }

        Ok(())
    }

    // Dropped visible-TPS values cannot be reconstructed, so down is a no-op (RL-S12).
    async fn down(&self, _manager: &SchemaManager) -> Result<(), DbErr> {
        Ok(())
    }
}
