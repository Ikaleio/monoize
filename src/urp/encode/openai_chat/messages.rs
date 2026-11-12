//! PC2: URP input nodes to the Chat `messages[]` array of an upstream request.

use super::reasoning::{ReasoningTarget, insert_reasoning_fields};
use super::{
    ChatDialect, chat_provider_item_wire_body, encode_chat_provider_part, encode_chat_tool_call,
    encode_legacy_function_call, flag_set, is_legacy_function_call, merge_chat_wire_extra,
};
use crate::urp::encode::ordinary_role_to_str;
use crate::urp::grouping::{Envelope, envelopes};
use crate::urp::{
    AudioSource, CHAT_LEGACY_FUNCTION_RESULT_EXTRA_KEY, CHAT_MESSAGE_AUDIO_EXTRA_KEY,
    CHAT_MESSAGE_ITEM_EXTRA_KEY, FileSource, ImageSource, MediaMetadata, Node, OrdinaryRole,
    ProviderProtocol, ToolResultContent, is_internal_key,
};
use serde_json::{Map, Value, json};
use std::collections::HashMap;

pub(super) fn encode_messages(nodes: &[Node], dialect: ChatDialect) -> Vec<Value> {
    let mut out = Vec::new();
    for envelope in envelopes(nodes) {
        match envelope {
            Envelope::Message {
                role, nodes, extra, ..
            } => encode_message_envelope(role, nodes, &extra, dialect, &mut out),
            Envelope::ToolResult { node, extra } => {
                if let Some(message) = encode_tool_result_message(node, &extra) {
                    out.push(message);
                }
            }
        }
    }
    out
}

/// One envelope normally yields one message. A Chat-native message item splits it: the item is
/// replayed verbatim between the messages built from the nodes before and after it.
fn encode_message_envelope<'a>(
    role: OrdinaryRole,
    nodes: &'a [Node],
    extra: &HashMap<String, Value>,
    dialect: ChatDialect,
    out: &mut Vec<Value>,
) {
    let mut pending: Option<PendingChatMessage<'a>> = None;
    for node in nodes {
        if let Node::ProviderItem {
            id,
            item_type,
            origin_protocol: ProviderProtocol::ChatCompletion,
            body,
            extra_body,
            ..
        } = node
            && flag_set(extra_body, CHAT_MESSAGE_ITEM_EXTRA_KEY)
        {
            if let Some(message) = pending.take() {
                message.flush(dialect, out);
            }
            out.push(chat_provider_item_wire_body(
                id.as_deref(),
                item_type,
                body,
                extra_body,
            ));
            continue;
        }
        pending
            .get_or_insert_with(|| PendingChatMessage::new(role, extra.clone()))
            .push(node);
    }
    if let Some(message) = pending {
        message.flush(dialect, out);
    }
}

struct PendingChatMessage<'a> {
    role: OrdinaryRole,
    content_parts: Vec<Value>,
    tool_calls: Vec<Value>,
    refusal: Option<&'a str>,
    reasoning: Vec<&'a Node>,
    legacy_function_call: Option<Value>,
    message_extra: HashMap<String, Value>,
}

impl<'a> PendingChatMessage<'a> {
    fn new(role: OrdinaryRole, message_extra: HashMap<String, Value>) -> Self {
        Self {
            role,
            content_parts: Vec::new(),
            tool_calls: Vec::new(),
            refusal: None,
            reasoning: Vec::new(),
            legacy_function_call: None,
            message_extra,
        }
    }

