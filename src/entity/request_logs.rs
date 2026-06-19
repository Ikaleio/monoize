use sea_orm::DeriveRelation;
use sea_orm::entity::prelude::*;

#[derive(Clone, Debug, PartialEq, DeriveEntityModel)]
#[sea_orm(table_name = "request_logs")]
pub struct Model {
    #[sea_orm(primary_key, auto_increment = false, column_type = "Text")]
    pub id: String,
    #[sea_orm(column_type = "Text")]
    pub request_id: Option<String>,
    #[sea_orm(column_type = "Text")]
    pub user_id: String,
    #[sea_orm(column_type = "Text")]
    pub api_key_id: Option<String>,
    #[sea_orm(column_type = "Text")]
    pub model: String,
    #[sea_orm(column_type = "Text")]
    pub provider_id: Option<String>,
    #[sea_orm(column_type = "Text")]
    pub upstream_model: Option<String>,
    #[sea_orm(column_type = "Text")]
    pub channel_id: Option<String>,
    pub is_stream: i32,
    pub input_tokens: Option<i64>,
    pub output_tokens: Option<i64>,
    pub cache_read_tokens: Option<i64>,
    pub cache_creation_tokens: Option<i64>,
    pub tool_prompt_tokens: Option<i64>,
    pub reasoning_tokens: Option<i64>,
    pub accepted_prediction_tokens: Option<i64>,
    pub rejected_prediction_tokens: Option<i64>,
    pub provider_multiplier: Option<f64>,
    #[sea_orm(column_type = "Text")]
    pub charge_nano_usd: Option<String>,
    #[sea_orm(column_type = "Text")]
    pub status: String,
    #[sea_orm(column_type = "Text")]
    pub usage_breakdown_json: Option<String>,
    #[sea_orm(column_type = "Text")]
    pub billing_breakdown_json: Option<String>,
    #[sea_orm(column_type = "Text")]
    pub error_code: Option<String>,
    #[sea_orm(column_type = "Text")]
    pub error_message: Option<String>,
    pub error_http_status: Option<i64>,
    pub duration_ms: Option<i64>,
    pub ttfb_ms: Option<i64>,
    pub first_visible_output_ms: Option<i64>,
    pub last_visible_output_ms: Option<i64>,
    pub visible_generation_ms: Option<i64>,
    pub visible_output_tokens: Option<i64>,
    #[sea_orm(column_type = "Text")]
    pub tps_mode: Option<String>,
    #[sea_orm(column_type = "Text")]
    pub request_ip: Option<String>,
    #[sea_orm(column_type = "Text")]
    pub reasoning_effort: Option<String>,
    #[sea_orm(column_type = "Text")]
    pub tried_providers_json: Option<String>,
    #[sea_orm(column_type = "Text")]
    pub request_kind: Option<String>,
    #[sea_orm(column_type = "Text")]
    pub effective_provider_type: Option<String>,
    pub affinity_hit: Option<i32>,
    #[sea_orm(column_type = "Text")]
    pub affinity_key_hash: Option<String>,
    #[sea_orm(column_type = "Text")]
    pub affinity_target: Option<String>,
    #[sea_orm(column_type = "Text")]
    pub created_at: String,
    pub created_at_unix_ms: Option<i64>,
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
