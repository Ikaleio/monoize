use crate::urp::encode::merge_extra;
use crate::urp::{FileSource, ImageSource, MediaMetadata, Node, ProviderProtocol};
use serde_json::{Map, Value, json};
use std::collections::HashMap;

pub(crate) fn messages_media_error_body(message: &str) -> Value {
    json!({"type":"error", "error":{"type":"api_error", "message":message}})
}

pub(crate) fn input_only_media_type(item_type: &str) -> bool {
    matches!(
        item_type,
        "image" | "document" | "file" | "audio" | "input_image" | "input_file" | "input_audio"
    )
}

pub(crate) fn validate_response_node(node: &Node) -> Result<(), String> {
    let kind = match node {
        Node::Image { .. } => Some("image"),
        Node::File { .. } => Some("document"),
        Node::Audio { .. } => Some("audio"),
        Node::ToolResult { .. } => Some("tool_result"),
        Node::ProviderItem {
            origin_protocol: ProviderProtocol::Messages,
            item_type,
            ..
        } if input_only_media_type(item_type) => Some(item_type.as_str()),
        _ => None,
    };
    match kind {
        Some(kind) => Err(format!(
            "Messages responses cannot represent top-level {kind} content"
        )),
        None => Ok(()),
    }
}

pub(crate) fn validate_response_nodes(nodes: &[Node]) -> Result<(), String> {
    nodes.iter().try_for_each(validate_response_node)
}

pub(super) fn encode_image_block(
    source: &ImageSource,
    metadata: &MediaMetadata,
    extra_body: &HashMap<String, Value>,
) -> Option<Value> {
    let source = match source {
        ImageSource::Url { url, .. } => match split_base64_image_data_url(url) {
            Some((media_type, data)) => {
                json!({ "type": "base64", "media_type": media_type, "data": data })
            }
            None => json!({ "type": "url", "url": url }),
        },
        ImageSource::Base64 { media_type, data } => {
            json!({ "type": "base64", "media_type": media_type, "data": data })
        }
        ImageSource::FileId { file_id, .. }
            if crate::urp::media::resource_matches(metadata, ProviderProtocol::Messages) =>
        {
            json!({ "type": "file", "file_id": file_id })
        }
        ImageSource::FileId { .. } => return None,
    };
    Some(Value::Object(media_block("image", source, extra_body)))
}

pub(super) fn encode_file_block(
    source: &FileSource,
    metadata: &MediaMetadata,
    extra_body: &HashMap<String, Value>,
) -> Option<Value> {
    let source = match source {
        FileSource::Url { url } => json!({ "type": "url", "url": url }),
        FileSource::Base64 { media_type, data } => {
            json!({ "type": "base64", "media_type": media_type, "data": data })
        }
        FileSource::Text { text } => {
            json!({ "type": "text", "media_type": "text/plain", "data": text })
        }
        FileSource::Content { content } => json!({ "type": "content", "content": content }),
        FileSource::FileId { file_id }
            if crate::urp::media::resource_matches(metadata, ProviderProtocol::Messages) =>
        {
            json!({ "type": "file", "file_id": file_id })
        }
        FileSource::FileId { .. } => return None,
    };
    let mut block = media_block("document", source, extra_body);
    if let Some(title) = &metadata.document_title {
        block.insert("title".into(), json!(title));
    }
    if let Some(context) = &metadata.document_context {
        block.insert("context".into(), json!(context));
    }
    if let Some(citations) = &metadata.document_citations {
        block.insert("citations".into(), citations.clone());
    }
    Some(Value::Object(block))
}

/// Typed metadata owns the document members; extras never supply them, and image blocks
/// have none.
fn media_block(
    kind: &str,
    source: Value,
    extra_body: &HashMap<String, Value>,
) -> Map<String, Value> {
    let mut block = Map::new();
    block.insert("type".to_string(), Value::String(kind.to_string()));
    block.insert("source".to_string(), source);
    merge_extra(&mut block, extra_body);
    for key in ["filename", "detail", "title", "context", "citations"] {
        block.remove(key);
    }
    block
}

fn split_base64_image_data_url(url: &str) -> Option<(&str, &str)> {
    let payload = url.strip_prefix("data:")?;
    let (metadata, data) = payload.split_once(',')?;
    let media_type = metadata.strip_suffix(";base64")?;
    if !media_type.starts_with("image/") || media_type.len() == "image/".len() || data.is_empty() {
        return None;
    }
    Some((media_type, data))
}
