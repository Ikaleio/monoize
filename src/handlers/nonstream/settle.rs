use super::*;

impl NonstreamRun<'_> {
    /// Settles a decoded response: MP-F3 gate, Channel success, the PIPE-1
    /// response phase, then billing and the success records. `Err` ends the
    /// request.
    pub(super) async fn settle<T>(
        &mut self,
        attempt: &MonoizeAttempt,
        mut call: AttemptCall,
        mut resp: urp::UrpResponse,
        upstream_response_model: Option<String>,
        accept: &impl Fn(&urp::UrpResponse, &str) -> AppResult<T>,
    ) -> AppResult<Served<T>> {
        // MP-F3: a fail-closed missing-usage billable success rejects with
        // 403 before response delivery.
        if resp.usage.is_none() && missing_usage_rejects(self.inputs.auth, attempt) {
            return Err(self.fail_request(attempt, missing_usage_error()).await);
        }
        mark_channel_success(self.inputs.state, attempt).await;
        refresh_channel_affinity(self.inputs.state, attempt).await;
        let (accepted, history) = self
            .finish_response(attempt, &mut call, &mut resp, accept)
            .await?;
        self.record_success(
            attempt,
            call.forwarded_headers,
            resp,
            accepted,
            history,
            upstream_response_model,
        )
        .await
    }

    /// PIPE-1 response phase: XTRA-5a foreign-state strip, mz2 envelopes,
    /// response transforms, Responses decoration, and the caller's `accept`.
    /// Returns the accepted value and the history context to retain.
    async fn finish_response<T>(
        &mut self,
        attempt: &MonoizeAttempt,
        call: &mut AttemptCall,
        resp: &mut urp::UrpResponse,
        accept: &impl Fn(&urp::UrpResponse, &str) -> AppResult<T>,
    ) -> AppResult<(T, Option<responses_history::HistoryContext>)> {
        let auth = self.inputs.auth;
        let downstream = self.inputs.downstream;
        let strip_foreign_response = self.strip_foreign_response;
        let foreign_extras = foreign_response_target(downstream, attempt.provider_type)
            .filter(|_| strip_foreign_response)
            .map(|target| urp::strip_foreign_response(resp, target));
        // Wrap newly produced encrypted reasoning payloads in mz2 envelopes
        // BEFORE any response-phase transform observes the response. Per
        // spec/urp-transform-system.spec.md PIPE-1 step 12 and PIPE-1d,
        // transforms must only see encrypted reasoning in `mz2.` envelope
        // form so that bulk-mutation transforms (e.g.
        // reasoning_strip_encrypted) can reason about that single canonical
        // surface.
        if auth.reasoning_envelope_enabled {
            urp::wrap_reasoning_envelopes_in_response(
                resp,
                reasoning_envelope_provider_type(attempt.provider_type),
                &call.req.model,
            );
        }
        self.apply_response_rules(attempt, resp, &attempt.provider_transforms)
            .await?;
        self.apply_response_rules(attempt, resp, &call.global_transforms)
            .await?;
        self.apply_response_rules(attempt, resp, &auth.transforms)
            .await?;
        if matches!(
            attempt.provider_type,
            ProviderType::OpenaiImage | ProviderType::OpenrouterImage
        ) && !matches!(downstream, DownstreamProtocol::Responses)
        {
            convert_assistant_images_to_markdown(resp);
        }
        let history = self
            .history_context
            .take()
            .map(|history| history.with_resource_scope(media_resource_scope(attempt)));
        if let Some(history) = &history {
            history.decorate_response(resp);
        }
        if let Some(echo) = call.responses_echo.take() {
            resp.extra_body
                .insert(urp::RESPONSES_REQUEST_ECHO_EXTRA_KEY.to_string(), echo);
        }
        if strip_foreign_response {
            let mask_sensitive_info = self
                .inputs
                .state
                .monoize_runtime
                .read()
                .await
                .mask_sensitive_info;
            normalize_outcome_error(
                &mut resp.outcome,
                downstream,
                self.inputs.request_id.as_deref(),
                mask_sensitive_info,
            );
        }
        let accepted = match accept(resp, &self.logical_model) {
            Ok(accepted) => accepted,
            Err(err) => return Err(self.fail_request(attempt, err).await),
        };
        if let Some(foreign_extras) = foreign_extras {
            foreign_extras.restore(resp);
        }
        Ok((accepted, history))
    }

    async fn apply_response_rules(
        &mut self,
        attempt: &MonoizeAttempt,
        resp: &mut urp::UrpResponse,
        rules: &[TransformRuleConfig],
    ) -> AppResult<()> {
        let applied = apply_transform_rules_response(
            self.inputs.state,
            resp,
            rules,
            &self.logical_model,
            Some(attempt.provider_type),
        )
        .await;
        match applied {
            Ok(()) => Ok(()),
            Err(err) => Err(self.fail_request(attempt, err).await),
        }
    }

    /// Billing, the success request log, capture persist, response history,
    /// and the SE1d headers of the serving attempt.
    async fn record_success<T>(
        &mut self,
        attempt: &MonoizeAttempt,
        mut forwarded_headers: crate::error::ForwardedHeaders,
        resp: urp::UrpResponse,
        accepted: T,
        history: Option<responses_history::HistoryContext>,
        upstream_response_model: Option<String>,
    ) -> AppResult<Served<T>> {
        let state = self.inputs.state;
        let auth = self.inputs.auth;
        let charged = maybe_charge_response(
            state,
            auth,
            attempt,
            &self.logical_model,
            &resp,
            self.inputs.request_id.as_deref(),
        )
        .await;
        let charge = match charged {
            Ok(charge) => charge,
            Err(err) => return Err(self.fail_request(attempt, err).await),
        };
        spawn_request_log(
            state,
            auth,
            attempt,
            RequestLogContext {
                model: &self.logical_model,
                is_stream: false,
                started_at: self.started_at,
                request_id: self.inputs.request_id.as_deref(),
                request_ip: self.inputs.request_ip.as_deref(),
                reasoning_effort: self.reasoning_effort.as_deref(),
            },
            RequestLogSuccess {
                usage: resp.usage.clone(),
                charge_nano_usd: charge.charge_nano_usd,
                billing_breakdown_json: charge.billing_breakdown,
                ttfb_ms: None,
                stream_terminal_diagnostics: None,
                tried_providers: std::mem::take(&mut self.attempts.tried_providers),
                client_gone: self
                    .inputs
                    .task_state
                    .is_some_and(AdmittedRequestTaskState::client_gone),
                upstream_response_model,
            },
        );
        if let Some(session) = self.inputs.capture.session.as_ref() {
            session
                .persist_with_result(resp.usage.as_ref(), false)
                .await;
        }
        if let Some(history) = history {
            history.retain_response(&resp).await;
        }
        if !self.inputs.downstream.is_same_family(attempt.provider_type) {
            forwarded_headers.clear();
        }
        Ok(Served {
            resp,
            accepted,
            forwarded_headers,
        })
    }
}
