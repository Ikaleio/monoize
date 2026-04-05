use super::*;

fn resolve_replicate_upstream_path(upstream_model: &str) -> (String, Option<String>) {
    if let Some(stripped) = upstream_model.strip_prefix("deployment:") {
        let path = format!("/v1/deployments/{stripped}/predictions");
        return (path, None);
    }
    if let Some((_owner_model, _version_id)) = upstream_model.split_once(':') {
        let path = "/v1/predictions".to_string();
        return (path, Some(upstream_model.to_string()));
    }
    let path = format!("/v1/models/{upstream_model}/predictions");
    (path, None)
}

fn build_replicate_upstream_body(body: &Value, version: Option<&str>) -> Value {
    let mut upstream = body.clone();
    if let Some(obj) = upstream.as_object_mut() {
        obj.remove("model");
        obj.remove("max_multiplier");
        if let Some(ver) = version {
            obj.insert("version".to_string(), Value::String(ver.to_string()));
        }
    }
    upstream
}

async fn pick_any_replicate_channel(
    state: &AppState,
) -> AppResult<(crate::monoize_routing::MonoizeProvider, crate::monoize_routing::MonoizeChannel)> {
    let providers =
        state.monoize_store.list_providers().await.map_err(|e| {
            AppError::new(StatusCode::INTERNAL_SERVER_ERROR, "provider_store_error", e)
        })?;
    for provider in providers {
        if !provider.enabled {
            continue;
        }
        if provider.provider_type != crate::monoize_routing::MonoizeProviderType::Replicate {
            continue;
        }
        for channel in &provider.channels {
            if channel.enabled && channel.weight > 0 {
                return Ok((provider.clone(), channel.clone()));
            }
        }
    }
    Err(AppError::new(
        StatusCode::BAD_GATEWAY,
        "upstream_error",
        "no available replicate provider",
    ))
}

async fn forward_replicate_get(
    state: &AppState,
    channel: &crate::monoize_routing::MonoizeChannel,
    path: &str,
    query: &str,
) -> AppResult<Response> {
    let base = channel.base_url.trim_end_matches('/');
    let url = if query.is_empty() {
        format!("{base}{path}")
    } else {
        format!("{base}{path}?{query}")
    };
    let resp = state
        .http
        .get(&url)
        .timeout(std::time::Duration::from_secs(30))
        .bearer_auth(&channel.api_key)
        .send()
        .await
        .map_err(|e| {
            AppError::new(StatusCode::BAD_GATEWAY, "upstream_error", e.to_string())
        })?;
    let status = StatusCode::from_u16(resp.status().as_u16()).unwrap_or(StatusCode::BAD_GATEWAY);
    let body_text = resp.text().await.unwrap_or_default();
    let value: Value = serde_json::from_str(&body_text).unwrap_or(Value::String(body_text));
    Ok((status, Json(value)).into_response())
}

async fn forward_replicate_post_empty(
    state: &AppState,
    channel: &crate::monoize_routing::MonoizeChannel,
    path: &str,
) -> AppResult<Response> {
    let base = channel.base_url.trim_end_matches('/');
    let url = format!("{base}{path}");
    let resp = state
        .http
        .post(&url)
        .timeout(std::time::Duration::from_secs(30))
        .bearer_auth(&channel.api_key)
        .header("content-type", "application/json")
        .body("{}")
        .send()
        .await
        .map_err(|e| {
            AppError::new(StatusCode::BAD_GATEWAY, "upstream_error", e.to_string())
        })?;
    let status = StatusCode::from_u16(resp.status().as_u16()).unwrap_or(StatusCode::BAD_GATEWAY);
    let body_text = resp.text().await.unwrap_or_default();
    let value: Value = serde_json::from_str(&body_text).unwrap_or(Value::String(body_text));
    Ok((status, Json(value)).into_response())
}

