use crate::urp::encode::{
    role_to_str, sanitize_provider_item_wire_body, text_parts, tool_choice_to_openai_value,
    usage_input_details, usage_output_details,
};
use crate::urp::internal_legacy_bridge::{Item, Part, Role, nodes_to_items};
use crate::urp::stream_helpers::{reasoning_encrypted_detail_value, reasoning_text_detail_value};
use crate::urp::{
    CHAT_REASONING_CONFIG_EXTRA_KEY, CHAT_REASONING_DETAIL_EXTRA_KEY,
    CHAT_REASONING_SURFACE_EXTRA_KEY, CHAT_REASONING_SURFACE_REASONING_CONTENT,
    CHAT_THINKING_CONFIG_EXTRA_KEY, FileSource, FinishReason, ImageSource, Node, OrdinaryRole,
    ProviderProtocol, ResponseFormat, ToolDefinition, ToolResultContent, UrpRequest, UrpResponse,
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
    message_extra: HashMap<String, Value>,
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
        } => {
            let text = match source {
                FileSource::Url { url } => format!("[file:{url}]"),
                FileSource::Base64 {
                    filename,
                    media_type,
                    ..
                } => format!(
                    "[file:{}:{}]",
                    filename.clone().unwrap_or_else(|| "file".to_string()),
                    media_type
                ),
                FileSource::FileId { .. }
                | FileSource::Text { .. }
                | FileSource::Content { .. } => return None,
            };
            let mut block = json!({ "type": "text", "text": text });
            if let Some(obj) = block.as_object_mut() {
                merge_chat_wire_extra(obj, extra_body);
            }
            Some(block)
        }
        Part::ProviderItem {
            origin_protocol,
            body,
            extra_body,
            ..
        } => encode_chat_provider_part(*origin_protocol, body, extra_body),
        _ => None,
    }
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
        message_extra: extra_body.clone(),
    });

    match part {
        Part::Text { .. } | Part::Image { .. } | Part::File { .. } | Part::ProviderItem { .. } => {
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
            call_id,
            name,
            arguments,
            ..
        } => {
            entry.tool_calls.push(json!({
                "id": call_id,
                "type": "function",
                "function": {
                    "name": name,
                    "arguments": arguments
                }
            }));
        }
        _ => {}
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
        obj.insert("tools".to_string(), Value::Array(encode_tools(tools)));
    }
    if let Some(tc) = &req.tool_choice {
        obj.insert("tool_choice".to_string(), tool_choice_to_openai_value(tc));
    }
    if let Some(parallel) = req.parallel_tool_calls {
        obj.insert("parallel_tool_calls".to_string(), Value::Bool(parallel));
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

    // Streaming usage otherwise arrives only when the caller explicitly opts in.
    // Non-stream requests carry usage on the response object and must not receive
    // a stream-only option.
    if req.stream == Some(true) {
        match obj.get_mut("stream_options") {
            Some(Value::Object(so)) => {
                so.entry("include_usage".to_string())
                    .or_insert(Value::Bool(true));
            }
            Some(_) => {}
            None => {
                obj.insert("stream_options".to_string(), json!({"include_usage": true}));
            }
        }
    }

    body
}

