use sea_orm::DeriveRelation;
use sea_orm::entity::prelude::*;

#[derive(Clone, Debug, PartialEq, Eq, DeriveEntityModel)]
#[sea_orm(table_name = "payment_channels")]
pub struct Model {
    #[sea_orm(primary_key, auto_increment = false, column_type = "Text")]
    pub id: String,
    #[sea_orm(column_type = "Text")]
    pub name: String,
    #[sea_orm(column_type = "Text")]
    pub type_id: String,
    pub enabled: i32,
    #[sea_orm(column_type = "Text")]
    pub currency: String,
    #[sea_orm(column_type = "Text")]
    pub usd_rate: String,
    #[sea_orm(column_type = "Text")]
    pub min_credit_usd: String,
    #[sea_orm(column_type = "Text")]
    pub max_credit_usd: String,
    #[sea_orm(column_type = "Text")]
    pub config_json: String,
    pub sort_order: i32,
    #[sea_orm(column_type = "Text")]
    pub created_at: String,
    #[sea_orm(column_type = "Text")]
    pub updated_at: String,
}

#[derive(Copy, Clone, Debug, EnumIter, DeriveRelation)]
pub enum Relation {}

impl sea_orm::ActiveModelBehavior for ActiveModel {}
