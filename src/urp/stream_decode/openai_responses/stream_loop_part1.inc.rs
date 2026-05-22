pub(crate) async fn stream_responses_to_urp_events(
    urp: &HandlerUrpRequest,
    mut pending_request_envelope_extra: Option<HashMap<String, Value>>,
    upstream_resp: reqwest::Response,
    tx: mpsc::Sender<UrpStreamEvent>,
    started_at: Option<std::time::Instant>,
    runtime_metrics: Option<Arc<Mutex<StreamRuntimeMetrics>>>,
) -> AppResult<()> {
    let response_id = format!("resp_{}", uuid::Uuid::new_v4());
    let created = now_ts();
    let mut output_texts_by_output_index: HashMap<u64, String> = HashMap::new();
    let mut message_phases_by_output_index: HashMap<u64, String> = HashMap::new();
    let mut message_item_extra_by_output_index: HashMap<u64, HashMap<String, Value>> =
        HashMap::new();
    let mut item_ids_by_output_index: HashMap<u64, String> = HashMap::new();
    let mut reasoning_text = String::new();
    let mut reasoning_summary_text = String::new();
    let mut reasoning_sig = String::new();
    let mut reasoning_source: Option<String> = None;
    let mut reasoning_output_index: Option<u64> = None;
    let mut call_order: Vec<String> = Vec::new();
    let mut calls: HashMap<String, (String, String)> = HashMap::new();
    let mut call_ids_by_output_index: HashMap<u64, String> = HashMap::new();
    let mut saw_text_delta = false;
    let mut saw_text_part_done = false;
    let mut response_done_sent = false;
    let mut index_state = ResponsesStreamIndexState::default();

    let _ = tx
        .send(UrpStreamEvent::ResponseStart {
            id: response_id.clone(),
            model: urp.model.clone(),
            extra_body: HashMap::from([
                ("object".to_string(), json!("response")),
                ("created_at".to_string(), json!(created)),
                ("status".to_string(), json!("in_progress")),
                ("output".to_string(), json!([])),
            ]),
        })
        .await;

    let idle_timeout = std::time::Duration::from_secs(120);
    let mut stream = upstream_resp.bytes_stream().eventsource();
    while let Some(ev) = tokio::time::timeout(idle_timeout, stream.next())
        .await
        .map_err(|_| {
            AppError::new(
                StatusCode::GATEWAY_TIMEOUT,
                "upstream_idle_timeout",
                "upstream stream idle for 120s without data",
            )
        })?
    {
        let ev = ev.map_err(|err| {
            AppError::new(
                StatusCode::BAD_GATEWAY,
                "upstream_stream_decode_failed",
                err.to_string(),
            )
        })?;
        if tx.is_closed() {
            break;
        }
        mark_stream_ttfb_if_needed(started_at, &runtime_metrics).await;
        if ev.data.trim() == "[DONE]" {
            record_stream_done_sentinel(&runtime_metrics).await;
            break;
        }
        let data_val: Value = serde_json::from_str(&ev.data).unwrap_or(Value::String(ev.data));
        record_stream_usage_if_present(
            &runtime_metrics,
            parse_usage_from_responses_object(&data_val),
        )
        .await;

        if ev.event == "error" || ev.event == "response.failed" {
            let (code, message, extra_body, terminal_error) =
                responses_stream_error_parts(&ev.event, data_val);
            let _ = tx
                .send(UrpStreamEvent::Error {
                    code,
                    message,
                    extra_body,
                })
                .await;
            record_stream_terminal_error(&runtime_metrics, &ev.event, terminal_error).await;
            return Ok(());
        }

        if ev.event == "response.output_text.delta" {
            if let Some(text) = data_val.get("delta").and_then(|v| v.as_str()) {
                let output_index = data_val
                    .get("output_index")
                    .and_then(|v| v.as_u64())
                    .unwrap_or(0);
                output_texts_by_output_index
                    .entry(output_index)
                    .or_default()
                    .push_str(text);
                if !message_item_extra_by_output_index.contains_key(&output_index) {
                    if let Some(extra_body) = pending_request_envelope_extra.take() {
                        output_state_for(&mut index_state, output_index).item_extra_body =
                            extra_body.clone();
                        message_item_extra_by_output_index.insert(output_index, extra_body);
                    }
                }
                saw_text_delta = true;
            }
        }
        if ev.event == "response.reasoning.delta" {
            tracing::info!(
                target: "monoize::urp::reasoning_trace",
                event = %ev.event,
                output_index = data_val.get("output_index").and_then(|v| v.as_u64()).unwrap_or(0),
                item_id = data_val.get("item_id").and_then(|v| v.as_str()).unwrap_or(""),
                has_text = data_val.get("delta").and_then(|v| v.as_str()).is_some_and(|v| !v.is_empty()),
                "responses reasoning delta observed"
            );
            if let Some(delta) = data_val
                .get("delta")
                .and_then(|v| v.as_str())
                .or_else(|| data_val.get("text").and_then(|v| v.as_str()))
            {
                reasoning_text.push_str(delta);
            }
        }
        if ev.event == "response.reasoning.done" {
            if let Some(text) = data_val
                .get("text")
                .and_then(|v| v.as_str())
                .or_else(|| data_val.get("delta").and_then(|v| v.as_str()))
            {
                if reasoning_text.is_empty() {
                    reasoning_text = text.to_string();
                }
            }
        }
        if ev.event == "response.reasoning_summary_text.delta" {
            tracing::info!(
                target: "monoize::urp::reasoning_trace",
                event = %ev.event,
                output_index = data_val.get("output_index").and_then(|v| v.as_u64()).unwrap_or(0),
                item_id = data_val.get("item_id").and_then(|v| v.as_str()).unwrap_or(""),
                has_text = data_val.get("delta").and_then(|v| v.as_str()).is_some_and(|v| !v.is_empty()),
                "responses reasoning summary delta observed"
            );
            if let Some(delta) = data_val
                .get("delta")
                .and_then(|v| v.as_str())
                .or_else(|| data_val.get("text").and_then(|v| v.as_str()))
            {
                reasoning_summary_text.push_str(delta);
            }
            if let (Some(idx), Some(id)) = (
                data_val.get("output_index").and_then(|v| v.as_u64()),
                data_val.get("item_id").and_then(|v| v.as_str()),
            ) {
                if !id.is_empty() {
                    item_ids_by_output_index.insert(idx, id.to_string());
                }
            }
        }
        if ev.event == "response.output_item.added" {
            let item = data_val.get("item").unwrap_or(&data_val);
            if item.get("type").and_then(|v| v.as_str()) == Some("reasoning") {
                tracing::info!(
                    target: "monoize::urp::reasoning_trace",
                    event = %ev.event,
                    output_index = data_val.get("output_index").and_then(|v| v.as_u64()).unwrap_or(0),
                    item_id = item.get("id").and_then(|v| v.as_str()).unwrap_or(""),
                    encrypted_len = item
                        .get("encrypted_content")
                        .and_then(|v| v.as_str())
                        .map(|v| v.len())
                        .unwrap_or(0),
                    "responses reasoning output item added"
                );
            }
            if let (Some(idx), Some(id)) = (
                data_val.get("output_index").and_then(|v| v.as_u64()),
                item.get("id").and_then(|v| v.as_str()),
            ) {
                if !id.is_empty() {
                    item_ids_by_output_index
                        .entry(idx)
                        .or_insert_with(|| id.to_string());
                }
            }
            if item.get("type").and_then(|v| v.as_str()) == Some("function_call") {
                if let Some(call_id) = item.get("call_id").and_then(|v| v.as_str()) {
                    if !calls.contains_key(call_id) {
                        call_order.push(call_id.to_string());
                        calls.insert(
                            call_id.to_string(),
                            (
                                item.get("name")
                                    .and_then(|v| v.as_str())
                                    .unwrap_or("")
                                    .to_string(),
                                item.get("arguments")
                                    .and_then(|v| v.as_str())
                                    .unwrap_or("")
                                    .to_string(),
                            ),
                        );
                    }
                    if let Some(idx) = data_val.get("output_index").and_then(|v| v.as_u64()) {
                        call_ids_by_output_index.insert(idx, call_id.to_string());
                    }
                }
            } else if item.get("type").and_then(|v| v.as_str()) == Some("message") {
                if let Some(idx) = data_val.get("output_index").and_then(|v| v.as_u64()) {
                    if !message_item_extra_by_output_index.contains_key(&idx) {
                        if let Some(extra_body) = pending_request_envelope_extra.take() {
                            output_state_for(&mut index_state, idx).item_extra_body =
                                extra_body.clone();
                            message_item_extra_by_output_index.insert(idx, extra_body);
                        }
                    }
                    let text = extract_responses_message_text(item);
                    if !text.is_empty() {
                        output_texts_by_output_index.entry(idx).or_insert(text);
                    }
                    if let Some(phase) = extract_responses_message_phase(item) {
                        message_phases_by_output_index.insert(idx, phase);
                    }
                }
            } else if item.get("type").and_then(|v| v.as_str()) == Some("reasoning") {
                if let Some(idx) = data_val.get("output_index").and_then(|v| v.as_u64()) {
                    reasoning_output_index.get_or_insert(idx);
                }
                merge_reasoning_source(&mut reasoning_source, reasoning_source_from_value(item));
                let (text, summary, sig) = extract_reasoning_parts(item);
                if reasoning_text.is_empty() && !text.is_empty() {
                    reasoning_text = text;
                }
                if reasoning_summary_text.is_empty() && !summary.is_empty() {
                    reasoning_summary_text = summary;
                }
                if reasoning_sig.is_empty() && !sig.is_empty() {
                    reasoning_sig = sig;
                }
            }
        }
        if ev.event == "response.function_call_arguments.delta" {
            let call_id_opt = data_val
                .get("call_id")
                .and_then(|v| v.as_str())
                .map(|s| s.to_string())
                .or_else(|| {
                    data_val
                        .get("output_index")
                        .and_then(|v| v.as_u64())
                        .and_then(|idx| call_ids_by_output_index.get(&idx).cloned())
                });
            if let Some(call_id) = call_id_opt {
                let name = data_val.get("name").and_then(|v| v.as_str()).unwrap_or("");
                let delta = data_val.get("delta").and_then(|v| v.as_str()).unwrap_or("");
                if !calls.contains_key(call_id.as_str()) {
                    call_order.push(call_id.clone());
                    calls.insert(call_id.clone(), (name.to_string(), String::new()));
                }
                if let Some(entry) = calls.get_mut(call_id.as_str()) {
                    if entry.0.is_empty() && !name.is_empty() {
                        entry.0 = name.to_string();
                    }
                    entry.1.push_str(delta);
                }
            }
        }
        if ev.event == "response.function_call_arguments.done" {
            let call_id_opt = data_val
                .get("call_id")
                .and_then(|v| v.as_str())
                .map(|s| s.to_string())
                .or_else(|| {
                    data_val
                        .get("output_index")
                        .and_then(|v| v.as_u64())
                        .and_then(|idx| call_ids_by_output_index.get(&idx).cloned())
                });
            if let Some(call_id) = call_id_opt {
                let args = data_val
                    .get("arguments")
                    .and_then(|v| v.as_str())
                    .unwrap_or("");
                if let Some(entry) = calls.get_mut(call_id.as_str()) {
                    if entry.1.is_empty() && !args.is_empty() {
                        entry.1 = args.to_string();
                    }
                }
            }
        }
        if ev.event == "response.output_item.done" {
            let item = data_val.get("item").unwrap_or(&data_val);
            if item.get("type").and_then(|v| v.as_str()) == Some("reasoning") {
                tracing::info!(
                    target: "monoize::urp::reasoning_trace",
                    event = %ev.event,
                    output_index = data_val.get("output_index").and_then(|v| v.as_u64()).unwrap_or(0),
                    item_id = item.get("id").and_then(|v| v.as_str()).unwrap_or(""),
                    encrypted_len = item
                        .get("encrypted_content")
                        .and_then(|v| v.as_str())
                        .map(|v| v.len())
                        .unwrap_or(0),
                    text_len = item.get("text").and_then(|v| v.as_str()).map(|v| v.len()).unwrap_or(0),
                    "responses reasoning output item done"
                );
            }
            if let (Some(idx), Some(id)) = (
                data_val.get("output_index").and_then(|v| v.as_u64()),
                item.get("id").and_then(|v| v.as_str()),
            ) {
                if !id.is_empty() {
                    item_ids_by_output_index
                        .entry(idx)
                        .or_insert_with(|| id.to_string());
                }
            }
            if item.get("type").and_then(|v| v.as_str()) == Some("function_call") {
                if let Some(call_id) = item.get("call_id").and_then(|v| v.as_str()) {
                    let name = item.get("name").and_then(|v| v.as_str()).unwrap_or("");
                    let args = item.get("arguments").and_then(|v| v.as_str()).unwrap_or("");
                    if !calls.contains_key(call_id) {
                        call_order.push(call_id.to_string());
                        calls.insert(call_id.to_string(), (name.to_string(), args.to_string()));
                    } else if let Some(entry) = calls.get_mut(call_id) {
                        if entry.0.is_empty() && !name.is_empty() {
                            entry.0 = name.to_string();
                        }
                        if entry.1.is_empty() && !args.is_empty() {
                            entry.1 = args.to_string();
                        }
                    }
                    if let Some(idx) = data_val.get("output_index").and_then(|v| v.as_u64()) {
                        call_ids_by_output_index.insert(idx, call_id.to_string());
                    }
                }
            } else if item.get("type").and_then(|v| v.as_str()) == Some("reasoning") {
                if let Some(idx) = data_val.get("output_index").and_then(|v| v.as_u64()) {
                    reasoning_output_index.get_or_insert(idx);
                }
                merge_reasoning_source(&mut reasoning_source, reasoning_source_from_value(item));
                let (text, summary, sig) = extract_reasoning_parts(item);
                if !text.is_empty() {
                    reasoning_text = text;
                }
                if !summary.is_empty() {
                    reasoning_summary_text = summary;
                }
                if !sig.is_empty() {
                    reasoning_sig = sig;
                }
            } else if item.get("type").and_then(|v| v.as_str()) == Some("message")
                && !saw_text_delta
            {
                if let Some(idx) = data_val.get("output_index").and_then(|v| v.as_u64()) {
                    let text = extract_responses_message_text(item);
                    if !text.is_empty() {
                        output_texts_by_output_index
                            .entry(idx)
                            .or_default()
                            .push_str(&text);
                    }
                    if let Some(phase) = extract_responses_message_phase(item) {
                        message_phases_by_output_index.insert(idx, phase);
                    }
                }
            }
        }
        if ev.event == "response.content_part.done"
            && data_val
                .get("part")
                .and_then(|part| part.get("type"))
                .and_then(|v| v.as_str())
                .is_some_and(|part_type| matches!(part_type, "output_text" | "text"))
            && data_val
                .get("part")
                .and_then(|part| part.get("text"))
                .and_then(|v| v.as_str())
                .is_some_and(|text| !text.is_empty())
        {
            saw_text_part_done = true;
        }

        let stream_events = if ev.event == "response.completed" {
            let accumulated_output_nodes = build_accumulated_output_nodes(
                &reasoning_text,
                &reasoning_summary_text,
                &reasoning_sig,
                reasoning_source.as_deref(),
                reasoning_output_index,
                &output_texts_by_output_index,
                &message_phases_by_output_index,
                &message_item_extra_by_output_index,
                &item_ids_by_output_index,
                &call_order,
                &calls,
                &call_ids_by_output_index,
            );
            map_response_completed_with_accumulated(
                data_val,
                &mut index_state,
                &accumulated_output_nodes,
            )
        } else {
            map_responses_event_to_urp_events_with_state(
                &ev.event,
                data_val,
                &message_phases_by_output_index,
                &mut index_state,
            )
        };
        for stream_event in stream_events {
            response_done_sent |= matches!(stream_event, UrpStreamEvent::ResponseDone { .. });
            let _ = tx.send(stream_event).await;
        }
    }

    if !response_done_sent {
        let output_nodes = build_accumulated_output_nodes(
            &reasoning_text,
            &reasoning_summary_text,
            &reasoning_sig,
            reasoning_source.as_deref(),
            reasoning_output_index,
            &output_texts_by_output_index,
            &message_phases_by_output_index,
            &message_item_extra_by_output_index,
            &item_ids_by_output_index,
            &call_order,
            &calls,
            &call_ids_by_output_index,
        );
        let output_items = nodes_to_items(&output_nodes);
        let final_usage = latest_stream_usage_snapshot(&runtime_metrics).await;

        if !saw_text_delta && !saw_text_part_done {
            let mut grouped_output_nodes = Vec::new();
            for output_item in &output_items {
                grouped_output_nodes.push(match output_item {
                    Item::Message {
                        id,
                        role,
                        parts,
                        extra_body,
                    } => {
                        let ordinary_role = role.to_ordinary().unwrap_or(OrdinaryRole::User);
                        let mut item_nodes = Vec::new();
                        for (index, part) in parts.iter().cloned().enumerate() {
                            let mut node = part.into_node(ordinary_role);
                            if index == 0 && !extra_body.is_empty() {
                                node.extra_body_mut().extend(extra_body.clone());
                            }
                            if index == 0 {
                                node.set_id(id.clone());
                            }
                            item_nodes.push(node);
                        }
                        item_nodes
                    }
                    Item::ToolResult {
                        id,
                        call_id,
                        is_error,
                        content,
                        extra_body,
                    } => vec![Node::ToolResult {
                        id: id.clone(),
                        call_id: call_id.clone(),
                        is_error: *is_error,
                        content: content.clone(),
                        extra_body: extra_body.clone(),
                    }],
                });
            }

            for (output_item, item_nodes) in output_items.iter().zip(grouped_output_nodes.iter()) {
                let Item::Message {
                    role: Role::Assistant,
                    ..
                } = output_item
                else {
                    continue;
                };

                for node in item_nodes.iter().filter(|node| {
                    matches!(
                        node,
                        Node::Text {
                            role: OrdinaryRole::Assistant,
                            ..
                        }
                    )
                }) {
                    let Node::Text {
                        id,
                        role: OrdinaryRole::Assistant,
                        content,
                        phase,
                        extra_body,
                    } = node.clone()
                    else {
                        continue;
                    };
                    let node_index = index_state.allocate_fresh_node_index();
                    let synthetic_node = Node::Text {
                        id,
                        role: OrdinaryRole::Assistant,
                        content: content.clone(),
                        phase: phase.clone(),
                        extra_body: extra_body.clone(),
                    };
                    let item_extra = item_extra_body_from_item(&output_item);
                    if !item_extra.is_empty() {
                        let _ = tx
                            .send(UrpStreamEvent::NodeStart {
                                node_index: index_state.allocate_fresh_node_index(),
                                header: NodeHeader::NextDownstreamEnvelopeExtra,
                                extra_body: item_extra.clone(),
                            })
                            .await;
                        let _ = tx
                            .send(UrpStreamEvent::NodeDone {
                                node_index: index_state.allocate_fresh_node_index() - 1,
                                node: Node::NextDownstreamEnvelopeExtra {
                                    extra_body: item_extra.clone(),
                                },
                                usage: final_usage.clone(),
                                extra_body: item_extra.clone(),
                            })
                            .await;
                    }

                    let _ = tx
                        .send(UrpStreamEvent::NodeStart {
                            node_index,
                            header: NodeHeader::Text {
                                id: synthetic_node.id().cloned(),
                                role: OrdinaryRole::Assistant,
                                phase,
                            },
                            extra_body: extra_body.clone(),
                        })
                        .await;
                    let _ = tx
                        .send(UrpStreamEvent::NodeDelta {
                            node_index,
                            delta: NodeDelta::Text {
                                content: content.clone(),
                            },
                            usage: final_usage.clone(),
                            extra_body: extra_body.clone(),
                        })
                        .await;
                    let _ = tx
                        .send(UrpStreamEvent::NodeDone {
                            node_index,
                            node: synthetic_node,
                            usage: final_usage.clone(),
                            extra_body: extra_body.clone(),
                        })
                        .await;
                }
            }
        }

        let _ = tx
            .send(UrpStreamEvent::ResponseDone {
                finish_reason: Some(if outputs_have_tool_calls(&output_nodes) {
                    FinishReason::ToolCalls
                } else {
                    FinishReason::Stop
                }),
                usage: final_usage,
                output: output_nodes,
                extra_body: HashMap::from([
                    ("id".to_string(), json!(response_id)),
                    ("object".to_string(), json!("response")),
                    ("created_at".to_string(), json!(created)),
                    ("model".to_string(), json!(urp.model.clone())),
                    ("status".to_string(), json!("completed")),
                ]),
            })
            .await;
    }
    record_stream_terminal_event(&runtime_metrics, "response.completed", None).await;
    Ok(())
}
