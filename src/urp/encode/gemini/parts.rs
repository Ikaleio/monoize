//! Node to Gemini Part encoding (GEM-2, GEM-3, PG4a, PG4d, PG4j, PG5a, PG5d2, PG8).

use crate::urp::decode::gemini::{
    CONTINUATION_ID_PREFIX, ITEM_CARRIER_ID_PREFIX, PART_SIGNATURE_ID_PREFIX,
    SYNTHETIC_CALL_ID_PREFIX, item_carrier_offset, item_carrier_part, node_extra,
    text_carrier_owner,
};
use crate::urp::encode::sanitize_provider_item_wire_body;
use crate::urp::media::{is_audio_mime, mime_essence};
use crate::urp::{
    AudioSource, FileSource, GEMINI_BOUND_ITEM_OFFSET_KEY, GEMINI_BOUND_OWNER_KEY,
    GEMINI_BOUND_SIGNATURE_KEY, GEMINI_CONTENT_EXTRA_KEY, GEMINI_FUNCTION_RESPONSE_EXTRA_KEY,
    GEMINI_PART_EXTRA_KEY, ImageSource, MediaMetadata, Node, OrdinaryRole, ProviderProtocol,
    ToolCallType, ToolResultContent, integerize_json_floats, is_internal_key,
};
use serde_json::{Map, Value, json};
use std::collections::HashMap;

/// Encodes one node as a Part with the role of its Content. A node without a Gemini Part
/// (controls, custom tools, foreign provider items, continuation carriers, file ids) yields
/// `None`. A tool result encodes as an assistant Part without call-name correlation; requests
/// use [`encode_tool_result`] directly. Bound signature carriers and server-tool carriers yield
/// placeholder Parts that `resolve_bound_signatures` must resolve before the Parts reach the
/// wire.
pub(super) fn encode_node_part(node: &Node) -> Option<(OrdinaryRole, Value)> {
    let (role, mut part) = match node {
        Node::Text {
            signature,
            role,
            content,
            ..
        } => {
            let mut part = json!({"text": content});
            if let Some(signature) = signature {
                part["thoughtSignature"] = signature.clone();
            }
            (*role, part)
        }
        Node::Image { role, source, .. } => (*role, encode_image_source(source)?),
        Node::File { role, source, .. } => (*role, encode_file_source(source)?),
        Node::Audio { role, source, .. } => (*role, encode_audio_source(source)),
        Node::Refusal { content, .. } => (OrdinaryRole::Assistant, json!({"text": content})),
        Node::Reasoning { .. } => (OrdinaryRole::Assistant, encode_reasoning(node)?),
        Node::ToolCall {
            tool_type,
            call_id,
            name,
            arguments,
            ..
        } => {
            if *tool_type == ToolCallType::Custom {
                return None;
            }
            let mut args = serde_json::from_str::<Value>(arguments).unwrap_or_else(|_| json!({}));
            integerize_json_floats(&mut args);
            let mut call = Map::new();
            // PG4d: a synthetic canonical call id never reaches the wire.
            if !call_id.starts_with(SYNTHETIC_CALL_ID_PREFIX) {
                call.insert("id".into(), json!(call_id));
            }
            call.insert("name".into(), json!(name));
            call.insert("args".into(), args);
            (
                OrdinaryRole::Assistant,
                Value::Object(Map::from_iter([(
                    "functionCall".to_string(),
                    Value::Object(call),
                )])),
            )
        }
        Node::ProviderItem {
            id,
            role,
            origin_protocol: ProviderProtocol::Gemini,
            item_type,
            body,
            ..
        } => (*role, encode_provider_part(id.as_deref(), item_type, body)),
        Node::ToolResult { .. } => {
            return Some((
                OrdinaryRole::Assistant,
                encode_tool_result(node, &HashMap::new())?,
            ));
        }
        Node::ProviderItem { .. } | Node::NextDownstreamEnvelopeExtra { .. } => return None,
    };
    apply_typed_members(node, &mut part);
    if let Some(Value::Object(native)) = node_extra(node).get(GEMINI_PART_EXTRA_KEY)
        && let Some(obj) = part.as_object_mut()
    {
        merge_native_part_members(node, native, obj);
    }
    Some((role, part))
}

