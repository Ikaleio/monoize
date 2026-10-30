use crate::urp::decode::{
    deserialize_u64ish_default, parse_compatible_media_part, retain_wire_extra_fields, split_extra,
};
use crate::urp::internal_legacy_bridge::{Part, Role};
use crate::urp::{
    FinishReason, InputDetails, JsonSchemaDefinition, Node, OrdinaryRole, OutputDetails,
    ProviderProtocol, ResponseFormat, ResponseOutcome, StopControl, ToolChoice, ToolResultContent,
    UrpRequest, UrpResponse, Usage,
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
        let tool_prompt_modality = value
            .extra
            .remove("toolUsePromptTokensDetails")
            .and_then(parse_modality);
        // Usage extras carry counters only; enum metadata such as `serviceTier` is not usage.
        value.extra.retain(|_, value| value.is_number());
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
            || tool_prompt_modality.is_some()
        {
            Some(InputDetails {
                standard_tokens: 0,
                cache_read_tokens: value.cached_content_token_count,
                cache_read_modality_breakdown: cache_modality,
                cache_creation_tokens: value.cache_creation_tokens,
                cache_creation_5m_tokens: 0,
                cache_creation_1h_tokens: 0,
                tool_prompt_tokens: value.tool_prompt_tokens,
                tool_prompt_modality_breakdown: tool_prompt_modality,
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
            iterations: None,
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

/// Renames documented proto-JSON snake_case fields to camelCase at structural positions only.
/// User payloads (function args, responses, schemas, labels) are never rewritten.
fn normalize_proto_json_names(value: &Value) -> Value {
    fn rename(obj: &mut Map<String, Value>, names: &[(&str, &str)]) {
        for (snake, camel) in names {
            if !obj.contains_key(*camel)
                && let Some(value) = obj.remove(*snake)
            {
                obj.insert((*camel).to_string(), value);
            }
        }
    }
    fn object<'a>(
        obj: &'a mut Map<String, Value>,
        key: &str,
    ) -> Option<&'a mut Map<String, Value>> {
        obj.get_mut(key).and_then(Value::as_object_mut)
    }
    fn part(value: &mut Value) {
        let Some(obj) = value.as_object_mut() else {
            return;
        };
        // OpenAI-compatible media parts use `file_data` as a string, so only proto-shaped
        // object members are renamed.
        for (snake, camel) in [
            ("inline_data", "inlineData"),
            ("file_data", "fileData"),
            ("function_call", "functionCall"),
            ("function_response", "functionResponse"),
            ("executable_code", "executableCode"),
            ("code_execution_result", "codeExecutionResult"),
            ("video_metadata", "videoMetadata"),
            ("media_resolution", "mediaResolution"),
            ("part_metadata", "partMetadata"),
        ] {
            if obj.get(snake).is_some_and(Value::is_object) {
                rename(obj, &[(snake, camel)]);
            }
        }
        rename(obj, &[("thought_signature", "thoughtSignature")]);
        for key in ["inlineData", "fileData"] {
            if let Some(data) = object(obj, key) {
                rename(
                    data,
                    &[
                        ("mime_type", "mimeType"),
                        ("file_uri", "fileUri"),
                        ("display_name", "displayName"),
                    ],
                );
            }
        }
        if let Some(metadata) = object(obj, "videoMetadata") {
            rename(
                metadata,
                &[("start_offset", "startOffset"), ("end_offset", "endOffset")],
            );
        }
        if let Some(response) = object(obj, "functionResponse") {
            rename(response, &[("will_continue", "willContinue")]);
            if let Some(parts) = response.get_mut("parts").and_then(Value::as_array_mut) {
                parts.iter_mut().for_each(part);
            }
        }
    }
    fn content(content: &mut Value) {
        match content.get_mut("parts") {
            Some(Value::Array(parts)) => parts.iter_mut().for_each(part),
            Some(single) => part(single),
            None => {}
        }
    }

    let mut value = value.clone();
    let Some(obj) = value.as_object_mut() else {
        return value;
    };
    rename(
        obj,
        &[
            ("system_instruction", "systemInstruction"),
            ("generation_config", "generationConfig"),
            ("tool_config", "toolConfig"),
            ("safety_settings", "safetySettings"),
            ("cached_content", "cachedContent"),
            ("continuation_token", "continuationToken"),
            ("service_tier", "serviceTier"),
        ],
    );
    if let Some(contents) = obj.get_mut("contents").and_then(Value::as_array_mut) {
        contents.iter_mut().for_each(content);
    }
    if let Some(system) = obj.get_mut("systemInstruction") {
        content(system);
    }
    if let Some(config) = object(obj, "generationConfig") {
        rename(
            config,
            &[
                ("stop_sequences", "stopSequences"),
                ("response_mime_type", "responseMimeType"),
                ("response_schema", "responseSchema"),
                ("response_json_schema", "responseJsonSchema"),
                ("response_modalities", "responseModalities"),
                ("candidate_count", "candidateCount"),
                ("max_output_tokens", "maxOutputTokens"),
                ("top_p", "topP"),
                ("top_k", "topK"),
                ("presence_penalty", "presencePenalty"),
                ("frequency_penalty", "frequencyPenalty"),
                ("response_logprobs", "responseLogprobs"),
                ("thinking_config", "thinkingConfig"),
                ("media_resolution", "mediaResolution"),
                ("image_config", "imageConfig"),
                ("speech_config", "speechConfig"),
                ("response_format", "responseFormat"),
            ],
        );
        if let Some(thinking) = object(config, "thinkingConfig") {
            rename(
                thinking,
                &[
                    ("thinking_budget", "thinkingBudget"),
                    ("thinking_level", "thinkingLevel"),
                    ("include_thoughts", "includeThoughts"),
                ],
            );
        }
        if let Some(image) = object(config, "imageConfig") {
            rename(
                image,
                &[("aspect_ratio", "aspectRatio"), ("image_size", "imageSize")],
            );
        }
    }
    if let Some(tools) = obj.get_mut("tools").and_then(Value::as_array_mut) {
        for tool in tools.iter_mut().filter_map(Value::as_object_mut) {
            rename(
                tool,
                &[
                    ("function_declarations", "functionDeclarations"),
                    ("google_search", "googleSearch"),
                    ("google_search_retrieval", "googleSearchRetrieval"),
                    ("code_execution", "codeExecution"),
                    ("url_context", "urlContext"),
                    ("file_search", "fileSearch"),
                    ("google_maps", "googleMaps"),
                    ("computer_use", "computerUse"),
                    ("mcp_servers", "mcpServers"),
                ],
            );
            if let Some(declarations) = tool
                .get_mut("functionDeclarations")
                .and_then(Value::as_array_mut)
            {
                for declaration in declarations.iter_mut().filter_map(Value::as_object_mut) {
                    rename(
                        declaration,
                        &[
                            ("parameters_json_schema", "parametersJsonSchema"),
                            ("response_json_schema", "responseJsonSchema"),
                        ],
                    );
                }
            }
        }
    }
    if let Some(config) = object(obj, "toolConfig") {
        rename(
            config,
            &[
                ("function_calling_config", "functionCallingConfig"),
                ("retrieval_config", "retrievalConfig"),
            ],
        );
        if let Some(function) = object(config, "functionCallingConfig") {
            rename(function, &[("allowed_function_names", "allowedFunctionNames")]);
        }
    }
    value
}

pub fn decode_request(value: &Value) -> Result<UrpRequest, String> {
    let value = &normalize_proto_json_names(value);
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
        let first_node = input_nodes.len();
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
        if let Some(node) = input_nodes.get_mut(first_node) {
            if let Some(system) = system_instruction
                .as_object()
                .filter(|system| system.contains_key("parts"))
            {
                let extra = split_extra(system, &["parts"]);
                if !extra.is_empty() {
                    node.extra_body_mut()
                        .insert(GEMINI_SYSTEM_EXTRA_KEY.into(), json!(extra));
                }
            }
        }
    }

    let mut resolved_results = std::collections::HashSet::new();
    if let Some(contents) = obj.get("contents").and_then(|v| v.as_array()) {
        for content in contents {
            let first_node = input_nodes.len();
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
            let message_extra = HashMap::new();
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
            if let Some(node) = input_nodes.get_mut(first_node) {
                node.extra_body_mut().insert(
                    GEMINI_CONTENT_EXTRA_KEY.into(),
                    json!(split_extra(content_obj, &["role", "parts"])),
                );
            }
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
    if obj
        .get("generationConfig")
        .and_then(|cfg| cfg.get("responseFormat"))
        .and_then(|format| format.get("text"))
        .and_then(|text| text.get("mimeType"))
        .and_then(Value::as_str)
        .is_some_and(|mime| matches!(mime, "TEXT_PLAIN" | "APPLICATION_JSON"))
    {
        request_extra.insert(GEMINI_TEXT_FORMAT_KEY.into(), json!(true));
    }
    if let Some(cfg) = request_extra
        .get_mut("generationConfig")
        .and_then(Value::as_object_mut)
    {
        for key in [
            "temperature",
            "topP",
            "maxOutputTokens",
            "stopSequences",
            "responseLogprobs",
            "logprobs",
            "topK",
            "seed",
            "presencePenalty",
            "frequencyPenalty",
        ] {
            cfg.remove(key);
        }
        strip_text_response_format(cfg);
        if matches!(
            cfg.get("responseMimeType").and_then(Value::as_str),
            Some("text/plain" | "application/json" | "text/x.enum")
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
        image_generation: None,
        sampling: obj
            .get("generationConfig")
            .and_then(Value::as_object)
            .and_then(|cfg| {
                let sampling = crate::urp::SamplingConfig {
                    top_k: cfg
                        .get("topK")
                        .and_then(Value::as_u64)
                        .and_then(|n| u32::try_from(n).ok()),
                    seed: cfg.get("seed").and_then(Value::as_i64),
                    presence_penalty: cfg.get("presencePenalty").and_then(Value::as_f64),
                    frequency_penalty: cfg.get("frequencyPenalty").and_then(Value::as_f64),
                };
                (sampling.top_k.is_some()
                    || sampling.seed.is_some()
                    || sampling.presence_penalty.is_some()
                    || sampling.frequency_penalty.is_some())
                .then_some(sampling)
            }),
        logprobs: obj
            .get("generationConfig")
            .and_then(Value::as_object)
            .and_then(|cfg| {
                let enabled = cfg.get("responseLogprobs").and_then(Value::as_bool);
                let top_k = cfg
                    .get("logprobs")
                    .and_then(Value::as_u64)
                    .and_then(|n| u32::try_from(n).ok());
                enabled
                    .or(top_k.map(|_| true))
                    .map(|enabled| crate::urp::LogprobConfig { enabled, top_k })
            }),
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
                    .filter(|effort| !effort.eq_ignore_ascii_case("THINKING_LEVEL_UNSPECIFIED"))
                    .map(str::to_ascii_lowercase),
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
        response_format: obj.get("generationConfig").and_then(decode_response_format),
        user: None,
        extra_body: request_extra,
    })
}

pub fn decode_response(value: &Value) -> Result<UrpResponse, String> {
    let obj = value
        .as_object()
        .ok_or_else(|| "gemini response must be object".to_string())?;

    if let Some(error) = native_error(value) {
        return Err(error);
    }
    let candidate = selected_candidate(value);
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
        crate::urp::logprobs::attach_gemini(candidate, &mut output_nodes);
    }
    let mut output_nodes: Vec<Node> = output_nodes
        .into_iter()
        .flat_map(project_response_node)
        .collect();
    if let Some(token) = candidate.and_then(continuation_token) {
        output_nodes.push(continuation_carrier(token));
    }
    let mut finish_reason = candidate
        .and_then(|candidate| candidate.get("finishReason"))
        .and_then(Value::as_str)
        .filter(|reason| !reason.is_empty() && *reason != "FINISH_REASON_UNSPECIFIED")
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
        outcome: response_outcome(candidate, finish_reason),
        id: obj
            .get("responseId")
            .or_else(|| obj.get("id"))
            .and_then(|v| v.as_str())
            .map(str::to_owned)
            .unwrap_or_else(|| format!("resp_{}", uuid::Uuid::new_v4())),
        model: obj
            .get("modelVersion")
            .or_else(|| obj.get("model"))
            .and_then(|v| v.as_str())
            .unwrap_or_default()
            .to_string(),
        // GenerateContentResponse has no creation time; record the decode time.
        created_at: Some(chrono::Utc::now().timestamp()),
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
                    &[
                        "name",
                        "description",
                        "parameters",
                        "parametersJsonSchema",
                        "response",
                        "responseJsonSchema",
                    ],
                );
                if decl.contains_key("parametersJsonSchema") || decl.contains_key("parameters") {
                    extra.insert(
                        "_monoize_gemini_parameters_json_schema".into(),
                        json!(decl.contains_key("parametersJsonSchema")),
                    );
                }
                if decl.contains_key("responseJsonSchema") || decl.contains_key("response") {
                    extra.insert(
                        "_monoize_gemini_response_json_schema".into(),
                        json!(decl.contains_key("responseJsonSchema")),
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
                        response_schema: decl.get("responseJsonSchema").cloned().or_else(|| {
                            decl.get("response")
                                .map(|schema| native_schema_types(schema, false))
                        }),
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
            logprobs: None,
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
pub(crate) const GEMINI_CONTENT_EXTRA_KEY: &str = "_monoize_gemini_content";
pub(crate) const GEMINI_SYSTEM_EXTRA_KEY: &str = "_monoize_gemini_system";
pub(crate) const GEMINI_TEXT_FORMAT_KEY: &str = "_monoize_gemini_text_response_format";
pub(crate) const GEMINI_ENUM_FORMAT_KEY: &str = "_monoize_gemini_enum_response";
pub(crate) const GEMINI_SYNTHETIC_CALL_PREFIX: &str = "call_gemini_";
/// Id prefix of Reasoning nodes that carry a Gemini-produced signature (provenance marker).
pub(crate) const GEMINI_SIGNATURE_ID_PREFIX: &str = "rs_gemini_";
/// Id prefix of a signature carrier bound to an owning text or media Part.
pub(crate) const GEMINI_PART_SIGNATURE_ID_PREFIX: &str = "rs_gemini_part_";
/// Id prefix of a carrier holding a whole server-tool Part (`toolCall` / `toolResponse`).
pub(crate) const GEMINI_ITEM_CARRIER_ID_PREFIX: &str = "rs_gemini_item_";
/// Id prefix of a carrier holding a `CONTINUATION` token.
pub(crate) const GEMINI_CONTINUATION_ID_PREFIX: &str = "rs_gemini_cont_";
pub(crate) const GEMINI_CONTINUATION_TOKEN_KEY: &str = "continuation_token";

fn carrier_id(prefix: &str) -> String {
    format!("{prefix}{}", uuid::Uuid::new_v4().simple())
}

/// Owner binding for a text signature: `rs_gemini_part_t<byte length>h<xxh3 hex>r<random>`.
/// It lets the encoder find the owning text after a downstream merged or moved Parts.
fn text_carrier_id(text: &str) -> String {
    format!(
        "{GEMINI_PART_SIGNATURE_ID_PREFIX}t{}h{:016x}r{}",
        text.len(),
        xxhash_rust::xxh3::xxh3_64(text.as_bytes()),
        &uuid::Uuid::new_v4().simple().to_string()[..8]
    )
}

/// Parses the owner binding of a text signature carrier id.
pub(crate) fn text_carrier_owner(id: &str) -> Option<(usize, u64)> {
    let rest = id.strip_prefix(GEMINI_PART_SIGNATURE_ID_PREFIX)?.strip_prefix('t')?;
    let (length, rest) = rest.split_once('h')?;
    let (hash, _) = rest.split_once('r')?;
    Some((length.parse().ok()?, u64::from_str_radix(hash, 16).ok()?))
}

fn reasoning_carrier(id: String, payload: Value, extra_body: HashMap<String, Value>) -> Node {
    Node::Reasoning {
        // Stream deltas expose only the metadata, so envelopes keep the carrier id through it.
        metadata: crate::urp::ReasoningMetadata {
            item_id: Some(id.clone()),
            ..Default::default()
        },
        id: Some(id),
        content: None,
        encrypted: Some(payload),
        summary: None,
        source: None,
        extra_body,
    }
}

/// The resumable `continuationToken` of a candidate that stopped with `CONTINUATION`.
pub(crate) fn continuation_token(candidate: &Map<String, Value>) -> Option<&str> {
    if candidate.get("finishReason").and_then(Value::as_str) != Some("CONTINUATION") {
        return None;
    }
    candidate
        .get("continuationToken")
        .and_then(Value::as_str)
        .filter(|token| !token.is_empty())
}

/// Carrier for a `CONTINUATION` token, appended after the partial output.
pub(crate) fn continuation_carrier(token: &str) -> Node {
    reasoning_carrier(
        carrier_id(GEMINI_CONTINUATION_ID_PREFIX),
        json!(token),
        HashMap::new(),
    )
}

/// Encodes a server-tool Part as an opaque carrier payload (unpadded base64url of its JSON).
fn item_carrier(body: &Value) -> Node {
    use base64::Engine as _;
    let wire = crate::urp::encode::sanitize_provider_item_wire_body(body);
    reasoning_carrier(
        carrier_id(GEMINI_ITEM_CARRIER_ID_PREFIX),
        json!(base64::engine::general_purpose::URL_SAFE_NO_PAD.encode(wire.to_string())),
        HashMap::new(),
    )
}

/// Decodes an item carrier payload back into the original Part.
pub(crate) fn item_carrier_part(payload: &Value) -> Option<Value> {
    use base64::Engine as _;
    let bytes = base64::engine::general_purpose::URL_SAFE_NO_PAD
        .decode(payload.as_str()?)
        .ok()?;
    serde_json::from_slice::<Value>(&bytes)
        .ok()
        .filter(Value::is_object)
}

/// Renders a Gemini code-execution Part as assistant text so non-Gemini clients can see it.
fn render_code_execution(node: Node) -> Node {
    let Node::ProviderItem {
        origin_protocol: ProviderProtocol::Gemini,
        role,
        item_type,
        body,
        ..
    } = &node
    else {
        return node;
    };
    let content = match item_type.as_str() {
        "executableCode" => {
            let code = &body["executableCode"];
            format!(
                "\n```{}\n{}\n```\n",
                code.get("language")
                    .and_then(Value::as_str)
                    .filter(|language| *language != "LANGUAGE_UNSPECIFIED")
                    .unwrap_or("")
                    .to_ascii_lowercase(),
                code.get("code")
                    .and_then(Value::as_str)
                    .unwrap_or_default()
                    .trim_end_matches('\n')
            )
        }
        "codeExecutionResult" => {
            let result = &body["codeExecutionResult"];
            let outcome = result
                .get("outcome")
                .and_then(Value::as_str)
                .filter(|outcome| *outcome != "OUTCOME_OK")
                .map(|outcome| format!("[{outcome}]\n"))
                .unwrap_or_default();
            format!(
                "\n```output\n{outcome}{}\n```\n",
                result
                    .get("output")
                    .and_then(Value::as_str)
                    .unwrap_or_default()
                    .trim_end_matches('\n')
            )
        }
        _ => return node,
    };
    Node::Text {
        signature: body.get("thoughtSignature").cloned(),
        id: None,
        role: *role,
        content,
        phase: None,
        logprobs: None,
        citations: Vec::new(),
        extra_body: HashMap::new(),
    }
}

/// Canonical response form for one decoded Gemini Part node.
/// Text and media Part signatures move into a following Reasoning carrier with id prefix
/// `rs_gemini_part_` (text carriers also record their owner binding); a signed empty text Part
/// becomes one standalone carrier with id prefix `rs_gemini_`; a signed thought receives an
/// `rs_gemini_` id; server-tool Parts become `rs_gemini_item_` carriers. Function-call
/// signatures stay on the ToolCall.
pub(crate) fn project_response_node(node: Node) -> Vec<Node> {
    if let Node::ProviderItem {
        origin_protocol: ProviderProtocol::Gemini,
        item_type,
        body,
        ..
    } = &node
        && matches!(item_type.as_str(), "toolCall" | "toolResponse")
    {
        return vec![item_carrier(body)];
    }
    let mut node = render_code_execution(node);
    let (signature, carrier) = match &mut node {
        Node::Text {
            content,
            signature,
            citations,
            logprobs,
            extra_body,
            ..
        } => {
            if signature.is_some() && content.is_empty() && citations.is_empty() && logprobs.is_none()
            {
                let signature = signature.take().expect("checked signature");
                return vec![reasoning_carrier(
                    carrier_id(GEMINI_SIGNATURE_ID_PREFIX),
                    signature,
                    std::mem::take(extra_body),
                )];
            }
            let signature = signature.take();
            let carrier = signature
                .as_ref()
                .map(|_| text_carrier_id(content))
                .unwrap_or_default();
            (signature, carrier)
        }
        Node::Image { metadata, .. } | Node::Audio { metadata, .. } | Node::File { metadata, .. } => {
            (
                metadata.signature.take(),
                carrier_id(GEMINI_PART_SIGNATURE_ID_PREFIX),
            )
        }
        Node::Reasoning {
            id: id @ None,
            encrypted: Some(_),
            ..
        } => {
            *id = Some(carrier_id(GEMINI_SIGNATURE_ID_PREFIX));
            (None, String::new())
        }
        _ => (None, String::new()),
    };
    match signature {
        Some(signature) => vec![node, reasoning_carrier(carrier, signature, HashMap::new())],
        None => vec![node],
    }
}

/// True for a Reasoning node that carries only a signature (no thought text).
pub(crate) fn is_signature_carrier(node: &Node) -> bool {
    matches!(
        node,
        Node::Reasoning {
            content,
            summary,
            encrypted: Some(_),
            ..
        } if content.as_deref().is_none_or(str::is_empty)
            && summary.as_deref().is_none_or(str::is_empty)
    )
}

/// Restores a custom tool call that was bridged through a Gemini function declaration.
pub(crate) fn restore_custom_bridge_call(
    node: &mut Node,
    names: &std::collections::HashSet<String>,
) {
    let Node::ToolCall {
        tool_type,
        name,
        arguments,
        ..
    } = node
    else {
        return;
    };
    if *tool_type != crate::urp::ToolCallType::Function || !names.contains(name) {
        return;
    }
    let Ok(Value::Object(object)) = serde_json::from_str::<Value>(arguments) else {
        return;
    };
    let Some(input) = object.get("input").and_then(Value::as_str) else {
        return;
    };
    *tool_type = crate::urp::ToolCallType::Custom;
    *arguments = input.to_string();
}
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
                    content: None,
                    encrypted: obj.get("thoughtSignature").cloned(),
                    summary: Some(text.to_string()),
                    source: None,
                    extra_body: part_extra(obj, &["text", "thought", "thoughtSignature"]),
                }
            } else {
                Part::Text {
                    logprobs: None,
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
                                node.extra_body_mut()
                                    .insert(GEMINI_CONTENT_EXTRA_KEY.into(), json!(extra));
                            }
                        }
                        output_nodes.push(node);
                    }
                }
                DecodedOutput::ToolResult(mut node) => {
                    if !did_attach_content_extra {
                        let extra =
                            take_output_extra(&content_extra, &mut did_attach_content_extra);
                        if !extra.is_empty() {
                            node.extra_body_mut()
                                .insert(GEMINI_CONTENT_EXTRA_KEY.into(), json!(extra));
                        }
                    }
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
        // CONTINUATION stops at the per-request token cap; the answer is resumable, not failed.
        "MAX_TOKENS" | "CONTINUATION" => FinishReason::Length,
        "SAFETY"
        | "RECITATION"
        | "LANGUAGE"
        | "BLOCKLIST"
        | "PROHIBITED_CONTENT"
        | "SPII"
        | "IMAGE_SAFETY"
        | "IMAGE_PROHIBITED_CONTENT"
        | "IMAGE_RECITATION"
        | "ESCALATION"
        | "PUP_LIMITED_DISABLED" => FinishReason::ContentFilter,
        "STOP" => FinishReason::Stop,
        // OTHER, MALFORMED_FUNCTION_CALL, UNEXPECTED_TOOL_CALL, TOO_MANY_TOOL_CALLS,
        // MISSING_THOUGHT_SIGNATURE, MALFORMED_RESPONSE, IMAGE_OTHER, NO_IMAGE and unknown values.
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
    let mut extra = split_extra(
        candidate,
        &[
            "content",
            "finishReason",
            "citationMetadata",
            "logprobsResult",
            "avgLogprobs",
            "groundingMetadata",
        ],
    );
    if let Some(metadata) = crate::urp::citations::gemini_metadata_extra(candidate) {
        extra.insert("groundingMetadata".into(), metadata);
    }
    if let Some(citations) = candidate.get("citationMetadata").and_then(Value::as_object) {
        let unknown = split_extra(citations, &["citationSources"]);
        if !unknown.is_empty() {
            extra.insert("citationMetadata".into(), json!(unknown));
        }
    }
    extra
}

