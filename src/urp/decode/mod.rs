pub mod anthropic;
pub mod gemini;
pub mod openai_chat;
pub mod openai_image;
pub mod openai_responses;
pub mod replicate;

use crate::urp::internal_legacy_bridge::Part;
use crate::urp::{
    AudioSource, CustomToolDefinition, FileSource, FunctionDefinition, ImageSource, MediaMetadata,
    MediaResource, Node, OrdinaryRole, ProviderProtocol, ToolDefinition,
};
use serde::{Deserialize, Deserializer};
use serde_json::{Map, Value};
use std::collections::HashMap;

pub fn is_internal_extra_key(key: &str) -> bool {
    key.starts_with("_monoize_")
}

pub fn retain_wire_extra_fields(extra: &mut HashMap<String, Value>) {
    extra.retain(|key, _| !is_internal_extra_key(key));
}

pub fn remove_untrusted_internal_object_keys(value: &mut Value) {
    if let Some(obj) = value.as_object_mut() {
        obj.retain(|key, _| !is_internal_extra_key(key));
    }
}

pub fn split_extra(obj: &Map<String, Value>, known: &[&str]) -> HashMap<String, Value> {
    let mut extra = HashMap::new();
    for (k, v) in obj {
        if !is_internal_extra_key(k) && !known.contains(&k.as_str()) {
            extra.insert(k.clone(), v.clone());
        }
    }
    extra
}

pub fn remove_untrusted_internal_keys(value: &mut Value) {
    match value {
        Value::Object(obj) => {
            obj.retain(|key, _| !key.starts_with("_monoize_"));
            for child in obj.values_mut() {
                remove_untrusted_internal_keys(child);
            }
        }
        Value::Array(values) => {
            for child in values {
                remove_untrusted_internal_keys(child);
            }
        }
        Value::Null | Value::Bool(_) | Value::Number(_) | Value::String(_) => {}
    }
}

pub fn normalize_reasoning_effort(effort: &str) -> String {
    if effort == "minimum" {
        "minimal".to_string()
    } else {
        effort.to_string()
    }
}

pub fn deserialize_u64ish_default<'de, D>(deserializer: D) -> Result<u64, D::Error>
where
    D: Deserializer<'de>,
{
    let value = Option::<Value>::deserialize(deserializer)?;
    Ok(value.and_then(|v| value_to_u64(&v)).unwrap_or(0))
}

pub fn value_to_u64(value: &Value) -> Option<u64> {
    match value {
        Value::Number(n) => n.as_u64(),
        Value::String(s) => s.parse::<u64>().ok(),
        _ => None,
    }
}

fn normalize_tool_parameters(params: Option<Value>) -> Option<Value> {
    let mut v = params?;
    if let Some(obj) = v.as_object_mut() {
        obj.entry("type".to_string())
            .or_insert_with(|| Value::String("object".to_string()));
    }
    Some(v)
}

fn string_field(obj: &Map<String, Value>, key: &str) -> Option<String> {
    obj.get(key).and_then(|v| v.as_str()).map(str::to_string)
}

fn parse_function_definition(function_obj: &Map<String, Value>) -> Option<FunctionDefinition> {
    Some(FunctionDefinition {
        name: string_field(function_obj, "name")?,
        description: string_field(function_obj, "description"),
        parameters: normalize_tool_parameters(
            function_obj
                .get("parameters")
                .cloned()
                .or_else(|| function_obj.get("input_schema").cloned()),
        ),
        strict: function_obj.get("strict").and_then(|v| v.as_bool()),
        extra_body: split_extra(
            function_obj,
            &[
                "name",
                "description",
                "parameters",
                "input_schema",
                "strict",
            ],
        ),
    })
}

fn parse_custom_tool_definition(
    custom_obj: &Map<String, Value>,
    known_fields: &[&str],
) -> Option<CustomToolDefinition> {
    Some(CustomToolDefinition {
        name: string_field(custom_obj, "name")?,
        description: string_field(custom_obj, "description"),
        format: custom_obj.get("format").cloned(),
        extra_body: split_extra(custom_obj, known_fields),
    })
}

