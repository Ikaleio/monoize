fn is_retained_responses_instruction_node(node: &Node) -> bool {
    let extra_body = match node {
        Node::Text { extra_body, .. }
        | Node::Image { extra_body, .. }
        | Node::Audio { extra_body, .. }
        | Node::File { extra_body, .. }
        | Node::Refusal { extra_body, .. }
        | Node::Reasoning { extra_body, .. }
        | Node::ToolCall { extra_body, .. }
        | Node::ProviderItem { extra_body, .. }
        | Node::ToolResult { extra_body, .. }
        | Node::NextDownstreamEnvelopeExtra { extra_body } => extra_body,
    };
    extra_body
        .get(RESPONSES_INSTRUCTION_NODE_EXTRA_KEY)
        .and_then(Value::as_bool)
        == Some(true)
}

pub fn encode_request(req: &UrpRequest, upstream_model: &str) -> Value {
    encode_request_checked(req, upstream_model)
        .unwrap_or_else(|error| crate::urp::media::error_body(&error))
}

pub fn encode_request_checked(req: &UrpRequest, upstream_model: &str) -> Result<Value, String> {
    let prepared = crate::urp::media::prepare_request(req, ProviderProtocol::Responses)?;
    validate_stable_responses_audio(&prepared.input)?;
    Ok(encode_request_prepared(&prepared, upstream_model))
}

fn encode_request_prepared(req: &UrpRequest, upstream_model: &str) -> Value {
    let instruction_nodes = req
        .input
        .iter()
        .filter(|node| is_retained_responses_instruction_node(node))
        .cloned()
        .collect::<Vec<_>>();
    let retained_instructions = req.instructions_format.map(|format| match format {
        crate::urp::InstructionsFormat::Null if instruction_nodes.is_empty() => Value::Null,
        crate::urp::InstructionsFormat::Items => {
            let mut items = Vec::new();
            for item in nodes_to_items(&instruction_nodes) {
                encode_message_to_input_items(&item, &mut items);
            }
            Value::Array(items)
        }
        _ => Value::String(
            instruction_nodes
                .iter()
                .filter_map(|node| match node {
                    Node::Text { content, .. } => Some(content.as_str()),
                    _ => None,
                })
                .collect::<Vec<_>>()
                .join("\n"),
        ),
    });
    let request_input = req
        .input
        .iter()
        .filter(|node| {
            req.instructions_format.is_none() || !is_retained_responses_instruction_node(node)
        })
        .cloned()
        .collect::<Vec<_>>();
    let request_items = nodes_to_items(&request_input);
    let mut input_items = Vec::new();
    let mut instructions = retained_instructions;

    for item in &request_items {
        if instructions.is_none() && can_use_responses_instructions(item) {
            if let Item::Message { parts, .. } = item {
                let text = text_parts(parts);
                if !text.is_empty() {
                    instructions = Some(Value::String(text));
                    continue;
                }
            }
        }
        encode_message_to_input_items(item, &mut input_items);
    }
    sanitize_request_input_items(&mut input_items);

    let mut body = json!({
        "model": upstream_model,
        "input": Value::Array(input_items),
    });
    let obj = body.as_object_mut().expect("responses request object");

    if let Some(instructions) = instructions {
        obj.insert("instructions".to_string(), instructions);
    }
    if let Some(stream) = req.stream {
        obj.insert("stream".to_string(), Value::Bool(stream));
    }
    if let Some(temp) = req.temperature {
        obj.insert("temperature".to_string(), Value::from(temp));
    }
    if let Some(top_p) = req.top_p {
        obj.insert("top_p".to_string(), Value::from(top_p));
    }
    if let Some(max) = req.max_output_tokens {
        obj.insert("max_output_tokens".to_string(), Value::from(max));
    }
    if let Some(reasoning) = &req.reasoning {
        let mut reasoning_obj = Map::new();
        if reasoning.disabled() {
            reasoning_obj.insert("effort".to_string(), json!("none"));
        } else if let Some(effort) = &reasoning.effort {
            reasoning_obj.insert("effort".to_string(), Value::String(effort.clone()));
        }
        if let Some(summary) = &reasoning.summary {
            reasoning_obj.insert("summary".to_string(), json!(summary));
        }
        let unknown = reasoning
            .extra_body
            .iter()
            .filter(|(key, _)| !crate::urp::ReasoningConfig::is_control(key))
            .map(|(key, value)| (key.clone(), value.clone()))
            .collect();
        merge_extra(&mut reasoning_obj, &unknown);
        if !reasoning_obj.is_empty() {
            obj.insert("reasoning".to_string(), Value::Object(reasoning_obj));
        }
    }
    if let Some(tools) = &req.tools {
        obj.insert("tools".to_string(), Value::Array(encode_tools(tools)));
    }
    if let Some(choice) = &req.tool_choice {
        obj.insert(
            "tool_choice".to_string(),
            tool_choice_to_responses_value(choice),
        );
    }
    if let Some(parallel) = req.parallel_tool_calls {
        obj.insert("parallel_tool_calls".to_string(), Value::Bool(parallel));
    }
    if let Some(user) = &req.user {
        obj.insert("user".to_string(), Value::String(user.clone()));
    }
    if let Some(format) = &req.response_format {
        apply_response_format(obj, format);
    }
    if let Some(verbosity) = &req.verbosity {
        let text = obj.entry("text".to_string()).or_insert_with(|| json!({}));
        if !text.is_object() {
            *text = json!({});
        }
        text.as_object_mut()
            .expect("responses text object")
            .insert("verbosity".to_string(), Value::String(verbosity.clone()));
    }
    merge_responses_text_config(obj, req.extra_body.get("text"));
    merge_extra(obj, &req.extra_body);
    crate::urp::logprobs::encode_request(
        &mut body,
        &req.logprobs,
        crate::urp::ProviderProtocol::Responses,
    );
    body
}

