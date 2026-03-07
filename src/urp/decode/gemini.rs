use crate::urp::decode::{
    deserialize_u64ish_default, parse_file_part_from_obj, parse_image_part_from_obj, split_extra,
};
use crate::urp::{
    FinishReason, InputDetails, Message, OutputDetails, Part, ReasoningConfig, Role, ToolChoice,
    UrpRequest, UrpResponse, Usage,
};
use serde::Deserialize;
use serde_json::{json, Map, Value};
use std::collections::HashMap;

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct GeminiUsage {
    #[serde(
        default,
        deserialize_with = "deserialize_u64ish_default",
        alias = "prompt_token_count"
    )]
    prompt_token_count: u64,
    #[serde(
        default,
        deserialize_with = "deserialize_u64ish_default",
        alias = "candidates_token_count"
    )]
    candidates_token_count: u64,
    #[serde(
        default,
        deserialize_with = "deserialize_u64ish_default",
        alias = "thoughts_token_count",
        alias = "reasoning_tokens",
        alias = "reasoning_output_token_count"
    )]
    thoughts_token_count: u64,
    #[serde(
        default,
        deserialize_with = "deserialize_u64ish_default",
        alias = "cached_content_token_count",
        alias = "cached_tokens",
        alias = "cache_read_tokens",
        alias = "cache_read_input_tokens"
    )]
    cached_content_token_count: u64,
    #[serde(
        default,
        deserialize_with = "deserialize_u64ish_default",
        alias = "cache_creation_input_tokens",
        alias = "cache_write_tokens",
        alias = "cacheCreationTokenCount",
        alias = "cache_creation_token_count"
    )]
    cache_creation_tokens: u64,
    #[serde(
        default,
        deserialize_with = "deserialize_u64ish_default",
        alias = "toolPromptInputTokenCount",
        alias = "tool_prompt_input_token_count",
        alias = "tool_prompt_tokens"
    )]
    tool_prompt_tokens: u64,
    #[serde(
        default,
        deserialize_with = "deserialize_u64ish_default",
        alias = "accepted_prediction_token_count",
        alias = "accepted_prediction_tokens",
        alias = "acceptedPredictionOutputTokenCount"
    )]
    accepted_prediction_token_count: u64,
    #[serde(
        default,
        deserialize_with = "deserialize_u64ish_default",
        alias = "rejected_prediction_token_count",
        alias = "rejected_prediction_tokens",
        alias = "rejectedPredictionOutputTokenCount"
    )]
    rejected_prediction_token_count: u64,
    #[serde(flatten)]
    extra: HashMap<String, Value>,
}

impl From<GeminiUsage> for Usage {
    fn from(value: GeminiUsage) -> Self {
        let input_details = if value.cached_content_token_count > 0
            || value.cache_creation_tokens > 0
            || value.tool_prompt_tokens > 0
        {
            Some(InputDetails {
                standard_tokens: 0,
                cache_read_tokens: value.cached_content_token_count,
                cache_creation_tokens: value.cache_creation_tokens,
                tool_prompt_tokens: value.tool_prompt_tokens,
                modality_breakdown: None,
            })
        } else {
            None
        };

        let output_details = if value.thoughts_token_count > 0
            || value.accepted_prediction_token_count > 0
            || value.rejected_prediction_token_count > 0
        {
            Some(OutputDetails {
                standard_tokens: 0,
                reasoning_tokens: value.thoughts_token_count,
                accepted_prediction_tokens: value.accepted_prediction_token_count,
                rejected_prediction_tokens: value.rejected_prediction_token_count,
                modality_breakdown: None,
            })
        } else {
            None
        };

        Usage {
            input_tokens: value.prompt_token_count,
            output_tokens: value.candidates_token_count,
            input_details,
            output_details,
            extra_body: value.extra,
        }
    }
}

