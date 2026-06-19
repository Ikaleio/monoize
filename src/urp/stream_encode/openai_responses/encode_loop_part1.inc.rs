pub(crate) async fn encode_urp_stream_as_responses(
    mut rx: mpsc::Receiver<UrpStreamEvent>,
    tx: mpsc::Sender<Event>,
    logical_model: &str,
    stream_started_at: Instant,
    sse_max_frame_length: Option<usize>,
) -> AppResult<()> {
    let mut seq = 1u64;
    let mut response_id = "resp".to_string();
    let mut created: Option<i64> = None;
    let mut error_terminal_sent = false;
    let mut next_output_index = 0usize;
    let mut node_states: HashMap<u32, StreamedNodeState> = HashMap::new();
    let mut completed_output_items: Vec<(usize, Value)> = Vec::new();
    let mut completed_output_indices: HashSet<usize> = HashSet::new();
    let mut streamed_output_indices: HashSet<usize> = HashSet::new();
    let mut reasoning_delta_indices: HashSet<usize> = HashSet::new();
    let mut reasoning_done_indices: HashSet<usize> = HashSet::new();
    let mut reasoning_summary_added_indices: HashSet<usize> = HashSet::new();
    let mut reasoning_summary_delta_indices: HashSet<usize> = HashSet::new();
    let mut reasoning_summary_text_done_indices: HashSet<usize> = HashSet::new();
    let mut reasoning_summary_part_done_indices: HashSet<usize> = HashSet::new();
    let mut function_args_delta_indices: HashSet<usize> = HashSet::new();
    let mut function_args_done_indices: HashSet<usize> = HashSet::new();
    let mut pending_envelope_extra: HashMap<String, Value> = HashMap::new();
    let mut active_node_message_output: Option<ActiveResponsesOutputItem> = None;
    async fn ensure_node_message_start_emitted(
        tx: &mpsc::Sender<Event>,
        seq: &mut u64,
        node_state: &mut StreamedNodeState,
        pending_envelope_extra: &mut HashMap<String, Value>,
        active_node_message_output: &mut Option<ActiveResponsesOutputItem>,
        streamed_output_indices: &mut HashSet<usize>,
        sse_max_frame_length: Option<usize>,
    ) -> AppResult<()> {
        if node_state.zone != ResponsesOutputZone::Message || node_state.message_start_emitted {
            return Ok(());
        }
        let Some(header) = node_state.header.as_ref() else {
            return Ok(());
        };
        let envelope_extra = pending_envelope_extra.clone();
        let item = if node_state.is_shared_message_output {
            let active = active_node_message_output
                .as_mut()
                .filter(|active| active.output_index == node_state.output_index)
                .expect("shared node message output exists");
            active.envelope_extra = envelope_extra.clone();
            if let Some(obj) = active.item.as_object_mut() {
                merge_json_extra_preserving_typed(obj, &active.envelope_extra);
                merge_json_extra(obj, &node_state.node_extra_body);
                obj.insert("status".to_string(), json!("in_progress"));
            }
            node_state.completed_item = Some(complete_stream_output_item(active.item.clone()));
            active.item.clone()
        } else {
            let item = stream_output_item_start_stub_from_node_header(
                node_state.zone,
                header,
                &node_state.node_extra_body,
                &envelope_extra,
            );
            node_state.completed_item = Some(complete_stream_output_item(item.clone()));
            item
        };
        node_state.item_id = item
            .get("id")
            .and_then(Value::as_str)
            .unwrap_or_default()
            .to_string();
        if node_state.is_shared_message_output
            && let Some(active) = active_node_message_output.as_mut()
            && active.output_index == node_state.output_index
        {
            active.item = item.clone();
            active.item_id = node_state.item_id.clone();
        }
        let first_visible_item_for_output = streamed_output_indices.insert(node_state.output_index);
        if first_visible_item_for_output {
            send_responses_event(
                tx,
                seq,
                "response.output_item.added",
                json!({
                    "output_index": node_state.output_index,
                    "item": item,
                }),
            )
            .await?;
        }
        send_responses_event(
            tx,
            seq,
            "response.content_part.added",
            json!({
                "output_index": node_state.output_index,
                "content_index": node_state.content_index.unwrap_or(0),
                "item_id": node_state.item_id,
                "part": encode_node_start_content_part(header),
            }),
        )
        .await?;
        pending_envelope_extra.clear();
        node_state.message_start_emitted = true;
        let _ = sse_max_frame_length;
        Ok(())
    }

    while let Some(event) = rx.recv().await {
        if error_terminal_sent {
            continue;
        }

        match event {
            UrpStreamEvent::ResponseStart { id, extra_body, .. } => {
                response_id = id.clone();
                created = Some(
                    extra_body
                        .get("created_at")
                        .or_else(|| extra_body.get("created"))
                        .and_then(|v| v.as_i64())
                        .unwrap_or_else(now_ts),
                );

                let payload = response_envelope_payload(
                    &id,
                    created.expect("response.created timestamp set from response start"),
                    logical_model,
                    "in_progress",
                    Value::Array(Vec::new()),
                );
                send_responses_event(&tx, &mut seq, "response.created", payload.clone()).await?;
                send_responses_event(&tx, &mut seq, "response.in_progress", payload).await?;
            }
            UrpStreamEvent::NodeStart {
                node_index,
                header,
                extra_body,
            } => {
                if matches!(header, urp::NodeHeader::NextDownstreamEnvelopeExtra) {
                    merge_hashmap_extra_preserving_typed(&mut pending_envelope_extra, &extra_body);
                    continue;
                }
                if let urp::NodeHeader::ProviderItem {
                    origin_protocol, ..
                } = &header
                    && *origin_protocol != urp::ProviderProtocol::Responses
                {
                    continue;
                }
                let zone = zone_from_node_header(&header);
                let phase = node_header_phase(&header);
                let starts_new_shared_message = zone == ResponsesOutputZone::Message
                    && active_node_message_output.as_ref().is_none_or(|active| {
                        active.zone != ResponsesOutputZone::Message
                            || active
                                .item
                                .get("phase")
                                .and_then(Value::as_str)
                                .map(str::to_string)
                                != phase
                    });
                if zone == ResponsesOutputZone::Message && starts_new_shared_message {
                    let item = stream_output_item_start_stub_from_node_header(
                        zone,
                        &header,
                        &extra_body,
                        &HashMap::new(),
                    );
                    let item_id = item
                        .get("id")
                        .and_then(Value::as_str)
                        .unwrap_or_default()
                        .to_string();
                    active_node_message_output = Some(ActiveResponsesOutputItem {
                        zone,
                        output_index: next_output_index,
                        item_id,
                        item,
                        next_content_index: 0,
                        envelope_extra: HashMap::new(),
                    });
                    next_output_index += 1;
                } else if zone != ResponsesOutputZone::Message {
                    let output_index = next_output_index;
                    next_output_index += 1;
                    let mut item = stream_output_item_start_stub_from_node_header(
                        zone,
                        &header,
                        &extra_body,
                        &pending_envelope_extra,
                    );
                    item = maybe_reasoning_added_item_with_duration(
                        item,
                        stream_started_at.elapsed().as_secs(),
                    );
                    pending_envelope_extra.clear();
                    send_responses_event(
                        &tx,
                        &mut seq,
                        "response.output_item.added",
                        json!({
                            "output_index": output_index,
                            "item": item.clone(),
                        }),
                    )
                    .await?;
                    streamed_output_indices.insert(output_index);
                    let item_id = item
                        .get("id")
                        .and_then(Value::as_str)
                        .unwrap_or_default()
                        .to_string();
                    node_states.insert(
                        node_index,
                        StreamedNodeState {
                            output_index,
                            zone,
                            content_index: None,
                            item_id,
                            phase,
                            call_id: node_header_call_id(&header),
                            name: node_header_name(&header),
                            reasoning_summary_part_added_sent: false,
                            message_start_emitted: true,
                            header: Some(header.clone()),
                            node_extra_body: extra_body.clone(),
                            completed_item: Some(complete_stream_output_item(item)),
                            is_shared_message_output: false,
                            reasoning_started_at: (zone == ResponsesOutputZone::Reasoning)
                                .then_some(Instant::now()),
                        },
                    );
                    continue;
                }
                let active = active_node_message_output
                    .as_mut()
                    .expect("message node stream output exists");
                let output_index = active.output_index;
                let content_index = Some(active.next_content_index as u32);
                active.next_content_index += 1;
                let item_id = active.item_id.clone();
                let message_start_emitted = false;
                let completed_item = None;
                let is_shared_message_output = true;
                node_states.insert(
                    node_index,
                    StreamedNodeState {
                        output_index,
                        zone,
                        content_index,
                        item_id,
                        phase,
                        call_id: node_header_call_id(&header),
                        name: node_header_name(&header),
                        reasoning_summary_part_added_sent: false,
                        message_start_emitted,
                        header: Some(header.clone()),
                        node_extra_body: extra_body.clone(),
                        completed_item,
                        is_shared_message_output,
                        reasoning_started_at: None,
                    },
                );
            }
            UrpStreamEvent::NodeDelta {
                node_index,
                delta,
                extra_body,
                ..
            } => {
                if let urp::NodeDelta::Image { source } = &delta
                    && let Some(downstream_event) =
                        image_generation_call_downstream_event(&extra_body)
                {
                    send_responses_event(
                        &tx,
                        &mut seq,
                        &downstream_event,
                        image_generation_call_image_event_payload(source, &extra_body),
                    )
                    .await?;
                    continue;
                }
                if let urp::NodeDelta::ProviderItem { data } = &delta
                    && let Some(downstream_event) = image_generation_call_downstream_event(&extra_body)
                {
                    send_responses_event(
                        &tx,
                        &mut seq,
                        &downstream_event,
                        image_generation_call_event_payload(data, &extra_body),
                    )
                    .await?;
                    continue;
                }
                if !node_states.contains_key(&node_index)
                    && let Some((output_item, synthesized_state)) =
                        synthesize_node_state_from_delta(node_index, &delta, &extra_body)
                {
                    send_responses_event(
                        &tx,
                        &mut seq,
                        "response.output_item.added",
                        json!({
                            "output_index": synthesized_state.output_index,
                            "item": output_item,
                        }),
                    )
                    .await?;
                    streamed_output_indices.insert(synthesized_state.output_index);
                    node_states.insert(node_index, synthesized_state);
                }
                let Some(node_state) = node_states.get_mut(&node_index) else {
                    continue;
                };
                if node_state.zone == ResponsesOutputZone::Message {
                    ensure_node_message_start_emitted(
                        &tx,
                        &mut seq,
                        node_state,
                        &mut pending_envelope_extra,
                        &mut active_node_message_output,
                        &mut streamed_output_indices,
                        sse_max_frame_length,
                    )
                    .await?;
                }
                match delta {
                    urp::NodeDelta::Text { content } => {
                        append_node_delta_to_completed_item(
                            node_state,
                            &urp::NodeDelta::Text {
                                content: content.clone(),
                            },
                            None,
                        );
                        send_responses_delta_string(
                            &tx,
                            &mut seq,
                            "response.output_text.delta",
                            responses_text_delta_payload(
                                node_state.phase.as_deref(),
                                &json!({ "id": node_state.item_id }),
                                node_state.output_index as u64,
                                node_state.content_index.unwrap_or(0) as u64,
                            ),
                            "delta",
                            &content,
                            sse_max_frame_length,
                        )
                        .await?;
                    }
                    urp::NodeDelta::Refusal { content } => {
                        append_node_delta_to_completed_item(
                            node_state,
                            &urp::NodeDelta::Refusal {
                                content: content.clone(),
                            },
                            None,
                        );
                        send_responses_delta_string(
                            &tx,
                            &mut seq,
                            "response.output_text.delta",
                            json!({
                                "item_id": node_state.item_id,
                                "output_index": node_state.output_index,
                                "content_index": node_state.content_index.unwrap_or(0),
                                "logprobs": Value::Null,
                                "type": "refusal"
                            }),
                            "delta",
                            &content,
                            sse_max_frame_length,
                        )
                        .await?;
                    }
                    urp::NodeDelta::Reasoning {
                        content,
                        encrypted,
                        summary,
                        source,
                    } => {
                        append_node_delta_to_completed_item(
                            node_state,
                            &urp::NodeDelta::Reasoning {
                                content: content.clone(),
                                encrypted: encrypted.clone(),
                                summary: summary.clone(),
                                source: source.clone(),
                            },
                            None,
                        );
                        if let Some(summary) =
                            summary.as_deref().filter(|summary| !summary.is_empty())
                        {
                            if !node_state.reasoning_summary_part_added_sent {
                                node_state.reasoning_summary_part_added_sent = true;
                                reasoning_summary_added_indices.insert(node_state.output_index);
                                send_responses_event(
                                    &tx,
                                    &mut seq,
                                    "response.reasoning_summary_part.added",
                                    json!({
                                        "item_id": node_state.item_id,
                                        "output_index": node_state.output_index,
                                        "summary_index": 0,
                                        "part": { "type": "summary_text", "text": "" },
                                    }),
                                )
                                .await?;
                            }
                            reasoning_summary_delta_indices.insert(node_state.output_index);
                            send_responses_delta_string(
                                &tx,
                                &mut seq,
                                "response.reasoning_summary_text.delta",
                                insert_reasoning_source(
                                    json!({
                                        "item_id": node_state.item_id,
                                        "output_index": node_state.output_index,
                                        "summary_index": 0,
                                    }),
                                    source.as_deref(),
                                ),
                                "delta",
                                summary,
                                sse_max_frame_length,
                            )
                            .await?;
                        }
                        if let Some(content) =
                            content.as_deref().filter(|content| !content.is_empty())
                        {
                            reasoning_delta_indices.insert(node_state.output_index);
                            send_responses_delta_string(
                                &tx,
                                &mut seq,
                                "response.reasoning_text.delta",
                                insert_reasoning_source(
                                    json!({
                                        "item_id": node_state.item_id,
                                        "output_index": node_state.output_index,
                                        "content_index": 0,
                                    }),
                                    source.as_deref(),
                                ),
                                "delta",
                                content,
                                sse_max_frame_length,
                            )
                            .await?;
                        }
                    }
                    urp::NodeDelta::ToolCallArguments { arguments } => {
                        append_node_delta_to_completed_item(
                            node_state,
                            &urp::NodeDelta::ToolCallArguments {
                                arguments: arguments.clone(),
                            },
                            None,
                        );
                        send_responses_delta_string(
                            &tx,
                            &mut seq,
                            "response.function_call_arguments.delta",
                            json!({
                                "item_id": node_state.item_id,
                                "output_index": node_state.output_index,
                            }),
                            "delta",
                            &arguments,
                            sse_max_frame_length,
                        )
                        .await?;
                        function_args_delta_indices.insert(node_state.output_index);
                    }
                    urp::NodeDelta::ProviderItem { data } => {
                        append_node_delta_to_completed_item(
                            node_state,
                            &urp::NodeDelta::ProviderItem { data },
                            None,
                        );
                    }
                    urp::NodeDelta::Image { .. }
                    | urp::NodeDelta::Audio { .. }
                    | urp::NodeDelta::File { .. } => {}
                }
            }
            UrpStreamEvent::NodeDone {
                node_index, node, ..
            } => {
                if matches!(node, urp::Node::NextDownstreamEnvelopeExtra { .. }) {
                    continue;
                }
                if let urp::Node::ProviderItem {
                    origin_protocol, ..
                } = &node
                    && *origin_protocol != urp::ProviderProtocol::Responses
                {
                    continue;
                }
                let Some(mut node_state) = node_states.remove(&node_index) else {
                    continue;
                };
                if node_state.zone == ResponsesOutputZone::Message {
                    ensure_node_message_start_emitted(
                        &tx,
                        &mut seq,
                        &mut node_state,
                        &mut pending_envelope_extra,
                        &mut active_node_message_output,
                        &mut streamed_output_indices,
                        sse_max_frame_length,
                    )
                    .await?;
                }
                match &node {
                    urp::Node::Text { content, .. } => {
                        apply_node_done_to_stream_output_item_state(&mut node_state, &node);
                        let mut done_payload = responses_text_delta_payload(
                            node_state.phase.as_deref(),
                            &json!({ "id": node_state.item_id }),
                            node_state.output_index as u64,
                            node_state.content_index.unwrap_or(0) as u64,
                        );
                        if let Some(obj) = done_payload.as_object_mut() {
                            obj.insert("text".to_string(), json!(content));
                        }
                        send_responses_event(
                            &tx,
                            &mut seq,
                            "response.output_text.done",
                            done_payload,
                        )
                        .await?;
                    }
                    urp::Node::Reasoning {
                        content,
                        encrypted,
                        summary,
                        source,
                        extra_body,
                        ..
                    } => {
                        append_node_delta_to_completed_item(
                            &mut node_state,
                            &urp::NodeDelta::Reasoning {
                                content: content.clone(),
                                encrypted: encrypted.clone(),
                                summary: summary.clone(),
                                source: source.clone(),
                            },
                            Some(extra_body),
                        );
                        apply_node_done_to_stream_output_item_state(&mut node_state, &node);
                        if let Some(summary) =
                            summary.as_deref().filter(|summary| !summary.is_empty())
                        {
                            if reasoning_summary_text_done_indices.insert(node_state.output_index) {
                                send_responses_event(
                                    &tx,
                                    &mut seq,
                                    "response.reasoning_summary_text.done",
                                    insert_reasoning_source(
                                        json!({
                                            "item_id": node_state.item_id,
                                            "output_index": node_state.output_index,
                                            "summary_index": 0,
                                            "text": summary,
                                        }),
                                        source.as_deref(),
                                    ),
                                )
                                .await?;
                            }
                            if reasoning_summary_part_done_indices.insert(node_state.output_index) {
                                send_responses_event(
                                    &tx,
                                    &mut seq,
                                    "response.reasoning_summary_part.done",
                                    json!({
                                        "item_id": node_state.item_id,
                                        "output_index": node_state.output_index,
                                        "summary_index": 0,
                                        "part": { "type": "summary_text", "text": summary },
                                    }),
                                )
                                .await?;
                            }
                        }
                        if let Some(content) =
                            content.as_deref().filter(|content| !content.is_empty())
                        {
                            if reasoning_done_indices.insert(node_state.output_index) {
                                send_responses_event(
                                    &tx,
                                    &mut seq,
                                    "response.reasoning_text.done",
                                    insert_reasoning_source(
                                        json!({
                                            "item_id": node_state.item_id,
                                            "output_index": node_state.output_index,
                                            "content_index": 0,
                                            "text": content,
                                        }),
                                        source.as_deref(),
                                    ),
                                )
                                .await?;
                            }
                        }
                    }
                    urp::Node::ToolCall {
                        arguments,
                        extra_body,
                        ..
                    } => {
                        append_node_delta_to_completed_item(
                            &mut node_state,
                            &urp::NodeDelta::ToolCallArguments {
                                arguments: arguments.clone(),
                            },
                            Some(extra_body),
                        );
                        apply_node_done_to_stream_output_item_state(&mut node_state, &node);
                        if function_args_done_indices.insert(node_state.output_index) {
                            send_responses_event(
                                &tx,
                                &mut seq,
                                "response.function_call_arguments.done",
                                json!({
                                    "arguments": arguments,
                                    "call_id": node_state.call_id.clone().unwrap_or_default(),
                                    "item_id": node_state.item_id,
                                    "name": node_state.name.clone().unwrap_or_default(),
                                    "output_index": node_state.output_index,
                                }),
                            )
                            .await?;
                        }
                    }
                    _ => {
                        apply_node_done_to_stream_output_item_state(&mut node_state, &node);
                    }
                }
                if node_state.zone == ResponsesOutputZone::Message {
                    send_responses_event(
                        &tx,
                        &mut seq,
                        "response.content_part.done",
                        json!({
                            "output_index": node_state.output_index,
                            "content_index": node_state.content_index.unwrap_or(0),
                            "item_id": node_state.item_id,
                            "part": encode_node_done_content_part(&node),
                        }),
                    )
                    .await?;
                }
                if node_state.is_shared_message_output
                    && let Some(active) = active_node_message_output.as_mut()
                    && active.output_index == node_state.output_index
                {
                    if active.item_id.is_empty() {
                        active.item_id = node_state.item_id.clone();
                    }
                    apply_node_done_to_stream_output_item(active, &node);
                }
                let completed_item = if node_state.is_shared_message_output {
                    sanitize_responses_output_item_for_frame_limit(
                        &active_node_message_output
                            .as_ref()
                            .filter(|active| active.output_index == node_state.output_index)
                            .map(|active| complete_stream_output_item(active.item.clone()))
                            .unwrap_or_else(|| {
                                node_state.completed_item.clone().unwrap_or_else(|| {
                                    complete_stream_output_item(
                                        encode_stream_output_item_from_node(&node),
                                    )
                                })
                            }),
                        sse_max_frame_length,
                    )
                } else {
                    let item = node_state.completed_item.take().unwrap_or_else(|| {
                        complete_stream_output_item(encode_stream_output_item_from_node(&node))
                    });
                    sanitize_responses_output_item_for_frame_limit(
                        &reasoning_item_with_duration(
                            item,
                            reasoning_duration_secs(&node_state, stream_started_at),
                        ),
                        sse_max_frame_length,
                    )
                };
                if completed_output_indices.insert(node_state.output_index) {
                    send_responses_event(
                        &tx,
                        &mut seq,
                        "response.output_item.done",
                        json!({
                            "output_index": node_state.output_index,
                            "item": completed_item.clone(),
                        }),
                    )
                    .await?;
                }
                let should_record_terminal_item = !completed_output_items
                    .iter()
                    .any(|(idx, _)| *idx == node_state.output_index);
                if should_record_terminal_item {
                    completed_output_items.push((node_state.output_index, completed_item));
                }
            }
            UrpStreamEvent::ResponseDone {
                finish_reason,
                usage,
                output,
                extra_body,
            } => {
                let mut remaining_node_indices: Vec<u32> = node_states.keys().copied().collect();
                remaining_node_indices.sort_unstable();
                let mut used_terminal_output_positions = HashSet::new();
                for node_index in remaining_node_indices {
                    let Some(mut node_state) = node_states.remove(&node_index) else {
                        continue;
                    };
                    let node = if let Some((matched_output_position, node)) =
                        find_terminal_output_node_for_state(
                            &output,
                            node_index as usize,
                            &node_state,
                            &used_terminal_output_positions,
                        ) {
                        used_terminal_output_positions.insert(matched_output_position);
                        node
                    } else if let Some(node) = synthesize_terminal_node_from_state(&node_state) {
                        node
                    } else {
                        continue;
                    };
                    if node_state.zone == ResponsesOutputZone::Message {
                        ensure_node_message_start_emitted(
                            &tx,
                            &mut seq,
                            &mut node_state,
                            &mut pending_envelope_extra,
                            &mut active_node_message_output,
                            &mut streamed_output_indices,
                            sse_max_frame_length,
                        )
                        .await?;
                    }
                    match &node {
                        urp::Node::Text { content, .. } => {
                            append_node_delta_to_completed_item(
                                &mut node_state,
                                &urp::NodeDelta::Text {
                                    content: content.clone(),
                                },
                                None,
                            );
                            let mut done_payload = responses_text_delta_payload(
                                node_state.phase.as_deref(),
                                &json!({ "id": node_state.item_id }),
                                node_state.output_index as u64,
                                node_state.content_index.unwrap_or(0) as u64,
                            );
                            if let Some(obj) = done_payload.as_object_mut() {
                                obj.insert("text".to_string(), json!(content));
                            }
                            send_responses_event(
                                &tx,
                                &mut seq,
                                "response.output_text.done",
                                done_payload,
                            )
                            .await?;
                        }
                        urp::Node::Refusal { content, .. } => {
                            append_node_delta_to_completed_item(
                                &mut node_state,
                                &urp::NodeDelta::Refusal {
                                    content: content.clone(),
                                },
                                None,
                            );
                        }
                        urp::Node::Reasoning {
                            content,
                            encrypted,
                            summary,
                            source,
                            extra_body,
                            ..
                        } => {
                            append_node_delta_to_completed_item(
                                &mut node_state,
                                &urp::NodeDelta::Reasoning {
                                    content: content.clone(),
                                    encrypted: encrypted.clone(),
                                    summary: summary.clone(),
                                    source: source.clone(),
                                },
                                Some(extra_body),
                            );
                            if let Some(summary) =
                                summary.as_deref().filter(|summary| !summary.is_empty())
                            {
                                if !node_state.reasoning_summary_part_added_sent {
                                    node_state.reasoning_summary_part_added_sent = true;
                                    reasoning_summary_added_indices.insert(node_state.output_index);
                                    send_responses_event(
                                        &tx,
                                        &mut seq,
                                        "response.reasoning_summary_part.added",
                                        json!({
                                            "item_id": node_state.item_id,
                                            "output_index": node_state.output_index,
                                            "summary_index": 0,
                                            "part": { "type": "summary_text", "text": "" },
                                        }),
                                    )
                                    .await?;
                                }
                                if reasoning_summary_delta_indices.insert(node_state.output_index) {
                                    send_responses_delta_string(
                                        &tx,
                                        &mut seq,
                                        "response.reasoning_summary_text.delta",
                                        insert_reasoning_source(
                                            json!({
                                                "item_id": node_state.item_id,
                                                "output_index": node_state.output_index,
                                                "summary_index": 0,
                                            }),
                                            source.as_deref(),
                                        ),
                                        "delta",
                                        summary,
                                        sse_max_frame_length,
                                    )
                                    .await?;
                                }
                                if reasoning_summary_text_done_indices
                                    .insert(node_state.output_index)
                                {
                                    send_responses_event(
                                        &tx,
                                        &mut seq,
                                        "response.reasoning_summary_text.done",
                                        insert_reasoning_source(
                                            json!({
                                                "item_id": node_state.item_id,
                                                "output_index": node_state.output_index,
                                                "summary_index": 0,
                                                "text": summary,
                                            }),
                                            source.as_deref(),
                                        ),
                                    )
                                    .await?;
                                }
                                if reasoning_summary_part_done_indices
                                    .insert(node_state.output_index)
                                {
                                    send_responses_event(
                                        &tx,
                                        &mut seq,
                                        "response.reasoning_summary_part.done",
                                        json!({
                                            "item_id": node_state.item_id,
                                            "output_index": node_state.output_index,
                                            "summary_index": 0,
                                            "part": { "type": "summary_text", "text": summary },
                                        }),
                                    )
                                    .await?;
                                }
                            }
                            if let Some(content) =
                                content.as_deref().filter(|content| !content.is_empty())
                            {
                                if reasoning_delta_indices.insert(node_state.output_index) {
                                    send_responses_delta_string(
                                        &tx,
                                        &mut seq,
                                        "response.reasoning_text.delta",
                                        insert_reasoning_source(
                                            json!({
                                                "item_id": node_state.item_id,
                                                "output_index": node_state.output_index,
                                                "content_index": 0,
                                            }),
                                            source.as_deref(),
                                        ),
                                        "delta",
                                        content,
                                        sse_max_frame_length,
                                    )
                                    .await?;
                                }
                                if reasoning_done_indices.insert(node_state.output_index) {
                                    send_responses_event(
                                        &tx,
                                        &mut seq,
                                        "response.reasoning_text.done",
                                        insert_reasoning_source(
                                            json!({
                                                "item_id": node_state.item_id,
                                                "output_index": node_state.output_index,
                                                "content_index": 0,
                                                "text": content,
                                            }),
                                            source.as_deref(),
                                        ),
                                    )
                                    .await?;
                                }
                            }
                        }
                        urp::Node::ToolCall {
                            arguments,
                            extra_body,
                            ..
                        } => {
                            append_node_delta_to_completed_item(
                                &mut node_state,
                                &urp::NodeDelta::ToolCallArguments {
                                    arguments: arguments.clone(),
                                },
                                Some(extra_body),
                            );
                            if function_args_delta_indices.insert(node_state.output_index) {
                                send_responses_delta_string(
                                    &tx,
                                    &mut seq,
                                    "response.function_call_arguments.delta",
                                    json!({
                                        "item_id": node_state.item_id,
                                        "output_index": node_state.output_index,
                                    }),
                                    "delta",
                                    arguments,
                                    sse_max_frame_length,
                                )
                                .await?;
                            }
                            if function_args_done_indices.insert(node_state.output_index) {
                                send_responses_event(
                                    &tx,
                                    &mut seq,
                                    "response.function_call_arguments.done",
                                    json!({
                                        "arguments": arguments,
                                        "call_id": node_state.call_id.clone().unwrap_or_default(),
                                        "item_id": node_state.item_id,
                                        "name": node_state.name.clone().unwrap_or_default(),
                                        "output_index": node_state.output_index,
                                    }),
                                )
                                .await?;
                            }
                        }
                        _ => {}
                    }
                    apply_node_done_to_stream_output_item_state(&mut node_state, &node);
                    if node_state.zone == ResponsesOutputZone::Message {
                        send_responses_event(
                            &tx,
                            &mut seq,
                            "response.content_part.done",
                            json!({
                                "output_index": node_state.output_index,
                                "content_index": node_state.content_index.unwrap_or(0),
                                "item_id": node_state.item_id,
                                "part": encode_node_done_content_part(&node),
                            }),
                        )
                        .await?;
                    }
                    if node_state.is_shared_message_output
                        && let Some(active) = active_node_message_output.as_mut()
                        && active.output_index == node_state.output_index
                    {
                        if active.item_id.is_empty() {
                            active.item_id = node_state.item_id.clone();
                        }
                        apply_node_done_to_stream_output_item(active, &node);
                    }
                    let completed_item = if node_state.is_shared_message_output {
                        sanitize_responses_output_item_for_frame_limit(
                            &active_node_message_output
                                .as_ref()
                                .filter(|active| active.output_index == node_state.output_index)
                                .map(|active| complete_stream_output_item(active.item.clone()))
                                .unwrap_or_else(|| {
                                    node_state.completed_item.clone().unwrap_or_else(|| {
                                        complete_stream_output_item(
                                            encode_stream_output_item_from_node(&node),
                                        )
                                    })
                                }),
                            sse_max_frame_length,
                        )
                    } else {
                        let item = node_state.completed_item.take().unwrap_or_else(|| {
                            complete_stream_output_item(encode_stream_output_item_from_node(&node))
                        });
                        sanitize_responses_output_item_for_frame_limit(
                            &reasoning_item_with_duration(
                                item,
                                reasoning_duration_secs(&node_state, stream_started_at),
                            ),
                            sse_max_frame_length,
                        )
                    };
                    if completed_output_indices.insert(node_state.output_index) {
                        send_responses_event(
                            &tx,
                            &mut seq,
                            "response.output_item.done",
                            json!({
                                "output_index": node_state.output_index,
                                "item": completed_item.clone(),
                            }),
                        )
                        .await?;
                    }
                    if !completed_output_items
                        .iter()
                        .any(|(idx, _)| *idx == node_state.output_index)
                    {
                        completed_output_items.push((node_state.output_index, completed_item));
                    }
                }
                let mut response = urp::encode::openai_responses::encode_response(
                    &urp::UrpResponse {
                        id: response_id.clone(),
                        model: logical_model.to_string(),
                        created_at: created,
                        output,
                        finish_reason,
                        usage,
                        extra_body,
                    },
                    logical_model,
                );
                if let Some(created) = created {
                    response["created_at"] = json!(created);
                }
                let mut terminal_output = response
                    .get("output")
                    .and_then(Value::as_array)
                    .cloned()
                    .unwrap_or_default();
                let shared_terminal_extra =
                    active_node_message_output.as_ref().and_then(|active| {
                        (!active.envelope_extra.is_empty()).then_some(active.envelope_extra.clone())
                    });
                if let Some(active) = active_node_message_output.take()
                    && active.item.is_object()
                    && completed_output_indices.insert(active.output_index)
                {
                    let done_item = sanitize_responses_output_item_for_frame_limit(
                        &complete_stream_output_item(active.item.clone()),
                        sse_max_frame_length,
                    );
                    send_responses_event(
                        &tx,
                        &mut seq,
                        "response.output_item.done",
                        json!({
                            "output_index": active.output_index,
                            "item": done_item,
                        }),
                    )
                    .await?;
                    completed_output_items.push((active.output_index, done_item));
                }
                let mut rebuild_output_items = completed_output_items.clone();
                for (output_index, item) in terminal_output.iter().enumerate() {
                    if let Some((_, existing_item)) = rebuild_output_items
                        .iter_mut()
                        .find(|(existing_index, _)| *existing_index == output_index)
                    {
                        merge_terminal_output_item(existing_item, item);
                    } else if let Some((_, existing_item)) = rebuild_output_items
                        .iter_mut()
                        .find(|(_, existing_item)| {
                            responses_output_items_semantically_match(existing_item, item)
                        })
                    {
                        merge_terminal_output_item(existing_item, item);
                    } else {
                        rebuild_output_items.push((output_index, item.clone()));
                    }
                }
                if !completed_output_items.is_empty() && !streamed_output_indices.is_empty() {
                    rebuild_output_items.sort_by_key(|(idx, _)| *idx);
                    response["output"] =
                        Value::Array(rebuild_completed_response_output(&rebuild_output_items));
                } else if terminal_output.is_empty() {
                    if !completed_output_items.is_empty() {
                        completed_output_items.sort_by_key(|(idx, _)| *idx);
                        response["output"] = Value::Array(rebuild_completed_response_output(
                            &completed_output_items,
                        ));
                    }
                } else {
                    if let Some(extra) = shared_terminal_extra.as_ref()
                        && let Some(first_message) = terminal_output.iter_mut().find(|item| {
                            item.get("type").and_then(Value::as_str) == Some("message")
                        })
                        && let Some(obj) = first_message.as_object_mut()
                    {
                        merge_json_extra_preserving_typed(obj, extra);
                    }
                    response["output"] = Value::Array(terminal_output);
                }
                terminal_output = response
                    .get("output")
                    .and_then(Value::as_array)
                    .cloned()
                    .unwrap_or_default();
                sync_completed_output_items_with_terminal_output(
                    &mut completed_output_items,
                    &terminal_output,
                );
                emit_missing_terminal_output_done_events(
                    &tx,
                    &mut seq,
                    &mut completed_output_indices,
                    &mut streamed_output_indices,
                    &mut completed_output_items,
                    &terminal_output,
                    &mut reasoning_delta_indices,
                    &mut reasoning_done_indices,
                    &mut reasoning_summary_added_indices,
                    &mut reasoning_summary_delta_indices,
                    &mut reasoning_summary_text_done_indices,
                    &mut reasoning_summary_part_done_indices,
                    &mut function_args_delta_indices,
                    &mut function_args_done_indices,
                    stream_started_at.elapsed().as_secs(),
                    sse_max_frame_length,
                )
                .await?;
                if !completed_output_items.is_empty() {
                    completed_output_items.sort_by_key(|(idx, _)| *idx);
                    response["output"] =
                        Value::Array(rebuild_completed_response_output(&completed_output_items));
                }
                emit_missing_terminal_sub_lifecycles(
                    &tx,
                    &mut seq,
                    &completed_output_items,
                    &mut reasoning_delta_indices,
                    &mut reasoning_done_indices,
                    &mut reasoning_summary_added_indices,
                    &mut reasoning_summary_delta_indices,
                    &mut reasoning_summary_text_done_indices,
                    &mut reasoning_summary_part_done_indices,
                    &mut function_args_delta_indices,
                    &mut function_args_done_indices,
                    sse_max_frame_length,
                )
                .await?;
                let mut completed_response = ensure_response_object_user_field(
                    sanitize_responses_completed_for_frame_limit(&response, sse_max_frame_length),
                );
                completed_response["completed_at"] = json!(now_ts());
                send_responses_event(
                    &tx,
                    &mut seq,
                    "response.completed",
                    json!({ "response": completed_response }),
                )
                .await?;
                send_plain_sse_data(&tx, "[DONE]".to_string()).await?;
            }
            UrpStreamEvent::ProviderControl { .. } => {}
            UrpStreamEvent::Error {
                code,
                message,
                extra_body,
            } => {
                let created_at = created.unwrap_or_else(now_ts);
                let failed_response = response_failed_payload(
                    &response_id,
                    created_at,
                    logical_model,
                    code.as_deref(),
                    &message,
                    &extra_body,
                );
                send_responses_event(
                    &tx,
                    &mut seq,
                    "response.failed",
                    json!({ "response": failed_response }),
                )
                .await?;
                send_plain_sse_data(&tx, "[DONE]".to_string()).await?;
                error_terminal_sent = true;
            }
        }
    }

    Ok(())
}

