fn accumulate_message_content_event(
    event_name: &str,
    data: &Value,
    index_state: &mut ResponsesStreamIndexState,
) {
    if !matches!(event_name,
        "response.output_text.delta" | "response.output_text.done"
        | "response.refusal.delta" | "response.refusal.done"
        | "response.output_text.annotation.added"
    ) { return; }
    let output_index = data.get("output_index").and_then(Value::as_u64).unwrap_or(0);
    let content_index = data.get("content_index").or_else(|| data.get("part_index"))
        .and_then(Value::as_u64).unwrap_or(0);
    let item_id = stable_message_item_id_for_output(index_state, output_index);
    let state = output_state_for(index_state, output_index);
    let citations: Vec<_> = state.text_citations.iter()
        .filter(|((part, _), _)| *part == content_index)
        .map(|(_, annotation)| annotation.clone()).collect();
    let is_refusal = event_name.starts_with("response.refusal.");
    let node = state.content_nodes.entry(content_index).or_insert_with(|| {
        if is_refusal {
            Node::Refusal {
                logprobs: None,
                id: Some(item_id.clone()),
                content: String::new(),
                extra_body: HashMap::new(),
            }
        } else {
            Node::Text {
                logprobs: None,
                id: Some(item_id.clone()),
                role: state
                    .role
                    .unwrap_or(Role::Assistant)
                    .to_ordinary()
                    .unwrap_or(OrdinaryRole::Assistant),
                content: String::new(),
                phase: state.message_phase.clone(),
                signature: None,
                citations: Vec::new(),
                extra_body: HashMap::new(),
            }
        }
    });
    match node {
        Node::Text {
            logprobs,
            content,
            citations: stored_citations,
            phase,
            ..
        } => {
            if event_name == "response.output_text.delta" {
                content.push_str(output_text_delta_content(data));
                crate::urp::logprobs::append(
                    logprobs,
                    &crate::urp::logprobs::decode(data.get("logprobs")),
                );
            } else if event_name == "response.output_text.done" {
                if let Some(text) = data.get("text").and_then(Value::as_str) {
                    *content = text.to_owned();
                }
                if let Some(scores) = crate::urp::logprobs::decode(data.get("logprobs")) {
                    *logprobs = Some(scores);
                }
            }
            if let Some(value) = data.get("phase").and_then(Value::as_str) {
                *phase = Some(value.to_owned());
            }
            *stored_citations = citations;
        }
        Node::Refusal {
            logprobs, content, ..
        } if is_refusal => {
            if event_name == "response.refusal.delta" {
                content.push_str(
                    data.get("delta")
                        .and_then(Value::as_str)
                        .unwrap_or_default(),
                );
            } else if let Some(text) = data.get("refusal").and_then(Value::as_str) {
                *content = text.to_owned();
            }
        }
        _ => {}
    }
}

fn outputs_have_tool_calls(items: &[Node]) -> bool {
    items
        .iter()
        .any(|item| matches!(item, Node::ToolCall { .. }))
}

