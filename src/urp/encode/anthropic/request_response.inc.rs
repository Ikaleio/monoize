pub fn encode_response(resp: &UrpResponse, logical_model: &str) -> Value {
    let response_nodes = &resp.output;
    let mut content = Vec::new();
    let mut envelope_extra = HashMap::new();
    let mut envelope_open = false;
    for node in response_nodes {
        if let Node::NextDownstreamEnvelopeExtra { extra_body } = node {
            if !envelope_open {
                for (key, value) in extra_body {
                    envelope_extra.insert(key.clone(), value.clone());
                }
            }
            continue;
        }
        if let Some(block) = encode_assistant_response_block(node) {
            envelope_open = true;
            content.push(block);
        }
    }

    let stop_reason = resp
        .extra_body
        .get("stop_reason")
        .and_then(Value::as_str)
        .filter(|reason| !reason.is_empty())
        .unwrap_or_else(|| finish_reason_to_stop_reason(resp.finish_reason));
    let mut body = json!({
        "id": resp.id,
        "type": "message",
        "role": "assistant",
        "model": logical_model,
        "content": content,
        "stop_reason": stop_reason,
    });

    let usage = resp.usage.clone().unwrap_or(Usage {
        input_tokens: 0,
        output_tokens: 0,
        input_details: None,
        output_details: None,
        extra_body: HashMap::new(),
    });
    body["usage"] = anthropic_native_usage_json(&usage);
    if let Some(obj) = body.as_object_mut() {
        merge_extra(obj, &envelope_extra);
        merge_extra(obj, &resp.extra_body);
    }
    body
}

#[derive(Clone)]
struct AnthropicMessageEnvelope {
    role: OrdinaryRole,
    content: Vec<Value>,
    extra_body: HashMap<String, Value>,
}

fn flush_pending_anthropic_message(
    pending: &mut Option<AnthropicMessageEnvelope>,
    out: &mut Vec<Value>,
) {
    let Some(message) = pending.take() else {
        return;
    };
    if message.content.is_empty() {
        return;
    }

    let role = match message.role {
        OrdinaryRole::Assistant => "assistant",
        OrdinaryRole::User | OrdinaryRole::System | OrdinaryRole::Developer => "user",
    };
    let mut msg = json!({ "role": role, "content": message.content });
    if let Some(obj) = msg.as_object_mut() {
        merge_extra(obj, &message.extra_body);
    }
    out.push(msg);
}

fn append_node_to_pending_anthropic_message(
    pending: &mut Option<AnthropicMessageEnvelope>,
    out: &mut Vec<Value>,
    pending_envelope_extra: &mut HashMap<String, Value>,
    node: &Node,
    sigil_mode: ReasoningSigilMode,
) {
    let Some(role) = anthropic_message_role_for_node(node) else {
        return;
    };
    let should_flush = pending
        .as_ref()
        .is_some_and(|existing| existing.role != role);
    if should_flush {
        flush_pending_anthropic_message(pending, out);
    }

    let Some(block) = encode_regular_message_block(node, sigil_mode) else {
        return;
    };
    let entry = pending.get_or_insert_with(|| AnthropicMessageEnvelope {
        role,
        content: Vec::new(),
        extra_body: std::mem::take(pending_envelope_extra),
    });
    entry.content.push(block);
}

fn append_tool_result_to_pending_anthropic_message(
    pending: &mut Option<AnthropicMessageEnvelope>,
    out: &mut Vec<Value>,
    pending_envelope_extra: &mut HashMap<String, Value>,
    call_id: &str,
    content: &[ToolResultContent],
    is_error: bool,
    extra_body: &HashMap<String, Value>,
) {
    let should_flush = pending.as_ref().is_some_and(|existing| {
        existing.role != OrdinaryRole::User
            || existing
                .content
                .iter()
                .any(|block| block.get("type").and_then(Value::as_str) != Some("tool_result"))
    });
    if should_flush {
        flush_pending_anthropic_message(pending, out);
    }

    let entry = pending.get_or_insert_with(|| AnthropicMessageEnvelope {
        role: OrdinaryRole::User,
        content: Vec::new(),
        extra_body: std::mem::take(pending_envelope_extra),
    });
    entry.content.push(encode_tool_result_block(
        call_id, content, is_error, extra_body,
    ));
}

fn anthropic_message_role_for_node(node: &Node) -> Option<OrdinaryRole> {
    match node {
        Node::Text { role, .. } | Node::Image { role, .. } | Node::File { role, .. } => {
            match role {
                OrdinaryRole::System | OrdinaryRole::Developer => None,
                OrdinaryRole::User | OrdinaryRole::Assistant => Some(*role),
            }
        }
        Node::ProviderItem {
            role,
            origin_protocol: ProviderProtocol::Messages,
            ..
        } => match role {
            OrdinaryRole::System | OrdinaryRole::Developer => None,
            OrdinaryRole::User | OrdinaryRole::Assistant => Some(*role),
        },
        Node::Reasoning { .. } | Node::ToolCall { .. } => Some(OrdinaryRole::Assistant),
        Node::ToolResult { .. }
        | Node::NextDownstreamEnvelopeExtra { .. }
        | Node::Audio { .. }
        | Node::Refusal { .. }
        | Node::ProviderItem { .. } => None,
    }
}

fn encode_system_block(node: &Node) -> Option<Value> {
    match node {
        Node::Text {
            content,
            phase,
            extra_body,
            ..
        } if !content.is_empty() => {
            let mut block = json!({ "type": "text", "text": content });
            if let Some(obj) = block.as_object_mut() {
                if let Some(phase) = phase {
                    obj.insert("phase".to_string(), Value::String(phase.clone()));
                }
                merge_extra(obj, extra_body);
            }
            Some(block)
        }
        Node::ProviderItem {
            role: OrdinaryRole::System | OrdinaryRole::Developer,
            origin_protocol,
            item_type,
            body,
            extra_body,
            ..
        } => encode_messages_provider_block(*origin_protocol, item_type, body, extra_body),
        _ => None,
    }
}
