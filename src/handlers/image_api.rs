use super::*;
use axum::extract::Multipart;
use base64::Engine as _;
use futures_util::StreamExt as _;
use std::collections::HashMap;

pub async fn create_image_generation(
    State(state): State<AppState>,
    headers: HeaderMap,
    Json(body): Json<Value>,
) -> AppResult<Response> {
    let auth = auth_tenant(&headers, &state).await?;
    // RCD-C9/RCD-C16 pre-check: the raw-input clone is skipped entirely when
    // no sub-request session could start.
    let capture_eligible = state
        .request_capture
        .would_start_session(&state.monoize_runtime, &auth)
        .await;
    // RCD-D4: the parsed downstream JSON body, shared by every sub-request
    // dump (RCD-D4b).
    let capture_raw_input = std::sync::Arc::new(if capture_eligible {
        body.clone()
    } else {
        Value::Null
    });

    let obj = body.as_object().ok_or_else(|| {
        AppError::new(
            StatusCode::BAD_REQUEST,
            "invalid_request",
            "body must be object",
        )
    })?;

    let prompt = obj
        .get("prompt")
        .and_then(|v| v.as_str())
        .filter(|s| !s.trim().is_empty())
        .ok_or_else(|| {
            AppError::new(
                StatusCode::BAD_REQUEST,
                "invalid_request",
                "missing or empty prompt",
            )
        })?
        .to_string();

    let mut model = obj
        .get("model")
        .and_then(|v| v.as_str())
        .filter(|s| !s.trim().is_empty())
        .ok_or_else(|| AppError::new(StatusCode::BAD_REQUEST, "invalid_request", "missing model"))?
        .to_string();

    apply_configured_model_redirects_to_model(&state, &mut model, &auth).await;

    let n = parse_n_field(obj.get("n"))?;

    // IG3: `stream` must be a JSON boolean when present.
    let stream_requested = match obj.get("stream") {
        None => false,
        Some(Value::Bool(flag)) => *flag,
        Some(_) => {
            return Err(AppError::new(
                StatusCode::BAD_REQUEST,
                "invalid_request",
                "stream must be a boolean",
            ));
        }
    };
    if stream_requested && n != 1 {
        // IG4: streaming responses carry no image index, so fan-out is
        // rejected instead of producing an ambiguous interleaved stream.
        return Err(AppError::new(
            StatusCode::BAD_REQUEST,
            "invalid_request",
            "stream=true requires n=1",
        ));
    }

    ensure_model_allowed(&auth, &model)?;

    let max_multiplier_val =
        resolve_image_max_multiplier(obj.get("max_multiplier"), &headers, &auth);
    let request_id = extract_request_id(&headers);
    let request_ip = extract_client_ip(&headers);

    let extra_body = build_extra_body(obj, &["prompt", "model", "n", "max_multiplier", "stream"]);

    let inputs = vec![urp::Node::Text {
        id: None,
        role: urp::OrdinaryRole::User,
        content: prompt,
        phase: None,
        extra_body: HashMap::new(),
    }];

    if stream_requested {
        return run_image_stream_downstream(
            state,
            auth,
            model,
            inputs,
            extra_body,
            max_multiplier_val,
            request_id,
            request_ip,
            extract_client_session_id(&headers),
            capture_raw_input,
            crate::request_capture::CaptureDownstreamProtocol::ImageGenerations,
            ImageStreamEventFamily::Generation,
        )
        .await;
    }

    let results = fan_out_subrequests(
        &state,
        &auth,
        &model,
        &inputs,
        &extra_body,
        max_multiplier_val,
        n,
        request_id,
        request_ip,
        extract_client_session_id(&headers),
        capture_raw_input,
        crate::request_capture::CaptureDownstreamProtocol::ImageGenerations,
    )
    .await;

    assemble_image_response(results)
}

