//! Proto-JSON snake_case request field names accepted at structural positions (GEM-4).

use serde_json::{Map, Value};

/// Renames documented proto-JSON snake_case fields to camelCase at structural positions only.
/// User payloads (function args, responses, schemas, labels) are never rewritten.
pub(super) fn normalize_proto_json_names(value: &Value) -> Value {
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
            rename(
                function,
                &[("allowed_function_names", "allowedFunctionNames")],
            );
        }
    }
    value
}
