use crate::urp::decode::{
    parse_audio_node_from_obj, parse_file_node_from_obj, parse_image_node_from_obj, split_extra,
    value_to_text,
};
use crate::urp::{
    AudioSource, FileSource, Node, OrdinaryRole, ProviderProtocol, ReasoningMetadata, ToolCallType,
    ToolResultContent, unwrap_reasoning_signature_sigil,
};
use serde_json::{Map, Value};
use std::collections::HashMap;

pub(super) fn decode_content_nodes(
    content: &Value,
    role: OrdinaryRole,
) -> Result<Vec<Node>, String> {
    if let Some(blocks) = content.as_array() {
        let mut nodes = Vec::new();
        for block in blocks {
            nodes.extend(decode_content_nodes(block, role)?);
        }
        return Ok(nodes);
    }
    Ok(decode_content_block(content, role)?.into_iter().collect())
}

pub(crate) fn decode_content_block(
    block: &Value,
    role: OrdinaryRole,
) -> Result<Option<Node>, String> {
    if let Some(text) = block.as_str() {
        return Ok(Some(Node::text(role, text)));
    }
    let Some(obj) = block.as_object() else {
        return Ok(None);
    };
    let kind = obj
        .get("type")
        .and_then(Value::as_str)
        .unwrap_or(if role == OrdinaryRole::System {
            "text"
        } else {
            ""
        });
    let node = match kind {
        "text" | "input_text" | "output_text" => obj
            .get("text")
            .and_then(Value::as_str)
            .map(|text| decode_text_block(obj, text, role)),
        "thinking" => decode_thinking_block(obj),
        "redacted_thinking" => decode_redacted_thinking_block(obj),
        "image" | "image_url" | "input_image" | "output_image" => Some(
            parse_image_node_from_obj(obj, role)
                .ok_or("Messages image source is unsupported or malformed")?,
        ),
        "document" | "file" | "input_file" | "output_file" => Some(
            parse_file_node_from_obj(obj, role)
                .ok_or("Messages document source is unsupported or malformed")?,
        ),
        "audio" | "input_audio" | "output_audio" => Some(
            parse_audio_node_from_obj(obj, role)
                .ok_or("Messages audio source is unsupported or malformed")?,
        ),
        "tool_use" => Some(Node::ToolCall {
            namespace: string_member(obj, "toolset_name"),
            signature: None,
            id: string_member(obj, "id"),
            tool_type: ToolCallType::Function,
            call_id: string_member(obj, "id").unwrap_or_default(),
            name: string_member(obj, "name").unwrap_or_default(),
            arguments: obj.get("input").unwrap_or(&Value::Null).to_string(),
            extra_body: split_extra(obj, &["type", "id", "name", "input", "toolset_name"]),
            cache_breakpoint: None,
        }),
        "tool_result" => Some(Node::ToolResult {
            signature: None,
            namespace: string_member(obj, "toolset_name"),
            name: None,
            id: None,
            tool_type: ToolCallType::Function,
            call_id: string_member(obj, "tool_use_id").unwrap_or_default(),
            is_error: obj
                .get("is_error")
                .and_then(Value::as_bool)
                .unwrap_or(false),
            content: decode_tool_result_content(obj.get("content"))?,
            extra_body: split_extra(
                obj,
                &["type", "tool_use_id", "is_error", "content", "toolset_name"],
            ),
            cache_breakpoint: None,
        }),
        _ => Some(provider_item_from_block(obj, role)),
    };
    Ok(node)
}

fn string_member(obj: &Map<String, Value>, key: &str) -> Option<String> {
    obj.get(key).and_then(Value::as_str).map(str::to_string)
}

fn decode_text_block(obj: &Map<String, Value>, text: &str, role: OrdinaryRole) -> Node {
    let mut extra_body = split_extra(obj, &["type", "text", "phase"]);
    let citations = extra_body
        .remove("citations")
        .and_then(|value| value.as_array().cloned())
        .unwrap_or_default();
    Node::Text {
        logprobs: None,
        signature: None,
        citations: crate::urp::citations::decode(citations, ProviderProtocol::Messages),
        id: None,
        role,
        content: text.to_string(),
        phase: string_member(obj, "phase"),
        extra_body,
        cache_breakpoint: None,
    }
}