pub async fn create_image_edit(
    State(state): State<AppState>,
    headers: HeaderMap,
    mut multipart: Multipart,
) -> AppResult<Response> {
    let auth = auth_tenant(&headers, &state).await?;
    // RCD-C9/RCD-C16 pre-check: the RCD-D4a multipart capture parts are only
    // recorded when a sub-request session could start.
    let capture_eligible = state
        .request_capture
        .would_start_session(&state.monoize_runtime, &auth)
        .await;

    let mut prompt: Option<String> = None;
    let mut model: Option<String> = None;
    let mut n_raw: Option<String> = None;
    let mut stream_raw: Option<String> = None;
    let mut image_data: Option<(String, String)> = None;
    let mut extra_images: Vec<(String, String)> = Vec::new();
    let mut mask_data: Option<(String, String)> = None;
    let mut max_multiplier_raw: Option<String> = None;
    let mut extra_text_fields: HashMap<String, Value> = HashMap::new();
    // RCD-D4a: consumed parts in wire order, exact wire text and raw bytes.
    let mut capture_parts: Vec<crate::request_capture::CapturedMultipartPart> = Vec::new();

    while let Some(field) = multipart
        .next_field()
        .await
        .map_err(|e| AppError::new(StatusCode::BAD_REQUEST, "invalid_request", e.to_string()))?
    {
        let field_name = field.name().unwrap_or("").to_string();
        match field_name.as_str() {
            "prompt" => {
                let text = field.text().await.map_err(|e| {
                    AppError::new(StatusCode::BAD_REQUEST, "invalid_request", e.to_string())
                })?;
                if capture_eligible {
                    capture_parts.push(crate::request_capture::CapturedMultipartPart::Text {
                        name: field_name,
                        text: text.clone(),
                    });
                }
                prompt = Some(text);
            }
            "model" => {
                let text = field.text().await.map_err(|e| {
                    AppError::new(StatusCode::BAD_REQUEST, "invalid_request", e.to_string())
                })?;
                if capture_eligible {
                    capture_parts.push(crate::request_capture::CapturedMultipartPart::Text {
                        name: field_name,
                        text: text.clone(),
                    });
                }
                model = Some(text);
            }
            "n" => {
                let text = field.text().await.map_err(|e| {
                    AppError::new(StatusCode::BAD_REQUEST, "invalid_request", e.to_string())
                })?;
                if capture_eligible {
                    capture_parts.push(crate::request_capture::CapturedMultipartPart::Text {
                        name: field_name,
                        text: text.clone(),
                    });
                }
                n_raw = Some(text);
            }
            "stream" => {
                let text = field.text().await.map_err(|e| {
                    AppError::new(StatusCode::BAD_REQUEST, "invalid_request", e.to_string())
                })?;
                if capture_eligible {
                    capture_parts.push(crate::request_capture::CapturedMultipartPart::Text {
                        name: field_name,
                        text: text.clone(),
                    });
                }
                stream_raw = Some(text);
            }
            "max_multiplier" => {
                let text = field.text().await.map_err(|e| {
                    AppError::new(StatusCode::BAD_REQUEST, "invalid_request", e.to_string())
                })?;
                if capture_eligible {
                    capture_parts.push(crate::request_capture::CapturedMultipartPart::Text {
                        name: field_name,
                        text: text.clone(),
                    });
                }
                max_multiplier_raw = Some(text);
            }
            "image" | "image[]" => {
                let wire_filename = field.file_name().map(|s| s.to_string());
                let wire_content_type = field.content_type().map(|s| s.to_string());
                let media_type = wire_content_type
                    .clone()
                    .unwrap_or_else(|| infer_media_type_from_filename(wire_filename.as_deref()));
                let bytes = field.bytes().await.map_err(|e| {
                    AppError::new(StatusCode::BAD_REQUEST, "invalid_request", e.to_string())
                })?;
                let b64 = base64::engine::general_purpose::STANDARD.encode(&bytes);
                if capture_eligible {
                    capture_parts.push(crate::request_capture::CapturedMultipartPart::File {
                        name: field_name,
                        filename: wire_filename,
                        content_type: wire_content_type,
                        data_base64: b64.clone(),
                        byte_length: bytes.len(),
                    });
                }
                if image_data.is_none() {
                    image_data = Some((media_type, b64));
                } else {
                    extra_images.push((media_type, b64));
                }
            }
            "mask" => {
                let wire_filename = field.file_name().map(|s| s.to_string());
                let wire_content_type = field.content_type().map(|s| s.to_string());
                let media_type = wire_content_type
                    .clone()
                    .unwrap_or_else(|| infer_media_type_from_filename(wire_filename.as_deref()));
                let bytes = field.bytes().await.map_err(|e| {
                    AppError::new(StatusCode::BAD_REQUEST, "invalid_request", e.to_string())
                })?;
                let b64 = base64::engine::general_purpose::STANDARD.encode(&bytes);
                if capture_eligible {
                    capture_parts.push(crate::request_capture::CapturedMultipartPart::File {
                        name: field_name,
                        filename: wire_filename,
                        content_type: wire_content_type,
                        data_base64: b64.clone(),
                        byte_length: bytes.len(),
                    });
                }
                mask_data = Some((media_type, b64));
            }
            _ => {
                // RCD-D4a: unknown parts appear iff the parser consumed them
                // as text fields (IE1); ignored file parts (IE2) are absent.
                if let Ok(text) = field.text().await {
                    if capture_eligible {
                        capture_parts.push(crate::request_capture::CapturedMultipartPart::Text {
                            name: field_name.clone(),
                            text: text.clone(),
                        });
                    }
                    extra_text_fields.insert(field_name, coerce_text_to_json_value(&text));
                }
            }
        }
    }

    let capture_raw_input = std::sync::Arc::new(if capture_eligible {
        crate::request_capture::multipart_capture_object(&capture_parts)
    } else {
        Value::Null
    });

    let prompt = prompt.filter(|s| !s.trim().is_empty()).ok_or_else(|| {
        AppError::new(
            StatusCode::BAD_REQUEST,
            "invalid_request",
            "missing or empty prompt",
        )
    })?;

    let mut model = model.filter(|s| !s.trim().is_empty()).ok_or_else(|| {
        AppError::new(StatusCode::BAD_REQUEST, "invalid_request", "missing model")
    })?;

    apply_configured_model_redirects_to_model(&state, &mut model, &auth).await;

    let (image_media_type, image_b64) = image_data.ok_or_else(|| {
        AppError::new(
            StatusCode::BAD_REQUEST,
            "invalid_request",
            "missing image file",
        )
    })?;

    let n = match &n_raw {
        Some(s) => parse_n_field(Some(&Value::String(s.clone())))?,
        None => 1,
    };

    // IE4: `stream` text field accepts exactly `true` or `false`.
    let stream_requested = match stream_raw.as_deref() {
        None => false,
        Some("true") => true,
        Some("false") => false,
        Some(_) => {
            return Err(AppError::new(
                StatusCode::BAD_REQUEST,
                "invalid_request",
                "stream must be true or false",
            ));
        }
    };
    if stream_requested && n != 1 {
        return Err(AppError::new(
            StatusCode::BAD_REQUEST,
            "invalid_request",
            "stream=true requires n=1",
        ));
    }

    ensure_model_allowed(&auth, &model)?;

    let max_multiplier_val = {
        let ceiling = auth.max_multiplier;
        let requested = max_multiplier_raw
            .and_then(|value| parse_positive_multiplier(&value))
            .or_else(|| parse_max_multiplier_header(&headers));
        match (ceiling, requested) {
            (Some(c), Some(r)) => Some(r.min(c)),
            (Some(c), None) => Some(c),
            (None, Some(r)) => Some(r),
            (None, None) => None,
        }
    };

    let request_id = extract_request_id(&headers);
    let request_ip = extract_client_ip(&headers);

    let mut inputs = Vec::new();
    inputs.push(urp::Node::Text {
        id: None,
        role: urp::OrdinaryRole::User,
        content: prompt,
        phase: None,
        extra_body: HashMap::new(),
    });
    inputs.push(urp::Node::Image {
        id: None,
        role: urp::OrdinaryRole::User,
        source: urp::ImageSource::Base64 {
            media_type: image_media_type,
            data: image_b64,
        },
        extra_body: HashMap::new(),
    });
    for (extra_media_type, extra_b64) in extra_images {
        inputs.push(urp::Node::Image {
            id: None,
            role: urp::OrdinaryRole::User,
            source: urp::ImageSource::Base64 {
                media_type: extra_media_type,
                data: extra_b64,
            },
            extra_body: HashMap::new(),
        });
    }
    if let Some((mask_media_type, mask_b64)) = mask_data {
        inputs.push(urp::Node::Image {
            id: Some("__monoize_image_api_mask".to_string()),
            role: urp::OrdinaryRole::User,
            source: urp::ImageSource::Base64 {
                media_type: mask_media_type,
                data: mask_b64,
            },
            extra_body: HashMap::new(),
        });
    }

    if stream_requested {
        return run_image_stream_downstream(
            state,
            auth,
            model,
            inputs,
            extra_text_fields,
            max_multiplier_val,
            request_id,
            request_ip,
            extract_client_session_id(&headers),
            capture_raw_input,
            crate::request_capture::CaptureDownstreamProtocol::ImageEdits,
            ImageStreamEventFamily::Edit,
        )
        .await;
    }

    let results = fan_out_subrequests(
        &state,
        &auth,
        &model,
        &inputs,
        &extra_text_fields,
        max_multiplier_val,
        n,
        request_id,
        request_ip,
        extract_client_session_id(&headers),
        capture_raw_input,
        crate::request_capture::CaptureDownstreamProtocol::ImageEdits,
    )
    .await;

    assemble_image_response(results)
}

/// Downstream SSE event family for one streaming Image API request
/// (`image-api-proxy.spec.md` §1).
#[derive(Clone, Copy)]
enum ImageStreamEventFamily {
    Generation,
    Edit,
}

impl ImageStreamEventFamily {
    fn partial_event_name(self) -> &'static str {
        match self {
            Self::Generation => "image_generation.partial_image",
            Self::Edit => "image_edit.partial_image",
        }
    }

    fn completed_event_name(self) -> &'static str {
        match self {
            Self::Generation => "image_generation.completed",
            Self::Edit => "image_edit.completed",
        }
    }
}

/// Downstream frame sink for one §5.5 streaming Image API sub-request. Send
/// failures are ignored: a disconnected client must not fail the upstream
/// collection, billing, or logging that the executor still has to finish.
struct ImageStreamSink {
    family: ImageStreamEventFamily,
    tx: mpsc::Sender<Event>,
    partial_frames_emitted: u64,
    completed_frames_emitted: u64,
}

impl ImageStreamSink {
    fn new(family: ImageStreamEventFamily, tx: mpsc::Sender<Event>) -> Self {
        Self {
            family,
            tx,
            partial_frames_emitted: 0,
            completed_frames_emitted: 0,
        }
    }

    /// IS7: true once any partial or completed frame reached the client.
    fn frames_emitted(&self) -> bool {
        self.partial_frames_emitted > 0 || self.completed_frames_emitted > 0
    }

    async fn send_frame(&self, event_name: &str, payload: Value) {
        let _ = self
            .tx
            .send(Event::default().event(event_name).data(payload.to_string()))
            .await;
    }

