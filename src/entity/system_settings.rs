use sea_orm::entity::prelude::*;
use sea_orm::DeriveRelation;

#[derive(Clone, Debug, PartialEq, Eq, DeriveEntityModel)]
#[sea_orm(table_name = "system_settings")]
pub struct Model {
    #[sea_orm(primary_key, auto_increment = false, column_type = "Text")]
    pub key: String,
    #[sea_orm(column_type = "Text")]
    pub value: String,
    #[sea_orm(column_type = "Text")]
    pub updated_at: String,
}

#[derive(Copy, Clone, Debug, EnumIter, DeriveRelation)]
pub enum Relation {}

impl sea_orm::ActiveModelBehavior for ActiveModel {}
