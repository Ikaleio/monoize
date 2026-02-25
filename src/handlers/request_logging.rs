use super::*;

pub(super) async fn insert_pending_request_log(
    state: &AppState,
    auth: &crate::auth::AuthResult,
    model: &str,
    is_stream: bool,
    request_id: Option<&str>,
    request_ip: Option<&str>,
) {
    let Some(user_id) = auth.user_id.as_deref() else {
        return;
    };
    let Some(request_id) = request_id.map(str::trim).filter(|v| !v.is_empty()) else {
        return;
    };

    if let Err(e) = state
        .user_store
        .insert_request_log_pending(
            request_id,
            user_id,
            auth.api_key_id.as_deref(),
            model,
            is_stream,
            request_ip,
        )
        .await
    {
        tracing::warn!("failed to insert pending request log: {e}");
    }
}

pub(super) async fn update_pending_channel_info(
    state: &AppState,
    auth: &crate::auth::AuthResult,
    attempt: &MonoizeAttempt,
    request_id: Option<&str>,
) {
    let Some(user_id) = auth.user_id.as_deref() else {
        return;
    };
    let Some(request_id) = request_id.map(str::trim).filter(|v| !v.is_empty()) else {
        return;
    };

    if let Err(e) = state
        .user_store
        .update_pending_request_log_channel(
            user_id,
            request_id,
            &attempt.provider_id,
            &attempt.channel_id,
            &attempt.upstream_model,
            attempt.model_multiplier,
        )
        .await
    {
        tracing::warn!("failed to update pending request log channel info: {e}");
    }
}

#[allow(clippy::too_many_arguments)]
pub(super) fn spawn_request_log(
    state: &AppState,
    auth: &crate::auth::AuthResult,
    attempt: &MonoizeAttempt,
    model: &str,
    usage: Option<urp::Usage>,
    charge_nano_usd: Option<i128>,
    billing_breakdown_json: Option<Value>,
    is_stream: bool,
    started_at: std::time::Instant,
    request_id: Option<String>,
    request_ip: Option<String>,
    channel_id: String,
    ttfb_ms: Option<u64>,
    reasoning_effort: Option<String>,
    tried_providers: Vec<TriedProvider>,
) {
    let Some(user_id) = auth.user_id.clone() else {
        return;
    };
    let api_key_id = auth.api_key_id.clone();
    let quota_unlimited = auth.quota_unlimited;
    let api_key_id_for_quota = api_key_id.clone();
    let provider_id = attempt.provider_id.clone();
    let upstream_model = attempt.upstream_model.clone();
    let model_multiplier = attempt.model_multiplier;
    let model = model.to_string();
    let duration_ms = started_at.elapsed().as_millis() as u64;
    let user_store = state.user_store.clone();
    let usage_breakdown_json = usage.as_ref().map(build_usage_breakdown);
    let tried_providers_json = if tried_providers.is_empty() {
        None
    } else {
        serde_json::to_value(&tried_providers).ok()
    };

    tokio::spawn(async move {
        let log = InsertRequestLog {
            request_id,
            user_id,
            api_key_id,
            model,
            provider_id: Some(provider_id),
            upstream_model: Some(upstream_model),
            channel_id: Some(channel_id),
            is_stream,
            input_tokens: usage.as_ref().map(|u| u.input_tokens),
            output_tokens: usage.as_ref().map(|u| u.output_tokens),
            cache_read_tokens: usage.as_ref().and_then(|u| u.cached_tokens()),
            cache_creation_tokens: usage
                .as_ref()
                .and_then(|u| u.input_details.as_ref().map(|d| d.cache_creation_tokens))
                .filter(|&v| v > 0),
            tool_prompt_tokens: usage
                .as_ref()
                .and_then(|u| u.input_details.as_ref().map(|d| d.tool_prompt_tokens))
                .filter(|&v| v > 0),
            reasoning_tokens: usage.as_ref().and_then(|u| u.reasoning_tokens()),
            accepted_prediction_tokens: usage
                .as_ref()
                .and_then(|u| {
                    u.output_details
                        .as_ref()
                        .map(|d| d.accepted_prediction_tokens)
                })
                .filter(|&v| v > 0),
            rejected_prediction_tokens: usage
                .as_ref()
                .and_then(|u| {
                    u.output_details
                        .as_ref()
                        .map(|d| d.rejected_prediction_tokens)
                })
                .filter(|&v| v > 0),
            provider_multiplier: Some(model_multiplier),
            charge_nano_usd,
            status: REQUEST_LOG_STATUS_SUCCESS.to_string(),
            usage_breakdown_json,
            billing_breakdown_json,
            error_code: None,
            error_message: None,
            error_http_status: None,
            duration_ms: Some(duration_ms),
            ttfb_ms,
            request_ip,
            reasoning_effort,
            tried_providers_json,
            request_kind: None,
        };
        if let Err(e) = user_store.finalize_request_log(log).await {
            tracing::warn!("failed to finalize request log: {e}");
        }
        if !quota_unlimited {
            if let Some(key_id) = api_key_id_for_quota.as_deref() {
                if let Err(e) = user_store.decrement_api_key_quota(key_id).await {
                    tracing::warn!("failed to decrement api key quota: {e}");
                }
            }
        }
    });
}