pub fn encode_response(resp: &UrpResponse, logical_model: &str) -> Value {
    encode_response_checked(resp, logical_model)
        .unwrap_or_else(|error| crate::urp::media::error_body(&error))
}

pub fn encode_response_checked(resp: &UrpResponse, logical_model: &str) -> Result<Value, String> {
    let mut prepared = resp.clone();
    prepare_response_nodes(&mut prepared.output)?;
    Ok(encode_response_validated(&prepared, logical_model))
}

fn validate_stable_responses_audio(nodes: &[Node]) -> Result<(), String> {
    fn audio_kind(kind: &str) -> bool { matches!(kind, "audio" | "input_audio" | "output_audio") }
    for node in nodes {
        let unsupported = match node {
            Node::ProviderItem { item_type, .. } => audio_kind(item_type),
            Node::ToolResult { content, .. } => content.iter().any(|part| {
                matches!(part, ToolResultContent::ProviderItem { item_type, .. } if audio_kind(item_type))
            }),
            _ => false,
        };
        if unsupported { return Err("The stable Responses schema has no audio content carrier".into()); }
    }
    Ok(())
}

pub(crate) fn prepare_response_nodes(nodes: &mut [Node]) -> Result<(), String> {
    validate_response_nodes(nodes)?;
    for node in nodes {
        if let Node::ToolResult { content, .. } = node {
            *content = crate::urp::media::prepare_tool_result_content(content, ProviderProtocol::Responses)?;
        }
    }
    Ok(())
}

pub(crate) fn validate_response_nodes(nodes: &[Node]) -> Result<(), String> {
    validate_stable_responses_audio(nodes)?;
    for node in nodes {
        match node {
            Node::Image { source: ImageSource::Base64 { media_type, .. }, extra_body, .. }
                if extra_body.contains_key(RESPONSES_IMAGE_GENERATION_CALL_EXTRA_KEY)
                    && matches!(media_type.as_str(), "image/png" | "image/jpeg" | "image/webp") => {}
            Node::Image { .. } | Node::File { .. } | Node::Audio { .. } => {
                return Err("Responses output cannot represent ordinary image, file, or audio media; native image_generation_call is required for generated images".into());
            }
            Node::ProviderItem { item_type, .. }
                if matches!(item_type.as_str(), "input_image" | "output_image" | "image_url" | "input_file" | "output_file" | "file" | "input_audio") => {
                return Err("Native response content cannot contain input-only media items".into());
            }
            _ => {}
        }
    }
    Ok(())
}

