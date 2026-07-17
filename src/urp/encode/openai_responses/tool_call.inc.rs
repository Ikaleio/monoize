pub fn encode_request(req: &UrpRequest, upstream_model: &str) -> Value {
    let request_items = nodes_to_items(&req.input);
    let mut input_items = Vec::new();
    let mut instructions: Option<String> = None;
    let mut consumed_instructions = false;

    for item in &request_items {
        if !consumed_instructions && can_use_responses_instructions(item) {
            if let Item::Message { parts, .. } = item {
                let text = text_parts(parts);
                if !text.is_empty() {
                    instructions = Some(text);
                    consumed_instructions = true;
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

    if let Some(text) = instructions {
        obj.insert("instructions".to_string(), Value::String(text));
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
        // "none" means "disable reasoning". OpenAI's Responses API only disables
        // reasoning when the effort field is *absent*; sending `"effort":"none"`
        // silently activates low-effort reasoning. So we omit the field entirely.
        if let Some(effort) = &reasoning.effort {
            if effort != "none" {
                reasoning_obj.insert("effort".to_string(), Value::String(effort.clone()));
            }
        }
        merge_extra(&mut reasoning_obj, &reasoning.extra_body);
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
            tool_choice_to_openai_value(choice),
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
    merge_responses_text_config(obj, req.extra_body.get("text"));
    merge_extra(obj, &req.extra_body);
    ensure_responses_encrypted_reasoning_include(obj);
    body
}

pub fn encode_response(resp: &UrpResponse, logical_model: &str) -> Value {
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
                    if let Some(image_generation_call) = encode_image_generation_call_part(part) {
                        flush_pending_message_item(&mut pending_message, &mut output);
                        output.push(image_generation_call);
                        continue;
                    }
                    if let Some(content_part) = encode_message_content_part(part, true) {
                        append_content_part_to_pending(
                            &mut pending_message,
                            &mut output,
                            *role,
                            text_part_phase(part),
                            &message_extra,
                            content_part,
                        );
                        continue;
                    }

                    flush_pending_message_item(&mut pending_message, &mut output);

                    if let Some(reasoning_item) = encode_reasoning_item(part) {
                        output.push(reasoning_item);
                        continue;
                    }

                    if let Some(tool_call_item) = encode_tool_call_item(part) {
                        output.push(tool_call_item);
                        continue;
                    }

                    if let Part::ProviderItem {
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
                        ) {
                            output.push(item);
                        }
                    }
                }
                flush_pending_message_item(&mut pending_message, &mut output);
            }
            Item::ToolResult {
                id,
                call_id,
                content,
                is_error,
                extra_body,
            } => encode_tool_result_item(
                id.as_deref(),
                call_id,
                content,
                *is_error,
                extra_body,
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

    let source_response = resp
        .extra_body
        .get(RESPONSES_RESPONSE_SOURCE_EXTRA_KEY)
        .and_then(Value::as_object)
        .cloned();
    let mut body = source_response.map(Value::Object).unwrap_or_else(|| {
        json!({
            "id": resp.id,
            "object": "response",
            "created_at": created_at,
            "completed_at": completed_at,
            "model": logical_model,
            "status": status,
            "output": output.clone(),
            "incomplete_details": null,
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
        obj.insert("model".to_string(), Value::String(logical_model.to_string()));
        obj.insert("output".to_string(), Value::Array(output));
    }

    if let Some(usage) = &resp.usage {
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
        if let Some(obj) = usage_value.as_object_mut() {
            for (k, v) in &usage.extra_body {
                if !k.starts_with("_monoize_") {
                    obj.insert(k.clone(), v.clone());
                }
            }
        }
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
            let output_text_type = matches!(role, Role::Assistant);

            for part in parts {
                if let Some(image_generation_call) = encode_image_generation_call_part(part) {
                    flush_pending_message_item(&mut pending_message, out);
                    out.push(image_generation_call);
                    continue;
                }
                if let Some(content_part) = encode_message_content_part(part, output_text_type) {
                    append_content_part_to_pending(
                        &mut pending_message,
                        out,
                        *role,
                        text_part_phase(part),
                        &message_extra,
                        content_part,
                    );
                    continue;
                }

                flush_pending_message_item(&mut pending_message, out);

                if let Some(mut item) =
                    encode_reasoning_request_item(part).or_else(|| encode_tool_call_item(part))
                {
                    sanitize_reasoning_request_item(&mut item);
                    out.push(item);
                    continue;
                }

                if let Part::ProviderItem {
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
                    )
                {
                    out.push(item);
                }
            }
            flush_pending_message_item(&mut pending_message, out);
        }
        Item::ToolResult {
            id,
            call_id,
            content,
            is_error,
            extra_body,
        } => encode_tool_result_item(id.as_deref(), call_id, content, *is_error, extra_body, out),
    }
}
