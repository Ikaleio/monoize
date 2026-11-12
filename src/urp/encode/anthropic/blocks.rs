use super::media::{encode_file_block, encode_image_block};
use super::reasoning::{ThinkingAudience, encoded_signature_value, plaintext_from_reasoning};
use crate::urp::encode::{merge_extra, sanitize_provider_item_wire_body};
use crate::urp::{Citation, Node, OrdinaryRole, ProviderProtocol, ToolCallType, ToolResultContent};
use serde_json::{Map, Value, json};
use std::collections::HashMap;

/// Parses function-call arguments into a Messages `tool_use.input` value. Chat providers
/// send `""` for zero-argument calls, which Messages represents as `{}` (DM4).
fn messages_tool_input(arguments: &str) -> Option<Value> {
    if arguments.trim().is_empty() {
        return Some(json!({}));
    }
    let mut input = serde_json::from_str::<Value>(arguments).ok()?;
    crate::urp::integerize_json_floats(&mut input);
    Some(input)
}

/// Rejects function arguments that a complete Messages input object cannot represent.
pub(crate) fn validate_complete_tool_inputs(nodes: &[Node]) -> Result<(), String> {
    for node in nodes {
        if let Node::ToolCall {
            tool_type: ToolCallType::Function,
            arguments,
            ..
        } = node
            && !messages_tool_input(arguments).is_some_and(|value| value.is_object())
        {
            return Err("Messages tool input must be a complete JSON object".to_string());
        }
    }
    Ok(())
}

/// Encodes one content block of a request `user` or `assistant` message.
pub(super) fn encode_request_block(node: &Node) -> Option<Value> {
    match node {
        Node::Text {
            citations,
            content,
            extra_body,
            ..
        } => {
            // `phase` is a Responses concept; Anthropic rejects it as an extra input (PM2a).
            if content.is_empty() && extra_body.is_empty() {
                return None;
            }
            let mut block = text_block(content, citations);
            merge_extra(&mut block, extra_body);
            Some(Value::Object(block))
        }
        Node::Image {
            source,
            metadata,
            extra_body,
            ..
        } => encode_image_block(source, metadata, extra_body),
        Node::File {
            source,
            metadata,
            extra_body,
            ..
        } => encode_file_block(source, metadata, extra_body),
        Node::Reasoning { .. } => encode_thinking_block(node, ThinkingAudience::Upstream),
        Node::ToolCall { .. } => encode_tool_use_block(node),
        Node::ProviderItem {
            id,
            origin_protocol,
            item_type,
            body,
            extra_body,
            ..
        } => encode_provider_block(*origin_protocol, Some(id), item_type, body, extra_body),
        Node::Audio { .. }
        | Node::Refusal { .. }
        | Node::ToolResult { .. }
        | Node::NextDownstreamEnvelopeExtra { .. } => None,
    }
}

/// Encodes one content block of a Messages response or stream.
pub(crate) fn encode_assistant_response_block(node: &Node) -> Option<Value> {
    match node {
        Node::Text {
            citations,
            role: OrdinaryRole::Assistant,
            content,
            phase,
            extra_body,
            ..
        } => {
            if content.is_empty() && phase.is_none() && extra_body.is_empty() {
                return None;
            }
            let mut block = text_block(content, citations);
            if let Some(phase) = phase {
                block.insert("phase".to_string(), Value::String(phase.clone()));
            }
            merge_extra(&mut block, extra_body);
            Some(Value::Object(block))
        }
        Node::Reasoning { .. } => encode_thinking_block(node, ThinkingAudience::Downstream),
        Node::ToolCall { .. } => encode_tool_use_block(node),
        Node::ProviderItem {
            id,
            role: OrdinaryRole::Assistant,
            origin_protocol,
            item_type,
            body,
            extra_body,
        } => encode_provider_block(*origin_protocol, Some(id), item_type, body, extra_body),
        Node::Refusal { content, .. } if !content.is_empty() => {
            Some(json!({ "type": "text", "text": content }))
        }
        Node::Audio { .. }
        | Node::Refusal { .. }
        | Node::ToolResult { .. }
        | Node::NextDownstreamEnvelopeExtra { .. }
        | Node::Text { .. }
        | Node::Image { .. }
        | Node::File { .. }
        | Node::ProviderItem { .. } => None,
    }
}

