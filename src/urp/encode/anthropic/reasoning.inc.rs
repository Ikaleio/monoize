const ANTHROPIC_DEFAULT_MAX_TOKENS: u64 = 64_000;

fn apply_messages_response_format(obj: &mut Map<String, Value>, format: &ResponseFormat) {
    match format {
        ResponseFormat::JsonSchema { json_schema } => {
            let mut output_config = obj
                .remove("output_config")
                .and_then(|value| value.as_object().cloned())
                .unwrap_or_default();
            let mut messages_format = output_config
                .remove("format")
                .and_then(|value| value.as_object().cloned())
                .unwrap_or_default();
            messages_format.insert("type".to_string(), json!("json_schema"));
            messages_format.insert("schema".to_string(), json_schema.schema.clone());
            output_config.insert("format".to_string(), Value::Object(messages_format));
            obj.insert("output_config".to_string(), Value::Object(output_config));
        }
        ResponseFormat::Text | ResponseFormat::JsonObject => {
            let Some(mut output_config) = obj
                .remove("output_config")
                .and_then(|value| value.as_object().cloned())
            else {
                return;
            };
            output_config.remove("format");
            if !output_config.is_empty() {
                obj.insert("output_config".to_string(), Value::Object(output_config));
            }
        }
    }
}

/// Controls whether the Anthropic encoder should embed a sigil (`mz1.<id>.<sig>`) in
/// `thinking.signature` / `redacted_thinking.data` to smuggle a reasoning item id through a
/// downstream client that strips unknown fields. Upstream-facing encoding MUST use `StripSigil`
/// so that the real Anthropic API receives a clean opaque signature. Downstream-facing encoding
/// MUST use `EmbedSigil` so that monoize can recover the item id when the client echoes the
/// history back. See `spec/unified_responses_proxy.spec.md` DM5.2 / PM5b.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ReasoningSigilMode {
    EmbedSigil,
    StripSigil,
}

fn reasoning_is_redacted(extra_body: &HashMap<String, Value>) -> bool {
    extra_body
        .get(REASONING_KIND_EXTRA_KEY)
        .and_then(Value::as_str)
        == Some(REASONING_KIND_REDACTED_THINKING)
}

fn reasoning_extra_for_wire(extra_body: &HashMap<String, Value>) -> HashMap<String, Value> {
    extra_body
        .iter()
        .filter(|(key, _)| key.as_str() != REASONING_KIND_EXTRA_KEY)
        .map(|(k, v)| (k.clone(), v.clone()))
        .collect()
}

fn plaintext_from_reasoning<'a>(
    content: &'a Option<String>,
    summary: &'a Option<String>,
) -> Option<&'a str> {
    content
        .as_deref()
        .filter(|s| !s.is_empty())
        .or_else(|| summary.as_deref().filter(|s| !s.is_empty()))
}

fn encoded_signature_value(
    encrypted: &Option<Value>,
    id: &Option<String>,
    mode: ReasoningSigilMode,
) -> Option<Value> {
    let raw = match encrypted {
        None | Some(Value::Null) => return None,
        Some(Value::String(s)) if s.is_empty() => return None,
        Some(Value::String(s)) => s.clone(),
        Some(other) => return Some(other.clone()),
    };
    let processed = match mode {
        ReasoningSigilMode::EmbedSigil if raw.starts_with(REASONING_ENVELOPE_PREFIX) => raw,
        ReasoningSigilMode::EmbedSigil => id
            .as_deref()
            .filter(|s| !s.is_empty())
            .and_then(|item_id| wrap_reasoning_signature_with_item_id(item_id, &raw))
            .unwrap_or(raw),
        ReasoningSigilMode::StripSigil => strip_reasoning_signature_sigil(&raw),
    };
    if processed.is_empty() {
        None
    } else {
        Some(Value::String(processed))
    }
}

fn chat_reasoning_detail_type(extra_body: &HashMap<String, Value>) -> Option<&str> {
    extra_body
        .get(CHAT_REASONING_DETAIL_EXTRA_KEY)
        .and_then(Value::as_object)
        .and_then(|detail| detail.get("type"))
        .and_then(Value::as_str)
}

