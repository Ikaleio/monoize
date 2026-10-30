use crate::urp::decode::gemini::{
    GEMINI_CANDIDATE_EXTRA_KEY, GEMINI_CONTENT_EXTRA_KEY, GEMINI_ENUM_FORMAT_KEY,
    GEMINI_PART_EXTRA_KEY, GEMINI_PART_SIGNATURE_ID_PREFIX, GEMINI_SIGNATURE_ID_PREFIX,
    GEMINI_SYNTHETIC_CALL_PREFIX, GEMINI_SYSTEM_EXTRA_KEY, GEMINI_TEXT_FORMAT_KEY,
    native_schema_types, strip_text_response_format,
};
use crate::urp::encode::{
    merge_extra, sanitize_provider_item_wire_body, usage_input_details, usage_output_details,
};
use crate::urp::{
    AudioSource, FileSource, FinishReason, FunctionDefinition, ImageSource, Node, OrdinaryRole,
    ProviderProtocol, ResponseFormat, StopControl, ToolChoice, ToolDefinition, ToolResultContent,
    UrpRequest, UrpResponse,
};
use serde_json::{Map, Value, json};
use std::collections::HashMap;

pub fn encode_request(req: &UrpRequest, upstream_model: &str) -> Value {
    encode_request_checked(req, upstream_model)
        .unwrap_or_else(|message| crate::urp::media::error_body(&message))
}

pub fn encode_request_checked(req: &UrpRequest, upstream_model: &str) -> Result<Value, String> {
    if req
        .logprobs
        .as_ref()
        .is_some_and(|config| config.enabled && config.top_k.is_some_and(|count| count > 20))
    {
        return Err("Gemini logprobs must be in the range 0 through 20.".into());
    }
    if matches!(&req.stop, Some(StopControl::Multiple(stops)) if stops.len() > 5) {
        return Err("Gemini accepts at most 5 stop sequences.".into());
    }
    for tool in req.tools.as_deref().unwrap_or_default() {
        if tool.tool_type != "function"
            && tool.origin_protocol != Some(ProviderProtocol::Gemini)
            && !is_gemini_mappable_tool(&tool.tool_type)
        {
            return Err(format!(
                "Gemini has no equivalent for tool type {}.",
                tool.tool_type
            ));
        }
    }
    let prepared = crate::urp::media::prepare_request(req, ProviderProtocol::Gemini)?;
    validate_media_nodes(&prepared.input)?;
    for node in &prepared.input {
        if matches!(node, Node::Image { role, .. } | Node::Audio { role, .. } | Node::File { role, .. } | Node::ProviderItem { role, .. }
            if matches!(role, OrdinaryRole::System | OrdinaryRole::Developer))
        {
            return Err(
                "Gemini system instructions support text only; media cannot be discarded".into(),
            );
        }
    }
    Ok(encode_prepared_request(&prepared, upstream_model))
}

