use crate::error::{AppError, AppResult};
use axum::http::StatusCode;
use axum::response::sse::Event;
use serde_json::{Map, Value, json};
use tokio::sync::mpsc;

pub(crate) async fn send_plain_sse_data(tx: &mpsc::Sender<Event>, data: String) -> AppResult<()> {
    tx.send(Event::default().data(data))
        .await
        .map_err(|e| AppError::new(StatusCode::BAD_GATEWAY, "stream_send_failed", e.to_string()))
}

pub(crate) async fn send_named_sse_json(
    tx: &mpsc::Sender<Event>,
    name: &str,
    data: Value,
) -> AppResult<()> {
    tx.send(Event::default().event(name).data(data.to_string()))
        .await
        .map_err(|e| AppError::new(StatusCode::BAD_GATEWAY, "stream_send_failed", e.to_string()))
}

pub(crate) async fn send_responses_event(
    tx: &mpsc::Sender<Event>,
    seq: &mut u64,
    name: &str,
    data: Value,
) -> AppResult<()> {
    tx.send(wrap_responses_event(seq, name, data))
        .await
        .map_err(|e| AppError::new(StatusCode::BAD_GATEWAY, "stream_send_failed", e.to_string()))
}

pub(crate) async fn send_responses_delta_string(
    tx: &mpsc::Sender<Event>,
    seq: &mut u64,
    name: &str,
    template: Value,
    field: &str,
    content: &str,
    max_frame_length: Option<usize>,
) -> AppResult<()> {
    for part in split_wrapped_responses_json_string_field(
        *seq,
        name,
        template,
        field,
        content,
        max_frame_length,
    ) {
        send_responses_event(tx, seq, name, part).await?;
    }
    Ok(())
}

pub(crate) async fn send_chat_chunk_string(
    tx: &mpsc::Sender<Event>,
    id: &str,
    created: i64,
    logical_model: &str,
    delta_template: Value,
    content: &str,
    patch: fn(&mut Value, &str),
    max_frame_length: Option<usize>,
) -> AppResult<()> {
    let base = json!({
        "id": id,
        "object": "chat.completion.chunk",
        "created": created,
        "model": logical_model,
        "choices": [{ "index": 0, "delta": delta_template, "finish_reason": Value::Null }]
    });
    for chunk in split_json_value_by_string_patch(base, content, patch, max_frame_length) {
        send_plain_sse_data(tx, chunk.to_string()).await?;
    }
    Ok(())
}

pub(crate) async fn send_messages_delta_string(
    tx: &mpsc::Sender<Event>,
    template: Value,
    patch: fn(&mut Value, &str),
    content: &str,
    max_frame_length: Option<usize>,
) -> AppResult<()> {
    let event_name = template
        .get("type")
        .and_then(Value::as_str)
        .ok_or_else(|| AppError::new(
            StatusCode::BAD_GATEWAY,
            "stream_encode_failed",
            "messages stream payload missing type field",
        ))?
        .to_string();
    for chunk in split_json_value_by_string_patch(template, content, patch, max_frame_length) {
        send_named_sse_json(tx, &event_name, chunk).await?;
    }
    Ok(())
}

pub(crate) fn split_wrapped_responses_json_string_field(
    seq: u64,
    event_name: &str,
    mut template: Value,
    field: &str,
    content: &str,
    max_frame_length: Option<usize>,
) -> Vec<Value> {
    if let Some(obj) = template.as_object_mut() {
        obj.insert(field.to_string(), Value::String(String::new()));
    }
    let wrapped_template_len = responses_payload_length(seq, event_name, &template);
    split_json_by_estimated_limit(template, content, max_frame_length, wrapped_template_len, move |value, part| {
        if let Some(obj) = value.as_object_mut() {
            obj.insert(field.to_string(), Value::String(part.to_string()));
        }
    })
}

