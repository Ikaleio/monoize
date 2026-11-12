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
            &self.logical_model,
            self.started_at,
            self.request_id.clone(),
            self.request_ip.clone(),
            None,
            stream_terminal_error_from_app(error),
            self.reasoning_effort.clone(),
            tried_providers,
            None,
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
    /// attempt re-derives its transformed request from (see
    /// `execute_nonstream_typed_with_validator`).
    original_req: Option<urp::UrpRequest>,
    attempt_count: usize,
    pub(super) history_context: Option<responses_history::HistoryContext>,
    pub(super) pending_request_log_guard: Option<PendingRequestLogGuard>,
    pub(super) tried_providers: Vec<TriedProvider>,
    last_failed_attempt: Option<MonoizeAttempt>,
    execution_state: AttemptExecutionState,
}

/// Settings resolved once per routed attempt and shared by its channel slots.
pub(super) struct AttemptPlan {
    pub(super) global_transforms: Vec<TransformRuleConfig>,
    pub(super) sse_max_frame_length: Option<usize>,
    requires_buffered_stream: bool,
    max_channel_attempts: usize,
    final_attempt: bool,
}

/// The position of one upstream request: its request-wide attempt number and
/// its index among the attempt's same-channel slots.
#[derive(Clone, Copy)]
pub(super) struct Slot {
    pub(super) attempt_number: u32,
    channel_attempt: usize,
}

/// One same-channel slot with its fully transformed request.
pub(super) struct ChannelTry {
    pub(super) slot: Slot,
    pub(super) req: urp::UrpRequest,
    pub(super) transform_chain: Value,
}

/// How one channel slot ended.
pub(super) enum SlotOutcome {
    /// The slot failed and the same channel serves another slot.
    Retry,
    /// The slot failed and the attempt is over.
    NextAttempt,
    Synthetic(SyntheticStream),
    Live(LiveStream),
}

