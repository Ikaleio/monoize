pub mod anthropic;
pub mod gemini;
pub mod openai_chat;
pub mod openai_image;
pub mod openai_responses;
pub mod replicate;

use crate::urp::internal_legacy_bridge::Part;
use crate::urp::{
    AudioSource, CustomToolDefinition, FILE_ID_ORIGIN_EXTRA_KEY, FILE_ID_ORIGIN_MESSAGES,
    FILE_ID_ORIGIN_OPENAI, FileSource, FunctionDefinition, ImageSource, Node, OrdinaryRole,
    ToolDefinition,
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
                tool_type,
                name: None,
                description: None,
                function: Some(parse_function_definition(function_obj)?),
                custom: None,
                extra_body: split_extra(obj, &["type", "function"]),
            });
        }

        let mut function = parse_function_definition(obj)?;
        function.extra_body = HashMap::new();
        return Some(ToolDefinition {
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
                    tool_type,
                    name: None,
                    description: None,
                    function: None,
                    custom: Some(custom),
                    extra_body: split_extra(obj, &["type", "custom"]),
                });
            }
        }

        if let Some(custom) =
            parse_custom_tool_definition(obj, &["type", "name", "description", "format"])
        {
            return Some(ToolDefinition {
                tool_type,
                name: None,
                description: None,
                function: None,
                custom: Some(custom),
                extra_body: HashMap::new(),
            });
        }
    }

    explicit_tool_type.map(|_| native_tool_definition(tool_type, obj))
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
        id,
        tool_type,
        call_id,
        name,
        arguments,
        extra_body,
    } = parse_tool_call_node_from_obj(obj)?
    else {
        return None;
    };
    Some(Part::ToolCall {
        id,
        tool_type,
        call_id,
        name,
        arguments,
        extra_body,
    })
}

pub fn parse_image_source_from_obj(obj: &Map<String, Value>) -> Option<ImageSource> {
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
                        .unwrap_or("image/png")
                        .to_string(),
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
                if let Some(url) = src.get("url").and_then(|v| v.as_str()) {
                    return Some(ImageSource::Url {
                        url: url.to_string(),
                        detail: obj
                            .get("detail")
                            .and_then(|v| v.as_str())
                            .map(|s| s.to_string()),
                    });
                }
                return Some(ImageSource::Base64 {
                    media_type: src
                        .get("media_type")
                        .and_then(|v| v.as_str())
                        .unwrap_or("image/png")
                        .to_string(),
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
    let mut extra_body = split_extra(
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
    mark_file_id_origin(&source, obj, &mut extra_body);
    Some(Node::Image {
        id: None,
        role,
        source,
        extra_body,
    })
}

pub fn parse_image_part_from_obj(obj: &Map<String, Value>) -> Option<Part> {
    let source = parse_image_source_from_obj(obj)?;
    let mut extra_body = split_extra(
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
    mark_file_id_origin(&source, obj, &mut extra_body);
    Some(Part::Image { source, extra_body })
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
                    return Some(FileSource::Base64 {
                        filename: file
                            .get("filename")
                            .and_then(Value::as_str)
                            .map(str::to_string),
                        media_type: "application/octet-stream".to_string(),
                        data: data.to_string(),
                    });
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
                            content: src.get("content")?.as_array()?.clone(),
                        });
                    }
                    Some("base64") => {}
                    _ => return None,
                }
                return Some(FileSource::Base64 {
                    filename: src
                        .get("filename")
                        .and_then(|v| v.as_str())
                        .map(|s| s.to_string()),
                    media_type: src
                        .get("media_type")
                        .and_then(|v| v.as_str())
                        .unwrap_or("application/octet-stream")
                        .to_string(),
                    data: src.get("data").and_then(|v| v.as_str())?.to_string(),
                });
            }
            if let Some(data) = obj
                .get("file_data")
                .or_else(|| obj.get("data"))
                .and_then(|v| v.as_str())
            {
                return Some(FileSource::Base64 {
                    filename: obj
                        .get("filename")
                        .and_then(|v| v.as_str())
                        .map(|s| s.to_string()),
                    media_type: obj
                        .get("media_type")
                        .and_then(|v| v.as_str())
                        .unwrap_or("application/octet-stream")
                        .to_string(),
                    data: data.to_string(),
                });
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
        ],
    );
    mark_file_id_origin(&source, obj, &mut extra_body);
    Some(Node::File {
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
        ],
    );
    mark_file_id_origin(&source, obj, &mut extra_body);
    Some(Part::File { source, extra_body })
}