/// A thought Part, or for a signature-only Reasoning node the Part its carrier id selects:
/// a bound signature placeholder, a restored server-tool Part, or a standalone signed empty
/// text Part. Continuation tokens are request controls (PG5d2), never Parts.
fn encode_reasoning(node: &Node) -> Option<Value> {
    let Node::Reasoning {
        id,
        metadata,
        content,
        encrypted,
        summary,
        ..
    } = node
    else {
        return None;
    };
    // Explicit empty text (RTYPE-NEW-4a) is not reasoning text: without a signature it never
    // becomes a thought Part; a signature keeps its existing carrier shape.
    let text = content
        .as_deref()
        .filter(|text| !text.is_empty())
        .or(summary.as_deref().filter(|text| !text.is_empty()))
        .or_else(|| {
            encrypted
                .is_some()
                .then(|| content.as_deref().or(summary.as_deref()))
                .flatten()
        });
    if let Some(text) = text {
        let mut part = json!({"text": text, "thought": true});
        if let Some(signature) = encrypted {
            part["thoughtSignature"] = signature.clone();
        }
        return Some(part);
    }
    let payload = encrypted.as_ref()?;
    let carrier_id = id.as_deref().or(metadata.item_id.as_deref());
    let carrier = |prefix: &str| carrier_id.is_some_and(|id| id.starts_with(prefix));
    if carrier(CONTINUATION_ID_PREFIX) {
        return None;
    }
    if carrier(ITEM_CARRIER_ID_PREFIX) {
        let mut part = item_carrier_part(payload)?;
        if let Some(offset) = carrier_id.and_then(item_carrier_offset) {
            part[GEMINI_BOUND_ITEM_OFFSET_KEY] = json!(offset);
        }
        return Some(part);
    }
    if carrier(PART_SIGNATURE_ID_PREFIX) {
        let mut part = json!({ GEMINI_BOUND_SIGNATURE_KEY: payload });
        if let Some((length, hash, offset)) = carrier_id.and_then(text_carrier_owner) {
            part[GEMINI_BOUND_OWNER_KEY] = json!([length, hash, offset]);
        }
        return Some(part);
    }
    // A Part needs a data member; Gemini streams standalone signatures this way.
    Some(json!({"text": "", "thoughtSignature": payload}))
}

/// A Gemini ProviderItem replays its native body; typed identity and type win over the body.
fn encode_provider_part(id: Option<&str>, item_type: &str, body: &Value) -> Value {
    let mut wire = sanitize_provider_item_wire_body(body);
    if let Some(obj) = wire.as_object_mut() {
        if obj.remove("id").is_some()
            && let Some(id) = id
        {
            obj.insert("id".into(), json!(id));
        }
        if obj.remove("type").is_some() && !item_type.is_empty() {
            obj.insert("type".into(), json!(item_type));
        }
    }
    wire
}

/// Typed signature, MIME, and filename members of call and media Parts.
fn apply_typed_members(node: &Node, part: &mut Value) {
    match node {
        Node::ToolCall {
            signature: Some(signature),
            ..
        } => part["thoughtSignature"] = signature.clone(),
        Node::Image { metadata, .. }
        | Node::Audio { metadata, .. }
        | Node::File { metadata, .. } => {
            apply_media_metadata(metadata, part);
        }
        _ => {}
    }
}

fn apply_media_metadata(metadata: &MediaMetadata, part: &mut Value) {
    if let Some(signature) = &metadata.signature {
        part["thoughtSignature"] = signature.clone();
    }
    if let Some(data) = part.get_mut("fileData").and_then(Value::as_object_mut) {
        data.remove("mimeType");
        if let Some(mime) = &metadata.media_type {
            data.insert("mimeType".into(), json!(mime));
        }
    }
    if let Some(filename) = &metadata.filename {
        let key = if part.get("inlineData").is_some() {
            "inlineData"
        } else {
            "fileData"
        };
        if let Some(data) = part.get_mut(key).and_then(Value::as_object_mut) {
            data.insert("displayName".into(), json!(filename));
        }
    }
}

