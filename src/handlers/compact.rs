use super::*;
use crate::monoize_routing::MonoizeModelEntry;

const OPENAI_COMPACT_SUFFIX: &str = "-openai-compact";

pub(crate) fn classify_openai_compact_scheme(models: &[String]) -> (&'static str, Vec<String>) {
    let set: HashSet<&str> = models.iter().map(String::as_str).collect();
    let mut compact_models: Vec<String> = models
        .iter()
        .filter(|id| {
            id.strip_suffix(OPENAI_COMPACT_SUFFIX)
                .is_some_and(|base| !base.is_empty() && set.contains(base))
        })
        .cloned()
        .collect();
    compact_models.sort();
    let scheme = if compact_models.is_empty() {
        "same_model"
    } else {
        "openai_compact_sibling"
    };
    (scheme, compact_models)
}

fn compact_wire_model(
    models: &HashMap<String, MonoizeModelEntry>,
    logical_model: &str,
    selected_upstream: &str,
) -> String {
    let mut bases = Vec::new();
    if !logical_model.is_empty() {
        bases.push(logical_model);
    }
    if selected_upstream != logical_model && !selected_upstream.is_empty() {
        bases.push(selected_upstream);
    }
    for base in bases {
        let sibling = if base.ends_with(OPENAI_COMPACT_SUFFIX) {
            base.to_string()
        } else {
            format!("{base}{OPENAI_COMPACT_SUFFIX}")
        };
        if let Some(entry) = models.get(&sibling) {
            return super::routing::resolve_upstream_model(&sibling, entry);
        }
    }
    selected_upstream.to_string()
}

struct CompactRequest {
    auth: crate::auth::AuthResult,
    logical_model: String,
    body: Value,
    routing_request: urp::UrpRequest,
    request_id: Option<String>,
    request_ip: Option<String>,
    started_at: Instant,
    capture: RequestCaptureContext,
}

struct CompactExecution<'a> {
    state: &'a AppState,
    request: CompactRequest,
    channel_models: HashMap<String, HashMap<String, MonoizeModelEntry>>,
}

struct CompactCall {
    wire_model: String,
    capture_body: Option<Value>,
    result: Result<Value, upstream::UpstreamCallError>,
}

pub async fn compact_response(
    State(state): State<AppState>,
    headers: HeaderMap,
    Json(body): Json<Value>,
) -> AppResult<Response> {
    let (request, attempts) = prepare_compact_request(&state, &headers, body).await?;
    let _pending_request_log_guard = insert_pending_request_log(
        &state,
        &request.auth,
        &request.logical_model,
        false,
        request.request_id.as_deref(),
        request.request_ip.as_deref(),
        request.started_at,
    )
    .await?;
    let channel_models =
        load_compact_channel_models(&state, &request.logical_model, &attempts).await;
    let execution = CompactExecution {
        state: &state,
        request,
        channel_models,
    };
    let mut driver = AttemptDriver::new(attempts);
    while let Some(mut slot) = driver.next() {
        let mut call = match execution.call(&mut slot.attempt).await {
            Ok(call) => call,
            Err(error) => {
                driver.fail(&state, slot, &error, None).await;
                continue;
            }
        };
        execution.capture_attempt(&slot, &mut call).await;
        match call.result {
            Ok(value) => {
                return execution
                    .accept(
                        &slot.attempt,
                        &call.wire_model,
                        value,
                        driver.tried_providers,
                    )
                    .await;
            }
            Err(error) => {
                let failure_class = is_same_channel_retryable_error(&error)
                    .then(|| classify_retryable_failure(&error));
                let mask = state.monoize_runtime.read().await.mask_sensitive_info;
                let error = upstream_error_to_app(error, mask);
                driver.fail(&state, slot, &error, failure_class).await;
            }
        }
    }
    let error =
        build_exhausted_upstream_error(&execution.request.logical_model, &driver.tried_providers);
    execution.log_error(
        driver.last_failed_attempt.as_ref(),
        &error,
        driver.tried_providers,
    );
    execution.persist(None, true).await;
    Err(error)
}

