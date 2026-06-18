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
            let state = index_state
                .output_state_by_index
                .get(&output_index)
                .cloned()
                .unwrap_or_default();
            if state.node_done_seen {
                return events;
            }
            if !state.emitted_any_node {
                emit_pending_envelope_control_if_needed(output_index, index_state, &mut events);
                events.push(UrpStreamEvent::NodeStart {
                    node_index,
                    header: node_header_from_node(&node),
                    extra_body: part_extra_body_from_value(item),
                });
                output_state_for(index_state, output_index).emitted_any_node = true;
            }
            if let Node::Reasoning {
                content,
                encrypted,
                summary,
                source,
                ..
            } = &node
            {
                let fallback_content = (!state.reasoning_text_delta_seen)
                    .then(|| content.clone())
                    .flatten();
                let fallback_summary = (!state.reasoning_summary_delta_seen)
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
                    if let Some(id) = item
                        .get("id")
                        .and_then(Value::as_str)
                        .map(str::to_string)
                        .or_else(|| state.item_id.clone())
                    {
                        delta_extra.insert("reasoning_item_id".to_string(), Value::String(id));
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
            } else if let Node::ToolCall { arguments, .. } = &node
                && !state.function_arguments_delta_seen
                && !arguments.is_empty()
            {
                events.push(UrpStreamEvent::NodeDelta {
                    node_index,
                    delta: NodeDelta::ToolCallArguments {
                        arguments: arguments.clone(),
                    },
                    usage: None,
                    extra_body: split_known_fields(
                        data_val.clone(),
                        &["item", "output_index", "content_index", "part_index"],
                    ),
                });
            }
            events.push(UrpStreamEvent::NodeDone {
                node_index,
                node: node.clone(),
                usage: None,
                extra_body: part_extra_body_from_value(item),
            });
            output_state_for(index_state, output_index).node_done_seen = true;
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
                if !node_is_empty_text(&node) {
                    events.push(UrpStreamEvent::NodeDone {
                        node_index,
                        node,
                        usage: None,
                        extra_body: part_extra_body_from_value(item),
                    });
                }
            }
        }
    }

    events
}

fn merge_response_completed_outputs(
    terminal_outputs: Vec<AccumulatedOutputEntry>,
    accumulated_outputs: &[AccumulatedOutputEntry],
) -> Result<Vec<Node>, String> {
    let mut merged_entries = accumulated_outputs.to_vec();
    let mut used_accumulated_indices: Vec<usize> = Vec::new();

    for terminal_entry in terminal_outputs {
        if let Some(index) = match_accumulated_entry_by_identity(
            &merged_entries,
            &terminal_entry,
            &used_accumulated_indices,
        )
        .or_else(|| {
            match_accumulated_entry_by_compatible_output_index(
                &merged_entries,
                &terminal_entry,
                &used_accumulated_indices,
            )
        })
        .or_else(|| {
            match_accumulated_entry_by_compatible_kind(
                &merged_entries,
                &terminal_entry,
                &used_accumulated_indices,
            )
        })
        {
            let merged_nodes =
                merge_output_node_lists(&merged_entries[index].nodes, &terminal_entry.nodes)?;
            merged_entries[index] = AccumulatedOutputEntry {
                output_index: merged_entries[index].output_index,
                nodes: merged_nodes,
            };
            used_accumulated_indices.push(index);
        } else if !terminal_entry.nodes.iter().all(node_is_empty_text) {
            merged_entries.push(terminal_entry);
        }
    }

    merged_entries.sort_by_key(|entry| entry.output_index);
    Ok(merged_entries
        .into_iter()
        .flat_map(|entry| entry.nodes)
        .collect())
}

fn match_accumulated_entry_by_compatible_output_index(
    accumulated_outputs: &[AccumulatedOutputEntry],
    terminal_entry: &AccumulatedOutputEntry,
    used_accumulated_indices: &[usize],
) -> Option<usize> {
    accumulated_outputs
        .iter()
        .enumerate()
        .position(|(index, entry)| {
            !used_accumulated_indices.contains(&index)
                && entry.output_index == terminal_entry.output_index
                && entries_have_compatible_output_kind(entry, terminal_entry)
        })
}

fn match_accumulated_entry_by_identity(
    accumulated_outputs: &[AccumulatedOutputEntry],
    terminal_entry: &AccumulatedOutputEntry,
    used_accumulated_indices: &[usize],
) -> Option<usize> {
    let terminal_keys = entry_identity_keys(terminal_entry);
    if terminal_keys.is_empty() {
        return None;
    }
    accumulated_outputs
        .iter()
        .enumerate()
        .position(|(index, entry)| {
            !used_accumulated_indices.contains(&index)
                && entry_identity_keys(entry)
                    .iter()
                    .any(|key| terminal_keys.contains(key))
        })
}