fn zone_from_node_header(header: &urp::NodeHeader) -> ResponsesOutputZone {
    match header {
        urp::NodeHeader::Reasoning { .. } => ResponsesOutputZone::Reasoning,
        urp::NodeHeader::ToolCall { .. } => ResponsesOutputZone::FunctionCall,
        urp::NodeHeader::ProviderItem { .. } => ResponsesOutputZone::ProviderItem,
        _ => ResponsesOutputZone::Message,
    }
}

fn node_header_id(header: &urp::NodeHeader) -> Option<String> {
    match header {
        urp::NodeHeader::Text { id, .. }
        | urp::NodeHeader::Image { id, .. }
        | urp::NodeHeader::Audio { id, .. }
        | urp::NodeHeader::File { id, .. }
        | urp::NodeHeader::Refusal { id }
        | urp::NodeHeader::Reasoning { id }
        | urp::NodeHeader::ToolCall { id, .. }
        | urp::NodeHeader::ProviderItem { id, .. }
        | urp::NodeHeader::ToolResult { id, .. } => id.clone(),
        urp::NodeHeader::NextDownstreamEnvelopeExtra => None,
    }
}

fn node_header_phase(header: &urp::NodeHeader) -> Option<String> {
    match header {
        urp::NodeHeader::Text { phase, .. } => phase.clone(),
        _ => None,
    }
}

