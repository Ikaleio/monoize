use super::*;
use chrono::{Duration as ChronoDuration, Utc};

#[must_use]
pub(super) struct PendingRequestLogGuard {
    request_id: String,
    lifecycle: std::sync::Arc<crate::app::RequestLogLifecycle>,
    fallback_log: InsertRequestLog,
    user_store: crate::users::UserStore,
    started_at: std::time::Instant,
    pending_request_logs: std::sync::Arc<dashmap::DashMap<String, InsertRequestLog>>,
    request_log_admissions:
        std::sync::Arc<dashmap::DashMap<String, std::sync::Arc<crate::app::RequestLogLifecycle>>>,
}

impl Drop for PendingRequestLogGuard {
    fn drop(&mut self) {
        let Some(reservation) = self.lifecycle.try_schedule_terminal() else {
            return;
        };
        let log = self
            .pending_request_logs
            .get(&self.request_id)
            .map(|entry| entry.value().clone())
            .unwrap_or_else(|| self.fallback_log.clone());
        let log = guard_fallback_terminal_log(
            log,
            &self.request_id,
            self.started_at.elapsed().as_millis() as u64,
        );
        spawn_claimed_terminal_log(
            self.user_store.clone(),
            self.request_log_admissions.clone(),
            ClaimedRequestLogTerminal {
                request_id: self.request_id.clone(),
                lifecycle: self.lifecycle.clone(),
                reservation,
            },
            log,
        );
    }
}

fn aborted_fallback_terminal_log(
    mut log: InsertRequestLog,
    request_id: &str,
    duration_ms: u64,
) -> InsertRequestLog {
    log.request_id = Some(request_id.to_string());
    log.status = REQUEST_LOG_STATUS_ERROR.to_string();
    log.charge_nano_usd = None;
    log.billing_breakdown_json = None;
    log.error_code = Some("request_finalization_aborted".to_string());
    log.error_message = Some("request ended before terminal log scheduling".to_string());
    log.error_http_status = Some(StatusCode::INTERNAL_SERVER_ERROR.as_u16());
    log.duration_ms = Some(duration_ms);
    log
}

fn client_gone_fallback_terminal_log(
    mut log: InsertRequestLog,
    request_id: &str,
    duration_ms: u64,
) -> InsertRequestLog {
    log.request_id = Some(request_id.to_string());
    log.status = REQUEST_LOG_STATUS_CLIENT_GONE.to_string();
    log.charge_nano_usd = None;
    log.billing_breakdown_json = None;
    log.error_code = Some("client_gone".to_string());
    log.error_message = Some("client disconnected".to_string());
    log.error_http_status = Some(499);
    log.duration_ms = Some(duration_ms);
    log
}

fn guard_fallback_terminal_log(
    log: InsertRequestLog,
    request_id: &str,
    duration_ms: u64,
) -> InsertRequestLog {
    if std::thread::panicking() {
        aborted_fallback_terminal_log(log, request_id, duration_ms)
    } else {
        client_gone_fallback_terminal_log(log, request_id, duration_ms)
    }
}

fn apply_client_gone_if_needed(log: &mut InsertRequestLog, client_gone: bool) {
    if !client_gone || log.status != REQUEST_LOG_STATUS_SUCCESS {
        return;
    }
    log.status = REQUEST_LOG_STATUS_CLIENT_GONE.to_string();
    log.error_code = Some("client_gone".to_string());
    log.error_message = Some("client disconnected".to_string());
    log.error_http_status = Some(499);
}

fn apply_usage_fields(log: &mut InsertRequestLog, usage: Option<&urp::Usage>) {
    if let Some(usage) = usage {
        log.input_tokens = Some(usage.input_tokens);
        log.output_tokens = Some(usage.output_tokens);
        log.cache_read_tokens = usage.cached_tokens();
        log.cache_creation_tokens = usage
            .input_details
            .as_ref()
            .map(|details| details.cache_creation_tokens)
            .filter(|&value| value > 0);
        log.tool_prompt_tokens = usage
            .input_details
            .as_ref()
            .map(|details| details.tool_prompt_tokens)
            .filter(|&value| value > 0);
        log.reasoning_tokens = usage.reasoning_tokens();
        log.accepted_prediction_tokens = usage
            .output_details
            .as_ref()
            .map(|details| details.accepted_prediction_tokens)
            .filter(|&value| value > 0);
        log.rejected_prediction_tokens = usage
            .output_details
            .as_ref()
            .map(|details| details.rejected_prediction_tokens)
            .filter(|&value| value > 0);
        log.usage_breakdown_json = Some(build_usage_breakdown(usage));
    }
}

