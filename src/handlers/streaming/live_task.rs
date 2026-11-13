use super::*;

impl StreamForward {
    /// Starts the forwarding task of a live attempt whose upstream produced
    /// its first output event.
    pub(super) fn spawn_live(
        self,
        attempt: MonoizeAttempt,
        plan: AttemptPlan,
        stream: LiveStream,
    ) -> ForwardEventStream {
        let request = self.request;
        let (tx, rx) = mpsc::channel::<Event>(64);
        let capture_frames = request
            .capture
            .session
            .as_ref()
            .map(|_| crate::request_capture::SseFrameCapture::new());
        let terminal_output = Arc::new(Mutex::new(Vec::<urp::Node>::new()));
        // RCD-D10a: the reconstruction slot exists only while a capture
        // session is active; without one the tap stage is not inserted at all.
        let reconstructed = request
            .capture
            .session
            .as_ref()
            .map(|_| Arc::new(Mutex::new(None::<Value>)));
        let history = self
            .history_context
            .map(|history| history.with_resource_scope(media_resource_scope(&attempt)));
        let LiveStream {
            committed,
            runtime_metrics,
            namespace_aliases,
            responses_echo,
            estimated_input_tokens,
            mask_sensitive_info,
            capture,
        } = stream;
        let stages = LiveStages {
            committed,
            state: request.state.clone(),
            provider_transforms: attempt.provider_transforms.clone(),
            global_transforms: plan.global_transforms,
            auth_transforms: request.auth.transforms.clone(),
            transform_model: request.logical_model.clone(),
            provider_type: attempt.provider_type,
            reasoning_envelope: request.auth.reasoning_envelope_enabled.then(|| {
                (
                    reasoning_envelope_provider_type(attempt.provider_type).to_string(),
                    capture.upstream_model.clone(),
                )
            }),
            namespace_aliases,
            foreign_target: foreign_response_target(request.downstream, attempt.provider_type),
            downstream: request.downstream,
            request_id: request.request_id.clone(),
            mask_sensitive_info,
            terminal_output: terminal_output.clone(),
            reconstructed: reconstructed.clone(),
            history,
            encode_options: StreamEncodeOptions {
                logical_model: request.logical_model.clone(),
                sse_max_frame_length: plan.sse_max_frame_length,
                mask_sensitive_info,
                chat_include_usage: request.chat_include_usage,
                responses_request_echo: responses_echo,
                started_at: request.started_at,
                replay_reasoning_duration_secs: None,
            },
        };
        let forwarding = LiveForwarding {
            request,
            attempt,
            tried_providers: self.attempts.tried_providers,
            pending_request_log_guard: self.pending_request_log_guard,
            stages,
            runtime_metrics,
            estimated_input_tokens,
            mask_sensitive_info,
            capture,
            capture_frames,
            terminal_output,
            reconstructed,
        };
        crate::payload::spawn(forwarding.run(tx));
        receiver_event_stream(rx)
    }
}

/// The stage inputs of a live stream: decode → retain → transform →
/// (capture tap) → (history) → encode.
struct LiveStages {
    committed: CommittedUpstreamStream,
    state: AppState,
    provider_transforms: Vec<TransformRuleConfig>,
    global_transforms: Vec<TransformRuleConfig>,
    auth_transforms: Vec<TransformRuleConfig>,
    transform_model: String,
    provider_type: ProviderType,
    /// PIPE-1d: provider type and upstream model of the mz2 envelopes.
    reasoning_envelope: Option<(String, String)>,
    namespace_aliases: HashMap<String, urp::ToolIdentity>,
    foreign_target: Option<urp::ProviderProtocol>,
    downstream: DownstreamProtocol,
    request_id: Option<String>,
    mask_sensitive_info: bool,
    terminal_output: Arc<Mutex<Vec<urp::Node>>>,
    reconstructed: Option<Arc<Mutex<Option<Value>>>>,
    history: Option<responses_history::HistoryContext>,
    encode_options: StreamEncodeOptions,
}