fn node_header_call_id(header: &urp::NodeHeader) -> Option<String> {
    match header {
        urp::NodeHeader::ToolCall { call_id, .. } | urp::NodeHeader::ToolResult { call_id, .. } => {
            Some(call_id.clone())
        }
        _ => None,
    }
}

fn node_header_name(header: &urp::NodeHeader) -> Option<String> {
    match header {
        urp::NodeHeader::ToolCall { name, .. } => Some(name.clone()),
        _ => None,
    }
}

fn synthesize_node_state_from_delta(
    _node_index: u32,
    _delta: &urp::NodeDelta,
    _extra_body: &HashMap<String, Value>,
) -> Option<(Value, StreamedNodeState)> {
    None
}

fn image_generation_call_downstream_event(extra_body: &HashMap<String, Value>) -> Option<String> {
    let event_type = extra_body
        .get("provider_event_type")
        .or_else(|| extra_body.get("type"))
        .and_then(Value::as_str)?;
    match event_type {
        "image_generation.partial_image" | "response.image_generation.partial_image" => {
            Some("response.image_generation_call.partial_image".to_string())
        }
        _ if event_type.starts_with("response.image_generation_call.") => {
            Some(event_type.to_string())
        }
        _ => None,
    }
}

fn image_generation_call_event_payload(
    data: &Value,
    extra_body: &HashMap<String, Value>,
) -> Value {
    let mut payload = match data {
        Value::Object(map) => map.clone(),
        Value::Null => Map::new(),
        other => {
            let mut map = Map::new();
            map.insert("data".to_string(), other.clone());
            map
        }
    };
    for (key, value) in extra_body {
        if key != "type" && key != "provider_event_type" && key != "sequence_number" {
            payload.entry(key.clone()).or_insert(value.clone());
        }
    }
    Value::Object(payload)
}

