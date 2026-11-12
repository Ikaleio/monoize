//! Responses media rules on stream events (media-transport, native image_generation_call).

use super::ResponsesStreamEncoder;
use super::nodes::NodeKind;
use crate::urp::encode::openai_responses::prepare_response_nodes;
use crate::urp::{
    ImageSource, NodeDelta, NodeHeader, RESPONSES_IMAGE_GENERATION_CALL_EXTRA_KEY, UrpStreamEvent,
    is_internal_key,
};
use serde_json::{Map, Value};
use std::collections::HashMap;

impl ResponsesStreamEncoder {
    /// Rejects media the Responses output schema cannot carry and prepares tool-result media
    /// of completed nodes in place.
    pub(super) fn validate_media_event(&self, event: &mut UrpStreamEvent) -> Result<(), String> {
        match event {
            UrpStreamEvent::NodeDelta {
                node_index,
                delta: NodeDelta::Image { .. },
                extra_body,
                ..
            } if !self
                .nodes
                .get(&*node_index)
                .is_some_and(|state| state.kind == NodeKind::Image)
                && image_generation_call_downstream_event(extra_body).is_none() =>
            {
                Err(
                    "Responses image fragments require a native image_generation_call lifecycle"
                        .into(),
                )
            }
            UrpStreamEvent::NodeStart {
                header: NodeHeader::ProviderItem { item_type, .. },
                ..
            } if matches!(
                item_type.as_str(),
                "input_image"
                    | "output_image"
                    | "image_url"
                    | "input_file"
                    | "output_file"
                    | "file"
                    | "input_audio"
                    | "audio"
                    | "output_audio"
            ) =>
            {
                Err("Native response content cannot contain input-only media items".into())
            }
            UrpStreamEvent::NodeStart {
                header: NodeHeader::Image { .. },
                extra_body,
                ..
            } if extra_body.contains_key(RESPONSES_IMAGE_GENERATION_CALL_EXTRA_KEY) => Ok(()),
            UrpStreamEvent::NodeStart {
                header:
                    NodeHeader::Image { .. } | NodeHeader::File { .. } | NodeHeader::Audio { .. },
                ..
            }
            | UrpStreamEvent::NodeDelta {
                delta: NodeDelta::File { .. } | NodeDelta::Audio { .. },
                ..
            } => {
                Err("Responses output cannot represent ordinary image, file, or audio media".into())
            }
            UrpStreamEvent::NodeDelta {
                delta: NodeDelta::Image { source },
                ..
            } if !matches!(&*source, ImageSource::Base64 { media_type, .. }
                if matches!(media_type.as_str(), "image/png" | "image/jpeg" | "image/webp" | "image/svg+xml" | "image/gif")) =>
            {
                Err("Responses image_generation_call requires supported Base64 image bytes".into())
            }
            UrpStreamEvent::NodeDone { node, .. } => {
                prepare_response_nodes(std::slice::from_mut(node))
            }
            UrpStreamEvent::ResponseDone { output, .. } => {
                output.iter_mut().try_for_each(|entry| {
                    prepare_response_nodes(std::slice::from_mut(&mut entry.node))
                })
            }
            _ => Ok(()),
        }
    }
}

/// The downstream event name of an image-generation progress fragment, when the fragment
/// came from a native image-generation event.
pub(super) fn image_generation_call_downstream_event(
    extra_body: &HashMap<String, Value>,
) -> Option<String> {
    let event_type = extra_body
        .get("provider_event_type")
        .or_else(|| extra_body.get("type"))
        .and_then(Value::as_str)?;
    match event_type {
        "image_generation.partial_image"
        | "image_edit.partial_image"
        | "response.image_generation.partial_image" => {
            Some("response.image_generation_call.partial_image".to_string())
        }
        _ if event_type.starts_with("response.image_generation_call.") => {
            Some(event_type.to_string())
        }
        _ => None,
    }
}

fn is_event_metadata_key(key: &str) -> bool {
    is_internal_key(key) || matches!(key, "type" | "provider_event_type" | "sequence_number")
}

pub(super) fn image_generation_call_event_payload(
    data: &Value,
    extra_body: &HashMap<String, Value>,
) -> Value {
    let mut payload = match data {
        Value::Object(map) => map.clone(),
        Value::Null => Map::new(),
        other => Map::from_iter([("data".to_string(), other.clone())]),
    };
    for (key, value) in extra_body {
        if !is_event_metadata_key(key) {
            payload.entry(key.clone()).or_insert(value.clone());
        }
    }
    Value::Object(payload)
}

pub(super) fn image_generation_call_image_event_payload(
    source: &ImageSource,
    extra_body: &HashMap<String, Value>,
) -> Value {
    let mut payload: Map<String, Value> = extra_body
        .iter()
        .filter(|(key, _)| !is_event_metadata_key(key))
        .map(|(key, value)| (key.clone(), value.clone()))
        .collect();
    for key in ["partial_image_b64", "b64_json", "result"] {
        payload.remove(key);
    }
    if let ImageSource::Base64 { data, .. } = source {
        payload.insert("partial_image_b64".to_string(), Value::String(data.clone()));
    }
    Value::Object(payload)
}
