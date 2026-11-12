use super::*;

pub(super) struct ImageStreamInput {
    pub(super) response: reqwest::Response,
    pub(super) legacy: UrpRequest,
}

pub(super) struct ImageCollection {
    pub(super) result: Result<urp::UrpResponse, ImageCollectionFailure>,
    pub(super) metrics: Arc<Mutex<StreamRuntimeMetrics>>,
    pub(super) reconstructed: Option<Value>,
}

pub(super) struct ImageCollectionFailure {
    pub(super) error: AppError,
    pub(super) retry: bool,
}

struct ImageDecoder {
    provider_type: ProviderType,
    response: reqwest::Response,
    request: urp::UrpRequest,
    legacy: UrpRequest,
    envelope_extra: Option<HashMap<String, Value>>,
    started_at: Instant,
    metrics: Arc<Mutex<StreamRuntimeMetrics>>,
    mask_sensitive_info: bool,
    idle_timeout_ms: u64,
}

struct ImageStreamPipeline {
    events: mpsc::Receiver<urp::UrpStreamEvent>,
    decode: tokio::task::JoinHandle<AppResult<()>>,
    transform: tokio::task::JoinHandle<AppResult<()>>,
    reconstruct: Option<tokio::task::JoinHandle<AppResult<()>>>,
    reconstruction_slot: Option<Arc<Mutex<Option<Value>>>>,
    metrics: Arc<Mutex<StreamRuntimeMetrics>>,
}

#[derive(Default)]
struct ImageCollectedEvents {
    response: Option<urp::UrpResponse>,
    error: Option<AppError>,
}

pub(super) async fn collect_image_stream(
    execution: &ImageStreamExecution<'_>,
    attempt: &MonoizeAttempt,
    prepared: &PreparedImageAttempt,
    input: ImageStreamInput,
    sink: &mut Option<&mut ImageStreamSink>,
) -> AppResult<ImageCollection> {
    let pipeline = start_image_pipeline(execution, attempt, prepared, input).await;
    pipeline.collect(&execution.logical_model, sink).await
}

async fn start_image_pipeline(
    execution: &ImageStreamExecution<'_>,
    attempt: &MonoizeAttempt,
    prepared: &PreparedImageAttempt,
    input: ImageStreamInput,
) -> ImageStreamPipeline {
    let envelope_extra = find_pending_request_envelope_extra(&execution.request.input);
    let (decoded_tx, decoded_rx) = mpsc::channel::<urp::UrpStreamEvent>(64);
    let (transformed_tx, transformed_rx) = mpsc::channel::<urp::UrpStreamEvent>(64);
    let metrics = Arc::new(Mutex::new(StreamRuntimeMetrics::default()));
    let idle_timeout_ms = execution
        .context
        .state
        .monoize_runtime
        .read()
        .await
        .stream_idle_timeout_ms
        .max(1);
    let mask_sensitive_info = execution
        .context
        .state
        .monoize_runtime
        .read()
        .await
        .mask_sensitive_info;
    let decoder = ImageDecoder {
        provider_type: attempt.provider_type,
        response: input.response,
        request: prepared.request.clone(),
        legacy: input.legacy,
        envelope_extra,
        started_at: execution.context.task_state.started_at(),
        metrics: metrics.clone(),
        mask_sensitive_info,
        idle_timeout_ms,
    };
    let decode = tokio::spawn(async move { decoder.run(decoded_tx).await });
    // RCD-D10c: capture the terminal event before response transforms.
    let reconstruction_slot = execution
        .context
        .capture
        .session
        .as_ref()
        .map(|_| Arc::new(Mutex::new(None::<Value>)));
    let (transform_input_rx, reconstruct) = match reconstruction_slot.clone() {
        Some(slot) => {
            let (tap_tx, tap_rx) = mpsc::channel::<urp::UrpStreamEvent>(64);
            let handle = tokio::spawn(async move {
                retain_reconstructed_urp_response(decoded_rx, tap_tx, slot).await
            });
            (tap_rx, Some(handle))
        }
        None => (decoded_rx, None),
    };
    let provider_rules = attempt.provider_transforms.clone();
    let global_rules = prepared.global_transforms.clone();
    let auth_rules = execution.context.auth.transforms.clone();
    let state = execution.context.state.clone();
    let model = execution.logical_model.clone();
    let provider_type = attempt.provider_type;
    let transform = tokio::spawn(async move {
        transform_urp_stream(
            &state,
            transform_input_rx,
            transformed_tx,
            StreamTransformConfig {
                provider_rules: &provider_rules,
                global_rules: &global_rules,
                auth_rules: &auth_rules,
                model: &model,
                upstream_provider_type: Some(provider_type),
                reasoning_envelope: None,
            },
        )
        .await
    });
    ImageStreamPipeline {
        events: transformed_rx,
        decode,
        transform,
        reconstruct,
        reconstruction_slot,
        metrics,
    }
}

