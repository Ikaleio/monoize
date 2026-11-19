use super::*;
use crate::monoize_routing::{ChannelProbe, MonoizeChannel, MonoizeProvider, MonoizeRuntimeConfig};
use crate::urp::stream_decode::{StreamDecodeContext, stream_upstream_to_urp_events};

pub(crate) struct ProbeRequestSettings {
    global_transforms: Vec<TransformRuleConfig>,
    extra_fields_whitelist: HashMap<String, Vec<String>>,
    strip_cross_protocol_nested_extra: bool,
}

impl From<&MonoizeRuntimeConfig> for ProbeRequestSettings {
    fn from(runtime: &MonoizeRuntimeConfig) -> Self {
        Self {
            global_transforms: runtime.global_transforms.clone(),
            extra_fields_whitelist: runtime.extra_fields_whitelist.clone(),
            strip_cross_protocol_nested_extra: runtime.strip_cross_protocol_nested_extra,
        }
    }
}

pub(crate) struct ChannelProbeOutcome {
    pub ok: bool,
    pub usage: Option<urp::Usage>,
    pub http_status: Option<u16>,
    pub retry_after_seconds: Option<u16>,
    pub rate_limited: bool,
    pub error_code: Option<String>,
    pub error_type: Option<String>,
    pub error: Option<String>,
}

pub(crate) struct ProbeFailure {
    code: Option<String>,
    error_type: Option<String>,
    message: String,
}

impl ProbeFailure {
    fn new(code: &str, error_type: &str, message: String) -> Self {
        Self {
            code: Some(code.to_owned()),
            error_type: Some(error_type.to_owned()),
            message,
        }
    }

    fn from_decode_error(error: AppError, signals: (Option<&str>, Option<&str>)) -> Self {
        let error = error.into_inner();
        Self {
            code: error
                .upstream_code
                .or_else(|| signals.0.map(str::to_owned))
                .or(Some(error.code)),
            error_type: error
                .upstream_type
                .or_else(|| signals.1.map(str::to_owned))
                .or(Some(error.error_type)),
            message: error.message,
        }
    }

    fn preparation(
        provider: &MonoizeProvider,
        channel: &MonoizeChannel,
        probe: ChannelProbe<'_>,
        stage: &str,
        code: &str,
        detail: &str,
    ) -> Self {
        tracing::warn!(channel_id = %channel.id, provider = %provider.name,
            probe_model = %probe.logical_model, stage, error = %detail, "probe request preparation failed");
        Self::new(
            code,
            "probe_preparation_error",
            format!("probe request {stage} failed: {detail}"),
        )
    }

    pub(crate) fn into_outcome(self) -> ChannelProbeOutcome {
        ProbeResponse::default().finish(Err(self))
    }
}

pub(crate) struct PreparedProbe {
    request: reqwest::RequestBuilder,
    completion: Option<urp::UrpRequest>,
    provider_type: ProviderType,
}

impl PreparedProbe {
    pub(crate) fn is_stream(&self) -> bool {
        self.completion
            .as_ref()
            .is_some_and(|req| req.stream == Some(true))
    }

    pub(crate) fn upstream_model(&self) -> Option<&str> {
        self.completion.as_ref().map(|req| req.model.as_str())
    }

    pub(crate) async fn send(self, timeout_ms: u64) -> ChannelProbeOutcome {
        let mut response = ProbeResponse::default();
        let result = tokio::time::timeout(
            Duration::from_millis(timeout_ms),
            self.decode(&mut response, timeout_ms),
        )
        .await
        .unwrap_or_else(|_| {
            let detail = format!("upstream probe timed out after {timeout_ms}ms");
            let message = match response.status.filter(|status| !status.is_success()) {
                Some(status) => format!("{} ({detail})", format_probe_http_error(status, "")),
                None => detail,
            };
            Err(ProbeFailure::new(
                "upstream_timeout",
                "transport_error",
                message,
            ))
        });
        response.finish(result)
    }