struct ClaimedRequestLogTerminal {
    request_id: String,
    lifecycle: std::sync::Arc<crate::app::RequestLogLifecycle>,
    reservation: crate::db_cache::RequestLogReservation,
}

struct RequestLogTerminalTaskCompletion {
    lifecycle: std::sync::Arc<crate::app::RequestLogLifecycle>,
}

impl Drop for RequestLogTerminalTaskCompletion {
    fn drop(&mut self) {
        self.lifecycle.complete_terminal_task();
    }
}

fn canonical_request_id(request_id: Option<&str>) -> Option<String> {
    request_id
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .map(str::to_string)
}

fn claim_request_log_terminal(
    state: &AppState,
    request_id: Option<&str>,
) -> Option<ClaimedRequestLogTerminal> {
    let Some(request_id) = canonical_request_id(request_id) else {
        tracing::error!("terminal request log is missing its canonical request id");
        return None;
    };
    let Some(lifecycle) = state
        .request_log_admissions
        .get(&request_id)
        .map(|entry| entry.value().clone())
    else {
        tracing::error!(request_id, "terminal request log is missing its lifecycle");
        return None;
    };
    let Some(reservation) = lifecycle.try_schedule_terminal() else {
        tracing::debug!(request_id, "request-log terminal was already scheduled");
        return None;
    };
    Some(ClaimedRequestLogTerminal {
        request_id,
        lifecycle,
        reservation,
    })
}

fn spawn_claimed_terminal_log(
    user_store: crate::users::UserStore,
    admissions: std::sync::Arc<
        dashmap::DashMap<String, std::sync::Arc<crate::app::RequestLogLifecycle>>,
    >,
    claim: ClaimedRequestLogTerminal,
    log: InsertRequestLog,
) {
    tokio::spawn(async move {
        let _completion = RequestLogTerminalTaskCompletion {
            lifecycle: claim.lifecycle.clone(),
        };
        match user_store
            .finalize_reserved_request_log(log, claim.reservation)
            .await
        {
            Ok(()) => {
                admissions.remove_if(&claim.request_id, |_, current| {
                    std::sync::Arc::ptr_eq(current, &claim.lifecycle)
                });
            }
            Err(error) => {
                tracing::error!(
                    request_id = claim.request_id,
                    "failed to durably enqueue terminal request log: {error}"
                );
            }
        }
    });
}

fn request_created_at(started_at: std::time::Instant) -> chrono::DateTime<Utc> {
    let elapsed = ChronoDuration::from_std(started_at.elapsed()).unwrap_or(ChronoDuration::MAX);
    Utc::now() - elapsed
}

