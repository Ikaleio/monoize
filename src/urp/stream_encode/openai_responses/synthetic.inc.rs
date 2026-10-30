pub(crate) async fn emit_synthetic_responses_stream(
    logical_model: &str,
    resp: &urp::UrpResponse,
    synthetic_reasoning_duration_secs: Option<u64>,
    sse_max_frame_length: Option<usize>,
    tx: mpsc::Sender<Event>,
) -> AppResult<()> {
    let mut seq = 0u64;
    let encoded = match urp::encode::openai_responses::encode_response_checked(resp, logical_model) {
        Ok(encoded) => encoded,
        Err(error) => return emit_responses_media_error(&tx, &mut seq, &error).await,
    };
    let encoded_output = encoded
        .get("output")
        .and_then(|v| v.as_array())
        .cloned()
        .unwrap_or_default();
    let response_id = encoded
        .get("id")
        .and_then(|v| v.as_str())
        .unwrap_or("resp")
        .to_string();
    let created = encoded
        .get("created_at")
        .and_then(|v| v.as_i64())
        .unwrap_or_else(now_ts);
    let mut base_response = response_envelope_payload(
        &response_id,
        created,
        logical_model,
        "in_progress",
        Value::Array(Vec::new()),
        resp.extra_body.get(urp::RESPONSES_REQUEST_ECHO_EXTRA_KEY),
    );
    for key in ["store", "previous_response_id"] {
        if let Some(value) = encoded.get(key) { base_response["response"][key] = value.clone(); }
    }
    send_responses_event(&tx, &mut seq, "response.created", base_response.clone()).await?;
    send_responses_event(&tx, &mut seq, "response.in_progress", base_response).await?;

    for (output_index, item) in encoded_output.iter().enumerate() {
        let mut added_item = item.clone();
        match item.get("type").and_then(Value::as_str) {
            Some("function_call") => added_item["arguments"] = json!(""),
            Some("custom_tool_call") => added_item["input"] = json!(""),
            Some("message") => added_item["content"] = json!([]),
            _ => {}
        }
        if added_item.get("status").is_some() {
            added_item["status"] = json!("in_progress");
        }
        let item_payload = json!({
            "output_index": output_index,
            "item": added_item
        });
        send_responses_event(&tx, &mut seq, "response.output_item.added", item_payload).await?;

        match item.get("type").and_then(|v| v.as_str()).unwrap_or("") {
            "reasoning" => {
                let (text, _, _) = extract_reasoning_parts(item);
                let source = item
                    .get("source")
                    .and_then(Value::as_str)
                    .filter(|source| !source.is_empty())
                    .map(|source| source.to_string());
                let mut summary_lifecycle = ReasoningSummaryLifecycle::default();
                summary_lifecycle
                    .finish(
                        ReasoningSummaryTarget {
                            tx: &tx,
                            item_id: item.get("id").unwrap_or(&Value::Null),
                            output_index,
                            source: source.as_deref(),
                            sse_max_frame_length,
                        },
                        &mut seq,
                        &reasoning_summary_parts_from_item(item),
                    )
                    .await?;
                if !text.is_empty() {
                    send_responses_event(
                        &tx,
                        &mut seq,
                        "response.content_part.added",
                        json!({
                            "item_id": item.get("id").cloned().unwrap_or(Value::Null),
                            "output_index": output_index,
                            "content_index": 0,
                            "part": { "type": "reasoning_text", "text": "" },
                        }),
                    )
                    .await?;
                    send_responses_delta_string(
                        &tx,
                        &mut seq,
                        "response.reasoning_text.delta",
                        insert_reasoning_source(
                            json!({
                            "item_id": item.get("id").cloned().unwrap_or(Value::Null),
                            "output_index": output_index,
                            "content_index": 0,
                            }),
                            source.as_deref(),
                        ),
                        "delta",
                        &text,
                        sse_max_frame_length,
                    )
                    .await?;
                    send_responses_event(
                        &tx,
                        &mut seq,
                        "response.reasoning_text.done",
                        insert_reasoning_source(
                            json!({
                            "item_id": item.get("id").cloned().unwrap_or(Value::Null),
                            "output_index": output_index,
                            "content_index": 0,
                            "text": text,
                            }),
                            source.as_deref(),
                        ),
                    )
                    .await?;
                    send_responses_event(
                        &tx,
                        &mut seq,
                        "response.content_part.done",
                        json!({
                            "item_id": item.get("id").cloned().unwrap_or(Value::Null),
                            "output_index": output_index,
                            "content_index": 0,
                            "part": { "type": "reasoning_text", "text": text },
                        }),
                    )
                    .await?;
                }
            }
            "function_call" | "custom_tool_call" => {
                let is_custom = item.get("type").and_then(Value::as_str)
                    == Some("custom_tool_call");
                let call_id = item.get("call_id").and_then(|v| v.as_str()).unwrap_or("");
                let name = item.get("name").and_then(|v| v.as_str()).unwrap_or("");
                let arguments = item
                    .get(if is_custom { "input" } else { "arguments" })
                    .and_then(|v| v.as_str())
                    .unwrap_or("");
                if !arguments.is_empty() {
                    send_responses_delta_string(
                        &tx,
                        &mut seq,
                        if is_custom {
                            "response.custom_tool_call_input.delta"
                        } else {
                            "response.function_call_arguments.delta"
                        },
                        json!({
                            "item_id": item.get("id").cloned().unwrap_or(Value::Null),
                            "output_index": output_index,
                        }),
                        "delta",
                        arguments,
                        sse_max_frame_length,
                    )
                    .await?;
                    send_responses_event(
                        &tx,
                        &mut seq,
                        if is_custom {
                            "response.custom_tool_call_input.done"
                        } else {
                            "response.function_call_arguments.done"
                        },
                        json!({
                            (if is_custom { "input" } else { "arguments" }): arguments,
                            "call_id": call_id,
                            "item_id": item.get("id").cloned().unwrap_or(Value::Null),
                            "name": name,
                            "output_index": output_index,
                        }),
                    )
                    .await?;
                }
            }
            "message" => {
                emit_missing_terminal_message_child_lifecycles(
                    &tx, &mut seq, output_index, item, sse_max_frame_length,
                ).await?;
            }
            _ => {}
        }

        let done_item = sanitize_responses_output_item_for_frame_limit(
            &reasoning_item_with_duration(item.clone(), synthetic_reasoning_duration_secs),
            sse_max_frame_length,
        );
        send_responses_event(
            &tx,
            &mut seq,
            "response.output_item.done",
            json!({
                "output_index": output_index,
                "item": done_item
            }),
        )
        .await?;
    }
    let mut completed_response = sanitize_responses_completed_for_frame_limit(
        &response_with_reasoning_durations(encoded, synthetic_reasoning_duration_secs),
        sse_max_frame_length,
    );
    let terminal_event = match completed_response.get("status").and_then(Value::as_str) {
        Some("incomplete") => "response.incomplete",
        Some("failed") => "response.failed",
        Some("cancelled") => "response.cancelled",
        _ => "response.completed",
    };
    if terminal_event == "response.completed" { completed_response["completed_at"] = json!(now_ts()); }
    send_responses_event(
        &tx,
        &mut seq,
        terminal_event,
        json!({ "response": completed_response }),
    )
    .await?;
    Ok(())
}
