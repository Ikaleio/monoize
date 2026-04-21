use crate::error::{AppError, AppResult};
use crate::handlers::usage::{
    latest_stream_usage_snapshot, mark_stream_ttfb_if_needed, parse_usage_from_gemini_object,
    record_stream_done_sentinel, record_stream_terminal_event, record_stream_usage_if_present,
};
use crate::handlers::{StreamRuntimeMetrics, UrpRequest as HandlerUrpRequest};
use crate::urp::{FinishReason, Node, NodeDelta, NodeHeader, OrdinaryRole, UrpStreamEvent};
use axum::http::StatusCode;
use eventsource_stream::Eventsource;
use futures_util::StreamExt;
use serde_json::Value;
use std::collections::HashMap;
use std::sync::Arc;
use tokio::sync::{Mutex, mpsc};

#[derive(Debug, Default)]
struct GeminiStreamState {
    node_order: Vec<u32>,
    active_nodes: HashMap<u32, ActiveNodeState>,
    completed_nodes: HashMap<u32, Node>,
}

#[derive(Debug, Clone)]
struct ActiveNodeState {
    kind: ActiveNodeKind,
    extra_body: HashMap<String, Value>,
}

#[derive(Debug, Clone)]
enum ActiveNodeKind {
    Text {
        content: String,
    },
    Reasoning {
        content: String,
        encrypted: String,
    },
    ToolCall {
        call_id: String,
        name: String,
        arguments: String,
    },
}

pub(crate) async fn stream_gemini_to_urp_events(
    urp: &HandlerUrpRequest,
    upstream_resp: reqwest::Response,
    tx: mpsc::Sender<UrpStreamEvent>,
    started_at: Option<std::time::Instant>,
    runtime_metrics: Option<Arc<Mutex<StreamRuntimeMetrics>>>,
) -> AppResult<()> {
    let response_id = format!("resp_{}", uuid::Uuid::new_v4());
    let mut started_response = false;
    let mut finish_reason: Option<FinishReason> = None;
    let mut state = GeminiStreamState::default();

    let idle_timeout = std::time::Duration::from_secs(120);
    let mut stream = upstream_resp.bytes_stream().eventsource();
    while let Some(ev) = tokio::time::timeout(idle_timeout, stream.next())
        .await
        .map_err(|_| {
            AppError::new(
                StatusCode::GATEWAY_TIMEOUT,
                "upstream_idle_timeout",
                "upstream stream idle for 120s without data",
            )
        })?
    {
        let ev = ev.map_err(|err| {
            AppError::new(
                StatusCode::BAD_GATEWAY,
                "upstream_stream_decode_failed",
                err.to_string(),
            )
        })?;
        if tx.is_closed() {
            break;
        }

        mark_stream_ttfb_if_needed(started_at, &runtime_metrics).await;

        if ev.data.trim() == "[DONE]" {
            record_stream_done_sentinel(&runtime_metrics).await;
            break;
        }

        let data_val: Value = serde_json::from_str(&ev.data).unwrap_or(Value::Null);
        record_stream_usage_if_present(&runtime_metrics, parse_usage_from_gemini_object(&data_val))
            .await;

        let Some(candidate) = data_val
            .get("candidates")
            .and_then(|v| v.as_array())
            .and_then(|arr| arr.first())
        else {
            continue;
        };

        let Some(parts) = candidate
            .get("content")
            .and_then(|v| v.get("parts"))
            .and_then(|v| v.as_array())
        else {
            continue;
        };

        if !started_response {
            let _ = tx
                .send(UrpStreamEvent::ResponseStart {
                    id: response_id.clone(),
                    model: urp.model.clone(),
                    extra_body: HashMap::new(),
                })
                .await;
            started_response = true;
        }

        let current_parts = parse_candidate_parts(parts);
        for candidate_part in current_parts {
            let node_index = candidate_part.node_index;
            if !state.node_order.contains(&node_index) {
                state.node_order.push(node_index);
            }

            let next_active = candidate_part.active_node;
            let next_node = node_from_active(&next_active);
            let next_extra = next_active.extra_body.clone();

            if let Some(existing_active) = state.active_nodes.get_mut(&node_index) {
                let deltas = update_active_node(existing_active, &next_active);
                for delta in deltas {
                    let _ = tx
                        .send(UrpStreamEvent::NodeDelta {
                            node_index,
                            delta,
                            usage: None,
                            extra_body: next_extra.clone(),
                        })
                        .await;
                }
            } else {
                state.active_nodes.insert(node_index, next_active.clone());
                let mut events = vec![UrpStreamEvent::NodeStart {
                    node_index,
                    header: node_header_from_node(&next_node),
                    extra_body: next_extra.clone(),
                }];
                for delta in initial_deltas_for_active_node(&next_active) {
                    events.push(UrpStreamEvent::NodeDelta {
                        node_index,
                        delta,
                        usage: None,
                        extra_body: next_extra.clone(),
                    });
                }
                for event in events {
                    let _ = tx.send(event).await;
                }
            }
        }

        if let Some(reason) = candidate.get("finishReason").and_then(|v| v.as_str()) {
            finish_reason = Some(parse_finish_reason(reason));
            break;
        }
    }

    let active_indices: Vec<u32> = state.node_order.clone();
    for node_index in active_indices {
        let Some(active_node) = state.active_nodes.remove(&node_index) else {
            continue;
        };
        let node = node_from_active(&active_node);
        let extra_body = active_node.extra_body.clone();
        state.completed_nodes.insert(node_index, node.clone());

        let _ = tx
            .send(UrpStreamEvent::NodeDone {
                node_index,
                node,
                usage: None,
                extra_body,
            })
            .await;
    }

    let output_nodes = ordered_completed_nodes(&state);
    let usage = latest_stream_usage_snapshot(&runtime_metrics).await;

    if started_response {
        let _ = tx
            .send(UrpStreamEvent::ResponseDone {
                finish_reason,
                usage,
                output: output_nodes,
                extra_body: HashMap::new(),
            })
            .await;
    }

    record_stream_terminal_event(&runtime_metrics, "response.completed", None).await;
    Ok(())
}

