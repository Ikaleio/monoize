use crate::urp::decode::{parse_compatible_media_node, split_extra, value_to_text};
use crate::urp::{
    AudioSource, FileSource, Node, OrdinaryRole, ProviderProtocol,
    RESPONSES_CONTENT_PART_SHAPE_KEY, ToolResultContent,
};
use serde_json::{Map, Value};
use std::collections::HashMap;

/// Item and content-part `type` values that decode as one compatible content part.
pub(super) fn is_content_part_type(kind: &str) -> bool {
    matches!(
        kind,
        "input_text"
            | "output_text"
            | "text"
            | "image"
            | "image_url"
            | "input_image"
            | "output_image"
            | "file"
            | "document"
            | "input_file"
            | "output_file"
            | "audio"
            | "input_audio"
            | "output_audio"
    )
}

/// A message `phase` applies to every text part; without one, a part's own `phase` member does.
fn text_node(
    content: &str,
    role: OrdinaryRole,
    message_phase: Option<&str>,
    mut extra_body: HashMap<String, Value>,
) -> Node {
    let part_phase = extra_body.remove("phase");
    let phase = match message_phase {
        Some(phase) => Some(phase.to_string()),
        None => part_phase.and_then(|value| value.as_str().map(str::to_string)),
    };
    let citations = extra_body
        .remove("annotations")
        .and_then(|value| value.as_array().cloned())
        .unwrap_or_default();
    Node::Text {
        logprobs: crate::urp::logprobs::decode(extra_body.remove("logprobs").as_ref()),
        signature: None,
        citations: crate::urp::citations::decode(citations, ProviderProtocol::Responses),
        id: None,
        role,
        phase,
        content: content.to_string(),
        extra_body,
        cache_breakpoint: None,
    }
}

/// Decodes one object content part. A part no URP node models becomes a `ProviderItem` marked
/// with its native content-part shape so encoders return it inside a message.
pub(super) fn content_object_node(
    obj: &Map<String, Value>,
    role: OrdinaryRole,
    message_phase: Option<&str>,
) -> Node {
    let kind = obj.get("type").and_then(Value::as_str).unwrap_or("");
    if matches!(kind, "text" | "input_text" | "output_text")
        && let Some(text) = obj
            .get("text")
            .or_else(|| obj.get("content"))
            .and_then(Value::as_str)
    {
        return text_node(
            text,
            role,
            message_phase,
            split_extra(obj, &["type", "text", "content"]),
        );
    }
    if kind == "refusal"
        && let Some(text) = obj.get("refusal").and_then(Value::as_str)
    {
        return Node::Refusal {
            logprobs: None,
            id: None,
            content: text.to_string(),
            extra_body: split_extra(obj, &["type", "refusal"]),
        };
    }
    if let Ok(Some(node)) = parse_compatible_media_node(obj, role) {
        return node;
    }
    Node::ProviderItem {
        id: obj.get("id").and_then(Value::as_str).map(str::to_owned),
        origin_protocol: ProviderProtocol::Responses,
        role,
        item_type: kind.to_string(),
        body: Value::Object(obj.clone()),
        extra_body: HashMap::from([(
            RESPONSES_CONTENT_PART_SHAPE_KEY.to_string(),
            Value::Bool(true),
        )]),
    }
}

/// Decodes message `content` (a string, one part, or a part array) into ordinary nodes.
pub(super) fn push_content_nodes(
    out: &mut Vec<Node>,
    content: &Value,
    role: OrdinaryRole,
    message_phase: Option<&str>,
) {
    let values = match content {
        Value::Null => return,
        Value::Array(values) => values.as_slice(),
        _ => std::slice::from_ref(content),
    };
    for value in values {
        match value {
            Value::String(text) => out.push(text_node(text, role, message_phase, HashMap::new())),
            Value::Object(obj) => out.push(content_object_node(obj, role, message_phase)),
            _ => {}
        }
    }
}

pub(super) fn decode_tool_result_content(output: &Value) -> Vec<ToolResultContent> {
    let mut content = Vec::new();
    match output {
        Value::Array(items) => {
            for item in items {
                push_tool_result_part(&mut content, item);
            }
        }
        other => push_tool_result_part(&mut content, other),
    }
    content
}

fn push_tool_result_text(content: &mut Vec<ToolResultContent>, text: String) {
    if !text.is_empty() {
        content.push(ToolResultContent::Text {
            text,
            extra_body: HashMap::new(),
            cache_breakpoint: None,
        });
    }
}

fn push_tool_result_part(content: &mut Vec<ToolResultContent>, value: &Value) {
    let obj = match value {
        Value::String(text) => return push_tool_result_text(content, text.clone()),
        Value::Object(obj) => obj,
        other => return push_tool_result_text(content, value_to_text(other)),
    };

    let kind = obj.get("type").and_then(Value::as_str).unwrap_or("");
    if matches!(kind, "input_text" | "output_text" | "text") {
        if let Some(text) = obj
            .get("text")
            .and_then(Value::as_str)
            .or_else(|| obj.get("content").and_then(Value::as_str))
        {
            content.push(ToolResultContent::Text {
                text: text.to_string(),
                extra_body: split_extra(obj, &["type", "text", "content"]),
                cache_breakpoint: None,
            });
        }
        return;
    }

    // Tool results carry no role; the node role is discarded below.
    let media = match parse_compatible_media_node(obj, OrdinaryRole::User) {
        Ok(Some(Node::Image {
            metadata,
            source,
            extra_body,
            ..
        })) => Some(ToolResultContent::Image {
            metadata,
            source,
            extra_body,
        }),
        Ok(Some(Node::File {
            metadata,
            source,
            extra_body,
            ..
        })) => Some(ToolResultContent::File {
            metadata,
            source,
            extra_body,
        }),
        // ToolResultContent has no audio variant; audio keeps its bytes as a file.
        Ok(Some(Node::Audio {
            metadata,
            source,
            extra_body,
            ..
        })) => Some(ToolResultContent::File {
            metadata,
            source: match source {
                AudioSource::Base64 { media_type, data } => FileSource::Base64 { media_type, data },
                AudioSource::Url { url } => FileSource::Url { url },
            },
            extra_body,
        }),
        _ => None,
    };
    content.push(media.unwrap_or_else(|| ToolResultContent::ProviderItem {
        origin_protocol: ProviderProtocol::Responses,
        item_type: kind.to_string(),
        body: value.clone(),
        extra_body: HashMap::new(),
    }));
}
