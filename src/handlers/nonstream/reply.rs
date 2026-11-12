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
            &self.logical_model,
            false,
            self.inputs.request_id.as_deref(),
            self.inputs.request_ip.as_deref(),
            self.started_at,
        )
        .await;
        let (resp, observed_response_model) = match self.decode_reply(attempt, &call, reply).await {
            Ok(decoded) => decoded,
            Err(mut err) => {
                err.forwarded_headers = std::mem::take(&mut call.forwarded_headers);
                let passive_failure_class = app_failure_class(&err);
                let retry_same_channel = self
                    .record_failure(attempt, call.slot, &err, passive_failure_class)
                    .await;
                return Ok(TryOutcome::Failed { retry_same_channel });
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

    /// A transport or HTTP failure: RCD-D6 error dump, then the failed-try
    /// bookkeeping. Returns whether the same Channel may serve another try.
    pub(super) async fn handle_call_failure(
        &mut self,
        attempt: &MonoizeAttempt,
        mut call: AttemptCall,
        err: UpstreamCallError,
    ) -> bool {
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
        self.record_failure(attempt, call.slot, &app_err, passive_failure_class)
            .await
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
        crate::request_capture::build_attempt_dump(
            call.slot.attempt_number,
            &attempt.provider_id,
            Some(&attempt.channel_id),
            attempt.provider_type,
            &self.logical_model,
            &call.req.model,
            &call.path,
            self.inputs.capture.raw_input.as_ref().clone(),
            &call.req,
            upstream_request,
            upstream_response,
            reconstructed_urp_response,
            None,
            call.capture_transform_chain.clone(),
            error,
        )
    }
}