fn encode_response_validated(resp: &UrpResponse, logical_model: &str) -> Value {
    let projected = crate::urp::tool_signature::project_response(resp);
    let resp = &projected;
    let response_items = nodes_to_items(&resp.output);
    let mut output = Vec::new();
    for item in &response_items {
        match item {
            Item::Message {
                id,
                role,
                parts,
                extra_body,
            } => {
                let mut message_extra = extra_body.clone();
                message_extra.remove(RESPONSES_IMAGE_GENERATION_CALL_EXTRA_KEY);
                if let Some(id) = id.clone() {
                    message_extra
                        .entry("id".to_string())
                        .or_insert(Value::String(id));
                }
                let mut pending_message: Option<PendingResponsesMessageItem> = None;
                for part in parts {
                    if let Some(image_generation_call) = encode_image_generation_call_part(part, id.as_deref()) {
                        flush_pending_message_item(&mut pending_message, &mut output, true);
                        output.push(image_generation_call);
                        continue;
                    }
                    if let Some(content_part) = encode_message_content_part(part, true) {
                        append_content_part_to_pending(
                            &mut pending_message,
                            &mut output,
                            true,
                            *role,
                            text_part_phase(part),
                            &message_extra,
                            content_part,
                        );
                        continue;
                    }

                    flush_pending_message_item(&mut pending_message, &mut output, true);

                    if let Some(reasoning_item) = encode_reasoning_item(part) {
                        output.push(reasoning_item);
                        continue;
                    }

                    if let Some(tool_call_item) = encode_tool_call_item(part, true) {
                        output.push(tool_call_item);
                        continue;
                    }

                    if let Part::ProviderItem {
                        id,
                        origin_protocol,
                        item_type,
                        body,
                        extra_body,
                        ..
                    } = part
                    {
                        if let Some(item) = encode_provider_item_for_responses(
                            *origin_protocol,
                            item_type,
                            body,
                            extra_body,
                            Some(id),
                        ) {
                            output.push(item);
                        }
                    }
                }
                flush_pending_message_item(&mut pending_message, &mut output, true);
            }
            Item::ToolResult {
                namespace, name,
                id,
                tool_type,
                call_id,
                content,
                is_error,
                extra_body,
            } => encode_tool_result_item(
                id.as_deref(),
                *tool_type,
                call_id,
                namespace.as_deref(), name.as_deref(),
                content,
                *is_error,
                extra_body,
                true,
                &mut output,
            ),
        }
    }

    let created_at = resp
        .created_at
        .unwrap_or_else(|| chrono::Utc::now().timestamp());
    let status = finish_reason_to_status(resp.finish_reason);
    let completed_at = if status == "completed" {
        Value::Number(serde_json::Number::from(chrono::Utc::now().timestamp()))
    } else {
        Value::Null
    };

    let source_response = resp.extra_body.contains_key(RESPONSES_RESPONSE_SOURCE_EXTRA_KEY)
        .then(Map::new);
    let mut body = source_response.map(Value::Object).unwrap_or_else(|| {
        json!({
            "id": resp.id,
            "object": "response",
            "created_at": created_at,
            "completed_at": completed_at,
            "model": logical_model,
            "status": status,
            "output": output.clone(),
            "incomplete_details": match resp.finish_reason {
                Some(FinishReason::ContentFilter) => json!({"reason": "content_filter"}),
                Some(FinishReason::Length) => json!({"reason": "max_output_tokens"}),
                _ => Value::Null,
            },
            "previous_response_id": null,
            "instructions": null,
            "error": null,
            "tools": [],
            "tool_choice": "auto",
            "truncation": "disabled",
            "parallel_tool_calls": true,
            "text": { "format": { "type": "text" } },
            "top_p": 1.0,
            "top_logprobs": 0,
            "temperature": 1.0,
            "reasoning": null,
            "max_output_tokens": null,
            "max_tool_calls": null,
            "store": false,
            "background": false,
            "metadata": {}
        })
    });
    if let Some(obj) = body.as_object_mut() {
        obj.retain(|key, _| !key.starts_with("_monoize_"));
        obj.insert("id".to_string(), Value::String(resp.id.clone()));
        obj.insert("object".to_string(), Value::String("response".to_string()));
        obj.insert("created_at".to_string(), json!(created_at));
        obj.insert(
            "model".to_string(),
            Value::String(logical_model.to_string()),
        );
        obj.insert("output".to_string(), Value::Array(output));
    }

    if let Some(usage) = &resp.usage {
        let usage_value = encode_usage(usage);
        body["usage"] = usage_value;
    }

    if let Some(obj) = body.as_object_mut() {
        for (key, value) in &resp.extra_body {
            if !key.starts_with("_monoize_")
                && !matches!(
                    key.as_str(),
                    "id" | "object" | "created" | "created_at" | "model" | "output" | "usage"
                )
            {
                obj.insert(key.clone(), value.clone());
            }
        }
    }
    let outcome = resp
        .outcome
        .clone()
        .unwrap_or_else(|| crate::urp::ResponseOutcome::from_finish(resp.finish_reason));
    outcome.write_responses(&mut body);

    body
}

