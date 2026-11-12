const UPSTREAM_RESPONSE_MODEL_MAX_CHARS: usize = 200;

pub(super) fn mismatched_upstream_response_model(sent: &str, observed: &str) -> Option<String> {
    let observed = observed.trim();
    if observed.is_empty() {
        return None;
    }
    let bounded: String = observed
        .chars()
        .take(UPSTREAM_RESPONSE_MODEL_MAX_CHARS)
        .collect();
    let sent = sent.trim();
    if !sent.is_empty() && sent.eq_ignore_ascii_case(&bounded) {
        return None;
    }
    Some(bounded)
}

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
        let usage = usage.accounting();
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
        log.usage_breakdown_json = Some(build_usage_breakdown(&usage));
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

/// Request identity and timing shared by pending and terminal snapshots.
#[derive(Clone, Copy)]
pub(super) struct RequestLogContext<'a> {
    pub(super) model: &'a str,
    pub(super) is_stream: bool,
    pub(super) started_at: std::time::Instant,
    pub(super) request_id: Option<&'a str>,
    pub(super) request_ip: Option<&'a str>,
    pub(super) reasoning_effort: Option<&'a str>,
}

pub(super) struct RequestLogSuccess {
    pub(super) usage: Option<urp::Usage>,
    pub(super) charge_nano_usd: Option<i128>,
    pub(super) billing_breakdown_json: Option<Value>,
    pub(super) ttfb_ms: Option<u64>,
    pub(super) stream_terminal_diagnostics: Option<StreamTerminalDiagnostics>,
    pub(super) tried_providers: Vec<TriedProvider>,
    pub(super) client_gone: bool,
    pub(super) upstream_response_model: Option<String>,
}

pub(super) struct RequestLogStreamError {
    pub(super) ttfb_ms: Option<u64>,
    pub(super) terminal_error: StreamTerminalError,
    pub(super) tried_providers: Vec<TriedProvider>,
    pub(super) usage: Option<urp::Usage>,
}

fn base_request_log(
    auth: &crate::auth::AuthResult,
    user_id: String,
    context: RequestLogContext<'_>,
) -> InsertRequestLog {
    InsertRequestLog {
        request_id: None,
        user_id,
        api_key_id: auth.api_key_id.clone(),
        model: context.model.to_string(),
        provider_id: None,
        upstream_model: None,
        upstream_response_model: None,
        channel_id: None,
        names: crate::users::RequestLogNameSnapshots {
            username: auth.username.clone(),
            api_key_name: auth.api_key_name.clone(),
            provider_name: None,
            channel_name: None,
        },
        is_stream: context.is_stream,
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
        error_code: None,
        error_message: None,
        error_http_status: None,
        duration_ms: None,
        ttfb_ms: None,
        request_ip: context.request_ip.map(ToOwned::to_owned),
        reasoning_effort: context.reasoning_effort.map(ToOwned::to_owned),
        tried_providers_json: None,
        request_kind: auth
            .internal_source
            .map(|source| source.request_kind().to_string()),
        effective_provider_type: None,
        affinity_hit: None,
        affinity_key_hash: None,
        affinity_target: None,
        session_affinity_value: None,
        created_at: request_created_at(context.started_at),
    }
}

fn apply_attempt_fields(log: &mut InsertRequestLog, attempt: &MonoizeAttempt) {
    log.provider_id = Some(attempt.provider_id.clone());
    log.upstream_model = Some(attempt.upstream_model.clone());
    log.channel_id = Some(attempt.channel_id.clone());
    log.names.provider_name = Some(attempt.provider_name.clone());
    log.names.channel_name = Some(attempt.channel_name.clone());
    log.provider_multiplier = Some(attempt.model_multiplier);
    log.effective_provider_type =
        Some(reasoning_envelope_provider_type(attempt.provider_type).to_string());
    log.affinity_hit = attempt.affinity_hit;
    log.affinity_key_hash = attempt.affinity_key_hash.clone();
    log.affinity_target = attempt.affinity_target.clone();
    log.session_affinity_value = attempt.session_affinity_value.clone();
}

fn tried_providers_json(tried: &[TriedProvider]) -> Option<Value> {
    (!tried.is_empty())
        .then(|| serde_json::to_value(tried).ok())
        .flatten()
}

