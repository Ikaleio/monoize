//! Responses `image_generation` tool requests and `image_generation_call` items.

use crate::urp::{
    ImageGenerationOptions, ImageSource, MediaMetadata, Node, OrdinaryRole, ProviderProtocol,
    RESPONSES_IMAGE_GENERATION_CALL_EXTRA_KEY, ToolDefinition, UrpRequest, is_internal_key,
};
use serde_json::{Map, Value, json};
use std::collections::HashMap;

const IMAGE_GENERATION_TOOL_SETTINGS: [&str; 12] = [
    "size",
    "quality",
    "complexity",
    "svg",
    "frame_count",
    "frame_duration_ms",
    "background",
    "output_format",
    "output_compression",
    "moderation",
    "partial_images",
    "input_fidelity",
];

fn compatible_image_generation_tool(tool: &ToolDefinition) -> bool {
    tool.tool_type == "image_generation"
        && tool
            .origin_protocol
            .is_none_or(|origin| origin == ProviderProtocol::Responses)
}

/// Moves Images settings and the edit mask of `req` into its Responses `image_generation`
/// tool configuration.
pub(super) fn prepare_image_generation_request(req: &mut UrpRequest) -> Result<(), String> {
    let mut mask = None;
    let mut source_images = 0;
    for node in &req.input {
        let Node::Image {
            role,
            source,
            metadata,
            ..
        } = node
        else {
            continue;
        };
        if !metadata.image_mask {
            if *role == OrdinaryRole::User {
                source_images += 1;
            }
            continue;
        }
        if *role != OrdinaryRole::User {
            return Err("An image edit mask must have the user role.".into());
        }
        if mask.is_some() {
            return Err("Responses image generation supports one image edit mask.".into());
        }
        mask = Some(match source {
            ImageSource::Base64 { media_type, data } => {
                json!({"image_url": format!("data:{media_type};base64,{data}")})
            }
            ImageSource::Url { url, .. } => json!({"image_url": url}),
            ImageSource::FileId { file_id, .. } => json!({"file_id": file_id}),
        });
    }
    if req.image_generation.is_none() && mask.is_none() {
        return Ok(());
    }
    if !req
        .tools
        .as_ref()
        .is_some_and(|tools| tools.iter().any(compatible_image_generation_tool))
    {
        return Err(
            "Images requests and image masks require a Responses image_generation tool. Configure image_enable_openai_generation_tool on this route."
                .into(),
        );
    }
    if mask.is_some() && source_images == 0 {
        return Err("An image edit mask requires a user source image.".into());
    }

    let settings = match &req.image_generation {
        Some(options) => {
            options.validate()?;
            options.to_object()
        }
        None => Map::new(),
    };
    if settings.contains_key("style") {
        return Err(
            "Responses image generation does not support the Images style parameter.".into(),
        );
    }
    if settings.get("n").is_some_and(|n| n.as_u64() != Some(1)) {
        return Err("Responses image generation requires n = 1 per sub-request.".into());
    }
    if settings
        .get("response_format")
        .is_some_and(|format| format.as_str() != Some("b64_json"))
    {
        return Err("Responses image generation supports only response_format = b64_json.".into());
    }

    for tool in req
        .tools
        .iter_mut()
        .flatten()
        .filter(|tool| compatible_image_generation_tool(tool))
    {
        let config = tool.config.get_or_insert_with(|| json!({}));
        let config = config
            .as_object_mut()
            .ok_or("Image generation tool config must be an object.")?;
        for key in IMAGE_GENERATION_TOOL_SETTINGS {
            if let Some(value) = settings.get(key) {
                config.insert(key.to_owned(), value.clone());
                tool.extra_body.remove(key);
            }
        }
        if let Some(mask) = &mask {
            config.insert("input_image_mask".into(), mask.clone());
            tool.extra_body.remove("input_image_mask");
        }
    }
    req.input
        .retain(|node| !matches!(node, Node::Image { metadata, .. } if metadata.image_mask));
    if req.image_generation.is_some() {
        req.extra_body.retain(|key, value| {
            !ImageGenerationOptions::KEYS.contains(&key.as_str())
                || (key == "background" && value.is_boolean())
        });
    }
    Ok(())
}

/// Rebuilds a top-level `image_generation_call` from an Image node that carries the Responses
/// image-generation shape marker and Base64 bytes (PR5c). `None` for any other image.
pub(crate) fn encode_image_generation_call_item(
    id: Option<&str>,
    source: &ImageSource,
    metadata: &MediaMetadata,
    extra_body: &HashMap<String, Value>,
) -> Option<Value> {
    extra_body.get(RESPONSES_IMAGE_GENERATION_CALL_EXTRA_KEY)?;
    let ImageSource::Base64 { data, .. } = source else {
        return None;
    };
    let mut item = Map::new();
    item.insert("type".to_string(), json!("image_generation_call"));
    item.insert("result".to_string(), Value::String(data.clone()));
    if let Some(id) = id.filter(|id| !id.is_empty()) {
        item.insert("id".to_string(), Value::String(id.to_string()));
    }
    for (key, value) in extra_body {
        if !is_internal_key(key)
            && !matches!(key.as_str(), "id" | "result" | "output_format" | "type")
        {
            item.entry(key.clone()).or_insert_with(|| value.clone());
        }
    }
    metadata
        .image_generation
        .for_source(source)
        .apply_to(&mut item);
    item.retain(|key, _| !is_internal_key(key));
    Some(Value::Object(item))
}
