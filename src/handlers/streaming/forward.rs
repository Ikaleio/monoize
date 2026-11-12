use super::*;
use crate::upstream_websocket::ResponsesWsTurn;

/// Request-scoped inputs every attempt of one streaming request reads. The
/// attempt that commits moves it into its forwarding task.
pub(super) struct StreamRequest {
    pub(super) state: AppState,
    pub(super) auth: crate::auth::AuthResult,
    pub(super) downstream: DownstreamProtocol,
    pub(super) max_multiplier: Option<Multiplier>,
    pub(super) request_id: Option<String>,
    pub(super) request_ip: Option<String>,
    pub(super) capture: RequestCaptureContext,
    pub(super) ws_turn: Option<ResponsesWsTurn>,
    pub(super) commit: StreamCommit,
    pub(super) started_at: std::time::Instant,
    pub(super) transform_match_model: String,
    pub(super) logical_model: String,
    pub(super) chat_include_usage: bool,
    pub(super) reasoning_effort: Option<String>,
    pub(super) pending_request_envelope_extra: Option<HashMap<String, Value>>,
}

impl StreamRequest {
    /// Writes the stream request-log row of an attempt that failed before
    /// its forwarding task started.
    pub(super) fn log_attempt_error(
        &self,
        attempt: &MonoizeAttempt,
        error: &AppError,
        tried_providers: Vec<TriedProvider>,
    ) {
        spawn_request_log_stream_terminal_error(
            &self.state,
            &self.auth,
            attempt,
            RequestLogContext {
                model: &self.logical_model,
                is_stream: true,
                started_at: self.started_at,
                request_id: self.request_id.as_deref(),
                request_ip: self.request_ip.as_deref(),
                reasoning_effort: self.reasoning_effort.as_deref(),
            },
            RequestLogStreamError {
                ttfb_ms: None,
                terminal_error: stream_terminal_error_from_app(error),
                tried_providers,
                usage: None,
            },
        );
    }

    pub(super) async fn mask_sensitive_info(&self) -> bool {
        self.state.monoize_runtime.read().await.mask_sensitive_info
    }

    /// RL1h-1/WS2g: whether the real client went away. `tx` is the last
    /// sender of the downstream event channel.
    pub(super) fn downstream_closed(&self, tx: &mpsc::Sender<Event>) -> bool {
        tx.is_closed()
            || self.commit.downstream_gone()
            || self
                .ws_turn
                .as_ref()
                .is_some_and(|turn| turn.is_disconnected())
    }
}

/// Routing progress of one streaming request across its attempts.
pub(super) struct StreamForward {
    pub(super) request: StreamRequest,
    /// FP6j-2: the suffix-normalized request is the pristine base every
    /// attempt re-derives its transformed request from before transforms.
    original_req: Option<urp::UrpRequest>,
    pub(super) history_context: Option<responses_history::HistoryContext>,
    pub(super) pending_request_log_guard: Option<PendingRequestLogGuard>,
    pub(super) attempts: AttemptDriver,
}

/// Settings resolved once per routed attempt and shared by its channel slots.
pub(super) struct AttemptPlan {
    pub(super) global_transforms: Vec<TransformRuleConfig>,
    pub(super) sse_max_frame_length: Option<usize>,
    requires_buffered_stream: bool,
}

/// One same-channel slot with its fully transformed request.
pub(super) struct ChannelTry {
    pub(super) slot: AttemptPosition,
    pub(super) req: urp::UrpRequest,
    pub(super) transform_chain: Value,
}

/// How one channel slot ended.
pub(super) enum SlotOutcome {
    Failed {
        error: AppError,
        failure_class: Option<RetryableFailureClass>,
    },
    Synthetic(Box<SyntheticStream>),
    Live(Box<LiveStream>),
}

enum AcceptedAttempt {
    Synthetic(MonoizeAttempt, Box<SyntheticStream>),
    Live(MonoizeAttempt, Box<LiveStream>),
}

/// RCD-D6: one attempt's capture dump. Callers build it only while a capture
/// session is active.
pub(super) struct AttemptDump<'a> {
    pub(super) attempt: &'a MonoizeAttempt,
    pub(super) attempt_number: u32,
    pub(super) logical_model: &'a str,
    pub(super) upstream_model: &'a str,
    pub(super) path: &'a str,
    pub(super) request: &'a urp::UrpRequest,
    pub(super) upstream_request: Value,
    pub(super) transform_chain: Value,
}

/// RCD-D6: the response side of one attempt's capture dump.
#[derive(Default)]
pub(super) struct DumpOutcome {
    pub(super) downstream_response: Option<Value>,
    pub(super) reconstructed: Option<Value>,
    pub(super) frames: Option<crate::request_capture::CapturedSseFrames>,
    pub(super) error: Option<Value>,
}

