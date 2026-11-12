use super::*;
use crate::urp::stream_decode::stream_upstream_to_urp_events;

/// Upstream transport of one try.
#[derive(Clone, Copy)]
pub(super) enum Transport {
    /// The upstream streams and the reply is collected into one response.
    CollectedStream,
    /// RCD-D16 multipart OpenAI image edit with a JSON reply.
    MultipartEdit,
    /// JSON request with a JSON reply.
    Json,
}

/// The transport and upstream path of a transformed, encoded request.
pub(super) fn upstream_route(
    req: &urp::UrpRequest,
    provider_type: ProviderType,
) -> (Transport, String) {
    let openai_image_edit = provider_type == ProviderType::OpenaiImage
        && urp::encode::openai_image::has_user_image_input(req);
    let path = if openai_image_edit {
        "/v1/images/edits".to_string()
    } else {
        upstream_path_for_model(provider_type, &req.model, req.stream.unwrap_or(false))
    };
    let transport = if req.stream == Some(true)
        && supports_nonstream_upstream_stream_collection(provider_type)
    {
        Transport::CollectedStream
    } else if openai_image_edit && !urp::encode::openai_image::edit_requires_json(req) {
        Transport::MultipartEdit
    } else {
        Transport::Json
    };
    (transport, path)
}

fn supports_nonstream_upstream_stream_collection(provider_type: ProviderType) -> bool {
    matches!(
        provider_type,
        ProviderType::Responses | ProviderType::OpenaiImage | ProviderType::OpenrouterImage
    )
}

/// What the upstream exchange of one try produced, when it did not end the
/// request.
// Built once per try and moved straight to its handler; boxing the collected
// response would only add an allocation.
#[allow(clippy::large_enum_variant)]
pub(super) enum Exchange {
    Reply(Reply),
    /// Transport or HTTP failure; the capture dump records it.
    CallFailed(UpstreamCallError),
    /// Channel setup or collection failed without a completed reply.
    AttemptFailed(AppError),
}

/// A successful upstream reply.
#[allow(clippy::large_enum_variant)]
pub(super) enum Reply {
    /// A provider JSON body to decode.
    Body(Value),
    /// A stream-collected terminal response and the model the stream reported.
    Collected {
        response: urp::UrpResponse,
        observed_model: Option<String>,
    },
}

