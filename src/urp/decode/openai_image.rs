use crate::urp::{
    FinishReason, ImageSource, InputDetails, ModalityBreakdown, Node, OrdinaryRole, OutputDetails,
    UrpResponse, Usage,
};
use serde_json::Value;
use std::collections::HashMap;

pub fn decode_response(value: &Value, model: &str) -> Result<UrpResponse, String> {
    let obj = value.as_object().ok_or("response is not an object")?;

    let id = obj
        .get("created")
        .and_then(|v| v.as_i64())
        .map(|v| v.to_string())
        .unwrap_or_else(|| format!("img-{}", uuid::Uuid::new_v4()));

    let data = obj
        .get("data")
        .and_then(|v| v.as_array())
        .ok_or("missing data array in image response")?;

    let mut output: Vec<Node> = Vec::new();
    let mut revised_prompt: Option<String> = None;

    for item in data {
        let item_obj = item.as_object().ok_or("data item is not an object")?;

        if let Some(rp) = item_obj.get("revised_prompt").and_then(|v| v.as_str()) {
            if revised_prompt.is_none() && !rp.trim().is_empty() {
                revised_prompt = Some(rp.to_string());
            }
        }

        if let Some(b64) = item_obj.get("b64_json").and_then(|v| v.as_str()) {
            output.push(Node::Image {
                id: None,
                role: OrdinaryRole::Assistant,
                source: ImageSource::Base64 {
                    media_type: "image/png".to_string(),
                    data: b64.to_string(),
                },
                extra_body: HashMap::new(),
            });
        } else if let Some(url) = item_obj.get("url").and_then(|v| v.as_str()) {
            output.push(Node::Image {
                id: None,
                role: OrdinaryRole::Assistant,
                source: ImageSource::Url {
                    url: url.to_string(),
                    detail: None,
                },
                extra_body: HashMap::new(),
            });
        }
    }

    if output.is_empty() {
        return Err("no images found in upstream response".to_string());
    }

    if let Some(rp) = revised_prompt {
        output.insert(
            0,
            Node::Text {
                id: None,
                role: OrdinaryRole::Assistant,
                content: rp,
                phase: None,
                extra_body: HashMap::new(),
            },
        );
    }

    let usage = obj.get("usage").and_then(parse_image_usage);

    Ok(UrpResponse {
        id,
        model: model.to_string(),
        created_at: obj.get("created").and_then(|v| v.as_i64()),
        output,
        finish_reason: Some(FinishReason::Stop),
        usage,
        extra_body: HashMap::new(),
    })
}

/// Parse an OpenAI Image API `usage` object (non-streaming response body or
/// streaming `*.completed` event payload, OIU-D6/OIU-S3a) into URP `Usage`.
pub(crate) fn parse_image_usage(usage_value: &Value) -> Option<Usage> {
    let usage_obj = usage_value.as_object()?;
    let input_tokens = usage_obj
        .get("input_tokens")
        .or_else(|| usage_obj.get("prompt_tokens"))
        .and_then(|v| v.as_u64())
        .unwrap_or(0);
    let output_tokens = usage_obj
        .get("output_tokens")
        .or_else(|| usage_obj.get("completion_tokens"))
        .and_then(|v| v.as_u64())
        .unwrap_or(0);

    let input_details = {
        let mut details = InputDetails::default();
        if let Some(id) = usage_obj
            .get("input_tokens_details")
            .and_then(|v| v.as_object())
        {
            details.cache_read_tokens = id
                .get("cached_tokens")
                .or_else(|| id.get("cache_read_tokens"))
                .and_then(|v| v.as_u64())
                .unwrap_or(0);
            details.cache_read_modality_breakdown = parse_cache_read_modality_breakdown(id);
            if let Some(mb) = parse_modality_breakdown(id) {
                details.modality_breakdown = Some(mb);
            }
        }
        Some(details)
    };

    let output_details = {
        let mut details = OutputDetails::default();
        if let Some(od) = usage_obj
            .get("output_tokens_details")
            .and_then(|v| v.as_object())
        {
            if let Some(mb) = parse_modality_breakdown(od) {
                details.modality_breakdown = Some(mb);
            }
        }
        Some(details)
    };

    Some(Usage {
        input_tokens,
        output_tokens,
        input_details,
        output_details,
        extra_body: HashMap::new(),
    })
}

fn parse_cache_read_modality_breakdown(
    obj: &serde_json::Map<String, Value>,
) -> Option<ModalityBreakdown> {
    for key in [
        "cache_read_tokens_details",
        "cached_tokens_details",
        "cached_input_tokens_details",
    ] {
        if let Some(nested) = obj.get(key).and_then(|v| v.as_object())
            && let Some(breakdown) = parse_modality_breakdown(nested)
        {
            return Some(breakdown);
        }
    }
    let text = obj
        .get("cache_read_text_tokens")
        .or_else(|| obj.get("cached_text_tokens"))
        .or_else(|| obj.get("cached_input_text_tokens"))
        .and_then(|v| v.as_u64());
    let image = obj
        .get("cache_read_image_tokens")
        .or_else(|| obj.get("cached_image_tokens"))
        .or_else(|| obj.get("cached_input_image_tokens"))
        .and_then(|v| v.as_u64());
    if text.is_some() || image.is_some() {
        Some(ModalityBreakdown {
            text_tokens: text,
            image_tokens: image,
            audio_tokens: None,
            video_tokens: None,
            document_tokens: None,
        })
    } else {
        None
    }
}

fn parse_modality_breakdown(obj: &serde_json::Map<String, Value>) -> Option<ModalityBreakdown> {
    let text = obj.get("text_tokens").and_then(|v| v.as_u64());
    let image = obj.get("image_tokens").and_then(|v| v.as_u64());
    if text.is_some() || image.is_some() {
        Some(ModalityBreakdown {
            text_tokens: text,
            image_tokens: image,
            audio_tokens: None,
            video_tokens: None,
            document_tokens: None,
        })
    } else {
        None
    }
}
