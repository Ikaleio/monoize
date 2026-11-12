use super::exchange::Reply;
use super::run::app_failure_class;
use super::*;

impl NonstreamRun<'_> {
    /// A reply arrived: capture dump, pending-log Channel info, then
    /// decoding. A decoded response is settled; a decode failure is a failed
    /// try. `Err` ends the request.
    pub(super) async fn handle_reply<T>(
        &mut self,
        attempt: &MonoizeAttempt,
        mut call: AttemptCall,
        reply: Reply,
        accept: &impl Fn(&urp::UrpResponse, &str) -> AppResult<T>,
    ) -> AppResult<TryOutcome<T>> {
        self.capture_reply(attempt, &mut call, &reply).await;
        update_pending_channel_info(
            self.inputs.state,
            self.inputs.auth,
            attempt,
            RequestLogContext {
                model: &self.logical_model,
                is_stream: false,
                started_at: self.started_at,
                request_id: self.inputs.request_id.as_deref(),
                request_ip: self.inputs.request_ip.as_deref(),
                reasoning_effort: None,
            },
        )
        .await;
        let (resp, observed_response_model) = match self.decode_reply(attempt, &call, reply).await {
            Ok(decoded) => decoded,
            Err(mut err) => {
                err.forwarded_headers = std::mem::take(&mut call.forwarded_headers);
                let passive_failure_class = app_failure_class(&err);
                return Ok(TryOutcome::Failed {
                    error: err,
                    failure_class: passive_failure_class,
                });
            }
        };
        // A decoded response ends retry processing (FP6j-2).
        drop(self.original_req.take());
        let upstream_response_model =
            mismatched_upstream_response_model(&call.req.model, &observed_response_model);
        self.settle(attempt, call, resp, upstream_response_model, accept)
            .await
            .map(|served| TryOutcome::Served(Box::new(served)))
    }

    /// The decoded response and the model the upstream reported for it.
    async fn decode_reply(
        &self,
        attempt: &MonoizeAttempt,
        call: &AttemptCall,
        reply: Reply,
    ) -> AppResult<(urp::UrpResponse, String)> {
        match reply {
            Reply::Collected {
                response,
                observed_model,
            } => Ok((response, observed_model.unwrap_or_default())),
            Reply::Body(body) => {
                let mask_sensitive_info = self
                    .inputs
                    .state
                    .monoize_runtime
                    .read()
                    .await
                    .mask_sensitive_info;
                let resp = decode_response_from_provider(
                    attempt.provider_type,
                    &body,
                    &call.req.model,
                    mask_sensitive_info,
                    &call.req,
                )?;
                let observed_response_model = body
                    .get("model")
                    .and_then(Value::as_str)
                    .filter(|model| !model.trim().is_empty())
                    .map(ToOwned::to_owned)
                    .unwrap_or_else(|| resp.model.clone());
                Ok((resp, observed_response_model))
            }
        }
    }

    /// A transport or HTTP failure: captures the error and returns its
    /// routing classification to the shared driver.
    pub(super) async fn handle_call_failure<T>(
        &mut self,
        attempt: &MonoizeAttempt,
        mut call: AttemptCall,
        err: UpstreamCallError,
    ) -> TryOutcome<T> {
        if let (Some(session), Some(upstream_request)) = (
            self.inputs.capture.session.as_ref(),
            call.capture_upstream_request.take(),
        ) {
            let error = json!({
                "message": err.message,
                "code": err.code,
                "status": err.status.map(|status| status.as_u16()),
            });
            let dump = self.attempt_dump(attempt, &call, upstream_request, None, None, Some(error));
            session.push_attempt(dump).await;
        }
        let passive_failure_class =
            is_same_channel_retryable_error(&err).then(|| classify_retryable_failure(&err));
        let mask_sensitive_info = self
            .inputs
            .state
            .monoize_runtime
            .read()
            .await
            .mask_sensitive_info;
        let app_err = upstream_error_to_app(err, mask_sensitive_info);
        TryOutcome::Failed {
            error: app_err,
            failure_class: passive_failure_class,
        }
    }

    pub(super) async fn capture_failure(
        &self,
        attempt: &MonoizeAttempt,
        call: &mut AttemptCall,
        err: &AppError,
    ) {
        if let (Some(session), Some(upstream_request)) = (
            self.inputs.capture.session.as_ref(),
            call.capture_upstream_request.take(),
        ) {
            let error = json!({
                "message": err.message,
                "code": err.code,
                "status": err.upstream_status.unwrap_or(err.status.as_u16()),
            });
            let dump = self.attempt_dump(attempt, call, upstream_request, None, None, Some(error));
            session.push_attempt(dump).await;
        }
    }

    /// RCD-D6 dump of a try whose upstream replied.
    async fn capture_reply(&self, attempt: &MonoizeAttempt, call: &mut AttemptCall, reply: &Reply) {
        let (Some(session), Some(upstream_request)) = (
            self.inputs.capture.session.as_ref(),
            call.capture_upstream_request.take(),
        ) else {
            return;
        };
        let (upstream_response, reconstructed_urp_response) = match reply {
            Reply::Body(body) => (Some(body.clone()), None),
            // RCD-D10c: a stream-collected attempt has no provider JSON body,
            // so the pre-response-transform collected terminal event
            // substitutes for it.
            Reply::Collected { response, .. } => (
                None,
                Some(crate::request_capture::reconstructed_response_json(
                    response,
                )),
            ),
        };
        let dump = self.attempt_dump(
            attempt,
            call,
            upstream_request,
            upstream_response,
            reconstructed_urp_response,
            None,
        );
        session.push_attempt(dump).await;
    }

    fn attempt_dump(
        &self,
        attempt: &MonoizeAttempt,
        call: &AttemptCall,
        upstream_request: Value,
        upstream_response: Option<Value>,
        reconstructed_urp_response: Option<Value>,
        error: Option<Value>,
    ) -> Value {
        crate::request_capture::build_attempt_dump(crate::request_capture::AttemptDumpInput {
            attempt_number: call.slot.attempt_number,
            provider_id: &attempt.provider_id,
            channel_id: Some(&attempt.channel_id),
            provider_type: attempt.provider_type,
            logical_model: &self.logical_model,
            upstream_model: &call.req.model,
            upstream_path: &call.path,
            raw_input: self.inputs.capture.raw_input.as_ref().clone(),
            transformed_urp_request: &call.req,
            upstream_request,
            downstream_response: upstream_response,
            reconstructed_urp_response,
            downstream_sse_frames: None,
            transform_chain: call.capture_transform_chain.clone(),
            error,
        })
    }
}