pub async fn create_prediction(
    State(state): State<AppState>,
    headers: HeaderMap,
    Json(body): Json<Value>,
) -> AppResult<Response> {
    let auth = auth_tenant(&headers, &state).await?;
    ensure_balance_before_forward(&state, &auth).await?;

    let obj = body.as_object().ok_or_else(|| {
        AppError::new(
            StatusCode::BAD_REQUEST,
            "invalid_request",
            "body must be object",
        )
    })?;

    let mut logical_model = obj
        .get("model")
        .and_then(|v| v.as_str())
        .filter(|s| !s.trim().is_empty())
        .ok_or_else(|| AppError::new(StatusCode::BAD_REQUEST, "invalid_request", "missing model"))?
        .to_string();
    apply_model_redirects_to_model(&mut logical_model, &auth.model_redirects);
    ensure_model_allowed(&auth, &logical_model)?;

    let max_multiplier = parse_max_multiplier_header(&headers)
        .or_else(|| {
            obj.get("max_multiplier")
                .and_then(|v| v.as_f64())
                .filter(|n| n.is_finite() && *n > 0.0)
        });

    let request_id = extract_request_id(&headers);
    let request_ip = extract_client_ip(&headers);
    let started_at = std::time::Instant::now();

    let routing_stub = crate::handlers::UrpRequest {
        model: logical_model.clone(),
        max_multiplier,
    };
    let attempts =
        build_monoize_attempts(&state, &routing_stub, auth.effective_groups.clone()).await?;

    insert_pending_request_log(
        &state,
        &auth,
        &logical_model,
        false,
        request_id.as_deref(),
        request_ip.as_deref(),
        started_at,
    )
    .await;

    let mut last_failed_attempt: Option<MonoizeAttempt> = None;
    let mut tried_providers: Vec<TriedProvider> = Vec::new();
    let mut execution_state = AttemptExecutionState::default();

    let prefer_header = headers.get("prefer").and_then(|v| v.to_str().ok()).map(|s| s.to_string());
    let cancel_after_header = headers.get("cancel-after").and_then(|v| v.to_str().ok()).map(|s| s.to_string());

    for attempt in attempts {
        if attempt.provider_type != ProviderType::Replicate {
            continue;
        }
        execution_state.enter_provider(&attempt.provider_id);
        if !execution_state.provider_budget_remaining(&attempt) {
            continue;
        }

        let max_channel_attempts = (attempt.channel_max_retries + 1).max(1) as usize;
        for channel_attempt in 0..max_channel_attempts {
            if !execution_state.provider_budget_remaining(&attempt) {
                break;
            }

            let attempt_number = execution_state.record_upstream_attempt();
            let (upstream_path, version) = resolve_replicate_upstream_path(&attempt.upstream_model);
            let upstream_body = build_replicate_upstream_body(&body, version.as_deref());

            let base = attempt.base_url.trim_end_matches('/');
            let url = format!("{base}{upstream_path}");

            let mut req = state
                .http
                .post(&url)
                .timeout(std::time::Duration::from_millis(attempt.request_timeout_ms))
                .bearer_auth(&attempt.api_key)
                .header("content-type", "application/json");

            if let Some(ref pref) = prefer_header {
                req = req.header("prefer", pref.as_str());
            }
            if let Some(ref ca) = cancel_after_header {
                req = req.header("cancel-after", ca.as_str());
            }

            let result = req
                .json(&upstream_body)
                .send()
                .await;

            match result {
                Ok(resp) => {
                    let status_code = resp.status();
                    let body_text = resp.text().await.unwrap_or_default();
                    let value: Value = serde_json::from_str(&body_text)
                        .unwrap_or(Value::String(body_text));

                    if status_code.is_success() || status_code == reqwest::StatusCode::CREATED {
                        mark_channel_success(&state, &attempt).await;
                        update_pending_channel_info(
                            &state,
                            &auth,
                            &attempt,
                            &logical_model,
                            false,
                            request_id.as_deref(),
                            request_ip.as_deref(),
                            started_at,
                        )
                        .await;

                        spawn_replicate_request_log(
                            &state,
                            &auth,
                            &attempt,
                            &logical_model,
                            started_at,
                            request_id,
                            request_ip,
                            tried_providers,
                        );

                        let axum_status = StatusCode::from_u16(status_code.as_u16())
                            .unwrap_or(StatusCode::OK);
                        return Ok((axum_status, Json(value)).into_response());
                    }

                    let axum_status = StatusCode::from_u16(status_code.as_u16())
                        .unwrap_or(StatusCode::BAD_GATEWAY);

                    let non_retryable = matches!(
                        axum_status,
                        StatusCode::BAD_REQUEST
                            | StatusCode::UNAUTHORIZED
                            | StatusCode::FORBIDDEN
                            | StatusCode::UNPROCESSABLE_ENTITY
                    );

                    if non_retryable {
                        let app_err = AppError::new(
                            axum_status,
                            "upstream_error",
                            format!("upstream status {axum_status}"),
                        );
                        spawn_request_log_error(
                            &state,
                            &auth,
                            &attempt,
                            &logical_model,
                            false,
                            started_at,
                            request_id,
                            request_ip,
                            &app_err,
                            None,
                            tried_providers,
                        );
                        return Ok((axum_status, Json(value)).into_response());
                    }

                    let retryable = matches!(
                        axum_status,
                        StatusCode::TOO_MANY_REQUESTS
                            | StatusCode::INTERNAL_SERVER_ERROR
                            | StatusCode::BAD_GATEWAY
                            | StatusCode::SERVICE_UNAVAILABLE
                            | StatusCode::GATEWAY_TIMEOUT
                    );

                    if retryable {
                        let retryable_failure_class = if axum_status == StatusCode::TOO_MANY_REQUESTS {
                            routing::RetryableFailureClass::RateLimited
                        } else {
                            routing::RetryableFailureClass::Transient
                        };
                        tried_providers.push(TriedProvider {
                            attempt_number,
                            provider_id: attempt.provider_id.clone(),
                            channel_id: attempt.channel_id.clone(),
                            error: format!("upstream status {axum_status}"),
                        });
                        mark_channel_retryable_failure(&state, &attempt, retryable_failure_class)
                            .await;
                        last_failed_attempt = Some(attempt.clone());
                        if !is_attempt_channel_healthy(&state, &attempt).await {
                            break;
                        }
                        if execution_state.provider_budget_remaining(&attempt) {
                            if channel_attempt + 1 < max_channel_attempts {
                                maybe_sleep_before_channel_retry(&attempt).await;
                            }
                            continue;
                        }
                        break;
                    }

                    return Ok((axum_status, Json(value)).into_response());
                }
                Err(err) => {
                    tried_providers.push(TriedProvider {
                        attempt_number,
                        provider_id: attempt.provider_id.clone(),
                        channel_id: attempt.channel_id.clone(),
                        error: err.to_string(),
                    });
                    let retryable_failure_class = routing::RetryableFailureClass::Transient;
                    mark_channel_retryable_failure(&state, &attempt, retryable_failure_class).await;
                    last_failed_attempt = Some(attempt.clone());
                    if !is_attempt_channel_healthy(&state, &attempt).await {
                        break;
                    }
                    if execution_state.provider_budget_remaining(&attempt) {
                        if channel_attempt + 1 < max_channel_attempts {
                            maybe_sleep_before_channel_retry(&attempt).await;
                        }
                        continue;
                    }
                    break;
                }
            }
        }
    }

    let final_err = AppError::new(
        StatusCode::BAD_GATEWAY,
        "upstream_error",
        build_exhausted_error_message(&logical_model, &tried_providers),
    );
    if let Some(attempt) = last_failed_attempt {
        spawn_request_log_error(
            &state,
            &auth,
            &attempt,
            &logical_model,
            false,
            started_at,
            request_id,
            request_ip,
            &final_err,
            None,
            tried_providers,
        );
    } else {
        spawn_request_log_error_no_attempt(
            &state,
            &auth,
            &logical_model,
            false,
            started_at,
            request_id,
            request_ip,
            &final_err,
            None,
            tried_providers,
        );
    }
    Err(final_err)
}