    /// IS3: one partial frame per canonical Base64 image delta.
    async fn send_partial(
        &mut self,
        source: &urp::ImageSource,
        extra_body: &HashMap<String, Value>,
    ) {
        let urp::ImageSource::Base64 { data, .. } = source else {
            return;
        };
        let event_name = self.family.partial_event_name();
        let mut payload = Map::new();
        payload.insert("type".to_string(), Value::String(event_name.to_string()));
        payload.insert("b64_json".to_string(), Value::String(data.clone()));
        let index = extra_body
            .get("partial_image_index")
            .and_then(Value::as_u64)
            .unwrap_or(self.partial_frames_emitted);
        payload.insert("partial_image_index".to_string(), Value::from(index));
        let created_at = extra_body
            .get("created_at")
            .and_then(Value::as_i64)
            .unwrap_or_else(|| chrono::Utc::now().timestamp());
        payload.insert("created_at".to_string(), Value::from(created_at));
        for key in ["output_format", "size", "quality", "background"] {
            if let Some(value) = extra_body.get(key) {
                payload.insert(key.to_string(), value.clone());
            }
        }
        self.partial_frames_emitted += 1;
        self.send_frame(event_name, Value::Object(payload)).await;
    }

    /// IS4: one completed frame per extracted image; the last frame carries
    /// `usage` iff the sub-request produced URP usage.
    async fn send_completed(&mut self, resp: &urp::UrpResponse) {
        let images = extract_images_from_response(resp);
        let total = images.len();
        for (position, image) in images.into_iter().enumerate() {
            let event_name = self.family.completed_event_name();
            let mut payload = Map::new();
            payload.insert("type".to_string(), Value::String(event_name.to_string()));
            if let Some(b64) = image.b64_json {
                payload.insert("b64_json".to_string(), Value::String(b64));
            } else if let Some(url) = image.url {
                payload.insert("url".to_string(), Value::String(url));
            }
            payload.insert(
                "created_at".to_string(),
                Value::from(chrono::Utc::now().timestamp()),
            );
            if position + 1 == total
                && let Some(usage) = &resp.usage
            {
                let mut aggregated = AggregatedUsage::default();
                accumulate_usage(&mut aggregated, usage);
                payload.insert("usage".to_string(), aggregated_usage_value(&aggregated));
            }
            self.completed_frames_emitted += 1;
            self.send_frame(event_name, Value::Object(payload)).await;
        }
    }

    /// IS6: exactly one `error` frame with the standard error object.
    async fn send_error(&mut self, err: &AppError) {
        self.send_frame(
            "error",
            json!({
                "type": "error",
                "error": {
                    "message": err.message,
                    "type": err.error_type,
                    "code": err.code,
                }
            }),
        )
        .await;
    }

    /// IS5: `data: [DONE]` terminator.
    async fn send_done(&self) {
        let _ = self.tx.send(Event::default().data("[DONE]")).await;
    }
}

/// IS7 guard used at every attempt-failover decision inside the streaming
/// executor.
fn sink_frames_emitted(sink: &Option<&mut ImageStreamSink>) -> bool {
    sink.as_ref().is_some_and(|sink| sink.frames_emitted())
}

/// IS1/IM3a: run the single streaming Image API sub-request and answer with
/// the §5.5 SSE stream. Billing and request logging happen inside the
/// executor (IS8); this function only owns the downstream frame encoding.
#[allow(clippy::too_many_arguments)]
async fn run_image_stream_downstream(
    state: AppState,
    auth: crate::auth::AuthResult,
    model: String,
    inputs: Vec<urp::Node>,
    extra_body: HashMap<String, Value>,
    max_multiplier: Option<Multiplier>,
    request_id: Option<String>,
    request_ip: Option<String>,
    client_session_id: Option<String>,
    capture_raw_input: std::sync::Arc<Value>,
    capture_protocol: crate::request_capture::CaptureDownstreamProtocol,
    family: ImageStreamEventFamily,
) -> AppResult<Response> {
    let req = urp::UrpRequest {
        model,
        input: inputs,
        stream: Some(true),
        temperature: None,
        top_p: None,
        max_output_tokens: None,
        reasoning: None,
        tools: None,
        tool_choice: None,
        parallel_tool_calls: None,
        stop: None,
        verbosity: None,
        response_format: None,
        user: None,
        extra_body,
    };
    // RCD-C16/RCD-D2c: one capture session for the single streaming
    // sub-request, recorded with is_stream true.
    let capture_session = state
        .request_capture
        .maybe_start_session(
            &state.monoize_runtime,
            &auth,
            request_id.clone(),
            capture_protocol,
            true,
        )
        .await;
    let capture = super::RequestCaptureContext {
        raw_input: capture_raw_input,
        session: capture_session,
    };
    let (tx, rx) = mpsc::channel::<Event>(64);
    tokio::spawn(async move {
        let task_state = AdmittedRequestTaskState::new(std::time::Instant::now());
        task_state.set_stream(true);
        let mut sink = ImageStreamSink::new(family, tx);
        let result = execute_stream_collected_image_typed(
            &state,
            &auth,
            req,
            max_multiplier,
            request_id,
            request_ip,
            client_session_id,
            capture,
            &task_state,
            Some(&mut sink),
        )
        .await;
        match result {
            // IS4: completed frames come from the validated terminal response.
            Ok((resp, _)) => sink.send_completed(&resp).await,
            // IS6: completed frames exist only after executor success, so
            // every failure emits exactly one error frame.
            Err(err) => sink.send_error(&err).await,
        }
        sink.send_done().await;
    });
    let stream =
        tokio_stream::wrappers::ReceiverStream::new(rx).map(Ok::<_, std::convert::Infallible>);
    Ok(Sse::new(stream)
        .keep_alive(api_stream_keep_alive())
        .into_response())
}

fn parse_n_field(value: Option<&Value>) -> AppResult<usize> {
    let Some(v) = value else {
        return Ok(1);
    };
    let n = match v {
        Value::Number(num) => num.as_u64().ok_or_else(|| {
            AppError::new(
                StatusCode::BAD_REQUEST,
                "invalid_request",
                "n must be a positive integer",
            )
        })? as usize,
        Value::String(s) => s.parse::<usize>().map_err(|_| {
            AppError::new(
                StatusCode::BAD_REQUEST,
                "invalid_request",
                "n must be a positive integer",
            )
        })?,
        _ => {
            return Err(AppError::new(
                StatusCode::BAD_REQUEST,
                "invalid_request",
                "n must be a positive integer",
            ));
        }
    };
    if n == 0 {
        return Err(AppError::new(
            StatusCode::BAD_REQUEST,
            "invalid_request",
            "n must be >= 1",
        ));
    }
    Ok(n)
}

fn build_extra_body(obj: &Map<String, Value>, exclude: &[&str]) -> HashMap<String, Value> {
    let exclude_set: std::collections::HashSet<&str> = exclude.iter().copied().collect();
    obj.iter()
        .filter(|(k, _)| !exclude_set.contains(k.as_str()))
        .map(|(k, v)| (k.clone(), v.clone()))
        .collect()
}

fn resolve_image_max_multiplier(
    body_value: Option<&Value>,
    headers: &HeaderMap,
    auth: &crate::auth::AuthResult,
) -> Option<Multiplier> {
    let ceiling = auth.max_multiplier;
    let requested = body_value
        .and_then(Value::as_str)
        .and_then(parse_positive_multiplier)
        .or_else(|| parse_max_multiplier_header(headers));

    match (ceiling, requested) {
        (Some(c), Some(r)) => Some(r.min(c)),
        (Some(c), None) => Some(c),
        (None, Some(r)) => Some(r),
        (None, None) => None,
    }
}

fn infer_media_type_from_filename(filename: Option<&str>) -> String {
    let ext = filename
        .and_then(|f| f.rsplit('.').next())
        .unwrap_or("")
        .to_lowercase();
    match ext.as_str() {
        "png" => "image/png",
        "jpg" | "jpeg" => "image/jpeg",
        "gif" => "image/gif",
        "webp" => "image/webp",
        "svg" => "image/svg+xml",
        _ => "application/octet-stream",
    }
    .to_string()
}

fn coerce_text_to_json_value(text: &str) -> Value {
    if let Ok(n) = text.parse::<i64>() {
        return Value::Number(n.into());
    }
    if let Ok(n) = text.parse::<f64>() {
        if n.is_finite() {
            if let Some(num) = serde_json::Number::from_f64(n) {
                return Value::Number(num);
            }
        }
    }
    match text {
        "true" => Value::Bool(true),
        "false" => Value::Bool(false),
        _ => Value::String(text.to_string()),
    }
}

