use super::*;
use axum::extract::{OriginalUri, Query};
use axum::http::{Method, Uri};
use percent_encoding::percent_decode_str;

const V1_MODELS: &str = "/v1/models/";
const V1BETA_MODELS: &str = "/v1beta/models/";

pub(crate) fn model_tail(path: &str) -> Option<&str> {
    let path = path.strip_prefix("/api").unwrap_or(path);
    path.strip_prefix(V1BETA_MODELS)
        .or_else(|| path.strip_prefix(V1_MODELS))
}

pub(crate) fn is_gemini_request(method: &Method, path: &str) -> bool {
    match *method {
        Method::POST => model_tail(path).is_some(),
        Method::GET | Method::HEAD => is_gemini_discovery_path(path),
        _ => false,
    }
}

fn is_gemini_discovery_path(path: &str) -> bool {
    let path = path.strip_prefix("/api").unwrap_or(path);
    path == "/v1beta/models" || path.starts_with(V1BETA_MODELS)
}

#[derive(Clone)]
struct QueryKey(String);

pub(crate) async fn remove_query_key(
    mut request: Request,
    next: axum::middleware::Next,
) -> Response {
    let path = request.uri().path();
    if (model_tail(path).is_some() || is_gemini_discovery_path(path))
        && let Some(uri) = without_query_key(request.uri())
    {
        if let Ok(Query(pairs)) = Query::<Vec<(String, String)>>::try_from_uri(request.uri())
            && let Some((_, key)) = pairs.into_iter().find(|(name, _)| name == "key")
        {
            request.extensions_mut().insert(QueryKey(key));
        }
        *request.uri_mut() = uri;
        if let Some(OriginalUri(original)) = request.extensions_mut().get_mut::<OriginalUri>()
            && let Some(uri) = without_query_key(original)
        {
            *original = uri;
        }
    }
    next.run(request).await
}

fn without_query_key(uri: &Uri) -> Option<Uri> {
    let query = uri.query()?;
    let is_key = |pair: &str| {
        let name = pair.split_once('=').map_or(pair, |(name, _)| name);
        percent_decode_str(name).eq(b"key".iter().copied())
    };
    if !query.split('&').any(is_key) {
        return None;
    }
    let mut path = uri.path().to_owned();
    for (index, pair) in query.split('&').filter(|pair| !is_key(pair)).enumerate() {
        path.push(if index == 0 { '?' } else { '&' });
        path.push_str(pair);
    }
    let mut parts = uri.clone().into_parts();
    parts.path_and_query = Some(
        path.parse()
            .expect("removing query pairs preserves a valid URI"),
    );
    Some(Uri::from_parts(parts).expect("only the URI query changed"))
}

async fn authenticate(
    headers: &HeaderMap,
    extensions: &axum::http::Extensions,
    state: &AppState,
) -> AppResult<crate::auth::AuthResult> {
    let key = headers
        .get("x-goog-api-key")
        .and_then(|value| value.to_str().ok())
        .filter(|value| !value.is_empty())
        .or_else(|| extensions.get::<QueryKey>().map(|key| key.0.as_str()));
    auth_tenant_with_gemini_key(headers, state, key).await
}

pub(super) fn error_response(err: AppError) -> Response {
    let body = urp::encode::gemini::error_json(err.status, &err.message);
    let err = err.into_inner();
    let mut response = (err.status, Json(body)).into_response();
    crate::error::append_forwarded_headers(&mut response, err.forwarded_headers);
    response
}

/// DG1: the percent-decoded model id without one leading `models/`.
fn parse_model(raw: &str) -> AppResult<String> {
    let invalid = || {
        AppError::new(
            StatusCode::BAD_REQUEST,
            "invalid_request",
            "invalid model path",
        )
    };
    let model = percent_decode_str(raw)
        .decode_utf8()
        .map_err(|_| invalid())?;
    let model = model.strip_prefix("models/").unwrap_or(&model);
    if model.is_empty() {
        return Err(invalid());
    }
    Ok(model.to_owned())
}

pub(crate) async fn post_model(State(state): State<AppState>, request: Request) -> Response {
    post_model_inner(state, request)
        .await
        .unwrap_or_else(error_response)
}

