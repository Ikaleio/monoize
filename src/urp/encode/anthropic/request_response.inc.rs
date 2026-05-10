pub fn encode_response(resp: &UrpResponse, logical_model: &str) -> Value {
    let response_nodes = &resp.output;
    let mut content = Vec::new();
    for node in response_nodes {
        if let Some(block) = encode_assistant_response_block(node) {
            content.push(block);
        }
    }

    let mut body = json!({
        "id": resp.id,
        "type": "message",
        "role": "assistant",
        "model": logical_model,
        "content": content,
        "stop_reason": finish_reason_to_stop_reason(resp.finish_reason),
    });

    let mut usage_value = json!({
        "input_tokens": 0,
        "output_tokens": 0,
        "cache_read_input_tokens": 0,
        "cache_creation_input_tokens": 0,
        "tool_prompt_input_tokens": 0,
        "reasoning_output_tokens": 0,
        "accepted_prediction_output_tokens": 0,
        "rejected_prediction_output_tokens": 0
    });
    if let Some(usage) = &resp.usage {
        if let Some(obj) = usage_value.as_object_mut() {
            let input_details = usage_input_details(usage);
            let output_details = usage_output_details(usage);
            obj.insert(
                "input_tokens".to_string(),
                Value::from(anthropic_native_input_tokens(usage)),
            );
            obj.insert(
                "output_tokens".to_string(),
                Value::from(usage.output_tokens),
            );
            obj.insert(
                "cache_read_input_tokens".to_string(),
                Value::from(input_details.cache_read_tokens),
            );
            obj.insert(
                "cache_creation_input_tokens".to_string(),
                Value::from(input_details.cache_creation_tokens),
            );
            obj.insert(
                "tool_prompt_input_tokens".to_string(),
                Value::from(input_details.tool_prompt_tokens),
            );
            obj.insert(
                "reasoning_output_tokens".to_string(),
                Value::from(output_details.reasoning_tokens),
            );
            obj.insert(
                "accepted_prediction_output_tokens".to_string(),
                Value::from(output_details.accepted_prediction_tokens),
            );
            obj.insert(
                "rejected_prediction_output_tokens".to_string(),
                Value::from(output_details.rejected_prediction_tokens),
            );
            for (k, v) in &usage.extra_body {
                obj.insert(k.clone(), v.clone());
            }
        }
    }
    body["usage"] = usage_value;
    if let Some(obj) = body.as_object_mut() {
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
        extra_body: anthropic_message_extra_from_node(node),
    });
    entry.content.push(block);
}

fn anthropic_message_role_for_node(node: &Node) -> Option<OrdinaryRole> {
    match node {
        Node::Text { role, .. }
        | Node::Image { role, .. }
        | Node::File { role, .. }
        | Node::ProviderItem { role, .. } => match role {
            OrdinaryRole::System | OrdinaryRole::Developer => None,
            OrdinaryRole::User | OrdinaryRole::Assistant => Some(*role),
        },
        Node::Reasoning { .. } | Node::ToolCall { .. } => Some(OrdinaryRole::Assistant),
        Node::ToolResult { .. }
        | Node::NextDownstreamEnvelopeExtra { .. }
        | Node::Audio { .. }
        | Node::Refusal { .. } => None,
    }
}

fn anthropic_message_extra_from_node(node: &Node) -> HashMap<String, Value> {
    match node {
        Node::Text {
            phase, extra_body, ..
        } => {
            let mut out = extra_body.clone();
            if let Some(phase) = phase {
                out.insert("phase".to_string(), Value::String(phase.clone()));
            }
            out
        }
        Node::Image { extra_body, .. }
        | Node::File { extra_body, .. }
        | Node::Reasoning { extra_body, .. }
        | Node::ToolCall { extra_body, .. }
        | Node::ProviderItem { extra_body, .. } => extra_body.clone(),
        Node::Audio { .. }
        | Node::Refusal { .. }
        | Node::ToolResult { .. }
        | Node::NextDownstreamEnvelopeExtra { .. } => HashMap::new(),
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
        _ => None,
    }
}

