//! Responses input media parts and media carrier validation.

use crate::urp::encode::merge_extra;
use crate::urp::{
    FileSource, ImageSource, MediaMetadata, Node, ProviderProtocol, ToolResultContent,
};
use serde_json::{Map, Value, json};
use std::collections::HashMap;

pub(crate) fn encode_input_image(
    source: &ImageSource,
    metadata: &MediaMetadata,
    extra_body: &HashMap<String, Value>,
) -> Option<Value> {
    let mut obj = Map::new();
    merge_extra(&mut obj, extra_body);
    for key in [
        "type",
        "source",
        "image_url",
        "url",
        "file_id",
        "detail",
        "filename",
        "media_type",
    ] {
        obj.remove(key);
    }
    obj.insert("type".into(), json!("input_image"));
    let detail = match source {
        ImageSource::Url { url, detail } => {
            obj.insert("image_url".into(), json!(url));
            detail.as_ref()
        }
        ImageSource::Base64 { media_type, data } => {
            obj.insert(
                "image_url".into(),
                json!(format!("data:{media_type};base64,{data}")),
            );
            metadata.detail.as_ref()
        }
        ImageSource::FileId { file_id, detail }
            if crate::urp::media::resource_matches(metadata, ProviderProtocol::Responses) =>
        {
            obj.insert("file_id".into(), json!(file_id));
            detail.as_ref()
        }
        ImageSource::FileId { .. } => return None,
    };
    if let Some(detail) = detail {
        obj.insert("detail".into(), json!(detail));
    }
    Some(Value::Object(obj))
}

pub(crate) fn encode_input_file(
    source: &FileSource,
    metadata: &MediaMetadata,
    extra_body: &HashMap<String, Value>,
) -> Option<Value> {
    let mut obj = Map::new();
    merge_extra(&mut obj, extra_body);
    for key in [
        "type",
        "source",
        "file_url",
        "url",
        "file_id",
        "file_data",
        "filename",
        "detail",
        "media_type",
    ] {
        obj.remove(key);
    }
    obj.insert("type".into(), json!("input_file"));
    match source {
        FileSource::Url { url } => {
            obj.insert("file_url".into(), json!(url));
        }
        FileSource::FileId { file_id }
            if crate::urp::media::resource_matches(metadata, ProviderProtocol::Responses) =>
        {
            obj.insert("file_id".into(), json!(file_id));
        }
        FileSource::Base64 { media_type, data } => {
            obj.insert(
                "file_data".into(),
                json!(format!("data:{media_type};base64,{data}")),
            );
        }
        FileSource::FileId { .. } | FileSource::Text { .. } | FileSource::Content { .. } => {
            return None;
        }
    }
    if let Some(filename) = &metadata.filename {
        obj.insert("filename".into(), json!(filename));
    }
    if let Some(detail) = &metadata.detail {
        obj.insert("detail".into(), json!(detail));
    }
    Some(Value::Object(obj))
}

/// The stable Responses schema has no audio carrier for opaque audio items.
pub(super) fn validate_stable_responses_audio(nodes: &[Node]) -> Result<(), String> {
    fn audio_kind(kind: &str) -> bool {
        matches!(kind, "audio" | "input_audio" | "output_audio")
    }
    for node in nodes {
        let unsupported = match node {
            Node::ProviderItem { item_type, .. } => audio_kind(item_type),
            Node::ToolResult { content, .. } => content.iter().any(|part| {
                matches!(part, ToolResultContent::ProviderItem { item_type, .. } if audio_kind(item_type))
            }),
            _ => false,
        };
        if unsupported {
            return Err("The stable Responses schema has no audio content carrier".into());
        }
    }
    Ok(())
}
