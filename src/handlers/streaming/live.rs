use super::*;
use crate::urp::stream_decode::openai_responses::stream_responses_frames_to_urp_events;

/// A live attempt whose upstream produced its first output event (STRM-2a).
pub(super) struct LiveStream {
    pub(super) committed: CommittedUpstreamStream,
    pub(super) runtime_metrics: Arc<Mutex<StreamRuntimeMetrics>>,
    pub(super) namespace_aliases: HashMap<String, urp::ToolIdentity>,
    pub(super) responses_echo: Option<Value>,
    pub(super) estimated_input_tokens: u64,
    pub(super) mask_sensitive_info: bool,
    pub(super) capture: LiveCapture,
}

/// RCD-D6: the request side of a live attempt's capture dump.
pub(super) struct LiveCapture {
    attempt_number: u32,
    pub(super) upstream_model: String,
    path: String,
    transform_chain: Value,
    /// The transformed request and its upstream encoding, kept only while a
    /// capture session may read them.
    request: Option<(urp::UrpRequest, Value)>,
}

impl LiveCapture {
    /// Pushes this attempt's dump while a capture session is active and
    /// returns the session that received it. `outcome` and the frame
    /// snapshot are built only for an active session.
    pub(super) async fn push<'c>(
        self,
        capture: &'c RequestCaptureContext,
        attempt: &MonoizeAttempt,
        logical_model: &str,
        frames: Option<&crate::request_capture::SseFrameCapture>,
        outcome: impl FnOnce() -> DumpOutcome,
    ) -> Option<&'c RequestCaptureSession> {
        let (Some(session), Some((request, upstream_request))) =
            (capture.session.as_ref(), self.request)
        else {
            return None;
        };
        let frames = match frames {
            Some(frames) => Some(frames.snapshot().await),
            None => None,
        };
        let outcome = DumpOutcome {
            frames,
            ..outcome()
        };
        AttemptDump {
            attempt,
            attempt_number: self.attempt_number,
            logical_model,
            upstream_model: &self.upstream_model,
            path: &self.path,
            request: &request,
            upstream_request,
            transform_chain: self.transform_chain,
        }
        .push(session, &capture.raw_input, outcome)
        .await;
        Some(session)
    }
}

/// The dispatched upstream request of a live slot.
struct LiveDispatch {
    path: String,
    /// RCD-D6/RCD-D6a capture `upstream_request`.
    capture_upstream_request: Option<Value>,
    call: Result<StreamUpstreamSource, UpstreamCallError>,
}

/// What a live slot's upstream answered, before the pre-output window.
struct LiveStart {
    source: StreamUpstreamSource,
    legacy: AppResult<UrpRequest>,
    namespace_aliases: HashMap<String, urp::ToolIdentity>,
    responses_echo: Option<Value>,
    estimated_input_tokens: u64,
    capture: LiveCapture,
}

impl StreamRequest {
    /// WS2c/WS2h: tries the turn's upstream WebSocket for a Responses
    /// attempt. A failed handshake falls back to HTTP SSE; `Err` is a create
    /// frame that could not be sent.
    async fn open_websocket_source(
        &self,
        attempt: &mut MonoizeAttempt,
        upstream_model: &str,
        upstream_body: &mut Option<Value>,
        extra_headers: &[(String, String)],
    ) -> Result<Option<StreamUpstreamSource>, String> {
        let Some(turn) = self.ws_turn.as_ref().filter(|_| {
            attempt.provider_type == ProviderType::Responses
                && attempt.websocket_supported != Some(false)
        }) else {
            return Ok(None);
        };
        let mut ws_headers = vec![crate::upstream_websocket::responses_authorization_header(
            &attempt.api_key,
        )];
        ws_headers.extend(extra_headers.iter().cloned());
        let proxy = attempt
            .proxy_url
            .as_deref()
            .map(str::trim)
            .filter(|value| !value.is_empty())
            .or(self.state.node.upstream_proxy_url.as_deref())
            .map(str::to_string);
        let target = crate::upstream_websocket::UpstreamWsTarget {
            base_url: attempt.base_url.clone(),
            headers: ws_headers,
            proxy,
            timeout_ms: attempt.request_timeout_ms,
            channel_id: attempt.channel_id.clone(),
            upstream_model: upstream_model.to_string(),
        };
        let create_body = if self.capture.session.is_some() {
            upstream_body.clone()
        } else {
            upstream_body.take()
        }
        .expect("the encoded body is present until dispatch");
        match crate::upstream_websocket::open_turn_source(turn, target, create_body).await {
            Ok(source) => {
                remember_websocket_supported(&self.state, attempt, true).await;
                turn.state
                    .set_upstream_kind(crate::upstream_websocket::UPSTREAM_KIND_WEBSOCKET);
                Ok(Some(StreamUpstreamSource::ResponsesWebSocket {
                    source: Box::new(source),
                    turn: turn.clone(),
                }))
            }
            Err(crate::upstream_websocket::TurnOpenError::Handshake { error, body }) => {
                tracing::debug!(
                    channel_id = %attempt.channel_id,
                    http_status = ?error.http_status,
                    "upstream WebSocket handshake failed; using HTTP SSE: {}",
                    error.message
                );
                remember_websocket_supported(&self.state, attempt, false).await;
                if upstream_body.is_none() {
                    *upstream_body = Some(body);
                }
                Ok(None)
            }
            Err(crate::upstream_websocket::TurnOpenError::Send(err)) => Err(err),
        }
    }
}

