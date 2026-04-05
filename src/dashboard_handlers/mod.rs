mod analytics_request_logs;
mod api_keys;
mod auth;
mod groups;
mod model_registry;
mod providers;
mod session_helpers;
mod settings;
mod users;

#[cfg(test)]
mod tests;

pub use analytics_request_logs::{
    AnalyticsQuery, RequestLogsQuery, get_dashboard_analytics, list_my_request_logs,
    stream_request_logs,
};
pub use api_keys::{
    ApiKeyCreatedResponse, ApiKeyResponse, BatchDeleteApiKeysRequest, CreateApiKeyRequest,
    TransferToSubAccountRequest, UpdateApiKeyRequest, batch_delete_api_keys, create_api_key,
    delete_api_key, get_api_key, get_apikey_presets, list_my_api_keys,
    transfer_to_sub_account, update_api_key,
};
pub use auth::{
    AuthResponse, LoginRequest, RegisterRequest, UpdateMeRequest, UserResponse, get_me, login,
    logout, register, update_me,
};
pub use groups::{DashboardGroupsResponse, list_dashboard_groups};
pub use model_registry::{
    create_model, delete_model, delete_model_metadata, get_model, get_model_metadata,
    list_marketplace_models, list_model_metadata, list_models, sync_model_metadata_models_dev,
    update_model, upsert_model_metadata,
};
pub use providers::{
    FetchChannelModelsRequest, TestChannelRequest, create_provider, delete_provider,
    fetch_channel_models, fetch_provider_models, get_provider, get_provider_presets,
    get_transform_registry, list_providers, reorder_providers, test_channel, update_provider,
};
pub use settings::{
    UpdateSettingsRequest, get_config_overview, get_dashboard_stats, get_public_settings,
    get_settings, update_settings,
};
pub use users::{
    CreateUserRequest, UpdateUserRequest, create_user, delete_user, get_user, list_users,
    update_user,
};