    async fn decode(
        self,
        meta: &mut ProbeResponse,
        timeout_ms: u64,
    ) -> Result<Option<urp::Usage>, ProbeFailure> {
        let stream = self.is_stream();
        let response = upstream::send_request(self.request)
            .await
            .map_err(|error| {
                ProbeFailure::new(
                    "upstream_connection_failed",
                    "transport_error",
                    format!("connection failed: {}", error.message),
                )
            })?;
        let status = response.status();
        meta.status = Some(status);
        meta.retry_after_seconds = upstream::parse_retry_after(response.headers(), now_ts());
        if !status.is_success() {
            let body = upstream::read_http_error_body(response).await;
            let info = serde_json::from_str::<Value>(&body)
                .ok()
                .map(|value| upstream::extract_error_members(body_error_object(&value)))
                .unwrap_or_default();
            return Err(ProbeFailure {
                code: info.code,
                error_type: info.error_type,
                message: format_probe_http_error(status, &body),
            });
        }

        if stream && let Some(req) = self.completion.as_ref() {
            return decode_probe_stream(
                self.provider_type,
                req,
                response,
                timeout_ms,
                &mut meta.upstream_rate_limited,
            )
            .await;
        }
        let value = match upstream::read_json_body(response).await {
            Ok(value) => value,
            // Native image and decision probes use 2xx as a connectivity signal.
            Err(err)
                if self.completion.is_none() && matches!(err.kind, UpstreamErrorKind::Http) =>
            {
                return Ok(None);
            }
            Err(err) if matches!(err.kind, UpstreamErrorKind::Network) => {
                return Err(ProbeFailure::new(
                    "upstream_connection_failed",
                    "transport_error",
                    format!("connection failed: {}", err.message),
                ));
            }
            Err(err) => {
                return Err(ProbeFailure::new(
                    "invalid_upstream_response",
                    "decode_error",
                    format!("invalid upstream JSON: {}", err.message),
                ));
            }
        };
        let signals = body_error_signals(&value);
        if let Some(req) = self.completion.as_ref() {
            let decoded =
                decode_response_from_provider(self.provider_type, &value, &req.model, false, req)
                    .map_err(|err| ProbeFailure::from_decode_error(err, signals))?;
            if let Some(failure) = terminal_outcome_failure(decoded.outcome, signals) {
                return Err(failure);
            }
            if classify_channel_health_failure(None, signals.0, signals.1).is_none() {
                return Ok(decoded.usage);
            }
        } else if classify_channel_health_failure(None, signals.0, signals.1).is_none() {
            return Ok(extract_native_usage(&value));
        }
        Err(ProbeFailure {
            code: signals.0.map(str::to_owned),
            error_type: signals.1.map(str::to_owned),
            message: format_probe_http_error(status, &value.to_string()),
        })
    }
}

pub(crate) async fn prepare_channel_probe(
    state: &AppState,
    http: &reqwest::Client,
    provider: &MonoizeProvider,
    channel: &MonoizeChannel,
    probe: ChannelProbe<'_>,
    settings: &ProbeRequestSettings,
    stream: bool,
) -> Result<PreparedProbe, ProbeFailure> {
    let provider_type = probe.provider_type.to_config_type();
    let preparation = |stage: &str, code: &str, detail: &str| {
        ProbeFailure::preparation(provider, channel, probe, stage, code, detail)
    };
    let session = crate::monoize_routing::is_direct_opencode_zen_url(&channel.base_url)
        .then_some("mono-probe");
    let (completion, path, body) = if probe.is_completion() {
        let mut req = urp::UrpRequest {
            model: String::new(),
            input: vec![urp::Node::text(urp::OrdinaryRole::User, "hi")],
            stream: Some(stream),
            max_output_tokens: Some(16),
            image_generation: None,
            logprobs: None,
            sampling: None,
            instructions_format: None,
            context: Default::default(),
            temperature: None,
            top_p: None,
            reasoning: None,
            tools: None,
            tool_choice: None,
            parallel_tool_calls: None,
            stop: None,
            verbosity: None,
            response_format: None,
            user: None,
            extra_body: HashMap::new(),
        };
        shape_upstream_request(
            &mut req,
            provider_type,
            probe.upstream_model,
            provider
                .strip_cross_protocol_nested_extra
                .unwrap_or(settings.strip_cross_protocol_nested_extra),
            None,
            None,
        )
        .map_err(|err| preparation("shaping", &err.code, &err.message))?;
        for rules in [&provider.transforms, &settings.global_transforms] {
            apply_transform_rules_request(
                state,
                &mut req,
                rules,
                probe.logical_model,
                Some(provider_type),
            )
            .await
            .map_err(|err| preparation("transform", &err.code, &err.message))?;
        }
        strip_monoize_context(&mut req);
        let whitelist = merge_extra_fields_whitelist(
            &settings.extra_fields_whitelist,
            &provider.extra_fields_whitelist,
            probe.provider_type,
        );
        let body = encode_upstream_request(
            &mut req,
            provider_type,
            probe.logical_model,
            probe.chat_dialect,
            &whitelist,
            None,
        )
        .map_err(|err| preparation("encoding", &err.code, &err.message))?;
        let path = upstream_path_for_model(provider_type, &req.model, req.stream == Some(true));
        (Some(req), path, body)
    } else {
        let (path, body) =
            native_probe_request(provider_type, probe.upstream_model).ok_or_else(|| {
                preparation(
                    "shaping",
                    "provider_type_not_supported",
                    "this API type does not support active probes",
                )
            })?;
        (None, path.to_owned(), body)
    };
    let provider_config =
        build_upstream_provider_config(provider_type, &provider.id, &channel.id, &channel.base_url);
    let headers = build_upstream_headers(
        provider_type,
        &body,
        channel.extra_headers.as_ref(),
        None,
        session,
    );
    let request = upstream::json_request(
        http,
        &provider_config,
        &channel.api_key,
        &path,
        upstream::JsonBody::new(&body),
        &headers,
    )
    .map_err(|err| preparation("HTTP construction", "upstream_request_failed", &err.message))?;
    Ok(PreparedProbe {
        request,
        completion,
        provider_type,
    })
}

