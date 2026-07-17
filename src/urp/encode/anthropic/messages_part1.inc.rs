fn encode_regular_message_block(node: &Node, sigil_mode: ReasoningSigilMode) -> Option<Value> {
    match node {
        Node::Text {
            content,
            phase,
            extra_body,
            ..
        } => {
            if content.is_empty() && phase.is_none() && extra_body.is_empty() {
                return None;
            }
            let mut block = json!({ "type": "text", "text": content });
            if let Some(obj) = block.as_object_mut() {
                if let Some(phase) = phase {
                    obj.insert("phase".to_string(), Value::String(phase.clone()));
                }
                merge_extra(obj, extra_body);
            }
            Some(block)
        }
        Node::Image {
            source, extra_body, ..
        } => {
            let mut block = encode_anthropic_image(source)?;
            if let Some(obj) = block.as_object_mut() {
                merge_extra(obj, extra_body);
            }
            Some(block)
        }
        Node::File {
            source, extra_body, ..
        } => {
            let mut block = encode_anthropic_file(source)?;
            if let Some(obj) = block.as_object_mut() {
                merge_extra(obj, extra_body);
            }
            Some(block)
        }
        Node::Reasoning {
            id,
            content,
            summary,
            encrypted,
            extra_body,
            ..
        } => {
            let wire_extra = reasoning_extra_for_wire(extra_body);
            let signature = encoded_signature_value(encrypted, id, sigil_mode);
            if reasoning_is_redacted(extra_body) {
                let data = signature?;
                let mut block = json!({ "type": "redacted_thinking", "data": data });
                let obj = block
                    .as_object_mut()
                    .expect("redacted_thinking block object");
                merge_extra(obj, &wire_extra);
                Some(block)
            } else if let Some(text) = plaintext_from_reasoning(content, summary) {
                let mut block = json!({ "type": "thinking", "thinking": text });
                let obj = block.as_object_mut().expect("thinking block object");
                if let Some(sig) = signature {
                    obj.insert("signature".to_string(), sig);
                }
                merge_extra(obj, &wire_extra);
                Some(block)
            } else {
                let signature = signature?;
                let mut block =
                    json!({ "type": "thinking", "thinking": "", "signature": signature });
                let obj = block.as_object_mut().expect("thinking block object");
                merge_extra(obj, &wire_extra);
                Some(block)
            }
        }
        Node::ToolCall {
            id: _,
            call_id,
            name,
            arguments,
            extra_body,
        } => {
            let input = serde_json::from_str::<Value>(arguments)
                .unwrap_or_else(|_| json!({ "_raw": arguments }));
            let mut block = json!({
                "type": "tool_use",
                "id": call_id,
                "name": name,
                "input": input
            });
            if let Some(obj) = block.as_object_mut() {
                merge_extra(obj, extra_body);
            }
            Some(block)
        }
        Node::ProviderItem {
            origin_protocol,
            item_type,
            body,
            extra_body,
            ..
        } => encode_messages_provider_block(*origin_protocol, item_type, body, extra_body),
        Node::Audio { .. }
        | Node::Refusal { .. }
        | Node::ToolResult { .. }
        | Node::NextDownstreamEnvelopeExtra { .. } => None,
    }
}