fn encode_prepared_request(req: &UrpRequest, upstream_model: &str) -> Value {
    let mut contents = Vec::new();
    let mut system_parts = Vec::new();
    let mut tool_names_by_call_id: Map<String, Value> = Map::new();
    let request_nodes = &req.input;

    for node in request_nodes {
        if let Node::ToolCall { call_id, name, .. } = node {
            tool_names_by_call_id
                .entry(call_id.clone())
                .or_insert_with(|| Value::String(name.clone()));
        }
    }

    let model = GeminiModel::parse(upstream_model);
    let mut pending_content: Option<GeminiMessageEnvelope> = None;
    let mut system_extra = HashMap::new();
    for node in request_nodes {
        // Gemini's decoder never emits envelope controls, so every control is foreign: it
        // neither splits Content nor adds Content members.
        if matches!(node, Node::NextDownstreamEnvelopeExtra { .. }) {
            continue;
        }
        let encoded = if matches!(node, Node::ToolResult { .. }) {
            encode_tool_result(node, &tool_names_by_call_id)
                .map(|part| (OrdinaryRole::User, part, content_extra(node)))
        } else {
            encode_request_node_part(node)
        };
        let Some((role, mut part, extra)) = encoded else {
            continue;
        };
        if model.media_resolution {
            add_media_resolution(node, &mut part);
        }
        if node_extra(node).contains_key(GEMINI_CONTENT_EXTRA_KEY) {
            flush_pending_gemini_message(&mut pending_content, &mut contents);
        }
        if matches!(role, OrdinaryRole::System | OrdinaryRole::Developer) {
            flush_pending_gemini_message(&mut pending_content, &mut contents);
            if let Some(native) = node_extra(node)
                .get(GEMINI_SYSTEM_EXTRA_KEY)
                .and_then(Value::as_object)
            {
                system_extra.extend(
                    native
                        .iter()
                        .map(|(key, value)| (key.clone(), value.clone())),
                );
            }
            system_extra.extend(extra);
            system_parts.push(part);
            continue;
        }
        if pending_content.as_ref().is_some_and(|pending| {
            pending.role != role || (!extra.is_empty() && pending.extra_body != extra)
        }) {
            flush_pending_gemini_message(&mut pending_content, &mut contents);
        }
        let pending = pending_content.get_or_insert_with(|| GeminiMessageEnvelope {
            role,
            parts: Vec::new(),
            extra_body: extra,
        });
        pending.parts.push(part);
    }
    flush_pending_gemini_message(&mut pending_content, &mut contents);
    if model.requires_call_signatures {
        sign_current_turn_calls(&mut contents);
    }

    let mut body = json!({ "contents": contents });
    let obj = body.as_object_mut().expect("gemini request object");
    if !system_parts.is_empty() {
        let mut system = json!({"parts": system_parts});
        merge_extra(system.as_object_mut().unwrap(), &system_extra);
        obj.insert("systemInstruction".into(), system);
    }

    let mut generation_config = req
        .extra_body
        .get("generationConfig")
        .and_then(Value::as_object)
        .cloned()
        .unwrap_or_default();
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
        generation_config.remove(key);
    }
    strip_text_response_format(&mut generation_config);
    if let Some(sampling) = &req.sampling {
        for (key, value) in [
            ("topK", sampling.top_k.map(Value::from)),
            ("seed", sampling.seed.map(Value::from)),
            (
                "presencePenalty",
                sampling.presence_penalty.map(Value::from),
            ),
            (
                "frequencyPenalty",
                sampling.frequency_penalty.map(Value::from),
            ),
        ] {
            if let Some(value) = value {
                generation_config.insert(key.into(), value);
            }
        }
    }
    if let Some(config) = &req.logprobs {
        generation_config.insert("responseLogprobs".into(), json!(config.enabled));
        if config.enabled {
            if let Some(count) = config.top_k {
                generation_config.insert("logprobs".into(), json!(count));
            }
        }
    }
    if req.response_format.is_some()
        || matches!(
            generation_config
                .get("responseMimeType")
                .and_then(Value::as_str),
            Some("text/plain" | "application/json" | "text/x.enum")
        )
    {
        for key in ["responseMimeType", "responseJsonSchema", "responseSchema"] {
            generation_config.remove(key);
        }
    }
    if let Some(temp) = req.temperature {
        generation_config.insert("temperature".to_string(), Value::from(temp));
    }
    if let Some(top_p) = req.top_p {
        generation_config.insert("topP".to_string(), Value::from(top_p));
    }
    if let Some(max_tokens) = req.max_output_tokens {
        generation_config.insert("maxOutputTokens".to_string(), Value::from(max_tokens));
    }
    if let Some(stop) = &req.stop {
        generation_config.insert(
            "stopSequences".to_string(),
            match stop {
                StopControl::Single(stop) => json!([stop]),
                StopControl::Multiple(stops) => json!(stops),
            },
        );
    }
    if let Some(format) = &req.response_format {
        let enum_format = matches!(format, ResponseFormat::JsonSchema { json_schema } if json_schema.extra_body.get(GEMINI_ENUM_FORMAT_KEY).and_then(Value::as_bool) == Some(true));
        generation_config.remove("responseSchema");
        generation_config.remove("responseJsonSchema");
        generation_config.insert(
            "responseMimeType".to_string(),
            json!(if matches!(format, ResponseFormat::Text) {
                "text/plain"
            } else if enum_format {
                "text/x.enum"
            } else {
                "application/json"
            }),
        );
        if let ResponseFormat::JsonSchema { json_schema } = format {
            generation_config.insert(
                if enum_format {
                    "responseSchema"
                } else {
                    "responseJsonSchema"
                }
                .into(),
                if enum_format {
                    native_schema_types(&json_schema.schema, true)
                } else {
                    json_schema.schema.clone()
                },
            );
        }
        if req
            .extra_body
            .get(GEMINI_TEXT_FORMAT_KEY)
            .and_then(Value::as_bool)
            == Some(true)
            && !enum_format
        {
            generation_config.remove("responseMimeType");
            generation_config.remove("responseJsonSchema");
            let native = generation_config
                .entry("responseFormat")
                .or_insert_with(|| json!({}));
            if !native.is_object() {
                *native = json!({});
            }
            let text = native
                .as_object_mut()
                .unwrap()
                .entry("text")
                .or_insert_with(|| json!({}));
            if !text.is_object() {
                *text = json!({});
            }
            text["mimeType"] = json!(if matches!(format, ResponseFormat::Text) {
                "TEXT_PLAIN"
            } else {
                "APPLICATION_JSON"
            });
            if let ResponseFormat::JsonSchema { json_schema } = format {
                text["schema"] = json_schema.schema.clone();
            }
        }
    }
    let mut thinking = generation_config
        .remove("thinkingConfig")
        .and_then(|v| v.as_object().cloned())
        .unwrap_or_default();
    for key in ["thinkingLevel", "thinkingBudget", "includeThoughts"] {
        thinking.remove(key);
    }
    if let Some(reasoning) = &req.reasoning {
        thinking.extend(model.thinking_config(reasoning));
    }
    if !thinking.is_empty() {
        generation_config.insert("thinkingConfig".into(), Value::Object(thinking));
    }
    let image_tool = req.tools.as_deref().and_then(|tools| {
        tools.iter().find(|tool| {
            tool.origin_protocol != Some(ProviderProtocol::Gemini)
                && tool.tool_type == "image_generation"
        })
    });
    // Explicit Images requests always ask for image output; a declared image_generation tool
    // only applies to image-capable models and is otherwise a dropped definition.
    let image_size = match (&req.image_generation, image_tool) {
        (Some(options), _) => Some(options.size.clone()),
        (None, Some(tool)) if model.image_output => Some(
            tool.config
                .as_ref()
                .and_then(|config| config.get("size"))
                .or_else(|| tool.extra_body.get("size"))
                .and_then(Value::as_str)
                .map(str::to_owned),
        ),
        _ => None,
    };
    if let Some(size) = image_size {
        generation_config
            .entry("responseModalities")
            .or_insert_with(|| json!(["TEXT", "IMAGE"]));
        if let Some(config) = size
            .as_deref()
            .and_then(|size| image_config_from_size(&model, size))
        {
            let native = generation_config
                .entry("imageConfig")
                .or_insert_with(|| json!({}));
            if !native.is_object() {
                *native = json!({});
            }
            let native = native.as_object_mut().expect("image config object");
            for (key, value) in config {
                native.entry(key).or_insert(value);
            }
        }
    }
    if !generation_config.is_empty() {
        obj.insert(
            "generationConfig".to_string(),
            Value::Object(generation_config),
        );
    }

    if let Some(tools) = &req.tools {
        let mut encoded = Vec::new();
        let declarations = encode_function_declarations(tools);
        if !declarations.is_empty() {
            encoded.push(json!({"functionDeclarations":declarations}));
        }
        let mut builtin = Vec::new();
        for tool in tools {
            if tool.origin_protocol == Some(ProviderProtocol::Gemini) {
                if let Some(config) = &tool.config {
                    encoded.push(json!({tool.tool_type.clone():config}));
                }
            } else if let Some(key) = gemini_builtin_tool_key(&tool.tool_type)
                && !builtin.contains(&key)
            {
                builtin.push(key);
                encoded.push(json!({ key: {} }));
            }
        }
        if !encoded.is_empty() {
            obj.insert("tools".into(), json!(encoded));
        }
    }
    let mut config = req
        .extra_body
        .get("toolConfig")
        .and_then(Value::as_object)
        .cloned()
        .unwrap_or_default();
    let mut function_config = config
        .remove("functionCallingConfig")
        .and_then(|value| value.as_object().cloned())
        .unwrap_or_default();
    function_config.remove("mode");
    function_config.remove("allowedFunctionNames");
    // Function-calling modes constrain function declarations only; without declarations a
    // typed choice has nothing to select and is omitted.
    let has_declarations = req.tools.as_deref().is_some_and(|tools| {
        tools
            .iter()
            .any(|tool| tool.tool_type == "function" && tool.function.is_some())
    });
    if let Some(choice) = req
        .tool_choice
        .as_ref()
        .filter(|_| has_declarations)
        .and_then(encode_tool_choice)
        .and_then(|value| value.as_object().cloned())
    {
        function_config.extend(choice);
    }
    if !function_config.is_empty() {
        config.insert("functionCallingConfig".into(), json!(function_config));
    }
    if !config.is_empty() {
        obj.insert("toolConfig".into(), json!(config));
    }
    if let Some(tier) = req
        .extra_body
        .get("service_tier")
        .and_then(Value::as_str)
        .and_then(gemini_service_tier)
    {
        obj.insert("serviceTier".into(), json!(tier));
    }
    let extra = req
        .extra_body
        .iter()
        .filter(|(key, _)| {
            !matches!(
                key.as_str(),
                "generationConfig"
                    | "tools"
                    | "toolConfig"
                    | "contents"
                    | "systemInstruction"
                    | "service_tier"
            )
        })
        .map(|(key, value)| (key.clone(), value.clone()))
        .collect();
    merge_extra(obj, &extra);

    if !upstream_model.is_empty() {
        obj.remove("model");
    }

    body
}

pub fn encode_response(resp: &UrpResponse, logical_model: &str) -> Value {
    encode_response_checked(resp, logical_model)
        .unwrap_or_else(|message| crate::urp::media::error_body(&message))
}

pub fn encode_response_checked(resp: &UrpResponse, logical_model: &str) -> Result<Value, String> {
    if let Some(body) = resp
        .outcome
        .as_ref()
        .and_then(|outcome| outcome.failure_body(false))
    {
        return Ok(body);
    }
    let mut prepared = resp.clone();
    prepared.output = crate::urp::media::prepare_nodes(&resp.output, ProviderProtocol::Gemini)?;
    validate_media_nodes(&prepared.output)?;
    for node in &prepared.output {
        if node
            .role()
            .is_some_and(|role| role != OrdinaryRole::Assistant)
        {
            return Err("Gemini response content must have the assistant role".into());
        }
    }
    Ok(encode_prepared_response(&prepared, logical_model))
}

pub(crate) fn encode_response_node_parts(node: &Node) -> Result<Vec<Value>, String> {
    let prepared =
        crate::urp::media::prepare_nodes(std::slice::from_ref(node), ProviderProtocol::Gemini)?;
    validate_media_nodes(&prepared)?;
    let mut parts = Vec::new();
    for node in &prepared {
        if node
            .role()
            .is_some_and(|role| role != OrdinaryRole::Assistant)
        {
            return Err("Gemini response content must have the assistant role".into());
        }
        if let Some((_, part, _)) = encode_request_node_part(node) {
            if !part.is_null() {
                parts.push(part);
            }
        }
    }
    // A single streamed node has no adjacent Part to bind; carriers stay standalone.
    resolve_bound_signatures(&mut parts);
    Ok(parts)
}

