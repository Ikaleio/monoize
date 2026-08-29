use sea_orm::{ConnectionTrait, DbBackend, Statement, TransactionTrait};
use sea_orm_migration::prelude::*;

#[derive(DeriveMigrationName)]
pub struct Migration;

const CREATE_NULLABLE_SUBSCRIPTIONS: &str = "CREATE TABLE billing_plan_subscriptions (\
    id TEXT PRIMARY KEY, \
    user_id TEXT NOT NULL, \
    plan_id TEXT NOT NULL, \
    price_id TEXT, \
    plan_name TEXT NOT NULL, \
    plan_description TEXT NOT NULL, \
    limit_5h_nano_usd TEXT, \
    limit_24h_nano_usd TEXT, \
    limit_7d_nano_usd TEXT, \
    limit_30d_nano_usd TEXT, \
    group_ids TEXT NOT NULL, \
    multiplier TEXT NOT NULL, \
    price_nano_usd TEXT, \
    starts_at TEXT NOT NULL, \
    expires_at TEXT NOT NULL, \
    created_at TEXT NOT NULL, \
    revoked_at TEXT)";

const CREATE_REQUIRED_SUBSCRIPTIONS: &str = "CREATE TABLE billing_plan_subscriptions (\
    id TEXT PRIMARY KEY, \
    user_id TEXT NOT NULL, \
    plan_id TEXT NOT NULL, \
    price_id TEXT NOT NULL, \
    plan_name TEXT NOT NULL, \
    plan_description TEXT NOT NULL, \
    limit_5h_nano_usd TEXT, \
    limit_24h_nano_usd TEXT, \
    limit_7d_nano_usd TEXT, \
    limit_30d_nano_usd TEXT, \
    group_ids TEXT NOT NULL, \
    multiplier TEXT NOT NULL, \
    price_nano_usd TEXT NOT NULL, \
    starts_at TEXT NOT NULL, \
    expires_at TEXT NOT NULL, \
    created_at TEXT NOT NULL, \
    revoked_at TEXT)";

const COPY_SUBSCRIPTIONS: &str = "INSERT INTO billing_plan_subscriptions (\
    id, user_id, plan_id, price_id, plan_name, plan_description, \
    limit_5h_nano_usd, limit_24h_nano_usd, limit_7d_nano_usd, \
    limit_30d_nano_usd, group_ids, multiplier, price_nano_usd, \
    starts_at, expires_at, created_at, revoked_at) SELECT \
    id, user_id, plan_id, price_id, plan_name, plan_description, \
    limit_5h_nano_usd, limit_24h_nano_usd, limit_7d_nano_usd, \
    limit_30d_nano_usd, group_ids, multiplier, price_nano_usd, \
    starts_at, expires_at, created_at, revoked_at \
    FROM billing_plan_subscriptions_before_admin_grants";

async fn execute<C: ConnectionTrait>(
    connection: &C,
    backend: DbBackend,
    sql: &str,
) -> Result<(), DbErr> {
    connection
        .execute(Statement::from_string(backend, sql.to_string()))
        .await?;
    Ok(())
}

async fn rebuild_sqlite<C: ConnectionTrait>(
    connection: &C,
    create_statement: &str,
) -> Result<(), DbErr> {
    execute(
        connection,
        DbBackend::Sqlite,
        "ALTER TABLE billing_plan_subscriptions RENAME TO billing_plan_subscriptions_before_admin_grants",
    )
    .await?;
    execute(connection, DbBackend::Sqlite, create_statement).await?;
    execute(connection, DbBackend::Sqlite, COPY_SUBSCRIPTIONS).await?;
    execute(
        connection,
        DbBackend::Sqlite,
        "DROP TABLE billing_plan_subscriptions_before_admin_grants",
    )
    .await?;
    execute(
        connection,
        DbBackend::Sqlite,
        "CREATE INDEX idx_billing_plan_subscriptions_user_expires ON billing_plan_subscriptions (user_id, expires_at)",
    )
    .await?;
    execute(
        connection,
        DbBackend::Sqlite,
        "CREATE INDEX idx_billing_plan_subscriptions_plan ON billing_plan_subscriptions (plan_id)",
    )
    .await
}

async fn ensure_no_admin_grants<C: ConnectionTrait>(
    connection: &C,
    backend: DbBackend,
) -> Result<(), DbErr> {
    let row = connection
        .query_one(Statement::from_string(
            backend,
            "SELECT COUNT(*) AS grant_count FROM billing_plan_subscriptions WHERE price_id IS NULL OR price_nano_usd IS NULL".to_string(),
        ))
        .await?
        .ok_or_else(|| DbErr::Custom("failed to count administrator grants".to_string()))?;
    let grant_count: i64 = row.try_get("", "grant_count")?;
    if grant_count > 0 {
        return Err(DbErr::Custom(
            "cannot restore required subscription prices while administrator grants exist"
                .to_string(),
        ));
    }
    Ok(())
}

#[async_trait::async_trait]
impl MigrationTrait for Migration {
    async fn up(&self, manager: &SchemaManager) -> Result<(), DbErr> {
        let backend = manager.get_database_backend();
        let tx = manager.get_connection().begin().await?;
        match backend {
            DbBackend::Sqlite => rebuild_sqlite(&tx, CREATE_NULLABLE_SUBSCRIPTIONS).await?,
            DbBackend::Postgres => {
                execute(
                    &tx,
                    backend,
                    "ALTER TABLE billing_plan_subscriptions ALTER COLUMN price_id DROP NOT NULL, ALTER COLUMN price_nano_usd DROP NOT NULL",
                )
                .await?;
            }
            _ => {}
        }
        tx.commit().await
    }

    async fn down(&self, manager: &SchemaManager) -> Result<(), DbErr> {
        let backend = manager.get_database_backend();
        let tx = manager.get_connection().begin().await?;
        if matches!(backend, DbBackend::Sqlite | DbBackend::Postgres) {
            ensure_no_admin_grants(&tx, backend).await?;
        }
        match backend {
            DbBackend::Sqlite => rebuild_sqlite(&tx, CREATE_REQUIRED_SUBSCRIPTIONS).await?,
            DbBackend::Postgres => {
                execute(
                    &tx,
                    backend,
                    "ALTER TABLE billing_plan_subscriptions ALTER COLUMN price_id SET NOT NULL, ALTER COLUMN price_nano_usd SET NOT NULL",
                )
                .await?;
            }
            _ => {}
        }
        tx.commit().await
    }
}