fn encode_assistant_response_block(node: &Node) -> Option<Value> {
    match node {
        Node::Text {
            role: OrdinaryRole::Assistant,
            content,
            phase,
            extra_body,
            ..
        } => {
            if content.is_empty() && phase.is_none() && extra_body.is_empty() {
                return None;
            }
            let mut block = json!({ "type": "text", "text": content });
            if let Some(obj) = block.as_object_mut() {
                if let Some(phase) = phase {
                    obj.insert("phase".to_string(), Value::String(phase.clone()));
                }
                merge_extra(obj, extra_body);
            }
            Some(block)
        }
        Node::Reasoning {
            id,
            content,
            summary,
            encrypted,
            extra_body,
            ..
        } => {
            let wire_extra = reasoning_extra_for_wire(extra_body);
            let signature = encoded_signature_value(encrypted, id, ReasoningSigilMode::EmbedSigil);
            if reasoning_is_redacted(extra_body) {
                let data = signature?;
                let mut block = Map::new();
                block.insert(
                    "type".to_string(),
                    Value::String("redacted_thinking".to_string()),
                );
                block.insert("data".to_string(), data);
                merge_extra(&mut block, &wire_extra);
                Some(Value::Object(block))
            } else if let Some(text) = plaintext_from_reasoning(content, summary) {
                let mut thinking = Map::new();
                thinking.insert("type".to_string(), Value::String("thinking".to_string()));
                thinking.insert("thinking".to_string(), Value::String(text.to_string()));
                if let Some(sig) = signature {
                    thinking.insert("signature".to_string(), sig);
                }
                merge_extra(&mut thinking, &wire_extra);
                Some(Value::Object(thinking))
            } else {
                let signature = signature?;
                let mut block = Map::new();
                block.insert("type".to_string(), Value::String("thinking".to_string()));
                block.insert("thinking".to_string(), Value::String(String::new()));
                block.insert("signature".to_string(), signature);
                merge_extra(&mut block, &wire_extra);
                Some(Value::Object(block))
            }
        }
        Node::ToolCall {
            id: _,
            call_id,
            name,
            arguments,
            extra_body,
        } => {
            let input = serde_json::from_str::<Value>(arguments)
                .unwrap_or_else(|_| json!({ "_raw": arguments }));
            let mut block = json!({
                "type": "tool_use",
                "id": call_id,
                "name": name,
                "input": input
            });
            if let Some(obj) = block.as_object_mut() {
                merge_extra(obj, extra_body);
            }
            Some(block)
        }
        Node::Image {
            role: OrdinaryRole::Assistant,
            source,
            extra_body,
            ..
        } => {
            let mut block = encode_anthropic_image(source)?;
            if let Some(obj) = block.as_object_mut() {
                merge_extra(obj, extra_body);
            }
            Some(block)
        }
        Node::File {
            role: OrdinaryRole::Assistant,
            source,
            extra_body,
            ..
        } => {
            let mut block = encode_anthropic_file(source)?;
            if let Some(obj) = block.as_object_mut() {
                merge_extra(obj, extra_body);
            }
            Some(block)
        }
        Node::ProviderItem {
            role: OrdinaryRole::Assistant,
            origin_protocol,
            item_type,
            body,
            extra_body,
            ..
        } => encode_messages_provider_block(*origin_protocol, item_type, body, extra_body),
        Node::Audio { .. }
        | Node::Refusal { .. }
        | Node::ToolResult { .. }
        | Node::NextDownstreamEnvelopeExtra { .. }
        | Node::Text { .. }
        | Node::Image { .. }
        | Node::File { .. }
        | Node::ProviderItem { .. } => None,
    }
}

fn encode_tool_result_block(
    call_id: &str,
    content: &[ToolResultContent],
    is_error: bool,
    extra_body: &HashMap<String, Value>,
) -> Value {
    let mut content: Vec<Value> = content
        .iter()
        .filter_map(|item| match item {
            ToolResultContent::Text { text, extra_body } => {
                let mut block = json!({ "type": "text", "text": text });
                merge_extra(block.as_object_mut()?, extra_body);
                Some(block)
            }
            ToolResultContent::Image { source, extra_body } => {
                let mut block = encode_anthropic_image(source)?;
                merge_extra(block.as_object_mut()?, extra_body);
                Some(block)
            }
            ToolResultContent::File { source, extra_body } => {
                let mut block = encode_anthropic_file(source)?;
                merge_extra(block.as_object_mut()?, extra_body);
                Some(block)
            }
            ToolResultContent::ProviderItem {
                origin_protocol,
                item_type,
                body,
                extra_body,
            } => encode_messages_provider_block(*origin_protocol, item_type, body, extra_body),
        })
        .collect();
    if content.is_empty() {
        content.push(json!({ "type": "text", "text": "" }));
    }
    let mut tool_result_block = json!({
        "type": "tool_result",
        "tool_use_id": call_id,
        "is_error": is_error,
        "content": content
    });
    if let Some(obj) = tool_result_block.as_object_mut() {
        merge_extra(obj, extra_body);
    }
    tool_result_block
}