async fn post_model_inner(state: AppState, request: Request) -> AppResult<Response> {
    let auth = authenticate(request.headers(), request.extensions(), &state).await?;
    let tail = model_tail(request.uri().path()).expect("Gemini model route");
    let (model, action) = tail.rsplit_once(':').ok_or_else(unknown_action)?;
    let (mode, count) = match action {
        "generateContent" => (None, false),
        "streamGenerateContent" => {
            let Query(query) = Query::<Vec<(String, String)>>::try_from_uri(request.uri())
                .map_err(|_| {
                    AppError::new(StatusCode::BAD_REQUEST, "invalid_request", "invalid query")
                })?;
            let framing = if query
                .iter()
                .any(|(key, value)| key == "alt" && value == "sse")
            {
                // GEM-9a: no heartbeat. `@google/genai` matches `^data: ` at the buffer start,
                // so a comment frame stalls it until it fails with "Incomplete JSON segment".
                StreamFraming::Sse(b"")
            } else {
                StreamFraming::GeminiJsonArray
            };
            (Some(framing), false)
        }
        "countTokens" => (None, true),
        _ => return Err(unknown_action()),
    };
    let model = parse_model(model)?;
    let headers = request.headers().clone();
    let crate::payload::JsonBody(body) =
        crate::payload::JsonBody::from_request(request, &state).await?;
    if count {
        let body = if let Some(request) = body
            .get("generateContentRequest")
            .or_else(|| body.get("generate_content_request"))
        {
            request
        } else if body.get("contents").is_some_and(Value::is_array) {
            &body
        } else {
            return Err(AppError::new(
                StatusCode::BAD_REQUEST,
                "invalid_request",
                "expected contents or generateContentRequest",
            ));
        };
        let mut req = decode_urp_body(DownstreamProtocol::Gemini, body)?;
        req.model = model;
        req.stream = Some(false);
        return count_tokens::forward_count(
            &state,
            &headers,
            &auth,
            DownstreamProtocol::Gemini,
            req,
        )
        .await;
    }
    let downstream = DownstreamProtocol::Gemini;
    let request_id = Some(canonical_request_id(&headers));
    let session = state
        .request_capture
        .maybe_start_session(
            &state.monoize_runtime,
            &auth,
            request_id.clone(),
            downstream.into(),
            mode.is_some(),
        )
        .await;
    let mut req = decode_urp_body(downstream, &body)?;
    req.model.clone_from(&model);
    req.context.downstream_model = Some(model);
    req.stream = Some(mode.is_some());
    apply_configured_model_redirects_to_model(&state, &mut req.model, &auth).await;
    ensure_model_allowed(&auth, &req.model)?;
    let max_multiplier = resolve_max_multiplier(&req, &headers, &auth);
    let request_ip = extract_client_ip(&headers);
    let session_id = extract_client_session_id(&headers);
    let capture = RequestCaptureContext {
        raw_input: Arc::new(if session.is_some() { body } else { Value::Null }),
        session,
    };
    if let Some(framing) = mode {
        return commit_forward_event_stream(downstream, request_id.clone(), framing, |commit| {
            forward_stream_typed(
                StreamForwardRequest {
                    state,
                    auth,
                    max_multiplier,
                    downstream,
                    request_id,
                    request_ip,
                    capture,
                    ws_turn: None,
                    commit,
                },
                req,
                session_id,
            )
        })
        .await;
    }
    let task_state = Arc::new(AdmittedRequestTaskState::new(Instant::now()));
    let watch = DownstreamGone(task_state.clone());
    let handle = crate::payload::spawn(async move {
        forward_nonstream_typed_with_task_state(
            NonstreamRequest {
                state: &state,
                auth: &auth,
                max_multiplier,
                downstream,
                request_id,
                request_ip,
                capture,
                task_state: Some(&task_state),
            },
            req,
            session_id,
        )
        .await
    });
    let (value, forwarded_headers) = join_while_client_present(handle, watch).await?;
    // DG2a: only the encoded error envelope overrides the served response's HTTP 200.
    let status = value
        .pointer("/error/code")
        .and_then(Value::as_u64)
        .and_then(|code| u16::try_from(code).ok())
        .and_then(|code| StatusCode::from_u16(code).ok());
    let mut response = crate::payload::json_response(value);
    if let Some(status) = status {
        *response.status_mut() = status;
    }
    crate::error::append_forwarded_headers(&mut response, forwarded_headers);
    Ok(response)
}

fn unknown_action() -> AppError {
    AppError::new(
        StatusCode::NOT_FOUND,
        "not_found",
        "unknown Gemini model action",
    )
}

pub(crate) async fn get_models(State(state): State<AppState>, request: Request) -> Response {
    get_models_inner(&state, request)
        .await
        .unwrap_or_else(error_response)
}

async fn get_models_inner(state: &AppState, request: Request) -> AppResult<Response> {
    let auth = authenticate(request.headers(), request.extensions(), state).await?;
    let models = visible_model_ids(state, &auth).await?;
    let metadata_error =
        |err| AppError::new(StatusCode::INTERNAL_SERVER_ERROR, "model_store_error", err);
    if let Some(tail) = model_tail(request.uri().path()) {
        let model = parse_model(tail)?;
        if !models.contains(&model) {
            return Err(AppError::new(
                StatusCode::NOT_FOUND,
                "not_found",
                "model not found",
            ));
        }
        let metadata = state
            .model_registry_store
            .get_model_metadata(&model)
            .await
            .map_err(metadata_error)?;
        return Ok(Json(model_descriptor(&model, metadata.as_ref())).into_response());
    }
    let metadata = state
        .model_registry_store
        .list_model_metadata()
        .await
        .map_err(metadata_error)?;
    let metadata: HashMap<_, _> = metadata
        .iter()
        .map(|row| (row.model_id.as_str(), row))
        .collect();
    let models: Vec<_> = models
        .iter()
        .map(|id| model_descriptor(id, metadata.get(id.as_str()).copied()))
        .collect();
    Ok(Json(json!({"models": models})).into_response())
}

fn model_descriptor(
    id: &str,
    metadata: Option<&crate::model_registry_store::DbModelMetadataRecord>,
) -> Value {
    let mut model = json!({
        "name": format!("models/{id}"),
        "displayName": id,
        "supportedGenerationMethods": ["generateContent", "streamGenerateContent", "countTokens"],
    });
    if let Some(metadata) = metadata {
        if let Some(limit) = metadata.max_input_tokens {
            model["inputTokenLimit"] = json!(limit);
        }
        if let Some(limit) = metadata.max_output_tokens {
            model["outputTokenLimit"] = json!(limit);
        }
    }
    model
}
