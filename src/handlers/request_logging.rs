use super::*;
use chrono::{Duration as ChronoDuration, Utc};

fn request_created_at(started_at: std::time::Instant) -> chrono::DateTime<Utc> {
    let elapsed = ChronoDuration::from_std(started_at.elapsed()).unwrap_or(ChronoDuration::MAX);
    Utc::now() - elapsed
}

#[allow(clippy::too_many_arguments)]
fn broadcast_pending_snapshot(
    state: &AppState,
    auth: &crate::auth::AuthResult,
    request_id: &str,
    model: &str,
    is_stream: bool,
    request_ip: Option<&str>,
    provider_id: Option<&str>,
    channel_id: Option<&str>,
    upstream_model: Option<&str>,
    provider_multiplier: Option<f64>,
    created_at: chrono::DateTime<Utc>,
) {
    let Some(user_id) = auth.user_id.as_deref() else {
        return;
    };

    let pending_log = InsertRequestLog {
        request_id: Some(request_id.to_string()),
        user_id: user_id.to_string(),
        api_key_id: auth.api_key_id.clone(),
        model: model.to_string(),
        provider_id: provider_id.map(ToOwned::to_owned),
        upstream_model: upstream_model.map(ToOwned::to_owned),
        channel_id: channel_id.map(ToOwned::to_owned),
        is_stream,
        input_tokens: None,
        output_tokens: None,
        cache_read_tokens: None,
        cache_creation_tokens: None,
        tool_prompt_tokens: None,
        reasoning_tokens: None,
        accepted_prediction_tokens: None,
        rejected_prediction_tokens: None,
        provider_multiplier,
        charge_nano_usd: None,
        status: crate::users::REQUEST_LOG_STATUS_PENDING.to_string(),
        usage_breakdown_json: None,
        billing_breakdown_json: None,
        error_code: None,
        error_message: None,
        error_http_status: None,
        duration_ms: None,
        ttfb_ms: None,
        request_ip: request_ip.map(ToOwned::to_owned),
        reasoning_effort: None,
        tried_providers_json: None,
        request_kind: None,
        created_at,
    };

    let _ = state.log_broadcast.send(vec![pending_log]);
}

pub(super) async fn insert_pending_request_log(
    state: &AppState,
    auth: &crate::auth::AuthResult,
    model: &str,
    is_stream: bool,
    request_id: Option<&str>,
    request_ip: Option<&str>,
    started_at: std::time::Instant,
) {
    let Some(_user_id) = auth.user_id.as_deref() else {
        return;
    };
    let Some(request_id) = request_id.map(str::trim).filter(|v| !v.is_empty()) else {
        return;
    };

    broadcast_pending_snapshot(
        state,
        auth,
        request_id,
        model,
        is_stream,
        request_ip,
        None,
        None,
        None,
        None,
        request_created_at(started_at),
    );
}

#[allow(clippy::too_many_arguments)]
pub(super) async fn update_pending_channel_info(
    state: &AppState,
    auth: &crate::auth::AuthResult,
    attempt: &MonoizeAttempt,
    model: &str,
    is_stream: bool,
    request_id: Option<&str>,
    request_ip: Option<&str>,
    started_at: std::time::Instant,
) {
    let Some(_user_id) = auth.user_id.as_deref() else {
        return;
    };
    let Some(request_id) = request_id.map(str::trim).filter(|v| !v.is_empty()) else {
        return;
    };

    broadcast_pending_snapshot(
        state,
        auth,
        request_id,
        model,
        is_stream,
        request_ip,
        Some(&attempt.provider_id),
        Some(&attempt.channel_id),
        Some(&attempt.upstream_model),
        Some(attempt.model_multiplier),
        request_created_at(started_at),
    );
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
    stream_terminal_diagnostics: Option<StreamTerminalDiagnostics>,
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
    let created_at = request_created_at(started_at);
    let user_store = state.user_store.clone();
    let usage_breakdown_json = usage.as_ref().map(build_usage_breakdown);
    let tried_providers_json = if tried_providers.is_empty() {
        None
    } else {
        serde_json::to_value(&tried_providers).ok()
    };

    tokio::spawn(async move {
        if is_stream && usage.is_none() {
            tracing::warn!(
                request_id = request_id.as_deref().unwrap_or(""),
                provider_id = %provider_id,
                channel_id = %channel_id,
                model = %model,
                upstream_model = %upstream_model,
                stream_saw_done_sentinel = stream_terminal_diagnostics
                    .as_ref()
                    .map(|diagnostics| diagnostics.saw_done_sentinel),
                stream_terminal_event = stream_terminal_diagnostics
                    .as_ref()
                    .and_then(|diagnostics| diagnostics.terminal_event.as_deref()),
                stream_terminal_finish_reason = stream_terminal_diagnostics
                    .as_ref()
                    .and_then(|diagnostics| diagnostics.terminal_finish_reason.as_deref()),
                stream_synthetic_terminal_emitted = stream_terminal_diagnostics
                    .as_ref()
                    .map(|diagnostics| diagnostics.synthetic_terminal_emitted),
                "stream request completed without usage snapshot"
            );
        }
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
            created_at,
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
    let created_at = request_created_at(started_at);
    let user_store = state.user_store.clone();
    let error_code = Some(error.code.clone());
    let error_message = Some(
        error
            .internal_message
            .clone()
            .unwrap_or_else(|| error.message.clone()),
    );
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
            created_at,
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
    let created_at = request_created_at(started_at);
    let user_store = state.user_store.clone();
    let error_code = Some(error.code.clone());
    let error_message = Some(
        error
            .internal_message
            .clone()
            .unwrap_or_else(|| error.message.clone()),
    );
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
            created_at,
        };
        if let Err(e) = user_store.finalize_request_log(log).await {
            tracing::warn!("failed to finalize request log: {e}");
        }
    });
}
