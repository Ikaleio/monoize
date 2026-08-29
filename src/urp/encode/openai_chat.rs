use crate::urp::encode::{
    file_id_origin_matches, role_to_str, sanitize_provider_item_wire_body, text_parts,
    tool_choice_to_chat_value, usage_input_details, usage_output_details,
};
use crate::urp::internal_legacy_bridge::{Item, Part, Role, nodes_to_items};
use crate::urp::stream_helpers::{reasoning_encrypted_detail_value, reasoning_text_detail_value};
use crate::urp::{
    AudioSource, CHAT_LEGACY_FUNCTION_CALL_EXTRA_KEY, CHAT_LEGACY_FUNCTION_CHOICE_EXTRA_KEY,
    CHAT_LEGACY_FUNCTION_DEFINITION_EXTRA_KEY, CHAT_LEGACY_FUNCTION_RESULT_EXTRA_KEY,
    CHAT_MESSAGE_AUDIO_EXTRA_KEY, CHAT_REASONING_CONFIG_EXTRA_KEY, CHAT_REASONING_DETAIL_EXTRA_KEY,
    CHAT_REASONING_SURFACE_EXTRA_KEY, CHAT_REASONING_SURFACE_REASONING_CONTENT,
    CHAT_THINKING_CONFIG_EXTRA_KEY, FILE_ID_ORIGIN_OPENAI, FileSource, FinishReason, ImageSource,
    Node, OrdinaryRole, ProviderProtocol, ResponseFormat, StopControl, ToolCallType, ToolChoice,
    ToolDefinition, ToolResultContent, UrpRequest, UrpResponse,
};
use serde_json::{Map, Value, json};
use std::collections::HashMap;

const CHAT_CHOICE_EXTRA_BODY_KEY: &str = "_monoize_chat_choice_extra";
const CHAT_NATIVE_FINISH_REASON_EXTRA_KEY: &str = "_monoize_chat_native_finish_reason";

struct PendingChatMessage {
    role: Role,
    content_parts: Vec<Value>,
    tool_calls: Vec<Value>,
    refusal: Option<String>,
    reasoning_parts: Vec<Part>,
    legacy_function_call: Option<Value>,
    message_extra: HashMap<String, Value>,
}

fn encode_chat_tool_call(
    tool_type: ToolCallType,
    call_id: &str,
    name: &str,
    arguments: &str,
) -> Value {
    match tool_type {
        ToolCallType::Function => json!({
            "id": call_id,
            "type": "function",
            "function": { "name": name, "arguments": arguments }
        }),
        ToolCallType::Custom => json!({
            "id": call_id,
            "type": "custom",
            "custom": { "name": name, "input": arguments }
        }),
    }
}

fn merge_chat_usage_extra(usage: &mut Value, extra: &HashMap<String, Value>) {
    let Some(usage_obj) = usage.as_object_mut() else {
        return;
    };

    for detail_key in ["prompt_tokens_details", "completion_tokens_details"] {
        let Some(extra_detail) = extra.get(detail_key).and_then(Value::as_object) else {
            continue;
        };
        let Some(generated_detail) = usage_obj.get_mut(detail_key).and_then(Value::as_object_mut)
        else {
            continue;
        };
        for (key, value) in extra_detail {
            if !key.starts_with("_monoize_") {
                generated_detail
                    .entry(key.clone())
                    .or_insert_with(|| value.clone());
            }
        }
    }

    for (key, value) in extra {
        if !key.starts_with("_monoize_")
            && !matches!(
                key.as_str(),
                "prompt_tokens_details" | "completion_tokens_details"
            )
        {
            usage_obj.insert(key.clone(), value.clone());
        }
    }
}

fn encode_chat_content_part(part: &Part) -> Option<Value> {
    match part {
        Part::Text {
            content,
            extra_body,
            ..
        } => {
            let mut block = json!({ "type": "text", "text": content });
            if let Some(obj) = block.as_object_mut() {
                merge_chat_wire_extra(obj, extra_body);
            }
            Some(block)
        }
        Part::Image {
            source, extra_body, ..
        } => {
            let mut image = match source {
                ImageSource::Url { url, detail } => {
                    json!({ "type": "image_url", "image_url": { "url": url, "detail": detail } })
                }
                ImageSource::Base64 { media_type, data } => json!({
                    "type": "image_url",
                    "image_url": { "url": format!("data:{};base64,{}", media_type, data) }
                }),
                ImageSource::FileId { .. } => return None,
            };
            if let Some(obj) = image.as_object_mut() {
                merge_chat_wire_extra(obj, extra_body);
            }
            Some(image)
        }
        Part::File {
            source, extra_body, ..
        } => encode_chat_file_part(source, extra_body),
        Part::Audio {
            source, extra_body, ..
        } => encode_chat_audio_part(source, extra_body),
        Part::ProviderItem {
            origin_protocol,
            body,
            extra_body,
            ..
        } => encode_chat_provider_part(*origin_protocol, body, extra_body),
        _ => None,
    }
}