pub fn decode_request(value: &Value) -> Result<UrpRequest, String> {
    let obj = value
        .as_object()
        .ok_or_else(|| "gemini request must be object".to_string())?;

    let model = obj
        .get("model")
        .and_then(|v| v.as_str())
        .unwrap_or_default()
        .to_string();

    let mut messages = Vec::new();

    if let Some(system_instruction) = obj.get("systemInstruction") {
        let text = collect_content_text(system_instruction);
        if !text.is_empty() {
            messages.push(Message::text(Role::System, text));
        }
    }

    if let Some(contents) = obj.get("contents").and_then(|v| v.as_array()) {
        for content in contents {
            let Some(content_obj) = content.as_object() else {
                continue;
            };
            let role = match content_obj.get("role").and_then(|v| v.as_str()) {
                Some("model") => Role::Assistant,
                Some("assistant") => Role::Assistant,
                Some("system") => Role::System,
                Some("developer") => Role::Developer,
                _ => Role::User,
            };
            let mut message = Message {
                role,
                parts: Vec::new(),
                extra_body: split_extra(content_obj, &["role", "parts"]),
            };
            if let Some(parts) = content_obj.get("parts").and_then(|v| v.as_array()) {
                for part in parts {
                    decode_part(part, &mut message.parts);
                }
            }
            if !message.parts.is_empty() {
                messages.push(message);
            }
        }
    }

    let mut reasoning = None;
    if let Some(gen_cfg) = obj.get("generationConfig").and_then(|v| v.as_object()) {
        if let Some(thinking) = gen_cfg.get("thinkingConfig").and_then(|v| v.as_object()) {
            let budget = thinking
                .get("thinkingBudget")
                .and_then(|v| v.as_u64())
                .unwrap_or(0);
            let effort = if budget == 0 {
                None
            } else if budget <= 512 {
                Some("low".to_string())
            } else if budget >= 2048 {
                Some("high".to_string())
            } else {
                Some("medium".to_string())
            };
            reasoning = Some(ReasoningConfig {
                effort,
                extra_body: split_extra(
                    thinking,
                    &["thinkingBudget", "includeThoughts", "thinkingLevel"],
                ),
            });
        }
    }

    let tools = obj
        .get("tools")
        .and_then(|v| v.as_array())
        .map(decode_tools);

    let tool_choice = obj
        .get("toolConfig")
        .and_then(|v| v.get("functionCallingConfig"))
        .cloned()
        .and_then(parse_tool_choice);

    Ok(UrpRequest {
        model,
        messages,
        stream: obj
            .get("stream")
            .and_then(|v| v.as_bool())
            .or_else(|| obj.get("streamGenerateContent").and_then(|v| v.as_bool())),
        temperature: obj
            .get("generationConfig")
            .and_then(|v| v.get("temperature"))
            .and_then(|v| v.as_f64()),
        top_p: obj
            .get("generationConfig")
            .and_then(|v| v.get("topP"))
            .and_then(|v| v.as_f64()),
        max_output_tokens: obj
            .get("generationConfig")
            .and_then(|v| v.get("maxOutputTokens"))
            .and_then(|v| v.as_u64()),
        reasoning,
        tools,
        tool_choice,
        response_format: None,
        user: None,
        extra_body: split_extra(
            obj,
            &[
                "model",
                "contents",
                "systemInstruction",
                "generationConfig",
                "tools",
                "toolConfig",
                "stream",
                "streamGenerateContent",
            ],
        ),
    })
}

pub fn decode_response(value: &Value) -> Result<UrpResponse, String> {
    let obj = value
        .as_object()
        .ok_or_else(|| "gemini response must be object".to_string())?;

    let candidate = obj
        .get("candidates")
        .and_then(|v| v.as_array())
        .and_then(|arr| arr.first())
        .and_then(|v| v.as_object())
        .ok_or_else(|| "missing candidates[0]".to_string())?;

    let content = candidate
        .get("content")
        .and_then(|v| v.as_object())
        .cloned()
        .unwrap_or_default();

    let mut message = Message {
        role: Role::Assistant,
        parts: Vec::new(),
        extra_body: split_extra(&content, &["role", "parts"]),
    };

    if let Some(parts) = content.get("parts").and_then(|v| v.as_array()) {
        for part in parts {
            decode_part(part, &mut message.parts);
        }
    }

    let finish_reason = candidate
        .get("finishReason")
        .and_then(|v| v.as_str())
        .map(parse_finish_reason);

    let usage = obj
        .get("usageMetadata")
        .and_then(|v| v.as_object())
        .map(parse_usage);

    Ok(UrpResponse {
        id: obj
            .get("responseId")
            .or_else(|| obj.get("id"))
            .and_then(|v| v.as_str())
            .unwrap_or("gemini_response")
            .to_string(),
        model: obj
            .get("modelVersion")
            .or_else(|| obj.get("model"))
            .and_then(|v| v.as_str())
            .unwrap_or_default()
            .to_string(),
        message,
        finish_reason,
        usage,
        extra_body: split_extra(
            obj,
            &[
                "candidates",
                "promptFeedback",
                "usageMetadata",
                "modelVersion",
                "responseId",
                "id",
                "model",
            ],
        ),
    })
}

