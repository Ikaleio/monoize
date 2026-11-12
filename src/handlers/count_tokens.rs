//! `POST /v1/messages/count_tokens`: Messages-only token counting pass-through
//! (`unified_responses_proxy.spec.md` §7.8.1, DMC1-DMC5).

use super::*;

pub async fn count_messages_tokens(
    State(state): State<AppState>,
    headers: HeaderMap,
    Json(body): Json<Value>,
) -> Response {
    let request_id = messages_wire_request_id(&canonical_request_id(&headers));
    let mut response = match count_messages_tokens_inner(&state, &headers, body, &request_id).await
    {
        Ok(response) => response,
        Err(err) => anthropic_error_response(err, Some(&request_id)),
    };
    insert_messages_request_id_header(&mut response, &request_id);
    response
}

fn messages_error_body(
    status: StatusCode,
    error_type: &str,
    message: &str,
    request_id: &str,
) -> Response {
    let body = json!({
        "type": "error",
        "error": { "type": error_type, "message": message },
        "request_id": request_id,
    });
    (status, Json(body)).into_response()
}

async fn count_messages_tokens_inner(
    state: &AppState,
    headers: &HeaderMap,
    mut body: Value,
    request_id: &str,
) -> AppResult<Response> {
    let auth = auth_tenant(headers, state).await?;
    let mut logical_model = body
        .as_object()
        .ok_or_else(|| {
            AppError::new(
                StatusCode::BAD_REQUEST,
                "invalid_request",
                "body must be object",
            )
        })?
        .get("model")
        .and_then(Value::as_str)
        .filter(|model| !model.trim().is_empty())
        .ok_or_else(|| AppError::new(StatusCode::BAD_REQUEST, "invalid_request", "missing model"))?
        .to_string();
    apply_configured_model_redirects_to_model(state, &mut logical_model, &auth).await;
    ensure_model_allowed(&auth, &logical_model)?;

    let routing_stub = build_embeddings_routing_stub(&logical_model, None);
    let mut attempts = build_monoize_attempts(state, &routing_stub, &auth).await?;
    attempts.retain(|attempt| attempt.provider_type == ProviderType::Messages);
    if attempts.is_empty() {
        // Claude Code treats 404 as "endpoint unsupported" and estimates locally (DMC2).
        return Ok(messages_error_body(
            StatusCode::NOT_FOUND,
            "not_found_error",
            &format!("token counting is not available for model {logical_model}"),
            request_id,
        ));
    }
    attach_client_session_id(&mut attempts, extract_client_session_id(headers), None);
    let client_headers = anthropic_client_headers(headers);
    for attempt in &mut attempts {
        attempt.client_anthropic_headers = client_headers.clone();
    }

    let mask_sensitive_info = state.monoize_runtime.read().await.mask_sensitive_info;
    let mut last_error: Option<AppError> = None;
    for attempt in attempts {
        body["model"] = Value::String(attempt.upstream_model.clone());
        let extra_headers = attempt_extra_headers(&attempt, &body);
        let provider = build_channel_provider_config(&attempt);
        let http = match client_http_for_attempt(state, &attempt) {
            Ok(http) => http,
            Err(error) => {
                last_error = Some(error);
                continue;
            }
        };
        match upstream::call_upstream_with_timeout_and_headers(
            &http,
            &provider,
            &attempt.api_key,
            "/v1/messages/count_tokens",
            upstream::JsonBody::new(&body),
            attempt.request_timeout_ms,
            &extra_headers,
        )
        .await
        {
            Ok(value) => return Ok(Json(value).into_response()),
            Err(err) => last_error = Some(upstream_error_to_app(err, mask_sensitive_info)),
        }
    }
    let err = last_error.expect("at least one count_tokens attempt ran");
    Ok(messages_error_body(
        err.status,
        err.upstream_type.as_deref().unwrap_or("api_error"),
        &err.message,
        request_id,
    ))
}