    fn push(&mut self, node: &'a Node) {
        match node {
            Node::Audio {
                metadata,
                extra_body,
                ..
            } if flag_set(extra_body, CHAT_MESSAGE_AUDIO_EXTRA_KEY) => {
                if let Some(id) = &metadata.reference_id {
                    self.message_extra
                        .insert("audio".into(), json!({ "id": id }));
                }
            }
            Node::ProviderItem {
                id,
                item_type,
                origin_protocol: ProviderProtocol::ChatCompletion,
                body,
                extra_body,
                ..
            } if flag_set(extra_body, CHAT_MESSAGE_AUDIO_EXTRA_KEY) => {
                self.message_extra.insert(
                    "audio".to_string(),
                    chat_provider_item_wire_body(id.as_deref(), item_type, body, extra_body),
                );
            }
            Node::Text {
                content,
                citations,
                phase,
                extra_body,
                ..
            } => {
                if !citations.is_empty() {
                    self.message_extra
                        .entry("annotations".into())
                        .or_insert_with(|| json!([]))
                        .as_array_mut()
                        .expect("canonical annotations array")
                        .extend(crate::urp::citations::encode(
                            citations,
                            ProviderProtocol::ChatCompletion,
                            0,
                        ));
                }
                self.content_parts
                    .push(encode_text_part(content, phase.as_deref(), extra_body));
            }
            Node::Image {
                metadata,
                source,
                extra_body,
                ..
            } => self.push_content(encode_image_part(metadata, source, extra_body)),
            Node::File {
                metadata,
                source,
                extra_body,
                ..
            } => self.push_content(encode_file_part(metadata, source, extra_body)),
            Node::Audio {
                source, extra_body, ..
            } => self.push_content(encode_audio_part(source, extra_body)),
            Node::ProviderItem {
                id,
                item_type,
                origin_protocol,
                body,
                extra_body,
                ..
            } => self.push_content(encode_chat_provider_part(
                *origin_protocol,
                id.as_deref(),
                item_type,
                body,
                extra_body,
            )),
            Node::Refusal { content, .. } => self.refusal = Some(content.as_str()),
            Node::Reasoning { .. } => self.reasoning.push(node),
            Node::ToolCall {
                tool_type,
                call_id,
                name,
                arguments,
                extra_body,
                ..
            } => {
                if is_legacy_function_call(*tool_type, extra_body) {
                    self.legacy_function_call =
                        Some(encode_legacy_function_call(name, arguments, extra_body));
                } else {
                    self.tool_calls
                        .push(encode_chat_tool_call(*tool_type, call_id, name, arguments));
                }
            }
            Node::ToolResult { .. } | Node::NextDownstreamEnvelopeExtra { .. } => {}
        }
    }

    fn push_content(&mut self, part: Option<Value>) {
        if let Some(part) = part {
            self.content_parts.push(part);
        }
    }

    fn flush(self, dialect: ChatDialect, out: &mut Vec<Value>) {
        let mut reasoning_fields = Map::new();
        insert_reasoning_fields(
            &mut reasoning_fields,
            &self.reasoning,
            ReasoningTarget::Request { dialect },
        );
        if self.content_parts.is_empty()
            && self.tool_calls.is_empty()
            && self.refusal.is_none()
            && reasoning_fields.is_empty()
            && self.legacy_function_call.is_none()
            && self.message_extra.is_empty()
        {
            return;
        }

        let mut m = Map::new();
        m.insert(
            "role".to_string(),
            Value::String(ordinary_role_to_str(self.role).to_string()),
        );
        m.insert("content".to_string(), message_content(self.content_parts));
        if let Some(refusal) = self.refusal {
            m.insert("refusal".to_string(), Value::String(refusal.to_string()));
        }
        if !self.tool_calls.is_empty() {
            m.insert("tool_calls".to_string(), Value::Array(self.tool_calls));
        }
        if let Some(function_call) = self.legacy_function_call {
            m.insert("function_call".to_string(), function_call);
        }
        m.extend(reasoning_fields);
        merge_chat_wire_extra(&mut m, &self.message_extra);
        out.push(Value::Object(m));
    }
}

/// PC2.2: a single text part without extra members collapses to scalar `content`.
fn message_content(mut content_parts: Vec<Value>) -> Value {
    let collapsible = content_parts.len() == 1
        && content_parts[0].as_object().is_some_and(|part| {
            part.len() == 2
                && part.get("type").and_then(Value::as_str) == Some("text")
                && part.get("text").is_some_and(Value::is_string)
        });
    if collapsible {
        content_parts[0]["text"].take()
    } else if content_parts.is_empty() {
        Value::String(String::new())
    } else {
        Value::Array(content_parts)
    }
}

fn encode_text_part(
    content: &str,
    phase: Option<&str>,
    extra_body: &HashMap<String, Value>,
) -> Value {
    let mut block = Map::from_iter([
        ("type".to_string(), json!("text")),
        ("text".to_string(), json!(content)),
    ]);
    // The typed `phase` is the only owner of that member; a stale extra copy never reaches the wire.
    if let Some(phase) = phase {
        block.insert("phase".to_string(), json!(phase));
    }
    for (key, value) in extra_body {
        if key != "phase" && !is_internal_key(key) && !block.contains_key(key) {
            block.insert(key.clone(), value.clone());
        }
    }
    Value::Object(block)
}

fn encode_image_part(
    metadata: &MediaMetadata,
    source: &ImageSource,
    extra_body: &HashMap<String, Value>,
) -> Option<Value> {
    let (url, detail) = match source {
        ImageSource::Url { url, detail } => (url.clone(), detail.as_ref()),
        ImageSource::Base64 { media_type, data } => (
            format!("data:{media_type};base64,{data}"),
            metadata.detail.as_ref(),
        ),
        ImageSource::FileId { .. } => return None,
    };
    let mut image_url = Map::from_iter([("url".to_string(), Value::String(url))]);
    if let Some(detail) = detail {
        image_url.insert("detail".into(), json!(detail));
    }
    let mut block = Map::from_iter([
        ("type".to_string(), json!("image_url")),
        ("image_url".to_string(), Value::Object(image_url)),
    ]);
    merge_chat_wire_extra(&mut block, extra_body);
    for key in ["detail", "filename", "media_type", "source"] {
        block.remove(key);
    }
    Some(Value::Object(block))
}

