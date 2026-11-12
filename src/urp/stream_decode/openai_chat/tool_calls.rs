//! Chat tool calls: `delta.tool_calls[]` fragments, tool-call content blocks, the deprecated
//! `function_call`, and their terminal `message` snapshots (PC7, PC7f).

use super::reasoning::snapshot_suffix;
use super::state::ChatStreamState;
use crate::error::AppResult;
use crate::urp::decode::parse_tool_call_arguments_value;
use crate::urp::{
    CHAT_LEGACY_FUNCTION_CALL_EXTRA_KEY, Node, NodeDelta, NodeHeader, ToolCallType,
    is_internal_key, synthetic_tool_call_id,
};
use serde_json::{Map, Value};
use std::collections::HashMap;

struct ToolCallState {
    node_index: u32,
    call_id: String,
    tool_type: ToolCallType,
    name: String,
    arguments: String,
    /// Decoded from the deprecated `function_call` member.
    legacy: bool,
}

/// Tool calls in first-seen order. Their nodes stay open until the terminal frame, because
/// Chat upstreams can interleave the arguments of parallel calls (PC7h).
#[derive(Default)]
pub(super) struct ToolCalls {
    calls: Vec<ToolCallState>,
    /// Native `index` to position in `calls`.
    by_wire_index: HashMap<u64, usize>,
}

/// The call a fragment belongs to.
#[derive(Clone, Copy)]
enum CallRef<'f> {
    Known(usize),
    New(&'f str),
}

impl ToolCalls {
    fn by_call_id<'f>(&self, call_id: &'f str) -> CallRef<'f> {
        self.calls
            .iter()
            .position(|call| call.call_id == call_id)
            .map_or(CallRef::New(call_id), CallRef::Known)
    }

    fn by_index(&self, fragment: &Map<String, Value>) -> Option<usize> {
        let index = fragment.get("index").and_then(Value::as_u64)?;
        self.by_wire_index.get(&index).copied()
    }

    /// A delta fragment's call: its own id, else the call at its native `index`, else the latest
    /// call for the first fragment of a list, else the call at the same list position.
    fn resolve_delta<'f>(
        &self,
        fragment: &'f Map<String, Value>,
        list_position: usize,
    ) -> Option<CallRef<'f>> {
        if let Some(call_id) = fragment_call_id(fragment) {
            return Some(self.by_call_id(call_id));
        }
        self.by_index(fragment)
            .or_else(|| {
                if list_position == 0 {
                    self.calls.len().checked_sub(1)
                } else {
                    (list_position < self.calls.len()).then_some(list_position)
                }
            })
            .map(CallRef::Known)
    }

    /// A terminal snapshot call's streamed call: its own id, else the call at its native
    /// `index`, else the call at the same list position.
    fn resolve_terminal<'f>(
        &self,
        snapshot: &'f Map<String, Value>,
        list_position: usize,
    ) -> Option<CallRef<'f>> {
        if let Some(call_id) = fragment_call_id(snapshot) {
            return Some(self.by_call_id(call_id));
        }
        self.by_index(snapshot)
            .or_else(|| (list_position < self.calls.len()).then_some(list_position))
            .map(CallRef::Known)
    }

    pub(super) fn into_nodes(self) -> impl Iterator<Item = (u32, Node)> {
        self.calls.into_iter().map(|call| {
            let extra_body = if call.legacy {
                HashMap::from([(
                    CHAT_LEGACY_FUNCTION_CALL_EXTRA_KEY.to_string(),
                    Value::Bool(true),
                )])
            } else {
                HashMap::new()
            };
            let node = Node::ToolCall {
                namespace: None,
                signature: None,
                id: Some(synthetic_tool_call_id()),
                tool_type: call.tool_type,
                call_id: call.call_id,
                name: call.name,
                arguments: call.arguments,
                cache_breakpoint: None,
                extra_body,
            };
            (call.node_index, node)
        })
    }
}