#[allow(clippy::too_many_arguments)]
pub(super) fn spawn_request_log_error(
    state: &AppState,
    auth: &crate::auth::AuthResult,
    attempt: &MonoizeAttempt,
    model: &str,
    is_stream: bool,
    started_at: std::time::Instant,
    request_id: Option<String>,
    request_ip: Option<String>,
    error: &AppError,
    reasoning_effort: Option<String>,
    tried_providers: Vec<TriedProvider>,
) {
    let Some(user_id) = auth.user_id.clone() else {
        return;
    };
    let api_key_id = auth.api_key_id.clone();
    let model = model.to_string();
    let provider_id = attempt.provider_id.clone();
    let upstream_model = attempt.upstream_model.clone();
    let model_multiplier = attempt.model_multiplier;
    let channel_id = attempt.channel_id.clone();
    let duration_ms = started_at.elapsed().as_millis() as u64;
    let user_store = state.user_store.clone();
    let error_code = Some(error.code.clone());
    let error_message = Some(error.internal_message.clone().unwrap_or_else(|| error.message.clone()));
    let error_http_status = Some(error.status.as_u16());
    let tried_providers_json = if tried_providers.is_empty() {
        None
    } else {
        serde_json::to_value(&tried_providers).ok()
    };

    tokio::spawn(async move {
        let log = InsertRequestLog {
            request_id,
            user_id,
            api_key_id,
            model,
            provider_id: Some(provider_id),
            upstream_model: Some(upstream_model),
            channel_id: Some(channel_id),
            is_stream,
            input_tokens: None,
            output_tokens: None,
            cache_read_tokens: None,
            cache_creation_tokens: None,
            tool_prompt_tokens: None,
            reasoning_tokens: None,
            accepted_prediction_tokens: None,
            rejected_prediction_tokens: None,
            provider_multiplier: Some(model_multiplier),
            charge_nano_usd: None,
            status: REQUEST_LOG_STATUS_ERROR.to_string(),
            usage_breakdown_json: None,
            billing_breakdown_json: None,
            error_code,
            error_message,
            error_http_status,
            duration_ms: Some(duration_ms),
            ttfb_ms: None,
            request_ip,
            reasoning_effort,
            tried_providers_json,
            request_kind: None,
        };
        if let Err(e) = user_store.finalize_request_log(log).await {
            tracing::warn!("failed to finalize request log: {e}");
        }
    });
}

#[allow(clippy::too_many_arguments)]
pub(super) fn spawn_request_log_error_no_attempt(
    state: &AppState,
    auth: &crate::auth::AuthResult,
    model: &str,
    is_stream: bool,
    started_at: std::time::Instant,
    request_id: Option<String>,
    request_ip: Option<String>,
    error: &AppError,
    reasoning_effort: Option<String>,
    tried_providers: Vec<TriedProvider>,
) {
    let Some(user_id) = auth.user_id.clone() else {
        return;
    };
    let api_key_id = auth.api_key_id.clone();
    let model = model.to_string();
    let duration_ms = started_at.elapsed().as_millis() as u64;
    let user_store = state.user_store.clone();
    let error_code = Some(error.code.clone());
    let error_message = Some(error.internal_message.clone().unwrap_or_else(|| error.message.clone()));
    let error_http_status = Some(error.status.as_u16());
    let tried_providers_json = if tried_providers.is_empty() {
        None
    } else {
        serde_json::to_value(&tried_providers).ok()
    };

    tokio::spawn(async move {
        let log = InsertRequestLog {
            request_id,
            user_id,
            api_key_id,
            model,
            provider_id: None,
            upstream_model: None,
            channel_id: None,
            is_stream,
            input_tokens: None,
            output_tokens: None,
            cache_read_tokens: None,
            cache_creation_tokens: None,
            tool_prompt_tokens: None,
            reasoning_tokens: None,
            accepted_prediction_tokens: None,
            rejected_prediction_tokens: None,
            provider_multiplier: None,
            charge_nano_usd: None,
            status: REQUEST_LOG_STATUS_ERROR.to_string(),
            usage_breakdown_json: None,
            billing_breakdown_json: None,
            error_code,
            error_message,
            error_http_status,
            duration_ms: Some(duration_ms),
            ttfb_ms: None,
            request_ip,
            reasoning_effort,
            tried_providers_json,
            request_kind: None,
        };
        if let Err(e) = user_store.finalize_request_log(log).await {
            tracing::warn!("failed to finalize request log: {e}");
        }
    });
}
