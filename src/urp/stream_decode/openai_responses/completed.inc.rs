fn item_extra_body_from_item(item: &Item) -> HashMap<String, Value> {
    match item {
        Item::Message { extra_body, .. } | Item::ToolResult { extra_body, .. } => {
            extra_body.clone()
        }
    }
}

fn outputs_have_tool_calls(items: &[Node]) -> bool {
    items
        .iter()
        .any(|item| matches!(item, Node::ToolCall { .. }))
}

#[allow(clippy::too_many_arguments)]
fn build_accumulated_output_nodes(
    reasoning_text: &str,
    reasoning_summary_text: &str,
    reasoning_sig: &str,
    reasoning_source: Option<&str>,
    reasoning_output_index: Option<u64>,
    output_texts_by_output_index: &HashMap<u64, String>,
    message_phases_by_output_index: &HashMap<u64, String>,
    message_item_extra_by_output_index: &HashMap<u64, HashMap<String, Value>>,
    item_ids_by_output_index: &HashMap<u64, String>,
    call_order: &[String],
    calls: &HashMap<String, (String, String)>,
    call_ids_by_output_index: &HashMap<u64, String>,
) -> Vec<Node> {
    let mut nodes = Vec::new();

    #[derive(Clone, Debug)]
    enum FallbackOutputKind {
        Reasoning(u64),
        Text(u64),
        ToolCall(u64, String),
    }

    let mut ordered_kinds = Vec::new();
    if !reasoning_text.is_empty() || !reasoning_summary_text.is_empty() || !reasoning_sig.is_empty()
    {
        ordered_kinds.push(FallbackOutputKind::Reasoning(
            reasoning_output_index.unwrap_or(0),
        ));
    }

    let mut text_indices = output_texts_by_output_index
        .keys()
        .copied()
        .collect::<Vec<_>>();
    text_indices.sort_unstable();
    ordered_kinds.extend(text_indices.into_iter().map(FallbackOutputKind::Text));

    let mut call_output_indices = call_order
        .iter()
        .enumerate()
        .map(|(call_position, call_id)| {
            let output_index = output_index_for_call_id(call_ids_by_output_index, call_id)
                .unwrap_or(call_position as u64 + 1);
            (output_index, call_id.clone())
        })
        .collect::<Vec<_>>();
    call_output_indices.sort_by_key(|(output_index, _)| *output_index);
    ordered_kinds.extend(
        call_output_indices
            .into_iter()
            .map(|(output_index, call_id)| FallbackOutputKind::ToolCall(output_index, call_id)),
    );

    ordered_kinds.sort_by_key(|kind| match kind {
        FallbackOutputKind::Reasoning(output_index) => *output_index,
        FallbackOutputKind::Text(output_index) => *output_index,
        FallbackOutputKind::ToolCall(output_index, _) => *output_index,
    });

    for kind in ordered_kinds {
        match kind {
            FallbackOutputKind::Reasoning(_) => {
                nodes.push(Node::Reasoning {
                    id: reasoning_output_index.and_then(|idx| {
                        item_ids_by_output_index.get(&idx).cloned().or_else(|| {
                            message_item_extra_by_output_index
                                .get(&idx)
                                .and_then(|extra| extra.get("id"))
                                .and_then(Value::as_str)
                                .map(|s| s.to_string())
                        })
                    }),
                    content: (!reasoning_text.is_empty()).then(|| reasoning_text.to_string()),
                    summary: (!reasoning_summary_text.is_empty())
                        .then(|| reasoning_summary_text.to_string()),
                    encrypted: (!reasoning_sig.is_empty())
                        .then(|| Value::String(reasoning_sig.to_string())),
                    source: reasoning_source.map(|source| source.to_string()),
                    extra_body: HashMap::new(),
                });
            }
            FallbackOutputKind::Text(output_index) => {
                let Some(output_text) = output_texts_by_output_index.get(&output_index) else {
                    continue;
                };
                if output_text.is_empty() {
                    continue;
                }
                let mut text_extra_body = HashMap::new();
                if let Some(phase) = message_phases_by_output_index.get(&output_index) {
                    text_extra_body.insert("phase".to_string(), json!(phase));
                }
                let mut item_extra_body = message_item_extra_by_output_index
                    .get(&output_index)
                    .cloned()
                    .unwrap_or_default();
                if let Some(phase) = text_extra_body.get("phase") {
                    item_extra_body
                        .entry("phase".to_string())
                        .or_insert_with(|| phase.clone());
                }
                let message_id = item_extra_body
                    .get("id")
                    .and_then(Value::as_str)
                    .map(|s| s.to_string())
                    .or_else(|| Some(crate::urp::synthetic_message_id()));
                nodes.push(Node::NextDownstreamEnvelopeExtra {
                    extra_body: item_extra_body,
                });
                nodes.push(Node::Text {
                    id: message_id,
                    role: OrdinaryRole::Assistant,
                    content: output_text.clone(),
                    phase: text_extra_body
                        .get("phase")
                        .and_then(Value::as_str)
                        .map(str::to_string),
                    extra_body: text_extra_body,
                });
            }
            FallbackOutputKind::ToolCall(_, call_id) => {
                if let Some((name, arguments)) = calls.get(&call_id) {
                    nodes.push(Node::ToolCall {
                        id: Some(crate::urp::synthetic_tool_call_id()),
                        call_id: call_id.clone(),
                        name: name.clone(),
                        arguments: arguments.clone(),
                        extra_body: HashMap::new(),
                    });
                }
            }
        }
    }

    nodes
}

fn output_index_for_call_id(
    call_ids_by_output_index: &HashMap<u64, String>,
    target_call_id: &str,
) -> Option<u64> {
    call_ids_by_output_index
        .iter()
        .find_map(|(output_index, call_id)| (call_id == target_call_id).then_some(*output_index))
}

fn item_extra_body_from_value(item: &Value) -> HashMap<String, Value> {
    split_known_fields(
        item.clone(),
        &[
            "type",
            "role",
            "content",
            "call_id",
            "id",
            "output",
            "name",
            "arguments",
        ],
    )
}

fn part_extra_body_from_value(part: &Value) -> HashMap<String, Value> {
    split_known_fields(
        part.clone(),
        &[
            "type",
            "text",
            "refusal",
            "call_id",
            "name",
            "arguments",
            "source",
            "encrypted_content",
        ],
    )
}

fn split_known_fields(value: Value, known_fields: &[&str]) -> HashMap<String, Value> {
    let mut out = HashMap::new();
    if let Some(obj) = value.as_object() {
        for (key, val) in obj {
            if !known_fields.iter().any(|known| known == key) {
                out.insert(key.clone(), val.clone());
            }
        }
    }
    out
}

