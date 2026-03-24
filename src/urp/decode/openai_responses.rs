use crate::urp::decode::{
    deserialize_u64ish_default, parse_file_part_from_obj, parse_image_part_from_obj,
    parse_tool_definition, split_extra, value_to_text,
};
use crate::urp::{
    FinishReason, InputDetails, Item, OutputDetails, Part, ReasoningConfig, Role, ToolChoice,
    ToolResultContent, UrpRequest, UrpResponse, Usage,
    greedy::{Action, GreedyMerger},
};
use serde::Deserialize;
use serde_json::{Map, Value};
use std::collections::HashMap;

#[derive(Debug, Clone, Deserialize)]
struct OpenAiResponsesUsage {
    #[serde(
        default,
        deserialize_with = "deserialize_u64ish_default",
        alias = "prompt_tokens"
    )]
    input_tokens: u64,
    #[serde(
        default,
        deserialize_with = "deserialize_u64ish_default",
        alias = "completion_tokens"
    )]
    output_tokens: u64,
    #[serde(default)]
    #[serde(alias = "prompt_tokens_details")]
    input_tokens_details: Option<OpenAiResponsesInputDetails>,
    #[serde(default)]
    #[serde(alias = "completion_tokens_details")]
    output_tokens_details: Option<OpenAiResponsesOutputDetails>,
    #[serde(flatten)]
    extra: HashMap<String, Value>,
}

