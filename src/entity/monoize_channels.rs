use sea_orm::DeriveRelation;
use sea_orm::entity::prelude::*;

#[derive(Clone, Debug, PartialEq, DeriveEntityModel)]
#[sea_orm(table_name = "monoize_channels")]
pub struct Model {
    #[sea_orm(primary_key, auto_increment = false, column_type = "Text")]
    pub id: String,
    #[sea_orm(column_type = "Text")]
    pub provider_id: String,
    #[sea_orm(column_type = "Text")]
    pub name: String,
    #[sea_orm(column_type = "Text")]
    pub base_url: String,
    #[sea_orm(column_type = "Text")]
    pub api_key: String,
    pub weight: i32,
    pub enabled: i32,
    pub passive_failure_count_threshold_override: Option<i64>,
    pub passive_cooldown_seconds_override: Option<i64>,
    pub passive_window_seconds_override: Option<i64>,
    pub passive_rate_limit_cooldown_seconds_override: Option<i64>,
    pub request_timeout_ms_override: Option<i64>,
    #[sea_orm(column_type = "Text")]
    pub groups: String,
    #[sea_orm(column_type = "Text")]
    pub created_at: String,
    #[sea_orm(column_type = "Text")]
    pub updated_at: String,
}

#[derive(Copy, Clone, Debug, EnumIter, DeriveRelation)]
pub enum Relation {
    #[sea_orm(
        belongs_to = "super::monoize_providers::Entity",
        from = "Column::ProviderId",
        to = "super::monoize_providers::Column::Id",
        on_delete = "Cascade"
    )]
    MonoizeProviders,
}

impl sea_orm::ActiveModelBehavior for ActiveModel {}