impl AttemptDump<'_> {
    pub(super) async fn push(
        self,
        session: &RequestCaptureSession,
        raw_input: &Value,
        outcome: DumpOutcome,
    ) {
        session
            .push_attempt(crate::request_capture::build_attempt_dump(
                crate::request_capture::AttemptDumpInput {
                    attempt_number: self.attempt_number,
                    provider_id: &self.attempt.provider_id,
                    channel_id: Some(&self.attempt.channel_id),
                    provider_type: self.attempt.provider_type,
                    logical_model: self.logical_model,
                    upstream_model: self.upstream_model,
                    upstream_path: self.path,
                    raw_input: raw_input.clone(),
                    transformed_urp_request: self.request,
                    upstream_request: self.upstream_request,
                    downstream_response: outcome.downstream_response,
                    reconstructed_urp_response: outcome.reconstructed,
                    downstream_sse_frames: outcome.frames,
                    transform_chain: self.transform_chain,
                    error: outcome.error,
                },
            ))
            .await;
    }
}

/// The capture `error` of an upstream call that failed before a response body.
pub(super) fn call_error_json(err: &UpstreamCallError) -> Value {
    json!({
        "message": err.message,
        "code": err.code,
        "status": err.status.map(|status| status.as_u16()),
    })
}

pub(in crate::handlers) struct StreamForwardRequest {
    pub(in crate::handlers) state: AppState,
    pub(in crate::handlers) auth: crate::auth::AuthResult,
    pub(in crate::handlers) max_multiplier: Option<Multiplier>,
    pub(in crate::handlers) downstream: DownstreamProtocol,
    pub(in crate::handlers) request_id: Option<String>,
    pub(in crate::handlers) request_ip: Option<String>,
    pub(in crate::handlers) capture: RequestCaptureContext,
    pub(in crate::handlers) ws_turn: Option<ResponsesWsTurn>,
    pub(in crate::handlers) commit: StreamCommit,
}

pub(in crate::handlers) async fn forward_stream_typed(
    inputs: StreamForwardRequest,
    mut req: urp::UrpRequest,
    client_session_id: Option<String>,
) -> AppResult<
    impl futures_util::Stream<Item = Result<Event, std::convert::Infallible>> + Send + 'static,
> {
    let StreamForwardRequest {
        state,
        auth,
        max_multiplier,
        downstream,
        request_id,
        request_ip,
        capture,
        ws_turn,
        commit,
    } = inputs;
    let started_at = std::time::Instant::now();
    let transform_match_model = resolve_model_suffix(&state, &mut req).await?;
    let request = StreamRequest {
        logical_model: req.model.clone(),
        chat_include_usage: req
            .extra_body
            .get("stream_options")
            .and_then(|options| options.get("include_usage"))
            .and_then(Value::as_bool)
            == Some(true),
        reasoning_effort: req.reasoning.as_ref().and_then(|r| r.effort.clone()),
        pending_request_envelope_extra: find_pending_request_envelope_extra(&req.input),
        state,
        auth,
        downstream,
        max_multiplier,
        request_id,
        request_ip,
        capture,
        ws_turn,
        commit,
        started_at,
        transform_match_model,
    };
    let Some(mut forward) = StreamForward::route(request, req, client_session_id).await? else {
        return Ok(receiver_event_stream(mpsc::channel::<Event>(1).1));
    };
    while let Some(slot) = forward.attempts.next() {
        let plan = forward.plan_attempt(&slot.attempt).await;
        match forward.run_channel(slot, &plan).await? {
            Some(AcceptedAttempt::Synthetic(attempt, stream)) => {
                return Ok(forward.spawn_synthetic(attempt, plan, *stream));
            }
            Some(AcceptedAttempt::Live(attempt, stream)) => {
                return Ok(forward.spawn_live(attempt, plan, *stream));
            }
            None => {}
        }
    }
    Err(forward.exhausted().await)
}

/// WS18/WS6: pins a steering successor to its session. `Ok(false)` marks a
/// warmup that no upstream WebSocket can prepare; the handler answers it
/// locally without a request log or charge.
fn admit_ws_turn(turn: &ResponsesWsTurn, attempts: &mut Vec<MonoizeAttempt>) -> AppResult<bool> {
    if let Some((channel_id, upstream_model)) = turn.successor.as_ref() {
        attempts.retain(|attempt| {
            attempt.channel_id == *channel_id && attempt.upstream_model == *upstream_model
        });
        if attempts.is_empty() {
            return Err(AppError::new(
                StatusCode::BAD_GATEWAY,
                "steer_successor_unavailable",
                "the steering successor's upstream session is not routable",
            ));
        }
    } else if turn.warmup
        && attempts.first().is_none_or(|attempt| {
            attempt.provider_type != ProviderType::Responses
                || attempt.websocket_supported == Some(false)
        })
    {
        turn.state.set_local_warmup();
        return Ok(false);
    }
    Ok(true)
}

