use crate::db_cache::ApiKeyCache;
use crate::entity::{api_keys, monoize_channels, monoize_providers, users};
use crate::users::{
    InsertRequestLog, RequestLogApiKey, RequestLogBilling, RequestLogChannel, RequestLogError,
    RequestLogProvider, RequestLogRow, RequestLogTiming, RequestLogTokens, RequestLogUser,
};
use dashmap::DashMap;
use sea_orm::{DatabaseConnection, EntityTrait};

#[derive(Clone, Debug, Default)]
pub struct NameCaches {
    pub providers: DashMap<String, String>,
    pub channels: DashMap<String, String>,
    pub users: DashMap<String, String>,
    pub api_keys: DashMap<String, String>,
}

impl NameCaches {
    pub fn new() -> Self {
        Self {
            providers: DashMap::new(),
            channels: DashMap::new(),
            users: DashMap::new(),
            api_keys: DashMap::new(),
        }
    }

    pub async fn init(db: &DatabaseConnection) -> Result<Self, String> {
        let providers = monoize_providers::Entity::find()
            .all(db)
            .await
            .map_err(|e| e.to_string())?;
        let channels = monoize_channels::Entity::find()
            .all(db)
            .await
            .map_err(|e| e.to_string())?;
        let users = users::Entity::find()
            .all(db)
            .await
            .map_err(|e| e.to_string())?;
        let api_keys = api_keys::Entity::find()
            .all(db)
            .await
            .map_err(|e| e.to_string())?;

        let caches = Self::new();
        for provider in providers {
            caches.providers.insert(provider.id, provider.name);
        }
        for channel in channels {
            caches.channels.insert(channel.id, channel.name);
        }
        for user in users {
            caches.users.insert(user.id, user.username);
        }
        for api_key in api_keys {
            caches.api_keys.insert(api_key.id, api_key.name);
        }

        Ok(caches)
    }

    pub fn get_provider_name(&self, id: &str) -> Option<String> {
        self.providers.get(id).map(|v| v.value().clone())
    }

    pub fn get_channel_name(&self, id: &str) -> Option<String> {
        self.channels.get(id).map(|v| v.value().clone())
    }

    pub fn get_username(&self, id: &str) -> Option<String> {
        self.users.get(id).map(|v| v.value().clone())
    }

    pub fn get_api_key_name(&self, id: &str) -> Option<String> {
        self.api_keys.get(id).map(|v| v.value().clone())
    }

    pub fn enrich_log(&self, raw: &InsertRequestLog, api_key_cache: &ApiKeyCache) -> RequestLogRow {
        let _ = api_key_cache;
        RequestLogRow {
            id: raw.request_id.clone().unwrap_or_default(),
            request_id: raw.request_id.clone(),
            created_at: raw.created_at.to_rfc3339(),
            status: raw.status.clone(),
            is_stream: raw.is_stream,
            model: raw.model.clone(),
            upstream_model: raw.upstream_model.clone(),
            request_kind: raw.request_kind.clone(),
            reasoning_effort: raw.reasoning_effort.clone(),
            request_ip: raw.request_ip.clone(),
            tried_providers: raw.tried_providers_json.clone(),
            provider: RequestLogProvider {
                id: raw.provider_id.clone(),
                name: raw
                    .provider_id
                    .as_ref()
                    .and_then(|provider_id| self.get_provider_name(provider_id)),
                multiplier: raw.provider_multiplier,
            },
            channel: RequestLogChannel {
                id: raw.channel_id.clone(),
                name: raw
                    .channel_id
                    .as_ref()
                    .and_then(|channel_id| self.get_channel_name(channel_id)),
            },
            user: RequestLogUser {
                id: raw.user_id.clone(),
                username: self.get_username(&raw.user_id),
            },
            api_key: RequestLogApiKey {
                id: raw.api_key_id.clone(),
                name: raw
                    .api_key_id
                    .as_ref()
                    .and_then(|api_key_id| self.get_api_key_name(api_key_id)),
            },
            tokens: RequestLogTokens {
                input: raw.input_tokens.and_then(|v| i64::try_from(v).ok()),
                output: raw.output_tokens.and_then(|v| i64::try_from(v).ok()),
                cache_read: raw.cache_read_tokens.and_then(|v| i64::try_from(v).ok()),
                cache_creation: raw
                    .cache_creation_tokens
                    .and_then(|v| i64::try_from(v).ok()),
                tool_prompt: raw.tool_prompt_tokens.and_then(|v| i64::try_from(v).ok()),
                reasoning: raw.reasoning_tokens.and_then(|v| i64::try_from(v).ok()),
                accepted_prediction: raw
                    .accepted_prediction_tokens
                    .and_then(|v| i64::try_from(v).ok()),
                rejected_prediction: raw
                    .rejected_prediction_tokens
                    .and_then(|v| i64::try_from(v).ok()),
            },
            timing: RequestLogTiming {
                duration_ms: raw.duration_ms.and_then(|v| i64::try_from(v).ok()),
                ttfb_ms: raw.ttfb_ms.and_then(|v| i64::try_from(v).ok()),
                duration_ms_alias: raw.duration_ms.and_then(|v| i64::try_from(v).ok()),
                elapsed_ms: raw.duration_ms.and_then(|v| i64::try_from(v).ok()),
                latency_ms: raw.duration_ms.and_then(|v| i64::try_from(v).ok()),
                ttfb_ms_alias: raw.ttfb_ms.and_then(|v| i64::try_from(v).ok()),
                first_token_ms: raw.ttfb_ms.and_then(|v| i64::try_from(v).ok()),
                first_token_ms_alias: raw.ttfb_ms.and_then(|v| i64::try_from(v).ok()),
            },
            billing: RequestLogBilling {
                charge_nano_usd: raw.charge_nano_usd.map(|v| v.to_string()),
                breakdown: raw.billing_breakdown_json.clone(),
            },
            usage: raw.usage_breakdown_json.clone(),
            error: RequestLogError {
                code: raw.error_code.clone(),
                message: raw.error_message.clone(),
                http_status: raw.error_http_status.map(i64::from),
            },
        }
    }
}
