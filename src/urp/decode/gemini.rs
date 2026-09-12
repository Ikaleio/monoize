use crate::urp::decode::{
    deserialize_u64ish_default, parse_compatible_media_part, retain_wire_extra_fields, split_extra,
};
use crate::urp::internal_legacy_bridge::{Part, Role};
use crate::urp::{
    FinishReason, InputDetails, JsonSchemaDefinition, Node, OrdinaryRole, OutputDetails,
    ProviderProtocol, ResponseFormat, StopControl, ToolChoice, ToolResultContent, UrpRequest,
    UrpResponse, Usage,
};
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
        let input_modality = value
            .extra
            .remove("promptTokensDetails")
            .and_then(parse_modality);
        let cache_modality = value
            .extra
            .remove("cacheTokensDetails")
            .and_then(parse_modality);
        let output_modality = value
            .extra
            .remove("candidatesTokensDetails")
            .and_then(parse_modality);
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
            || input_modality.is_some()
            || cache_modality.is_some()
        {
            Some(InputDetails {
                standard_tokens: 0,
                cache_read_tokens: value.cached_content_token_count,
                cache_read_modality_breakdown: cache_modality,
                cache_creation_tokens: value.cache_creation_tokens,
                cache_creation_5m_tokens: 0,
                cache_creation_1h_tokens: 0,
                tool_prompt_tokens: value.tool_prompt_tokens,
                modality_breakdown: input_modality,
            })
        } else {
            None
        };

        let output_details = if value.thoughts_token_count > 0
            || value.accepted_prediction_token_count > 0
            || value.rejected_prediction_token_count > 0
            || output_modality.is_some()
        {
            Some(OutputDetails {
                standard_tokens: 0,
                reasoning_tokens: value.thoughts_token_count,
                accepted_prediction_tokens: value.accepted_prediction_token_count,
                rejected_prediction_tokens: value.rejected_prediction_token_count,
                modality_breakdown: output_modality,
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

fn parse_modality(value: Value) -> Option<crate::urp::ModalityBreakdown> {
    let mut result = crate::urp::ModalityBreakdown::default();
    for entry in value.as_array()? {
        let count = entry.get("tokenCount").and_then(|value| {
            value
                .as_u64()
                .or_else(|| value.as_str().and_then(|text| text.parse().ok()))
        });
        match entry.get("modality").and_then(Value::as_str) {
            Some("TEXT") => result.text_tokens = count,
            Some("IMAGE") => result.image_tokens = count,
            Some("AUDIO") => result.audio_tokens = count,
            Some("VIDEO") => result.video_tokens = count,
            Some("DOCUMENT") => result.document_tokens = count,
            _ => {}
        }
    }
    Some(result)
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
        let parts = system_instruction
            .get("parts")
            .unwrap_or(system_instruction);
        for part in content_parts(parts) {
            match decode_input_part(part)? {
                DecodedInput::Parts(parts) => {
                    input_nodes.extend(parts_to_nodes(Role::System, parts, HashMap::new()));
                }
                DecodedInput::ToolResult(node) => input_nodes.push(node),
            }
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
            if let Some(parts) = content_obj.get("parts") {
                for part in content_parts(parts) {
                    match decode_input_part(part)? {
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
                                name,
                                ..
                            } = &mut node
                            {
                                if let Some(name) = name.as_deref() {
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
        .map(|tools| decode_tools(tools));

    let tool_choice = obj
        .get("toolConfig")
        .and_then(|v| v.get("functionCallingConfig"))
        .cloned()
        .and_then(parse_tool_choice);

    let mut request_extra = split_extra(
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
    );
    if let Some(cfg) = request_extra
        .get_mut("generationConfig")
        .and_then(Value::as_object_mut)
    {
        for key in ["temperature", "topP", "maxOutputTokens", "stopSequences"] {
            cfg.remove(key);
        }
        if matches!(
            cfg.get("responseMimeType").and_then(Value::as_str),
            Some("text/plain" | "application/json")
        ) {
            for key in ["responseMimeType", "responseJsonSchema", "responseSchema"] {
                cfg.remove(key);
            }
        }
        if let Some(thinking) = cfg.get_mut("thinkingConfig").and_then(Value::as_object_mut) {
            for key in ["thinkingLevel", "thinkingBudget", "includeThoughts"] {
                thinking.remove(key);
            }
        }
    }
    if let Some(config) = obj.get("toolConfig").and_then(Value::as_object) {
        let mut extra = split_extra(config, &["functionCallingConfig"]);
        if let Some(function) = config
            .get("functionCallingConfig")
            .and_then(Value::as_object)
        {
            let unknown = split_extra(function, &["mode", "allowedFunctionNames"]);
            if !unknown.is_empty() {
                extra.insert("functionCallingConfig".into(), json!(unknown));
            }
        }
        if !extra.is_empty() {
            request_extra.insert("toolConfig".into(), json!(extra));
        }
    }
    crate::urp::tool_signature::restore_request_call_signatures(&mut input_nodes);
    Ok(UrpRequest {
        context: Default::default(),
        instructions_format: None,
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
        reasoning: obj
            .get("generationConfig")
            .and_then(|v| v.get("thinkingConfig"))
            .and_then(Value::as_object)
            .map(|cfg| crate::urp::ReasoningConfig {
                effort: cfg
                    .get("thinkingLevel")
                    .and_then(Value::as_str)
                    .map(str::to_string),
                budget_tokens: cfg.get("thinkingBudget").and_then(Value::as_u64),
                mode: cfg
                    .get("thinkingBudget")
                    .and_then(Value::as_i64)
                    .and_then(|n| match n {
                        0 => Some("disabled".into()),
                        -1 => Some("adaptive".into()),
                        _ => None,
                    }),
                summary: cfg
                    .get("includeThoughts")
                    .and_then(Value::as_bool)
                    .map(|enabled| {
                        if enabled {
                            "auto".into()
                        } else {
                            "none".into()
                        }
                    }),
                ..Default::default()
            }),
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
            match cfg.get("responseMimeType").and_then(Value::as_str) {
                Some("text/plain") => return Some(ResponseFormat::Text),
                Some("application/json") => {}
                _ => return None,
            }
            match cfg
                .get("responseJsonSchema")
                .or_else(|| cfg.get("responseSchema"))
            {
                Some(schema) => Some(ResponseFormat::JsonSchema {
                    json_schema: JsonSchemaDefinition {
                        name: "gemini_response".to_string(),
                        description: None,
                        schema: if cfg.get("responseJsonSchema").is_some() {
                            schema.clone()
                        } else {
                            native_schema_types(schema, false)
                        },
                        strict: None,
                        extra_body: HashMap::new(),
                    },
                }),
                None => Some(ResponseFormat::JsonObject),
            }
        }),
        user: None,
        extra_body: request_extra,
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
    let mut output_nodes = decode_response_nodes(&content)?;
    if let Some(candidate) = candidate {
        attach_candidate_citations(candidate, &mut output_nodes);
    }
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
        extra_body: {
            let mut extra = split_extra(
                obj,
                &[
                    "candidates",
                    "usageMetadata",
                    "modelVersion",
                    "responseId",
                    "id",
                    "model",
                ],
            );
            if let Some(candidate) = candidate {
                let metadata = candidate_extra(candidate);
                if !metadata.is_empty() {
                    extra.insert(GEMINI_CANDIDATE_EXTRA_KEY.into(), json!(metadata));
                }
            }
            extra
        },
    })
}

fn decode_tools(tools: &[Value]) -> Vec<crate::urp::ToolDefinition> {
    let mut out = Vec::new();
    for tool in tools {
        let Some(tool_obj) = tool.as_object() else {
            continue;
        };
        if let Some(declarations) = tool_obj
            .get("functionDeclarations")
            .and_then(Value::as_array)
        {
            for declaration in declarations {
                let Some(decl) = declaration.as_object() else {
                    continue;
                };
                let Some(name) = decl.get("name").and_then(Value::as_str) else {
                    continue;
                };
                let mut extra = split_extra(
                    decl,
                    &["name", "description", "parameters", "parametersJsonSchema"],
                );
                if decl.contains_key("parametersJsonSchema") || decl.contains_key("parameters") {
                    extra.insert(
                        "_monoize_gemini_parameters_json_schema".into(),
                        json!(decl.contains_key("parametersJsonSchema")),
                    );
                }
                out.push(crate::urp::ToolDefinition {
                    namespace: None,
                    tools: None,
                    origin_protocol: None,
                    config: None,
                    tool_type: "function".into(),
                    name: None,
                    description: None,
                    custom: None,
                    function: Some(crate::urp::FunctionDefinition {
                        name: name.into(),
                        description: decl
                            .get("description")
                            .and_then(Value::as_str)
                            .map(str::to_string),
                        parameters: decl.get("parametersJsonSchema").cloned().or_else(|| {
                            decl.get("parameters")
                                .map(|schema| native_schema_types(schema, false))
                        }),
                        strict: None,
                        extra_body: extra,
                    }),
                    extra_body: HashMap::new(),
                });
            }
        }
        for (kind, config) in tool_obj {
            if kind == "functionDeclarations" {
                continue;
            }
            out.push(crate::urp::ToolDefinition {
                namespace: None,
                tools: None,
                origin_protocol: Some(ProviderProtocol::Gemini),
                config: Some(config.clone()),
                tool_type: kind.clone(),
                name: None,
                description: None,
                function: None,
                custom: None,
                extra_body: HashMap::new(),
            });
        }
    }
    out
}

fn parse_tool_choice(value: Value) -> Option<ToolChoice> {
    let obj = value.as_object()?;
    let mode = obj.get("mode").and_then(Value::as_str).unwrap_or("AUTO");
    let mode = match mode {
        "NONE" => "none",
        "ANY" => "required",
        "VALIDATED" => "validated",
        _ => "auto",
    };
    if let Some(names) = obj.get("allowedFunctionNames").and_then(Value::as_array) {
        return Some(ToolChoice::Specific(json!({
            "type":"allowed_tools", "mode":mode,
            "tools":names.iter().filter_map(Value::as_str).map(|name| json!({"type":"function","name":name})).collect::<Vec<_>>()
        })));
    }
    Some(ToolChoice::Mode(mode.into()))
}

enum DecodedInput {
    Parts(Vec<Part>),
    ToolResult(Node),
}

enum DecodedOutput {
    Nodes(Vec<Node>),
    ToolResult(Node),
}

pub(crate) fn content_parts(value: &Value) -> &[Value] {
    value
        .as_array()
        .map(Vec::as_slice)
        .unwrap_or_else(|| std::slice::from_ref(value))
}

fn decode_input_part(part: &Value) -> Result<DecodedInput, String> {
    if let Some(obj) = part.as_object() {
        if let Some(fr) = obj.get("functionResponse").and_then(|v| v.as_object()) {
            return Ok(DecodedInput::ToolResult(decode_function_response(obj, fr)?));
        }
    }
    Ok(DecodedInput::Parts(decode_part_value(part)?))
}

fn decode_output_part(part: &Value) -> Result<DecodedOutput, String> {
    if let Some(obj) = part.as_object() {
        if let Some(fr) = obj.get("functionResponse").and_then(|v| v.as_object()) {
            return Ok(DecodedOutput::ToolResult(decode_function_response(
                obj, fr,
            )?));
        }
    }
    Ok(DecodedOutput::Nodes(parts_to_nodes(
        Role::Assistant,
        decode_part_value(part)?,
        HashMap::new(),
    )))
}

fn decode_part_value(value: &Value) -> Result<Vec<Part>, String> {
    match value {
        Value::String(text) => Ok(vec![Part::Text {
            content: text.clone(),
            signature: None,
            citations: Vec::new(),
            extra_body: HashMap::new(),
        }]),
        Value::Object(obj) => decode_content_parts(obj),
        _ => Ok(vec![Part::ProviderItem {
            id: Some(crate::urp::synthetic_provider_item_id()),
            origin_protocol: ProviderProtocol::Gemini,
            item_type: "unknown_part".into(),
            body: value.clone(),
            extra_body: HashMap::new(),
        }]),
    }
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
pub(crate) const GEMINI_CANDIDATE_EXTRA_KEY: &str = "_monoize_gemini_candidate";
pub(crate) const GEMINI_SYNTHETIC_CALL_PREFIX: &str = "call_gemini_";
fn part_extra(obj: &Map<String, Value>, known: &[&str]) -> HashMap<String, Value> {
    let native = split_extra(obj, known);
    let mut extra = HashMap::new();
    if !native.is_empty() {
        extra.insert(GEMINI_PART_EXTRA_KEY.to_string(), json!(native));
    }
    extra
}

fn preserve_media_extra(
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
        part[key] = json!(unknown);
    }
}

fn decode_content_parts(obj: &Map<String, Value>) -> Result<Vec<Part>, String> {
    if let Some(text) = obj.get("text").and_then(Value::as_str) {
        return Ok(vec![
            if obj.get("thought").and_then(Value::as_bool) == Some(true) {
                Part::Reasoning {
                    metadata: Default::default(),

                    id: None,
                    content: Some(text.to_string()),
                    encrypted: obj.get("thoughtSignature").cloned(),
                    summary: None,
                    source: None,
                    extra_body: part_extra(obj, &["text", "thought", "thoughtSignature"]),
                }
            } else {
                Part::Text {
                    citations: Vec::new(),
                    signature: obj.get("thoughtSignature").cloned(),

                    content: text.to_string(),
                    extra_body: part_extra(obj, &["text", "thoughtSignature"]),
                }
            },
        ]);
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
            preserve_media_extra(fc, "functionCall", &["id", "name", "args"], &mut extra);
            let parts = vec![Part::ToolCall {
                namespace: None,
                signature: obj.get("thoughtSignature").cloned(),
                id: fc.get("id").and_then(Value::as_str).map(str::to_string),
                tool_type: crate::urp::ToolCallType::Function,
                call_id: call_id.clone(),
                name: name.to_string(),
                arguments: serde_json::to_string(fc.get("args").unwrap_or(&json!({})))
                    .unwrap_or_default(),
                extra_body: extra,
            }];
            return Ok(parts);
        }
    }
    if let Some(value) = obj.get("inlineData") {
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
        preserve_media_extra(data, "inlineData", &["mimeType", "data"], &mut extra_body);
        let metadata = crate::urp::MediaMetadata {
            signature: obj.get("thoughtSignature").cloned(),
            ..Default::default()
        };
        return Ok(vec![
            if crate::urp::media::mime_essence(&mime).starts_with("image/") {
                Part::Image {
                    metadata,
                    source: crate::urp::ImageSource::Base64 {
                        media_type: mime,
                        data: bytes,
                    },
                    extra_body,
                }
            } else if crate::urp::media::is_audio_mime(&mime) {
                Part::Audio {
                    metadata,
                    source: crate::urp::AudioSource::Base64 {
                        media_type: mime,
                        data: bytes,
                    },
                    extra_body,
                }
            } else {
                Part::File {
                    metadata,
                    source: crate::urp::FileSource::Base64 {
                        media_type: mime,
                        data: bytes,
                    },
                    extra_body,
                }
            },
        ]);
    }
    if let Some(value) = obj.get("fileData") {
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
        preserve_media_extra(data, "fileData", &["mimeType", "fileUri"], &mut extra_body);
        let metadata = crate::urp::MediaMetadata {
            signature: obj.get("thoughtSignature").cloned(),
            resource: crate::urp::media::resource_for_url(&url),
            media_type: mime.map(str::to_string),
            ..Default::default()
        };
        return Ok(vec![if mime
            .is_some_and(|mime| crate::urp::media::mime_essence(mime).starts_with("image/"))
        {
            Part::Image {
                metadata,
                source: crate::urp::ImageSource::Url { url, detail: None },
                extra_body,
            }
        } else if mime.is_some_and(crate::urp::media::is_audio_mime) {
            Part::Audio {
                metadata,
                source: crate::urp::AudioSource::Url { url },
                extra_body,
            }
        } else {
            Part::File {
                metadata,
                source: crate::urp::FileSource::Url { url },
                extra_body,
            }
        }]);
    }
    if let Some(mut media) = parse_compatible_media_part(obj)? {
        if let Part::Image {
            metadata,
            extra_body,
            ..
        }
        | Part::Audio {
            metadata,
            extra_body,
            ..
        }
        | Part::File {
            metadata,
            extra_body,
            ..
        } = &mut media
        {
            if let Some(signature) = obj.get("thoughtSignature") {
                metadata.signature = Some(signature.clone());
                extra_body.remove("thoughtSignature");
            }
        }
        return Ok(vec![media]);
    }
    if obj.contains_key("thoughtSignature")
        && ![
            "functionCall",
            "inlineData",
            "fileData",
            "executableCode",
            "codeExecutionResult",
            "toolCall",
            "toolResponse",
        ]
        .iter()
        .any(|key| obj.contains_key(*key))
    {
        return Ok(vec![Part::Reasoning {
            metadata: Default::default(),
            id: None,
            content: None,
            summary: None,
            source: None,
            encrypted: obj.get("thoughtSignature").cloned(),
            extra_body: part_extra(obj, &["thoughtSignature"]),
        }]);
    }
    Ok(vec![Part::ProviderItem {
        id: obj
            .get("id")
            .and_then(Value::as_str)
            .map(str::to_string)
            .or_else(|| Some(crate::urp::synthetic_provider_item_id())),
        origin_protocol: ProviderProtocol::Gemini,
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
    }])
}

pub(crate) fn decode_stream_part(part: &Value) -> Result<Vec<Node>, String> {
    Ok(match decode_output_part(part)? {
        DecodedOutput::Nodes(nodes) => nodes,
        DecodedOutput::ToolResult(node) => vec![node],
    })
}

fn decode_function_response(
    parent: &Map<String, Value>,
    fr: &Map<String, Value>,
) -> Result<Node, String> {
    let name = fr
        .get("name")
        .and_then(|v| v.as_str())
        .unwrap_or("")
        .to_string();
    let response_value = fr.get("response").cloned().unwrap_or(Value::Null);
    let mut content = vec![ToolResultContent::Text {
        text: serde_json::to_string(&response_value).unwrap_or_default(),
        extra_body: HashMap::new(),
    }];
    if let Some(parts) = fr.get("parts") {
        for part in content_parts(parts) {
            for node in decode_stream_part(part)? {
                match node {
                    Node::Text {
                        content: text,
                        extra_body,
                        ..
                    } => {
                        content.push(ToolResultContent::Text { text, extra_body });
                    }
                    Node::Image {
                        source,
                        metadata,
                        extra_body,
                        ..
                    } => content.push(ToolResultContent::Image {
                        source,
                        metadata,
                        extra_body,
                    }),
                    Node::File {
                        source,
                        metadata,
                        extra_body,
                        ..
                    } => content.push(ToolResultContent::File {
                        source,
                        metadata,
                        extra_body,
                    }),
                    Node::Audio {
                        source: crate::urp::AudioSource::Base64 { media_type, data },
                        metadata,
                        extra_body,
                        ..
                    } => content.push(ToolResultContent::File {
                        source: crate::urp::FileSource::Base64 { media_type, data },
                        metadata,
                        extra_body,
                    }),
                    Node::Audio {
                        source: crate::urp::AudioSource::Url { url },
                        metadata,
                        extra_body,
                        ..
                    } => content.push(ToolResultContent::File {
                        source: crate::urp::FileSource::Url { url },
                        metadata,
                        extra_body,
                    }),
                    Node::ProviderItem {
                        origin_protocol,
                        item_type,
                        body,
                        extra_body,
                        ..
                    } => content.push(ToolResultContent::ProviderItem {
                        origin_protocol,
                        item_type,
                        body,
                        extra_body,
                    }),
                    _ => return Err("Gemini functionResponse.parts contains unsupported nested tool or reasoning content.".into()),
                }
            }
        }
    }
    Ok(Node::ToolResult {
        signature: parent.get("thoughtSignature").cloned(),
        namespace: None,
        name: Some(name.clone()),
        id: fr.get("id").and_then(|v| v.as_str()).map(|s| s.to_string()),
        tool_type: crate::urp::ToolCallType::Function,
        call_id: fr
            .get("id")
            .and_then(Value::as_str)
            .unwrap_or(&name)
            .to_string(),
        is_error: response_value.get("error").is_some(),
        content,
        extra_body: {
            let mut extra = split_extra(fr, &["id", "name", "response", "parts"]);
            extra.extend(part_extra(
                parent,
                &["functionResponse", "thoughtSignature"],
            ));
            extra.insert(
                "_monoize_gemini_function_response".to_string(),
                Value::Bool(true),
            );
            extra
        },
    })
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

fn decode_response_nodes(content: &Map<String, Value>) -> Result<Vec<Node>, String> {
    let content_extra = split_extra(content, &["role", "parts"]);
    let mut output_nodes = Vec::new();
    let mut did_attach_content_extra = false;

    if let Some(parts) = content.get("parts") {
        for part in content_parts(parts) {
            match decode_output_part(part)? {
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

    Ok(output_nodes)
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

pub(crate) fn parse_usage(obj: &Map<String, Value>) -> Result<Usage, String> {
    let mut native = obj.clone();
    native.remove("totalTokenCount");
    serde_json::from_value::<GeminiUsage>(Value::Object(native))
        .map_err(|err| format!("invalid Gemini usage metadata: {err}"))?
        .try_into()
}

pub(crate) fn candidate_extra(candidate: &Map<String, Value>) -> HashMap<String, Value> {
    let mut extra = split_extra(candidate, &["content", "finishReason", "citationMetadata"]);
    if let Some(citations) = candidate.get("citationMetadata").and_then(Value::as_object) {
        let unknown = split_extra(citations, &["citationSources"]);
        if !unknown.is_empty() {
            extra.insert("citationMetadata".into(), json!(unknown));
        }
    }
    extra
}

pub(crate) fn attach_candidate_citations(candidate: &Map<String, Value>, nodes: &mut [Node]) {
    let Some(sources) = candidate
        .get("citationMetadata")
        .and_then(|value| value.get("citationSources"))
        .and_then(Value::as_array)
    else {
        return;
    };
    if let Some(Node::Text { citations, .. }) = nodes
        .iter_mut()
        .find(|node| matches!(node, Node::Text { .. }))
    {
        citations.extend(sources.iter().cloned());
    }
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

pub(crate) fn native_schema_types(schema: &Value, uppercase: bool) -> Value {
    match schema {
        Value::Object(object) => Value::Object(
            object
                .iter()
                .map(|(key, value)| {
                    let value = if key == "type" {
                        value
                            .as_str()
                            .map(|kind| {
                                json!(if uppercase {
                                    kind.to_ascii_uppercase()
                                } else {
                                    kind.to_ascii_lowercase()
                                })
                            })
                            .unwrap_or_else(|| native_schema_types(value, uppercase))
                    } else {
                        native_schema_types(value, uppercase)
                    };
                    (key.clone(), value)
                })
                .collect(),
        ),
        Value::Array(items) => json!(
            items
                .iter()
                .map(|item| native_schema_types(item, uppercase))
                .collect::<Vec<_>>()
        ),
        _ => schema.clone(),
    }
}
