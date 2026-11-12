use super::exchange::Exchange;
use super::*;

impl<'a> NonstreamRun<'a> {
    /// Routing, balance admission, and the pending request log. Every error
    /// here precedes the first upstream attempt and logs nothing.
    pub(super) async fn admit(
        inputs: NonstreamInputs<'a>,
        mut req: urp::UrpRequest,
        client_session_id: Option<String>,
    ) -> AppResult<(Self, Vec<MonoizeAttempt>)> {
        let state = inputs.state;
        let auth = inputs.auth;
        let started_at = inputs
            .task_state
            .map(AdmittedRequestTaskState::started_at)
            .unwrap_or_else(std::time::Instant::now);
        let transform_match_model = resolve_model_suffix(state, &mut req).await?;
        let logical_model = req.model.clone();
        let reasoning_effort = req.reasoning.as_ref().and_then(|r| r.effort.clone());
        let routing_stub = build_routing_stub(&req, inputs.max_multiplier);
        let mut attempts = build_monoize_attempts(state, &routing_stub, auth).await?;
        // Session affinity derives from the request as decoded, before media
        // routes are bound into its input nodes.
        attach_client_session_id(&mut attempts, client_session_id, Some(&req));
        bind_media_request_routes(&mut req, &mut attempts)?;
        let history_context =
            responses_history::HistoryContext::from_request(state, &req, inputs.downstream);
        ensure_balance_before_forward_for_attempts(state, auth, &attempts).await?;
        let pending_request_log_guard = insert_pending_request_log(
            state,
            auth,
            &logical_model,
            false,
            inputs.request_id.as_deref(),
            inputs.request_ip.as_deref(),
            started_at,
        )
        .await?;
        let pending_request_log_guard = match inputs.task_state {
            Some(task_state) => {
                task_state.retain_pending_guard(pending_request_log_guard);
                None
            }
            None => pending_request_log_guard,
        };
        let run = Self {
            inputs,
            started_at,
            logical_model,
            transform_match_model,
            reasoning_effort,
            _pending_request_log_guard: pending_request_log_guard,
            // The suffix-normalized request is the pristine base every attempt
            // re-derives its transformed request from. This matters because the
            // cross-family strip runs BEFORE all transforms per attempt
            // (auto_cache_* etc. must observe the stripped request so their
            // cache breakpoints actually survive into the upstream encoding).
            original_req: Some(req),
            history_context,
            tried_providers: Vec::new(),
            last_failed_attempt: None,
            execution_state: AttemptExecutionState::default(),
        };
        Ok((run, attempts))
    }

    /// RTA-4: the same-Channel tries of one routed attempt. `Ok(None)` moves
    /// on to the next attempt; `Err` ends the request.
    pub(super) async fn serve_attempt<T>(
        &mut self,
        mut attempt: MonoizeAttempt,
        last_attempt: bool,
        accept: &impl Fn(&urp::UrpResponse, &str) -> AppResult<T>,
    ) -> AppResult<Option<Served<T>>> {
        if self.execution_state.should_skip(&attempt) {
            return Ok(None);
        }
        let max_channel_attempts = same_channel_attempt_slots(&attempt);
        for channel_attempt in 0..max_channel_attempts {
            if self.execution_state.should_skip(&attempt) {
                break;
            }
            let final_slot = last_attempt && channel_attempt + 1 == max_channel_attempts;
            let outcome = self
                .try_channel(&mut attempt, channel_attempt + 1, final_slot, accept)
                .await?;
            let retry_same_channel = match outcome {
                TryOutcome::Served(served) => return Ok(Some(served)),
                TryOutcome::Failed { retry_same_channel } => retry_same_channel,
            };
            if !retry_same_channel {
                break;
            }
            maybe_sleep_before_channel_retry(&attempt).await;
        }
        Ok(None)
    }

