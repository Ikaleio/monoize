use sea_orm_migration::prelude::*;

#[derive(DeriveMigrationName)]
pub struct Migration;

const COLUMNS: [&str; 2] = ["cache_write_30m_usd_per_1m", "cache_write_12h_usd_per_1m"];

#[async_trait::async_trait]
impl MigrationTrait for Migration {
    async fn up(&self, manager: &SchemaManager) -> Result<(), DbErr> {
        for column in COLUMNS {
            if !manager.has_column("model_prices", column).await? {
                manager
                    .alter_table(
                        Table::alter()
                            .table(Alias::new("model_prices"))
                            .add_column(ColumnDef::new(Alias::new(column)).text().null())
                            .to_owned(),
                    )
                    .await?;
            }
        }
        Ok(())
    }

    async fn down(&self, manager: &SchemaManager) -> Result<(), DbErr> {
        for column in COLUMNS {
            if manager.has_column("model_prices", column).await? {
                manager
                    .alter_table(
                        Table::alter()
                            .table(Alias::new("model_prices"))
                            .drop_column(Alias::new(column))
                            .to_owned(),
                    )
                    .await?;
            }
        }
        Ok(())
    }
}