impl NonstreamRun<'_> {
    /// Sends the encoded body over the try's transport. Each transport
    /// consumes `upstream_body` before awaiting upstream, keeping it only for
    /// a capture session (FP6j-2). `Err` ends the request.
    pub(super) async fn send_upstream(
        &mut self,
        attempt: &mut MonoizeAttempt,
        call: &mut AttemptCall,
        upstream_body: Value,
    ) -> AppResult<Exchange> {
        let http = match client_http_for_attempt(self.inputs.state, attempt) {
            Ok(http) => http,
            Err(err) => {
                call.capture_upstream_request = self
                    .inputs
                    .capture
                    .session
                    .is_some()
                    .then_some(upstream_body);
                return Ok(Exchange::AttemptFailed(err));
            }
        };
        match call.transport {
            Transport::CollectedStream => {
                self.send_collected_stream(&http, attempt, call, upstream_body)
                    .await
            }
            Transport::MultipartEdit => {
                self.send_multipart_edit(&http, attempt, call, upstream_body)
                    .await
            }
            Transport::Json => self.send_json(&http, attempt, call, upstream_body).await,
        }
    }

    async fn send_collected_stream(
        &mut self,
        http: &reqwest::Client,
        attempt: &mut MonoizeAttempt,
        call: &mut AttemptCall,
        upstream_body: Value,
    ) -> AppResult<Exchange> {
        let state = self.inputs.state;
        let stream_idle_timeout_ms = state
            .monoize_runtime
            .read()
            .await
            .stream_idle_timeout_ms
            .max(1);
        let legacy = match typed_request_to_legacy(&call.req, self.inputs.max_multiplier) {
            Ok(legacy) => legacy,
            Err(err) => return Err(self.fail_request(attempt, err).await),
        };
        let pending_request_envelope_extra = find_pending_request_envelope_extra(&call.req.input);
        let extra_headers = attempt_extra_headers(attempt, &upstream_body);
        attempt.session_affinity_value = resolve_session_affinity_value(attempt, &upstream_body);
        // Streaming edits use the same JSON or multipart transport as
        // non-streaming edits.
        let stream_call = match call_streaming_image_capable_upstream(
            http,
            attempt,
            &call.req,
            upstream_body,
            attempt.request_timeout_ms.saturating_mul(10).max(600_000),
            &extra_headers,
            self.inputs.capture.session.is_some(),
        )
        .await
        {
            Ok(stream_call) => stream_call,
            Err(err) => return Err(self.fail_request(attempt, err).await),
        };
        call.path = stream_call.path;
        call.capture_upstream_request = stream_call.capture_upstream_request;
        let upstream_resp = match stream_call.result {
            Ok(upstream_resp) => upstream_resp,
            Err(err) => return Ok(Exchange::CallFailed(err)),
        };
        call.forwarded_headers = upstream::forwarded_response_headers(upstream_resp.headers());
        release_encoded_input(&mut call.req, &self.inputs.capture);
        let collected = collect_streamed_upstream_response(
            legacy,
            pending_request_envelope_extra,
            attempt.provider_type,
            upstream_resp,
            self.started_at,
            &self.logical_model,
            stream_idle_timeout_ms,
        )
        .await;
        match collected {
            Ok((response, observed_model)) => Ok(Exchange::Reply(Reply::Collected {
                response,
                observed_model,
            })),
            Err(CollectedUpstreamError::Internal(err)) => {
                self.capture_failure(attempt, call, &err).await;
                Err(self.fail_request(attempt, err).await)
            }
            Err(CollectedUpstreamError::Upstream(mut err)) => {
                err.forwarded_headers = std::mem::take(&mut call.forwarded_headers);
                Ok(Exchange::AttemptFailed(err))
            }
        }
    }

    async fn send_multipart_edit(
        &mut self,
        http: &reqwest::Client,
        attempt: &mut MonoizeAttempt,
        call: &mut AttemptCall,
        upstream_body: Value,
    ) -> AppResult<Exchange> {
        let capture_active = self.inputs.capture.session.is_some();
        let form = urp::encode::openai_image::multipart_fields(&call.req, &call.req.model)
            .and_then(|fields| {
                // RCD-D6a: capture the multipart body from the same field
                // list the sent form is built from.
                if capture_active {
                    call.capture_upstream_request = Some(
                        crate::request_capture::multipart_capture_object_from_upstream_fields(
                            &fields,
                        ),
                    );
                }
                urp::encode::openai_image::form_from_fields(fields)
            });
        let form = match form {
            Ok(form) => form,
            Err(message) => {
                let err = AppError::new(StatusCode::BAD_REQUEST, "invalid_request", message);
                return Err(self.fail_request(attempt, err).await);
            }
        };
        release_encoded_input(&mut call.req, &self.inputs.capture);
        let extra_headers = attempt_extra_headers(attempt, &upstream_body);
        attempt.session_affinity_value = resolve_session_affinity_value(attempt, &upstream_body);
        drop(upstream_body);
        let provider = build_channel_provider_config(attempt);
        let result = upstream::call_upstream_multipart_json_with_timeout_and_headers(
            http,
            &provider,
            &attempt.api_key,
            &call.path,
            form,
            attempt.request_timeout_ms,
            &extra_headers,
        )
        .await;
        Ok(match result {
            Ok(value) => Exchange::Reply(Reply::Body(value)),
            Err(err) => Exchange::CallFailed(err),
        })
    }

    async fn send_json(
        &self,
        http: &reqwest::Client,
        attempt: &mut MonoizeAttempt,
        call: &mut AttemptCall,
        upstream_body: Value,
    ) -> AppResult<Exchange> {
        release_encoded_input(&mut call.req, &self.inputs.capture);
        let extra_headers = attempt_extra_headers(attempt, &upstream_body);
        attempt.session_affinity_value = resolve_session_affinity_value(attempt, &upstream_body);
        let body = upstream::JsonBody::new(&upstream_body);
        call.capture_upstream_request = self
            .inputs
            .capture
            .session
            .is_some()
            .then_some(upstream_body);
        let provider = build_channel_provider_config(attempt);
        let result = upstream::call_upstream_json_with_response_headers(
            http,
            &provider,
            &attempt.api_key,
            &call.path,
            body,
            attempt.request_timeout_ms,
            &extra_headers,
        )
        .await;
        Ok(match result {
            Ok((value, headers)) => {
                call.forwarded_headers = headers;
                Exchange::Reply(Reply::Body(value))
            }
            Err(err) => Exchange::CallFailed(err),
        })
    }
}