#[derive(Debug, Clone)]
struct CandidatePartState {
    node_index: u32,
    active_node: ActiveNodeState,
}

fn parse_candidate_parts(parts: &[Value]) -> Vec<CandidatePartState> {
    let mut parsed = Vec::new();
    let mut next_tool_node_index: u32 = 2;

    for part in parts {
        if let Some(text) = part.get("text").and_then(|v| v.as_str()) {
            if part.get("thought").and_then(|v| v.as_bool()) == Some(true) {
                let signature = part
                    .get("thoughtSignature")
                    .map(|sig| {
                        sig.as_str()
                            .map(|s| s.to_string())
                            .unwrap_or_else(|| sig.to_string())
                    })
                    .unwrap_or_default();
                parsed.push(CandidatePartState {
                    node_index: 1,
                    active_node: ActiveNodeState {
                        kind: ActiveNodeKind::Reasoning {
                            content: text.to_string(),
                            encrypted: signature,
                        },
                        extra_body: HashMap::new(),
                    },
                });
            } else {
                parsed.push(CandidatePartState {
                    node_index: 0,
                    active_node: ActiveNodeState {
                        kind: ActiveNodeKind::Text {
                            content: text.to_string(),
                        },
                        extra_body: HashMap::new(),
                    },
                });
            }
        }

        if let Some(fc) = part.get("functionCall").and_then(|v| v.as_object()) {
            let call_id = fc
                .get("id")
                .or_else(|| fc.get("name"))
                .and_then(|v| v.as_str())
                .unwrap_or("")
                .to_string();
            let name = fc
                .get("name")
                .and_then(|v| v.as_str())
                .unwrap_or("")
                .to_string();
            let arguments = serde_json::to_string(&fc.get("args").cloned().unwrap_or(Value::Null))
                .unwrap_or_else(|_| "{}".to_string());
            if !name.is_empty() {
                let canonical_call_id = if call_id.is_empty() {
                    format!("call_{}", next_tool_node_index - 1)
                } else {
                    call_id
                };
                parsed.push(CandidatePartState {
                    node_index: next_tool_node_index,
                    active_node: ActiveNodeState {
                        kind: ActiveNodeKind::ToolCall {
                            call_id: canonical_call_id,
                            name,
                            arguments,
                        },
                        extra_body: HashMap::new(),
                    },
                });
                next_tool_node_index += 1;
            }
        }
    }

    parsed
}