fn encode_chat_file_part(
    source: &FileSource,
    extra_body: &HashMap<String, Value>,
) -> Option<Value> {
    let file = match source {
        FileSource::FileId { file_id }
            if file_id_origin_matches(extra_body, FILE_ID_ORIGIN_OPENAI) =>
        {
            json!({ "file_id": file_id })
        }
        FileSource::Base64 { filename, data, .. } => {
            let mut file = json!({ "file_data": data });
            if let Some(filename) = filename {
                file["filename"] = json!(filename);
            }
            file
        }
        FileSource::Url { .. }
        | FileSource::FileId { .. }
        | FileSource::Text { .. }
        | FileSource::Content { .. } => return None,
    };
    let mut block = json!({ "type": "file", "file": file });
    merge_chat_wire_extra(block.as_object_mut()?, extra_body);
    Some(block)
}

fn encode_chat_audio_part(
    source: &AudioSource,
    extra_body: &HashMap<String, Value>,
) -> Option<Value> {
    let AudioSource::Base64 { media_type, data } = source else {
        return None;
    };
    let format = match media_type.as_str() {
        "audio/wav" | "audio/x-wav" => "wav",
        "audio/mpeg" | "audio/mp3" => "mp3",
        _ => return None,
    };
    let mut block = json!({
        "type": "input_audio",
        "input_audio": { "data": data, "format": format }
    });
    merge_chat_wire_extra(block.as_object_mut()?, extra_body);
    Some(block)
}

fn encode_chat_provider_part(
    origin_protocol: ProviderProtocol,
    body: &Value,
    extra_body: &HashMap<String, Value>,
) -> Option<Value> {
    if origin_protocol != ProviderProtocol::ChatCompletion {
        return None;
    }
    let mut part = sanitize_provider_item_wire_body(body);
    if let Some(obj) = part.as_object_mut() {
        merge_chat_wire_extra(obj, extra_body);
    }
    Some(part)
}

fn finalize_chat_message_content(m: &mut Map<String, Value>, content_parts: Vec<Value>) {
    if !content_parts.is_empty() {
        let can_collapse_single_text = content_parts.len() == 1
            && content_parts[0].get("type").and_then(|v| v.as_str()) == Some("text")
            && content_parts[0]
                .as_object()
                .map(|obj| obj.keys().all(|k| k == "type" || k == "text"))
                .unwrap_or(false);

        if can_collapse_single_text {
            if let Some(text) = content_parts[0].get("text").and_then(|v| v.as_str()) {
                m.insert("content".to_string(), Value::String(text.to_string()));
            }
        } else {
            m.insert("content".to_string(), Value::Array(content_parts));
        }
    } else {
        m.insert("content".to_string(), Value::String(String::new()));
    }
}

fn finalize_chat_response_content(m: &mut Map<String, Value>, content_parts: Vec<Value>) {
    if content_parts
        .iter()
        .any(|part| part.get("type").and_then(Value::as_str) != Some("text"))
    {
        m.insert("content".to_string(), Value::Array(content_parts));
        return;
    }

    let content = content_parts
        .into_iter()
        .filter_map(|part| {
            (part.get("type").and_then(|v| v.as_str()) == Some("text"))
                .then(|| part.get("text").and_then(|v| v.as_str()))
                .flatten()
                .map(str::to_string)
        })
        .collect::<Vec<_>>()
        .join("\n\n");
    m.insert("content".to_string(), Value::String(content));
}

fn flush_pending_chat_message(pending: &mut Option<PendingChatMessage>, out: &mut Vec<Value>) {
    let Some(pending_msg) = pending.take() else {
        return;
    };
    if pending_msg.content_parts.is_empty()
        && pending_msg.tool_calls.is_empty()
        && pending_msg.refusal.is_none()
        && pending_msg.reasoning_parts.is_empty()
        && pending_msg.legacy_function_call.is_none()
        && pending_msg.message_extra.is_empty()
    {
        return;
    }

    let mut m = Map::new();
    m.insert(
        "role".to_string(),
        Value::String(role_to_str(pending_msg.role).to_string()),
    );
    finalize_chat_message_content(&mut m, pending_msg.content_parts);
    if let Some(refusal) = pending_msg.refusal {
        m.insert("refusal".to_string(), Value::String(refusal));
    }
    if !pending_msg.tool_calls.is_empty() {
        m.insert(
            "tool_calls".to_string(),
            Value::Array(pending_msg.tool_calls),
        );
    }
    if let Some(function_call) = pending_msg.legacy_function_call {
        m.insert("function_call".to_string(), function_call);
    }
    insert_openrouter_reasoning_fields(&mut m, &pending_msg.reasoning_parts, false);
    merge_chat_wire_extra(&mut m, &pending_msg.message_extra);
    out.push(Value::Object(m));
}

fn should_split_chat_message(existing: &PendingChatMessage, part: &Part) -> bool {
    let _ = existing;
    let _ = part;
    false
}