fn native_tool_definition(tool_type: String, obj: &Map<String, Value>) -> ToolDefinition {
    ToolDefinition {
        namespace: None,
        tools: None,
        origin_protocol: None,
        config: None,

        tool_type,
        name: string_field(obj, "name"),
        description: string_field(obj, "description"),
        function: None,
        custom: None,
        extra_body: split_extra(obj, &["type", "name", "description"]),
    }
}

pub fn parse_tool_definition(raw: &Value) -> Option<ToolDefinition> {
    let obj = raw.as_object()?;
    let explicit_tool_type = obj.get("type").and_then(|v| v.as_str());
    let tool_type = explicit_tool_type.unwrap_or("function").to_string();

    if tool_type == "function" {
        let function_obj = obj.get("function").and_then(|v| v.as_object());
        if let Some(function_obj) = function_obj {
            return Some(ToolDefinition {
                namespace: string_field(obj, "namespace"),
                tools: None,
                origin_protocol: None,
                config: None,

                tool_type,
                name: None,
                description: None,
                function: Some(parse_function_definition(function_obj)?),
                custom: None,
                extra_body: split_extra(obj, &["type", "function", "namespace"]),
            });
        }

        let mut function = parse_function_definition(obj)?;
        function.extra_body = HashMap::new();
        return Some(ToolDefinition {
            namespace: string_field(obj, "namespace"),
            tools: None,
            origin_protocol: None,
            config: None,

            tool_type,
            name: None,
            description: None,
            function: Some(function),
            custom: None,
            extra_body: split_extra(
                obj,
                &[
                    "type",
                    "name",
                    "description",
                    "parameters",
                    "input_schema",
                    "strict",
                    "namespace",
                ],
            ),
        });
    }

    if tool_type == "custom" {
        if let Some(custom_obj) = obj.get("custom").and_then(|v| v.as_object()) {
            if let Some(custom) =
                parse_custom_tool_definition(custom_obj, &["name", "description", "format"])
            {
                return Some(ToolDefinition {
                    namespace: string_field(obj, "namespace"),
                    tools: None,
                    origin_protocol: None,
                    config: None,

                    tool_type,
                    name: None,
                    description: None,
                    function: None,
                    custom: Some(custom),
                    extra_body: split_extra(obj, &["type", "custom", "namespace"]),
                });
            }
        }

        if let Some(custom) = parse_custom_tool_definition(
            obj,
            &["type", "name", "description", "format", "namespace"],
        ) {
            return Some(ToolDefinition {
                namespace: string_field(obj, "namespace"),
                tools: None,
                origin_protocol: None,
                config: None,

                tool_type,
                name: None,
                description: None,
                function: None,
                custom: Some(custom),
                extra_body: HashMap::new(),
            });
        }
    }

    explicit_tool_type.map(|_| {
        let mut tool = native_tool_definition(tool_type, obj);
        tool.namespace = string_field(obj, "namespace");
        tool.extra_body.remove("namespace");
        if tool.tool_type == "namespace" {
            tool.tools = obj
                .get("tools")
                .and_then(Value::as_array)
                .map(|tools| tools.iter().filter_map(parse_tool_definition).collect());
            tool.extra_body.remove("tools");
        } else {
            tool.config = Some(Value::Object(
                std::mem::take(&mut tool.extra_body).into_iter().collect(),
            ));
        }
        tool
    })
}

#[cfg(test)]
mod canonical_tool_definition_tests {
    use super::*;

    #[test]
    fn flat_custom_namespace_has_one_owner() {
        let tool = parse_tool_definition(&serde_json::json!({
            "type":"custom", "name":"patch", "namespace":"functions",
            "format":{"type":"text"}, "future":true
        }))
        .unwrap();
        assert_eq!(tool.namespace.as_deref(), Some("functions"));
        assert!(!tool.extra_body.contains_key("namespace"));
        let custom = tool.custom.unwrap();
        assert!(!custom.extra_body.contains_key("namespace"));
        assert_eq!(custom.extra_body.get("future"), Some(&Value::Bool(true)));
    }
}

