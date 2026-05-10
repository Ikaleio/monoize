fn map_response_completed_with_accumulated(
    data_val: Value,
    _index_state: &mut ResponsesStreamIndexState,
    accumulated_outputs: &[Node],
) -> Vec<UrpStreamEvent> {
    let mut events = Vec::new();
    let response_obj = data_val
        .get("response")
        .and_then(|v| v.as_object())
        .cloned()
        .or_else(|| data_val.as_object().cloned());
    let Some(response_obj) = response_obj else {
        return events;
    };
    let response_value = Value::Object(response_obj.clone());
    let decoded = crate::urp::decode::openai_responses::decode_response(&response_value).ok();
    let terminal_outputs = decoded
        .as_ref()
        .map(|resp| resp.output.clone())
        .unwrap_or_else(|| {
            response_obj
                .get("output")
                .and_then(|v| v.as_array())
                .map(|items| items.iter().flat_map(nodes_from_item_value).collect())
                .unwrap_or_default()
        });
    let outputs = merge_response_completed_outputs(terminal_outputs, accumulated_outputs);
    let finish_reason = if outputs_have_tool_calls(&outputs) {
        Some(FinishReason::ToolCalls)
    } else {
        decoded
            .as_ref()
            .and_then(|resp| resp.finish_reason)
            .or_else(
                || match response_obj.get("status").and_then(|v| v.as_str()) {
                    Some("completed") => Some(FinishReason::Stop),
                    Some("incomplete") => Some(FinishReason::Length),
                    Some("failed") => Some(FinishReason::Other),
                    _ => None,
                },
            )
    };
    events.push(UrpStreamEvent::ResponseDone {
        finish_reason,
        usage: decoded
            .and_then(|resp| resp.usage)
            .or_else(|| parse_usage_from_responses_object(&response_value)),
        output: outputs,
        extra_body: split_known_fields(
            response_value,
            &[
                "id",
                "object",
                "created",
                "created_at",
                "model",
                "status",
                "output",
                "usage",
                "error",
            ],
        ),
    });
    events
}

fn map_response_completed(
    data_val: Value,
    index_state: &mut ResponsesStreamIndexState,
) -> Vec<UrpStreamEvent> {
    map_response_completed_with_accumulated(data_val, index_state, &[])
}

fn urp_node_index_from_delta(data_val: &Value, index_state: &mut ResponsesStreamIndexState) -> u32 {
    let output_index = data_val
        .get("output_index")
        .and_then(|v| v.as_u64())
        .unwrap_or(0);
    if let Some(content_index) = data_val
        .get("content_index")
        .or_else(|| data_val.get("part_index"))
        .and_then(|v| v.as_u64())
    {
        return index_state.node_index_for_content(output_index, content_index);
    }
    index_state.synthetic_node_index_for_output(output_index)
}

fn output_text_delta_content(data_val: &Value) -> &str {
    data_val
        .get("delta")
        .and_then(|v| v.as_str())
        .or_else(|| data_val.get("text").and_then(|v| v.as_str()))
        .unwrap_or_default()
}

fn delta_extra_body_with_phase(
    data_val: Value,
    message_phases_by_output_index: &HashMap<u64, String>,
) -> HashMap<String, Value> {
    let mut extra = split_known_fields(
        data_val.clone(),
        &[
            "delta",
            "text",
            "output_index",
            "content_index",
            "part_index",
            "item_id",
            "logprobs",
            "phase",
        ],
    );
    if let Some(idx) = data_val.get("output_index").and_then(|v| v.as_u64()) {
        if let Some(phase) = message_phases_by_output_index.get(&idx) {
            extra
                .entry("phase".to_string())
                .or_insert_with(|| json!(phase));
        }
    }
    extra
}

fn role_from_item(item: &Value) -> Role {
    match item
        .get("role")
        .and_then(|v| v.as_str())
        .unwrap_or("assistant")
    {
        "system" => Role::System,
        "developer" => Role::Developer,
        "user" => Role::User,
        "tool" => Role::Tool,
        _ => Role::Assistant,
    }
}

