fn nodes_from_item_value(item: &Value) -> Vec<Node> {
    crate::urp::decode::openai_responses::decode_response(&json!({"output": [item]}))
        .map(|response| response.output.into_iter().filter(|node| {
            !matches!(node, Node::NextDownstreamEnvelopeExtra { .. })
        }).collect())
        .unwrap_or_default()
}

fn node_from_part_value(part: &Value, role: Role, item_id: Option<String>) -> Node {
    let mut node = decode_part_from_value(part).into_node(output_role_to_ordinary(role));
    node.set_id(item_id);
    node
}

fn node_header_from_node(node: &Node) -> NodeHeader {
    match node {
        Node::Text {
            id, role, phase, signature, citations, ..
        } => NodeHeader::Text {
            signature: signature.clone(),
            citations: citations.clone(),
            id: id.clone(),
            role: *role,
            phase: phase.clone(),
        },
        Node::Image { id, role, metadata, .. } => NodeHeader::Image {
            metadata: metadata.clone(),
            id: id.clone(),
            role: *role,
        },
        Node::Audio { id, role, metadata, .. } => NodeHeader::Audio {
            metadata: metadata.clone(),
            id: id.clone(),
            role: *role,
        },
        Node::File { id, role, metadata, .. } => NodeHeader::File {
            metadata: metadata.clone(),
            id: id.clone(),
            role: *role,
        },
        Node::Refusal { id, .. } => NodeHeader::Refusal { id: id.clone() },
        Node::Reasoning { id, metadata, .. } => NodeHeader::Reasoning {
            metadata: metadata.clone(),
            id: id.clone(),
        },
        Node::ToolCall {
            namespace, signature,
            id,
            tool_type,
            call_id,
            name,
            ..
        } => NodeHeader::ToolCall {
            namespace: namespace.clone(), signature: signature.clone(),
            id: id.clone(),
            tool_type: *tool_type,
            call_id: call_id.clone(),
            name: name.clone(),
        },
        Node::ProviderItem {
            body,
            id,
            origin_protocol,
            role,
            item_type,
            ..
        } => NodeHeader::ProviderItem {
            body: Some(body.clone()),
            id: id.clone(),
            origin_protocol: *origin_protocol,
            role: *role,
            item_type: item_type.clone(),
        },
        Node::ToolResult {
            signature, namespace, name,
            id,
            tool_type,
            call_id,
            ..
        } => NodeHeader::ToolResult {
            signature: signature.clone(),
            namespace: namespace.clone(), name: name.clone(),
            id: id.clone(),
            tool_type: *tool_type,
            call_id: call_id.clone(),
        },
        Node::NextDownstreamEnvelopeExtra { .. } => NodeHeader::NextDownstreamEnvelopeExtra,
    }
}

fn node_delta_from_reasoning_event(
    event_name: &str,
    data_val: &Value,
    source: Option<String>,
) -> NodeDelta {
    NodeDelta::Reasoning {
        metadata: Default::default(),
        content: if event_name == "response.reasoning_summary_text.delta" {
            None
        } else {
            data_val
                .get("delta")
                .or_else(|| data_val.get("text"))
                .and_then(|v| v.as_str())
                .filter(|s| !s.is_empty())
                .map(|s| s.to_string())
        },
        encrypted: None,
        summary: if event_name == "response.reasoning_summary_text.delta" {
            data_val
                .get("delta")
                .or_else(|| data_val.get("text"))
                .and_then(|v| v.as_str())
                .filter(|s| !s.is_empty())
                .map(|s| s.to_string())
        } else {
            None
        },
        source,
    }
}

