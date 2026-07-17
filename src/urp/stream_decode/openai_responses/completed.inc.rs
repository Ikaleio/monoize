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

#[derive(Clone, Debug, Default)]
struct AccumulatedReasoningSlot {
    id: Option<String>,
    content: String,
    summary: String,
    summary_parts: BTreeMap<u64, String>,
    encrypted: Option<Value>,
    source: Option<String>,
    extra_body: HashMap<String, Value>,
}

impl AccumulatedReasoningSlot {
    fn has_typed_output(&self) -> bool {
        !self.content.is_empty() || self.summary_text().is_some() || self.encrypted.is_some()
    }

    fn summary_text(&self) -> Option<String> {
        let mut parts = self
            .summary_parts
            .values()
            .filter(|part| !part.is_empty())
            .cloned()
            .collect::<Vec<_>>();
        if parts.is_empty() && !self.summary.is_empty() {
            return Some(self.summary.clone());
        }
        if parts.is_empty() {
            return None;
        }
        if !self.summary.is_empty() {
            parts.push(self.summary.clone());
        }
        Some(parts.concat())
    }
}

fn reasoning_slot_for_event<'a>(
    reasoning_by_output_index: &'a mut HashMap<u64, AccumulatedReasoningSlot>,
    data_val: &Value,
) -> &'a mut AccumulatedReasoningSlot {
    let output_index = data_val
        .get("output_index")
        .and_then(Value::as_u64)
        .unwrap_or(0);
    let slot = reasoning_by_output_index.entry(output_index).or_default();
    if let Some(id) = data_val
        .get("item_id")
        .or_else(|| data_val.get("id"))
        .and_then(Value::as_str)
        .filter(|id| !id.is_empty())
    {
        slot.id = Some(id.to_string());
    }
    merge_reasoning_source(&mut slot.source, reasoning_source_from_value(data_val));
    slot
}

fn reasoning_slot_for_item<'a>(
    reasoning_by_output_index: &'a mut HashMap<u64, AccumulatedReasoningSlot>,
    output_index: u64,
    item: &Value,
) -> &'a mut AccumulatedReasoningSlot {
    let slot = reasoning_by_output_index.entry(output_index).or_default();
    if let Some(id) = item
        .get("id")
        .and_then(Value::as_str)
        .filter(|id| !id.is_empty())
    {
        slot.id = Some(id.to_string());
    }
    merge_reasoning_source(&mut slot.source, reasoning_source_from_value(item));
    let item_extra = part_extra_body_from_value(item);
    for (key, value) in item_extra {
        slot.extra_body.entry(key).or_insert(value);
    }
    slot
}

fn append_reasoning_text_delta(slot: &mut AccumulatedReasoningSlot, delta: &str) {
    if !delta.is_empty() {
        slot.content.push_str(delta);
    }
}

fn append_reasoning_summary_delta(
    slot: &mut AccumulatedReasoningSlot,
    summary_index: Option<u64>,
    delta: &str,
) {
    if !delta.is_empty() {
        if let Some(summary_index) = summary_index {
            slot.summary_parts
                .entry(summary_index)
                .or_default()
                .push_str(delta);
        } else {
            slot.summary.push_str(delta);
        }
    }
}

fn complete_reasoning_text(slot: &mut AccumulatedReasoningSlot, text: &str) {
    if !text.is_empty() && slot.content.is_empty() {
        slot.content = text.to_string();
    }
}

fn complete_reasoning_summary(
    slot: &mut AccumulatedReasoningSlot,
    summary_index: Option<u64>,
    summary: &str,
) {
    if summary.is_empty() {
        return;
    }
    if let Some(summary_index) = summary_index {
        let part = slot.summary_parts.entry(summary_index).or_default();
        if part.is_empty() {
            *part = summary.to_string();
        }
    } else if slot.summary.is_empty() {
        slot.summary = summary.to_string();
    }
}

fn merge_reasoning_item_snapshot(
    slot: &mut AccumulatedReasoningSlot,
    item: &Value,
    overwrite_terminal_fields: bool,
) {
    let (text, summary, encrypted) = extract_reasoning_parts(item);
    if !text.is_empty() && (overwrite_terminal_fields || slot.content.is_empty()) {
        slot.content = text;
    }
    if !summary.is_empty()
        && (overwrite_terminal_fields || (slot.summary.is_empty() && slot.summary_parts.is_empty()))
    {
        slot.summary = summary;
    }
    if !encrypted.is_empty() && (overwrite_terminal_fields || slot.encrypted.is_none()) {
        slot.encrypted = Some(Value::String(encrypted));
    }
}

#[derive(Clone, Debug)]
struct AccumulatedOutputEntry {
    output_index: u64,
    nodes: Vec<Node>,
}