pub async fn get_prediction(
    State(state): State<AppState>,
    headers: HeaderMap,
    axum::extract::Path(prediction_id): axum::extract::Path<String>,
) -> AppResult<Response> {
    let _auth = auth_tenant(&headers, &state).await?;
    let (_provider, channel) = pick_any_replicate_channel(&state).await?;
    let path = format!("/v1/predictions/{prediction_id}");
    forward_replicate_get(&state, &channel, &path, "").await
}

pub async fn list_predictions(
    State(state): State<AppState>,
    headers: HeaderMap,
    axum::extract::Query(query): axum::extract::Query<std::collections::HashMap<String, String>>,
) -> AppResult<Response> {
    let _auth = auth_tenant(&headers, &state).await?;
    let (_provider, channel) = pick_any_replicate_channel(&state).await?;
    let base = channel.base_url.trim_end_matches('/');
    let url = format!("{base}/v1/predictions");
    let resp = state
        .http
        .get(&url)
        .timeout(std::time::Duration::from_secs(30))
        .bearer_auth(&channel.api_key)
        .query(&query)
        .send()
        .await
        .map_err(|e| {
            AppError::new(StatusCode::BAD_GATEWAY, "upstream_error", e.to_string())
        })?;
    let status = StatusCode::from_u16(resp.status().as_u16()).unwrap_or(StatusCode::BAD_GATEWAY);
    let body_text = resp.text().await.unwrap_or_default();
    let value: Value = serde_json::from_str(&body_text).unwrap_or(Value::String(body_text));
    Ok((status, Json(value)).into_response())
}

