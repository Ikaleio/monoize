use sea_orm::DeriveRelation;
use sea_orm::entity::prelude::*;

#[derive(Clone, Debug, PartialEq, Eq, DeriveEntityModel)]
#[sea_orm(table_name = "group_members")]
pub struct Model {
    #[sea_orm(primary_key, auto_increment = false, column_type = "Text")]
    pub id: String,
    #[sea_orm(column_type = "Text")]
    pub group_provider_id: String,
    #[sea_orm(column_type = "Text")]
    pub member_provider_id: String,
    pub weight: i32,
    pub priority: i32,
    #[sea_orm(column_type = "Text")]
    pub created_at: String,
}

#[derive(Copy, Clone, Debug, EnumIter, DeriveRelation)]
pub enum Relation {
    #[sea_orm(
        belongs_to = "super::providers::Entity",
        from = "Column::GroupProviderId",
        to = "super::providers::Column::Id",
        on_delete = "Cascade"
    )]
    GroupProvider,
    #[sea_orm(
        belongs_to = "super::providers::Entity",
        from = "Column::MemberProviderId",
        to = "super::providers::Column::Id",
        on_delete = "Cascade"
    )]
    MemberProvider,
}

impl sea_orm::ActiveModelBehavior for ActiveModel {}
