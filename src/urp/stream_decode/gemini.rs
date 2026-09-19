use crate::error::{AppError, AppResult};
use crate::handlers::usage::{
    mark_stream_ttfb_if_needed, parse_usage_from_gemini_object, record_stream_done_sentinel,
    record_stream_terminal_event, record_stream_usage_if_present,
    record_visible_stream_event_delta,
};
use crate::handlers::{StreamRuntimeMetrics, UrpRequest as HandlerUrpRequest};
use crate::urp::decode::gemini::{
    GEMINI_CANDIDATE_EXTRA_KEY, attach_candidate_citations, candidate_extra, content_parts,
    decode_stream_part, parse_finish_reason, parse_usage, prompt_block_reason, prompt_refusal,
};
use crate::urp::{FinishReason, Node, NodeDelta, NodeHeader, UrpStreamEvent};
use axum::http::StatusCode;
use eventsource_stream::Eventsource;
use futures_util::StreamExt;
use serde_json::Value;
use std::collections::HashMap;
use std::sync::Arc;
use tokio::sync::{Mutex, mpsc};

pub(crate) async fn stream_gemini_to_urp_events(
    urp: &HandlerUrpRequest,
    upstream_resp: reqwest::Response,
    tx: mpsc::Sender<UrpStreamEvent>,
    started_at: Option<std::time::Instant>,
    runtime_metrics: Option<Arc<Mutex<StreamRuntimeMetrics>>>,
    idle_timeout_ms: u64,
) -> AppResult<()> {
    let mut response_id = format!("resp_{}", uuid::Uuid::new_v4());
    let mut started_response = false;
    let mut finish_reason = None;
    let mut usage = None;
    let mut output = Vec::<Node>::new();
    let mut extra_body = HashMap::new();
    let idle_timeout = std::time::Duration::from_millis(idle_timeout_ms.max(1));
    let mut stream = upstream_resp.bytes_stream().eventsource();
    while let Some(event) = tokio::time::timeout(idle_timeout, stream.next())
        .await
        .map_err(|_| {
            AppError::new(
                StatusCode::GATEWAY_TIMEOUT,
                "upstream_idle_timeout",
                format!("upstream stream idle for {idle_timeout_ms}ms without data"),
            )
        })?
    {
        let event =
            event.map_err(|err| stream_error("upstream_stream_decode_failed", err.to_string()))?;
        mark_stream_ttfb_if_needed(started_at, &runtime_metrics).await;
        if event.data.trim() == "[DONE]" {
            record_stream_done_sentinel(&runtime_metrics).await;
            break;
        }
        let data: Value = serde_json::from_str(&event.data)
            .map_err(|err| stream_error("upstream_stream_decode_failed", err.to_string()))?;
        if let Some(error) = data.get("error") {
            return Err(stream_error("upstream_error", error.to_string()));
        }
        let obj = data.as_object().ok_or_else(|| {
            stream_error(
                "upstream_stream_decode_failed",
                "Gemini stream event must be an object".to_string(),
            )
        })?;
        if let Some(native) = data.get("usageMetadata").and_then(Value::as_object) {
            usage = Some(
                parse_usage(native)
                    .map_err(|error| stream_error("upstream_stream_decode_failed", error))?,
            );
        }
        record_stream_usage_if_present(&runtime_metrics, parse_usage_from_gemini_object(&data))
            .await;
        extra_body.extend(crate::urp::decode::split_extra(
            obj,
            &["candidates", "usageMetadata", "responseId", "modelVersion"],
        ));
        if !started_response {
            if let Some(id) = data.get("responseId").and_then(Value::as_str) {
                response_id = id.to_string();
            }
            let _ = tx
                .send(UrpStreamEvent::ResponseStart {
                    usage: usage.clone(),
                    id: response_id.clone(),
                    model: data
                        .get("modelVersion")
                        .and_then(Value::as_str)
                        .unwrap_or(&urp.model)
                        .to_string(),
                    extra_body: extra_body.clone(),
                })
                .await;
            started_response = true;
        }
        let candidate = data
            .get("candidates")
            .and_then(Value::as_array)
            .and_then(|candidates| candidates.first());
        let mut nodes = Vec::new();
        if let Some(parts) = candidate
            .and_then(|candidate| candidate.get("content"))
            .and_then(|content| content.get("parts"))
        {
            for part in content_parts(parts) {
                nodes.extend(
                    decode_stream_part(part)
                        .map_err(|error| stream_error("upstream_stream_decode_failed", error))?,
                );
            }
        }
        if let Some(candidate) = candidate.and_then(Value::as_object) {
            let metadata = candidate_extra(candidate);
            if !metadata.is_empty() {
                let stored = extra_body
                    .entry(GEMINI_CANDIDATE_EXTRA_KEY.into())
                    .or_insert_with(|| serde_json::json!({}));
                stored.as_object_mut().unwrap().extend(metadata);
            }
            if nodes.iter().any(|node| matches!(node, Node::Text { .. })) {
                attach_candidate_citations(candidate, &mut nodes);
            } else if let Some(sources) = candidate
                .get("citationMetadata")
                .and_then(|metadata| metadata.get("citationSources"))
                .and_then(Value::as_array)
            {
                if let Some((index, Node::Text { citations, .. })) = output
                    .iter_mut()
                    .enumerate()
                    .find(|(_, node)| matches!(node, Node::Text { .. }))
                {
                    let mut added = Vec::new();
                    for source in &crate::urp::citations::decode(
                        sources.clone(),
                        crate::urp::ProviderProtocol::Gemini,
                    ) {
                        if !citations.contains(source) {
                            citations.push(source.clone());
                            added.push(source.clone());
                        }
                    }
                    if !added.is_empty() {
                        let _ = tx
                            .send(UrpStreamEvent::NodeDelta {
                                node_index: index as u32,
                                delta: NodeDelta::Text {
                                    logprobs: None,
                                    content: String::new(),
                                    signature: None,
                                    citations: added,
                                },
                                usage: None,
                                extra_body: HashMap::new(),
                            })
                            .await;
                    }
                }
            }
        }
        if let Some(reason) = prompt_block_reason(&data) {
            nodes.push(prompt_refusal(reason));
            finish_reason = Some(FinishReason::ContentFilter);
        }
        for node in nodes {
            let delta = initial_delta(&node);
            let merged = output
                .last_mut()
                .is_some_and(|last| append_fragment(last, &node));
            if !merged {
                if let Some(last) = output.last() {
                    let _ = tx
                        .send(UrpStreamEvent::NodeDone {
                            node_index: (output.len() - 1) as u32,
                            node: last.clone(),
                            usage: None,
                            extra_body: node_extra(last).clone(),
                        })
                        .await;
                }
                let event = UrpStreamEvent::NodeStart {
                    node_index: output.len() as u32,
                    header: node_header_from_node(&node),
                    extra_body: node_extra(&node).clone(),
                };
                record_visible_stream_event_delta(&runtime_metrics, &event).await;
                let _ = tx.send(event).await;
                output.push(node.clone());
            }
            if let Some(delta) = delta {
                let event = UrpStreamEvent::NodeDelta {
                    node_index: (output.len() - 1) as u32,
                    delta,
                    usage: None,
                    extra_body: node_extra(&node).clone(),
                };
                record_visible_stream_event_delta(&runtime_metrics, &event).await;
                let _ = tx.send(event).await;
            }
        }
        if finish_reason.is_none() {
            finish_reason = candidate
                .and_then(|candidate| candidate.get("finishReason"))
                .and_then(Value::as_str)
                .filter(|reason| !reason.is_empty() && *reason != "FINISH_REASON_UNSPECIFIED")
                .map(parse_finish_reason);
        }
    }
    if finish_reason.is_none() {
        return Err(stream_error(
            "upstream_stream_missing_terminal",
            "Gemini stream ended without a finishReason or prompt block".to_string(),
        ));
    }
    if finish_reason == Some(FinishReason::Stop)
        && output
            .iter()
            .any(|node| matches!(node, Node::ToolCall { .. }))
    {
        finish_reason = Some(FinishReason::ToolCalls);
    }
    if let Some(last) = output.last() {
        let _ = tx
            .send(UrpStreamEvent::NodeDone {
                node_index: (output.len() - 1) as u32,
                node: last.clone(),
                usage: None,
                extra_body: node_extra(last).clone(),
            })
            .await;
    }
    let _ = tx
        .send(UrpStreamEvent::ResponseDone {
            outcome: None,
            finish_reason,
            usage,
            output,
            extra_body,
        })
        .await;
    record_stream_terminal_event(&runtime_metrics, "response.completed", None).await;
    Ok(())
}

