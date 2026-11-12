//! Image generation progress and completion events.

use super::items::output_index;
use super::nodes::{image_node, node_extra, start_header, wire_extra};
use super::state::ResponsesStreamDecoder;
use crate::urp::{Node, NodeDelta, UrpStreamEvent};
use serde_json::Value;
use std::collections::HashMap;

fn is_partial_image(payload: &Value) -> bool {
    matches!(
        payload.get("type").and_then(Value::as_str),
        Some(
            "response.image_generation_call.partial_image"
                | "response.image_generation.partial_image"
                | "image_generation.partial_image"
        )
    )
}

/// The members of a progress event. `provider_event_type` names the native event, which
/// encoders forward from their own members.
fn progress_extra(payload: &Value) -> HashMap<String, Value> {
    let mut extra_body = wire_extra(payload, &["type", "sequence_number"]);
    if let Some(kind) = payload.get("type").and_then(Value::as_str) {
        extra_body.insert(
            "provider_event_type".to_string(),
            Value::String(kind.to_string()),
        );
    }
    extra_body
}

impl ResponsesStreamDecoder {
    /// An `image_generation.completed` image belongs to no output item and gets its own node.
    pub(super) fn on_image_completed(&mut self, data: &Value) -> Vec<UrpStreamEvent> {
        let Some(node) = image_node(data) else {
            return Vec::new();
        };
        let node_index = self.allocate();
        let id = match &node {
            Node::Image { id, .. } => id.clone(),
            _ => None,
        };
        self.standalone_images.push((node_index, id));
        let extra_body = node_extra(&node).clone();
        vec![
            UrpStreamEvent::NodeStart {
                node_index,
                header: start_header(&node),
                extra_body: extra_body.clone(),
            },
            UrpStreamEvent::NodeDone {
                node_index,
                node,
                usage: None,
                extra_body,
            },
        ]
    }

    /// A partial image becomes an image delta of the item's node; other progress events carry
    /// only their members.
    pub(super) fn on_image_call_event(&mut self, data: &Value) -> Vec<UrpStreamEvent> {
        let node_index = self.item_node_index(output_index(data));
        if is_partial_image(data)
            && let Some(Node::Image { source, .. }) = image_node(data)
        {
            let mut extra_body = progress_extra(data);
            for key in ["partial_image_b64", "b64_json", "result"] {
                extra_body.remove(key);
            }
            return vec![UrpStreamEvent::NodeDelta {
                node_index,
                delta: NodeDelta::Image { source },
                usage: None,
                extra_body,
            }];
        }
        vec![UrpStreamEvent::NodeDelta {
            node_index,
            delta: NodeDelta::ProviderItem { data: Value::Null },
            usage: None,
            extra_body: progress_extra(data),
        }]
    }
}
