use crate::error::{AppError, AppResult};
use crate::handlers::usage::{
    mark_stream_ttfb_if_needed, parse_usage_from_gemini_object,
    record_observed_upstream_response_model, record_stream_done_sentinel,
    record_stream_terminal_error, record_stream_terminal_event, record_stream_usage_if_present,
    record_visible_stream_event_delta,
};
use crate::handlers::{StreamRuntimeMetrics, StreamTerminalError, UrpRequest as HandlerUrpRequest};
use crate::urp::decode::gemini::{
    GEMINI_CANDIDATE_EXTRA_KEY, GEMINI_CONTENT_EXTRA_KEY, candidate_extra, content_parts,
    decode_stream_part, parse_finish_reason, parse_usage, prompt_block_reason, prompt_refusal,
    response_outcome, selected_candidate,
};
use crate::urp::{FinishReason, Node, NodeDelta, NodeHeader, UrpStreamEvent};
use axum::http::StatusCode;
use eventsource_stream::Eventsource;
use futures_util::StreamExt;
use serde_json::{Map, Value};
use std::collections::{HashMap, HashSet};
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
    let mut closed = HashSet::new();
    let mut candidate_index = None;
    let mut extra_body = HashMap::new();
    let mut candidate_metadata = Map::new();
    let mut citation_frames = Vec::new();
    let mut grounding = StreamGrounding::default();
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
        if let Some(error) = data.get("error").filter(|error| !error.is_null()) {
            return Err(emit_native_error(&tx, &runtime_metrics, error).await);
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
        if let Some(model) = data.get("modelVersion").and_then(Value::as_str) {
            record_observed_upstream_response_model(&runtime_metrics, model, true).await;
        }
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
        let candidate = if let Some(index) = candidate_index {
            data.get("candidates")
                .and_then(Value::as_array)
                .into_iter()
                .flatten()
                .filter_map(Value::as_object)
                .find(|candidate| {
                    candidate.get("index").and_then(Value::as_u64).unwrap_or(0) == index
                })
        } else {
            let candidate = selected_candidate(&data);
            if let Some(candidate) = candidate {
                candidate_index = Some(candidate.get("index").and_then(Value::as_u64).unwrap_or(0));
            }
            candidate
        };
        let mut frame_parts = HashMap::new();
        if finish_reason.is_none() {
            if let Some(content) = candidate
                .and_then(|candidate| candidate.get("content"))
                .and_then(Value::as_object)
            {
                let content_extra = crate::urp::decode::split_extra(content, &["role", "parts"]);
                if let Some(parts) = content.get("parts") {
                    let frame_scores = candidate
                        .map(|candidate| frame_text_scores(candidate, content_parts(parts)))
                        .unwrap_or_default();
                    for (part_index, part) in content_parts(parts).iter().enumerate() {
                        if let Some(signature) = continuation_signature(part) {
                            if let Some(last) = output.last_mut() {
                                if bind_signature(last, part, signature) {
                                    if let Some(delta) = signature_delta(last) {
                                        let _ = tx
                                            .send(UrpStreamEvent::NodeDelta {
                                                node_index: (output.len() - 1) as u32,
                                                delta,
                                                usage: None,
                                                extra_body: HashMap::new(),
                                            })
                                            .await;
                                    }
                                    continue;
                                }
                            }
                        }
                        let nodes = decode_stream_part(part).map_err(|error| {
                            stream_error("upstream_stream_decode_failed", error)
                        })?;
                        for (offset, mut node) in nodes.into_iter().enumerate() {
                            if let Node::Text { logprobs, .. } = &mut node {
                                if output.iter().all(|node| match node {
                                    Node::Text {
                                        content, logprobs, ..
                                    } => {
                                        content.is_empty()
                                            || crate::urp::logprobs::valid(logprobs, content)
                                                .is_some()
                                    }
                                    _ => true,
                                }) {
                                    *logprobs = frame_scores.get(&part_index).cloned();
                                }
                            }
                            if part_index == 0 && offset == 0 && !content_extra.is_empty() {
                                node.extra_body_mut().insert(
                                    GEMINI_CONTENT_EXTRA_KEY.into(),
                                    serde_json::json!(content_extra),
                                );
                            }
                            let previous_text_bytes = output.last().map_or(0, |node| match node {
                                Node::Text { content, .. } => content.len() as u64,
                                _ => 0,
                            });
                            let merged = part_index == 0
                                && offset == 0
                                && output
                                    .last_mut()
                                    .is_some_and(|last| append_fragment(last, &mut node));
                            let delta = initial_delta(&node);
                            if !merged {
                                if let Some(last) = output.last() {
                                    let index = (output.len() - 1) as u32;
                                    if !matches!(last, Node::Text { .. }) && closed.insert(index) {
                                        let _ = tx
                                            .send(UrpStreamEvent::NodeDone {
                                                node_index: index,
                                                node: last.clone(),
                                                usage: None,
                                                extra_body: node_extra(last).clone(),
                                            })
                                            .await;
                                    }
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
                            frame_parts.entry(part_index).or_insert((
                                output.len() - 1,
                                if merged { previous_text_bytes } else { 0 },
                            ));
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
                    }
                }
            }
            if let Some(reason) = prompt_block_reason(&data) {
                let node = prompt_refusal(reason);
                let _ = tx
                    .send(UrpStreamEvent::NodeStart {
                        node_index: output.len() as u32,
                        header: node_header_from_node(&node),
                        extra_body: HashMap::new(),
                    })
                    .await;
                if let Some(delta) = initial_delta(&node) {
                    let _ = tx
                        .send(UrpStreamEvent::NodeDelta {
                            node_index: output.len() as u32,
                            delta,
                            usage: None,
                            extra_body: HashMap::new(),
                        })
                        .await;
                }
                output.push(node);
                finish_reason = Some(FinishReason::ContentFilter);
            }
            if finish_reason.is_none() {
                finish_reason = candidate
                    .and_then(|candidate| candidate.get("finishReason"))
                    .and_then(Value::as_str)
                    .filter(|reason| !reason.is_empty() && *reason != "FINISH_REASON_UNSPECIFIED")
                    .map(parse_finish_reason);
            }
        }
        if let Some(candidate) = candidate {
            if candidate.contains_key("citationMetadata") {
                citation_frames.push(
                    candidate
                        .iter()
                        .filter(|(key, _)| key.as_str() == "citationMetadata")
                        .map(|(key, value)| (key.clone(), value.clone()))
                        .collect::<Map<String, Value>>(),
                );
            }
            grounding.ingest(candidate, &frame_parts);
            merge_candidate_metadata(&mut candidate_metadata, candidate, &output);
            let metadata = candidate_extra(candidate);
            if !metadata.is_empty() {
                let stored = extra_body
                    .entry(GEMINI_CANDIDATE_EXTRA_KEY.into())
                    .or_insert_with(|| serde_json::json!({}));
                stored.as_object_mut().unwrap().extend(metadata);
            }
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
    for candidate in citation_frames {
        crate::urp::citations::attach_gemini(&candidate, &mut output);
    }
    if let Some(candidate) = grounding.candidate() {
        crate::urp::citations::attach_gemini(&candidate, &mut output);
        let stored = extra_body
            .entry(GEMINI_CANDIDATE_EXTRA_KEY.into())
            .or_insert_with(|| serde_json::json!({}));
        if let Some(metadata) = crate::urp::citations::gemini_metadata_extra(&candidate) {
            stored["groundingMetadata"] = metadata;
        } else {
            stored.as_object_mut().unwrap().remove("groundingMetadata");
        }
    }
    let streamed_scores: Vec<_> = output
        .iter()
        .map(|node| match node {
            Node::Text { logprobs, .. } => logprobs.clone().unwrap_or_default(),
            _ => Vec::new(),
        })
        .collect();
    crate::urp::logprobs::attach_gemini(&candidate_metadata, &mut output);
    for (index, node) in output.iter().enumerate() {
        if let Node::Text {
            citations,
            logprobs,
            ..
        } = node
        {
            let scores = logprobs
                .as_ref()
                .filter(|scores| scores.starts_with(&streamed_scores[index]))
                .map(|scores| scores[streamed_scores[index].len()..].to_vec())
                .filter(|scores| !scores.is_empty());
            if !citations.is_empty() || scores.is_some() {
                let _ = tx
                    .send(UrpStreamEvent::NodeDelta {
                        node_index: index as u32,
                        delta: NodeDelta::Text {
                            content: String::new(),
                            signature: None,
                            citations: citations.clone(),
                            logprobs: scores,
                        },
                        usage: None,
                        extra_body: HashMap::new(),
                    })
                    .await;
            }
        }
    }
    let outcome = response_outcome(Some(&candidate_metadata), finish_reason);
    let terminal_event = if outcome
        .as_ref()
        .is_some_and(|outcome| outcome.status == crate::urp::ResponseStatus::Failed)
    {
        "response.failed"
    } else {
        "response.completed"
    };
    // Candidate citations and scores can arrive after all content. Close each node only once.
    for (index, node) in output.iter().enumerate() {
        if closed.contains(&(index as u32)) {
            continue;
        }
        let _ = tx
            .send(UrpStreamEvent::NodeDone {
                node_index: index as u32,
                node: node.clone(),
                usage: None,
                extra_body: node_extra(node).clone(),
            })
            .await;
    }
    let _ = tx
        .send(UrpStreamEvent::ResponseDone {
            outcome,
            finish_reason,
            usage,
            output,
            extra_body,
        })
        .await;
    record_stream_terminal_event(&runtime_metrics, terminal_event, None).await;
    Ok(())
}

fn stream_error(code: &'static str, message: String) -> AppError {
    AppError::new(StatusCode::BAD_GATEWAY, code, message)
}

#[derive(Default)]
struct StreamGrounding {
    chunks: Vec<Value>,
    supports: Vec<Value>,
    metadata: Map<String, Value>,
}

impl StreamGrounding {
    fn ingest(
        &mut self,
        candidate: &Map<String, Value>,
        frame_parts: &HashMap<usize, (usize, u64)>,
    ) {
        let Some(grounding) = candidate
            .get("groundingMetadata")
            .and_then(Value::as_object)
        else {
            return;
        };
        if let Some(chunks) = grounding.get("groundingChunks").and_then(Value::as_array) {
            self.chunks.extend(chunks.iter().cloned());
        }
        self.metadata.extend(
            grounding
                .iter()
                .filter(|(key, _)| !matches!(key.as_str(), "groundingChunks" | "groundingSupports"))
                .map(|(key, value)| (key.clone(), value.clone())),
        );
        for support in grounding
            .get("groundingSupports")
            .and_then(Value::as_array)
            .into_iter()
            .flatten()
        {
            let mut support = support.clone();
            if !frame_parts.is_empty() {
                let Some(segment) = support.get_mut("segment").and_then(Value::as_object_mut)
                else {
                    continue;
                };
                let part = segment
                    .get("partIndex")
                    .and_then(Value::as_u64)
                    .unwrap_or(0) as usize;
                let Some((node, offset)) = frame_parts.get(&part) else {
                    continue;
                };
                let Some(start) = segment
                    .get("startIndex")
                    .and_then(Value::as_u64)
                    .unwrap_or(0)
                    .checked_add(*offset)
                else {
                    continue;
                };
                let Some(end) = segment
                    .get("endIndex")
                    .and_then(Value::as_u64)
                    .and_then(|end| end.checked_add(*offset))
                else {
                    continue;
                };
                segment.insert("partIndex".into(), Value::from(*node));
                segment.insert("startIndex".into(), Value::from(start));
                segment.insert("endIndex".into(), Value::from(end));
            }
            self.supports.push(support);
        }
    }

    fn candidate(self) -> Option<Map<String, Value>> {
        if self.chunks.is_empty() && self.supports.is_empty() && self.metadata.is_empty() {
            return None;
        }
        let mut metadata = self.metadata;
        if !self.chunks.is_empty() {
            metadata.insert("groundingChunks".into(), Value::Array(self.chunks));
        }
        if !self.supports.is_empty() {
            metadata.insert("groundingSupports".into(), Value::Array(self.supports));
        }
        Some(Map::from_iter([(
            "groundingMetadata".into(),
            Value::Object(metadata),
        )]))
    }
}

async fn emit_native_error(
    tx: &mpsc::Sender<UrpStreamEvent>,
    runtime_metrics: &Option<Arc<Mutex<StreamRuntimeMetrics>>>,
    error: &Value,
) -> AppError {
    let code = error
        .get("status")
        .and_then(Value::as_str)
        .map(str::to_owned)
        .or_else(|| {
            error
                .get("code")
                .filter(|value| !value.is_null())
                .map(|value| {
                    value
                        .as_str()
                        .map(str::to_owned)
                        .unwrap_or_else(|| value.to_string())
                })
        })
        .unwrap_or_else(|| "upstream_error".into());
    let message = error
        .get("message")
        .and_then(Value::as_str)
        .map(str::to_owned)
        .unwrap_or_else(|| error.to_string());
    let extra_body = error
        .as_object()
        .map(|error| crate::urp::decode::split_extra(error, &["code", "message", "status"]))
        .unwrap_or_default();
    let sent = tx
        .send(UrpStreamEvent::Error {
            code: Some(code.clone()),
            message: message.clone(),
            extra_body,
        })
        .await
        .is_ok();
    record_stream_terminal_error(
        runtime_metrics,
        "error",
        StreamTerminalError {
            code: code.clone(),
            message: message.clone(),
            http_status: StatusCode::BAD_GATEWAY.as_u16(),
            error_type: error
                .get("status")
                .and_then(Value::as_str)
                .map(str::to_owned),
            param: None,
        },
    )
    .await;
    AppError::new(StatusCode::BAD_GATEWAY, code, message).with_downstream_stream_terminal_sent(sent)
}

pub(crate) fn continuation_signature(part: &Value) -> Option<&Value> {
    let part = part.as_object()?;
    if part
        .keys()
        .any(|key| !matches!(key.as_str(), "text" | "thought" | "thoughtSignature"))
        || part
            .get("text")
            .is_some_and(|text| text.as_str() != Some(""))
    {
        return None;
    }
    part.get("thoughtSignature")
        .filter(|signature| !signature.is_null())
}

pub(crate) fn bind_signature(node: &mut Node, part: &Value, signature: &Value) -> bool {
    if let Some(thought) = part.get("thought").and_then(Value::as_bool) {
        if thought != matches!(node, Node::Reasoning { .. }) {
            return false;
        }
    }
    let target = match node {
        Node::Text { signature, .. }
        | Node::ToolCall { signature, .. }
        | Node::ToolResult { signature, .. } => signature,
        Node::Reasoning { encrypted, .. } => encrypted,
        Node::Image { metadata, .. }
        | Node::Audio { metadata, .. }
        | Node::File { metadata, .. } => &mut metadata.signature,
        _ => return false,
    };
    if target.is_some() {
        return false;
    }
    *target = Some(signature.clone());
    true
}

fn signature_delta(node: &Node) -> Option<NodeDelta> {
    match node {
        Node::Text { signature, .. } => Some(NodeDelta::Text {
            content: String::new(),
            signature: signature.clone(),
            citations: Vec::new(),
            logprobs: None,
        }),
        Node::Reasoning {
            encrypted,
            metadata,
            ..
        } => Some(NodeDelta::Reasoning {
            metadata: metadata.clone(),
            content: None,
            encrypted: encrypted.clone(),
            summary: None,
            source: None,
        }),
        _ => None,
    }
}

fn frame_text_scores(
    candidate: &Map<String, Value>,
    parts: &[Value],
) -> HashMap<usize, Vec<crate::urp::TokenLogprob>> {
    if !candidate.contains_key("logprobsResult") {
        return HashMap::new();
    }
    let mut indices = Vec::new();
    let mut nodes = Vec::new();
    for (index, part) in parts.iter().enumerate() {
        if part.get("thought").and_then(Value::as_bool) != Some(true) {
            if let Some(text) = part.get("text").and_then(Value::as_str) {
                indices.push(index);
                nodes.push(Node::assistant_text(text));
            }
        }
    }
    crate::urp::logprobs::attach_gemini(candidate, &mut nodes);
    indices
        .into_iter()
        .zip(nodes)
        .filter_map(|(index, node)| match node {
            Node::Text {
                logprobs: Some(scores),
                ..
            } => Some((index, scores)),
            _ => None,
        })
        .collect()
}

pub(crate) fn append_fragment(current: &mut Node, next: &mut Node) -> bool {
    if !node_extra(current).is_empty() || !node_extra(next).is_empty() {
        return false;
    }
    match (current, next) {
        (
            Node::Text {
                content,
                logprobs,
                signature: None,
                citations,
                ..
            },
            Node::Text {
                content: fragment,
                logprobs: fragment_scores,
                signature: None,
                citations: next_citations,
                ..
            },
        ) if citations.is_empty() && next_citations.is_empty() => {
            if !content.is_empty() && crate::urp::logprobs::valid(logprobs, content).is_none() {
                *fragment_scores = None;
            }
            crate::urp::logprobs::append(logprobs, fragment_scores);
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

fn merge_candidate_metadata(
    stored: &mut Map<String, Value>,
    candidate: &Map<String, Value>,
    output: &[Node],
) {
    for (key, value) in candidate {
        if key == "content" {
            continue;
        }
        if key != "logprobsResult" {
            stored.insert(key.clone(), value.clone());
            continue;
        }
        let text: String = output
            .iter()
            .filter_map(|node| match node {
                Node::Text { content, .. } => Some(content.as_str()),
                _ => None,
            })
            .collect();
        let tokens: String = value
            .get("chosenCandidates")
            .and_then(Value::as_array)
            .into_iter()
            .flatten()
            .filter_map(|score| score.get("token").and_then(Value::as_str))
            .collect();
        if tokens == text || !stored.contains_key(key) {
            stored.insert(key.clone(), value.clone());
        } else if let (Some(previous), Some(next)) = (
            stored.get_mut(key).and_then(Value::as_object_mut),
            value.as_object(),
        ) {
            for (field, value) in next {
                if matches!(field.as_str(), "chosenCandidates" | "topCandidates") {
                    if let Some(values) = value.as_array() {
                        if let Some(stored) = previous
                            .entry(field.clone())
                            .or_insert_with(|| Value::Array(Vec::new()))
                            .as_array_mut()
                        {
                            stored.extend(values.iter().cloned());
                        }
                    }
                } else {
                    previous.insert(field.clone(), value.clone());
                }
            }
        }
    }
}

fn initial_delta(node: &Node) -> Option<NodeDelta> {
    match node {
        Node::Text {
            logprobs,
            signature,
            citations,
            content,
            ..
        } => (!content.is_empty()
            || signature.is_some()
            || !citations.is_empty()
            || logprobs.is_some())
        .then(|| NodeDelta::Text {
            logprobs: logprobs.clone(),
            signature: signature.clone(),
            citations: citations.clone(),
            content: content.clone(),
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
