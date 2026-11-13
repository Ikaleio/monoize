//! Cross-frame state of one upstream Gemini stream and the handler of each frame member
//! (GEM-8, GEM-10, PG5b, PG5b1, PG5d).

use super::candidate::{CandidateAccumulator, frame_text_scores};
use super::grounding::FrameParts;
use crate::error::{AppError, AppResult};
use crate::handlers::usage::{
    mark_stream_ttfb_if_needed, parse_usage_from_gemini_object,
    record_observed_upstream_response_model, record_stream_done_sentinel,
    record_stream_terminal_error, record_stream_terminal_event, record_stream_usage_if_present,
    record_visible_stream_event_delta,
};
use crate::handlers::{StreamRuntimeMetrics, StreamTerminalError, UrpRequest as HandlerUrpRequest};
use crate::urp::decode::gemini::{
    append_fragment, content_parts, continuation_carrier, continuation_token, decode_part,
    is_signature_carrier, native_finish_reason, node_extra, parse_usage, project_response_node,
    prompt_block_reason, prompt_refusal, response_outcome, selected_candidate, text_bytes,
};
use crate::urp::decode::split_extra;
use crate::urp::{
    FinishReason, GEMINI_CONTENT_EXTRA_KEY, Node, NodeDelta, NodeHeader, OrdinaryRole,
    ResponseStatus, TerminalNode, TokenLogprob, ToolCallType, UrpStreamEvent, Usage,
};
use axum::http::StatusCode;
use serde_json::{Map, Value};
use std::collections::{HashMap, HashSet};
use std::sync::Arc;
use std::time::Instant;
use tokio::sync::{Mutex, mpsc};

/// What the driver loop does after one SSE data payload.
pub(super) enum Flow {
    Continue,
    /// The upstream sent the `[DONE]` sentinel.
    Done,
}

pub(super) struct GeminiStreamState<'a> {
    custom_tool_names: &'a HashSet<String>,
    request_model: &'a str,
    tx: mpsc::Sender<UrpStreamEvent>,
    runtime_metrics: Option<Arc<Mutex<StreamRuntimeMetrics>>>,
    started_at: Option<Instant>,
    response_id: String,
    response_started: bool,
    /// GEM-5: the candidate index selected in the first candidate-bearing frame.
    candidate_index: Option<u64>,
    /// Every announced node at its node index.
    output: Vec<Node>,
    /// `closed[i]`: node `i` already received its `NodeDone`.
    closed: Vec<bool>,
    /// Set by the native finish reason or a prompt block; content after it is ignored.
    finish_reason: Option<FinishReason>,
    /// Latest cumulative `usageMetadata` snapshot.
    usage: Option<Usage>,
    extra_body: HashMap<String, Value>,
    candidate: CandidateAccumulator,
}

impl<'a> GeminiStreamState<'a> {
    pub(super) fn new(
        urp: &'a HandlerUrpRequest,
        tx: mpsc::Sender<UrpStreamEvent>,
        started_at: Option<Instant>,
        runtime_metrics: Option<Arc<Mutex<StreamRuntimeMetrics>>>,
    ) -> Self {
        Self {
            custom_tool_names: &urp.messages_custom_tool_names,
            request_model: &urp.model,
            tx,
            runtime_metrics,
            started_at,
            response_id: format!("resp_{}", uuid::Uuid::new_v4()),
            response_started: false,
            candidate_index: None,
            output: Vec::new(),
            closed: Vec::new(),
            finish_reason: None,
            usage: None,
            extra_body: HashMap::new(),
            candidate: CandidateAccumulator::default(),
        }
    }

    /// A finish reason or prompt block established terminal completion (GEM-10).
    pub(super) fn completed(&self) -> bool {
        self.finish_reason.is_some()
    }

    async fn send(&self, event: UrpStreamEvent) {
        let _ = self.tx.send(event).await;
    }

    /// Sends an event that carries client-visible content and records it for stream metrics.
    async fn send_visible(&self, event: UrpStreamEvent) {
        record_visible_stream_event_delta(&self.runtime_metrics, &event).await;
        self.send(event).await;
    }

    pub(super) async fn on_sse_data(
        &mut self,
        data: crate::payload::sse::FrameData,
    ) -> AppResult<Flow> {
        mark_stream_ttfb_if_needed(&mut self.started_at, &self.runtime_metrics).await;
        if data.is_done() {
            record_stream_done_sentinel(&self.runtime_metrics).await;
            return Ok(Flow::Done);
        }
        let frame: Value = data.into_json().map_err(|(error, _)| decode_error(error))?;
        self.on_frame(&frame).await?;
        Ok(Flow::Continue)
    }