enum AttemptOutcome {
    Exhausted,
    Synthetic(MonoizeAttempt, AttemptPlan, SyntheticStream),
    Live(MonoizeAttempt, AttemptPlan, LiveStream),
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
                self.attempt_number,
                &self.attempt.provider_id,
                Some(&self.attempt.channel_id),
                self.attempt.provider_type,
                self.logical_model,
                self.upstream_model,
                self.path,
                raw_input.clone(),
                self.request,
                self.upstream_request,
                outcome.downstream_response,
                outcome.reconstructed,
                outcome.frames,
                self.transform_chain,
                outcome.error,
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

#[allow(clippy::too_many_arguments)]
pub(in crate::handlers) async fn forward_stream_typed(
    state: AppState,
    auth: crate::auth::AuthResult,
    mut req: urp::UrpRequest,
    max_multiplier: Option<Multiplier>,
    downstream: DownstreamProtocol,
    request_id: Option<String>,
    request_ip: Option<String>,
    client_session_id: Option<String>,
    capture: RequestCaptureContext,
    ws_turn: Option<ResponsesWsTurn>,
    commit: StreamCommit,
) -> AppResult<
    impl futures_util::Stream<Item = Result<Event, std::convert::Infallible>> + Send + 'static,
> {
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
    let Some((mut forward, attempts)) =
        StreamForward::route(request, req, client_session_id).await?
    else {
        return Ok(receiver_event_stream(mpsc::channel::<Event>(1).1));
    };
    for (attempt_index, attempt) in attempts.into_iter().enumerate() {
        let final_attempt = attempt_index + 1 == forward.attempt_count;
        match forward.run_attempt(attempt, final_attempt).await? {
            AttemptOutcome::Exhausted => {}
            AttemptOutcome::Synthetic(attempt, plan, stream) => {
                return Ok(forward.spawn_synthetic(attempt, plan, stream));
            }
            AttemptOutcome::Live(attempt, plan, stream) => {
                return Ok(forward.spawn_live(attempt, plan, stream));
            }
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
    // Unwrap mz2 reasoning envelopes BEFORE any request-phase transform
    // observes the request input. See `nonstream.rs` for rationale and
    // spec references (urp-transform-system PIPE-1 step 6, PIPE-1d).
    urp::filter_and_unwrap_reasoning_envelopes_for_upstream(
        &mut req.input,
        reasoning_envelope_provider_type(attempt.provider_type),
        &req.model,
        request.auth.reasoning_envelope_enabled,
    );
}

impl StreamForward {
    /// Builds the attempt list and admits the request (balance, pending
    /// request log). `None` is a WS6 warmup the handler answers locally.
    async fn route(
        request: StreamRequest,
        mut req: urp::UrpRequest,
        client_session_id: Option<String>,
    ) -> AppResult<Option<(Self, Vec<MonoizeAttempt>)>> {
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
            attempt_count: attempts.len(),
            history_context,
            pending_request_log_guard,
            tried_providers: Vec::new(),
            last_failed_attempt: None,
            execution_state: AttemptExecutionState::default(),
        };
        Ok(Some((forward, attempts)))
    }

    /// Runs the same-channel slots of one attempt until one commits a stream.
    async fn run_attempt(
        &mut self,
        mut attempt: MonoizeAttempt,
        final_attempt: bool,
    ) -> AppResult<AttemptOutcome> {
        if self.execution_state.should_skip(&attempt) {
            return Ok(AttemptOutcome::Exhausted);
        }
        let plan = self.plan_attempt(&attempt, final_attempt).await;
        for channel_attempt in 0..plan.max_channel_attempts {
            if self.execution_state.should_skip(&attempt) {
                break;
            }
            let channel_try = self
                .prepare_channel_try(&attempt, &plan, channel_attempt)
                .await?;
            let outcome = if plan.requires_buffered_stream {
                self.try_buffered(&mut attempt, &plan, channel_try).await?
            } else {
                self.try_live(&mut attempt, channel_try).await?
            };
            match outcome {
                SlotOutcome::Retry => {}
                SlotOutcome::NextAttempt => break,
                SlotOutcome::Synthetic(stream) => {
                    return Ok(AttemptOutcome::Synthetic(attempt, plan, stream));
                }
                SlotOutcome::Live(stream) => {
                    return Ok(AttemptOutcome::Live(attempt, plan, stream));
                }
            }
        }
        Ok(AttemptOutcome::Exhausted)
    }

    async fn plan_attempt(&self, attempt: &MonoizeAttempt, final_attempt: bool) -> AttemptPlan {
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
            max_channel_attempts: same_channel_attempt_slots(attempt),
            final_attempt,
        }
    }

    /// Derives the slot's request from the pristine request (pre-transforms)
    /// so that the cross-family strip runs BEFORE provider, global, and
    /// API-key transforms; see `execute_nonstream_typed_with_validator`.
    async fn prepare_channel_try(
        &mut self,
        attempt: &MonoizeAttempt,
        plan: &AttemptPlan,
        channel_attempt: usize,
    ) -> AppResult<ChannelTry> {
        let slot = Slot {
            attempt_number: self.execution_state.record_upstream_attempt(attempt),
            channel_attempt,
        };
        let final_slot = plan.final_attempt && channel_attempt + 1 == plan.max_channel_attempts;
        let mut req = attempt_request_from_original(&mut self.original_req, final_slot);
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
        let transform_chain = crate::request_capture::build_transform_chain(
            &attempt.provider_transforms,
            &plan.global_transforms,
            &request.auth.transforms,
            &request.transform_match_model,
        );
        Ok(ChannelTry {
            slot,
            req,
            transform_chain,
        })
    }

    /// Logs an error that ends the request before any forwarding task starts.
    pub(super) fn fail_request(&self, attempt: &MonoizeAttempt, err: AppError) -> AppError {
        self.request
            .log_attempt_error(attempt, &err, self.tried_providers.clone());
        err
    }

    /// RTA-4: records a failed slot and decides whether the same channel
    /// serves the next one.
    pub(super) async fn record_try_failure(
        &mut self,
        attempt: &MonoizeAttempt,
        slot: Slot,
        err: &AppError,
        failure_class: Option<RetryableFailureClass>,
    ) -> SlotOutcome {
        record_upstream_attempt_failure(
            &self.request.state,
            attempt,
            slot.attempt_number,
            err,
            failure_class,
            &mut self.tried_providers,
            &mut self.execution_state,
        )
        .await;
        self.last_failed_attempt = Some(attempt.clone());
        if allow_same_channel_retry(
            &self.request.state,
            attempt,
            &self.execution_state,
            slot.channel_attempt + 1,
            failure_class,
        )
        .await
        {
            maybe_sleep_before_channel_retry(attempt).await;
            return SlotOutcome::Retry;
        }
        SlotOutcome::NextAttempt
    }

    /// Records an upstream call that failed before any response body.
    pub(super) async fn record_call_failure(
        &mut self,
        attempt: &MonoizeAttempt,
        slot: Slot,
        err: UpstreamCallError,
    ) -> SlotOutcome {
        let passive_failure_class =
            is_same_channel_retryable_error(&err).then(|| classify_retryable_failure(&err));
        let mask_sensitive_info = self.request.mask_sensitive_info().await;
        let app_err = upstream_error_to_app(err, mask_sensitive_info);
        self.record_try_failure(attempt, slot, &app_err, passive_failure_class)
            .await
    }

    /// Every attempt failed: logs the exhausted error and persists the capture.
    async fn exhausted(self) -> AppError {
        let request = self.request;
        let final_err = build_exhausted_error_for_downstream(
            &request.logical_model,
            &self.tried_providers,
            self.last_failed_attempt.as_ref(),
            request.downstream,
        );
        if let Some(attempt) = self.last_failed_attempt {
            spawn_request_log_stream_terminal_error(
                &request.state,
                &request.auth,
                &attempt,
                &request.logical_model,
                request.started_at,
                request.request_id,
                request.request_ip,
                None,
                stream_terminal_error_from_app(&final_err),
                request.reasoning_effort,
                self.tried_providers,
                None,
            );
        } else {
            spawn_request_log_error_no_attempt(
                &request.state,
                &request.auth,
                &request.logical_model,
                true,
                request.started_at,
                request.request_id,
                request.request_ip,
                &final_err,
                request.reasoning_effort,
                self.tried_providers,
            );
        }
        if let Some(session) = request.capture.session.as_ref() {
            session.persist_with_result(None, true).await;
        }
        final_err
    }
}
