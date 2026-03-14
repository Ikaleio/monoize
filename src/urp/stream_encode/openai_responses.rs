use crate::error::AppResult;
use crate::handlers::routing::now_ts;
use crate::urp::{
    self, Item, ItemHeader, Part, PartDelta, PartHeader, Role, ToolResultContent,
    UrpStreamEvent,
};
use crate::urp::stream_helpers::*;
use axum::response::sse::Event;
use serde_json::{Map, Value, json};
use std::collections::HashMap;
use tokio::sync::mpsc;

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
        .get("created")
        .and_then(|v| v.as_i64())
        .unwrap_or_else(now_ts);
    let base_response = json!({
        "id": response_id,
        "object": "response",
        "created": created,
        "model": logical_model,
        "status": "in_progress",
        "output": []
    });
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
                let (text, sig) = extract_reasoning_text_and_signature(item);
                if !text.is_empty() {
                    send_responses_delta_string(
                        &tx,
                        &mut seq,
                        "response.reasoning_text.delta",
                        json!({}),
                        "delta",
                        &text,
                        sse_max_frame_length,
                    )
                    .await?;
                }
                if !sig.is_empty() {
                    send_responses_delta_string(
                        &tx,
                        &mut seq,
                        "response.reasoning_signature.delta",
                        json!({}),
                        "delta",
                        &sig,
                        sse_max_frame_length,
                    )
                    .await?;
                }
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
                            "output_index": output_index,
                            "call_id": call_id,
                            "name": name
                        }),
                        "delta",
                        arguments,
                        sse_max_frame_length,
                    )
                    .await?;
                }
            }
            "message" => {
                let text = extract_responses_message_text(item);
                if !text.is_empty() {
                    let phase = extract_responses_message_phase(item);
                    send_responses_delta_string(
                        &tx,
                        &mut seq,
                        "response.output_text.delta",
                        responses_text_delta_payload("", phase.as_deref()),
                        "delta",
                        &text,
                        sse_max_frame_length,
                    )
                    .await?;
                }
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
    send_responses_event(&tx, &mut seq, "response.output_text.done", json!({})).await?;
    let completed_response = sanitize_responses_completed_for_frame_limit(&encoded, sse_max_frame_length);
    send_responses_event(
        &tx,
        &mut seq,
        "response.completed",
        json!({ "response": completed_response }),
    )
    .await?;
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
    let mut output_indices: HashMap<u32, usize> = HashMap::new();
    let mut part_indices: HashMap<u32, (usize, u32)> = HashMap::new();
    let mut tool_calls_by_part_index: HashMap<u32, (usize, String, String)> = HashMap::new();

    while let Some(event) = rx.recv().await {
        match event {
            UrpStreamEvent::ResponseStart { id, extra_body, .. } => {
                response_id = id.clone();
                created = Some(
                    extra_body
                        .get("created")
                        .and_then(|v| v.as_i64())
                        .unwrap_or_else(now_ts),
                );

                let payload = json!({
                    "id": id,
                    "object": "response",
                    "created": created.expect("response.created timestamp set from response start"),
                    "model": logical_model,
                    "status": "in_progress",
                    "output": []
                });
                send_responses_event(&tx, &mut seq, "response.created", payload.clone()).await?;
                send_responses_event(&tx, &mut seq, "response.in_progress", payload).await?;
            }
            UrpStreamEvent::ItemStart {
                item_index,
                header,
                ..
            } => {
                let output_index = item_index as usize;
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
            UrpStreamEvent::PartStart {
                part_index,
                item_index,
                header,
                ..
            } => {
                let output_index = *output_indices
                    .entry(item_index)
                    .or_insert(item_index as usize);
                part_indices.insert(part_index, (output_index, part_index));
                if let PartHeader::ToolCall { call_id, name } = &header {
                    tool_calls_by_part_index.insert(
                        part_index,
                        (output_index, call_id.clone(), name.clone()),
                    );
                }

                send_responses_event(
                    &tx,
                    &mut seq,
                    "response.content_part.added",
                    json!({
                        "output_index": output_index,
                        "content_index": part_index,
                        "part": encode_part_start_header(&header),
                    }),
                )
                .await?;
            }
            UrpStreamEvent::Delta {
                part_index, delta, ..
            } => match delta {
                PartDelta::Text { content } => {
                    let (output_index, content_index) = part_indices
                        .get(&part_index)
                        .copied()
                        .unwrap_or((0, part_index));
                    send_responses_delta_string(
                        &tx,
                        &mut seq,
                        "response.output_text.delta",
                        responses_text_delta_payload("", None)
                            .as_object()
                            .cloned()
                            .map(Value::Object)
                            .map(|mut value| {
                                if let Some(obj) = value.as_object_mut() {
                                    obj.insert("output_index".to_string(), json!(output_index));
                                    obj.insert("content_index".to_string(), json!(content_index));
                                }
                                value
                            })
                            .unwrap_or_else(|| json!({ "output_index": output_index, "content_index": content_index })),
                        "delta",
                        &content,
                        sse_max_frame_length,
                    )
                    .await?;
                }
                PartDelta::Reasoning { content } => {
                    let (output_index, content_index) = part_indices
                        .get(&part_index)
                        .copied()
                        .unwrap_or((0, part_index));
                    send_responses_delta_string(
                        &tx,
                        &mut seq,
                        "response.reasoning_text.delta",
                        json!({
                            "output_index": output_index,
                            "content_index": content_index,
                        }),
                        "delta",
                        &content,
                        sse_max_frame_length,
                    )
                    .await?;
                }
                PartDelta::ToolCallArguments { arguments } => {
                    let (output_index, call_id, name) = tool_calls_by_part_index
                        .get(&part_index)
                        .cloned()
                        .unwrap_or_else(|| (0, String::new(), String::new()));
                    send_responses_delta_string(
                        &tx,
                        &mut seq,
                        "response.function_call_arguments.delta",
                        json!({
                            "output_index": output_index,
                            "content_index": part_index,
                            "call_id": call_id,
                            "name": name,
                        }),
                        "delta",
                        &arguments,
                        sse_max_frame_length,
                    )
                    .await?;
                }
                PartDelta::Refusal { content } => {
                    let (output_index, content_index) = part_indices
                        .get(&part_index)
                        .copied()
                        .unwrap_or((0, part_index));
                    send_responses_delta_string(
                        &tx,
                        &mut seq,
                        "response.output_text.delta",
                        json!({
                            "output_index": output_index,
                            "content_index": content_index,
                            "delta": "",
                            "type": "refusal"
                        }),
                        "delta",
                        &content,
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
                let (output_index, content_index) = part_indices
                    .get(&part_index)
                    .copied()
                    .unwrap_or((0, part_index));
                send_responses_event(
                    &tx,
                    &mut seq,
                    "response.content_part.done",
                    json!({
                        "output_index": output_index,
                        "content_index": content_index,
                        "part": encode_part_value(&part),
                    }),
                )
                .await?;
            }
            UrpStreamEvent::ItemDone {
                item_index, item, ..
            } => {
                let output_index = *output_indices
                    .entry(item_index)
                    .or_insert(item_index as usize);
                let encoded_item = sanitize_responses_output_item_for_frame_limit(
                    &encode_stream_output_item(&item),
                    sse_max_frame_length,
                );
                send_responses_event(
                    &tx,
                    &mut seq,
                    "response.output_item.done",
                    json!({
                        "output_index": output_index,
                        "item": encoded_item,
                    }),
                )
                .await?;
            }
            UrpStreamEvent::ResponseDone {
                finish_reason,
                usage,
                outputs,
                ..
            } => {
                send_responses_event(&tx, &mut seq, "response.output_text.done", json!({})).await?;
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
                    response["created"] = json!(created);
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

fn encode_stream_output_item(item: &Item) -> Value {
    match item {
        Item::Message {
            role,
            parts,
            extra_body,
        } => {
            if parts.len() == 1 {
                if let Some(reasoning) = encode_reasoning_output_item(&parts[0]) {
                    return reasoning;
                }
                if let Some(tool_call) = encode_function_call_output_item(&parts[0]) {
                    return tool_call;
                }
            }

            let mut obj = Map::new();
            obj.insert("type".to_string(), json!("message"));
            obj.insert("role".to_string(), json!(role_to_str(*role)));
            obj.insert(
                "content".to_string(),
                Value::Array(parts.iter().map(encode_part_value).collect()),
            );
            obj.insert(
                "id".to_string(),
                json!(format!("msg_{}", uuid::Uuid::new_v4())),
            );
            obj.insert("status".to_string(), json!("completed"));
            merge_json_extra(&mut obj, extra_body);
            Value::Object(obj)
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
            Value::Object(obj)
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
    if let Some(text) = summary.as_ref().or(content.as_ref()) {
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
        PartHeader::Text => json!({ "type": "output_text", "text": "" }),
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

fn encode_file_part(
    source: &crate::urp::FileSource,
    extra_body: &HashMap<String, Value>,
) -> Value {
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
