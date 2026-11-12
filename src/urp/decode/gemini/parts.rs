//! Gemini Part decoding shared by request, response, and stream decoding (PG5c). Every Part
//! decodes to exactly one node.

use crate::urp::decode::{parse_compatible_media_node, split_extra};
use crate::urp::media::{is_audio_mime, mime_essence, resource_for_url};
use crate::urp::{
    AudioSource, FileSource, GEMINI_FUNCTION_RESPONSE_EXTRA_KEY, GEMINI_PART_EXTRA_KEY,
    ImageSource, MediaMetadata, Node, OrdinaryRole, ProviderProtocol, ToolCallType,
    ToolResultContent, synthetic_provider_item_id,
};
use serde_json::{Map, Value, json};
use std::collections::HashMap;

/// Prefix of the canonical call id synthesized for a `functionCall` without `id` (PG4d). The
/// encoder never sends such an id back.
pub(crate) const SYNTHETIC_CALL_ID_PREFIX: &str = "call_gemini_";

/// Part members that make a signed Part more than a standalone signature.
const DATA_PART_KEYS: [&str; 7] = [
    "functionCall",
    "inlineData",
    "fileData",
    "executableCode",
    "codeExecutionResult",
    "toolCall",
    "toolResponse",
];

/// The Parts of a `parts` member; Gemini also accepts a single Part in its place.
pub(crate) fn content_parts(value: &Value) -> &[Value] {
    value
        .as_array()
        .map(Vec::as_slice)
        .unwrap_or_else(|| std::slice::from_ref(value))
}

pub(crate) fn node_extra(node: &Node) -> &HashMap<String, Value> {
    match node {
        Node::Text { extra_body, .. }
        | Node::Image { extra_body, .. }
        | Node::Audio { extra_body, .. }
        | Node::File { extra_body, .. }
        | Node::Refusal { extra_body, .. }
        | Node::Reasoning { extra_body, .. }
        | Node::ToolCall { extra_body, .. }
        | Node::ProviderItem { extra_body, .. }
        | Node::ToolResult { extra_body, .. }
        | Node::NextDownstreamEnvelopeExtra { extra_body } => extra_body,
    }
}

/// Decodes one Part as a node with `role`. A `functionResponse` Part becomes a ToolResult.
pub(crate) fn decode_part(part: &Value, role: OrdinaryRole) -> Result<Node, String> {
    match part {
        Value::String(text) => Ok(Node::text(role, text.clone())),
        Value::Object(obj) => match obj.get("functionResponse").and_then(Value::as_object) {
            Some(response) => decode_function_response(obj, response),
            None => decode_object_part(obj, role),
        },
        _ => Ok(Node::ProviderItem {
            id: Some(synthetic_provider_item_id()),
            origin_protocol: ProviderProtocol::Gemini,
            role,
            item_type: "unknown_part".into(),
            body: part.clone(),
            extra_body: HashMap::new(),
        }),
    }
}

fn decode_object_part(obj: &Map<String, Value>, role: OrdinaryRole) -> Result<Node, String> {
    let signature = obj.get("thoughtSignature");
    if let Some(text) = obj.get("text").and_then(Value::as_str) {
        return Ok(
            if obj.get("thought").and_then(Value::as_bool) == Some(true) {
                Node::Reasoning {
                    metadata: Default::default(),
                    id: None,
                    content: None,
                    encrypted: signature.cloned(),
                    summary: Some(text.to_string()),
                    source: None,
                    extra_body: part_extra(obj, &["text", "thought", "thoughtSignature"]),
                }
            } else {
                Node::Text {
                    logprobs: None,
                    citations: Vec::new(),
                    signature: signature.cloned(),
                    id: None,
                    role,
                    content: text.to_string(),
                    phase: None,
                    cache_breakpoint: None,
                    extra_body: part_extra(obj, &["text", "thoughtSignature"]),
                }
            },
        );
    }
    if let Some(call) = obj.get("functionCall").and_then(Value::as_object)
        && let Some(name) = call
            .get("name")
            .and_then(Value::as_str)
            .filter(|name| !name.is_empty())
    {
        return Ok(decode_function_call(obj, call, name));
    }
    if let Some(data) = obj.get("inlineData") {
        return decode_inline_data(obj, data, role);
    }
    if let Some(data) = obj.get("fileData") {
        return decode_file_data(obj, data, role);
    }
    if let Some(mut media) = parse_compatible_media_node(obj, role)? {
        if let Some(signature) = signature
            && let Node::Image {
                metadata,
                extra_body,
                ..
            }
            | Node::Audio {
                metadata,
                extra_body,
                ..
            }
            | Node::File {
                metadata,
                extra_body,
                ..
            } = &mut media
        {
            metadata.signature = Some(signature.clone());
            extra_body.remove("thoughtSignature");
        }
        return Ok(media);
    }
    if signature.is_some() && !DATA_PART_KEYS.iter().any(|key| obj.contains_key(*key)) {
        return Ok(Node::Reasoning {
            metadata: Default::default(),
            id: None,
            content: None,
            encrypted: signature.cloned(),
            summary: None,
            source: None,
            extra_body: part_extra(obj, &["thoughtSignature"]),
        });
    }
    Ok(Node::ProviderItem {
        id: obj
            .get("id")
            .and_then(Value::as_str)
            .map(str::to_string)
            .or_else(|| Some(synthetic_provider_item_id())),
        origin_protocol: ProviderProtocol::Gemini,
        role,
        item_type: obj
            .get("type")
            .and_then(Value::as_str)
            .or_else(|| {
                [
                    "executableCode",
                    "codeExecutionResult",
                    "toolCall",
                    "toolResponse",
                    "thoughtSignature",
                ]
                .into_iter()
                .find(|key| obj.contains_key(*key))
            })
            .unwrap_or("unknown_part")
            .to_string(),
        body: Value::Object(obj.clone()),
        extra_body: HashMap::new(),
    })
}

