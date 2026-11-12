//! PR6c field merge of one reconstructed output item with its terminal snapshot. The callers
//! pair the item by upstream identity; this module only merges the paired values.

use crate::urp::{Node, ToolCallType};
use serde_json::Value;
use std::collections::HashMap;

fn is_control(node: &Node) -> bool {
    matches!(node, Node::NextDownstreamEnvelopeExtra { .. })
}

/// Merges the nodes of one item. Typed nodes pair by position; the terminal control members
/// win over the accumulated ones (STR3q).
pub(super) fn merge_node_lists(
    accumulated: &[Node],
    terminal: &[Node],
) -> Result<Vec<Node>, String> {
    let accumulated_typed: Vec<&Node> = accumulated
        .iter()
        .filter(|node| !is_control(node))
        .collect();
    let terminal_typed: Vec<&Node> = terminal.iter().filter(|node| !is_control(node)).collect();
    if accumulated_typed.is_empty() {
        return Ok(terminal.to_vec());
    }
    if terminal_typed.is_empty() {
        return Ok(accumulated.to_vec());
    }
    if accumulated_typed.len() != terminal_typed.len() {
        return Err("completed output item has a different typed node count".to_string());
    }
    let mut merged: Vec<Node> = accumulated
        .iter()
        .filter(|node| is_control(node))
        .cloned()
        .collect();
    let terminal_control = terminal.iter().find_map(|node| match node {
        Node::NextDownstreamEnvelopeExtra { extra_body } => Some(extra_body),
        _ => None,
    });
    if let Some(terminal_extra) = terminal_control {
        match merged.first_mut() {
            Some(Node::NextDownstreamEnvelopeExtra { extra_body }) => extra_body.extend(
                terminal_extra
                    .iter()
                    .map(|(key, value)| (key.clone(), value.clone())),
            ),
            _ => merged.push(Node::NextDownstreamEnvelopeExtra {
                extra_body: terminal_extra.clone(),
            }),
        }
    }
    for (left, right) in accumulated_typed.into_iter().zip(terminal_typed) {
        merged.push(merge_nodes(left, right)?);
    }
    Ok(merged)
}

/// Merges one accumulated node with its terminal counterpart. A value missing or empty on one
/// side fills from the other; differing non-empty typed values conflict except under the PR6c
/// rules for arguments, boundary whitespace, summaries, and encrypted content.
pub(super) fn merge_nodes(accumulated: &Node, terminal: &Node) -> Result<Node, String> {
    match (accumulated, terminal) {
        (Node::Reasoning { .. }, Node::Reasoning { .. }) => merge_reasoning(accumulated, terminal),
        (Node::Text { .. }, Node::Text { .. }) => merge_text(accumulated, terminal),
        (Node::ToolCall { .. }, Node::ToolCall { .. }) => merge_tool_call(accumulated, terminal),
        (
            Node::Refusal {
                id: left_id,
                content: left,
                ..
            },
            Node::Refusal {
                logprobs,
                id,
                content: right,
                extra_body,
            },
        ) => Ok(Node::Refusal {
            logprobs: logprobs.clone(),
            id: id.clone().or_else(|| left_id.clone()),
            content: merge_string_field("refusal", left, right)?,
            extra_body: extra_body.clone(),
        }),
        // The item pairing is the identity; the terminal snapshot is the node's final state.
        (left, right) if std::mem::discriminant(left) == std::mem::discriminant(right) => {
            Ok(right.clone())
        }
        _ => Err("completed output item type differs from accumulated stream state".to_string()),
    }
}

fn merge_reasoning(accumulated: &Node, terminal: &Node) -> Result<Node, String> {
    let (
        Node::Reasoning {
            metadata: left_metadata,
            id: left_id,
            content: left_content,
            encrypted: left_encrypted,
            summary: left_summary,
            source: left_source,
            extra_body: left_extra,
        },
        Node::Reasoning {
            metadata: right_metadata,
            id: right_id,
            content: right_content,
            encrypted: right_encrypted,
            summary: right_summary,
            source: right_source,
            extra_body: right_extra,
        },
    ) = (accumulated, terminal)
    else {
        unreachable!("merge_reasoning takes two reasoning nodes");
    };
    let mut metadata = left_metadata.clone();
    metadata.merge(right_metadata);
    Ok(Node::Reasoning {
        metadata,
        id: right_id.clone().or_else(|| left_id.clone()),
        content: merge_optional_string_field("reasoning.text", left_content, right_content)?,
        // The terminal summary and encrypted snapshot are authoritative when non-empty.
        encrypted: non_empty_value(right_encrypted).or_else(|| non_empty_value(left_encrypted)),
        summary: non_empty_string(right_summary).or_else(|| non_empty_string(left_summary)),
        source: right_source.clone().or_else(|| left_source.clone()),
        extra_body: merge_extra_body(left_extra, right_extra),
    })
}