fn emit_pending_envelope_control_if_needed(
    output_index: u64,
    index_state: &mut ResponsesStreamIndexState,
    events: &mut Vec<UrpStreamEvent>,
) {
    let (should_emit, extra_body) = {
        let output_state = output_state_for(index_state, output_index);
        let mut extra_body = output_state.item_extra_body.clone();
        if output_state.item_type.as_deref() == Some("reasoning")
            && !extra_body.contains_key("id")
            && let Some(item_id) = output_state.item_id.as_deref().filter(|id| !id.is_empty())
        {
            extra_body.insert("id".to_string(), Value::String(item_id.to_string()));
        }
        (
            !output_state.control_emitted && !extra_body.is_empty(),
            extra_body,
        )
    };
    if !should_emit {
        return;
    }
    let node_index = index_state.allocate_fresh_node_index();
    events.push(UrpStreamEvent::NodeStart {
        node_index,
        header: NodeHeader::NextDownstreamEnvelopeExtra,
        extra_body: extra_body.clone(),
    });
    events.push(UrpStreamEvent::NodeDone {
        node_index,
        node: Node::NextDownstreamEnvelopeExtra {
            extra_body: extra_body.clone(),
        },
        usage: None,
        extra_body,
    });
    output_state_for(index_state, output_index).control_emitted = true;
}

fn map_output_item_added(
    data_val: Value,
    index_state: &mut ResponsesStreamIndexState,
) -> Vec<UrpStreamEvent> {
    let Some(item) = data_val.get("item") else {
        return Vec::new();
    };
    let output_index = data_val
        .get("output_index")
        .and_then(|v| v.as_u64())
        .unwrap_or(0);
    let item_type = item.get("type").and_then(|v| v.as_str()).unwrap_or("");
    let role = role_from_item(item);
    let item_extra_body = item_extra_body_from_value(item);
    let mut events = Vec::new();

    {
        let output_state = index_state
            .output_state_by_index
            .entry(output_index)
            .or_default();
        output_state.item_type = Some(item_type.to_string());
        output_state.role = Some(role);
        output_state.message_phase = extract_responses_message_phase(item);
        if let Some(id) = item.get("id").and_then(Value::as_str) {
            output_state.item_id = Some(id.to_string());
        } else if item_type == "message" && output_state.item_id.is_none() {
            output_state.item_id = Some(crate::urp::synthetic_message_id());
        }
        if output_state.item_extra_body.is_empty() {
            output_state.item_extra_body = item_extra_body.clone();
        } else {
            for (key, value) in item_extra_body.clone() {
                output_state.item_extra_body.entry(key).or_insert(value);
            }
        }
        if item_type == "reasoning" {
            merge_reasoning_source(
                &mut output_state.reasoning_source,
                reasoning_source_from_value(item),
            );
        }
    }

    match item_type {
        "reasoning" => {
            let node = first_node_from_item_value(item).unwrap_or_else(|| Node::Reasoning {
                metadata: Default::default(),
                id: item
                    .get("id")
                    .and_then(Value::as_str)
                    .map(|s| s.to_string()),
                content: None,
                encrypted: None,
                summary: None,
                source: None,
                extra_body: part_extra_body_from_value(item),
            });
            let node_index = index_state.synthetic_node_index_for_output(output_index);
            emit_pending_envelope_control_if_needed(output_index, index_state, &mut events);
            events.push(UrpStreamEvent::NodeStart {
                node_index,
                header: node_header_from_node(&node),
                extra_body: part_extra_body_from_value(item),
            });
            output_state_for(index_state, output_index).emitted_any_node = true;
        }
        "message" => {}
        "function_call" | "custom_tool_call" => {
            let tool_type = if item_type == "custom_tool_call" {
                ToolCallType::Custom
            } else {
                ToolCallType::Function
            };
            let node = first_node_from_item_value(item).unwrap_or_else(|| Node::ToolCall {
                namespace: item.get("namespace").and_then(Value::as_str).map(str::to_string),
                signature: None,
                id: item
                    .get("id")
                    .and_then(|v| v.as_str())
                    .map(|s| s.to_string())
                    .or_else(|| Some(crate::urp::synthetic_tool_call_id())),
                tool_type,
                call_id: item
                    .get("call_id")
                    .and_then(|v| v.as_str())
                    .unwrap_or_default()
                    .to_string(),
                name: item
                    .get("name")
                    .and_then(|v| v.as_str())
                    .unwrap_or_default()
                    .to_string(),
                arguments: item
                    .get(if tool_type == ToolCallType::Custom {
                        "input"
                    } else {
                        "arguments"
                    })
                    .and_then(|v| v.as_str())
                    .unwrap_or_default()
                    .to_string(),
                extra_body: part_extra_body_from_value(item),
            });
            let node_index = index_state.synthetic_node_index_for_output(output_index);
            emit_pending_envelope_control_if_needed(output_index, index_state, &mut events);
            events.push(UrpStreamEvent::NodeStart {
                node_index,
                header: node_header_from_node(&node),
                extra_body: part_extra_body_from_value(item),
            });
            output_state_for(index_state, output_index).emitted_any_node = true;
        }
        "function_call_output" | "custom_tool_call_output" => {
            let tool_type = if item_type == "custom_tool_call_output" {
                ToolCallType::Custom
            } else {
                ToolCallType::Function
            };
            let node = first_node_from_item_value(item).unwrap_or_else(|| Node::ToolResult {
                signature: None,
                namespace: item.get("namespace").and_then(Value::as_str).map(str::to_string),
                name: item.get("name").and_then(Value::as_str).map(str::to_string),
                id: item
                    .get("id")
                    .and_then(|v| v.as_str())
                    .map(|s| s.to_string())
                    .or_else(|| Some(crate::urp::synthetic_tool_result_id())),
                tool_type,
                call_id: item
                    .get("call_id")
                    .or_else(|| item.get("id"))
                    .and_then(|v| v.as_str())
                    .unwrap_or_default()
                    .to_string(),
                is_error: false,
                content: Vec::new(),
                extra_body: item_extra_body_from_value(item),
            });
            let node_index = index_state.synthetic_node_index_for_output(output_index);
            emit_pending_envelope_control_if_needed(output_index, index_state, &mut events);
            events.push(UrpStreamEvent::NodeStart {
                node_index,
                header: node_header_from_node(&node),
                extra_body: item_extra_body_from_value(item),
            });
            output_state_for(index_state, output_index).emitted_any_node = true;
        }
        "image_generation_call" => {
            let node_index = index_state.synthetic_node_index_for_output(output_index);
            emit_pending_envelope_control_if_needed(output_index, index_state, &mut events);
            events.push(UrpStreamEvent::NodeStart {
                node_index,
                header: NodeHeader::Image {
                    metadata: Default::default(),
                    id: item.get("id").and_then(Value::as_str).map(str::to_string),
                    role: OrdinaryRole::Assistant,
                },
                extra_body: item_extra_body,
            });
            output_state_for(index_state, output_index).emitted_any_node = true;
        }
        _ => {
            let node = first_node_from_item_value(item).unwrap_or_else(|| Node::ProviderItem {
                id: item
                    .get("id")
                    .and_then(Value::as_str)
                    .map(|s| s.to_string())
                    .or_else(|| Some(crate::urp::synthetic_provider_item_id())),
                origin_protocol: ProviderProtocol::Responses,
                role: OrdinaryRole::Assistant,
                item_type: item_type.to_string(),
                body: item.clone(),
                extra_body: HashMap::new(),
            });
            let node_index = index_state.synthetic_node_index_for_output(output_index);
            emit_pending_envelope_control_if_needed(output_index, index_state, &mut events);
            events.push(UrpStreamEvent::NodeStart {
                node_index,
                header: node_header_from_node(&node),
                extra_body: item_extra_body_from_value(item),
            });
            output_state_for(index_state, output_index).emitted_any_node = true;
        }
    }

    events
}

