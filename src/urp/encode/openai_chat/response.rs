//! DC: URP responses to downstream non-stream Chat completions.

use super::reasoning::{ReasoningTarget, insert_reasoning_fields};
use super::usage::encode_chat_usage;
use super::{
    chat_completion_id, chat_provider_item_wire_body, encode_chat_provider_part,
    encode_chat_tool_call, encode_legacy_function_call, flag_set, is_legacy_function_call,
    merge_chat_wire_extra,
};
use crate::urp::{
    AudioSource, CHAT_CHOICE_EXTRA_BODY_KEY, CHAT_MESSAGE_AUDIO_EXTRA_KEY,
    CHAT_NATIVE_FINISH_REASON_EXTRA_KEY, FinishReason, MediaMetadata, Node, OrdinaryRole,
    ProviderProtocol, ToolResultContent, UrpResponse, is_internal_key,
};
use serde_json::{Map, Value, json};
use std::collections::HashMap;

pub fn encode_response(resp: &UrpResponse, logical_model: &str) -> Value {
    encode_response_checked(resp, logical_model)
        .unwrap_or_else(|error| crate::urp::media::error_body(&error))
}

pub fn encode_response_checked(resp: &UrpResponse, logical_model: &str) -> Result<Value, String> {
    if let Some(body) = resp
        .outcome
        .as_ref()
        .and_then(|outcome| outcome.failure_body(false))
    {
        return Ok(body);
    }
    validate_response_nodes(&resp.output)?;
    Ok(encode_response_validated(resp, logical_model))
}

pub(crate) fn validate_response_nodes(nodes: &[Node]) -> Result<(), String> {
    for node in nodes {
        match node {
            Node::Image { .. } | Node::File { .. } => {
                return Err(
                    "Chat Completions responses cannot represent ordinary image or file output"
                        .into(),
                );
            }
            Node::Audio {
                role: OrdinaryRole::Assistant,
                source: AudioSource::Base64 { .. },
                extra_body,
                ..
            } if flag_set(extra_body, CHAT_MESSAGE_AUDIO_EXTRA_KEY) => {}
            Node::Audio { .. } => {
                return Err(
                    "Chat Completions responses require native message.audio for audio output"
                        .into(),
                );
            }
            Node::ToolResult { content, .. }
                if content.iter().any(|part| {
                    matches!(
                        part,
                        ToolResultContent::Image { .. } | ToolResultContent::File { .. }
                    )
                }) =>
            {
                return Err("Chat Completions responses cannot represent tool-result media".into());
            }
            Node::ProviderItem { item_type, .. }
                if matches!(
                    item_type.as_str(),
                    "input_image"
                        | "output_image"
                        | "image_url"
                        | "input_file"
                        | "output_file"
                        | "file"
                        | "input_audio"
                ) =>
            {
                return Err("Native response content cannot contain input-only media items".into());
            }
            _ => {}
        }
    }
    Ok(())
}