fn publish_request_log_admission(
    admissions: &dashmap::DashMap<String, std::sync::Arc<crate::app::RequestLogLifecycle>>,
    request_id: &str,
    admission: crate::db_cache::RequestLogReservation,
    tracker: crate::app::RequestLogTaskTracker,
) -> Result<std::sync::Arc<crate::app::RequestLogLifecycle>, ()> {
    match admissions.entry(request_id.to_string()) {
        dashmap::mapref::entry::Entry::Vacant(entry) => {
            let lifecycle =
                std::sync::Arc::new(crate::app::RequestLogLifecycle::new(admission, tracker));
            entry.insert(lifecycle.clone());
            Ok(lifecycle)
        }
        dashmap::mapref::entry::Entry::Occupied(_) => Err(()),
    }
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
    provider_name: Option<&str>,
    channel_id: Option<&str>,
    channel_name: Option<&str>,
    upstream_model: Option<&str>,
    provider_multiplier: Option<Multiplier>,
    effective_provider_type: Option<&str>,
    affinity_hit: Option<bool>,
    affinity_key_hash: Option<&str>,
    affinity_target: Option<&str>,
    session_affinity_value: Option<&str>,
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
        names: crate::users::RequestLogNameSnapshots {
            username: auth.username.clone(),
            api_key_name: auth.api_key_name.clone(),
            provider_name: provider_name.map(ToOwned::to_owned),
            channel_name: channel_name.map(ToOwned::to_owned),
        },
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
        request_kind: auth
            .internal_source
            .map(|source| source.request_kind().to_string()),
        effective_provider_type: effective_provider_type.map(ToOwned::to_owned),
        affinity_hit,
        affinity_key_hash: affinity_key_hash.map(ToOwned::to_owned),
        affinity_target: affinity_target.map(ToOwned::to_owned),
        session_affinity_value: session_affinity_value.map(ToOwned::to_owned),
        created_at,
    };

    state
        .pending_request_logs
        .insert(request_id.to_string(), pending_log.clone());
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
) -> AppResult<Option<PendingRequestLogGuard>> {
    let Some(_user_id) = auth.user_id.as_deref() else {
        return Ok(None);
    };
    let request_id = canonical_request_id(request_id).ok_or_else(|| {
        let error = "request id missing before request-log spool admission";
        tracing::error!(
            stage = "request_id",
            request_id = "<missing>",
            error,
            "request-log spool admission failed"
        );
        AppError::new(
            StatusCode::SERVICE_UNAVAILABLE,
            "request_log_spool_unavailable",
            "request log spool is unavailable",
        )
        .with_internal_message(error)
        .with_type("server_error")
    })?;
    let admission = state
        .user_store
        .reserve_terminal_request_log()
        .map_err(|error| {
            tracing::error!(
                stage = "reserve",
                request_id = %request_id,
                error = %error,
                "request-log spool admission failed"
            );
            AppError::new(
                StatusCode::SERVICE_UNAVAILABLE,
                "request_log_spool_unavailable",
                "request log spool is unavailable",
            )
            .with_internal_message(error)
            .with_type("server_error")
        })?;
    let arm_reservation = admission.clone();
    let lifecycle = publish_request_log_admission(
        &state.request_log_admissions,
        &request_id,
        admission,
        state.request_log_tasks.clone(),
    )
    .map_err(|()| {
        AppError::new(
            StatusCode::CONFLICT,
            "duplicate_request_id",
            "request_id is already active",
        )
        .with_param("request_id")
    })?;

    broadcast_pending_snapshot(
        state,
        auth,
        &request_id,
        model,
        is_stream,
        request_ip,
        None,
        None,
        None,
        None,
        None,
        None,
        None,
        None,
        None,
        None,
        None,
        request_created_at(started_at),
    );
    let fallback_log = state
        .pending_request_logs
        .get(&request_id)
        .map(|entry| entry.value().clone())
        .expect("pending request-log snapshot exists after broadcast");
    let durable_fallback = aborted_fallback_terminal_log(
        fallback_log.clone(),
        &request_id,
        started_at.elapsed().as_millis() as u64,
    );
    if let Err(error) = state
        .user_store
        .arm_terminal_request_log(durable_fallback, &arm_reservation)
        .await
    {
        tracing::error!(
            stage = "arm",
            request_id = %request_id,
            error = %error,
            "request-log spool admission failed"
        );
        state.pending_request_logs.remove(&request_id);
        state
            .request_log_admissions
            .remove_if(&request_id, |_, current| {
                std::sync::Arc::ptr_eq(current, &lifecycle)
            });
        lifecycle.complete_terminal_task();
        return Err(AppError::new(
            StatusCode::SERVICE_UNAVAILABLE,
            "request_log_spool_unavailable",
            "request log spool is unavailable",
        )
        .with_internal_message(error)
        .with_type("server_error"));
    }
    Ok(Some(PendingRequestLogGuard {
        request_id,
        lifecycle,
        fallback_log,
        user_store: state.user_store.clone(),
        started_at,
        pending_request_logs: state.pending_request_logs.clone(),
        request_log_admissions: state.request_log_admissions.clone(),
    }))
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
        Some(&attempt.provider_name),
        Some(&attempt.channel_id),
        Some(&attempt.channel_name),
        Some(&attempt.upstream_model),
        Some(attempt.model_multiplier),
        Some(reasoning_envelope_provider_type(attempt.provider_type)),
        attempt.affinity_hit,
        attempt.affinity_key_hash.as_deref(),
        attempt.affinity_target.as_deref(),
        attempt.session_affinity_value.as_deref(),
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
    client_gone: bool,
) {
    let Some(user_id) = auth.user_id.clone() else {
        return;
    };
    let Some(claim) = claim_request_log_terminal(state, request_id.as_deref()) else {
        return;
    };
    let request_id = Some(claim.request_id.clone());
    let api_key_id = auth.api_key_id.clone();
    let provider_id = attempt.provider_id.clone();
    let upstream_model = attempt.upstream_model.clone();
    let model_multiplier = attempt.model_multiplier;
    let effective_provider_type =
        reasoning_envelope_provider_type(attempt.provider_type).to_string();
    let affinity_hit = attempt.affinity_hit;
    let affinity_key_hash = attempt.affinity_key_hash.clone();
    let affinity_target = attempt.affinity_target.clone();
    let session_affinity_value = attempt.session_affinity_value.clone();
    let model = model.to_string();
    let duration_ms = started_at.elapsed().as_millis() as u64;
    let created_at = request_created_at(started_at);
    let names = crate::users::RequestLogNameSnapshots {
        username: auth.username.clone(),
        api_key_name: auth.api_key_name.clone(),
        provider_name: Some(attempt.provider_name.clone()),
        channel_name: Some(attempt.channel_name.clone()),
    };
    let usage_breakdown_json = usage.as_ref().map(build_usage_breakdown);
    let tried_providers_json = if tried_providers.is_empty() {
        None
    } else {
        serde_json::to_value(&tried_providers).ok()
    };

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
            stream_terminal_error_code = stream_terminal_diagnostics
                .as_ref()
                .and_then(|diagnostics| diagnostics.terminal_error.as_ref())
                .map(|err| err.code.as_str()),
            stream_terminal_error_http_status = stream_terminal_diagnostics
                .as_ref()
                .and_then(|diagnostics| diagnostics.terminal_error.as_ref())
                .map(|err| err.http_status),
            stream_synthetic_terminal_emitted = stream_terminal_diagnostics
                .as_ref()
                .map(|diagnostics| diagnostics.synthetic_terminal_emitted),
            "stream request completed without usage snapshot"
        );
    }
    let mut log = InsertRequestLog {
        request_id,
        user_id,
        api_key_id,
        model,
        provider_id: Some(provider_id),
        upstream_model: Some(upstream_model),
        channel_id: Some(channel_id),
        names,
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
        request_kind: auth
            .internal_source
            .map(|source| source.request_kind().to_string()),
        effective_provider_type: Some(effective_provider_type),
        affinity_hit,
        affinity_key_hash,
        affinity_target,
        session_affinity_value,
        created_at,
    };
    apply_client_gone_if_needed(&mut log, client_gone);
    spawn_claimed_terminal_log(
        state.user_store.clone(),
        state.request_log_admissions.clone(),
        claim,
        log,
    );
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
    let Some(claim) = claim_request_log_terminal(state, request_id.as_deref()) else {
        return;
    };
    let request_id = Some(claim.request_id.clone());
    let api_key_id = auth.api_key_id.clone();
    let model = model.to_string();
    let provider_id = attempt.provider_id.clone();
    let upstream_model = attempt.upstream_model.clone();
    let model_multiplier = attempt.model_multiplier;
    let channel_id = attempt.channel_id.clone();
    let effective_provider_type =
        reasoning_envelope_provider_type(attempt.provider_type).to_string();
    let affinity_hit = attempt.affinity_hit;
    let affinity_key_hash = attempt.affinity_key_hash.clone();
    let affinity_target = attempt.affinity_target.clone();
    let session_affinity_value = attempt.session_affinity_value.clone();
    let duration_ms = started_at.elapsed().as_millis() as u64;
    let created_at = request_created_at(started_at);
    let names = crate::users::RequestLogNameSnapshots {
        username: auth.username.clone(),
        api_key_name: auth.api_key_name.clone(),
        provider_name: Some(attempt.provider_name.clone()),
        channel_name: Some(attempt.channel_name.clone()),
    };
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

    let log = InsertRequestLog {
        request_id,
        user_id,
        api_key_id,
        model,
        provider_id: Some(provider_id),
        upstream_model: Some(upstream_model),
        channel_id: Some(channel_id),
        names,
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
        request_kind: auth
            .internal_source
            .map(|source| source.request_kind().to_string()),
        effective_provider_type: Some(effective_provider_type),
        affinity_hit,
        affinity_key_hash,
        affinity_target,
        session_affinity_value,
        created_at,
    };
    spawn_claimed_terminal_log(
        state.user_store.clone(),
        state.request_log_admissions.clone(),
        claim,
        log,
    );
}