pub(crate) fn attach_candidate_citations(candidate: &Map<String, Value>, nodes: &mut [Node]) {
    crate::urp::citations::attach_gemini(candidate, nodes);
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
        logprobs: None,
        id: None,
        content: format!("Gemini blocked the prompt: {reason}"),
        extra_body: HashMap::new(),
    }
}

pub(crate) fn native_schema_types(schema: &Value, uppercase: bool) -> Value {
    let Some(object) = schema.as_object() else {
        return schema.clone();
    };
    let mut result = object.clone();
    if let Some(kind) = object.get("type").and_then(Value::as_str) {
        result.insert(
            "type".into(),
            json!(if uppercase {
                kind.to_ascii_uppercase()
            } else {
                kind.to_ascii_lowercase()
            }),
        );
    }
    for key in [
        "properties",
        "patternProperties",
        "$defs",
        "definitions",
        "dependentSchemas",
    ] {
        if let Some(properties) = object.get(key).and_then(Value::as_object) {
            result.insert(
                key.into(),
                Value::Object(
                    properties
                        .iter()
                        .map(|(name, schema)| {
                            (name.clone(), native_schema_types(schema, uppercase))
                        })
                        .collect(),
                ),
            );
        }
    }
    for key in [
        "items",
        "additionalProperties",
        "additionalItems",
        "contains",
        "not",
        "if",
        "then",
        "else",
        "propertyNames",
        "unevaluatedProperties",
        "unevaluatedItems",
    ] {
        if let Some(schema) = object.get(key) {
            result.insert(
                key.into(),
                if let Some(items) = schema.as_array() {
                    Value::Array(
                        items
                            .iter()
                            .map(|schema| native_schema_types(schema, uppercase))
                            .collect(),
                    )
                } else {
                    native_schema_types(schema, uppercase)
                },
            );
        }
    }
    for key in ["anyOf", "allOf", "oneOf", "prefixItems"] {
        if let Some(items) = object.get(key).and_then(Value::as_array) {
            result.insert(
                key.into(),
                Value::Array(
                    items
                        .iter()
                        .map(|schema| native_schema_types(schema, uppercase))
                        .collect(),
                ),
            );
        }
    }
    Value::Object(result)
}

