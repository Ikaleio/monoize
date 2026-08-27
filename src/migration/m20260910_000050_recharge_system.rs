use sea_orm::{ConnectionTrait, DbBackend, Statement};
use sea_orm_migration::prelude::*;

#[derive(DeriveMigrationName)]
pub struct Migration;

// RC-V1/RC-V2 (`recharge-system.spec.md` §14): purely additive step that
// creates `payment_channels` and `recharge_orders` with the §3 columns and
// indexes. `billing_ledger` and `users` are intentionally untouched: the
// idempotency-key column and its partial unique index already exist
// (m20260823_000033_billing_ledger_delta_dedupe).
const UP_PAYMENT_CHANNELS: &str = "CREATE TABLE payment_channels (\
    id TEXT PRIMARY KEY, \
    name TEXT NOT NULL, \
    type_id TEXT NOT NULL, \
    enabled INTEGER NOT NULL DEFAULT 1, \
    currency TEXT NOT NULL, \
    usd_rate TEXT NOT NULL, \
    min_credit_usd TEXT NOT NULL DEFAULT '1', \
    max_credit_usd TEXT NOT NULL DEFAULT '10000', \
    config_json TEXT NOT NULL, \
    sort_order INTEGER NOT NULL DEFAULT 0, \
    created_at TEXT NOT NULL, \
    updated_at TEXT NOT NULL)";

// The unique expression index enforces §3.1 name uniqueness after
// lower(trim(name)) at the storage layer, so concurrent creates cannot race
// past the handler-level duplicate check (RC-A6).
const UP_PAYMENT_CHANNELS_NAME_INDEX: &str = "CREATE UNIQUE INDEX \
    uidx_payment_channels_name ON payment_channels (lower(trim(name)))";

const UP_RECHARGE_ORDERS: &str = "CREATE TABLE recharge_orders (\
    id TEXT PRIMARY KEY, \
    user_id TEXT NOT NULL, \
    payment_channel_id TEXT NOT NULL, \
    channel_type_id TEXT NOT NULL, \
    channel_name TEXT NOT NULL, \
    status TEXT NOT NULL, \
    credit_nano_usd TEXT NOT NULL, \
    pay_currency TEXT NOT NULL, \
    pay_amount TEXT NOT NULL, \
    usd_rate TEXT NOT NULL, \
    provider_order_id TEXT, \
    error_code TEXT, \
    paid_at TEXT, \
    expires_at TEXT NOT NULL, \
    meta_json TEXT NOT NULL DEFAULT '{}', \
    created_at TEXT NOT NULL, \
    updated_at TEXT NOT NULL)";

const UP_ORDER_INDEXES: [&str; 3] = [
    "CREATE INDEX idx_recharge_orders_user_created ON recharge_orders (user_id, created_at)",
    "CREATE INDEX idx_recharge_orders_status_expires ON recharge_orders (status, expires_at)",
    "CREATE INDEX idx_recharge_orders_provider_order_id ON recharge_orders (provider_order_id)",
];

const DOWN_STATEMENTS: [&str; 2] = [
    "DROP TABLE IF EXISTS recharge_orders",
    "DROP TABLE IF EXISTS payment_channels",
];

#[async_trait::async_trait]
impl MigrationTrait for Migration {
    async fn up(&self, manager: &SchemaManager) -> Result<(), DbErr> {
        let backend = manager.get_database_backend();
        if !matches!(backend, DbBackend::Sqlite | DbBackend::Postgres) {
            return Ok(());
        }
        let connection = manager.get_connection();
        connection
            .execute(Statement::from_string(
                backend,
                UP_PAYMENT_CHANNELS.to_string(),
            ))
            .await?;
        connection
            .execute(Statement::from_string(
                backend,
                UP_PAYMENT_CHANNELS_NAME_INDEX.to_string(),
            ))
            .await?;
        connection
            .execute(Statement::from_string(
                backend,
                UP_RECHARGE_ORDERS.to_string(),
            ))
            .await?;
        for statement in UP_ORDER_INDEXES {
            connection
                .execute(Statement::from_string(backend, statement.to_string()))
                .await?;
        }
        Ok(())
    }

    async fn down(&self, manager: &SchemaManager) -> Result<(), DbErr> {
        let backend = manager.get_database_backend();
        if !matches!(backend, DbBackend::Sqlite | DbBackend::Postgres) {
            return Ok(());
        }
        for statement in DOWN_STATEMENTS {
            manager
                .get_connection()
                .execute(Statement::from_string(backend, statement.to_string()))
                .await?;
        }
        Ok(())
    }
}