/// The spawned stage tasks of a live stream.
struct RunningStages {
    decode: tokio::task::JoinHandle<AppResult<()>>,
    retain_output: tokio::task::JoinHandle<AppResult<()>>,
    transform: tokio::task::JoinHandle<AppResult<()>>,
    encode: tokio::task::JoinHandle<AppResult<()>>,
    reconstruct: Option<tokio::task::JoinHandle<AppResult<()>>>,
    history: Option<tokio::task::JoinHandle<Option<urp::UrpResponse>>>,
    history_for_commit: Option<responses_history::HistoryContext>,
}

impl LiveStages {
    /// Runs every stage to completion. Returns the combined stage result and,
    /// after a successful stream, the response history to retain.
    async fn run(
        self,
        tx: mpsc::Sender<Event>,
    ) -> (
        AppResult<()>,
        Option<(responses_history::HistoryContext, urp::UrpResponse)>,
    ) {
        self.spawn(tx).join().await
    }

    /// Spawns every stage. Each stage inherits the SSE capture scope current
    /// at this call.
    fn spawn(self, tx: mpsc::Sender<Event>) -> RunningStages {
        let Self {
            committed,
            state,
            provider_transforms,
            global_transforms,
            auth_transforms,
            transform_model,
            provider_type,
            reasoning_envelope,
            namespace_aliases,
            foreign_target,
            downstream,
            request_id,
            mask_sensitive_info,
            terminal_output,
            reconstructed,
            history,
            encode_options,
        } = self;
        let decoded_rx = replay_then_forward(committed.buffered, committed.decoded_rx);
        let decode_handle = committed.decode_handle;
        let (retained_tx, retained_rx) = mpsc::channel::<urp::UrpStreamEvent>(64);
        let (transformed_tx, transformed_rx) = mpsc::channel::<urp::UrpStreamEvent>(64);
        let retain_output_handle =
            crate::request_capture::spawn_with_sse_capture(retain_decoded_terminal_output(
                decoded_rx,
                retained_tx,
                terminal_output,
                RetainedOutputContext {
                    aliases: namespace_aliases,
                    foreign_target,
                    downstream,
                    request_id,
                    mask_sensitive_info,
                },
            ));
        let transform_handle = crate::request_capture::spawn_with_sse_capture(async move {
            let reasoning_envelope =
                reasoning_envelope
                    .as_ref()
                    .map(|(provider_type, upstream_model)| {
                        (provider_type.as_str(), upstream_model.as_str())
                    });
            transform_urp_stream(
                &state,
                retained_rx,
                transformed_tx,
                StreamTransformConfig {
                    provider_rules: &provider_transforms,
                    global_rules: &global_transforms,
                    auth_rules: &auth_transforms,
                    model: &transform_model,
                    upstream_provider_type: Some(provider_type),
                    reasoning_envelope,
                },
            )
            .await
        });
        let (encode_input_rx, reconstruct_handle) = match reconstructed {
            Some(slot) => {
                let (tap_tx, tap_rx) = mpsc::channel::<urp::UrpStreamEvent>(64);
                let handle = crate::request_capture::spawn_with_sse_capture(
                    retain_reconstructed_urp_response(transformed_rx, tap_tx, slot),
                );
                (tap_rx, Some(handle))
            }
            None => (transformed_rx, None),
        };
        let history_for_commit = history.clone();
        let (encode_input_rx, history_handle) = match history {
            Some(history) => {
                let (history_tx, history_rx) = mpsc::channel(64);
                let handle =
                    crate::payload::spawn(history.forward_stream(encode_input_rx, history_tx));
                (history_rx, Some(handle))
            }
            None => (encode_input_rx, None),
        };
        let encode_handle = crate::request_capture::spawn_with_sse_capture(encode_urp_stream(
            downstream,
            encode_input_rx,
            tx,
            encode_options,
        ));
        RunningStages {
            decode: decode_handle,
            retain_output: retain_output_handle,
            transform: transform_handle,
            encode: encode_handle,
            reconstruct: reconstruct_handle,
            history: history_handle,
            history_for_commit,
        }
    }
}