fn image_generation_call_image_event_payload(
    source: &urp::ImageSource,
    extra_body: &HashMap<String, Value>,
) -> Value {
    let mut payload = Map::new();
    for (key, value) in extra_body {
        if key != "type" && key != "provider_event_type" && key != "sequence_number" {
            payload.insert(key.clone(), value.clone());
        }
    }
    if !payload.contains_key("partial_image_b64") {
        if let urp::ImageSource::Base64 { data, .. } = source {
            payload.insert("partial_image_b64".to_string(), Value::String(data.clone()));
        }
    }
    Value::Object(payload)
}

fn ordinary_role_to_str(role: urp::OrdinaryRole) -> &'static str {
    match role {
        urp::OrdinaryRole::System => "system",
        urp::OrdinaryRole::Developer => "developer",
        urp::OrdinaryRole::User => "user",
        urp::OrdinaryRole::Assistant => "assistant",
    }
}

fn stream_output_item_start_stub_from_node_header(
    zone: ResponsesOutputZone,
    header: &urp::NodeHeader,
    extra_body: &HashMap<String, Value>,
    envelope_extra: &HashMap<String, Value>,
) -> Value {
    match zone {
        ResponsesOutputZone::Message => {
            let role = match header {
                urp::NodeHeader::Text { role, .. }
                | urp::NodeHeader::Image { role, .. }
                | urp::NodeHeader::Audio { role, .. }
                | urp::NodeHeader::File { role, .. }
                | urp::NodeHeader::ProviderItem { role, .. } => ordinary_role_to_str(*role),
                urp::NodeHeader::Refusal { .. } => "assistant",
                _ => "assistant",
            };
            let mut obj = Map::new();
            obj.insert("type".to_string(), json!("message"));
            obj.insert("role".to_string(), json!(role));
            obj.insert("content".to_string(), json!([]));
            let id = node_header_id(header)
                .or_else(|| {
                    extra_body
                        .get("id")
                        .and_then(Value::as_str)
                        .map(|s| s.to_string())
                })
                .or_else(|| {
                    envelope_extra
                        .get("id")
                        .and_then(Value::as_str)
                        .map(|s| s.to_string())
                })
                .unwrap_or_else(|| format!("msg_{}", uuid::Uuid::new_v4()));
            obj.insert("id".to_string(), json!(id));
            obj.insert("status".to_string(), json!("in_progress"));
            if let Some(phase) = node_header_phase(header) {
                obj.insert("phase".to_string(), json!(phase));
            }
            merge_json_extra_preserving_typed(&mut obj, envelope_extra);
            merge_json_extra(&mut obj, extra_body);
            Value::Object(obj)
        }
        ResponsesOutputZone::Reasoning => {
            let mut obj = Map::new();
            obj.insert("type".to_string(), json!("reasoning"));
            let id = node_header_id(header)
                .or_else(|| {
                    extra_body
                        .get("id")
                        .and_then(Value::as_str)
                        .map(str::to_string)
                })
                .or_else(|| {
                    envelope_extra
                        .get("id")
                        .and_then(Value::as_str)
                        .map(str::to_string)
                })
                .unwrap_or_else(|| format!("rs_{}", uuid::Uuid::new_v4()));
            obj.insert("id".to_string(), json!(id));
            obj.insert("status".to_string(), json!("in_progress"));
            obj.insert(
                "started_at".to_string(),
                json!(chrono::Utc::now().timestamp()),
            );
            merge_json_extra_preserving_typed(&mut obj, envelope_extra);
            merge_json_extra(&mut obj, extra_body);
            Value::Object(obj)
        }
        ResponsesOutputZone::FunctionCall => {
            let (call_id, name) = match header {
                urp::NodeHeader::ToolCall { call_id, name, .. } => (call_id.clone(), name.clone()),
                _ => (String::new(), String::new()),
            };
            let mut obj = Map::new();
            obj.insert("type".to_string(), json!("function_call"));
            obj.insert("call_id".to_string(), json!(call_id));
            obj.insert("name".to_string(), json!(name));
            obj.insert("arguments".to_string(), json!(""));
            let id = node_header_id(header)
                .or_else(|| {
                    extra_body
                        .get("id")
                        .and_then(Value::as_str)
                        .map(|s| s.to_string())
                })
                .or_else(|| {
                    envelope_extra
                        .get("id")
                        .and_then(Value::as_str)
                        .map(|s| s.to_string())
                })
                .unwrap_or_else(|| format!("fc_{}", uuid::Uuid::new_v4()));
            obj.insert("id".to_string(), json!(id));
            obj.insert("status".to_string(), json!("in_progress"));
            merge_json_extra_preserving_typed(&mut obj, envelope_extra);
            merge_json_extra(&mut obj, extra_body);
            Value::Object(obj)
        }
        ResponsesOutputZone::ProviderItem => {
            let urp::NodeHeader::ProviderItem { id, item_type, .. } = header else {
                return Value::Null;
            };
            let mut obj = Map::new();
            obj.insert("type".to_string(), json!(item_type));
            if let Some(id) = id
                .clone()
                .or_else(|| extra_body.get("id").and_then(Value::as_str).map(str::to_string))
                .or_else(|| {
                    envelope_extra
                        .get("id")
                        .and_then(Value::as_str)
                        .map(str::to_string)
                })
            {
                obj.insert("id".to_string(), json!(id));
            }
            merge_json_extra_preserving_typed(&mut obj, envelope_extra);
            merge_json_extra(&mut obj, extra_body);
            Value::Object(obj)
        }
    }
}