fn encode_prepared_response(resp: &UrpResponse, logical_model: &str) -> Value {
    let mut parts = Vec::new();
    let mut emitted_nodes = Vec::new();
    for node in &resp.output {
        if let Some((OrdinaryRole::Assistant, part, _)) = encode_request_node_part(node) {
            if !part.is_null() {
                parts.push(part);
                emitted_nodes.push(node.clone());
            }
        }
    }
    for removed in resolve_bound_signatures(&mut parts) {
        emitted_nodes.remove(removed);
    }
    let mut usage_metadata = json!({
        "promptTokenCount": 0,
        "candidatesTokenCount": 0,
        "totalTokenCount": 0,
        "thoughtsTokenCount": 0,
        "cachedContentTokenCount": 0,
        "toolUsePromptTokenCount": 0
    });
    if let Some(usage) = &resp.usage {
        let usage = usage.accounting();
        if let Some(obj) = usage_metadata.as_object_mut() {
            let input_details = usage_input_details(&usage);
            let output_details = usage_output_details(&usage);
            for (key, details) in [
                (
                    "promptTokensDetails",
                    input_details.modality_breakdown.as_ref(),
                ),
                (
                    "cacheTokensDetails",
                    input_details.cache_read_modality_breakdown.as_ref(),
                ),
                (
                    "toolUsePromptTokensDetails",
                    input_details.tool_prompt_modality_breakdown.as_ref(),
                ),
                (
                    "candidatesTokensDetails",
                    output_details.modality_breakdown.as_ref(),
                ),
            ] {
                if let Some(details) = details {
                    obj.insert(key.into(), encode_modality(details));
                }
            }
            obj.insert(
                "promptTokenCount".to_string(),
                Value::from(
                    usage
                        .input_tokens
                        .saturating_sub(input_details.tool_prompt_tokens),
                ),
            );
            obj.insert(
                "candidatesTokenCount".to_string(),
                Value::from(
                    usage
                        .output_tokens
                        .saturating_sub(output_details.reasoning_tokens),
                ),
            );
            obj.insert(
                "totalTokenCount".to_string(),
                Value::from(usage.total_tokens()),
            );
            obj.insert(
                "thoughtsTokenCount".to_string(),
                Value::from(output_details.reasoning_tokens),
            );
            obj.insert(
                "cachedContentTokenCount".to_string(),
                Value::from(input_details.cache_read_tokens),
            );
            obj.insert(
                "toolUsePromptTokenCount".to_string(),
                Value::from(input_details.tool_prompt_tokens),
            );
            // Counters absent from UsageMetadata are extensions, emitted only when non-zero.
            for (key, count) in [
                (
                    "cacheCreationTokenCount",
                    input_details.cache_creation_tokens,
                ),
                (
                    "acceptedPredictionOutputTokenCount",
                    output_details.accepted_prediction_tokens,
                ),
                (
                    "rejectedPredictionOutputTokenCount",
                    output_details.rejected_prediction_tokens,
                ),
            ] {
                if count > 0 {
                    obj.insert(key.to_string(), Value::from(count));
                }
            }
            for (k, v) in &usage.extra_body {
                if !k.starts_with("_monoize_")
                    && !matches!(
                        k.as_str(),
                        "promptTokensDetails"
                            | "cacheTokensDetails"
                            | "candidatesTokensDetails"
                            | "toolUsePromptTokensDetails"
                    )
                {
                    obj.entry(k.clone()).or_insert_with(|| v.clone());
                }
            }
        }
    }
    let mut body = json!({
        "candidates": [{
            "index": 0,
            "content": {
                "role": "model",
                "parts": parts,
            },
        }],
        "usageMetadata": usage_metadata,
        "modelVersion": logical_model,
        "responseId": resp.id,
    });
    if let Some(reason) = response_finish_reason(resp) {
        body["candidates"][0]["finishReason"] = json!(finish_reason_to_gemini(Some(reason)));
    }
    for node in &resp.output {
        if let Some(content) = body["candidates"][0]["content"].as_object_mut() {
            merge_extra(content, &content_extra(node));
        }
    }
    if resp.usage.is_none() {
        body.as_object_mut().unwrap().remove("usageMetadata");
    }
    if let Some(metadata) = resp
        .extra_body
        .get(GEMINI_CANDIDATE_EXTRA_KEY)
        .and_then(Value::as_object)
    {
        let candidate = body["candidates"][0].as_object_mut().unwrap();
        for (key, value) in metadata {
            if !matches!(
                key.as_str(),
                "content" | "finishReason" | "logprobsResult" | "avgLogprobs"
            ) {
                candidate
                    .entry(key.clone())
                    .or_insert_with(|| value.clone());
            }
        }
    }
    let citations = crate::urp::citations::encode_gemini(&emitted_nodes);
    if !citations.is_empty() {
        body["candidates"][0]["citationMetadata"]["citationSources"] = json!(citations);
    } else if let Some(metadata) = body["candidates"][0]
        .get_mut("citationMetadata")
        .and_then(Value::as_object_mut)
    {
        metadata.remove("citationSources");
    }
    if let Some(metadata) = body["candidates"][0]
        .get_mut("groundingMetadata")
        .and_then(Value::as_object_mut)
    {
        if metadata.contains_key("groundingSupports") {
            metadata.remove("groundingChunks");
        }
        metadata.remove("groundingSupports");
    }
    if let Some(Value::Object(grounding)) =
        crate::urp::citations::encode_gemini_grounding(&emitted_nodes)
    {
        let candidate = body["candidates"][0].as_object_mut().unwrap();
        let metadata = candidate
            .entry("groundingMetadata")
            .or_insert_with(|| json!({}));
        if !metadata.is_object() {
            *metadata = json!({});
        }
        metadata.as_object_mut().unwrap().extend(grounding);
    }
    if let Some(scores) = crate::urp::logprobs::encode_gemini(&emitted_nodes) {
        body["candidates"][0]["logprobsResult"] = scores;
        if let Some(average) = crate::urp::logprobs::gemini_summary(&emitted_nodes) {
            body["candidates"][0]["avgLogprobs"] = json!(average);
        }
    }

    if let Some(obj) = body.as_object_mut() {
        merge_extra(obj, &resp.extra_body);
    }
    if is_prompt_block_response(resp) {
        body["candidates"] = json!([]);
    } else if let Some(feedback) = body
        .get_mut("promptFeedback")
        .and_then(Value::as_object_mut)
    {
        feedback.remove("blockReason");
    }
    body
}

pub(crate) fn is_prompt_block_response(resp: &UrpResponse) -> bool {
    if response_finish_reason(resp) != Some(FinishReason::ContentFilter) {
        return false;
    }
    let Some(reason) = resp
        .extra_body
        .get("promptFeedback")
        .and_then(|value| value.get("blockReason"))
        .and_then(Value::as_str)
    else {
        return false;
    };
    matches!(resp.output.as_slice(), [Node::Refusal { content, .. }] if content == &format!("Gemini blocked the prompt: {reason}"))
}

fn encode_function_declarations(tools: &[ToolDefinition]) -> Vec<Value> {
    let mut out = Vec::new();
    for tool in tools {
        if tool.tool_type != "function" {
            continue;
        }
        let Some(function) = &tool.function else {
            continue;
        };
        out.push(encode_function_declaration(function));
    }
    out
}

