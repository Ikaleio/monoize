//! Media a Chat stream can carry (MT41-MT46): generated audio only as native `message.audio`.

use crate::urp::encode::openai_chat::validate_response_nodes;
use crate::urp::{
    AudioSource, CHAT_MESSAGE_AUDIO_EXTRA_KEY, NodeDelta, NodeHeader, OrdinaryRole, UrpStreamEvent,
};
use serde_json::Value;
use std::collections::HashMap;

/// Whether `extra_body` selects the native assistant `message.audio` shape.
pub(super) fn is_message_audio(extra_body: &HashMap<String, Value>) -> bool {
    extra_body
        .get(CHAT_MESSAGE_AUDIO_EXTRA_KEY)
        .and_then(Value::as_bool)
        == Some(true)
}

/// The media error of an event the Chat stream cannot represent. `message_audio` tells whether
/// the node of an audio fragment opened as native `message.audio`.
pub(super) fn validate_event(event: &UrpStreamEvent, message_audio: bool) -> Result<(), String> {
    match event {
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
        ) =>
        {
            Err("Native response content cannot contain input-only media items".into())
        }
        UrpStreamEvent::NodeStart {
            header:
                NodeHeader::Audio {
                    role: OrdinaryRole::Assistant,
                    ..
                },
            extra_body,
            ..
        } if is_message_audio(extra_body) => Ok(()),
        UrpStreamEvent::NodeDelta {
            delta: NodeDelta::Audio { .. },
            ..
        } if !message_audio => {
            Err("Chat audio fragments require a native message.audio lifecycle".into())
        }
        UrpStreamEvent::NodeStart {
            header: NodeHeader::Image { .. } | NodeHeader::File { .. } | NodeHeader::Audio { .. },
            ..
        }
        | UrpStreamEvent::NodeDelta {
            delta:
                NodeDelta::Image { .. }
                | NodeDelta::File { .. }
                | NodeDelta::Audio {
                    source: AudioSource::Url { .. },
                },
            ..
        } => Err("Chat Completions responses cannot represent ordinary media content".into()),
        UrpStreamEvent::NodeDone { node, .. } => {
            validate_response_nodes(std::slice::from_ref(node))
        }
        UrpStreamEvent::ResponseDone { output, .. } => output
            .iter()
            .try_for_each(|entry| validate_response_nodes(std::slice::from_ref(&entry.node))),
        _ => Ok(()),
    }
}
