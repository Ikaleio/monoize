fn map_response_completed_with_accumulated(
    data_val: Value,
    index_state: &mut ResponsesStreamIndexState,
    accumulated_outputs: &[AccumulatedOutputEntry],
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
    let terminal_outputs = response_obj
        .get("output")
        .and_then(|v| v.as_array())
        .map(|items| {
            items
                .iter()
                .enumerate()
                .map(|(output_index, item)| {
                    let output_index = output_index as u64;
                    let mut item = item.clone();
                    let terminal_id_missing = item
                        .get("id")
                        .and_then(Value::as_str)
                        .is_none_or(str::is_empty);
                    let terminal_item_kind = item
                        .get("type")
                        .and_then(Value::as_str)
                        .and_then(response_output_item_type_kind);
                    let streamed_item_kind = index_state
                        .output_state_by_index
                        .get(&output_index)
                        .and_then(|state| state.item_type.as_deref())
                        .and_then(response_output_item_type_kind);
                    let position_has_compatible_kind =
                        terminal_item_kind.is_some() && terminal_item_kind == streamed_item_kind;
                    let mut borrowed_streamed_id = false;
                    if terminal_id_missing
                        && position_has_compatible_kind
                        && let Some(streamed_id) = index_state
                            .output_state_by_index
                            .get(&output_index)
                            .and_then(|state| state.item_id.as_deref())
                            .filter(|id| !id.is_empty())
                        && let Some(item_obj) = item.as_object_mut()
                    {
                        item_obj.insert("id".to_string(), Value::String(streamed_id.to_string()));
                        borrowed_streamed_id = true;
                    }
                    let mut nodes = nodes_from_item_value(&item);
                    if terminal_id_missing && !borrowed_streamed_id {
                        for node in &mut nodes {
                            node.set_id(None);
                        }
                    }
                    AccumulatedOutputEntry {
                        output_index,
                        nodes,
                    }
                })
                .collect::<Vec<_>>()
        })
        .unwrap_or_default();
    let outputs = match merge_response_completed_outputs(terminal_outputs, accumulated_outputs) {
        Ok(outputs) => outputs,
        Err(reason) => {
            events.push(UrpStreamEvent::Error {
                code: Some("responses_terminal_conflict".to_string()),
                message: format!(
                    "Responses stream terminal output conflicts with streamed state: {reason}"
                ),
                extra_body: HashMap::from([("conflict_reason".to_string(), Value::String(reason))]),
            });
            return events;
        }
    };
    let finish_reason = match response_obj.get("status").and_then(Value::as_str) {
        Some("incomplete") => {
            Some(crate::urp::decode::openai_responses::incomplete_finish_reason(&response_obj))
        }
        Some("failed" | "cancelled") => Some(FinishReason::Other),
        Some("completed") if outputs_have_tool_calls(&outputs) => Some(FinishReason::ToolCalls),
        Some("completed") => decoded
            .as_ref()
            .and_then(|resp| resp.finish_reason)
            .or(Some(FinishReason::Stop)),
        _ => decoded.as_ref().and_then(|resp| resp.finish_reason),
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
                "output",
                "usage",
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
    if index_state
        .output_state_by_index
        .get(&output_index)
        .and_then(|state| state.item_type.as_deref())
        == Some("reasoning")
    {
        return index_state.synthetic_node_index_for_output(output_index);
    }
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

fn text_delta_extra_body(data_val: Value) -> HashMap<String, Value> {
    split_known_fields(
        data_val,
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
    )
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

fn decode_part_from_value(part: &Value) -> Part {
    if let Some(obj) = part.as_object() {
        if let Ok(Some(media)) = crate::urp::decode::parse_compatible_media_part(obj) { return media; }
    }
    let part_type = part.get("type").and_then(|v| v.as_str()).unwrap_or("");
    match part_type {
        "input_text" | "output_text" | "text" => Part::Text {
            signature: None,
            citations: part.get("annotations").and_then(Value::as_array).cloned().unwrap_or_default(),
            content: part
                .get("text")
                .and_then(|v| v.as_str())
                .unwrap_or_default()
                .to_string(),
            extra_body: part_extra_body_from_value(part),
        },
        "reasoning" => Part::Reasoning {
            metadata: Default::default(),
            id: part
                .get("id")
                .and_then(|v| v.as_str())
                .map(|s| s.to_string())
                .or_else(|| Some(crate::urp::synthetic_reasoning_id())),
            content: {
                let (content, _, _) = extract_reasoning_parts(part);
                (!content.is_empty()).then_some(content)
            },
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
        "reasoning_text" => Part::Reasoning {
            metadata: Default::default(),
            id: None,
            content: part
                .get("text")
                .and_then(Value::as_str)
                .filter(|text| !text.is_empty())
                .map(str::to_string),
            encrypted: None,
            summary: None,
            source: part
                .get("source")
                .and_then(Value::as_str)
                .filter(|source| !source.is_empty())
                .map(str::to_string),
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
        "function_call" | "tool_call" | "custom_tool_call" => Part::ToolCall {
            namespace: part.get("namespace").and_then(Value::as_str).map(str::to_string),
            signature: None,
            id: part
                .get("id")
                .and_then(|v| v.as_str())
                .map(|s| s.to_string())
                .or_else(|| Some(crate::urp::synthetic_tool_call_id())),
            tool_type: if part_type == "custom_tool_call" {
                ToolCallType::Custom
            } else {
                ToolCallType::Function
            },
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
                .get(if part_type == "custom_tool_call" {
                    "input"
                } else {
                    "arguments"
                })
                .and_then(|v| v.as_str())
                .unwrap_or_default()
                .to_string(),
            extra_body: part_extra_body_from_value(part),
        },
        "image_generation_call" => image_node_from_image_generation_payload(part)
            .map(|node| match node {
                Node::Image {
                    source, metadata, extra_body, ..
                } => Part::Image { source, metadata, extra_body },
                _ => unreachable!(),
            })
            .unwrap_or_else(|| Part::ProviderItem {
                id: part
                    .get("id")
                    .and_then(|v| v.as_str())
                    .map(|s| s.to_string())
                    .or_else(|| Some(crate::urp::synthetic_provider_item_id())),
                origin_protocol: ProviderProtocol::Responses,
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
            origin_protocol: ProviderProtocol::Responses,
            item_type: other.to_string(),
            body: part.clone(),
            extra_body: HashMap::new(),
        },
    }
}
