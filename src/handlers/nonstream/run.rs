use super::exchange::Exchange;
use super::*;

impl<'a> NonstreamRun<'a> {
    /// Routing, balance admission, and the pending request log. Every error
    /// here precedes the first upstream attempt and logs nothing.
    pub(super) async fn admit(
        inputs: NonstreamRequest<'a>,
        mut req: urp::UrpRequest,
        client_session_id: Option<String>,
        strip_foreign_response: bool,
    ) -> AppResult<Self> {
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
            strip_foreign_response,
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
            attempts: AttemptDriver::new(attempts),
        };
        Ok(run)
    }

    /// One upstream try on the attempt's Channel. `Err` ends the request.
    pub(super) async fn try_channel<T>(
        &mut self,
        attempt: &mut MonoizeAttempt,
        slot: AttemptPosition,
        accept: &impl Fn(&urp::UrpResponse, &str) -> AppResult<T>,
    ) -> AppResult<TryOutcome<T>> {
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
        let req = attempt_request_from_original(&mut self.original_req, slot.final_slot);
        let (mut call, upstream_body) = self.prepare_call(attempt, slot, req).await?;
        let exchange = self
            .send_upstream(attempt, &mut call, upstream_body)
            .await?;
        match exchange {
            Exchange::Reply(reply) => return self.handle_reply(attempt, call, reply, accept).await,
            Exchange::CallFailed(err) => Ok(self.handle_call_failure(attempt, call, err).await),
            Exchange::AttemptFailed(err) => {
                self.capture_failure(attempt, &mut call, &err).await;
                Ok(TryOutcome::Failed {
                    failure_class: app_failure_class(&err),
                    error: err,
                })
            }
        }
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
            RequestLogContext {
                model: &self.logical_model,
                is_stream: false,
                started_at: self.started_at,
                request_id: self.inputs.request_id.as_deref(),
                request_ip: self.inputs.request_ip.as_deref(),
                reasoning_effort: self.reasoning_effort.as_deref(),
            },
            &error,
            std::mem::take(&mut self.attempts.tried_providers),
        );
        if let Some(session) = self.inputs.capture.session.as_ref() {
            session.persist_with_result(None, false).await;
        }
        error
    }

    /// RTA-8: every attempt failed or was skipped.
    pub(super) async fn finish_exhausted(self) -> AppError {
        let final_err = self
            .attempts
            .exhausted_error(&self.logical_model, self.inputs.downstream);
        if let Some(attempt) = self.attempts.last_failed_attempt {
            spawn_request_log_error(
                self.inputs.state,
                self.inputs.auth,
                &attempt,
                RequestLogContext {
                    model: &self.logical_model,
                    is_stream: false,
                    started_at: self.started_at,
                    request_id: (self.inputs.request_id).as_deref(),
                    request_ip: (self.inputs.request_ip).as_deref(),
                    reasoning_effort: (self.reasoning_effort).as_deref(),
                },
                &final_err,
                self.attempts.tried_providers,
            );
        } else {
            spawn_request_log_error_no_attempt(
                self.inputs.state,
                self.inputs.auth,
                RequestLogContext {
                    model: &self.logical_model,
                    is_stream: false,
                    started_at: self.started_at,
                    request_id: (self.inputs.request_id).as_deref(),
                    request_ip: (self.inputs.request_ip).as_deref(),
                    reasoning_effort: (self.reasoning_effort).as_deref(),
                },
                &final_err,
                self.attempts.tried_providers,
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
