use sea_orm::entity::prelude::*;
use sea_orm::DeriveRelation;

#[derive(Clone, Debug, PartialEq, DeriveEntityModel)]
#[sea_orm(table_name = "providers")]
pub struct Model {
    #[sea_orm(primary_key, auto_increment = false, column_type = "Text")]
    pub id: String,
    #[sea_orm(column_type = "Text")]
    pub name: String,
    #[sea_orm(column_type = "Text")]
    pub provider_type: String,
    #[sea_orm(column_type = "Text")]
    pub base_url: Option<String>,
    #[sea_orm(column_type = "Text")]
    pub auth_type: Option<String>,
    #[sea_orm(column_type = "Text")]
    pub auth_value: Option<String>,
    #[sea_orm(column_type = "Text")]
    pub auth_header_name: Option<String>,
    #[sea_orm(column_type = "Text")]
    pub auth_query_name: Option<String>,
    #[sea_orm(column_type = "Text")]
    pub capabilities_json: Option<String>,
    #[sea_orm(column_type = "Text")]
    pub strategy_json: Option<String>,
    pub enabled: i32,
    pub priority: i32,
    pub weight: i32,
    #[sea_orm(column_type = "Text")]
    pub tag: Option<String>,
    #[sea_orm(column_type = "Text")]
    pub groups_json: String,
    pub balance: Option<f64>,
    #[sea_orm(column_type = "Text")]
    pub created_at: String,
    #[sea_orm(column_type = "Text")]
    pub updated_at: String,
}

#[derive(Copy, Clone, Debug, EnumIter, DeriveRelation)]
pub enum Relation {}

impl sea_orm::ActiveModelBehavior for ActiveModel {}