pub(crate) fn split_json_value_by_string_patch(
    template: Value,
    content: &str,
    patch: fn(&mut Value, &str),
    max_frame_length: Option<usize>,
) -> Vec<Value> {
    let mut empty_template = template.clone();
    patch(&mut empty_template, "");
    let template_len = empty_template.to_string().len();
    split_json_by_estimated_limit(template, content, max_frame_length, template_len, patch)
}

pub(crate) fn split_json_by_estimated_limit(
    template: Value,
    content: &str,
    max_frame_length: Option<usize>,
    template_len: usize,
    patch: impl Fn(&mut Value, &str),
) -> Vec<Value> {
    const ESTIMATED_ESCAPE_RESERVE_BYTES: usize = 128;

    let Some(max_len) = max_frame_length else {
        let mut value = template;
        patch(&mut value, content);
        return vec![value];
    };

    let mut empty_value = template.clone();
    patch(&mut empty_value, "");
    if template_len > max_len {
        return vec![empty_value];
    }
    if content.is_empty() {
        return vec![empty_value];
    }

    let chunk_size = max_len
        .saturating_sub(template_len)
        .saturating_sub(ESTIMATED_ESCAPE_RESERVE_BYTES)
        .max(1);

    split_string_by_bytes(content, chunk_size)
        .into_iter()
        .map(|part| {
            let mut value = template.clone();
            patch(&mut value, &part);
            value
        })
        .collect()
}

pub(crate) fn wrap_responses_event(seq: &mut u64, name: &str, data: Value) -> Event {
    let payload = normalize_responses_payload(*seq, name, data);
    *seq += 1;
    Event::default().event(name).data(payload.to_string())
}

pub(crate) fn normalize_responses_payload(seq: u64, name: &str, data: Value) -> Value {
    match data {
        Value::Object(mut obj) => {
            obj.insert("type".to_string(), Value::String(name.to_string()));
            obj.insert("sequence_number".to_string(), json!(seq));
            Value::Object(obj)
        }
        other => {
            let mut obj = Map::new();
            obj.insert("type".to_string(), Value::String(name.to_string()));
            obj.insert("sequence_number".to_string(), json!(seq));
            obj.insert("data".to_string(), other);
            Value::Object(obj)
        }
    }
}

pub(crate) fn responses_payload_length(seq: u64, name: &str, data: &Value) -> usize {
    normalize_responses_payload(seq, name, data.clone())
        .to_string()
        .len()
}

pub(crate) fn split_string_by_bytes(content: &str, max_bytes: usize) -> Vec<String> {
    if content.is_empty() {
        return vec![String::new()];
    }

    let mut parts = Vec::new();
    let mut current = String::new();
    let mut current_bytes = 0usize;
    for ch in content.chars() {
        let ch_len = ch.len_utf8();
        if !current.is_empty() && current_bytes + ch_len > max_bytes {
            parts.push(current);
            current = String::new();
            current_bytes = 0;
        }
        current.push(ch);
        current_bytes += ch_len;
    }
    if !current.is_empty() {
        parts.push(current);
    }
    if parts.is_empty() {
        parts.push(String::new());
    }
    parts
}

pub(crate) fn chat_delta_path_content(value: &mut Value, content: &str) {
    if let Some(delta) = value
        .get_mut("choices")
        .and_then(Value::as_array_mut)
        .and_then(|arr| arr.first_mut())
        .and_then(Value::as_object_mut)
        .and_then(|choice| choice.get_mut("delta"))
        .and_then(Value::as_object_mut)
    {
        delta.insert("content".to_string(), Value::String(content.to_string()));
    }
}

pub(crate) fn chat_delta_path_reasoning_text(value: &mut Value, content: &str) {
    if let Some(delta) = value
        .get_mut("choices")
        .and_then(Value::as_array_mut)
        .and_then(|arr| arr.first_mut())
        .and_then(Value::as_object_mut)
        .and_then(|choice| choice.get_mut("delta"))
        .and_then(Value::as_object_mut)
    {
        delta.insert(
            "reasoning_details".to_string(),
            Value::Array(vec![reasoning_text_detail_value(content, None, None)]),
        );
    }
}