    async fn on_frame(&mut self, frame: &Value) -> AppResult<()> {
        if let Some(error) = frame.get("error").filter(|error| !error.is_null()) {
            return Err(self.native_error(error).await);
        }
        let obj = frame
            .as_object()
            .ok_or_else(|| decode_error("Gemini stream event must be an object".into()))?;
        if let Some(native) = obj.get("usageMetadata").and_then(Value::as_object) {
            self.usage = Some(parse_usage(native).map_err(decode_error)?);
        }
        record_stream_usage_if_present(
            &self.runtime_metrics,
            parse_usage_from_gemini_object(frame),
        )
        .await;
        if let Some(model) = obj.get("modelVersion").and_then(Value::as_str) {
            record_observed_upstream_response_model(&self.runtime_metrics, model, true).await;
        }
        self.extra_body.extend(split_extra(
            obj,
            &["candidates", "usageMetadata", "responseId", "modelVersion"],
        ));
        if !self.response_started {
            self.start_response(obj).await;
        }
        let candidate = self.select_candidate(frame);
        let mut frame_parts = FrameParts::new();
        if self.finish_reason.is_none() {
            if let Some(candidate) = candidate
                && let Some(content) = candidate.get("content").and_then(Value::as_object)
            {
                frame_parts = self.on_content(candidate, content).await?;
            }
            if let Some(reason) = prompt_block_reason(frame) {
                self.announce(prompt_refusal(reason)).await;
                self.finish_reason = Some(FinishReason::ContentFilter);
            }
            if self.finish_reason.is_none() {
                self.finish_reason = candidate.and_then(native_finish_reason);
            }
        }
        // Metadata after the finish reason still updates the terminal snapshot (GEM-10).
        if let Some(candidate) = candidate {
            self.candidate
                .ingest(candidate, &frame_parts, &self.output, &mut self.extra_body);
        }
        Ok(())
    }

    async fn start_response(&mut self, obj: &Map<String, Value>) {
        if let Some(id) = obj.get("responseId").and_then(Value::as_str) {
            self.response_id = id.to_string();
        }
        self.response_started = true;
        self.send(UrpStreamEvent::ResponseStart {
            usage: self.usage.clone(),
            id: self.response_id.clone(),
            model: obj
                .get("modelVersion")
                .and_then(Value::as_str)
                .unwrap_or(self.request_model)
                .to_string(),
            // GenerateContentResponse carries no creation time.
            created_at: None,
            extra_body: self.extra_body.clone(),
        })
        .await;
    }

