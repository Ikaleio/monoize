use crate::urp::encode::{
    merge_extra, tool_choice_to_value, usage_input_details, usage_output_details,
};
use crate::urp::{
    FileSource, FinishReason, ImageSource, Node, OrdinaryRole, REASONING_KIND_EXTRA_KEY,
    REASONING_KIND_REDACTED_THINKING, ToolDefinition, ToolResultContent, UrpRequest, UrpResponse,
    Usage, strip_reasoning_signature_sigil, wrap_reasoning_signature_with_item_id,
};
use serde_json::{Map, Value, json};
use std::collections::HashMap;

const ANTHROPIC_DEFAULT_MAX_TOKENS: u64 = 64_000;

/// Controls whether the Anthropic encoder should embed a sigil (`mz1.<id>.<sig>`) in
/// `thinking.signature` / `redacted_thinking.data` to smuggle a reasoning item id through a
/// downstream client that strips unknown fields. Upstream-facing encoding MUST use `StripSigil`
/// so that the real Anthropic API receives a clean opaque signature. Downstream-facing encoding
/// MUST use `EmbedSigil` so that monoize can recover the item id when the client echoes the
/// history back. See `spec/unified_responses_proxy.spec.md` DM5.2 / PM5b.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ReasoningSigilMode {
    EmbedSigil,
    StripSigil,
}

fn reasoning_is_redacted(extra_body: &HashMap<String, Value>) -> bool {
    extra_body
        .get(REASONING_KIND_EXTRA_KEY)
        .and_then(Value::as_str)
        == Some(REASONING_KIND_REDACTED_THINKING)
}

fn reasoning_extra_for_wire(extra_body: &HashMap<String, Value>) -> HashMap<String, Value> {
    extra_body
        .iter()
        .filter(|(key, _)| key.as_str() != REASONING_KIND_EXTRA_KEY)
        .map(|(k, v)| (k.clone(), v.clone()))
        .collect()
}

fn plaintext_from_reasoning<'a>(
    content: &'a Option<String>,
    summary: &'a Option<String>,
) -> Option<&'a str> {
    content
        .as_deref()
        .filter(|s| !s.is_empty())
        .or_else(|| summary.as_deref().filter(|s| !s.is_empty()))
}

fn encoded_signature_value(
    encrypted: &Option<Value>,
    id: &Option<String>,
    mode: ReasoningSigilMode,
) -> Option<Value> {
    let raw = match encrypted {
        None | Some(Value::Null) => return None,
        Some(Value::String(s)) if s.is_empty() => return None,
        Some(Value::String(s)) => s.clone(),
        Some(other) => return Some(other.clone()),
    };
    let processed = match mode {
        ReasoningSigilMode::EmbedSigil => id
            .as_deref()
            .filter(|s| !s.is_empty())
            .and_then(|item_id| wrap_reasoning_signature_with_item_id(item_id, &raw))
            .unwrap_or(raw),
        ReasoningSigilMode::StripSigil => strip_reasoning_signature_sigil(&raw),
    };
    if processed.is_empty() {
        None
    } else {
        Some(Value::String(processed))
    }
}

// Invert the normalization performed in decode/anthropic.rs: internal `Usage.input_tokens`
// is aggregate/inclusive (spec § 5 C3), but Anthropic's wire format requires `input_tokens`
// to exclude cache_read and cache_creation buckets. Saturating subtraction guards against
// malformed upstream data where cache buckets alone exceed the recorded total.
pub(crate) fn anthropic_native_input_tokens(usage: &Usage) -> u64 {
    let cache_read = usage
        .input_details
        .as_ref()
        .map(|d| d.cache_read_tokens)
        .unwrap_or(0);
    let cache_creation = usage
        .input_details
        .as_ref()
        .map(|d| d.cache_creation_tokens)
        .unwrap_or(0);
    usage
        .input_tokens
        .saturating_sub(cache_read)
        .saturating_sub(cache_creation)
}

