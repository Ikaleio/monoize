use crate::urp::{
    items_to_nodes, FinishReason, ImageSource, InputDetails, Item, ModalityBreakdown,
    OutputDetails, Part, Role, UrpResponse, Usage,
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

    let mut parts: Vec<Part> = Vec::new();
    let mut revised_prompt: Option<String> = None;

    for item in data {
        let item_obj = item.as_object().ok_or("data item is not an object")?;

        if let Some(rp) = item_obj.get("revised_prompt").and_then(|v| v.as_str()) {
            if revised_prompt.is_none() && !rp.trim().is_empty() {
                revised_prompt = Some(rp.to_string());
            }
        }

        if let Some(b64) = item_obj.get("b64_json").and_then(|v| v.as_str()) {
            parts.push(Part::Image {
                source: ImageSource::Base64 {
                    media_type: "image/png".to_string(),
                    data: b64.to_string(),
                },
                extra_body: HashMap::new(),
            });
        } else if let Some(url) = item_obj.get("url").and_then(|v| v.as_str()) {
            parts.push(Part::Image {
                source: ImageSource::Url {
                    url: url.to_string(),
                    detail: None,
                },
                extra_body: HashMap::new(),
            });
        }
    }

    if parts.is_empty() {
        return Err("no images found in upstream response".to_string());
    }

    let mut all_parts: Vec<Part> = Vec::new();
    if let Some(rp) = revised_prompt {
        all_parts.push(Part::Text {
            content: rp,
            extra_body: HashMap::new(),
        });
    }
    all_parts.extend(parts);

    let outputs = items_to_nodes(vec![Item::Message {
        id: None,
        role: Role::Assistant,
        parts: all_parts,
        extra_body: HashMap::new(),
    }]);

    let usage = obj.get("usage").and_then(|u| {
        let usage_obj = u.as_object()?;
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
    });

    Ok(UrpResponse {
        id,
        model: model.to_string(),
        created_at: obj.get("created").and_then(|v| v.as_i64()),
        output: outputs,
        finish_reason: Some(FinishReason::Stop),
        usage,
        extra_body: HashMap::new(),
    })
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

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn decodes_revised_prompt_images_and_usage() {
        let resp = decode_response(
            &json!({
                "created": 123456,
                "data": [
                    {
                        "revised_prompt": "a refined prompt",
                        "b64_json": "QUJD"
                    },
                    {
                        "url": "https://example.com/image.png"
                    }
                ],
                "usage": {
                    "input_tokens": 12,
                    "output_tokens": 34,
                    "input_tokens_details": {
                        "text_tokens": 12
                    },
                    "output_tokens_details": {
                        "image_tokens": 34
                    }
                }
            }),
            "gpt-image-1",
        )
        .expect("image response should decode");

        assert_eq!(resp.id, "123456");
        assert_eq!(resp.model, "gpt-image-1");
        assert_eq!(resp.finish_reason, Some(FinishReason::Stop));

        let outputs = crate::urp::nodes_to_items(&resp.output);
        let Item::Message { role, parts, .. } = &outputs[0] else {
            panic!("expected assistant message");
        };
        assert_eq!(*role, Role::Assistant);
        assert!(matches!(
            &parts[0],
            Part::Text { content, .. } if content == "a refined prompt"
        ));
        assert!(matches!(
            &parts[1],
            Part::Image {
                source: ImageSource::Base64 { media_type, data },
                ..
            } if media_type == "image/png" && data == "QUJD"
        ));
        assert!(matches!(
            &parts[2],
            Part::Image {
                source: ImageSource::Url { url, detail: None },
                ..
            } if url == "https://example.com/image.png"
        ));

        let usage = resp.usage.expect("usage should be present");
        assert_eq!(usage.input_tokens, 12);
        assert_eq!(usage.output_tokens, 34);
        assert_eq!(
            usage
                .input_details
                .and_then(|d| d.modality_breakdown)
                .and_then(|m| m.text_tokens),
            Some(12)
        );
        assert_eq!(
            usage
                .output_details
                .and_then(|d| d.modality_breakdown)
                .and_then(|m| m.image_tokens),
            Some(34)
        );
    }

    #[test]
    fn rejects_image_responses_without_images() {
        let err = decode_response(&json!({ "data": [{}] }), "gpt-image-1")
            .expect_err("missing image payload should fail");
        assert!(err.contains("no images found"));
    }
}