fn encode_file_part(
    metadata: &MediaMetadata,
    source: &FileSource,
    extra_body: &HashMap<String, Value>,
) -> Option<Value> {
    let mut file = match source {
        FileSource::FileId { file_id }
            if crate::urp::media::resource_matches(metadata, ProviderProtocol::ChatCompletion) =>
        {
            json!({ "file_id": file_id })
        }
        FileSource::Base64 { media_type, data } => {
            json!({ "file_data": format!("data:{media_type};base64,{data}") })
        }
        FileSource::Url { .. }
        | FileSource::FileId { .. }
        | FileSource::Text { .. }
        | FileSource::Content { .. } => return None,
    };
    if let Some(filename) = &metadata.filename {
        file["filename"] = json!(filename);
    }
    let mut block = Map::new();
    merge_chat_wire_extra(&mut block, extra_body);
    for key in [
        "filename",
        "detail",
        "media_type",
        "source",
        "file_url",
        "url",
        "file_id",
        "file_data",
    ] {
        block.remove(key);
    }
    block.insert("type".into(), json!("file"));
    block.insert("file".into(), file);
    Some(Value::Object(block))
}

fn encode_audio_part(source: &AudioSource, extra_body: &HashMap<String, Value>) -> Option<Value> {
    let AudioSource::Base64 { media_type, data } = source else {
        return None;
    };
    let format = match media_type.as_str() {
        "audio/wav" | "audio/x-wav" => "wav",
        "audio/mpeg" | "audio/mp3" => "mp3",
        _ => return None,
    };
    let mut block = Map::from_iter([
        ("type".to_string(), json!("input_audio")),
        (
            "input_audio".to_string(),
            json!({ "data": data, "format": format }),
        ),
    ]);
    merge_chat_wire_extra(&mut block, extra_body);
    Some(Value::Object(block))
}

/// A `role = "tool"` message, or `role = "function"` for a deprecated function result (PC2.9).
fn encode_tool_result_message(node: &Node, extra: &HashMap<String, Value>) -> Option<Value> {
    let Node::ToolResult {
        id,
        call_id,
        name,
        content,
        is_error,
        extra_body,
        ..
    } = node
    else {
        return None;
    };
    let content = encode_tool_result_content(content, *is_error);
    let mut m = Map::new();
    if extra_body.contains_key(CHAT_LEGACY_FUNCTION_RESULT_EXTRA_KEY)
        || extra.contains_key(CHAT_LEGACY_FUNCTION_RESULT_EXTRA_KEY)
    {
        m.insert("role".to_string(), Value::String("function".to_string()));
        m.insert("content".to_string(), content);
    } else {
        m.insert("role".to_string(), Value::String("tool".to_string()));
        m.insert("content".to_string(), content);
        m.insert("tool_call_id".to_string(), Value::String(call_id.clone()));
    }
    // The node's own extras win over those of preceding control nodes.
    merge_chat_wire_extra(&mut m, extra_body);
    merge_chat_wire_extra(&mut m, extra);
    m.remove("id");
    m.remove("name");
    if let Some(id) = id {
        m.insert("id".to_string(), Value::String(id.clone()));
    }
    if let Some(name) = name {
        m.insert("name".to_string(), Value::String(name.clone()));
    }
    Some(Value::Object(m))
}

/// PC2.10: Chat tool messages accept a string or an array of text parts and have no error flag.
fn encode_tool_result_content(content: &[ToolResultContent], is_error: bool) -> Value {
    let mut entries = content
        .iter()
        .filter_map(|content| match content {
            ToolResultContent::Text {
                text, extra_body, ..
            } => Some((text.clone(), extra_body)),
            _ => None,
        })
        .collect::<Vec<_>>();
    let no_extra = HashMap::new();
    if is_error {
        match entries.first_mut() {
            Some((first, _)) => first.insert_str(0, "Error: "),
            None => entries.push(("Error".to_string(), &no_extra)),
        }
    }
    if entries.is_empty() {
        return Value::String(String::new());
    }
    let mut parts = entries
        .into_iter()
        .map(|(text, extra_body)| {
            let mut part = Map::from_iter([
                ("type".to_string(), json!("text")),
                ("text".to_string(), Value::String(text)),
            ]);
            merge_chat_wire_extra(&mut part, extra_body);
            part
        })
        .collect::<Vec<_>>();
    if parts.len() == 1 && parts[0].len() == 2 {
        return parts.remove(0).remove("text").unwrap_or_default();
    }
    Value::Array(parts.into_iter().map(Value::Object).collect())
}