pub fn parse_tool_call_arguments_value(obj: &Map<String, Value>) -> Option<Value> {
    obj.get("arguments")
        .cloned()
        .or_else(|| obj.get("input").cloned())
        .or_else(|| obj.get("args").cloned())
        .or_else(|| {
            obj.get("function")
                .and_then(|value| value.as_object())
                .and_then(|function| {
                    function
                        .get("arguments")
                        .cloned()
                        .or_else(|| function.get("input").cloned())
                        .or_else(|| function.get("args").cloned())
                })
        })
        .or_else(|| {
            obj.get("custom")
                .and_then(|value| value.as_object())
                .and_then(|custom| custom.get("input").cloned())
        })
}

pub fn parse_tool_call_node_from_obj(obj: &Map<String, Value>) -> Option<Node> {
    let item_type = obj.get("type").and_then(|v| v.as_str())?;
    if !matches!(
        item_type,
        "tool_call" | "function_call" | "tool_use" | "custom_tool_call" | "function" | "custom"
    ) {
        return None;
    }
    let tool_type = if item_type == "custom_tool_call" || obj.contains_key("custom") {
        crate::urp::ToolCallType::Custom
    } else {
        crate::urp::ToolCallType::Function
    };

    let call_id = obj
        .get("call_id")
        .or_else(|| obj.get("id"))
        .and_then(|v| v.as_str())
        .unwrap_or("")
        .to_string();
    let payload_key = if tool_type == crate::urp::ToolCallType::Custom {
        "custom"
    } else {
        "function"
    };
    let name = obj
        .get("name")
        .and_then(|v| v.as_str())
        .or_else(|| {
            obj.get(payload_key)
                .and_then(|value| value.as_object())
                .and_then(|payload| payload.get("name"))
                .and_then(|v| v.as_str())
        })
        .unwrap_or("")
        .to_string();
    let arguments = parse_tool_call_arguments_value(obj)
        .map(|value| {
            value
                .as_str()
                .map(|text| text.to_string())
                .unwrap_or_else(|| {
                    serde_json::to_string(&value).unwrap_or_else(|_| "{}".to_string())
                })
        })
        .unwrap_or_else(|| "{}".to_string());

    if call_id.is_empty() || name.is_empty() {
        return None;
    }

    Some(Node::ToolCall {
        namespace: string_field(obj, "namespace").or_else(|| string_field(obj, "toolset_name")),
        signature: None,

        id: obj
            .get("id")
            .and_then(|v| v.as_str())
            .map(|s| s.to_string()),
        tool_type,
        call_id,
        name,
        arguments,
        extra_body: split_extra(
            obj,
            &[
                "type",
                "call_id",
                "namespace",
                "toolset_name",
                "id",
                "name",
                "arguments",
                "input",
                "args",
                "function",
                "custom",
            ],
        ),
    })
}

pub fn parse_tool_call_part_from_obj(obj: &Map<String, Value>) -> Option<Part> {
    let Node::ToolCall {
        namespace,
        signature,
        id,
        tool_type,
        call_id,
        name,
        arguments,
        extra_body,
        ..
    } = parse_tool_call_node_from_obj(obj)?
    else {
        return None;
    };
    Some(Part::ToolCall {
        namespace,
        signature,

        id,
        tool_type,
        call_id,
        name,
        arguments,
        extra_body,
    })
}

pub fn parse_image_source_from_obj(obj: &Map<String, Value>) -> Option<ImageSource> {
    let source = parse_image_source_raw(obj)?;
    if let ImageSource::Url { url, .. } = &source {
        if let Some((mime, data)) = crate::urp::media::parse_data_url(url) {
            return Some(ImageSource::Base64 {
                media_type: mime.into(),
                data: data.into(),
            });
        }
    }
    Some(source)
}