pub fn encode_response(resp: &UrpResponse, logical_model: &str) -> Value {
    let message = encode_assistant_chat_message_from_nodes(&resp.output);

    let native_finish_reason = resp
        .extra_body
        .get(CHAT_NATIVE_FINISH_REASON_EXTRA_KEY)
        .and_then(Value::as_str)
        .filter(|reason| !reason.is_empty());
    let finish_reason = match resp.finish_reason {
        Some(FinishReason::Other) => native_finish_reason.unwrap_or("error"),
        Some(reason) => finish_reason_to_chat(reason),
        None => {
            if resp
                .output
                .iter()
                .any(|node| matches!(node, Node::ToolCall { .. }))
            {
                "tool_calls"
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
        if let Some(obj) = usage_value.as_object_mut() {
            for (k, v) in &usage.extra_body {
                if !k.starts_with("_monoize_") {
                    obj.insert(k.clone(), v.clone());
                }
            }
        }
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
                source,
                extra_body,
                ..
            } => {
                let text = match source {
                    FileSource::Url { url } => format!("[file:{url}]"),
                    FileSource::Base64 {
                        filename,
                        media_type,
                        ..
                    } => format!(
                        "[file:{}:{}]",
                        filename.clone().unwrap_or_else(|| "file".to_string()),
                        media_type
                    ),
                    FileSource::FileId { .. }
                    | FileSource::Text { .. }
                    | FileSource::Content { .. } => continue,
                };
                let mut block = json!({ "type": "text", "text": text });
                if let Some(obj) = block.as_object_mut() {
                    merge_chat_wire_extra(obj, extra_body);
                }
                merge_extra_preserving_existing(
                    &mut message_extra,
                    assistant_message_extra_from_node(node),
                );
                content_parts.push(block);
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
                call_id,
                name,
                arguments,
                ..
            } => {
                tool_calls.push(json!({
                    "id": call_id,
                    "type": "function",
                    "function": {
                        "name": name,
                        "arguments": arguments
                    }
                }));
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

    finalize_chat_response_content(&mut message, content_parts);
    if let Some(refusal) = refusal {
        message.insert("refusal".to_string(), Value::String(refusal));
    }
    if !tool_calls.is_empty() {
        message.insert("tool_calls".to_string(), Value::Array(tool_calls));
    }
    insert_openrouter_reasoning_fields(&mut message, &reasoning_parts, true);
    merge_chat_wire_extra(&mut message, &message_extra);
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
                m.insert("role".to_string(), Value::String("tool".to_string()));
                m.insert("content".to_string(), Value::String(text));
                m.insert("tool_call_id".to_string(), Value::String(call_id.clone()));
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

#[cfg(test)]
mod tests {
    use super::*;
    use crate::urp::decode::openai_chat as decode_chat;
    use crate::urp::internal_legacy_bridge::{items_to_nodes, nodes_to_items};
    use crate::urp::{InputDetails, OutputDetails, UrpResponse, Usage};
    use std::collections::HashMap;

    fn empty_map() -> HashMap<String, Value> {
        HashMap::new()
    }

    fn base_request(messages: Vec<Item>) -> UrpRequest {
        UrpRequest {
            model: "logical-model".to_string(),
            input: items_to_nodes(messages),
            stream: None,
            temperature: None,
            top_p: None,
            max_output_tokens: None,
            reasoning: None,
            tools: None,
            tool_choice: None,
            parallel_tool_calls: None,
            response_format: None,
            user: None,
            extra_body: empty_map(),
        }
    }

    #[test]
    fn chat_stream_options_are_synthesized_only_for_streaming_requests() {
        let mut req = base_request(vec![Item::new_message(Role::User)]);

        let absent = encode_request(&req, "gpt-5.4");
        assert!(absent.get("stream_options").is_none());

        req.stream = Some(false);
        let non_stream = encode_request(&req, "gpt-5.4");
        assert!(non_stream.get("stream_options").is_none());

        req.stream = Some(true);
        let stream = encode_request(&req, "gpt-5.4");
        assert_eq!(stream["stream_options"]["include_usage"], json!(true));
    }

    #[test]
    fn deepseek_request_uses_current_thinking_and_token_controls() {
        let mut req = base_request(vec![Item::new_message(Role::User)]);
        req.max_output_tokens = Some(2048);
        req.reasoning = Some(crate::urp::ReasoningConfig {
            effort: Some("low".to_string()),
            extra_body: empty_map(),
        });

        let enabled = encode_request(&req, "deepseek-v4");
        assert_eq!(enabled["thinking"], json!({ "type": "enabled" }));
        assert_eq!(enabled["reasoning_effort"], json!("high"));
        assert_eq!(enabled["max_tokens"], json!(2048));
        assert!(enabled.get("max_completion_tokens").is_none());

        req.reasoning.as_mut().expect("reasoning").effort = Some("max".to_string());
        let max = encode_request(&req, "deepseek-v4");
        assert_eq!(max["reasoning_effort"], json!("max"));

        req.reasoning.as_mut().expect("reasoning").effort = Some("none".to_string());
        let disabled = encode_request(&req, "deepseek-v4");
        assert_eq!(disabled["thinking"], json!({ "type": "disabled" }));
        assert!(disabled.get("reasoning_effort").is_none());
    }

    #[test]
    fn native_reasoning_object_is_the_single_chat_effort_container() {
        let req = decode_chat::decode_request(&json!({
            "model": "openai/gpt-5.4",
            "messages": [{ "role": "user", "content": "hello" }],
            "reasoning_effort": "high",
            "reasoning": {
                "effort": "low",
                "max_tokens": 4096,
                "exclude": true,
                "vendor_flag": "keep"
            }
        }))
        .expect("decode Chat request with native reasoning object");

        let encoded = encode_request(&req, "openai/gpt-5.4");
        assert_eq!(encoded["reasoning"]["effort"], json!("high"));
        assert_eq!(encoded["reasoning"]["max_tokens"], json!(4096));
        assert_eq!(encoded["reasoning"]["exclude"], json!(true));
        assert_eq!(encoded["reasoning"]["vendor_flag"], json!("keep"));
        assert!(encoded.get("reasoning_effort").is_none());
    }

    #[test]
    fn deepseek_normalized_effort_overrides_conflicting_raw_thinking() {
        let disabled = decode_chat::decode_request(&json!({
            "model": "deepseek-v4",
            "messages": [{ "role": "user", "content": "hello" }],
            "reasoning_effort": "none",
            "thinking": { "type": "enabled", "vendor_flag": true }
        }))
        .expect("decode disabled DeepSeek request");
        let disabled = encode_request(&disabled, "deepseek-v4");
        assert_eq!(disabled["thinking"], json!({ "type": "disabled" }));
        assert!(disabled.get("reasoning_effort").is_none());

        let enabled = decode_chat::decode_request(&json!({
            "model": "deepseek-v4",
            "messages": [{ "role": "user", "content": "hello" }],
            "reasoning_effort": "high",
            "reasoning": { "effort": "low", "vendor_flag": true },
            "thinking": { "type": "disabled", "vendor_flag": true }
        }))
        .expect("decode enabled DeepSeek request");
        let enabled = encode_request(&enabled, "deepseek-v4");
        assert_eq!(enabled["thinking"], json!({ "type": "enabled" }));
        assert_eq!(enabled["reasoning_effort"], json!("high"));
        assert!(enabled.get("reasoning").is_none());

        let raw_only = decode_chat::decode_request(&json!({
            "model": "deepseek-v4",
            "messages": [{ "role": "user", "content": "hello" }],
            "thinking": { "type": "enabled", "vendor_flag": true }
        }))
        .expect("decode raw-only DeepSeek request");
        let raw_only = encode_request(&raw_only, "deepseek-v4");
        assert_eq!(
            raw_only["thinking"],
            json!({ "type": "enabled", "vendor_flag": true })
        );
        assert!(raw_only.get("reasoning_effort").is_none());

        let raw_reasoning_only = decode_chat::decode_request(&json!({
            "model": "deepseek-v4",
            "messages": [{ "role": "user", "content": "hello" }],
            "reasoning": { "summary": "detailed", "vendor_flag": true }
        }))
        .expect("decode raw-reasoning-only DeepSeek request");
        let raw_reasoning_only = encode_request(&raw_reasoning_only, "deepseek-v4");
        assert_eq!(
            raw_reasoning_only["reasoning"],
            json!({ "summary": "detailed", "vendor_flag": true })
        );
        assert!(raw_reasoning_only.get("thinking").is_none());
        assert!(raw_reasoning_only.get("reasoning_effort").is_none());
    }

    #[test]
    fn deepseek_tool_loop_replays_reasoning_content_without_openrouter_aliases() {
        let downstream = json!({
            "model": "deepseek-v4",
            "messages": [
                { "role": "user", "content": "lookup" },
                {
                    "role": "assistant",
                    "content": null,
                    "reasoning_content": "private tool reasoning",
                    "tool_calls": [{
                        "id": "call_1",
                        "type": "function",
                        "function": { "name": "lookup", "arguments": "{}" }
                    }]
                },
                { "role": "tool", "tool_call_id": "call_1", "content": "ok" }
            ]
        });

        let decoded = decode_chat::decode_request(&downstream).expect("decode DeepSeek history");
        let encoded = encode_request(&decoded, "deepseek-v4");
        let assistant = encoded["messages"]
            .as_array()
            .expect("messages")
            .iter()
            .find(|message| message.get("tool_calls").is_some())
            .expect("assistant tool-call message");

        assert_eq!(
            assistant["reasoning_content"],
            json!("private tool reasoning")
        );
        assert!(assistant.get("reasoning").is_none());
        assert!(assistant.get("reasoning_details").is_none());
    }

    #[test]
    fn openai_chat_custom_tool_round_trips() {
        let downstream = json!({
            "model": "gpt-5.4",
            "messages": [{ "role": "user", "content": "use the grammar" }],
            "tools": [{
                "type": "custom",
                "custom": {
                    "name": "freeform_nested",
                    "description": "Nested custom tool",
                    "format": {
                        "type": "grammar",
                        "grammar": {
                            "syntax": "lark",
                            "definition": "start: /[a-z]+/"
                        }
                    },
                    "x_custom": 7
                },
                "cache_control": { "type": "ephemeral" }
            }]
        });

        let decoded = decode_chat::decode_request(&downstream).expect("decode chat request");
        let decoded_tools = decoded.tools.as_ref().expect("tools decoded");
        let decoded_custom = decoded_tools[0].custom.as_ref().expect("custom IR");
        assert_eq!(decoded_custom.name, "freeform_nested");
        assert_eq!(decoded_custom.extra_body.get("x_custom"), Some(&json!(7)));
        assert_eq!(
            decoded_tools[0].extra_body.get("cache_control"),
            Some(&json!({ "type": "ephemeral" }))
        );

        let encoded = encode_request(&decoded, "gpt-5.4");
        let encoded_tools = encoded["tools"].as_array().expect("encoded tools");
        assert_eq!(encoded_tools.len(), 1);

        let tool = encoded_tools[0].as_object().expect("tool object");
        assert_eq!(tool.get("type"), Some(&json!("custom")));
        assert!(tool.get("name").is_none());
        assert_eq!(
            tool.get("cache_control"),
            Some(&json!({ "type": "ephemeral" }))
        );
        assert!(tool.get("x_custom").is_none());

        let custom = tool
            .get("custom")
            .and_then(Value::as_object)
            .expect("nested custom object");
        assert_eq!(custom.get("name"), Some(&json!("freeform_nested")));
        assert_eq!(
            custom.get("description"),
            Some(&json!("Nested custom tool"))
        );
        assert_eq!(custom.get("x_custom"), Some(&json!(7)));
        assert_eq!(
            custom.get("format"),
            Some(&json!({
                "type": "grammar",
                "grammar": {
                    "syntax": "lark",
                    "definition": "start: /[a-z]+/"
                }
            }))
        );
    }

    #[test]
    fn openai_chat_unsupported_builtin_tool_is_not_blindly_emitted() {
        let downstream = json!({
            "model": "gpt-5.4",
            "messages": [{ "role": "user", "content": "search if allowed" }],
            "tools": [
                {
                    "type": "file_search",
                    "name": "docs_search",
                    "vector_store_ids": ["vs_1"]
                },
                {
                    "type": "function",
                    "function": {
                        "name": "lookup",
                        "description": "Lookup docs",
                        "parameters": {
                            "type": "object",
                            "properties": {
                                "query": { "type": "string" }
                            }
                        }
                    }
                }
            ]
        });

        let decoded = decode_chat::decode_request(&downstream).expect("decode chat request");
        assert_eq!(decoded.tools.as_ref().expect("decoded tools").len(), 2);

        let encoded = encode_request(&decoded, "gpt-5.4");
        let encoded_tools = encoded["tools"].as_array().expect("encoded tools");
        assert_eq!(encoded_tools.len(), 1);
        assert!(
            encoded_tools
                .iter()
                .all(|tool| { tool.get("type").and_then(Value::as_str) != Some("file_search") })
        );
        assert_eq!(encoded_tools[0]["type"], json!("function"));
        assert_eq!(encoded_tools[0]["function"]["name"], json!("lookup"));
    }

    #[test]
    fn keeps_array_content_when_single_text_block_has_extra_fields() {
        let mut part_extra = HashMap::new();
        part_extra.insert("cache_control".to_string(), json!({ "type": "ephemeral" }));

        let req = base_request(vec![Item::Message {
            id: None,
            role: Role::User,
            parts: vec![Part::Text {
                content: "hello".to_string(),
                extra_body: part_extra,
            }],
            extra_body: empty_map(),
        }]);

        let encoded = encode_request(&req, "claude-haiku-4.5");
        let msg = encoded["messages"][0].as_object().expect("message object");
        let content = msg.get("content").expect("content present");
        let block = content
            .as_array()
            .and_then(|arr| arr.first())
            .and_then(|v| v.as_object())
            .expect("content should remain block array");

        assert_eq!(block.get("type"), Some(&Value::String("text".to_string())));
        assert_eq!(block.get("text"), Some(&Value::String("hello".to_string())));
        assert_eq!(
            block.get("cache_control"),
            Some(&json!({ "type": "ephemeral" }))
        );
    }

    #[test]
    fn still_collapses_single_plain_text_block_to_string() {
        let req = base_request(vec![Item::Message {
            id: None,
            role: Role::User,
            parts: vec![Part::Text {
                content: "hello".to_string(),
                extra_body: empty_map(),
            }],
            extra_body: empty_map(),
        }]);

        let encoded = encode_request(&req, "claude-haiku-4.5");
        assert_eq!(
            encoded["messages"][0]["content"],
            Value::String("hello".to_string())
        );
    }

    #[test]
    fn chat_unknown_content_part_round_trips_only_for_chat_protocol() {
        let downstream = json!({
            "model": "gpt-5.4",
            "messages": [{
                "role": "user",
                "content": [{
                    "type": "vendor_part",
                    "payload": { "x": 1 }
                }]
            }]
        });

        let decoded = decode_chat::decode_request(&downstream).expect("decode chat request");
        assert!(matches!(
            &decoded.input[0],
            Node::ProviderItem {
                origin_protocol: ProviderProtocol::ChatCompletion,
                role: OrdinaryRole::User,
                item_type,
                body,
                ..
            } if item_type == "vendor_part"
                && body == &downstream["messages"][0]["content"][0]
        ));

        let encoded = encode_request(&decoded, "gpt-5.4");
        assert_eq!(
            encoded["messages"][0]["content"][0],
            downstream["messages"][0]["content"][0]
        );
    }

    #[test]
    fn chat_provider_part_filters_nested_internal_metadata_on_wire() {
        let downstream = json!({
            "model": "gpt-5.4",
            "messages": [{
                "role": "user",
                "content": [{
                    "type": "vendor_part",
                    "payload": {
                        "keep": 1,
                        "_monoize_nested": "drop",
                        "rows": [{ "keep_row": true, "_monoize_row": "drop" }]
                    },
                    "_monoize_top": "drop"
                }]
            }]
        });

        let decoded = decode_chat::decode_request(&downstream).expect("decode chat request");
        let encoded = encode_request(&decoded, "gpt-5.4");

        assert_eq!(
            encoded["messages"][0]["content"][0],
            json!({
                "type": "vendor_part",
                "payload": { "keep": 1, "rows": [{ "keep_row": true }] }
            })
        );
        assert!(matches!(
            &decoded.input[0],
            Node::ProviderItem { body, .. } if body == &downstream["messages"][0]["content"][0]
        ));
    }

    #[test]
    fn chat_encoder_ignores_cross_protocol_provider_item_without_textifying() {
        let req = UrpRequest {
            model: "logical-model".to_string(),
            input: vec![Node::ProviderItem {
                id: Some("cmp_1".to_string()),
                origin_protocol: ProviderProtocol::Responses,
                role: OrdinaryRole::User,
                item_type: "compaction".to_string(),
                body: json!({
                    "type": "compaction",
                    "encrypted_content": "opaque"
                }),
                extra_body: empty_map(),
            }],
            stream: None,
            temperature: None,
            top_p: None,
            max_output_tokens: None,
            reasoning: None,
            tools: None,
            tool_choice: None,
            parallel_tool_calls: None,
            response_format: None,
            user: None,
            extra_body: empty_map(),
        };

        let encoded = encode_request(&req, "gpt-5.4");
        let wire = serde_json::to_string(&encoded).expect("chat json");
        assert_eq!(encoded["messages"], json!([]));
        assert!(!wire.contains("compaction"));
        assert!(!wire.contains("opaque"));
    }

    #[test]
    fn preserves_assistant_content_and_tool_calls_in_one_message() {
        let req = base_request(vec![Item::Message {
            id: None,
            role: Role::Assistant,
            parts: vec![
                Part::Text {
                    content: "prep".to_string(),
                    extra_body: empty_map(),
                },
                Part::ToolCall {
                    id: None,
                    call_id: "call_1".to_string(),
                    name: "tool".to_string(),
                    arguments: "{}".to_string(),
                    extra_body: empty_map(),
                },
                Part::Text {
                    content: "answer".to_string(),
                    extra_body: empty_map(),
                },
            ],
            extra_body: empty_map(),
        }]);

        let encoded = encode_request(&req, "gpt-5.4");
        let messages = encoded["messages"].as_array().expect("messages array");

        assert_eq!(messages.len(), 2);
        assert_eq!(messages[0]["role"], json!("assistant"));
        assert_eq!(
            messages[0]["tool_calls"][0]["function"]["name"],
            json!("tool")
        );
        assert_eq!(messages[0]["content"], json!("prep"));
        assert_eq!(messages[1]["role"], json!("assistant"));
        assert_eq!(messages[1]["content"], json!("answer"));
    }

    #[test]
    fn encodes_tool_result_items_as_tool_messages_with_text_only_content() {
        let req = base_request(vec![Item::ToolResult {
            id: None,
            call_id: "call_1".to_string(),
            is_error: false,
            content: vec![
                ToolResultContent::Text {
                    text: "hello".to_string(),
                    extra_body: HashMap::new(),
                },
                ToolResultContent::Image {
                    source: ImageSource::Url {
                        url: "https://example.com/image.png".to_string(),
                        detail: None,
                    },
                    extra_body: HashMap::new(),
                },
                ToolResultContent::Text {
                    text: " world".to_string(),
                    extra_body: HashMap::new(),
                },
            ],
            extra_body: HashMap::from([("provider_field".to_string(), json!(true))]),
        }]);

        let encoded = encode_request(&req, "gpt-5.4");
        let msg = encoded["messages"][0].as_object().expect("message object");

        assert_eq!(msg.get("role"), Some(&json!("tool")));
        assert_eq!(msg.get("tool_call_id"), Some(&json!("call_1")));
        assert_eq!(msg.get("content"), Some(&json!("hello world")));
        assert_eq!(msg.get("provider_field"), Some(&json!(true)));
    }

    #[test]
    fn chat_usage_round_trips_all_typed_usage_fields_without_extra_leakage() {
        let mut usage_extra = HashMap::new();
        usage_extra.insert("provider_specific".to_string(), json!(true));
        let response = UrpResponse {
            id: "chatcmpl_usage".to_string(),
            model: "gpt-5.4".to_string(),
            created_at: None,
            output: items_to_nodes(vec![Item::new_message(Role::Assistant)]),
            finish_reason: Some(FinishReason::Stop),
            usage: Some(Usage {
                input_tokens: 20,
                output_tokens: 10,
                input_details: Some(InputDetails {
                    standard_tokens: 0,
                    cache_read_tokens: 1,
                    cache_read_modality_breakdown: None,
                    cache_creation_tokens: 2,
                    cache_creation_5m_tokens: 0,
                    cache_creation_1h_tokens: 0,
                    tool_prompt_tokens: 3,
                    modality_breakdown: None,
                }),
                output_details: Some(OutputDetails {
                    standard_tokens: 0,
                    reasoning_tokens: 4,
                    accepted_prediction_tokens: 5,
                    rejected_prediction_tokens: 6,
                    modality_breakdown: None,
                }),
                extra_body: usage_extra,
            }),
            extra_body: empty_map(),
        };

        let encoded = encode_response(&response, "gpt-5.4");
        assert_eq!(
            encoded["usage"]["prompt_tokens_details"]["cached_tokens"],
            json!(1)
        );
        assert_eq!(
            encoded["usage"]["prompt_tokens_details"]["cache_creation_tokens"],
            json!(2)
        );
        assert_eq!(
            encoded["usage"]["prompt_tokens_details"]["tool_prompt_tokens"],
            json!(3)
        );
        assert_eq!(
            encoded["usage"]["completion_tokens_details"]["reasoning_tokens"],
            json!(4)
        );
        assert_eq!(
            encoded["usage"]["completion_tokens_details"]["accepted_prediction_tokens"],
            json!(5)
        );
        assert_eq!(
            encoded["usage"]["completion_tokens_details"]["rejected_prediction_tokens"],
            json!(6)
        );

        let decoded = decode_chat::decode_response(&encoded).expect("decode response");
        let decoded_usage = decoded.usage.expect("usage should decode");
        let input = decoded_usage.input_details.expect("input details");
        let output = decoded_usage.output_details.expect("output details");
        assert_eq!(input.cache_read_tokens, 1);
        assert_eq!(input.cache_creation_tokens, 2);
        assert_eq!(input.tool_prompt_tokens, 3);
        assert_eq!(output.reasoning_tokens, 4);
        assert_eq!(output.accepted_prediction_tokens, 5);
        assert_eq!(output.rejected_prediction_tokens, 6);
        assert!(
            !decoded_usage
                .extra_body
                .contains_key("prompt_tokens_details")
        );
        assert!(
            !decoded_usage
                .extra_body
                .contains_key("completion_tokens_details")
        );
        assert_eq!(
            decoded_usage.extra_body.get("provider_specific"),
            Some(&json!(true))
        );
    }

    #[test]
    fn encode_response_merges_multiple_assistant_segments_into_one_chat_message() {
        let response = UrpResponse {
            id: "chatcmpl_segments".to_string(),
            model: "gpt-5.4".to_string(),
            created_at: None,
            output: items_to_nodes(vec![
                Item::Message {
                    id: None,
                    role: Role::Assistant,
                    parts: vec![Part::Text {
                        content: "prep".to_string(),
                        extra_body: empty_map(),
                    }],
                    extra_body: HashMap::from([("phase".to_string(), json!("analysis"))]),
                },
                Item::Message {
                    id: None,
                    role: Role::Assistant,
                    parts: vec![Part::ToolCall {
                        id: None,
                        call_id: "call_1".to_string(),
                        name: "tool".to_string(),
                        arguments: "{}".to_string(),
                        extra_body: empty_map(),
                    }],
                    extra_body: empty_map(),
                },
                Item::Message {
                    id: None,
                    role: Role::Assistant,
                    parts: vec![
                        Part::Reasoning {
                            id: None,
                            content: Some("think".to_string()),
                            encrypted: Some(json!("sig_1")),
                            summary: None,
                            source: None,
                            extra_body: empty_map(),
                        },
                        Part::Text {
                            content: "answer".to_string(),
                            extra_body: empty_map(),
                        },
                    ],
                    extra_body: HashMap::from([("segment".to_string(), json!(3))]),
                },
            ]),
            finish_reason: Some(FinishReason::ToolCalls),
            usage: None,
            extra_body: empty_map(),
        };

        let encoded = encode_response(&response, "gpt-5.4");
        let message = encoded["choices"][0]["message"]
            .as_object()
            .expect("chat message object");
        assert_eq!(message.get("content"), Some(&json!("prep\n\nanswer")));
        assert_eq!(message["tool_calls"][0]["function"]["name"], json!("tool"));
        assert_eq!(message["reasoning"], json!("think"));
        assert_eq!(
            message["reasoning_details"][1]["type"],
            json!("reasoning.encrypted")
        );
        assert_eq!(message["reasoning_details"][1]["data"], json!("sig_1"));
        assert_eq!(message.get("phase"), Some(&json!("analysis")));
        assert_eq!(message.get("segment"), Some(&json!(3)));
    }

    #[test]
    fn encode_response_keeps_chat_message_content_as_string_when_text_parts_have_phase() {
        let response = UrpResponse {
            id: "chatcmpl_phase_string".to_string(),
            model: "gpt-5.4".to_string(),
            created_at: None,
            output: items_to_nodes(vec![Item::Message {
                id: None,
                role: Role::Assistant,
                parts: vec![
                    Part::Text {
                        content: "analysis".to_string(),
                        extra_body: HashMap::from([("phase".to_string(), json!("commentary"))]),
                    },
                    Part::Text {
                        content: "final".to_string(),
                        extra_body: HashMap::from([("phase".to_string(), json!("final_answer"))]),
                    },
                ],
                extra_body: empty_map(),
            }]),
            finish_reason: Some(FinishReason::Stop),
            usage: None,
            extra_body: empty_map(),
        };

        let encoded = encode_response(&response, "gpt-5.4");
        assert_eq!(
            encoded["choices"][0]["message"]["content"],
            json!("analysis\n\nfinal")
        );
        assert!(encoded["choices"][0]["message"]["content"].is_string());
    }

    #[test]
    fn chat_response_round_trip_preserves_reasoning_summary_and_signature() {
        let response = UrpResponse {
            id: "chatcmpl_roundtrip_reasoning".to_string(),
            model: "gpt-5.4".to_string(),
            created_at: None,
            output: items_to_nodes(vec![Item::Message {
                id: None,
                role: Role::Assistant,
                parts: vec![Part::Reasoning {
                    id: None,
                    content: Some("full reasoning".to_string()),
                    encrypted: Some(json!("sig_1")),
                    summary: Some("brief summary".to_string()),
                    source: Some("openrouter".to_string()),
                    extra_body: empty_map(),
                }],
                extra_body: empty_map(),
            }]),
            finish_reason: Some(FinishReason::Stop),
            usage: None,
            extra_body: empty_map(),
        };

        let encoded = encode_response(&response, "gpt-5.4");
        let decoded = decode_chat::decode_response(&encoded).expect("decode response");
        let decoded_outputs = nodes_to_items(&decoded.output);
        let Item::Message { parts, .. } = decoded_outputs.first().expect("assistant output") else {
            panic!("expected assistant output");
        };

        assert!(parts.iter().any(|part| matches!(
            part,
            Part::Reasoning {
                summary: Some(summary),
                ..
            } if summary == "brief summary"
        )));
        assert!(parts.iter().any(|part| matches!(
            part,
            Part::Reasoning {
                content: Some(content),
                ..
            } if content == "full reasoning"
        )));
        assert!(parts.iter().any(|part| matches!(
            part,
            Part::Reasoning {
                encrypted: Some(Value::String(sig)),
                ..
            } if sig == "sig_1"
        )));
        let message = &encoded["choices"][0]["message"];
        assert_eq!(message["reasoning"], json!("full reasoning"));
        assert_eq!(
            message["reasoning_details"][0]["format"],
            json!("openrouter")
        );
        assert_eq!(
            message["reasoning_details"][1]["format"],
            json!("openrouter")
        );
        assert_eq!(
            message["reasoning_details"][2]["format"],
            json!("openrouter")
        );
        assert!(message["reasoning_details"][1].get("signature").is_none());
    }

    #[test]
    fn chat_response_uses_summary_as_reasoning_alias_when_text_is_absent() {
        let response = UrpResponse {
            id: "chatcmpl_summary_alias".to_string(),
            model: "gpt-5.4".to_string(),
            created_at: None,
            output: items_to_nodes(vec![Item::Message {
                id: None,
                role: Role::Assistant,
                parts: vec![Part::Reasoning {
                    id: None,
                    content: None,
                    encrypted: Some(json!("sig_only_summary")),
                    summary: Some("brief summary only".to_string()),
                    source: Some("openrouter".to_string()),
                    extra_body: empty_map(),
                }],
                extra_body: empty_map(),
            }]),
            finish_reason: Some(FinishReason::Stop),
            usage: None,
            extra_body: empty_map(),
        };

        let encoded = encode_response(&response, "gpt-5.4");
        let message = &encoded["choices"][0]["message"];
        assert_eq!(message["reasoning"], json!("brief summary only"));
        let details = message["reasoning_details"]
            .as_array()
            .expect("details array");
        assert!(details.iter().any(|detail| {
            detail["type"].as_str() == Some("reasoning.summary")
                && detail["summary"].as_str() == Some("brief summary only")
        }));
        assert!(details.iter().any(|detail| {
            detail["type"].as_str() == Some("reasoning.encrypted")
                && detail["data"].as_str() == Some("sig_only_summary")
        }));
    }

    #[test]
    fn merge_assistant_chat_messages_preserves_reasoning_details_across_segments() {
        let response = UrpResponse {
            id: "chatcmpl_merge_reasoning_parts".to_string(),
            model: "gpt-5.4".to_string(),
            created_at: None,
            output: items_to_nodes(vec![
                Item::Message {
                    id: None,
                    role: Role::Assistant,
                    parts: vec![Part::Reasoning {
                        id: None,
                        content: Some("segment reasoning".to_string()),
                        encrypted: None,
                        summary: None,
                        source: None,
                        extra_body: empty_map(),
                    }],
                    extra_body: empty_map(),
                },
                Item::Message {
                    id: None,
                    role: Role::Assistant,
                    parts: vec![Part::Reasoning {
                        id: None,
                        content: None,
                        encrypted: Some(json!("sig_merged")),
                        summary: None,
                        source: None,
                        extra_body: empty_map(),
                    }],
                    extra_body: empty_map(),
                },
            ]),
            finish_reason: Some(FinishReason::Stop),
            usage: None,
            extra_body: empty_map(),
        };

        let encoded = encode_response(&response, "gpt-5.4");
        let message = &encoded["choices"][0]["message"];
        assert_eq!(message["reasoning"], json!("segment reasoning"));
        let details = message["reasoning_details"]
            .as_array()
            .expect("reasoning details");
        assert!(details.iter().any(|detail| {
            detail["type"].as_str() == Some("reasoning.text")
                && detail["text"].as_str() == Some("segment reasoning")
        }));
        assert!(details.iter().any(|detail| {
            detail["type"].as_str() == Some("reasoning.encrypted")
                && detail["data"].as_str() == Some("sig_merged")
        }));
    }

    #[test]
    fn chat_response_keeps_encrypted_reasoning_from_multiple_segments() {
        let response = UrpResponse {
            id: "chatcmpl_multi_encrypted".to_string(),
            model: "gpt-5.4".to_string(),
            created_at: None,
            output: items_to_nodes(vec![
                Item::Message {
                    id: None,
                    role: Role::Assistant,
                    parts: vec![Part::Reasoning {
                        id: None,
                        content: Some("segment reasoning".to_string()),
                        encrypted: Some(json!("sig_first")),
                        summary: None,
                        source: Some("openrouter".to_string()),
                        extra_body: empty_map(),
                    }],
                    extra_body: empty_map(),
                },
                Item::Message {
                    id: None,
                    role: Role::Assistant,
                    parts: vec![Part::Reasoning {
                        id: None,
                        content: None,
                        encrypted: Some(json!("sig_second")),
                        summary: None,
                        source: Some("openrouter".to_string()),
                        extra_body: empty_map(),
                    }],
                    extra_body: empty_map(),
                },
            ]),
            finish_reason: Some(FinishReason::Stop),
            usage: None,
            extra_body: empty_map(),
        };

        let encoded = encode_response(&response, "gpt-5.4");
        let details = encoded["choices"][0]["message"]["reasoning_details"]
            .as_array()
            .expect("reasoning details");

        let encrypted = details
            .iter()
            .filter(|detail| detail["type"].as_str() == Some("reasoning.encrypted"))
            .collect::<Vec<_>>();

        assert_eq!(encrypted.len(), 2);
        assert!(encrypted.iter().any(|detail| {
            detail["data"].as_str() == Some("sig_first")
                && detail["format"].as_str() == Some("openrouter")
        }));
        assert!(encrypted.iter().any(|detail| {
            detail["data"].as_str() == Some("sig_second")
                && detail["format"].as_str() == Some("openrouter")
        }));
    }

    #[test]
    fn chat_response_emits_reasoning_content_when_openwebui_transform_marks_summary() {
        let response = UrpResponse {
            id: "chatcmpl_reasoning_content".to_string(),
            model: "gpt-5.4".to_string(),
            created_at: None,
            output: items_to_nodes(vec![Item::Message {
                id: None,
                role: Role::Assistant,
                parts: vec![Part::Reasoning {
                    id: None,
                    content: Some("full reasoning".to_string()),
                    encrypted: None,
                    summary: Some("brief summary".to_string()),
                    source: None,
                    extra_body: HashMap::from([(
                        "openwebui_reasoning_content".to_string(),
                        json!(true),
                    )]),
                }],
                extra_body: empty_map(),
            }]),
            finish_reason: Some(FinishReason::Stop),
            usage: None,
            extra_body: empty_map(),
        };

        let encoded = encode_response(&response, "gpt-5.4");
        let message = &encoded["choices"][0]["message"];
        assert_eq!(message["reasoning_content"].as_str(), Some("brief summary"));
        assert_eq!(
            message["reasoning_details"][0]["type"].as_str(),
            Some("reasoning.summary")
        );
    }

    #[test]
    fn round_trip_real_upstream_gpt5_chat_payload_keeps_encrypted_reasoning() {
        let upstream = json!({
            "id": "resp_real_shape",
            "object": "chat.completion",
            "created": 1773667800i64,
            "model": "gpt-5.4-2026-03-05",
            "choices": [{
                "index": 0,
                "message": {
                    "role": "assistant",
                    "content": "One valid combination is 8 packs of pencils and 4 packs of pens.",
                    "reasoning": "plain reasoning",
                    "reasoning_content": "plain reasoning",
                    "reasoning_details": [
                        {
                            "type": "reasoning.text",
                            "text": "plain reasoning"
                        },
                        {
                            "type": "reasoning.encrypted",
                            "data": "opaque_sig_payload"
                        }
                    ],
                    "reasoning_opaque": "opaque_sig_payload"
                },
                "finish_reason": "stop"
            }],
            "usage": {
                "prompt_tokens": 52,
                "completion_tokens": 287,
                "total_tokens": 339,
                "prompt_tokens_details": { "cached_tokens": 0 },
                "completion_tokens_details": { "reasoning_tokens": 210 }
            }
        });

        let decoded = decode_chat::decode_response(&upstream).expect("decode response");
        let reencoded = encode_response(&decoded, "gpt-5.4");
        assert_eq!(reencoded["created"], json!(1773667800i64));
        assert_eq!(
            reencoded["choices"][0]["message"]["reasoning"],
            json!("plain reasoning")
        );
        let details = reencoded["choices"][0]["message"]["reasoning_details"]
            .as_array()
            .expect("reasoning details array");

        assert!(details.iter().any(|detail| {
            detail["type"].as_str() == Some("reasoning.text")
                && detail["text"].as_str() == Some("plain reasoning")
        }));
        assert!(details.iter().any(|detail| {
            detail["type"].as_str() == Some("reasoning.encrypted")
                && detail["data"].as_str() == Some("opaque_sig_payload")
        }));
    }

    #[test]
    fn deepseek_insufficient_system_resource_round_trips_with_choice_extras() {
        let upstream = json!({
            "id": "chatcmpl_deepseek",
            "object": "chat.completion",
            "created": 1773667800i64,
            "model": "deepseek-v4",
            "choices": [{
                "index": 0,
                "message": { "role": "assistant", "content": "partial" },
                "finish_reason": "insufficient_system_resource",
                "native_finish_reason": "insufficient_system_resource",
                "provider_marker": "deepseek"
            }]
        });

        let decoded = decode_chat::decode_response(&upstream).expect("decode response");
        let reencoded = encode_response(&decoded, "deepseek-v4");

        assert_eq!(
            reencoded["choices"][0]["finish_reason"],
            json!("insufficient_system_resource")
        );
        assert_eq!(
            reencoded["choices"][0]["native_finish_reason"],
            json!("insufficient_system_resource")
        );
        assert_eq!(
            reencoded["choices"][0]["provider_marker"],
            json!("deepseek")
        );
        assert!(reencoded.get(CHAT_NATIVE_FINISH_REASON_EXTRA_KEY).is_none());
        assert!(reencoded.get(CHAT_CHOICE_EXTRA_BODY_KEY).is_none());
    }
}