fn match_accumulated_entry_by_compatible_kind(
    accumulated_outputs: &[AccumulatedOutputEntry],
    terminal_entry: &AccumulatedOutputEntry,
    used_accumulated_indices: &[usize],
) -> Option<usize> {
    let terminal_kind = entry_output_kind(terminal_entry)?;
    accumulated_outputs
        .iter()
        .enumerate()
        .position(|(index, entry)| {
            !used_accumulated_indices.contains(&index)
                && entry_output_kind(entry) == Some(terminal_kind)
                && merge_output_node_lists(&entry.nodes, &terminal_entry.nodes).is_ok()
        })
}

fn entry_identity_keys(entry: &AccumulatedOutputEntry) -> Vec<String> {
    entry
        .nodes
        .iter()
        .flat_map(|node| match node {
            Node::Text { id, .. }
            | Node::Reasoning { id, .. }
            | Node::ProviderItem { id, .. }
            | Node::ToolResult { id, .. } => id
                .as_deref()
                .filter(|id| !id.is_empty())
                .map(|id| vec![format!("id:{id}")])
                .unwrap_or_default(),
            Node::ToolCall { id, call_id, .. } => {
                let mut keys = Vec::new();
                if let Some(id) = id.as_deref().filter(|id| !id.is_empty()) {
                    keys.push(format!("id:{id}"));
                }
                if !call_id.is_empty() {
                    keys.push(format!("call_id:{call_id}"));
                }
                keys
            }
            _ => Vec::new(),
        })
        .collect()
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum OutputEntryKind {
    Reasoning,
    Message,
    ToolCall,
    ToolResult,
    ProviderItem,
}

fn entries_have_compatible_output_kind(
    left: &AccumulatedOutputEntry,
    right: &AccumulatedOutputEntry,
) -> bool {
    match (entry_output_kind(left), entry_output_kind(right)) {
        (Some(left), Some(right)) => left == right,
        _ => false,
    }
}

fn entry_output_kind(entry: &AccumulatedOutputEntry) -> Option<OutputEntryKind> {
    let mut observed = entry
        .nodes
        .iter()
        .filter(|node| !matches!(node, Node::NextDownstreamEnvelopeExtra { .. }))
        .filter(|node| !node_is_empty_text(node))
        .filter_map(node_output_kind);
    let first = observed.next()?;
    observed.all(|kind| kind == first).then_some(first)
}

fn node_output_kind(node: &Node) -> Option<OutputEntryKind> {
    match node {
        Node::Reasoning { .. } => Some(OutputEntryKind::Reasoning),
        Node::Text { .. }
        | Node::Image { .. }
        | Node::Audio { .. }
        | Node::File { .. }
        | Node::Refusal { .. } => Some(OutputEntryKind::Message),
        Node::ToolCall { .. } => Some(OutputEntryKind::ToolCall),
        Node::ToolResult { .. } => Some(OutputEntryKind::ToolResult),
        Node::ProviderItem { .. } => Some(OutputEntryKind::ProviderItem),
        Node::NextDownstreamEnvelopeExtra { .. } => None,
    }
}

fn merge_output_node_lists(accumulated: &[Node], terminal: &[Node]) -> Result<Vec<Node>, String> {
    let accumulated_typed = accumulated
        .iter()
        .filter(|node| !matches!(node, Node::NextDownstreamEnvelopeExtra { .. }))
        .filter(|node| !node_is_empty_text(node))
        .cloned()
        .collect::<Vec<_>>();
    let terminal_typed = terminal
        .iter()
        .filter(|node| !matches!(node, Node::NextDownstreamEnvelopeExtra { .. }))
        .filter(|node| !node_is_empty_text(node))
        .cloned()
        .collect::<Vec<_>>();

    if accumulated_typed.is_empty() {
        return Ok(terminal.to_vec());
    }
    if terminal_typed.is_empty() {
        return Ok(accumulated.to_vec());
    }
    if accumulated_typed.len() != terminal_typed.len() {
        return Err("completed output item has a different typed node count".to_string());
    }

    let mut merged = accumulated
        .iter()
        .filter(|node| matches!(node, Node::NextDownstreamEnvelopeExtra { .. }))
        .cloned()
        .collect::<Vec<_>>();
    for (left, right) in accumulated_typed.iter().zip(terminal_typed.iter()) {
        merged.push(merge_output_node(left, right)?);
    }
    Ok(merged)
}

fn merge_output_node(accumulated: &Node, terminal: &Node) -> Result<Node, String> {
    match (accumulated, terminal) {
        (
            Node::Reasoning {
                id: left_id,
                content: left_content,
                encrypted: left_encrypted,
                summary: left_summary,
                source: left_source,
                extra_body: left_extra,
            },
            Node::Reasoning {
                id: right_id,
                content: right_content,
                encrypted: right_encrypted,
                summary: right_summary,
                source: right_source,
                extra_body: right_extra,
            },
        ) => Ok(Node::Reasoning {
            id: right_id.clone().or_else(|| left_id.clone()),
            content: merge_optional_string_field("reasoning.text", left_content, right_content)?,
            encrypted: merge_optional_value_field(
                "reasoning.encrypted_content",
                left_encrypted,
                right_encrypted,
            )?,
            summary: merge_optional_string_field("reasoning.summary", left_summary, right_summary)?,
            source: right_source.clone().or_else(|| left_source.clone()),
            extra_body: merge_extra_body(left_extra, right_extra),
        }),
        (
            Node::Text {
                id: left_id,
                role: left_role,
                content: left_content,
                phase: left_phase,
                extra_body: left_extra,
            },
            Node::Text {
                id: right_id,
                role: right_role,
                content: right_content,
                phase: right_phase,
                extra_body: right_extra,
            },
        ) => {
            if left_role != right_role {
                return Err("message role differs from completed output".to_string());
            }
            Ok(Node::Text {
                id: right_id.clone().or_else(|| left_id.clone()),
                role: *right_role,
                content: merge_string_field("message.text", left_content, right_content)?,
                phase: merge_optional_string_field("message.phase", left_phase, right_phase)?,
                extra_body: merge_extra_body(left_extra, right_extra),
            })
        }
        (
            Node::ToolCall {
                id: left_id,
                call_id: left_call_id,
                name: left_name,
                arguments: left_arguments,
                extra_body: left_extra,
            },
            Node::ToolCall {
                id: right_id,
                call_id: right_call_id,
                name: right_name,
                arguments: right_arguments,
                extra_body: right_extra,
            },
        ) => Ok(Node::ToolCall {
            id: right_id.clone().or_else(|| left_id.clone()),
            call_id: merge_string_field("function_call.call_id", left_call_id, right_call_id)?,
            name: merge_string_field("function_call.name", left_name, right_name)?,
            arguments: merge_string_field(
                "function_call.arguments",
                left_arguments,
                right_arguments,
            )?,
            extra_body: merge_extra_body(left_extra, right_extra),
        }),
        (left, right) if nodes_semantically_match(left, right) => Ok(right.clone()),
        (left, right)
            if std::mem::discriminant(left) == std::mem::discriminant(right)
                && node_is_empty_text(left) =>
        {
            Ok(right.clone())
        }
        _ => Err("completed output item type differs from accumulated stream state".to_string()),
    }
}

fn merge_string_field(field: &str, left: &str, right: &str) -> Result<String, String> {
    match (left.is_empty(), right.is_empty(), left == right) {
        (true, _, _) => Ok(right.to_string()),
        (_, true, _) => Ok(left.to_string()),
        (_, _, true) => Ok(right.to_string()),
        _ => Err(format!("{field} differs from completed output")),
    }
}

fn merge_optional_string_field(
    field: &str,
    left: &Option<String>,
    right: &Option<String>,
) -> Result<Option<String>, String> {
    match (left.as_deref(), right.as_deref()) {
        (Some(left), Some(right)) => merge_string_field(field, left, right).map(Some),
        (Some(left), None) if !left.is_empty() => Ok(Some(left.to_string())),
        (_, Some(right)) if !right.is_empty() => Ok(Some(right.to_string())),
        _ => Ok(None),
    }
}

fn merge_optional_value_field(
    field: &str,
    left: &Option<Value>,
    right: &Option<Value>,
) -> Result<Option<Value>, String> {
    match (left, right) {
        (Some(left), Some(right)) if left == right => Ok(Some(right.clone())),
        (Some(left), Some(right)) if value_is_empty(left) => Ok(Some(right.clone())),
        (Some(left), Some(right)) if value_is_empty(right) => Ok(Some(left.clone())),
        (Some(_), Some(_)) => Err(format!("{field} differs from completed output")),
        (Some(left), None) if !value_is_empty(left) => Ok(Some(left.clone())),
        (_, Some(right)) if !value_is_empty(right) => Ok(Some(right.clone())),
        _ => Ok(None),
    }
}

fn value_is_empty(value: &Value) -> bool {
    match value {
        Value::Null => true,
        Value::String(value) => value.is_empty(),
        Value::Array(values) => values.is_empty(),
        Value::Object(values) => values.is_empty(),
        _ => false,
    }
}

fn merge_extra_body(
    left: &HashMap<String, Value>,
    right: &HashMap<String, Value>,
) -> HashMap<String, Value> {
    let mut merged = left.clone();
    for (key, value) in right {
        merged.insert(key.clone(), value.clone());
    }
    merged
}
