use crate::urp::encode::{
    extract_reasoning_plain, merge_extra, role_to_str, text_parts, tool_choice_to_value,
    usage_input_details, usage_output_details,
};
use crate::urp::{
    FileSource, FinishReason, ImageSource, Item, Part, ResponseFormat, Role, ToolDefinition,
    ToolResultContent, UrpRequest, UrpResponse,
};
use serde_json::{json, Map, Value};
use std::collections::HashMap;

struct PendingChatMessage {
    role: Role,
    content_parts: Vec<Value>,
    tool_calls: Vec<Value>,
    refusal: Option<String>,
    reasoning_parts: Vec<Part>,
    message_extra: HashMap<String, Value>,
}

fn is_message_content_part(part: &Part) -> bool {
    matches!(
        part,
        Part::Text { .. } | Part::Image { .. } | Part::File { .. } | Part::Refusal { .. }
    )
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
                merge_extra(obj, extra_body);
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
            };
            if let Some(obj) = image.as_object_mut() {
                merge_extra(obj, extra_body);
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
            };
            let mut block = json!({ "type": "text", "text": text });
            if let Some(obj) = block.as_object_mut() {
                merge_extra(obj, extra_body);
            }
            Some(block)
        }
        _ => None,
    }
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
    insert_openrouter_reasoning_fields(&mut m, &pending_msg.reasoning_parts);
    merge_extra(&mut m, &pending_msg.message_extra);
    out.push(Value::Object(m));
}

fn should_split_chat_message(existing: &PendingChatMessage, part: &Part) -> bool {
    if matches!(part, Part::ToolCall { .. }) && !existing.content_parts.is_empty() {
        return true;
    }
    if is_message_content_part(part) && !existing.tool_calls.is_empty() {
        return true;
    }
    let _ = existing;
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
        Part::Text { .. } | Part::Image { .. } | Part::File { .. } => {
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
    let mut body = json!({
        "model": upstream_model,
        "messages": encode_messages(&req.inputs),
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
        obj.insert("max_completion_tokens".to_string(), Value::from(max));
    }
    if let Some(reasoning) = &req.reasoning {
        if let Some(effort) = &reasoning.effort {
            obj.insert(
                "reasoning_effort".to_string(),
                Value::String(effort.clone()),
            );
        }
    }
    if let Some(tools) = &req.tools {
        obj.insert("tools".to_string(), Value::Array(encode_tools(tools)));
    }
    if let Some(tc) = &req.tool_choice {
        obj.insert("tool_choice".to_string(), tool_choice_to_value(tc));
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

    merge_extra(obj, &req.extra_body);
    body
}

pub fn encode_response(resp: &UrpResponse, logical_model: &str) -> Value {
    let mut assistant_messages = Vec::new();
    for item in &resp.outputs {
        match item {
            Item::Message {
                role: Role::Assistant,
                ..
            } => assistant_messages.push(item.clone()),
            Item::ToolResult { .. } | Item::Message { .. } => continue,
        }
    }

    let message = merge_assistant_chat_messages(&assistant_messages);

    let finish_reason = resp
        .finish_reason
        .map(finish_reason_to_chat)
        .unwrap_or_else(|| {
            if assistant_messages.iter().any(has_tool_calls) {
                "tool_calls"
            } else {
                "stop"
            }
        });

    let mut result = json!({
        "id": resp.id,
        "object": "chat.completion",
        "created": chrono::Utc::now().timestamp(),
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
                obj.insert(k.clone(), v.clone());
            }
        }
        result["usage"] = usage_value;
    }

    let obj = result.as_object_mut().expect("chat response object");
    merge_extra(obj, &resp.extra_body);
    result
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
                        ToolResultContent::Text { text } => Some(text.as_str()),
                        _ => None,
                    })
                    .collect::<Vec<_>>()
                    .join("");
                let mut m = Map::new();
                m.insert("role".to_string(), Value::String("tool".to_string()));
                m.insert("content".to_string(), Value::String(text));
                m.insert("tool_call_id".to_string(), Value::String(call_id.clone()));
                merge_extra(&mut m, extra_body);
                out.push(Value::Object(m));
            }
            Item::Message {
                role,
                parts,
                extra_body,
            } => {
                if *role == Role::Tool {
                    let mut m = Map::new();
                    m.insert("role".to_string(), Value::String("tool".to_string()));
                    m.insert("content".to_string(), Value::String(text_parts(parts)));
                    merge_extra(&mut m, extra_body);
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
                super::merge_extra(&mut fn_obj, &function.extra_body);

                let mut obj = Map::new();
                obj.insert("type".to_string(), Value::String("function".to_string()));
                obj.insert("function".to_string(), Value::Object(fn_obj));
                super::merge_extra(&mut obj, &tool.extra_body);
                out.push(Value::Object(obj));
            }
        } else {
            let mut obj = Map::new();
            obj.insert("type".to_string(), Value::String(tool.tool_type.clone()));
            super::merge_extra(&mut obj, &tool.extra_body);
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
            super::merge_extra(&mut schema_obj, &json_schema.extra_body);
            json!({
                "type": "json_schema",
                "json_schema": Value::Object(schema_obj),
            })
        }
    }
}