fn encrypted_reasoning_is_nonempty(encrypted: &Option<Value>) -> bool {
    match encrypted {
        None | Some(Value::Null) => false,
        Some(Value::String(value)) => !value.is_empty(),
        Some(_) => true,
    }
}

fn merge_adjacent_chat_reasoning_for_messages(
    plaintext_node: &Node,
    encrypted_node: &Node,
) -> Option<Node> {
    let Node::Reasoning {
        id: plaintext_id,
        content: plaintext_content,
        encrypted: plaintext_encrypted,
        summary: plaintext_summary,
        source: plaintext_source,
        extra_body: plaintext_extra,
    } = plaintext_node
    else {
        return None;
    };
    let Node::Reasoning {
        id: encrypted_id,
        content: encrypted_content,
        encrypted,
        summary: encrypted_summary,
        source: encrypted_source,
        extra_body: encrypted_extra,
    } = encrypted_node
    else {
        return None;
    };

    if chat_reasoning_detail_type(plaintext_extra) != Some("reasoning.text")
        || chat_reasoning_detail_type(encrypted_extra) != Some("reasoning.encrypted")
        || plaintext_content.as_deref().is_none_or(str::is_empty)
        || encrypted_reasoning_is_nonempty(plaintext_encrypted)
        || encrypted_content.as_deref().is_some_and(|value| !value.is_empty())
        || encrypted_summary.as_deref().is_some_and(|value| !value.is_empty())
        || !encrypted_reasoning_is_nonempty(encrypted)
        || reasoning_is_redacted(plaintext_extra)
        || reasoning_is_redacted(encrypted_extra)
    {
        return None;
    }

    let mut extra_body = plaintext_extra.clone();
    for (key, value) in encrypted_extra {
        extra_body.entry(key.clone()).or_insert_with(|| value.clone());
    }
    Some(Node::Reasoning {
        id: encrypted_id.clone().or_else(|| plaintext_id.clone()),
        content: plaintext_content.clone(),
        encrypted: encrypted.clone(),
        summary: plaintext_summary.clone(),
        source: plaintext_source
            .clone()
            .or_else(|| encrypted_source.clone()),
        extra_body,
    })
}

// Invert the normalization performed in decode/anthropic.rs: internal `Usage.input_tokens`
// is aggregate/inclusive (spec § 5 C3), but Anthropic's wire format requires `input_tokens`
// to exclude cache_read and cache_creation buckets. Saturating subtraction guards against
// malformed upstream data where cache buckets alone exceed the recorded total.
pub(crate) fn anthropic_native_input_tokens(usage: &Usage) -> u64 {
    let cache_read = usage
        .input_details
        .as_ref()
        .map(|d| d.cache_read_tokens)
        .unwrap_or(0);
    let cache_creation = usage
        .input_details
        .as_ref()
        .map(|d| d.cache_creation_tokens)
        .unwrap_or(0);
    usage
        .input_tokens
        .saturating_sub(cache_read)
        .saturating_sub(cache_creation)
}

