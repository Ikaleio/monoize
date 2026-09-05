use crate::urp::decode::{
    deserialize_u64ish_default, parse_file_part_from_obj, parse_image_part_from_obj,
    retain_wire_extra_fields, split_extra,
};
use crate::urp::internal_legacy_bridge::{Part, Role};
use crate::urp::{
    FinishReason, InputDetails, JsonSchemaDefinition, Node, OrdinaryRole, OutputDetails,
    ProviderProtocol, ResponseFormat, StopControl, ToolChoice, ToolResultContent, UrpRequest,
    UrpResponse, Usage,
};
use base64::{Engine as _, engine::general_purpose::URL_SAFE_NO_PAD};
use serde::Deserialize;
use serde_json::{Map, Value, json};
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
        alias = "toolUsePromptTokenCount",
        alias = "tool_use_prompt_token_count",
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

impl TryFrom<GeminiUsage> for Usage {
    type Error = String;

    fn try_from(mut value: GeminiUsage) -> Result<Self, Self::Error> {
        retain_wire_extra_fields(&mut value.extra);
        let input_tokens = value
            .prompt_token_count
            .checked_add(value.tool_prompt_tokens)
            .ok_or_else(|| "Gemini input token total overflow".to_string())?;
        let output_tokens = value
            .candidates_token_count
            .checked_add(value.thoughts_token_count)
            .ok_or_else(|| "Gemini output token total overflow".to_string())?;
        let input_details = if value.cached_content_token_count > 0
            || value.cache_creation_tokens > 0
            || value.tool_prompt_tokens > 0
        {
            Some(InputDetails {
                standard_tokens: 0,
                cache_read_tokens: value.cached_content_token_count,
                cache_read_modality_breakdown: None,
                cache_creation_tokens: value.cache_creation_tokens,
                cache_creation_5m_tokens: 0,
                cache_creation_1h_tokens: 0,
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

        Ok(Usage {
            input_tokens,
            output_tokens,
            input_details,
            output_details,
            extra_body: value.extra,
        })
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

    let mut resolved_results = std::collections::HashSet::new();
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
                        DecodedInput::ToolResult(mut node) => {
                            push_message_item(
                                &mut input_nodes,
                                role,
                                &mut message_parts,
                                message_extra.clone(),
                            );
                            if let Node::ToolResult {
                                id: None,
                                call_id,
                                extra_body,
                                ..
                            } = &mut node
                            {
                                if let Some(name) = extra_body.get("name").and_then(Value::as_str) {
                                    if let Some(matching_id) = input_nodes
                                        .iter()
                                        .filter_map(|node| match node {
                                            Node::ToolCall {
                                                call_id,
                                                name: call_name,
                                                ..
                                            } if call_name == name
                                                && !resolved_results.contains(call_id) =>
                                            {
                                                Some(call_id.clone())
                                            }
                                            _ => None,
                                        })
                                        .next()
                                    {
                                        *call_id = matching_id;
                                    }
                                }
                            }
                            if let Node::ToolResult { call_id, .. } = &node {
                                resolved_results.insert(call_id.clone());
                            }
                            input_nodes.push(node);
                        }
                    }
                }
            }
            push_message_item(&mut input_nodes, role, &mut message_parts, message_extra);
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
        reasoning: None,
        tools,
        tool_choice,
        parallel_tool_calls: None,
        stop: obj
            .get("generationConfig")
            .and_then(|v| v.get("stopSequences"))
            .cloned()
            .and_then(|v| serde_json::from_value::<Vec<String>>(v).ok())
            .map(StopControl::Multiple),
        verbosity: None,
        response_format: obj.get("generationConfig").and_then(|cfg| {
            if cfg.get("responseMimeType").and_then(Value::as_str) != Some("application/json") {
                return None;
            }
            match cfg.get("responseJsonSchema") {
                Some(schema) => Some(ResponseFormat::JsonSchema {
                    json_schema: JsonSchemaDefinition {
                        name: "gemini_response".to_string(),
                        description: None,
                        schema: schema.clone(),
                        strict: None,
                        extra_body: HashMap::new(),
                    },
                }),
                None if cfg.get("responseSchema").is_none() => Some(ResponseFormat::JsonObject),
                None => None,
            }
        }),
        user: None,
        extra_body: split_extra(
            obj,
            &[
                "model",
                "contents",
                "systemInstruction",
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
        .and_then(Value::as_array)
        .and_then(|items| items.first())
        .and_then(Value::as_object);
    let blocked = prompt_block_reason(value);
    if candidate.is_none() && blocked.is_none() {
        return Err("missing candidates[0]".to_string());
    }
    let content = candidate
        .and_then(|candidate| candidate.get("content"))
        .and_then(Value::as_object)
        .cloned()
        .unwrap_or_default();
    let mut output_nodes = decode_response_nodes(&content);
    let mut finish_reason = candidate
        .and_then(|candidate| candidate.get("finishReason"))
        .and_then(Value::as_str)
        .map(parse_finish_reason);
    if let Some(reason) = blocked {
        output_nodes.push(prompt_refusal(reason));
        finish_reason = Some(FinishReason::ContentFilter);
    } else if finish_reason == Some(FinishReason::Stop)
        && output_nodes
            .iter()
            .any(|node| matches!(node, Node::ToolCall { .. }))
    {
        finish_reason = Some(FinishReason::ToolCalls);
    }

    let usage = match obj.get("usageMetadata").and_then(|v| v.as_object()) {
        Some(usage) => Some(parse_usage(usage)?),
        None => None,
    };

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
                name: None,
                description: None,
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
                custom: None,
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

pub(crate) const GEMINI_PART_EXTRA_KEY: &str = "_monoize_gemini_part";
pub(crate) const GEMINI_SYNTHETIC_CALL_PREFIX: &str = "call_gemini_";
const GEMINI_CALL_SIGNATURE_PREFIX: &str = "rs_gemini_call_";

pub(crate) fn signature_call_id(id: &str) -> Option<String> {
    String::from_utf8(
        URL_SAFE_NO_PAD
            .decode(id.strip_prefix(GEMINI_CALL_SIGNATURE_PREFIX)?)
            .ok()?,
    )
    .ok()
}

fn part_extra(obj: &Map<String, Value>, known: &[&str]) -> HashMap<String, Value> {
    let native = split_extra(obj, known);
    let mut extra = HashMap::new();
    if !native.is_empty() {
        extra.insert(GEMINI_PART_EXTRA_KEY.to_string(), json!(native));
    }
    extra
}

fn decode_content_parts(obj: &Map<String, Value>) -> Vec<Part> {
    if let Some(text) = obj.get("text").and_then(Value::as_str) {
        return vec![
            if obj.get("thought").and_then(Value::as_bool) == Some(true) {
                Part::Reasoning {
                    id: None,
                    content: Some(text.to_string()),
                    encrypted: obj.get("thoughtSignature").cloned(),
                    summary: None,
                    source: None,
                    extra_body: part_extra(obj, &["text", "thought", "thoughtSignature"]),
                }
            } else {
                Part::Text {
                    content: text.to_string(),
                    extra_body: part_extra(obj, &["text"]),
                }
            },
        ];
    }
    if let Some(fc) = obj.get("functionCall").and_then(Value::as_object) {
        if let Some(name) = fc
            .get("name")
            .and_then(Value::as_str)
            .filter(|name| !name.is_empty())
        {
            let call_id = fc
                .get("id")
                .and_then(Value::as_str)
                .filter(|id| !id.is_empty())
                .map(str::to_string)
                .unwrap_or_else(|| {
                    format!(
                        "{GEMINI_SYNTHETIC_CALL_PREFIX}{}",
                        uuid::Uuid::new_v4().simple()
                    )
                });
            let mut extra = part_extra(obj, &["functionCall", "thoughtSignature"]);
            extra.extend(split_extra(fc, &["id", "name", "args"]));
            let mut parts = vec![Part::ToolCall {
                id: fc.get("id").and_then(Value::as_str).map(str::to_string),
                tool_type: crate::urp::ToolCallType::Function,
                call_id: call_id.clone(),
                name: name.to_string(),
                arguments: serde_json::to_string(fc.get("args").unwrap_or(&json!({})))
                    .unwrap_or_default(),
                extra_body: extra,
            }];
            if let Some(signature) = obj.get("thoughtSignature") {
                parts.insert(
                    0,
                    Part::Reasoning {
                        id: Some(format!(
                            "{GEMINI_CALL_SIGNATURE_PREFIX}{}",
                            URL_SAFE_NO_PAD.encode(call_id)
                        )),
                        content: None,
                        encrypted: Some(signature.clone()),
                        summary: None,
                        source: None,
                        extra_body: HashMap::new(),
                    },
                );
            }
            return parts;
        }
    }
    if let Some(data) = obj.get("inlineData").and_then(Value::as_object) {
        let mime = data
            .get("mimeType")
            .and_then(Value::as_str)
            .unwrap_or("application/octet-stream")
            .to_string();
        let bytes = data
            .get("data")
            .and_then(Value::as_str)
            .unwrap_or("")
            .to_string();
        let extra_body = part_extra(obj, &["inlineData"]);
        return vec![if mime.starts_with("image/") {
            Part::Image {
                source: crate::urp::ImageSource::Base64 {
                    media_type: mime,
                    data: bytes,
                },
                extra_body,
            }
        } else {
            Part::File {
                source: crate::urp::FileSource::Base64 {
                    filename: None,
                    media_type: mime,
                    data: bytes,
                },
                extra_body,
            }
        }];
    }
    if let Some(data) = obj.get("fileData").and_then(Value::as_object) {
        let url = data
            .get("fileUri")
            .and_then(Value::as_str)
            .unwrap_or("")
            .to_string();
        let mime = data
            .get("mimeType")
            .and_then(Value::as_str)
            .unwrap_or("application/octet-stream");
        let extra_body = part_extra(obj, &["fileData"]);
        return vec![if mime.starts_with("image/") {
            Part::Image {
                source: crate::urp::ImageSource::Url { url, detail: None },
                extra_body,
            }
        } else {
            Part::File {
                source: crate::urp::FileSource::Url { url },
                extra_body,
            }
        }];
    }
    if let Some(image) = parse_image_part_from_obj(obj) {
        return vec![image];
    }
    if let Some(file) = parse_file_part_from_obj(obj) {
        return vec![file];
    }
    vec![Part::ProviderItem {
        id: obj
            .get("id")
            .and_then(Value::as_str)
            .map(str::to_string)
            .or_else(|| Some(crate::urp::synthetic_provider_item_id())),
        origin_protocol: ProviderProtocol::Gemini,
        item_type: obj
            .get("type")
            .and_then(Value::as_str)
            .unwrap_or("")
            .to_string(),
        body: Value::Object(obj.clone()),
        extra_body: HashMap::new(),
    }]
}

pub(crate) fn decode_stream_part(part: &Value) -> Vec<Node> {
    match decode_output_part(part) {
        DecodedOutput::Nodes(nodes) => nodes,
        DecodedOutput::ToolResult(node) => vec![node],
    }
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
        tool_type: crate::urp::ToolCallType::Function,
        call_id: fr
            .get("id")
            .and_then(Value::as_str)
            .unwrap_or(&name)
            .to_string(),
        is_error: false,
        content: vec![ToolResultContent::Text {
            text: serde_json::to_string(&response_value).unwrap_or_default(),
            extra_body: HashMap::new(),
        }],
        extra_body: {
            let mut extra = split_extra(fr, &["id", "response"]);
            extra.insert(
                "_monoize_gemini_function_response".to_string(),
                Value::Bool(true),
            );
            extra
        },
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

pub(crate) fn parse_finish_reason(reason: &str) -> FinishReason {
    match reason {
        "MAX_TOKENS" => FinishReason::Length,
        "SAFETY"
        | "RECITATION"
        | "BLOCKLIST"
        | "PROHIBITED_CONTENT"
        | "SPII"
        | "IMAGE_SAFETY"
        | "IMAGE_PROHIBITED_CONTENT"
        | "IMAGE_RECITATION" => FinishReason::ContentFilter,
        "STOP" => FinishReason::Stop,
        _ => FinishReason::Other,
    }
}

fn parse_usage(obj: &Map<String, Value>) -> Result<Usage, String> {
    serde_json::from_value::<GeminiUsage>(Value::Object(obj.clone()))
        .map_err(|err| format!("invalid Gemini usage metadata: {err}"))?
        .try_into()
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

pub(crate) fn prompt_block_reason(value: &Value) -> Option<&str> {
    value
        .get("promptFeedback")?
        .get("blockReason")?
        .as_str()
        .filter(|reason| !reason.is_empty() && *reason != "BLOCK_REASON_UNSPECIFIED")
}

pub(crate) fn prompt_refusal(reason: &str) -> Node {
    Node::Refusal {
        id: None,
        content: format!("Gemini blocked the prompt: {reason}"),
        extra_body: HashMap::new(),
    }
}