/// Restores unknown native Part members. Typed members win: payload, signature, and thought
/// members never come from extras, and video metadata survives only on a video source.
fn merge_native_part_members(
    node: &Node,
    native: &Map<String, Value>,
    obj: &mut Map<String, Value>,
) {
    for (key, value) in native {
        if matches!(key.as_str(), "videoMetadata" | "mediaProcessing") && !has_video_source(node) {
            continue;
        }
        if matches!(
            key.as_str(),
            "text" | "thoughtSignature" | "functionResponse"
        ) || (key == "thought" && matches!(node, Node::Text { .. } | Node::Reasoning { .. }))
        {
            continue;
        }
        if matches!(key.as_str(), "inlineData" | "fileData" | "functionCall") {
            if let (Some(target), Some(unknown)) = (
                obj.get_mut(key).and_then(Value::as_object_mut),
                value.as_object(),
            ) {
                for (field, value) in unknown {
                    if !matches!(
                        field.as_str(),
                        "mimeType" | "data" | "fileUri" | "id" | "name" | "args"
                    ) {
                        target.entry(field.clone()).or_insert_with(|| value.clone());
                    }
                }
            }
        } else {
            obj.entry(key.clone()).or_insert_with(|| value.clone());
        }
    }
}

fn has_video_source(node: &Node) -> bool {
    let (source_mime, metadata) = match node {
        Node::Image {
            source, metadata, ..
        } => (
            match source {
                ImageSource::Base64 { media_type, .. } => Some(media_type.as_str()),
                _ => None,
            },
            metadata,
        ),
        Node::Audio {
            source, metadata, ..
        } => (
            match source {
                AudioSource::Base64 { media_type, .. } => Some(media_type.as_str()),
                _ => None,
            },
            metadata,
        ),
        Node::File {
            source, metadata, ..
        } => (
            match source {
                FileSource::Base64 { media_type, .. } => Some(media_type.as_str()),
                _ => None,
            },
            metadata,
        ),
        _ => return false,
    };
    source_mime
        .or(metadata.media_type.as_deref())
        .is_some_and(|mime| {
            let mime = mime_essence(mime);
            mime.starts_with("video/") && !is_audio_mime(&mime) && mime != "video/text/timestamp"
        })
}

fn encode_image_source(source: &ImageSource) -> Option<Value> {
    match source {
        ImageSource::Url { url, .. } => Some(json!({"fileData": {"fileUri": url}})),
        ImageSource::Base64 { media_type, data } => {
            Some(json!({"inlineData": {"mimeType": media_type, "data": data}}))
        }
        ImageSource::FileId { .. } => None,
    }
}

fn encode_file_source(source: &FileSource) -> Option<Value> {
    match source {
        FileSource::Url { url } => Some(json!({"fileData": {"fileUri": url}})),
        FileSource::Base64 { media_type, data } => {
            Some(json!({"inlineData": {"mimeType": media_type, "data": data}}))
        }
        FileSource::FileId { .. } | FileSource::Text { .. } | FileSource::Content { .. } => None,
    }
}

fn encode_audio_source(source: &AudioSource) -> Value {
    match source {
        AudioSource::Url { url } => json!({"fileData": {"fileUri": url}}),
        AudioSource::Base64 { media_type, data } => {
            json!({"inlineData": {"mimeType": media_type, "data": data}})
        }
    }
}

/// Native Content members a Gemini-native decode retained on the first node of a Content.
pub(crate) fn content_extra(node: &Node) -> HashMap<String, Value> {
    node_extra(node)
        .get(GEMINI_CONTENT_EXTRA_KEY)
        .and_then(Value::as_object)
        .map(|extra| {
            extra
                .iter()
                .map(|(key, value)| (key.clone(), value.clone()))
                .collect()
        })
        .unwrap_or_default()
}