pub(crate) fn selected_candidate(value: &Value) -> Option<&Map<String, Value>> {
    let candidates = value.get("candidates")?.as_array()?;
    candidates
        .iter()
        .find(|candidate| candidate.get("index").and_then(Value::as_u64) == Some(0))
        .or_else(|| candidates.first())?
        .as_object()
}

pub(crate) fn native_error(value: &Value) -> Option<String> {
    value
        .get("error")
        .filter(|error| !error.is_null())
        .map(|error| {
            error
                .get("message")
                .and_then(Value::as_str)
                .map(str::to_owned)
                .unwrap_or_else(|| format!("Gemini API error: {error}"))
        })
}

pub(crate) fn response_outcome(
    candidate: Option<&Map<String, Value>>,
    finish: Option<FinishReason>,
) -> Option<ResponseOutcome> {
    let mut outcome = ResponseOutcome::from_finish(Some(finish?));
    let native_reason = candidate
        .and_then(|candidate| candidate.get("finishReason"))
        .and_then(Value::as_str);
    if let Some(token) = candidate.and_then(continuation_token) {
        outcome
            .incomplete_extra
            .insert(GEMINI_CONTINUATION_TOKEN_KEY.into(), json!(token));
    }
    if finish == Some(FinishReason::Other) {
        outcome.error = Some(crate::urp::outcome::ResponseError {
            code: native_reason.map(str::to_owned),
            message: candidate
                .and_then(|candidate| candidate.get("finishMessage"))
                .and_then(Value::as_str)
                .map(str::to_owned),
            extra_body: HashMap::new(),
        });
    }
    Some(outcome)
}

