use crate::urp::decode::{parse_compatible_media_node, parse_tool_call_node_from_obj, split_extra};
use crate::urp::{
    AudioSource, CHAT_LEGACY_FUNCTION_CALL_EXTRA_KEY, CHAT_MESSAGE_AUDIO_EXTRA_KEY, MediaMetadata,
    Node, OrdinaryRole, ProviderProtocol, ToolCallType,
};
use serde_json::{Map, Value};
use std::collections::HashMap;

/// Message members decoded into nodes; every other member is envelope passthrough.
pub(super) const MESSAGE_BODY_FIELDS: &[&str] = &[
    "role",
    "content",
    "tool_calls",
    "reasoning",
    "reasoning_details",
    "annotations",
    "reasoning_content",
    "reasoning_opaque",
    "refusal",
    "function_call",
    "audio",
    "phase",
];

/// Appends one decoded message: its envelope extras ride a control node in front of the content.
pub(super) fn push_message(
    out: &mut Vec<Node>,
    nodes: Vec<Node>,
    extra_body: HashMap<String, Value>,
) {
    if !nodes.is_empty() && !extra_body.is_empty() {
        out.push(Node::NextDownstreamEnvelopeExtra { extra_body });
    }
    out.extend(nodes);
}

pub(super) fn legacy_function_call_id(name: &str) -> String {
    format!("legacy_function:{name}")
}

/// Decodes the deprecated assistant `function_call` member as a function ToolCall.
pub(super) fn legacy_function_call_node(message: &Map<String, Value>) -> Option<Node> {
    let function_call = message.get("function_call")?.as_object()?;
    let name = function_call.get("name")?.as_str()?.to_string();
    let arguments = function_call
        .get("arguments")
        .map(|arguments| {
            arguments
                .as_str()
                .map(str::to_string)
                .unwrap_or_else(|| arguments.to_string())
        })
        .unwrap_or_default();
    let mut extra_body = split_extra(function_call, &["name", "arguments"]);
    extra_body.insert(
        CHAT_LEGACY_FUNCTION_CALL_EXTRA_KEY.to_string(),
        Value::Bool(true),
    );
    Some(Node::ToolCall {
        namespace: None,
        signature: None,
        id: None,
        tool_type: ToolCallType::Function,
        call_id: legacy_function_call_id(&name),
        name,
        arguments,
        cache_breakpoint: None,
        extra_body,
    })
}

pub(super) fn tool_call_nodes(message: &Map<String, Value>) -> impl Iterator<Item = Node> + '_ {
    message
        .get("tool_calls")
        .and_then(Value::as_array)
        .into_iter()
        .flatten()
        .filter_map(Value::as_object)
        .filter_map(parse_tool_call_node_from_obj)
}

/// Decodes an assistant message `audio` member.
pub(crate) fn parse_chat_message_audio_node(value: &Value) -> Option<Node> {
    chat_message_audio_node(value, OrdinaryRole::Assistant)
}

pub(super) fn message_audio_node(message: &Map<String, Value>, role: OrdinaryRole) -> Option<Node> {
    message
        .get("audio")
        .filter(|audio| !audio.is_null())
        .and_then(|audio| chat_message_audio_node(audio, role))
}

/// Audio with inline `data` is an Audio node; a reference-only body (`{id}`) is kept verbatim.
fn chat_message_audio_node(value: &Value, role: OrdinaryRole) -> Option<Node> {
    let body = value.as_object()?;
    if body.is_empty() {
        return None;
    }
    let shape = HashMap::from([(CHAT_MESSAGE_AUDIO_EXTRA_KEY.to_string(), Value::Bool(true))]);
    let id = body.get("id").and_then(Value::as_str).map(str::to_string);
    if let Some(data) = body.get("data").and_then(Value::as_str) {
        let mut extra_body = split_extra(body, &["id", "data", "transcript", "expires_at"]);
        extra_body.extend(shape);
        return Some(Node::Audio {
            metadata: MediaMetadata {
                reference_id: id,
                transcript: body
                    .get("transcript")
                    .and_then(Value::as_str)
                    .map(str::to_string),
                expires_at: body.get("expires_at").and_then(Value::as_i64),
                ..Default::default()
            },
            id: None,
            role,
            source: AudioSource::Base64 {
                media_type: "audio/unknown".into(),
                data: data.into(),
            },
            extra_body,
        });
    }
    Some(Node::ProviderItem {
        id,
        origin_protocol: ProviderProtocol::ChatCompletion,
        role,
        item_type: "audio".to_string(),
        body: Value::Object(body.clone()),
        extra_body: shape,
    })
}