fn stream_error(code: &'static str, message: String) -> AppError {
    AppError::new(StatusCode::BAD_GATEWAY, code, message)
}

fn append_fragment(current: &mut Node, next: &Node) -> bool {
    if [ &*current, next ].iter().any(|node| matches!(node, Node::Text { signature, citations, .. } if signature.is_some() || !citations.is_empty())) { return false; }
    if !node_extra(current).is_empty() || !node_extra(next).is_empty() {
        return false;
    }
    match (current, next) {
        (
            Node::Text { content, .. },
            Node::Text {
                content: fragment, ..
            },
        ) => {
            content.push_str(fragment);
            true
        }
        (
            Node::Reasoning {
                id: None,
                content: Some(content),
                encrypted: None,
                ..
            },
            Node::Reasoning {
                id: None,
                content: Some(fragment),
                encrypted: None,
                ..
            },
        ) => {
            content.push_str(fragment);
            true
        }
        _ => false,
    }
}

fn initial_delta(node: &Node) -> Option<NodeDelta> {
    match node {
        Node::Text {
            signature,
            citations,
            content,
            ..
        } => (!content.is_empty() || signature.is_some() || !citations.is_empty()).then(|| {
            NodeDelta::Text {
                logprobs: None,
                signature: signature.clone(),
                citations: citations.clone(),
                content: content.clone(),
            }
        }),
        Node::Reasoning {
            content,
            encrypted,
            summary,
            source,
            metadata,
            ..
        } => Some(NodeDelta::Reasoning {
            metadata: metadata.clone(),
            content: content.clone(),
            encrypted: encrypted.clone(),
            summary: summary.clone(),
            source: source.clone(),
        }),
        Node::ToolCall { arguments, .. } => Some(NodeDelta::ToolCallArguments {
            arguments: arguments.clone(),
        }),
        Node::Image { source, .. } => Some(NodeDelta::Image {
            source: source.clone(),
        }),
        Node::Audio { source, .. } => Some(NodeDelta::Audio {
            source: source.clone(),
        }),
        Node::File { source, .. } => Some(NodeDelta::File {
            source: source.clone(),
        }),
        Node::Refusal { content, .. } => Some(NodeDelta::Refusal {
            logprobs: None,
            content: content.clone(),
        }),
        _ => None,
    }
}

