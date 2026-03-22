use sea_orm_migration::prelude::*;

pub struct Migrator;

#[async_trait::async_trait]
impl MigratorTrait for Migrator {
    fn migrations() -> Vec<Box<dyn MigrationTrait>> {
        vec![
            Box::new(m20250101_000001_create_tables::Migration),
            Box::new(m20260229_000002_pg_request_logs_native_shadow::Migration),
            Box::new(m20260307_000003_drop_pg_request_logs_shadow::Migration),
            Box::new(m20260314_000004_request_log_retention_indexes::Migration),
            Box::new(m20260322_000005_retry_breaker_refactor::Migration),
            Box::new(m20260322_000006_provider_retry_interval_and_breaker_toggle::Migration),
        ]
    }
}

mod m20250101_000001_create_tables;
mod m20260229_000002_pg_request_logs_native_shadow;
mod m20260307_000003_drop_pg_request_logs_shadow;
mod m20260314_000004_request_log_retention_indexes;
mod m20260322_000005_retry_breaker_refactor;
mod m20260322_000006_provider_retry_interval_and_breaker_toggle;