pub(super) fn refusal_node(message: &Map<String, Value>) -> Option<Node> {
    let refusal = message
        .get("refusal")
        .and_then(Value::as_str)
        .filter(|refusal| !refusal.is_empty())?;
    Some(Node::Refusal {
        logprobs: None,
        id: None,
        content: refusal.to_string(),
        extra_body: HashMap::new(),
    })
}

/// Message-level `annotations` cite the first text node of the message.
pub(super) fn attach_annotations(nodes: &mut [Node], message: &Map<String, Value>) {
    if let Some(annotations) = message.get("annotations").and_then(Value::as_array)
        && let Some(Node::Text { citations, .. }) = nodes
            .iter_mut()
            .find(|node| matches!(node, Node::Text { .. }))
    {
        citations.extend(crate::urp::citations::decode(
            annotations.clone(),
            ProviderProtocol::ChatCompletion,
        ));
    }
}

/// A message-level `phase` overrides the part's own `phase` member.
fn text_node(
    content: &str,
    role: OrdinaryRole,
    message_phase: Option<&str>,
    mut extra_body: HashMap<String, Value>,
) -> Node {
    let part_phase = extra_body.remove("phase");
    let phase = match message_phase {
        Some(phase) => Some(phase.to_string()),
        None => part_phase.and_then(|phase| phase.as_str().map(str::to_string)),
    };
    Node::Text {
        logprobs: None,
        citations: Vec::new(),
        signature: None,
        id: None,
        role,
        content: content.to_string(),
        phase,
        cache_breakpoint: None,
        extra_body,
    }
}

/// Decodes message `content` (string, part array, or a single part object).
pub(super) fn push_content_nodes(
    nodes: &mut Vec<Node>,
    content: &Value,
    role: OrdinaryRole,
    message_phase: Option<&str>,
) -> Result<(), String> {
    let items = match content {
        Value::String(text) => {
            if !text.is_empty() {
                nodes.push(text_node(text, role, message_phase, HashMap::new()));
            }
            return Ok(());
        }
        Value::Array(items) => items.as_slice(),
        Value::Object(_) => std::slice::from_ref(content),
        _ => return Ok(()),
    };

    for item in items {
        if let Some(text) = item.as_str() {
            if !text.is_empty() {
                nodes.push(text_node(text, role, message_phase, HashMap::new()));
            }
            continue;
        }
        let Some(item_obj) = item.as_object() else {
            continue;
        };
        let mut recognized = false;
        if let Some(text) = item_obj.get("text").and_then(Value::as_str) {
            let item_type = item_obj.get("type").and_then(Value::as_str);
            if !text.is_empty() && matches!(item_type, Some("input_text" | "text" | "output_text"))
            {
                nodes.push(text_node(
                    text,
                    role,
                    message_phase,
                    split_extra(item_obj, &["type", "text"]),
                ));
                recognized = true;
            }
        }
        if let Some(media) = parse_compatible_media_node(item_obj, role)? {
            nodes.push(media);
            recognized = true;
        }
        if let Some(tool_call) = parse_tool_call_node_from_obj(item_obj) {
            nodes.push(tool_call);
            recognized = true;
        }
        if !recognized {
            nodes.push(Node::ProviderItem {
                id: item_obj
                    .get("id")
                    .and_then(Value::as_str)
                    .map(str::to_string)
                    .or_else(|| Some(crate::urp::synthetic_provider_item_id())),
                origin_protocol: ProviderProtocol::ChatCompletion,
                role,
                item_type: item_obj
                    .get("type")
                    .and_then(Value::as_str)
                    .unwrap_or("")
                    .to_string(),
                body: Value::Object(item_obj.clone()),
                extra_body: HashMap::new(),
            });
        }
    }
    Ok(())
}