fn has_tool_calls(item: &Item) -> bool {
    match item {
        Item::Message { parts, .. } => parts.iter().any(|p| matches!(p, Part::ToolCall { .. })),
        Item::ToolResult { .. } => false,
    }
}

fn insert_openrouter_reasoning_fields(message: &mut Map<String, Value>, parts: &[Part]) {
    let reasoning_text = extract_reasoning_plain(parts);
    let encrypted = super::extract_reasoning_encrypted(parts);
    let mut details = Vec::new();

    if !reasoning_text.is_empty() {
        message.insert(
            "reasoning".to_string(),
            Value::String(reasoning_text.clone()),
        );

        let signature = encrypted.as_ref().and_then(|v| v.as_str());
        details.push(json!({
            "type": "reasoning.text",
            "text": reasoning_text,
            "signature": signature,
            "format": "unknown"
        }));

        if let Some(enc) = encrypted {
            if !enc.is_string() && !matches!(enc, Value::Null) {
                details.push(json!({
                    "type": "reasoning.encrypted",
                    "data": enc,
                    "format": "unknown"
                }));
            }
        }
    } else if let Some(enc) = encrypted {
        if !matches!(enc, Value::Null) {
            if let Some(s) = enc.as_str() {
                if s.is_empty() {
                    return;
                }
            }
            details.push(json!({
                "type": "reasoning.encrypted",
                "data": enc,
                "format": "unknown"
            }));
        }
    }

    if !details.is_empty() {
        message.insert("reasoning_details".to_string(), Value::Array(details));
    }
}

fn merge_assistant_chat_messages(assistant_messages: &[Item]) -> Map<String, Value> {
    let encoded_messages = encode_messages(assistant_messages);
    if encoded_messages.is_empty() {
        let mut fallback = Map::new();
        fallback.insert("role".to_string(), Value::String("assistant".to_string()));
        fallback.insert("content".to_string(), Value::String(String::new()));
        if let Some(Item::Message { extra_body, .. }) = assistant_messages.first() {
            merge_extra(&mut fallback, extra_body);
        }
        return fallback;
    }

    let mut merged = Map::new();
    merged.insert("role".to_string(), Value::String("assistant".to_string()));

    let mut merged_content_parts = Vec::new();
    let mut merged_tool_calls = Vec::new();
    let mut merged_reasoning_details = Vec::new();
    let mut refusal: Option<String> = None;

    for encoded in encoded_messages {
        let Some(obj) = encoded.as_object() else {
            continue;
        };

        merge_message_content_parts(obj.get("content"), &mut merged_content_parts);

        if let Some(tool_calls) = obj.get("tool_calls").and_then(|v| v.as_array()) {
            merged_tool_calls.extend(tool_calls.iter().cloned());
        }

        if let Some(details) = obj.get("reasoning_details").and_then(|v| v.as_array()) {
            merged_reasoning_details.extend(details.iter().cloned());
        }

        if let Some(reasoning) = obj.get("reasoning") {
            merged
                .entry("reasoning".to_string())
                .or_insert_with(|| reasoning.clone());
        }

        if refusal.is_none() {
            refusal = obj
                .get("refusal")
                .and_then(|v| v.as_str())
                .filter(|v| !v.is_empty())
                .map(str::to_string);
        }

        for (key, value) in obj {
            if matches!(
                key.as_str(),
                "role" | "content" | "tool_calls" | "refusal" | "reasoning" | "reasoning_details"
            ) {
                continue;
            }
            merged.entry(key.clone()).or_insert_with(|| value.clone());
        }
    }

    finalize_chat_response_content(&mut merged, merged_content_parts);
    if !merged_tool_calls.is_empty() {
        merged.insert("tool_calls".to_string(), Value::Array(merged_tool_calls));
    }
    if let Some(refusal) = refusal {
        merged.insert("refusal".to_string(), Value::String(refusal));
    }
    if !merged_reasoning_details.is_empty() {
        merged.insert(
            "reasoning_details".to_string(),
            Value::Array(merged_reasoning_details),
        );
    }

    merged
}