fn decode_response_format(config: &Value) -> Option<ResponseFormat> {
    let modern = config
        .get("responseFormat")
        .and_then(|format| format.get("text"));
    let mime = modern
        .and_then(|format| format.get("mimeType"))
        .and_then(Value::as_str)
        .or_else(|| config.get("responseMimeType").and_then(Value::as_str))?;
    match mime {
        "text/plain" | "TEXT_PLAIN" => return Some(ResponseFormat::Text),
        "application/json" | "APPLICATION_JSON" | "text/x.enum" => {}
        _ => return None,
    }
    let schema = modern
        .and_then(|format| format.get("schema"))
        .or_else(|| config.get("responseJsonSchema"))
        .or_else(|| config.get("responseSchema"));
    Some(match schema {
        Some(schema) => {
            let native = modern.and_then(|format| format.get("schema")).is_none()
                && config.get("responseJsonSchema").is_none();
            ResponseFormat::JsonSchema {
                json_schema: JsonSchemaDefinition {
                    name: "gemini_response".into(),
                    description: None,
                    schema: if native {
                        native_schema_types(schema, false)
                    } else {
                        schema.clone()
                    },
                    strict: None,
                    extra_body: if mime == "text/x.enum" {
                        HashMap::from([(GEMINI_ENUM_FORMAT_KEY.into(), json!(true))])
                    } else {
                        HashMap::new()
                    },
                },
            }
        }
        None if mime == "text/x.enum" => ResponseFormat::JsonSchema {
            json_schema: JsonSchemaDefinition {
                name: "gemini_response".into(),
                description: None,
                schema: json!({"type":"string"}),
                strict: None,
                extra_body: HashMap::from([(GEMINI_ENUM_FORMAT_KEY.into(), json!(true))]),
            },
        },
        None => ResponseFormat::JsonObject,
    })
}

pub(crate) fn strip_text_response_format(config: &mut Map<String, Value>) {
    if let Some(format) = config
        .get_mut("responseFormat")
        .and_then(Value::as_object_mut)
    {
        if let Some(text) = format.get_mut("text").and_then(Value::as_object_mut) {
            if text
                .get("mimeType")
                .and_then(Value::as_str)
                .is_some_and(|mime| {
                    matches!(mime, "TEXT_PLAIN" | "APPLICATION_JSON" | "text/x.enum")
                })
            {
                text.remove("mimeType");
                text.remove("schema");
            }
            if text.is_empty() {
                format.remove("text");
            }
        }
        if format.is_empty() {
            config.remove("responseFormat");
        }
    }
}