fn encode_node_start_content_part(header: &urp::NodeHeader) -> Value {
    match header {
        urp::NodeHeader::Text { .. } => {
            json!({ "type": "output_text", "text": "", "annotations": [], "logprobs": [] })
        }
        urp::NodeHeader::Refusal { .. } => json!({ "type": "refusal", "refusal": "" }),
        urp::NodeHeader::Image { .. } => json!({ "type": "output_image" }),
        urp::NodeHeader::Audio { .. } => json!({ "type": "audio" }),
        urp::NodeHeader::File { .. } => json!({ "type": "output_file" }),
        urp::NodeHeader::ProviderItem { .. } => Value::Null,
        _ => Value::Null,
    }
}

fn encode_node_done_content_part(node: &urp::Node) -> Value {
    match node {
        urp::Node::Text {
            content,
            extra_body,
            ..
        } => {
            let mut obj = Map::new();
            obj.insert("type".to_string(), json!("output_text"));
            obj.insert("text".to_string(), json!(content));
            obj.insert("annotations".to_string(), json!([]));
            obj.insert("logprobs".to_string(), json!([]));
            merge_json_extra(&mut obj, extra_body);
            Value::Object(obj)
        }
        urp::Node::Refusal {
            content,
            extra_body,
            ..
        } => {
            let mut obj = Map::new();
            obj.insert("type".to_string(), json!("refusal"));
            obj.insert("refusal".to_string(), json!(content));
            merge_json_extra(&mut obj, extra_body);
            Value::Object(obj)
        }
        urp::Node::Image {
            source, extra_body, ..
        } => encode_image_part(source, extra_body),
        urp::Node::Audio {
            source, extra_body, ..
        } => {
            let mut obj = Map::new();
            obj.insert("type".to_string(), json!("audio"));
            obj.insert("source".to_string(), encode_audio_source(source));
            merge_json_extra(&mut obj, extra_body);
            Value::Object(obj)
        }
        urp::Node::File {
            source, extra_body, ..
        } => encode_file_part(source, extra_body),
        urp::Node::ProviderItem {
            origin_protocol,
            item_type,
            body,
            extra_body,
            ..
        } => {
            if *origin_protocol != urp::ProviderProtocol::Responses {
                return Value::Null;
            }
            let mut obj = match body {
                Value::Object(map) => map.clone(),
                other => {
                    let mut map = Map::new();
                    map.insert("body".to_string(), other.clone());
                    map
                }
            };
            obj.entry("type".to_string())
                .or_insert_with(|| Value::String(item_type.clone()));
            merge_json_extra(&mut obj, extra_body);
            Value::Object(obj)
        }
        _ => Value::Null,
    }
}

fn encode_responses_provider_output_item(
    item_type: &str,
    body: &Value,
    extra_body: &HashMap<String, Value>,
    id: Option<&String>,
) -> Value {
    let mut obj = match body {
        Value::Object(map) => map.clone(),
        other => {
            let mut map = Map::new();
            map.insert("body".to_string(), other.clone());
            map
        }
    };
    obj.entry("type".to_string())
        .or_insert_with(|| Value::String(item_type.to_string()));
    if let Some(id) = id.filter(|id| !id.is_empty()) {
        obj.entry("id".to_string())
            .or_insert_with(|| Value::String(id.clone()));
    }
    merge_json_extra(&mut obj, extra_body);
    Value::Object(obj)
}

fn encode_stream_output_item_from_node(node: &urp::Node) -> Value {
    match node {
        urp::Node::Text {
            role,
            content,
            phase,
            extra_body,
            ..
        } => {
            let mut obj = Map::new();
            obj.insert("type".to_string(), json!("message"));
            obj.insert("role".to_string(), json!(ordinary_role_to_str(*role)));
            obj.insert("content".to_string(), json!([{ "type": "output_text", "text": content, "annotations": [], "logprobs": [] }]));
            let id = extra_body
                .get("id")
                .and_then(Value::as_str)
                .map(|s| s.to_string())
                .or_else(|| node.id().cloned())
                .unwrap_or_else(|| format!("msg_{}", uuid::Uuid::new_v4()));
            obj.insert("id".to_string(), json!(id));
            obj.insert("status".to_string(), json!("completed"));
            if let Some(phase) = phase {
                obj.insert("phase".to_string(), json!(phase));
            }
            merge_json_extra(&mut obj, extra_body);
            Value::Object(obj)
        }
        urp::Node::Refusal {
            content,
            extra_body,
            ..
        } => {
            let mut obj = Map::new();
            obj.insert("type".to_string(), json!("message"));
            obj.insert("role".to_string(), json!("assistant"));
            obj.insert(
                "content".to_string(),
                json!([{ "type": "refusal", "refusal": content }]),
            );
            let id = extra_body
                .get("id")
                .and_then(Value::as_str)
                .map(|s| s.to_string())
                .or_else(|| node.id().cloned())
                .unwrap_or_else(|| format!("msg_{}", uuid::Uuid::new_v4()));
            obj.insert("id".to_string(), json!(id));
            obj.insert("status".to_string(), json!("completed"));
            merge_json_extra(&mut obj, extra_body);
            Value::Object(obj)
        }
        urp::Node::Reasoning {
            id,
            content,
            encrypted,
            summary,
            source,
            extra_body,
        } => {
            let mut obj = Map::new();
            obj.insert("type".to_string(), json!("reasoning"));
            obj.insert(
                "id".to_string(),
                json!(
                    id.clone()
                        .or_else(|| {
                            extra_body
                                .get("id")
                                .and_then(Value::as_str)
                                .map(|s| s.to_string())
                        })
                        .unwrap_or_else(|| format!("rs_{}", uuid::Uuid::new_v4()))
                ),
            );
            if let Some(text) = summary.as_ref().filter(|text| !text.is_empty()) {
                obj.insert(
                    "summary".to_string(),
                    Value::Array(vec![json!({ "type": "summary_text", "text": text })]),
                );
            }
            if let Some(text) = content.as_ref().filter(|text| !text.is_empty()) {
                obj.insert("text".to_string(), json!(text));
            }
            if let Some(encrypted) = encrypted.as_ref().filter(|encrypted| !encrypted.is_null()) {
                obj.insert("encrypted_content".to_string(), encrypted.clone());
            }
            if let Some(source) = source.as_ref().filter(|source| !source.is_empty()) {
                obj.insert("source".to_string(), json!(source));
            }
            obj.insert("status".to_string(), json!("completed"));
            merge_json_extra(&mut obj, extra_body);
            Value::Object(obj)
        }
        urp::Node::ToolCall {
            id,
            call_id,
            name,
            arguments,
            extra_body,
        } => {
            let mut obj = Map::new();
            obj.insert("type".to_string(), json!("function_call"));
            obj.insert("call_id".to_string(), json!(call_id));
            obj.insert("name".to_string(), json!(name));
            obj.insert("arguments".to_string(), json!(arguments));
            obj.insert(
                "id".to_string(),
                json!(
                    id.clone()
                        .or_else(|| {
                            extra_body
                                .get("id")
                                .and_then(Value::as_str)
                                .map(|s| s.to_string())
                        })
                        .unwrap_or_else(|| format!("fc_{}", uuid::Uuid::new_v4()))
                ),
            );
            obj.insert("status".to_string(), json!("completed"));
            merge_json_extra(&mut obj, extra_body);
            Value::Object(obj)
        }
        urp::Node::Image {
            role,
            source,
            extra_body,
            ..
        } => {
            let mut obj = Map::new();
            obj.insert("type".to_string(), json!("message"));
            obj.insert("role".to_string(), json!(ordinary_role_to_str(*role)));
            obj.insert(
                "content".to_string(),
                json!([encode_image_part(source, extra_body)]),
            );
            let id = extra_body
                .get("id")
                .and_then(Value::as_str)
                .map(|s| s.to_string())
                .or_else(|| node.id().cloned())
                .unwrap_or_else(|| format!("msg_{}", uuid::Uuid::new_v4()));
            obj.insert("id".to_string(), json!(id));
            obj.insert("status".to_string(), json!("completed"));
            merge_json_extra(&mut obj, extra_body);
            Value::Object(obj)
        }
        urp::Node::Audio {
            role,
            source,
            extra_body,
            ..
        } => {
            let mut obj = Map::new();
            obj.insert("type".to_string(), json!("message"));
            obj.insert("role".to_string(), json!(ordinary_role_to_str(*role)));
            obj.insert(
                "content".to_string(),
                json!([{ "type": "audio", "source": encode_audio_source(source) }]),
            );
            let id = extra_body
                .get("id")
                .and_then(Value::as_str)
                .map(|s| s.to_string())
                .or_else(|| node.id().cloned())
                .unwrap_or_else(|| format!("msg_{}", uuid::Uuid::new_v4()));
            obj.insert("id".to_string(), json!(id));
            obj.insert("status".to_string(), json!("completed"));
            merge_json_extra(&mut obj, extra_body);
            Value::Object(obj)
        }
        urp::Node::File {
            role,
            source,
            extra_body,
            ..
        } => {
            let mut obj = Map::new();
            obj.insert("type".to_string(), json!("message"));
            obj.insert("role".to_string(), json!(ordinary_role_to_str(*role)));
            obj.insert(
                "content".to_string(),
                json!([encode_file_part(source, extra_body)]),
            );
            let id = extra_body
                .get("id")
                .and_then(Value::as_str)
                .map(|s| s.to_string())
                .or_else(|| node.id().cloned())
                .unwrap_or_else(|| format!("msg_{}", uuid::Uuid::new_v4()));
            obj.insert("id".to_string(), json!(id));
            obj.insert("status".to_string(), json!("completed"));
            merge_json_extra(&mut obj, extra_body);
            Value::Object(obj)
        }
        urp::Node::ProviderItem {
            origin_protocol,
            item_type,
            body,
            extra_body,
            ..
        } => {
            if *origin_protocol != urp::ProviderProtocol::Responses {
                return Value::Null;
            }
            encode_responses_provider_output_item(item_type, body, extra_body, node.id())
        }
        urp::Node::ToolResult {
            id,
            call_id,
            is_error,
            content,
            extra_body,
        } => {
            let mut obj = Map::new();
            obj.insert("type".to_string(), json!("function_call_output"));
            obj.insert("call_id".to_string(), json!(call_id));
            obj.insert(
                "id".to_string(),
                json!(
                    id.clone()
                        .or_else(|| extra_body
                            .get("id")
                            .and_then(Value::as_str)
                            .map(|s| s.to_string()))
                        .unwrap_or_else(|| format!("tr_{}", uuid::Uuid::new_v4()))
                ),
            );
            obj.insert("status".to_string(), json!("completed"));
            obj.insert("output".to_string(), encode_tool_result_output(content));
            if *is_error {
                obj.insert("is_error".to_string(), Value::Bool(true));
            }
            merge_json_extra(&mut obj, extra_body);
            Value::Object(obj)
        }
        urp::Node::NextDownstreamEnvelopeExtra { extra_body } => {
            let mut obj = Map::new();
            merge_json_extra(&mut obj, extra_body);
            Value::Object(obj)
        }
    }
}

