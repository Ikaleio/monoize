const ANTHROPIC_DEFAULT_MAX_TOKENS: u64 = 64_000;

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

pub fn encode_request(req: &UrpRequest, upstream_model: &str) -> Value {
    let mut system_blocks: Vec<Value> = Vec::new();
    let mut messages: Vec<Value> = Vec::new();
    let request_nodes = &req.input;
    let mut pending_message: Option<AnthropicMessageEnvelope> = None;

    for node in request_nodes {
        match node {
            Node::NextDownstreamEnvelopeExtra { .. } => {
                flush_pending_anthropic_message(&mut pending_message, &mut messages);
            }
            Node::ToolResult {
                id: _,
                call_id,
                content,
                is_error,
                extra_body,
            } => {
                append_tool_result_to_pending_anthropic_message(
                    &mut pending_message,
                    &mut messages,
                    call_id,
                    content,
                    *is_error,
                    extra_body,
                );
            }
            Node::Text {
                role: OrdinaryRole::System | OrdinaryRole::Developer,
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
        if let Some(effort) = &reasoning.effort {
            if model_supports_adaptive(upstream_model) {
                obj.insert("thinking".to_string(), json!({ "type": "adaptive" }));
                obj.insert("output_config".to_string(), json!({ "effort": effort }));
            } else {
                obj.insert(
                    "thinking".to_string(),
                    json!({
                        "type": "enabled",
                        "budget_tokens": effort_to_budget(effort)
                    }),
                );
            }
        }
    }
    merge_extra(obj, &req.extra_body);
    body
}