fn update_active_node(current: &mut ActiveNodeState, next: &ActiveNodeState) -> Vec<NodeDelta> {
    let mut deltas = Vec::new();

    match (&mut current.kind, &next.kind) {
        (
            ActiveNodeKind::Text { content },
            ActiveNodeKind::Text {
                content: next_content,
            },
        ) => {
            if next_content.len() > content.len() {
                let delta = next_content[content.len()..].to_string();
                if !delta.is_empty() {
                    deltas.push(NodeDelta::Text { content: delta });
                }
            }
            *content = next_content.clone();
        }
        (
            ActiveNodeKind::Reasoning { content, encrypted },
            ActiveNodeKind::Reasoning {
                content: next_content,
                encrypted: next_encrypted,
            },
        ) => {
            let content_delta = if next_content.len() > content.len() {
                Some(next_content[content.len()..].to_string())
            } else {
                None
            };
            let encrypted_delta = if next_encrypted.len() > encrypted.len() {
                Some(Value::String(next_encrypted[encrypted.len()..].to_string()))
            } else {
                None
            };
            if content_delta
                .as_ref()
                .is_some_and(|delta| !delta.is_empty())
                || encrypted_delta.is_some()
            {
                deltas.push(NodeDelta::Reasoning {
                    content: content_delta.filter(|delta| !delta.is_empty()),
                    encrypted: encrypted_delta,
                    summary: None,
                    source: None,
                });
            }
            *content = next_content.clone();
            *encrypted = next_encrypted.clone();
        }
        (
            ActiveNodeKind::ToolCall {
                call_id: _,
                name,
                arguments,
            },
            ActiveNodeKind::ToolCall {
                call_id: _,
                name: next_name,
                arguments: next_arguments,
            },
        ) => {
            if next_arguments.len() > arguments.len() {
                let delta = next_arguments[arguments.len()..].to_string();
                if !delta.is_empty() {
                    deltas.push(NodeDelta::ToolCallArguments { arguments: delta });
                }
            }
            *name = next_name.clone();
            *arguments = next_arguments.clone();
        }
        _ => {
            *current = next.clone();
        }
    }

    current.extra_body = next.extra_body.clone();
    deltas
}

fn initial_deltas_for_active_node(active_node: &ActiveNodeState) -> Vec<NodeDelta> {
    match &active_node.kind {
        ActiveNodeKind::Text { content } if !content.is_empty() => vec![NodeDelta::Text {
            content: content.clone(),
        }],
        ActiveNodeKind::Reasoning { content, encrypted } => {
            if content.is_empty() && encrypted.is_empty() {
                Vec::new()
            } else {
                vec![NodeDelta::Reasoning {
                    content: (!content.is_empty()).then(|| content.clone()),
                    encrypted: (!encrypted.is_empty()).then(|| Value::String(encrypted.clone())),
                    summary: None,
                    source: None,
                }]
            }
        }
        ActiveNodeKind::ToolCall { arguments, .. } if !arguments.is_empty() => {
            vec![NodeDelta::ToolCallArguments {
                arguments: arguments.clone(),
            }]
        }
        _ => Vec::new(),
    }
}

fn node_from_active(active_node: &ActiveNodeState) -> Node {
    match &active_node.kind {
        ActiveNodeKind::Text { content } => Node::Text {
            id: None,
            role: OrdinaryRole::Assistant,
            content: content.clone(),
            phase: None,
            extra_body: active_node.extra_body.clone(),
        },
        ActiveNodeKind::Reasoning { content, encrypted } => Node::Reasoning {
            id: None,
            content: (!content.is_empty()).then(|| content.clone()),
            encrypted: (!encrypted.is_empty()).then(|| Value::String(encrypted.clone())),
            summary: None,
            source: None,
            extra_body: active_node.extra_body.clone(),
        },
        ActiveNodeKind::ToolCall {
            call_id,
            name,
            arguments,
        } => Node::ToolCall {
            id: Some(call_id.clone()),
            call_id: call_id.clone(),
            name: name.clone(),
            arguments: arguments.clone(),
            extra_body: active_node.extra_body.clone(),
        },
    }
}

fn node_header_from_node(node: &Node) -> NodeHeader {
    match node {
        Node::Text { role, phase, .. } => NodeHeader::Text {
            id: node.id().cloned(),
            role: *role,
            phase: phase.clone(),
        },
        Node::Reasoning { .. } => NodeHeader::Reasoning {
            id: node.id().cloned(),
        },
        Node::ToolCall { call_id, name, .. } => NodeHeader::ToolCall {
            id: node.id().cloned(),
            call_id: call_id.clone(),
            name: name.clone(),
        },
        Node::Image { role, .. } => NodeHeader::Image {
            id: node.id().cloned(),
            role: *role,
        },
        Node::Audio { role, .. } => NodeHeader::Audio {
            id: node.id().cloned(),
            role: *role,
        },
        Node::File { role, .. } => NodeHeader::File {
            id: node.id().cloned(),
            role: *role,
        },
        Node::Refusal { .. } => NodeHeader::Refusal {
            id: node.id().cloned(),
        },
        Node::ProviderItem {
            role, item_type, ..
        } => NodeHeader::ProviderItem {
            id: node.id().cloned(),
            role: *role,
            item_type: item_type.clone(),
        },
        Node::ToolResult { call_id, .. } => NodeHeader::ToolResult {
            id: node.id().cloned(),
            call_id: call_id.clone(),
        },
        Node::NextDownstreamEnvelopeExtra { .. } => NodeHeader::NextDownstreamEnvelopeExtra,
    }
}

