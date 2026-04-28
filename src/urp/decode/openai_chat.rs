use crate::urp::decode::{
    deserialize_u64ish_default, parse_file_part_from_obj, parse_image_part_from_obj,
    parse_tool_call_part_from_obj, parse_tool_definition, split_extra, value_to_text,
};
use crate::urp::internal_legacy_bridge::{Part, Role};
use crate::urp::{
    FinishReason, InputDetails, Node, OrdinaryRole, OutputDetails, ReasoningConfig, ToolChoice,
    ToolResultContent, UrpRequest, UrpResponse, Usage, parse_reasoning_envelope,
};
use serde::Deserialize;
use serde_json::{Map, Value};
use std::collections::HashMap;

#[derive(Debug, Clone, Deserialize)]
struct OpenAiChatUsage {
    #[serde(default, deserialize_with = "deserialize_u64ish_default")]
    prompt_tokens: u64,
    #[serde(default, deserialize_with = "deserialize_u64ish_default")]
    completion_tokens: u64,
    #[serde(default)]
    #[serde(alias = "input_tokens_details")]
    prompt_tokens_details: Option<OpenAiChatInputDetails>,
    #[serde(default)]
    #[serde(alias = "output_tokens_details")]
    completion_tokens_details: Option<OpenAiChatOutputDetails>,
    #[serde(flatten)]
    extra: HashMap<String, Value>,
}

#[derive(Debug, Clone, Deserialize)]
struct OpenAiChatInputDetails {
    #[serde(
        default,
        deserialize_with = "deserialize_u64ish_default",
        alias = "cache_read_tokens"
    )]
    cached_tokens: u64,
    #[serde(default, deserialize_with = "deserialize_u64ish_default")]
    cache_write_tokens: u64,
    #[serde(default, deserialize_with = "deserialize_u64ish_default")]
    cache_creation_tokens: u64,
    #[serde(
        default,
        deserialize_with = "deserialize_u64ish_default",
        alias = "tool_prompt_input_tokens"
    )]
    tool_prompt_tokens: u64,
    #[serde(flatten)]
    extra: HashMap<String, Value>,
}

#[derive(Debug, Clone, Deserialize)]
struct OpenAiChatOutputDetails {
    #[serde(default, deserialize_with = "deserialize_u64ish_default")]
    reasoning_tokens: u64,
    #[serde(
        default,
        deserialize_with = "deserialize_u64ish_default",
        alias = "accepted_prediction_output_tokens"
    )]
    accepted_prediction_tokens: u64,
    #[serde(
        default,
        deserialize_with = "deserialize_u64ish_default",
        alias = "rejected_prediction_output_tokens"
    )]
    rejected_prediction_tokens: u64,
    #[serde(flatten)]
    extra: HashMap<String, Value>,
}

impl From<OpenAiChatUsage> for Usage {
    fn from(value: OpenAiChatUsage) -> Self {
        let OpenAiChatUsage {
            prompt_tokens,
            completion_tokens,
            prompt_tokens_details,
            completion_tokens_details,
            mut extra,
        } = value;

        let input_details = prompt_tokens_details.clone().and_then(|details| {
            let cache_creation_tokens = details
                .cache_creation_tokens
                .max(details.cache_write_tokens);
            if details.cached_tokens > 0
                || cache_creation_tokens > 0
                || details.tool_prompt_tokens > 0
            {
                Some(InputDetails {
                    standard_tokens: 0,
                    cache_read_tokens: details.cached_tokens,
                    cache_creation_tokens,
                    tool_prompt_tokens: details.tool_prompt_tokens,
                    modality_breakdown: None,
                })
            } else {
                None
            }
        });

        let output_details = completion_tokens_details.clone().and_then(|details| {
            if details.reasoning_tokens > 0
                || details.accepted_prediction_tokens > 0
                || details.rejected_prediction_tokens > 0
            {
                Some(OutputDetails {
                    standard_tokens: 0,
                    reasoning_tokens: details.reasoning_tokens,
                    accepted_prediction_tokens: details.accepted_prediction_tokens,
                    rejected_prediction_tokens: details.rejected_prediction_tokens,
                    modality_breakdown: None,
                })
            } else {
                None
            }
        });

        if let Some(details) = prompt_tokens_details {
            extra.extend(details.extra);
        }
        if let Some(details) = completion_tokens_details {
            extra.extend(details.extra);
        }

        Usage {
            input_tokens: prompt_tokens,
            output_tokens: completion_tokens,
            input_details,
            output_details,
            extra_body: extra,
        }
    }
}