pub fn parse_audio_part_from_obj(obj: &Map<String, Value>) -> Option<Part> {
    if obj.get("type").and_then(Value::as_str) != Some("input_audio") {
        return None;
    }
    let input_audio = obj.get("input_audio")?.as_object()?;
    let media_type = match input_audio.get("format")?.as_str()? {
        "wav" => "audio/wav",
        "mp3" => "audio/mpeg",
        _ => return None,
    };
    Some(Part::Audio {
        source: AudioSource::Base64 {
            media_type: media_type.to_string(),
            data: input_audio.get("data")?.as_str()?.to_string(),
        },
        extra_body: split_extra(obj, &["type", "input_audio"]),
    })
}

fn file_id_origin_for_obj(obj: &Map<String, Value>) -> &'static str {
    if obj
        .get("source")
        .and_then(Value::as_object)
        .and_then(|source| source.get("type"))
        .and_then(Value::as_str)
        == Some("file")
    {
        FILE_ID_ORIGIN_MESSAGES
    } else {
        FILE_ID_ORIGIN_OPENAI
    }
}

trait FileIdSource {
    fn is_file_id(&self) -> bool;
}

impl FileIdSource for ImageSource {
    fn is_file_id(&self) -> bool {
        matches!(self, ImageSource::FileId { .. })
    }
}

impl FileIdSource for FileSource {
    fn is_file_id(&self) -> bool {
        matches!(self, FileSource::FileId { .. })
    }
}