fn push_part_into_pending_chat_message(
    pending: &mut Option<PendingChatMessage>,
    out: &mut Vec<Value>,
    role: Role,
    extra_body: &HashMap<String, Value>,
    part: &Part,
) {
    let should_flush = pending
        .as_ref()
        .is_some_and(|existing| should_split_chat_message(existing, part));
    if should_flush {
        flush_pending_chat_message(pending, out);
    }

    let entry = pending.get_or_insert_with(|| PendingChatMessage {
        role,
        content_parts: Vec::new(),
        tool_calls: Vec::new(),
        refusal: None,
        reasoning_parts: Vec::new(),
        legacy_function_call: None,
        message_extra: extra_body.clone(),
    });

    match part {
        Part::ProviderItem {
            origin_protocol: ProviderProtocol::ChatCompletion,
            body,
            extra_body,
            ..
        } if extra_body
            .get(CHAT_MESSAGE_AUDIO_EXTRA_KEY)
            .and_then(Value::as_bool)
            == Some(true) =>
        {
            entry
                .message_extra
                .insert("audio".to_string(), sanitize_provider_item_wire_body(body));
        }
        Part::Text { .. }
        | Part::Image { .. }
        | Part::Audio { .. }
        | Part::File { .. }
        | Part::ProviderItem { .. } => {
            if let Some(content) = encode_chat_content_part(part) {
                entry.content_parts.push(content);
            }
        }
        Part::Refusal { content, .. } => {
            entry.refusal = Some(content.clone());
        }
        Part::Reasoning { .. } => {
            entry.reasoning_parts.push(part.clone());
        }
        Part::ToolCall {
            tool_type,
            call_id,
            name,
            arguments,
            extra_body,
            ..
        } => {
            if *tool_type == ToolCallType::Function
                && extra_body
                    .get(CHAT_LEGACY_FUNCTION_CALL_EXTRA_KEY)
                    .and_then(Value::as_bool)
                    == Some(true)
            {
                let mut function_call = json!({ "name": name, "arguments": arguments });
                if let Some(obj) = function_call.as_object_mut() {
                    merge_chat_wire_extra(obj, extra_body);
                }
                entry.legacy_function_call = Some(function_call);
            } else {
                entry
                    .tool_calls
                    .push(encode_chat_tool_call(*tool_type, call_id, name, arguments));
            }
        }
    }
}

pub fn encode_request(req: &UrpRequest, upstream_model: &str) -> Value {
    let request_items = nodes_to_items(&req.input);
    let mut body = json!({
        "model": upstream_model,
        "messages": encode_messages(&request_items),
    });

    let obj = body.as_object_mut().expect("chat request object");
    if let Some(stream) = req.stream {
        obj.insert("stream".to_string(), Value::Bool(stream));
    }
    if let Some(temp) = req.temperature {
        obj.insert("temperature".to_string(), Value::from(temp));
    }
    if let Some(top_p) = req.top_p {
        obj.insert("top_p".to_string(), Value::from(top_p));
    }
    if let Some(max) = req.max_output_tokens {
        let key = if is_deepseek_model(upstream_model) {
            "max_tokens"
        } else {
            "max_completion_tokens"
        };
        obj.insert(key.to_string(), Value::from(max));
    }
    if let Some(reasoning) = &req.reasoning {
        let deepseek_model = is_deepseek_model(upstream_model);
        let raw_reasoning = reasoning
            .extra_body
            .get(CHAT_REASONING_CONFIG_EXTRA_KEY)
            .and_then(Value::as_object)
            .cloned();
        let raw_thinking = reasoning
            .extra_body
            .get(CHAT_THINKING_CONFIG_EXTRA_KEY)
            .cloned();
        let had_raw_reasoning = raw_reasoning.is_some();
        if let Some(mut raw_reasoning) = raw_reasoning {
            if let Some(effort) = reasoning.effort.as_deref() {
                raw_reasoning.remove("max_tokens");
                raw_reasoning.insert(
                    "effort".to_string(),
                    Value::String(chat_wire_effort(effort).to_string()),
                );
            }
            if !(deepseek_model && reasoning.effort.is_some()) {
                obj.insert("reasoning".to_string(), Value::Object(raw_reasoning));
            }
        }
        if let Some(raw_thinking) = raw_thinking {
            obj.insert("thinking".to_string(), raw_thinking);
        }
        if let Some(effort) = reasoning.effort.as_deref() {
            if deepseek_model {
                let wire_effort = deepseek_wire_effort(effort);
                if effort == "none" {
                    obj.insert("thinking".to_string(), json!({ "type": "disabled" }));
                    obj.remove("reasoning_effort");
                } else {
                    obj.insert("thinking".to_string(), json!({ "type": "enabled" }));
                    obj.insert(
                        "reasoning_effort".to_string(),
                        Value::String(wire_effort.to_string()),
                    );
                }
            } else if !had_raw_reasoning && effort != "none" {
                obj.insert(
                    "reasoning_effort".to_string(),
                    Value::String(chat_wire_effort(effort).to_string()),
                );
            }
        }
    }
    if let Some(tools) = &req.tools {
        let modern_tools = encode_tools(tools);
        let legacy_functions = encode_legacy_functions(tools);
        if !modern_tools.is_empty() || legacy_functions.is_empty() {
            obj.insert("tools".to_string(), Value::Array(modern_tools));
        }
        if !legacy_functions.is_empty() {
            obj.insert("functions".to_string(), Value::Array(legacy_functions));
        }
    }
    if let Some(tc) = &req.tool_choice {
        if let Some(raw_legacy_choice) = req.extra_body.get(CHAT_LEGACY_FUNCTION_CHOICE_EXTRA_KEY) {
            if let Some(choice) = encode_legacy_function_choice(tc, raw_legacy_choice) {
                obj.insert("function_call".to_string(), choice);
            }
        } else {
            obj.insert("tool_choice".to_string(), tool_choice_to_chat_value(tc));
        }
    }
    if let Some(parallel) = req.parallel_tool_calls {
        obj.insert("parallel_tool_calls".to_string(), Value::Bool(parallel));
    }
    if let Some(stop) = &req.stop {
        let value = match stop {
            StopControl::Single(stop) => Value::String(stop.clone()),
            StopControl::Multiple(stops) => Value::Array(
                stops
                    .iter()
                    .map(|stop| Value::String(stop.clone()))
                    .collect(),
            ),
        };
        obj.insert("stop".to_string(), value);
    }
    if let Some(verbosity) = &req.verbosity {
        obj.insert("verbosity".to_string(), Value::String(verbosity.clone()));
    }
    if let Some(format) = &req.response_format {
        obj.insert(
            "response_format".to_string(),
            encode_response_format(format),
        );
    }
    if let Some(user) = &req.user {
        obj.insert("user".to_string(), Value::String(user.clone()));
    }

    merge_chat_wire_extra(obj, &req.extra_body);

    if req.stream == Some(true) {
        match obj.get_mut("stream_options") {
            Some(Value::Object(so)) => {
                so.insert("include_usage".to_string(), Value::Bool(true));
            }
            Some(_) => {
                obj.insert("stream_options".to_string(), json!({"include_usage": true}));
            }
            None => {
                obj.insert("stream_options".to_string(), json!({"include_usage": true}));
            }
        }
    }

    body
}

