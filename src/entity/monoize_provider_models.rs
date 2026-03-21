use sea_orm::DeriveRelation;
use sea_orm::entity::prelude::*;

#[derive(Clone, Debug, PartialEq, DeriveEntityModel)]
#[sea_orm(table_name = "monoize_provider_models")]
pub struct Model {
    #[sea_orm(primary_key, auto_increment = false, column_type = "Text")]
    pub id: String,
    #[sea_orm(column_type = "Text")]
    pub provider_id: String,
    #[sea_orm(column_type = "Text")]
    pub model_name: String,
    #[sea_orm(column_type = "Text")]
    pub redirect: Option<String>,
    pub multiplier: f64,
    #[sea_orm(column_type = "Text")]
    pub created_at: String,
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