fn mark_file_id_origin<T: FileIdSource>(
    source: &T,
    obj: &Map<String, Value>,
    extra_body: &mut HashMap<String, Value>,
) {
    if source.is_file_id() {
        extra_body.insert(
            FILE_ID_ORIGIN_EXTRA_KEY.to_string(),
            Value::String(file_id_origin_for_obj(obj).to_string()),
        );
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

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn split_extra_rejects_wire_spoofing_of_internal_metadata() {
        let object = json!({
            "known": 1,
            "vendor": { "keep": true },
            "_monoize_responses_instructions": [{ "role": "developer", "content": "spoof" }]
        });
        let extra = split_extra(object.as_object().unwrap(), &["known"]);

        assert_eq!(extra.get("vendor"), Some(&json!({ "keep": true })));
        assert!(!extra.contains_key("known"));
        assert!(!extra.contains_key("_monoize_responses_instructions"));
    }

    #[test]
    fn parse_tool_definition_accepts_function_and_custom_shapes() {
        let nested_function = json!({
            "type": "function",
            "function": {
                "name": "lookup",
                "description": "Lookup a value",
                "parameters": {
                    "properties": {
                        "id": { "type": "string" }
                    },
                    "required": ["id"]
                },
                "input_schema": {
                    "properties": {
                        "ignored": { "type": "boolean" }
                    }
                },
                "strict": true,
                "defer_loading": true
            },
            "x_tool": "outer"
        });
        let tool = parse_tool_definition(&nested_function).expect("nested function tool");
        assert_eq!(tool.tool_type, "function");
        assert_eq!(tool.name, None);
        assert_eq!(tool.description, None);
        assert!(tool.custom.is_none());
        assert_eq!(tool.extra_body.get("x_tool"), Some(&json!("outer")));

        let function = tool.function.as_ref().expect("function IR");
        assert_eq!(function.name, "lookup");
        assert_eq!(function.description.as_deref(), Some("Lookup a value"));
        assert_eq!(function.strict, Some(true));
        let parameters = function.parameters.as_ref().expect("parameters");
        assert_eq!(parameters["type"], json!("object"));
        assert_eq!(parameters["properties"]["id"]["type"], json!("string"));
        assert!(parameters["properties"].get("ignored").is_none());
        assert_eq!(function.extra_body.get("defer_loading"), Some(&json!(true)));

        let missing_type_function = json!({
            "name": "from_input_schema",
            "description": "Uses Anthropic schema alias",
            "input_schema": {
                "properties": {
                    "query": { "type": "string" }
                }
            },
            "strict": false
        });
        let tool = parse_tool_definition(&missing_type_function).expect("missing type function");
        assert_eq!(tool.tool_type, "function");
        let function = tool.function.as_ref().expect("function IR");
        assert_eq!(function.name, "from_input_schema");
        assert_eq!(function.strict, Some(false));
        assert_eq!(
            function.parameters.as_ref(),
            Some(&json!({
                "type": "object",
                "properties": {
                    "query": { "type": "string" }
                }
            }))
        );

        let non_object_schema = json!({
            "name": "schema_reference",
            "input_schema": "schema://opaque"
        });
        let tool = parse_tool_definition(&non_object_schema).expect("non-object schema function");
        let function = tool.function.as_ref().expect("function IR");
        assert_eq!(
            function.parameters.as_ref(),
            Some(&json!("schema://opaque"))
        );

        let nested_custom = json!({
            "type": "custom",
            "custom": {
                "name": "freeform_nested",
                "description": "Nested custom tool",
                "format": {
                    "type": "grammar",
                    "syntax": "lark",
                    "definition": "start: /[a-z]+/"
                },
                "x_custom": 7
            },
            "cache_control": { "type": "ephemeral" }
        });
        let tool = parse_tool_definition(&nested_custom).expect("nested custom tool");
        assert_eq!(tool.tool_type, "custom");
        assert_eq!(tool.name, None);
        assert_eq!(tool.description, None);
        assert!(tool.function.is_none());
        assert_eq!(
            tool.extra_body.get("cache_control"),
            Some(&json!({ "type": "ephemeral" }))
        );
        let custom = tool.custom.as_ref().expect("custom IR");
        assert_eq!(custom.name, "freeform_nested");
        assert_eq!(custom.description.as_deref(), Some("Nested custom tool"));
        assert_eq!(
            custom.format.as_ref().expect("format")["type"],
            json!("grammar")
        );
        assert_eq!(custom.extra_body.get("x_custom"), Some(&json!(7)));

        let flat_custom = json!({
            "type": "custom",
            "name": "freeform_flat",
            "description": "Flat custom tool",
            "format": { "type": "text" },
            "defer_loading": true
        });
        let tool = parse_tool_definition(&flat_custom).expect("flat custom tool");
        assert_eq!(tool.tool_type, "custom");
        assert!(tool.function.is_none());
        assert!(tool.extra_body.is_empty());
        let custom = tool.custom.as_ref().expect("custom IR");
        assert_eq!(custom.name, "freeform_flat");
        assert_eq!(custom.description.as_deref(), Some("Flat custom tool"));
        assert_eq!(
            custom.format.as_ref().expect("format")["type"],
            json!("text")
        );
        assert_eq!(custom.extra_body.get("defer_loading"), Some(&json!(true)));
    }

    #[test]
    fn parse_tool_definition_preserves_builtin_config() {
        let builtin = json!({
            "type": "file_search",
            "name": "docs_search",
            "description": "Search documentation",
            "vector_store_ids": ["vs_1", "vs_2"],
            "filters": {
                "type": "eq",
                "key": "tenant",
                "value": "monoize"
            },
            "ranking_options": {
                "ranker": "default-2024-11-15",
                "score_threshold": 0.25
            }
        });
        let tool = parse_tool_definition(&builtin).expect("builtin tool");
        assert_eq!(tool.tool_type, "file_search");
        assert_eq!(tool.name.as_deref(), Some("docs_search"));
        assert_eq!(tool.description.as_deref(), Some("Search documentation"));
        assert!(tool.function.is_none());
        assert!(tool.custom.is_none());
        assert_eq!(
            tool.extra_body.get("vector_store_ids"),
            Some(&json!(["vs_1", "vs_2"]))
        );
        assert_eq!(
            tool.extra_body.get("filters"),
            Some(&json!({
                "type": "eq",
                "key": "tenant",
                "value": "monoize"
            }))
        );
        assert_eq!(
            tool.extra_body.get("ranking_options"),
            Some(&json!({
                "ranker": "default-2024-11-15",
                "score_threshold": 0.25
            }))
        );
        assert!(!tool.extra_body.contains_key("type"));
        assert!(!tool.extra_body.contains_key("name"));
        assert!(!tool.extra_body.contains_key("description"));

        let unknown_native = json!({
            "type": "vendor_native",
            "name": "native_tool",
            "description": "Native provider tool",
            "enabled": true,
            "schema": "opaque"
        });
        let tool = parse_tool_definition(&unknown_native).expect("unknown native tool");
        assert_eq!(tool.tool_type, "vendor_native");
        assert_eq!(tool.name.as_deref(), Some("native_tool"));
        assert_eq!(tool.description.as_deref(), Some("Native provider tool"));
        assert!(tool.function.is_none());
        assert!(tool.custom.is_none());
        assert_eq!(tool.extra_body.get("enabled"), Some(&json!(true)));
        assert_eq!(tool.extra_body.get("schema"), Some(&json!("opaque")));

        let web_search = json!({
            "type": "web_search",
            "search_context_size": "medium",
            "user_location": {
                "type": "approximate",
                "country": "US",
                "city": "San Francisco"
            },
            "filters": {
                "allowed_domains": ["example.com"]
            }
        });
        let tool = parse_tool_definition(&web_search).expect("web_search tool");
        assert_eq!(tool.tool_type, "web_search");
        assert_eq!(tool.name, None);
        assert_eq!(tool.description, None);
        assert!(tool.function.is_none());
        assert!(tool.custom.is_none());
        assert_eq!(
            tool.extra_body.get("search_context_size"),
            Some(&json!("medium"))
        );
        assert_eq!(
            tool.extra_body.get("user_location"),
            Some(&json!({
                "type": "approximate",
                "country": "US",
                "city": "San Francisco"
            }))
        );
        assert_eq!(
            tool.extra_body.get("filters"),
            Some(&json!({ "allowed_domains": ["example.com"] }))
        );

        let computer = json!({
            "type": "computer_20251124",
            "name": "desktop",
            "description": "Computer use beta tool",
            "display_width_px": 1024,
            "display_height_px": 768,
            "display_number": 1,
            "cache_control": { "type": "ephemeral" }
        });
        let tool = parse_tool_definition(&computer).expect("computer_20251124 tool");
        assert_eq!(tool.tool_type, "computer_20251124");
        assert_eq!(tool.name.as_deref(), Some("desktop"));
        assert_eq!(tool.description.as_deref(), Some("Computer use beta tool"));
        assert!(tool.function.is_none());
        assert!(tool.custom.is_none());
        assert_eq!(tool.extra_body.get("display_width_px"), Some(&json!(1024)));
        assert_eq!(tool.extra_body.get("display_height_px"), Some(&json!(768)));
        assert_eq!(tool.extra_body.get("display_number"), Some(&json!(1)));
        assert_eq!(
            tool.extra_body.get("cache_control"),
            Some(&json!({ "type": "ephemeral" }))
        );

        let web_search_versioned = json!({
            "type": "web_search_20260209",
            "name": "web_search",
            "max_uses": 4,
            "allowed_domains": ["example.com"],
            "user_location": {
                "type": "approximate",
                "country": "US",
                "region": "CA",
                "city": "San Francisco"
            }
        });
        let tool = parse_tool_definition(&web_search_versioned).expect("web_search_20260209 tool");
        assert_eq!(tool.tool_type, "web_search_20260209");
        assert_eq!(tool.name.as_deref(), Some("web_search"));
        assert!(tool.function.is_none());
        assert!(tool.custom.is_none());
        assert_eq!(tool.extra_body.get("max_uses"), Some(&json!(4)));
        assert_eq!(
            tool.extra_body.get("allowed_domains"),
            Some(&json!(["example.com"]))
        );
        assert_eq!(
            tool.extra_body.get("user_location"),
            Some(&json!({
                "type": "approximate",
                "country": "US",
                "region": "CA",
                "city": "San Francisco"
            }))
        );

        let mcp_toolset = json!({
            "type": "mcp_toolset",
            "mcp_server_name": "docs",
            "default_config": { "enabled": true },
            "configs": {
                "search": { "enabled": true, "defer_loading": true }
            },
            "cache_control": { "type": "ephemeral" }
        });
        let tool = parse_tool_definition(&mcp_toolset).expect("mcp_toolset tool");
        assert_eq!(tool.tool_type, "mcp_toolset");
        assert_eq!(tool.name, None);
        assert!(tool.function.is_none());
        assert!(tool.custom.is_none());
        assert_eq!(tool.extra_body.get("mcp_server_name"), Some(&json!("docs")));
        assert_eq!(
            tool.extra_body.get("default_config"),
            Some(&json!({ "enabled": true }))
        );
        assert_eq!(
            tool.extra_body.get("configs"),
            Some(&json!({
                "search": { "enabled": true, "defer_loading": true }
            }))
        );
        assert_eq!(
            tool.extra_body.get("cache_control"),
            Some(&json!({ "type": "ephemeral" }))
        );
    }

    #[test]
    fn parse_tool_definition_parameters_precede_input_schema_alias() {
        let raw = json!({
            "type": "function",
            "function": {
                "name": "schema_precedence",
                "parameters": {
                    "type": "object",
                    "properties": {
                        "canonical": { "type": "string" }
                    }
                },
                "input_schema": {
                    "type": "object",
                    "properties": {
                        "alias": { "type": "boolean" }
                    }
                }
            }
        });

        let tool = parse_tool_definition(&raw).expect("function tool");
        let function = tool.function.as_ref().expect("function IR");

        assert_eq!(
            function.parameters.as_ref(),
            Some(&json!({
                "type": "object",
                "properties": {
                    "canonical": { "type": "string" }
                }
            })),
            "parameters is the canonical schema field and deterministically wins over input_schema"
        );
        assert!(
            function.extra_body.get("input_schema").is_none(),
            "the losing input_schema alias is consumed, not preserved as an extra collision"
        );
    }

    #[test]
    fn parse_tool_definition_preserves_non_object_schema_values() {
        for schema in [json!("schema://opaque"), json!(false), json!(["ref", "v1"])] {
            let raw = json!({
                "name": "schema_reference",
                "input_schema": schema.clone()
            });

            let tool = parse_tool_definition(&raw).expect("non-object schema function");
            let function = tool.function.as_ref().expect("function IR");

            assert_eq!(
                function.parameters.as_ref(),
                Some(&schema),
                "non-object schema values are preserved without normalization or panic"
            );
        }
    }
}