pub fn encode_response(resp: &UrpResponse, logical_model: &str) -> Value {
    let message = encode_assistant_chat_message_from_nodes(&resp.output);
    let has_legacy_function_call = resp.output.iter().any(|node| {
        matches!(
            node,
            Node::ToolCall {
                tool_type: ToolCallType::Function,
                extra_body,
                ..
            } if extra_body
                .get(CHAT_LEGACY_FUNCTION_CALL_EXTRA_KEY)
                .and_then(Value::as_bool)
                == Some(true)
        )
    });

    let native_finish_reason = resp
        .extra_body
        .get(CHAT_NATIVE_FINISH_REASON_EXTRA_KEY)
        .and_then(Value::as_str)
        .filter(|reason| !reason.is_empty());
    let finish_reason = match resp.finish_reason {
        Some(FinishReason::Other) => native_finish_reason.unwrap_or("error"),
        Some(FinishReason::ToolCalls) if has_legacy_function_call => "function_call",
        Some(reason) => finish_reason_to_chat(reason),
        None => {
            if resp
                .output
                .iter()
                .any(|node| matches!(node, Node::ToolCall { .. }))
            {
                if has_legacy_function_call {
                    "function_call"
                } else {
                    "tool_calls"
                }
            } else {
                "stop"
            }
        }
    };

    let mut result = json!({
        "id": resp.id,
        "object": "chat.completion",
        "created": resp
            .created_at
            .unwrap_or_else(|| chrono::Utc::now().timestamp()),
        "model": logical_model,
        "choices": [{
            "index": 0,
            "message": Value::Object(message),
            "finish_reason": finish_reason,
        }],
    });

    if let Some(usage) = &resp.usage {
        let input_details = usage_input_details(usage);
        let output_details = usage_output_details(usage);
        let mut usage_value = json!({
            "prompt_tokens": usage.input_tokens,
            "completion_tokens": usage.output_tokens,
            "total_tokens": usage.total_tokens(),
            "completion_tokens_details": {
                "reasoning_tokens": output_details.reasoning_tokens,
                "accepted_prediction_tokens": output_details.accepted_prediction_tokens,
                "rejected_prediction_tokens": output_details.rejected_prediction_tokens
            },
            "prompt_tokens_details": {
                "cached_tokens": input_details.cache_read_tokens,
                "cache_write_tokens": input_details.cache_creation_tokens,
                "cache_creation_tokens": input_details.cache_creation_tokens,
                "tool_prompt_tokens": input_details.tool_prompt_tokens
            }
        });
        merge_chat_usage_extra(&mut usage_value, &usage.extra_body);
        result["usage"] = usage_value;
    }

    if let Some(choice_extra) = resp
        .extra_body
        .get(CHAT_CHOICE_EXTRA_BODY_KEY)
        .and_then(Value::as_object)
        && let Some(choice) = result
            .get_mut("choices")
            .and_then(Value::as_array_mut)
            .and_then(|choices| choices.first_mut())
            .and_then(Value::as_object_mut)
    {
        for (key, value) in choice_extra {
            if !key.starts_with("_monoize_") {
                choice.entry(key.clone()).or_insert_with(|| value.clone());
            }
        }
    }

    let obj = result.as_object_mut().expect("chat response object");
    let mut response_extra = resp.extra_body.clone();
    response_extra.remove(CHAT_CHOICE_EXTRA_BODY_KEY);
    response_extra.remove(CHAT_NATIVE_FINISH_REASON_EXTRA_KEY);
    merge_chat_wire_extra(obj, &response_extra);
    result
}

