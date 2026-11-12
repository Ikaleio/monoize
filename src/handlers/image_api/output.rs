use super::*;

fn collect_response_text(resp: &urp::UrpResponse) -> String {
    let mut parts = Vec::new();
    for item in &resp.output {
        match item {
            urp::Node::Text {
                logprobs, content, ..
            }
            | urp::Node::Refusal {
                logprobs, content, ..
            } if !content.trim().is_empty() => {
                parts.push(content.as_str());
            }
            _ => {}
        }
    }
    parts.join("\n")
}

/// One extracted image from a URP response.
pub(super) struct ExtractedImage {
    pub(super) b64_json: Option<String>,
    pub(super) url: Option<String>,
    pub(super) mime_type: Option<String>,
    pub(super) revised_prompt: Option<String>,
    pub(super) generation: urp::ImageGenerationMetadata,
}

fn image_item_mime(source: &urp::ImageSource, metadata: &urp::MediaMetadata) -> Option<String> {
    match source {
        urp::ImageSource::Base64 { media_type, .. } if !media_type.trim().is_empty() => {
            Some(media_type.clone())
        }
        urp::ImageSource::Url { .. } => metadata
            .media_type
            .as_ref()
            .filter(|value| !value.trim().is_empty())
            .cloned(),
        _ => None,
    }
}

pub(super) fn extract_images_from_response(resp: &urp::UrpResponse) -> Vec<ExtractedImage> {
    let mut images = Vec::new();
    let mut text_parts = Vec::new();

    for item in &resp.output {
        match item {
            urp::Node::Image {
                role: urp::OrdinaryRole::Assistant,
                source,
                metadata,
                ..
            } => match source {
                urp::ImageSource::Base64 { data, .. } => {
                    if data.trim().is_empty() {
                        continue;
                    }
                    images.push(ExtractedImage {
                        b64_json: Some(data.clone()),
                        url: None,
                        mime_type: image_item_mime(source, metadata),
                        revised_prompt: metadata.image_generation.revised_prompt.clone(),
                        generation: metadata.image_generation.for_source(source),
                    });
                }
                urp::ImageSource::Url { url, .. } => {
                    if url.trim().is_empty() {
                        continue;
                    }
                    images.push(ExtractedImage {
                        b64_json: None,
                        url: Some(url.clone()),
                        mime_type: image_item_mime(source, metadata),
                        revised_prompt: metadata.image_generation.revised_prompt.clone(),
                        generation: metadata.image_generation.for_source(source),
                    });
                }
                urp::ImageSource::FileId { .. } => continue,
            },
            urp::Node::Text {
                logprobs,
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
            if img.revised_prompt.is_none() {
                img.revised_prompt = Some(revised.clone());
            }
        }
    }

    images
}

pub(super) fn image_upstream_error(
    code: Option<&str>,
    message: &str,
    kind: Option<&str>,
    param: Option<&str>,
    status: Option<u64>,
) -> AppError {
    let code = code.unwrap_or("upstream_error");
    let status = status
        .filter(|v| (400..=599).contains(v))
        .and_then(|v| StatusCode::from_u16(v as u16).ok())
        .unwrap_or(match (code, kind) {
            (
                "invalid_value"
                | "invalid_request_error"
                | "unsupported_value"
                | "invalid_parameter",
                _,
            )
            | (_, Some("invalid_request_error")) => StatusCode::BAD_REQUEST,
            ("rate_limit_exceeded" | "rate_limit_error", _) => StatusCode::TOO_MANY_REQUESTS,
            _ => StatusCode::BAD_GATEWAY,
        });
    let mut err = AppError::new(status, code, message)
        .with_type(kind.unwrap_or_else(|| openai_error_type_for_status(status)));
    err.param = param.map(str::to_owned);
    err
}

pub(super) fn validate_image_subrequest_response(resp: &urp::UrpResponse) -> AppResult<()> {
    if let Some(outcome) = &resp.outcome
        && let Some(body) = outcome.failure_body(false)
    {
        let error = &body["error"];
        return Err(image_upstream_error(
            error["code"].as_str(),
            error["message"]
                .as_str()
                .unwrap_or("Upstream image request failed"),
            error["type"].as_str(),
            error["param"].as_str(),
            error["status"].as_u64(),
        ));
    }
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

pub(super) fn assemble_image_response(resp: urp::UrpResponse) -> AppResult<Response> {
    validate_image_subrequest_response(&resp)?;
    let mut data_items = Vec::new();
    let mut common_generation: Option<Map<String, Value>> = None;
    let images = extract_images_from_response(&resp);
    for img in images {
        let generation = img.generation.common_response_fields();
        if let Some(common) = &mut common_generation {
            common.retain(|key, value| generation.get(key) == Some(value));
        } else {
            common_generation = Some(generation);
        }
        let mut item = Map::new();
        if let Some(b64) = img.b64_json {
            item.insert("b64_json".to_string(), Value::String(b64));
        }
        if let Some(url) = img.url {
            item.insert("url".to_string(), Value::String(url));
        }
        if let Some(mime_type) = img.mime_type {
            item.insert("mime_type".to_string(), Value::String(mime_type));
        }
        if let Some(revised) = img.revised_prompt {
            item.insert("revised_prompt".to_string(), Value::String(revised));
        }
        data_items.push(Value::Object(item));
    }
    let created = resp
        .created_at
        .unwrap_or_else(|| chrono::Utc::now().timestamp());
    let mut response = json!({
        "created": created,
        "data": data_items,
    });
    if let Some(generation) = common_generation {
        response.as_object_mut().unwrap().extend(generation);
    }

    if let Some(usage) = &resp.usage {
        let mut total = AggregatedUsage::default();
        accumulate_usage(&mut total, usage);
        response
            .as_object_mut()
            .unwrap()
            .insert("usage".to_string(), aggregated_usage_value(&total));
    }

    Ok(Json(response).into_response())
}

#[derive(Default)]
pub(super) struct AggregatedUsage {
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

pub(super) fn accumulate_usage(agg: &mut AggregatedUsage, usage: &urp::Usage) {
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
pub(super) fn aggregated_usage_value(usage: &AggregatedUsage) -> Value {
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