async fn prepare_compact_request(
    state: &AppState,
    headers: &HeaderMap,
    mut body: Value,
) -> AppResult<(CompactRequest, Vec<MonoizeAttempt>)> {
    let auth = auth_tenant(headers, state).await?;
    let request_id = extract_request_id(headers);
    // Capture the received body before redirects; skip its clone when capture is off.
    let capture_session = state
        .request_capture
        .maybe_start_session(
            &state.monoize_runtime,
            &auth,
            request_id.clone(),
            DownstreamProtocol::Responses.into(),
            false,
        )
        .await;
    let raw_input = Arc::new(if capture_session.is_some() {
        body.clone()
    } else {
        Value::Null
    });
    let mut logical_model = compact_request_model(&body)?;
    apply_configured_model_redirects_to_model(state, &mut logical_model, &auth).await;
    ensure_model_allowed(&auth, &logical_model)?;
    body["model"] = Value::String(logical_model.clone());
    let max_multiplier = resolve_max_multiplier_for_embeddings(&body, headers, &auth);
    let routing_request = urp::decode::openai_responses::decode_request(&body)
        .map_err(|message| AppError::new(StatusCode::BAD_REQUEST, "invalid_request", message))?;
    let routing_stub = build_routing_stub(&routing_request, max_multiplier);
    let mut attempts = build_monoize_attempts_for_provider_type(
        state,
        &routing_stub,
        &auth,
        Some(ProviderType::Responses),
    )
    .await?;
    if attempts.is_empty() {
        return Err(AppError::new(
            StatusCode::BAD_REQUEST,
            "provider_type_not_supported",
            format!("model '{logical_model}' has no Responses provider"),
        ));
    }
    attach_client_session_id(
        &mut attempts,
        extract_client_session_id(headers),
        Some(&routing_request),
    );
    ensure_balance_before_forward_for_attempts(state, &auth, &attempts).await?;
    Ok((
        CompactRequest {
            auth,
            logical_model,
            body,
            routing_request,
            request_id,
            request_ip: extract_client_ip(headers),
            started_at: Instant::now(),
            capture: RequestCaptureContext {
                raw_input,
                session: capture_session,
            },
        },
        attempts,
    ))
}

fn compact_request_model(body: &Value) -> AppResult<String> {
    let body = body.as_object().ok_or_else(|| {
        AppError::new(
            StatusCode::BAD_REQUEST,
            "invalid_request",
            "body must be object",
        )
    })?;
    if body.get("stream").and_then(Value::as_bool) == Some(true) {
        return Err(AppError::new(
            StatusCode::BAD_REQUEST,
            "invalid_request",
            "responses compact does not support streaming",
        ));
    }
    let has_previous_response = body
        .get("previous_response_id")
        .and_then(Value::as_str)
        .is_some_and(|id| !id.is_empty());
    if !body.contains_key("input") && !has_previous_response {
        return Err(AppError::new(
            StatusCode::BAD_REQUEST,
            "invalid_request",
            "compact requires input or previous_response_id",
        ));
    }
    body.get("model")
        .and_then(Value::as_str)
        .map(str::trim)
        .filter(|model| !model.is_empty())
        .map(str::to_string)
        .ok_or_else(|| AppError::new(StatusCode::BAD_REQUEST, "invalid_request", "missing model"))
}