    fn select_candidate<'v>(&mut self, frame: &'v Value) -> Option<&'v Map<String, Value>> {
        let index_of = |candidate: &Map<String, Value>| {
            candidate.get("index").and_then(Value::as_u64).unwrap_or(0)
        };
        match self.candidate_index {
            Some(selected) => frame
                .get("candidates")
                .and_then(Value::as_array)
                .into_iter()
                .flatten()
                .filter_map(Value::as_object)
                .find(|candidate| index_of(candidate) == selected),
            None => {
                let candidate = selected_candidate(frame)?;
                self.candidate_index = Some(index_of(candidate));
                Some(candidate)
            }
        }
    }

    /// Decodes the Parts of one frame and returns where each native Part landed in the output.
    async fn on_content(
        &mut self,
        candidate: &Map<String, Value>,
        content: &Map<String, Value>,
    ) -> AppResult<FrameParts> {
        let mut frame_parts = FrameParts::new();
        let Some(parts) = content.get("parts").map(content_parts) else {
            return Ok(frame_parts);
        };
        let mut content_extra =
            Some(split_extra(content, &["role", "parts"])).filter(|extra| !extra.is_empty());
        let frame_scores = frame_text_scores(candidate, parts);
        for (part_index, part) in parts.iter().enumerate() {
            let mut node = decode_part(part, OrdinaryRole::Assistant).map_err(decode_error)?;
            restore_custom_bridge_call(&mut node, self.custom_tool_names);
            if let Node::Text { logprobs, .. } = &mut node
                && self.scores_aligned()
            {
                *logprobs = frame_scores.get(&part_index).cloned();
            }
            if let Some(extra) = content_extra.take() {
                node.extra_body_mut().insert(
                    GEMINI_CONTENT_EXTRA_KEY.into(),
                    Value::Object(extra.into_iter().collect()),
                );
            }
            // Only the first Part of a frame can continue the previous frame's fragment.
            if part_index == 0
                && let Some(prefix) = self.append_to_last(&mut node)
            {
                let node_index = self.output.len() - 1;
                frame_parts.insert(part_index, (node_index, prefix));
                if let Some(delta) = initial_delta(&node) {
                    self.send_visible(UrpStreamEvent::NodeDelta {
                        node_index: node_index as u32,
                        delta,
                        usage: None,
                        extra_body: node_extra(&node).clone(),
                    })
                    .await;
                }
                continue;
            }
            let text_offset = text_bytes(&self.output);
            for (position, node) in project_response_node(node, text_offset)
                .into_iter()
                .enumerate()
            {
                let node_index = self.start_node(node).await;
                if position == 0 {
                    frame_parts.entry(part_index).or_insert((node_index, 0));
                }
            }
        }
        Ok(frame_parts)
    }

    /// Frame scores attach only while every earlier non-empty Text node still holds valid scores,
    /// so one score sequence never spans unscored text.
    fn scores_aligned(&self) -> bool {
        self.output.iter().all(|node| match node {
            Node::Text {
                content, logprobs, ..
            } => content.is_empty() || crate::urp::logprobs::valid(logprobs, content).is_some(),
            _ => true,
        })
    }

    /// PG5b: appends `fragment` to the last node when both are mergeable fragments. Returns the
    /// text bytes the last node held before the fragment.
    fn append_to_last(&mut self, fragment: &mut Node) -> Option<u64> {
        let last = self.output.last_mut()?;
        let prefix = match &*last {
            Node::Text { content, .. } => content.len() as u64,
            _ => 0,
        };
        append_fragment(last, fragment).then_some(prefix)
    }

    /// PG5b1: starting a node completes every earlier open node, except that an open Text node
    /// stays open for a following signature carrier, so late citations can still reach it.
    async fn start_node(&mut self, node: Node) -> usize {
        let carrier = is_signature_carrier(&node);
        for index in 0..self.output.len() {
            if self.closed[index] || (carrier && matches!(self.output[index], Node::Text { .. })) {
                continue;
            }
            self.closed[index] = true;
            let prior = self.output[index].clone();
            let extra_body = node_extra(&prior).clone();
            self.send(UrpStreamEvent::NodeDone {
                node_index: index as u32,
                node: prior,
                usage: None,
                extra_body,
            })
            .await;
        }
        let node_index = self.output.len();
        let extra_body = node_extra(&node).clone();
        self.send_visible(UrpStreamEvent::NodeStart {
            node_index: node_index as u32,
            header: node_header(&node),
            extra_body: extra_body.clone(),
        })
        .await;
        let delta = initial_delta(&node);
        self.output.push(node);
        self.closed.push(false);
        if let Some(delta) = delta {
            self.send_visible(UrpStreamEvent::NodeDelta {
                node_index: node_index as u32,
                delta,
                usage: None,
                extra_body,
            })
            .await;
        }
        node_index
    }

    /// Announces a node that the decoder synthesizes at a boundary (prompt refusal, continuation
    /// carrier). Earlier open nodes stay open until the terminal boundary.
    async fn announce(&mut self, node: Node) {
        let node_index = self.output.len() as u32;
        self.send(UrpStreamEvent::NodeStart {
            node_index,
            header: node_header(&node),
            extra_body: HashMap::new(),
        })
        .await;
        if let Some(delta) = initial_delta(&node) {
            self.send(UrpStreamEvent::NodeDelta {
                node_index,
                delta,
                usage: None,
                extra_body: HashMap::new(),
            })
            .await;
        }
        self.output.push(node);
        self.closed.push(false);
    }

    /// Emits the terminal snapshot. Without a finish reason or prompt block the stream fails
    /// with `upstream_stream_missing_terminal` (PG5d).
    pub(super) async fn finish(mut self) -> AppResult<()> {
        let Some(mut finish_reason) = self.finish_reason else {
            return Err(AppError::new(
                StatusCode::BAD_GATEWAY,
                "upstream_stream_missing_terminal",
                "Gemini stream ended without a finishReason or prompt block",
            ));
        };
        if finish_reason == FinishReason::Stop
            && self
                .output
                .iter()
                .any(|node| matches!(node, Node::ToolCall { .. }))
        {
            finish_reason = FinishReason::ToolCalls;
        }
        self.candidate
            .attach_citations(&mut self.output, &mut self.extra_body);
        let streamed_scores: Vec<Vec<TokenLogprob>> = self
            .output
            .iter()
            .map(|node| match node {
                Node::Text { logprobs, .. } => logprobs.clone().unwrap_or_default(),
                _ => Vec::new(),
            })
            .collect();
        self.candidate.attach_logprobs(&mut self.output);
        self.send_late_metadata(&streamed_scores).await;

        let outcome = response_outcome(Some(self.candidate.metadata()), Some(finish_reason));
        if let Some(carrier) =
            continuation_token(self.candidate.metadata()).map(continuation_carrier)
        {
            self.announce(carrier).await;
        }
        let terminal_event = if outcome
            .as_ref()
            .is_some_and(|outcome| outcome.status == ResponseStatus::Failed)
        {
            "response.failed"
        } else {
            "response.completed"
        };
        for index in 0..self.output.len() {
            if self.closed[index] {
                continue;
            }
            let node = self.output[index].clone();
            let extra_body = node_extra(&node).clone();
            self.send(UrpStreamEvent::NodeDone {
                node_index: index as u32,
                node,
                usage: None,
                extra_body,
            })
            .await;
        }
        let output = (0u32..)
            .zip(std::mem::take(&mut self.output))
            .map(|(index, node)| TerminalNode::streamed(index, node))
            .collect();
        let done = UrpStreamEvent::ResponseDone {
            outcome,
            finish_reason: Some(finish_reason),
            usage: self.usage.take(),
            output,
            extra_body: std::mem::take(&mut self.extra_body),
        };
        self.send(done).await;
        record_stream_terminal_event(&self.runtime_metrics, terminal_event, None).await;
        Ok(())
    }

    /// GEM-8: late citations and token scores reach still-open Text nodes as an empty-text
    /// delta; scores already streamed are not repeated. Closed nodes receive them only through
    /// `ResponseDone.output`.
    async fn send_late_metadata(&self, streamed_scores: &[Vec<TokenLogprob>]) {
        for (index, node) in self.output.iter().enumerate() {
            if self.closed[index] {
                continue;
            }
            let Node::Text {
                citations,
                logprobs,
                ..
            } = node
            else {
                continue;
            };
            let streamed = &streamed_scores[index];
            let scores = logprobs
                .as_ref()
                .filter(|scores| scores.starts_with(streamed))
                .map(|scores| scores[streamed.len()..].to_vec())
                .filter(|scores| !scores.is_empty());
            if citations.is_empty() && scores.is_none() {
                continue;
            }
            self.send(UrpStreamEvent::NodeDelta {
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

    /// Emits the URP error of a native error object and returns the stream failure.
    async fn native_error(&self, error: &Value) -> AppError {
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
            .map(|error| split_extra(error, &["code", "message", "status"]))
            .unwrap_or_default();
        let sent = self
            .tx
            .send(UrpStreamEvent::Error {
                code: Some(code.clone()),
                message: message.clone(),
                extra_body,
            })
            .await
            .is_ok();
        record_stream_terminal_error(
            &self.runtime_metrics,
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
        AppError::new(StatusCode::BAD_GATEWAY, code, message)
            .with_downstream_stream_terminal_sent(sent)
    }
}

pub(super) fn decode_error(message: String) -> AppError {
    AppError::new(
        StatusCode::BAD_GATEWAY,
        "upstream_stream_decode_failed",
        message,
    )
}

/// TCI7f: restores a custom tool call that was bridged through a Gemini function declaration.
fn restore_custom_bridge_call(node: &mut Node, names: &HashSet<String>) {
    let Node::ToolCall {
        tool_type,
        name,
        arguments,
        ..
    } = node
    else {
        return;
    };
    if *tool_type != ToolCallType::Function || !names.contains(name) {
        return;
    }
    let Ok(Value::Object(object)) = serde_json::from_str::<Value>(arguments) else {
        return;
    };
    let Some(input) = object.get("input").and_then(Value::as_str) else {
        return;
    };
    *tool_type = ToolCallType::Custom;
    *arguments = input.to_string();
}

/// The `NodeStart` header of a decoded node. A Gemini call signature arrives with its call, so
/// it travels on the header.
fn node_header(node: &Node) -> NodeHeader {
    let mut header = NodeHeader::from(node);
    if let (
        NodeHeader::ToolCall { signature, .. },
        Node::ToolCall {
            signature: Some(call_signature),
            ..
        },
    ) = (&mut header, node)
    {
        *signature = Some(call_signature.clone());
    }
    header
}

/// The whole payload of a decoded node as its first delta. Gemini Parts arrive complete, so the
/// first delta of a node carries everything the Part held.
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