fn decode_function_call(obj: &Map<String, Value>, call: &Map<String, Value>, name: &str) -> Node {
    let id = call.get("id").and_then(Value::as_str);
    let call_id = id
        .filter(|id| !id.is_empty())
        .map(str::to_string)
        .unwrap_or_else(|| {
            format!(
                "{SYNTHETIC_CALL_ID_PREFIX}{}",
                uuid::Uuid::new_v4().simple()
            )
        });
    let mut extra_body = part_extra(obj, &["functionCall", "thoughtSignature"]);
    preserve_nested_extra(
        call,
        "functionCall",
        &["id", "name", "args"],
        &mut extra_body,
    );
    Node::ToolCall {
        namespace: None,
        signature: obj.get("thoughtSignature").cloned(),
        id: id.map(str::to_string),
        tool_type: ToolCallType::Function,
        call_id,
        name: name.to_string(),
        arguments: serde_json::to_string(call.get("args").unwrap_or(&json!({})))
            .unwrap_or_default(),
        cache_breakpoint: None,
        extra_body,
    }
}

fn decode_inline_data(
    obj: &Map<String, Value>,
    value: &Value,
    role: OrdinaryRole,
) -> Result<Node, String> {
    let data = value
        .as_object()
        .ok_or("Gemini inlineData must be an object.")?;
    let mime = data
        .get("mimeType")
        .and_then(Value::as_str)
        .filter(|mime| !mime.is_empty())
        .ok_or("Gemini inlineData.mimeType must be a non-empty string.")?
        .to_string();
    let bytes = data
        .get("data")
        .and_then(Value::as_str)
        .ok_or("Gemini inlineData.data must contain Base64 bytes as a string.")?
        .to_string();
    let mut extra_body = part_extra(obj, &["inlineData", "thoughtSignature"]);
    preserve_nested_extra(data, "inlineData", &["mimeType", "data"], &mut extra_body);
    let metadata = MediaMetadata {
        signature: obj.get("thoughtSignature").cloned(),
        ..Default::default()
    };
    Ok(if mime_essence(&mime).starts_with("image/") {
        Node::Image {
            metadata,
            id: None,
            role,
            source: ImageSource::Base64 {
                media_type: mime,
                data: bytes,
            },
            extra_body,
        }
    } else if is_audio_mime(&mime) {
        Node::Audio {
            metadata,
            id: None,
            role,
            source: AudioSource::Base64 {
                media_type: mime,
                data: bytes,
            },
            extra_body,
        }
    } else {
        Node::File {
            metadata,
            id: None,
            role,
            source: FileSource::Base64 {
                media_type: mime,
                data: bytes,
            },
            extra_body,
        }
    })
}

fn decode_file_data(
    obj: &Map<String, Value>,
    value: &Value,
    role: OrdinaryRole,
) -> Result<Node, String> {
    let data = value
        .as_object()
        .ok_or("Gemini fileData must be an object.")?;
    let url = data
        .get("fileUri")
        .and_then(Value::as_str)
        .filter(|url| !url.is_empty())
        .ok_or("Gemini fileData.fileUri must be a non-empty string.")?
        .to_string();
    let mime = data
        .get("mimeType")
        .filter(|value| !value.is_null())
        .map(|value| {
            value
                .as_str()
                .filter(|mime| !mime.is_empty())
                .ok_or("Gemini fileData.mimeType must be a non-empty string when present.")
        })
        .transpose()?;
    let mut extra_body = part_extra(obj, &["fileData", "thoughtSignature"]);
    preserve_nested_extra(data, "fileData", &["mimeType", "fileUri"], &mut extra_body);
    let metadata = MediaMetadata {
        signature: obj.get("thoughtSignature").cloned(),
        resource: resource_for_url(&url),
        media_type: mime.map(str::to_string),
        ..Default::default()
    };
    Ok(
        if mime.is_some_and(|mime| mime_essence(mime).starts_with("image/")) {
            Node::Image {
                metadata,
                id: None,
                role,
                source: ImageSource::Url { url, detail: None },
                extra_body,
            }
        } else if mime.is_some_and(is_audio_mime) {
            Node::Audio {
                metadata,
                id: None,
                role,
                source: AudioSource::Url { url },
                extra_body,
            }
        } else {
            Node::File {
                metadata,
                id: None,
                role,
                source: FileSource::Url { url },
                extra_body,
            }
        },
    )
}

