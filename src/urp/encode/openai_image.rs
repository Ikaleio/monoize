use crate::urp::{ImageGenerationOptions, ImageSource, Node, OrdinaryRole, UrpRequest};
use base64::Engine as _;
use serde_json::{Map, Value};

pub fn encode_request(req: &UrpRequest, model: &str) -> Value {
    encode_request_checked(req, model)
        .unwrap_or_else(|message| crate::urp::media::error_body(&message))
}

pub fn encode_request_checked(req: &UrpRequest, model: &str) -> Result<Value, String> {
    let prepared = prepare_request(req)?;
    if has_user_image_input(&prepared) {
        validate_edit_images(&prepared)?;
        if edit_requires_json(req) {
            return Ok(encode_edit_body(&prepared, model));
        }
    }
    Ok(Value::Object(request_fields(&prepared, model)))
}

fn prepare_request(req: &UrpRequest) -> Result<UrpRequest, String> {
    if let Some(options) = &req.image_generation {
        options.validate()?;
    }
    crate::urp::media::prepare_request(req, crate::urp::ProviderProtocol::OpenaiImage)
}

fn request_fields(req: &UrpRequest, model: &str) -> Map<String, Value> {
    let mut body = Map::new();
    for (key, value) in &req.extra_body {
        if key.starts_with("_monoize_")
            || matches!(
                key.as_str(),
                "model" | "prompt" | "stream" | "user" | "image" | "images" | "mask"
            )
            || (req.image_generation.is_some()
                && ImageGenerationOptions::KEYS.contains(&key.as_str()))
        {
            continue;
        }
        body.insert(key.clone(), value.clone());
    }
    if let Some(options) = &req.image_generation {
        body.extend(options.to_object());
    }
    body.insert("model".to_string(), Value::String(model.to_string()));
    body.insert("prompt".to_string(), Value::String(user_prompt(req)));
    if req.stream == Some(true) {
        body.insert("stream".to_string(), Value::Bool(true));
    }
    if let Some(user) = &req.user {
        body.insert("user".to_string(), Value::String(user.clone()));
    }
    body
}

pub fn edit_requires_json(req: &UrpRequest) -> bool {
    req.input.iter().any(|node| {
        matches!(
            node,
            Node::Image {
                role: OrdinaryRole::User,
                source: ImageSource::Url { .. } | ImageSource::FileId { .. },
                ..
            }
        )
    })
}

pub fn encode_edit_request(req: &UrpRequest, model: &str) -> Result<Value, String> {
    let prepared = prepare_request(req)?;
    validate_edit_images(&prepared)?;
    Ok(encode_edit_body(&prepared, model))
}

fn encode_edit_body(req: &UrpRequest, model: &str) -> Value {
    let mut body = request_fields(req, model);
    let mut images = Vec::new();
    for node in &req.input {
        let Node::Image {
            role: OrdinaryRole::User,
            source,
            metadata,
            ..
        } = node
        else {
            continue;
        };
        let reference = match source {
            ImageSource::Base64 { media_type, data } => {
                serde_json::json!({ "image_url": format!("data:{media_type};base64,{data}") })
            }
            ImageSource::Url { url, .. } => serde_json::json!({ "image_url": url }),
            ImageSource::FileId { file_id, .. } => serde_json::json!({ "file_id": file_id }),
        };
        if metadata.image_mask {
            body.insert("mask".to_string(), reference);
        } else {
            images.push(reference);
        }
    }
    body.insert("images".to_string(), Value::Array(images));
    Value::Object(body)
}

fn validate_edit_images(req: &UrpRequest) -> Result<usize, String> {
    let mut images = 0;
    let mut masks = 0;
    for node in &req.input {
        if let Node::Image {
            role: OrdinaryRole::User,
            metadata,
            ..
        } = node
        {
            if metadata.image_mask {
                masks += 1;
            } else {
                images += 1;
            }
        }
    }
    if !(1..=16).contains(&images) {
        return Err("image edits require 1 through 16 source images".to_string());
    }
    if masks > 1 {
        return Err("image edits support at most one mask".to_string());
    }
    Ok(images)
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

/// One part of the upstream edit multipart body (OIU-E5a..OIU-E5f), in send
/// order. The intermediate representation exists so the same parts feed both
/// the sent `reqwest` form and the RCD-D6a/RCD-D16 capture object (OIU-E5g)
/// without the two ever diverging.
pub enum MultipartField {
    Text {
        name: String,
        value: String,
    },
    File {
        name: String,
        filename: String,
        content_type: String,
        bytes: Vec<u8>,
    },
}

pub fn multipart_fields(req: &UrpRequest, model: &str) -> Result<Vec<MultipartField>, String> {
    if edit_requires_json(req) {
        return Err("image references require a JSON image edit request".to_string());
    }
    let prepared = prepare_request(req)?;
    let req = &prepared;
    let image_count = validate_edit_images(req)?;
    let mut body = request_fields(req, model);
    body.remove("model");
    body.remove("prompt");
    let mut fields = vec![
        MultipartField::Text {
            name: "model".to_string(),
            value: model.to_string(),
        },
        MultipartField::Text {
            name: "prompt".to_string(),
            value: user_prompt(req),
        },
    ];

    for (key, value) in body {
        fields.push(MultipartField::Text {
            name: key,
            value: extra_value_to_text(&value),
        });
    }

    for (idx, item) in req.input.iter().enumerate() {
        let Node::Image {
            role: OrdinaryRole::User,
            source,
            metadata,
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
            ImageSource::Url { .. } | ImageSource::FileId { .. } => {
                return Err("image references require a JSON image edit request".to_string());
            }
        };
        let field_name = if metadata.image_mask {
            "mask"
        } else if image_count > 1 {
            "image[]"
        } else {
            "image"
        };
        fields.push(MultipartField::File {
            name: field_name.to_string(),
            filename: format!("image-{idx}"),
            content_type: media_type,
            bytes,
        });
    }

    Ok(fields)
}

pub fn form_from_fields(fields: Vec<MultipartField>) -> Result<reqwest::multipart::Form, String> {
    let mut form = reqwest::multipart::Form::new();
    for field in fields {
        form = match field {
            MultipartField::Text { name, value } => form.text(name, value),
            MultipartField::File {
                name,
                filename,
                content_type,
                bytes,
            } => {
                let part = reqwest::multipart::Part::bytes(bytes)
                    .file_name(filename)
                    .mime_str(&content_type)
                    .map_err(|e| format!("invalid image media type: {e}"))?;
                form.part(name, part)
            }
        };
    }
    Ok(form)
}

pub fn multipart_form(req: &UrpRequest, model: &str) -> Result<reqwest::multipart::Form, String> {
    form_from_fields(multipart_fields(req, model)?)
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