fn map_content_part_added(
    data_val: Value,
    index_state: &mut ResponsesStreamIndexState,
) -> Vec<UrpStreamEvent> {
    let Some(part) = data_val.get("part") else {
        return Vec::new();
    };
    let output_index = data_val
        .get("output_index")
        .and_then(|v| v.as_u64())
        .unwrap_or(0);
    let content_index = data_val
        .get("content_index")
        .or_else(|| data_val.get("part_index"))
        .and_then(|v| v.as_u64())
        .unwrap_or(0);
    let is_reasoning_part = part.get("type").and_then(Value::as_str) == Some("reasoning_text")
        || index_state
            .output_state_by_index
            .get(&output_index)
            .and_then(|state| state.item_type.as_deref())
            == Some("reasoning");
    let node_index = if is_reasoning_part {
        index_state.synthetic_node_index_for_output(output_index)
    } else {
        index_state.node_index_for_content(output_index, content_index)
    };
    if is_reasoning_part
        && index_state
            .output_state_by_index
            .get(&output_index)
            .is_some_and(|state| state.emitted_any_node)
    {
        return Vec::new();
    }
    let role = output_state_for(index_state, output_index)
        .role
        .unwrap_or(Role::Assistant);

    let mut events = Vec::new();
    let item_id = if is_reasoning_part {
        output_state_for(index_state, output_index).item_id.clone()
    } else {
        Some(stable_message_item_id_for_output(index_state, output_index))
    };
    let mut node = node_from_part_value(part, role, item_id);
    if let Node::ProviderItem {id,extra_body,..} = &mut node {
        *id = part.get("id").and_then(Value::as_str).map(str::to_owned);
        extra_body.insert(crate::urp::decode::openai_responses::RESPONSES_CONTENT_PART_SHAPE_KEY.into(), Value::Bool(true));
    }
    if let Node::Text { phase, citations, .. } = &mut node {
        let state = output_state_for(index_state, output_index);
        *phase = state.message_phase.clone();
        *citations = state.text_citations.iter()
            .filter(|((index, _), _)| *index == content_index)
            .map(|(_, value)| value.clone()).collect();
    }
    if !is_reasoning_part {
        output_state_for(index_state, output_index).content_nodes.insert(content_index, node.clone());
    }
    emit_pending_envelope_control_if_needed(output_index, index_state, &mut events);
    events.push(UrpStreamEvent::NodeStart {
        node_index,
        header: node_header_from_node(&node),
        extra_body: node.extra_body_mut().clone(),
    });
    output_state_for(index_state, output_index).emitted_any_node = true;
    events
}