fn encode_function_declaration(function: &FunctionDefinition) -> Value {
    let mut obj = Map::new();
    obj.insert("name".to_string(), Value::String(function.name.clone()));
    // FunctionDeclaration.description is a required field.
    obj.insert(
        "description".to_string(),
        Value::String(function.description.clone().unwrap_or_default()),
    );
    if let Some(params) = &function.parameters {
        let key = if function
            .extra_body
            .get("_monoize_gemini_parameters_json_schema")
            .and_then(Value::as_bool)
            == Some(false)
        {
            "parameters"
        } else {
            "parametersJsonSchema"
        };
        obj.insert(
            key.into(),
            if key == "parameters" {
                native_schema_types(params, true)
            } else {
                params.clone()
            },
        );
    }
    if let Some(schema) = &function.response_schema {
        let native = function
            .extra_body
            .get("_monoize_gemini_response_json_schema")
            .and_then(Value::as_bool)
            == Some(false);
        obj.insert(
            if native {
                "response"
            } else {
                "responseJsonSchema"
            }
            .into(),
            if native {
                native_schema_types(schema, true)
            } else {
                schema.clone()
            },
        );
    }
    merge_extra(
        &mut obj,
        &function
            .extra_body
            .iter()
            .filter(|(key, _)| {
                !matches!(
                    key.as_str(),
                    "name"
                        | "description"
                        | "parameters"
                        | "parametersJsonSchema"
                        | "response"
                        | "responseJsonSchema"
                )
            })
            .map(|(key, value)| (key.clone(), value.clone()))
            .collect(),
    );
    Value::Object(obj)
}

fn encode_tool_choice(tc: &ToolChoice) -> Option<Value> {
    match tc {
        ToolChoice::Mode(mode) => match mode.as_str() {
            "none" => Some(json!({ "mode": "NONE" })),
            "required" => Some(json!({ "mode": "ANY" })),
            "validated" => Some(json!({ "mode": "VALIDATED" })),
            _ => Some(json!({ "mode": "AUTO" })),
        },
        ToolChoice::Specific(v) => {
            if v.get("type").and_then(Value::as_str) == Some("allowed_tools") {
                // Responses uses a flat selector; Chat nests it under `allowed_tools`.
                let selector = v
                    .get("allowed_tools")
                    .filter(|nested| nested.is_object())
                    .unwrap_or(v);
                let names: Vec<_> = selector
                    .get("tools")
                    .and_then(Value::as_array)
                    .into_iter()
                    .flatten()
                    .filter_map(|tool| {
                        tool.get("name").or_else(|| {
                            tool.get("function")
                                .and_then(|function| function.get("name"))
                        })
                    })
                    .filter(|value| value.is_string())
                    .cloned()
                    .collect();
                // allowedFunctionNames is valid only with ANY or VALIDATED; an `auto` choice
                // restricted to named functions is Gemini VALIDATED.
                let mode = match selector.get("mode").and_then(Value::as_str) {
                    Some("required" | "any") => "ANY",
                    Some("none") => "NONE",
                    Some("validated") => "VALIDATED",
                    _ if !names.is_empty() => "VALIDATED",
                    _ => "AUTO",
                };
                if names.is_empty() || mode == "NONE" {
                    return Some(json!({ "mode": mode }));
                }
                return Some(json!({"mode":mode,"allowedFunctionNames":names}));
            }
            let name = v
                .get("name")
                .or_else(|| v.get("function").and_then(|f| f.get("name")))
                .and_then(|n| n.as_str())
                .map(|s| s.to_string());
            name.map(|n| json!({ "mode": "ANY", "allowedFunctionNames": [n] }))
        }
    }
}

fn encode_image_part(source: &ImageSource) -> Option<Value> {
    match source {
        ImageSource::Url { url, .. } => Some(json!({ "fileData": { "fileUri": url } })),
        ImageSource::Base64 { media_type, data } => {
            Some(json!({ "inlineData": { "mimeType": media_type, "data": data } }))
        }
        ImageSource::FileId { .. } => None,
    }
}

fn encode_file_part(source: &FileSource) -> Option<Value> {
    match source {
        FileSource::Url { url } => Some(json!({ "fileData": { "fileUri": url } })),
        FileSource::Base64 {
            media_type, data, ..
        } => Some(json!({ "inlineData": { "mimeType": media_type, "data": data } })),
        FileSource::FileId { .. } | FileSource::Text { .. } | FileSource::Content { .. } => None,
    }
}

fn encode_audio_part(source: &AudioSource) -> Value {
    match source {
        AudioSource::Url { url } => {
            json!({ "fileData": { "fileUri": url } })
        }
        AudioSource::Base64 { media_type, data } => {
            json!({ "inlineData": { "mimeType": media_type, "data": data } })
        }
    }
}

fn finish_reason_to_gemini(finish_reason: Option<FinishReason>) -> &'static str {
    match finish_reason {
        Some(FinishReason::Length | FinishReason::ContextLimit) => "MAX_TOKENS",
        Some(FinishReason::ToolCalls) => "STOP",
        Some(FinishReason::ContentFilter) => "SAFETY",
        Some(FinishReason::Stop) => "STOP",
        _ => "OTHER",
    }
}

#[derive(Clone)]
struct GeminiMessageEnvelope {
    role: OrdinaryRole,
    parts: Vec<Value>,
    extra_body: HashMap<String, Value>,
}

fn flush_pending_gemini_message(pending: &mut Option<GeminiMessageEnvelope>, out: &mut Vec<Value>) {
    let Some(message) = pending.take() else {
        return;
    };
    if message.parts.is_empty() {
        return;
    }
    let role = if message.role == OrdinaryRole::Assistant {
        "model"
    } else {
        "user"
    };
    let mut obj = Map::new();
    obj.insert("role".to_string(), Value::String(role.to_string()));
    let mut parts = message.parts;
    resolve_bound_signatures(&mut parts);
    obj.insert("parts".to_string(), Value::Array(parts));
    merge_extra(&mut obj, &message.extra_body);
    out.push(Value::Object(obj));
}