#[allow(clippy::too_many_arguments)]
pub(super) fn spawn_request_log_stream_terminal_error(
    state: &AppState,
    auth: &crate::auth::AuthResult,
    attempt: &MonoizeAttempt,
    model: &str,
    started_at: std::time::Instant,
    request_id: Option<String>,
    request_ip: Option<String>,
    ttfb_ms: Option<u64>,
    terminal_error: StreamTerminalError,
    reasoning_effort: Option<String>,
    tried_providers: Vec<TriedProvider>,
    usage: Option<urp::Usage>,
) {
    let Some(user_id) = auth.user_id.clone() else {
        return;
    };
    let Some(claim) = claim_request_log_terminal(state, request_id.as_deref()) else {
        return;
    };
    let request_id = Some(claim.request_id.clone());
    let api_key_id = auth.api_key_id.clone();
    let model = model.to_string();
    let provider_id = attempt.provider_id.clone();
    let upstream_model = attempt.upstream_model.clone();
    let model_multiplier = attempt.model_multiplier;
    let channel_id = attempt.channel_id.clone();
    let effective_provider_type =
        reasoning_envelope_provider_type(attempt.provider_type).to_string();
    let affinity_hit = attempt.affinity_hit;
    let affinity_key_hash = attempt.affinity_key_hash.clone();
    let affinity_target = attempt.affinity_target.clone();
    let session_affinity_value = attempt.session_affinity_value.clone();
    let duration_ms = started_at.elapsed().as_millis() as u64;
    let created_at = request_created_at(started_at);
    let names = crate::users::RequestLogNameSnapshots {
        username: auth.username.clone(),
        api_key_name: auth.api_key_name.clone(),
        provider_name: Some(attempt.provider_name.clone()),
        channel_name: Some(attempt.channel_name.clone()),
    };
    let tried_providers_json = if tried_providers.is_empty() {
        None
    } else {
        serde_json::to_value(&tried_providers).ok()
    };

    let mut log = InsertRequestLog {
        request_id,
        user_id,
        api_key_id,
        model,
        provider_id: Some(provider_id),
        upstream_model: Some(upstream_model),
        channel_id: Some(channel_id),
        names,
        is_stream: true,
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
        error_code: Some(terminal_error.code),
        error_message: Some(terminal_error.message),
        error_http_status: Some(terminal_error.http_status),
        duration_ms: Some(duration_ms),
        ttfb_ms,
        request_ip,
        reasoning_effort,
        tried_providers_json,
        request_kind: auth
            .internal_source
            .map(|source| source.request_kind().to_string()),
        effective_provider_type: Some(effective_provider_type),
        affinity_hit,
        affinity_key_hash,
        affinity_target,
        session_affinity_value,
        created_at,
    };
    apply_usage_fields(&mut log, usage.as_ref());
    spawn_claimed_terminal_log(
        state.user_store.clone(),
        state.request_log_admissions.clone(),
        claim,
        log,
    );
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
    let Some(claim) = claim_request_log_terminal(state, request_id.as_deref()) else {
        return;
    };
    let request_id = Some(claim.request_id.clone());
    let api_key_id = auth.api_key_id.clone();
    let model = model.to_string();
    let duration_ms = started_at.elapsed().as_millis() as u64;
    let created_at = request_created_at(started_at);
    let names = crate::users::RequestLogNameSnapshots {
        username: auth.username.clone(),
        api_key_name: auth.api_key_name.clone(),
        provider_name: None,
        channel_name: None,
    };
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

    let log = InsertRequestLog {
        request_id,
        user_id,
        api_key_id,
        model,
        provider_id: None,
        upstream_model: None,
        channel_id: None,
        names,
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
        request_kind: auth
            .internal_source
            .map(|source| source.request_kind().to_string()),
        effective_provider_type: None,
        affinity_hit: None,
        affinity_key_hash: None,
        affinity_target: None,
        session_affinity_value: None,
        created_at,
    };
    spawn_claimed_terminal_log(
        state.user_store.clone(),
        state.request_log_admissions.clone(),
        claim,
        log,
    );
}

