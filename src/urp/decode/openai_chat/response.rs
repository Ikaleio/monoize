use super::message::{
    MESSAGE_BODY_FIELDS, attach_annotations, legacy_function_call_node, message_audio_node,
    push_content_nodes, push_message, refusal_node, tool_call_nodes,
};
use super::reasoning::reasoning_nodes;
use super::usage::parse_chat_usage;
use crate::urp::decode::split_extra;
use crate::urp::{
    CHAT_CHOICE_EXTRA_BODY_KEY, CHAT_NATIVE_FINISH_REASON_EXTRA_KEY, FinishReason, Node,
    OrdinaryRole, UrpResponse,
};
use serde_json::{Map, Value};

pub fn decode_response(value: &Value) -> Result<UrpResponse, String> {
    let obj = value
        .as_object()
        .ok_or_else(|| "chat response must be object".to_string())?;

    if let Some(error) = obj.get("error").filter(|error| !error.is_null()) {
        return Err(format_chat_completion_error(error));
    }

    let choice = obj
        .get("choices")
        .and_then(Value::as_array)
        .and_then(|choices| choices.first())
        .and_then(Value::as_object)
        .ok_or_else(|| "missing choices[0]".to_string())?;

    if let Some(error) = choice.get("error").filter(|error| !error.is_null()) {
        return Err(format_chat_completion_error(error));
    }

    let native_finish_reason = choice
        .get("finish_reason")
        .and_then(Value::as_str)
        .filter(|reason| !reason.is_empty());
    if native_finish_reason == Some("error") {
        return Err("upstream chat completion terminated with finish_reason=error".to_string());
    }

    let msg_obj = choice
        .get("message")
        .and_then(Value::as_object)
        .ok_or_else(|| "missing choices[0].message".to_string())?;

    let output = decode_assistant_message(msg_obj, choice.get("logprobs"))?;
    let finish_reason = native_finish_reason
        .map(parse_finish_reason)
        .map(|reason| normalize_tool_call_finish(reason, &output));

    let mut extra_body = split_extra(
        obj,
        &[
            "id", "object", "created", "model", "choices", "usage", "provider",
        ],
    );
    let choice_extra = choice
        .iter()
        .filter(|(key, _)| {
            !matches!(
                key.as_str(),
                "index" | "message" | "finish_reason" | "logprobs"
            )
        })
        .map(|(key, value)| (key.clone(), value.clone()))
        .collect::<Map<String, Value>>();
    if !choice_extra.is_empty() {
        extra_body.insert(
            CHAT_CHOICE_EXTRA_BODY_KEY.to_string(),
            Value::Object(choice_extra),
        );
    }
    if let Some(native_finish_reason) =
        native_finish_reason.filter(|reason| parse_finish_reason(reason) == FinishReason::Other)
    {
        extra_body.insert(
            CHAT_NATIVE_FINISH_REASON_EXTRA_KEY.to_string(),
            Value::String(native_finish_reason.to_string()),
        );
    }

    Ok(UrpResponse {
        outcome: None,
        id: obj
            .get("id")
            .and_then(Value::as_str)
            .unwrap_or("chat_completion")
            .to_string(),
        model: obj
            .get("model")
            .and_then(Value::as_str)
            .unwrap_or_default()
            .to_string(),
        created_at: obj.get("created").and_then(Value::as_i64),
        output,
        finish_reason,
        usage: obj
            .get("usage")
            .and_then(Value::as_object)
            .map(parse_chat_usage),
        extra_body,
    })
}

fn decode_assistant_message(
    msg_obj: &Map<String, Value>,
    logprobs: Option<&Value>,
) -> Result<Vec<Node>, String> {
    let role = OrdinaryRole::Assistant;
    let mut nodes = reasoning_nodes(msg_obj);
    nodes.extend(message_audio_node(msg_obj, role));
    if let Some(content) = msg_obj.get("content") {
        let phase = msg_obj.get("phase").and_then(Value::as_str);
        push_content_nodes(&mut nodes, content, role, phase)?;
    }
    nodes.extend(tool_call_nodes(msg_obj));
    nodes.extend(legacy_function_call_node(msg_obj));
    nodes.extend(refusal_node(msg_obj));
    attach_annotations(&mut nodes, msg_obj);

    if let Some(scores) = logprobs {
        for node in &mut nodes {
            match node {
                Node::Text { logprobs, .. } => {
                    *logprobs = crate::urp::logprobs::decode(scores.get("content"))
                }
                Node::Refusal { logprobs, .. } => {
                    *logprobs = crate::urp::logprobs::decode(scores.get("refusal"))
                }
                _ => {}
            }
        }
    }

    let mut output = Vec::new();
    push_message(
        &mut output,
        nodes,
        split_extra(msg_obj, MESSAGE_BODY_FIELDS),
    );
    Ok(output)
}

fn format_chat_completion_error(error: &Value) -> String {
    let message = error
        .get("message")
        .and_then(Value::as_str)
        .or_else(|| error.as_str())
        .filter(|message| !message.is_empty())
        .unwrap_or("upstream chat completion error");
    let code = error.get("code").and_then(|code| match code {
        Value::String(code) if !code.is_empty() => Some(code.clone()),
        Value::Number(code) => Some(code.to_string()),
        _ => None,
    });
    match code {
        Some(code) => format!("{message} (code: {code})"),
        None => message.to_string(),
    }
}

fn parse_finish_reason(reason: &str) -> FinishReason {
    match reason {
        "stop" => FinishReason::Stop,
        "length" => FinishReason::Length,
        "tool_calls" | "function_call" => FinishReason::ToolCalls,
        "content_filter" => FinishReason::ContentFilter,
        _ => FinishReason::Other,
    }
}

/// PC7b: an upstream `stop` that terminates decoded tool calls is a tool-call turn.
pub(crate) fn normalize_tool_call_finish(reason: FinishReason, output: &[Node]) -> FinishReason {
    if reason == FinishReason::Stop
        && output
            .iter()
            .any(|node| matches!(node, Node::ToolCall { .. }))
    {
        FinishReason::ToolCalls
    } else {
        reason
    }
}