fn content_extra(node: &Node) -> HashMap<String, Value> {
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

fn node_extra(node: &Node) -> &HashMap<String, Value> {
    match node {
        Node::Text { extra_body, .. }
        | Node::Image { extra_body, .. }
        | Node::Audio { extra_body, .. }
        | Node::File { extra_body, .. }
        | Node::Reasoning { extra_body, .. }
        | Node::Refusal { extra_body, .. }
        | Node::ToolCall { extra_body, .. }
        | Node::ToolResult { extra_body, .. }
        | Node::ProviderItem { extra_body, .. }
        | Node::NextDownstreamEnvelopeExtra { extra_body } => extra_body,
    }
}

fn response_finish_reason(response: &UrpResponse) -> Option<FinishReason> {
    if let Some(outcome) = &response.outcome {
        match outcome.status {
            crate::urp::ResponseStatus::Queued | crate::urp::ResponseStatus::InProgress => {
                return None;
            }
            crate::urp::ResponseStatus::Incomplete => {
                return Some(match outcome.incomplete_reason.as_deref() {
                    Some("max_output_tokens" | "context_length_exceeded") => FinishReason::Length,
                    Some("content_filter") => FinishReason::ContentFilter,
                    _ => match response.finish_reason {
                        Some(FinishReason::ContentFilter) => FinishReason::ContentFilter,
                        _ => FinishReason::Length,
                    },
                });
            }
            crate::urp::ResponseStatus::Completed => return Some(FinishReason::Stop),
            _ => {}
        }
    }
    response.finish_reason
}

pub(crate) fn encode_request_node_part(
    node: &Node,
) -> Option<(OrdinaryRole, Value, HashMap<String, Value>)> {
    let (role, mut part, mut extra) = match node {
        Node::Text {
            signature,

            role,
            content,
            extra_body,
            ..
        } => {
            let mut part = json!({"text":content});
            if let Some(signature) = signature {
                part["thoughtSignature"] = signature.clone();
            }
            Some((*role, part, extra_body.clone()))
        }
        Node::Image {
            role,
            source,
            extra_body,
            ..
        } => Some((*role, encode_image_part(source)?, extra_body.clone())),
        Node::File {
            role,
            source,
            extra_body,
            ..
        } => Some((*role, encode_file_part(source)?, extra_body.clone())),
        Node::Audio {
            role,
            source,
            extra_body,
            ..
        } => Some((*role, encode_audio_part(source), extra_body.clone())),
        Node::Refusal {
            content,
            extra_body,
            ..
        } => Some((
            OrdinaryRole::Assistant,
            json!({ "text": content }),
            extra_body.clone(),
        )),
        Node::Reasoning {
            id,
            metadata,
            content,
            encrypted,
            summary,
            extra_body,
            ..
        } if content.is_some() || summary.is_some() || encrypted.is_some() => {
            let text = content.as_deref().or(summary.as_deref());
            let part = match (text, encrypted) {
                (Some(text), signature) => {
                    let mut part = json!({"text": text, "thought": true});
                    if let Some(signature) = signature {
                        part["thoughtSignature"] = signature.clone();
                    }
                    part
                }
                (None, Some(signature))
                    if id
                        .as_deref()
                        .or(metadata.item_id.as_deref())
                        .is_some_and(|id| id.starts_with(GEMINI_PART_SIGNATURE_ID_PREFIX)) =>
                {
                    json!({ BOUND_SIGNATURE_KEY: signature })
                }
                // A Part needs a data member; Gemini streams standalone signatures this way.
                (None, Some(signature)) => json!({"text": "", "thoughtSignature": signature}),
                (None, None) => unreachable!("guarded by the match arm"),
            };
            Some((OrdinaryRole::Assistant, part, extra_body.clone()))
        }
        Node::ToolCall {
            id: _,
            namespace: _,
            signature: _,
            tool_type,
            call_id,
            name,
            arguments,
            extra_body,
        } => {
            if *tool_type == crate::urp::ToolCallType::Custom {
                return None;
            }
            let mut args = serde_json::from_str::<Value>(arguments).unwrap_or_else(|_| json!({}));
            crate::urp::integerize_json_floats(&mut args);
            Some((
                OrdinaryRole::Assistant,
                json!({
                    "functionCall": {
                        "id": call_id,
                        "name": name,
                        "args": args
                    }
                }),
                extra_body.clone(),
            ))
        }
        Node::Reasoning { .. } => None,
        Node::ProviderItem {
            id,
            role,
            origin_protocol: ProviderProtocol::Gemini,
            item_type,
            body,
            extra_body,
            ..
        } => {
            let mut wire = sanitize_provider_item_wire_body(body);
            if let Some(obj) = wire.as_object_mut() {
                if obj.contains_key("id") {
                    obj.remove("id");
                    if let Some(id) = id {
                        obj.insert("id".into(), json!(id));
                    }
                }
                if obj.contains_key("type") {
                    obj.remove("type");
                    if !item_type.is_empty() {
                        obj.insert("type".into(), json!(item_type));
                    }
                }
            }
            Some((*role, wire, extra_body.clone()))
        }
        Node::ProviderItem { .. } => None,
        Node::ToolResult { .. } => Some((
            OrdinaryRole::Assistant,
            encode_tool_result(node, &Map::new())?,
            node_extra(node)
                .get(GEMINI_CONTENT_EXTRA_KEY)
                .map(|value| {
                    HashMap::from([(GEMINI_CONTENT_EXTRA_KEY.to_string(), value.clone())])
                })
                .unwrap_or_default(),
        )),
        Node::NextDownstreamEnvelopeExtra { .. } => None,
    }?;
    match node {
        Node::ToolCall {
            signature: Some(signature),
            ..
        } => part["thoughtSignature"] = signature.clone(),
        Node::Image { metadata, .. }
        | Node::Audio { metadata, .. }
        | Node::File { metadata, .. } => {
            if let Some(signature) = &metadata.signature {
                part["thoughtSignature"] = signature.clone();
            }
            if let Some(data) = part.get_mut("fileData").and_then(Value::as_object_mut) {
                data.remove("mimeType");
                if let Some(mime) = &metadata.media_type {
                    data.insert("mimeType".into(), json!(mime));
                }
            }
        }
        _ => {}
    }
    if let Some(Value::Object(native)) = extra.remove(GEMINI_PART_EXTRA_KEY) {
        if let Some(obj) = part.as_object_mut() {
            for (key, value) in native {
                if matches!(key.as_str(), "videoMetadata" | "mediaProcessing")
                    && !node_has_video_source(node)
                {
                    continue;
                }
                if matches!(
                    key.as_str(),
                    "text" | "thoughtSignature" | "functionResponse"
                ) || (matches!(node, Node::Text { .. } | Node::Reasoning { .. })
                    && key == "thought")
                {
                    continue;
                }
                if matches!(key.as_str(), "inlineData" | "fileData" | "functionCall") {
                    if let (Some(target), Some(unknown)) = (
                        obj.get_mut(&key).and_then(Value::as_object_mut),
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
                    obj.entry(key).or_insert(value);
                }
            }
        }
    }
    if let Node::ToolCall { call_id, .. } = node {
        if call_id.starts_with(GEMINI_SYNTHETIC_CALL_PREFIX) {
            if let Some(fc) = part.get_mut("functionCall").and_then(Value::as_object_mut) {
                fc.remove("id");
            }
        }
    }
    // Content carries only `role` and `parts` plus native Gemini Content fields; node extras
    // from other protocols never become Content members.
    let content = match extra.remove(GEMINI_CONTENT_EXTRA_KEY) {
        Some(Value::Object(content)) => content.into_iter().collect(),
        _ => HashMap::new(),
    };
    Some((role, part, content))
}

fn node_has_video_source(node: &Node) -> bool {
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
            let mime = crate::urp::media::mime_essence(mime);
            mime.starts_with("video/")
                && !crate::urp::media::is_audio_mime(&mime)
                && mime != "video/text/timestamp"
        })
}

fn validate_media_nodes(nodes: &[Node]) -> Result<(), String> {
    fn image(source: &ImageSource, metadata: &crate::urp::MediaMetadata) -> Result<(), String> {
        match source {
            ImageSource::Base64 { media_type, .. } => validate_media_mime(media_type, None),
            ImageSource::Url { url, .. } => metadata
                .media_type
                .as_deref()
                .map_or(Ok(()), |mime| validate_media_mime(mime, Some(url))),
            _ => Ok(()),
        }
    }
    fn file(source: &FileSource, metadata: &crate::urp::MediaMetadata) -> Result<(), String> {
        match source {
            FileSource::Base64 { media_type, .. } => validate_media_mime(media_type, None),
            FileSource::Url { url } => metadata
                .media_type
                .as_deref()
                .map_or(Ok(()), |mime| validate_media_mime(mime, Some(url))),
            _ => Ok(()),
        }
    }
    for node in nodes {
        match node {
            Node::ToolCall {
                tool_type: crate::urp::ToolCallType::Custom,
                ..
            }
            | Node::ToolResult {
                tool_type: crate::urp::ToolCallType::Custom,
                ..
            } => {
                return Err("Gemini does not support custom tool calls or results without a function bridge.".into());
            }
            Node::ToolCall {
                name, arguments, ..
            } => {
                if name.is_empty() {
                    return Err("Gemini function calls require a non-empty name.".into());
                }
                let args: Value = serde_json::from_str(arguments).map_err(|error| {
                    format!("Gemini function arguments must be valid JSON: {error}")
                })?;
                if !args.is_object() {
                    return Err("Gemini function arguments must be a JSON object.".into());
                }
            }
            Node::Image {
                source, metadata, ..
            } => image(source, metadata)?,
            Node::File {
                source, metadata, ..
            } => file(source, metadata)?,
            Node::Audio {
                source, metadata, ..
            } => match source {
                AudioSource::Base64 { media_type, .. } => validate_media_mime(media_type, None)?,
                AudioSource::Url { url } => {
                    if let Some(mime) = &metadata.media_type {
                        validate_media_mime(mime, Some(url))?;
                    }
                }
            },
            Node::ToolResult { content, .. } => {
                for part in content {
                    match part {
                        ToolResultContent::Image {
                            source, metadata, ..
                        } => image(source, metadata)?,
                        ToolResultContent::File {
                            source, metadata, ..
                        } => file(source, metadata)?,
                        _ => {}
                    }
                }
            }
            _ => {}
        }
    }
    Ok(())
}