impl RunningStages {
    async fn join(
        self,
    ) -> (
        AppResult<()>,
        Option<(responses_history::HistoryContext, urp::UrpResponse)>,
    ) {
        let (decode_result, retain_output_result, transform_result, encode_result) =
            tokio::join!(self.decode, self.retain_output, self.transform, self.encode);
        // The tap forwards every event, so it ends before encode does; this
        // await never blocks the stream.
        if let Some(handle) = self.reconstruct {
            let _ = handle.await;
        }
        let history_response = match self.history {
            Some(handle) => handle.await.ok().flatten(),
            None => None,
        };
        let result = combine_stream_stage_results(
            [
                decode_result,
                retain_output_result,
                transform_result,
                encode_result,
            ]
            .map(|result| {
                result.unwrap_or_else(|e| {
                    Err(AppError::new(
                        StatusCode::INTERNAL_SERVER_ERROR,
                        "task_panic",
                        e.to_string(),
                    ))
                })
            }),
        );
        // FP6j-1: history retention runs after downstream EOF.
        let history_commit = match (&result, self.history_for_commit, history_response) {
            (Ok(()), Some(history), Some(response)) => Some((history, response)),
            _ => None,
        };
        (result, history_commit)
    }
}

/// The forwarding task of a live attempt.
struct LiveForwarding {
    request: StreamRequest,
    attempt: MonoizeAttempt,
    tried_providers: Vec<TriedProvider>,
    pending_request_log_guard: Option<PendingRequestLogGuard>,
    stages: LiveStages,
    runtime_metrics: Arc<Mutex<StreamRuntimeMetrics>>,
    estimated_input_tokens: u64,
    mask_sensitive_info: bool,
    capture: LiveCapture,
    capture_frames: Option<crate::request_capture::SseFrameCapture>,
    terminal_output: Arc<Mutex<Vec<urp::Node>>>,
    reconstructed: Option<Arc<Mutex<Option<Value>>>>,
}

impl LiveForwarding {
    async fn run(self, tx: mpsc::Sender<Event>) {
        let Self {
            request,
            attempt,
            tried_providers,
            pending_request_log_guard: _pending_request_log_guard,
            stages,
            runtime_metrics,
            estimated_input_tokens,
            mask_sensitive_info,
            capture,
            capture_frames,
            terminal_output,
            reconstructed,
        } = self;
        let tx_err = tx.clone();
        let pipeline = stages.run(tx);
        let (stream_result, history_commit) = match capture_frames.clone() {
            Some(frames) => crate::request_capture::with_sse_capture(frames, pipeline).await,
            None => pipeline.await,
        };
        // Every stage has ended, so the shared slots have no other reader.
        let settled_output = std::mem::take(&mut *terminal_output.lock().await);
        let terminal_visible_output_bytes = decoded_visible_output_bytes(&settled_output);
        // RCD-D10a/RCD-D10b: None when the stream ended without a terminal
        // response_done event.
        let reconstructed = match reconstructed.as_ref() {
            Some(slot) => slot.lock().await.take(),
            None => None,
        };
        let metrics = SettledStreamMetrics::read(
            &runtime_metrics,
            &attempt,
            estimated_input_tokens,
            terminal_visible_output_bytes,
        )
        .await;
        // SE2: a stage failure without an upstream terminal error ends the
        // wire with a Monoize error frame.
        if metrics.terminal.terminal_error.is_none()
            && let Err(err) = &stream_result
        {
            emit_stream_error_if_needed(
                request.downstream,
                err,
                request.request_id.as_deref(),
                mask_sensitive_info,
                &tx_err,
                capture_frames.as_ref(),
            )
            .await;
        }
        // Retained before EOF so a client chaining `previous_response_id`
        // right after EOF finds it.
        if let Some((history, response)) = history_commit {
            history.retain_response(&response).await;
        }
        // FP6j-1: dropping the last sender ends the downstream SSE before
        // settlement, logging, and capture persistence.
        let downstream_closed = request.downstream_closed(&tx_err);
        drop(tx_err);
        let finish = LiveFinish {
            request,
            attempt,
            tried_providers,
            capture,
            capture_frames,
            reconstructed,
            metrics,
        };
        finish
            .finish(stream_result, &settled_output, downstream_closed)
            .await;
    }
}
