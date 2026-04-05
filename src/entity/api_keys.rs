use sea_orm::entity::prelude::*;
use sea_orm::DeriveRelation;

#[derive(Clone, Debug, PartialEq, DeriveEntityModel)]
#[sea_orm(table_name = "api_keys")]
pub struct Model {
    #[sea_orm(primary_key, auto_increment = false, column_type = "Text")]
    pub id: String,
    #[sea_orm(column_type = "Text")]
    pub user_id: String,
    #[sea_orm(column_type = "Text")]
    pub name: String,
    #[sea_orm(column_type = "Text")]
    pub key_prefix: String,
    #[sea_orm(column_type = "Text")]
    pub key: String,
    #[sea_orm(column_type = "Text")]
    pub key_hash: String,
    #[sea_orm(column_type = "Text")]
    pub created_at: String,
    #[sea_orm(column_type = "Text")]
    pub expires_at: Option<String>,
    #[sea_orm(column_type = "Text")]
    pub last_used_at: Option<String>,
    pub enabled: i32,
    pub sub_account_enabled: i32,
    #[sea_orm(column_type = "Text")]
    pub sub_account_balance_nano: String,
    pub model_limits_enabled: i32,
    #[sea_orm(column_type = "Text")]
    pub model_limits: String,
    #[sea_orm(column_type = "Text")]
    pub ip_whitelist: String,
    #[sea_orm(column_type = "Text")]
    pub allowed_groups: String,
    #[sea_orm(column_type = "Text")]
    pub token_group: String,
    pub max_multiplier: Option<f64>,
    #[sea_orm(column_type = "Text")]
    pub transforms: String,
    #[sea_orm(column_type = "Text")]
    pub model_redirects: String,
}

#[derive(Copy, Clone, Debug, EnumIter, DeriveRelation)]
pub enum Relation {
    #[sea_orm(
        belongs_to = "super::users::Entity",
        from = "Column::UserId",
        to = "super::users::Column::Id",
        on_delete = "Cascade"
    )]
    Users,
}

impl sea_orm::ActiveModelBehavior for ActiveModel {}