#[allow(clippy::too_many_arguments)]
fn build_accumulated_output_entries(
    reasoning_by_output_index: &HashMap<u64, AccumulatedReasoningSlot>,
    output_texts_by_output_index: &HashMap<u64, String>,
    message_phases_by_output_index: &HashMap<u64, String>,
    message_item_extra_by_output_index: &HashMap<u64, HashMap<String, Value>>,
    item_ids_by_output_index: &HashMap<u64, String>,
    call_order: &[String],
    calls: &HashMap<String, (String, String)>,
    call_ids_by_output_index: &HashMap<u64, String>,
) -> Vec<AccumulatedOutputEntry> {
    #[derive(Clone, Debug)]
    enum FallbackOutputKind {
        Reasoning(u64),
        Text(u64),
        ToolCall(u64, String),
    }

    let mut ordered_kinds = Vec::new();
    let mut reasoning_indices = reasoning_by_output_index
        .iter()
        .filter_map(|(output_index, slot)| {
            slot.has_typed_output()
                .then_some(FallbackOutputKind::Reasoning(*output_index))
        })
        .collect::<Vec<_>>();
    reasoning_indices.sort_by_key(|kind| match kind {
        FallbackOutputKind::Reasoning(output_index) => *output_index,
        _ => 0,
    });
    ordered_kinds.extend(reasoning_indices);

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

    let mut entries = Vec::new();
    for kind in ordered_kinds {
        match kind {
            FallbackOutputKind::Reasoning(output_index) => {
                let Some(slot) = reasoning_by_output_index.get(&output_index) else {
                    continue;
                };
                let id = slot.id.clone().or_else(|| {
                    item_ids_by_output_index
                        .get(&output_index)
                        .cloned()
                        .or_else(|| {
                            message_item_extra_by_output_index
                                .get(&output_index)
                                .and_then(|extra| extra.get("id"))
                                .and_then(Value::as_str)
                                .map(|s| s.to_string())
                        })
                });
                entries.push(AccumulatedOutputEntry {
                    output_index,
                    nodes: vec![Node::Reasoning {
                        id,
                        content: (!slot.content.is_empty()).then(|| slot.content.clone()),
                        summary: slot.summary_text(),
                        encrypted: slot.encrypted.clone(),
                        source: slot.source.clone(),
                        extra_body: slot.extra_body.clone(),
                    }],
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
                    .or_else(|| item_ids_by_output_index.get(&output_index).cloned())
                    .or_else(|| Some(crate::urp::synthetic_message_id()));
                entries.push(AccumulatedOutputEntry {
                    output_index,
                    nodes: vec![
                        Node::NextDownstreamEnvelopeExtra {
                            extra_body: item_extra_body,
                        },
                        Node::Text {
                            id: message_id,
                            role: OrdinaryRole::Assistant,
                            content: output_text.clone(),
                            phase: text_extra_body
                                .get("phase")
                                .and_then(Value::as_str)
                                .map(str::to_string),
                            extra_body: text_extra_body,
                        },
                    ],
                });
            }
            FallbackOutputKind::ToolCall(output_index, call_id) => {
                if let Some((name, arguments)) = calls.get(&call_id) {
                    entries.push(AccumulatedOutputEntry {
                        output_index,
                        nodes: vec![Node::ToolCall {
                            id: Some(crate::urp::synthetic_tool_call_id()),
                            call_id: call_id.clone(),
                            name: name.clone(),
                            arguments: arguments.clone(),
                            extra_body: HashMap::new(),
                        }],
                    });
                }
            }
        }
    }

    entries
}

#[allow(clippy::too_many_arguments)]
fn build_accumulated_output_nodes_from_reasoning_slots(
    reasoning_by_output_index: &HashMap<u64, AccumulatedReasoningSlot>,
    output_texts_by_output_index: &HashMap<u64, String>,
    message_phases_by_output_index: &HashMap<u64, String>,
    message_item_extra_by_output_index: &HashMap<u64, HashMap<String, Value>>,
    item_ids_by_output_index: &HashMap<u64, String>,
    call_order: &[String],
    calls: &HashMap<String, (String, String)>,
    call_ids_by_output_index: &HashMap<u64, String>,
) -> Vec<Node> {
    build_accumulated_output_entries(
        reasoning_by_output_index,
        output_texts_by_output_index,
        message_phases_by_output_index,
        message_item_extra_by_output_index,
        item_ids_by_output_index,
        call_order,
        calls,
        call_ids_by_output_index,
    )
    .into_iter()
    .flat_map(|entry| entry.nodes)
    .collect()
}

#[allow(clippy::too_many_arguments)]
#[cfg(test)]
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
    let mut reasoning_by_output_index = HashMap::new();
    if !reasoning_text.is_empty() || !reasoning_summary_text.is_empty() || !reasoning_sig.is_empty()
    {
        let output_index = reasoning_output_index.unwrap_or(0);
        reasoning_by_output_index.insert(
            output_index,
            AccumulatedReasoningSlot {
                id: item_ids_by_output_index.get(&output_index).cloned(),
                content: reasoning_text.to_string(),
                summary: reasoning_summary_text.to_string(),
                summary_parts: BTreeMap::new(),
                encrypted: (!reasoning_sig.is_empty()).then(|| Value::String(reasoning_sig.to_string())),
                source: reasoning_source.map(str::to_string),
                extra_body: HashMap::new(),
            },
        );
    }
    build_accumulated_output_nodes_from_reasoning_slots(
        &reasoning_by_output_index,
        output_texts_by_output_index,
        message_phases_by_output_index,
        message_item_extra_by_output_index,
        item_ids_by_output_index,
        call_order,
        calls,
        call_ids_by_output_index,
    )
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
            "content",
            "text",
            "summary",
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