fn encode_assistant_chat_message_from_nodes(nodes: &[Node]) -> Map<String, Value> {
    let mut message = Map::new();
    message.insert("role".to_string(), Value::String("assistant".to_string()));

    let mut content_parts = Vec::new();
    let mut tool_calls = Vec::new();
    let mut refusal: Option<String> = None;
    let mut reasoning_parts = Vec::new();
    let mut legacy_function_call: Option<Value> = None;
    let mut message_extra = HashMap::new();

    for node in nodes {
        match node {
            Node::NextDownstreamEnvelopeExtra { extra_body } => {
                merge_extra_preserving_existing(
                    &mut message_extra,
                    extra_body
                        .iter()
                        .filter(|(key, _)| !key.starts_with("_monoize_"))
                        .map(|(key, value)| (key.clone(), value.clone()))
                        .collect(),
                );
            }
            Node::Text {
                role: OrdinaryRole::Assistant,
                content,
                extra_body,
                ..
            } => {
                let mut block = json!({ "type": "text", "text": content });
                if let Some(obj) = block.as_object_mut() {
                    merge_chat_wire_extra(obj, extra_body);
                }
                merge_extra_preserving_existing(
                    &mut message_extra,
                    assistant_message_extra_from_node(node),
                );
                content_parts.push(block);
            }
            Node::Image {
                role: OrdinaryRole::Assistant,
                source,
                extra_body,
                ..
            } => {
                let mut image = match source {
                    ImageSource::Url { url, detail } => {
                        json!({ "type": "image_url", "image_url": { "url": url, "detail": detail } })
                    }
                    ImageSource::Base64 { media_type, data } => json!({
                        "type": "image_url",
                        "image_url": { "url": format!("data:{};base64,{}", media_type, data) }
                    }),
                    ImageSource::FileId { .. } => continue,
                };
                if let Some(obj) = image.as_object_mut() {
                    merge_chat_wire_extra(obj, extra_body);
                }
                merge_extra_preserving_existing(
                    &mut message_extra,
                    assistant_message_extra_from_node(node),
                );
                content_parts.push(image);
            }
            Node::File {
                role: OrdinaryRole::Assistant,
                ..
            } => {
                continue;
            }
            Node::Refusal { content, .. } => {
                refusal.get_or_insert_with(|| content.clone());
            }
            Node::Reasoning { .. } => {
                if let Node::Reasoning {
                    id: _,
                    content,
                    encrypted,
                    summary,
                    source,
                    extra_body,
                } = node
                {
                    reasoning_parts.push(Part::Reasoning {
                        id: None,
                        content: content.clone(),
                        encrypted: encrypted.clone(),
                        summary: summary.clone(),
                        source: source.clone(),
                        extra_body: extra_body.clone(),
                    });
                }
                merge_extra_preserving_existing(
                    &mut message_extra,
                    assistant_message_extra_from_node(node),
                );
            }
            Node::ToolCall {
                tool_type,
                call_id,
                name,
                arguments,
                extra_body,
                ..
            } => {
                if *tool_type == ToolCallType::Function
                    && extra_body
                        .get(CHAT_LEGACY_FUNCTION_CALL_EXTRA_KEY)
                        .and_then(Value::as_bool)
                        == Some(true)
                {
                    let mut function_call = json!({ "name": name, "arguments": arguments });
                    if let Some(obj) = function_call.as_object_mut() {
                        merge_chat_wire_extra(obj, extra_body);
                    }
                    legacy_function_call = Some(function_call);
                } else {
                    tool_calls.push(encode_chat_tool_call(*tool_type, call_id, name, arguments));
                }
            }
            Node::ProviderItem {
                role: OrdinaryRole::Assistant,
                origin_protocol: ProviderProtocol::ChatCompletion,
                body,
                extra_body,
                ..
            } if extra_body
                .get(CHAT_MESSAGE_AUDIO_EXTRA_KEY)
                .and_then(Value::as_bool)
                == Some(true) =>
            {
                message_extra.insert("audio".to_string(), sanitize_provider_item_wire_body(body));
            }
            Node::ProviderItem {
                role: OrdinaryRole::Assistant,
                origin_protocol,
                body,
                extra_body,
                ..
            } => {
                if let Some(part) = encode_chat_provider_part(*origin_protocol, body, extra_body) {
                    merge_extra_preserving_existing(
                        &mut message_extra,
                        assistant_message_extra_from_node(node),
                    );
                    content_parts.push(part);
                }
            }
            _ => {}
        }
    }

    let had_content_parts = !content_parts.is_empty();
    finalize_chat_response_content(&mut message, content_parts);
    if let Some(refusal) = refusal {
        message.insert("refusal".to_string(), Value::String(refusal));
    }
    if !tool_calls.is_empty() {
        message.insert("tool_calls".to_string(), Value::Array(tool_calls));
    }
    if let Some(function_call) = legacy_function_call {
        message.insert("function_call".to_string(), function_call);
    }
    insert_openrouter_reasoning_fields(&mut message, &reasoning_parts, true);
    merge_chat_wire_extra(&mut message, &message_extra);
    if !had_content_parts
        && (message.contains_key("audio") || message.contains_key("function_call"))
    {
        message.insert("content".to_string(), Value::Null);
    }
    message
}