pub(super) fn encode_tool_result_block(
    call_id: &str,
    namespace: Option<&str>,
    content: &[ToolResultContent],
    is_error: bool,
    extra_body: &HashMap<String, Value>,
) -> Value {
    let mut content: Vec<Value> = content
        .iter()
        .filter_map(|item| match item {
            ToolResultContent::Text {
                text, extra_body, ..
            } => {
                let mut block = Map::new();
                block.insert("type".to_string(), json!("text"));
                block.insert("text".to_string(), Value::String(text.clone()));
                merge_extra(&mut block, extra_body);
                Some(Value::Object(block))
            }
            ToolResultContent::Image {
                source,
                metadata,
                extra_body,
            } => encode_image_block(source, metadata, extra_body),
            ToolResultContent::File {
                source,
                metadata,
                extra_body,
            } => encode_file_block(source, metadata, extra_body),
            ToolResultContent::ProviderItem {
                origin_protocol,
                item_type,
                body,
                extra_body,
            } => encode_provider_block(*origin_protocol, None, item_type, body, extra_body),
        })
        .collect();
    if content.is_empty() {
        content.push(json!({ "type": "text", "text": "" }));
    }
    let mut block = Map::new();
    block.insert("type".to_string(), json!("tool_result"));
    block.insert("tool_use_id".to_string(), json!(call_id));
    block.insert("is_error".to_string(), Value::Bool(is_error));
    block.insert("content".to_string(), Value::Array(content));
    insert_toolset_name(&mut block, namespace, extra_body);
    Value::Object(block)
}

/// Re-emits a native Messages block captured as a ProviderItem; foreign items have no
/// Messages form.
pub(super) fn encode_provider_block(
    origin_protocol: ProviderProtocol,
    id: Option<&Option<String>>,
    item_type: &str,
    body: &Value,
    extra_body: &HashMap<String, Value>,
) -> Option<Value> {
    if origin_protocol != ProviderProtocol::Messages {
        return None;
    }
    let Value::Object(mut block) = sanitize_provider_item_wire_body(body) else {
        return None;
    };
    block.insert("type".to_string(), Value::String(item_type.to_string()));
    if block.contains_key("id")
        && let Some(id) = id
    {
        block.remove("id");
        if let Some(id) = id {
            block.insert("id".into(), Value::String(id.clone()));
        }
    }
    merge_extra(&mut block, extra_body);
    Some(Value::Object(block))
}

fn text_block(content: &str, citations: &[Citation]) -> Map<String, Value> {
    let mut block = Map::new();
    block.insert("type".to_string(), json!("text"));
    block.insert("text".to_string(), Value::String(content.to_string()));
    if !citations.is_empty() {
        block.insert(
            "citations".to_string(),
            Value::Array(crate::urp::citations::encode(
                citations,
                ProviderProtocol::Messages,
                0,
            )),
        );
    }
    block
}

fn encode_thinking_block(node: &Node, audience: ThinkingAudience) -> Option<Value> {
    let Node::Reasoning {
        metadata,
        id,
        content,
        summary,
        encrypted,
        extra_body,
        ..
    } = node
    else {
        return None;
    };
    let signature = encoded_signature_value(encrypted, id, audience);
    let mut block = Map::new();
    if metadata.redacted {
        block.insert("type".to_string(), json!("redacted_thinking"));
        block.insert("data".to_string(), signature?);
    } else {
        let text = plaintext_from_reasoning(content, summary);
        let signature = match (signature, text) {
            (Some(signature), _) => signature,
            // Anthropic requires a signature on every replayed thinking block (PM5d).
            (None, Some(_)) if audience == ThinkingAudience::Downstream => {
                Value::String(String::new())
            }
            (None, _) => return None,
        };
        block.insert("type".to_string(), json!("thinking"));
        block.insert(
            "thinking".to_string(),
            Value::String(text.unwrap_or_default().to_string()),
        );
        block.insert("signature".to_string(), signature);
    }
    merge_extra(&mut block, extra_body);
    Some(Value::Object(block))
}

/// Encodes a function call as `tool_use`; custom (freeform) calls have no Messages form.
fn encode_tool_use_block(node: &Node) -> Option<Value> {
    let Node::ToolCall {
        namespace,
        tool_type: ToolCallType::Function,
        call_id,
        name,
        arguments,
        extra_body,
        ..
    } = node
    else {
        return None;
    };
    let input = messages_tool_input(arguments)?;
    let mut block = Map::new();
    block.insert("type".to_string(), json!("tool_use"));
    block.insert("id".to_string(), Value::String(call_id.clone()));
    block.insert("name".to_string(), Value::String(name.clone()));
    block.insert("input".to_string(), input);
    insert_toolset_name(&mut block, namespace.as_deref(), extra_body);
    Some(Value::Object(block))
}

/// The typed namespace owns `toolset_name`; extras never supply it.
fn insert_toolset_name(
    block: &mut Map<String, Value>,
    namespace: Option<&str>,
    extra_body: &HashMap<String, Value>,
) {
    if let Some(namespace) = namespace {
        block.insert("toolset_name".to_string(), json!(namespace));
    }
    merge_extra(block, extra_body);
    if namespace.is_none() {
        block.remove("toolset_name");
    }
}
