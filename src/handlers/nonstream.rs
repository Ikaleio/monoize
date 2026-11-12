mod codec;
mod exchange;
mod prepare;
mod reply;
mod run;
mod settle;

use super::*;

pub(super) use codec::{
    decode_response_from_provider, encode_request_for_provider, encode_response_for_downstream,
    responses_request_echo_for_attempt,
};

pub(super) struct AdmittedRequestTaskState {
    started_at: std::time::Instant,
    admitted: std::sync::atomic::AtomicBool,
    is_stream: std::sync::atomic::AtomicBool,
    client_gone: std::sync::atomic::AtomicBool,
    attempt: std::sync::Mutex<Option<MonoizeAttempt>>,
    pending_guard: std::sync::Mutex<Option<PendingRequestLogGuard>>,
}

impl AdmittedRequestTaskState {
    pub(super) fn new(started_at: std::time::Instant) -> Self {
        Self {
            started_at,
            admitted: std::sync::atomic::AtomicBool::new(false),
            is_stream: std::sync::atomic::AtomicBool::new(false),
            client_gone: std::sync::atomic::AtomicBool::new(false),
            attempt: std::sync::Mutex::new(None),
            pending_guard: std::sync::Mutex::new(None),
        }
    }

    pub(super) fn mark_client_gone(&self) {
        self.client_gone
            .store(true, std::sync::atomic::Ordering::Release);
    }

    pub(super) fn client_gone(&self) -> bool {
        self.client_gone.load(std::sync::atomic::Ordering::Acquire)
    }

    pub(super) fn set_stream(&self, is_stream: bool) {
        self.is_stream
            .store(is_stream, std::sync::atomic::Ordering::Release);
    }

    pub(super) fn started_at(&self) -> std::time::Instant {
        self.started_at
    }

    pub(super) fn retain_pending_guard(&self, guard: Option<PendingRequestLogGuard>) {
        let admitted = guard.is_some();
        *self.pending_guard.lock().unwrap_or_else(|e| e.into_inner()) = guard;
        self.admitted
            .store(admitted, std::sync::atomic::Ordering::Release);
    }

    pub(super) fn set_attempt(&self, attempt: &MonoizeAttempt) {
        *self.attempt.lock().unwrap_or_else(|e| e.into_inner()) = Some(attempt.clone());
    }

    pub(super) fn terminal_snapshot(
        &self,
    ) -> Option<(std::time::Instant, bool, Option<MonoizeAttempt>)> {
        if !self.admitted.load(std::sync::atomic::Ordering::Acquire) {
            return None;
        }
        Some((
            self.started_at,
            self.is_stream.load(std::sync::atomic::Ordering::Acquire),
            self.attempt
                .lock()
                .unwrap_or_else(|e| e.into_inner())
                .clone(),
        ))
    }
}

/// Returns the request for one upstream attempt slot (FP6j-2). The pristine
/// request is cloned while another slot may follow and moved into the final
/// slot, so no pristine copy outlives the last possible attempt.
pub(super) fn attempt_request_from_original(
    original: &mut Option<urp::UrpRequest>,
    final_slot: bool,
) -> urp::UrpRequest {
    if final_slot {
        original.take()
    } else {
        original.clone()
    }
    .expect("only the final attempt slot takes the pristine request")
}

/// FP6j-2: once the attempt's input nodes are encoded into the upstream body,
/// only a capture dump still reads them; upstream response decoding reads
/// tools, context, extra body, and model only.
pub(super) fn release_encoded_input(req: &mut urp::UrpRequest, capture: &RequestCaptureContext) {
    if capture.session.is_none() {
        req.input = Vec::new();
    }
}

pub(super) fn find_pending_request_envelope_extra(
    input: &[urp::Node],
) -> Option<HashMap<String, Value>> {
    input.iter().find_map(|node| match node {
        urp::Node::NextDownstreamEnvelopeExtra { extra_body } if !extra_body.is_empty() => {
            Some(extra_body.clone())
        }
        _ => None,
    })
}

pub(super) async fn execute_nonstream_typed_with_validator(
    inputs: NonstreamRequest<'_>,
    req: urp::UrpRequest,
    client_session_id: Option<String>,
    response_validator: Option<fn(&urp::UrpResponse) -> AppResult<()>>,
) -> AppResult<(urp::UrpResponse, String)> {
    let downstream = inputs.downstream;
    let (resp, logical_model, (), _) = execute_nonstream(
        inputs,
        req,
        client_session_id,
        response_validator.is_none(),
        |resp, logical_model| match response_validator {
            Some(validate) => validate(resp),
            None => encode_response_for_downstream(downstream, resp, logical_model).map(|_| ()),
        },
    )
    .await?;
    Ok((resp, logical_model))
}