fn terminal_log(
    state: &AppState,
    auth: &crate::auth::AuthResult,
    attempt: Option<&MonoizeAttempt>,
    context: RequestLogContext<'_>,
) -> Option<(ClaimedRequestLogTerminal, InsertRequestLog)> {
    let user_id = auth.user_id.clone()?;
    let claim = claim_request_log_terminal(state, context.request_id)?;
    let mut log = base_request_log(auth, user_id, context);
    log.request_id = Some(claim.request_id.clone());
    log.duration_ms = Some(context.started_at.elapsed().as_millis() as u64);
    if let Some(attempt) = attempt {
        apply_attempt_fields(&mut log, attempt);
    }
    Some((claim, log))
}

fn submit_terminal_log(state: &AppState, claim: ClaimedRequestLogTerminal, log: InsertRequestLog) {
    spawn_claimed_terminal_log(
        state.user_store.clone(),
        state.request_log_admissions.clone(),
        claim,
        log,
    );
}

fn broadcast_pending_snapshot(
    state: &AppState,
    auth: &crate::auth::AuthResult,
    request_id: &str,
    context: RequestLogContext<'_>,
    attempt: Option<&MonoizeAttempt>,
) {
    let Some(user_id) = auth.user_id.clone() else {
        return;
    };
    let mut log = base_request_log(auth, user_id, context);
    log.request_id = Some(request_id.to_string());
    log.status = crate::users::REQUEST_LOG_STATUS_PENDING.to_string();
    if let Some(attempt) = attempt {
        apply_attempt_fields(&mut log, attempt);
    }
    state
        .pending_request_logs
        .insert(request_id.to_string(), log.clone());
    let _ = state.log_broadcast.send(vec![log]);
}