impl CompactExecution<'_> {
    async fn call(&self, attempt: &mut MonoizeAttempt) -> AppResult<CompactCall> {
        let request = &self.request;
        let mut body = crate::urp::encode::sanitize_provider_item_wire_body(&request.body);
        let wire_model = self
            .channel_models
            .get(&attempt.channel_id)
            .map(|models| {
                compact_wire_model(models, &request.logical_model, &attempt.upstream_model)
            })
            .unwrap_or_else(|| attempt.upstream_model.clone());
        if let Some(obj) = body.as_object_mut() {
            obj.insert("model".to_string(), Value::String(wire_model.clone()));
            obj.remove("max_multiplier");
        }
        let provider = build_channel_provider_config(attempt);
        let http = client_http_for_attempt(self.state, attempt)?;
        let extra_headers = attempt_extra_headers(attempt, &body);
        attempt.session_affinity_value = resolve_session_affinity_value(attempt, &body);
        let request_body = upstream::JsonBody::new(&body);
        let capture_body = request.capture.session.is_some().then_some(body);
        let result = upstream::call_upstream_with_timeout_and_headers(
            &http,
            &provider,
            &attempt.api_key,
            "/v1/responses/compact",
            request_body,
            attempt.request_timeout_ms,
            &extra_headers,
        )
        .await;
        Ok(CompactCall {
            wire_model,
            capture_body,
            result,
        })
    }

    async fn capture_attempt(&self, slot: &AttemptSlot, call: &mut CompactCall) {
        let request = &self.request;
        let (Some(session), Some(body)) =
            (request.capture.session.as_ref(), call.capture_body.take())
        else {
            return;
        };
        let attempt = &slot.attempt;
        let (downstream_response, error) = match &call.result {
            Ok(value) => (Some(value.clone()), None),
            Err(error) => (
                None,
                Some(json!({
                    "message": error.message,
                    "code": error.code,
                    "status": error.status.map(|status| status.as_u16()),
                })),
            ),
        };
        session
            .push_attempt(crate::request_capture::build_attempt_dump(
                crate::request_capture::AttemptDumpInput {
                    attempt_number: slot.position.attempt_number,
                    provider_id: &attempt.provider_id,
                    channel_id: Some(&attempt.channel_id),
                    provider_type: attempt.provider_type,
                    logical_model: &request.logical_model,
                    upstream_model: &call.wire_model,
                    upstream_path: "/v1/responses/compact",
                    raw_input: request.capture.raw_input.as_ref().clone(),
                    transformed_urp_request: &request.routing_request,
                    upstream_request: body,
                    downstream_response,
                    reconstructed_urp_response: None,
                    downstream_sse_frames: None,
                    // RCD-D3b: compact passthrough applies no URP transforms.
                    transform_chain: json!([]),
                    error,
                },
            ))
            .await;
    }

    async fn accept(
        &self,
        attempt: &MonoizeAttempt,
        wire_model: &str,
        value: Value,
        mut tried: Vec<TriedProvider>,
    ) -> AppResult<Response> {
        let request = &self.request;
        update_pending_channel_info(self.state, &request.auth, attempt, self.log_context()).await;
        let usage = parse_usage_from_responses_object(&value);
        if usage.is_none() && missing_usage_rejects(&request.auth, attempt) {
            return Err(self
                .finish_error(attempt, missing_usage_error(), tried)
                .await);
        }
        let upstream_response_model = mismatched_upstream_response_model(
            wire_model,
            value.get("model").and_then(Value::as_str).unwrap_or(""),
        );
        let response_service_tier = usage::response_service_tier(&value).map(str::to_string);
        mark_channel_success(self.state, attempt).await;
        refresh_channel_affinity(self.state, attempt).await;
        let settled_usage = match usage.as_ref() {
            Some(usage) => crate::settlement::SettledUsage::Reported(usage),
            None => crate::settlement::SettledUsage::MissingFree,
        };
        let charge = match maybe_charge_settled(
            self.state,
            &request.auth,
            attempt,
            &request.logical_model,
            SettlementRequest {
                usage: settled_usage,
                output: None,
                response_service_tier: response_service_tier.as_deref(),
            },
            request.request_id.as_deref(),
        )
        .await
        {
            Ok(charge) => charge,
            Err(error) => {
                return Err(self
                    .finish_error(attempt, error, std::mem::take(&mut tried))
                    .await);
            }
        };
        spawn_request_log(
            self.state,
            &request.auth,
            attempt,
            self.log_context(),
            RequestLogSuccess {
                usage: usage.clone(),
                charge_nano_usd: charge.charge_nano_usd,
                billing_breakdown_json: charge.billing_breakdown,
                ttfb_ms: None,
                stream_terminal_diagnostics: None,
                tried_providers: tried,
                client_gone: false,
                upstream_response_model,
            },
        );
        self.persist(usage.as_ref(), false).await;
        Ok(Json(value).into_response())
    }

    async fn finish_error(
        &self,
        attempt: &MonoizeAttempt,
        error: AppError,
        tried: Vec<TriedProvider>,
    ) -> AppError {
        self.log_error(Some(attempt), &error, tried);
        self.persist(None, false).await;
        error
    }

    async fn persist(&self, usage: Option<&urp::Usage>, failed: bool) {
        if let Some(session) = self.request.capture.session.as_ref() {
            session.persist_with_result(usage, failed).await;
        }
    }

    fn log_error(
        &self,
        attempt: Option<&MonoizeAttempt>,
        error: &AppError,
        tried: Vec<TriedProvider>,
    ) {
        let request = &self.request;
        if let Some(attempt) = attempt {
            spawn_request_log_error(
                self.state,
                &request.auth,
                attempt,
                self.log_context(),
                error,
                tried,
            );
        } else {
            spawn_request_log_error_no_attempt(
                self.state,
                &request.auth,
                self.log_context(),
                error,
                tried,
            );
        }
    }

    fn log_context(&self) -> RequestLogContext<'_> {
        RequestLogContext {
            model: &self.request.logical_model,
            is_stream: false,
            started_at: self.request.started_at,
            request_id: self.request.request_id.as_deref(),
            request_ip: self.request.request_ip.as_deref(),
            reasoning_effort: None,
        }
    }
}

async fn load_compact_channel_models(
    state: &AppState,
    logical_model: &str,
    attempts: &[MonoizeAttempt],
) -> HashMap<String, HashMap<String, MonoizeModelEntry>> {
    let mut out = HashMap::new();
    if attempts.is_empty() {
        return out;
    }
    let Ok(providers) = state
        .monoize_store
        .list_providers_for_model(logical_model)
        .await
    else {
        return out;
    };
    for attempt in attempts {
        if out.contains_key(&attempt.channel_id) {
            continue;
        }
        let Some(provider) = providers
            .iter()
            .find(|provider| provider.id == attempt.provider_id)
        else {
            continue;
        };
        let Some(channel) = provider
            .channels
            .iter()
            .find(|channel| channel.id == attempt.channel_id)
        else {
            continue;
        };
        out.insert(attempt.channel_id.clone(), channel.models.clone());
    }
    out
}