fn append_string_field_to_message_content(
    item: &mut Value,
    content_type: &str,
    field_name: &str,
    delta: &str,
) {
    let Some(content) = item.get_mut("content").and_then(Value::as_array_mut) else {
        return;
    };
    let needs_new_part = content
        .last()
        .is_none_or(|last| last.get("type").and_then(Value::as_str) != Some(content_type));
    if needs_new_part {
        content.push(json!({ "type": content_type, field_name: "" }));
    }
    if let Some(last_part) = content.last_mut().and_then(Value::as_object_mut) {
        let current = last_part
            .get(field_name)
            .and_then(Value::as_str)
            .unwrap_or_default();
        last_part.insert(field_name.to_string(), json!(format!("{current}{delta}")));
    }
}

fn append_string_field(item: &mut Value, field_name: &str, delta: &str) {
    let Some(obj) = item.as_object_mut() else {
        return;
    };
    let current = obj
        .get(field_name)
        .and_then(Value::as_str)
        .unwrap_or_default();
    obj.insert(field_name.to_string(), json!(format!("{current}{delta}")));
}

fn append_reasoning_summary_field(item: &mut Value, delta: &str) {
    let Some(obj) = item.as_object_mut() else {
        return;
    };
    let summary = obj
        .entry("summary".to_string())
        .or_insert_with(|| Value::Array(vec![json!({ "type": "summary_text", "text": "" })]));
    let Some(entries) = summary.as_array_mut() else {
        return;
    };
    if entries.is_empty() {
        entries.push(json!({ "type": "summary_text", "text": "" }));
    }
    let Some(last) = entries.last_mut().and_then(Value::as_object_mut) else {
        return;
    };
    let current = last.get("text").and_then(Value::as_str).unwrap_or_default();
    last.insert("text".to_string(), json!(format!("{current}{delta}")));
}

fn append_node_delta_to_completed_item(
    node_state: &mut StreamedNodeState,
    delta: &urp::NodeDelta,
    extra_body: Option<&HashMap<String, Value>>,
) {
    let Some(mut item) = node_state.completed_item.take() else {
        return;
    };
    match (node_state.zone, delta) {
        (ResponsesOutputZone::Message, urp::NodeDelta::Text { content }) => {
            append_string_field_to_message_content(&mut item, "output_text", "text", content);
        }
        (ResponsesOutputZone::Message, urp::NodeDelta::Refusal { content }) => {
            append_string_field_to_message_content(&mut item, "refusal", "refusal", content);
        }
        (
            ResponsesOutputZone::Reasoning,
            urp::NodeDelta::Reasoning {
                content,
                encrypted,
                summary,
                source,
            },
        ) => {
            if let Some(content) = content.as_deref().filter(|content| !content.is_empty()) {
                append_string_field(&mut item, "text", content);
            }
            if let Some(summary) = summary.as_deref().filter(|summary| !summary.is_empty()) {
                append_reasoning_summary_field(&mut item, summary);
            }
            if let Some(encrypted) = encrypted.as_ref().filter(|encrypted| !encrypted.is_null())
                && let Some(obj) = item.as_object_mut()
            {
                obj.insert("encrypted_content".to_string(), encrypted.clone());
            }
            if let Some(source) = source.as_deref().filter(|source| !source.is_empty()) {
                item = insert_reasoning_source(item, Some(source));
            }
        }
        (ResponsesOutputZone::FunctionCall, urp::NodeDelta::ToolCallArguments { arguments }) => {
            append_string_field(&mut item, "arguments", arguments);
        }
        (ResponsesOutputZone::ProviderItem, urp::NodeDelta::ProviderItem { data }) => {
            match (item.as_object_mut(), data) {
                (Some(obj), Value::Object(delta_obj)) => {
                    for (key, value) in delta_obj {
                        obj.insert(key.clone(), value.clone());
                    }
                }
                (Some(obj), Value::Null) => {
                    let _ = obj;
                }
                (Some(obj), other) => {
                    obj.insert("data".to_string(), other.clone());
                }
                _ => {}
            }
        }
        _ => {}
    }
    if let Some(extra_body) = extra_body
        && let Some(obj) = item.as_object_mut()
    {
        merge_json_extra(obj, extra_body);
    }
    node_state.completed_item = Some(item);
}

fn apply_node_done_to_stream_output_item_state(
    node_state: &mut StreamedNodeState,
    node: &urp::Node,
) {
    let Some(item) = node_state.completed_item.as_mut() else {
        return;
    };
    match node_state.zone {
        ResponsesOutputZone::Message => {
            if item.get("content").and_then(Value::as_array).is_none()
                && let Some(obj) = item.as_object_mut()
            {
                obj.insert("content".to_string(), json!([]));
            }
            if let Some(content) = item.get_mut("content").and_then(Value::as_array_mut) {
                let encoded_part = encode_node_done_content_part(node);
                let encoded_type = encoded_part
                    .get("type")
                    .and_then(Value::as_str)
                    .unwrap_or_default()
                    .to_string();
                let matches_last_type = content.last().is_some_and(|last| {
                    last.get("type").and_then(Value::as_str) == Some(encoded_type.as_str())
                });
                if content.is_empty() {
                    content.push(encoded_part);
                } else if matches_last_type {
                    if let Some(last_part) = content.last_mut() {
                        *last_part = encoded_part;
                    }
                } else {
                    content.push(encoded_part);
                }
            }
        }
        ResponsesOutputZone::Reasoning => {
            let existing_id = item.get("id").cloned();
            *item = complete_stream_output_item(encode_stream_output_item_from_node(node));
            if let Some(existing_id) = existing_id
                && let Some(obj) = item.as_object_mut()
            {
                obj.insert("id".to_string(), existing_id);
            }
        }
        ResponsesOutputZone::FunctionCall => {
            let existing_id = item.get("id").cloned();
            *item = complete_stream_output_item(encode_stream_output_item_from_node(node));
            if let Some(existing_id) = existing_id
                && let Some(obj) = item.as_object_mut()
            {
                obj.insert("id".to_string(), existing_id);
            }
        }
        ResponsesOutputZone::ProviderItem => {
            let existing_id = item.get("id").cloned();
            *item = encode_stream_output_item_from_node(node);
            if let Some(existing_id) = existing_id
                && item.get("id").is_none()
                && let Some(obj) = item.as_object_mut()
            {
                obj.insert("id".to_string(), existing_id);
            }
        }
    }
}

fn insert_reasoning_source(mut payload: Value, source: Option<&str>) -> Value {
    let Some(source) = source.filter(|source| !source.is_empty()) else {
        return payload;
    };
    let Some(obj) = payload.as_object_mut() else {
        return payload;
    };
    obj.insert("source".to_string(), Value::String(source.to_string()));
    payload
}

fn apply_node_done_to_stream_output_item(
    active_output: &mut ActiveResponsesOutputItem,
    node: &urp::Node,
) {
    match active_output.zone {
        ResponsesOutputZone::Message => {
            if active_output
                .item
                .get("content")
                .and_then(Value::as_array)
                .is_none()
                && let Some(obj) = active_output.item.as_object_mut()
            {
                obj.insert("content".to_string(), json!([]));
            }
            if let Some(content) = active_output
                .item
                .get_mut("content")
                .and_then(Value::as_array_mut)
            {
                let encoded_part = encode_node_done_content_part(node);
                let encoded_type = encoded_part
                    .get("type")
                    .and_then(Value::as_str)
                    .unwrap_or_default()
                    .to_string();
                let matches_last_type = content.last().is_some_and(|last| {
                    last.get("type").and_then(Value::as_str) == Some(encoded_type.as_str())
                });
                if content.is_empty() {
                    content.push(encoded_part);
                } else if matches_last_type {
                    if let Some(last_part) = content.last_mut() {
                        *last_part = encoded_part;
                    }
                } else {
                    content.push(encoded_part);
                }
            }
        }
        ResponsesOutputZone::Reasoning
        | ResponsesOutputZone::FunctionCall
        | ResponsesOutputZone::ProviderItem => {}
    }
}

fn response_envelope_payload(
    id: &str,
    created_at: i64,
    model: &str,
    status: &str,
    output: Value,
) -> Value {
    let completed_at = if status == "completed" {
        Value::Number(serde_json::Number::from(created_at))
    } else {
        Value::Null
    };
    json!({
        "response": {
            "id": id,
            "object": "response",
            "created_at": created_at,
            "completed_at": completed_at,
            "model": model,
            "status": status,
            "output": output,
            "incomplete_details": null,
            "previous_response_id": null,
            "instructions": null,
            "error": null,
            "tools": [],
            "tool_choice": "auto",
            "truncation": "auto",
            "parallel_tool_calls": true,
            "text": { "format": { "type": "text" } },
            "top_p": 1.0,
            "presence_penalty": 0,
            "frequency_penalty": 0,
            "top_logprobs": 0,
            "temperature": 1.0,
            "reasoning": null,
            "max_output_tokens": null,
            "max_tool_calls": null,
            "store": false,
            "background": false,
            "metadata": {},
            "safety_identifier": null,
            "prompt_cache_key": null,
            "usage": null,
            "user": null,
        }
    })
}

