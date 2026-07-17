use crate::urp::{ImageSource, Node, OrdinaryRole, UrpRequest};
use base64::Engine as _;
use serde_json::{Map, Value};

pub fn encode_request(req: &UrpRequest, model: &str) -> Value {
    let mut prompt_parts: Vec<String> = Vec::new();
    for item in &req.input {
        if let Node::Text {
            role: OrdinaryRole::User,
            content,
            ..
        } = item
            && !content.trim().is_empty()
        {
            prompt_parts.push(content.clone());
        }
    }
    let prompt = prompt_parts.join("\n");

    let mut body = Map::new();
    body.insert("model".to_string(), Value::String(model.to_string()));
    body.insert("prompt".to_string(), Value::String(prompt));
    if req.stream == Some(true) {
        body.insert("stream".to_string(), Value::Bool(true));
    }

    for (k, v) in &req.extra_body {
        if !k.starts_with("_monoize_") && k != "model" && k != "prompt" && k != "stream" {
            body.insert(k.clone(), v.clone());
        }
    }

    Value::Object(body)
}

pub fn has_user_image_input(req: &UrpRequest) -> bool {
    req.input.iter().any(|item| {
        matches!(
            item,
            Node::Image {
                role: OrdinaryRole::User,
                ..
            }
        )
    })
}

pub fn multipart_form(req: &UrpRequest, model: &str) -> Result<reqwest::multipart::Form, String> {
    let prompt = user_prompt(req);
    let mut form = reqwest::multipart::Form::new()
        .text("model", model.to_string())
        .text("prompt", prompt);

    if req.stream == Some(true) {
        form = form.text("stream", "true");
    }

    for (k, v) in &req.extra_body {
        if k.starts_with("_monoize_") || k == "model" || k == "prompt" || k == "stream" {
            continue;
        }
        form = form.text(k.clone(), extra_value_to_text(v));
    }

    for (idx, item) in req.input.iter().enumerate() {
        let Node::Image {
            id,
            role: OrdinaryRole::User,
            source,
            ..
        } = item
        else {
            continue;
        };
        let (media_type, bytes) = match source {
            ImageSource::Base64 { media_type, data } => {
                let bytes = base64::engine::general_purpose::STANDARD
                    .decode(data)
                    .map_err(|e| format!("invalid base64 image input: {e}"))?;
                (media_type.clone(), bytes)
            }
            ImageSource::Url { url, .. } => ("text/plain".to_string(), url.as_bytes().to_vec()),
            ImageSource::FileId { .. } => {
                return Err("file_id image input is unsupported by the image API".to_string());
            }
        };
        let field_name = if id.as_deref() == Some("__monoize_image_api_mask") {
            "mask"
        } else {
            "image"
        };
        let part = reqwest::multipart::Part::bytes(bytes)
            .file_name(format!("image-{idx}"))
            .mime_str(&media_type)
            .map_err(|e| format!("invalid image media type: {e}"))?;
        form = form.part(field_name.to_string(), part);
    }

    Ok(form)
}

fn user_prompt(req: &UrpRequest) -> String {
    let mut prompt_parts: Vec<String> = Vec::new();
    for item in &req.input {
        if let Node::Text {
            role: OrdinaryRole::User,
            content,
            ..
        } = item
            && !content.trim().is_empty()
        {
            prompt_parts.push(content.clone());
        }
    }
    prompt_parts.join("\n")
}

fn extra_value_to_text(value: &Value) -> String {
    match value {
        Value::String(s) => s.clone(),
        other => other.to_string(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::urp::{Node, OrdinaryRole};
    use serde_json::json;
    use std::collections::HashMap;

    fn empty_map() -> HashMap<String, Value> {
        HashMap::new()
    }

    #[test]
    fn encodes_user_text_parts_into_prompt_and_preserves_allowed_extra_fields() {
        let req = UrpRequest {
            model: "logical-model".to_string(),
            input: vec![
                Node::Text {
                    id: None,
                    role: OrdinaryRole::System,
                    content: "ignore system".to_string(),
                    phase: None,
                    extra_body: empty_map(),
                },
                Node::Text {
                    id: None,
                    role: OrdinaryRole::User,
                    content: "draw a cat".to_string(),
                    phase: None,
                    extra_body: empty_map(),
                },
                Node::Text {
                    id: None,
                    role: OrdinaryRole::User,
                    content: "  ".to_string(),
                    phase: None,
                    extra_body: empty_map(),
                },
                Node::Text {
                    id: None,
                    role: OrdinaryRole::Assistant,
                    content: "ignore assistant".to_string(),
                    phase: None,
                    extra_body: empty_map(),
                },
                Node::Text {
                    id: None,
                    role: OrdinaryRole::User,
                    content: "in watercolor".to_string(),
                    phase: None,
                    extra_body: empty_map(),
                },
            ],
            stream: Some(true),
            temperature: Some(0.3),
            top_p: Some(0.9),
            max_output_tokens: Some(100),
            reasoning: None,
            tools: None,
            tool_choice: None,
            parallel_tool_calls: None,
            response_format: None,
            user: None,
            extra_body: HashMap::from([
                ("size".to_string(), json!("1024x1024")),
                ("n".to_string(), json!(2)),
                ("stream".to_string(), json!(true)),
                ("prompt".to_string(), json!("override")),
                ("model".to_string(), json!("override-model")),
            ]),
        };

        let encoded = encode_request(&req, "gpt-image-1");
        assert_eq!(encoded["model"], json!("gpt-image-1"));
        assert_eq!(encoded["prompt"], json!("draw a cat\nin watercolor"));
        assert_eq!(encoded["size"], json!("1024x1024"));
        assert_eq!(encoded["n"], json!(2));
        assert_eq!(encoded["stream"], json!(true));
    }

    #[test]
    fn encodes_gpt_image_2_custom_size() {
        let req = UrpRequest {
            model: "logical-model".to_string(),
            input: vec![Node::Text {
                id: None,
                role: OrdinaryRole::User,
                content: "draw a blue square".to_string(),
                phase: None,
                extra_body: empty_map(),
            }],
            stream: Some(false),
            temperature: None,
            top_p: None,
            max_output_tokens: None,
            reasoning: None,
            tools: None,
            tool_choice: None,
            parallel_tool_calls: None,
            response_format: None,
            user: None,
            extra_body: HashMap::from([("size".to_string(), json!("1280x720"))]),
        };

        let encoded = encode_request(&req, "gpt-image-2");
        assert_eq!(encoded["model"], json!("gpt-image-2"));
        assert_eq!(encoded["prompt"], json!("draw a blue square"));
        assert_eq!(encoded["size"], json!("1280x720"));
    }

    #[test]
    fn omits_stream_field_when_stream_is_false() {
        let req = UrpRequest {
            model: "logical-model".to_string(),
            input: vec![Node::Text {
                id: None,
                role: OrdinaryRole::User,
                content: "draw a blue square".to_string(),
                phase: None,
                extra_body: empty_map(),
            }],
            stream: Some(false),
            temperature: None,
            top_p: None,
            max_output_tokens: None,
            reasoning: None,
            tools: None,
            tool_choice: None,
            parallel_tool_calls: None,
            response_format: None,
            user: None,
            extra_body: HashMap::from([("stream".to_string(), json!(true))]),
        };

        let encoded = encode_request(&req, "gpt-image-2");
        assert!(encoded.get("stream").is_none());
    }
}
