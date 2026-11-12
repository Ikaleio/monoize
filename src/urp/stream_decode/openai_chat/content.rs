//! Content-block arrays of a Chat delta: text, reasoning, tool-call, media, and unrecognized
//! blocks.

use super::state::ChatStreamState;
use crate::error::AppResult;
use crate::urp::decode::parse_compatible_media_node;
use crate::urp::stream_helpers::extract_chat_reasoning_content_block;
use crate::urp::{
    Node, NodeDelta, NodeHeader, OrdinaryRole, ProviderProtocol, synthetic_provider_item_id,
};
use serde_json::{Map, Value};
use std::collections::HashMap;

impl ChatStreamState<'_> {
    pub(super) async fn on_content_blocks(&mut self, blocks: &[Value]) -> AppResult<()> {
        for (position, block) in blocks.iter().enumerate() {
            if let Some(text) = block.as_str() {
                self.close_reasoning(false).await?;
                self.push_text(text, None).await?;
                continue;
            }
            let Some(block_obj) = block.as_object() else {
                continue;
            };
            if let Some(reasoning) = extract_chat_reasoning_content_block(block) {
                self.close_text().await?;
                let format = reasoning.format.as_deref();
                if let Some(summary) = reasoning.summary.as_deref() {
                    self.push_reasoning_summary(summary, format).await?;
                }
                if let Some(content) = reasoning.content.as_deref() {
                    self.push_reasoning_text(content, format, None).await?;
                }
                if let Some(encrypted) = &reasoning.encrypted {
                    self.push_reasoning_encrypted(encrypted, format, None)
                        .await?;
                }
            } else if let Some(text) = text_block_text(block_obj) {
                self.close_reasoning(false).await?;
                self.push_text(text, None).await?;
            } else if block_is_tool_call_like(block_obj) {
                self.close_reasoning(false).await?;
                self.close_text().await?;
                self.on_tool_call_delta(block, position, false).await?;
            } else {
                self.start_content_item(block_obj).await?;
            }
        }
        Ok(())
    }

    /// Emits a media or unrecognized block as one complete node.
    pub(super) async fn start_content_item(&mut self, block: &Map<String, Value>) -> AppResult<()> {
        let mut node = parse_compatible_media_node(block, OrdinaryRole::Assistant)
            .ok()
            .flatten()
            .unwrap_or_else(|| Node::ProviderItem {
                id: block
                    .get("id")
                    .and_then(Value::as_str)
                    .map(str::to_string)
                    .or_else(|| Some(synthetic_provider_item_id())),
                origin_protocol: ProviderProtocol::ChatCompletion,
                role: OrdinaryRole::Assistant,
                item_type: block
                    .get("type")
                    .and_then(Value::as_str)
                    .unwrap_or("")
                    .to_string(),
                body: Value::Object(block.clone()),
                extra_body: HashMap::new(),
            });
        let mut extra_body = node.extra_body_mut().clone();
        extra_body.extend(self.take_delta_extra());
        let node_index = self.start_node(NodeHeader::from(&node), extra_body).await?;
        if let Some(delta) = media_source_delta(&node) {
            self.send_delta(node_index, delta, HashMap::new()).await?;
        }
        self.provider_items.push((node_index, node));
        Ok(())
    }
}

/// A single object block is a one-element block array.
pub(super) fn content_blocks(content: &Value) -> Option<&[Value]> {
    match content {
        Value::Array(blocks) => Some(blocks),
        Value::Object(_) => Some(std::slice::from_ref(content)),
        _ => None,
    }
}

/// A malformed media block ends the stream (`Err` carries the message).
pub(super) fn validate_content_media(content: &Value) -> Result<(), String> {
    for block in content_blocks(content).into_iter().flatten() {
        if let Some(block) = block.as_object() {
            parse_compatible_media_node(block, OrdinaryRole::Assistant)?;
        }
    }
    Ok(())
}

/// The text of a text-typed block.
pub(super) fn text_block_text(block: &Map<String, Value>) -> Option<&str> {
    let text = block.get("text").and_then(Value::as_str)?;
    matches!(
        block.get("type").and_then(Value::as_str),
        Some("input_text" | "text" | "output_text")
    )
    .then_some(text)
}

pub(super) fn block_is_tool_call_like(block: &Map<String, Value>) -> bool {
    matches!(
        block.get("type").and_then(Value::as_str),
        Some("tool_call" | "function_call" | "tool_use" | "custom_tool_call" | "custom")
    ) || ["function", "custom", "call_id", "input"]
        .iter()
        .any(|key| block.contains_key(*key))
}

/// The payload delta of a media node; a provider item carries its body on `NodeStart`.
fn media_source_delta(node: &Node) -> Option<NodeDelta> {
    match node {
        Node::Image { source, .. } => Some(NodeDelta::Image {
            source: source.clone(),
        }),
        Node::File { source, .. } => Some(NodeDelta::File {
            source: source.clone(),
        }),
        Node::Audio { source, .. } => Some(NodeDelta::Audio {
            source: source.clone(),
        }),
        _ => None,
    }
}