#[derive(Debug, Clone, Deserialize)]
struct OpenAiResponsesInputDetails {
    #[serde(
        default,
        deserialize_with = "deserialize_u64ish_default",
        alias = "cache_read_tokens"
    )]
    cached_tokens: u64,
    #[serde(default, deserialize_with = "deserialize_u64ish_default")]
    cache_creation_tokens: u64,
    #[serde(default, deserialize_with = "deserialize_u64ish_default")]
    cache_write_tokens: u64,
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
struct OpenAiResponsesOutputDetails {
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

impl From<OpenAiResponsesUsage> for Usage {
    fn from(value: OpenAiResponsesUsage) -> Self {
        let OpenAiResponsesUsage {
            input_tokens,
            output_tokens,
            input_tokens_details,
            output_tokens_details,
            mut extra,
        } = value;

        let input_details = input_tokens_details.clone().and_then(|details| {
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

        let output_details = output_tokens_details.clone().and_then(|details| {
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

        if let Some(details) = input_tokens_details {
            extra.extend(details.extra);
        }
        if let Some(details) = output_tokens_details {
            extra.extend(details.extra);
        }

        Usage {
            input_tokens,
            output_tokens,
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

fn build_message_item(role: Role, parts: Vec<Part>, extra_body: HashMap<String, Value>) -> Item {
    let mut obj = Map::new();
    obj.insert("type".to_string(), Value::String("message".to_string()));
    obj.insert(
        "role".to_string(),
        serde_json::to_value(role).expect("role should serialize"),
    );
    obj.insert(
        "parts".to_string(),
        serde_json::to_value(parts).expect("parts should serialize"),
    );
    obj.extend(extra_body);
    serde_json::from_value(Value::Object(obj)).expect("message item should deserialize")
}

fn push_part(item: &mut Item, part: Part) {
    let mut obj = serde_json::to_value(item.clone())
        .expect("item should serialize")
        .as_object()
        .cloned()
        .expect("item should serialize to object");

    let parts = obj
        .entry("parts".to_string())
        .or_insert_with(|| Value::Array(Vec::new()));
    let Value::Array(parts) = parts else {
        panic!("message item should contain parts array");
    };
    parts.push(serde_json::to_value(part).expect("part should serialize"));

    *item = serde_json::from_value(Value::Object(obj)).expect("item should deserialize");
}

fn item_parts(item: &Item) -> Option<Vec<Part>> {
    let obj = serde_json::to_value(item).ok()?.as_object()?.clone();
    if obj.get("type")?.as_str()? != "message" {
        return None;
    }
    serde_json::from_value(obj.get("parts")?.clone()).ok()
}

pub fn decode_request(value: &Value) -> Result<UrpRequest, String> {
    let obj = value
        .as_object()
        .ok_or_else(|| "responses request must be object".to_string())?;

    let model = obj
        .get("model")
        .and_then(|v| v.as_str())
        .ok_or_else(|| "missing model".to_string())?
        .to_string();

    let mut inputs = Vec::new();

    if let Some(instructions) = obj.get("instructions").and_then(|v| v.as_str()) {
        if !instructions.is_empty() {
            inputs.push(Item::text(Role::Developer, instructions));
        }
    }

    if let Some(input) = obj.get("input") {
        decode_input_items(input, &mut inputs);
    }

    let reasoning = obj
        .get("reasoning")
        .and_then(|v| v.as_object())
        .and_then(|reasoning_obj| {
            let effort = reasoning_obj
                .get("effort")
                .and_then(|v| v.as_str())
                .map(|s| s.to_string());
            effort.as_ref()?;
            Some(ReasoningConfig {
                effort,
                extra_body: split_extra(reasoning_obj, &["effort"]),
            })
        });

    let tools = obj.get("tools").and_then(|v| v.as_array()).map(|arr| {
        arr.iter()
            .filter_map(parse_tool_definition)
            .collect::<Vec<_>>()
    });

    Ok(UrpRequest {
        model,
        inputs,
        stream: obj.get("stream").and_then(|v| v.as_bool()),
        temperature: obj.get("temperature").and_then(|v| v.as_f64()),
        top_p: obj.get("top_p").and_then(|v| v.as_f64()),
        max_output_tokens: obj.get("max_output_tokens").and_then(|v| v.as_u64()),
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
                "instructions",
                "input",
                "stream",
                "temperature",
                "top_p",
                "max_output_tokens",
                "reasoning",
                "tools",
                "tool_choice",
                "response_format",
                "text",
                "user",
            ],
        ),
    })
}

fn decode_input_items(input: &Value, out: &mut Vec<Item>) {
    if let Some(s) = input.as_str() {
        out.push(Item::text(Role::User, s));
        return;
    }

    if let Some(obj) = input.as_object() {
        decode_input_item(obj, out);
        return;
    }

    if let Some(arr) = input.as_array() {
        for item in arr {
            if let Some(obj) = item.as_object() {
                decode_input_item(obj, out);
            } else if let Some(s) = item.as_str() {
                out.push(Item::text(Role::User, s));
            }
        }
    }
}

fn decode_input_item(obj: &Map<String, Value>, out: &mut Vec<Item>) {
    let item_type = obj.get("type").and_then(|v| v.as_str()).unwrap_or("");
    match item_type {
        "function_call" => {
            let call_id = obj
                .get("call_id")
                .or_else(|| obj.get("id"))
                .and_then(|v| v.as_str())
                .unwrap_or("")
                .to_string();
            let name = obj
                .get("name")
                .and_then(|v| v.as_str())
                .unwrap_or("")
                .to_string();
            let arguments = obj
                .get("arguments")
                .cloned()
                .unwrap_or(Value::String("{}".to_string()));
            let arguments = if let Some(s) = arguments.as_str() {
                s.to_string()
            } else {
                serde_json::to_string(&arguments).unwrap_or_else(|_| "{}".to_string())
            };
            let mut item = Item::new_message(Role::Assistant);
            push_part(
                &mut item,
                Part::ToolCall {
                    call_id,
                    name,
                    arguments,
                    extra_body: split_extra(obj, &["type", "call_id", "id", "name", "arguments"]),
                },
            );
            out.push(item);
        }
        "function_call_output" => {
            let call_id = obj
                .get("call_id")
                .and_then(|v| v.as_str())
                .unwrap_or("")
                .to_string();
            let mut content = Vec::new();
            if let Some(output) = obj.get("output") {
                decode_tool_result_content(output, &mut content);
            }
            out.push(Item::ToolResult {
                call_id,
                is_error: false,
                content,
                extra_body: split_extra(obj, &["type", "call_id", "output"]),
            });
        }
        "message" | "" => {
            let role = match obj.get("role").and_then(|v| v.as_str()).unwrap_or("user") {
                "system" => Role::System,
                "developer" => Role::Developer,
                "assistant" => Role::Assistant,
                "tool" => Role::Tool,
                _ => Role::User,
            };
            let message_phase = obj.get("phase").and_then(|v| v.as_str());
            let mut parts = Vec::new();

            if let Some(content) = obj.get("content") {
                if let Some(s) = content.as_str() {
                    if !s.is_empty() {
                        parts.push(text_part_with_phase(s, message_phase, HashMap::new()));
                    }
                } else if let Some(content_arr) = content.as_array() {
                    for p in content_arr {
                        let Some(pobj) = p.as_object() else { continue };
                        let ptype = pobj.get("type").and_then(|v| v.as_str()).unwrap_or("");
                        match ptype {
                            "input_text" | "output_text" | "text" => {
                                if let Some(text) = pobj
                                    .get("text")
                                    .and_then(|v| v.as_str())
                                    .or_else(|| pobj.get("content").and_then(|v| v.as_str()))
                                {
                                    parts.push(text_part_with_phase(
                                        text,
                                        message_phase,
                                        split_extra(pobj, &["type", "text", "content"]),
                                    ));
                                }
                            }
                            "refusal" => {
                                if let Some(text) = pobj.get("refusal").and_then(|v| v.as_str()) {
                                    parts.push(Part::Refusal {
                                        content: text.to_string(),
                                        extra_body: split_extra(pobj, &["type", "refusal"]),
                                    });
                                }
                            }
                            _ => {
                                if let Some(image) = parse_image_part_from_obj(pobj) {
                                    parts.push(image);
                                }
                                if let Some(file) = parse_file_part_from_obj(pobj) {
                                    parts.push(file);
                                }
                            }
                        }
                    }
                }
            }

            out.push(build_message_item(
                role,
                parts,
                split_extra(obj, &["type", "role", "content", "phase"]),
            ));
        }
        _ => {
            out.push(build_message_item(
                Role::User,
                vec![text_part_with_phase(
                    serde_json::to_string(obj).unwrap_or_default(),
                    None,
                    HashMap::new(),
                )],
                HashMap::new(),
            ));
        }
    }
}

fn decode_tool_result_content(output: &Value, content: &mut Vec<ToolResultContent>) {
    match output {
        Value::String(text) => {
            if !text.is_empty() {
                content.push(ToolResultContent::Text { text: text.clone() });
            }
        }
        Value::Array(items) => {
            for item in items {
                decode_tool_result_item(item, content);
            }
        }
        Value::Object(_) => decode_tool_result_item(output, content),
        other => {
            let text = value_to_text(other);
            if !text.is_empty() {
                content.push(ToolResultContent::Text { text });
            }
        }
    }
}

fn decode_tool_result_item(value: &Value, content: &mut Vec<ToolResultContent>) {
    if let Some(text) = value.as_str() {
        if !text.is_empty() {
            content.push(ToolResultContent::Text {
                text: text.to_string(),
            });
        }
        return;
    }
    let Some(obj) = value.as_object() else {
        let text = value_to_text(value);
        if !text.is_empty() {
            content.push(ToolResultContent::Text { text });
        }
        return;
    };

    let ptype = obj.get("type").and_then(|v| v.as_str()).unwrap_or("");
    match ptype {
        "input_text" | "output_text" | "text" => {
            if let Some(text) = obj
                .get("text")
                .and_then(|v| v.as_str())
                .or_else(|| obj.get("content").and_then(|v| v.as_str()))
            {
                content.push(ToolResultContent::Text {
                    text: text.to_string(),
                });
            }
        }
        _ => {
            if let Some(image) = parse_image_part_from_obj(obj) {
                let Part::Image { source, .. } = image else {
                    unreachable!();
                };
                content.push(ToolResultContent::Image { source });
                return;
            }
            if let Some(file) = parse_file_part_from_obj(obj) {
                let Part::File { source, .. } = file else {
                    unreachable!();
                };
                content.push(ToolResultContent::File { source });
                return;
            }
            let text = value_to_text(value);
            if !text.is_empty() {
                content.push(ToolResultContent::Text { text });
            }
        }
    }
}

fn merge_extra_body(dst: &mut HashMap<String, Value>, src: HashMap<String, Value>) {
    for (key, value) in src {
        dst.entry(key).or_insert(value);
    }
}

fn flush_assistant_merger(
    merger: &mut GreedyMerger,
    pending_extra_body: &mut HashMap<String, Value>,
    outputs: &mut Vec<Item>,
) {
    if let Some(parts) = merger.finish() {
        outputs.push(build_message_item(
            Role::Assistant,
            parts,
            std::mem::take(pending_extra_body),
        ));
    }
}

fn feed_assistant_part(
    merger: &mut GreedyMerger,
    pending_extra_body: &mut HashMap<String, Value>,
    outputs: &mut Vec<Item>,
    part: Part,
    item_extra_body: &HashMap<String, Value>,
) {
    match merger.feed(part, Role::Assistant) {
        Action::Append => {
            if pending_extra_body.is_empty() {
                *pending_extra_body = item_extra_body.clone();
            } else {
                merge_extra_body(pending_extra_body, item_extra_body.clone());
            }
        }
        Action::FlushAndNew(flushed_parts) => {
            outputs.push(build_message_item(
                Role::Assistant,
                flushed_parts,
                std::mem::take(pending_extra_body),
            ));
            *pending_extra_body = item_extra_body.clone();
        }
    }
}

fn push_assistant_parts(
    merger: &mut GreedyMerger,
    pending_extra_body: &mut HashMap<String, Value>,
    outputs: &mut Vec<Item>,
    item_extra_body: HashMap<String, Value>,
    parts: Vec<Part>,
) {
    for part in parts {
        feed_assistant_part(merger, pending_extra_body, outputs, part, &item_extra_body);
    }
}

pub fn decode_response(value: &Value) -> Result<UrpResponse, String> {
    let obj = value
        .as_object()
        .ok_or_else(|| "responses response must be object".to_string())?;

    let mut outputs = Vec::new();
    let mut merger = GreedyMerger::new();
    let mut pending_assistant_extra_body = HashMap::new();

    if let Some(output) = obj.get("output").and_then(|v| v.as_array()) {
        for item in output {
            let Some(item_obj) = item.as_object() else {
                continue;
            };
            let item_type = item_obj.get("type").and_then(|v| v.as_str()).unwrap_or("");
            match item_type {
                "message" => {
                    let message_phase = item_obj.get("phase").and_then(|v| v.as_str());
                    let role = match item_obj
                        .get("role")
                        .and_then(|v| v.as_str())
                        .unwrap_or("assistant")
                    {
                        "system" => Role::System,
                        "developer" => Role::Developer,
                        "user" => Role::User,
                        "tool" => Role::Tool,
                        _ => Role::Assistant,
                    };
                    let extra_body = split_extra(item_obj, &["type", "role", "content", "phase"]);
                    let mut parts = Vec::new();
                    if let Some(content_arr) = item_obj.get("content").and_then(|v| v.as_array()) {
                        for p in content_arr {
                            let Some(pobj) = p.as_object() else { continue };
                            let ptype = pobj.get("type").and_then(|v| v.as_str()).unwrap_or("");
                            match ptype {
                                "output_text" | "text" => {
                                    if let Some(text) = pobj.get("text").and_then(|v| v.as_str()) {
                                        parts.push(text_part_with_phase(
                                            text,
                                            message_phase,
                                            split_extra(pobj, &["type", "text"]),
                                        ));
                                    }
                                }
                                "refusal" => {
                                    if let Some(text) = pobj.get("refusal").and_then(|v| v.as_str())
                                    {
                                        parts.push(Part::Refusal {
                                            content: text.to_string(),
                                            extra_body: split_extra(pobj, &["type", "refusal"]),
                                        });
                                    }
                                }
                                _ => {
                                    if let Some(image) = parse_image_part_from_obj(pobj) {
                                        parts.push(image);
                                    }
                                    if let Some(file) = parse_file_part_from_obj(pobj) {
                                        parts.push(file);
                                    }
                                }
                            }
                        }
                    }

                    if role == Role::Assistant {
                        push_assistant_parts(
                            &mut merger,
                            &mut pending_assistant_extra_body,
                            &mut outputs,
                            extra_body,
                            parts,
                        );
                    } else {
                        flush_assistant_merger(
                            &mut merger,
                            &mut pending_assistant_extra_body,
                            &mut outputs,
                        );
                        outputs.push(build_message_item(role, parts, extra_body));
                    }
                }
                "function_call" => {
                    let call_id = item_obj
                        .get("call_id")
                        .and_then(|v| v.as_str())
                        .unwrap_or("")
                        .to_string();
                    let name = item_obj
                        .get("name")
                        .and_then(|v| v.as_str())
                        .unwrap_or("")
                        .to_string();
                    let arguments = item_obj
                        .get("arguments")
                        .cloned()
                        .unwrap_or(Value::String("{}".to_string()));
                    let arguments = if let Some(s) = arguments.as_str() {
                        s.to_string()
                    } else {
                        serde_json::to_string(&arguments).unwrap_or_else(|_| "{}".to_string())
                    };
                    feed_assistant_part(
                        &mut merger,
                        &mut pending_assistant_extra_body,
                        &mut outputs,
                        Part::ToolCall {
                            call_id,
                            name,
                            arguments,
                            extra_body: split_extra(
                                item_obj,
                                &["type", "call_id", "name", "arguments"],
                            ),
                        },
                        &HashMap::new(),
                    );
                }
                "function_call_output" => {
                    flush_assistant_merger(
                        &mut merger,
                        &mut pending_assistant_extra_body,
                        &mut outputs,
                    );
                    let call_id = item_obj
                        .get("call_id")
                        .or_else(|| item_obj.get("id"))
                        .and_then(|v| v.as_str())
                        .unwrap_or("")
                        .to_string();
                    let mut content = Vec::new();
                    if let Some(output) = item_obj.get("output") {
                        decode_tool_result_content(output, &mut content);
                    }
                    outputs.push(Item::ToolResult {
                        call_id,
                        is_error: false,
                        content,
                        extra_body: split_extra(item_obj, &["type", "call_id", "id", "output"]),
                    });
                }
                "reasoning" => {
                    let shared_extra = split_extra(
                        item_obj,
                        &["type", "encrypted_content", "summary", "text", "source"],
                    );
                    let encrypted = item_obj.get("encrypted_content").map(|value| match value {
                        Value::String(text) => Value::String(text.clone()),
                        _ => value.clone(),
                    });
                    let summary = item_obj
                        .get("summary")
                        .and_then(|value| value.as_array())
                        .and_then(|_| summary_to_text(item_obj));
                    let text = item_obj
                        .get("text")
                        .and_then(|v| v.as_str())
                        .map(|s| s.to_string());
                    let source = item_obj
                        .get("source")
                        .and_then(|v| v.as_str())
                        .map(|s| s.to_string());
                    if text.is_some() || summary.is_some() || encrypted.is_some() {
                        feed_assistant_part(
                            &mut merger,
                            &mut pending_assistant_extra_body,
                            &mut outputs,
                            Part::Reasoning {
                                content: text.or_else(|| summary.clone()),
                                encrypted,
                                summary,
                                source,
                                extra_body: shared_extra,
                            },
                            &HashMap::new(),
                        );
                    }
                }
                _ => {
                    feed_assistant_part(
                        &mut merger,
                        &mut pending_assistant_extra_body,
                        &mut outputs,
                        Part::ProviderItem {
                            item_type: item_type.to_string(),
                            body: Value::Object(item_obj.clone()),
                            extra_body: HashMap::new(),
                        },
                        &HashMap::new(),
                    );
                }
            }
        }
    }

    flush_assistant_merger(&mut merger, &mut pending_assistant_extra_body, &mut outputs);

    let has_tool_calls = outputs.iter().any(|item| {
        item_parts(item)
            .map(|parts| {
                parts
                    .iter()
                    .any(|part| matches!(part, Part::ToolCall { .. }))
            })
            .unwrap_or(false)
    });

    let finish_reason = match obj.get("status").and_then(|v| v.as_str()) {
        Some("completed") => Some(if has_tool_calls {
            FinishReason::ToolCalls
        } else {
            FinishReason::Stop
        }),
        Some("incomplete") => Some(FinishReason::Length),
        Some("failed") => Some(FinishReason::Other),
        _ => None,
    };

    let usage = obj
        .get("usage")
        .and_then(|v| v.as_object())
        .map(parse_usage_from_responses);

    Ok(UrpResponse {
        id: obj
            .get("id")
            .and_then(|v| v.as_str())
            .unwrap_or("resp")
            .to_string(),
        model: obj
            .get("model")
            .and_then(|v| v.as_str())
            .unwrap_or_default()
            .to_string(),
        outputs,
        finish_reason,
        usage,
        extra_body: split_extra(
            obj,
            &[
                "id",
                "object",
                "created",
                "created_at",
                "model",
                "status",
                "output",
                "usage",
                "error",
            ],
        ),
    })
}

fn summary_to_text(item_obj: &Map<String, Value>) -> Option<String> {
    let mut out = String::new();
    if let Some(summary) = item_obj.get("summary").and_then(|v| v.as_array()) {
        for s in summary {
            if s.get("type").and_then(|v| v.as_str()) == Some("summary_text") {
                if let Some(t) = s.get("text").and_then(|v| v.as_str()) {
                    out.push_str(t);
                }
            }
        }
    }
    if out.is_empty() { None } else { Some(out) }
}

fn parse_usage_from_responses(obj: &Map<String, Value>) -> Usage {
    serde_json::from_value::<OpenAiResponsesUsage>(Value::Object(obj.clone()))
        .map(Usage::from)
        .unwrap_or_else(|_| Usage {
            input_tokens: 0,
            output_tokens: 0,
            input_details: None,
            output_details: None,
            extra_body: obj.clone().into_iter().collect(),
        })
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
        if obj.get("type").and_then(|x| x.as_str()) == Some("json_schema") {
            let schema_obj = obj.get("json_schema")?.as_object()?;
            let name = schema_obj.get("name")?.as_str()?.to_string();
            let schema = schema_obj.get("schema").cloned().unwrap_or(Value::Null);
            return Some(crate::urp::ResponseFormat::JsonSchema {
                json_schema: crate::urp::JsonSchemaDefinition {
                    name,
                    description: schema_obj
                        .get("description")
                        .and_then(|x| x.as_str())
                        .map(|s| s.to_string()),
                    schema,
                    strict: schema_obj.get("strict").and_then(|x| x.as_bool()),
                    extra_body: split_extra(
                        schema_obj,
                        &["name", "description", "schema", "strict"],
                    ),
                },
            });
        }
        if obj.get("type").and_then(|x| x.as_str()) == Some("json_object") {
            return Some(crate::urp::ResponseFormat::JsonObject);
        }
    }
    None
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn parse_usage_reads_cache_write_tokens_from_input_details() {
        let usage = parse_usage_from_responses(
            json!({
                "input_tokens": 120,
                "output_tokens": 35,
                "input_tokens_details": {
                    "cached_tokens": 10,
                    "cache_write_tokens": 64
                }
            })
            .as_object()
            .expect("usage json object expected"),
        );

        let details = usage
            .input_details
            .expect("input_details should be present");
        assert_eq!(details.cache_read_tokens, 10);
        assert_eq!(details.cache_creation_tokens, 64);
    }

    #[test]
    fn parse_usage_keeps_cache_creation_when_cached_tokens_are_zero() {
        let usage = parse_usage_from_responses(
            json!({
                "input_tokens": 98,
                "output_tokens": 2,
                "input_tokens_details": {
                    "cached_tokens": 0,
                    "cache_creation_tokens": 98
                }
            })
            .as_object()
            .expect("usage json object expected"),
        );

        let details = usage
            .input_details
            .expect("input_details should be present");
        assert_eq!(details.cache_read_tokens, 0);
        assert_eq!(details.cache_creation_tokens, 98);
    }
}