fn parse_image_source_raw(obj: &Map<String, Value>) -> Option<ImageSource> {
    let t = obj.get("type")?.as_str()?;
    match t {
        "image_url" | "input_image" | "output_image" | "image" => {
            if let Some(file_id) = obj.get("file_id").and_then(|v| v.as_str()) {
                return Some(ImageSource::FileId {
                    file_id: file_id.to_string(),
                    detail: obj
                        .get("detail")
                        .and_then(|v| v.as_str())
                        .map(str::to_string),
                });
            }
            if let Some(url) = obj.get("image_url").and_then(|v| v.as_str()) {
                return Some(ImageSource::Url {
                    url: url.to_string(),
                    detail: obj
                        .get("detail")
                        .and_then(|v| v.as_str())
                        .map(|s| s.to_string()),
                });
            }
            if let Some(url_obj) = obj.get("image_url").and_then(|v| v.as_object()) {
                return Some(ImageSource::Url {
                    url: url_obj.get("url")?.as_str()?.to_string(),
                    detail: url_obj
                        .get("detail")
                        .and_then(|v| v.as_str())
                        .map(|s| s.to_string()),
                });
            }
            if let Some(url) = obj.get("url").and_then(|v| v.as_str()) {
                return Some(ImageSource::Url {
                    url: url.to_string(),
                    detail: obj
                        .get("detail")
                        .and_then(|v| v.as_str())
                        .map(|s| s.to_string()),
                });
            }
            if let Some(data) = obj.get("image_base64").and_then(|v| v.as_str()) {
                return Some(ImageSource::Base64 {
                    media_type: obj
                        .get("media_type")
                        .and_then(|v| v.as_str())
                        .map(str::to_owned)
                        .unwrap_or_else(|| crate::urp::media::infer_mime(data, None)),
                    data: data.to_string(),
                });
            }
            if let Some(src) = obj.get("source").and_then(|v| v.as_object()) {
                if src.get("type").and_then(|v| v.as_str()) == Some("file") {
                    return Some(ImageSource::FileId {
                        file_id: src.get("file_id")?.as_str()?.to_string(),
                        detail: obj
                            .get("detail")
                            .and_then(|v| v.as_str())
                            .map(str::to_string),
                    });
                }
                if src.get("type").and_then(Value::as_str) == Some("url") {
                    let url = src.get("url")?.as_str()?;
                    return Some(ImageSource::Url {
                        url: url.to_string(),
                        detail: obj
                            .get("detail")
                            .and_then(|v| v.as_str())
                            .map(|s| s.to_string()),
                    });
                }
                if src.get("type").and_then(Value::as_str) != Some("base64") {
                    return None;
                }
                return Some(ImageSource::Base64 {
                    media_type: src
                        .get("media_type")
                        .and_then(|v| v.as_str())
                        .map(str::to_owned)
                        .unwrap_or_else(|| {
                            crate::urp::media::infer_mime(
                                src.get("data").and_then(Value::as_str).unwrap_or(""),
                                None,
                            )
                        }),
                    data: src.get("data").and_then(|v| v.as_str())?.to_string(),
                });
            }
            None
        }
        _ => None,
    }
}

pub fn parse_image_node_from_obj(obj: &Map<String, Value>, role: OrdinaryRole) -> Option<Node> {
    let source = parse_image_source_from_obj(obj)?;
    let extra_body = split_extra(
        obj,
        &[
            "type",
            "image_url",
            "detail",
            "url",
            "image_base64",
            "media_type",
            "source",
            "file_id",
        ],
    );
    let metadata = media_metadata(obj, matches!(source, ImageSource::FileId { .. }), false);
    Some(Node::Image {
        metadata,

        id: None,
        role,
        source,
        extra_body,
    })
}

pub fn parse_image_part_from_obj(obj: &Map<String, Value>) -> Option<Part> {
    let source = parse_image_source_from_obj(obj)?;
    let extra_body = split_extra(
        obj,
        &[
            "type",
            "image_url",
            "detail",
            "url",
            "image_base64",
            "media_type",
            "source",
            "file_id",
        ],
    );
    let metadata = media_metadata(obj, matches!(source, ImageSource::FileId { .. }), false);
    Some(Part::Image {
        metadata,
        source,
        extra_body,
    })
}

fn file_base64_source(filename: Option<String>, media_type: &str, data: &str) -> FileSource {
    let (media_type, data) = crate::urp::media::parse_data_url(data)
        .filter(|(mime, _)| mime.parse::<mime::Mime>().is_ok())
        .unwrap_or((media_type, data));
    FileSource::Base64 {
        media_type: if media_type == "application/octet-stream" || media_type.is_empty() {
            crate::urp::media::infer_mime(data, filename.as_deref())
        } else {
            media_type.to_string()
        },
        data: data.to_string(),
    }
}