pub(crate) fn chat_delta_path_reasoning_summary(value: &mut Value, content: &str) {
    if let Some(delta) = value
        .get_mut("choices")
        .and_then(Value::as_array_mut)
        .and_then(|arr| arr.first_mut())
        .and_then(Value::as_object_mut)
        .and_then(|choice| choice.get_mut("delta"))
        .and_then(Value::as_object_mut)
    {
        delta.insert(
            "reasoning_details".to_string(),
            json!([{ "type": "reasoning.summary", "summary": content }]),
        );
    }
}

pub(crate) fn chat_delta_path_reasoning_encrypted(value: &mut Value, content: &str) {
    if let Some(delta) = value
        .get_mut("choices")
        .and_then(Value::as_array_mut)
        .and_then(|arr| arr.first_mut())
        .and_then(Value::as_object_mut)
        .and_then(|choice| choice.get_mut("delta"))
        .and_then(Value::as_object_mut)
    {
        delta.insert(
            "reasoning_details".to_string(),
            Value::Array(vec![reasoning_encrypted_detail_value(
                Value::String(content.to_string()),
                None,
            )]),
        );
    }
}

pub(crate) fn chat_delta_path_tool_arguments(value: &mut Value, content: &str) {
    if let Some(function) = value
        .get_mut("choices")
        .and_then(Value::as_array_mut)
        .and_then(|arr| arr.first_mut())
        .and_then(Value::as_object_mut)
        .and_then(|choice| choice.get_mut("delta"))
        .and_then(Value::as_object_mut)
        .and_then(|delta| delta.get_mut("tool_calls"))
        .and_then(Value::as_array_mut)
        .and_then(|arr| arr.first_mut())
        .and_then(Value::as_object_mut)
        .and_then(|tool| tool.get_mut("function"))
        .and_then(Value::as_object_mut)
    {
        function.insert("arguments".to_string(), Value::String(content.to_string()));
    }
}

pub(crate) fn messages_delta_path_text(value: &mut Value, content: &str) {
    if let Some(delta) = value
        .get_mut("delta")
        .and_then(Value::as_object_mut)
    {
        delta.insert("text".to_string(), Value::String(content.to_string()));
    }
}

pub(crate) fn messages_delta_path_thinking(value: &mut Value, content: &str) {
    if let Some(delta) = value
        .get_mut("delta")
        .and_then(Value::as_object_mut)
    {
        delta.insert("thinking".to_string(), Value::String(content.to_string()));
    }
}

pub(crate) fn messages_delta_path_signature(value: &mut Value, content: &str) {
    if let Some(delta) = value
        .get_mut("delta")
        .and_then(Value::as_object_mut)
    {
        delta.insert("signature".to_string(), Value::String(content.to_string()));
    }
}

pub(crate) fn messages_delta_path_partial_json(value: &mut Value, content: &str) {
    if let Some(delta) = value
        .get_mut("delta")
        .and_then(Value::as_object_mut)
    {
        delta.insert("partial_json".to_string(), Value::String(content.to_string()));
    }
}

pub(crate) fn sanitize_responses_output_item_for_frame_limit(item: &Value, max_frame_length: Option<usize>) -> Value {
    let Some(max_len) = max_frame_length else {
        return item.clone();
    };
    if item.to_string().len() <= max_len {
        return item.clone();
    }
    let mut sanitized = item.clone();
    if let Some(obj) = sanitized.as_object_mut() {
        match obj.get("type").and_then(|v| v.as_str()) {
            Some("message") => {
                if let Some(content) = obj.get_mut("content").and_then(Value::as_array_mut) {
                    for part in content {
                        if let Some(part_obj) = part.as_object_mut() {
                            if part_obj.get("type").and_then(|v| v.as_str()) == Some("output_text") {
                                part_obj.insert("text".to_string(), Value::String(String::new()));
                            }
                        }
                    }
                }
            }
            Some("reasoning") => {
                obj.insert("text".to_string(), Value::String(String::new()));
                if let Some(summary) = obj.get_mut("summary").and_then(Value::as_array_mut) {
                    for part in summary {
                        if let Some(part_obj) = part.as_object_mut() {
                            part_obj.insert("text".to_string(), Value::String(String::new()));
                        }
                    }
                }
            }
            Some("function_call") => {
                obj.insert("arguments".to_string(), Value::String(String::new()));
            }
            _ => {}
        }
    }
    sanitized
}