pub(crate) fn anthropic_native_usage_json(usage: &Usage) -> Value {
    let input_details = usage_input_details(usage);
    let output_details = usage_output_details(usage);
    let mut usage_object = Map::new();

    for (key, value) in &usage.extra_body {
        if !key.starts_with("_monoize_") {
            usage_object.insert(key.clone(), value.clone());
        }
    }

    usage_object.insert(
        "input_tokens".to_string(),
        Value::from(anthropic_native_input_tokens(usage)),
    );
    usage_object.insert(
        "output_tokens".to_string(),
        Value::from(usage.output_tokens),
    );
    usage_object.insert(
        "cache_read_input_tokens".to_string(),
        Value::from(input_details.cache_read_tokens),
    );
    usage_object.insert(
        "cache_creation_input_tokens".to_string(),
        Value::from(input_details.cache_creation_tokens),
    );
    usage_object.insert(
        "tool_prompt_input_tokens".to_string(),
        Value::from(input_details.tool_prompt_tokens),
    );
    usage_object.remove("reasoning_output_tokens");
    let mut native_output_details = usage_object
        .remove("output_tokens_details")
        .and_then(|value| value.as_object().cloned())
        .unwrap_or_default();
    native_output_details.retain(|key, _| !key.starts_with("_monoize_"));
    if usage.output_details.is_some() {
        native_output_details.insert(
            "thinking_tokens".to_string(),
            Value::from(output_details.reasoning_tokens),
        );
    }
    if !native_output_details.is_empty() {
        usage_object.insert(
            "output_tokens_details".to_string(),
            Value::Object(native_output_details),
        );
    }
    usage_object.insert(
        "accepted_prediction_output_tokens".to_string(),
        Value::from(output_details.accepted_prediction_tokens),
    );
    usage_object.insert(
        "rejected_prediction_output_tokens".to_string(),
        Value::from(output_details.rejected_prediction_tokens),
    );

    usage_object.remove("cache_creation");
    if input_details.cache_creation_5m_tokens > 0
        || input_details.cache_creation_1h_tokens > 0
    {
        usage_object.insert(
            "cache_creation".to_string(),
            json!({
                "ephemeral_5m_input_tokens": input_details.cache_creation_5m_tokens,
                "ephemeral_1h_input_tokens": input_details.cache_creation_1h_tokens
            }),
        );
    }

    Value::Object(usage_object)
}