#[allow(clippy::too_many_arguments)]
async fn fan_out_subrequests(
    state: &AppState,
    auth: &crate::auth::AuthResult,
    model: &str,
    input: &[urp::Node],
    extra_body: &HashMap<String, Value>,
    max_multiplier: Option<Multiplier>,
    n: usize,
    request_id: Option<String>,
    request_ip: Option<String>,
    client_session_id: Option<String>,
    capture_raw_input: std::sync::Arc<Value>,
    capture_protocol: crate::request_capture::CaptureDownstreamProtocol,
) -> Vec<Result<(urp::UrpResponse, String), AppError>> {
    let mut join_set = tokio::task::JoinSet::new();
    let mut task_contexts = HashMap::new();

    for i in 0..n {
        let state = state.clone();
        let auth = auth.clone();
        let req = urp::UrpRequest {
            model: model.to_string(),
            input: input.to_vec(),
            stream: Some(false),
            temperature: None,
            top_p: None,
            max_output_tokens: None,
            reasoning: None,
            tools: None,
            tool_choice: None,
            parallel_tool_calls: None,
            stop: None,
            verbosity: None,
            response_format: None,
            user: None,
            extra_body: extra_body.clone(),
        };
        let rid = request_id
            .clone()
            .map(|id| if n > 1 { format!("{id}:img:{i}") } else { id });
        let rip = request_ip.clone();
        let task_session_id = client_session_id.clone();
        let task_state = Arc::new(AdmittedRequestTaskState::new(std::time::Instant::now()));
        let task_state_for_task = task_state.clone();
        let task_request_id = rid.clone();
        let task_request_ip = rip.clone();
        // RCD-C16: one independent capture session per fan-out sub-request,
        // keyed by the sub-request id, sharing the one downstream raw input
        // (RCD-D4b). Image API dumps record is_stream false (RCD-D2c).
        let capture_session = state
            .request_capture
            .maybe_start_session(
                &state.monoize_runtime,
                &auth,
                rid.clone(),
                capture_protocol,
                false,
            )
            .await;
        let capture = super::RequestCaptureContext {
            raw_input: capture_raw_input.clone(),
            session: capture_session,
        };

        let abort_handle = join_set.spawn(async move {
            execute_image_subrequest_typed(
                &state,
                &auth,
                req,
                max_multiplier,
                rid,
                rip,
                task_session_id,
                capture,
                &task_state_for_task,
            )
            .await
        });
        task_contexts.insert(
            abort_handle.id(),
            (task_state, task_request_id, task_request_ip),
        );
    }

    let mut results = Vec::with_capacity(n);
    while let Some(join_result) = join_set.join_next_with_id().await {
        match join_result {
            Ok((task_id, inner)) => {
                task_contexts.remove(&task_id);
                results.push(inner);
            }
            Err(join_error) => {
                let task_context = task_contexts.remove(&join_error.id());
                let code = if join_error.is_cancelled() {
                    "task_cancelled"
                } else {
                    "task_panic"
                };
                let err = AppError::new(
                    StatusCode::INTERNAL_SERVER_ERROR,
                    code,
                    format!("image sub-request task failed: {join_error}"),
                )
                .with_type("server_error");
                if let Some((task_state, task_request_id, task_request_ip)) = task_context
                    && let Some((started_at, is_stream, attempt)) = task_state.terminal_snapshot()
                {
                    if let Some(attempt) = attempt {
                        spawn_request_log_error(
                            state,
                            auth,
                            &attempt,
                            model,
                            is_stream,
                            started_at,
                            task_request_id,
                            task_request_ip,
                            &err,
                            None,
                            Vec::new(),
                        );
                    } else {
                        spawn_request_log_error_no_attempt(
                            state,
                            auth,
                            model,
                            is_stream,
                            started_at,
                            task_request_id,
                            task_request_ip,
                            &err,
                            None,
                            Vec::new(),
                        );
                    }
                }
                results.push(Err(err));
            }
        }
    }
    results
}

#[allow(clippy::too_many_arguments)]
async fn execute_image_subrequest_typed(
    state: &AppState,
    auth: &crate::auth::AuthResult,
    req: urp::UrpRequest,
    max_multiplier: Option<Multiplier>,
    request_id: Option<String>,
    request_ip: Option<String>,
    client_session_id: Option<String>,
    capture: super::RequestCaptureContext,
    task_state: &AdmittedRequestTaskState,
) -> AppResult<(urp::UrpResponse, String)> {
    let routing_stub = build_routing_stub(&req, max_multiplier);
    let mut attempts = build_monoize_attempts(state, &routing_stub, auth).await?;
    attach_client_session_id(&mut attempts, client_session_id.clone(), Some(&req));
    let all_responses = !attempts.is_empty()
        && attempts
            .iter()
            .all(|attempt| attempt.provider_type == ProviderType::Responses);
    task_state.set_stream(all_responses);

    if all_responses {
        return execute_stream_collected_image_typed(
            state,
            auth,
            req,
            max_multiplier,
            request_id,
            request_ip,
            client_session_id,
            capture,
            task_state,
            None,
        )
        .await;
    }

    execute_nonstream_typed_with_validator(
        state,
        auth,
        req,
        max_multiplier,
        super::DownstreamProtocol::Responses,
        request_id,
        request_ip,
        client_session_id,
        capture,
        Some(validate_image_subrequest_response),
        Some(task_state),
    )
    .await
}

#[allow(clippy::too_many_arguments)]
async fn finish_image_stream_error(
    state: &AppState,
    auth: &crate::auth::AuthResult,
    attempt: &MonoizeAttempt,
    logical_model: &str,
    started_at: std::time::Instant,
    request_id: &Option<String>,
    request_ip: &Option<String>,
    reasoning_effort: Option<String>,
    tried_providers: Vec<TriedProvider>,
    capture: &super::RequestCaptureContext,
    error: AppError,
) -> AppError {
    spawn_request_log_error(
        state,
        auth,
        attempt,
        logical_model,
        true,
        started_at,
        request_id.clone(),
        request_ip.clone(),
        &error,
        reasoning_effort,
        tried_providers,
    );
    if let Some(session) = capture.session.as_ref() {
        session.persist_with_result(None, false).await;
    }
    error
}

/// Record one RCD-D10c stream-collected attempt into the capture session:
/// `downstream_response` and `downstream_sse_frames` stay null, and the
/// pre-transform collected terminal event fills `reconstructed_urp_response`.
#[allow(clippy::too_many_arguments)]
async fn push_image_stream_attempt(
    capture: &super::RequestCaptureContext,
    attempt_number: u32,
    attempt: &MonoizeAttempt,
    logical_model: &str,
    req_attempt: &urp::UrpRequest,
    path: &str,
    upstream_body: &Value,
    reconstructed_urp_response: Option<Value>,
    transform_chain: Value,
    error: Option<&AppError>,
) {
    let Some(session) = capture.session.as_ref() else {
        return;
    };
    session
        .push_attempt(crate::request_capture::build_attempt_dump(
            attempt_number,
            &attempt.provider_id,
            Some(&attempt.channel_id),
            attempt.provider_type,
            logical_model,
            &req_attempt.model,
            path,
            capture.raw_input.as_ref().clone(),
            req_attempt,
            upstream_body.clone(),
            None,
            reconstructed_urp_response,
            None,
            transform_chain,
            error.map(|err| {
                json!({
                    "message": err.message,
                    "code": err.code,
                    "status": err.status.as_u16(),
                })
            }),
        ))
        .await;
}