fn encode_message_to_input_items(item: &Item, out: &mut Vec<Value>) {
    match item {
        Item::Message {
            id,
            role,
            parts,
            extra_body,
        } => {
            let mut message_extra = extra_body.clone();
            message_extra.remove(RESPONSES_IMAGE_GENERATION_CALL_EXTRA_KEY);
            if let Some(id) = id.clone() {
                message_extra
                    .entry("id".to_string())
                    .or_insert(Value::String(id));
            }
            let mut pending_message: Option<PendingResponsesMessageItem> = None;
            let has_media = parts.iter().any(|part| matches!(part, Part::Image { .. } | Part::File { .. }));
            let output_text_type = matches!(role, Role::Assistant) && !has_media;
            if has_media {
                // Easy-input messages have no native output item identity or status.
                message_extra.remove("id");
                message_extra.remove("status");
            }

            for part in parts {
                if let Some(image_generation_call) = encode_image_generation_call_part(part, id.as_deref()) {
                    flush_pending_message_item(&mut pending_message, out, false);
                    out.push(image_generation_call);
                    continue;
                }
                if let Some(content_part) = encode_message_content_part(part, output_text_type) {
                    append_content_part_to_pending(
                        &mut pending_message,
                        out,
                        false,
                        *role,
                        text_part_phase(part),
                        &message_extra,
                        content_part,
                    );
                    continue;
                }

                flush_pending_message_item(&mut pending_message, out, false);

                if let Some(mut item) = encode_reasoning_request_item(part)
                    .or_else(|| encode_tool_call_item(part, false))
                {
                    sanitize_reasoning_request_item(&mut item);
                    out.push(item);
                    continue;
                }

                if let Part::ProviderItem {
                        id,
                    origin_protocol,
                    item_type,
                    body,
                    extra_body,
                    ..
                } = part
                    && let Some(item) = encode_provider_item_for_responses(
                        *origin_protocol,
                        item_type,
                        body,
                        extra_body,
                        Some(id),
                    )
                {
                    out.push(item);
                }
            }
            flush_pending_message_item(&mut pending_message, out, false);
        }
        Item::ToolResult {
                namespace, name,
            id,
            tool_type,
            call_id,
            content,
            is_error,
            extra_body,
        } => encode_tool_result_item(
            id.as_deref(),
            *tool_type,
            call_id,
            namespace.as_deref(), name.as_deref(),
            content,
            *is_error,
            extra_body,
            false,
            out,
        ),
    }
}

pub(crate) fn encode_usage(usage: &crate::urp::Usage) -> Value {
    let aggregate = usage.accounting();
    let usage = aggregate.as_ref();
    let input_details = usage_input_details(usage);
    let output_details = usage_output_details(usage);
    let mut usage_value = json!({
        "input_tokens": usage.input_tokens,
        "output_tokens": usage.output_tokens,
        "total_tokens": usage.total_tokens(),
        "output_tokens_details": {
            "reasoning_tokens": output_details.reasoning_tokens,
            "accepted_prediction_tokens": output_details.accepted_prediction_tokens,
            "rejected_prediction_tokens": output_details.rejected_prediction_tokens
        },
        "input_tokens_details": {
            "cached_tokens": input_details.cache_read_tokens,
            "cache_write_tokens": input_details.cache_creation_tokens,
            "cache_creation_tokens": input_details.cache_creation_tokens,
            "tool_prompt_tokens": input_details.tool_prompt_tokens
        }
    });
    merge_responses_usage_extra(&mut usage_value, &usage.extra_body);
    usage_value
}
