fn map_image_generation_completed(
    data_val: Value,
    index_state: &mut ResponsesStreamIndexState,
) -> Vec<UrpStreamEvent> {
    let Some(node) = image_node_from_image_generation_payload(&data_val) else {
        return Vec::new();
    };
    let node_index = index_state.allocate_fresh_node_index();
    let extra_body = split_known_fields(
        data_val,
        &[
            "type",
            "id",
            "b64_json",
            "result",
            "output_format",
            "partial_image_index",
        ],
    );
    vec![
        UrpStreamEvent::NodeStart {
            node_index,
            header: node_header_from_node(&node),
            extra_body: extra_body.clone(),
        },
        UrpStreamEvent::NodeDone {
            node_index,
            node,
            usage: None,
            extra_body,
        },
    ]
}

fn map_output_item_done(
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
    let mut events = Vec::new();

    match item_type {
        "function_call_output" => {
            let node = first_node_from_item_value(item).unwrap_or_else(|| Node::ToolResult {
                id: item
                    .get("id")
                    .and_then(|v| v.as_str())
                    .map(|s| s.to_string())
                    .or_else(|| Some(crate::urp::synthetic_tool_result_id())),
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
            events.push(UrpStreamEvent::NodeDone {
                node_index,
                node,
                usage: None,
                extra_body: item_extra_body_from_value(item),
            });
        }
        "reasoning" | "function_call" => {
            let role = output_state_for(index_state, output_index)
                .role
                .unwrap_or(Role::Assistant);
            let node = first_node_from_item_value(item).unwrap_or_else(|| {
                node_from_part_value(
                    item,
                    role,
                    output_state_for(index_state, output_index)
                        .item_id
                        .clone()
                        .or_else(|| {
                            item.get("id")
                                .and_then(Value::as_str)
                                .map(|s| s.to_string())
                        }),
                )
            });
            let node_index = index_state.synthetic_node_index_for_output(output_index);
            let state = index_state.output_state_by_index.get(&output_index);
            let emitted_any_node = state.map(|state| state.emitted_any_node).unwrap_or(false);
            if !emitted_any_node {
                let reasoning_text_delta_seen = state
                    .map(|state| state.reasoning_text_delta_seen)
                    .unwrap_or(false);
                let reasoning_summary_delta_seen = state
                    .map(|state| state.reasoning_summary_delta_seen)
                    .unwrap_or(false);
                let reasoning_item_id = item
                    .get("id")
                    .and_then(Value::as_str)
                    .map(|s| s.to_string())
                    .or_else(|| state.and_then(|state| state.item_id.clone()));
                if let Node::Reasoning {
                    content,
                    encrypted,
                    summary,
                    source,
                    ..
                } = &node
                {
                    let fallback_content = (!reasoning_text_delta_seen)
                        .then(|| content.clone())
                        .flatten();
                    let fallback_summary = (!reasoning_summary_delta_seen)
                        .then(|| summary.clone())
                        .flatten();
                    let fallback_encrypted = encrypted.clone();
                    if fallback_content
                        .as_deref()
                        .is_some_and(|content| !content.is_empty())
                        || fallback_summary
                            .as_deref()
                            .is_some_and(|summary| !summary.is_empty())
                        || fallback_encrypted.is_some()
                    {
                        let mut delta_extra = HashMap::new();
                        if let Some(id) = &reasoning_item_id {
                            delta_extra
                                .insert("reasoning_item_id".to_string(), Value::String(id.clone()));
                        }
                        events.push(UrpStreamEvent::NodeDelta {
                            node_index,
                            delta: NodeDelta::Reasoning {
                                content: fallback_content,
                                encrypted: fallback_encrypted,
                                summary: fallback_summary,
                                source: source.clone(),
                            },
                            usage: None,
                            extra_body: delta_extra,
                        });
                    }
                }
                events.push(UrpStreamEvent::NodeDone {
                    node_index,
                    node: node.clone(),
                    usage: None,
                    extra_body: part_extra_body_from_value(item),
                });
            }
        }
        "message" => {
            let (part_done_seen, emitted_any_node) = index_state
                .output_state_by_index
                .get(&output_index)
                .map(|state| (state.part_done_seen, state.emitted_any_node))
                .unwrap_or((false, false));
            if !part_done_seen && !emitted_any_node {
                let decoded_item = decode_item_from_value(item);
                if let Item::Message { .. } = decoded_item {
                    let nodes = nodes_from_item_value(item);
                    for node in nodes {
                        let node_index = index_state.allocate_fresh_node_index();
                        emit_pending_envelope_control_if_needed(
                            output_index,
                            index_state,
                            &mut events,
                        );
                        events.push(UrpStreamEvent::NodeStart {
                            node_index,
                            header: node_header_from_node(&node),
                            extra_body: item_extra_body_from_value(item),
                        });
                        output_state_for(index_state, output_index).emitted_any_node = true;
                        events.push(UrpStreamEvent::NodeDone {
                            node_index,
                            node,
                            usage: None,
                            extra_body: HashMap::new(),
                        });
                    }
                }
            }
        }
        _ => {
            let emitted_any_node = index_state
                .output_state_by_index
                .get(&output_index)
                .map(|state| state.emitted_any_node)
                .unwrap_or(false);
            if !emitted_any_node {
                let role = output_state_for(index_state, output_index)
                    .role
                    .unwrap_or(Role::Assistant);
                let node = first_node_from_item_value(item).unwrap_or_else(|| {
                    node_from_part_value(
                        item,
                        role,
                        output_state_for(index_state, output_index)
                            .item_id
                            .clone()
                            .or_else(|| {
                                item.get("id")
                                    .and_then(Value::as_str)
                                    .map(|s| s.to_string())
                            }),
                    )
                });
                let node_index = index_state.synthetic_node_index_for_output(output_index);
                events.push(UrpStreamEvent::NodeDone {
                    node_index,
                    node,
                    usage: None,
                    extra_body: part_extra_body_from_value(item),
                });
            }
        }
    }

    events
}

fn merge_response_completed_outputs(
    terminal_outputs: Vec<Node>,
    accumulated_outputs: &[Node],
) -> Vec<Node> {
    if accumulated_outputs.is_empty() {
        return terminal_outputs;
    }

    let mut merged = accumulated_outputs.to_vec();
    for terminal in terminal_outputs {
        if let Some(index) = merged
            .iter()
            .position(|candidate| response_completed_nodes_match(candidate, &terminal))
        {
            merged[index] = terminal;
        } else {
            merged.push(terminal);
        }
    }

    merged
}

fn response_completed_nodes_match(left: &Node, right: &Node) -> bool {
    match (left, right) {
        (
            Node::Text {
                id: left_id,
                role: left_role,
                phase: left_phase,
                content: left_content,
                ..
            },
            Node::Text {
                id: right_id,
                role: right_role,
                phase: right_phase,
                content: right_content,
                ..
            },
        ) => {
            (left_id.is_some() && left_id == right_id)
                || (left_role == right_role
                    && left_phase == right_phase
                    && left_content == right_content)
        }
        (Node::Image { id: left_id, .. }, Node::Image { id: right_id, .. })
        | (Node::Audio { id: left_id, .. }, Node::Audio { id: right_id, .. })
        | (Node::File { id: left_id, .. }, Node::File { id: right_id, .. })
        | (Node::Refusal { id: left_id, .. }, Node::Refusal { id: right_id, .. })
        | (Node::ProviderItem { id: left_id, .. }, Node::ProviderItem { id: right_id, .. }) => {
            left_id.is_some() && left_id == right_id
        }
        (
            Node::Reasoning {
                id: left_id,
                content: left_content,
                encrypted: left_encrypted,
                summary: left_summary,
                source: left_source,
                extra_body: left_extra_body,
            },
            Node::Reasoning {
                id: right_id,
                content: right_content,
                encrypted: right_encrypted,
                summary: right_summary,
                source: right_source,
                extra_body: right_extra_body,
            },
        ) => {
            (left_id.is_some() && left_id == right_id)
                || (left_content == right_content
                    && left_encrypted == right_encrypted
                    && left_summary == right_summary
                    && left_source == right_source
                    && left_extra_body == right_extra_body)
        }
        (
            Node::ToolCall {
                id: left_id,
                call_id: left_call_id,
                ..
            },
            Node::ToolCall {
                id: right_id,
                call_id: right_call_id,
                ..
            },
        )
        | (
            Node::ToolResult {
                id: left_id,
                call_id: left_call_id,
                ..
            },
            Node::ToolResult {
                id: right_id,
                call_id: right_call_id,
                ..
            },
        ) => left_call_id == right_call_id || (left_id.is_some() && left_id == right_id),
        _ => false,
    }
}