fn assistant_message_extra_from_node(node: &Node) -> HashMap<String, Value> {
    match node {
        Node::Text { phase, .. } => {
            let mut out = HashMap::new();
            if let Some(phase) = phase {
                out.insert("phase".to_string(), Value::String(phase.clone()));
            }
            out
        }
        Node::Image { .. }
        | Node::Audio { .. }
        | Node::File { .. }
        | Node::Refusal { .. }
        | Node::ToolCall { .. }
        | Node::ProviderItem { .. }
        | Node::Reasoning { .. }
        | Node::ToolResult { .. }
        | Node::NextDownstreamEnvelopeExtra { .. } => HashMap::new(),
    }
}

fn merge_extra_preserving_existing(dst: &mut HashMap<String, Value>, src: HashMap<String, Value>) {
    for (k, v) in src {
        dst.entry(k).or_insert(v);
    }
}

fn merge_chat_wire_extra(
    wire_object: &mut Map<String, Value>,
    extra_body: &HashMap<String, Value>,
) {
    for (key, value) in extra_body {
        if !key.starts_with("_monoize_") && !wire_object.contains_key(key) {
            wire_object.insert(key.clone(), value.clone());
        }
    }
}

fn encode_messages(messages: &[Item]) -> Vec<Value> {
    let mut out = Vec::new();
    for item in messages {
        match item {
            Item::ToolResult {
                call_id,
                content,
                extra_body,
                ..
            } => {
                let text = content
                    .iter()
                    .filter_map(|content| match content {
                        ToolResultContent::Text { text, .. } => Some(text.as_str()),
                        _ => None,
                    })
                    .collect::<Vec<_>>()
                    .join("");
                let mut m = Map::new();
                if let Some(name) = extra_body
                    .get(CHAT_LEGACY_FUNCTION_RESULT_EXTRA_KEY)
                    .and_then(Value::as_str)
                {
                    m.insert("role".to_string(), Value::String("function".to_string()));
                    m.insert("name".to_string(), Value::String(name.to_string()));
                    m.insert("content".to_string(), Value::String(text));
                } else {
                    m.insert("role".to_string(), Value::String("tool".to_string()));
                    m.insert("content".to_string(), Value::String(text));
                    m.insert("tool_call_id".to_string(), Value::String(call_id.clone()));
                }
                merge_chat_wire_extra(&mut m, extra_body);
                out.push(Value::Object(m));
            }
            Item::Message {
                id: _,
                role,
                parts,
                extra_body,
            } => {
                if *role == Role::Tool {
                    let mut m = Map::new();
                    m.insert("role".to_string(), Value::String("tool".to_string()));
                    m.insert("content".to_string(), Value::String(text_parts(parts)));
                    merge_chat_wire_extra(&mut m, extra_body);
                    out.push(Value::Object(m));
                    continue;
                }

                let mut pending: Option<PendingChatMessage> = None;
                for part in parts {
                    push_part_into_pending_chat_message(
                        &mut pending,
                        &mut out,
                        *role,
                        extra_body,
                        part,
                    );
                }
                flush_pending_chat_message(&mut pending, &mut out);
            }
        }
    }
    out
}