impl ChatStreamState<'_> {
    pub(super) async fn on_tool_call_delta(
        &mut self,
        raw: &Value,
        list_position: usize,
        legacy: bool,
    ) -> AppResult<()> {
        let Some(fragment) = raw.as_object() else {
            return Ok(());
        };
        let Some(call) = self.tool_calls.resolve_delta(fragment, list_position) else {
            return Ok(());
        };
        let arguments = arguments_text(fragment).unwrap_or_default();
        self.apply_tool_call(call, fragment, legacy, &arguments)
            .await
    }

    /// PC7f: a snapshot call completes its streamed call with the argument suffix the stream
    /// lacked; a call the stream never announced streams in full.
    pub(super) async fn on_terminal_tool_call(
        &mut self,
        raw: &Value,
        list_position: usize,
        legacy: bool,
    ) -> AppResult<()> {
        let Some(snapshot) = raw.as_object() else {
            return Ok(());
        };
        let Some(call) = self.tool_calls.resolve_terminal(snapshot, list_position) else {
            return Ok(());
        };
        let arguments = arguments_text(snapshot).unwrap_or_default();
        let arguments = match call {
            CallRef::Known(position) => {
                snapshot_suffix(&self.tool_calls.calls[position].arguments, &arguments)
                    .unwrap_or_default()
                    .to_string()
            }
            CallRef::New(_) => arguments,
        };
        self.apply_tool_call(call, snapshot, legacy, &arguments)
            .await
    }

    /// Merges a fragment into its call, announcing a new call first, and streams `arguments`.
    async fn apply_tool_call(
        &mut self,
        call: CallRef<'_>,
        fragment: &Map<String, Value>,
        legacy: bool,
        arguments: &str,
    ) -> AppResult<()> {
        let tool_type = tool_call_type(fragment);
        let name = fragment_name(fragment).unwrap_or_default();
        let position = match call {
            CallRef::Known(position) => position,
            CallRef::New(call_id) => {
                self.start_tool_call(call_id, tool_type, name, legacy)
                    .await?
            }
        };
        if let Some(index) = fragment.get("index").and_then(Value::as_u64) {
            self.tool_calls.by_wire_index.insert(index, position);
        }
        let state = &mut self.tool_calls.calls[position];
        if tool_type == ToolCallType::Custom {
            state.tool_type = ToolCallType::Custom;
        }
        state.legacy |= legacy;
        if state.name.is_empty() && !name.is_empty() {
            state.name = name.to_string();
        }
        if arguments.is_empty() {
            return Ok(());
        }
        state.arguments.push_str(arguments);
        let node_index = state.node_index;
        let extra_body = self.take_delta_extra();
        self.send_delta(
            node_index,
            NodeDelta::ToolCallArguments {
                arguments: arguments.to_string(),
            },
            extra_body,
        )
        .await
    }

    async fn start_tool_call(
        &mut self,
        call_id: &str,
        tool_type: ToolCallType,
        name: &str,
        legacy: bool,
    ) -> AppResult<usize> {
        let mut extra_body = self.take_delta_extra();
        if legacy {
            extra_body.insert(
                CHAT_LEGACY_FUNCTION_CALL_EXTRA_KEY.to_string(),
                Value::Bool(true),
            );
        }
        let header = NodeHeader::ToolCall {
            namespace: None,
            signature: None,
            id: None,
            tool_type,
            call_id: call_id.to_string(),
            name: name.to_string(),
        };
        let node_index = self.start_node(header, extra_body).await?;
        self.tool_calls.calls.push(ToolCallState {
            node_index,
            call_id: call_id.to_string(),
            tool_type,
            name: name.to_string(),
            arguments: String::new(),
            legacy,
        });
        Ok(self.tool_calls.calls.len() - 1)
    }
}

/// The deprecated `function_call` member as a `tool_calls[]` entry with `index = 0` and the
/// legacy call id of its name.
pub(super) fn legacy_function_call_as_tool_call(function_call: &Value) -> Option<Value> {
    let mut function = function_call.as_object()?.clone();
    function.retain(|key, _| !is_internal_key(key));
    if function.is_empty() {
        return None;
    }
    let name = function
        .get("name")
        .and_then(Value::as_str)
        .filter(|name| !name.is_empty())
        .map(str::to_string);
    let mut normalized = Map::from_iter([
        ("index".to_string(), Value::from(0)),
        ("type".to_string(), Value::String("function".to_string())),
        ("function".to_string(), Value::Object(function)),
    ]);
    if let Some(name) = name {
        normalized.insert(
            "id".to_string(),
            Value::String(format!("legacy_function:{name}")),
        );
    }
    Some(Value::Object(normalized))
}

fn fragment_call_id(fragment: &Map<String, Value>) -> Option<&str> {
    fragment
        .get("id")
        .or_else(|| fragment.get("call_id"))
        .and_then(Value::as_str)
        .filter(|call_id| !call_id.is_empty())
}

fn fragment_name(fragment: &Map<String, Value>) -> Option<&str> {
    fragment
        .get("name")
        .and_then(Value::as_str)
        .or_else(|| {
            fragment
                .get("function")
                .and_then(|function| function.get("name"))
                .and_then(Value::as_str)
        })
        .or_else(|| {
            fragment
                .get("custom")
                .and_then(|custom| custom.get("name"))
                .and_then(Value::as_str)
        })
}

/// The argument text of a fragment. An empty `tool_use` input object carries no arguments.
fn arguments_text(fragment: &Map<String, Value>) -> Option<String> {
    let value = parse_tool_call_arguments_value(fragment)?;
    if fragment.get("type").and_then(Value::as_str) == Some("tool_use")
        && value.as_object().is_some_and(Map::is_empty)
    {
        return None;
    }
    Some(match value {
        Value::String(text) => text,
        value => value.to_string(),
    })
    .filter(|text| !text.is_empty())
}

fn tool_call_type(fragment: &Map<String, Value>) -> ToolCallType {
    if matches!(
        fragment.get("type").and_then(Value::as_str),
        Some("custom" | "custom_tool_call")
    ) || fragment.contains_key("custom")
    {
        ToolCallType::Custom
    } else {
        ToolCallType::Function
    }
}
