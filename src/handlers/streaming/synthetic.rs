use super::*;

/// A buffered attempt's decoded and transformed response, ready for replay.
pub(super) struct SyntheticStream {
    resp: urp::UrpResponse,
    forwarded_headers: crate::error::ForwardedHeaders,
    foreign_extras: Option<urp::ForeignResponseExtras>,
    responses_echo: Option<Value>,
    upstream_response_model: Option<String>,
    mask_sensitive_info: bool,
}

/// The slot-specific parts of a buffered attempt's capture dump.
struct BufferedDump<'a> {
    attempt_number: u32,
    path: &'a str,
    transform_chain: Value,
}

impl<'a> BufferedDump<'a> {
    fn attempt_dump(
        self,
        attempt: &'a MonoizeAttempt,
        request: &'a StreamRequest,
        nonstream_req: &'a urp::UrpRequest,
        upstream_request: Value,
    ) -> AttemptDump<'a> {
        AttemptDump {
            attempt,
            attempt_number: self.attempt_number,
            logical_model: &request.logical_model,
            upstream_model: &nonstream_req.model,
            path: self.path,
            request: nonstream_req,
            upstream_request,
            transform_chain: self.transform_chain,
        }
    }

    async fn push_error(
        self,
        attempt: &'a MonoizeAttempt,
        request: &'a StreamRequest,
        req: &'a urp::UrpRequest,
        upstream_body: Value,
        error: &AppError,
    ) {
        let Some(session) = request.capture.session.as_ref() else {
            return;
        };
        let outcome = DumpOutcome {
            error: Some(json!({
                "message": error.message,
                "code": error.code,
                "status": error.status.as_u16(),
            })),
            ..DumpOutcome::default()
        };
        self.attempt_dump(attempt, request, req, upstream_body)
            .push(session, &request.capture.raw_input, outcome)
            .await;
    }
}

/// The upstream reply of a buffered slot and the request that produced it.
struct BufferedReply {
    req: urp::UrpRequest,
    value: Value,
    forwarded_headers: crate::error::ForwardedHeaders,
    responses_echo: Option<Value>,
}

impl StreamForward {
    /// Buffered path: one non-stream upstream call whose response is later
    /// replayed as a synthetic stream.
    pub(super) async fn try_buffered(
        &mut self,
        attempt: &mut MonoizeAttempt,
        plan: &AttemptPlan,
        channel_try: ChannelTry,
    ) -> AppResult<SlotOutcome> {
        let ChannelTry {
            slot,
            req: mut nonstream_req,
            transform_chain,
        } = channel_try;
        // Every exit of this slot ends it, so the attempt request moves
        // instead of being copied.
        nonstream_req.stream = Some(false);
        let downstream = self.request.downstream;
        let mut upstream_body =
            match encode_request_for_provider(&mut nonstream_req, attempt, downstream) {
                Ok(body) => body,
                Err(err) => return Err(self.fail_request(attempt, err)),
            };
        enforce_ws_encrypted_include(
            self.request.ws_turn.as_ref(),
            attempt.provider_type,
            &mut upstream_body,
        );
        let responses_echo =
            responses_request_echo_for_attempt(&nonstream_req, attempt, downstream);
        release_encoded_input(&mut nonstream_req, &self.request.capture);
        attempt.session_affinity_value = resolve_session_affinity_value(attempt, &upstream_body);
        let provider = build_channel_provider_config(attempt);
        let path = upstream_path_for_model(attempt.provider_type, &nonstream_req.model, false);
        let dump = BufferedDump {
            attempt_number: slot.attempt_number,
            path: &path,
            transform_chain,
        };
        let http = match client_http_for_attempt(&self.request.state, attempt) {
            Ok(http) => http,
            Err(error) => {
                dump.push_error(
                    attempt,
                    &self.request,
                    &nonstream_req,
                    upstream_body,
                    &error,
                )
                .await;
                return Ok(SlotOutcome::Failed {
                    error,
                    failure_class: None,
                });
            }
        };
        let extra_headers = attempt_extra_headers(attempt, &upstream_body);
        // FP6j-2: only a capture dump reads the encoded value after
        // serialization.
        let body = upstream::JsonBody::new(&upstream_body);
        let capture_upstream_body = self
            .request
            .capture
            .session
            .is_some()
            .then_some(upstream_body);
        let call = upstream::call_upstream_json_with_response_headers(
            &http,
            &provider,
            &attempt.api_key,
            &path,
            body,
            attempt.request_timeout_ms,
            &extra_headers,
        )
        .await;
        match call {
            Ok((value, forwarded_headers)) => {
                if let (Some(session), Some(upstream_body)) =
                    (self.request.capture.session.as_ref(), capture_upstream_body)
                {
                    // RCD-D10b: buffered synthetic streams keep the provider
                    // payload in downstream_response.
                    let outcome = DumpOutcome {
                        downstream_response: Some(value.clone()),
                        ..DumpOutcome::default()
                    };
                    dump.attempt_dump(attempt, &self.request, &nonstream_req, upstream_body)
                        .push(session, &self.request.capture.raw_input, outcome)
                        .await;
                }
                let reply = BufferedReply {
                    req: nonstream_req,
                    value,
                    forwarded_headers,
                    responses_echo,
                };
                self.accept_buffered_reply(attempt, plan, reply).await
            }
            Err(err) => {
                if let (Some(session), Some(upstream_body)) =
                    (self.request.capture.session.as_ref(), capture_upstream_body)
                {
                    let outcome = DumpOutcome {
                        error: Some(call_error_json(&err)),
                        ..DumpOutcome::default()
                    };
                    dump.attempt_dump(attempt, &self.request, &nonstream_req, upstream_body)
                        .push(session, &self.request.capture.raw_input, outcome)
                        .await;
                }
                Ok(self.record_call_failure(err).await)
            }
        }
    }

