use crate::urp::encode::{
    merge_extra, role_to_str, text_parts, tool_choice_to_value, usage_input_details,
    usage_output_details,
};
use crate::urp::{
    FileSource, FinishReason, ImageSource, Item, Part, ResponseFormat, Role, ToolDefinition,
    ToolResultContent, UrpRequest, UrpResponse,
};
use serde_json::{json, Map, Value};
use std::collections::HashMap;

#[derive(Clone)]
struct PendingResponsesMessageItem {
    role: Role,
    phase: Option<String>,
    content: Vec<Value>,
    extra_body: HashMap<String, Value>,
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
            merge_extra(&mut obj, extra_body);
            Some(Value::Object(obj))
        }
        Part::Image { source, extra_body } => Some(if output_text_type {
            let mut value = encode_output_image(source, extra_body);
            if let Some(obj) = value.as_object_mut() {
                merge_extra(obj, extra_body);
            }
            value
        } else {
            encode_input_image(source, extra_body)
        }),
        Part::File { source, extra_body } => Some(if output_text_type {
            let mut value = encode_output_file(source, extra_body);
            if let Some(obj) = value.as_object_mut() {
                merge_extra(obj, extra_body);
            }
            value
        } else {
            encode_input_file(source, extra_body)
        }),
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

fn encode_reasoning_item(part: &Part) -> Option<Value> {
    match part {
        Part::Reasoning {
            content,
            encrypted,
            summary,
            source,
            extra_body,
        } => {
            let mut obj = Map::new();
            obj.insert("type".to_string(), Value::String("reasoning".to_string()));
            if let Some(summary) = summary.as_ref() {
                obj.insert(
                    "summary".to_string(),
                    Value::Array(vec![json!({ "type": "summary_text", "text": summary })]),
                );
            }
            if let Some(content) = content {
                obj.insert("text".to_string(), Value::String(content.clone()));
            }
            if let Some(encrypted) = encrypted {
                obj.insert("encrypted_content".to_string(), encrypted.clone());
            }
            if let Some(source) = source {
                obj.insert("source".to_string(), Value::String(source.clone()));
            }
            merge_extra(&mut obj, extra_body);
            Some(Value::Object(obj))
        }
        _ => None,
    }
}

fn encode_tool_call_item(part: &Part) -> Option<Value> {
    match part {
        Part::ToolCall {
            call_id,
            name,
            arguments,
            extra_body,
        } => {
            let mut obj = Map::new();
            obj.insert(
                "type".to_string(),
                Value::String("function_call".to_string()),
            );
            obj.insert("call_id".to_string(), Value::String(call_id.clone()));
            obj.insert("name".to_string(), Value::String(name.clone()));
            obj.insert("arguments".to_string(), Value::String(arguments.clone()));
            merge_extra(&mut obj, extra_body);
            Some(Value::Object(obj))
        }
        _ => None,
    }
}

pub fn encode_request(req: &UrpRequest, upstream_model: &str) -> Value {
    let mut input_items = Vec::new();
    let mut instructions: Option<String> = None;
    let mut consumed_instructions = false;

    for item in &req.inputs {
        if !consumed_instructions && can_use_responses_instructions(item) {
            if let Item::Message { parts, .. } = item {
                let text = text_parts(parts);
                if !text.is_empty() {
                    instructions = Some(text);
                    consumed_instructions = true;
                    continue;
                }
            }
        }
        encode_message_to_input_items(item, &mut input_items);
    }

    let mut body = json!({
        "model": upstream_model,
        "input": Value::Array(input_items),
    });
    let obj = body.as_object_mut().expect("responses request object");

    if let Some(text) = instructions {
        obj.insert("instructions".to_string(), Value::String(text));
    }
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
        obj.insert("max_output_tokens".to_string(), Value::from(max));
    }
    if let Some(reasoning) = &req.reasoning {
        let mut reasoning_obj = Map::new();
        if let Some(effort) = &reasoning.effort {
            reasoning_obj.insert("effort".to_string(), Value::String(effort.clone()));
        }
        merge_extra(&mut reasoning_obj, &reasoning.extra_body);
        if !reasoning_obj.is_empty() {
            obj.insert("reasoning".to_string(), Value::Object(reasoning_obj));
        }
    }
    if let Some(tools) = &req.tools {
        obj.insert("tools".to_string(), Value::Array(encode_tools(tools)));
    }
    if let Some(choice) = &req.tool_choice {
        obj.insert("tool_choice".to_string(), tool_choice_to_value(choice));
    }
    if let Some(user) = &req.user {
        obj.insert("user".to_string(), Value::String(user.clone()));
    }
    if let Some(format) = &req.response_format {
        apply_response_format(obj, format);
    }
    merge_extra(obj, &req.extra_body);
    body
}