fn decode_function_response(
    parent: &Map<String, Value>,
    response: &Map<String, Value>,
) -> Result<Node, String> {
    let name = response
        .get("name")
        .and_then(Value::as_str)
        .unwrap_or_default();
    let id = response.get("id").and_then(Value::as_str);
    let response_value = response.get("response").cloned().unwrap_or(Value::Null);
    let mut content = vec![ToolResultContent::Text {
        text: serde_json::to_string(&response_value).unwrap_or_default(),
        extra_body: HashMap::new(),
        cache_breakpoint: None,
    }];
    for part in response.get("parts").map(content_parts).unwrap_or_default() {
        content.push(result_content(decode_part(part, OrdinaryRole::Assistant)?)?);
    }
    let mut extra_body = split_extra(response, &["id", "name", "response", "parts"]);
    extra_body.extend(part_extra(
        parent,
        &["functionResponse", "thoughtSignature"],
    ));
    extra_body.insert(GEMINI_FUNCTION_RESPONSE_EXTRA_KEY.into(), Value::Bool(true));
    Ok(Node::ToolResult {
        signature: parent.get("thoughtSignature").cloned(),
        namespace: None,
        name: Some(name.to_string()),
        id: id.map(str::to_string),
        tool_type: ToolCallType::Function,
        call_id: id.unwrap_or(name).to_string(),
        is_error: response_value.get("error").is_some(),
        content,
        cache_breakpoint: None,
        extra_body,
    })
}

/// A nested `functionResponse.parts[]` node as result content. `FunctionResponseBlob` carries
/// audio as a file.
fn result_content(node: Node) -> Result<ToolResultContent, String> {
    Ok(match node {
        Node::Text {
            content,
            extra_body,
            ..
        } => ToolResultContent::Text {
            text: content,
            extra_body,
            cache_breakpoint: None,
        },
        Node::Image {
            source,
            metadata,
            extra_body,
            ..
        } => ToolResultContent::Image {
            source,
            metadata,
            extra_body,
        },
        Node::File {
            source,
            metadata,
            extra_body,
            ..
        } => ToolResultContent::File {
            source,
            metadata,
            extra_body,
        },
        Node::Audio {
            source,
            metadata,
            extra_body,
            ..
        } => ToolResultContent::File {
            source: match source {
                AudioSource::Base64 { media_type, data } => FileSource::Base64 { media_type, data },
                AudioSource::Url { url } => FileSource::Url { url },
            },
            metadata,
            extra_body,
        },
        Node::ProviderItem {
            origin_protocol,
            item_type,
            body,
            extra_body,
            ..
        } => ToolResultContent::ProviderItem {
            origin_protocol,
            item_type,
            body,
            extra_body,
        },
        _ => {
            return Err("Gemini functionResponse.parts contains unsupported nested tool or reasoning content.".into());
        }
    })
}

/// Unknown Part members, kept under [`GEMINI_PART_EXTRA_KEY`].
fn part_extra(obj: &Map<String, Value>, known: &[&str]) -> HashMap<String, Value> {
    let native = split_extra(obj, known);
    let mut extra = HashMap::new();
    if !native.is_empty() {
        extra.insert(
            GEMINI_PART_EXTRA_KEY.to_string(),
            Value::Object(native.into_iter().collect()),
        );
    }
    extra
}

/// Keeps the unknown members of the nested Part object `key` inside the Part extras.
fn preserve_nested_extra(
    data: &Map<String, Value>,
    key: &str,
    known: &[&str],
    extra: &mut HashMap<String, Value>,
) {
    let unknown = split_extra(data, known);
    if !unknown.is_empty() {
        let part = extra
            .entry(GEMINI_PART_EXTRA_KEY.into())
            .or_insert_with(|| json!({}));
        part[key] = Value::Object(unknown.into_iter().collect());
    }
}

/// PG5b: a stream client appends `next` to `current` when both are unsigned plain text, or both
/// unsigned unidentified thought summaries, and neither carries extras or citations.
pub(crate) fn append_fragment(current: &mut Node, next: &mut Node) -> bool {
    if !node_extra(current).is_empty() || !node_extra(next).is_empty() {
        return false;
    }
    match (current, next) {
        (
            Node::Text {
                content,
                logprobs,
                signature: None,
                citations,
                ..
            },
            Node::Text {
                content: fragment,
                logprobs: fragment_scores,
                signature: None,
                citations: next_citations,
                ..
            },
        ) if citations.is_empty() && next_citations.is_empty() => {
            if !content.is_empty() && crate::urp::logprobs::valid(logprobs, content).is_none() {
                *fragment_scores = None;
            }
            crate::urp::logprobs::append(logprobs, fragment_scores);
            content.push_str(fragment);
            true
        }
        (
            Node::Reasoning {
                id: None,
                content: None,
                summary: Some(summary),
                encrypted: None,
                ..
            },
            Node::Reasoning {
                id: None,
                content: None,
                summary: Some(fragment),
                encrypted: None,
                ..
            },
        ) => {
            summary.push_str(fragment);
            true
        }
        _ => false,
    }
}