enum CollectedUpstreamError {
    Internal(AppError),
    Upstream(AppError),
}

async fn collect_streamed_upstream_response(
    legacy: UrpRequest,
    pending_request_envelope_extra: Option<HashMap<String, Value>>,
    provider_type: ProviderType,
    upstream_resp: reqwest::Response,
    started_at: std::time::Instant,
    logical_model: &str,
    stream_idle_timeout_ms: u64,
) -> Result<(urp::UrpResponse, Option<String>), CollectedUpstreamError> {
    let (decoded_tx, mut decoded_rx) = mpsc::channel::<crate::urp::UrpStreamEvent>(64);
    let runtime_metrics = Arc::new(Mutex::new(StreamRuntimeMetrics::default()));
    let decode_handle = {
        let runtime_metrics = runtime_metrics.clone();
        tokio::spawn(async move {
            stream_upstream_to_urp_events(
                &legacy,
                pending_request_envelope_extra,
                provider_type,
                upstream_resp,
                decoded_tx,
                crate::urp::stream_decode::StreamDecodeContext {
                    started_at: Some(started_at),
                    runtime_metrics: Some(runtime_metrics),
                    idle_timeout_ms: stream_idle_timeout_ms,
                },
            )
            .await
        })
    };

    let mut final_response: Option<urp::UrpResponse> = None;
    let mut stream_error: Option<AppError> = None;
    while let Some(event) = decoded_rx.recv().await {
        match event {
            crate::urp::UrpStreamEvent::ResponseDone {
                outcome,
                finish_reason,
                usage,
                output,
                extra_body,
            } => {
                final_response = Some(urp::UrpResponse {
                    outcome,
                    id: extra_body
                        .get("id")
                        .and_then(|value| value.as_str())
                        .unwrap_or("resp_stream_collected")
                        .to_string(),
                    model: extra_body
                        .get("model")
                        .and_then(|value| value.as_str())
                        .unwrap_or(logical_model)
                        .to_string(),
                    created_at: extra_body
                        .get("created_at")
                        .and_then(|value| value.as_i64()),
                    output: urp::TerminalNode::into_nodes(output),
                    finish_reason,
                    usage,
                    extra_body,
                });
            }
            crate::urp::UrpStreamEvent::Error { code, message, .. } => {
                stream_error = Some(AppError::new(
                    StatusCode::BAD_GATEWAY,
                    code.unwrap_or_else(|| "upstream_stream_error".to_string()),
                    message,
                ));
            }
            _ => {}
        }
    }
    decode_handle
        .await
        .map_err(|e| {
            CollectedUpstreamError::Internal(AppError::new(
                StatusCode::INTERNAL_SERVER_ERROR,
                "task_panic",
                e.to_string(),
            ))
        })?
        .map_err(CollectedUpstreamError::Upstream)?;
    if let Some(err) = stream_error {
        return Err(CollectedUpstreamError::Upstream(err));
    }
    let observed_response_model = runtime_metrics.lock().await.response_model.clone();
    final_response
        .map(|response| (response, observed_response_model))
        .ok_or_else(|| {
            CollectedUpstreamError::Upstream(AppError::new(
                StatusCode::BAD_GATEWAY,
                "upstream_stream_error",
                "stream completed without terminal response",
            ))
        })
}