fn text_part_with_phase(
    content: impl Into<String>,
    phase: Option<&str>,
    mut extra_body: HashMap<String, Value>,
) -> Part {
    if let Some(phase) = phase {
        extra_body.insert("phase".to_string(), Value::String(phase.to_string()));
    }
    Part::Text {
        content: content.into(),
        extra_body,
    }
}

fn push_message_nodes(
    out: &mut Vec<Node>,
    role: Role,
    parts: Vec<Part>,
    extra_body: HashMap<String, Value>,
) {
    let ordinary_role = role.to_ordinary().unwrap_or(OrdinaryRole::User);
    for (index, part) in parts.into_iter().enumerate() {
        let mut node = part.into_node(ordinary_role);
        if index == 0 && !extra_body.is_empty() {
            node.extra_body_mut().extend(extra_body.clone());
        }
        out.push(node);
    }
}

fn push_chat_content_parts(parts: &mut Vec<Part>, content: &Value, message_phase: Option<&str>) {
    if let Some(s) = content.as_str() {
        if !s.is_empty() {
            parts.push(text_part_with_phase(s, message_phase, HashMap::new()));
        }
        return;
    }

    let Some(arr) = content.as_array() else {
        return;
    };

    for item in arr {
        if let Some(s) = item.as_str() {
            if !s.is_empty() {
                parts.push(text_part_with_phase(s, message_phase, HashMap::new()));
            }
            continue;
        }
        let Some(item_obj) = item.as_object() else {
            continue;
        };
        if let Some(text) = item_obj.get("text").and_then(|v| v.as_str()) {
            let item_type = item_obj.get("type").and_then(|v| v.as_str());
            if !text.is_empty()
                && !matches!(item_type, Some("tool_call" | "function_call" | "tool_use"))
            {
                parts.push(text_part_with_phase(
                    text,
                    message_phase,
                    split_extra(item_obj, &["type", "text"]),
                ));
            }
        }
        if let Some(image_part) = parse_image_part_from_obj(item_obj) {
            parts.push(image_part);
            continue;
        }
        if let Some(file_part) = parse_file_part_from_obj(item_obj) {
            parts.push(file_part);
            continue;
        }
        if let Some(tool_call_part) = parse_tool_call_part_from_obj(item_obj) {
            parts.push(tool_call_part);
        }
    }
}

