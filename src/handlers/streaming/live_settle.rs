use super::*;

/// The decoder metrics a finished live stream settles from.
pub(super) struct SettledStreamMetrics {
    ttfb_ms: Option<u64>,
    /// The usage the upstream reported.
    upstream_usage: Option<urp::Usage>,
    /// The usage the stream settles from; an estimate when `is_estimated`.
    usage: Option<urp::Usage>,
    is_estimated: bool,
    pub(super) terminal: StreamTerminalDiagnostics,
    response_service_tier: Option<String>,
    response_model: Option<String>,
}

impl SettledStreamMetrics {
    pub(super) async fn read(
        runtime_metrics: &Mutex<StreamRuntimeMetrics>,
        attempt: &MonoizeAttempt,
        estimated_input_tokens: u64,
        terminal_visible_output_bytes: u64,
    ) -> Self {
        let guard = runtime_metrics.lock().await;
        // MP-F3: a pass-through stream without upstream usage settles free
        // under the effective `allow_free_when_missing_usage` flag (or the
        // MP-F5 unpriced rule); otherwise it settles from the byte estimate
        // with `estimated = true`.
        let (usage, is_estimated) = match guard.usage.clone() {
            Some(usage) => (Some(usage), false),
            None if attempt.allow_free_when_missing_usage || attempt.model_price.is_none() => {
                (None, false)
            }
            None => {
                let visible_output_bytes = guard
                    .visible_output_bytes
                    .max(terminal_visible_output_bytes);
                let estimated_output_tokens =
                    estimated_tokens_from_utf8_bytes(visible_output_bytes);
                tracing::warn!(
                    estimated_input_tokens,
                    estimated_output_tokens,
                    "upstream stream ended without usage; billing from estimate"
                );
                (
                    Some(urp::Usage {
                        iterations: None,
                        input_tokens: estimated_input_tokens,
                        output_tokens: estimated_output_tokens,
                        input_details: None,
                        output_details: None,
                        extra_body: HashMap::new(),
                    }),
                    true,
                )
            }
        };
        Self {
            ttfb_ms: guard.ttfb_ms,
            upstream_usage: guard.usage.clone(),
            usage,
            is_estimated,
            terminal: guard.terminal.clone(),
            response_service_tier: guard.response_service_tier.clone(),
            response_model: guard.response_model.clone(),
        }
    }
}

/// A live stream after its downstream EOF: health, settlement, request log
/// and capture.
pub(super) struct LiveFinish {
    pub(super) request: StreamRequest,
    pub(super) attempt: MonoizeAttempt,
    pub(super) tried_providers: Vec<TriedProvider>,
    pub(super) capture: LiveCapture,
    pub(super) capture_frames: Option<crate::request_capture::SseFrameCapture>,
    pub(super) reconstructed: Option<Value>,
    pub(super) metrics: SettledStreamMetrics,
}

impl LiveFinish {
    /// Ends the stream by its outcome: an upstream terminal error, a stage
    /// failure, or a completed stream.
    pub(super) async fn finish(
        self,
        stream_result: AppResult<()>,
        settled_output: &[urp::Node],
        downstream_closed: bool,
    ) {
        if let Some(terminal_error) = self.metrics.terminal.terminal_error.clone() {
            self.terminal_error(terminal_error).await;
        } else if let Err(err) = stream_result {
            self.stage_error(&err).await;
        } else {
            self.settle(settled_output, downstream_closed).await;
        }
    }

    /// STRM-4: the upstream ended the stream with a terminal error.
    async fn terminal_error(self, terminal_error: StreamTerminalError) {
        let Self {
            request,
            attempt,
            tried_providers,
            capture,
            capture_frames,
            reconstructed,
            metrics,
        } = self;
        if let Some(failure_class) = midstream_terminal_failure_class(
            terminal_error.http_status,
            Some(&terminal_error.code),
            terminal_error.error_type.as_deref(),
        ) {
            record_midstream_terminal_failure(&request.state, &attempt, failure_class).await;
        }
        spawn_request_log_stream_terminal_error(
            &request.state,
            &request.auth,
            &attempt,
            &request.logical_model,
            request.started_at,
            request.request_id,
            request.request_ip,
            metrics.ttfb_ms,
            terminal_error,
            request.reasoning_effort,
            tried_providers,
            metrics.upstream_usage.clone(),
        );
        let outcome = || DumpOutcome {
            reconstructed,
            error: metrics.terminal.terminal_error.as_ref().map(|err| {
                json!({
                    "message": err.message,
                    "code": err.code,
                    "status": err.http_status,
                })
            }),
            ..DumpOutcome::default()
        };
        if let Some(session) = capture
            .push(
                &request.capture,
                &attempt,
                &request.logical_model,
                capture_frames.as_ref(),
                outcome,
            )
            .await
        {
            session
                .persist_with_result(metrics.upstream_usage.as_ref(), true)
                .await;
        }
    }

