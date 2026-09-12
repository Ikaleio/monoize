use crate::urp::{Node, ToolCallType, UrpStreamEvent};
use serde_json::{Number, Value};
use std::borrow::Cow;

/// Rewrite JSON numbers that are integer-valued floats into JSON integers.
///
/// Some upstreams (notably Grok/xAI structured tool calls) emit `4338.0` for
/// whole numbers. Clients such as Codex deserialize those fields as `i32`/`u64`
/// and reject the float form. Numbers that are already integers, non-finite,
/// not integer-valued, or outside `i64` stay unchanged.
pub fn integerize_json_floats(value: &mut Value) -> bool {
    match value {
        Value::Number(number) => {
            if number.as_i64().is_some() || number.as_u64().is_some() {
                return false;
            }
            let Some(float) = number.as_f64() else {
                return false;
            };
            if !float.is_finite() {
                return false;
            }
            if float < i64::MIN as f64 || float > i64::MAX as f64 {
                return false;
            }
            let integer = float as i64;
            if integer as f64 != float {
                return false;
            }
            *number = Number::from(integer);
            true
        }
        Value::Array(values) => {
            let mut changed = false;
            for value in values {
                changed |= integerize_json_floats(value);
            }
            changed
        }
        Value::Object(map) => {
            let mut changed = false;
            for value in map.values_mut() {
                changed |= integerize_json_floats(value);
            }
            changed
        }
        Value::Null | Value::Bool(_) | Value::String(_) => false,
    }
}

/// Parse tool-call `arguments` JSON and rewrite integer-valued floats.
/// Non-JSON strings are returned unchanged.
pub fn integerize_tool_call_arguments_json(raw: &str) -> Cow<'_, str> {
    let Ok(mut value) = serde_json::from_str::<Value>(raw) else {
        return Cow::Borrowed(raw);
    };
    if !integerize_json_floats(&mut value) {
        return Cow::Borrowed(raw);
    }
    match serde_json::to_string(&value) {
        Ok(encoded) => Cow::Owned(encoded),
        Err(_) => Cow::Borrowed(raw),
    }
}

/// Owned wire form of tool-call arguments after integer-valued float rewrite.
pub fn tool_call_arguments_for_wire(arguments: impl AsRef<str>) -> String {
    integerize_tool_call_arguments_json(arguments.as_ref()).into_owned()
}

pub fn integerize_tool_call_node(node: &mut Node) {
    if let Node::ToolCall {
        tool_type: ToolCallType::Function,
        arguments,
        ..
    } = node
        && let Cow::Owned(next) = integerize_tool_call_arguments_json(arguments)
    {
        *arguments = next;
    }
}

pub fn integerize_tool_call_nodes(nodes: &mut [Node]) {
    for node in nodes {
        integerize_tool_call_node(node);
    }
}

pub fn integerize_tool_call_stream_event(event: &mut UrpStreamEvent) {
    match event {
        UrpStreamEvent::NodeDone { node, .. } => integerize_tool_call_node(node),
        UrpStreamEvent::ResponseDone { output, .. } => integerize_tool_call_nodes(output),
        _ => {}
    }
}