fn encode_tools(tools: &[ToolDefinition]) -> Vec<Value> {
    let mut out = Vec::new();
    for tool in tools {
        if tool
            .extra_body
            .get(CHAT_LEGACY_FUNCTION_DEFINITION_EXTRA_KEY)
            .and_then(Value::as_bool)
            == Some(true)
        {
            continue;
        }
        if tool.tool_type == "function" {
            if let Some(function) = &tool.function {
                let mut fn_obj = Map::new();
                fn_obj.insert("name".to_string(), Value::String(function.name.clone()));
                if let Some(desc) = &function.description {
                    fn_obj.insert("description".to_string(), Value::String(desc.clone()));
                }
                if let Some(parameters) = &function.parameters {
                    fn_obj.insert("parameters".to_string(), parameters.clone());
                }
                if let Some(strict) = function.strict {
                    fn_obj.insert("strict".to_string(), Value::Bool(strict));
                }
                merge_chat_wire_extra(&mut fn_obj, &function.extra_body);

                let mut obj = Map::new();
                obj.insert("type".to_string(), Value::String("function".to_string()));
                obj.insert("function".to_string(), Value::Object(fn_obj));
                merge_chat_wire_extra(&mut obj, &tool.extra_body);
                out.push(Value::Object(obj));
            }
        } else if tool.tool_type == "custom"
            && let Some(custom) = &tool.custom
        {
            let mut custom_obj = Map::new();
            custom_obj.insert("name".to_string(), Value::String(custom.name.clone()));
            if let Some(desc) = &custom.description {
                custom_obj.insert("description".to_string(), Value::String(desc.clone()));
            }
            if let Some(format) = &custom.format {
                custom_obj.insert("format".to_string(), format.clone());
            }
            merge_chat_wire_extra(&mut custom_obj, &custom.extra_body);

            let mut obj = Map::new();
            obj.insert("type".to_string(), Value::String("custom".to_string()));
            obj.insert("custom".to_string(), Value::Object(custom_obj));
            merge_chat_wire_extra(&mut obj, &tool.extra_body);
            out.push(Value::Object(obj));
        }
    }
    out
}

fn encode_legacy_functions(tools: &[ToolDefinition]) -> Vec<Value> {
    let mut out = Vec::new();
    for tool in tools {
        if tool.tool_type != "function"
            || tool
                .extra_body
                .get(CHAT_LEGACY_FUNCTION_DEFINITION_EXTRA_KEY)
                .and_then(Value::as_bool)
                != Some(true)
        {
            continue;
        }
        let Some(function) = &tool.function else {
            continue;
        };

        let mut function_obj = Map::new();
        function_obj.insert("name".to_string(), Value::String(function.name.clone()));
        if let Some(description) = &function.description {
            function_obj.insert(
                "description".to_string(),
                Value::String(description.clone()),
            );
        }
        if let Some(parameters) = &function.parameters {
            function_obj.insert("parameters".to_string(), parameters.clone());
        }
        if let Some(strict) = function.strict {
            function_obj.insert("strict".to_string(), Value::Bool(strict));
        }
        merge_chat_wire_extra(&mut function_obj, &function.extra_body);
        merge_chat_wire_extra(&mut function_obj, &tool.extra_body);
        out.push(Value::Object(function_obj));
    }
    out
}

fn encode_legacy_function_choice(choice: &ToolChoice, raw_choice: &Value) -> Option<Value> {
    match choice {
        ToolChoice::Mode(mode) => Some(Value::String(mode.clone())),
        ToolChoice::Specific(Value::Object(choice_obj)) => {
            let name = choice_obj
                .get("function")
                .and_then(Value::as_object)
                .and_then(|function| function.get("name"))
                .or_else(|| choice_obj.get("name"))
                .and_then(Value::as_str)?;
            let sanitized_raw = sanitize_provider_item_wire_body(raw_choice);
            let mut legacy_choice = sanitized_raw.as_object().cloned().unwrap_or_default();
            legacy_choice.remove("name");
            legacy_choice.insert("name".to_string(), Value::String(name.to_string()));
            Some(Value::Object(legacy_choice))
        }
        ToolChoice::Specific(_) => None,
    }
}

fn encode_response_format(format: &ResponseFormat) -> Value {
    match format {
        ResponseFormat::Text => json!({ "type": "text" }),
        ResponseFormat::JsonObject => json!({ "type": "json_object" }),
        ResponseFormat::JsonSchema { json_schema } => {
            let mut schema_obj = Map::new();
            schema_obj.insert("name".to_string(), Value::String(json_schema.name.clone()));
            schema_obj.insert("schema".to_string(), json_schema.schema.clone());
            if let Some(desc) = &json_schema.description {
                schema_obj.insert("description".to_string(), Value::String(desc.clone()));
            }
            if let Some(strict) = json_schema.strict {
                schema_obj.insert("strict".to_string(), Value::Bool(strict));
            }
            merge_chat_wire_extra(&mut schema_obj, &json_schema.extra_body);
            json!({
                "type": "json_schema",
                "json_schema": Value::Object(schema_obj),
            })
        }
    }
}

