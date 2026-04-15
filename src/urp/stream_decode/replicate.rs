use crate::error::{AppError, AppResult};
use crate::handlers::usage::{
    mark_stream_ttfb_if_needed, record_stream_done_sentinel, record_stream_terminal_event,
};
use crate::handlers::{StreamRuntimeMetrics, UrpRequest as HandlerUrpRequest};
use crate::urp::{
    FinishReason, Item, ItemHeader, Node, NodeDelta, NodeHeader, OrdinaryRole, Part, PartDelta,
    PartHeader, Role, UrpStreamEvent, nodes_to_items,
};
use axum::http::StatusCode;
use eventsource_stream::Eventsource;
use futures_util::StreamExt;
use std::collections::HashMap;
use std::sync::Arc;
use tokio::sync::{Mutex, mpsc};

pub(crate) async fn stream_replicate_to_urp_events(
    urp: &HandlerUrpRequest,
    upstream_resp: reqwest::Response,
    tx: mpsc::Sender<UrpStreamEvent>,
    started_at: Option<std::time::Instant>,
    runtime_metrics: Option<Arc<Mutex<StreamRuntimeMetrics>>>,
) -> AppResult<()> {
    let response_id = format!("resp_{}", uuid::Uuid::new_v4());
    let mut started_response = false;
    let mut bridge_item_started = false;
    let mut text_started = false;
    let mut output_text = String::new();
    let mut had_error = false;
    let mut finish_reason: Option<FinishReason> = None;

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

        match ev.event.as_str() {
            "output" => {
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

                if !text_started {
                    let _ = tx
                        .send(UrpStreamEvent::NodeStart {
                            node_index: 0,
                            header: NodeHeader::Text {
                                id: None,
                                role: OrdinaryRole::Assistant,
                                phase: None,
                            },
                            extra_body: HashMap::new(),
                        })
                        .await;
                    if !bridge_item_started {
                        let _ = tx
                            .send(UrpStreamEvent::ItemStart {
                                item_index: 0,
                                header: ItemHeader::Message {
                                    id: None,
                                    role: Role::Assistant,
                                },
                                extra_body: HashMap::new(),
                            })
                            .await;
                        bridge_item_started = true;
                    }
                    let _ = tx
                        .send(UrpStreamEvent::PartStart {
                            part_index: 0,
                            item_index: 0,
                            header: PartHeader::Text,
                            extra_body: HashMap::new(),
                        })
                        .await;
                    text_started = true;
                }

                output_text.push_str(&ev.data);
                let delta = NodeDelta::Text {
                    content: ev.data.clone(),
                };
                let _ = tx
                    .send(UrpStreamEvent::NodeDelta {
                        node_index: 0,
                        delta: delta.clone(),
                        usage: None,
                        extra_body: HashMap::new(),
                    })
                    .await;
                let _ = tx
                    .send(UrpStreamEvent::Delta {
                        part_index: 0,
                        delta: part_delta_from_node_delta(&delta)
                            .unwrap_or(PartDelta::Text { content: String::new() }),
                        usage: None,
                        extra_body: HashMap::new(),
                    })
                    .await;
            }
            "error" => {
                had_error = true;
                let _ = tx
                    .send(UrpStreamEvent::Error {
                        code: Some("replicate_error".to_string()),
                        message: ev.data.clone(),
                        extra_body: HashMap::new(),
                    })
                    .await;
            }
            "done" => {
                record_stream_done_sentinel(&runtime_metrics).await;
                finish_reason = Some(if had_error {
                    FinishReason::Other
                } else {
                    FinishReason::Stop
                });
                break;
            }
            _ => {}
        }
    }

    let completed_nodes = if text_started {
        vec![Node::Text {
            id: None,
            role: OrdinaryRole::Assistant,
            content: output_text.clone(),
            phase: None,
            extra_body: HashMap::new(),
        }]
    } else {
        Vec::new()
    };

    for node in &completed_nodes {
        let _ = tx
            .send(UrpStreamEvent::NodeDone {
                node_index: 0,
                node: node.clone(),
                usage: None,
                extra_body: HashMap::new(),
            })
            .await;
        if let Some(part) = bridge_part_from_node(node) {
            let _ = tx
                .send(UrpStreamEvent::PartDone {
                    part_index: 0,
                    part,
                    usage: None,
                    extra_body: HashMap::new(),
                })
                .await;
        }
    }

    let output_item = build_completed_message_item(&completed_nodes);
    let outputs = completed_nodes.clone();

    if started_response {
        if !bridge_item_started {
            let _ = tx
                .send(UrpStreamEvent::ItemStart {
                    item_index: 0,
                    header: ItemHeader::Message {
                        id: None,
                        role: Role::Assistant,
                    },
                    extra_body: HashMap::new(),
                })
                .await;
        }
        let _ = tx
            .send(UrpStreamEvent::ItemDone {
                item_index: 0,
                item: output_item,
                usage: None,
                extra_body: HashMap::new(),
            })
            .await;
        let _ = tx
            .send(UrpStreamEvent::ResponseDone {
                finish_reason,
                usage: None,
                output: outputs,
                extra_body: HashMap::new(),
            })
            .await;
    }

    record_stream_terminal_event(&runtime_metrics, "response.completed", None).await;
    Ok(())
}

fn part_delta_from_node_delta(delta: &NodeDelta) -> Option<PartDelta> {
    match delta {
        NodeDelta::Text { content } => Some(PartDelta::Text {
            content: content.clone(),
        }),
        NodeDelta::Reasoning {
            content,
            encrypted,
            summary,
            source,
        } => Some(PartDelta::Reasoning {
            content: content.clone(),
            encrypted: encrypted.clone(),
            summary: summary.clone(),
            source: source.clone(),
        }),
        NodeDelta::Refusal { content } => Some(PartDelta::Refusal {
            content: content.clone(),
        }),
        NodeDelta::ToolCallArguments { arguments } => Some(PartDelta::ToolCallArguments {
            arguments: arguments.clone(),
        }),
        NodeDelta::Image { source } => Some(PartDelta::Image {
            source: source.clone(),
        }),
        NodeDelta::Audio { source } => Some(PartDelta::Audio {
            source: source.clone(),
        }),
        NodeDelta::File { source } => Some(PartDelta::File {
            source: source.clone(),
        }),
        NodeDelta::ProviderItem { data } => Some(PartDelta::ProviderItem { data: data.clone() }),
    }
}

fn bridge_part_from_node(node: &Node) -> Option<Part> {
    match nodes_to_items(std::slice::from_ref(node)).into_iter().next() {
        Some(Item::Message { mut parts, .. }) if !parts.is_empty() => Some(parts.remove(0)),
        _ => None,
    }
}

fn build_completed_message_item(nodes: &[Node]) -> Item {
    Item::Message {
        id: Some(crate::urp::synthetic_message_id()),
        role: Role::Assistant,
        parts: nodes.iter().filter_map(bridge_part_from_node).collect(),
        extra_body: HashMap::new(),
    }
}
