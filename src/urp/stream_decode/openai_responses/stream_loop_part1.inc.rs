pub(crate) async fn stream_responses_to_urp_events(
    urp: &HandlerUrpRequest,
    pending_request_envelope_extra: Option<HashMap<String, Value>>,
    upstream_resp: reqwest::Response,
    tx: mpsc::Sender<UrpStreamEvent>,
    started_at: Option<std::time::Instant>,
    runtime_metrics: Option<Arc<Mutex<StreamRuntimeMetrics>>>,
    idle_timeout_ms: u64,
) -> AppResult<()> {
    let (frame_tx, frame_rx) = mpsc::channel(32);
    let feeder = tokio::spawn(async move {
        feed_responses_sse_frames(upstream_resp, frame_tx).await;
    });
    let result = consume_responses_json_frames(
        urp,
        pending_request_envelope_extra,
        frame_rx,
        tx,
        started_at,
        runtime_metrics,
        idle_timeout_ms,
    )
    .await;
    feeder.abort();
    result
}

pub(crate) async fn stream_responses_websocket_to_urp_events(
    urp: &HandlerUrpRequest,
    pending_request_envelope_extra: Option<HashMap<String, Value>>,
    session: Box<dyn ResponsesWsSession>,
    tx: mpsc::Sender<UrpStreamEvent>,
    started_at: Option<std::time::Instant>,
    runtime_metrics: Option<Arc<Mutex<StreamRuntimeMetrics>>>,
    idle_timeout_ms: u64,
) -> AppResult<()> {
    let (frame_tx, frame_rx) = mpsc::channel(32);
    let feeder = tokio::spawn(async move {
        feed_responses_ws_frames(session, frame_tx).await;
    });
    let result = consume_responses_json_frames(
        urp,
        pending_request_envelope_extra,
        frame_rx,
        tx,
        started_at,
        runtime_metrics,
        idle_timeout_ms,
    )
    .await;
    feeder.abort();
    result
}

async fn feed_responses_sse_frames(
    upstream_resp: reqwest::Response,
    frame_tx: mpsc::Sender<AppResult<(String, String)>>,
) {
    let mut stream = upstream_resp.bytes_stream().eventsource();
    while let Some(ev) = stream.next().await {
        let ev = match ev {
            Ok(ev) => ev,
            Err(err) => {
                let _ = frame_tx
                    .send(Err(AppError::new(
                        StatusCode::BAD_GATEWAY,
                        "upstream_stream_decode_failed",
                        err.to_string(),
                    )))
                    .await;
                break;
            }
        };
        if frame_tx.send(Ok((ev.event, ev.data))).await.is_err() {
            break;
        }
    }
}

async fn feed_responses_ws_frames(
    mut session: Box<dyn ResponsesWsSession>,
    frame_tx: mpsc::Sender<AppResult<(String, String)>>,
) {
    loop {
        match session.next_text().await {
            Ok(None) => break,
            Ok(Some(text)) => {
                let event_name = serde_json::from_str::<Value>(&text)
                    .ok()
                    .and_then(|value| {
                        value
                            .get("type")
                            .and_then(Value::as_str)
                            .map(str::to_string)
                    })
                    .unwrap_or_default();
                if frame_tx.send(Ok((event_name, text))).await.is_err() {
                    break;
                }
            }
            Err(err) => {
                let _ = frame_tx
                    .send(Err(AppError::new(
                        StatusCode::BAD_GATEWAY,
                        "upstream_stream_decode_failed",
                        err,
                    )))
                    .await;
                break;
            }
        }
    }
    session.close().await;
}