fn insert_openrouter_reasoning_fields(
    message: &mut Map<String, Value>,
    parts: &[Part],
    derive_scalar_aliases_from_raw_details: bool,
) {
    let mut details = Vec::new();
    let mut reasoning_value: Option<String> = None;
    let mut reasoning_summary_value: Option<String> = None;
    let mut reasoning_content_value: Option<String> = None;

    for part in parts {
        let Part::Reasoning {
            id,
            content,
            encrypted,
            summary,
            source,
            extra_body,
        } = part
        else {
            continue;
        };
        let format = source.as_deref().filter(|format| !format.is_empty());

        if let Some(raw_detail) = extra_body
            .get(CHAT_REASONING_DETAIL_EXTRA_KEY)
            .and_then(Value::as_object)
        {
            let mut detail = raw_detail.clone();
            if let Some(id) = id.as_deref().filter(|id| !id.is_empty()) {
                detail.insert("id".to_string(), Value::String(id.to_string()));
            }
            if let Some(format) = format {
                detail.insert("format".to_string(), Value::String(format.to_string()));
            }
            match detail.get("type").and_then(Value::as_str) {
                Some("reasoning.summary") => {
                    if let Some(summary) = summary {
                        if derive_scalar_aliases_from_raw_details
                            && reasoning_summary_value.is_none()
                            && !summary.is_empty()
                        {
                            reasoning_summary_value = Some(summary.clone());
                        }
                        detail.insert("summary".to_string(), Value::String(summary.clone()));
                    }
                }
                Some("reasoning.text") => {
                    if let Some(content) = content {
                        if derive_scalar_aliases_from_raw_details
                            && reasoning_value.is_none()
                            && !content.is_empty()
                        {
                            reasoning_value = Some(content.clone());
                        }
                        detail.insert("text".to_string(), Value::String(content.clone()));
                    }
                }
                Some("reasoning.encrypted") => {
                    if let Some(encrypted) = encrypted {
                        detail.insert("data".to_string(), encrypted.clone());
                    }
                }
                _ => {}
            }
            details.push(Value::Object(detail));
            continue;
        }

        if extra_body
            .get(CHAT_REASONING_SURFACE_EXTRA_KEY)
            .and_then(Value::as_str)
            == Some(CHAT_REASONING_SURFACE_REASONING_CONTENT)
        {
            if reasoning_content_value.is_none() {
                reasoning_content_value = content
                    .as_deref()
                    .filter(|value| !value.is_empty())
                    .or_else(|| summary.as_deref().filter(|value| !value.is_empty()))
                    .map(str::to_string);
            }
            continue;
        }

        if let Some(summary) = summary.as_deref().filter(|summary| !summary.is_empty()) {
            if reasoning_summary_value.is_none() {
                reasoning_summary_value = Some(summary.to_string());
            }
            if (extra_body
                .get("openwebui_reasoning_content")
                .and_then(Value::as_bool)
                == Some(true))
                && reasoning_content_value.is_none()
            {
                reasoning_content_value = Some(summary.to_string());
            }
            details.push(json!({
                "type": "reasoning.summary",
                "summary": summary,
            }));
            if let Some(format) = format {
                details
                    .last_mut()
                    .and_then(Value::as_object_mut)
                    .map(|obj| obj.insert("format".to_string(), Value::String(format.to_string())));
            }
        }

        if let Some(content) = content.as_deref().filter(|content| !content.is_empty()) {
            if reasoning_value.is_none() {
                reasoning_value = Some(content.to_string());
            }
            details.push(reasoning_text_detail_value(content, format));
        }

        if let Some(enc) = encrypted {
            if !matches!(enc, Value::Null) {
                if let Some(s) = enc.as_str() {
                    if s.is_empty() {
                        continue;
                    }
                }

                let mut detail = reasoning_encrypted_detail_value(enc.clone(), format);
                if let Some(id) = id
                    .as_deref()
                    .filter(|id| !id.is_empty())
                    .or_else(|| extra_body.get("id").and_then(Value::as_str))
                {
                    detail["id"] = Value::String(id.to_string());
                }
                details.push(detail);
            }
        }
    }

    if let Some(reasoning_text) = reasoning_value.or(reasoning_summary_value) {
        message.insert("reasoning".to_string(), Value::String(reasoning_text));
    }

    if let Some(reasoning_content) = reasoning_content_value {
        message.insert(
            "reasoning_content".to_string(),
            Value::String(reasoning_content),
        );
    }

    if !details.is_empty() {
        message.insert("reasoning_details".to_string(), Value::Array(details));
    }
}

fn is_deepseek_model(model: &str) -> bool {
    model.to_ascii_lowercase().contains("deepseek")
}

fn chat_wire_effort(effort: &str) -> &str {
    if effort == "minimum" {
        "minimal"
    } else {
        effort
    }
}

fn deepseek_wire_effort(effort: &str) -> &str {
    match effort {
        "none" | "minimal" | "minimum" | "low" | "medium" => "high",
        "xhigh" | "max" => "max",
        _ => "high",
    }
}

fn finish_reason_to_chat(finish_reason: FinishReason) -> &'static str {
    match finish_reason {
        FinishReason::Stop => "stop",
        FinishReason::Length => "length",
        FinishReason::ToolCalls => "tool_calls",
        FinishReason::ContentFilter => "content_filter",
        FinishReason::Other => "error",
    }
}