async fn decode_probe_stream(
    provider_type: ProviderType,
    req: &urp::UrpRequest,
    response: reqwest::Response,
    timeout_ms: u64,
    upstream_rate_limited: &mut bool,
) -> Result<Option<urp::Usage>, ProbeFailure> {
    let legacy = typed_request_to_legacy(req, None);
    let pending_extra = find_pending_request_envelope_extra(&req.input);
    let (tx, mut rx) = mpsc::channel(16);
    let metrics = Arc::new(Mutex::new(StreamRuntimeMetrics::default()));
    let decode = stream_upstream_to_urp_events(
        &legacy,
        pending_extra,
        provider_type,
        response,
        tx,
        StreamDecodeContext {
            started_at: None,
            runtime_metrics: Some(metrics.clone()),
            idle_timeout_ms: timeout_ms,
        },
    );
    let read = async {
        let mut terminal = None;
        let mut error = None;
        while let Some(event) = rx.recv().await {
            match event {
                urp::UrpStreamEvent::ResponseDone { outcome, usage, .. } => {
                    terminal = Some((outcome, usage))
                }
                urp::UrpStreamEvent::Error {
                    code,
                    message,
                    extra_body,
                } => {
                    error = Some(ProbeFailure {
                        code,
                        error_type: extra_body
                            .get("type")
                            .and_then(Value::as_str)
                            .map(str::to_owned),
                        message,
                    });
                }
                _ => {}
            }
        }
        (terminal, error)
    };
    let (decoded, (terminal, event_error)) = tokio::join!(decode, read);
    let diagnostic = {
        let mut metrics = metrics.lock().await;
        *upstream_rate_limited = metrics.upstream_terminal_rate_limited;
        metrics.terminal.terminal_error.take()
    };
    if let Some(error) = diagnostic {
        return Err(ProbeFailure {
            code: Some(error.code),
            error_type: error.error_type,
            message: error.message,
        });
    }
    if let Some(error) = event_error {
        return Err(error);
    }
    decoded.map_err(|err| ProbeFailure::from_decode_error(err, (None, None)))?;
    let (outcome, usage) = terminal.ok_or_else(|| {
        ProbeFailure::new(
            "upstream_stream_missing_terminal",
            "stream_error",
            "upstream stream ended without a terminal response event".to_owned(),
        )
    })?;
    if let Some(error) = terminal_outcome_failure(outcome, (None, None)) {
        return Err(error);
    }
    Ok(usage)
}