#[allow(clippy::too_many_arguments)]
async fn execute_stream_collected_image_typed(
    state: &AppState,
    auth: &crate::auth::AuthResult,
    mut req: urp::UrpRequest,
    max_multiplier: Option<Multiplier>,
    request_id: Option<String>,
    request_ip: Option<String>,
    client_session_id: Option<String>,
    capture: super::RequestCaptureContext,
    task_state: &AdmittedRequestTaskState,
    mut sink: Option<&mut ImageStreamSink>,
) -> AppResult<(urp::UrpResponse, String)> {
    let started_at = task_state.started_at();
    let transform_match_model = resolve_model_suffix(state, &mut req).await?;
    let original_req = req.clone();
    let logical_model = req.model.clone();
    let routing_stub = build_routing_stub(&req, max_multiplier);
    let mut attempts = build_monoize_attempts(state, &routing_stub, auth).await?;
    attach_client_session_id(&mut attempts, client_session_id, Some(&req));
    ensure_balance_before_forward_for_attempts(state, auth, &attempts).await?;
    let pending_request_log_guard = insert_pending_request_log(
        state,
        auth,
        &req.model,
        true,
        request_id.as_deref(),
        request_ip.as_deref(),
        started_at,
    )
    .await?;
    task_state.retain_pending_guard(pending_request_log_guard);

    let mut last_failed_attempt: Option<MonoizeAttempt> = None;
    let mut tried_providers: Vec<TriedProvider> = Vec::new();
    let mut execution_state = AttemptExecutionState::default();

    for attempt in attempts {
        if execution_state.should_skip(&attempt) {
            continue;
        }

        let max_channel_attempts = same_channel_attempt_slots(&attempt);
        'channel_attempts: for channel_attempt in 0..max_channel_attempts {
            if execution_state.should_skip(&attempt) {
                break;
            }

            let attempt_number = execution_state.record_upstream_attempt(&attempt);
            task_state.set_attempt(&attempt);
            let mut req_attempt = original_req.clone();
            if let Some(target_protocol) = super::provider_type_protocol(attempt.provider_type) {
                urp::retain_provider_items_for_protocol(&mut req_attempt.input, target_protocol);
                if target_protocol == urp::ProviderProtocol::Responses {
                    urp::remove_downstream_only_reasoning_for_responses(&mut req_attempt.input);
                }
            }
            if attempt.strip_cross_protocol_nested_extra
                && !super::DownstreamProtocol::Responses.is_same_family(attempt.provider_type)
            {
                urp::strip_nested_extra_body(&mut req_attempt.input);
            }
            inject_monoize_context(auth, &mut req_attempt);
            req_attempt.model = attempt.upstream_model.clone();
            if let Err(err) = apply_transform_rules_request(
                state,
                &mut req_attempt,
                &attempt.provider_transforms,
                &transform_match_model,
                Some(attempt.provider_type),
            )
            .await
            {
                return Err(finish_image_stream_error(
                    state,
                    auth,
                    &attempt,
                    &logical_model,
                    started_at,
                    &request_id,
                    &request_ip,
                    req.reasoning.as_ref().and_then(|r| r.effort.clone()),
                    tried_providers,
                    &capture,
                    err,
                )
                .await);
            }
            let global_transforms = state.monoize_runtime.read().await.global_transforms.clone();
            if let Err(err) = apply_transform_rules_request(
                state,
                &mut req_attempt,
                &global_transforms,
                &transform_match_model,
                Some(attempt.provider_type),
            )
            .await
            {
                return Err(finish_image_stream_error(
                    state,
                    auth,
                    &attempt,
                    &logical_model,
                    started_at,
                    &request_id,
                    &request_ip,
                    req.reasoning.as_ref().and_then(|r| r.effort.clone()),
                    tried_providers,
                    &capture,
                    err,
                )
                .await);
            }
            if let Err(err) = apply_transform_rules_request(
                state,
                &mut req_attempt,
                &auth.transforms,
                &transform_match_model,
                Some(attempt.provider_type),
            )
            .await
            {
                return Err(finish_image_stream_error(
                    state,
                    auth,
                    &attempt,
                    &logical_model,
                    started_at,
                    &request_id,
                    &request_ip,
                    req.reasoning.as_ref().and_then(|r| r.effort.clone()),
                    tried_providers,
                    &capture,
                    err,
                )
                .await);
            }
            strip_monoize_context(&mut req_attempt);
            let capture_transform_chain = crate::request_capture::build_transform_chain(
                &attempt.provider_transforms,
                &global_transforms,
                &auth.transforms,
                &transform_match_model,
            );
            req_attempt.stream = Some(true);

            let upstream_body = match encode_request_for_provider(
                &mut req_attempt,
                &attempt,
                super::DownstreamProtocol::Responses,
            ) {
                Ok(body) => body,
                Err(err) => {
                    return Err(finish_image_stream_error(
                        state,
                        auth,
                        &attempt,
                        &logical_model,
                        started_at,
                        &request_id,
                        &request_ip,
                        req.reasoning.as_ref().and_then(|r| r.effort.clone()),
                        tried_providers,
                        &capture,
                        err,
                    )
                    .await);
                }
            };
            let http = client_http_for_attempt(state, &attempt)?;
            // OIU-S7/IS9: openai_image edits stream through multipart
            // `/v1/images/edits`; every other attempt posts the JSON body.
            let stream_call = match call_streaming_image_capable_upstream(
                &http,
                &attempt,
                &req_attempt,
                &upstream_body,
                attempt.request_timeout_ms.saturating_mul(10).max(600_000),
                &attempt_extra_headers(&attempt, &upstream_body),
                capture.session.is_some(),
            )
            .await
            {
                Ok(stream_call) => stream_call,
                Err(err) => {
                    return Err(finish_image_stream_error(
                        state,
                        auth,
                        &attempt,
                        &logical_model,
                        started_at,
                        &request_id,
                        &request_ip,
                        req.reasoning.as_ref().and_then(|r| r.effort.clone()),
                        tried_providers,
                        &capture,
                        err,
                    )
                    .await);
                }
            };
            let path = stream_call.path;
            // RCD-D6a/OIU-E5g: a multipart edit attempt records the sent form
            // as `upstream_request` instead of the unused JSON encoding.
            let capture_upstream_request = stream_call
                .capture_multipart_request
                .unwrap_or_else(|| upstream_body.clone());
            let call = stream_call.result;

            match call {
                Ok(upstream_resp) => {
                    update_pending_channel_info(
                        state,
                        auth,
                        &attempt,
                        &logical_model,
                        true,
                        request_id.as_deref(),
                        request_ip.as_deref(),
                        started_at,
                    )
                    .await;
                    let legacy = match typed_request_to_legacy(&req_attempt, max_multiplier) {
                        Ok(legacy) => legacy,
                        Err(err) => {
                            return Err(finish_image_stream_error(
                                state,
                                auth,
                                &attempt,
                                &logical_model,
                                started_at,
                                &request_id,
                                &request_ip,
                                req.reasoning.as_ref().and_then(|r| r.effort.clone()),
                                tried_providers,
                                &capture,
                                err,
                            )
                            .await);
                        }
                    };
                    let pending_request_envelope_extra =
                        req.input.clone().into_iter().find_map(|node| match node {
                            crate::urp::Node::NextDownstreamEnvelopeExtra { extra_body }
                                if !extra_body.is_empty() =>
                            {
                                Some(extra_body)
                            }
                            _ => None,
                        });

                    let (decoded_tx, decoded_rx) = mpsc::channel::<crate::urp::UrpStreamEvent>(64);
                    let (transformed_tx, mut transformed_rx) =
                        mpsc::channel::<crate::urp::UrpStreamEvent>(64);
                    let runtime_metrics = Arc::new(Mutex::new(StreamRuntimeMetrics::default()));
                    let stream_idle_timeout_ms = state
                        .monoize_runtime
                        .read()
                        .await
                        .stream_idle_timeout_ms
                        .max(1);

                    let decode_handle = {
                        let runtime_metrics = runtime_metrics.clone();
                        let provider_type = attempt.provider_type;
                        tokio::spawn(async move {
                            crate::urp::stream_decode::stream_upstream_to_urp_events(
                                &legacy,
                                pending_request_envelope_extra,
                                provider_type,
                                upstream_resp,
                                decoded_tx,
                                Some(started_at),
                                Some(runtime_metrics),
                                stream_idle_timeout_ms,
                            )
                            .await
                        })
                    };

                    // RCD-D10c: tap the decoded stream BEFORE response-phase
                    // transforms; the collected terminal event substitutes for
                    // the missing provider response body in the dump.
                    let reconstruction_slot = capture
                        .session
                        .as_ref()
                        .map(|_| Arc::new(Mutex::new(None::<Value>)));
                    let (transform_input_rx, reconstruct_handle) = match reconstruction_slot.clone()
                    {
                        Some(slot) => {
                            let (tap_tx, tap_rx) = mpsc::channel::<crate::urp::UrpStreamEvent>(64);
                            let handle = tokio::spawn(async move {
                                retain_reconstructed_urp_response(decoded_rx, tap_tx, slot).await
                            });
                            (tap_rx, Some(handle))
                        }
                        None => (decoded_rx, None),
                    };

                    let provider_rules = attempt.provider_transforms.clone();
                    let global_rules = global_transforms.clone();
                    let auth_rules = auth.transforms.clone();
                    let state_for_transform = state.clone();
                    let model_for_transform = logical_model.clone();
                    let transform_provider_type = attempt.provider_type;
                    let transform_handle = tokio::spawn(async move {
                        transform_urp_stream(
                            &state_for_transform,
                            transform_input_rx,
                            transformed_tx,
                            &provider_rules,
                            &global_rules,
                            &auth_rules,
                            &model_for_transform,
                            Some(transform_provider_type),
                            None,
                        )
                        .await
                    });

                    let mut final_response: Option<urp::UrpResponse> = None;
                    let mut stream_error: Option<AppError> = None;

                    while let Some(event) = transformed_rx.recv().await {
                        match event {
                            crate::urp::UrpStreamEvent::NodeDelta {
                                delta: crate::urp::NodeDelta::Image { source },
                                extra_body,
                                ..
                            } => {
                                // IS3: partial image deltas surface downstream
                                // only when the Image API sink is attached.
                                if let Some(sink) = sink.as_deref_mut() {
                                    sink.send_partial(&source, &extra_body).await;
                                }
                            }
                            crate::urp::UrpStreamEvent::ResponseDone {
                                finish_reason,
                                usage,
                                output,
                                extra_body,
                            } => {
                                final_response = Some(urp::UrpResponse {
                                    id: extra_body
                                        .get("id")
                                        .and_then(|value| value.as_str())
                                        .unwrap_or("resp_stream_collected")
                                        .to_string(),
                                    model: extra_body
                                        .get("model")
                                        .and_then(|value| value.as_str())
                                        .unwrap_or(&logical_model)
                                        .to_string(),
                                    created_at: extra_body
                                        .get("created_at")
                                        .and_then(|value| value.as_i64()),
                                    output,
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

                    let (decode_join, transform_join) =
                        tokio::join!(decode_handle, transform_handle);
                    let decode_result = match decode_join {
                        Ok(result) => result,
                        Err(join_error) => {
                            let err = AppError::new(
                                StatusCode::INTERNAL_SERVER_ERROR,
                                if join_error.is_cancelled() {
                                    "task_cancelled"
                                } else {
                                    "task_panic"
                                },
                                format!("image stream decoder task failed: {join_error}"),
                            )
                            .with_type("server_error");
                            return Err(finish_image_stream_error(
                                state,
                                auth,
                                &attempt,
                                &logical_model,
                                started_at,
                                &request_id,
                                &request_ip,
                                req.reasoning.as_ref().and_then(|r| r.effort.clone()),
                                tried_providers,
                                &capture,
                                err,
                            )
                            .await);
                        }
                    };
                    let transform_result = match transform_join {
                        Ok(result) => result,
                        Err(join_error) => Err(AppError::new(
                            StatusCode::INTERNAL_SERVER_ERROR,
                            if join_error.is_cancelled() {
                                "task_cancelled"
                            } else {
                                "task_panic"
                            },
                            format!("image stream transform task failed: {join_error}"),
                        )
                        .with_type("server_error")),
                    };
                    if let Some(handle) = reconstruct_handle {
                        let _ = handle.await;
                    }
                    let reconstructed_urp_response = match reconstruction_slot.as_ref() {
                        Some(slot) => slot.lock().await.clone(),
                        None => None,
                    };
                    if let Err(err) = decode_result {
                        push_image_stream_attempt(
                            &capture,
                            attempt_number,
                            &attempt,
                            &logical_model,
                            &req_attempt,
                            &path,
                            &capture_upstream_request,
                            reconstructed_urp_response.clone(),
                            capture_transform_chain.clone(),
                            Some(&err),
                        )
                        .await;
                        let same_channel_retryable = is_same_channel_retryable_app_error(&err);
                        let passive_failure_class =
                            same_channel_retryable.then(|| classify_retryable_app_failure(&err));
                        record_upstream_attempt_failure(
                            state,
                            &attempt,
                            attempt_number,
                            &err,
                            passive_failure_class,
                            &mut tried_providers,
                            &mut execution_state,
                        )
                        .await;
                        last_failed_attempt = Some(attempt.clone());
                        if sink_frames_emitted(&sink) {
                            // IS7: image frames already reached the client, so failover
                            // would duplicate them; the stream terminates with this error.
                            return Err(finish_image_stream_error(
                                state,
                                auth,
                                &attempt,
                                &logical_model,
                                started_at,
                                &request_id,
                                &request_ip,
                                req.reasoning.as_ref().and_then(|r| r.effort.clone()),
                                tried_providers,
                                &capture,
                                err,
                            )
                            .await);
                        }
                        if allow_same_channel_retry(
                            state,
                            &attempt,
                            &execution_state,
                            channel_attempt + 1,
                            passive_failure_class,
                        )
                        .await
                        {
                            maybe_sleep_before_channel_retry(&attempt).await;
                            continue 'channel_attempts;
                        }
                        break 'channel_attempts;
                    }
                    if let Err(err) = transform_result {
                        push_image_stream_attempt(
                            &capture,
                            attempt_number,
                            &attempt,
                            &logical_model,
                            &req_attempt,
                            &path,
                            &capture_upstream_request,
                            reconstructed_urp_response.clone(),
                            capture_transform_chain.clone(),
                            Some(&err),
                        )
                        .await;
                        return Err(finish_image_stream_error(
                            state,
                            auth,
                            &attempt,
                            &logical_model,
                            started_at,
                            &request_id,
                            &request_ip,
                            req.reasoning.as_ref().and_then(|r| r.effort.clone()),
                            tried_providers,
                            &capture,
                            err,
                        )
                        .await);
                    }
                    if let Some(err) = stream_error {
                        push_image_stream_attempt(
                            &capture,
                            attempt_number,
                            &attempt,
                            &logical_model,
                            &req_attempt,
                            &path,
                            &capture_upstream_request,
                            reconstructed_urp_response.clone(),
                            capture_transform_chain.clone(),
                            Some(&err),
                        )
                        .await;
                        let same_channel_retryable = is_same_channel_retryable_app_error(&err);
                        let passive_failure_class =
                            same_channel_retryable.then(|| classify_retryable_app_failure(&err));
                        record_upstream_attempt_failure(
                            state,
                            &attempt,
                            attempt_number,
                            &err,
                            passive_failure_class,
                            &mut tried_providers,
                            &mut execution_state,
                        )
                        .await;
                        last_failed_attempt = Some(attempt.clone());
                        if sink_frames_emitted(&sink) {
                            // IS7: image frames already reached the client, so failover
                            // would duplicate them; the stream terminates with this error.
                            return Err(finish_image_stream_error(
                                state,
                                auth,
                                &attempt,
                                &logical_model,
                                started_at,
                                &request_id,
                                &request_ip,
                                req.reasoning.as_ref().and_then(|r| r.effort.clone()),
                                tried_providers,
                                &capture,
                                err,
                            )
                            .await);
                        }
                        if allow_same_channel_retry(
                            state,
                            &attempt,
                            &execution_state,
                            channel_attempt + 1,
                            passive_failure_class,
                        )
                        .await
                        {
                            maybe_sleep_before_channel_retry(&attempt).await;
                            continue 'channel_attempts;
                        }
                        break 'channel_attempts;
                    }

                    let resp = match final_response {
                        Some(resp) => resp,
                        None => {
                            let err = AppError::new(
                                StatusCode::BAD_GATEWAY,
                                "upstream_stream_error",
                                "stream completed without terminal response",
                            );
                            push_image_stream_attempt(
                                &capture,
                                attempt_number,
                                &attempt,
                                &logical_model,
                                &req_attempt,
                                &path,
                                &capture_upstream_request,
                                reconstructed_urp_response.clone(),
                                capture_transform_chain.clone(),
                                Some(&err),
                            )
                            .await;
                            let same_channel_retryable = is_same_channel_retryable_app_error(&err);
                            let passive_failure_class = same_channel_retryable
                                .then(|| classify_retryable_app_failure(&err));
                            record_upstream_attempt_failure(
                                state,
                                &attempt,
                                attempt_number,
                                &err,
                                passive_failure_class,
                                &mut tried_providers,
                                &mut execution_state,
                            )
                            .await;
                            last_failed_attempt = Some(attempt.clone());
                            if sink_frames_emitted(&sink) {
                                // IS7: image frames already reached the client, so failover
                                // would duplicate them; the stream terminates with this error.
                                return Err(finish_image_stream_error(
                                    state,
                                    auth,
                                    &attempt,
                                    &logical_model,
                                    started_at,
                                    &request_id,
                                    &request_ip,
                                    req.reasoning.as_ref().and_then(|r| r.effort.clone()),
                                    tried_providers,
                                    &capture,
                                    err,
                                )
                                .await);
                            }
                            if allow_same_channel_retry(
                                state,
                                &attempt,
                                &execution_state,
                                channel_attempt + 1,
                                passive_failure_class,
                            )
                            .await
                            {
                                maybe_sleep_before_channel_retry(&attempt).await;
                                continue 'channel_attempts;
                            }
                            break 'channel_attempts;
                        }
                    };

                    if let Err(err) = validate_image_subrequest_response(&resp) {
                        push_image_stream_attempt(
                            &capture,
                            attempt_number,
                            &attempt,
                            &logical_model,
                            &req_attempt,
                            &path,
                            &capture_upstream_request,
                            reconstructed_urp_response.clone(),
                            capture_transform_chain.clone(),
                            Some(&err),
                        )
                        .await;
                        let same_channel_retryable = is_same_channel_retryable_app_error(&err);
                        let passive_failure_class =
                            same_channel_retryable.then(|| classify_retryable_app_failure(&err));
                        record_upstream_attempt_failure(
                            state,
                            &attempt,
                            attempt_number,
                            &err,
                            passive_failure_class,
                            &mut tried_providers,
                            &mut execution_state,
                        )
                        .await;
                        last_failed_attempt = Some(attempt.clone());
                        if sink_frames_emitted(&sink) {
                            // IS7: image frames already reached the client, so failover
                            // would duplicate them; the stream terminates with this error.
                            return Err(finish_image_stream_error(
                                state,
                                auth,
                                &attempt,
                                &logical_model,
                                started_at,
                                &request_id,
                                &request_ip,
                                req.reasoning.as_ref().and_then(|r| r.effort.clone()),
                                tried_providers,
                                &capture,
                                err,
                            )
                            .await);
                        }
                        if allow_same_channel_retry(
                            state,
                            &attempt,
                            &execution_state,
                            channel_attempt + 1,
                            passive_failure_class,
                        )
                        .await
                        {
                            maybe_sleep_before_channel_retry(&attempt).await;
                            continue 'channel_attempts;
                        }
                        break 'channel_attempts;
                    }

                    // MP-F3: a fail-closed missing-usage billable success
                    // rejects with 403 before response delivery.
                    if resp.usage.is_none() && missing_usage_rejects(auth, &attempt) {
                        let err = missing_usage_error();
                        push_image_stream_attempt(
                            &capture,
                            attempt_number,
                            &attempt,
                            &logical_model,
                            &req_attempt,
                            &path,
                            &capture_upstream_request,
                            reconstructed_urp_response.clone(),
                            capture_transform_chain.clone(),
                            Some(&err),
                        )
                        .await;
                        return Err(finish_image_stream_error(
                            state,
                            auth,
                            &attempt,
                            &logical_model,
                            started_at,
                            &request_id,
                            &request_ip,
                            req.reasoning.as_ref().and_then(|r| r.effort.clone()),
                            tried_providers,
                            &capture,
                            err,
                        )
                        .await);
                    }

                    push_image_stream_attempt(
                        &capture,
                        attempt_number,
                        &attempt,
                        &logical_model,
                        &req_attempt,
                        &path,
                        &capture_upstream_request,
                        reconstructed_urp_response.clone(),
                        capture_transform_chain.clone(),
                        None,
                    )
                    .await;
                    mark_channel_success(state, &attempt).await;
                    refresh_channel_affinity(state, &attempt).await;
                    let charge = match maybe_charge_response(
                        state,
                        auth,
                        &attempt,
                        &logical_model,
                        &resp,
                        request_id.as_deref(),
                    )
                    .await
                    {
                        Ok(charge) => charge,
                        Err(err) => {
                            return Err(finish_image_stream_error(
                                state,
                                auth,
                                &attempt,
                                &logical_model,
                                started_at,
                                &request_id,
                                &request_ip,
                                req.reasoning.as_ref().and_then(|r| r.effort.clone()),
                                tried_providers,
                                &capture,
                                err,
                            )
                            .await);
                        }
                    };
                    spawn_request_log(
                        state,
                        auth,
                        &attempt,
                        &logical_model,
                        resp.usage.clone(),
                        charge.charge_nano_usd,
                        charge.billing_breakdown,
                        true,
                        started_at,
                        request_id.clone(),
                        request_ip.clone(),
                        attempt.channel_id.clone(),
                        None,
                        None,
                        req.reasoning.as_ref().and_then(|r| r.effort.clone()),
                        tried_providers,
                        task_state.client_gone(),
                    );
                    if let Some(session) = capture.session.as_ref() {
                        session
                            .persist_with_result(resp.usage.as_ref(), false)
                            .await;
                    }
                    return Ok((resp, logical_model.clone()));
                }
                Err(err) => {
                    let same_channel_retryable = is_same_channel_retryable_error(&err);
                    let passive_failure_class =
                        same_channel_retryable.then(|| classify_retryable_failure(&err));
                    let mask_sensitive_info =
                        state.monoize_runtime.read().await.mask_sensitive_info;
                    let app_err = upstream_error_to_app(err, mask_sensitive_info);
                    push_image_stream_attempt(
                        &capture,
                        attempt_number,
                        &attempt,
                        &logical_model,
                        &req_attempt,
                        &path,
                        &capture_upstream_request,
                        None,
                        capture_transform_chain.clone(),
                        Some(&app_err),
                    )
                    .await;
                    record_upstream_attempt_failure(
                        state,
                        &attempt,
                        attempt_number,
                        &app_err,
                        passive_failure_class,
                        &mut tried_providers,
                        &mut execution_state,
                    )
                    .await;
                    last_failed_attempt = Some(attempt.clone());
                    if sink_frames_emitted(&sink) {
                        // IS7: image frames already reached the client, so failover
                        // would duplicate them; the stream terminates with this error.
                        return Err(finish_image_stream_error(
                            state,
                            auth,
                            &attempt,
                            &logical_model,
                            started_at,
                            &request_id,
                            &request_ip,
                            req.reasoning.as_ref().and_then(|r| r.effort.clone()),
                            tried_providers,
                            &capture,
                            app_err,
                        )
                        .await);
                    }
                    if allow_same_channel_retry(
                        state,
                        &attempt,
                        &execution_state,
                        channel_attempt + 1,
                        passive_failure_class,
                    )
                    .await
                    {
                        maybe_sleep_before_channel_retry(&attempt).await;
                        continue;
                    }
                    break;
                }
            }
        }
    }

    let final_err = build_exhausted_upstream_error(&logical_model, &tried_providers);
    if let Some(attempt) = last_failed_attempt {
        spawn_request_log_error(
            state,
            auth,
            &attempt,
            &logical_model,
            true,
            started_at,
            request_id,
            request_ip,
            &final_err,
            req.reasoning.as_ref().and_then(|r| r.effort.clone()),
            tried_providers,
        );
    } else {
        spawn_request_log_error_no_attempt(
            state,
            auth,
            &logical_model,
            true,
            started_at,
            request_id,
            request_ip,
            &final_err,
            req.reasoning.as_ref().and_then(|r| r.effort.clone()),
            tried_providers,
        );
    }
    if let Some(session) = capture.session.as_ref() {
        session.persist_with_result(None, true).await;
    }
    Err(final_err)
}

/// Represents one extracted image from a URP response.
fn collect_response_text(resp: &urp::UrpResponse) -> String {
    let mut parts = Vec::new();
    for item in &resp.output {
        match item {
            urp::Node::Text { content, .. } | urp::Node::Refusal { content, .. }
                if !content.trim().is_empty() =>
            {
                parts.push(content.as_str());
            }
            _ => {}
        }
    }
    parts.join("\n")
}

struct ExtractedImage {
    b64_json: Option<String>,
    url: Option<String>,
    revised_prompt: Option<String>,
}

fn extract_images_from_response(resp: &urp::UrpResponse) -> Vec<ExtractedImage> {
    let mut images = Vec::new();
    let mut text_parts = Vec::new();
    let mut seen_base64 = std::collections::HashSet::new();
    let mut seen_urls = std::collections::HashSet::new();

    for item in &resp.output {
        match item {
            urp::Node::Image { source, .. } => match source {
                urp::ImageSource::Base64 { data, .. } => {
                    if !seen_base64.insert(data.clone()) {
                        continue;
                    }
                    images.push(ExtractedImage {
                        b64_json: Some(data.clone()),
                        url: None,
                        revised_prompt: None,
                    });
                }
                urp::ImageSource::Url { url, .. } => {
                    if !seen_urls.insert(url.clone()) {
                        continue;
                    }
                    images.push(ExtractedImage {
                        b64_json: None,
                        url: Some(url.clone()),
                        revised_prompt: None,
                    });
                }
                urp::ImageSource::FileId { .. } => continue,
            },
            urp::Node::Text {
                role: urp::OrdinaryRole::Assistant,
                content,
                ..
            } if !content.trim().is_empty() => {
                text_parts.push(content.clone());
            }
            _ => {}
        }
    }

    if !text_parts.is_empty() && !images.is_empty() {
        let revised = text_parts.join("");
        for img in &mut images {
            img.revised_prompt = Some(revised.clone());
        }
    }

    images
}

fn validate_image_subrequest_response(resp: &urp::UrpResponse) -> AppResult<()> {
    if !extract_images_from_response(resp).is_empty() {
        return Ok(());
    }
    let upstream_text = collect_response_text(resp);
    let detail = if upstream_text.is_empty() {
        "upstream response contained no images".to_string()
    } else {
        format!("upstream response contained no images. upstream output: {upstream_text}")
    };
    Err(AppError::new(
        StatusCode::BAD_GATEWAY,
        "upstream_error",
        detail,
    ))
}

fn assemble_image_response(
    results: Vec<Result<(urp::UrpResponse, String), AppError>>,
) -> AppResult<Response> {
    let mut data_items: Vec<Value> = Vec::new();
    let mut last_error: Option<AppError> = None;
    let mut total_usage: Option<AggregatedUsage> = None;

    for result in results {
        match result {
            Ok((resp, _logical_model)) => {
                if let Err(err) = validate_image_subrequest_response(&resp) {
                    last_error = Some(err);
                    continue;
                }
                let images = extract_images_from_response(&resp);
                for img in images {
                    let mut item = Map::new();
                    if let Some(b64) = img.b64_json {
                        item.insert("b64_json".to_string(), Value::String(b64));
                    }
                    if let Some(url) = img.url {
                        item.insert("url".to_string(), Value::String(url));
                    }
                    if let Some(revised) = img.revised_prompt {
                        item.insert("revised_prompt".to_string(), Value::String(revised));
                    }
                    data_items.push(Value::Object(item));
                }
                if let Some(usage) = &resp.usage {
                    accumulate_usage(total_usage.get_or_insert(AggregatedUsage::default()), usage);
                }
            }
            Err(e) => {
                last_error = Some(e);
            }
        }
    }

    if data_items.is_empty() {
        return Err(last_error.unwrap_or_else(|| {
            AppError::new(
                StatusCode::BAD_GATEWAY,
                "upstream_error",
                "no images generated",
            )
        }));
    }

    let created = chrono::Utc::now().timestamp();
    let mut response = json!({
        "created": created,
        "data": data_items,
    });

    if let Some(usage) = total_usage {
        response
            .as_object_mut()
            .unwrap()
            .insert("usage".to_string(), aggregated_usage_value(&usage));
    }

    Ok(Json(response).into_response())
}

#[derive(Default)]
struct AggregatedUsage {
    input_tokens: u64,
    output_tokens: u64,
    input_text_tokens: u64,
    input_image_tokens: u64,
    input_cached_tokens: u64,
    input_cached_text_tokens: u64,
    input_cached_image_tokens: u64,
    output_text_tokens: u64,
    output_image_tokens: u64,
}

fn accumulate_usage(agg: &mut AggregatedUsage, usage: &urp::Usage) {
    agg.input_tokens += usage.input_tokens;
    agg.output_tokens += usage.output_tokens;
    if let Some(details) = &usage.input_details {
        agg.input_cached_tokens += details.cache_read_tokens;
        if let Some(cached_modality) = &details.cache_read_modality_breakdown {
            agg.input_cached_text_tokens += cached_modality.text_tokens.unwrap_or(0);
            agg.input_cached_image_tokens += cached_modality.image_tokens.unwrap_or(0);
        }
        if let Some(modality) = &details.modality_breakdown {
            agg.input_text_tokens += modality.text_tokens.unwrap_or(0);
            agg.input_image_tokens += modality.image_tokens.unwrap_or(0);
        }
    }
    if let Some(details) = &usage.output_details
        && let Some(modality) = &details.modality_breakdown
    {
        agg.output_text_tokens += modality.text_tokens.unwrap_or(0);
        agg.output_image_tokens += modality.image_tokens.unwrap_or(0);
    }
}

/// IR10 usage envelope shared by the non-streaming response body and the IS4
/// last completed frame.
fn aggregated_usage_value(usage: &AggregatedUsage) -> Value {
    json!({
        "input_tokens": usage.input_tokens,
        "output_tokens": usage.output_tokens,
        "total_tokens": usage.input_tokens + usage.output_tokens,
        "input_tokens_details": {
            "text_tokens": usage.input_text_tokens,
            "image_tokens": usage.input_image_tokens,
            "cached_tokens": usage.input_cached_tokens,
            "cached_tokens_details": {
                "text_tokens": usage.input_cached_text_tokens,
                "image_tokens": usage.input_cached_image_tokens,
            },
        },
        "output_tokens_details": {
            "text_tokens": usage.output_text_tokens,
            "image_tokens": usage.output_image_tokens,
        }
    })
}
