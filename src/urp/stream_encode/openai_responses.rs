use crate::error::AppResult;
use crate::handlers::routing::now_ts;
use crate::urp::stream_helpers::*;
use crate::urp::{
    self, Item, ItemHeader, Part, PartDelta, PartHeader, Role, ToolResultContent, UrpStreamEvent,
};
use axum::response::sse::Event;
use serde_json::{Map, Value, json};
use std::collections::HashMap;
use tokio::sync::mpsc;

#[derive(Clone)]
struct PendingResponsesMessageItem {
    role: Role,
    phase: Option<String>,
    content: Vec<Value>,
    extra_body: HashMap<String, Value>,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum ResponsesOutputZone {
    Message,
    Reasoning,
    FunctionCall,
}

#[derive(Clone, Debug)]
struct ActiveResponsesOutputItem {
    zone: ResponsesOutputZone,
    output_index: usize,
    item_id: String,
    item: Value,
    next_content_index: u64,
    summary_part_added_sent: bool,
}

#[derive(Clone, Debug)]
struct PendingResponsesAssistantItem {
    role: Role,
    item_extra_body: HashMap<String, Value>,
    active_output: Option<ActiveResponsesOutputItem>,
    staged_outputs: Vec<ActiveResponsesOutputItem>,
    streamed_any_output: bool,
}

#[derive(Clone, Debug)]
struct StreamedPartState {
    item_index: u32,
    output_index: usize,
    zone: ResponsesOutputZone,
    content_index: Option<u32>,
    item_id: String,
    call_id: Option<String>,
    name: Option<String>,
}

pub(crate) async fn emit_synthetic_responses_stream(
    logical_model: &str,
    resp: &urp::UrpResponse,
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
                        json!({
                            "item_id": item.get("id").cloned().unwrap_or(Value::Null),
                            "output_index": output_index,
                            "summary_index": 0,
                        }),
                        "delta",
                        &summary,
                        sse_max_frame_length,
                    )
                    .await?;
                    send_responses_event(
                        &tx,
                        &mut seq,
                        "response.reasoning_summary_text.done",
                        json!({
                            "item_id": item.get("id").cloned().unwrap_or(Value::Null),
                            "output_index": output_index,
                            "summary_index": 0,
                            "text": summary,
                        }),
                    )
                    .await?;
                }
                if !text.is_empty() {
                    send_responses_delta_string(
                        &tx,
                        &mut seq,
                        "response.reasoning.delta",
                        json!({
                            "item_id": item.get("id").cloned().unwrap_or(Value::Null),
                            "output_index": output_index,
                        }),
                        "delta",
                        &text,
                        sse_max_frame_length,
                    )
                    .await?;
                    send_responses_event(
                        &tx,
                        &mut seq,
                        "response.reasoning.done",
                        json!({
                            "item_id": item.get("id").cloned().unwrap_or(Value::Null),
                            "output_index": output_index,
                            "text": text,
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
            "function_call" => {
                let call_id = item.get("call_id").and_then(|v| v.as_str()).unwrap_or("");
                let name = item.get("name").and_then(|v| v.as_str()).unwrap_or("");
                let arguments = item.get("arguments").and_then(|v| v.as_str()).unwrap_or("");
                if !arguments.is_empty() {
                    send_responses_delta_string(
                        &tx,
                        &mut seq,
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
                    send_responses_event(
                        &tx,
                        &mut seq,
                        "response.function_call_arguments.done",
                        json!({
                            "arguments": arguments,
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
                        "part": { "type": "output_text", "text": "", "annotations": [] },
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
                        },
                    }),
                )
                .await?;
            }
            _ => {}
        }

        let done_item = sanitize_responses_output_item_for_frame_limit(item, sse_max_frame_length);
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
    let completed_response =
        sanitize_responses_completed_for_frame_limit(&encoded, sse_max_frame_length);
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

pub(crate) async fn encode_urp_stream_as_responses(
    mut rx: mpsc::Receiver<UrpStreamEvent>,
    tx: mpsc::Sender<Event>,
    logical_model: &str,
    sse_max_frame_length: Option<usize>,
) -> AppResult<()> {
    let mut seq = 1u64;
    let mut response_id = "resp".to_string();
    let mut created: Option<i64> = None;
    let mut next_output_index = 0usize;
    let mut output_indices: HashMap<u32, usize> = HashMap::new();
    let mut part_states: HashMap<u32, StreamedPartState> = HashMap::new();
    let mut pending_assistant_items: HashMap<u32, PendingResponsesAssistantItem> = HashMap::new();

    while let Some(event) = rx.recv().await {
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
            UrpStreamEvent::ItemStart {
                item_index,
                header,
                extra_body,
            } => match header {
                ItemHeader::Message { role } => {
                    pending_assistant_items.insert(
                        item_index,
                        PendingResponsesAssistantItem {
                            role,
                            item_extra_body: extra_body,
                            active_output: None,
                            staged_outputs: Vec::new(),
                            streamed_any_output: false,
                        },
                    );
                }
                ItemHeader::ToolResult { .. } => {
                    let output_index = next_output_index;
                    next_output_index += 1;
                    output_indices.insert(item_index, output_index);
                    let item = encode_item_start_stub(&header);
                    send_responses_event(
                        &tx,
                        &mut seq,
                        "response.output_item.added",
                        json!({
                            "output_index": output_index,
                            "item": item,
                        }),
                    )
                    .await?;
                }
            },
            UrpStreamEvent::PartStart {
                part_index,
                item_index,
                header,
                extra_body,
            } => {
                if let Some(pending_item) = pending_assistant_items.get_mut(&item_index) {
                    let zone = zone_from_part_header(&header);
                    let content_index = if zone == ResponsesOutputZone::Message {
                        let next = pending_item
                            .active_output
                            .as_ref()
                            .filter(|active| active.zone == zone)
                            .map(|active| active.next_content_index)
                            .unwrap_or(0);
                        Some(next)
                    } else {
                        None
                    };
                    let needs_new_output =
                        pending_item.active_output.as_ref().is_none_or(|active| {
                            active.zone != zone || matches!(zone, ResponsesOutputZone::FunctionCall)
                        });

                    if needs_new_output {
                        stage_active_stream_output(
                            &mut pending_item.active_output,
                            &mut pending_item.staged_outputs,
                        );
                    }

                    let active_output = pending_item.active_output.get_or_insert_with(|| {
                        let output_index = next_output_index;
                        next_output_index += 1;
                        let item = stream_output_item_start_stub(
                            zone,
                            pending_item.role,
                            &pending_item.item_extra_body,
                            &header,
                            &extra_body,
                        );
                        ActiveResponsesOutputItem {
                            zone,
                            output_index,
                            item_id: item
                                .get("id")
                                .and_then(Value::as_str)
                                .unwrap_or_default()
                                .to_string(),
                            item,
                            next_content_index: 0,
                            summary_part_added_sent: false,
                        }
                    });

                    if needs_new_output {
                        pending_item.streamed_any_output = true;
                        send_responses_event(
                            &tx,
                            &mut seq,
                            "response.output_item.added",
                            json!({
                                "output_index": active_output.output_index,
                                "item": active_output.item.clone(),
                            }),
                        )
                        .await?;
                    }

                    if zone == ResponsesOutputZone::Message {
                        active_output.next_content_index += 1;
                        send_responses_event(
                            &tx,
                            &mut seq,
                            "response.content_part.added",
                            json!({
                                "output_index": active_output.output_index,
                                "content_index": content_index.unwrap_or(0),
                                "item_id": active_output.item_id,
                                "part": encode_part_start_header(&header),
                            }),
                        )
                        .await?;
                    }

                    part_states.insert(
                        part_index,
                        StreamedPartState {
                            item_index,
                            output_index: active_output.output_index,
                            zone,
                            content_index: content_index.map(|v| v as u32),
                            item_id: active_output.item_id.clone(),
                            call_id: part_call_id_from_header(&header),
                            name: part_name_from_header(&header),
                        },
                    );
                }
            }
            UrpStreamEvent::Delta {
                part_index, delta, ..
            } => match delta {
                PartDelta::Text { ref content } => {
                    let part_state =
                        part_states
                            .get(&part_index)
                            .cloned()
                            .unwrap_or(StreamedPartState {
                                item_index: 0,
                                output_index: 0,
                                zone: ResponsesOutputZone::Message,
                                content_index: Some(part_index),
                                item_id: String::new(),
                                call_id: None,
                                name: None,
                            });
                    if let Some(active_output) = pending_assistant_items
                        .get_mut(&part_state.item_index)
                        .and_then(|pending| stream_output_mut(pending, part_state.output_index))
                    {
                        append_delta_to_stream_output_item(active_output, &delta);
                    }
                    send_responses_delta_string(
                        &tx,
                        &mut seq,
                        "response.output_text.delta",
                        responses_text_delta_payload(
                            None,
                            &json!({ "id": part_state.item_id }),
                            part_state.output_index as u64,
                            part_state.content_index.unwrap_or(part_index) as u64,
                        ),
                        "delta",
                        content,
                        sse_max_frame_length,
                    )
                    .await?;
                }
                PartDelta::Reasoning {
                    ref content,
                    ref encrypted,
                    ref summary,
                    ref source,
                } => {
                    let part_state =
                        part_states
                            .get(&part_index)
                            .cloned()
                            .unwrap_or(StreamedPartState {
                                item_index: 0,
                                output_index: 0,
                                zone: ResponsesOutputZone::Reasoning,
                                content_index: None,
                                item_id: String::new(),
                                call_id: None,
                                name: None,
                            });
                    if let Some(active_output) = pending_assistant_items
                        .get_mut(&part_state.item_index)
                        .and_then(|pending| stream_output_mut(pending, part_state.output_index))
                    {
                        append_delta_to_stream_output_item(active_output, &delta);
                    }
                    if let Some(content) = content.as_deref().filter(|content| !content.is_empty())
                    {
                        send_responses_delta_string(
                            &tx,
                            &mut seq,
                            "response.reasoning.delta",
                            json!({
                                "item_id": part_state.item_id,
                                "output_index": part_state.output_index,
                            }),
                            "delta",
                            content,
                            sse_max_frame_length,
                        )
                        .await?;
                    }
                    if let Some(summary) = summary.as_deref().filter(|summary| !summary.is_empty())
                    {
                        let should_emit_added = pending_assistant_items
                            .get_mut(&part_state.item_index)
                            .and_then(|pending| stream_output_mut(pending, part_state.output_index))
                            .map(|output| {
                                if output.summary_part_added_sent {
                                    false
                                } else {
                                    output.summary_part_added_sent = true;
                                    true
                                }
                            })
                            .unwrap_or(true);
                        if should_emit_added {
                            send_responses_event(
                                &tx,
                                &mut seq,
                                "response.reasoning_summary_part.added",
                                json!({
                                    "item_id": part_state.item_id,
                                    "output_index": part_state.output_index,
                                    "summary_index": 0,
                                    "part": { "type": "summary_text", "text": "" },
                                }),
                            )
                            .await?;
                        }
                        send_responses_delta_string(
                            &tx,
                            &mut seq,
                            "response.reasoning_summary_text.delta",
                            json!({
                                "item_id": part_state.item_id,
                                "output_index": part_state.output_index,
                                "summary_index": 0,
                            }),
                            "delta",
                            summary,
                            sse_max_frame_length,
                        )
                        .await?;
                    }
                    let _ = encrypted;
                    let _ = source;
                }
                PartDelta::ToolCallArguments { ref arguments } => {
                    let part_state =
                        part_states
                            .get(&part_index)
                            .cloned()
                            .unwrap_or(StreamedPartState {
                                item_index: 0,
                                output_index: 0,
                                zone: ResponsesOutputZone::FunctionCall,
                                content_index: None,
                                item_id: String::new(),
                                call_id: Some(String::new()),
                                name: Some(String::new()),
                            });
                    if let Some(active_output) = pending_assistant_items
                        .get_mut(&part_state.item_index)
                        .and_then(|pending| stream_output_mut(pending, part_state.output_index))
                    {
                        append_delta_to_stream_output_item(active_output, &delta);
                    }
                    send_responses_delta_string(
                        &tx,
                        &mut seq,
                        "response.function_call_arguments.delta",
                        json!({
                            "item_id": part_state.item_id,
                            "output_index": part_state.output_index,
                        }),
                        "delta",
                        arguments,
                        sse_max_frame_length,
                    )
                    .await?;
                }
                PartDelta::Refusal { ref content } => {
                    let part_state =
                        part_states
                            .get(&part_index)
                            .cloned()
                            .unwrap_or(StreamedPartState {
                                item_index: 0,
                                output_index: 0,
                                zone: ResponsesOutputZone::Message,
                                content_index: Some(part_index),
                                item_id: String::new(),
                                call_id: None,
                                name: None,
                            });
                    if let Some(active_output) = pending_assistant_items
                        .get_mut(&part_state.item_index)
                        .and_then(|pending| pending.active_output.as_mut())
                    {
                        append_delta_to_stream_output_item(active_output, &delta);
                    }
                    send_responses_delta_string(
                        &tx,
                        &mut seq,
                        "response.output_text.delta",
                        json!({
                            "item_id": part_state.item_id,
                            "output_index": part_state.output_index,
                            "content_index": part_state.content_index.unwrap_or(part_index),
                            "logprobs": Value::Null,
                            "type": "refusal"
                        }),
                        "delta",
                        content,
                        sse_max_frame_length,
                    )
                    .await?;
                }
                PartDelta::Image { .. }
                | PartDelta::Audio { .. }
                | PartDelta::File { .. }
                | PartDelta::ProviderItem { .. } => {}
            },
            UrpStreamEvent::PartDone {
                part_index, part, ..
            } => {
                if let Some(part_state) = part_states.get(&part_index).cloned() {
                    let is_staged_message = pending_assistant_items
                        .get(&part_state.item_index)
                        .is_some_and(|pending| is_staged_output(pending, part_state.output_index));

                    if let Some(active_output) = pending_assistant_items
                        .get_mut(&part_state.item_index)
                        .and_then(|pending| stream_output_mut(pending, part_state.output_index))
                    {
                        apply_part_done_to_stream_output_item(active_output, &part);
                    }

                    if part_state.zone == ResponsesOutputZone::Message {
                        if matches!(part, Part::Text { .. }) {
                            let mut done_payload = responses_text_delta_payload(
                                None,
                                &json!({ "id": part_state.item_id }),
                                part_state.output_index as u64,
                                part_state.content_index.unwrap_or(part_index) as u64,
                            );
                            if let Some(obj) = done_payload.as_object_mut() {
                                obj.insert(
                                    "text".to_string(),
                                    json!(match &part {
                                        Part::Text { content, .. } => content,
                                        _ => "",
                                    }),
                                );
                            }
                            send_responses_event(
                                &tx,
                                &mut seq,
                                "response.output_text.done",
                                done_payload,
                            )
                            .await?;
                        }
                        send_responses_event(
                            &tx,
                            &mut seq,
                            "response.content_part.done",
                            json!({
                                "output_index": part_state.output_index,
                                "content_index": part_state.content_index.unwrap_or(part_index),
                                "item_id": part_state.item_id,
                                "part": encode_part_value(&part),
                            }),
                        )
                        .await?;
                        if is_staged_message
                            && let Some(output) = pending_assistant_items
                                .get_mut(&part_state.item_index)
                                .and_then(|pending| {
                                    take_stream_output(pending, part_state.output_index)
                                })
                        {
                            flush_stream_output(tx.clone(), &mut seq, output, sse_max_frame_length)
                                .await?;
                        }
                    } else if part_state.zone == ResponsesOutputZone::FunctionCall {
                        if let Part::ToolCall { arguments, .. } = &part {
                            send_responses_event(
                                &tx,
                                &mut seq,
                                "response.function_call_arguments.done",
                                json!({
                                    "arguments": arguments,
                                    "call_id": part_state.call_id.clone().unwrap_or_default(),
                                    "item_id": part_state.item_id,
                                    "name": part_state.name.clone().unwrap_or_default(),
                                    "output_index": part_state.output_index,
                                }),
                            )
                            .await?;
                        }
                        if let Some(output) = pending_assistant_items
                            .get_mut(&part_state.item_index)
                            .and_then(|pending| {
                                take_stream_output(pending, part_state.output_index)
                            })
                        {
                            flush_stream_output(tx.clone(), &mut seq, output, sse_max_frame_length)
                                .await?;
                        }
                    } else if part_state.zone == ResponsesOutputZone::Reasoning {
                        if let Part::Reasoning {
                            content, summary, ..
                        } = &part
                        {
                            if let Some(summary) =
                                summary.as_deref().filter(|summary| !summary.is_empty())
                            {
                                send_responses_event(
                                    &tx,
                                    &mut seq,
                                    "response.reasoning_summary_text.done",
                                    json!({
                                        "item_id": part_state.item_id,
                                        "output_index": part_state.output_index,
                                        "summary_index": 0,
                                        "text": summary,
                                    }),
                                )
                                .await?;
                                send_responses_event(
                                    &tx,
                                    &mut seq,
                                    "response.reasoning_summary_part.done",
                                    json!({
                                        "item_id": part_state.item_id,
                                        "output_index": part_state.output_index,
                                        "summary_index": 0,
                                        "part": { "type": "summary_text", "text": summary },
                                    }),
                                )
                                .await?;
                            }
                            if content
                                .as_deref()
                                .is_some_and(|content| !content.is_empty())
                            {
                                send_responses_event(
                                    &tx,
                                    &mut seq,
                                    "response.reasoning.done",
                                    json!({
                                        "item_id": part_state.item_id,
                                        "output_index": part_state.output_index,
                                        "text": content,
                                    }),
                                )
                                .await?;
                            }
                        }
                        if let Some(output) = pending_assistant_items
                            .get_mut(&part_state.item_index)
                            .and_then(|pending| {
                                take_stream_output(pending, part_state.output_index)
                            })
                        {
                            flush_stream_output(tx.clone(), &mut seq, output, sse_max_frame_length)
                                .await?;
                        }
                    }
                }
            }
            UrpStreamEvent::ItemDone {
                item_index, item, ..
            } => {
                if let Some(mut pending_item) = pending_assistant_items.remove(&item_index) {
                    if pending_item.active_output.is_some()
                        || !pending_item.staged_outputs.is_empty()
                    {
                        flush_pending_stream_outputs(
                            tx.clone(),
                            &mut seq,
                            &mut pending_item,
                            sse_max_frame_length,
                        )
                        .await?;
                    } else if !pending_item.streamed_any_output {
                        for encoded_item in encode_stream_output_item(&item) {
                            let output_index = next_output_index;
                            next_output_index += 1;
                            send_responses_event(
                                &tx,
                                &mut seq,
                                "response.output_item.added",
                                json!({
                                    "output_index": output_index,
                                    "item": mark_stream_output_item_in_progress(&encoded_item),
                                }),
                            )
                            .await?;
                            let done_item = sanitize_responses_output_item_for_frame_limit(
                                &encoded_item,
                                sse_max_frame_length,
                            );
                            send_responses_event(
                                &tx,
                                &mut seq,
                                "response.output_item.done",
                                json!({
                                    "output_index": output_index,
                                    "item": done_item,
                                }),
                            )
                            .await?;
                        }
                    }
                } else {
                    let output_index = *output_indices.entry(item_index).or_insert_with(|| {
                        let output_index = next_output_index;
                        next_output_index += 1;
                        output_index
                    });
                    let done_item = sanitize_responses_output_item_for_frame_limit(
                        &encode_stream_output_item(&item)
                            .into_iter()
                            .next()
                            .unwrap_or_else(|| {
                                encode_item_start_stub(&item_header_from_item(&item))
                            }),
                        sse_max_frame_length,
                    );
                    send_responses_event(
                        &tx,
                        &mut seq,
                        "response.output_item.done",
                        json!({
                            "output_index": output_index,
                            "item": done_item,
                        }),
                    )
                    .await?;
                }
            }
            UrpStreamEvent::ResponseDone {
                finish_reason,
                usage,
                outputs,
                ..
            } => {
                let mut response = urp::encode::openai_responses::encode_response(
                    &urp::UrpResponse {
                        id: response_id.clone(),
                        model: logical_model.to_string(),
                        outputs,
                        finish_reason,
                        usage,
                        extra_body: HashMap::new(),
                    },
                    logical_model,
                );
                if let Some(created) = created {
                    response["created_at"] = json!(created);
                }
                let completed_response =
                    sanitize_responses_completed_for_frame_limit(&response, sse_max_frame_length);
                send_responses_event(
                    &tx,
                    &mut seq,
                    "response.completed",
                    json!({ "response": completed_response }),
                )
                .await?;
                send_plain_sse_data(&tx, "[DONE]".to_string()).await?;
            }
            UrpStreamEvent::Error { code, message, .. } => {
                send_responses_event(
                    &tx,
                    &mut seq,
                    "error",
                    json!({
                        "code": code,
                        "message": message,
                    }),
                )
                .await?;
            }
        }
    }

    Ok(())
}

fn encode_item_start_stub(header: &ItemHeader) -> Value {
    match header {
        ItemHeader::Message { role } => json!({
            "type": "message",
            "role": role_to_str(*role),
            "content": [],
            "id": format!("msg_{}", uuid::Uuid::new_v4()),
            "status": "in_progress",
        }),
        ItemHeader::ToolResult { call_id } => json!({
            "type": "tool_result",
            "call_id": call_id,
            "output": "",
            "id": format!("tr_{}", uuid::Uuid::new_v4()),
            "status": "in_progress",
        }),
    }
}

fn item_header_from_item(item: &Item) -> ItemHeader {
    match item {
        Item::Message { role, .. } => ItemHeader::Message { role: *role },
        Item::ToolResult { call_id, .. } => ItemHeader::ToolResult {
            call_id: call_id.clone(),
        },
    }
}

fn zone_from_part_header(header: &PartHeader) -> ResponsesOutputZone {
    match header {
        PartHeader::Reasoning => ResponsesOutputZone::Reasoning,
        PartHeader::ToolCall { .. } => ResponsesOutputZone::FunctionCall,
        _ => ResponsesOutputZone::Message,
    }
}

fn part_call_id_from_header(header: &PartHeader) -> Option<String> {
    match header {
        PartHeader::ToolCall { call_id, .. } => Some(call_id.clone()),
        _ => None,
    }
}

fn part_name_from_header(header: &PartHeader) -> Option<String> {
    match header {
        PartHeader::ToolCall { name, .. } => Some(name.clone()),
        _ => None,
    }
}

fn stream_output_item_start_stub(
    zone: ResponsesOutputZone,
    role: Role,
    item_extra_body: &HashMap<String, Value>,
    header: &PartHeader,
    part_extra_body: &HashMap<String, Value>,
) -> Value {
    match zone {
        ResponsesOutputZone::Message => {
            let mut obj = Map::new();
            obj.insert("type".to_string(), json!("message"));
            obj.insert("role".to_string(), json!(role_to_str(role)));
            obj.insert("content".to_string(), json!([]));
            obj.insert(
                "id".to_string(),
                json!(format!("msg_{}", uuid::Uuid::new_v4())),
            );
            obj.insert("status".to_string(), json!("in_progress"));
            if let Some(phase) = part_extra_body
                .get("phase")
                .and_then(|value| value.as_str())
            {
                obj.insert("phase".to_string(), json!(phase));
            }
            merge_json_extra(&mut obj, item_extra_body);
            Value::Object(obj)
        }
        ResponsesOutputZone::Reasoning => {
            let mut obj = Map::new();
            obj.insert("type".to_string(), json!("reasoning"));
            obj.insert(
                "id".to_string(),
                json!(format!("rs_{}", uuid::Uuid::new_v4())),
            );
            obj.insert("status".to_string(), json!("in_progress"));
            if let PartHeader::ProviderItem { body, .. } = header {
                merge_json_extra_value(&mut obj, body);
            }
            merge_json_extra(&mut obj, part_extra_body);
            Value::Object(obj)
        }
        ResponsesOutputZone::FunctionCall => {
            let PartHeader::ToolCall { call_id, name } = header else {
                unreachable!("function-call zone requires tool-call header");
            };
            let mut obj = Map::new();
            obj.insert("type".to_string(), json!("function_call"));
            obj.insert("call_id".to_string(), json!(call_id));
            obj.insert("name".to_string(), json!(name));
            obj.insert("arguments".to_string(), json!(""));
            obj.insert(
                "id".to_string(),
                json!(format!("fc_{}", uuid::Uuid::new_v4())),
            );
            obj.insert("status".to_string(), json!("in_progress"));
            merge_json_extra(&mut obj, part_extra_body);
            Value::Object(obj)
        }
    }
}

fn merge_json_extra_value(obj: &mut Map<String, Value>, value: &Value) {
    if let Some(map) = value.as_object() {
        for (key, value) in map {
            obj.entry(key.clone()).or_insert_with(|| value.clone());
        }
    }
}

fn append_delta_to_stream_output_item(
    active_output: &mut ActiveResponsesOutputItem,
    delta: &PartDelta,
) {
    match (&active_output.zone, delta) {
        (ResponsesOutputZone::Message, PartDelta::Text { content }) => {
            append_string_field_to_message_content(
                &mut active_output.item,
                "output_text",
                "text",
                content,
            );
        }
        (ResponsesOutputZone::Message, PartDelta::Refusal { content }) => {
            append_string_field_to_message_content(
                &mut active_output.item,
                "refusal",
                "refusal",
                content,
            );
        }
        (
            ResponsesOutputZone::Reasoning,
            PartDelta::Reasoning {
                content,
                encrypted,
                summary,
                source,
            },
        ) => {
            if let Some(content) = content.as_deref().filter(|content| !content.is_empty()) {
                append_string_field(&mut active_output.item, "text", content);
            }
            if let Some(summary) = summary.as_deref().filter(|summary| !summary.is_empty()) {
                append_reasoning_summary_field(&mut active_output.item, summary);
            }
            if let Some(encrypted) = encrypted.as_ref().filter(|encrypted| !encrypted.is_null()) {
                let Some(obj) = active_output.item.as_object_mut() else {
                    return;
                };
                obj.insert("encrypted_content".to_string(), encrypted.clone());
            }
            if let Some(source) = source.as_deref().filter(|source| !source.is_empty()) {
                let Some(obj) = active_output.item.as_object_mut() else {
                    return;
                };
                obj.insert("source".to_string(), Value::String(source.to_string()));
            }
        }
        (ResponsesOutputZone::FunctionCall, PartDelta::ToolCallArguments { arguments }) => {
            append_string_field(&mut active_output.item, "arguments", arguments);
        }
        _ => {}
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

fn apply_part_done_to_stream_output_item(
    active_output: &mut ActiveResponsesOutputItem,
    part: &Part,
) {
    match active_output.zone {
        ResponsesOutputZone::Message => {
            if let Some(content) = active_output
                .item
                .get_mut("content")
                .and_then(Value::as_array_mut)
            {
                let encoded_part = encode_part_value(part);
                let encoded_type = encoded_part
                    .get("type")
                    .and_then(Value::as_str)
                    .unwrap_or_default()
                    .to_string();
                let matches_last_type = content.last().is_some_and(|last| {
                    last.get("type").and_then(Value::as_str) == Some(encoded_type.as_str())
                });
                if content.is_empty() {
                    content.push(encode_part_value(part));
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
            if let Some(reasoning_item) = encode_reasoning_output_item(part) {
                let mut item = mark_stream_output_item_in_progress(&reasoning_item);
                if let Some(existing_id) = active_output.item.get("id").cloned()
                    && let Some(obj) = item.as_object_mut()
                {
                    obj.insert("id".to_string(), existing_id);
                }
                active_output.item = item;
            }
        }
        ResponsesOutputZone::FunctionCall => {
            if let Some(function_item) = encode_function_call_output_item(part) {
                let mut item = mark_stream_output_item_in_progress(&function_item);
                if let Some(existing_id) = active_output.item.get("id").cloned()
                    && let Some(obj) = item.as_object_mut()
                {
                    obj.insert("id".to_string(), existing_id);
                }
                active_output.item = item;
            }
        }
    }
}

fn mark_stream_output_item_in_progress(item: &Value) -> Value {
    let mut item = item.clone();
    if let Some(obj) = item.as_object_mut() {
        obj.insert("status".to_string(), json!("in_progress"));
    }
    item
}

fn stage_active_stream_output(
    active_output: &mut Option<ActiveResponsesOutputItem>,
    staged_outputs: &mut Vec<ActiveResponsesOutputItem>,
) {
    if let Some(active_output) = active_output.take() {
        staged_outputs.push(active_output);
    }
}

fn stream_output_mut(
    pending: &mut PendingResponsesAssistantItem,
    output_index: usize,
) -> Option<&mut ActiveResponsesOutputItem> {
    if pending
        .active_output
        .as_ref()
        .is_some_and(|output| output.output_index == output_index)
    {
        return pending.active_output.as_mut();
    }
    pending
        .staged_outputs
        .iter_mut()
        .find(|output| output.output_index == output_index)
}

fn is_staged_output(pending: &PendingResponsesAssistantItem, output_index: usize) -> bool {
    pending
        .staged_outputs
        .iter()
        .any(|output| output.output_index == output_index)
}

fn take_stream_output(
    pending: &mut PendingResponsesAssistantItem,
    output_index: usize,
) -> Option<ActiveResponsesOutputItem> {
    if pending
        .active_output
        .as_ref()
        .is_some_and(|output| output.output_index == output_index)
    {
        return pending.active_output.take();
    }
    let idx = pending
        .staged_outputs
        .iter()
        .position(|output| output.output_index == output_index)?;
    Some(pending.staged_outputs.remove(idx))
}

async fn flush_stream_output(
    tx: mpsc::Sender<Event>,
    seq: &mut u64,
    active_output: ActiveResponsesOutputItem,
    sse_max_frame_length: Option<usize>,
) -> AppResult<()> {
    let completed_item = complete_stream_output_item(active_output.item);
    let done_item =
        sanitize_responses_output_item_for_frame_limit(&completed_item, sse_max_frame_length);
    send_responses_event(
        &tx,
        seq,
        "response.output_item.done",
        json!({
            "output_index": active_output.output_index,
            "item": done_item,
        }),
    )
    .await
}

async fn flush_pending_stream_outputs(
    tx: mpsc::Sender<Event>,
    seq: &mut u64,
    pending_item: &mut PendingResponsesAssistantItem,
    sse_max_frame_length: Option<usize>,
) -> AppResult<()> {
    while let Some(output) = pending_item.staged_outputs.pop() {
        flush_stream_output(tx.clone(), seq, output, sse_max_frame_length).await?;
    }
    if let Some(output) = pending_item.active_output.take() {
        flush_stream_output(tx, seq, output, sse_max_frame_length).await?;
    }
    Ok(())
}

fn response_envelope_payload(
    id: &str,
    created_at: i64,
    model: &str,
    status: &str,
    output: Value,
) -> Value {
    json!({
        "response": {
            "id": id,
            "object": "response",
            "created_at": created_at,
            "model": model,
            "status": status,
            "output": output,
        }
    })
}

fn complete_stream_output_item(mut item: Value) -> Value {
    if let Some(obj) = item.as_object_mut() {
        obj.insert("status".to_string(), json!("completed"));
    }
    item
}

fn text_part_phase(part: &Part) -> Option<&str> {
    match part {
        Part::Text { extra_body, .. } => extra_body.get("phase").and_then(|v| v.as_str()),
        _ => None,
    }
}

fn flush_pending_message_item(
    pending: &mut Option<PendingResponsesMessageItem>,
    out: &mut Vec<Value>,
) {
    let Some(pending_item) = pending.take() else {
        return;
    };
    if pending_item.content.is_empty() {
        return;
    }

    let mut obj = Map::new();
    obj.insert("type".to_string(), json!("message"));
    obj.insert("role".to_string(), json!(role_to_str(pending_item.role)));
    obj.insert("content".to_string(), Value::Array(pending_item.content));
    obj.insert(
        "id".to_string(),
        json!(format!("msg_{}", uuid::Uuid::new_v4())),
    );
    obj.insert("status".to_string(), json!("completed"));
    if let Some(phase) = pending_item.phase {
        obj.insert("phase".to_string(), Value::String(phase));
    }
    merge_json_extra(&mut obj, &pending_item.extra_body);
    out.push(Value::Object(obj));
}

fn append_content_part_to_pending(
    pending: &mut Option<PendingResponsesMessageItem>,
    out: &mut Vec<Value>,
    role: Role,
    phase: Option<&str>,
    message_extra: &HashMap<String, Value>,
    content_part: Value,
) {
    let phase_owned = phase.map(str::to_string);
    let should_flush = pending.as_ref().is_some_and(|existing| {
        existing.role != role
            || existing.phase != phase_owned
            || existing.extra_body != *message_extra
    });
    if should_flush {
        flush_pending_message_item(pending, out);
    }

    let entry = pending.get_or_insert_with(|| PendingResponsesMessageItem {
        role,
        phase: phase_owned,
        content: Vec::new(),
        extra_body: message_extra.clone(),
    });
    entry.content.push(content_part);
}

fn encode_message_content_part(part: &Part) -> Option<Value> {
    match part {
        Part::Text {
            content,
            extra_body,
        } => {
            let mut obj = Map::new();
            obj.insert("type".to_string(), json!("output_text"));
            obj.insert("text".to_string(), json!(content));
            merge_json_extra(&mut obj, extra_body);
            Some(Value::Object(obj))
        }
        Part::Image { source, extra_body } => Some(encode_image_part(source, extra_body)),
        Part::File { source, extra_body } => Some(encode_file_part(source, extra_body)),
        Part::Refusal {
            content,
            extra_body,
        } => {
            let mut obj = Map::new();
            obj.insert("type".to_string(), json!("refusal"));
            obj.insert("refusal".to_string(), json!(content));
            merge_json_extra(&mut obj, extra_body);
            Some(Value::Object(obj))
        }
        _ => None,
    }
}

fn encode_stream_output_item(item: &Item) -> Vec<Value> {
    match item {
        Item::Message {
            role,
            parts,
            extra_body,
        } => {
            let mut output = Vec::new();
            let mut pending_message: Option<PendingResponsesMessageItem> = None;

            for part in parts {
                if let Some(content_part) = encode_message_content_part(part) {
                    append_content_part_to_pending(
                        &mut pending_message,
                        &mut output,
                        *role,
                        text_part_phase(part),
                        extra_body,
                        content_part,
                    );
                    continue;
                }

                flush_pending_message_item(&mut pending_message, &mut output);

                if let Some(reasoning_item) = encode_reasoning_output_item(part) {
                    output.push(reasoning_item);
                    continue;
                }
                if let Some(tool_call_item) = encode_function_call_output_item(part) {
                    output.push(tool_call_item);
                    continue;
                }
            }

            flush_pending_message_item(&mut pending_message, &mut output);
            output
        }
        Item::ToolResult {
            call_id,
            content,
            is_error,
            extra_body,
        } => {
            let mut obj = Map::new();
            obj.insert("type".to_string(), json!("function_call_output"));
            obj.insert("call_id".to_string(), json!(call_id));
            obj.insert(
                "id".to_string(),
                json!(format!("tr_{}", uuid::Uuid::new_v4())),
            );
            obj.insert("status".to_string(), json!("completed"));
            obj.insert("output".to_string(), encode_tool_result_output(content));
            if *is_error {
                obj.insert("is_error".to_string(), Value::Bool(true));
            }
            merge_json_extra(&mut obj, extra_body);
            vec![Value::Object(obj)]
        }
    }
}

fn encode_reasoning_output_item(part: &Part) -> Option<Value> {
    let Part::Reasoning {
        content,
        encrypted,
        summary,
        source,
        extra_body,
    } = part
    else {
        return None;
    };

    let mut obj = Map::new();
    obj.insert("type".to_string(), json!("reasoning"));
    if let Some(text) = summary.as_ref() {
        obj.insert(
            "summary".to_string(),
            Value::Array(vec![json!({ "type": "summary_text", "text": text })]),
        );
    }
    if let Some(text) = content {
        obj.insert("text".to_string(), json!(text));
    }
    if let Some(encrypted) = encrypted {
        obj.insert("encrypted_content".to_string(), encrypted.clone());
    }
    if let Some(source) = source {
        obj.insert("source".to_string(), json!(source));
    }
    merge_json_extra(&mut obj, extra_body);
    Some(Value::Object(obj))
}

fn encode_function_call_output_item(part: &Part) -> Option<Value> {
    let Part::ToolCall {
        call_id,
        name,
        arguments,
        extra_body,
    } = part
    else {
        return None;
    };

    let mut obj = Map::new();
    obj.insert("type".to_string(), json!("function_call"));
    obj.insert("call_id".to_string(), json!(call_id));
    obj.insert("name".to_string(), json!(name));
    obj.insert("arguments".to_string(), json!(arguments));
    obj.insert(
        "id".to_string(),
        json!(format!("fc_{}", uuid::Uuid::new_v4())),
    );
    obj.insert("status".to_string(), json!("completed"));
    merge_json_extra(&mut obj, extra_body);
    Some(Value::Object(obj))
}

fn encode_part_start_header(header: &PartHeader) -> Value {
    match header {
        PartHeader::Text => json!({ "type": "output_text", "text": "", "annotations": [] }),
        PartHeader::Reasoning => json!({ "type": "reasoning", "text": "" }),
        PartHeader::Refusal => json!({ "type": "refusal", "refusal": "" }),
        PartHeader::ToolCall { call_id, name } => json!({
            "type": "function_call",
            "call_id": call_id,
            "name": name,
            "arguments": "",
        }),
        PartHeader::Image { extra_body } => {
            let mut obj = Map::new();
            obj.insert("type".to_string(), json!("output_image"));
            merge_json_extra(&mut obj, extra_body);
            Value::Object(obj)
        }
        PartHeader::Audio { extra_body } => {
            let mut obj = Map::new();
            obj.insert("type".to_string(), json!("audio"));
            merge_json_extra(&mut obj, extra_body);
            Value::Object(obj)
        }
        PartHeader::File { extra_body } => {
            let mut obj = Map::new();
            obj.insert("type".to_string(), json!("output_file"));
            merge_json_extra(&mut obj, extra_body);
            Value::Object(obj)
        }
        PartHeader::ProviderItem { item_type, body } => {
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
            Value::Object(obj)
        }
    }
}

fn encode_part_value(part: &Part) -> Value {
    match part {
        Part::Text {
            content,
            extra_body,
        } => {
            let mut obj = Map::new();
            obj.insert("type".to_string(), json!("output_text"));
            obj.insert("text".to_string(), json!(content));
            merge_json_extra(&mut obj, extra_body);
            Value::Object(obj)
        }
        Part::Reasoning {
            content,
            encrypted,
            summary,
            source,
            extra_body,
        } => {
            let mut obj = Map::new();
            obj.insert("type".to_string(), json!("reasoning"));
            if let Some(text) = content {
                obj.insert("text".to_string(), json!(text));
            }
            if let Some(text) = summary {
                obj.insert(
                    "summary".to_string(),
                    Value::Array(vec![json!({ "type": "summary_text", "text": text })]),
                );
            }
            if let Some(encrypted) = encrypted {
                obj.insert("encrypted_content".to_string(), encrypted.clone());
            }
            if let Some(source) = source {
                obj.insert("source".to_string(), json!(source));
            }
            merge_json_extra(&mut obj, extra_body);
            Value::Object(obj)
        }
        Part::ToolCall {
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
            merge_json_extra(&mut obj, extra_body);
            Value::Object(obj)
        }
        Part::Refusal {
            content,
            extra_body,
        } => {
            let mut obj = Map::new();
            obj.insert("type".to_string(), json!("refusal"));
            obj.insert("refusal".to_string(), json!(content));
            merge_json_extra(&mut obj, extra_body);
            Value::Object(obj)
        }
        Part::Image { source, extra_body } => encode_image_part(source, extra_body),
        Part::Audio { source, extra_body } => {
            let mut obj = Map::new();
            obj.insert("type".to_string(), json!("audio"));
            obj.insert("source".to_string(), encode_audio_source(source));
            merge_json_extra(&mut obj, extra_body);
            Value::Object(obj)
        }
        Part::File { source, extra_body } => encode_file_part(source, extra_body),
        Part::ProviderItem {
            item_type,
            body,
            extra_body,
        } => {
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
    }
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

fn role_to_str(role: Role) -> &'static str {
    match role {
        Role::System => "system",
        Role::Developer => "developer",
        Role::User => "user",
        Role::Assistant => "assistant",
        Role::Tool => "tool",
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::urp::{FinishReason, Part, Role, UrpResponse};

    fn empty_map() -> HashMap<String, Value> {
        HashMap::new()
    }

    #[test]
    fn streamed_completion_uses_nonstream_response_output_shape_for_merged_items() {
        let outputs = vec![Item::Message {
            role: Role::Assistant,
            parts: vec![
                Part::Reasoning {
                    content: Some("think".to_string()),
                    encrypted: Some(json!("sig_1")),
                    summary: None,
                    source: None,
                    extra_body: empty_map(),
                },
                Part::Text {
                    content: "answer".to_string(),
                    extra_body: {
                        let mut map = empty_map();
                        map.insert("phase".to_string(), json!("analysis"));
                        map
                    },
                },
                Part::ToolCall {
                    call_id: "call_1".to_string(),
                    name: "lookup".to_string(),
                    arguments: "{}".to_string(),
                    extra_body: empty_map(),
                },
            ],
            extra_body: {
                let mut map = empty_map();
                map.insert("custom_message_field".to_string(), json!(true));
                map
            },
        }];

        let encoded = urp::encode::openai_responses::encode_response(
            &UrpResponse {
                id: "resp_1".to_string(),
                model: "gpt-5.4".to_string(),
                outputs,
                finish_reason: Some(FinishReason::ToolCalls),
                usage: None,
                extra_body: empty_map(),
            },
            "gpt-5.4",
        );
        let output = encoded["output"].as_array().expect("output array");
        assert_eq!(output.len(), 3);
        assert_eq!(output[0]["type"], json!("reasoning"));
        assert_eq!(output[1]["type"], json!("message"));
        assert_eq!(output[1]["phase"], json!("analysis"));
        assert_eq!(output[1]["custom_message_field"], json!(true));
        assert_eq!(output[2]["type"], json!("function_call"));
    }
}
