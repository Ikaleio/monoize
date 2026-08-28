use sea_orm::{ConnectionTrait, DbBackend, Statement, TransactionTrait};
use sea_orm_migration::prelude::*;

#[derive(DeriveMigrationName)]
pub struct Migration;

const CREATE_PLANS: &str = "CREATE TABLE billing_plans (\
    id TEXT PRIMARY KEY, \
    name TEXT NOT NULL, \
    description TEXT NOT NULL DEFAULT '', \
    limit_5h_nano_usd TEXT, \
    limit_24h_nano_usd TEXT, \
    limit_7d_nano_usd TEXT, \
    limit_30d_nano_usd TEXT, \
    group_ids TEXT NOT NULL, \
    multiplier TEXT NOT NULL DEFAULT '1', \
    listed INTEGER NOT NULL DEFAULT 0, \
    created_at TEXT NOT NULL, \
    updated_at TEXT NOT NULL)";

const CREATE_PRICES: &str = "CREATE TABLE billing_plan_prices (\
    id TEXT PRIMARY KEY, \
    plan_id TEXT NOT NULL, \
    price_nano_usd TEXT NOT NULL, \
    duration_seconds BIGINT NOT NULL, \
    created_at TEXT NOT NULL)";

const CREATE_SUBSCRIPTIONS: &str = "CREATE TABLE billing_plan_subscriptions (\
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
    created_at TEXT NOT NULL)";

const CREATE_USAGE: &str = "CREATE TABLE billing_plan_usage (\
    id TEXT PRIMARY KEY, \
    subscription_id TEXT NOT NULL, \
    user_id TEXT NOT NULL, \
    api_key_id TEXT NOT NULL, \
    request_id TEXT NOT NULL, \
    group_id TEXT NOT NULL, \
    amount_nano_usd TEXT NOT NULL, \
    created_at TEXT NOT NULL)";

const CREATE_INDEXES: [&str; 7] = [
    "CREATE UNIQUE INDEX uq_billing_plans_name_lower ON billing_plans (lower(trim(name)))",
    "CREATE UNIQUE INDEX uq_billing_plan_prices_duration ON billing_plan_prices (plan_id, duration_seconds)",
    "CREATE INDEX idx_billing_plan_prices_plan ON billing_plan_prices (plan_id)",
    "CREATE INDEX idx_billing_plan_subscriptions_user_expires ON billing_plan_subscriptions (user_id, expires_at)",
    "CREATE INDEX idx_billing_plan_subscriptions_plan ON billing_plan_subscriptions (plan_id)",
    "CREATE UNIQUE INDEX uq_billing_plan_usage_request ON billing_plan_usage (request_id)",
    "CREATE INDEX idx_billing_plan_usage_subscription_time ON billing_plan_usage (subscription_id, created_at)",
];

const CREATE_OLD_PLANS: &str = "CREATE TABLE billing_plans (\
    id TEXT NOT NULL PRIMARY KEY, \
    name TEXT NOT NULL, \
    grant_amount_nano_usd TEXT NOT NULL, \
    schedule TEXT NOT NULL, \
    group_ids TEXT NOT NULL DEFAULT '[]', \
    enabled INTEGER NOT NULL DEFAULT 1, \
    created_at TEXT NOT NULL, \
    updated_at TEXT NOT NULL)";

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

#[async_trait::async_trait]
impl MigrationTrait for Migration {
    async fn up(&self, manager: &SchemaManager) -> Result<(), DbErr> {
        let backend = manager.get_database_backend();
        if !matches!(backend, DbBackend::Sqlite | DbBackend::Postgres) {
            return Ok(());
        }
        let tx = manager.get_connection().begin().await?;
        execute(
            &tx,
            backend,
            "UPDATE users SET billing_plan_id = NULL, next_grant_at = NULL",
        )
        .await?;
        execute(
            &tx,
            backend,
            "DROP INDEX IF EXISTS idx_users_billing_plan_id",
        )
        .await?;
        execute(
            &tx,
            backend,
            "DROP INDEX IF EXISTS uq_billing_plans_name_lower",
        )
        .await?;
        execute(&tx, backend, "DROP TABLE billing_plans").await?;
        execute(
            &tx,
            backend,
            "ALTER TABLE users DROP COLUMN billing_plan_id",
        )
        .await?;
        execute(&tx, backend, "ALTER TABLE users DROP COLUMN next_grant_at").await?;
        execute(&tx, backend, CREATE_PLANS).await?;
        execute(&tx, backend, CREATE_PRICES).await?;
        execute(&tx, backend, CREATE_SUBSCRIPTIONS).await?;
        execute(&tx, backend, CREATE_USAGE).await?;
        for sql in CREATE_INDEXES {
            execute(&tx, backend, sql).await?;
        }
        tx.commit().await?;
        Ok(())
    }

    async fn down(&self, manager: &SchemaManager) -> Result<(), DbErr> {
        let backend = manager.get_database_backend();
        if !matches!(backend, DbBackend::Sqlite | DbBackend::Postgres) {
            return Ok(());
        }
        let tx = manager.get_connection().begin().await?;
        for sql in [
            "DROP TABLE IF EXISTS billing_plan_usage",
            "DROP TABLE IF EXISTS billing_plan_subscriptions",
            "DROP TABLE IF EXISTS billing_plan_prices",
            "DROP TABLE IF EXISTS billing_plans",
        ] {
            execute(&tx, backend, sql).await?;
        }
        execute(&tx, backend, CREATE_OLD_PLANS).await?;
        execute(
            &tx,
            backend,
            "CREATE UNIQUE INDEX uq_billing_plans_name_lower ON billing_plans (lower(name))",
        )
        .await?;
        let add_plan_column = if backend == DbBackend::Postgres {
            "ALTER TABLE users ADD COLUMN IF NOT EXISTS billing_plan_id TEXT"
        } else {
            "ALTER TABLE users ADD COLUMN billing_plan_id TEXT"
        };
        let add_grant_column = if backend == DbBackend::Postgres {
            "ALTER TABLE users ADD COLUMN IF NOT EXISTS next_grant_at TEXT"
        } else {
            "ALTER TABLE users ADD COLUMN next_grant_at TEXT"
        };
        execute(&tx, backend, add_plan_column).await?;
        execute(&tx, backend, add_grant_column).await?;
        execute(
            &tx,
            backend,
            "CREATE INDEX idx_users_billing_plan_id ON users (billing_plan_id)",
        )
        .await?;
        tx.commit().await?;
        Ok(())
    }
}
