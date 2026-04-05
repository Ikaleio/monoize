pub mod anthropic;
pub mod gemini;
pub mod openai_chat;
pub mod openai_image;
pub mod openai_responses;

use crate::urp::{FileSource, FunctionDefinition, ImageSource, Part, ToolDefinition};
use serde::{Deserialize, Deserializer};
use serde_json::{Map, Value};
use std::collections::HashMap;

pub fn split_extra(obj: &Map<String, Value>, known: &[&str]) -> HashMap<String, Value> {
    let mut extra = HashMap::new();
    for (k, v) in obj {
        if !known.contains(&k.as_str()) {
            extra.insert(k.clone(), v.clone());
        }
    }
    extra
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

pub fn parse_tool_definition(raw: &Value) -> Option<ToolDefinition> {
    let obj = raw.as_object()?;
    let tool_type = obj
        .get("type")
        .and_then(|v| v.as_str())
        .unwrap_or("function")
        .to_string();

    if tool_type == "function" {
        let function_obj = obj.get("function").and_then(|v| v.as_object());
        if let Some(function_obj) = function_obj {
            let name = function_obj
                .get("name")
                .and_then(|v| v.as_str())?
                .to_string();
            let mut fn_extra = HashMap::new();
            for (k, v) in function_obj {
                if !["name", "description", "parameters", "strict"].contains(&k.as_str()) {
                    fn_extra.insert(k.clone(), v.clone());
                }
            }
            return Some(ToolDefinition {
                tool_type,
                function: Some(FunctionDefinition {
                    name,
                    description: function_obj
                        .get("description")
                        .and_then(|v| v.as_str())
                        .map(|s| s.to_string()),
                    parameters: normalize_tool_parameters(function_obj.get("parameters").cloned()),
                    strict: function_obj.get("strict").and_then(|v| v.as_bool()),
                    extra_body: fn_extra,
                }),
                extra_body: split_extra(obj, &["type", "function"]),
            });
        }

        let name = obj.get("name").and_then(|v| v.as_str())?.to_string();
        return Some(ToolDefinition {
            tool_type,
            function: Some(FunctionDefinition {
                name,
                description: obj
                    .get("description")
                    .and_then(|v| v.as_str())
                    .map(|s| s.to_string()),
                parameters: normalize_tool_parameters(
                    obj.get("parameters")
                        .cloned()
                        .or_else(|| obj.get("input_schema").cloned()),
                ),
                strict: obj.get("strict").and_then(|v| v.as_bool()),
                extra_body: HashMap::new(),
            }),
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

    Some(ToolDefinition {
        tool_type,
        function: None,
        extra_body: split_extra(obj, &["type"]),
    })
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
}

pub fn parse_tool_call_part_from_obj(obj: &Map<String, Value>) -> Option<Part> {
    let tool_type = obj.get("type").and_then(|v| v.as_str())?;
    if !matches!(tool_type, "tool_call" | "function_call" | "tool_use") {
        return None;
    }

    let call_id = obj
        .get("call_id")
        .or_else(|| obj.get("id"))
        .and_then(|v| v.as_str())
        .unwrap_or("")
        .to_string();
    let name = obj
        .get("name")
        .and_then(|v| v.as_str())
        .or_else(|| {
            obj.get("function")
                .and_then(|value| value.as_object())
                .and_then(|function| function.get("name"))
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

    Some(Part::ToolCall {
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
            ],
        ),
    })
}

pub fn parse_image_part_from_obj(obj: &Map<String, Value>) -> Option<Part> {
    let t = obj.get("type")?.as_str()?;
    match t {
        "image_url" | "input_image" | "output_image" | "image" => {
            if let Some(url) = obj.get("image_url").and_then(|v| v.as_str()) {
                return Some(Part::Image {
                    source: ImageSource::Url {
                        url: url.to_string(),
                        detail: obj
                            .get("detail")
                            .and_then(|v| v.as_str())
                            .map(|s| s.to_string()),
                    },
                    extra_body: split_extra(obj, &["type", "image_url", "detail"]),
                });
            }
            if let Some(url_obj) = obj.get("image_url").and_then(|v| v.as_object()) {
                let url = url_obj.get("url")?.as_str()?.to_string();
                let detail = url_obj
                    .get("detail")
                    .and_then(|v| v.as_str())
                    .map(|s| s.to_string());
                return Some(Part::Image {
                    source: ImageSource::Url { url, detail },
                    extra_body: HashMap::new(),
                });
            }
            if let Some(url) = obj.get("url").and_then(|v| v.as_str()) {
                return Some(Part::Image {
                    source: ImageSource::Url {
                        url: url.to_string(),
                        detail: obj
                            .get("detail")
                            .and_then(|v| v.as_str())
                            .map(|s| s.to_string()),
                    },
                    extra_body: split_extra(obj, &["type", "url", "detail"]),
                });
            }
            if let Some(data) = obj.get("image_base64").and_then(|v| v.as_str()) {
                return Some(Part::Image {
                    source: ImageSource::Base64 {
                        media_type: obj
                            .get("media_type")
                            .and_then(|v| v.as_str())
                            .unwrap_or("image/png")
                            .to_string(),
                        data: data.to_string(),
                    },
                    extra_body: split_extra(obj, &["type", "image_base64", "media_type"]),
                });
            }
            if let Some(src) = obj.get("source").and_then(|v| v.as_object()) {
                if let Some(url) = src.get("url").and_then(|v| v.as_str()) {
                    return Some(Part::Image {
                        source: ImageSource::Url {
                            url: url.to_string(),
                            detail: obj
                                .get("detail")
                                .and_then(|v| v.as_str())
                                .map(|s| s.to_string()),
                        },
                        extra_body: split_extra(obj, &["type", "source", "detail"]),
                    });
                }
                let media_type = src
                    .get("media_type")
                    .and_then(|v| v.as_str())
                    .unwrap_or("image/png")
                    .to_string();
                let data = src.get("data").and_then(|v| v.as_str())?.to_string();
                return Some(Part::Image {
                    source: ImageSource::Base64 { media_type, data },
                    extra_body: split_extra(obj, &["type", "source"]),
                });
            }
            None
        }
        _ => None,
    }
}

pub fn parse_file_part_from_obj(obj: &Map<String, Value>) -> Option<Part> {
    let t = obj.get("type")?.as_str()?;
    match t {
        "input_file" | "output_file" | "document" | "file" => {
            if let Some(url) = obj.get("url").and_then(|v| v.as_str()) {
                return Some(Part::File {
                    source: FileSource::Url {
                        url: url.to_string(),
                    },
                    extra_body: split_extra(obj, &["type", "url"]),
                });
            }
            if let Some(url) = obj.get("file_url").and_then(|v| v.as_str()) {
                return Some(Part::File {
                    source: FileSource::Url {
                        url: url.to_string(),
                    },
                    extra_body: split_extra(obj, &["type", "file_url"]),
                });
            }
            if let Some(src) = obj.get("source").and_then(|v| v.as_object()) {
                if let Some(url) = src.get("url").and_then(|v| v.as_str()) {
                    return Some(Part::File {
                        source: FileSource::Url {
                            url: url.to_string(),
                        },
                        extra_body: split_extra(obj, &["type", "source"]),
                    });
                }
                let media_type = src
                    .get("media_type")
                    .and_then(|v| v.as_str())
                    .unwrap_or("application/octet-stream")
                    .to_string();
                let data = src.get("data").and_then(|v| v.as_str())?.to_string();
                let filename = src
                    .get("filename")
                    .and_then(|v| v.as_str())
                    .map(|s| s.to_string());
                return Some(Part::File {
                    source: FileSource::Base64 {
                        filename,
                        media_type,
                        data,
                    },
                    extra_body: split_extra(obj, &["type", "source"]),
                });
            }
            if let Some(data) = obj
                .get("file_data")
                .or_else(|| obj.get("data"))
                .and_then(|v| v.as_str())
            {
                return Some(Part::File {
                    source: FileSource::Base64 {
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
                    },
                    extra_body: split_extra(
                        obj,
                        &["type", "file_data", "data", "filename", "media_type"],
                    ),
                });
            }
            if let Some(file_id) = obj.get("file_id").and_then(|v| v.as_str()) {
                return Some(Part::File {
                    source: FileSource::Url {
                        url: format!("file_id://{file_id}"),
                    },
                    extra_body: split_extra(obj, &["type", "file_id"]),
                });
            }
            None
        }
        _ => None,
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
            if let Some(obj) = item.as_object() {
                if let Some(text) = obj.get("text").and_then(|x| x.as_str()) {
                    out.push_str(text);
                }
            }
        }
        return out;
    }
    serde_json::to_string(v).unwrap_or_default()
}