pub fn decode_request(value: &Value) -> Result<UrpRequest, String> {
    let obj = value
        .as_object()
        .ok_or_else(|| "chat request must be object".to_string())?;

    let model = obj
        .get("model")
        .and_then(|v| v.as_str())
        .ok_or_else(|| "missing model".to_string())?
        .to_string();

    let mut input_nodes = Vec::new();
    for raw_msg in obj
        .get("messages")
        .and_then(|v| v.as_array())
        .ok_or_else(|| "missing messages".to_string())?
    {
        let msg_obj = match raw_msg.as_object() {
            Some(v) => v,
            None => continue,
        };
        let role = match msg_obj
            .get("role")
            .and_then(|v| v.as_str())
            .unwrap_or("user")
        {
            "system" => Role::System,
            "developer" => Role::Developer,
            "assistant" => Role::Assistant,
            "tool" => Role::Tool,
            _ => Role::User,
        };

        if role == Role::Tool {
            let call_id = msg_obj
                .get("tool_call_id")
                .and_then(|v| v.as_str())
                .unwrap_or("")
                .to_string();
            let content = msg_obj.get("content").cloned().unwrap_or(Value::Null);
            let text = value_to_text(&content);
            let mut tool_result_content = Vec::new();
            if !text.is_empty() {
                tool_result_content.push(ToolResultContent::Text { text });
            }
            input_nodes.push(Node::ToolResult {
                id: msg_obj
                    .get("id")
                    .and_then(|v| v.as_str())
                    .map(|s| s.to_string()),
                call_id,
                is_error: false,
                content: tool_result_content,
                extra_body: split_extra(msg_obj, &["role", "tool_call_id", "content"]),
            });
            continue;
        }

        let message_phase = msg_obj.get("phase").and_then(|v| v.as_str());
        let mut parts = Vec::new();
        let extra_body = split_extra(
            msg_obj,
            &[
                "role",
                "content",
                "tool_calls",
                "reasoning",
                "reasoning_details",
                "reasoning_content",
                "reasoning_opaque",
                "refusal",
                "phase",
            ],
        );

        parse_chat_reasoning_fields(msg_obj, &mut parts);

        if let Some(content) = msg_obj.get("content") {
            push_chat_content_parts(&mut parts, content, message_phase);
        }

        if let Some(refusal) = msg_obj.get("refusal").and_then(|v| v.as_str()) {
            if !refusal.is_empty() {
                parts.push(Part::Refusal {
                    content: refusal.to_string(),
                    extra_body: HashMap::new(),
                });
            }
        }

        if let Some(tool_calls) = msg_obj.get("tool_calls").and_then(|v| v.as_array()) {
            for tool_call in tool_calls {
                let tc_obj = match tool_call.as_object() {
                    Some(v) => v,
                    None => continue,
                };
                let call_id = tc_obj
                    .get("id")
                    .or_else(|| tc_obj.get("call_id"))
                    .and_then(|v| v.as_str())
                    .unwrap_or("")
                    .to_string();
                let name = tc_obj
                    .get("function")
                    .and_then(|v| v.get("name"))
                    .and_then(|v| v.as_str())
                    .unwrap_or("")
                    .to_string();
                let arguments = tc_obj
                    .get("function")
                    .and_then(|v| v.get("arguments"))
                    .cloned()
                    .unwrap_or(Value::String("{}".to_string()));
                let arguments = if let Some(s) = arguments.as_str() {
                    s.to_string()
                } else {
                    serde_json::to_string(&arguments).unwrap_or_else(|_| "{}".to_string())
                };
                if !call_id.is_empty() && !name.is_empty() {
                    parts.push(Part::ToolCall {
                        id: tc_obj
                            .get("id")
                            .and_then(|v| v.as_str())
                            .map(|s| s.to_string()),
                        call_id,
                        name,
                        arguments,
                        extra_body: split_extra(tc_obj, &["id", "type", "function", "call_id"]),
                    });
                }
            }
        }

        push_message_nodes(&mut input_nodes, role, parts, extra_body);
    }

    let reasoning = extract_reasoning(obj);
    let tools = obj.get("tools").and_then(|v| v.as_array()).map(|arr| {
        arr.iter()
            .filter_map(parse_tool_definition)
            .collect::<Vec<_>>()
    });

    Ok(UrpRequest {
        model,
        input: input_nodes,
        stream: obj.get("stream").and_then(|v| v.as_bool()),
        temperature: obj.get("temperature").and_then(|v| v.as_f64()),
        top_p: obj.get("top_p").and_then(|v| v.as_f64()),
        max_output_tokens: obj
            .get("max_completion_tokens")
            .or_else(|| obj.get("max_tokens"))
            .and_then(|v| v.as_u64()),
        reasoning,
        tools,
        tool_choice: obj.get("tool_choice").cloned().map(tool_choice_from_value),
        response_format: obj
            .get("response_format")
            .cloned()
            .and_then(parse_response_format),
        user: obj
            .get("user")
            .and_then(|v| v.as_str())
            .map(|s| s.to_string()),
        extra_body: split_extra(
            obj,
            &[
                "model",
                "messages",
                "stream",
                "temperature",
                "top_p",
                "max_completion_tokens",
                "max_tokens",
                "reasoning_effort",
                "reasoning",
                "tools",
                "tool_choice",
                "response_format",
                "user",
            ],
        ),
    })
}