fn validate_media_mime(mime: &str, url: Option<&str>) -> Result<(), String> {
    let base = crate::urp::media::mime_essence(mime);
    let supported = matches!(
        base.as_str(),
        "image/png"
            | "image/jpeg"
            | "image/jpg"
            | "image/webp"
            | "image/heic"
            | "image/heif"
            | "image/gif"
            | "image/avif"
            | "audio/wav"
            | "audio/mp3"
            | "audio/aiff"
            | "audio/aac"
            | "audio/ogg"
            | "audio/flac"
            | "audio/mpeg"
            | "audio/m4a"
            | "audio/l16"
            | "audio/opus"
            | "audio/alaw"
            | "audio/mulaw"
            | "audio/webm"
            | "audio/pcm"
            | "video/audio/s16le"
            | "video/audio/wav"
            | "video/mp4"
            | "video/mpeg"
            | "video/quicktime"
            | "video/avi"
            | "video/x-flv"
            | "video/mpg"
            | "video/webm"
            | "video/wmv"
            | "video/3gpp"
            | "video/text/timestamp"
            | "application/pdf"
            | "application/rtf"
    ) || crate::urp::media::is_text_mime(&base)
        || (base == "video/*"
            && url
                .and_then(|url| reqwest::Url::parse(url).ok())
                .is_some_and(|url| {
                    matches!(
                        url.host_str(),
                        Some("youtube.com" | "www.youtube.com" | "m.youtube.com" | "youtu.be")
                    )
                }));
    if supported {
        Ok(())
    } else {
        Err(format!(
            "Gemini does not support media MIME type {mime}. Supply a documented format without changing the actual bytes."
        ))
    }
}

fn encode_result_media(content: &ToolResultContent) -> Option<Value> {
    let (mut part, extra) = match content {
        ToolResultContent::Image {
            source: ImageSource::Base64 { media_type, data },
            extra_body,
            ..
        }
        | ToolResultContent::File {
            source: FileSource::Base64 { media_type, data },
            extra_body,
            ..
        } => (
            json!({"inlineData":{"mimeType":media_type,"data":data}}),
            extra_body,
        ),
        ToolResultContent::ProviderItem {
            origin_protocol: ProviderProtocol::Gemini,
            body,
            ..
        } => return Some(sanitize_provider_item_wire_body(body)),
        _ => return None,
    };
    if let Some(unknown) = extra
        .get(GEMINI_PART_EXTRA_KEY)
        .and_then(|native| native.get("inlineData"))
        .and_then(Value::as_object)
    {
        for (key, value) in unknown {
            if !matches!(key.as_str(), "mimeType" | "data") {
                part["inlineData"]
                    .as_object_mut()
                    .unwrap()
                    .entry(key.clone())
                    .or_insert_with(|| value.clone());
            }
        }
    }
    Some(part)
}

fn encode_modality(details: &crate::urp::ModalityBreakdown) -> Value {
    json!(
        [
            ("TEXT", details.text_tokens),
            ("IMAGE", details.image_tokens),
            ("AUDIO", details.audio_tokens),
            ("VIDEO", details.video_tokens),
            ("DOCUMENT", details.document_tokens),
        ]
        .into_iter()
        .filter_map(
            |(modality, count)| count.map(|count| json!({"modality":modality,"tokenCount":count}))
        )
        .collect::<Vec<_>>()
    )
}

fn encode_tool_result(node: &Node, tool_names_by_call_id: &Map<String, Value>) -> Option<Value> {
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
    if *tool_type == crate::urp::ToolCallType::Custom {
        return None;
    }
    let result = content
        .iter()
        .filter_map(|entry| match entry {
            ToolResultContent::Text { text, .. } => Some(text.as_str()),
            _ => None,
        })
        .collect::<Vec<_>>()
        .join("");
    let function_name = name
        .as_deref()
        .or_else(|| tool_names_by_call_id.get(call_id).and_then(|v| v.as_str()))
        .unwrap_or(call_id);
    let response = if extra_body
        .get("_monoize_gemini_function_response")
        .and_then(Value::as_bool)
        == Some(true)
    {
        serde_json::from_str::<Value>(&result)
            .ok()
            .filter(Value::is_object)
            .unwrap_or_else(|| json!({"output": result}))
    } else if *is_error {
        // FunctionResponse.response documents `output` for results and `error` for failures.
        json!({"error": result})
    } else {
        json!({"output": result})
    };
    let mut response = response;
    if *is_error && response.get("error").is_none() {
        response = json!({"error":response});
    } else if !*is_error {
        if let Some(obj) = response.as_object_mut() {
            obj.remove("error");
        }
    }
    let mut function_response = json!({"name": function_name, "response": response});
    if let Some(obj) = function_response.as_object_mut() {
        merge_extra(
            obj,
            &extra_body
                .iter()
                .filter(|(key, _)| !matches!(key.as_str(), "name" | "id" | "response" | "parts"))
                .map(|(key, value)| (key.clone(), value.clone()))
                .collect(),
        );
    }
    let media: Vec<_> = content.iter().filter_map(encode_result_media).collect();
    if !media.is_empty() {
        function_response["parts"] = json!(media);
    }
    if !call_id.is_empty()
        && !call_id.starts_with(GEMINI_SYNTHETIC_CALL_PREFIX)
        && !(id.is_none() && name.as_deref() == Some(call_id.as_str()))
    {
        function_response["id"] = json!(call_id);
    }
    let mut part = json!({"functionResponse": function_response});
    if let Some(signature) = signature {
        part["thoughtSignature"] = signature.clone();
    }
    if let Some(native) = extra_body
        .get(GEMINI_PART_EXTRA_KEY)
        .and_then(Value::as_object)
    {
        for (key, value) in native {
            if !matches!(key.as_str(), "functionResponse" | "thoughtSignature") {
                part.as_object_mut()
                    .unwrap()
                    .entry(key.clone())
                    .or_insert_with(|| value.clone());
            }
        }
    }
    Some(part)
}

/// Encoder-local placeholder for a signature bound to an adjacent Part; resolved before any
/// Part reaches the wire.
const BOUND_SIGNATURE_KEY: &str = "_monoize_gemini_bound_signature";
/// Documented validator-skip value for a current-turn function call without a Gemini signature.
const DUMMY_THOUGHT_SIGNATURE: &str = "skip_thought_signature_validator";

#[derive(Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
enum ThinkingLevel {
    Minimal,
    Low,
    Medium,
    High,
}

impl ThinkingLevel {
    fn wire(self) -> &'static str {
        match self {
            ThinkingLevel::Minimal => "MINIMAL",
            ThinkingLevel::Low => "LOW",
            ThinkingLevel::Medium => "MEDIUM",
            ThinkingLevel::High => "HIGH",
        }
    }
}

const ALL_LEVELS: &[ThinkingLevel] = &[
    ThinkingLevel::Minimal,
    ThinkingLevel::Low,
    ThinkingLevel::Medium,
    ThinkingLevel::High,
];
const LEVELS_WITHOUT_MINIMAL: &[ThinkingLevel] =
    &[ThinkingLevel::Low, ThinkingLevel::Medium, ThinkingLevel::High];
const MINIMAL_OR_HIGH: &[ThinkingLevel] = &[ThinkingLevel::Minimal, ThinkingLevel::High];
const MINIMAL_MEDIUM_HIGH: &[ThinkingLevel] = &[
    ThinkingLevel::Minimal,
    ThinkingLevel::Medium,
    ThinkingLevel::High,
];

enum ThinkingTarget {
    Lowest,
    Level(ThinkingLevel),
    Highest,
}