pub(crate) fn sanitize_responses_completed_for_frame_limit(encoded: &Value, max_frame_length: Option<usize>) -> Value {
    let Some(max_len) = max_frame_length else {
        return encoded.clone();
    };
    if encoded.to_string().len() <= max_len {
        return encoded.clone();
    }
    let mut sanitized = encoded.clone();
    if let Some(output) = sanitized.get_mut("output").and_then(Value::as_array_mut) {
        for item in output.iter_mut() {
            *item = sanitize_responses_output_item_for_frame_limit(item, Some(max_len));
        }
    }
    sanitized
}

pub(crate) fn extract_reasoning_parts(item: &Value) -> (String, String, String) {
    let text = item
        .get("text")
        .and_then(|v| v.as_str())
        .unwrap_or("")
        .to_string();
    let mut summary_text = String::new();
    if let Some(summary) = item.get("summary").and_then(|v| v.as_array()) {
        let mut parts = Vec::new();
        for s in summary {
            if s.get("type").and_then(|v| v.as_str()) == Some("summary_text") {
                if let Some(t) = s.get("text").and_then(|v| v.as_str()) {
                    if !t.is_empty() {
                        parts.push(t);
                    }
                }
            }
        }
        if !parts.is_empty() {
            summary_text = parts.join("\n");
        }
    }
    let mut signature = item
        .get("signature")
        .and_then(|v| v.as_str())
        .unwrap_or("")
        .to_string();
    if signature.is_empty() {
        signature = item
            .get("id")
            .and_then(|v| v.as_str())
            .unwrap_or("")
            .to_string();
    }
    (text, summary_text, signature)
}

pub(crate) fn reasoning_text_detail_value(
    text: &str,
    signature: Option<&str>,
    format: Option<&str>,
) -> Value {
    let mut value = json!({
        "type": "reasoning.text",
        "text": text,
    });
    if let Some(signature) = signature {
        value["signature"] = Value::String(signature.to_string());
    }
    if let Some(format) = format {
        value["format"] = Value::String(format.to_string());
    }
    value
}

pub(crate) fn reasoning_encrypted_detail_value(data: Value, format: Option<&str>) -> Value {
    let mut value = json!({
        "type": "reasoning.encrypted",
        "data": data,
    });
    if let Some(format) = format {
        value["format"] = Value::String(format.to_string());
    }
    value
}

pub(crate) fn extract_chat_reasoning_from_detail(
    detail: &Value,
    text_out: &mut Vec<String>,
    summary_out: &mut Vec<String>,
    sig_out: &mut Vec<String>,
) {
    let Some(obj) = detail.as_object() else {
        return;
    };
    match obj.get("type").and_then(|v| v.as_str()).unwrap_or("") {
        "reasoning.text" => {
            if let Some(t) = obj.get("text").and_then(|v| v.as_str()) {
                if !t.is_empty() {
                    text_out.push(t.to_string());
                }
            }
        }
        "reasoning.encrypted" => {
            if let Some(data) = obj.get("data") {
                match data {
                    Value::String(s) if !s.is_empty() => sig_out.push(s.clone()),
                    Value::String(_) | Value::Null => {}
                    other => sig_out.push(other.to_string()),
                }
            }
        }
        "reasoning.summary" => {
            if let Some(summary) = obj.get("summary").and_then(|v| v.as_str()) {
                if !summary.is_empty() {
                    summary_out.push(summary.to_string());
                }
            }
        }
        _ => {}
    }
}