pub fn decode_response(value: &Value) -> Result<UrpResponse, String> {
    let obj = value
        .as_object()
        .ok_or_else(|| "chat response must be object".to_string())?;

    let choice = obj
        .get("choices")
        .and_then(|v| v.as_array())
        .and_then(|arr| arr.first())
        .and_then(|v| v.as_object())
        .ok_or_else(|| "missing choices[0]".to_string())?;
    let msg_obj = choice
        .get("message")
        .and_then(|v| v.as_object())
        .ok_or_else(|| "missing choices[0].message".to_string())?;

    let mut parts = Vec::new();
    let message_extra_body = split_extra(
        msg_obj,
        &[
            "role",
            "content",
            "reasoning",
            "reasoning_details",
            "reasoning_content",
            "reasoning_opaque",
            "tool_calls",
            "refusal",
            "phase",
        ],
    );
    let message_phase = msg_obj.get("phase").and_then(|v| v.as_str());

    parse_chat_reasoning_fields(msg_obj, &mut parts);

    if let Some(content) = msg_obj.get("content") {
        push_chat_content_parts(&mut parts, content, message_phase);
    }

    if let Some(tool_calls) = msg_obj.get("tool_calls").and_then(|v| v.as_array()) {
        for tool_call in tool_calls {
            let tc_obj = match tool_call.as_object() {
                Some(v) => v,
                None => continue,
            };
            let call_id = tc_obj
                .get("id")
                .or_else(|| tc_obj.get("call_id"))
                .and_then(|v| v.as_str())
                .unwrap_or("")
                .to_string();
            let name = tc_obj
                .get("function")
                .and_then(|v| v.get("name"))
                .and_then(|v| v.as_str())
                .unwrap_or("")
                .to_string();
            let arguments = tc_obj
                .get("function")
                .and_then(|v| v.get("arguments"))
                .cloned()
                .unwrap_or(Value::String("{}".to_string()));
            let arguments = if let Some(s) = arguments.as_str() {
                s.to_string()
            } else {
                serde_json::to_string(&arguments).unwrap_or_else(|_| "{}".to_string())
            };
            if !call_id.is_empty() && !name.is_empty() {
                parts.push(Part::ToolCall {
                    id: tc_obj
                        .get("id")
                        .and_then(|v| v.as_str())
                        .map(|s| s.to_string()),
                    call_id,
                    name,
                    arguments,
                    extra_body: split_extra(tc_obj, &["id", "type", "function", "call_id"]),
                });
            }
        }
    }

    if let Some(refusal) = msg_obj.get("refusal").and_then(|v| v.as_str()) {
        if !refusal.is_empty() {
            parts.push(Part::Refusal {
                content: refusal.to_string(),
                extra_body: HashMap::new(),
            });
        }
    }

    let mut output_nodes = Vec::new();
    push_message_nodes(
        &mut output_nodes,
        Role::Assistant,
        parts,
        message_extra_body.clone(),
    );

    let finish_reason = choice
        .get("finish_reason")
        .and_then(|v| v.as_str())
        .map(parse_finish_reason);

    let usage = obj
        .get("usage")
        .and_then(|v| v.as_object())
        .map(parse_usage_from_chat);

    Ok(UrpResponse {
        id: obj
            .get("id")
            .and_then(|v| v.as_str())
            .unwrap_or("chat_completion")
            .to_string(),
        model: obj
            .get("model")
            .and_then(|v| v.as_str())
            .unwrap_or_default()
            .to_string(),
        created_at: obj.get("created").and_then(|v| v.as_i64()),
        output: output_nodes,
        finish_reason,
        usage,
        extra_body: split_extra(
            obj,
            &["id", "object", "created", "model", "choices", "usage"],
        ),
    })
}

fn extract_reasoning(obj: &Map<String, Value>) -> Option<ReasoningConfig> {
    if let Some(effort) = obj
        .get("reasoning_effort")
        .and_then(|v| v.as_str())
        .map(|s| s.to_string())
    {
        return Some(ReasoningConfig {
            effort: Some(effort),
            extra_body: HashMap::new(),
        });
    }
    if let Some(effort) = obj
        .get("reasoning")
        .and_then(|v| v.as_object())
        .and_then(|v| v.get("effort"))
        .and_then(|v| v.as_str())
        .map(|s| s.to_string())
    {
        return Some(ReasoningConfig {
            effort: Some(effort),
            extra_body: HashMap::new(),
        });
    }
    None
}