/// PG4a: a `functionResponse` Part. The function name is the typed name, else the name of the
/// correlated call in `call_names`, else the call id.
pub(super) fn encode_tool_result(node: &Node, call_names: &HashMap<&str, &str>) -> Option<Value> {
    let Node::ToolResult {
        signature,
        name,
        id,
        tool_type,
        call_id,
        content,
        is_error,
        extra_body,
        ..
    } = node
    else {
        return None;
    };
    if *tool_type == ToolCallType::Custom {
        return None;
    }
    let result: String = content
        .iter()
        .filter_map(|entry| match entry {
            ToolResultContent::Text { text, .. } => Some(text.as_str()),
            _ => None,
        })
        .collect();
    let function_name = name
        .as_deref()
        .or_else(|| call_names.get(call_id.as_str()).copied())
        .unwrap_or(call_id);
    let native_response = extra_body
        .get(GEMINI_FUNCTION_RESPONSE_EXTRA_KEY)
        .and_then(Value::as_bool)
        == Some(true);
    // Same-Gemini replay keeps the native response object (PG4d).
    let native_object = native_response
        .then(|| serde_json::from_str::<Value>(&result).ok())
        .flatten()
        .filter(Value::is_object);
    let mut response = native_object.unwrap_or_else(|| {
        // FunctionResponse.response documents `output` for results and `error` for failures.
        let key = if *is_error && !native_response {
            "error"
        } else {
            "output"
        };
        Value::Object(Map::from_iter([(key.to_string(), Value::String(result))]))
    });
    if *is_error && response.get("error").is_none() {
        response = Value::Object(Map::from_iter([("error".to_string(), response)]));
    } else if !*is_error && let Some(obj) = response.as_object_mut() {
        obj.remove("error");
    }
    let mut function_response = Map::new();
    function_response.insert("name".into(), json!(function_name));
    function_response.insert("response".into(), response);
    for (key, value) in extra_body {
        if !is_internal_key(key)
            && !function_response.contains_key(key)
            && !matches!(key.as_str(), "name" | "id" | "response" | "parts")
        {
            function_response.insert(key.clone(), value.clone());
        }
    }
    let media: Vec<Value> = content.iter().filter_map(encode_result_media).collect();
    if !media.is_empty() {
        function_response.insert("parts".into(), Value::Array(media));
    }
    if !call_id.is_empty()
        && !call_id.starts_with(SYNTHETIC_CALL_ID_PREFIX)
        && !(id.is_none() && name.as_deref() == Some(call_id.as_str()))
    {
        function_response.insert("id".into(), json!(call_id));
    }
    let mut part = Map::new();
    part.insert("functionResponse".into(), Value::Object(function_response));
    if let Some(signature) = signature {
        part.insert("thoughtSignature".into(), signature.clone());
    }
    if let Some(native) = extra_body
        .get(GEMINI_PART_EXTRA_KEY)
        .and_then(Value::as_object)
    {
        for (key, value) in native {
            if !matches!(key.as_str(), "functionResponse" | "thoughtSignature") {
                part.entry(key.clone()).or_insert_with(|| value.clone());
            }
        }
    }
    Some(Value::Object(part))
}

/// A `FunctionResponsePart`: inline media only; URL and file id sources were rejected earlier.
fn encode_result_media(content: &ToolResultContent) -> Option<Value> {
    let (media_type, data, extra_body) = match content {
        ToolResultContent::Image {
            source: ImageSource::Base64 { media_type, data },
            extra_body,
            ..
        }
        | ToolResultContent::File {
            source: FileSource::Base64 { media_type, data },
            extra_body,
            ..
        } => (media_type, data, extra_body),
        ToolResultContent::ProviderItem {
            origin_protocol: ProviderProtocol::Gemini,
            body,
            ..
        } => return Some(sanitize_provider_item_wire_body(body)),
        _ => return None,
    };
    let mut inline = Map::new();
    inline.insert("mimeType".into(), json!(media_type));
    inline.insert("data".into(), json!(data));
    if let Some(unknown) = extra_body
        .get(GEMINI_PART_EXTRA_KEY)
        .and_then(|native| native.get("inlineData"))
        .and_then(Value::as_object)
    {
        for (key, value) in unknown {
            if !matches!(key.as_str(), "mimeType" | "data") {
                inline.entry(key.clone()).or_insert_with(|| value.clone());
            }
        }
    }
    Some(Value::Object(Map::from_iter([(
        "inlineData".to_string(),
        Value::Object(inline),
    )])))
}
