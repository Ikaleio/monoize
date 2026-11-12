use super::blocks::encode_provider_block;
use crate::urp::encode::merge_extra;
use crate::urp::{Node, OrdinaryRole};
use serde_json::{Map, Value, json};

/// A cross-protocol mid-conversation system or developer block waiting for the next message.
pub(super) struct DeferredSystemBlock {
    pub(super) block: Value,
    /// Instruction text, present only for text blocks; only text can become a reminder.
    pub(super) text: Option<String>,
}

impl DeferredSystemBlock {
    /// Renders the block as reminder text inside a user message (MSG-1a).
    pub(super) fn into_reminder(self) -> Value {
        let mut block = self.block;
        block["text"] = json!(format!(
            "<system-reminder>\n{}\n</system-reminder>",
            self.text.unwrap_or_default()
        ));
        block
    }
}

pub(super) fn encode_system_block(node: &Node) -> Option<Value> {
    match node {
        Node::Text {
            content,
            extra_body,
            ..
        } if !content.is_empty() => {
            let mut block = Map::new();
            block.insert("type".to_string(), json!("text"));
            block.insert("text".to_string(), Value::String(content.clone()));
            merge_extra(&mut block, extra_body);
            Some(Value::Object(block))
        }
        Node::ProviderItem {
            id,
            role: OrdinaryRole::System | OrdinaryRole::Developer,
            origin_protocol,
            item_type,
            body,
            extra_body,
        } => encode_provider_block(*origin_protocol, Some(id), item_type, body, extra_body),
        _ => None,
    }
}

/// Text that keeps a system message carrying only inline tool changes non-empty. Relays
/// that do not implement `tool_addition` / `tool_removal` drop those blocks and then reject
/// the message because no block remains.
pub(super) fn system_tool_change_note(blocks: &[Value]) -> Option<Value> {
    if has_nonempty_text_block(blocks) {
        return None;
    }
    let notes: Vec<String> = blocks
        .iter()
        .filter_map(|block| {
            let kind = block.get("type").and_then(Value::as_str);
            let name = block
                .pointer("/tool/name")
                .or_else(|| block.pointer("/tool/definition/name"))
                .and_then(Value::as_str);
            match (kind, name) {
                (Some("tool_removal"), Some(name)) => {
                    Some(format!("Tool {name} is no longer available."))
                }
                (Some("tool_addition"), Some(name)) => {
                    Some(format!("Tool {name} is now available."))
                }
                (Some("tool_removal" | "tool_addition"), None) => {
                    Some("The available tools changed.".to_string())
                }
                _ => None,
            }
        })
        .collect();
    (!notes.is_empty()).then(|| json!({ "type": "text", "text": notes.join(" ") }))
}

fn has_nonempty_text_block(blocks: &[Value]) -> bool {
    blocks.iter().any(|block| {
        block.get("type").and_then(Value::as_str) == Some("text")
            && block
                .get("text")
                .and_then(Value::as_str)
                .is_some_and(|text| !text.trim().is_empty())
    })
}
