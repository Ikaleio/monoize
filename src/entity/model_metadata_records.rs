use sea_orm::DeriveRelation;
use sea_orm::entity::prelude::*;

#[derive(Clone, Debug, PartialEq, Eq, DeriveEntityModel)]
#[sea_orm(table_name = "model_metadata_records")]
pub struct Model {
    #[sea_orm(primary_key, auto_increment = false, column_type = "Text")]
    pub model_id: String,
    #[sea_orm(column_type = "Text")]
    pub models_dev_provider: Option<String>,
    #[sea_orm(column_type = "Text")]
    pub mode: Option<String>,
    #[sea_orm(column_type = "Text")]
    pub input_cost_per_token_nano: Option<String>,
    #[sea_orm(column_type = "Text")]
    pub output_cost_per_token_nano: Option<String>,
    #[sea_orm(column_type = "Text")]
    pub cache_read_input_cost_per_token_nano: Option<String>,
    #[sea_orm(column_type = "Text")]
    pub cache_creation_input_cost_per_token_nano: Option<String>,
    #[sea_orm(column_type = "Text")]
    pub output_cost_per_reasoning_token_nano: Option<String>,
    pub max_input_tokens: Option<i64>,
    pub max_output_tokens: Option<i64>,
    pub max_tokens: Option<i64>,
    #[sea_orm(column_type = "Text")]
    pub raw_json: String,
    #[sea_orm(column_type = "Text")]
    pub source: String,
    #[sea_orm(column_type = "Text")]
    pub updated_at: String,
}

#[derive(Copy, Clone, Debug, EnumIter, DeriveRelation)]
pub enum Relation {}

impl sea_orm::ActiveModelBehavior for ActiveModel {}