#[cfg(test)]
mod admission_tests {
    use super::*;
    use std::sync::Arc;
    use tempfile::TempDir;

    fn reservations() -> (
        TempDir,
        crate::db_cache::RequestLogBatcher,
        crate::db_cache::RequestLogReservation,
        crate::db_cache::RequestLogReservation,
    ) {
        let temp = TempDir::new().unwrap();
        let (broadcast, _) = tokio::sync::broadcast::channel(1);
        let batcher = crate::db_cache::RequestLogBatcher::new_with_limits(
            2,
            temp.path().to_path_buf(),
            crate::db_cache::REQUEST_LOG_MIN_ENTRY_BYTES * 2,
            crate::db_cache::REQUEST_LOG_MIN_ENTRY_BYTES,
            broadcast,
            Arc::new(dashmap::DashMap::new()),
        );
        let first = batcher.reserve_terminal_log().unwrap();
        let second = batcher.reserve_terminal_log().unwrap();
        (temp, batcher, first, second)
    }

    #[test]
    fn canonical_request_id_is_trimmed_once() {
        assert_eq!(
            canonical_request_id(Some("  request-1\t")),
            Some("request-1".to_string())
        );
        assert_eq!(canonical_request_id(Some(" \t ")), None);
        assert_eq!(canonical_request_id(None), None);
    }