pub(super) async fn forward_nonstream_typed_with_task_state(
    inputs: NonstreamRequest<'_>,
    req: urp::UrpRequest,
    client_session_id: Option<String>,
) -> AppResult<(Value, crate::error::ForwardedHeaders)> {
    let downstream = inputs.downstream;
    let (_, _, body, forwarded_headers) = execute_nonstream(
        inputs,
        req,
        client_session_id,
        true,
        |resp, logical_model| encode_response_for_downstream(downstream, resp, logical_model),
    )
    .await?;
    Ok((body, forwarded_headers))
}

/// Request-scoped inputs of one non-stream execution.
pub(super) struct NonstreamRequest<'a> {
    pub(super) state: &'a AppState,
    pub(super) auth: &'a crate::auth::AuthResult,
    pub(super) max_multiplier: Option<Multiplier>,
    pub(super) downstream: DownstreamProtocol,
    pub(super) request_id: Option<String>,
    pub(super) request_ip: Option<String>,
    pub(super) capture: RequestCaptureContext,
    pub(super) task_state: Option<&'a AdmittedRequestTaskState>,
}

/// Runs routing, upstream attempts, response transforms, and billing for one
/// non-stream request. `accept` runs on the final (history-decorated)
/// response before billing; its error fails the request and its value is
/// returned with the response (FP6k.1a). The last tuple member is the SE1d
/// header set of the serving attempt.
async fn execute_nonstream<T>(
    inputs: NonstreamRequest<'_>,
    req: urp::UrpRequest,
    client_session_id: Option<String>,
    strip_foreign_response: bool,
    accept: impl Fn(&urp::UrpResponse, &str) -> AppResult<T>,
) -> AppResult<(urp::UrpResponse, String, T, crate::error::ForwardedHeaders)> {
    let mut run =
        NonstreamRun::admit(inputs, req, client_session_id, strip_foreign_response).await?;
    while let Some(mut slot) = run.attempts.next() {
        match run
            .try_channel(&mut slot.attempt, slot.position, &accept)
            .await?
        {
            TryOutcome::Served(served) => {
                return Ok((
                    served.resp,
                    run.logical_model,
                    served.accepted,
                    served.forwarded_headers,
                ));
            }
            TryOutcome::Failed {
                error,
                failure_class,
            } => {
                run.attempts
                    .fail(run.inputs.state, slot, &error, failure_class)
                    .await;
            }
        }
    }
    Err(run.finish_exhausted().await)
}

/// One admitted non-stream request: its inputs, what admission resolved, and
/// the bookkeeping carried from one upstream try to the next.
struct NonstreamRun<'a> {
    inputs: NonstreamRequest<'a>,
    /// XTRA-5a; the Image API opts out because it reads response extras.
    strip_foreign_response: bool,
    started_at: std::time::Instant,
    logical_model: String,
    transform_match_model: String,
    reasoning_effort: Option<String>,
    /// Held until the run ends: its drop writes the fallback terminal log
    /// unless a terminal log was claimed. `None` when the task state holds it.
    _pending_request_log_guard: Option<PendingRequestLogGuard>,
    /// The suffix-normalized request every try re-derives its transformed
    /// request from; the final slot or a decoded response takes it (FP6j-2).
    original_req: Option<urp::UrpRequest>,
    history_context: Option<responses_history::HistoryContext>,
    attempts: AttemptDriver,
}

/// A settled response, the caller's `accept` value, and the SE1d headers of
/// the serving attempt.
struct Served<T> {
    resp: urp::UrpResponse,
    accepted: T,
    forwarded_headers: crate::error::ForwardedHeaders,
}

/// How one upstream try ended when it did not end the request.
enum TryOutcome<T> {
    Served(Box<Served<T>>),
    Failed {
        error: AppError,
        failure_class: Option<RetryableFailureClass>,
    },
}

/// One upstream try on one Channel slot, from request encoding to its reply.
struct AttemptCall {
    slot: AttemptPosition,
    /// The transformed request. Its input nodes are released once encoded
    /// unless a capture session still reads them (FP6j-2).
    req: urp::UrpRequest,
    transport: exchange::Transport,
    path: String,
    /// STR3t echo inserted into the final response.
    responses_echo: Option<Value>,
    /// Global rules read for the request phase; the response phase reuses them.
    global_transforms: Vec<TransformRuleConfig>,
    capture_transform_chain: Value,
    /// RCD-D6/RCD-D6a capture `upstream_request`; set only for a capture session.
    capture_upstream_request: Option<Value>,
    /// SE1d `FWD` of the upstream reply.
    forwarded_headers: crate::error::ForwardedHeaders,
}