fn encode_response_validated(resp: &UrpResponse, logical_model: &str) -> Value {
    let resp = &crate::urp::tool_signature::project_response(resp);
    let message = encode_assistant_chat_message_from_nodes(&resp.output);
    let has_legacy_function_call = resp.output.iter().any(|node| {
        matches!(node, Node::ToolCall { tool_type, extra_body, .. }
            if is_legacy_function_call(*tool_type, extra_body))
    });

    let native_finish_reason = resp
        .extra_body
        .get(CHAT_NATIVE_FINISH_REASON_EXTRA_KEY)
        .and_then(Value::as_str)
        .filter(|reason| !reason.is_empty());
    let finish_reason = match resp.finish_reason {
        Some(FinishReason::Other) => native_finish_reason.unwrap_or("stop"),
        Some(FinishReason::ToolCalls) if has_legacy_function_call => "function_call",
        Some(reason) => finish_reason_to_chat(reason),
        None if has_legacy_function_call => "function_call",
        None if resp
            .output
            .iter()
            .any(|node| matches!(node, Node::ToolCall { .. })) =>
        {
            "tool_calls"
        }
        None => "stop",
    };

    let mut result = json!({
        "id": chat_completion_id(&resp.id),
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

    let mut content_scores = Vec::new();
    let mut refusal_scores = Vec::new();
    for node in &resp.output {
        match node {
            Node::Text {
                content, logprobs, ..
            } => {
                if let Some(scores) = crate::urp::logprobs::valid(logprobs, content) {
                    content_scores.extend_from_slice(scores);
                }
            }
            Node::Refusal {
                content, logprobs, ..
            } => {
                if let Some(scores) = crate::urp::logprobs::valid(logprobs, content) {
                    refusal_scores.extend_from_slice(scores);
                }
            }
            _ => {}
        }
    }
    let message = &result["choices"][0]["message"];
    let content_scores = Some(content_scores);
    let refusal_scores = Some(refusal_scores);
    let content_scores = message
        .get("content")
        .and_then(Value::as_str)
        .and_then(|text| crate::urp::logprobs::valid(&content_scores, text));
    let refusal_scores = message
        .get("refusal")
        .and_then(Value::as_str)
        .and_then(|text| crate::urp::logprobs::valid(&refusal_scores, text));
    if content_scores.is_some_and(|v| !v.is_empty())
        || refusal_scores.is_some_and(|v| !v.is_empty())
    {
        result["choices"][0]["logprobs"] = json!({
            "content": content_scores.map(crate::urp::logprobs::encode_openai),
            "refusal": refusal_scores.map(crate::urp::logprobs::encode_openai),
        });
    }
    if let Some(usage) = &resp.usage {
        result["usage"] = encode_chat_usage(usage);
    }

    if let Some(choice_extra) = resp
        .extra_body
        .get(CHAT_CHOICE_EXTRA_BODY_KEY)
        .and_then(Value::as_object)
        && let Some(choice) = result["choices"][0].as_object_mut()
    {
        for (key, value) in choice_extra {
            if !is_internal_key(key) && key != "logprobs" {
                choice.entry(key.clone()).or_insert_with(|| value.clone());
            }
        }
    }

    let obj = result.as_object_mut().expect("chat response object");
    merge_chat_wire_extra(obj, &resp.extra_body);
    result
}

#[derive(Default)]
struct AssistantContent<'a> {
    parts: Vec<Value>,
    text_offset: u64,
    text_phases: Vec<Option<&'a String>>,
}

impl<'a> AssistantContent<'a> {
    fn push_text(
        &mut self,
        message: &mut Map<String, Value>,
        content: &str,
        citations: &[crate::urp::Citation],
        phase: Option<&'a String>,
        extra_body: &HashMap<String, Value>,
    ) {
        if !citations.is_empty() {
            message
                .entry("annotations".to_string())
                .or_insert_with(|| json!([]))
                .as_array_mut()
                .expect("generated annotations array")
                .extend(crate::urp::citations::encode(
                    citations,
                    ProviderProtocol::ChatCompletion,
                    self.text_offset,
                ));
        }
        let mut block = Map::from_iter([
            ("type".to_string(), json!("text")),
            ("text".to_string(), json!(content)),
        ]);
        merge_chat_wire_extra(&mut block, extra_body);
        self.text_phases.push(phase);
        self.text_offset += content.chars().count() as u64;
        self.parts.push(Value::Object(block));
    }

    fn insert_phase(&self, message_extra: &mut HashMap<String, Value>) {
        // DC4a: `phase` describes the whole message only when every text node agrees.
        if let Some(Some(first)) = self.text_phases.first()
            && self.text_phases.iter().all(|phase| *phase == Some(*first))
        {
            message_extra
                .entry("phase".to_string())
                .or_insert_with(|| Value::String((*first).clone()));
        }
    }
}

/// DC4: the assistant `message` of a Chat completion from the response output nodes.
pub(crate) fn encode_assistant_chat_message_from_nodes(nodes: &[Node]) -> Map<String, Value> {
    let mut message = Map::new();
    message.insert("role".to_string(), Value::String("assistant".to_string()));

    let mut content = AssistantContent::default();
    let mut tool_calls = Vec::new();
    let mut refusal: Option<String> = None;
    let mut reasoning = Vec::new();
    let mut legacy_function_call: Option<Value> = None;
    let mut message_extra = HashMap::new();

    for node in nodes {
        match node {
            Node::NextDownstreamEnvelopeExtra { extra_body } => {
                merge_message_extra(&mut message_extra, extra_body);
            }
            Node::Text {
                role: OrdinaryRole::Assistant,
                content: text,
                citations,
                phase,
                extra_body,
                ..
            } => {
                content.push_text(&mut message, text, citations, phase.as_ref(), extra_body);
            }
            Node::Audio {
                metadata,
                source,
                extra_body,
                ..
            } if flag_set(extra_body, CHAT_MESSAGE_AUDIO_EXTRA_KEY) => {
                if let Some(audio) = encode_generated_audio(metadata, source, extra_body) {
                    message_extra.insert("audio".into(), audio);
                }
            }
            Node::Refusal { content, .. } => {
                refusal.get_or_insert_with(String::new).push_str(content);
            }
            Node::Reasoning { .. } => reasoning.push(node),
            Node::ToolCall {
                tool_type,
                call_id,
                name,
                arguments,
                extra_body,
                ..
            } => {
                if is_legacy_function_call(*tool_type, extra_body) {
                    legacy_function_call =
                        Some(encode_legacy_function_call(name, arguments, extra_body));
                } else {
                    tool_calls.push(encode_chat_tool_call(*tool_type, call_id, name, arguments));
                }
            }
            Node::ProviderItem {
                id,
                item_type,
                role: OrdinaryRole::Assistant,
                origin_protocol: ProviderProtocol::ChatCompletion,
                body,
                extra_body,
            } if flag_set(extra_body, CHAT_MESSAGE_AUDIO_EXTRA_KEY) => {
                message_extra.insert(
                    "audio".to_string(),
                    chat_provider_item_wire_body(id.as_deref(), item_type, body, extra_body),
                );
            }
            Node::ProviderItem {
                id,
                item_type,
                role: OrdinaryRole::Assistant,
                origin_protocol,
                body,
                extra_body,
            } => {
                if let Some(part) = encode_chat_provider_part(
                    *origin_protocol,
                    id.as_deref(),
                    item_type,
                    body,
                    extra_body,
                ) {
                    content.parts.push(part);
                }
            }
            _ => {}
        }
    }

    content.insert_phase(&mut message_extra);
    let had_content_parts = !content.parts.is_empty();
    message.insert("content".to_string(), response_content(content.parts));
    if let Some(refusal) = refusal {
        message.insert("refusal".to_string(), Value::String(refusal));
    }
    if !tool_calls.is_empty() {
        message.insert("tool_calls".to_string(), Value::Array(tool_calls));
    }
    if let Some(function_call) = legacy_function_call {
        message.insert("function_call".to_string(), function_call);
    }
    insert_reasoning_fields(&mut message, &reasoning, ReasoningTarget::Response);
    merge_chat_wire_extra(&mut message, &message_extra);
    if !had_content_parts
        && (message.contains_key("audio")
            || message.contains_key("function_call")
            || message.contains_key("tool_calls")
            || message.contains_key("refusal"))
    {
        message.insert("content".to_string(), Value::Null);
    }
    message
}

fn merge_message_extra(target: &mut HashMap<String, Value>, extra: &HashMap<String, Value>) {
    for (key, value) in extra {
        if !is_internal_key(key) {
            target.entry(key.clone()).or_insert_with(|| value.clone());
        }
    }
}

/// DC4: text-only content concatenates to one string; any other part keeps the array form.
fn response_content(content_parts: Vec<Value>) -> Value {
    let is_text = |part: &Value| part.get("type").and_then(Value::as_str) == Some("text");
    if !content_parts.iter().all(is_text) {
        return Value::Array(content_parts);
    }
    Value::String(
        content_parts
            .iter()
            .filter_map(|part| part.get("text").and_then(Value::as_str))
            .collect(),
    )
}

/// The native Chat `message.audio` object of a generated Audio node.
fn encode_generated_audio(
    metadata: &MediaMetadata,
    source: &AudioSource,
    extra_body: &HashMap<String, Value>,
) -> Option<Value> {
    let AudioSource::Base64 { data, .. } = source else {
        return None;
    };
    let mut audio = Map::new();
    for (key, value) in extra_body {
        if !is_internal_key(key)
            && !matches!(key.as_str(), "id" | "data" | "transcript" | "expires_at")
        {
            audio.insert(key.clone(), value.clone());
        }
    }
    audio.insert("data".into(), json!(data));
    if let Some(id) = &metadata.reference_id {
        audio.insert("id".into(), json!(id));
    }
    if let Some(transcript) = &metadata.transcript {
        audio.insert("transcript".into(), json!(transcript));
    }
    if let Some(expires_at) = metadata.expires_at {
        audio.insert("expires_at".into(), json!(expires_at));
    }
    Some(Value::Object(audio))
}

pub(crate) fn finish_reason_to_chat(finish_reason: FinishReason) -> &'static str {
    match finish_reason {
        FinishReason::Stop => "stop",
        FinishReason::Length
        | FinishReason::ContextLimit
        | FinishReason::Paused
        | FinishReason::Compaction => "length",
        FinishReason::ToolCalls => "tool_calls",
        FinishReason::ContentFilter => "content_filter",
        FinishReason::Other => "stop",
    }
}