pub fn parse_file_source_from_obj(obj: &Map<String, Value>) -> Option<FileSource> {
    let t = obj.get("type")?.as_str()?;
    match t {
        "input_file" | "output_file" | "document" | "file" => {
            if let Some(file_id) = obj.get("file_id").and_then(|v| v.as_str()) {
                return Some(FileSource::FileId {
                    file_id: file_id.to_string(),
                });
            }
            if let Some(file) = obj.get("file").and_then(Value::as_object) {
                if let Some(file_id) = file.get("file_id").and_then(Value::as_str) {
                    return Some(FileSource::FileId {
                        file_id: file_id.to_string(),
                    });
                }
                if let Some(data) = file.get("file_data").and_then(Value::as_str) {
                    return Some(file_base64_source(
                        file.get("filename")
                            .and_then(Value::as_str)
                            .map(str::to_string),
                        "application/octet-stream",
                        data,
                    ));
                }
            }
            if let Some(url) = obj.get("url").and_then(|v| v.as_str()) {
                return Some(FileSource::Url {
                    url: url.to_string(),
                });
            }
            if let Some(url) = obj.get("file_url").and_then(|v| v.as_str()) {
                return Some(FileSource::Url {
                    url: url.to_string(),
                });
            }
            if let Some(src) = obj.get("source").and_then(|v| v.as_object()) {
                match src.get("type").and_then(|v| v.as_str()) {
                    Some("file") => {
                        return Some(FileSource::FileId {
                            file_id: src.get("file_id")?.as_str()?.to_string(),
                        });
                    }
                    Some("url") => {
                        return Some(FileSource::Url {
                            url: src.get("url")?.as_str()?.to_string(),
                        });
                    }
                    Some("text") => {
                        return Some(FileSource::Text {
                            text: src
                                .get("data")
                                .or_else(|| src.get("text"))?
                                .as_str()?
                                .to_string(),
                        });
                    }
                    Some("content") => {
                        return Some(FileSource::Content {
                            content: match src.get("content")? {
                                Value::String(text) => {
                                    vec![serde_json::json!({"type":"text", "text":text})]
                                }
                                Value::Array(content) => content.clone(),
                                Value::Object(_) => vec![src.get("content")?.clone()],
                                _ => return None,
                            },
                        });
                    }
                    Some("base64") => {}
                    _ => return None,
                }
                return Some(file_base64_source(
                    src.get("filename")
                        .and_then(|v| v.as_str())
                        .map(|s| s.to_string()),
                    src.get("media_type")
                        .and_then(|v| v.as_str())
                        .unwrap_or("application/octet-stream"),
                    src.get("data").and_then(|v| v.as_str())?,
                ));
            }
            if let Some(data) = obj
                .get("file_data")
                .or_else(|| obj.get("data"))
                .and_then(|v| v.as_str())
            {
                return Some(file_base64_source(
                    obj.get("filename")
                        .and_then(Value::as_str)
                        .map(str::to_string),
                    obj.get("media_type")
                        .and_then(Value::as_str)
                        .unwrap_or("application/octet-stream"),
                    data,
                ));
            }
            None
        }
        _ => None,
    }
}

pub fn parse_file_node_from_obj(obj: &Map<String, Value>, role: OrdinaryRole) -> Option<Node> {
    let source = parse_file_source_from_obj(obj)?;
    let mut extra_body = split_extra(
        obj,
        &[
            "type",
            "url",
            "file_url",
            "source",
            "file_data",
            "data",
            "filename",
            "media_type",
            "file_id",
            "file",
            "detail",
            "title",
            "context",
            "citations",
        ],
    );
    let mut metadata = media_metadata(obj, matches!(source, FileSource::FileId { .. }), true);
    metadata.resource = metadata
        .resource
        .or_else(|| crate::urp::media::document_resource(&source));
    document_shape(obj, &mut extra_body);
    Some(Node::File {
        metadata,

        id: None,
        role,
        source,
        extra_body,
    })
}

pub fn parse_file_part_from_obj(obj: &Map<String, Value>) -> Option<Part> {
    let source = parse_file_source_from_obj(obj)?;
    let mut extra_body = split_extra(
        obj,
        &[
            "type",
            "url",
            "file_url",
            "source",
            "file_data",
            "data",
            "filename",
            "media_type",
            "file_id",
            "file",
            "detail",
            "title",
            "context",
            "citations",
        ],
    );
    let mut metadata = media_metadata(obj, matches!(source, FileSource::FileId { .. }), true);
    metadata.resource = metadata
        .resource
        .or_else(|| crate::urp::media::document_resource(&source));
    document_shape(obj, &mut extra_body);
    Some(Part::File {
        metadata,
        source,
        extra_body,
    })
}

