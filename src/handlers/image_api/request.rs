use super::*;
use futures_util::StreamExt as _;

/// IS1/IM3a: billing and logging finish in the executor before completed frames.
pub(super) async fn run_image_stream_downstream(
    state: AppState,
    auth: crate::auth::AuthResult,
    image: ImageApiRequest,
    meta: ImageRequestMeta,
    family: ImageStreamEventFamily,
) -> AppResult<Response> {
    let req = image.into_urp(true);
    let mut preflight_req = req.clone();
    resolve_model_suffix(&state, &mut preflight_req).await?;
    let routing_stub = build_routing_stub(&preflight_req, meta.max_multiplier);
    let mut attempts = build_monoize_attempts(&state, &routing_stub, &auth).await?;
    bind_media_request_routes(&mut preflight_req, &mut attempts)?;
    ensure_balance_before_forward_for_attempts(&state, &auth, &attempts).await?;
    let (meta, capture) = meta.capture(&state, &auth, true).await;
    let (tx, rx) = mpsc::channel::<Event>(64);
    let _ = tx.send(Event::default().comment("heartbeat")).await;
    tokio::spawn(async move {
        let task_state = AdmittedRequestTaskState::new(Instant::now());
        task_state.set_stream(true);
        let mut sink = ImageStreamSink::new(family, tx);
        let context = ImageExecutionContext {
            state: &state,
            auth: &auth,
            meta,
            capture,
            task_state: &task_state,
        };
        match execute_stream_collected_image_typed(context, req, Some(&mut sink)).await {
            Ok((response, _)) => sink.send_completed(&response).await,
            Err(error) => sink.send_error(&error).await,
        }
        sink.send_done().await;
    });
    Ok(image_sse_response(rx))
}

fn image_sse_response(rx: mpsc::Receiver<Event>) -> Response {
    let stream =
        tokio_stream::wrappers::ReceiverStream::new(rx).map(Ok::<_, std::convert::Infallible>);
    let mut response = Sse::new(stream)
        .keep_alive(api_stream_keep_alive())
        .into_response();
    response.headers_mut().insert(
        axum::http::header::CACHE_CONTROL,
        axum::http::HeaderValue::from_static("no-cache, no-transform"),
    );
    response.headers_mut().insert(
        "x-accel-buffering",
        axum::http::HeaderValue::from_static("no"),
    );
    response
}

pub(super) async fn run_image_request(
    state: &AppState,
    auth: &crate::auth::AuthResult,
    image: ImageApiRequest,
    meta: ImageRequestMeta,
) -> AppResult<(urp::UrpResponse, String)> {
    let model = image.model.clone();
    let req = image.into_urp(false);
    let task_state = Arc::new(AdmittedRequestTaskState::new(Instant::now()));
    let task_state_for_task = task_state.clone();
    let (meta, capture) = meta.capture(state, auth, false).await;
    let worker_state = state.clone();
    let worker_auth = auth.clone();
    let request_id = meta.request_id.clone();
    let request_ip = meta.request_ip.clone();
    let mut worker = tokio::task::JoinSet::new();
    worker.spawn(async move {
        execute_image_subrequest_typed(
            ImageExecutionContext {
                state: &worker_state,
                auth: &worker_auth,
                meta,
                capture,
                task_state: &task_state_for_task,
            },
            req,
        )
        .await
    });
    match worker
        .join_next()
        .await
        .expect("one image worker was spawned")
    {
        Ok(result) => result,
        Err(join_error) => {
            let error = image_task_join_error("image sub-request", join_error);
            if let Some((started_at, is_stream, attempt)) = task_state.terminal_snapshot() {
                let log = RequestLogContext {
                    model: &model,
                    is_stream,
                    started_at,
                    request_id: request_id.as_deref(),
                    request_ip: request_ip.as_deref(),
                    reasoning_effort: None,
                };
                if let Some(attempt) = attempt {
                    spawn_request_log_error(state, auth, &attempt, log, &error, Vec::new());
                } else {
                    spawn_request_log_error_no_attempt(state, auth, log, &error, Vec::new());
                }
            }
            Err(error)
        }
    }
}

async fn execute_image_subrequest_typed(
    context: ImageExecutionContext<'_>,
    req: urp::UrpRequest,
) -> AppResult<(urp::UrpResponse, String)> {
    let routing_stub = build_routing_stub(&req, context.meta.max_multiplier);
    let mut attempts = build_monoize_attempts(context.state, &routing_stub, context.auth).await?;
    attach_client_session_id(
        &mut attempts,
        context.meta.client_session_id.clone(),
        Some(&req),
    );
    let all_responses = !attempts.is_empty()
        && attempts
            .iter()
            .all(|attempt| attempt.provider_type == ProviderType::Responses);
    context.task_state.set_stream(all_responses);
    if all_responses {
        return execute_stream_collected_image_typed(context, req, None).await;
    }
    execute_nonstream_typed_with_validator(
        NonstreamRequest {
            state: context.state,
            auth: context.auth,
            max_multiplier: context.meta.max_multiplier,
            downstream: DownstreamProtocol::Responses,
            request_id: context.meta.request_id,
            request_ip: context.meta.request_ip,
            capture: context.capture,
            task_state: Some(context.task_state),
        },
        req,
        context.meta.client_session_id,
        Some(validate_image_subrequest_response),
    )
    .await
}

pub(super) fn image_task_join_error(task: &str, error: tokio::task::JoinError) -> AppError {
    AppError::new(
        StatusCode::INTERNAL_SERVER_ERROR,
        if error.is_cancelled() {
            "task_cancelled"
        } else {
            "task_panic"
        },
        format!("{task} task failed: {error}"),
    )
    .with_type("server_error")
}