fn ordered_completed_nodes(state: &GeminiStreamState) -> Vec<Node> {
    state
        .node_order
        .iter()
        .filter_map(|node_index| state.completed_nodes.get(node_index).cloned())
        .collect()
}

fn parse_finish_reason(reason: &str) -> FinishReason {
    match reason {
        "MAX_TOKENS" => FinishReason::Length,
        "SAFETY" => FinishReason::ContentFilter,
        "STOP" => FinishReason::Stop,
        _ => FinishReason::Other,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn gemini_start_and_initial_delta_share_canonical_node_index() {
        let node = Node::Text {
            id: None,
            role: OrdinaryRole::Assistant,
            content: "hello".to_string(),
            phase: None,
            extra_body: HashMap::new(),
        };
        let active = ActiveNodeState {
            kind: ActiveNodeKind::Text {
                content: "hello".to_string(),
            },
            extra_body: HashMap::new(),
        };
        let mut events = Vec::new();

        events.push(UrpStreamEvent::NodeStart {
            node_index: 0,
            header: node_header_from_node(&node),
            extra_body: HashMap::new(),
        });
        for delta in initial_deltas_for_active_node(&active) {
            events.push(UrpStreamEvent::NodeDelta {
                node_index: 0,
                delta,
                usage: None,
                extra_body: HashMap::new(),
            });
        }

        assert_eq!(events.len(), 2);
        assert!(matches!(
            &events[0],
            UrpStreamEvent::NodeStart {
                node_index,
                header: NodeHeader::Text { role: OrdinaryRole::Assistant, .. },
                ..
            } if *node_index == 0
        ));
        assert!(matches!(
            &events[1],
            UrpStreamEvent::NodeDelta {
                node_index,
                delta: NodeDelta::Text { content },
                ..
            } if *node_index == 0 && content == "hello"
        ));
    }

    #[test]
    fn gemini_completion_uses_ordered_completed_nodes_for_response_done() {
        let state = GeminiStreamState {
            node_order: vec![1, 0, 2],
            active_nodes: HashMap::new(),
            completed_nodes: HashMap::from([
                (
                    0,
                    Node::Text {
                        id: None,
                        role: OrdinaryRole::Assistant,
                        content: "answer".to_string(),
                        phase: None,
                        extra_body: HashMap::new(),
                    },
                ),
                (
                    1,
                    Node::Reasoning {
                        id: None,
                        content: Some("think".to_string()),
                        encrypted: None,
                        summary: Some("sum".to_string()),
                        source: None,
                        extra_body: HashMap::new(),
                    },
                ),
                (
                    2,
                    Node::ToolCall {
                        id: None,
                        call_id: "call_1".to_string(),
                        name: "lookup".to_string(),
                        arguments: "{\"a\":1}".to_string(),
                        extra_body: HashMap::new(),
                    },
                ),
            ]),
        };

        let ordered = ordered_completed_nodes(&state);
        assert!(matches!(&ordered[0], Node::Reasoning { .. }));
        assert!(matches!(&ordered[1], Node::Text { content, .. } if content == "answer"));
        assert!(matches!(&ordered[2], Node::ToolCall { call_id, .. } if call_id == "call_1"));

        let completion_event = UrpStreamEvent::ResponseDone {
            finish_reason: Some(FinishReason::Stop),
            usage: None,
            output: ordered,
            extra_body: HashMap::new(),
        };

        assert!(matches!(
            &completion_event,
            UrpStreamEvent::ResponseDone {
                finish_reason: Some(FinishReason::Stop),
                output,
                ..
            }
            if matches!(&output[0], Node::Reasoning { content: Some(content), summary: Some(summary), .. } if content == "think" && summary == "sum")
                && matches!(&output[1], Node::Text { content, .. } if content == "answer")
                && matches!(&output[2], Node::ToolCall { call_id, arguments, .. } if call_id == "call_1" && arguments == "{\"a\":1}")
        ));
    }
}