enum ThinkingProfile {
    /// The model has no thinking controls; typed reasoning is not encoded.
    Unsupported,
    /// Gemini 2.5-style integer budget.
    Budget {
        min: u64,
        max: u64,
        can_disable: bool,
    },
    /// Gemini 3+/4 thinkingLevel; the slice lists supported levels in ascending order.
    Levels(&'static [ThinkingLevel]),
}

/// Model-family facts the encoder needs, derived from the upstream model id.
struct GeminiModel {
    thinking: ThinkingProfile,
    /// Gemini 3+ validates current-turn function-call signatures.
    requires_call_signatures: bool,
    /// Per-Part `mediaResolution` exists for Gemini 3+.
    media_resolution: bool,
    image_output: bool,
    /// Documented `imageConfig.imageSize` tiers in ascending order; empty means 1K only.
    image_sizes: &'static [&'static str],
    /// The model documents the 1:4, 4:1, 1:8 and 8:1 aspect ratios.
    extended_ratios: bool,
}

impl GeminiModel {
    fn parse(model: &str) -> Self {
        let name = model
            .trim()
            .rsplit('/')
            .next()
            .unwrap_or_default()
            .to_ascii_lowercase();
        let version = name.strip_prefix("gemini-").and_then(|rest| {
            let mut numbers = rest.split('-').next()?.split('.');
            let major = numbers.next()?.parse::<u32>().ok()?;
            let minor = match numbers.next() {
                Some(minor) => minor.parse::<u32>().ok()?,
                None => 0,
            };
            Some((major, minor))
        });
        let image_output = name.contains("image") || name.contains("nano-banana");
        let thinking = if name.contains("robotics-er-1") {
            ThinkingProfile::Budget {
                min: 0,
                max: 24576,
                can_disable: true,
            }
        } else if name.starts_with("gemma") || name.starts_with("learnlm") {
            ThinkingProfile::Unsupported
        } else if name.contains("nano-banana") {
            ThinkingProfile::Levels(MINIMAL_MEDIUM_HIGH)
        } else {
            match version {
                Some(version) if version < (2, 5) => ThinkingProfile::Unsupported,
                // Gemini 2.5 image models document thinking as not supported.
                Some((2, _)) if image_output => ThinkingProfile::Unsupported,
                Some((2, _)) if name.contains("pro") => ThinkingProfile::Budget {
                    min: 128,
                    max: 32768,
                    can_disable: false,
                },
                Some((2, _)) if name.contains("flash-lite") => ThinkingProfile::Budget {
                    min: 512,
                    max: 24576,
                    can_disable: true,
                },
                Some((2, _)) => ThinkingProfile::Budget {
                    min: 0,
                    max: 24576,
                    can_disable: true,
                },
                Some((3, _))
                    if name.contains("flash-lite-image") || name.contains("flash-image") =>
                {
                    ThinkingProfile::Levels(MINIMAL_OR_HIGH)
                }
                Some((3, minor)) if name.contains("pro") || minor == 8 => {
                    ThinkingProfile::Levels(LEVELS_WITHOUT_MINIMAL)
                }
                Some((3, _)) => ThinkingProfile::Levels(ALL_LEVELS),
                _ if name.contains("robotics-er-2") => ThinkingProfile::Levels(ALL_LEVELS),
                // Gemini 4, `-latest` aliases and unknown ids: the level set every current
                // generation accepts.
                _ => ThinkingProfile::Levels(LEVELS_WITHOUT_MINIMAL),
            }
        };
        let current = matches!(thinking, ThinkingProfile::Levels(_));
        // Image generation guide: 2.5 Flash Image and 3.1 Flash Lite Image output 1K only;
        // 3.1 Flash Image adds 512; Nano Banana 2.1 and 3 Pro Image offer 1K/2K/4K.
        let flash_image = name.contains("flash-image") && !name.contains("flash-lite-image");
        let image_sizes: &'static [&'static str] = if !image_output
            || name.contains("flash-lite-image")
            || matches!(version, Some((2, _)))
        {
            &[]
        } else if flash_image {
            &["512", "1K", "2K", "4K"]
        } else {
            &["1K", "2K", "4K"]
        };
        Self {
            thinking,
            requires_call_signatures: current,
            media_resolution: current,
            image_output,
            image_sizes,
            extended_ratios: flash_image || name.contains("nano-banana"),
        }
    }

    fn thinking_config(&self, reasoning: &crate::urp::ReasoningConfig) -> Map<String, Value> {
        let mut config = Map::new();
        let dynamic = matches!(reasoning.mode.as_deref(), Some("adaptive" | "enabled"));
        match self.thinking {
            ThinkingProfile::Unsupported => return config,
            ThinkingProfile::Budget {
                min,
                max,
                can_disable,
            } => {
                let off = if can_disable { 0 } else { min };
                let clamp = |budget: u64| budget.clamp(min.max(1), max);
                let budget = if reasoning.disabled() {
                    Some(off as i64)
                } else if let Some(budget) = reasoning.budget_tokens {
                    Some(if budget == 0 { off } else { clamp(budget) } as i64)
                } else if let Some(effort) = reasoning.effort.as_deref() {
                    effort_budget(effort).map(|budget| clamp(budget) as i64)
                } else if dynamic {
                    Some(-1)
                } else {
                    None
                };
                if let Some(budget) = budget {
                    config.insert("thinkingBudget".into(), json!(budget));
                }
            }
            ThinkingProfile::Levels(levels) => {
                let target = if reasoning.disabled() {
                    Some(ThinkingTarget::Lowest)
                } else if let Some(budget) = reasoning.budget_tokens {
                    Some(match budget {
                        0 => ThinkingTarget::Lowest,
                        1..=1024 => ThinkingTarget::Level(ThinkingLevel::Low),
                        1025..=8192 => ThinkingTarget::Level(ThinkingLevel::Medium),
                        _ => ThinkingTarget::Level(ThinkingLevel::High),
                    })
                } else {
                    match reasoning.effort.as_deref() {
                        Some("minimal") => Some(ThinkingTarget::Level(ThinkingLevel::Minimal)),
                        Some("low") => Some(ThinkingTarget::Level(ThinkingLevel::Low)),
                        Some("medium") => Some(ThinkingTarget::Level(ThinkingLevel::Medium)),
                        Some("high") => Some(ThinkingTarget::Level(ThinkingLevel::High)),
                        Some("xhigh" | "max") => Some(ThinkingTarget::Highest),
                        // Adaptive or unknown effort keeps the model's default dynamic level.
                        _ => None,
                    }
                };
                if let Some(target) = target {
                    config.insert(
                        "thinkingLevel".into(),
                        json!(select_level(levels, target).wire()),
                    );
                }
            }
        }
        let include_thoughts = if let Some(summary) = &reasoning.summary {
            Some(summary != "none")
        } else if reasoning.disabled() {
            None
        } else if dynamic {
            Some(reasoning.display.as_deref() != Some("omitted"))
        } else if reasoning.effort.is_some() || reasoning.budget_tokens.is_some() {
            Some(true)
        } else {
            None
        };
        if let Some(include) = include_thoughts {
            config.insert("includeThoughts".into(), json!(include));
        }
        config
    }
}

fn effort_budget(effort: &str) -> Option<u64> {
    Some(match effort {
        "minimal" => 512,
        "low" => 1024,
        "medium" => 8192,
        "high" => 24576,
        "xhigh" | "max" => u64::MAX,
        _ => return None,
    })
}

/// Picks the supported level nearest to the target; ties resolve to the higher level.
fn select_level(levels: &[ThinkingLevel], target: ThinkingTarget) -> ThinkingLevel {
    let first = levels[0];
    let last = levels[levels.len() - 1];
    match target {
        ThinkingTarget::Lowest => first,
        ThinkingTarget::Highest => last,
        ThinkingTarget::Level(level) => levels
            .iter()
            .copied()
            .min_by_key(|candidate| {
                let distance = (*candidate as i8 - level as i8).unsigned_abs();
                (distance, std::cmp::Reverse(*candidate))
            })
            .unwrap_or(last),
    }
}

/// Gemini Tool member equivalent to a cross-protocol built-in tool type.
pub(crate) fn gemini_builtin_tool_key(tool_type: &str) -> Option<&'static str> {
    if tool_type == "web_search" || tool_type.starts_with("web_search_") {
        Some("googleSearch")
    } else if tool_type == "code_interpreter" || tool_type.starts_with("code_execution") {
        Some("codeExecution")
    } else if tool_type.starts_with("web_fetch") {
        Some("urlContext")
    } else {
        None
    }
}

