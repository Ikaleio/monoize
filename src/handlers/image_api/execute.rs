use super::*;

pub(super) struct ImageStreamExecution<'a> {
    pub(super) context: ImageExecutionContext<'a>,
    pub(super) request: urp::UrpRequest,
    pub(super) original_request: urp::UrpRequest,
    pub(super) logical_model: String,
    pub(super) transform_match_model: String,
}

pub(super) struct PreparedImageAttempt {
    pub(super) request: urp::UrpRequest,
    pub(super) global_transforms: Vec<TransformRuleConfig>,
    pub(super) transform_chain: Value,
}

pub(super) struct ImageAttemptCapture<'a> {
    pub(super) request: &'a urp::UrpRequest,
    pub(super) path: &'a str,
    pub(super) upstream_body: Option<&'a Value>,
    pub(super) transform_chain: &'a Value,
}

pub(super) async fn execute_stream_collected_image_typed(
    context: ImageExecutionContext<'_>,
    req: urp::UrpRequest,
    mut sink: Option<&mut ImageStreamSink>,
) -> AppResult<(urp::UrpResponse, String)> {
    let (execution, attempts) = prepare_image_execution(context, req).await?;
    let mut driver = AttemptDriver::new(attempts);
    while let Some(mut slot) = driver.next() {
        execution.context.task_state.set_attempt(&slot.attempt);
        let mut prepared = match execution.prepare_attempt(&slot.attempt).await {
            Ok(prepared) => prepared,
            Err(error) => {
                return Err(execution
                    .finish_error(&slot.attempt, error, driver.tried_providers)
                    .await);
            }
        };
        let http = match client_http_for_attempt(execution.context.state, &slot.attempt) {
            Ok(http) => http,
            Err(error) => {
                execution
                    .retry_failure(&mut driver, slot, error, None, sink_frames_emitted(&sink))
                    .await?;
                continue;
            }
        };
        let DispatchedImageAttempt { call, legacy } = match dispatch_image_attempt(
            &execution,
            &mut prepared,
            &http,
            &mut slot,
            &mut driver,
            &sink,
        )
        .await
        {
            Ok(call) => call,
            Err(error) => {
                return Err(execution
                    .finish_error(&slot.attempt, error, driver.tried_providers)
                    .await);
            }
        };
        let capture = ImageAttemptCapture {
            request: &prepared.request,
            path: &call.path,
            upstream_body: call.capture_upstream_request.as_ref(),
            transform_chain: &prepared.transform_chain,
        };
        match call.result {
            Ok(response) => {
                if let Some(result) = execution
                    .accept_upstream_response(
                        slot,
                        &prepared,
                        &capture,
                        ImageStreamInput { response, legacy },
                        &mut driver,
                        &mut sink,
                    )
                    .await?
                {
                    return Ok(result);
                }
            }
            Err(error) => {
                let class = is_same_channel_retryable_error(&error)
                    .then(|| classify_retryable_failure(&error));
                let mask = execution
                    .context
                    .state
                    .monoize_runtime
                    .read()
                    .await
                    .mask_sensitive_info;
                let error = upstream_error_to_app(error, mask);
                execution
                    .push_attempt(&slot, &capture, None, Some(&error))
                    .await;
                execution
                    .retry_failure(&mut driver, slot, error, class, sink_frames_emitted(&sink))
                    .await?;
            }
        }
    }
    let error = build_exhausted_upstream_error(&execution.logical_model, &driver.tried_providers);
    execution.log_error(
        driver.last_failed_attempt.as_ref(),
        &error,
        driver.tried_providers,
    );
    execution.persist(None, true).await;
    Err(error)
}

async fn prepare_image_execution(
    mut context: ImageExecutionContext<'_>,
    mut request: urp::UrpRequest,
) -> AppResult<(ImageStreamExecution<'_>, Vec<MonoizeAttempt>)> {
    let transform_match_model = resolve_model_suffix(context.state, &mut request).await?;
    let mut original_request = request.clone();
    let logical_model = request.model.clone();
    let routing_stub = build_routing_stub(&request, context.meta.max_multiplier);
    let mut attempts = build_monoize_attempts(context.state, &routing_stub, context.auth).await?;
    bind_media_request_routes(&mut original_request, &mut attempts)?;
    attach_client_session_id(
        &mut attempts,
        context.meta.client_session_id.take(),
        Some(&request),
    );
    ensure_balance_before_forward_for_attempts(context.state, context.auth, &attempts).await?;
    let guard = insert_pending_request_log(
        context.state,
        context.auth,
        &request.model,
        true,
        context.meta.request_id.as_deref(),
        context.meta.request_ip.as_deref(),
        context.task_state.started_at(),
    )
    .await?;
    context.task_state.retain_pending_guard(guard);
    Ok((
        ImageStreamExecution {
            context,
            request,
            original_request,
            logical_model,
            transform_match_model,
        },
        attempts,
    ))
}

