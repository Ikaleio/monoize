use sea_orm::DeriveRelation;
use sea_orm::entity::prelude::*;

#[derive(Clone, Debug, PartialEq, DeriveEntityModel)]
#[sea_orm(table_name = "billing_plans")]
pub struct Model {
    #[sea_orm(primary_key, auto_increment = false, column_type = "Text")]
    pub id: String,
    #[sea_orm(unique, column_type = "Text")]
    pub name: String,
    #[sea_orm(column_type = "Text")]
    pub grant_amount_nano_usd: String,
    #[sea_orm(column_type = "BigInteger")]
    pub period_seconds: i64,
    #[sea_orm(column_type = "Text")]
    pub allowed_groups: String,
    pub enabled: i32,
    #[sea_orm(column_type = "Text")]
    pub created_at: String,
    #[sea_orm(column_type = "Text")]
    pub updated_at: String,
}

#[derive(Copy, Clone, Debug, EnumIter, DeriveRelation)]
pub enum Relation {}

impl sea_orm::ActiveModelBehavior for ActiveModel {}
