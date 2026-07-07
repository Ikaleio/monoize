use sea_orm::{ConnectionTrait, Statement};
use sea_orm_migration::prelude::*;

#[derive(DeriveMigrationName)]
pub struct Migration;

#[async_trait::async_trait]
impl MigrationTrait for Migration {
    async fn up(&self, manager: &SchemaManager) -> Result<(), DbErr> {
        manager
            .create_table(
                Table::create()
                    .table(BillingRateRecords::Table)
                    .if_not_exists()
                    .col(
                        ColumnDef::new(BillingRateRecords::Id)
                            .text()
                            .not_null()
                            .primary_key(),
                    )
                    .col(ColumnDef::new(BillingRateRecords::Source).text().not_null())
                    .col(
                        ColumnDef::new(BillingRateRecords::PricingProfile)
                            .text()
                            .not_null(),
                    )
                    .col(ColumnDef::new(BillingRateRecords::ModelPattern).text())
                    .col(ColumnDef::new(BillingRateRecords::ProviderType).text())
                    .col(
                        ColumnDef::new(BillingRateRecords::RateKind)
                            .text()
                            .not_null(),
                    )
                    .col(
                        ColumnDef::new(BillingRateRecords::UsageClass)
                            .text()
                            .not_null(),
                    )
                    .col(ColumnDef::new(BillingRateRecords::Unit).text().not_null())
                    .col(
                        ColumnDef::new(BillingRateRecords::UnitPriceNanoUsd)
                            .text()
                            .not_null(),
                    )
                    .col(ColumnDef::new(BillingRateRecords::ContextTier).text())
                    .col(ColumnDef::new(BillingRateRecords::ServiceTier).text())
                    .col(ColumnDef::new(BillingRateRecords::Modality).text())
                    .col(ColumnDef::new(BillingRateRecords::CacheTtl).text())
                    .col(
                        ColumnDef::new(BillingRateRecords::MatchJson)
                            .text()
                            .not_null()
                            .default("{}"),
                    )
                    .col(
                        ColumnDef::new(BillingRateRecords::Priority)
                            .integer()
                            .not_null()
                            .default(0),
                    )
                    .col(
                        ColumnDef::new(BillingRateRecords::Enabled)
                            .integer()
                            .not_null()
                            .default(1),
                    )
                    .col(
                        ColumnDef::new(BillingRateRecords::RawJson)
                            .text()
                            .not_null()
                            .default("{}"),
                    )
                    .col(
                        ColumnDef::new(BillingRateRecords::UpdatedAt)
                            .text()
                            .not_null(),
                    )
                    .to_owned(),
            )
            .await?;

        manager
            .create_index(
                Index::create()
                    .if_not_exists()
                    .name("idx_billing_rate_records_lookup")
                    .table(BillingRateRecords::Table)
                    .col(BillingRateRecords::PricingProfile)
                    .col(BillingRateRecords::RateKind)
                    .col(BillingRateRecords::UsageClass)
                    .to_owned(),
            )
            .await?;

        backfill_from_model_metadata(manager).await?;

        Ok(())
    }

    async fn down(&self, manager: &SchemaManager) -> Result<(), DbErr> {
        manager
            .drop_table(
                Table::drop()
                    .table(BillingRateRecords::Table)
                    .if_exists()
                    .to_owned(),
            )
            .await
    }
}

async fn backfill_from_model_metadata(manager: &SchemaManager<'_>) -> Result<(), DbErr> {
    let conn = manager.get_connection();
    let backend = manager.get_database_backend();
    for (suffix, column, usage_class) in [
        (
            "input_uncached",
            "input_cost_per_token_nano",
            "input_uncached",
        ),
        ("output", "output_cost_per_token_nano", "output"),
        (
            "cache_read",
            "cache_read_input_cost_per_token_nano",
            "cache_read",
        ),
        (
            "cache_write_5m",
            "cache_creation_input_cost_per_token_nano",
            "cache_write_5m",
        ),
        (
            "reasoning_output",
            "output_cost_per_reasoning_token_nano",
            "reasoning_output",
        ),
    ] {
        let sql = format!(
            "INSERT INTO billing_rate_records
             (id, source, pricing_profile, model_pattern, provider_type, rate_kind, usage_class,
              unit, unit_price_nano_usd, match_json, priority, enabled, raw_json, updated_at)
             SELECT
              'model_metadata:' || model_id || ':{suffix}',
              source,
              COALESCE(models_dev_provider, 'default'),
              model_id,
              NULL,
              'token',
              '{usage_class}',
              'token',
              {column},
              '{{}}',
              0,
              1,
              '{{\"backfill\":\"model_metadata_records\"}}',
              updated_at
             FROM model_metadata_records
             WHERE {column} IS NOT NULL
             ON CONFLICT(id) DO NOTHING"
        );
        conn.execute(Statement::from_string(backend, sql)).await?;
    }
    Ok(())
}

#[derive(DeriveIden)]
enum BillingRateRecords {
    Table,
    Id,
    Source,
    PricingProfile,
    ModelPattern,
    ProviderType,
    RateKind,
    UsageClass,
    Unit,
    UnitPriceNanoUsd,
    ContextTier,
    ServiceTier,
    Modality,
    CacheTtl,
    MatchJson,
    Priority,
    Enabled,
    RawJson,
    UpdatedAt,
}
