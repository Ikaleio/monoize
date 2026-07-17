pub(crate) async fn emit_synthetic_responses_stream(
    logical_model: &str,
    resp: &urp::UrpResponse,
    synthetic_reasoning_duration_secs: Option<u64>,
    sse_max_frame_length: Option<usize>,
    tx: mpsc::Sender<Event>,
) -> AppResult<()> {
    let mut seq = 1u64;
    let encoded = urp::encode::openai_responses::encode_response(resp, logical_model);
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
    let base_response = response_envelope_payload(
        &response_id,
        created,
        logical_model,
        "in_progress",
        Value::Array(Vec::new()),
    );
    send_responses_event(&tx, &mut seq, "response.created", base_response.clone()).await?;
    send_responses_event(&tx, &mut seq, "response.in_progress", base_response).await?;

    for (output_index, item) in encoded_output.iter().enumerate() {
        let item_payload = json!({
            "output_index": output_index,
            "item": item.clone()
        });
        send_responses_event(&tx, &mut seq, "response.output_item.added", item_payload).await?;

        match item.get("type").and_then(|v| v.as_str()).unwrap_or("") {
            "reasoning" => {
                let (text, summary, sig) = extract_reasoning_parts(item);
                let source = item
                    .get("source")
                    .and_then(Value::as_str)
                    .filter(|source| !source.is_empty())
                    .map(|source| source.to_string());
                if !summary.is_empty() {
                    send_responses_event(
                        &tx,
                        &mut seq,
                        "response.reasoning_summary_part.added",
                        json!({
                            "item_id": item.get("id").cloned().unwrap_or(Value::Null),
                            "output_index": output_index,
                            "summary_index": 0,
                            "part": { "type": "summary_text", "text": "" },
                        }),
                    )
                    .await?;
                    send_responses_delta_string(
                        &tx,
                        &mut seq,
                        "response.reasoning_summary_text.delta",
                        insert_reasoning_source(
                            json!({
                            "item_id": item.get("id").cloned().unwrap_or(Value::Null),
                            "output_index": output_index,
                            "summary_index": 0,
                            }),
                            source.as_deref(),
                        ),
                        "delta",
                        &summary,
                        sse_max_frame_length,
                    )
                    .await?;
                    send_responses_event(
                        &tx,
                        &mut seq,
                        "response.reasoning_summary_text.done",
                        insert_reasoning_source(
                            json!({
                            "item_id": item.get("id").cloned().unwrap_or(Value::Null),
                            "output_index": output_index,
                            "summary_index": 0,
                            "text": summary,
                            }),
                            source.as_deref(),
                        ),
                    )
                    .await?;
                }
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
                if !summary.is_empty() {
                    send_responses_event(
                        &tx,
                        &mut seq,
                        "response.reasoning_summary_part.done",
                        json!({
                            "item_id": item.get("id").cloned().unwrap_or(Value::Null),
                            "output_index": output_index,
                            "summary_index": 0,
                            "part": { "type": "summary_text", "text": summary },
                        }),
                    )
                    .await?;
                }
                let _ = sig;
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
                let text = extract_responses_message_text(item);
                let phase = extract_responses_message_phase(item);
                send_responses_event(
                    &tx,
                    &mut seq,
                    "response.content_part.added",
                    json!({
                        "output_index": output_index,
                        "content_index": 0,
                        "item_id": item.get("id").cloned().unwrap_or(Value::Null),
                                    "part": { "type": "output_text", "text": "", "annotations": [], "logprobs": [] },
                    }),
                )
                .await?;
                if !text.is_empty() {
                    send_responses_delta_string(
                        &tx,
                        &mut seq,
                        "response.output_text.delta",
                        responses_text_delta_payload(
                            phase.as_deref(),
                            item,
                            output_index as u64,
                            0,
                        ),
                        "delta",
                        &text,
                        sse_max_frame_length,
                    )
                    .await?;
                }
                let mut done_payload =
                    responses_text_delta_payload(phase.as_deref(), item, output_index as u64, 0);
                if let Some(obj) = done_payload.as_object_mut() {
                    obj.insert("text".to_string(), json!(text));
                }
                send_responses_event(&tx, &mut seq, "response.output_text.done", done_payload)
                    .await?;
                send_responses_event(
                    &tx,
                    &mut seq,
                    "response.content_part.done",
                    json!({
                        "output_index": output_index,
                        "content_index": 0,
                        "item_id": item.get("id").cloned().unwrap_or(Value::Null),
                        "part": {
                            "type": "output_text",
                            "text": text,
                            "annotations": [],
                            "logprobs": [],
                        },
                    }),
                )
                .await?;
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
    let mut completed_response =
        ensure_response_object_user_field(sanitize_responses_completed_for_frame_limit(
            &response_with_reasoning_durations(encoded, synthetic_reasoning_duration_secs),
            sse_max_frame_length,
        ));
    completed_response["completed_at"] = json!(now_ts());
    send_responses_event(
        &tx,
        &mut seq,
        "response.completed",
        json!({ "response": completed_response }),
    )
    .await?;
    send_plain_sse_data(&tx, "[DONE]".to_string()).await?;
    Ok(())
}