fn terminal_outcome_failure(
    outcome: Option<urp::ResponseOutcome>,
    signals: (Option<&str>, Option<&str>),
) -> Option<ProbeFailure> {
    let outcome = outcome?;
    if outcome.error.is_none()
        && matches!(
            outcome.status,
            urp::ResponseStatus::Completed | urp::ResponseStatus::Incomplete
        )
    {
        return None;
    }
    let (code, error_type, message) = outcome.error.map_or((None, None, None), |error| {
        let error_type = error
            .extra_body
            .get("type")
            .and_then(Value::as_str)
            .map(str::to_owned);
        (error.code, error_type, error.message)
    });
    let code = code.or_else(|| signals.0.map(str::to_owned));
    let error_type = error_type.or_else(|| signals.1.map(str::to_owned));
    let message = message
        .unwrap_or_else(|| format!("upstream response ended with status {:?}", outcome.status));
    Some(ProbeFailure {
        code,
        error_type,
        message,
    })
}

#[derive(Default)]
struct ProbeResponse {
    upstream_rate_limited: bool,
    status: Option<StatusCode>,
    retry_after_seconds: Option<u16>,
}

impl ProbeResponse {
    fn finish(self, result: Result<Option<urp::Usage>, ProbeFailure>) -> ChannelProbeOutcome {
        let mut outcome = ChannelProbeOutcome {
            ok: result.is_ok(),
            usage: None,
            http_status: self.status.map(|status| status.as_u16()),
            retry_after_seconds: self.retry_after_seconds,
            rate_limited: false,
            error_code: None,
            error_type: None,
            error: None,
        };
        match result {
            Ok(usage) => outcome.usage = usage,
            Err(failure) => {
                outcome.rate_limited = self.upstream_rate_limited
                    || (outcome.http_status.is_some()
                        && matches!(
                            classify_channel_health_failure(
                                outcome.http_status,
                                failure.code.as_deref(),
                                failure.error_type.as_deref()
                            ),
                            Some(RetryableFailureClass::RateLimited)
                        ));
                outcome.error_code = failure.code;
                outcome.error_type = failure.error_type;
                outcome.error = Some(failure.message);
            }
        }
        outcome
    }
}

fn format_probe_http_error(status: StatusCode, body: &str) -> String {
    let body = body.trim();
    let mut end = body.len().min(512);
    while !body.is_char_boundary(end) {
        end -= 1;
    }
    let body = &body[..end];
    let mut message = format!("upstream returned {}", status.as_u16());
    if let Some(reason) = status.canonical_reason() {
        message.push(' ');
        message.push_str(reason);
    }
    if !body.is_empty() {
        message.push_str(": ");
        message.push_str(body);
    }
    message
}

fn body_error_signals(body: &Value) -> (Option<&str>, Option<&str>) {
    let error = body_error_object(body);
    let member = |key| {
        error
            .get(key)
            .and_then(Value::as_str)
            .filter(|value| !value.trim().is_empty())
    };
    (member("code"), member("type"))
}

fn body_error_object(body: &Value) -> &Value {
    body.get("error")
        .filter(|error| error.is_object())
        .unwrap_or(body)
}

fn extract_native_usage(body: &Value) -> Option<urp::Usage> {
    let usage = body.get("usage")?;
    Some(urp::Usage {
        reported_token_counts: true,
        input_tokens: usage
            .get("prompt_tokens")
            .and_then(Value::as_u64)
            .or_else(|| usage.get("input_tokens").and_then(Value::as_u64))?,
        output_tokens: usage
            .get("completion_tokens")
            .and_then(Value::as_u64)
            .or_else(|| usage.get("output_tokens").and_then(Value::as_u64))?,
        ..Default::default()
    })
}

fn native_probe_request(provider_type: ProviderType, model: &str) -> Option<(&'static str, Value)> {
    let body = match provider_type {
        ProviderType::OpenaiImage => {
            json!({"model": model, "prompt": "test", "size": "1024x1024", "n": 1})
        }
        ProviderType::OpenrouterImage => json!({"model": model, "prompt": "test", "n": 1}),
        ProviderType::SystemOne => json!({
            "model": model, "state": "ping", "questions": {"ping": {"type": "noul", "instructions": "Is this a ping?"}},
        }),
        ProviderType::OpenaiDecisions => json!({
            "model": model, "input": "ping", "questions": [{"type": "predicate", "name": "ping", "instructions": "Is this a ping?"}],
        }),
        _ => return None,
    };
    Some((upstream_path(provider_type), body))
}