    #[test]
    fn duplicate_active_request_id_is_rejected_without_replacing_lifecycle() {
        let (_temp, _batcher, first, second) = reservations();
        let admissions = dashmap::DashMap::new();
        let tracker = crate::app::RequestLogTaskTracker::default();
        let first_lifecycle =
            publish_request_log_admission(&admissions, "same", first, tracker.clone()).unwrap();
        assert!(
            publish_request_log_admission(&admissions, "same", second, tracker.clone()).is_err()
        );
        assert!(Arc::ptr_eq(
            admissions.get("same").unwrap().value(),
            &first_lifecycle
        ));
        assert_eq!(tracker.active_count(), 1);
        first_lifecycle.complete_terminal_task();
    }

    #[test]
    fn terminal_scheduling_is_exactly_once() {
        let (_temp, _batcher, first, _second) = reservations();
        let tracker = crate::app::RequestLogTaskTracker::default();
        let lifecycle = crate::app::RequestLogLifecycle::new(first, tracker.clone());
        assert!(lifecycle.try_schedule_terminal().is_some());
        assert!(lifecycle.terminal_scheduled());
        assert!(lifecycle.try_schedule_terminal().is_none());
        lifecycle.complete_terminal_task();
        assert_eq!(tracker.active_count(), 0);
    }

    #[test]
    fn stale_terminal_cannot_remove_a_reused_request_id() {
        let (_temp, _batcher, first, second) = reservations();
        let admissions = Arc::new(dashmap::DashMap::new());
        let tracker = crate::app::RequestLogTaskTracker::default();
        let first_lifecycle =
            publish_request_log_admission(&admissions, "reused", first, tracker.clone()).unwrap();
        assert!(first_lifecycle.try_schedule_terminal().is_some());
        admissions.remove_if("reused", |_, current| {
            Arc::ptr_eq(current, &first_lifecycle)
        });
        first_lifecycle.complete_terminal_task();

        let second_lifecycle =
            publish_request_log_admission(&admissions, "reused", second, tracker.clone()).unwrap();
        assert!(
            admissions
                .remove_if("reused", |_, current| Arc::ptr_eq(
                    current,
                    &first_lifecycle
                ))
                .is_none()
        );
        assert!(Arc::ptr_eq(
            admissions.get("reused").unwrap().value(),
            &second_lifecycle
        ));
        second_lifecycle.complete_terminal_task();
    }