/// The attempt-specific request rewrites that precede the request transforms.
fn adapt_attempt_request(
    req: &mut urp::UrpRequest,
    attempt: &MonoizeAttempt,
    request: &StreamRequest,
) {
    let downstream = request.downstream;
    if matches!(downstream, DownstreamProtocol::Responses)
        || attempt.provider_type == ProviderType::Gemini
    {
        promote_responses_additional_tools(req, attempt.provider_type);
    }
    if let Some(target_protocol) = provider_type_protocol(attempt.provider_type) {
        urp::retain_provider_items_for_protocol(&mut req.input, target_protocol);
        if target_protocol == urp::ProviderProtocol::Responses {
            urp::remove_downstream_only_reasoning_for_responses(&mut req.input);
        }
    }
    if attempt.strip_cross_protocol_nested_extra
        && !downstream.is_same_family(attempt.provider_type)
    {
        urp::strip_nested_extra_body(&mut req.input);
    }
    // WS5a: the appended include is only for Responses attempts.
    if attempt.provider_type != ProviderType::Responses
        && request
            .ws_turn
            .as_ref()
            .is_some_and(|turn| turn.encrypted_include_added)
        && let Some(Value::Array(include)) = req.extra_body.get_mut("include")
    {
        include.retain(|value| value.as_str() != Some("reasoning.encrypted_content"));
        if include.is_empty() {
            req.extra_body.remove("include");
        }
    }
    inject_monoize_context(&request.auth, req);
    req.model = attempt.upstream_model.clone();
    // PIPE-1d: resolve provider/model-bound mz2 reasoning before transforms
    // can observe or mutate its encrypted replay payload.
    urp::filter_and_unwrap_reasoning_envelopes_for_upstream(
        &mut req.input,
        reasoning_envelope_provider_type(attempt.provider_type),
        &req.model,
        request.auth.reasoning_envelope_enabled,
    );
}

impl StreamForward {
    async fn run_channel(
        &mut self,
        first_slot: AttemptSlot,
        plan: &AttemptPlan,
    ) -> AppResult<Option<AcceptedAttempt>> {
        let mut next_slot = Some(first_slot);
        while let Some(mut slot) = next_slot {
            let channel_try = self
                .prepare_channel_try(&slot.attempt, plan, slot.position)
                .await?;
            let outcome = if plan.requires_buffered_stream {
                self.try_buffered(&mut slot.attempt, plan, channel_try)
                    .await?
            } else {
                self.try_live(&mut slot.attempt, channel_try).await?
            };
            match outcome {
                SlotOutcome::Failed {
                    error,
                    failure_class,
                } => {
                    self.attempts
                        .fail(&self.request.state, slot, &error, failure_class)
                        .await;
                    next_slot = self.attempts.next_retry();
                }
                SlotOutcome::Synthetic(stream) => {
                    return Ok(Some(AcceptedAttempt::Synthetic(slot.attempt, stream)));
                }
                SlotOutcome::Live(stream) => {
                    return Ok(Some(AcceptedAttempt::Live(slot.attempt, stream)));
                }
            }
        }
        Ok(None)
    }

    /// Builds the attempt list and admits the request (balance, pending
    /// request log). `None` is a WS6 warmup the handler answers locally.
    async fn route(
        request: StreamRequest,
        mut req: urp::UrpRequest,
        client_session_id: Option<String>,
    ) -> AppResult<Option<Self>> {
        let routing_stub = build_routing_stub(&req, request.max_multiplier);
        let mut attempts =
            build_monoize_attempts(&request.state, &routing_stub, &request.auth).await?;
        // Session affinity derives from the request as decoded, before media
        // routes are bound into its input nodes.
        attach_client_session_id(&mut attempts, client_session_id, Some(&req));
        if let Some(turn) = request.ws_turn.as_ref()
            && !admit_ws_turn(turn, &mut attempts)?
        {
            return Ok(None);
        }
        bind_media_request_routes(&mut req, &mut attempts)?;
        let history_context = responses_history::HistoryContext::from_request(
            &request.state,
            &req,
            request.downstream,
        );
        ensure_balance_before_forward_for_attempts(&request.state, &request.auth, &attempts)
            .await?;
        let pending_request_log_guard = insert_pending_request_log(
            &request.state,
            &request.auth,
            &request.logical_model,
            true,
            request.request_id.as_deref(),
            request.request_ip.as_deref(),
            request.started_at,
        )
        .await?;
        let forward = Self {
            request,
            original_req: Some(req),
            history_context,
            pending_request_log_guard,
            attempts: AttemptDriver::new(attempts),
        };
        Ok(Some(forward))
    }