    /// One upstream try on the attempt's Channel. `Err` ends the request.
    async fn try_channel<T>(
        &mut self,
        attempt: &mut MonoizeAttempt,
        channel_attempts_used: usize,
        final_slot: bool,
        accept: &impl Fn(&urp::UrpResponse, &str) -> AppResult<T>,
    ) -> AppResult<TryOutcome<T>> {
        let slot = TrySlot {
            attempt_number: self.execution_state.record_upstream_attempt(attempt),
            channel_attempts_used,
        };
        if let Some(task_state) = self.inputs.task_state {
            task_state.set_attempt(attempt);
        }
        // Derive from the pristine request (pre-transforms) so that the
        // cross-family strip can run BEFORE provider, global, and API-key
        // transforms. This guarantees that transforms which inject
        // upstream-specific part-level metadata (e.g.
        // `cache_anthropic_system`, `cache_anthropic_tool_use`) survive into
        // the encoded upstream request even when the downstream and
        // upstream protocol families differ.
        let req = attempt_request_from_original(&mut self.original_req, final_slot);
        let (mut call, upstream_body) = self.prepare_call(attempt, slot, req).await?;
        let exchange = self
            .send_upstream(attempt, &mut call, upstream_body)
            .await?;
        let retry_same_channel = match exchange {
            Exchange::Reply(reply) => return self.handle_reply(attempt, call, reply, accept).await,
            Exchange::CallFailed(err) => self.handle_call_failure(attempt, call, err).await,
            Exchange::CollectFailed(err) => {
                let passive_failure_class = app_failure_class(&err);
                self.record_failure(attempt, slot, &err, passive_failure_class)
                    .await
            }
        };
        Ok(TryOutcome::Failed { retry_same_channel })
    }

    /// Records a failed try and reports whether the same Channel may serve
    /// another one (RTA-4/RTA-5).
    pub(super) async fn record_failure(
        &mut self,
        attempt: &MonoizeAttempt,
        slot: TrySlot,
        err: &AppError,
        passive_failure_class: Option<RetryableFailureClass>,
    ) -> bool {
        let state = self.inputs.state;
        record_upstream_attempt_failure(
            state,
            attempt,
            slot.attempt_number,
            err,
            passive_failure_class,
            &mut self.tried_providers,
            &mut self.execution_state,
        )
        .await;
        self.last_failed_attempt = Some(attempt.clone());
        allow_same_channel_retry(
            state,
            attempt,
            &self.execution_state,
            slot.channel_attempts_used,
            passive_failure_class,
        )
        .await
    }

    /// Ends the request with `error`: error request log, then capture persist.
    pub(super) async fn fail_request(
        &mut self,
        attempt: &MonoizeAttempt,
        error: AppError,
    ) -> AppError {
        spawn_request_log_error(
            self.inputs.state,
            self.inputs.auth,
            attempt,
            &self.logical_model,
            false,
            self.started_at,
            self.inputs.request_id.clone(),
            self.inputs.request_ip.clone(),
            &error,
            self.reasoning_effort.clone(),
            std::mem::take(&mut self.tried_providers),
        );
        if let Some(session) = self.inputs.capture.session.as_ref() {
            session.persist_with_result(None, false).await;
        }
        error
    }

    /// RTA-8: every attempt failed or was skipped.
    pub(super) async fn finish_exhausted(self) -> AppError {
        let final_err = build_exhausted_error_for_downstream(
            &self.logical_model,
            &self.tried_providers,
            self.last_failed_attempt.as_ref(),
            self.inputs.downstream,
        );
        if let Some(attempt) = self.last_failed_attempt {
            spawn_request_log_error(
                self.inputs.state,
                self.inputs.auth,
                &attempt,
                &self.logical_model,
                false,
                self.started_at,
                self.inputs.request_id,
                self.inputs.request_ip,
                &final_err,
                self.reasoning_effort,
                self.tried_providers,
            );
        } else {
            spawn_request_log_error_no_attempt(
                self.inputs.state,
                self.inputs.auth,
                &self.logical_model,
                false,
                self.started_at,
                self.inputs.request_id,
                self.inputs.request_ip,
                &final_err,
                self.reasoning_effort,
                self.tried_providers,
            );
        }
        if let Some(session) = self.inputs.capture.session.as_ref() {
            session.persist_with_result(None, true).await;
        }
        final_err
    }
}

/// Same-Channel retry class of a failure reported as an `AppError`.
pub(super) fn app_failure_class(err: &AppError) -> Option<RetryableFailureClass> {
    is_same_channel_retryable_app_error(err).then(|| classify_retryable_app_failure(err))
}