    #[test]
    fn guard_fallback_builds_client_gone_when_not_panicking() {
        let pending = pending_log("original");
        let terminal = guard_fallback_terminal_log(pending, "canonical", 42);
        assert_eq!(terminal.request_id.as_deref(), Some("canonical"));
        assert_eq!(terminal.status, REQUEST_LOG_STATUS_CLIENT_GONE);
        assert_eq!(terminal.error_code.as_deref(), Some("client_gone"));
        assert_eq!(terminal.error_http_status, Some(499));
        assert_eq!(terminal.duration_ms, Some(42));
    }

    #[test]
    fn admission_arm_fallback_remains_server_abort() {
        let pending = pending_log("original");
        let terminal = aborted_fallback_terminal_log(pending, "canonical", 42);
        assert_eq!(terminal.status, REQUEST_LOG_STATUS_ERROR);
        assert_eq!(
            terminal.error_code.as_deref(),
            Some("request_finalization_aborted")
        );
        assert_eq!(terminal.error_http_status, Some(500));
    }

    #[test]
    fn billing_settlement_error_keeps_usage_and_timing() {
        let mut log = pending_log("canonical");
        log.status = REQUEST_LOG_STATUS_ERROR.to_string();
        log.error_code = Some("billing_settlement_failed".to_string());
        let usage = urp::Usage {
            input_tokens: 12,
            output_tokens: 3,
            input_details: None,
            output_details: None,
            extra_body: Default::default(),
        };
        apply_usage_fields(&mut log, Some(&usage));
        assert_eq!(log.input_tokens, Some(12));
        assert_eq!(log.output_tokens, Some(3));
        assert!(log.usage_breakdown_json.is_some());
        assert_eq!(log.error_code.as_deref(), Some("billing_settlement_failed"));
    }

    #[tokio::test]
    async fn task_tracker_waits_for_registered_lifecycle() {
        let (_temp, _batcher, first, _second) = reservations();
        let tracker = crate::app::RequestLogTaskTracker::default();
        let lifecycle = Arc::new(crate::app::RequestLogLifecycle::new(first, tracker.clone()));
        let waiting = tokio::spawn({
            let tracker = tracker.clone();
            async move { tracker.wait_for_idle().await }
        });
        tokio::task::yield_now().await;
        assert!(!waiting.is_finished());
        lifecycle.complete_terminal_task();
        tokio::time::timeout(std::time::Duration::from_secs(1), waiting)
            .await
            .expect("tracker wait completes")
            .expect("tracker wait task succeeds");
    }

    fn pending_log(request_id: &str) -> InsertRequestLog {
        InsertRequestLog {
            request_id: Some(request_id.to_string()),
            user_id: "user-1".to_string(),
            api_key_id: Some("key-1".to_string()),
            model: "model-1".to_string(),
            provider_id: Some("provider-1".to_string()),
            upstream_model: Some("upstream-1".to_string()),
            channel_id: Some("channel-1".to_string()),
            names: crate::users::RequestLogNameSnapshots::default(),
            is_stream: false,
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
            status: crate::users::REQUEST_LOG_STATUS_PENDING.to_string(),
            usage_breakdown_json: None,
            billing_breakdown_json: None,
            error_code: None,
            error_message: None,
            error_http_status: None,
            duration_ms: None,
            ttfb_ms: None,
            request_ip: None,
            reasoning_effort: None,
            tried_providers_json: None,
            request_kind: None,
            effective_provider_type: None,
            affinity_hit: None,
            affinity_key_hash: None,
            affinity_target: None,
            session_affinity_value: None,
            created_at: chrono::Utc::now(),
        }
    }
}