fn merge_text(accumulated: &Node, terminal: &Node) -> Result<Node, String> {
    let (
        Node::Text {
            logprobs: left_logprobs,
            signature: left_signature,
            citations: left_citations,
            id: left_id,
            role: left_role,
            content: left_content,
            phase: left_phase,
            extra_body: left_extra,
            ..
        },
        Node::Text {
            logprobs: right_logprobs,
            signature: right_signature,
            citations: right_citations,
            id: right_id,
            role: right_role,
            content: right_content,
            phase: right_phase,
            extra_body: right_extra,
            ..
        },
    ) = (accumulated, terminal)
    else {
        unreachable!("merge_text takes two text nodes");
    };
    if left_role != right_role {
        return Err("message role differs from completed output".to_string());
    }
    // Boundary whitespace may differ; the terminal text is kept exactly.
    let content = if !left_content.is_empty()
        && !right_content.is_empty()
        && left_content.trim() == right_content.trim()
    {
        right_content.clone()
    } else {
        merge_string_field("message.text", left_content, right_content)?
    };
    Ok(Node::Text {
        logprobs: right_logprobs.clone().or_else(|| left_logprobs.clone()),
        signature: right_signature.clone().or_else(|| left_signature.clone()),
        citations: if right_citations.is_empty() {
            left_citations.clone()
        } else {
            right_citations.clone()
        },
        id: right_id.clone().or_else(|| left_id.clone()),
        role: *right_role,
        content,
        phase: merge_optional_string_field("message.phase", left_phase, right_phase)?,
        cache_breakpoint: None,
        extra_body: merge_extra_body(left_extra, right_extra),
    })
}

fn merge_tool_call(accumulated: &Node, terminal: &Node) -> Result<Node, String> {
    let (
        Node::ToolCall {
            namespace: left_namespace,
            signature: left_signature,
            id: left_id,
            tool_type: left_tool_type,
            call_id: left_call_id,
            name: left_name,
            arguments: left_arguments,
            extra_body: left_extra,
            ..
        },
        Node::ToolCall {
            namespace: right_namespace,
            signature: right_signature,
            id: right_id,
            tool_type: right_tool_type,
            call_id: right_call_id,
            name: right_name,
            arguments: right_arguments,
            extra_body: right_extra,
            ..
        },
    ) = (accumulated, terminal)
    else {
        unreachable!("merge_tool_call takes two tool-call nodes");
    };
    let custom =
        *left_tool_type == ToolCallType::Custom || *right_tool_type == ToolCallType::Custom;
    Ok(Node::ToolCall {
        namespace: right_namespace.clone().or_else(|| left_namespace.clone()),
        signature: right_signature.clone().or_else(|| left_signature.clone()),
        id: right_id.clone().or_else(|| left_id.clone()),
        tool_type: if custom {
            ToolCallType::Custom
        } else {
            ToolCallType::Function
        },
        call_id: merge_string_field("function_call.call_id", left_call_id, right_call_id)?,
        name: merge_string_field("function_call.name", left_name, right_name)?,
        arguments: merge_arguments(custom, left_arguments, right_arguments)?,
        extra_body: merge_extra_body(left_extra, right_extra),
        cache_breakpoint: None,
    })
}

/// Function arguments accept the terminal string when both parse to equal JSON, or when the
/// accumulated string is the projected `input` (or a `Begin Patch` `patch`/`command`/`content`)
/// member of the terminal object. Custom input compares exactly.
fn merge_arguments(custom: bool, accumulated: &str, terminal: &str) -> Result<String, String> {
    if !custom
        && accumulated != terminal
        && (json_equal(accumulated, terminal) || is_projected_member(accumulated, terminal))
    {
        return Ok(terminal.to_string());
    }
    merge_string_field("function_call.arguments", accumulated, terminal)
}

fn json_equal(left: &str, right: &str) -> bool {
    matches!(
        (serde_json::from_str::<Value>(left), serde_json::from_str::<Value>(right)),
        (Ok(left), Ok(right)) if left == right
    )
}

fn is_projected_member(accumulated: &str, terminal: &str) -> bool {
    if accumulated.is_empty() {
        return false;
    }
    let Ok(Value::Object(members)) = serde_json::from_str::<Value>(terminal) else {
        return false;
    };
    if members.get("input").and_then(Value::as_str) == Some(accumulated) {
        return true;
    }
    ["patch", "command", "content"].iter().any(|key| {
        members
            .get(*key)
            .and_then(Value::as_str)
            .is_some_and(|value| value.contains("Begin Patch") && value == accumulated)
    })
}

fn merge_string_field(field: &str, left: &str, right: &str) -> Result<String, String> {
    if left.is_empty() || left == right {
        Ok(right.to_string())
    } else if right.is_empty() {
        Ok(left.to_string())
    } else {
        Err(format!("{field} differs from completed output"))
    }
}

fn merge_optional_string_field(
    field: &str,
    left: &Option<String>,
    right: &Option<String>,
) -> Result<Option<String>, String> {
    match (left.as_deref(), right.as_deref()) {
        (Some(left), Some(right)) => merge_string_field(field, left, right).map(Some),
        (Some(left), None) if !left.is_empty() => Ok(Some(left.to_string())),
        (_, Some(right)) if !right.is_empty() => Ok(Some(right.to_string())),
        _ => Ok(None),
    }
}

fn non_empty_string(value: &Option<String>) -> Option<String> {
    value.clone().filter(|value| !value.is_empty())
}

fn non_empty_value(value: &Option<Value>) -> Option<Value> {
    value.clone().filter(|value| match value {
        Value::Null => false,
        Value::String(value) => !value.is_empty(),
        Value::Array(values) => !values.is_empty(),
        Value::Object(values) => !values.is_empty(),
        _ => true,
    })
}

fn merge_extra_body(
    left: &HashMap<String, Value>,
    right: &HashMap<String, Value>,
) -> HashMap<String, Value> {
    let mut merged = left.clone();
    merged.extend(
        right
            .iter()
            .map(|(key, value)| (key.clone(), value.clone())),
    );
    merged
}
