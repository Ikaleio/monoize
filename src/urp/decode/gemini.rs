use crate::urp::decode::{
    deserialize_u64ish_default, parse_file_part_from_obj, parse_image_part_from_obj, split_extra,
};
use crate::urp::internal_legacy_bridge::{Part, Role, nodes_to_items};
use crate::urp::{
    FinishReason, InputDetails, Node, OrdinaryRole, OutputDetails, ReasoningConfig, ToolChoice,
    ToolResultContent, UrpRequest, UrpResponse, Usage,
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

    let mut input_nodes = Vec::new();

    if let Some(system_instruction) = obj.get("systemInstruction") {
        let text = collect_content_text(system_instruction);
        if !text.is_empty() {
            input_nodes.push(Node::Text {
                id: None,
                role: OrdinaryRole::System,
                content: text,
                phase: None,
                extra_body: HashMap::new(),
            });
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
            let message_extra = split_extra(content_obj, &["role", "parts"]);
            let mut message_parts = Vec::new();
            if let Some(parts) = content_obj.get("parts").and_then(|v| v.as_array()) {
                for part in parts {
                    match decode_input_part(part) {
                        DecodedInput::Parts(parts) => message_parts.extend(parts),
                        DecodedInput::ToolResult(node) => {
                            push_message_item(
                                &mut input_nodes,
                                role,
                                &mut message_parts,
                                message_extra.clone(),
                            );
                            input_nodes.push(node);
                        }
                    }
                }
            }
            push_message_item(&mut input_nodes, role, &mut message_parts, message_extra);
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
        input: input_nodes,
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

    let output_nodes = decode_response_nodes(&content);
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
        created_at: None,
        output: output_nodes,
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

enum DecodedInput {
    Parts(Vec<Part>),
    ToolResult(Node),
}

enum DecodedOutput {
    Nodes(Vec<Node>),
    ToolResult(Node),
}

fn decode_input_part(part: &Value) -> DecodedInput {
    let Some(obj) = part.as_object() else {
        return DecodedInput::Parts(Vec::new());
    };

    if let Some(fr) = obj.get("functionResponse").and_then(|v| v.as_object()) {
        return DecodedInput::ToolResult(decode_function_response(fr));
    }

    DecodedInput::Parts(decode_content_parts(obj))
}

fn decode_output_part(part: &Value) -> DecodedOutput {
    let Some(obj) = part.as_object() else {
        return DecodedOutput::Nodes(Vec::new());
    };

    if let Some(fr) = obj.get("functionResponse").and_then(|v| v.as_object()) {
        return DecodedOutput::ToolResult(decode_function_response(fr));
    }

    DecodedOutput::Nodes(parts_to_nodes(
        Role::Assistant,
        decode_content_parts(obj),
        HashMap::new(),
    ))
}

fn parts_to_nodes(role: Role, parts: Vec<Part>, extra_body: HashMap<String, Value>) -> Vec<Node> {
    let ordinary_role = role.to_ordinary().unwrap_or(OrdinaryRole::User);
    let mut nodes = Vec::new();
    for (index, part) in parts.into_iter().enumerate() {
        let mut node = part.into_node(ordinary_role);
        if index == 0 && !extra_body.is_empty() {
            node.extra_body_mut().extend(extra_body.clone());
        }
        nodes.push(node);
    }
    nodes
}

fn decode_content_parts(obj: &Map<String, Value>) -> Vec<Part> {
    let mut out = Vec::new();

    if let Some(text) = obj.get("text").and_then(|v| v.as_str()) {
        if !text.is_empty() {
            if obj.get("thought").and_then(|v| v.as_bool()) == Some(true) {
                out.push(Part::Reasoning {
                    id: None,
                    content: Some(text.to_string()),
                    encrypted: None,
                    summary: None,
                    source: None,
                    extra_body: split_extra(obj, &["text", "thought", "thoughtSignature"]),
                });
            } else {
                out.push(Part::Text {
                    content: text.to_string(),
                    extra_body: split_extra(obj, &["text", "thought", "thoughtSignature"]),
                });
            }
        }
    }

    if let Some(sig) = obj.get("thoughtSignature") {
        out.push(Part::Reasoning {
            id: None,
            content: None,
            encrypted: Some(sig.clone()),
            summary: None,
            source: None,
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
                id: fc.get("id").and_then(|v| v.as_str()).map(|s| s.to_string()),
                call_id,
                name,
                arguments: args,
                extra_body: split_extra(fc, &["id", "name", "args"]),
            });
        }
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

    out
}

fn decode_function_response(fr: &Map<String, Value>) -> Node {
    let name = fr
        .get("name")
        .and_then(|v| v.as_str())
        .unwrap_or("")
        .to_string();
    let response_value = fr.get("response").cloned().unwrap_or(Value::Null);
    Node::ToolResult {
        id: fr.get("id").and_then(|v| v.as_str()).map(|s| s.to_string()),
        call_id: name.clone(),
        is_error: false,
        content: vec![ToolResultContent::Text {
            text: serde_json::to_string(&response_value).unwrap_or_default(),
        }],
        extra_body: split_extra(fr, &["id", "name", "response"]),
    }
}

fn push_message_item(
    input: &mut Vec<Node>,
    role: Role,
    parts: &mut Vec<Part>,
    extra_body: HashMap<String, Value>,
) {
    if parts.is_empty() {
        return;
    }

    input.extend(parts_to_nodes(role, std::mem::take(parts), extra_body));
}

fn decode_response_nodes(content: &Map<String, Value>) -> Vec<Node> {
    let content_extra = split_extra(content, &["role", "parts"]);
    let mut output_nodes = Vec::new();
    let mut did_attach_content_extra = false;

    if let Some(parts) = content.get("parts").and_then(|v| v.as_array()) {
        for part in parts {
            match decode_output_part(part) {
                DecodedOutput::Nodes(nodes) => {
                    for node in nodes {
                        let mut node = node;
                        if !did_attach_content_extra {
                            let extra =
                                take_output_extra(&content_extra, &mut did_attach_content_extra);
                            if !extra.is_empty() {
                                node.extra_body_mut().extend(extra);
                            }
                        }
                        output_nodes.push(node);
                    }
                }
                DecodedOutput::ToolResult(node) => {
                    output_nodes.push(node);
                }
            }
        }
    }

    output_nodes
}

fn take_output_extra(
    content_extra: &HashMap<String, Value>,
    did_attach_content_extra: &mut bool,
) -> HashMap<String, Value> {
    if *did_attach_content_extra {
        HashMap::new()
    } else {
        *did_attach_content_extra = true;
        content_extra.clone()
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

#[cfg(test)]
mod tests {
    use super::decode_response;
    use crate::urp::internal_legacy_bridge::nodes_to_items;
    use serde_json::{json, Value};

    #[test]
    fn decode_response_greedy_merges_assistant_parts_and_extracts_tool_results() {
        let response = decode_response(&json!({
            "responseId": "resp_1",
            "modelVersion": "gemini-2.0-flash",
            "candidates": [{
                "finishReason": "STOP",
                "content": {
                    "role": "model",
                    "parts": [
                        { "text": "thinking", "thought": true },
                        { "text": "hello" },
                        { "functionCall": { "name": "lookup", "args": { "q": 1 } } },
                        { "text": "after" },
                        { "functionResponse": { "name": "lookup", "response": { "result": { "ok": true } } } }
                    ],
                    "custom": true
                }
            }]
        }))
        .expect("response should decode");

        let outputs = serde_json::to_value(&response.output).expect("outputs should serialize");
        assert_eq!(
            outputs,
            Value::Array(vec![
                json!({
                    "type": "reasoning",
                    "content": "thinking",
                    "custom": true
                }),
                json!({
                    "type": "text",
                    "role": "assistant",
                    "content": "hello"
                }),
                json!({
                    "type": "tool_call",
                    "call_id": "lookup",
                    "name": "lookup",
                    "arguments": "{\"q\":1}"
                }),
                json!({
                    "type": "text",
                    "role": "assistant",
                    "content": "after"
                }),
                json!({
                    "type": "tool_result",
                    "call_id": "lookup",
                    "is_error": false,
                    "content": [
                        { "type": "text", "text": "{\"result\":{\"ok\":true}}" }
                    ]
                })
            ])
        );
        assert_eq!(nodes_to_items(&response.output).len(), 3);
    }
}