fn decode_tools(tools: &Vec<Value>) -> Vec<crate::urp::ToolDefinition> {
    let mut out = Vec::new();
    for tool in tools {
        let Some(tool_obj) = tool.as_object() else {
            continue;
        };
        let Some(decls) = tool_obj
            .get("functionDeclarations")
            .and_then(|v| v.as_array())
        else {
            continue;
        };
        for decl in decls {
            let Some(decl_obj) = decl.as_object() else {
                continue;
            };
            let Some(name) = decl_obj.get("name").and_then(|v| v.as_str()) else {
                continue;
            };
            out.push(crate::urp::ToolDefinition {
                tool_type: "function".to_string(),
                function: Some(crate::urp::FunctionDefinition {
                    name: name.to_string(),
                    description: decl_obj
                        .get("description")
                        .and_then(|v| v.as_str())
                        .map(|s| s.to_string()),
                    parameters: decl_obj.get("parameters").cloned(),
                    strict: None,
                    extra_body: split_extra(decl_obj, &["name", "description", "parameters"]),
                }),
                extra_body: HashMap::new(),
            });
        }
    }
    out
}

fn parse_tool_choice(value: Value) -> Option<ToolChoice> {
    let obj = value.as_object()?;
    let mode = obj.get("mode").and_then(|v| v.as_str()).unwrap_or("AUTO");
    match mode {
        "NONE" => Some(ToolChoice::Mode("none".to_string())),
        "ANY" => {
            if let Some(first_name) = obj
                .get("allowedFunctionNames")
                .and_then(|v| v.as_array())
                .and_then(|arr| arr.first())
                .and_then(|v| v.as_str())
            {
                Some(ToolChoice::Specific(json!({
                    "type": "function",
                    "function": { "name": first_name }
                })))
            } else {
                Some(ToolChoice::Mode("required".to_string()))
            }
        }
        _ => Some(ToolChoice::Mode("auto".to_string())),
    }
}

