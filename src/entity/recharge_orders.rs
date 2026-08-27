use sea_orm::DeriveRelation;
use sea_orm::entity::prelude::*;

#[derive(Clone, Debug, PartialEq, Eq, DeriveEntityModel)]
#[sea_orm(table_name = "recharge_orders")]
pub struct Model {
    #[sea_orm(primary_key, auto_increment = false, column_type = "Text")]
    pub id: String,
    #[sea_orm(column_type = "Text")]
    pub user_id: String,
    #[sea_orm(column_type = "Text")]
    pub payment_channel_id: String,
    #[sea_orm(column_type = "Text")]
    pub channel_type_id: String,
    #[sea_orm(column_type = "Text")]
    pub channel_name: String,
    #[sea_orm(column_type = "Text")]
    pub status: String,
    #[sea_orm(column_type = "Text")]
    pub credit_nano_usd: String,
    #[sea_orm(column_type = "Text")]
    pub pay_currency: String,
    #[sea_orm(column_type = "Text")]
    pub pay_amount: String,
    #[sea_orm(column_type = "Text")]
    pub usd_rate: String,
    #[sea_orm(column_type = "Text")]
    pub provider_order_id: Option<String>,
    #[sea_orm(column_type = "Text")]
    pub error_code: Option<String>,
    #[sea_orm(column_type = "Text")]
    pub paid_at: Option<String>,
    #[sea_orm(column_type = "Text")]
    pub expires_at: String,
    #[sea_orm(column_type = "Text")]
    pub meta_json: String,
    #[sea_orm(column_type = "Text")]
    pub created_at: String,
    #[sea_orm(column_type = "Text")]
    pub updated_at: String,
}

#[derive(Copy, Clone, Debug, EnumIter, DeriveRelation)]
pub enum Relation {}

impl sea_orm::ActiveModelBehavior for ActiveModel {}