/// True for a cross-protocol built-in tool the Gemini encoder can represent.
pub(crate) fn is_gemini_mappable_tool(tool_type: &str) -> bool {
    gemini_builtin_tool_key(tool_type).is_some() || tool_type == "image_generation"
}

fn gemini_service_tier(tier: &str) -> Option<&'static str> {
    match tier {
        "default" | "standard" | "standard_only" => Some("standard"),
        "flex" => Some("flex"),
        "priority" => Some("priority"),
        _ => None,
    }
}

/// Maps an OpenAI-style `WxH` size to the nearest aspect ratio and the smallest size tier the
/// model documents for that request (image generation guide tables).
fn image_config_from_size(model: &GeminiModel, size: &str) -> Option<Map<String, Value>> {
    const RATIOS: [(u32, u32); 14] = [
        (1, 1),
        (2, 3),
        (3, 2),
        (3, 4),
        (4, 3),
        (4, 5),
        (5, 4),
        (9, 16),
        (16, 9),
        (21, 9),
        (1, 4),
        (4, 1),
        (1, 8),
        (8, 1),
    ];
    let ratios = if model.extended_ratios {
        &RATIOS[..]
    } else {
        &RATIOS[..10]
    };
    let (width, height) = size.split_once('x')?;
    let width = width.trim().parse::<u32>().ok().filter(|value| *value > 0)?;
    let height = height.trim().parse::<u32>().ok().filter(|value| *value > 0)?;
    let target = (f64::from(width) / f64::from(height)).ln();
    let distance = |(w, h): &(u32, u32)| ((f64::from(*w) / f64::from(*h)).ln() - target).abs();
    let (ratio_width, ratio_height) = ratios
        .iter()
        .min_by(|left, right| distance(left).total_cmp(&distance(right)))?;
    let mut config = Map::from_iter([(
        "aspectRatio".to_string(),
        json!(format!("{ratio_width}:{ratio_height}")),
    )]);
    // Longest documented edge per tier across ratios: 512 → 792, 1K → 1584, 2K → 3168.
    let wanted = match width.max(height) {
        0..=792 => "512",
        793..=1584 => "1K",
        1585..=3168 => "2K",
        _ => "4K",
    };
    let tiers = ["512", "1K", "2K", "4K"];
    let rank = |tier: &str| tiers.iter().position(|candidate| *candidate == tier);
    if let Some(tier) = model
        .image_sizes
        .iter()
        .find(|tier| rank(tier) >= rank(wanted))
        .or(model.image_sizes.last())
    {
        config.insert("imageSize".to_string(), json!(tier));
    }
    Some(config)
}

/// Maps a typed image `detail` to the Gemini 3+ per-Part media resolution.
fn add_media_resolution(node: &Node, part: &mut Value) {
    let Node::Image {
        source, metadata, ..
    } = node
    else {
        return;
    };
    let detail = metadata.detail.as_deref().or(match source {
        ImageSource::Url { detail, .. } => detail.as_deref(),
        _ => None,
    });
    let level = match detail {
        Some("low") => "MEDIA_RESOLUTION_LOW",
        Some("high") => "MEDIA_RESOLUTION_HIGH",
        Some("original") => "MEDIA_RESOLUTION_ULTRA_HIGH",
        _ => return,
    };
    if let Some(part) = part.as_object_mut() {
        part.entry("mediaResolution")
            .or_insert_with(|| json!({ "level": level }));
    }
}

/// Gemini 3+ rejects a current-turn step whose first functionCall lacks a signature. The
/// current turn starts after the last user Content that holds anything besides function
/// responses; each model Content after it is one step.
fn sign_current_turn_calls(contents: &mut [Value]) {
    let start = contents
        .iter()
        .rposition(|content| {
            content.get("role").and_then(Value::as_str) == Some("user")
                && content
                    .get("parts")
                    .and_then(Value::as_array)
                    .is_some_and(|parts| {
                        parts.iter().any(|part| part.get("functionResponse").is_none())
                    })
        })
        .map_or(0, |index| index + 1);
    for content in &mut contents[start..] {
        if content.get("role").and_then(Value::as_str) != Some("model") {
            continue;
        }
        let Some(part) = content
            .get_mut("parts")
            .and_then(Value::as_array_mut)
            .and_then(|parts| parts.iter_mut().find(|part| part.get("functionCall").is_some()))
        else {
            continue;
        };
        let unsigned = match part.get("thoughtSignature") {
            None | Some(Value::Null) => true,
            Some(Value::String(signature)) => signature.is_empty(),
            Some(_) => false,
        };
        if unsigned {
            part["thoughtSignature"] = json!(DUMMY_THOUGHT_SIGNATURE);
        }
    }
}

/// Attaches each bound signature placeholder to the adjacent unsigned text or media Part
/// (preceding first, then following); otherwise it becomes a standalone empty-text Part.
/// Returns the removed placeholder positions in removal order.
fn resolve_bound_signatures(parts: &mut Vec<Value>) -> Vec<usize> {
    fn accepts(part: &Value) -> bool {
        part.get(BOUND_SIGNATURE_KEY).is_none()
            && part.get("thoughtSignature").is_none()
            && (part.get("inlineData").is_some()
                || part.get("fileData").is_some()
                || (part.get("text").is_some()
                    && part.get("thought").and_then(Value::as_bool) != Some(true)))
    }
    let mut removed = Vec::new();
    let mut index = 0;
    while index < parts.len() {
        let Some(signature) = parts[index].get(BOUND_SIGNATURE_KEY).cloned() else {
            index += 1;
            continue;
        };
        let target = [index.checked_sub(1), Some(index + 1)]
            .into_iter()
            .flatten()
            .find(|target| parts.get(*target).is_some_and(accepts));
        match target {
            Some(target) => {
                parts[target]["thoughtSignature"] = signature;
                parts.remove(index);
                removed.push(index);
            }
            None => {
                parts[index] = json!({"text": "", "thoughtSignature": signature});
                index += 1;
            }
        }
    }
    removed
}

/// Gemini accepts only its own signatures. Before envelope unwrapping, clears call signatures
/// and drops reasoning signatures whose provenance is not Gemini: an `mz2.` envelope must name
/// provider type `gemini`, an `mz1.` sigil must bind an `rs_gemini_` item id, and a raw
/// reasoning value must belong to an `rs_gemini_` Reasoning id. Raw call signatures can only
/// arrive through the Gemini call transport and are kept. Reasoning left without text is
/// removed.
pub fn retain_gemini_signature_provenance(nodes: &mut Vec<Node>) {
    fn gemini_provenance(value: &Value, id: Option<&str>, raw_accepted: bool) -> bool {
        let gemini_id = |id: &str| id.starts_with(GEMINI_SIGNATURE_ID_PREFIX);
        match crate::urp::parse_reasoning_envelope(value) {
            Some(envelope) if envelope.v == 2 => envelope.provider_type == "gemini",
            Some(envelope) => envelope.item_id.as_deref().is_some_and(gemini_id),
            None => raw_accepted || id.is_some_and(gemini_id),
        }
    }
    nodes.retain_mut(|node| match node {
        Node::ToolCall { signature, .. } => {
            if signature
                .as_ref()
                .is_some_and(|value| !gemini_provenance(value, None, true))
            {
                *signature = None;
            }
            true
        }
        Node::Reasoning {
            id,
            metadata,
            content,
            summary,
            encrypted,
            ..
        } => {
            let foreign = encrypted.as_ref().is_some_and(|value| {
                !gemini_provenance(value, id.as_deref().or(metadata.item_id.as_deref()), false)
            });
            if foreign {
                *encrypted = None;
                return content.as_deref().is_some_and(|text| !text.is_empty())
                    || summary.as_deref().is_some_and(|text| !text.is_empty());
            }
            true
        }
        _ => true,
    });
}
