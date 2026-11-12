use super::*;
use axum::extract::Request;

pub async fn create_image_generation(
    State(state): State<AppState>,
    headers: HeaderMap,
    Json(body): Json<Value>,
) -> AppResult<Response> {
    let mut auth = auth_tenant(&headers, &state).await?;
    auth.internal_source
        .get_or_insert(crate::auth::InternalRequestSource::ImageGeneration);
    let capture_eligible = state
        .request_capture
        .would_start_session(&state.monoize_runtime, &auth)
        .await;
    let capture_raw_input = Arc::new(if capture_eligible {
        body.clone()
    } else {
        Value::Null
    });
    let obj = body
        .as_object()
        .ok_or_else(|| image_request_error("body must be object"))?;
    let prompt = required_image_text(obj, "prompt")?;
    let mut model = required_image_text(obj, "model")?;
    apply_configured_model_redirects_to_model(&state, &mut model, &auth).await;
    validate_image_count(obj.get("n"))?;
    let stream_requested = parse_image_stream(obj.get("stream"))?;
    ensure_model_allowed(&auth, &model)?;
    let max_multiplier = resolve_image_max_multiplier(obj.get("max_multiplier"), &headers, &auth);
    let options = ImageRequestOptions::parse(build_extra_body(
        obj,
        &["prompt", "model", "n", "max_multiplier", "stream"],
    ))?;
    tracing::info!(model = %model, n = 1, stream = stream_requested, endpoint = "generations", "image API request");
    options.validate(false, stream_requested)?;
    let image = ImageApiRequest {
        model,
        input: vec![image_prompt_node(prompt)],
        options,
    };
    let meta = ImageRequestMeta::from_headers(
        &headers,
        max_multiplier,
        capture_raw_input,
        crate::request_capture::CaptureDownstreamProtocol::ImageGenerations,
    );
    dispatch_image_request(
        state,
        auth,
        image,
        meta,
        stream_requested,
        ImageStreamEventFamily::Generation,
    )
    .await
}

pub async fn create_image_edit(
    State(state): State<AppState>,
    headers: HeaderMap,
    request: Request,
) -> AppResult<Response> {
    let mut auth = auth_tenant(&headers, &state).await?;
    auth.internal_source
        .get_or_insert(crate::auth::InternalRequestSource::ImageEdit);
    let capture_eligible = state
        .request_capture
        .would_start_session(&state.monoize_runtime, &auth)
        .await;
    let edit = parse_edit_request(&state, &headers, request, capture_eligible).await?;
    let mut model = match edit.fields.get("model") {
        None | Some(Value::Null) => "gpt-image-2.5-sunburst".to_owned(),
        _ => required_image_text(&edit.fields, "model")?,
    };
    let prompt = required_image_text(&edit.fields, "prompt")?;
    validate_image_count(edit.fields.get("n"))?;
    let stream_requested = parse_image_stream(edit.fields.get("stream"))?;
    apply_configured_model_redirects_to_model(&state, &mut model, &auth).await;
    ensure_model_allowed(&auth, &model)?;
    let max_multiplier =
        resolve_image_max_multiplier(edit.fields.get("max_multiplier"), &headers, &auth);
    let options = parse_edit_options(&edit, stream_requested)?;
    let image_count = edit.images.len();
    let has_mask = edit.mask.is_some();
    let mut input = vec![image_prompt_node(prompt)];
    input.extend(edit.images);
    input.extend(edit.mask);
    input.extend(edit.markup);
    let (inline_bytes_estimate, reference_count) = image_input_size(&input);
    tracing::info!(model = %model, n = 1, stream = stream_requested, endpoint = "edits", image_count,
        inline_bytes_estimate, reference_count, has_mask, "image API request");
    let meta = ImageRequestMeta::from_headers(
        &headers,
        max_multiplier,
        edit.capture_raw_input,
        crate::request_capture::CaptureDownstreamProtocol::ImageEdits,
    );
    let image = ImageApiRequest {
        model,
        input,
        options,
    };
    dispatch_image_request(
        state,
        auth,
        image,
        meta,
        stream_requested,
        ImageStreamEventFamily::Edit,
    )
    .await
}

fn image_prompt_node(prompt: String) -> urp::Node {
    urp::Node::Text {
        logprobs: None,
        citations: Vec::new(),
        signature: None,
        id: None,
        role: urp::OrdinaryRole::User,
        content: prompt,
        phase: None,
        extra_body: HashMap::new(),
        cache_breakpoint: None,
    }
}

fn image_input_size(inputs: &[urp::Node]) -> (usize, usize) {
    let mut inline_bytes_estimate = 0usize;
    let mut reference_count = 0usize;
    for node in inputs {
        if let urp::Node::Image { source, .. } = node {
            match source {
                urp::ImageSource::Base64 { data, .. } => {
                    inline_bytes_estimate += data.len() / 4 * 3
                }
                _ => reference_count += 1,
            }
        }
    }
    (inline_bytes_estimate, reference_count)
}

async fn dispatch_image_request(
    state: AppState,
    auth: crate::auth::AuthResult,
    image: ImageApiRequest,
    meta: ImageRequestMeta,
    stream: bool,
    family: ImageStreamEventFamily,
) -> AppResult<Response> {
    if stream {
        return run_image_stream_downstream(state, auth, image, meta, family).await;
    }
    let (response, _) = run_image_request(&state, &auth, image, meta).await?;
    assemble_image_response(response)
}