fn merge_message_content_parts(content: Option<&Value>, out: &mut Vec<Value>) {
    let Some(content) = content else {
        return;
    };

    match content {
        Value::String(text) => {
            if !text.is_empty() {
                out.push(json!({ "type": "text", "text": text }));
            }
        }
        Value::Array(parts) => out.extend(parts.iter().cloned()),
        _ => {}
    }
}

fn finish_reason_to_chat(finish_reason: FinishReason) -> &'static str {
    match finish_reason {
        FinishReason::Stop => "stop",
        FinishReason::Length => "length",
        FinishReason::ToolCalls => "tool_calls",
        FinishReason::ContentFilter => "content_filter",
        FinishReason::Other => "stop",
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::urp::decode::openai_chat as decode_chat;
    use crate::urp::{InputDetails, OutputDetails, UrpResponse, Usage};
    use std::collections::HashMap;

    fn empty_map() -> HashMap<String, Value> {
        HashMap::new()
    }

    fn base_request(messages: Vec<Item>) -> UrpRequest {
        UrpRequest {
            model: "logical-model".to_string(),
            inputs: messages,
            stream: None,
            temperature: None,
            top_p: None,
            max_output_tokens: None,
            reasoning: None,
            tools: None,
            tool_choice: None,
            response_format: None,
            user: None,
            extra_body: empty_map(),
        }
    }

    #[test]
    fn keeps_array_content_when_single_text_block_has_extra_fields() {
        let mut part_extra = HashMap::new();
        part_extra.insert("cache_control".to_string(), json!({ "type": "ephemeral" }));

        let req = base_request(vec![Item::Message {
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
    fn splits_assistant_messages_when_tool_calls_break_messages() {
        let req = base_request(vec![Item::Message {
            role: Role::Assistant,
            parts: vec![
                Part::Text {
                    content: "prep".to_string(),
                    extra_body: empty_map(),
                },
                Part::ToolCall {
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

        assert_eq!(messages.len(), 3);
        assert_eq!(messages[0]["content"], json!("prep"));
        assert_eq!(
            messages[1]["tool_calls"][0]["function"]["name"],
            json!("tool")
        );
        assert_eq!(messages[2]["content"], json!("answer"));
    }

    #[test]
    fn encodes_tool_result_items_as_tool_messages_with_text_only_content() {
        let req = base_request(vec![Item::ToolResult {
            call_id: "call_1".to_string(),
            is_error: false,
            content: vec![
                ToolResultContent::Text {
                    text: "hello".to_string(),
                },
                ToolResultContent::Image {
                    source: ImageSource::Url {
                        url: "https://example.com/image.png".to_string(),
                        detail: None,
                    },
                },
                ToolResultContent::Text {
                    text: " world".to_string(),
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
            outputs: vec![Item::new_message(Role::Assistant)],
            finish_reason: Some(FinishReason::Stop),
            usage: Some(Usage {
                input_tokens: 20,
                output_tokens: 10,
                input_details: Some(InputDetails {
                    standard_tokens: 0,
                    cache_read_tokens: 1,
                    cache_creation_tokens: 2,
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
        assert!(decoded_usage
            .extra_body
            .get("prompt_tokens_details")
            .is_none());
        assert!(decoded_usage
            .extra_body
            .get("completion_tokens_details")
            .is_none());
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
            outputs: vec![
                Item::Message {
                    role: Role::Assistant,
                    parts: vec![Part::Text {
                        content: "prep".to_string(),
                        extra_body: empty_map(),
                    }],
                    extra_body: HashMap::from([("phase".to_string(), json!("analysis"))]),
                },
                Item::Message {
                    role: Role::Assistant,
                    parts: vec![Part::ToolCall {
                        call_id: "call_1".to_string(),
                        name: "tool".to_string(),
                        arguments: "{}".to_string(),
                        extra_body: empty_map(),
                    }],
                    extra_body: empty_map(),
                },
                Item::Message {
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
                            extra_body: empty_map(),
                        },
                    ],
                    extra_body: HashMap::from([("segment".to_string(), json!(3))]),
                },
            ],
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
        assert_eq!(message["reasoning_details"][0]["signature"], json!("sig_1"));
        assert_eq!(message.get("phase"), Some(&json!("analysis")));
        assert_eq!(message.get("segment"), Some(&json!(3)));
    }

    #[test]
    fn encode_response_keeps_chat_message_content_as_string_when_text_parts_have_phase() {
        let response = UrpResponse {
            id: "chatcmpl_phase_string".to_string(),
            model: "gpt-5.4".to_string(),
            outputs: vec![Item::Message {
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
            }],
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
}