fn parse_chat_reasoning_fields(msg_obj: &Map<String, Value>, parts: &mut Vec<Part>) {
    fn merge_reasoning_part(
        parts: &mut Vec<Part>,
        content: Option<String>,
        encrypted: Option<Value>,
        summary: Option<String>,
        source: Option<String>,
    ) {
        if let Some(Part::Reasoning {
            content: existing_content,
            encrypted: existing_encrypted,
            summary: existing_summary,
            source: existing_source,
            ..
        }) = parts.last_mut()
        {
            if existing_content.is_none() && content.is_some() {
                *existing_content = content;
            }
            if existing_encrypted.is_none() && encrypted.is_some() {
                *existing_encrypted = encrypted;
            }
            if existing_summary.is_none() && summary.is_some() {
                *existing_summary = summary;
            }
            if existing_source.is_none() && source.is_some() {
                *existing_source = source;
            }
            return;
        }

        parts.push(Part::Reasoning {
            id: None,
            content,
            encrypted,
            summary,
            source,
            extra_body: HashMap::new(),
        });
    }

    let mut saw_plain = false;
    let mut saw_encrypted = false;

    if let Some(details) = msg_obj.get("reasoning_details").and_then(|v| v.as_array()) {
        for detail in details {
            let Some(detail_obj) = detail.as_object() else {
                continue;
            };
            match detail_obj
                .get("type")
                .and_then(|v| v.as_str())
                .unwrap_or("")
            {
                "reasoning.text" => {
                    let source = detail_obj
                        .get("format")
                        .and_then(|v| v.as_str())
                        .filter(|format| !format.is_empty())
                        .map(|format| format.to_string());
                    if let Some(text) = detail_obj.get("text").and_then(|v| v.as_str()) {
                        if !text.is_empty() {
                            merge_reasoning_part(
                                parts,
                                Some(text.to_string()),
                                None,
                                None,
                                source.clone(),
                            );
                            saw_plain = true;
                        }
                    }
                }
                "reasoning.encrypted" => {
                    let source = detail_obj
                        .get("format")
                        .and_then(|v| v.as_str())
                        .filter(|format| !format.is_empty())
                        .map(|format| format.to_string());
                    let detail_id = detail_obj
                        .get("id")
                        .and_then(|v| v.as_str())
                        .map(|s| s.to_string())
                        .or_else(|| {
                            detail_obj
                                .get("data")
                                .and_then(parse_reasoning_envelope)
                                .and_then(|envelope| envelope.item_id)
                        });
                    if let Some(data) = detail_obj.get("data") {
                        if !matches!(data, Value::Null) {
                            if let Some(s) = data.as_str() {
                                if s.is_empty() {
                                    continue;
                                }
                            }
                            merge_reasoning_part(parts, None, Some(data.clone()), None, source);
                            if let Some(id) = detail_id {
                                if let Some(Part::Reasoning { extra_body, .. }) = parts.last_mut() {
                                    extra_body.insert("id".to_string(), Value::String(id));
                                }
                            }
                            saw_encrypted = true;
                        }
                    }
                }
                "reasoning.summary" => {
                    let source = detail_obj
                        .get("format")
                        .and_then(|v| v.as_str())
                        .filter(|format| !format.is_empty())
                        .map(|format| format.to_string());
                    if let Some(summary) = detail_obj.get("summary").and_then(|v| v.as_str()) {
                        if !summary.is_empty() {
                            merge_reasoning_part(
                                parts,
                                None,
                                None,
                                Some(summary.to_string()),
                                source,
                            );
                            saw_plain = true;
                        }
                    }
                }
                _ => {}
            }
        }
    }

    if !saw_plain {
        if let Some(reasoning) = msg_obj.get("reasoning").and_then(|v| v.as_str()) {
            if !reasoning.is_empty() {
                merge_reasoning_part(parts, Some(reasoning.to_string()), None, None, None);
                saw_plain = true;
            }
        }
    }

    if !saw_plain {
        if let Some(reasoning) = msg_obj.get("reasoning_content").and_then(|v| v.as_str()) {
            if !reasoning.is_empty() {
                merge_reasoning_part(parts, Some(reasoning.to_string()), None, None, None);
            }
        }
    }

    if !saw_encrypted {
        if let Some(opaque) = msg_obj.get("reasoning_opaque").and_then(|v| v.as_str()) {
            if !opaque.is_empty() {
                merge_reasoning_part(
                    parts,
                    None,
                    Some(Value::String(opaque.to_string())),
                    None,
                    None,
                );
            }
        }
    }
}