async fn consume_responses_json_frames(
    urp: &HandlerUrpRequest,
    mut pending_request_envelope_extra: Option<HashMap<String, Value>>,
    mut frames: mpsc::Receiver<AppResult<(String, String)>>,
    tx: mpsc::Sender<UrpStreamEvent>,
    started_at: Option<std::time::Instant>,
    runtime_metrics: Option<Arc<Mutex<StreamRuntimeMetrics>>>,
    idle_timeout_ms: u64,
) -> AppResult<()> {
    let mut response_id = format!("resp_{}", uuid::Uuid::new_v4());
    let mut created = now_ts();
    let mut response_start_sent = false;
    let mut output_texts_by_output_index: HashMap<u64, String> = HashMap::new();
    let mut message_phases_by_output_index: HashMap<u64, String> = HashMap::new();
    let mut message_item_extra_by_output_index: HashMap<u64, HashMap<String, Value>> =
        HashMap::new();
    let mut item_ids_by_output_index: HashMap<u64, String> = HashMap::new();
    let mut reasoning_by_output_index: HashMap<u64, AccumulatedReasoningSlot> = HashMap::new();
    let mut call_order: Vec<String> = Vec::new();
    let mut calls: HashMap<String, (ToolCallType, String, String)> = HashMap::new();
    let mut call_ids_by_output_index: HashMap<u64, String> = HashMap::new();
    let mut saw_text_delta = false;
    let mut response_done_sent = false;
    let mut terminal_event_name: Option<String> = None;
    let mut index_state = ResponsesStreamIndexState::default();

    let idle_timeout = std::time::Duration::from_millis(idle_timeout_ms.max(1));
    while let Some(frame) = tokio::time::timeout(idle_timeout, frames.recv())
        .await
        .map_err(|_| {
            AppError::new(
                StatusCode::GATEWAY_TIMEOUT,
                "upstream_idle_timeout",
                format!("upstream stream idle for {idle_timeout_ms}ms without data"),
            )
        })?
    {
        let (mut event_name, data) = frame?;
        mark_stream_ttfb_if_needed(started_at, &runtime_metrics).await;
        if data.trim() == "[DONE]" {
            record_stream_done_sentinel(&runtime_metrics).await;
            break;
        }
        let data_val: Value = match serde_json::from_str(&data) {
            Ok(value) => value,
            Err(error) => {
                let code = "responses_invalid_sse_json".to_string();
                let message = format!("invalid JSON in upstream Responses event: {error}");
                let _ = tx
                    .send(UrpStreamEvent::Error {
                        code: Some(code.clone()),
                        message: message.clone(),
                        extra_body: HashMap::from([
                            ("event_name".to_string(), json!(event_name)),
                            ("raw_data".to_string(), json!(data)),
                        ]),
                    })
                    .await;
                record_stream_terminal_error(
                    &runtime_metrics,
                    "responses_invalid_sse_json",
                    StreamTerminalError {
                        code,
                        message,
                        http_status: StatusCode::BAD_GATEWAY.as_u16(),
                        error_type: Some("upstream_protocol_error".to_string()),
                        param: None,
                    },
                )
                .await;
                return Ok(());
            }
        };
        if (event_name.is_empty() || event_name == "message")
            && let Some(payload_type) = data_val
                .get("type")
                .and_then(Value::as_str)
                .filter(|name| !name.is_empty())
        {
            event_name.clear();
            event_name.push_str(payload_type);
        }
        if let Err(message) = validate_responses_event_shape(&event_name, &data_val) {
            let (code, message, extra_body, terminal_error) = responses_stream_error_parts("error", json!({
                "error": { "code":"responses_invalid_event", "type":"upstream_protocol_error", "message":message }
            }));
            let _ = tx.send(UrpStreamEvent::Error { code, message, extra_body }).await;
            record_stream_terminal_error(&runtime_metrics, "responses_invalid_event", terminal_error).await;
            return Ok(());
        }
        let content_validation = match event_name.as_str() {
            "response.content_part.added" | "response.content_part.done" => data_val.get("part")
                .map(crate::urp::decode::openai_responses::validate_compatible_content),
            "response.output_item.added" | "response.output_item.done" => data_val.get("item")
                .map(crate::urp::decode::openai_responses::validate_responses_items),
            "response.completed" | "response.failed" | "response.incomplete" => data_val.get("response").and_then(|response| response.get("output"))
                .map(crate::urp::decode::openai_responses::validate_responses_items),
            _ => None,
        };
        if let Some(Err(message)) = content_validation {
            let (code, message, extra_body, terminal_error) = responses_stream_error_parts("error", json!({
                "error": { "code":"malformed_media", "type":"upstream_protocol_error", "message":message }
            }));
            let _ = tx.send(UrpStreamEvent::Error { code, message, extra_body }).await;
            record_stream_terminal_error(&runtime_metrics, "malformed_media", terminal_error).await;
            return Ok(());
        }
        record_stream_response_service_tier(&runtime_metrics, &data_val).await;
        if let Some(native_response_id) = data_val
            .get("response")
            .and_then(|response| response.get("id"))
            .and_then(Value::as_str)
            .filter(|id| !id.is_empty())
        {
            response_id = native_response_id.to_string();
            record_stream_response_id(&runtime_metrics, native_response_id).await;
        }
        let native_start_event = matches!(
            event_name.as_str(),
            "response.created" | "response.in_progress"
        );
        let terminal_response_event = matches!(
            event_name.as_str(),
            "response.completed" | "response.incomplete" | "response.failed" | "response.cancelled"
        );
        let output_event = event_name.starts_with("response.output_")
            || event_name.starts_with("response.content_part.")
            || event_name.starts_with("response.reasoning_")
            || event_name.starts_with("response.function_call_")
            || event_name.starts_with("response.image_generation")
            || event_name.starts_with("image_generation.");
        if !response_start_sent && (native_start_event || terminal_response_event || output_event) {
            let source_response = data_val.get("response").and_then(Value::as_object).cloned();
            let sanitized_source_response = source_response.as_ref().map(|source| {
                crate::urp::decode::split_extra(
                    source,
                    &["id", "model", "output", "usage", "status", "incomplete_details", "error"],
                )
            });
            let mut start_model = urp.model.clone();
            if let Some(source) = source_response.as_ref() {
                if let Some(id) = source
                    .get("id")
                    .and_then(Value::as_str)
                    .filter(|id| !id.is_empty())
                {
                    response_id = id.to_string();
                }
                if let Some(source_created) = source
                    .get("created_at")
                    .or_else(|| source.get("created"))
                    .and_then(Value::as_i64)
                {
                    created = source_created;
                }
                if let Some(model) = source
                    .get("model")
                    .and_then(Value::as_str)
                    .filter(|model| !model.is_empty())
                {
                    start_model = model.to_string();
                }
            }
            let mut start_extra = if native_start_event {
                sanitized_source_response.clone().unwrap_or_default()
            } else {
                HashMap::from([
                    ("object".to_string(), json!("response")),
                    ("created_at".to_string(), json!(created)),
                    ("status".to_string(), json!("in_progress")),
                    ("output".to_string(), json!([])),
                ])
            };
            if native_start_event {
                start_extra.insert(
                    RESPONSES_STREAM_START_SOURCE_EXTRA_KEY.to_string(),
                    json!({}),
                );
            }
            let _ = tx
                .send(UrpStreamEvent::ResponseStart {
                    usage: source_response
                        .as_ref()
                        .and_then(|source| source.get("usage"))
                        .and_then(Value::as_object)
                        .map(crate::urp::decode::openai_responses::parse_usage_from_responses),
                    id: response_id.clone(),
                    model: start_model,
                    extra_body: start_extra,
                })
                .await;
            response_start_sent = true;
        }
        record_stream_usage_if_present(
            &runtime_metrics,
            parse_usage_from_responses_object(&data_val),
        )
        .await;

        let bare_error = data_val.get("error").is_some_and(|error| !error.is_null())
            && data_val.get("response").is_none();
        if matches!(event_name.as_str(), "error") || bare_error {
            let (code, message, extra_body, terminal_error) =
                responses_stream_error_parts(&event_name, data_val);
            let _ = tx
                .send(UrpStreamEvent::Error {
                    code,
                    message,
                    extra_body,
                })
                .await;
            record_stream_terminal_error(&runtime_metrics, &event_name, terminal_error).await;
            return Ok(());
        }

        if event_name == "response.failed" {
            let (_, _, _, terminal_error) =
                responses_stream_error_parts(&event_name, data_val.clone());
            record_stream_terminal_error(&runtime_metrics, &event_name, terminal_error).await;
        }
        accumulate_text_annotations(&event_name, &data_val, &mut index_state);
        if event_name == "response.output_text.delta" {
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
        if event_name == "response.reasoning_text.delta" {
            if let Some(delta) = data_val
                .get("delta")
                .and_then(|v| v.as_str())
                .or_else(|| data_val.get("text").and_then(|v| v.as_str()))
            {
                append_reasoning_text_delta(
                    reasoning_slot_for_event(&mut reasoning_by_output_index, &data_val),
                    delta,
                );
            }
        }
        if event_name == "response.reasoning_text.done" {
            if let Some(text) = data_val
                .get("text")
                .and_then(|v| v.as_str())
                .or_else(|| data_val.get("delta").and_then(|v| v.as_str()))
            {
                complete_reasoning_text(
                    reasoning_slot_for_event(&mut reasoning_by_output_index, &data_val),
                    text,
                );
            }
        }
        if event_name == "response.reasoning_summary_text.delta" {
            if let Some(delta) = data_val
                .get("delta")
                .and_then(|v| v.as_str())
                .or_else(|| data_val.get("text").and_then(|v| v.as_str()))
            {
                append_reasoning_summary_delta(
                    reasoning_slot_for_event(&mut reasoning_by_output_index, &data_val),
                    data_val.get("summary_index").and_then(Value::as_u64),
                    delta,
                );
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
        if event_name == "response.reasoning_summary_text.done" {
            if let Some(summary) = data_val
                .get("text")
                .and_then(|v| v.as_str())
                .or_else(|| data_val.get("delta").and_then(|v| v.as_str()))
            {
                complete_reasoning_summary(
                    reasoning_slot_for_event(&mut reasoning_by_output_index, &data_val),
                    data_val.get("summary_index").and_then(Value::as_u64),
                    summary,
                );
            }
        }
        if event_name == "response.reasoning_summary_part.done"
            && let Some(part) = data_val.get("part")
            && let Some(summary) = part.get("text").and_then(Value::as_str)
        {
            complete_reasoning_summary(
                reasoning_slot_for_event(&mut reasoning_by_output_index, &data_val),
                data_val.get("summary_index").and_then(Value::as_u64),
                summary,
            );
        }
        if event_name == "response.output_item.added" {
            let item = data_val.get("item").unwrap_or(&data_val);
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
            let item_type = item.get("type").and_then(|v| v.as_str());
            if matches!(item_type, Some("function_call" | "custom_tool_call")) {
                let tool_type = if item_type == Some("custom_tool_call") {
                    ToolCallType::Custom
                } else {
                    ToolCallType::Function
                };
                if let Some(call_id) = item.get("call_id").and_then(|v| v.as_str()) {
                    if !calls.contains_key(call_id) {
                        call_order.push(call_id.to_string());
                        calls.insert(
                            call_id.to_string(),
                            (
                                tool_type,
                                item.get("name")
                                    .and_then(|v| v.as_str())
                                    .unwrap_or("")
                                    .to_string(),
                                item.get(if tool_type == ToolCallType::Custom {
                                    "input"
                                } else {
                                    "arguments"
                                })
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
                    let slot = reasoning_slot_for_item(&mut reasoning_by_output_index, idx, item);
                    merge_reasoning_item_snapshot(slot, item, false);
                }
            }
        }
        if matches!(
            event_name.as_str(),
            "response.function_call_arguments.delta" | "response.custom_tool_call_input.delta"
        ) {
            let tool_type = if event_name == "response.custom_tool_call_input.delta" {
                ToolCallType::Custom
            } else {
                ToolCallType::Function
            };
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
                    calls.insert(
                        call_id.clone(),
                        (tool_type, name.to_string(), String::new()),
                    );
                }
                if let Some(entry) = calls.get_mut(call_id.as_str()) {
                    if tool_type == ToolCallType::Custom {
                        entry.0 = ToolCallType::Custom;
                    }
                    if entry.1.is_empty() && !name.is_empty() {
                        entry.1 = name.to_string();
                    }
                    entry.2.push_str(delta);
                }
            }
        }
        if matches!(
            event_name.as_str(),
            "response.function_call_arguments.done" | "response.custom_tool_call_input.done"
        ) {
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
                    .get(if event_name == "response.custom_tool_call_input.done" {
                        "input"
                    } else {
                        "arguments"
                    })
                    .or_else(|| data_val.get("arguments"))
                    .and_then(|v| v.as_str())
                    .unwrap_or("");
                if let Some(entry) = calls.get_mut(call_id.as_str()) {
                    if event_name == "response.custom_tool_call_input.done" {
                        entry.0 = ToolCallType::Custom;
                    }
                    replace_nonempty_tool_arguments(&mut entry.2, args);
                }
            }
        }
        if event_name == "response.output_item.done" {
            let item = data_val.get("item").unwrap_or(&data_val);
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
            let item_type = item.get("type").and_then(|v| v.as_str());
            if matches!(item_type, Some("function_call" | "custom_tool_call")) {
                let tool_type = if item_type == Some("custom_tool_call") {
                    ToolCallType::Custom
                } else {
                    ToolCallType::Function
                };
                if let Some(call_id) = item.get("call_id").and_then(|v| v.as_str()) {
                    let name = item.get("name").and_then(|v| v.as_str()).unwrap_or("");
                    let args = item
                        .get(if tool_type == ToolCallType::Custom {
                            "input"
                        } else {
                            "arguments"
                        })
                        .and_then(|v| v.as_str())
                        .unwrap_or("");
                    if !calls.contains_key(call_id) {
                        call_order.push(call_id.to_string());
                        calls.insert(
                            call_id.to_string(),
                            (tool_type, name.to_string(), args.to_string()),
                        );
                    } else if let Some(entry) = calls.get_mut(call_id) {
                        if tool_type == ToolCallType::Custom {
                            entry.0 = ToolCallType::Custom;
                        }
                        if entry.1.is_empty() && !name.is_empty() {
                            entry.1 = name.to_string();
                        }
                        replace_nonempty_tool_arguments(&mut entry.2, args);
                    }
                    if let Some(idx) = data_val.get("output_index").and_then(|v| v.as_u64()) {
                        call_ids_by_output_index.insert(idx, call_id.to_string());
                    }
                }
            } else if item.get("type").and_then(|v| v.as_str()) == Some("reasoning") {
                if let Some(idx) = data_val.get("output_index").and_then(|v| v.as_u64()) {
                    let slot = reasoning_slot_for_item(&mut reasoning_by_output_index, idx, item);
                    merge_reasoning_item_snapshot(slot, item, true);
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
        let is_terminal_response_event = matches!(
            event_name.as_str(),
            "response.completed" | "response.incomplete" | "response.failed" | "response.cancelled"
        );
        let stream_events = if is_terminal_response_event {
            for (output_index, output_state) in &index_state.output_state_by_index {
                if let Some(item_id) = output_state
                    .item_id
                    .as_deref()
                    .filter(|item_id| !item_id.is_empty())
                {
                    item_ids_by_output_index
                        .entry(*output_index)
                        .or_insert_with(|| item_id.to_string());
                }
            }
            let accumulated_output_entries = build_accumulated_output_entries(
                &reasoning_by_output_index,
                &output_texts_by_output_index,
                &message_phases_by_output_index,
                &message_item_extra_by_output_index,
                &item_ids_by_output_index,
                &call_order,
                &calls,
                &call_ids_by_output_index,
                &index_state,
            );
            map_response_completed_with_accumulated(
                data_val,
                &mut index_state,
                &accumulated_output_entries,
            )
        } else {
            map_responses_event_to_urp_events_with_state(
                &event_name,
                data_val,
                &message_phases_by_output_index,
                &mut index_state,
            )
        };
        for stream_event in stream_events {
            if let UrpStreamEvent::Error {
                code,
                message,
                extra_body: _,
            } = &stream_event
                && code.as_deref() == Some("responses_terminal_conflict")
            {
                let terminal_error = StreamTerminalError {
                    code: code
                        .clone()
                        .unwrap_or_else(|| "responses_terminal_conflict".to_string()),
                    message: message.clone(),
                    http_status: StatusCode::BAD_GATEWAY.as_u16(),
                    error_type: Some("upstream_protocol_error".to_string()),
                    param: Some("response.output".to_string()),
                };
                let _ = tx.send(stream_event).await;
                record_stream_terminal_error(
                    &runtime_metrics,
                    "responses_terminal_conflict",
                    terminal_error,
                )
                .await;
                return Ok(());
            }
            response_done_sent |= matches!(stream_event, UrpStreamEvent::ResponseDone { .. });
            record_visible_stream_event_delta(&runtime_metrics, &stream_event).await;
            let _ = tx.send(stream_event).await;
        }
        if response_done_sent && is_terminal_response_event {
            terminal_event_name = Some(event_name);
            break;
        }
    }

    if !response_done_sent {
        let code = "responses_stream_missing_terminal".to_string();
        let message = "upstream Responses stream ended without a terminal response event";
        let _ = tx
            .send(UrpStreamEvent::Error {
                code: Some(code.clone()),
                message: message.to_string(),
                extra_body: HashMap::from([(
                    "expected_events".to_string(),
                    json!([
                        "response.completed",
                        "response.incomplete",
                        "response.failed",
                        "response.cancelled"
                    ]),
                )]),
            })
            .await;
        record_stream_terminal_error(
            &runtime_metrics,
            "responses_stream_missing_terminal",
            StreamTerminalError {
                code,
                message: message.to_string(),
                http_status: StatusCode::BAD_GATEWAY.as_u16(),
                error_type: Some("upstream_protocol_error".to_string()),
                param: None,
            },
        )
        .await;
        return Ok(());
    }
    record_stream_terminal_event(
        &runtime_metrics,
        terminal_event_name
            .as_deref()
            .unwrap_or("responses.terminal"),
        None,
    )
    .await;
    Ok(())
}

fn validate_responses_event_shape(event: &str, data: &Value) -> Result<(), String> {
    if !data.is_object() {
        return Err(format!("{event} payload must be an object"));
    }
    let required = match event {
        "response.created"
        | "response.in_progress"
        | "response.queued"
        | "response.completed"
        | "response.incomplete"
        | "response.failed"
        | "response.cancelled" => Some("response"),
        "response.output_item.added" | "response.output_item.done" => Some("item"),
        "response.content_part.added" | "response.content_part.done" => Some("part"),
        _ => None,
    };
    if let Some(field) = required {
        let object = data
            .get(field)
            .and_then(Value::as_object)
            .ok_or_else(|| format!("{event}.{field} must be an object"))?;
        if field == "response" {
            if object.get("output").is_some_and(|value| !value.is_array()) {
                return Err(format!("{event}.response.output must be an array"));
            }
            if matches!(
                event,
                "response.completed"
                    | "response.incomplete"
                    | "response.failed"
                    | "response.cancelled"
            ) && let Some(status) = object.get("status")
                && status.as_str() != event.strip_prefix("response.")
            {
                return Err(format!("{event} conflicts with response.status"));
            }
        }
    }
    if matches!(
        event,
        "response.output_text.delta"
            | "response.refusal.delta"
            | "response.reasoning_text.delta"
            | "response.reasoning_summary_text.delta"
            | "response.function_call_arguments.delta"
            | "response.custom_tool_call_input.delta"
    ) && !data.get("delta").is_some_and(Value::is_string)
    {
        return Err(format!("{event}.delta must be a string"));
    }
    Ok(())
}
