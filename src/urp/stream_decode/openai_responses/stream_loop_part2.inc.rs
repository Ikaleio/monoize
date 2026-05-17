fn map_responses_event_to_urp_events_with_state(
    event_name: &str,
    data_val: Value,
    message_phases_by_output_index: &HashMap<u64, String>,
    index_state: &mut ResponsesStreamIndexState,
) -> Vec<UrpStreamEvent> {
    match event_name {
        "response.created" | "response.in_progress" => Vec::new(),
        "response.output_item.added" => map_output_item_added(data_val, index_state),
        "response.content_part.added" => map_content_part_added(data_val, index_state),
        "response.output_text.delta" => {
            let mut extra =
                delta_extra_body_with_phase(data_val.clone(), message_phases_by_output_index);
            let mut events = Vec::new();
            let output_index = data_val
                .get("output_index")
                .and_then(|v| v.as_u64())
                .unwrap_or(0);
            if let Some(item_extra_body) = index_state
                .output_state_by_index
                .get(&output_index)
                .map(|state| state.item_extra_body.clone())
                .filter(|extra_body| !extra_body.is_empty())
            {
                for (key, value) in item_extra_body {
                    extra.entry(key).or_insert(value);
                }
            }
            let content_index = data_val
                .get("content_index")
                .or_else(|| data_val.get("part_index"))
                .and_then(|v| v.as_u64())
                .unwrap_or(0);
            let node_index = index_state.node_index_for_content(output_index, content_index);
            let should_emit_start = {
                let output_state = output_state_for(index_state, output_index);
                !output_state.emitted_any_node
            };
            if should_emit_start {
                let output_state = output_state_for(index_state, output_index);
                if output_state.item_extra_body.is_empty() {
                    output_state.item_extra_body = extra.clone();
                }
                emit_pending_envelope_control_if_needed(output_index, index_state, &mut events);
                let node = Node::Text {
                    id: output_state_for(index_state, output_index)
                        .item_id
                        .clone()
                        .or_else(|| Some(crate::urp::synthetic_message_id())),
                    role: output_state_for(index_state, output_index)
                        .role
                        .unwrap_or(Role::Assistant)
                        .to_ordinary()
                        .unwrap_or(OrdinaryRole::Assistant),
                    content: String::new(),
                    phase: extra
                        .get("phase")
                        .and_then(Value::as_str)
                        .map(str::to_string),
                    extra_body: extra.clone(),
                };
                events.push(UrpStreamEvent::NodeStart {
                    node_index,
                    header: node_header_from_node(&node),
                    extra_body: extra.clone(),
                });
                output_state_for(index_state, output_index).emitted_any_node = true;
            }
            events.push(UrpStreamEvent::NodeDelta {
                node_index,
                delta: NodeDelta::Text {
                    content: output_text_delta_content(&data_val).to_string(),
                },
                usage: None,
                extra_body: extra,
            });
            events
        }
        "response.reasoning.delta" | "response.reasoning_summary_text.delta" => {
            let (reasoning_source, reasoning_item_id) = data_val
                .get("output_index")
                .and_then(|v| v.as_u64())
                .map(|output_index| {
                    let output_state = index_state
                        .output_state_by_index
                        .entry(output_index)
                        .or_default();
                    if let Some(item_id) = data_val.get("item_id").and_then(|v| v.as_str())
                        && !item_id.is_empty()
                    {
                        output_state.item_id = Some(item_id.to_string());
                    }
                    merge_reasoning_source(
                        &mut output_state.reasoning_source,
                        reasoning_source_from_value(&data_val),
                    );
                    if event_name == "response.reasoning_summary_text.delta" {
                        output_state.reasoning_summary_delta_seen = true;
                    } else {
                        output_state.reasoning_text_delta_seen = true;
                    }
                    (
                        output_state.reasoning_source.clone(),
                        output_state.item_id.clone(),
                    )
                })
                .unwrap_or_default();
            let mut extra_body = split_known_fields(
                data_val.clone(),
                &[
                    "delta",
                    "text",
                    "output_index",
                    "content_index",
                    "part_index",
                    "summary_index",
                ],
            );
            if let Some(id) = reasoning_item_id {
                extra_body.insert("reasoning_item_id".to_string(), Value::String(id));
            }
            vec![UrpStreamEvent::NodeDelta {
                node_index: urp_node_index_from_delta(&data_val, index_state),
                delta: node_delta_from_reasoning_event(event_name, &data_val, reasoning_source),
                usage: None,
                extra_body,
            }]
        }
        "response.reasoning.done" => {
            let node_index = urp_node_index_from_delta(&data_val, index_state);
            let reasoning_source = data_val
                .get("output_index")
                .and_then(|v| v.as_u64())
                .and_then(|output_index| {
                    let output_state = index_state
                        .output_state_by_index
                        .entry(output_index)
                        .or_default();
                    merge_reasoning_source(
                        &mut output_state.reasoning_source,
                        reasoning_source_from_value(&data_val),
                    );
                    output_state.reasoning_source.clone()
                });
            let output_index = data_val
                .get("output_index")
                .and_then(|v| v.as_u64())
                .unwrap_or(0);
            vec![UrpStreamEvent::NodeDone {
                node_index,
                node: Node::Reasoning {
                    id: output_state_for(index_state, output_index)
                        .item_id
                        .clone()
                        .or_else(|| Some(crate::urp::synthetic_reasoning_id())),
                    content: data_val
                        .get("text")
                        .and_then(|v| v.as_str())
                        .filter(|text| !text.is_empty())
                        .map(|text| text.to_string()),
                    encrypted: None,
                    summary: None,
                    source: reasoning_source,
                    extra_body: split_known_fields(
                        data_val.clone(),
                        &[
                            "text",
                            "delta",
                            "output_index",
                            "content_index",
                            "part_index",
                        ],
                    ),
                },
                usage: None,
                extra_body: HashMap::new(),
            }]
        }
        "response.function_call_arguments.delta" => {
            vec![UrpStreamEvent::NodeDelta {
                node_index: urp_node_index_from_delta(&data_val, index_state),
                delta: NodeDelta::ToolCallArguments {
                    arguments: data_val
                        .get("delta")
                        .and_then(|v| v.as_str())
                        .unwrap_or_default()
                        .to_string(),
                },
                usage: None,
                extra_body: split_known_fields(
                    data_val,
                    &["delta", "output_index", "content_index", "part_index"],
                ),
            }]
        }
        "image_generation.completed" | "response.image_generation.completed" => {
            map_image_generation_completed(data_val, index_state)
        }
        "image_generation.partial_image" | "response.image_generation.partial_image" => Vec::new(),
        "response.content_part.done" => map_content_part_done(data_val, index_state),
        "response.output_item.done" => map_output_item_done(data_val, index_state),
        "response.completed" => map_response_completed(data_val, index_state),
        "error" => vec![UrpStreamEvent::Error {
            code: data_val
                .get("code")
                .and_then(|v| v.as_str())
                .map(|s| s.to_string()),
            message: data_val
                .get("message")
                .and_then(|v| v.as_str())
                .unwrap_or_else(|| data_val.as_str().unwrap_or("upstream error"))
                .to_string(),
            extra_body: split_known_fields(data_val, &["code", "message"]),
        }],
        _ => Vec::new(),
    }
}

