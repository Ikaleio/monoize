use crate::urp::{ImageGenerationOptions, ImageSource, Node, OrdinaryRole, UrpRequest};
use serde_json::{Map, Value, json};

pub fn encode_request(req: &UrpRequest, model: &str) -> Result<Value, String> {
    if let Some(options) = &req.image_generation {
        options.validate()?;
    }
    let req =
        crate::urp::media::prepare_request(req, crate::urp::ProviderProtocol::OpenrouterImage)?;
    let mut object = Map::new();
    for (key, value) in &req.extra_body {
        if key.starts_with("_monoize_")
            || matches!(
                key.as_str(),
                "model"
                    | "prompt"
                    | "stream"
                    | "user"
                    | "input_references"
                    | "image"
                    | "images"
                    | "mask"
            )
            || (req.image_generation.is_some()
                && ImageGenerationOptions::KEYS.contains(&key.as_str()))
        {
            continue;
        }
        object.insert(key.clone(), value.clone());
    }
    if let Some(options) = &req.image_generation {
        object.extend(options.to_object().into_iter().filter(|(key, _)| {
            matches!(
                key.as_str(),
                "n" | "size" | "quality" | "background" | "output_format" | "output_compression"
            )
        }));
    }
    let prompt = req
        .input
        .iter()
        .filter_map(|node| match node {
            Node::Text {
                role: OrdinaryRole::User,
                content,
                ..
            } if !content.trim().is_empty() => Some(content.as_str()),
            _ => None,
        })
        .collect::<Vec<_>>()
        .join("\n");
    object.insert("model".to_string(), Value::String(model.to_string()));
    object.insert("prompt".to_string(), Value::String(prompt));
    if req.stream == Some(true) {
        object.insert("stream".to_string(), Value::Bool(true));
    }
    let mut references = Vec::new();
    for node in &req.input {
        let Node::Image {
            metadata,
            role: OrdinaryRole::User,
            source,
            ..
        } = node
        else {
            continue;
        };
        if metadata.image_mask {
            return Err("OpenRouter Images does not support image masks".to_string());
        }
        let url = match source {
            ImageSource::Url { url, .. } => url.clone(),
            ImageSource::Base64 { media_type, data } => {
                format!("data:{media_type};base64,{data}")
            }
            ImageSource::FileId { .. } => {
                return Err("OpenRouter Images does not support file_id image inputs".to_string());
            }
        };
        references.push(json!({"type": "image_url", "image_url": {"url": url}}));
    }
    if !references.is_empty() {
        object.insert("input_references".to_string(), Value::Array(references));
    }
    if let Some(user) = &req.user {
        object.insert("user".to_string(), Value::String(user.clone()));
    }
    Ok(Value::Object(object))
}