impl StreamForward {
    /// Live path: dispatches the slot's streaming request and waits for the
    /// upstream's first output event.
    pub(super) async fn try_live(
        &mut self,
        attempt: &mut MonoizeAttempt,
        channel_try: ChannelTry,
    ) -> AppResult<SlotOutcome> {
        let ChannelTry {
            slot,
            mut req,
            transform_chain,
        } = channel_try;
        let downstream = self.request.downstream;
        let mut upstream_body = match encode_request_for_provider(&mut req, attempt, downstream) {
            Ok(body) => body,
            Err(err) => return Err(self.fail_request(attempt, err)),
        };
        enforce_ws_encrypted_include(
            self.request.ws_turn.as_ref(),
            attempt.provider_type,
            &mut upstream_body,
        );
        let responses_echo = responses_request_echo_for_attempt(&req, attempt, downstream);
        attempt.session_affinity_value = resolve_session_affinity_value(attempt, &upstream_body);
        let estimated_input_tokens = estimated_tokens_from_utf8_bytes(
            u64::try_from(upstream::serialized_json_len(&upstream_body)).unwrap_or(u64::MAX),
        );
        let extra_headers = attempt_extra_headers(attempt, &upstream_body);
        let dispatch = match self
            .dispatch_live(attempt, slot, &req, upstream_body, &extra_headers)
            .await?
        {
            Ok(dispatch) => dispatch,
            Err(outcome) => return Ok(outcome),
        };
        let upstream_model = req.model.clone();
        let legacy = typed_request_to_legacy(&req, self.request.max_multiplier);
        let namespace_aliases = tool_namespace_aliases(&req);
        // FP6j-2: only a capture session reads the transformed request or its
        // encoding after dispatch; without one both are freed here, before
        // the pre-output window. RCD-D6a/OIU-E5g: a multipart edit attempt
        // records the sent form as `upstream_request`.
        let capture = LiveCapture {
            attempt_number: slot.attempt_number,
            upstream_model,
            path: dispatch.path,
            transform_chain,
            request: dispatch
                .capture_upstream_request
                .map(move |upstream| (req, upstream)),
        };
        match dispatch.call {
            Ok(source) => {
                let start = LiveStart {
                    source,
                    legacy,
                    namespace_aliases,
                    responses_echo,
                    estimated_input_tokens,
                    capture,
                };
                self.open_live_stream(attempt, slot, start).await
            }
            Err(err) => {
                capture
                    .push(
                        &self.request.capture,
                        attempt,
                        &self.request.logical_model,
                        None,
                        || DumpOutcome {
                            error: Some(call_error_json(&err)),
                            ..DumpOutcome::default()
                        },
                    )
                    .await;
                Ok(self.record_call_failure(attempt, slot, err).await)
            }
        }
    }