#[derive(Debug, Default)]
struct ResponsesStreamIndexState {
    next_node_index: u32,
    node_index_by_content_key: HashMap<(u64, u64), u32>,
    synthetic_node_index_by_output_index: HashMap<u64, u32>,
    output_state_by_index: HashMap<u64, OutputItemStreamState>,
}

impl ResponsesStreamIndexState {
    fn node_index_for_content(&mut self, output_index: u64, content_index: u64) -> u32 {
        *self
            .node_index_by_content_key
            .entry((output_index, content_index))
            .or_insert_with(|| {
                let next = self.next_node_index;
                self.next_node_index += 1;
                next
            })
    }

    fn synthetic_node_index_for_output(&mut self, output_index: u64) -> u32 {
        *self
            .synthetic_node_index_by_output_index
            .entry(output_index)
            .or_insert_with(|| {
                let next = self.next_node_index;
                self.next_node_index += 1;
                next
            })
    }

    fn allocate_fresh_node_index(&mut self) -> u32 {
        let next = self.next_node_index;
        self.next_node_index += 1;
        next
    }
}

#[derive(Debug, Clone, Default)]
struct OutputItemStreamState {
    item_type: Option<String>,
    item_id: Option<String>,
    role: Option<Role>,
    item_extra_body: HashMap<String, Value>,
    emitted_any_node: bool,
    control_emitted: bool,
    part_done_seen: bool,
    reasoning_text_delta_seen: bool,
    reasoning_summary_delta_seen: bool,
    reasoning_source: Option<String>,
}

fn merge_reasoning_source(dst: &mut Option<String>, source: Option<String>) {
    if let Some(source) = source.filter(|source| !source.is_empty()) {
        *dst = Some(source);
    }
}

fn reasoning_source_from_value(value: &Value) -> Option<String> {
    value
        .get("source")
        .and_then(|value| value.as_str())
        .filter(|source| !source.is_empty())
        .map(|source| source.to_string())
}

fn responses_stream_error_parts(
    event_name: &str,
    data_val: Value,
) -> (Option<String>, String, HashMap<String, Value>) {
    let error_value = if event_name == "response.failed" {
        data_val
            .get("response")
            .and_then(|response| response.get("error"))
            .cloned()
            .unwrap_or(Value::Null)
    } else {
        data_val
            .get("error")
            .cloned()
            .unwrap_or_else(|| data_val.clone())
    };
    let code = error_value
        .get("code")
        .and_then(|v| v.as_str())
        .or_else(|| data_val.get("code").and_then(|v| v.as_str()))
        .map(|s| s.to_string());
    let message = error_value
        .get("message")
        .and_then(|v| v.as_str())
        .or_else(|| data_val.get("message").and_then(|v| v.as_str()))
        .unwrap_or_else(|| data_val.as_str().unwrap_or("upstream error"))
        .to_string();
    let mut extra_body = split_known_fields(error_value, &["code", "message"]);
    extra_body.extend(split_known_fields(
        data_val,
        &["code", "message", "error", "response"],
    ));
    (code, message, extra_body)
}

fn output_state_for<'a>(
    index_state: &'a mut ResponsesStreamIndexState,
    output_index: u64,
) -> &'a mut OutputItemStreamState {
    index_state
        .output_state_by_index
        .entry(output_index)
        .or_default()
}

fn output_role_to_ordinary(role: Role) -> OrdinaryRole {
    role.to_ordinary().unwrap_or(OrdinaryRole::Assistant)
}

fn first_node_from_item_value(item: &Value) -> Option<Node> {
    nodes_from_item_value(item).into_iter().next()
}
