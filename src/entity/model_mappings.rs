use sea_orm::entity::prelude::*;
use sea_orm::DeriveRelation;

#[derive(Clone, Debug, PartialEq, Eq, DeriveEntityModel)]
#[sea_orm(table_name = "model_mappings")]
pub struct Model {
    #[sea_orm(primary_key, auto_increment = false, column_type = "Text")]
    pub id: String,
    #[sea_orm(column_type = "Text")]
    pub provider_id: String,
    #[sea_orm(column_type = "Text")]
    pub logical_model: String,
    #[sea_orm(column_type = "Text")]
    pub upstream_model: String,
    #[sea_orm(column_type = "Text")]
    pub created_at: String,
}

#[derive(Copy, Clone, Debug, EnumIter, DeriveRelation)]
pub enum Relation {
    #[sea_orm(
        belongs_to = "super::providers::Entity",
        from = "Column::ProviderId",
        to = "super::providers::Column::Id",
        on_delete = "Cascade"
    )]
    Providers,
}

impl sea_orm::ActiveModelBehavior for ActiveModel {}