fn decode_thinking_block(obj: &Map<String, Value>) -> Option<Node> {
    let thinking = obj
        .get("thinking")
        .and_then(Value::as_str)
        .filter(|thinking| !thinking.is_empty())
        .map(str::to_string);
    let raw_signature = obj
        .get("signature")
        .and_then(Value::as_str)
        .filter(|signature| !signature.is_empty());
    if thinking.is_none() && raw_signature.is_none() {
        return None;
    }
    let (id, encrypted) = match raw_signature {
        Some(signature) => match unwrap_reasoning_signature_sigil(signature) {
            Some((id, original)) => (Some(id), Some(Value::String(original))),
            None => (None, Some(Value::String(signature.to_string()))),
        },
        None => (None, None),
    };
    Some(Node::Reasoning {
        metadata: ReasoningMetadata {
            downstream_only: thinking.is_some() && id.is_none() && encrypted.is_none(),
            summary_as_thinking: true,
            ..Default::default()
        },
        id,
        content: None,
        encrypted,
        summary: thinking,
        source: None,
        extra_body: split_extra(obj, &["type", "thinking", "signature"]),
    })
}

fn decode_redacted_thinking_block(obj: &Map<String, Value>) -> Option<Node> {
    let raw_data = obj.get("data").cloned().filter(|value| match value {
        Value::String(data) => !data.is_empty(),
        Value::Null => false,
        _ => true,
    })?;
    let (id, encrypted) = match raw_data.as_str().and_then(unwrap_reasoning_signature_sigil) {
        Some((id, original)) => (Some(id), Value::String(original)),
        None => (None, raw_data),
    };
    Some(Node::Reasoning {
        metadata: ReasoningMetadata {
            redacted: true,
            ..Default::default()
        },
        id,
        content: None,
        encrypted: Some(encrypted),
        summary: None,
        source: None,
        extra_body: split_extra(obj, &["type", "data"]),
    })
}

fn provider_item_from_block(block: &Map<String, Value>, role: OrdinaryRole) -> Node {
    Node::ProviderItem {
        id: string_member(block, "id").or_else(|| Some(crate::urp::synthetic_provider_item_id())),
        origin_protocol: ProviderProtocol::Messages,
        role,
        item_type: string_member(block, "type").unwrap_or_default(),
        body: Value::Object(block.clone()),
        extra_body: HashMap::new(),
    }
}

fn decode_tool_result_content(content: Option<&Value>) -> Result<Vec<ToolResultContent>, String> {
    let mut blocks = Vec::new();
    if let Some(content) = content {
        decode_tool_result_content_block(content, &mut blocks)?;
    }
    Ok(blocks)
}

fn decode_tool_result_content_block(
    block: &Value,
    content: &mut Vec<ToolResultContent>,
) -> Result<(), String> {
    if let Some(blocks) = block.as_array() {
        for block in blocks {
            decode_tool_result_content_block(block, content)?;
        }
        return Ok(());
    }
    let Some(obj) = block.as_object() else {
        let text = value_to_text(block);
        if !text.is_empty() {
            content.push(ToolResultContent::Text {
                text,
                extra_body: HashMap::new(),
                cache_breakpoint: None,
            });
        }
        return Ok(());
    };
    let kind = obj.get("type").and_then(Value::as_str).unwrap_or("");
    match kind {
        "text" | "input_text" | "output_text" => {
            if let Some(text) = obj.get("text").and_then(Value::as_str) {
                content.push(ToolResultContent::Text {
                    text: text.to_string(),
                    extra_body: split_extra(obj, &["type", "text"]),
                    cache_breakpoint: None,
                });
            }
        }
        "image" | "image_url" | "input_image" | "output_image" => {
            let Some(Node::Image {
                source,
                metadata,
                extra_body,
                ..
            }) = parse_image_node_from_obj(obj, OrdinaryRole::User)
            else {
                return Err("Messages tool-result image source is unsupported or malformed".into());
            };
            content.push(ToolResultContent::Image {
                source,
                metadata,
                extra_body,
            });
        }
        "document" | "file" | "input_file" | "output_file" => {
            let Some(Node::File {
                source,
                metadata,
                extra_body,
                ..
            }) = parse_file_node_from_obj(obj, OrdinaryRole::User)
            else {
                return Err(
                    "Messages tool-result document source is unsupported or malformed".into(),
                );
            };
            content.push(ToolResultContent::File {
                source,
                metadata,
                extra_body,
            });
        }
        "audio" | "input_audio" | "output_audio" => {
            let Some(Node::Audio {
                source,
                metadata,
                extra_body,
                ..
            }) = parse_audio_node_from_obj(obj, OrdinaryRole::User)
            else {
                return Err("Messages tool-result audio source is unsupported or malformed".into());
            };
            let source = match source {
                AudioSource::Base64 { media_type, data } => FileSource::Base64 { media_type, data },
                AudioSource::Url { url } => FileSource::Url { url },
            };
            content.push(ToolResultContent::File {
                source,
                metadata,
                extra_body,
            });
        }
        _ => content.push(ToolResultContent::ProviderItem {
            origin_protocol: ProviderProtocol::Messages,
            item_type: kind.to_string(),
            body: block.clone(),
            extra_body: HashMap::new(),
        }),
    }
    Ok(())
}