fn tool_choice_from_value(v: Value) -> ToolChoice {
    if let Some(s) = v.as_str() {
        ToolChoice::Mode(s.to_string())
    } else {
        ToolChoice::Specific(v)
    }
}

fn parse_response_format(v: Value) -> Option<crate::urp::ResponseFormat> {
    if let Some(obj) = v.as_object() {
        match obj.get("type").and_then(|x| x.as_str()) {
            Some("json_object") => return Some(crate::urp::ResponseFormat::JsonObject),
            Some("json_schema") => {
                let schema_obj = obj.get("json_schema")?.as_object()?;
                let name = schema_obj.get("name")?.as_str()?.to_string();
                let schema = schema_obj.get("schema").cloned().unwrap_or(Value::Null);
                let description = schema_obj
                    .get("description")
                    .and_then(|x| x.as_str())
                    .map(|s| s.to_string());
                let strict = schema_obj.get("strict").and_then(|x| x.as_bool());
                let mut extra = HashMap::new();
                for (k, v) in schema_obj {
                    if !["name", "schema", "description", "strict"].contains(&k.as_str()) {
                        extra.insert(k.clone(), v.clone());
                    }
                }
                return Some(crate::urp::ResponseFormat::JsonSchema {
                    json_schema: crate::urp::JsonSchemaDefinition {
                        name,
                        description,
                        schema,
                        strict,
                        extra_body: extra,
                    },
                });
            }
            _ => {}
        }
    }
    if let Some(s) = v.as_str() {
        if s == "json_object" {
            return Some(crate::urp::ResponseFormat::JsonObject);
        }
        if s == "text" {
            return Some(crate::urp::ResponseFormat::Text);
        }
    }
    None
}

fn parse_finish_reason(s: &str) -> FinishReason {
    match s {
        "stop" => FinishReason::Stop,
        "length" => FinishReason::Length,
        "tool_calls" | "function_call" => FinishReason::ToolCalls,
        "content_filter" => FinishReason::ContentFilter,
        _ => FinishReason::Other,
    }
}