    /// Decodes and transforms a buffered reply into a replayable response.
    async fn accept_buffered_reply(
        &mut self,
        attempt: &MonoizeAttempt,
        plan: &AttemptPlan,
        reply: BufferedReply,
    ) -> AppResult<SlotOutcome> {
        let BufferedReply {
            req,
            value,
            forwarded_headers,
            responses_echo,
        } = reply;
        update_pending_channel_info(
            &self.request.state,
            &self.request.auth,
            attempt,
            RequestLogContext {
                model: &self.request.logical_model,
                is_stream: true,
                started_at: self.request.started_at,
                request_id: self.request.request_id.as_deref(),
                request_ip: self.request.request_ip.as_deref(),
                reasoning_effort: None,
            },
        )
        .await;
        // Read before the match: a read guard in the scrutinee would live
        // through the failure arm, which reads the same lock again and may
        // sleep before a retry.
        let mask_sensitive_info = self.request.mask_sensitive_info().await;
        let mut resp = match decode_response_from_provider(
            attempt.provider_type,
            &value,
            &req.model,
            mask_sensitive_info,
            &req,
        ) {
            Ok(resp) => resp,
            Err(mut err) => {
                err.forwarded_headers = forwarded_headers;
                let passive_failure_class = is_same_channel_retryable_app_error(&err)
                    .then(|| classify_retryable_app_failure(&err));
                return Ok(SlotOutcome::Failed {
                    error: err,
                    failure_class: passive_failure_class,
                });
            }
        };
        let upstream_response_model = mismatched_upstream_response_model(&req.model, &resp.model);
        // MP-F3: a fail-closed missing-usage billable success rejects with 403
        // before the synthetic stream starts.
        if resp.usage.is_none() && missing_usage_rejects(&self.request.auth, attempt) {
            return Err(self
                .fail_buffered_reply(attempt, missing_usage_error())
                .await);
        }
        mark_channel_success(&self.request.state, attempt).await;
        refresh_channel_affinity(&self.request.state, attempt).await;
        let downstream = self.request.downstream;
        let foreign_extras = foreign_response_target(downstream, attempt.provider_type)
            .map(|target| urp::strip_foreign_response(&mut resp, target));
        let mask_sensitive_info = self.request.mask_sensitive_info().await;
        normalize_outcome_error(
            &mut resp.outcome,
            downstream,
            self.request.request_id.as_deref(),
            mask_sensitive_info,
        );
        // Wrap newly produced encrypted reasoning payloads in mz2 envelopes
        // BEFORE response-phase transforms can alter their payload (PIPE-1d).
        if self.request.auth.reasoning_envelope_enabled {
            urp::wrap_reasoning_envelopes_in_response(
                &mut resp,
                reasoning_envelope_provider_type(attempt.provider_type),
                &req.model,
            );
        }
        for rules in [
            &attempt.provider_transforms,
            &plan.global_transforms,
            &self.request.auth.transforms,
        ] {
            if let Err(err) = apply_transform_rules_response(
                &self.request.state,
                &mut resp,
                rules,
                &self.request.logical_model,
                Some(attempt.provider_type),
            )
            .await
            {
                return Err(self.fail_buffered_reply(attempt, err).await);
            }
        }
        if matches!(
            attempt.provider_type,
            ProviderType::OpenaiImage | ProviderType::OpenrouterImage
        ) && !matches!(downstream, DownstreamProtocol::Responses)
        {
            convert_assistant_images_to_markdown(&mut resp);
        }
        Ok(SlotOutcome::Synthetic(Box::new(SyntheticStream {
            resp,
            forwarded_headers,
            foreign_extras,
            responses_echo,
            upstream_response_model,
            mask_sensitive_info,
        })))
    }