fn spool_admission_error(stage: &str, request_id: &str, error: impl Into<String>) -> AppError {
    let error = error.into();
    tracing::error!(
        stage,
        request_id,
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
}

async fn arm_request_log_fallback(
    state: &AppState,
    request_id: &str,
    lifecycle: &std::sync::Arc<crate::app::RequestLogLifecycle>,
    reservation: &crate::db_cache::RequestLogReservation,
    fallback_log: &InsertRequestLog,
    started_at: std::time::Instant,
) -> AppResult<()> {
    let fallback = aborted_fallback_terminal_log(
        fallback_log.clone(),
        request_id,
        started_at.elapsed().as_millis() as u64,
    );
    if let Err(error) = state
        .user_store
        .arm_terminal_request_log(fallback, reservation)
        .await
    {
        let error = spool_admission_error("arm", request_id, error);
        state.pending_request_logs.remove(request_id);
        state
            .request_log_admissions
            .remove_if(request_id, |_, current| {
                std::sync::Arc::ptr_eq(current, lifecycle)
            });
        lifecycle.complete_terminal_task();
        return Err(error);
    }
    Ok(())
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
    if auth.user_id.is_none() {
        return Ok(None);
    }
    let request_id = canonical_request_id(request_id).ok_or_else(|| {
        spool_admission_error(
            "request_id",
            "<missing>",
            "request id missing before request-log spool admission",
        )
    })?;
    let admission = state
        .user_store
        .reserve_terminal_request_log()
        .map_err(|error| spool_admission_error("reserve", &request_id, error))?;
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
        RequestLogContext {
            model,
            is_stream,
            request_id: Some(&request_id),
            request_ip,
            started_at,
            reasoning_effort: None,
        },
        None,
    );
    let fallback_log = state
        .pending_request_logs
        .get(&request_id)
        .map(|entry| entry.value().clone())
        .expect("pending request-log snapshot exists after broadcast");
    arm_request_log_fallback(
        state,
        &request_id,
        &lifecycle,
        &arm_reservation,
        &fallback_log,
        started_at,
    )
    .await?;
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

pub(super) async fn update_pending_channel_info(
    state: &AppState,
    auth: &crate::auth::AuthResult,
    attempt: &MonoizeAttempt,
    context: RequestLogContext<'_>,
) {
    if auth.user_id.is_none() {
        return;
    }
    let Some(request_id) = context.request_id.map(str::trim).filter(|v| !v.is_empty()) else {
        return;
    };
    broadcast_pending_snapshot(state, auth, request_id, context, Some(attempt));
}

fn warn_stream_missing_usage(
    log: &InsertRequestLog,
    diagnostics: Option<&StreamTerminalDiagnostics>,
) {
    tracing::warn!(
        request_id = log.request_id.as_deref().unwrap_or(""),
        provider_id = log.provider_id.as_deref().unwrap_or(""),
        channel_id = log.channel_id.as_deref().unwrap_or(""),
        model = %log.model,
        upstream_model = log.upstream_model.as_deref().unwrap_or(""),
        stream_saw_done_sentinel = diagnostics.map(|d| d.saw_done_sentinel),
        stream_terminal_event = diagnostics.and_then(|d| d.terminal_event.as_deref()),
        stream_terminal_finish_reason = diagnostics.and_then(|d| d.terminal_finish_reason.as_deref()),
        stream_terminal_error_code = diagnostics.and_then(|d| d.terminal_error.as_ref()).map(|e| e.code.as_str()),
        stream_terminal_error_http_status = diagnostics.and_then(|d| d.terminal_error.as_ref()).map(|e| e.http_status),
        stream_synthetic_terminal_emitted = diagnostics.map(|d| d.synthetic_terminal_emitted),
        "stream request completed without usage snapshot"
    );
}

pub(super) fn spawn_request_log(
    state: &AppState,
    auth: &crate::auth::AuthResult,
    attempt: &MonoizeAttempt,
    context: RequestLogContext<'_>,
    success: RequestLogSuccess,
) {
    let Some((claim, mut log)) = terminal_log(state, auth, Some(attempt), context) else {
        return;
    };
    if context.is_stream && success.usage.is_none() {
        warn_stream_missing_usage(&log, success.stream_terminal_diagnostics.as_ref());
    }
    log.status = REQUEST_LOG_STATUS_SUCCESS.to_string();
    log.charge_nano_usd = success.charge_nano_usd;
    log.billing_breakdown_json = success.billing_breakdown_json;
    log.ttfb_ms = success.ttfb_ms;
    log.tried_providers_json = tried_providers_json(&success.tried_providers);
    log.upstream_response_model = success.upstream_response_model;
    apply_usage_fields(&mut log, success.usage.as_ref());
    apply_client_gone_if_needed(&mut log, success.client_gone);
    submit_terminal_log(state, claim, log);
}

fn apply_error_fields(log: &mut InsertRequestLog, error: &AppError) {
    log.error_code = Some(error.code.clone());
    log.error_message = Some(
        error
            .internal_message
            .clone()
            .unwrap_or_else(|| error.message.clone()),
    );
    log.error_http_status = Some(error.status.as_u16());
}

pub(super) fn spawn_request_log_error(
    state: &AppState,
    auth: &crate::auth::AuthResult,
    attempt: &MonoizeAttempt,
    context: RequestLogContext<'_>,
    error: &AppError,
    tried_providers: Vec<TriedProvider>,
) {
    let Some((claim, mut log)) = terminal_log(state, auth, Some(attempt), context) else {
        return;
    };
    apply_error_fields(&mut log, error);
    log.tried_providers_json = tried_providers_json(&tried_providers);
    submit_terminal_log(state, claim, log);
}

pub(super) fn spawn_request_log_stream_terminal_error(
    state: &AppState,
    auth: &crate::auth::AuthResult,
    attempt: &MonoizeAttempt,
    context: RequestLogContext<'_>,
    error: RequestLogStreamError,
) {
    let Some((claim, mut log)) = terminal_log(state, auth, Some(attempt), context) else {
        return;
    };
    log.is_stream = true;
    log.error_code = Some(error.terminal_error.code);
    log.error_message = Some(error.terminal_error.message);
    log.error_http_status = Some(error.terminal_error.http_status);
    log.ttfb_ms = error.ttfb_ms;
    log.tried_providers_json = tried_providers_json(&error.tried_providers);
    let usage = error.usage.as_ref().map(urp::Usage::accounting);
    apply_usage_fields(&mut log, usage.as_deref());
    submit_terminal_log(state, claim, log);
}

pub(super) fn spawn_request_log_error_no_attempt(
    state: &AppState,
    auth: &crate::auth::AuthResult,
    context: RequestLogContext<'_>,
    error: &AppError,
    tried_providers: Vec<TriedProvider>,
) {
    let Some((claim, mut log)) = terminal_log(state, auth, None, context) else {
        return;
    };
    apply_error_fields(&mut log, error);
    log.tried_providers_json = tried_providers_json(&tried_providers);
    submit_terminal_log(state, claim, log);
}