pub fn encode_request(req: &UrpRequest, upstream_model: &str) -> Value {
    let mut system_blocks: Vec<Value> = Vec::new();
    let mut messages: Vec<Value> = Vec::new();
    let request_nodes = &req.input;
    let mut pending_message: Option<AnthropicMessageEnvelope> = None;

    for node in request_nodes {
        match node {
            Node::NextDownstreamEnvelopeExtra { .. } => {
                flush_pending_anthropic_message(&mut pending_message, &mut messages);
            }
            Node::ToolResult {
                id: _,
                call_id,
                content,
                is_error,
                extra_body,
            } => {
                flush_pending_anthropic_message(&mut pending_message, &mut messages);
                messages.push(encode_tool_result_message(
                    call_id, content, *is_error, extra_body,
                ));
            }
            Node::Text {
                role: OrdinaryRole::System | OrdinaryRole::Developer,
                ..
            } => {
                flush_pending_anthropic_message(&mut pending_message, &mut messages);
                if let Some(block) = encode_system_block(node) {
                    system_blocks.push(block);
                }
            }
            Node::Text {
                role: OrdinaryRole::User | OrdinaryRole::Assistant,
                ..
            }
            | Node::Image {
                role: OrdinaryRole::User | OrdinaryRole::Assistant,
                ..
            }
            | Node::File {
                role: OrdinaryRole::User | OrdinaryRole::Assistant,
                ..
            }
            | Node::ProviderItem {
                role: OrdinaryRole::User | OrdinaryRole::Assistant,
                ..
            }
            | Node::Reasoning { .. }
            | Node::ToolCall { .. } => {
                append_node_to_pending_anthropic_message(
                    &mut pending_message,
                    &mut messages,
                    node,
                    ReasoningSigilMode::StripSigil,
                );
            }
            Node::Image {
                role: OrdinaryRole::System | OrdinaryRole::Developer,
                ..
            }
            | Node::File {
                role: OrdinaryRole::System | OrdinaryRole::Developer,
                ..
            }
            | Node::ProviderItem {
                role: OrdinaryRole::System | OrdinaryRole::Developer,
                ..
            } => {
                flush_pending_anthropic_message(&mut pending_message, &mut messages);
            }
            Node::Audio { .. } | Node::Refusal { .. } => {}
        }
    }
    flush_pending_anthropic_message(&mut pending_message, &mut messages);

    let mut body = json!({
        "model": upstream_model,
        "messages": messages,
        "max_tokens": req
            .max_output_tokens
            .unwrap_or(ANTHROPIC_DEFAULT_MAX_TOKENS),
    });
    let obj = body.as_object_mut().expect("anthropic request object");

    if !system_blocks.is_empty() {
        obj.insert("system".to_string(), Value::Array(system_blocks));
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
    if let Some(tools) = &req.tools {
        obj.insert("tools".to_string(), Value::Array(encode_tools(tools)));
    }
    if let Some(choice) = &req.tool_choice {
        obj.insert(
            "tool_choice".to_string(),
            encode_tool_choice_for_anthropic(choice),
        );
    }
    if let Some(reasoning) = &req.reasoning {
        if let Some(effort) = &reasoning.effort {
            if model_supports_adaptive(upstream_model) {
                obj.insert("thinking".to_string(), json!({ "type": "adaptive" }));
                obj.insert("output_config".to_string(), json!({ "effort": effort }));
            } else {
                obj.insert(
                    "thinking".to_string(),
                    json!({
                        "type": "enabled",
                        "budget_tokens": effort_to_budget(effort)
                    }),
                );
            }
        }
    }
    merge_extra(obj, &req.extra_body);
    body
}

pub fn encode_response(resp: &UrpResponse, logical_model: &str) -> Value {
    let response_nodes = &resp.output;
    let mut content = Vec::new();
    for node in response_nodes {
        if let Some(block) = encode_assistant_response_block(node) {
            content.push(block);
        }
    }

    let mut body = json!({
        "id": resp.id,
        "type": "message",
        "role": "assistant",
        "model": logical_model,
        "content": content,
        "stop_reason": finish_reason_to_stop_reason(resp.finish_reason),
    });

    let mut usage_value = json!({
        "input_tokens": 0,
        "output_tokens": 0,
        "cache_read_input_tokens": 0,
        "cache_creation_input_tokens": 0,
        "tool_prompt_input_tokens": 0,
        "reasoning_output_tokens": 0,
        "accepted_prediction_output_tokens": 0,
        "rejected_prediction_output_tokens": 0
    });
    if let Some(usage) = &resp.usage {
        if let Some(obj) = usage_value.as_object_mut() {
            let input_details = usage_input_details(usage);
            let output_details = usage_output_details(usage);
            obj.insert(
                "input_tokens".to_string(),
                Value::from(anthropic_native_input_tokens(usage)),
            );
            obj.insert(
                "output_tokens".to_string(),
                Value::from(usage.output_tokens),
            );
            obj.insert(
                "cache_read_input_tokens".to_string(),
                Value::from(input_details.cache_read_tokens),
            );
            obj.insert(
                "cache_creation_input_tokens".to_string(),
                Value::from(input_details.cache_creation_tokens),
            );
            obj.insert(
                "tool_prompt_input_tokens".to_string(),
                Value::from(input_details.tool_prompt_tokens),
            );
            obj.insert(
                "reasoning_output_tokens".to_string(),
                Value::from(output_details.reasoning_tokens),
            );
            obj.insert(
                "accepted_prediction_output_tokens".to_string(),
                Value::from(output_details.accepted_prediction_tokens),
            );
            obj.insert(
                "rejected_prediction_output_tokens".to_string(),
                Value::from(output_details.rejected_prediction_tokens),
            );
            for (k, v) in &usage.extra_body {
                obj.insert(k.clone(), v.clone());
            }
        }
    }
    body["usage"] = usage_value;
    if let Some(obj) = body.as_object_mut() {
        merge_extra(obj, &resp.extra_body);
    }
    body
}

#[derive(Clone)]
struct AnthropicMessageEnvelope {
    role: OrdinaryRole,
    content: Vec<Value>,
    extra_body: HashMap<String, Value>,
}

fn flush_pending_anthropic_message(
    pending: &mut Option<AnthropicMessageEnvelope>,
    out: &mut Vec<Value>,
) {
    let Some(message) = pending.take() else {
        return;
    };
    if message.content.is_empty() {
        return;
    }

    let role = match message.role {
        OrdinaryRole::Assistant => "assistant",
        OrdinaryRole::User | OrdinaryRole::System | OrdinaryRole::Developer => "user",
    };
    let mut msg = json!({ "role": role, "content": message.content });
    if let Some(obj) = msg.as_object_mut() {
        merge_extra(obj, &message.extra_body);
    }
    out.push(msg);
}

fn append_node_to_pending_anthropic_message(
    pending: &mut Option<AnthropicMessageEnvelope>,
    out: &mut Vec<Value>,
    node: &Node,
    sigil_mode: ReasoningSigilMode,
) {
    let Some(role) = anthropic_message_role_for_node(node) else {
        return;
    };
    let should_flush = pending
        .as_ref()
        .is_some_and(|existing| existing.role != role);
    if should_flush {
        flush_pending_anthropic_message(pending, out);
    }

    let Some(block) = encode_regular_message_block(node, sigil_mode) else {
        return;
    };
    let entry = pending.get_or_insert_with(|| AnthropicMessageEnvelope {
        role,
        content: Vec::new(),
        extra_body: anthropic_message_extra_from_node(node),
    });
    entry.content.push(block);
}

fn anthropic_message_role_for_node(node: &Node) -> Option<OrdinaryRole> {
    match node {
        Node::Text { role, .. }
        | Node::Image { role, .. }
        | Node::File { role, .. }
        | Node::ProviderItem { role, .. } => match role {
            OrdinaryRole::System | OrdinaryRole::Developer => None,
            OrdinaryRole::User | OrdinaryRole::Assistant => Some(*role),
        },
        Node::Reasoning { .. } | Node::ToolCall { .. } => Some(OrdinaryRole::Assistant),
        Node::ToolResult { .. }
        | Node::NextDownstreamEnvelopeExtra { .. }
        | Node::Audio { .. }
        | Node::Refusal { .. } => None,
    }
}

fn anthropic_message_extra_from_node(node: &Node) -> HashMap<String, Value> {
    match node {
        Node::Text {
            phase, extra_body, ..
        } => {
            let mut out = extra_body.clone();
            if let Some(phase) = phase {
                out.insert("phase".to_string(), Value::String(phase.clone()));
            }
            out
        }
        Node::Image { extra_body, .. }
        | Node::File { extra_body, .. }
        | Node::Reasoning { extra_body, .. }
        | Node::ToolCall { extra_body, .. }
        | Node::ProviderItem { extra_body, .. } => extra_body.clone(),
        Node::Audio { .. }
        | Node::Refusal { .. }
        | Node::ToolResult { .. }
        | Node::NextDownstreamEnvelopeExtra { .. } => HashMap::new(),
    }
}

fn encode_system_block(node: &Node) -> Option<Value> {
    match node {
        Node::Text {
            content,
            phase,
            extra_body,
            ..
        } if !content.is_empty() => {
            let mut block = json!({ "type": "text", "text": content });
            if let Some(obj) = block.as_object_mut() {
                if let Some(phase) = phase {
                    obj.insert("phase".to_string(), Value::String(phase.clone()));
                }
                merge_extra(obj, extra_body);
            }
            Some(block)
        }
        _ => None,
    }
}

fn encode_regular_message_block(node: &Node, sigil_mode: ReasoningSigilMode) -> Option<Value> {
    match node {
        Node::Text {
            content,
            phase,
            extra_body,
            ..
        } => {
            let mut block = json!({ "type": "text", "text": content });
            if let Some(obj) = block.as_object_mut() {
                if let Some(phase) = phase {
                    obj.insert("phase".to_string(), Value::String(phase.clone()));
                }
                merge_extra(obj, extra_body);
            }
            Some(block)
        }
        Node::Image {
            source, extra_body, ..
        } => {
            let mut block = encode_anthropic_image(source);
            if let Some(obj) = block.as_object_mut() {
                merge_extra(obj, extra_body);
            }
            Some(block)
        }
        Node::File {
            source, extra_body, ..
        } => {
            let mut block = encode_anthropic_file(source);
            if let Some(obj) = block.as_object_mut() {
                merge_extra(obj, extra_body);
            }
            Some(block)
        }
        Node::Reasoning {
            id,
            content,
            summary,
            encrypted,
            extra_body,
            ..
        } => {
            let wire_extra = reasoning_extra_for_wire(extra_body);
            let signature = encoded_signature_value(encrypted, id, sigil_mode);
            if let Some(text) = plaintext_from_reasoning(content, summary) {
                let mut block = json!({ "type": "thinking", "thinking": text });
                let obj = block.as_object_mut().expect("thinking block object");
                if let Some(sig) = signature {
                    obj.insert("signature".to_string(), sig);
                }
                merge_extra(obj, &wire_extra);
                Some(block)
            } else if reasoning_is_redacted(extra_body) {
                let data = signature?;
                let mut block = json!({ "type": "redacted_thinking", "data": data });
                let obj = block
                    .as_object_mut()
                    .expect("redacted_thinking block object");
                merge_extra(obj, &wire_extra);
                Some(block)
            } else {
                None
            }
        }
        Node::ToolCall {
            id: _,
            call_id,
            name,
            arguments,
            extra_body,
        } => {
            let input = serde_json::from_str::<Value>(arguments)
                .unwrap_or_else(|_| json!({ "_raw": arguments }));
            let mut block = json!({
                "type": "tool_use",
                "id": call_id,
                "name": name,
                "input": input
            });
            if let Some(obj) = block.as_object_mut() {
                merge_extra(obj, extra_body);
            }
            Some(block)
        }
        Node::ProviderItem {
            body, extra_body, ..
        } => {
            let mut block = body.clone();
            if let Some(obj) = block.as_object_mut() {
                merge_extra(obj, extra_body);
            }
            Some(block)
        }
        Node::Audio { .. }
        | Node::Refusal { .. }
        | Node::ToolResult { .. }
        | Node::NextDownstreamEnvelopeExtra { .. } => None,
    }
}

fn encode_assistant_response_block(node: &Node) -> Option<Value> {
    match node {
        Node::Text {
            role: OrdinaryRole::Assistant,
            content,
            phase,
            extra_body,
            ..
        } => {
            let mut block = json!({ "type": "text", "text": content });
            if let Some(obj) = block.as_object_mut() {
                if let Some(phase) = phase {
                    obj.insert("phase".to_string(), Value::String(phase.clone()));
                }
                merge_extra(obj, extra_body);
            }
            Some(block)
        }
        Node::Reasoning {
            id,
            content,
            summary,
            encrypted,
            extra_body,
            ..
        } => {
            let wire_extra = reasoning_extra_for_wire(extra_body);
            let signature = encoded_signature_value(encrypted, id, ReasoningSigilMode::EmbedSigil);
            if let Some(text) = plaintext_from_reasoning(content, summary) {
                let mut thinking = Map::new();
                thinking.insert("type".to_string(), Value::String("thinking".to_string()));
                thinking.insert("thinking".to_string(), Value::String(text.to_string()));
                if let Some(sig) = signature {
                    thinking.insert("signature".to_string(), sig);
                }
                merge_extra(&mut thinking, &wire_extra);
                Some(Value::Object(thinking))
            } else if reasoning_is_redacted(extra_body) {
                let data = signature?;
                let mut block = Map::new();
                block.insert(
                    "type".to_string(),
                    Value::String("redacted_thinking".to_string()),
                );
                block.insert("data".to_string(), data);
                merge_extra(&mut block, &wire_extra);
                Some(Value::Object(block))
            } else {
                None
            }
        }
        Node::ToolCall {
            id: _,
            call_id,
            name,
            arguments,
            extra_body,
        } => {
            let input = serde_json::from_str::<Value>(arguments)
                .unwrap_or_else(|_| json!({ "_raw": arguments }));
            let mut block = json!({
                "type": "tool_use",
                "id": call_id,
                "name": name,
                "input": input
            });
            if let Some(obj) = block.as_object_mut() {
                merge_extra(obj, extra_body);
            }
            Some(block)
        }
        Node::Image {
            role: OrdinaryRole::Assistant,
            source,
            extra_body,
            ..
        } => {
            let mut block = encode_anthropic_image(source);
            if let Some(obj) = block.as_object_mut() {
                merge_extra(obj, extra_body);
            }
            Some(block)
        }
        Node::File {
            role: OrdinaryRole::Assistant,
            source,
            extra_body,
            ..
        } => {
            let mut block = encode_anthropic_file(source);
            if let Some(obj) = block.as_object_mut() {
                merge_extra(obj, extra_body);
            }
            Some(block)
        }
        Node::ProviderItem {
            role: OrdinaryRole::Assistant,
            body,
            extra_body,
            ..
        } => {
            let mut block = body.clone();
            if let Some(obj) = block.as_object_mut() {
                merge_extra(obj, extra_body);
            }
            Some(block)
        }
        Node::Audio { .. }
        | Node::Refusal { .. }
        | Node::ToolResult { .. }
        | Node::NextDownstreamEnvelopeExtra { .. }
        | Node::Text { .. }
        | Node::Image { .. }
        | Node::File { .. }
        | Node::ProviderItem { .. } => None,
    }
}

fn encode_tool_result_message(
    call_id: &str,
    content: &[ToolResultContent],
    is_error: bool,
    extra_body: &HashMap<String, Value>,
) -> Value {
    let mut content: Vec<Value> = content
        .iter()
        .map(|item| match item {
            ToolResultContent::Text { text } => json!({ "type": "text", "text": text }),
            ToolResultContent::Image { source } => encode_anthropic_image(source),
            ToolResultContent::File { source } => encode_anthropic_file(source),
        })
        .collect();
    if content.is_empty() {
        content.push(json!({ "type": "text", "text": "" }));
    }
    let mut tool_result_block = json!({
        "type": "tool_result",
        "tool_use_id": call_id,
        "is_error": is_error,
        "content": content
    });
    if let Some(obj) = tool_result_block.as_object_mut() {
        merge_extra(obj, extra_body);
    }
    json!({
        "role": "user",
        "content": [tool_result_block]
    })
}

fn encode_tools(tools: &[ToolDefinition]) -> Vec<Value> {
    let mut out = Vec::new();
    for tool in tools {
        if tool.tool_type == "function" {
            if let Some(function) = &tool.function {
                out.push(json!({
                    "name": function.name,
                    "description": function.description,
                    "input_schema": function.parameters.clone().unwrap_or(json!({
                        "type": "object",
                        "properties": {},
                        "additionalProperties": true
                    }))
                }));
            }
        } else {
            let mut obj = HashMap::new();
            obj.insert("name".to_string(), Value::String(tool.tool_type.clone()));
            for (k, v) in &tool.extra_body {
                obj.entry(k.clone()).or_insert_with(|| v.clone());
            }
            out.push(Value::Object(obj.into_iter().collect()));
        }
    }
    out
}

fn encode_tool_choice_for_anthropic(choice: &crate::urp::ToolChoice) -> Value {
    match tool_choice_to_value(choice) {
        Value::String(mode) => match mode.as_str() {
            "auto" => json!({ "type": "auto" }),
            "required" => json!({ "type": "any" }),
            "none" => json!({ "type": "none" }),
            _ => Value::String(mode),
        },
        Value::Object(obj) => {
            if let Some(name) = obj
                .get("function")
                .and_then(|v| v.get("name"))
                .and_then(|v| v.as_str())
            {
                json!({ "type": "tool", "name": name })
            } else {
                Value::Object(obj)
            }
        }
        other => other,
    }
}

fn encode_anthropic_image(source: &ImageSource) -> Value {
    match source {
        ImageSource::Url { url, .. } => json!({
            "type": "image",
            "source": { "type": "url", "url": url }
        }),
        ImageSource::Base64 { media_type, data } => json!({
            "type": "image",
            "source": { "type": "base64", "media_type": media_type, "data": data }
        }),
    }
}

fn encode_anthropic_file(source: &FileSource) -> Value {
    match source {
        FileSource::Url { url } => json!({
            "type": "document",
            "source": { "type": "url", "url": url }
        }),
        FileSource::Base64 {
            filename,
            media_type,
            data,
        } => json!({
            "type": "document",
            "source": {
                "type": "base64",
                "media_type": media_type,
                "data": data,
                "filename": filename
            }
        }),
    }
}

/// Claude models with adaptive-thinking support use `thinking: {type: "adaptive"}`
/// + `output_config: {effort}`. Older models require the deprecated
/// `thinking: {type: "enabled", budget_tokens: N}` shape.
///
/// A model supports adaptive thinking iff its identifier contains an
/// `opus-<major>[.-]<minor>` or `sonnet-<major>[.-]<minor>` family segment whose
/// (major, minor) version is >= (4, 6). This covers Opus/Sonnet 4.6, 4.7, 4.8
/// and any 5.x+ release without per-minor-version maintenance.
fn model_supports_adaptive(model: &str) -> bool {
    let m = model.to_lowercase();
    for family in ["opus-", "sonnet-"] {
        let Some(pos) = m.find(family) else { continue };
        let after = &m[pos + family.len()..];
        let major_str: String = after.chars().take_while(|c| c.is_ascii_digit()).collect();
        if major_str.is_empty() {
            continue;
        }
        let Ok(major) = major_str.parse::<u32>() else {
            continue;
        };
        if major >= 5 {
            return true;
        }
        if major < 4 {
            continue;
        }
        // major == 4: require minor >= 6. Accept `-` or `.` as the minor separator.
        let rest = &after[major_str.len()..];
        let rest = rest
            .strip_prefix('-')
            .or_else(|| rest.strip_prefix('.'))
            .unwrap_or(rest);
        let minor_str: String = rest.chars().take_while(|c| c.is_ascii_digit()).collect();
        if let Ok(minor) = minor_str.parse::<u32>() {
            if minor >= 6 {
                return true;
            }
        }
    }
    false
}

fn effort_to_budget(effort: &str) -> u32 {
    // Non-adaptive Anthropic models use a fixed budget table. `xhigh` and `max`
    // share the same budget here; their distinction only surfaces on
    // adaptive-thinking models via `output_config.effort`.
    match effort {
        "minimum" => 1024,
        "low" => 1024,
        "medium" => 4096,
        "high" => 16384,
        "xhigh" | "max" => 32000,
        _ => 4096,
    }
}

fn finish_reason_to_stop_reason(finish_reason: Option<FinishReason>) -> &'static str {
    match finish_reason {
        Some(FinishReason::Length) => "max_tokens",
        Some(FinishReason::ToolCalls) => "tool_use",
        _ => "end_turn",
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::urp::decode::anthropic as decode_anthropic;
    use crate::urp::internal_legacy_bridge::{Item, Part, Role, items_to_nodes, nodes_to_items};
    use crate::urp::{OutputDetails, ResponseFormat, UrpRequest, UrpResponse, Usage};
    use std::collections::HashMap;

    fn empty_map() -> HashMap<String, Value> {
        HashMap::new()
    }

    #[test]
    fn encode_request_does_not_emit_fake_response_format() {
        let req = UrpRequest {
            model: "claude-sonnet-4-5".to_string(),
            input: items_to_nodes(vec![Item::Message {
                id: None,
                role: Role::User,
                parts: vec![Part::Text {
                    content: "hello".to_string(),
                    extra_body: empty_map(),
                }],
                extra_body: empty_map(),
            }]),
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

        let encoded = encode_request(&req, "claude-sonnet-4-5");
        assert!(
            encoded.get("response_format").is_none(),
            "Anthropic requests must omit unsupported response_format"
        );
        assert_eq!(
            encoded["max_tokens"],
            json!(ANTHROPIC_DEFAULT_MAX_TOKENS),
            "Anthropic requests without a downstream cap must default to Anthropic's max output budget"
        );
    }

    #[test]
    fn encode_request_preserves_explicit_max_output_tokens() {
        let req = UrpRequest {
            model: "claude-sonnet-4-5".to_string(),
            input: items_to_nodes(vec![Item::Message {
                id: None,
                role: Role::User,
                parts: vec![Part::Text {
                    content: "hello".to_string(),
                    extra_body: empty_map(),
                }],
                extra_body: empty_map(),
            }]),
            stream: None,
            temperature: None,
            top_p: None,
            max_output_tokens: Some(321),
            reasoning: None,
            tools: None,
            tool_choice: None,
            response_format: None,
            user: None,
            extra_body: empty_map(),
        };

        let encoded = encode_request(&req, "claude-sonnet-4-5");
        assert_eq!(encoded["max_tokens"], json!(321));
    }

    #[test]
    fn anthropic_text_block_phase_round_trips_to_responses_compatible_urp() {
        let source = json!({
            "id": "msg_1",
            "type": "message",
            "role": "assistant",
            "model": "claude",
            "content": [
                { "type": "text", "text": "prep", "phase": "commentary" },
                { "type": "tool_use", "id": "call_1", "name": "tool", "input": {} },
                { "type": "text", "text": "done", "phase": "final_answer" }
            ],
            "stop_reason": "tool_use"
        });

        let decoded = decode_anthropic::decode_response(&source).expect("decode response");
        let encoded = encode_response(&decoded, "claude");
        let content = encoded["content"].as_array().expect("content array");

        assert_eq!(content[0]["phase"], json!("commentary"));
        assert_eq!(content[1]["type"], json!("tool_use"));
        assert_eq!(content[2]["phase"], json!("final_answer"));
    }

    #[test]
    fn anthropic_usage_round_trips_extension_fields_without_leaking_nested_aliases() {
        let mut usage_extra = HashMap::new();
        usage_extra.insert("native_counter".to_string(), json!(7));
        let response = UrpResponse {
            id: "msg_usage".to_string(),
            model: "claude".to_string(),
            created_at: None,
            output: items_to_nodes(vec![Item::new_message(Role::Assistant)]),
            finish_reason: Some(FinishReason::Stop),
            usage: Some(Usage {
                input_tokens: 11,
                output_tokens: 5,
                input_details: Some(crate::urp::InputDetails {
                    standard_tokens: 0,
                    cache_read_tokens: 2,
                    cache_creation_tokens: 3,
                    tool_prompt_tokens: 4,
                    modality_breakdown: None,
                }),
                output_details: Some(OutputDetails {
                    standard_tokens: 0,
                    reasoning_tokens: 6,
                    accepted_prediction_tokens: 7,
                    rejected_prediction_tokens: 8,
                    modality_breakdown: None,
                }),
                extra_body: usage_extra,
            }),
            extra_body: empty_map(),
        };

        let encoded = encode_response(&response, "claude");
        let usage = encoded["usage"].as_object().expect("usage object");
        assert_eq!(usage.get("tool_prompt_input_tokens"), Some(&json!(4)));
        assert_eq!(usage.get("reasoning_output_tokens"), Some(&json!(6)));
        assert_eq!(
            usage.get("accepted_prediction_output_tokens"),
            Some(&json!(7))
        );
        assert_eq!(
            usage.get("rejected_prediction_output_tokens"),
            Some(&json!(8))
        );
        assert_eq!(usage.get("native_counter"), Some(&json!(7)));

        let decoded = decode_anthropic::decode_response(&encoded).expect("decode response");
        let decoded_usage = decoded.usage.expect("usage should decode");
        assert_eq!(
            decoded_usage
                .input_details
                .expect("input details")
                .tool_prompt_tokens,
            4
        );
        let decoded_output = decoded_usage.output_details.expect("output details");
        assert_eq!(decoded_output.reasoning_tokens, 6);
        assert_eq!(decoded_output.accepted_prediction_tokens, 7);
        assert_eq!(decoded_output.rejected_prediction_tokens, 8);
        assert!(
            !decoded_usage
                .extra_body
                .contains_key("tool_prompt_input_tokens")
        );
        assert!(
            !decoded_usage
                .extra_body
                .contains_key("reasoning_output_tokens")
        );
        assert_eq!(
            decoded_usage.extra_body.get("native_counter"),
            Some(&json!(7))
        );
    }

    #[test]
    fn anthropic_response_round_trip_preserves_combined_thinking_block_shape() {
        let response = UrpResponse {
            id: "msg_roundtrip_reasoning".to_string(),
            model: "claude".to_string(),
            created_at: None,
            output: items_to_nodes(vec![Item::Message {
                id: None,
                role: Role::Assistant,
                parts: vec![Part::Reasoning {
                    id: None,
                    content: Some("full reasoning".to_string()),
                    encrypted: Some(json!("sig_1")),
                    summary: None,
                    source: None,
                    extra_body: empty_map(),
                }],
                extra_body: empty_map(),
            }]),
            finish_reason: Some(FinishReason::Stop),
            usage: None,
            extra_body: empty_map(),
        };

        let encoded = encode_response(&response, "claude");
        let decoded = decode_anthropic::decode_response(&encoded).expect("decode response");
        let decoded_outputs = nodes_to_items(&decoded.output);
        let Item::Message { parts, .. } = &decoded_outputs[0] else {
            panic!("expected assistant output");
        };

        assert_eq!(
            parts.len(),
            1,
            "thinking block should decode to one reasoning part"
        );
        assert!(matches!(
            &parts[0],
            Part::Reasoning {
                content: Some(content),
                encrypted: Some(Value::String(sig)),
                summary: None,
                ..
            } if content == "full reasoning" && sig == "sig_1"
        ));
    }

    #[test]
    fn encode_request_strips_orphaned_tool_use_via_shared_pre_encode() {
        use crate::handlers::strip_orphaned_tool_calls;
        use crate::urp::ToolResultContent;

        let mut req = UrpRequest {
            model: "claude-sonnet-4-6".to_string(),
            input: items_to_nodes(vec![
                Item::text(Role::User, "list files"),
                Item::Message {
                    id: None,
                    role: Role::Assistant,
                    parts: vec![
                        Part::ToolCall {
                            id: None,
                            call_id: "answered".to_string(),
                            name: "bash".to_string(),
                            arguments: r#"{"command":"ls"}"#.to_string(),
                            extra_body: empty_map(),
                        },
                        Part::ToolCall {
                            id: None,
                            call_id: "orphan".to_string(),
                            name: "bash".to_string(),
                            arguments: r#"{"command":"cat x"}"#.to_string(),
                            extra_body: empty_map(),
                        },
                    ],
                    extra_body: empty_map(),
                },
                Item::ToolResult {
                    id: None,
                    call_id: "answered".to_string(),
                    is_error: false,
                    content: vec![ToolResultContent::Text {
                        text: "file1.txt".to_string(),
                    }],
                    extra_body: empty_map(),
                },
                Item::text(Role::User, "thanks"),
            ]),
            stream: None,
            temperature: None,
            top_p: None,
            max_output_tokens: Some(256),
            reasoning: None,
            tools: None,
            tool_choice: None,
            response_format: None,
            user: None,
            extra_body: empty_map(),
        };

        strip_orphaned_tool_calls(&mut req);
        let encoded = encode_request(&req, "claude-sonnet-4-6");
        let messages = encoded["messages"].as_array().expect("messages array");

        let assistant_msg = &messages[1];
        let assistant_content = assistant_msg["content"].as_array().expect("content array");
        assert_eq!(
            assistant_content.len(),
            1,
            "orphaned tool_use should be stripped"
        );
        assert_eq!(assistant_content[0]["id"], json!("answered"));
    }

    fn req_with_effort(model: &str, effort: &str) -> UrpRequest {
        UrpRequest {
            model: model.to_string(),
            input: items_to_nodes(vec![Item::Message {
                id: None,
                role: Role::User,
                parts: vec![Part::Text {
                    content: "hi".to_string(),
                    extra_body: empty_map(),
                }],
                extra_body: empty_map(),
            }]),
            stream: None,
            temperature: None,
            top_p: None,
            max_output_tokens: None,
            reasoning: Some(crate::urp::ReasoningConfig {
                effort: Some(effort.to_string()),
                extra_body: HashMap::new(),
            }),
            tools: None,
            tool_choice: None,
            response_format: None,
            user: None,
            extra_body: empty_map(),
        }
    }

    #[test]
    fn adaptive_model_detection_covers_4_6_through_5_and_beyond() {
        for m in [
            "claude-opus-4-6",
            "claude-opus-4.6",
            "claude-sonnet-4-6-20250101",
            "claude-opus-4-7-20260101",
            "claude-opus-4.7",
            "claude-sonnet-4-7",
            "claude-opus-4-8",
            "claude-opus-5-0",
            "claude-sonnet-6-0",
            "opus-4-7",
            "sonnet-4.7",
        ] {
            assert!(
                model_supports_adaptive(m),
                "{m} must be detected as adaptive-thinking model"
            );
        }
        for m in [
            "claude-opus-4-5",
            "claude-opus-4.5",
            "claude-sonnet-4-0",
            "claude-sonnet-3-7",
            "claude-haiku-4-6",
            "claude-3-5-sonnet",
        ] {
            assert!(
                !model_supports_adaptive(m),
                "{m} must NOT be detected as adaptive-thinking model"
            );
        }
    }

    #[test]
    fn adaptive_encoder_passes_xhigh_and_max_through_distinctly() {
        for effort in ["xhigh", "max"] {
            let encoded = encode_request(
                &req_with_effort("claude-opus-4-7", effort),
                "claude-opus-4-7",
            );
            assert_eq!(encoded["thinking"], json!({ "type": "adaptive" }));
            assert_eq!(
                encoded["output_config"]["effort"],
                json!(effort),
                "adaptive path must forward {effort} as-is"
            );
        }
    }

    #[test]
    fn non_adaptive_encoder_uses_32000_for_both_xhigh_and_max() {
        for effort in ["xhigh", "max"] {
            let encoded = encode_request(
                &req_with_effort("claude-sonnet-4-5", effort),
                "claude-sonnet-4-5",
            );
            assert_eq!(
                encoded["thinking"],
                json!({ "type": "enabled", "budget_tokens": 32000 }),
                "non-adaptive {effort} must emit budget_tokens=32000"
            );
            assert!(
                encoded.get("output_config").is_none(),
                "non-adaptive path must not emit output_config"
            );
        }
    }

    #[test]
    fn non_adaptive_encoder_budget_table_is_stable() {
        for (effort, expected) in [
            ("minimum", 1024),
            ("low", 1024),
            ("medium", 4096),
            ("high", 16384),
            ("xhigh", 32000),
            ("max", 32000),
        ] {
            assert_eq!(
                effort_to_budget(effort),
                expected,
                "effort_to_budget({effort}) regressed"
            );
        }
    }
}