    /// STRM-4: a stage failed after the first output event.
    async fn stage_error(self, err: &AppError) {
        let Self {
            request,
            attempt,
            tried_providers,
            capture,
            capture_frames,
            reconstructed,
            metrics,
        } = self;
        tracing::warn!("stream passthrough adapter failed: {}", err.message);
        if is_upstream_adapter_failure(err) {
            record_midstream_terminal_failure(
                &request.state,
                &attempt,
                RetryableFailureClass::Transient,
            )
            .await;
        }
        spawn_request_log_stream_terminal_error(
            &request.state,
            &request.auth,
            &attempt,
            &request.logical_model,
            request.started_at,
            request.request_id,
            request.request_ip,
            metrics.ttfb_ms,
            stream_terminal_error_from_app(err),
            request.reasoning_effort,
            tried_providers,
            None,
        );
        let outcome = || DumpOutcome {
            reconstructed,
            error: Some(json!({
                "message": err.message,
                "code": err.code,
                "status": err.status.as_u16(),
            })),
            ..DumpOutcome::default()
        };
        if let Some(session) = capture
            .push(
                &request.capture,
                &attempt,
                &request.logical_model,
                capture_frames.as_ref(),
                outcome,
            )
            .await
        {
            session
                .persist_with_result(metrics.upstream_usage.as_ref(), true)
                .await;
        }
    }

    /// Charges a completed stream, then writes its request-log row and
    /// capture.
    async fn settle(self, settled_output: &[urp::Node], downstream_closed: bool) {
        let Self {
            request,
            attempt,
            tried_providers,
            capture,
            capture_frames,
            reconstructed,
            metrics,
        } = self;
        let settled_usage = match metrics.usage.as_ref() {
            Some(usage) if metrics.is_estimated => {
                crate::settlement::SettledUsage::Estimated(usage)
            }
            Some(usage) => crate::settlement::SettledUsage::Reported(usage),
            None => crate::settlement::SettledUsage::MissingFree,
        };
        let charge = match maybe_charge_stream_usage(
            &request.state,
            &request.auth,
            &attempt,
            &request.logical_model,
            settled_usage,
            settled_output,
            metrics.response_service_tier.as_deref(),
            request.request_id.as_deref(),
        )
        .await
        {
            Ok(charge) => charge,
            Err(err) => {
                tracing::error!(
                    code = %err.code,
                    "failed to settle passthrough stream billing: {}",
                    err.message
                );
                spawn_request_log_stream_terminal_error(
                    &request.state,
                    &request.auth,
                    &attempt,
                    &request.logical_model,
                    request.started_at,
                    request.request_id,
                    request.request_ip,
                    metrics.ttfb_ms,
                    billing_settlement_error(&err),
                    request.reasoning_effort,
                    tried_providers,
                    metrics.usage.clone(),
                );
                if let Some(session) = request.capture.session.as_ref() {
                    session
                        .persist_with_result(metrics.usage.as_ref(), true)
                        .await;
                }
                return;
            }
        };
        refresh_channel_affinity(&request.state, &attempt).await;
        let upstream_response_model = mismatched_upstream_response_model(
            &capture.upstream_model,
            metrics.response_model.as_deref().unwrap_or(""),
        );
        spawn_request_log(
            &request.state,
            &request.auth,
            &attempt,
            &request.logical_model,
            metrics.usage,
            charge.charge_nano_usd,
            charge.billing_breakdown,
            true,
            request.started_at,
            request.request_id,
            request.request_ip,
            attempt.channel_id.clone(),
            metrics.ttfb_ms,
            Some(metrics.terminal),
            request.reasoning_effort,
            tried_providers,
            downstream_closed,
            upstream_response_model,
        );
        let outcome = || DumpOutcome {
            reconstructed,
            ..DumpOutcome::default()
        };
        if let Some(session) = capture
            .push(
                &request.capture,
                &attempt,
                &request.logical_model,
                capture_frames.as_ref(),
                outcome,
            )
            .await
        {
            session
                .persist_with_result(metrics.upstream_usage.as_ref(), false)
                .await;
        }
    }
}
