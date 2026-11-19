use super::types::*;
use super::validation::matching_api_type_override;
use crate::urp::encode::openai_chat::ChatDialect;
use std::time::Duration;

pub fn apply_provider_api_key(
    request: reqwest::RequestBuilder,
    provider_type: MonoizeProviderType,
    api_key: &str,
) -> reqwest::RequestBuilder {
    match provider_type {
        MonoizeProviderType::Gemini => request.header("x-goog-api-key", api_key),
        MonoizeProviderType::Messages => request.header("x-api-key", api_key).bearer_auth(api_key),
        _ => request.bearer_auth(api_key),
    }
}

pub async fn probe_channel_list_models(
    client: &reqwest::Client,
    channel: &MonoizeChannel,
    timeout_ms: u64,
) -> bool {
    let base = channel.base_url.trim_end_matches('/');
    let url = if channel.provider_type == MonoizeProviderType::OpenrouterImage {
        crate::upstream::join_url(base, "/v1/images/models")
    } else {
        format!("{base}/v1/models")
    };

    let mut request = client.get(url).timeout(Duration::from_millis(timeout_ms));
    request = apply_provider_api_key(request, channel.provider_type, &channel.api_key);
    if channel.provider_type == MonoizeProviderType::Messages {
        request = request.header("anthropic-version", "2023-06-01");
    }
    let result = request.send().await;

    match result {
        Ok(resp) => resp.status().is_success(),
        Err(_) => false,
    }
}

pub(crate) fn select_channel_probe_model<'a>(
    provider: &MonoizeProvider,
    channel: &'a MonoizeChannel,
    global_probe_model: Option<&str>,
) -> Option<&'a str> {
    channel
        .active_probe_model_override
        .as_deref()
        .or(provider.active_probe_model_override.as_deref())
        .or(global_probe_model)
        .and_then(|model| channel.models.get_key_value(model))
        .map(|(key, _)| key.as_str())
        .or_else(|| channel.models.keys().min().map(String::as_str))
}

#[derive(Clone, Copy)]
pub(crate) struct ChannelProbe<'a> {
    pub logical_model: &'a str,
    pub upstream_model: &'a str,
    pub provider_type: MonoizeProviderType,
    pub multiplier: crate::exact_decimal::Multiplier,
    pub chat_dialect: ChatDialect,
}

impl<'a> ChannelProbe<'a> {
    pub fn is_completion(self) -> bool {
        matches!(
            self.provider_type,
            MonoizeProviderType::Responses
                | MonoizeProviderType::ChatCompletion
                | MonoizeProviderType::Messages
                | MonoizeProviderType::Gemini
        )
    }

    pub fn for_model(
        provider: &MonoizeProvider,
        channel: &'a MonoizeChannel,
        logical_model: &'a str,
    ) -> Option<Self> {
        let entry = channel.models.get(logical_model)?;
        let selected_override =
            matching_api_type_override(&provider.api_type_overrides, logical_model);
        Some(Self {
            logical_model,
            upstream_model: entry
                .redirect
                .as_deref()
                .map(str::trim)
                .filter(|model| !model.is_empty())
                .unwrap_or(logical_model),
            provider_type: selected_override.map_or(channel.provider_type, |entry| entry.api_type),
            multiplier: entry.multiplier,
            chat_dialect: selected_override
                .map_or(channel.chat_dialect, |entry| entry.chat_dialect)
                .unwrap_or(ChatDialect::Generic),
        })
    }
}