fn decode_item_from_value(item: &Value) -> Item {
    match item.get("type").and_then(|v| v.as_str()).unwrap_or("") {
        "message" => {
            let parts = item
                .get("content")
                .and_then(|v| v.as_array())
                .map(|parts| parts.iter().map(decode_part_from_value).collect())
                .unwrap_or_default();
            Item::Message {
                id: item
                    .get("id")
                    .and_then(|v| v.as_str())
                    .map(|s| s.to_string())
                    .or_else(|| Some(crate::urp::synthetic_message_id())),
                role: role_from_item(item),
                parts,
                extra_body: item_extra_body_from_value(item),
            }
        }
        "function_call_output" => Item::ToolResult {
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
        },
        "reasoning" => Item::Message {
            id: item
                .get("id")
                .and_then(|v| v.as_str())
                .map(|s| s.to_string())
                .or_else(|| Some(crate::urp::synthetic_reasoning_id())),
            role: Role::Assistant,
            parts: vec![decode_part_from_value(item)],
            extra_body: HashMap::new(),
        },
        "function_call" => Item::Message {
            id: item
                .get("id")
                .and_then(|v| v.as_str())
                .map(|s| s.to_string())
                .or_else(|| Some(crate::urp::synthetic_tool_call_id())),
            role: Role::Assistant,
            parts: vec![decode_part_from_value(item)],
            extra_body: HashMap::new(),
        },
        "image_generation_call" => Item::Message {
            id: item
                .get("id")
                .and_then(|v| v.as_str())
                .map(|s| s.to_string())
                .or_else(|| Some(crate::urp::synthetic_provider_item_id())),
            role: Role::Assistant,
            parts: vec![decode_part_from_value(item)],
            extra_body: HashMap::new(),
        },
        other => Item::Message {
            id: item
                .get("id")
                .and_then(|v| v.as_str())
                .map(|s| s.to_string())
                .or_else(|| Some(crate::urp::synthetic_provider_item_id())),
            role: Role::Assistant,
            parts: vec![Part::ProviderItem {
                id: item
                    .get("id")
                    .and_then(|v| v.as_str())
                    .map(|s| s.to_string())
                    .or_else(|| Some(crate::urp::synthetic_provider_item_id())),
                item_type: other.to_string(),
                body: item.clone(),
                extra_body: HashMap::new(),
            }],
            extra_body: HashMap::new(),
        },
    }
}

fn decode_part_from_value(part: &Value) -> Part {
    match part.get("type").and_then(|v| v.as_str()).unwrap_or("") {
        "output_text" | "text" => Part::Text {
            content: part
                .get("text")
                .and_then(|v| v.as_str())
                .unwrap_or_default()
                .to_string(),
            extra_body: part_extra_body_from_value(part),
        },
        "reasoning" => Part::Reasoning {
            id: part
                .get("id")
                .and_then(|v| v.as_str())
                .map(|s| s.to_string())
                .or_else(|| Some(crate::urp::synthetic_reasoning_id())),
            content: part
                .get("text")
                .and_then(|v| v.as_str())
                .map(|s| s.to_string()),
            encrypted: part.get("encrypted_content").cloned(),
            summary: part
                .get("summary")
                .and_then(|v| v.as_array())
                .map(|summary| {
                    summary
                        .iter()
                        .filter(|entry| {
                            entry.get("type").and_then(|v| v.as_str()) == Some("summary_text")
                        })
                        .filter_map(|entry| entry.get("text").and_then(|v| v.as_str()))
                        .filter(|text| !text.is_empty())
                        .collect::<Vec<_>>()
                        .join("\n")
                })
                .filter(|summary| !summary.is_empty()),
            source: part
                .get("source")
                .and_then(|v| v.as_str())
                .map(|s| s.to_string()),
            extra_body: part_extra_body_from_value(part),
        },
        "refusal" => Part::Refusal {
            content: part
                .get("refusal")
                .or_else(|| part.get("text"))
                .and_then(|v| v.as_str())
                .unwrap_or_default()
                .to_string(),
            extra_body: part_extra_body_from_value(part),
        },
        "function_call" | "tool_call" => Part::ToolCall {
            id: part
                .get("id")
                .and_then(|v| v.as_str())
                .map(|s| s.to_string())
                .or_else(|| Some(crate::urp::synthetic_tool_call_id())),
            call_id: part
                .get("call_id")
                .and_then(|v| v.as_str())
                .unwrap_or_default()
                .to_string(),
            name: part
                .get("name")
                .and_then(|v| v.as_str())
                .unwrap_or_default()
                .to_string(),
            arguments: part
                .get("arguments")
                .and_then(|v| v.as_str())
                .unwrap_or_default()
                .to_string(),
            extra_body: part_extra_body_from_value(part),
        },
        "image_generation_call" => image_node_from_image_generation_payload(part)
            .map(|node| match node {
                Node::Image {
                    source, extra_body, ..
                } => Part::Image { source, extra_body },
                _ => unreachable!(),
            })
            .unwrap_or_else(|| Part::ProviderItem {
                id: part
                    .get("id")
                    .and_then(|v| v.as_str())
                    .map(|s| s.to_string())
                    .or_else(|| Some(crate::urp::synthetic_provider_item_id())),
                item_type: "image_generation_call".to_string(),
                body: part.clone(),
                extra_body: HashMap::new(),
            }),
        other => Part::ProviderItem {
            id: part
                .get("id")
                .and_then(|v| v.as_str())
                .map(|s| s.to_string())
                .or_else(|| Some(crate::urp::synthetic_provider_item_id())),
            item_type: other.to_string(),
            body: part.clone(),
            extra_body: HashMap::new(),
        },
    }
}