    /// Sends the encoded request over the turn's WebSocket, as local warmup
    /// frames, or over HTTP. FP6j-2: the encoded value is released once
    /// serialized for dispatch; it stays `Some` after dispatch only for a
    /// capture session. `Ok(Err(_))` is a failed WebSocket send.
    async fn dispatch_live(
        &mut self,
        attempt: &mut MonoizeAttempt,
        slot: Slot,
        req: &urp::UrpRequest,
        upstream_body: Value,
        extra_headers: &[(String, String)],
    ) -> AppResult<Result<LiveDispatch, SlotOutcome>> {
        let mut upstream_body = Some(upstream_body);
        let mut websocket_source = match self
            .request
            .open_websocket_source(attempt, &req.model, &mut upstream_body, extra_headers)
            .await
        {
            Ok(source) => source,
            Err(err) => return Ok(Err(self.fail_websocket_send(attempt, slot, err).await)),
        };
        if websocket_source.is_none()
            && let Some(turn) = self.request.ws_turn.as_ref()
        {
            if turn.warmup {
                // WS2c/WS6: a warmup never reaches an HTTP upstream.
                websocket_source = Some(StreamUpstreamSource::LocalFrames(
                    crate::upstream_websocket::local_warmup_frames(&req.model),
                ));
            } else {
                turn.state
                    .set_upstream_kind(crate::upstream_websocket::UPSTREAM_KIND_HTTP);
            }
        }
        if let Some(source) = websocket_source {
            return Ok(Ok(LiveDispatch {
                path: "/v1/responses".to_string(),
                capture_upstream_request: upstream_body,
                call: Ok(source),
            }));
        }
        let http = client_http_for_attempt(&self.request.state, attempt)?;
        // Reference edits use JSON; inline Base64 edits use multipart.
        let stream_call = match call_streaming_image_capable_upstream(
            &http,
            attempt,
            req,
            upstream_body
                .take()
                .expect("the encoded body is present until dispatch"),
            attempt.request_timeout_ms.saturating_mul(10).max(600_000),
            extra_headers,
            self.request.capture.session.is_some(),
        )
        .await
        {
            Ok(stream_call) => stream_call,
            Err(err) => return Err(self.fail_request(attempt, err)),
        };
        Ok(Ok(LiveDispatch {
            path: stream_call.path,
            capture_upstream_request: stream_call.capture_upstream_request,
            call: stream_call.result.map(StreamUpstreamSource::Http),
        }))
    }

    /// WS2h: a create frame that could not be sent fails the slot.
    async fn fail_websocket_send(
        &mut self,
        attempt: &MonoizeAttempt,
        slot: Slot,
        err: String,
    ) -> SlotOutcome {
        let app_err = AppError::new(
            StatusCode::BAD_GATEWAY,
            "upstream_websocket_send_failed",
            err,
        );
        self.request
            .log_attempt_error(attempt, &app_err, self.tried_providers.clone());
        let passive_failure_class = is_same_channel_retryable_app_error(&app_err)
            .then(|| classify_retryable_app_failure(&app_err));
        self.record_try_failure(attempt, slot, &app_err, passive_failure_class)
            .await
    }

    /// Starts the decoder and holds the slot open until its first output
    /// event (STRM-2a) or a pre-output failure (STRM-2b).
    async fn open_live_stream(
        &mut self,
        attempt: &MonoizeAttempt,
        slot: Slot,
        start: LiveStart,
    ) -> AppResult<SlotOutcome> {
        let LiveStart {
            source,
            legacy,
            namespace_aliases,
            responses_echo,
            estimated_input_tokens,
            capture,
        } = start;
        let downstream = self.request.downstream;
        // FP4h: an HTTP attempt is accepted by its 2xx headers; a WebSocket
        // attempt only by its first non-control frame.
        if let StreamUpstreamSource::Http(upstream_resp) = &source {
            self.request
                .commit
                .accept(if downstream.is_same_family(attempt.provider_type) {
                    upstream::forwarded_response_headers(upstream_resp.headers())
                } else {
                    Vec::new()
                });
        }
        let legacy = match legacy {
            Ok(legacy) => legacy,
            Err(err) => return Err(self.fail_request(attempt, err)),
        };
        // XTRA-5b: request-envelope members never cross families.
        let pending_request_envelope_extra = self
            .request
            .pending_request_envelope_extra
            .as_ref()
            .filter(|_| downstream.is_same_family(attempt.provider_type))
            .cloned();
        let runtime_metrics = Arc::new(Mutex::new(StreamRuntimeMetrics::default()));
        let (stream_idle_timeout_ms, mask_sensitive_info) = {
            let runtime = self.request.state.monoize_runtime.read().await;
            (
                runtime.stream_idle_timeout_ms.max(1),
                runtime.mask_sensitive_info,
            )
        };
        let decoder = StreamDecoder {
            legacy,
            pending_request_envelope_extra,
            provider_type: attempt.provider_type,
            started_at: self.request.started_at,
            runtime_metrics: runtime_metrics.clone(),
            idle_timeout_ms: stream_idle_timeout_ms,
        };
        let (decoded_rx, decode_handle) = decoder.spawn(source);
        let committed = match probe_stream_first_output(
            decoded_rx,
            decode_handle,
            &runtime_metrics,
            &self.request.commit,
        )
        .await
        {
            Ok(committed) => committed,
            Err(failure) => return Ok(self.fail_pre_output(attempt, slot, capture, failure).await),
        };
        update_pending_channel_info(
            &self.request.state,
            &self.request.auth,
            attempt,
            &self.request.logical_model,
            true,
            self.request.request_id.as_deref(),
            self.request.request_ip.as_deref(),
            self.request.started_at,
        )
        .await;
        mark_channel_success(&self.request.state, attempt).await;
        Ok(SlotOutcome::Live(Box::new(LiveStream {
            committed,
            runtime_metrics,
            namespace_aliases,
            responses_echo,
            estimated_input_tokens,
            mask_sensitive_info,
            capture,
        })))
    }