fn node_header_from_node(node: &Node) -> NodeHeader {
    match node {
        Node::Text {
            signature,
            citations,
            role,
            phase,
            ..
        } => NodeHeader::Text {
            signature: signature.clone(),
            citations: citations.clone(),
            id: node.id().cloned(),
            role: *role,
            phase: phase.clone(),
        },
        Node::Reasoning { metadata, .. } => NodeHeader::Reasoning {
            metadata: metadata.clone(),
            id: node.id().cloned(),
        },
        Node::ToolCall {
            namespace,
            signature,
            tool_type,
            call_id,
            name,
            ..
        } => NodeHeader::ToolCall {
            namespace: namespace.clone(),
            signature: signature.clone(),
            id: node.id().cloned(),
            tool_type: *tool_type,
            call_id: call_id.clone(),
            name: name.clone(),
        },
        Node::Image { role, metadata, .. } => NodeHeader::Image {
            metadata: metadata.clone(),
            id: node.id().cloned(),
            role: *role,
        },
        Node::Audio { role, metadata, .. } => NodeHeader::Audio {
            metadata: metadata.clone(),
            id: node.id().cloned(),
            role: *role,
        },
        Node::File { role, metadata, .. } => NodeHeader::File {
            metadata: metadata.clone(),
            id: node.id().cloned(),
            role: *role,
        },
        Node::Refusal { .. } => NodeHeader::Refusal {
            id: node.id().cloned(),
        },
        Node::ProviderItem {
            role,
            origin_protocol,
            item_type,
            body,
            ..
        } => NodeHeader::ProviderItem {
            body: Some(body.clone()),
            id: node.id().cloned(),
            origin_protocol: *origin_protocol,
            role: *role,
            item_type: item_type.clone(),
        },
        Node::ToolResult {
            signature,
            namespace,
            name,
            tool_type,
            call_id,
            ..
        } => NodeHeader::ToolResult {
            signature: signature.clone(),
            namespace: namespace.clone(),
            name: name.clone(),
            id: node.id().cloned(),
            tool_type: *tool_type,
            call_id: call_id.clone(),
        },
        Node::NextDownstreamEnvelopeExtra { .. } => NodeHeader::NextDownstreamEnvelopeExtra,
    }
}

fn node_extra(node: &Node) -> &HashMap<String, Value> {
    match node {
        Node::Text { extra_body, .. }
        | Node::Image { extra_body, .. }
        | Node::Audio { extra_body, .. }
        | Node::File { extra_body, .. }
        | Node::Refusal { extra_body, .. }
        | Node::Reasoning { extra_body, .. }
        | Node::ToolCall { extra_body, .. }
        | Node::ProviderItem { extra_body, .. }
        | Node::ToolResult { extra_body, .. }
        | Node::NextDownstreamEnvelopeExtra { extra_body, .. } => extra_body,
    }
}
