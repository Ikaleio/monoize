use sea_orm::entity::prelude::*;
use sea_orm::DeriveRelation;

#[derive(Clone, Debug, PartialEq, DeriveEntityModel)]
#[sea_orm(table_name = "users")]
pub struct Model {
    #[sea_orm(primary_key, auto_increment = false, column_type = "Text")]
    pub id: String,
    #[sea_orm(column_type = "Text")]
    pub username: String,
    #[sea_orm(column_type = "Text")]
    pub password_hash: String,
    #[sea_orm(column_type = "Text")]
    pub role: String,
    #[sea_orm(column_type = "Text")]
    pub created_at: String,
    #[sea_orm(column_type = "Text")]
    pub updated_at: String,
    #[sea_orm(column_type = "Text")]
    pub last_login_at: Option<String>,
    pub enabled: i32,
    #[sea_orm(column_type = "Text")]
    pub balance_nano_usd: String,
    pub balance_unlimited: i32,
    #[sea_orm(column_type = "Text")]
    pub email: Option<String>,
}

#[derive(Copy, Clone, Debug, EnumIter, DeriveRelation)]
pub enum Relation {}

impl sea_orm::ActiveModelBehavior for ActiveModel {}
