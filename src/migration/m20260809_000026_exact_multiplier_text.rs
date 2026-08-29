use sea_orm::{ConnectionTrait, DbBackend, Statement, TransactionTrait};
use sea_orm_migration::prelude::*;

#[derive(DeriveMigrationName)]
pub struct Migration;

#[async_trait::async_trait]
impl MigrationTrait for Migration {
    async fn up(&self, manager: &SchemaManager) -> Result<(), DbErr> {
        let backend = manager.get_database_backend();
        let conn = manager.get_connection();
        let statements = upgrade_sql(backend);
        if statements.is_empty() {
            return Ok(());
        }
        let tx = conn.begin().await?;
        let result: Result<(), DbErr> = async {
            for sql in statements {
                tx.execute(Statement::from_string(backend, String::from(*sql)))
                    .await?;
            }
            Ok(())
        }
        .await;
        if let Err(error) = result {
            let _ = tx.rollback().await;
            return Err(error);
        }
        tx.commit().await
    }

    async fn down(&self, _manager: &SchemaManager) -> Result<(), DbErr> {
        Ok(())
    }
}

fn upgrade_sql(backend: DbBackend) -> &'static [&'static str] {
    match backend {
        DbBackend::Postgres => &[
            "ALTER TABLE monoize_channel_models ALTER COLUMN multiplier DROP DEFAULT",
            "ALTER TABLE monoize_channel_models ALTER COLUMN multiplier TYPE TEXT USING multiplier::TEXT",
            "ALTER TABLE monoize_channel_models ALTER COLUMN multiplier SET DEFAULT '1'",
            "ALTER TABLE api_keys ALTER COLUMN max_multiplier TYPE TEXT USING max_multiplier::TEXT",
            "ALTER TABLE request_logs ALTER COLUMN provider_multiplier TYPE TEXT USING provider_multiplier::TEXT",
        ],
        DbBackend::Sqlite => &[
            "ALTER TABLE monoize_channel_models ADD COLUMN multiplier_exact TEXT NOT NULL DEFAULT '1'",
            "UPDATE monoize_channel_models SET multiplier_exact = CAST(multiplier AS TEXT)",
            "ALTER TABLE monoize_channel_models DROP COLUMN multiplier",
            "ALTER TABLE monoize_channel_models RENAME COLUMN multiplier_exact TO multiplier",
            "ALTER TABLE api_keys ADD COLUMN max_multiplier_exact TEXT",
            "UPDATE api_keys SET max_multiplier_exact = CAST(max_multiplier AS TEXT) WHERE max_multiplier IS NOT NULL",
            "ALTER TABLE api_keys DROP COLUMN max_multiplier",
            "ALTER TABLE api_keys RENAME COLUMN max_multiplier_exact TO max_multiplier",
            "ALTER TABLE request_logs ADD COLUMN provider_multiplier_exact TEXT",
            "UPDATE request_logs SET provider_multiplier_exact = CAST(provider_multiplier AS TEXT) WHERE provider_multiplier IS NOT NULL",
            "ALTER TABLE request_logs DROP COLUMN provider_multiplier",
            "ALTER TABLE request_logs RENAME COLUMN provider_multiplier_exact TO provider_multiplier",
        ],
        _ => &[],
    }
}