    /// A buffered reply that cannot be replayed ends the request. The capture
    /// persists before the error log.
    async fn fail_buffered_reply(&self, attempt: &MonoizeAttempt, err: AppError) -> AppError {
        if let Some(session) = self.request.capture.session.as_ref() {
            session.persist_with_result(None, false).await;
        }
        self.fail_request(attempt, err)
    }

    /// Commits a buffered attempt and starts its replay task.
    pub(super) fn spawn_synthetic(
        self,
        attempt: MonoizeAttempt,
        plan: AttemptPlan,
        stream: SyntheticStream,
    ) -> ForwardEventStream {
        let SyntheticStream {
            mut resp,
            forwarded_headers,
            foreign_extras,
            responses_echo,
            upstream_response_model,
            mask_sensitive_info,
        } = stream;
        let history = self
            .history_context
            .map(|history| history.with_resource_scope(media_resource_scope(&attempt)));
        if let Some(history) = &history {
            history.decorate_response(&mut resp);
        }
        let (tx, rx) = mpsc::channel::<Event>(64);
        if self
            .request
            .downstream
            .is_same_family(attempt.provider_type)
        {
            self.request.commit.accept(forwarded_headers);
        }
        let replay = SyntheticReplay {
            request: self.request,
            attempt,
            tried_providers: self.attempts.tried_providers,
            pending_request_log_guard: self.pending_request_log_guard,
            history,
            sse_max_frame_length: plan.sse_max_frame_length,
            resp,
            foreign_extras,
            responses_echo,
            upstream_response_model,
            mask_sensitive_info,
        };
        tokio::spawn(replay.run(tx));
        receiver_event_stream(rx)
    }
}

/// The forwarding task of a buffered attempt.
struct SyntheticReplay {
    request: StreamRequest,
    attempt: MonoizeAttempt,
    tried_providers: Vec<TriedProvider>,
    pending_request_log_guard: Option<PendingRequestLogGuard>,
    history: Option<responses_history::HistoryContext>,
    sse_max_frame_length: Option<usize>,
    resp: urp::UrpResponse,
    foreign_extras: Option<urp::ForeignResponseExtras>,
    responses_echo: Option<Value>,
    upstream_response_model: Option<String>,
    mask_sensitive_info: bool,
}