pub fn parse_audio_part_from_obj(obj: &Map<String, Value>) -> Option<Part> {
    if !matches!(
        obj.get("type").and_then(Value::as_str),
        Some("input_audio" | "audio" | "output_audio")
    ) {
        return None;
    }
    let body = obj
        .get("input_audio")
        .or_else(|| obj.get("audio"))
        .and_then(Value::as_object)
        .unwrap_or(obj);
    let native_source = body.get("source").and_then(Value::as_object);
    if native_source.is_some_and(|source| {
        !matches!(
            source.get("type").and_then(Value::as_str),
            Some("base64" | "url")
        )
    }) {
        return None;
    }
    let source_body = native_source.unwrap_or(body);
    let source_kind = native_source
        .and_then(|source| source.get("type"))
        .and_then(Value::as_str);
    let media_type = source_body
        .get("media_type")
        .or_else(|| body.get("media_type"))
        .or_else(|| body.get("audio_url").and_then(|url| url.get("media_type")))
        .and_then(Value::as_str)
        .map(str::to_owned)
        .or_else(|| {
            body.get("format")
                .and_then(Value::as_str)
                .and_then(|format| {
                    Some(
                        match format {
                            "wav" => "audio/wav",
                            "mp3" => "audio/mpeg",
                            "flac" => "audio/flac",
                            "opus" => "audio/opus",
                            "aac" => "audio/aac",
                            "ogg" => "audio/ogg",
                            "m4a" => "audio/mp4",
                            "webm" => "audio/webm",
                            "pcm16" => "audio/pcm",
                            _ => return None,
                        }
                        .to_owned(),
                    )
                })
        });
    let url = source_body
        .get("url")
        .or_else(|| body.get("audio_url").filter(|url| url.is_string()))
        .or_else(|| body.get("audio_url").and_then(|url| url.get("url")))
        .and_then(Value::as_str);
    let source = if let Some(url) = url {
        if source_kind == Some("base64") {
            return None;
        }
        match crate::urp::media::parse_data_url(url) {
            Some((mime, data)) => AudioSource::Base64 {
                media_type: mime.into(),
                data: data.into(),
            },
            None => AudioSource::Url { url: url.into() },
        }
    } else {
        if source_kind == Some("url") {
            return None;
        }
        let data = source_body
            .get("data")
            .or_else(|| body.get("audio_base64"))?
            .as_str()?;
        let (mime, data) = crate::urp::media::parse_data_url(data)
            .map(|(mime, data)| (mime.to_owned(), data))
            .unwrap_or_else(|| {
                (
                    media_type
                        .clone()
                        .unwrap_or_else(|| crate::urp::media::infer_mime(data, None)),
                    data,
                )
            });
        AudioSource::Base64 {
            media_type: mime,
            data: data.into(),
        }
    };
    let resource = match &source {
        AudioSource::Url { url } => crate::urp::media::resource_for_url(url),
        _ => None,
    };
    Some(Part::Audio {
        metadata: MediaMetadata {
            media_type: matches!(source, AudioSource::Url { .. })
                .then_some(media_type)
                .flatten(),
            resource,
            reference_id: body.get("id").and_then(Value::as_str).map(str::to_owned),
            transcript: body
                .get("transcript")
                .and_then(Value::as_str)
                .map(str::to_owned),
            expires_at: body.get("expires_at").and_then(Value::as_i64),
            ..Default::default()
        },
        source,
        extra_body: split_extra(
            obj,
            &[
                "type",
                "input_audio",
                "audio",
                "source",
                "audio_url",
                "url",
                "data",
                "audio_base64",
                "media_type",
                "format",
                "id",
                "transcript",
                "expires_at",
            ],
        ),
    })
}

pub fn parse_audio_node_from_obj(obj: &Map<String, Value>, role: OrdinaryRole) -> Option<Node> {
    let Part::Audio {
        source,
        metadata,
        extra_body,
    } = parse_audio_part_from_obj(obj)?
    else {
        unreachable!()
    };
    Some(Node::Audio {
        id: None,
        role,
        source,
        metadata,
        extra_body,
    })
}