fn response_failed_payload(
    id: &str,
    created_at: i64,
    model: &str,
    code: Option<&str>,
    message: &str,
    error_extra_body: &HashMap<String, Value>,
) -> Value {
    let mut response =
        response_envelope_payload(id, created_at, model, "failed", Value::Array(Vec::new()));
    let mut error = json!({
        "code": code.unwrap_or("upstream_error"),
        "message": message,
    });
    if let Some(error_obj) = error.as_object_mut() {
        merge_json_extra(error_obj, error_extra_body);
    }
    if let Some(obj) = response.get_mut("response").and_then(Value::as_object_mut) {
        obj.insert("completed_at".to_string(), json!(now_ts()));
        obj.insert("error".to_string(), error.clone());
    }
    response.get("response").cloned().unwrap_or_else(|| {
        json!({
            "id": id,
            "object": "response",
            "created_at": created_at,
            "completed_at": now_ts(),
            "model": model,
            "status": "failed",
            "output": [],
            "error": error
        })
    })
}

fn ensure_response_object_user_field(mut response: Value) -> Value {
    if let Some(obj) = response.as_object_mut() {
        obj.entry("user".to_string()).or_insert(Value::Null);
    }
    response
}

fn complete_stream_output_item(mut item: Value) -> Value {
    if let Some(obj) = item.as_object_mut() {
        if matches!(
            obj.get("type").and_then(Value::as_str),
            Some("message" | "reasoning" | "function_call" | "function_call_output")
        ) {
            obj.insert("status".to_string(), json!("completed"));
        }
    }
    item
}

fn reasoning_item_with_duration(mut item: Value, duration_secs: Option<u64>) -> Value {
    if item.get("type").and_then(Value::as_str) != Some("reasoning") {
        return item;
    }
    let Some(duration_secs) = duration_secs else {
        return item;
    };
    if let Some(obj) = item.as_object_mut() {
        obj.entry("duration".to_string())
            .or_insert_with(|| json!(duration_secs));
    }
    item
}

fn maybe_reasoning_added_item_with_duration(item: Value, duration_secs: u64) -> Value {
    if item.get("type").and_then(Value::as_str) != Some("reasoning") {
        return item;
    }
    let completed_or_terminal = item.get("status").and_then(Value::as_str) == Some("completed")
        || item
            .get("summary")
            .and_then(Value::as_array)
            .is_some_and(|summary| summary.iter().any(summary_part_has_text))
        || item
            .get("content")
            .and_then(Value::as_array)
            .is_some_and(|content| content.iter().any(content_part_has_text))
        || item
            .get("text")
            .and_then(Value::as_str)
            .is_some_and(|text| !text.is_empty());
    if completed_or_terminal {
        reasoning_item_with_duration(item, Some(duration_secs))
    } else {
        item
    }
}

fn summary_part_has_text(part: &Value) -> bool {
    part.get("text")
        .and_then(Value::as_str)
        .is_some_and(|text| !text.is_empty())
}

fn content_part_has_text(part: &Value) -> bool {
    part.get("text")
        .and_then(Value::as_str)
        .or_else(|| part.get("summary").and_then(Value::as_str))
        .is_some_and(|text| !text.is_empty())
}

fn reasoning_duration_secs(
    node_state: &StreamedNodeState,
    stream_started_at: Instant,
) -> Option<u64> {
    if node_state.zone != ResponsesOutputZone::Reasoning {
        return None;
    }
    let stream_elapsed = stream_started_at.elapsed().as_secs();
    Some(match node_state.reasoning_started_at {
        Some(started_at) => stream_elapsed.max(started_at.elapsed().as_secs()),
        None => stream_elapsed,
    })
}

fn response_with_reasoning_durations(mut response: Value, duration_secs: Option<u64>) -> Value {
    let Some(duration_secs) = duration_secs else {
        return response;
    };
    let Some(output) = response.get_mut("output").and_then(Value::as_array_mut) else {
        return response;
    };
    for item in output {
        if item.get("type").and_then(Value::as_str) == Some("reasoning") {
            if let Some(obj) = item.as_object_mut() {
                obj.entry("duration".to_string())
                    .or_insert_with(|| json!(duration_secs));
            }
        }
    }
    response
}

fn rebuild_completed_response_output(completed_output_items: &[(usize, Value)]) -> Vec<Value> {
    completed_output_items
        .iter()
        .map(|(_, item)| item.clone())
        .collect()
}

fn sync_completed_output_items_with_terminal_output(
    completed_output_items: &mut Vec<(usize, Value)>,
    terminal_output: &[Value],
) {
    for (output_index, item) in terminal_output.iter().enumerate() {
        if let Some((_, existing_item)) = completed_output_items
            .iter_mut()
            .find(|(existing_index, _)| *existing_index == output_index)
        {
            merge_terminal_output_item(existing_item, item);
        } else if let Some((_, existing_item)) = completed_output_items
            .iter_mut()
            .find(|(_, existing_item)| responses_output_items_semantically_match(existing_item, item))
        {
            merge_terminal_output_item(existing_item, item);
        }
    }
}

fn responses_output_items_semantically_match(left: &Value, right: &Value) -> bool {
    let left_type = left.get("type").and_then(Value::as_str);
    let right_type = right.get("type").and_then(Value::as_str);
    if left_type.is_none() || left_type != right_type {
        return false;
    }

    if non_empty_string_field_matches(left, right, "id") {
        return true;
    }

    match left_type {
        Some("message") => responses_message_items_semantically_match(left, right),
        Some("function_call") => non_empty_string_field_matches(left, right, "call_id"),
        Some("reasoning") => responses_reasoning_items_semantically_match(left, right),
        _ => false,
    }
}

fn non_empty_string_field_matches(left: &Value, right: &Value, field: &str) -> bool {
    match (
        left.get(field).and_then(Value::as_str),
        right.get(field).and_then(Value::as_str),
    ) {
        (Some(left_value), Some(right_value)) => !left_value.is_empty() && left_value == right_value,
        _ => false,
    }
}

fn optional_string_fields_compatible(left: &Value, right: &Value, field: &str) -> bool {
    let left_value = left.get(field).and_then(Value::as_str);
    let right_value = right.get(field).and_then(Value::as_str);
    left_value == right_value || left_value.is_none() || right_value.is_none()
}

fn responses_message_items_semantically_match(left: &Value, right: &Value) -> bool {
    let left_role = left
        .get("role")
        .and_then(Value::as_str)
        .unwrap_or("assistant");
    let right_role = right
        .get("role")
        .and_then(Value::as_str)
        .unwrap_or("assistant");
    if left_role != right_role || !optional_string_fields_compatible(left, right, "phase") {
        return false;
    }

    let left_text = responses_message_text_signature(left);
    let right_text = responses_message_text_signature(right);
    !left_text.is_empty() && left_text == right_text
}

fn responses_message_text_signature(item: &Value) -> Vec<String> {
    item.get("content")
        .and_then(Value::as_array)
        .map(|parts| {
            parts
                .iter()
                .filter_map(|part| {
                    part.get("text")
                        .or_else(|| part.get("refusal"))
                        .and_then(Value::as_str)
                        .map(str::to_string)
                })
                .collect()
        })
        .unwrap_or_default()
}

fn responses_reasoning_items_semantically_match(left: &Value, right: &Value) -> bool {
    ["text", "encrypted_content", "source"]
        .iter()
        .all(|field| left.get(*field) == right.get(*field))
        && left.get("summary") == right.get("summary")
}

fn merge_terminal_output_item(existing_item: &mut Value, terminal_item: &Value) {
    let existing_type = existing_item
        .get("type")
        .and_then(Value::as_str)
        .map(str::to_string);
    let terminal_type = terminal_item
        .get("type")
        .and_then(Value::as_str)
        .map(str::to_string);
    if existing_type.is_none() || existing_type != terminal_type {
        return;
    }

    let Some(existing_obj) = existing_item.as_object_mut() else {
        return;
    };
    let Some(terminal_obj) = terminal_item.as_object() else {
        return;
    };

    match existing_type.as_deref() {
        Some("message") => {
            if let Some(content) = terminal_obj.get("content").cloned() {
                existing_obj.insert("content".to_string(), content);
            }
            if let Some(status) = terminal_obj.get("status").cloned() {
                existing_obj.insert("status".to_string(), status);
            }
        }
        Some("reasoning") => {
            for key in ["text", "summary", "encrypted_content", "source", "status"] {
                if let Some(value) = terminal_obj.get(key).cloned() {
                    existing_obj.insert(key.to_string(), value);
                }
            }
        }
        Some("function_call") => {
            for key in ["arguments", "call_id", "name", "status"] {
                if let Some(value) = terminal_obj.get(key).cloned() {
                    existing_obj.insert(key.to_string(), value);
                }
            }
        }
        _ => {
            for (key, value) in terminal_obj {
                if key != "id" && key != "type" {
                    existing_obj.insert(key.clone(), value.clone());
                }
            }
        }
    }
}

#[allow(clippy::too_many_arguments)]
async fn emit_missing_terminal_output_done_events(
    tx: &mpsc::Sender<Event>,
    seq: &mut u64,
    completed_output_indices: &mut HashSet<usize>,
    streamed_output_indices: &mut HashSet<usize>,
    completed_output_items: &mut Vec<(usize, Value)>,
    terminal_output: &[Value],
    reasoning_delta_indices: &mut HashSet<usize>,
    reasoning_done_indices: &mut HashSet<usize>,
    reasoning_summary_added_indices: &mut HashSet<usize>,
    reasoning_summary_delta_indices: &mut HashSet<usize>,
    reasoning_summary_text_done_indices: &mut HashSet<usize>,
    reasoning_summary_part_done_indices: &mut HashSet<usize>,
    function_args_delta_indices: &mut HashSet<usize>,
    function_args_done_indices: &mut HashSet<usize>,
    default_reasoning_duration_secs: u64,
    sse_max_frame_length: Option<usize>,
) -> AppResult<()> {
    for (output_index, item) in terminal_output.iter().enumerate() {
        if completed_output_indices.contains(&output_index) {
            continue;
        }
        if let Some((_, existing_item)) = completed_output_items
            .iter_mut()
            .find(|(_, existing_item)| responses_output_items_semantically_match(existing_item, item))
        {
            merge_terminal_output_item(existing_item, item);
            continue;
        }
        let done_item = sanitize_responses_output_item_for_frame_limit(
            &reasoning_item_with_duration(item.clone(), Some(default_reasoning_duration_secs)),
            sse_max_frame_length,
        );
        if streamed_output_indices.insert(output_index) {
            send_responses_event(
                tx,
                seq,
                "response.output_item.added",
                json!({
                    "output_index": output_index,
                    "item": done_item.clone(),
                }),
            )
            .await?;
        }
        emit_missing_terminal_message_child_lifecycles(
            tx,
            seq,
            output_index,
            &done_item,
            sse_max_frame_length,
        )
        .await?;
        let single_item = vec![(output_index, done_item.clone())];
        emit_missing_terminal_sub_lifecycles(
            tx,
            seq,
            &single_item,
            reasoning_delta_indices,
            reasoning_done_indices,
            reasoning_summary_added_indices,
            reasoning_summary_delta_indices,
            reasoning_summary_text_done_indices,
            reasoning_summary_part_done_indices,
            function_args_delta_indices,
            function_args_done_indices,
            sse_max_frame_length,
        )
        .await?;
        send_responses_event(
            tx,
            seq,
            "response.output_item.done",
            json!({
                "output_index": output_index,
                "item": done_item,
            }),
        )
        .await?;
        completed_output_indices.insert(output_index);
        completed_output_items.push((output_index, done_item));
    }
    Ok(())
}

