use super::*;
use axum::extract::Multipart;
use base64::Engine as _;
use std::collections::HashMap;

pub async fn create_image_generation(
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

    apply_model_redirects_to_model(&mut model, &auth.model_redirects);

    let n = parse_n_field(obj.get("n"))?;

    ensure_model_allowed(&auth, &model)?;

    let max_multiplier_val =
        resolve_image_max_multiplier(obj.get("max_multiplier"), &headers, &auth);
    let request_id = extract_request_id(&headers);
    let request_ip = extract_client_ip(&headers);

    let extra_body = build_extra_body(obj, &["prompt", "model", "n", "max_multiplier"]);

    let inputs = vec![urp::Node::Text {
        id: None,
        role: urp::OrdinaryRole::User,
        content: prompt,
        phase: None,
        extra_body: HashMap::new(),
    }];

    tracing::info!(
        model = %model,
        n = n,
        endpoint = "generations",
        "image api request"
    );

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
    ensure_balance_before_forward(&state, &auth).await?;

    let mut prompt: Option<String> = None;
    let mut model: Option<String> = None;
    let mut n_raw: Option<String> = None;
    let mut image_data: Option<(String, String)> = None;
    let mut extra_images: Vec<(String, String)> = Vec::new();
    let mut mask_data: Option<(String, String)> = None;
    let mut max_multiplier_raw: Option<String> = None;
    let mut extra_text_fields: HashMap<String, Value> = HashMap::new();

    while let Some(field) = multipart
        .next_field()
        .await
        .map_err(|e| AppError::new(StatusCode::BAD_REQUEST, "invalid_request", e.to_string()))?
    {
        let field_name = field.name().unwrap_or("").to_string();
        match field_name.as_str() {
            "prompt" => {
                prompt = Some(field.text().await.map_err(|e| {
                    AppError::new(StatusCode::BAD_REQUEST, "invalid_request", e.to_string())
                })?);
            }
            "model" => {
                model = Some(field.text().await.map_err(|e| {
                    AppError::new(StatusCode::BAD_REQUEST, "invalid_request", e.to_string())
                })?);
            }
            "n" => {
                n_raw = Some(field.text().await.map_err(|e| {
                    AppError::new(StatusCode::BAD_REQUEST, "invalid_request", e.to_string())
                })?);
            }
            "max_multiplier" => {
                max_multiplier_raw = Some(field.text().await.map_err(|e| {
                    AppError::new(StatusCode::BAD_REQUEST, "invalid_request", e.to_string())
                })?);
            }
            "image" | "image[]" => {
                let media_type = field
                    .content_type()
                    .map(|s| s.to_string())
                    .unwrap_or_else(|| infer_media_type_from_filename(field.file_name()));
                let bytes = field.bytes().await.map_err(|e| {
                    AppError::new(StatusCode::BAD_REQUEST, "invalid_request", e.to_string())
                })?;
                let b64 = base64::engine::general_purpose::STANDARD.encode(&bytes);
                if image_data.is_none() {
                    image_data = Some((media_type, b64));
                } else {
                    extra_images.push((media_type, b64));
                }
            }
            "mask" => {
                let media_type = field
                    .content_type()
                    .map(|s| s.to_string())
                    .unwrap_or_else(|| infer_media_type_from_filename(field.file_name()));
                let bytes = field.bytes().await.map_err(|e| {
                    AppError::new(StatusCode::BAD_REQUEST, "invalid_request", e.to_string())
                })?;
                let b64 = base64::engine::general_purpose::STANDARD.encode(&bytes);
                mask_data = Some((media_type, b64));
            }
            _ => {
                if let Ok(text) = field.text().await {
                    extra_text_fields.insert(field_name, coerce_text_to_json_value(&text));
                }
            }
        }
    }

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

    apply_model_redirects_to_model(&mut model, &auth.model_redirects);

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

    ensure_model_allowed(&auth, &model)?;

    let max_multiplier_val = {
        let ceiling = auth.max_multiplier;
        let requested = max_multiplier_raw
            .and_then(|s| s.parse::<f64>().ok())
            .filter(|v| v.is_finite() && *v > 0.0)
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

    let has_mask = mask_data.is_some();

    let mut inputs = Vec::new();
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
            id: None,
            role: urp::OrdinaryRole::User,
            source: urp::ImageSource::Base64 {
                media_type: mask_media_type,
                data: mask_b64,
            },
            extra_body: HashMap::new(),
        });
    }
    inputs.push(urp::Node::Text {
        id: None,
        role: urp::OrdinaryRole::User,
        content: prompt,
        phase: None,
        extra_body: HashMap::new(),
    });

    tracing::info!(
        model = %model,
        n = n,
        endpoint = "edits",
        has_mask = has_mask,
        "image api request"
    );

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
    )
    .await;

    assemble_image_response(results)
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
) -> Option<f64> {
    let ceiling = auth.max_multiplier;
    let requested = body_value
        .and_then(|v| {
            v.as_f64().or_else(|| {
                v.as_str()
                    .and_then(|s| s.parse::<f64>().ok())
                    .filter(|n| n.is_finite())
            })
        })
        .filter(|n| *n > 0.0)
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
    max_multiplier: Option<f64>,
    n: usize,
    request_id: Option<String>,
    request_ip: Option<String>,
) -> Vec<Result<(urp::UrpResponse, String), AppError>> {
    let mut join_set = tokio::task::JoinSet::new();

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
            response_format: None,
            user: None,
            extra_body: extra_body.clone(),
        };
        let rid = request_id
            .clone()
            .map(|id| if n > 1 { format!("{id}:img:{i}") } else { id });
        let rip = request_ip.clone();

        join_set.spawn(async move {
            execute_nonstream_typed(
                &state,
                &auth,
                req,
                max_multiplier,
                super::DownstreamProtocol::Responses,
                rid,
                rip,
            )
            .await
        });
    }

    let mut results = Vec::with_capacity(n);
    while let Some(join_result) = join_set.join_next().await {
        match join_result {
            Ok(inner) => results.push(inner),
            Err(e) => results.push(Err(AppError::new(
                StatusCode::INTERNAL_SERVER_ERROR,
                "internal_error",
                format!("sub-request task panicked: {e}"),
            ))),
        }
    }
    results
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

    for item in &resp.output {
        match item {
            urp::Node::Image { source, .. } => match source {
                urp::ImageSource::Base64 { data, .. } => {
                    images.push(ExtractedImage {
                        b64_json: Some(data.clone()),
                        url: None,
                        revised_prompt: None,
                    });
                }
                urp::ImageSource::Url { url, .. } => {
                    images.push(ExtractedImage {
                        b64_json: None,
                        url: Some(url.clone()),
                        revised_prompt: None,
                    });
                }
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

fn assemble_image_response(
    results: Vec<Result<(urp::UrpResponse, String), AppError>>,
) -> AppResult<Response> {
    let mut data_items: Vec<Value> = Vec::new();
    let mut last_error: Option<AppError> = None;
    let mut total_usage: Option<AggregatedUsage> = None;

    for result in results {
        match result {
            Ok((resp, _logical_model)) => {
                let images = extract_images_from_response(&resp);
                if images.is_empty() {
                    let upstream_text = collect_response_text(&resp);
                    let detail = if upstream_text.is_empty() {
                        "upstream response contained no images".to_string()
                    } else {
                        format!(
                            "upstream response contained no images. upstream output: {upstream_text}"
                        )
                    };
                    last_error = Some(AppError::new(
                        StatusCode::BAD_GATEWAY,
                        "upstream_error",
                        detail,
                    ));
                    continue;
                }
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
                    let agg = total_usage.get_or_insert(AggregatedUsage::default());
                    agg.input_tokens += usage.input_tokens;
                    agg.output_tokens += usage.output_tokens;
                    if let Some(details) = &usage.input_details {
                        if let Some(modality) = &details.modality_breakdown {
                            agg.input_text_tokens += modality.text_tokens.unwrap_or(0);
                            agg.input_image_tokens += modality.image_tokens.unwrap_or(0);
                        }
                    }
                    if let Some(details) = &usage.output_details {
                        if let Some(modality) = &details.modality_breakdown {
                            agg.output_text_tokens += modality.text_tokens.unwrap_or(0);
                            agg.output_image_tokens += modality.image_tokens.unwrap_or(0);
                        }
                    }
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
        response.as_object_mut().unwrap().insert(
            "usage".to_string(),
            json!({
                "input_tokens": usage.input_tokens,
                "output_tokens": usage.output_tokens,
                "total_tokens": usage.input_tokens + usage.output_tokens,
                "input_tokens_details": {
                    "text_tokens": usage.input_text_tokens,
                    "image_tokens": usage.input_image_tokens,
                },
                "output_tokens_details": {
                    "text_tokens": usage.output_text_tokens,
                    "image_tokens": usage.output_image_tokens,
                }
            }),
        );
    }

    Ok(Json(response).into_response())
}

#[derive(Default)]
struct AggregatedUsage {
    input_tokens: u64,
    output_tokens: u64,
    input_text_tokens: u64,
    input_image_tokens: u64,
    output_text_tokens: u64,
    output_image_tokens: u64,
}