pub async fn cancel_prediction(
    State(state): State<AppState>,
    headers: HeaderMap,
    axum::extract::Path(prediction_id): axum::extract::Path<String>,
) -> AppResult<Response> {
    let _auth = auth_tenant(&headers, &state).await?;
    let (_provider, channel) = pick_any_replicate_channel(&state).await?;
    let path = format!("/v1/predictions/{prediction_id}/cancel");
    forward_replicate_post_empty(&state, &channel, &path).await
}

pub async fn create_model_prediction(
    State(state): State<AppState>,
    headers: HeaderMap,
    axum::extract::Path((model_owner, model_name)): axum::extract::Path<(String, String)>,
    Json(body): Json<Value>,
) -> AppResult<Response> {
    let mut merged_body = body.clone();
    if let Some(obj) = merged_body.as_object_mut() {
        obj.insert(
            "model".to_string(),
            Value::String(format!("{model_owner}/{model_name}")),
        );
    }
    let state_clone = state.clone();
    create_prediction(State(state_clone), headers, Json(merged_body)).await
}

pub async fn create_deployment_prediction(
    State(state): State<AppState>,
    headers: HeaderMap,
    axum::extract::Path((deployment_owner, deployment_name)): axum::extract::Path<(String, String)>,
    Json(body): Json<Value>,
) -> AppResult<Response> {
    let mut merged_body = body.clone();
    if let Some(obj) = merged_body.as_object_mut() {
        obj.insert(
            "model".to_string(),
            Value::String(format!("deployment:{deployment_owner}/{deployment_name}")),
        );
    }
    let state_clone = state.clone();
    create_prediction(State(state_clone), headers, Json(merged_body)).await
}

fn spawn_replicate_request_log(
    state: &AppState,
    auth: &crate::auth::AuthResult,
    attempt: &MonoizeAttempt,
    model: &str,
    started_at: std::time::Instant,
    request_id: Option<String>,
    request_ip: Option<String>,
    tried_providers: Vec<TriedProvider>,
) {
    let Some(user_id) = auth.user_id.clone() else {
        return;
    };
    let api_key_id = auth.api_key_id.clone();
    let provider_id = attempt.provider_id.clone();
    let upstream_model = attempt.upstream_model.clone();
    let model_multiplier = attempt.model_multiplier;
    let channel_id = attempt.channel_id.clone();
    let model = model.to_string();
    let duration_ms = started_at.elapsed().as_millis() as u64;
    let created_at = chrono::Utc::now()
        - chrono::Duration::from_std(started_at.elapsed()).unwrap_or(chrono::Duration::MAX);
    let user_store = state.user_store.clone();
    let tried_providers_json = if tried_providers.is_empty() {
        None
    } else {
        serde_json::to_value(&tried_providers).ok()
    };

    tokio::spawn(async move {
        let log = InsertRequestLog {
            request_id,
            user_id,
            api_key_id,
            model,
            provider_id: Some(provider_id),
            upstream_model: Some(upstream_model),
            channel_id: Some(channel_id),
            is_stream: false,
            input_tokens: None,
            output_tokens: None,
            cache_read_tokens: None,
            cache_creation_tokens: None,
            tool_prompt_tokens: None,
            reasoning_tokens: None,
            accepted_prediction_tokens: None,
            rejected_prediction_tokens: None,
            provider_multiplier: Some(model_multiplier),
            charge_nano_usd: None,
            status: REQUEST_LOG_STATUS_SUCCESS.to_string(),
            usage_breakdown_json: None,
            billing_breakdown_json: None,
            error_code: None,
            error_message: None,
            error_http_status: None,
            duration_ms: Some(duration_ms),
            ttfb_ms: None,
            request_ip,
            reasoning_effort: None,
            tried_providers_json,
            request_kind: Some("replicate_prediction".to_string()),
            created_at,
        };
        if let Err(e) = user_store.finalize_request_log(log).await {
            tracing::warn!("failed to finalize replicate request log: {e}");
        }
    });
}