pub fn encode_request(req: &UrpRequest, upstream_model: &str) -> Value {
    let mut system_blocks: Vec<Value> = Vec::new();
    let mut messages: Vec<Value> = Vec::new();
    let request_nodes = &req.input;
    let mut pending_message: Option<AnthropicMessageEnvelope> = None;
    let mut pending_envelope_extra = HashMap::new();

    let mut node_index = 0;
    while node_index < request_nodes.len() {
        let merged_reasoning = request_nodes.get(node_index + 1).and_then(|next| {
            merge_adjacent_chat_reasoning_for_messages(&request_nodes[node_index], next)
        });
        let consumed_nodes = if merged_reasoning.is_some() { 2 } else { 1 };
        let node = merged_reasoning
            .as_ref()
            .unwrap_or(&request_nodes[node_index]);
        match node {
            Node::NextDownstreamEnvelopeExtra { extra_body } => {
                flush_pending_anthropic_message(&mut pending_message, &mut messages);
                for (key, value) in extra_body {
                    pending_envelope_extra.insert(key.clone(), value.clone());
                }
            }
            Node::ToolResult {
                id: _,
                tool_type,
                call_id,
                content,
                is_error,
                extra_body,
            } => {
                if *tool_type == ToolCallType::Custom {
                    node_index += consumed_nodes;
                    continue;
                }
                append_tool_result_to_pending_anthropic_message(
                    &mut pending_message,
                    &mut messages,
                    &mut pending_envelope_extra,
                    call_id,
                    content,
                    *is_error,
                    extra_body,
                );
            }
            Node::Text {
                role: OrdinaryRole::System | OrdinaryRole::Developer,
                ..
            }
            | Node::ProviderItem {
                role: OrdinaryRole::System | OrdinaryRole::Developer,
                origin_protocol: ProviderProtocol::Messages,
                ..
            } => {
                flush_pending_anthropic_message(&mut pending_message, &mut messages);
                if let Some(block) = encode_system_block(node) {
                    system_blocks.push(block);
                }
            }
            Node::Text {
                role: OrdinaryRole::User | OrdinaryRole::Assistant,
                ..
            }
            | Node::Image {
                role: OrdinaryRole::User | OrdinaryRole::Assistant,
                ..
            }
            | Node::File {
                role: OrdinaryRole::User | OrdinaryRole::Assistant,
                ..
            }
            | Node::ProviderItem {
                role: OrdinaryRole::User | OrdinaryRole::Assistant,
                ..
            }
            | Node::Reasoning { .. }
            | Node::ToolCall { .. } => {
                append_node_to_pending_anthropic_message(
                    &mut pending_message,
                    &mut messages,
                    &mut pending_envelope_extra,
                    node,
                    ReasoningSigilMode::StripSigil,
                );
            }
            Node::Image {
                role: OrdinaryRole::System | OrdinaryRole::Developer,
                ..
            }
            | Node::File {
                role: OrdinaryRole::System | OrdinaryRole::Developer,
                ..
            }
            | Node::ProviderItem {
                role: OrdinaryRole::System | OrdinaryRole::Developer,
                ..
            } => {
                flush_pending_anthropic_message(&mut pending_message, &mut messages);
            }
            Node::Audio { .. } | Node::Refusal { .. } => {}
        }
        node_index += consumed_nodes;
    }
    flush_pending_anthropic_message(&mut pending_message, &mut messages);

    let mut body = json!({
        "model": upstream_model,
        "messages": messages,
        "max_tokens": req
            .max_output_tokens
            .unwrap_or(ANTHROPIC_DEFAULT_MAX_TOKENS),
    });
    let obj = body.as_object_mut().expect("anthropic request object");

    if !system_blocks.is_empty() {
        obj.insert("system".to_string(), Value::Array(system_blocks));
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
    if let Some(tools) = &req.tools {
        obj.insert("tools".to_string(), Value::Array(encode_tools(tools)));
    }
    if let Some(choice) = &req.tool_choice {
        obj.insert(
            "tool_choice".to_string(),
            encode_tool_choice_for_anthropic(choice, req.parallel_tool_calls),
        );
    } else if req.parallel_tool_calls == Some(false)
        && req.tools.as_ref().is_some_and(|tools| !tools.is_empty())
    {
        obj.insert(
            "tool_choice".to_string(),
            json!({ "type": "auto", "disable_parallel_tool_use": true }),
        );
    }
    if let Some(reasoning) = &req.reasoning {
        let explicit_thinking = reasoning
            .extra_body
            .get(MESSAGES_THINKING_CONFIG_EXTRA_KEY)
            .and_then(Value::as_object)
            .cloned();
        let explicit_output_config = reasoning
            .extra_body
            .get(MESSAGES_OUTPUT_CONFIG_EXTRA_KEY)
            .and_then(Value::as_object)
            .cloned();
        let has_explicit_messages_config =
            explicit_thinking.is_some() || explicit_output_config.is_some();

        if let Some(thinking) = explicit_thinking {
            obj.insert("thinking".to_string(), Value::Object(thinking));
        }
        if let Some(output_config) = explicit_output_config {
            obj.insert("output_config".to_string(), Value::Object(output_config));
        }

        if !has_explicit_messages_config && model_supports_adaptive(upstream_model) {
            obj.insert("thinking".to_string(), json!({ "type": "adaptive" }));
            if let Some(effort) = reasoning
                .effort
                .as_deref()
                .filter(|effort| !matches!(*effort, "none" | "minimal" | "minimum"))
            {
                obj.insert("output_config".to_string(), json!({ "effort": effort }));
            }
        } else if !has_explicit_messages_config {
            let effort = reasoning.effort.as_deref().unwrap_or("medium");
            obj.insert(
                "thinking".to_string(),
                json!({
                    "type": "enabled",
                    "budget_tokens": effort_to_budget(effort)
                }),
            );
        }
    }
    if let Some(stop) = &req.stop {
        let stops = match stop {
            StopControl::Single(stop) => vec![Value::String(stop.clone())],
            StopControl::Multiple(stops) => stops
                .iter()
                .map(|stop| Value::String(stop.clone()))
                .collect(),
        };
        obj.insert("stop_sequences".to_string(), Value::Array(stops));
    }
    merge_extra(obj, &req.extra_body);
    if let Some(user) = &req.user {
        let metadata = obj
            .entry("metadata".to_string())
            .or_insert_with(|| json!({}));
        if !metadata.is_object() {
            *metadata = json!({});
        }
        metadata
            .as_object_mut()
            .expect("messages metadata object")
            .insert("user_id".to_string(), Value::String(user.clone()));
    }
    if let Some(format) = &req.response_format {
        apply_messages_response_format(obj, format);
    }
    body
}