fn accumulate_text_annotations(event_name: &str, data: &Value, index_state: &mut ResponsesStreamIndexState) {
    let Some(output_index) = data.get("output_index").and_then(Value::as_u64) else {
        return;
    };
    let state = output_state_for(index_state, output_index);
    let content_index = data
        .get("content_index")
        .or_else(|| data.get("part_index"))
        .and_then(Value::as_u64)
        .unwrap_or(0);
    if event_name == "response.output_text.annotation.added" {
        if let Some(annotation) = data.get("annotation") {
            let annotation_index = data
                .get("annotation_index")
                .and_then(Value::as_u64)
                .unwrap_or_else(|| {
                    state
                        .text_citations
                        .keys()
                        .filter(|(part, _)| *part == content_index)
                        .count() as u64
                });
            state.text_citations.insert(
                (content_index, annotation_index),
                crate::urp::Citation::decode(
                    annotation.clone(),
                    crate::urp::ProviderProtocol::Responses,
                ),
            );
        }
    }
    let mut add_part = |part: &Value, part_index: u64| {
        if let Some(annotations) = part.get("annotations").and_then(Value::as_array) {
            for (annotation_index, annotation) in annotations.iter().enumerate() {
                state.text_citations.insert(
                    (part_index, annotation_index as u64),
                    crate::urp::Citation::decode(
                        annotation.clone(),
                        crate::urp::ProviderProtocol::Responses,
                    ),
                );
            }
        }
    };
    if matches!(event_name, "response.content_part.added" | "response.content_part.done") {
        if let Some(part) = data.get("part") { add_part(part, content_index); }
    }
    if matches!(event_name, "response.output_item.added" | "response.output_item.done") {
        if let Some(parts) = data.get("item").and_then(|item| item.get("content")).and_then(Value::as_array) {
            for (part_index, part) in parts.iter().enumerate() { add_part(part, part_index as u64); }
        }
    }
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

fn replace_nonempty_tool_arguments(current: &mut String, snapshot: &str) {
    if !snapshot.is_empty() {
        *current = snapshot.to_string();
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
        slot.summary_parts
            .insert(summary_index, summary.to_string());
    } else {
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
    if !summary.is_empty() {
        if overwrite_terminal_fields {
            slot.summary_parts.clear();
            slot.summary = summary;
        } else if slot.summary.is_empty() && slot.summary_parts.is_empty() {
            slot.summary = summary;
        }
    }
    // The added snapshot travels in item-level event state. The terminal accumulator accepts
    // only a complete done snapshot, so no encrypted snapshot is treated as a string delta.
    if overwrite_terminal_fields && !encrypted.is_empty() {
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
    calls: &HashMap<String, (ToolCallType, String, String)>,
    call_ids_by_output_index: &HashMap<u64, String>,
    index_state: &ResponsesStreamIndexState,
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
                        metadata: Default::default(),

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
                let mut item_extra_body = message_item_extra_by_output_index
                    .get(&output_index)
                    .cloned()
                    .unwrap_or_default();
                if let Some(state) = index_state.output_state_by_index.get(&output_index) {
                    for (key, value) in &state.item_extra_body {
                        item_extra_body.entry(key.clone()).or_insert_with(|| value.clone());
                    }
                }
                let message_id = item_extra_body
                    .remove("id")
                    .as_ref()
                    .and_then(Value::as_str)
                    .map(|s| s.to_string())
                    .or_else(|| item_ids_by_output_index.get(&output_index).cloned())
                    .or_else(|| Some(crate::urp::synthetic_message_id()));
                item_extra_body.remove("phase");
                entries.push(AccumulatedOutputEntry {
                    output_index,
                    nodes: vec![
                        Node::NextDownstreamEnvelopeExtra {
                            extra_body: item_extra_body,
                        },
                        Node::Text {
                            logprobs: None,
                            citations: index_state
                                .output_state_by_index
                                .get(&output_index)
                                .map(|state| state.text_citations.values().cloned().collect())
                                .unwrap_or_default(),
                            signature: None,

                            id: message_id,
                            role: OrdinaryRole::Assistant,
                            content: output_text.clone(),
                            phase: message_phases_by_output_index.get(&output_index).cloned(),
                            extra_body: HashMap::new(),
                        },
                    ],
                });
            }
            FallbackOutputKind::ToolCall(output_index, call_id) => {
                if let Some((tool_type, name, arguments)) = calls.get(&call_id) {
                    entries.push(AccumulatedOutputEntry {
                        output_index,
                        nodes: vec![Node::ToolCall {
                            namespace: None, signature: None,
                            id: Some(crate::urp::synthetic_tool_call_id()),
                            tool_type: *tool_type,
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

    for (output_index,state) in &index_state.output_state_by_index {
        if state.content_nodes.is_empty() { continue; }
        entries.retain(|entry| entry.output_index != *output_index);
        let mut nodes=Vec::new();
        if !state.item_extra_body.is_empty() {
            nodes.push(Node::NextDownstreamEnvelopeExtra {extra_body:state.item_extra_body.clone()});
        }
        nodes.extend(state.content_nodes.values().cloned());
        entries.push(AccumulatedOutputEntry {output_index:*output_index,nodes});
    }
    entries.sort_by_key(|entry|entry.output_index);
    entries
}

#[allow(clippy::too_many_arguments)]
#[allow(clippy::too_many_arguments)]

fn output_index_for_call_id(
    call_ids_by_output_index: &HashMap<u64, String>,
    target_call_id: &str,
) -> Option<u64> {
    call_ids_by_output_index
        .iter()
        .find_map(|(output_index, call_id)| (call_id == target_call_id).then_some(*output_index))
}

fn item_extra_body_from_value(item: &Value) -> HashMap<String, Value> {
    if !matches!(item.get("type").and_then(Value::as_str), Some(
        "message" | "reasoning" | "function_call" | "custom_tool_call"
        | "function_call_output" | "custom_tool_call_output" | "image_generation_call"
    )) {
        return HashMap::new();
    }
    let mut extra_body = split_known_fields(
        item.clone(),
        &[
            "type",
            "role",
            "content",
            "call_id",
            "id",
            "phase",
            "output",
            "name",
            "arguments",
            "namespace",
            "input",
            "result",
            "output_format",
        ],
    );
    if let Some(native_body) = native_image_generation_call_body(item) {
        extra_body.insert(
            RESPONSES_IMAGE_GENERATION_CALL_EXTRA_KEY.to_string(),
            native_body,
        );
    }
    extra_body
}

fn part_extra_body_from_value(part: &Value) -> HashMap<String, Value> {
    if let Some(obj) = part.as_object()
        && let Ok(Some(media)) = crate::urp::decode::parse_compatible_media_part(obj)
    {
        return match media {
            Part::Image {extra_body,..} | Part::File {extra_body,..} | Part::Audio {extra_body,..} => extra_body,
            _ => unreachable!(),
        };
    }
    if !matches!(part.get("type").and_then(Value::as_str), Some(
        "input_text" | "output_text" | "text" | "refusal" | "reasoning_text" | "reasoning"
        | "function_call" | "custom_tool_call" | "function_call_output" | "custom_tool_call_output"
        | "image_generation_call" | "input_image" | "output_image" | "input_file" | "output_file"
    )) {
        return HashMap::new();
    }
    let mut extra_body = split_known_fields(
        part.clone(),
        &[
            "type",
            "id",
            "content",
            "text",
            "summary",
            "refusal",
            "annotations",
            "call_id",
            "name",
            "arguments",
            "namespace",
            "input",
            "result",
            "output_format",
            "source",
            "encrypted_content",
        ],
    );
    if let Some(native_body) = native_image_generation_call_body(part) {
        extra_body.insert(
            RESPONSES_IMAGE_GENERATION_CALL_EXTRA_KEY.to_string(),
            native_body,
        );
    }
    extra_body
}

fn split_known_fields(value: Value, known_fields: &[&str]) -> HashMap<String, Value> {
    let mut out = HashMap::new();
    if let Some(obj) = value.as_object() {
        for (key, val) in obj {
            if !crate::urp::decode::is_internal_extra_key(key)
                && !known_fields.iter().any(|known| known == key)
            {
                out.insert(key.clone(), val.clone());
            }
        }
    }
    out
}