    async fn plan_attempt(&self, attempt: &MonoizeAttempt) -> AttemptPlan {
        let request = &self.request;
        let global_transforms = request
            .state
            .monoize_runtime
            .read()
            .await
            .global_transforms
            .clone();
        let sse_max_frame_length = effective_sse_max_frame_length(
            &attempt.provider_transforms,
            &global_transforms,
            &request.auth.transforms,
            &request.logical_model,
        );
        let requires_buffered_stream = (requires_buffered_response_stream(
            &attempt.provider_transforms,
            &global_transforms,
            &request.auth.transforms,
            &request.logical_model,
            request.downstream,
        ) || attempt.provider_type == ProviderType::Replicate)
            // WS6/WS18: warmups and steering successors never make an HTTP call.
            && !request
                .ws_turn
                .as_ref()
                .is_some_and(|turn| turn.warmup || turn.successor.is_some());
        if requires_buffered_stream && let Some(turn) = request.ws_turn.as_ref() {
            turn.state
                .set_upstream_kind(crate::upstream_websocket::UPSTREAM_KIND_HTTP);
        }
        AttemptPlan {
            global_transforms,
            sse_max_frame_length,
            requires_buffered_stream,
        }
    }

    /// Derives the slot's request from the pristine request (pre-transforms)
    /// so that the cross-family strip runs BEFORE provider, global, and
    /// API-key transforms (PIPE-1).
    async fn prepare_channel_try(
        &mut self,
        attempt: &MonoizeAttempt,
        plan: &AttemptPlan,
        slot: AttemptPosition,
    ) -> AppResult<ChannelTry> {
        let mut req = attempt_request_from_original(&mut self.original_req, slot.final_slot);
        let request = &self.request;
        adapt_attempt_request(&mut req, attempt, request);
        for rules in [
            &attempt.provider_transforms,
            &plan.global_transforms,
            &request.auth.transforms,
        ] {
            if let Err(err) = apply_transform_rules_request(
                &request.state,
                &mut req,
                rules,
                &request.transform_match_model,
                Some(attempt.provider_type),
            )
            .await
            {
                return Err(self.fail_request(attempt, err));
            }
        }
        strip_monoize_context(&mut req);
        let transform_chain = request.capture.session.as_ref().map_or(Value::Null, |_| {
            crate::request_capture::build_transform_chain(
                &attempt.provider_transforms,
                &plan.global_transforms,
                &request.auth.transforms,
                &request.transform_match_model,
            )
        });
        Ok(ChannelTry {
            slot,
            req,
            transform_chain,
        })
    }

    /// Logs an error that ends the request before any forwarding task starts.
    pub(super) fn fail_request(&self, attempt: &MonoizeAttempt, err: AppError) -> AppError {
        self.request
            .log_attempt_error(attempt, &err, self.attempts.tried_providers.clone());
        err
    }

    /// Records an upstream call that failed before any response body.
    pub(super) async fn record_call_failure(&self, err: UpstreamCallError) -> SlotOutcome {
        let passive_failure_class =
            is_same_channel_retryable_error(&err).then(|| classify_retryable_failure(&err));
        let mask_sensitive_info = self.request.mask_sensitive_info().await;
        let app_err = upstream_error_to_app(err, mask_sensitive_info);
        SlotOutcome::Failed {
            error: app_err,
            failure_class: passive_failure_class,
        }
    }

    /// Every attempt failed: logs the exhausted error and persists the capture.
    async fn exhausted(self) -> AppError {
        let request = self.request;
        let final_err = self
            .attempts
            .exhausted_error(&request.logical_model, request.downstream);
        if let Some(attempt) = self.attempts.last_failed_attempt {
            spawn_request_log_stream_terminal_error(
                &request.state,
                &request.auth,
                &attempt,
                RequestLogContext {
                    model: &request.logical_model,
                    is_stream: true,
                    started_at: request.started_at,
                    request_id: (request.request_id).as_deref(),
                    request_ip: (request.request_ip).as_deref(),
                    reasoning_effort: (request.reasoning_effort).as_deref(),
                },
                RequestLogStreamError {
                    ttfb_ms: None,
                    terminal_error: stream_terminal_error_from_app(&final_err),
                    tried_providers: self.attempts.tried_providers,
                    usage: None,
                },
            );
        } else {
            spawn_request_log_error_no_attempt(
                &request.state,
                &request.auth,
                RequestLogContext {
                    model: &request.logical_model,
                    is_stream: true,
                    started_at: request.started_at,
                    request_id: (request.request_id).as_deref(),
                    request_ip: (request.request_ip).as_deref(),
                    reasoning_effort: (request.reasoning_effort).as_deref(),
                },
                &final_err,
                self.attempts.tried_providers,
            );
        }
        if let Some(session) = request.capture.session.as_ref() {
            session.persist_with_result(None, true).await;
        }
        final_err
    }
}