pub(crate) fn extract_chat_reasoning_deltas(delta: &Value) -> (Vec<String>, Vec<String>, Vec<String>) {
    let mut text_parts = Vec::new();
    let mut summary_parts = Vec::new();
    let mut sig_parts = Vec::new();

    if let Some(details) = delta.get("reasoning_details").and_then(|v| v.as_array()) {
        for detail in details {
            extract_chat_reasoning_from_detail(
                detail,
                &mut text_parts,
                &mut summary_parts,
                &mut sig_parts,
            );
        }
    }

    if text_parts.is_empty() {
        if let Some(reasoning) = delta.get("reasoning").and_then(|v| v.as_str()) {
            if !reasoning.is_empty() {
                text_parts.push(reasoning.to_string());
            }
        }
    }

    if let Some(reasoning) = delta.get("reasoning_content").and_then(|v| v.as_str()) {
        if !reasoning.is_empty() {
            text_parts.push(reasoning.to_string());
        }
    }
    if let Some(sig) = delta.get("reasoning_opaque").and_then(|v| v.as_str()) {
        if !sig.is_empty() {
            sig_parts.push(sig.to_string());
        }
    }

    (text_parts, summary_parts, sig_parts)
}

pub(crate) fn chat_reasoning_delta_from_text(text: &str, format: Option<&str>) -> Value {
    json!({
        "reasoning_details": [reasoning_text_detail_value(text, None, format)]
    })
}

pub(crate) fn chat_reasoning_delta_from_summary(summary: &str, format: Option<&str>) -> Value {
    let mut detail = json!({
        "type": "reasoning.summary",
        "summary": summary
    });
    if let Some(format) = format {
        detail["format"] = Value::String(format.to_string());
    }
    json!({
        "reasoning_details": [detail]
    })
}

pub(crate) fn chat_reasoning_delta_from_encrypted(signature: &str, format: Option<&str>) -> Value {
    json!({
        "reasoning_details": [reasoning_encrypted_detail_value(Value::String(signature.to_string()), format)]
    })
}

pub(crate) fn extract_responses_message_text(item: &Value) -> String {
    let mut out = String::new();
    if item.get("type").and_then(|v| v.as_str()) != Some("message") {
        return out;
    }
    if let Some(content) = item.get("content").and_then(|v| v.as_array()) {
        for part in content {
            if part.get("type").and_then(|v| v.as_str()) == Some("output_text") {
                if let Some(text) = part.get("text").and_then(|v| v.as_str()) {
                    out.push_str(text);
                }
            }
        }
    }
    out
}

pub(crate) fn extract_responses_message_phase(item: &Value) -> Option<String> {
    if item.get("type").and_then(|v| v.as_str()) != Some("message") {
        return None;
    }
    item.get("phase")
        .and_then(|v| v.as_str())
        .map(|s| s.to_string())
}

pub(crate) fn insert_phase_if_present(obj: &mut Map<String, Value>, phase: Option<&str>) {
    if let Some(phase) = phase {
        obj.insert("phase".to_string(), Value::String(phase.to_string()));
    }
}

pub(crate) fn responses_text_delta_payload(
    text: &str,
    phase: Option<&str>,
    response_id: &str,
    item: &Value,
) -> Value {
    let mut obj = Map::new();
    obj.insert(
        "response_id".to_string(),
        Value::String(response_id.to_string()),
    );
    if let Some(item_id) = item.get("id").and_then(Value::as_str) {
        obj.insert("item_id".to_string(), Value::String(item_id.to_string()));
    }
    obj.insert("text".to_string(), Value::String(text.to_string()));
    insert_phase_if_present(&mut obj, phase);
    Value::Object(obj)
}