fn parse_usage_from_chat(obj: &Map<String, Value>) -> Usage {
    serde_json::from_value::<OpenAiChatUsage>(Value::Object(obj.clone()))
        .map(Usage::from)
        .unwrap_or_else(|_| Usage {
            input_tokens: 0,
            output_tokens: 0,
            input_details: None,
            output_details: None,
            extra_body: obj.clone().into_iter().collect(),
        })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::urp::Node;
    use crate::urp::internal_legacy_bridge::{Item, nodes_to_items};
    use serde_json::json;

    fn output_parts(nodes: &[Node]) -> Vec<Part> {
        nodes_to_items(nodes)
            .into_iter()
            .find_map(|item| match item {
                Item::Message { parts, .. } => Some(parts),
                Item::ToolResult { .. } => None,
            })
            .unwrap_or_default()
    }

    #[test]
    fn decode_response_reads_openrouter_reasoning_details() {
        let value = json!({
            "id": "chatcmpl_test",
            "model": "m",
            "choices": [{
                "index": 0,
                "finish_reason": "stop",
                "message": {
                    "role": "assistant",
                    "content": "ok",
                    "reasoning": "new_reasoning",
                    "reasoning_details": [
                        {
                            "type": "reasoning.text",
                            "text": "new_reasoning",
                            "format": "openrouter"
                        },
                        {
                            "type": "reasoning.encrypted",
                            "data": "new_sig",
                            "format": "openrouter"
                        }
                    ],
                    "reasoning_content": "legacy_reasoning",
                    "reasoning_opaque": "legacy_sig"
                }
            }]
        });

        let decoded = decode_response(&value).expect("decode_response should succeed");
        let mut saw_reasoning = false;
        let mut saw_sig = false;
        let parts = output_parts(&decoded.output);
        for part in &parts {
            if let Part::Reasoning {
                content, encrypted, ..
            } = part
            {
                assert_eq!(content.as_deref(), Some("new_reasoning"));
                assert_eq!(encrypted.as_ref().and_then(|v| v.as_str()), Some("new_sig"));
                saw_reasoning = true;
                saw_sig = true;
            }
        }
        assert!(saw_reasoning);
        assert!(saw_sig);
    }

    #[test]
    fn decode_response_accepts_legacy_reasoning_fields() {
        let value = json!({
            "id": "chatcmpl_test",
            "model": "m",
            "choices": [{
                "index": 0,
                "finish_reason": "stop",
                "message": {
                    "role": "assistant",
                    "content": "ok",
                    "reasoning_content": "legacy_reasoning",
                    "reasoning_opaque": "legacy_sig"
                }
            }]
        });

        let decoded = decode_response(&value).expect("decode_response should succeed");
        let mut saw_reasoning = false;
        let mut saw_sig = false;
        let parts = output_parts(&decoded.output);
        for part in &parts {
            if let Part::Reasoning {
                content, encrypted, ..
            } = part
            {
                assert_eq!(content.as_deref(), Some("legacy_reasoning"));
                assert_eq!(
                    encrypted.as_ref().and_then(|v| v.as_str()),
                    Some("legacy_sig")
                );
                saw_reasoning = true;
                saw_sig = true;
            }
        }
        assert!(saw_reasoning);
        assert!(saw_sig);
    }

    #[test]
    fn decode_response_accepts_real_upstream_gpt5_reasoning_payload_shape() {
        let value = json!({
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

        let decoded = decode_response(&value).expect("decode_response should succeed");
        let parts = output_parts(&decoded.output);
        assert!(parts.iter().any(|part| {
            matches!(
                part,
                Part::Reasoning {
                    content: Some(content),
                    encrypted: Some(Value::String(sig)),
                    ..
                } if content == "plain reasoning" && sig == "opaque_sig_payload"
            )
        }));
    }

    #[test]
    fn decode_response_accepts_content_array_tool_call_blocks() {
        let value = json!({
            "id": "chatcmpl_test",
            "model": "m",
            "choices": [{
                "index": 0,
                "finish_reason": "tool_calls",
                "message": {
                    "role": "assistant",
                    "content": [
                        { "type": "text", "text": "before tool" },
                        { "type": "tool_call", "id": "call_1", "name": "lookup", "arguments": { "q": 1 } }
                    ]
                }
            }]
        });

        let decoded = decode_response(&value).expect("decode_response should succeed");
        let parts = output_parts(&decoded.output);

        assert!(parts.iter().any(|part| {
            matches!(part, Part::Text { content, .. } if content == "before tool")
        }));
        assert!(parts.iter().any(|part| {
            matches!(
                part,
                Part::ToolCall {
                    call_id,
                    name,
                    arguments,
                    ..
                } if call_id == "call_1" && name == "lookup" && arguments == "{\"q\":1}"
            )
        }));
    }

    #[test]
    fn decode_response_accepts_content_array_tool_use_blocks() {
        let value = json!({
            "id": "chatcmpl_test",
            "model": "m",
            "choices": [{
                "index": 0,
                "finish_reason": "tool_calls",
                "message": {
                    "role": "assistant",
                    "content": [
                        { "type": "text", "text": "before tool" },
                        { "type": "tool_use", "id": "call_1", "name": "lookup", "input": { "q": 1 } }
                    ]
                }
            }]
        });

        let decoded = decode_response(&value).expect("decode_response should succeed");
        let parts = output_parts(&decoded.output);

        assert!(parts.iter().any(|part| {
            matches!(part, Part::Text { content, .. } if content == "before tool")
        }));
        assert!(parts.iter().any(|part| {
            matches!(
                part,
                Part::ToolCall {
                    call_id,
                    name,
                    arguments,
                    ..
                } if call_id == "call_1" && name == "lookup" && arguments == "{\"q\":1}"
            )
        }));
    }
}