impl SyntheticReplay {
    async fn run(self, tx: mpsc::Sender<Event>) {
        let Self {
            request,
            attempt,
            tried_providers,
            pending_request_log_guard: _pending_request_log_guard,
            history,
            sse_max_frame_length,
            mut resp,
            foreign_extras,
            responses_echo,
            upstream_response_model,
            mask_sensitive_info,
        } = self;
        let downstream = request.downstream;
        let tx_err = tx.clone();
        let encode_options = StreamEncodeOptions {
            logical_model: request.logical_model.clone(),
            sse_max_frame_length,
            mask_sensitive_info,
            chat_include_usage: request.chat_include_usage,
            responses_request_echo: responses_echo,
            started_at: request.started_at,
            replay_reasoning_duration_secs: Some(request.started_at.elapsed().as_secs()),
        };
        // History retention, settlement and logging still read `resp`.
        let stream_result = replay_urp_response(downstream, resp.clone(), tx, encode_options).await;
        if let Err(err) = &stream_result {
            emit_stream_error_if_needed(
                downstream,
                err,
                request.request_id.as_deref(),
                mask_sensitive_info,
                &tx_err,
                None,
            )
            .await;
        }
        // Retained before EOF so a client chaining `previous_response_id`
        // right after EOF finds it.
        if stream_result.is_ok()
            && let Some(history) = &history
        {
            history.retain_response(&resp).await;
        }
        // FP6j-1: dropping the last sender ends the downstream SSE before
        // settlement and logging.
        let downstream_closed = request.downstream_closed(&tx_err);
        drop(tx_err);
        if let Some(foreign_extras) = foreign_extras {
            foreign_extras.restore(&mut resp);
        }
        match stream_result {
            Ok(()) => {
                settle_replayed_response(
                    request,
                    &attempt,
                    &resp,
                    tried_providers,
                    downstream_closed,
                    upstream_response_model,
                )
                .await;
            }
            Err(err) => {
                tracing::warn!("synthetic stream failed: {}", err.message);
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
                        ttfb_ms: Some(request.started_at.elapsed().as_millis() as u64),
                        terminal_error: stream_terminal_error_from_app(&err),
                        tried_providers,
                        usage: None,
                    },
                );
                if let Some(session) = request.capture.session.as_ref() {
                    session.persist_with_result(None, true).await;
                }
            }
        }
    }
}

/// Charges a replayed response and writes its request-log row and capture.
async fn settle_replayed_response(
    request: StreamRequest,
    attempt: &MonoizeAttempt,
    resp: &urp::UrpResponse,
    tried_providers: Vec<TriedProvider>,
    downstream_closed: bool,
    upstream_response_model: Option<String>,
) {
    match maybe_charge_response(
        &request.state,
        &request.auth,
        attempt,
        &request.logical_model,
        resp,
        request.request_id.as_deref(),
    )
    .await
    {
        Ok(charge) => spawn_request_log(
            &request.state,
            &request.auth,
            attempt,
            RequestLogContext {
                model: &request.logical_model,
                is_stream: true,
                started_at: request.started_at,
                request_id: (request.request_id).as_deref(),
                request_ip: (request.request_ip).as_deref(),
                reasoning_effort: (request.reasoning_effort).as_deref(),
            },
            RequestLogSuccess {
                usage: resp.usage.clone(),
                charge_nano_usd: charge.charge_nano_usd,
                billing_breakdown_json: charge.billing_breakdown,
                ttfb_ms: Some(request.started_at.elapsed().as_millis() as u64),
                stream_terminal_diagnostics: None,
                tried_providers,
                client_gone: downstream_closed,
                upstream_response_model,
            },
        ),
        Err(err) => {
            tracing::error!(
                code = %err.code,
                "failed to settle buffered stream billing: {}",
                err.message
            );
            spawn_request_log_stream_terminal_error(
                &request.state,
                &request.auth,
                attempt,
                RequestLogContext {
                    model: &request.logical_model,
                    is_stream: true,
                    started_at: request.started_at,
                    request_id: (request.request_id).as_deref(),
                    request_ip: (request.request_ip).as_deref(),
                    reasoning_effort: (request.reasoning_effort).as_deref(),
                },
                RequestLogStreamError {
                    ttfb_ms: Some(request.started_at.elapsed().as_millis() as u64),
                    terminal_error: billing_settlement_error(&err),
                    tried_providers,
                    usage: resp.usage.clone(),
                },
            );
        }
    }
    if let Some(session) = request.capture.session.as_ref() {
        session
            .persist_with_result(resp.usage.as_ref(), false)
            .await;
    }
}