async fn emit_missing_terminal_message_child_lifecycles(
    tx: &mpsc::Sender<Event>,
    seq: &mut u64,
    output_index: usize,
    item: &Value,
    sse_max_frame_length: Option<usize>,
) -> AppResult<()> {
    if item.get("type").and_then(Value::as_str) != Some("message") {
        return Ok(());
    }
    let item_id = item.get("id").cloned().unwrap_or(Value::Null);
    let phase = item.get("phase").and_then(Value::as_str);
    let Some(content) = item.get("content").and_then(Value::as_array) else {
        return Ok(());
    };
    for (content_index, part) in content.iter().enumerate() {
        let part_type = part.get("type").and_then(Value::as_str).unwrap_or_default();
        if !matches!(part_type, "output_text" | "text") {
            continue;
        }
        let text = part
            .get("text")
            .and_then(Value::as_str)
            .unwrap_or_default();
        let mut added_part = part.clone();
        if let Some(obj) = added_part.as_object_mut() {
            obj.insert("text".to_string(), Value::String(String::new()));
        }
        send_responses_event(
            tx,
            seq,
            "response.content_part.added",
            json!({
                "output_index": output_index,
                "content_index": content_index,
                "item_id": item_id.clone(),
                "part": added_part,
            }),
        )
        .await?;
        if !text.is_empty() {
            send_responses_delta_string(
                tx,
                seq,
                "response.output_text.delta",
                responses_text_delta_payload(
                    phase,
                    item,
                    output_index as u64,
                    content_index as u64,
                ),
                "delta",
                text,
                sse_max_frame_length,
            )
            .await?;
        }
        let mut done_payload = responses_text_delta_payload(
            phase,
            item,
            output_index as u64,
            content_index as u64,
        );
        if let Some(obj) = done_payload.as_object_mut() {
            obj.insert("text".to_string(), Value::String(text.to_string()));
        }
        send_responses_event(tx, seq, "response.output_text.done", done_payload).await?;
        send_responses_event(
            tx,
            seq,
            "response.content_part.done",
            json!({
                "output_index": output_index,
                "content_index": content_index,
                "item_id": item_id.clone(),
                "part": part,
            }),
        )
        .await?;
    }
    Ok(())
}

async fn emit_missing_terminal_sub_lifecycles(
    tx: &mpsc::Sender<Event>,
    seq: &mut u64,
    completed_output_items: &[(usize, Value)],
    reasoning_delta_indices: &mut HashSet<usize>,
    reasoning_done_indices: &mut HashSet<usize>,
    reasoning_summary_added_indices: &mut HashSet<usize>,
    reasoning_summary_delta_indices: &mut HashSet<usize>,
    reasoning_summary_text_done_indices: &mut HashSet<usize>,
    reasoning_summary_part_done_indices: &mut HashSet<usize>,
    function_args_delta_indices: &mut HashSet<usize>,
    function_args_done_indices: &mut HashSet<usize>,
    sse_max_frame_length: Option<usize>,
) -> AppResult<()> {
    for (output_index, item) in completed_output_items {
        match item.get("type").and_then(Value::as_str).unwrap_or_default() {
            "reasoning" => {
                let item_id = item.get("id").cloned().unwrap_or(Value::Null);
                let source = item.get("source").and_then(Value::as_str);
                if let Some(summary_entries) = item.get("summary").and_then(Value::as_array)
                    && let Some(summary_text) = summary_entries
                        .iter()
                        .find_map(|entry| entry.get("text").and_then(Value::as_str))
                    && !summary_text.is_empty()
                {
                    if reasoning_summary_added_indices.insert(*output_index) {
                        send_responses_event(
                            tx,
                            seq,
                            "response.reasoning_summary_part.added",
                            json!({
                                "item_id": item_id,
                                "output_index": output_index,
                                "summary_index": 0,
                                "part": { "type": "summary_text", "text": "" },
                            }),
                        )
                        .await?;
                    }
                    if reasoning_summary_delta_indices.insert(*output_index) {
                        send_responses_delta_string(
                            tx,
                            seq,
                            "response.reasoning_summary_text.delta",
                            insert_reasoning_source(
                                json!({
                                    "item_id": item_id,
                                    "output_index": output_index,
                                    "summary_index": 0,
                                }),
                                source,
                            ),
                            "delta",
                            summary_text,
                            sse_max_frame_length,
                        )
                        .await?;
                    }
                    if reasoning_summary_text_done_indices.insert(*output_index) {
                        send_responses_event(
                            tx,
                            seq,
                            "response.reasoning_summary_text.done",
                            insert_reasoning_source(
                                json!({
                                    "item_id": item_id,
                                    "output_index": output_index,
                                    "summary_index": 0,
                                    "text": summary_text,
                                }),
                                source,
                            ),
                        )
                        .await?;
                    }
                    if reasoning_summary_part_done_indices.insert(*output_index) {
                        send_responses_event(
                            tx,
                            seq,
                            "response.reasoning_summary_part.done",
                            json!({
                                "item_id": item_id,
                                "output_index": output_index,
                                "summary_index": 0,
                                "part": { "type": "summary_text", "text": summary_text },
                            }),
                        )
                        .await?;
                    }
                }
                if let Some(text) = item.get("text").and_then(Value::as_str)
                    && !text.is_empty()
                {
                    if reasoning_delta_indices.insert(*output_index) {
                        send_responses_delta_string(
                            tx,
                            seq,
                            "response.reasoning_text.delta",
                            insert_reasoning_source(
                                json!({
                                    "item_id": item_id,
                                    "output_index": output_index,
                                    "content_index": 0,
                                }),
                                source,
                            ),
                            "delta",
                            text,
                            sse_max_frame_length,
                        )
                        .await?;
                    }
                    if reasoning_done_indices.insert(*output_index) {
                        send_responses_event(
                            tx,
                            seq,
                            "response.reasoning_text.done",
                            insert_reasoning_source(
                                json!({
                                    "item_id": item_id,
                                    "output_index": output_index,
                                    "content_index": 0,
                                    "text": text,
                                }),
                                source,
                            ),
                        )
                        .await?;
                    }
                }
            }
            "function_call" => {
                let arguments = item
                    .get("arguments")
                    .and_then(Value::as_str)
                    .unwrap_or_default();
                if function_args_delta_indices.insert(*output_index) && !arguments.is_empty() {
                    send_responses_delta_string(
                        tx,
                        seq,
                        "response.function_call_arguments.delta",
                        json!({
                            "item_id": item.get("id").cloned().unwrap_or(Value::Null),
                            "output_index": output_index,
                        }),
                        "delta",
                        arguments,
                        sse_max_frame_length,
                    )
                    .await?;
                }
                if function_args_done_indices.insert(*output_index) {
                    send_responses_event(
                        tx,
                        seq,
                        "response.function_call_arguments.done",
                        json!({
                            "arguments": item.get("arguments").cloned().unwrap_or(Value::String(String::new())),
                            "call_id": item.get("call_id").cloned().unwrap_or(Value::Null),
                            "item_id": item.get("id").cloned().unwrap_or(Value::Null),
                            "name": item.get("name").cloned().unwrap_or(Value::Null),
                            "output_index": output_index,
                        }),
                    )
                    .await?;
                }
            }
            _ => {}
        }
    }
    Ok(())
}

fn encode_image_part(
    source: &crate::urp::ImageSource,
    extra_body: &HashMap<String, Value>,
) -> Value {
    let mut obj = Map::new();
    obj.insert("type".to_string(), json!("output_image"));
    match source {
        crate::urp::ImageSource::Url { url, detail } => {
            obj.insert("url".to_string(), json!(url));
            if let Some(detail) = detail {
                obj.insert("detail".to_string(), json!(detail));
            }
        }
        crate::urp::ImageSource::Base64 { media_type, data } => {
            obj.insert(
                "source".to_string(),
                json!({ "type": "base64", "media_type": media_type, "data": data }),
            );
        }
    }
    merge_json_extra(&mut obj, extra_body);
    Value::Object(obj)
}

fn encode_file_part(source: &crate::urp::FileSource, extra_body: &HashMap<String, Value>) -> Value {
    let mut obj = Map::new();
    obj.insert("type".to_string(), json!("output_file"));
    match source {
        crate::urp::FileSource::Url { url } => {
            obj.insert("url".to_string(), json!(url));
        }
        crate::urp::FileSource::Base64 {
            filename,
            media_type,
            data,
        } => {
            obj.insert(
                "source".to_string(),
                json!({
                    "type": "base64",
                    "filename": filename,
                    "media_type": media_type,
                    "data": data,
                }),
            );
        }
    }
    merge_json_extra(&mut obj, extra_body);
    Value::Object(obj)
}

fn encode_audio_source(source: &crate::urp::AudioSource) -> Value {
    match source {
        crate::urp::AudioSource::Url { url } => json!({ "type": "url", "url": url }),
        crate::urp::AudioSource::Base64 { media_type, data } => {
            json!({ "type": "base64", "media_type": media_type, "data": data })
        }
    }
}

fn encode_tool_result_output(content: &[ToolResultContent]) -> Value {
    if content.is_empty() {
        return Value::String(String::new());
    }
    if content.len() == 1 {
        if let ToolResultContent::Text { text } = &content[0] {
            return Value::String(text.clone());
        }
    }

    Value::Array(
        content
            .iter()
            .map(|part| match part {
                ToolResultContent::Text { text } => json!({ "type": "input_text", "text": text }),
                ToolResultContent::Image { source } => encode_image_part(source, &HashMap::new()),
                ToolResultContent::File { source } => encode_file_part(source, &HashMap::new()),
            })
            .collect(),
    )
}

fn merge_json_extra(obj: &mut Map<String, Value>, extra: &HashMap<String, Value>) {
    for (k, v) in extra {
        obj.insert(k.clone(), v.clone());
    }
}

fn merge_hashmap_extra_preserving_typed(
    dst: &mut HashMap<String, Value>,
    extra: &HashMap<String, Value>,
) {
    for (k, v) in extra {
        if !dst.contains_key(k) {
            dst.insert(k.clone(), v.clone());
        }
    }
}

fn merge_json_extra_preserving_typed(obj: &mut Map<String, Value>, extra: &HashMap<String, Value>) {
    for (k, v) in extra {
        if !obj.contains_key(k) {
            obj.insert(k.clone(), v.clone());
        }
    }
}
