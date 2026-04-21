use sea_orm::DeriveRelation;
use sea_orm::entity::prelude::*;

#[derive(Clone, Debug, PartialEq, Eq, DeriveEntityModel)]
#[sea_orm(table_name = "monoize_providers")]
pub struct Model {
    #[sea_orm(primary_key, auto_increment = false, column_type = "Text")]
    pub id: String,
    #[sea_orm(column_type = "Text")]
    pub name: String,
    #[sea_orm(column_type = "Text")]
    pub provider_type: String,
    pub max_retries: i32,
    pub channel_max_retries: i32,
    pub channel_retry_interval_ms: i32,
    pub circuit_breaker_enabled: i32,
    pub per_model_circuit_break: i32,
    #[sea_orm(column_type = "Text")]
    pub transforms: String,
    #[sea_orm(column_type = "Text")]
    pub api_type_overrides: String,
    pub active_probe_enabled_override: Option<i32>,
    pub active_probe_interval_seconds_override: Option<i64>,
    pub active_probe_success_threshold_override: Option<i64>,
    #[sea_orm(column_type = "Text")]
    pub active_probe_model_override: Option<String>,
    pub request_timeout_ms_override: Option<i64>,
    #[sea_orm(column_type = "Text")]
    pub extra_fields_whitelist: Option<String>,
    pub strip_cross_protocol_nested_extra: Option<i32>,
    #[sea_orm(column_type = "Text")]
    pub groups: String,
    pub enabled: i32,
    pub priority: i32,
    #[sea_orm(column_type = "Text")]
    pub created_at: String,
    #[sea_orm(column_type = "Text")]
    pub updated_at: String,
}

#[derive(Copy, Clone, Debug, EnumIter, DeriveRelation)]
pub enum Relation {}

impl sea_orm::ActiveModelBehavior for ActiveModel {}