pub fn parse_compatible_media_part(obj: &Map<String, Value>) -> Result<Option<Part>, String> {
    let kind = obj.get("type").and_then(Value::as_str).unwrap_or("");
    let part = match kind {
        "image" | "image_url" | "input_image" | "output_image" => parse_image_part_from_obj(obj),
        "file" | "document" | "input_file" | "output_file" => parse_file_part_from_obj(obj),
        "audio" | "input_audio" | "output_audio" => parse_audio_part_from_obj(obj),
        _ => return Ok(None),
    };
    part.map(Some)
        .ok_or_else(|| format!("Malformed {kind} content block."))
}

fn media_metadata(obj: &Map<String, Value>, file_id: bool, document: bool) -> MediaMetadata {
    let source = obj.get("source").and_then(Value::as_object);
    let file = obj.get("file").and_then(Value::as_object);
    let url = obj
        .get("file_url")
        .or_else(|| obj.get("url"))
        .or_else(|| obj.get("image_url").filter(|v| v.is_string()))
        .or_else(|| obj.get("image_url").and_then(|v| v.get("url")))
        .or_else(|| source.and_then(|src| src.get("url")))
        .and_then(Value::as_str);
    let resource = if file_id {
        Some(MediaResource {
            protocol: if source
                .and_then(|src| src.get("type"))
                .and_then(Value::as_str)
                == Some("file")
            {
                ProviderProtocol::Messages
            } else {
                ProviderProtocol::Responses
            },
            provider_id: None,
            channel_id: None,
            credential_scope: None,
        })
    } else {
        url.and_then(crate::urp::media::resource_for_url)
    };
    let image_url = obj.get("image_url");
    let inline_image = url.is_some_and(|url| url.starts_with("data:"))
        || obj.contains_key("image_base64")
        || source
            .and_then(|src| src.get("type"))
            .and_then(Value::as_str)
            == Some("base64");
    MediaMetadata {
        resource,
        filename: obj
            .get("filename")
            .or_else(|| file.and_then(|f| f.get("filename")))
            .or_else(|| source.and_then(|s| s.get("filename")))
            .and_then(Value::as_str)
            .map(str::to_owned),
        detail: (document || inline_image)
            .then(|| {
                obj.get("detail")
                    .or_else(|| image_url.and_then(|v| v.get("detail")))
                    .and_then(Value::as_str)
                    .map(str::to_owned)
            })
            .flatten(),
        document_title: document
            .then(|| obj.get("title").and_then(Value::as_str).map(str::to_owned))
            .flatten(),
        document_context: document
            .then(|| {
                obj.get("context")
                    .and_then(Value::as_str)
                    .map(str::to_owned)
            })
            .flatten(),
        document_citations: document.then(|| obj.get("citations").cloned()).flatten(),
        media_type: url.filter(|url| !url.starts_with("data:")).and_then(|_| {
            obj.get("media_type")
                .or_else(|| source.and_then(|src| src.get("media_type")))
                .or_else(|| image_url.and_then(|image| image.get("media_type")))
                .and_then(Value::as_str)
                .map(str::to_owned)
                .or_else(|| {
                    (document && obj.get("type").and_then(Value::as_str) == Some("document"))
                        .then(|| "application/pdf".into())
                })
        }),
        ..Default::default()
    }
}

fn document_shape(obj: &Map<String, Value>, extra: &mut HashMap<String, Value>) {
    if obj
        .get("source")
        .and_then(|v| v.get("content"))
        .is_some_and(Value::is_string)
    {
        extra.insert("_monoize_document_content_string".into(), Value::Bool(true));
    }
}

pub fn value_to_text(v: &Value) -> String {
    if let Some(s) = v.as_str() {
        return s.to_string();
    }
    if let Some(arr) = v.as_array() {
        let mut out = String::new();
        for item in arr {
            if let Some(s) = item.as_str() {
                out.push_str(s);
                continue;
            }
            if let Some(obj) = item.as_object()
                && let Some(text) = obj.get("text").and_then(|x| x.as_str())
            {
                out.push_str(text);
            }
        }
        return out;
    }
    serde_json::to_string(v).unwrap_or_default()
}