fn decode_part(part: &Value, out: &mut Vec<Part>) {
    let Some(obj) = part.as_object() else {
        return;
    };

    if let Some(text) = obj.get("text").and_then(|v| v.as_str()) {
        if !text.is_empty() {
            if obj.get("thought").and_then(|v| v.as_bool()) == Some(true) {
                out.push(Part::Reasoning {
                    content: text.to_string(),
                    extra_body: split_extra(obj, &["text", "thought", "thoughtSignature"]),
                });
            } else {
                out.push(Part::Text {
                    content: text.to_string(),
                    phase: None,
                    extra_body: split_extra(obj, &["text", "thought", "thoughtSignature"]),
                });
            }
        }
    }

    if let Some(sig) = obj.get("thoughtSignature") {
        out.push(Part::ReasoningEncrypted {
            data: sig.clone(),
            extra_body: HashMap::new(),
        });
    }

    if let Some(fc) = obj.get("functionCall").and_then(|v| v.as_object()) {
        let call_id = fc
            .get("id")
            .or_else(|| fc.get("name"))
            .and_then(|v| v.as_str())
            .unwrap_or("")
            .to_string();
        let name = fc
            .get("name")
            .and_then(|v| v.as_str())
            .unwrap_or("")
            .to_string();
        let args = serde_json::to_string(&fc.get("args").cloned().unwrap_or(Value::Null))
            .unwrap_or_else(|_| "{}".to_string());
        if !name.is_empty() {
            out.push(Part::ToolCall {
                call_id,
                name,
                arguments: args,
                extra_body: split_extra(fc, &["id", "name", "args"]),
            });
        }
    }

    if let Some(fr) = obj.get("functionResponse").and_then(|v| v.as_object()) {
        let call_id = fr
            .get("id")
            .or_else(|| fr.get("name"))
            .and_then(|v| v.as_str())
            .unwrap_or("")
            .to_string();
        let response = fr.get("response").cloned().unwrap_or(Value::Null);
        let output = response
            .get("result")
            .cloned()
            .unwrap_or_else(|| response.clone());
        let text = if let Some(s) = output.as_str() {
            s.to_string()
        } else {
            output.to_string()
        };
        let mut msg = Message::new(Role::Tool);
        msg.parts.push(Part::ToolResult {
            call_id,
            is_error: response
                .get("is_error")
                .and_then(|v| v.as_bool())
                .unwrap_or(false),
            extra_body: split_extra(fr, &["id", "name", "response"]),
        });
        if !text.is_empty() {
            msg.parts.push(Part::Text {
                content: text,
                phase: None,
                extra_body: HashMap::new(),
            });
        }
        out.extend(msg.parts);
    }

    if let Some(inline_data) = obj.get("inlineData").and_then(|v| v.as_object()) {
        let mime = inline_data
            .get("mimeType")
            .and_then(|v| v.as_str())
            .unwrap_or("application/octet-stream")
            .to_string();
        let data = inline_data
            .get("data")
            .and_then(|v| v.as_str())
            .unwrap_or("")
            .to_string();
        if mime.starts_with("image/") {
            out.push(Part::Image {
                source: crate::urp::ImageSource::Base64 {
                    media_type: mime,
                    data,
                },
                extra_body: split_extra(obj, &["inlineData"]),
            });
        } else {
            out.push(Part::File {
                source: crate::urp::FileSource::Base64 {
                    filename: None,
                    media_type: mime,
                    data,
                },
                extra_body: split_extra(obj, &["inlineData"]),
            });
        }
    }

    if let Some(file_data) = obj.get("fileData").and_then(|v| v.as_object()) {
        let uri = file_data
            .get("fileUri")
            .and_then(|v| v.as_str())
            .unwrap_or("")
            .to_string();
        let mime = file_data
            .get("mimeType")
            .and_then(|v| v.as_str())
            .unwrap_or("application/octet-stream");
        if mime.starts_with("image/") {
            out.push(Part::Image {
                source: crate::urp::ImageSource::Url {
                    url: uri,
                    detail: None,
                },
                extra_body: split_extra(obj, &["fileData"]),
            });
        } else {
            out.push(Part::File {
                source: crate::urp::FileSource::Url { url: uri },
                extra_body: split_extra(obj, &["fileData"]),
            });
        }
    }

    if let Some(image) = parse_image_part_from_obj(obj) {
        out.push(image);
    }
    if let Some(file) = parse_file_part_from_obj(obj) {
        out.push(file);
    }
}

fn parse_finish_reason(reason: &str) -> FinishReason {
    match reason {
        "MAX_TOKENS" => FinishReason::Length,
        "SAFETY" => FinishReason::ContentFilter,
        "STOP" => FinishReason::Stop,
        _ => FinishReason::Other,
    }
}

fn parse_usage(obj: &Map<String, Value>) -> Usage {
    serde_json::from_value::<GeminiUsage>(Value::Object(obj.clone()))
        .map(Usage::from)
        .unwrap_or_else(|_| Usage {
            input_tokens: 0,
            output_tokens: 0,
            input_details: None,
            output_details: None,
            extra_body: obj.clone().into_iter().collect(),
        })
}

fn collect_content_text(value: &Value) -> String {
    if let Some(s) = value.as_str() {
        return s.to_string();
    }
    let mut out = String::new();
    if let Some(obj) = value.as_object() {
        if let Some(parts) = obj.get("parts").and_then(|v| v.as_array()) {
            for part in parts {
                if let Some(text) = part.get("text").and_then(|v| v.as_str()) {
                    out.push_str(text);
                }
            }
        }
    }
    out
}