pub fn encode_response(resp: &UrpResponse, logical_model: &str) -> Value {
    let mut output = Vec::new();
    for item in &resp.outputs {
        match item {
            Item::Message {
                role,
                parts,
                extra_body,
            } => {
                let mut pending_message: Option<PendingResponsesMessageItem> = None;
                for part in parts {
                    if let Some(content_part) = encode_message_content_part(part, true) {
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

                    if let Some(reasoning_item) = encode_reasoning_item(part) {
                        output.push(reasoning_item);
                        continue;
                    }

                    if let Some(tool_call_item) = encode_tool_call_item(part) {
                        output.push(tool_call_item);
                        continue;
                    }

                    if let Part::ProviderItem {
                        item_type,
                        body,
                        extra_body,
                    } = part
                    {
                        let mut item = match body {
                            Value::Object(obj) => obj.clone(),
                            other => {
                                let mut obj = Map::new();
                                obj.insert("body".to_string(), other.clone());
                                obj
                            }
                        };
                        item.entry("type".to_string())
                            .or_insert_with(|| Value::String(item_type.clone()));
                        merge_extra(&mut item, extra_body);
                        output.push(Value::Object(item));
                    }
                }
                flush_pending_message_item(&mut pending_message, &mut output);
            }
            Item::ToolResult {
                call_id,
                content,
                is_error,
                extra_body,
            } => encode_tool_result_item(call_id, content, *is_error, extra_body, &mut output),
        }
    }

    let mut body = json!({
        "id": resp.id,
        "object": "response",
        "created": chrono::Utc::now().timestamp(),
        "model": logical_model,
        "status": finish_reason_to_status(resp.finish_reason),
        "output": output
    });

    if let Some(usage) = &resp.usage {
        let input_details = usage_input_details(usage);
        let output_details = usage_output_details(usage);
        let mut usage_value = json!({
            "input_tokens": usage.input_tokens,
            "output_tokens": usage.output_tokens,
            "total_tokens": usage.total_tokens(),
            "output_tokens_details": {
                "reasoning_tokens": output_details.reasoning_tokens,
                "accepted_prediction_tokens": output_details.accepted_prediction_tokens,
                "rejected_prediction_tokens": output_details.rejected_prediction_tokens
            },
            "input_tokens_details": {
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
        body["usage"] = usage_value;
    }

    if let Some(obj) = body.as_object_mut() {
        merge_extra(obj, &resp.extra_body);
    }
    body
}

fn encode_message_to_input_items(item: &Item, out: &mut Vec<Value>) {
    match item {
        Item::Message {
            role,
            parts,
            extra_body,
        } => {
            let mut pending_message: Option<PendingResponsesMessageItem> = None;

            for part in parts {
                if let Some(content_part) = encode_message_content_part(part, false) {
                    append_content_part_to_pending(
                        &mut pending_message,
                        out,
                        *role,
                        text_part_phase(part),
                        extra_body,
                        content_part,
                    );
                    continue;
                }

                flush_pending_message_item(&mut pending_message, out);

                match part {
                    Part::Reasoning {
                        content: Some(_),
                        encrypted: Some(_),
                        ..
                    } => {
                        let mut reasoning_part = part.clone();
                        if let Part::Reasoning {
                            content, summary, ..
                        } = &mut reasoning_part
                        {
                            *content = None;
                            *summary = None;
                        }
                        if let Some(item) = encode_reasoning_item(&reasoning_part) {
                            out.push(item);
                        }
                    }
                    _ => {
                        if let Some(item) =
                            encode_reasoning_item(part).or_else(|| encode_tool_call_item(part))
                        {
                            out.push(item);
                        }
                    }
                }
            }
            flush_pending_message_item(&mut pending_message, out);
        }
        Item::ToolResult {
            call_id,
            content,
            is_error,
            extra_body,
        } => encode_tool_result_item(call_id, content, *is_error, extra_body, out),
    }
}

fn encode_tool_result_item(
    call_id: &str,
    content: &[ToolResultContent],
    _is_error: bool,
    extra_body: &HashMap<String, Value>,
    out: &mut Vec<Value>,
) {
    let mut tool_content = Vec::new();
    for item in content {
        match item {
            ToolResultContent::Text { text } => {
                tool_content.push(json!({
                    "type": "input_text",
                    "text": text,
                }));
            }
            ToolResultContent::Image { source } => {
                tool_content.push(encode_input_image(source, &HashMap::new()));
            }
            ToolResultContent::File { source } => {
                tool_content.push(encode_input_file(source, &HashMap::new()));
            }
        }
    }

    let mut obj = Map::new();
    obj.insert(
        "type".to_string(),
        Value::String("function_call_output".to_string()),
    );
    obj.insert("call_id".to_string(), Value::String(call_id.to_string()));

    if tool_content.is_empty() {
        obj.insert("output".to_string(), Value::String(String::new()));
    } else if tool_content.len() == 1
        && tool_content[0].get("type").and_then(|v| v.as_str()) == Some("input_text")
    {
        obj.insert(
            "output".to_string(),
            tool_content[0]
                .get("text")
                .cloned()
                .unwrap_or(Value::String(String::new())),
        );
    } else {
        obj.insert("output".to_string(), Value::Array(tool_content));
    }

    merge_extra(&mut obj, extra_body);
    out.push(Value::Object(obj));
}

fn encode_input_image(
    source: &ImageSource,
    extra_body: &std::collections::HashMap<String, Value>,
) -> Value {
    match source {
        ImageSource::Url { url, detail } => {
            let mut obj = Map::new();
            obj.insert("type".to_string(), Value::String("input_image".to_string()));
            obj.insert("image_url".to_string(), Value::String(url.clone()));
            if let Some(detail) = detail {
                obj.insert("detail".to_string(), Value::String(detail.clone()));
            }
            merge_extra(&mut obj, extra_body);
            Value::Object(obj)
        }
        ImageSource::Base64 { media_type, data } => {
            let mut obj = Map::new();
            obj.insert("type".to_string(), Value::String("input_image".to_string()));
            obj.insert("image_base64".to_string(), Value::String(data.clone()));
            obj.insert("media_type".to_string(), Value::String(media_type.clone()));
            merge_extra(&mut obj, extra_body);
            Value::Object(obj)
        }
    }
}

fn encode_input_file(
    source: &FileSource,
    extra_body: &std::collections::HashMap<String, Value>,
) -> Value {
    if let Some(file_id) = extra_body.get("file_id").and_then(|v| v.as_str()) {
        let mut obj = Map::new();
        obj.insert("type".to_string(), Value::String("input_file".to_string()));
        obj.insert("file_id".to_string(), Value::String(file_id.to_string()));
        merge_extra(&mut obj, extra_body);
        return Value::Object(obj);
    }

    match source {
        FileSource::Url { url } => {
            let mut obj = Map::new();
            obj.insert("type".to_string(), Value::String("input_file".to_string()));
            if let Some(file_id) = url.strip_prefix("file_id://") {
                obj.insert("file_id".to_string(), Value::String(file_id.to_string()));
            } else {
                obj.insert("file_url".to_string(), Value::String(url.clone()));
            }
            merge_extra(&mut obj, extra_body);
            Value::Object(obj)
        }
        FileSource::Base64 {
            filename,
            media_type,
            data,
        } => {
            let mut obj = Map::new();
            obj.insert("type".to_string(), Value::String("input_file".to_string()));
            obj.insert("file_data".to_string(), Value::String(data.clone()));
            obj.insert("media_type".to_string(), Value::String(media_type.clone()));
            if let Some(name) = filename {
                obj.insert("filename".to_string(), Value::String(name.clone()));
            }
            merge_extra(&mut obj, extra_body);
            Value::Object(obj)
        }
    }
}

fn encode_output_image(
    source: &ImageSource,
    extra_body: &std::collections::HashMap<String, Value>,
) -> Value {
    match source {
        ImageSource::Url { url, detail } => {
            let mut obj = Map::new();
            obj.insert(
                "type".to_string(),
                Value::String("output_image".to_string()),
            );
            obj.insert("url".to_string(), Value::String(url.clone()));
            if let Some(detail) = detail {
                obj.insert("detail".to_string(), Value::String(detail.clone()));
            }
            merge_extra(&mut obj, extra_body);
            Value::Object(obj)
        }
        ImageSource::Base64 { media_type, data } => {
            let mut obj = Map::new();
            obj.insert(
                "type".to_string(),
                Value::String("output_image".to_string()),
            );
            obj.insert(
                "source".to_string(),
                json!({
                    "type": "base64",
                    "media_type": media_type,
                    "data": data
                }),
            );
            merge_extra(&mut obj, extra_body);
            Value::Object(obj)
        }
    }
}

fn encode_output_file(
    source: &FileSource,
    extra_body: &std::collections::HashMap<String, Value>,
) -> Value {
    match source {
        FileSource::Url { url } => {
            let mut obj = Map::new();
            obj.insert("type".to_string(), Value::String("output_file".to_string()));
            obj.insert("url".to_string(), Value::String(url.clone()));
            merge_extra(&mut obj, extra_body);
            Value::Object(obj)
        }
        FileSource::Base64 {
            filename,
            media_type,
            data,
        } => {
            let mut obj = Map::new();
            obj.insert("type".to_string(), Value::String("output_file".to_string()));
            obj.insert(
                "source".to_string(),
                json!({
                    "type": "base64",
                    "filename": filename,
                    "media_type": media_type,
                    "data": data
                }),
            );
            merge_extra(&mut obj, extra_body);
            Value::Object(obj)
        }
    }
}

fn encode_tools(tools: &[ToolDefinition]) -> Vec<Value> {
    let mut out = Vec::new();
    for tool in tools {
        if tool.tool_type == "function" {
            if let Some(function) = &tool.function {
                let mut item = json!({
                    "type": "function",
                    "name": function.name,
                    "parameters": function.parameters.clone().unwrap_or(json!({
                        "type": "object",
                        "properties": {},
                        "additionalProperties": true
                    }))
                });
                if let Some(description) = &function.description {
                    item["description"] = Value::String(description.clone());
                }
                if let Some(strict) = function.strict {
                    item["strict"] = Value::Bool(strict);
                }
                out.push(item);
            }
        } else {
            let mut item = Map::new();
            item.insert("type".to_string(), Value::String(tool.tool_type.clone()));
            merge_extra(&mut item, &tool.extra_body);
            out.push(Value::Object(item));
        }
    }
    out
}

fn apply_response_format(obj: &mut Map<String, Value>, format: &ResponseFormat) {
    match format {
        ResponseFormat::Text => {
            obj.insert("text".to_string(), json!({"format": { "type": "text" }}));
        }
        ResponseFormat::JsonObject => {
            obj.insert(
                "text".to_string(),
                json!({"format": { "type": "json_object" }}),
            );
        }
        ResponseFormat::JsonSchema { json_schema } => {
            obj.insert(
                "text".to_string(),
                json!({
                    "format": {
                        "type": "json_schema",
                        "name": json_schema.name,
                        "description": json_schema.description,
                        "strict": json_schema.strict,
                        "schema": json_schema.schema
                    }
                }),
            );
        }
    }
}

fn finish_reason_to_status(finish_reason: Option<FinishReason>) -> &'static str {
    match finish_reason {
        Some(FinishReason::Length) => "incomplete",
        Some(FinishReason::Other) => "failed",
        _ => "completed",
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::urp::decode::openai_responses as decode_responses;
    use crate::urp::{InputDetails, Item, OutputDetails, Role, Usage};

    fn empty_map() -> HashMap<String, Value> {
        HashMap::new()
    }

    #[test]
    fn encode_response_preserves_message_phase_and_order() {
        let resp = UrpResponse {
            id: "resp_1".to_string(),
            model: "gpt-5.4".to_string(),
            outputs: vec![Item::Message {
                role: Role::Assistant,
                parts: vec![
                    Part::Text {
                        content: "thinking".to_string(),
                        extra_body: {
                            let mut m = empty_map();
                            m.insert("phase".to_string(), json!("commentary"));
                            m
                        },
                    },
                    Part::ToolCall {
                        call_id: "call_1".to_string(),
                        name: "tool_a".to_string(),
                        arguments: "{}".to_string(),
                        extra_body: empty_map(),
                    },
                    Part::Text {
                        content: "done".to_string(),
                        extra_body: {
                            let mut m = empty_map();
                            m.insert("phase".to_string(), json!("final_answer"));
                            m
                        },
                    },
                ],
                extra_body: {
                    let mut m = empty_map();
                    m.insert("custom_message_field".to_string(), json!(true));
                    m
                },
            }],
            finish_reason: Some(FinishReason::ToolCalls),
            usage: None,
            extra_body: empty_map(),
        };

        let encoded = encode_response(&resp, "gpt-5.4");
        let output = encoded["output"].as_array().expect("output array");

        assert_eq!(output.len(), 3);
        assert_eq!(output[0]["type"], Value::String("message".to_string()));
        assert_eq!(output[0]["phase"], Value::String("commentary".to_string()));
        assert_eq!(output[0]["custom_message_field"], json!(true));
        assert_eq!(
            output[1]["type"],
            Value::String("function_call".to_string())
        );
        assert_eq!(output[2]["type"], Value::String("message".to_string()));
        assert_eq!(
            output[2]["phase"],
            Value::String("final_answer".to_string())
        );
    }

    #[test]
    fn responses_round_trip_keeps_phase_order_and_unknown_fields() {
        let source = json!({
            "id": "resp_1",
            "model": "gpt-5.4",
            "status": "completed",
            "output": [
                {
                    "type": "message",
                    "role": "assistant",
                    "phase": "commentary",
                    "custom_message_field": true,
                    "content": [{ "type": "output_text", "text": "one" }]
                },
                {
                    "type": "function_call",
                    "call_id": "call_1",
                    "name": "tool_a",
                    "arguments": "{}"
                },
                {
                    "type": "message",
                    "role": "assistant",
                    "phase": "final_answer",
                    "content": [{ "type": "output_text", "text": "two" }]
                }
            ]
        });

        let decoded = decode_responses::decode_response(&source).expect("decode response");
        let reencoded = encode_response(&decoded, "gpt-5.4");
        let output = reencoded["output"].as_array().expect("output array");

        assert_eq!(output.len(), 3);
        assert_eq!(output[0]["phase"], json!("commentary"));
        assert_eq!(output[0]["custom_message_field"], json!(true));
        assert_eq!(output[1]["type"], json!("function_call"));
        assert_eq!(output[2]["phase"], json!("final_answer"));
    }

    #[test]
    fn responses_round_trip_content_content_boundary() {
        let source = json!({
            "id": "resp_cc",
            "model": "gpt-5.4",
            "status": "completed",
            "output": [
                {
                    "type": "reasoning",
                    "text": "hmm"
                },
                {
                    "type": "message",
                    "role": "assistant",
                    "phase": "commentary",
                    "content": [{ "type": "output_text", "text": "phase A" }]
                },
                {
                    "type": "message",
                    "role": "assistant",
                    "phase": "final_answer",
                    "content": [{ "type": "output_text", "text": "phase B" }]
                },
                {
                    "type": "function_call",
                    "call_id": "call_2",
                    "name": "tool_b",
                    "arguments": "{}"
                }
            ]
        });

        let decoded = decode_responses::decode_response(&source).expect("decode");
        assert_eq!(
            decoded.outputs.len(),
            2,
            "greedy merger must produce 2 items"
        );

        let reencoded = encode_response(&decoded, "gpt-5.4");
        let output = reencoded["output"].as_array().expect("output array");

        assert_eq!(output.len(), 4);
        assert_eq!(output[0]["type"], json!("reasoning"));
        assert_eq!(output[1]["type"], json!("message"));
        assert_eq!(output[1]["phase"], json!("commentary"));
        assert_eq!(output[2]["type"], json!("message"));
        assert_eq!(output[2]["phase"], json!("final_answer"));
        assert_eq!(output[3]["type"], json!("function_call"));
    }

    #[test]
    fn encode_request_keeps_phased_developer_message_as_input_message() {
        let req = UrpRequest {
            model: "gpt-5.4".to_string(),
            inputs: vec![Item::Message {
                role: Role::Developer,
                parts: vec![Part::Text {
                    content: "preface".to_string(),
                    extra_body: {
                        let mut m = empty_map();
                        m.insert("phase".to_string(), json!("commentary"));
                        m
                    },
                }],
                extra_body: empty_map(),
            }],
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
        };

        let encoded = encode_request(&req, "gpt-5.4");
        assert!(encoded.get("instructions").is_none());
        assert_eq!(encoded["input"][0]["type"], json!("message"));
        assert_eq!(encoded["input"][0]["phase"], json!("commentary"));
    }

    #[test]
    fn encode_request_preserves_distinct_plain_reasoning_parts_when_other_parts_are_encrypted() {
        let req = UrpRequest {
            model: "gpt-5.4".to_string(),
            inputs: vec![Item::Message {
                role: Role::Assistant,
                parts: vec![
                    Part::Reasoning {
                        content: Some("signed think".to_string()),
                        encrypted: Some(json!("sig_1")),
                        summary: Some("signed summary".to_string()),
                        source: None,
                        extra_body: empty_map(),
                    },
                    Part::Reasoning {
                        content: Some("plain think".to_string()),
                        encrypted: None,
                        summary: None,
                        source: None,
                        extra_body: empty_map(),
                    },
                ],
                extra_body: empty_map(),
            }],
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
        };

        let encoded = encode_request(&req, "gpt-5.4");
        let input = encoded["input"].as_array().expect("input array");
        assert_eq!(input.len(), 2);
        assert_eq!(input[0]["type"], json!("reasoning"));
        assert_eq!(input[0]["encrypted_content"], json!("sig_1"));
        assert!(input[0].get("text").is_none());
        assert_eq!(input[1]["type"], json!("reasoning"));
        assert_eq!(input[1]["text"], json!("plain think"));
    }

    #[test]
    fn responses_usage_round_trips_all_typed_usage_fields_without_detail_leakage() {
        let mut usage_extra = HashMap::new();
        usage_extra.insert("upstream_counter".to_string(), json!(42));
        let response = UrpResponse {
            id: "resp_usage".to_string(),
            model: "gpt-5.4".to_string(),
            outputs: vec![Item::new_message(Role::Assistant)],
            finish_reason: Some(FinishReason::Stop),
            usage: Some(Usage {
                input_tokens: 30,
                output_tokens: 12,
                input_details: Some(InputDetails {
                    standard_tokens: 0,
                    cache_read_tokens: 2,
                    cache_creation_tokens: 3,
                    tool_prompt_tokens: 4,
                    modality_breakdown: None,
                }),
                output_details: Some(OutputDetails {
                    standard_tokens: 0,
                    reasoning_tokens: 5,
                    accepted_prediction_tokens: 6,
                    rejected_prediction_tokens: 7,
                    modality_breakdown: None,
                }),
                extra_body: usage_extra,
            }),
            extra_body: empty_map(),
        };

        let encoded = encode_response(&response, "gpt-5.4");
        assert_eq!(
            encoded["usage"]["input_tokens_details"]["cached_tokens"],
            json!(2)
        );
        assert_eq!(
            encoded["usage"]["input_tokens_details"]["cache_creation_tokens"],
            json!(3)
        );
        assert_eq!(
            encoded["usage"]["input_tokens_details"]["tool_prompt_tokens"],
            json!(4)
        );
        assert_eq!(
            encoded["usage"]["output_tokens_details"]["reasoning_tokens"],
            json!(5)
        );
        assert_eq!(
            encoded["usage"]["output_tokens_details"]["accepted_prediction_tokens"],
            json!(6)
        );
        assert_eq!(
            encoded["usage"]["output_tokens_details"]["rejected_prediction_tokens"],
            json!(7)
        );

        let decoded = decode_responses::decode_response(&encoded).expect("decode response");
        let decoded_usage = decoded.usage.expect("usage should decode");
        let input = decoded_usage.input_details.expect("input details");
        let output = decoded_usage.output_details.expect("output details");
        assert_eq!(input.cache_read_tokens, 2);
        assert_eq!(input.cache_creation_tokens, 3);
        assert_eq!(input.tool_prompt_tokens, 4);
        assert_eq!(output.reasoning_tokens, 5);
        assert_eq!(output.accepted_prediction_tokens, 6);
        assert_eq!(output.rejected_prediction_tokens, 7);
        assert!(decoded_usage
            .extra_body
            .get("input_tokens_details")
            .is_none());
        assert!(decoded_usage
            .extra_body
            .get("output_tokens_details")
            .is_none());
        assert_eq!(
            decoded_usage.extra_body.get("upstream_counter"),
            Some(&json!(42))
        );
    }

    #[test]
    fn responses_response_round_trip_preserves_reasoning_summary_separately_from_content() {
        let response = UrpResponse {
            id: "resp_roundtrip_reasoning".to_string(),
            model: "gpt-5.4".to_string(),
            outputs: vec![Item::Message {
                role: Role::Assistant,
                parts: vec![Part::Reasoning {
                    content: Some("full reasoning".to_string()),
                    encrypted: Some(json!("sig_1")),
                    summary: Some("brief summary".to_string()),
                    source: None,
                    extra_body: empty_map(),
                }],
                extra_body: empty_map(),
            }],
            finish_reason: Some(FinishReason::Stop),
            usage: None,
            extra_body: empty_map(),
        };

        let encoded = encode_response(&response, "gpt-5.4");
        let decoded = decode_responses::decode_response(&encoded).expect("decode response");
        let Item::Message { parts, .. } = &decoded.outputs[0] else {
            panic!("expected assistant output");
        };

        assert!(matches!(
            &parts[0],
            Part::Reasoning {
                content: Some(content),
                summary: Some(summary),
                encrypted: Some(Value::String(sig)),
                ..
            } if content == "full reasoning" && summary == "brief summary" && sig == "sig_1"
        ));
    }

    #[test]
    fn responses_response_round_trip_does_not_invent_summary_from_plain_reasoning_content() {
        let response = UrpResponse {
            id: "resp_roundtrip_plain_reasoning".to_string(),
            model: "gpt-5.4".to_string(),
            outputs: vec![Item::Message {
                role: Role::Assistant,
                parts: vec![Part::Reasoning {
                    content: Some("plain reasoning".to_string()),
                    encrypted: None,
                    summary: None,
                    source: None,
                    extra_body: empty_map(),
                }],
                extra_body: empty_map(),
            }],
            finish_reason: Some(FinishReason::Stop),
            usage: None,
            extra_body: empty_map(),
        };

        let encoded = encode_response(&response, "gpt-5.4");
        let reasoning_item = encoded["output"]
            .as_array()
            .and_then(|items| items.first())
            .expect("reasoning output item");
        assert!(reasoning_item.get("summary").is_none());

        let decoded = decode_responses::decode_response(&encoded).expect("decode response");
        let Item::Message { parts, .. } = &decoded.outputs[0] else {
            panic!("expected assistant output");
        };
        assert!(matches!(
            &parts[0],
            Part::Reasoning {
                content: Some(content),
                summary: None,
                encrypted: None,
                ..
            } if content == "plain reasoning"
        ));
    }

    #[test]
    fn encode_request_keeps_json_object_response_format() {
        let req = UrpRequest {
            model: "gpt-5.4".to_string(),
            inputs: vec![Item::Message {
                role: Role::User,
                parts: vec![Part::Text {
                    content: "hello".to_string(),
                    extra_body: empty_map(),
                }],
                extra_body: empty_map(),
            }],
            stream: None,
            temperature: None,
            top_p: None,
            max_output_tokens: None,
            reasoning: None,
            tools: None,
            tool_choice: None,
            response_format: Some(ResponseFormat::JsonObject),
            user: None,
            extra_body: empty_map(),
        };

        let encoded = encode_request(&req, "gpt-5.4");
        assert_eq!(encoded["text"]["format"]["type"], json!("json_object"));
        assert!(encoded["text"]["format"].get("schema").is_none());
        assert!(encoded["text"]["format"].get("name").is_none());
    }
}