impl ImageDecoder {
    async fn run(self, tx: mpsc::Sender<urp::UrpStreamEvent>) -> AppResult<()> {
        let json_response = self
            .response
            .headers()
            .get(axum::http::header::CONTENT_TYPE)
            .and_then(|value| value.to_str().ok())
            .is_some_and(|value| {
                value
                    .split(';')
                    .next()
                    .is_some_and(|mime| mime.trim().eq_ignore_ascii_case("application/json"))
            });
        if self.provider_type != ProviderType::Replicate
            && (self.request.stream == Some(false) || json_response)
        {
            return decode_image_json_as_stream(
                self.provider_type,
                self.response,
                &self.request,
                self.mask_sensitive_info,
                tx,
                self.idle_timeout_ms,
            )
            .await;
        }
        crate::urp::stream_decode::stream_upstream_to_urp_events(
            &self.legacy,
            self.envelope_extra,
            self.provider_type,
            self.response,
            tx,
            crate::urp::stream_decode::StreamDecodeContext {
                started_at: Some(self.started_at),
                runtime_metrics: Some(self.metrics),
                idle_timeout_ms: self.idle_timeout_ms,
            },
        )
        .await
    }
}

impl ImageStreamPipeline {
    async fn collect(
        mut self,
        model: &str,
        sink: &mut Option<&mut ImageStreamSink>,
    ) -> AppResult<ImageCollection> {
        let mut collected = ImageCollectedEvents::default();
        while let Some(event) = self.events.recv().await {
            collected.accept(event, model, sink).await;
        }
        let (decode_join, transform_join) = tokio::join!(self.decode, self.transform);
        let decode_result =
            decode_join.map_err(|error| image_task_join_error("image stream decoder", error))?;
        let transform_result = transform_join
            .unwrap_or_else(|error| Err(image_task_join_error("image stream transform", error)));
        if let Some(handle) = self.reconstruct {
            let _ = handle.await;
        }
        let reconstructed = match self.reconstruction_slot.as_ref() {
            Some(slot) => slot.lock().await.clone(),
            None => None,
        };
        let result = collected.finish(decode_result, transform_result);
        Ok(ImageCollection {
            result,
            metrics: self.metrics,
            reconstructed,
        })
    }
}

impl ImageCollectedEvents {
    async fn accept(
        &mut self,
        event: urp::UrpStreamEvent,
        model: &str,
        sink: &mut Option<&mut ImageStreamSink>,
    ) {
        match event {
            urp::UrpStreamEvent::NodeDelta {
                delta: urp::NodeDelta::Image { source },
                extra_body,
                ..
            } => {
                if let Some(sink) = sink.as_deref_mut() {
                    sink.send_partial(&source, &extra_body).await;
                }
            }
            urp::UrpStreamEvent::ResponseDone {
                outcome,
                finish_reason,
                usage,
                output,
                extra_body,
            } => {
                self.response = Some(urp::UrpResponse {
                    outcome,
                    id: extra_body
                        .get("id")
                        .and_then(Value::as_str)
                        .unwrap_or("resp_stream_collected")
                        .to_string(),
                    model: extra_body
                        .get("model")
                        .and_then(Value::as_str)
                        .unwrap_or(model)
                        .to_string(),
                    created_at: extra_body.get("created_at").and_then(Value::as_i64),
                    output: urp::TerminalNode::into_nodes(output),
                    finish_reason,
                    usage,
                    extra_body,
                });
            }
            urp::UrpStreamEvent::Error {
                code,
                message,
                extra_body,
            } => {
                self.error = Some(image_upstream_error(
                    code.as_deref(),
                    &message,
                    extra_body.get("type").and_then(Value::as_str),
                    extra_body.get("param").and_then(Value::as_str),
                    extra_body.get("status").and_then(Value::as_u64),
                ));
            }
            _ => {}
        }
    }

    fn finish(
        self,
        decoded: AppResult<()>,
        transformed: AppResult<()>,
    ) -> Result<urp::UrpResponse, ImageCollectionFailure> {
        decoded.map_err(|error| ImageCollectionFailure { error, retry: true })?;
        transformed.map_err(|error| ImageCollectionFailure {
            error,
            retry: false,
        })?;
        if let Some(error) = self.error {
            return Err(ImageCollectionFailure { error, retry: true });
        }
        let response = self.response.ok_or_else(|| ImageCollectionFailure {
            error: AppError::new(
                StatusCode::BAD_GATEWAY,
                "upstream_stream_error",
                "stream completed without terminal response",
            ),
            retry: true,
        })?;
        validate_image_subrequest_response(&response)
            .map_err(|error| ImageCollectionFailure { error, retry: true })?;
        Ok(response)
    }
}
