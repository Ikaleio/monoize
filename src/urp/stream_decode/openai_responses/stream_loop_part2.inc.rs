fn map_responses_event_to_urp_events_with_state(
    event_name: &str,
    data_val: Value,
    message_phases_by_output_index: &HashMap<u64, String>,
    index_state: &mut ResponsesStreamIndexState,
) -> Vec<UrpStreamEvent> {
    accumulate_message_content_event(event_name, &data_val, index_state);
    match event_name {
        "response.created" | "response.in_progress" => Vec::new(),
        "response.output_item.added" => map_output_item_added(data_val, index_state),
        "response.content_part.added" => map_content_part_added(data_val, index_state),
        "response.refusal.delta" => vec![UrpStreamEvent::NodeDelta {
            node_index: urp_node_index_from_delta(&data_val, index_state),
            delta: NodeDelta::Refusal {
                content: data_val.get("delta").and_then(Value::as_str).unwrap_or_default().to_string(),
            },
            usage: None,
            extra_body: HashMap::new(),
        }],
        "response.output_text.annotation.added" => vec![UrpStreamEvent::NodeDelta {
            node_index: urp_node_index_from_delta(&data_val, index_state),
            delta: NodeDelta::Text {
                signature: None,
                citations: data_val.get("annotation").cloned().into_iter().collect(),
                content: String::new(),
            },
            usage: None,
            extra_body: HashMap::new(),
        }],
        "response.output_text.delta" => {
            let mut extra = text_delta_extra_body(data_val.clone());
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
            let should_emit_start = !index_state.node_index_by_content_key.contains_key(&(output_index, content_index));
            let node_index = index_state.node_index_for_content(output_index, content_index);
            if should_emit_start {
                let output_state = output_state_for(index_state, output_index);
                if output_state.item_extra_body.is_empty() {
                    output_state.item_extra_body = extra.clone();
                }
                emit_pending_envelope_control_if_needed(output_index, index_state, &mut events);
                let item_id = stable_message_item_id_for_output(index_state, output_index);
                let node = Node::Text {
                    signature: None,
                    citations: Vec::new(),
                    id: Some(item_id),
                    role: output_state_for(index_state, output_index)
                        .role
                        .unwrap_or(Role::Assistant)
                        .to_ordinary()
                        .unwrap_or(OrdinaryRole::Assistant),
                    content: String::new(),
                    phase: data_val
                        .get("phase")
                        .and_then(Value::as_str)
                        .map(str::to_string)
                        .or_else(|| message_phases_by_output_index.get(&output_index).cloned()),
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
                    signature: None,
                    citations: Vec::new(),
                    content: output_text_delta_content(&data_val).to_string(),
                },
                usage: None,
                extra_body: extra,
            });
            events
        }
        "response.reasoning_text.delta" | "response.reasoning_summary_text.delta" => {
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
            let extra_body = split_known_fields(
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
            let mut delta =
                node_delta_from_reasoning_event(event_name, &data_val, reasoning_source);
            if let NodeDelta::Reasoning { metadata, .. } = &mut delta {
                metadata.item_id = reasoning_item_id;
            }
            vec![UrpStreamEvent::NodeDelta {
                node_index: urp_node_index_from_delta(&data_val, index_state),
                delta,
                usage: None,
                extra_body,
            }]
        }
        "response.reasoning_text.done"
        | "response.reasoning_summary_text.done"
        | "response.reasoning_summary_part.added"
        | "response.reasoning_summary_part.done"
        | "response.output_text.done"
        | "response.refusal.done"
        | "response.function_call_arguments.done"
        | "response.custom_tool_call_input.done" => Vec::new(),
        "response.function_call_arguments.delta" | "response.custom_tool_call_input.delta" => {
            if let Some(output_index) = data_val.get("output_index").and_then(|v| v.as_u64()) {
                output_state_for(index_state, output_index).function_arguments_delta_seen = true;
            }
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
        "image_generation.partial_image" | "response.image_generation.partial_image" => {
            let output_index = data_val
                .get("output_index")
                .and_then(|value| value.as_u64())
                .unwrap_or(0);
            map_image_generation_call_event(data_val, output_index, index_state)
        }
        "response.image_generation_call.in_progress"
        | "response.image_generation_call.generating"
        | "response.image_generation_call.partial_image"
        | "response.image_generation_call.completed" => {
            let output_index = data_val
                .get("output_index")
                .and_then(|value| value.as_u64())
                .unwrap_or(0);
            map_image_generation_call_event(data_val, output_index, index_state)
        }
        "response.content_part.done" => map_content_part_done(data_val, index_state),
        "response.output_item.done" => map_output_item_done(data_val, index_state),
        "response.completed" | "response.incomplete" | "response.failed" | "response.cancelled" => {
            map_response_completed(data_val, index_state)
        }
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
        _ => vec![UrpStreamEvent::ProviderControl {
            protocol: "responses".to_string(),
            event_name: event_name.to_string(),
            data: data_val,
            extra_body: HashMap::new(),
        }],
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
    node_done_seen: bool,
    reasoning_text_delta_seen: bool,
    reasoning_summary_delta_seen: bool,
    function_arguments_delta_seen: bool,
    reasoning_source: Option<String>,
    message_phase: Option<String>,
    text_citations: BTreeMap<(u64, u64), Value>,
    content_nodes: BTreeMap<u64, Node>,
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
) -> (
    Option<String>,
    String,
    HashMap<String, Value>,
    StreamTerminalError,
) {
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
    let error_type = error_value
        .get("type")
        .and_then(|v| v.as_str())
        .or_else(|| data_val.get("type").and_then(|v| v.as_str()))
        .filter(|value| *value != event_name)
        .map(|value| value.to_string());
    let param = error_value
        .get("param")
        .and_then(|v| v.as_str())
        .or_else(|| data_val.get("param").and_then(|v| v.as_str()))
        .map(|value| value.to_string());
    let http_status = error_value
        .get("status")
        .and_then(|v| v.as_u64())
        .or_else(|| data_val.get("status").and_then(|v| v.as_u64()))
        .filter(|status| (400..=599).contains(status))
        .and_then(|status| u16::try_from(status).ok())
        .unwrap_or(StatusCode::BAD_REQUEST.as_u16());
    let terminal_error = StreamTerminalError {
        code: code
            .clone()
            .unwrap_or_else(|| "upstream_stream_error".to_string()),
        message: message.clone(),
        http_status,
        error_type: error_type.clone(),
        param: param.clone(),
    };
    let mut extra_body = split_known_fields(error_value, &["code", "message"]);
    extra_body.extend(split_known_fields(
        data_val,
        &["code", "message", "type", "error", "response"],
    ));
    (code, message, extra_body, terminal_error)
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

fn stable_message_item_id_for_output(
    index_state: &mut ResponsesStreamIndexState,
    output_index: u64,
) -> String {
    output_state_for(index_state, output_index)
        .item_id
        .get_or_insert_with(crate::urp::synthetic_message_id)
        .clone()
}

fn output_role_to_ordinary(role: Role) -> OrdinaryRole {
    role.to_ordinary().unwrap_or(OrdinaryRole::Assistant)
}

fn first_node_from_item_value(item: &Value) -> Option<Node> {
    nodes_from_item_value(item).into_iter().next()
}
