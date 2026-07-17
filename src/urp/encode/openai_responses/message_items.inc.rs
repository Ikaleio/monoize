#[derive(Clone)]
struct PendingResponsesMessageItem {
    id: Option<String>,
    role: Role,
    phase: Option<String>,
    content: Vec<Value>,
    extra_body: HashMap<String, Value>,
}

fn normalize_openai_message_id(id: Option<&str>) -> String {
    match id {
        Some(existing) if existing.starts_with("msg_") => existing.to_string(),
        _ => format!("msg_{}", uuid::Uuid::new_v4().simple()),
    }
}

fn normalize_openai_function_call_item_id(id: Option<&str>) -> String {
    match id {
        Some(existing) if existing.starts_with("fc_") => existing.to_string(),
        _ => format!("fc_{}", uuid::Uuid::new_v4().simple()),
    }
}

fn normalize_openai_function_output_id(id: Option<&str>) -> String {
    match id {
        Some(existing) if existing.starts_with("fco_") => existing.to_string(),
        Some(existing) if existing.starts_with("fc_") => existing.replacen("fc_", "fco_", 1),
        _ => format!("fco_{}", uuid::Uuid::new_v4().simple()),
    }
}

fn text_part_phase(part: &Part) -> Option<&str> {
    match part {
        Part::Text { extra_body, .. } => extra_body.get("phase").and_then(|v| v.as_str()),
        _ => None,
    }
}

fn can_use_responses_instructions(item: &Item) -> bool {
    let Item::Message {
        role,
        parts,
        extra_body,
        ..
    } = item
    else {
        return false;
    };

    matches!(role, Role::System | Role::Developer)
        && !parts.is_empty()
        && extra_body.is_empty()
        && parts.iter().all(|part| {
            matches!(
                part,
                Part::Text {
                    extra_body,
                    ..
                } if extra_body.get("phase").is_none() && extra_body.is_empty()
            )
        })
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
    obj.insert("type".to_string(), Value::String("message".to_string()));
    obj.insert(
        "id".to_string(),
        Value::String(normalize_openai_message_id(pending_item.id.as_deref())),
    );
    obj.insert("status".to_string(), Value::String("completed".to_string()));
    obj.insert(
        "role".to_string(),
        Value::String(role_to_str(pending_item.role).to_string()),
    );
    obj.insert("content".to_string(), Value::Array(pending_item.content));
    if let Some(phase) = pending_item.phase {
        obj.insert("phase".to_string(), Value::String(phase));
    }
    merge_extra(&mut obj, &pending_item.extra_body);
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
        id: message_extra
            .get("id")
            .and_then(Value::as_str)
            .map(|s| s.to_string()),
        role,
        phase: phase_owned,
        content: Vec::new(),
        extra_body: message_extra.clone(),
    });
    entry.content.push(content_part);
}

fn encode_message_content_part(part: &Part, output_text_type: bool) -> Option<Value> {
    match part {
        Part::Text {
            content,
            extra_body,
            ..
        } => {
            let mut obj = Map::new();
            obj.insert(
                "type".to_string(),
                Value::String(
                    if output_text_type {
                        "output_text"
                    } else {
                        "input_text"
                    }
                    .to_string(),
                ),
            );
            obj.insert("text".to_string(), Value::String(content.clone()));
            if output_text_type {
                obj.entry("annotations".to_string())
                    .or_insert_with(|| Value::Array(Vec::new()));
                obj.entry("logprobs".to_string())
                    .or_insert_with(|| Value::Array(Vec::new()));
            }
            merge_extra(&mut obj, extra_body);
            Some(Value::Object(obj))
        }
        Part::Image { source, extra_body } => {
            let mut value = if output_text_type {
                encode_output_image(source, extra_body)?
            } else {
                encode_input_image(source, extra_body)?
            };
            if let Some(obj) = value.as_object_mut() {
                merge_extra(obj, extra_body);
            }
            Some(value)
        }
        Part::File { source, extra_body } => {
            let mut value = if output_text_type {
                encode_output_file(source, extra_body)?
            } else {
                encode_input_file(source, extra_body)?
            };
            if let Some(obj) = value.as_object_mut() {
                merge_extra(obj, extra_body);
            }
            Some(value)
        }
        Part::Refusal {
            content,
            extra_body,
        } => {
            let mut obj = Map::new();
            obj.insert("type".to_string(), Value::String("refusal".to_string()));
            obj.insert("refusal".to_string(), Value::String(content.clone()));
            merge_extra(&mut obj, extra_body);
            Some(Value::Object(obj))
        }
        _ => None,
    }
}