impl ImageStreamExecution<'_> {
    async fn accept_upstream_response(
        &self,
        slot: AttemptSlot,
        prepared: &PreparedImageAttempt,
        capture: &ImageAttemptCapture<'_>,
        input: ImageStreamInput,
        driver: &mut AttemptDriver,
        sink: &mut Option<&mut ImageStreamSink>,
    ) -> AppResult<Option<(urp::UrpResponse, String)>> {
        self.update_pending(&slot.attempt).await;
        let collected = match collect_image_stream(self, &slot.attempt, prepared, input, sink).await
        {
            Ok(collected) => collected,
            Err(error) => {
                return Err(self
                    .finish_error(
                        &slot.attempt,
                        error,
                        std::mem::take(&mut driver.tried_providers),
                    )
                    .await);
            }
        };
        match collected.result {
            Ok(response) => {
                return self
                    .finish_success(
                        &slot,
                        capture,
                        response,
                        collected.metrics,
                        collected.reconstructed,
                        std::mem::take(&mut driver.tried_providers),
                    )
                    .await
                    .map(Some);
            }
            Err(failure) => {
                self.push_attempt(
                    &slot,
                    capture,
                    collected.reconstructed,
                    Some(&failure.error),
                )
                .await;
                if !failure.retry {
                    return Err(self
                        .finish_error(
                            &slot.attempt,
                            failure.error,
                            std::mem::take(&mut driver.tried_providers),
                        )
                        .await);
                }
                let class = is_same_channel_retryable_app_error(&failure.error)
                    .then(|| classify_retryable_app_failure(&failure.error));
                self.retry_failure(
                    driver,
                    slot,
                    failure.error,
                    class,
                    sink_frames_emitted(sink),
                )
                .await?;
            }
        }
        Ok(None)
    }

    async fn prepare_attempt(&self, attempt: &MonoizeAttempt) -> AppResult<PreparedImageAttempt> {
        let mut request = self.original_request.clone();
        if let Some(protocol) = super::super::provider_type_protocol(attempt.provider_type) {
            urp::retain_provider_items_for_protocol(&mut request.input, protocol);
            if protocol == urp::ProviderProtocol::Responses {
                urp::remove_downstream_only_reasoning_for_responses(&mut request.input);
            }
        }
        if attempt.strip_cross_protocol_nested_extra
            && !DownstreamProtocol::Responses.is_same_family(attempt.provider_type)
        {
            urp::strip_nested_extra_body(&mut request.input);
        }
        inject_monoize_context(self.context.auth, &mut request);
        request.model = attempt.upstream_model.clone();
        self.apply_request_rules(
            &mut request,
            &attempt.provider_transforms,
            attempt.provider_type,
        )
        .await?;
        let global_transforms = self
            .context
            .state
            .monoize_runtime
            .read()
            .await
            .global_transforms
            .clone();
        self.apply_request_rules(&mut request, &global_transforms, attempt.provider_type)
            .await?;
        self.apply_request_rules(
            &mut request,
            &self.context.auth.transforms,
            attempt.provider_type,
        )
        .await?;
        strip_monoize_context(&mut request);
        let transform_chain = self
            .context
            .capture
            .session
            .as_ref()
            .map_or(Value::Null, |_| {
                crate::request_capture::build_transform_chain(
                    &attempt.provider_transforms,
                    &global_transforms,
                    &self.context.auth.transforms,
                    &self.transform_match_model,
                )
            });
        if attempt.provider_type == ProviderType::OpenrouterImage
            && urp::encode::openai_image::has_user_image_input(&request)
        {
            request.stream = Some(false);
        }
        Ok(PreparedImageAttempt {
            request,
            global_transforms,
            transform_chain,
        })
    }

    async fn apply_request_rules(
        &self,
        request: &mut urp::UrpRequest,
        rules: &[TransformRuleConfig],
        provider_type: ProviderType,
    ) -> AppResult<()> {
        apply_transform_rules_request(
            self.context.state,
            request,
            rules,
            &self.transform_match_model,
            Some(provider_type),
        )
        .await
    }

    pub(super) fn log_context(&self) -> RequestLogContext<'_> {
        RequestLogContext {
            model: &self.logical_model,
            is_stream: true,
            started_at: self.context.task_state.started_at(),
            request_id: self.context.meta.request_id.as_deref(),
            request_ip: self.context.meta.request_ip.as_deref(),
            reasoning_effort: self
                .request
                .reasoning
                .as_ref()
                .and_then(|r| r.effort.as_deref()),
        }
    }

    async fn update_pending(&self, attempt: &MonoizeAttempt) {
        update_pending_channel_info(
            self.context.state,
            self.context.auth,
            attempt,
            self.log_context(),
        )
        .await;
    }

    async fn retry_failure(
        &self,
        driver: &mut AttemptDriver,
        slot: AttemptSlot,
        error: AppError,
        class: Option<RetryableFailureClass>,
        emitted: bool,
    ) -> AppResult<()> {
        if emitted {
            driver
                .record_terminal_failure(self.context.state, &slot, &error, class)
                .await;
            return Err(self
                .finish_error(
                    &slot.attempt,
                    error,
                    std::mem::take(&mut driver.tried_providers),
                )
                .await);
        }
        driver.fail(self.context.state, slot, &error, class).await;
        Ok(())
    }

    pub(super) async fn finish_error(
        &self,
        attempt: &MonoizeAttempt,
        error: AppError,
        tried: Vec<TriedProvider>,
    ) -> AppError {
        self.log_error(Some(attempt), &error, tried);
        self.persist(None, false).await;
        error
    }

    fn log_error(
        &self,
        attempt: Option<&MonoizeAttempt>,
        error: &AppError,
        tried: Vec<TriedProvider>,
    ) {
        if let Some(attempt) = attempt {
            spawn_request_log_error(
                self.context.state,
                self.context.auth,
                attempt,
                self.log_context(),
                error,
                tried,
            );
        } else {
            spawn_request_log_error_no_attempt(
                self.context.state,
                self.context.auth,
                self.log_context(),
                error,
                tried,
            );
        }
    }

    async fn persist(&self, usage: Option<&urp::Usage>, failed: bool) {
        if let Some(session) = self.context.capture.session.as_ref() {
            session.persist_with_result(usage, failed).await;
        }
    }

    pub(super) async fn push_attempt(
        &self,
        slot: &AttemptSlot,
        capture: &ImageAttemptCapture<'_>,
        reconstructed: Option<Value>,
        error: Option<&AppError>,
    ) {
        let (Some(session), Some(body)) =
            (self.context.capture.session.as_ref(), capture.upstream_body)
        else {
            return;
        };
        let attempt = &slot.attempt;
        session.push_attempt(crate::request_capture::build_attempt_dump(crate::request_capture::AttemptDumpInput {
            attempt_number: slot.position.attempt_number,
            provider_id: &attempt.provider_id,
            channel_id: Some(&attempt.channel_id),
            provider_type: attempt.provider_type,
            logical_model: &self.logical_model,
            upstream_model: &capture.request.model,
            upstream_path: capture.path,
            raw_input: self.context.capture.raw_input.as_ref().clone(),
            transformed_urp_request: capture.request,
            upstream_request: body.clone(),
            downstream_response: None,
            reconstructed_urp_response: reconstructed,
            downstream_sse_frames: None,
            transform_chain: capture.transform_chain.clone(),
            error: error.map(|err| json!({ "message": err.message, "code": err.code, "status": err.status.as_u16() })),
        })).await;
    }

    async fn finish_success(
        &self,
        slot: &AttemptSlot,
        capture: &ImageAttemptCapture<'_>,
        response: urp::UrpResponse,
        metrics: Arc<Mutex<StreamRuntimeMetrics>>,
        reconstructed: Option<Value>,
        tried: Vec<TriedProvider>,
    ) -> AppResult<(urp::UrpResponse, String)> {
        if response.usage.is_none() && missing_usage_rejects(self.context.auth, &slot.attempt) {
            let error = missing_usage_error();
            self.push_attempt(slot, capture, reconstructed, Some(&error))
                .await;
            return Err(self.finish_error(&slot.attempt, error, tried).await);
        }
        self.push_attempt(slot, capture, reconstructed, None).await;
        mark_channel_success(self.context.state, &slot.attempt).await;
        let observed_response_model = metrics.lock().await.response_model.clone();
        let upstream_response_model = mismatched_upstream_response_model(
            &capture.request.model,
            observed_response_model.as_deref().unwrap_or(""),
        );
        refresh_channel_affinity(self.context.state, &slot.attempt).await;
        let charge = match maybe_charge_response(
            self.context.state,
            self.context.auth,
            &slot.attempt,
            &self.logical_model,
            &response,
            self.context.meta.request_id.as_deref(),
        )
        .await
        {
            Ok(charge) => charge,
            Err(error) => return Err(self.finish_error(&slot.attempt, error, tried).await),
        };
        spawn_request_log(
            self.context.state,
            self.context.auth,
            &slot.attempt,
            self.log_context(),
            RequestLogSuccess {
                usage: response.usage.clone(),
                charge_nano_usd: charge.charge_nano_usd,
                billing_breakdown_json: charge.billing_breakdown,
                ttfb_ms: None,
                stream_terminal_diagnostics: None,
                tried_providers: tried,
                client_gone: self.context.task_state.client_gone(),
                upstream_response_model,
            },
        );
        self.persist(response.usage.as_ref(), false).await;
        Ok((response, self.logical_model.clone()))
    }
}
