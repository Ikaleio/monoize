use sea_orm::DeriveRelation;
use sea_orm::entity::prelude::*;

#[derive(Clone, Debug, PartialEq, DeriveEntityModel)]
#[sea_orm(table_name = "billing_plans")]
pub struct Model {
    #[sea_orm(primary_key, auto_increment = false, column_type = "Text")]
    pub id: String,
    #[sea_orm(column_type = "Text")]
    pub name: String,
    #[sea_orm(column_type = "Text")]
    pub description: String,
    #[sea_orm(column_type = "Text")]
    pub limit_5h_nano_usd: Option<String>,
    #[sea_orm(column_type = "Text")]
    pub limit_24h_nano_usd: Option<String>,
    #[sea_orm(column_type = "Text")]
    pub limit_7d_nano_usd: Option<String>,
    #[sea_orm(column_type = "Text")]
    pub limit_30d_nano_usd: Option<String>,
    #[sea_orm(column_type = "Text")]
    pub group_ids: String,
    #[sea_orm(column_type = "Text")]
    pub multiplier: String,
    pub listed: i32,
    #[sea_orm(column_type = "Text")]
    pub created_at: String,
    #[sea_orm(column_type = "Text")]
    pub updated_at: String,
}

#[derive(Copy, Clone, Debug, EnumIter, DeriveRelation)]
pub enum Relation {}

impl sea_orm::ActiveModelBehavior for ActiveModel {}
