use sea_orm::DeriveRelation;
use sea_orm::entity::prelude::*;

#[derive(Clone, Debug, PartialEq, Eq, DeriveEntityModel)]
#[sea_orm(table_name = "billing_rate_records")]
pub struct Model {
    #[sea_orm(primary_key, auto_increment = false, column_type = "Text")]
    pub id: String,
    #[sea_orm(column_type = "Text")]
    pub source: String,
    #[sea_orm(column_type = "Text")]
    pub pricing_profile: String,
    #[sea_orm(column_type = "Text")]
    pub model_pattern: Option<String>,
    #[sea_orm(column_type = "Text")]
    pub provider_type: Option<String>,
    #[sea_orm(column_type = "Text")]
    pub rate_kind: String,
    #[sea_orm(column_type = "Text")]
    pub usage_class: String,
    #[sea_orm(column_type = "Text")]
    pub unit: String,
    #[sea_orm(column_type = "Text")]
    pub unit_price_nano_usd: String,
    #[sea_orm(column_type = "Text")]
    pub context_tier: Option<String>,
    #[sea_orm(column_type = "Text")]
    pub service_tier: Option<String>,
    #[sea_orm(column_type = "Text")]
    pub modality: Option<String>,
    #[sea_orm(column_type = "Text")]
    pub cache_ttl: Option<String>,
    #[sea_orm(column_type = "Text")]
    pub match_json: String,
    pub priority: i32,
    pub enabled: i32,
    #[sea_orm(column_type = "Text")]
    pub raw_json: String,
    #[sea_orm(column_type = "Text")]
    pub updated_at: String,
}

#[derive(Copy, Clone, Debug, EnumIter, DeriveRelation)]
pub enum Relation {}

impl sea_orm::ActiveModelBehavior for ActiveModel {}
