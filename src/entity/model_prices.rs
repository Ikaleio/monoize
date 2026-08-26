use sea_orm::DeriveRelation;
use sea_orm::entity::prelude::*;

/// One price sheet per model (`model-pricing.spec.md` §2.1). All price columns
/// are exact USD decimal strings; ratios are derived in the UI, never stored.
#[derive(Clone, Debug, PartialEq, Eq, DeriveEntityModel)]
#[sea_orm(table_name = "model_prices")]
pub struct Model {
    #[sea_orm(primary_key, auto_increment = false, column_type = "Text")]
    pub model_id: String,
    #[sea_orm(column_type = "Text")]
    pub billing_mode: String,
    #[sea_orm(column_type = "Text")]
    pub input_usd_per_1m: Option<String>,
    #[sea_orm(column_type = "Text")]
    pub output_usd_per_1m: Option<String>,
    #[sea_orm(column_type = "Text")]
    pub cache_read_usd_per_1m: Option<String>,
    #[sea_orm(column_type = "Text")]
    pub cache_write_usd_per_1m: Option<String>,
    #[sea_orm(column_type = "Text")]
    pub cache_write_1h_usd_per_1m: Option<String>,
    #[sea_orm(column_type = "Text")]
    pub reasoning_usd_per_1m: Option<String>,
    #[sea_orm(column_type = "Text")]
    pub per_request_usd: Option<String>,
    #[sea_orm(column_type = "Text")]
    pub billing_expr: Option<String>,
    #[sea_orm(column_type = "Text")]
    pub source: String,
    #[sea_orm(column_type = "Text")]
    pub locked_fields: String,
    #[sea_orm(column_type = "Text")]
    pub raw_json: String,
    pub enabled: i32,
    #[sea_orm(column_type = "Text")]
    pub updated_at: String,
}

#[derive(Copy, Clone, Debug, EnumIter, DeriveRelation)]
pub enum Relation {}

impl sea_orm::ActiveModelBehavior for ActiveModel {}