fn map_content_part_done(
    data_val: Value,
    index_state: &mut ResponsesStreamIndexState,
) -> Vec<UrpStreamEvent> {
    let Some(part) = data_val.get("part") else {
        return Vec::new();
    };
    let output_index = data_val
        .get("output_index")
        .and_then(|v| v.as_u64())
        .unwrap_or(0);
    let content_index = data_val
        .get("content_index")
        .or_else(|| data_val.get("part_index"))
        .and_then(|v| v.as_u64())
        .unwrap_or(0);
    let is_reasoning_part = part.get("type").and_then(Value::as_str) == Some("reasoning_text")
        || index_state
            .output_state_by_index
            .get(&output_index)
            .and_then(|state| state.item_type.as_deref())
            == Some("reasoning");
    let node_index = if is_reasoning_part {
        index_state.synthetic_node_index_for_output(output_index)
    } else {
        index_state.node_index_for_content(output_index, content_index)
    };
    let role = output_state_for(index_state, output_index)
        .role
        .unwrap_or(Role::Assistant);
    let item_id = if is_reasoning_part {
        output_state_for(index_state, output_index).item_id.clone()
    } else {
        Some(stable_message_item_id_for_output(index_state, output_index))
    };
    let mut node = node_from_part_value(part, role, item_id);
    if let Node::ProviderItem {id,extra_body,..} = &mut node {
        *id = part.get("id").and_then(Value::as_str).map(str::to_owned);
        extra_body.insert(crate::urp::decode::openai_responses::RESPONSES_CONTENT_PART_SHAPE_KEY.into(), Value::Bool(true));
    }
    if let Node::Text { phase, citations, .. } = &mut node {
        let state = output_state_for(index_state, output_index);
        *phase = state.message_phase.clone();
        *citations = state.text_citations.iter()
            .filter(|((index, _), _)| *index == content_index)
            .map(|(_, value)| value.clone()).collect();
    }
    if let Some(output_state) = index_state.output_state_by_index.get_mut(&output_index) {
        output_state.part_done_seen = true;
        if !is_reasoning_part { output_state.content_nodes.insert(content_index, node.clone()); }
    }
    if is_reasoning_part {
        return Vec::new();
    }
    vec![UrpStreamEvent::NodeDone {
        node_index,
        extra_body: node.extra_body_mut().clone(),
        node,
        usage: None,
    }]
}