    /// STRM-2b: a stream that failed before its first output event fails the
    /// slot so the router can fall back.
    async fn fail_pre_output(
        &mut self,
        attempt: &MonoizeAttempt,
        slot: Slot,
        capture: LiveCapture,
        failure: PreOutputStreamFailure,
    ) -> SlotOutcome {
        tracing::warn!(
            attempt_number = slot.attempt_number,
            channel_id = %attempt.channel_id,
            code = %failure.error.code,
            "upstream stream failed before its first output event: {}",
            failure.error.message
        );
        capture
            .push(
                &self.request.capture,
                attempt,
                &self.request.logical_model,
                None,
                || DumpOutcome {
                    error: Some(json!({
                        "message": failure.error.message,
                        "code": failure.error.code,
                        "status": failure.error.status.as_u16(),
                    })),
                    ..DumpOutcome::default()
                },
            )
            .await;
        self.record_try_failure(attempt, slot, &failure.error, failure.failure_class)
            .await
    }
}

/// The decoder task inputs of one live slot.
struct StreamDecoder {
    legacy: UrpRequest,
    pending_request_envelope_extra: Option<HashMap<String, Value>>,
    provider_type: ProviderType,
    started_at: std::time::Instant,
    runtime_metrics: Arc<Mutex<StreamRuntimeMetrics>>,
    idle_timeout_ms: u64,
}

impl StreamDecoder {
    fn spawn(
        self,
        source: StreamUpstreamSource,
    ) -> (
        mpsc::Receiver<urp::UrpStreamEvent>,
        tokio::task::JoinHandle<AppResult<()>>,
    ) {
        let (decoded_tx, decoded_rx) = mpsc::channel::<urp::UrpStreamEvent>(64);
        let handle = tokio::spawn(async move {
            let Self {
                legacy,
                pending_request_envelope_extra,
                provider_type,
                started_at,
                runtime_metrics,
                idle_timeout_ms,
            } = self;
            let frame_rx = match source {
                StreamUpstreamSource::Http(upstream_resp) => {
                    return stream_upstream_to_urp_events(
                        &legacy,
                        pending_request_envelope_extra,
                        provider_type,
                        upstream_resp,
                        decoded_tx,
                        Some(started_at),
                        Some(runtime_metrics),
                        idle_timeout_ms,
                    )
                    .await;
                }
                StreamUpstreamSource::ResponsesWebSocket { source, turn } => {
                    let (frame_tx, frame_rx) = mpsc::channel(32);
                    crate::upstream_websocket::spawn_turn_feeder(
                        *source,
                        turn,
                        frame_tx,
                        idle_timeout_ms,
                    );
                    frame_rx
                }
                StreamUpstreamSource::LocalFrames(local_frames) => {
                    let (frame_tx, frame_rx) = mpsc::channel(local_frames.len().max(1));
                    for frame in local_frames {
                        let _ = frame_tx.try_send(Ok((String::new(), frame)));
                    }
                    frame_rx
                }
            };
            stream_responses_frames_to_urp_events(
                &legacy,
                pending_request_envelope_extra,
                frame_rx,
                decoded_tx,
                Some(started_at),
                Some(runtime_metrics),
                idle_timeout_ms,
            )
            .await
        });
        (decoded_rx, handle)
    }
}
