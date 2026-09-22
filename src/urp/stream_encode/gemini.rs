use crate::error::{AppError, AppResult};
use crate::urp::encode::gemini::{
    encode_request_node_part, encode_response_checked, encode_response_node_parts,
    is_prompt_block_response,
};
use crate::urp::{
    FinishReason, Node, NodeDelta, NodeHeader, ResponseStatus, UrpResponse, UrpStreamEvent,
};
use axum::http::StatusCode;
use axum::response::sse::Event;
use serde_json::{Value, json};
use std::collections::{BTreeMap, BTreeSet, HashMap};
use tokio::sync::mpsc;

#[derive(Default)]
struct PendingNode {
    partial: Option<Node>,
    complete: Option<Node>,
}

/// Emits ordered text deltas and reconciles final nodes without repeating emitted content.
/// Gemini cannot retract an emitted Part. Conflicting terminal replacements return an error.
pub struct GeminiStreamEncoder {
    id: String,
    model: String,
    next_index: u32,
    pending: BTreeMap<u32, PendingNode>,
    completed: BTreeSet<u32>,
    emitted: BTreeMap<u32, Vec<Value>>,
    last_wire_fragment: Option<(bool, u32)>,
    merged_wire_nodes: BTreeSet<u32>,
    terminal: bool,
}

impl GeminiStreamEncoder {
    pub fn new(model: &str) -> Self {
        Self {
            id: String::new(),
            model: model.into(),
            next_index: 0,
            pending: BTreeMap::new(),
            completed: BTreeSet::new(),
            emitted: BTreeMap::new(),
            last_wire_fragment: None,
            merged_wire_nodes: BTreeSet::new(),
            terminal: false,
        }
    }

    pub fn push_event(&mut self, event: UrpStreamEvent) -> Result<Vec<Value>, String> {
        if self.terminal {
            return Ok(Vec::new());
        }
        match event {
            UrpStreamEvent::ResponseStart { id, .. } => {
                self.id = id;
                Ok(Vec::new())
            }
            UrpStreamEvent::NodeStart {
                node_index,
                header,
                extra_body,
            } => {
                if self.pending.contains_key(&node_index) || self.completed.contains(&node_index) {
                    return Err("Gemini stream received a duplicate node start".into());
                }
                self.pending.entry(node_index).or_default().partial =
                    partial_node(header, extra_body);
                Ok(Vec::new())
            }
            UrpStreamEvent::NodeDelta {
                node_index,
                delta,
                extra_body,
                ..
            } => {
                if self.completed.contains(&node_index) || !self.pending.contains_key(&node_index) {
                    return Err("Gemini stream delta requires an open canonical node".into());
                }
                if let Some(node) = self
                    .pending
                    .get_mut(&node_index)
                    .and_then(|slot| slot.partial.as_mut())
                {
                    apply_delta(node, delta);
                    node.extra_body_mut().extend(extra_body);
                }
                if node_index < self.next_index {
                    let node = self
                        .pending
                        .get(&node_index)
                        .and_then(|slot| slot.partial.clone());
                    return node
                        .as_ref()
                        .map(|node| self.emit_node(node_index, node))
                        .transpose()
                        .map(|frame| frame.flatten().into_iter().collect());
                }
                self.flush_available()
            }
            UrpStreamEvent::NodeDone {
                node_index, node, ..
            } => {
                if !self.completed.insert(node_index) {
                    return Err("Gemini stream received a duplicate node completion".into());
                }
                if node_index < self.next_index {
                    return self
                        .emit_node(node_index, &node)
                        .map(|frame| frame.into_iter().collect());
                }
                self.pending.entry(node_index).or_default().complete = Some(node);
                self.flush_available()
            }
            UrpStreamEvent::ResponseDone {
                outcome,
                mut finish_reason,
                usage,
                output,
                extra_body,
            } => {
                if let Some(outcome) = &outcome {
                    if matches!(
                        outcome.status,
                        ResponseStatus::InProgress | ResponseStatus::Queued
                    ) {
                        return Err(
                            "Gemini terminal output cannot have an active response outcome".into(),
                        );
                    }
                }
                if finish_reason.is_none()
                    && outcome
                        .as_ref()
                        .is_none_or(|outcome| outcome.status == ResponseStatus::Completed)
                {
                    finish_reason = Some(FinishReason::Stop);
                }
                let response = UrpResponse {
                    outcome,
                    id: self.id.clone(),
                    model: self.model.clone(),
                    created_at: None,
                    output,
                    finish_reason,
                    usage,
                    extra_body,
                };
                let mut terminal = encode_response_checked(&response, &self.model)?;
                if terminal.get("error").is_some() {
                    self.terminal = true;
                    return Ok(vec![terminal]);
                }
                if self
                    .emitted
                    .keys()
                    .any(|index| *index as usize >= response.output.len())
                {
                    return Err("Gemini cannot delete a Part that was already emitted".into());
                }
                let blocked = is_prompt_block_response(&response);
                if blocked && !self.emitted.is_empty() {
                    return Err("Gemini cannot retract emitted content for a prompt block".into());
                }
                let mut frames = Vec::new();
                if !blocked {
                    for (index, node) in response.output.iter().enumerate() {
                        if let Some(frame) = self.emit_node(index as u32, node)? {
                            frames.push(frame);
                        }
                    }
                }
                if let Some(candidate) = terminal["candidates"]
                    .as_array_mut()
                    .and_then(|values| values.first_mut())
                    .and_then(Value::as_object_mut)
                {
                    rebase_terminal_grounding(candidate, &response.output, &self.emitted)?;
                    candidate.remove("content");
                }
                frames.push(terminal);
                self.terminal = true;
                Ok(frames)
            }
            UrpStreamEvent::Error {
                code,
                message,
                extra_body,
            } => {
                let mut error = serde_json::Map::new();
                for (key, value) in extra_body {
                    if !key.starts_with("_monoize_")
                        && !matches!(key.as_str(), "code" | "message" | "status")
                    {
                        error.insert(key, value);
                    }
                }
                if let Some(code) = code {
                    error.insert("code".into(), json!(code));
                }
                error.insert("message".into(), json!(message));
                self.terminal = true;
                Ok(vec![json!({"error":error})])
            }
            _ => Ok(Vec::new()),
        }
    }

    fn flush_available(&mut self) -> Result<Vec<Value>, String> {
        let mut frames = Vec::new();
        loop {
            let index = self.next_index;
            let Some(slot) = self.pending.get(&index) else {
                break;
            };
            let complete = slot.complete.is_some();
            let node = slot.complete.as_ref().or(slot.partial.as_ref()).cloned();
            if let Some(node) = node {
                let live = self.emitted.contains_key(&index) || live_text(&node);
                if complete || live {
                    if !matches!(node, Node::Refusal { .. }) {
                        frames.extend(self.emit_node(index, &node)?);
                    }
                }
            }
            if complete {
                self.next_index += 1;
                continue;
            }
            // A later content event establishes a wire boundary. Earlier metadata may still finish later.
            let next_ready = self.pending.get(&(index + 1)).is_some_and(|next| {
                next.complete.is_some() || next.partial.as_ref().is_some_and(live_text)
            });
            if self.emitted.contains_key(&index) && next_ready {
                self.next_index += 1;
            } else {
                break;
            }
        }
        Ok(frames)
    }

    fn emit_node(&mut self, index: u32, node: &Node) -> Result<Option<Value>, String> {
        let parts = encode_response_node_parts(node)?;
        if parts.is_empty() {
            return if self.emitted.contains_key(&index) {
                Err("Gemini cannot delete a Part that was already emitted".into())
            } else {
                Ok(None)
            };
        }
        let mut fragments = parts.clone();
        if let Some(previous) = self.emitted.get(&index) {
            if *previous == parts {
                return Ok(None);
            }
            if self.emitted.keys().any(|emitted| *emitted > index) {
                return Err("Gemini cannot modify a Part before a later emitted Part".into());
            }
            if previous.len() != 1 || parts.len() != 1 {
                return Err("Gemini cannot replace Parts that were already emitted".into());
            }
            let previous = &previous[0];
            let part = &parts[0];
            if previous.get("thoughtSignature").is_none()
                && part.get("thoughtSignature").is_some()
                && self.merged_wire_nodes.contains(&index)
            {
                return Err("Gemini cannot attach a late signature after its text merged with an earlier Part".into());
            }
            let prefix = previous.get("text").and_then(Value::as_str);
            let text = part.get("text").and_then(Value::as_str);
            let mut old_shape = previous.clone();
            let mut new_shape = part.clone();
            for shape in [&mut old_shape, &mut new_shape] {
                if let Some(obj) = shape.as_object_mut() {
                    obj.remove("text");
                    obj.remove("thoughtSignature");
                }
            }
            if let (Some(prefix), Some(text)) = (prefix, text) {
                if old_shape == new_shape
                    && previous.get("thoughtSignature").is_none()
                    && text.starts_with(prefix)
                {
                    fragments[0]["text"] = json!(&text[prefix.len()..]);
                } else {
                    return Err("Gemini cannot replace a Part that was already emitted".into());
                }
            } else {
                return Err("Gemini cannot replace a Part that was already emitted".into());
            }
        } else if self.emitted.keys().any(|emitted| *emitted > index) {
            return Err("Gemini cannot insert a Part before a later emitted Part".into());
        }
        self.emitted.insert(index, parts);
        let mut content = json!({"role":"model","parts":fragments});
        if let Some((_, _, extra)) = encode_request_node_part(node) {
            crate::urp::encode::merge_extra(content.as_object_mut().unwrap(), &extra);
        }
        let has_envelope_extra = content
            .as_object()
            .unwrap()
            .keys()
            .any(|key| !matches!(key.as_str(), "role" | "parts"));
        for (part_index, part) in content["parts"].as_array().unwrap().iter().enumerate() {
            let kind = (!has_envelope_extra)
                .then(|| plain_fragment_kind(part))
                .flatten();
            if let Some(kind) = kind {
                if part_index == 0
                    && self
                        .last_wire_fragment
                        .is_some_and(|(previous, owner)| previous == kind && owner != index)
                {
                    self.merged_wire_nodes.insert(index);
                }
            }
            self.last_wire_fragment = kind.map(|kind| (kind, index));
        }
        Ok(Some(json!({"responseId":self.id,"modelVersion":self.model,
            "candidates":[{"index":0,"content":content}]})))
    }
}

fn plain_fragment_kind(part: &Value) -> Option<bool> {
    let part = part.as_object()?;
    part.get("text")?.as_str()?;
    let thought = part.get("thought").and_then(Value::as_bool) == Some(true);
    part.keys()
        .all(|key| key == "text" || (thought && key == "thought"))
        .then_some(thought)
}

fn rebase_terminal_grounding(
    candidate: &mut serde_json::Map<String, Value>,
    output: &[Node],
    emitted: &BTreeMap<u32, Vec<Value>>,
) -> Result<(), String> {
    use crate::urp::decode::gemini::{GEMINI_CONTENT_EXTRA_KEY, decode_stream_part};
    use crate::urp::stream_decode::gemini::{
        append_fragment, bind_signature, continuation_signature,
    };

    let Some(supports) = candidate
        .get_mut("groundingMetadata")
        .and_then(|metadata| metadata.get_mut("groundingSupports"))
        .and_then(Value::as_array_mut)
    else {
        return Ok(());
    };
    let mut positions = Vec::new();
    let mut last = None::<Node>;
    let mut logical_parts = 0usize;
    for (index, parts) in emitted {
        let envelope = encode_request_node_part(&output[*index as usize])
            .map(|(_, _, extra)| {
                extra
                    .into_iter()
                    .filter(|(key, _)| {
                        !key.starts_with("_monoize_") && !matches!(key.as_str(), "role" | "parts")
                    })
                    .collect::<HashMap<_, _>>()
            })
            .unwrap_or_default();
        for (part_index, part) in parts.iter().enumerate() {
            if let Some(signature) = continuation_signature(part) {
                if last
                    .as_mut()
                    .is_some_and(|last| bind_signature(last, part, signature))
                {
                    positions.push((logical_parts - 1, 0u64));
                    continue;
                }
            }
            let mut position = None;
            for (offset, mut node) in decode_stream_part(part)?.into_iter().enumerate() {
                if part_index == 0 && offset == 0 && !envelope.is_empty() {
                    node.extra_body_mut()
                        .insert(GEMINI_CONTENT_EXTRA_KEY.into(), json!(envelope));
                }
                let prefix = last.as_ref().map_or(0, |node| match node {
                    Node::Text { content, .. } => content.len() as u64,
                    _ => 0,
                });
                let merged = part_index == 0
                    && offset == 0
                    && last
                        .as_mut()
                        .is_some_and(|last| append_fragment(last, &mut node));
                if !merged {
                    last = Some(node);
                    logical_parts += 1;
                }
                position.get_or_insert((logical_parts - 1, if merged { prefix } else { 0 }));
            }
            positions.push(position.ok_or("Gemini grounding references an empty emitted Part")?);
        }
    }
    for support in supports {
        let Some(segment) = support.get_mut("segment").and_then(Value::as_object_mut) else {
            continue;
        };
        let native_part = segment
            .get("partIndex")
            .and_then(Value::as_u64)
            .unwrap_or(0) as usize;
        let (part, offset) = positions
            .get(native_part)
            .ok_or("Gemini grounding references a Part that was not emitted")?;
        let start = segment
            .get("startIndex")
            .and_then(Value::as_u64)
            .unwrap_or(0)
            .checked_add(*offset)
            .ok_or("Gemini grounding start index overflow")?;
        let end = segment
            .get("endIndex")
            .and_then(Value::as_u64)
            .and_then(|end| end.checked_add(*offset))
            .ok_or("Gemini grounding end index is invalid")?;
        segment.insert("partIndex".into(), Value::from(*part));
        segment.insert("startIndex".into(), Value::from(start));
        segment.insert("endIndex".into(), Value::from(end));
    }
    Ok(())
}

fn partial_node(header: NodeHeader, extra_body: HashMap<String, Value>) -> Option<Node> {
    match header {
        NodeHeader::Text {
            id,
            role,
            phase,
            signature,
            citations,
        } => Some(Node::Text {
            id,
            role,
            phase,
            signature,
            citations,
            content: String::new(),
            logprobs: None,
            extra_body,
        }),
        NodeHeader::Reasoning { id, metadata } => Some(Node::Reasoning {
            id,
            metadata,
            content: None,
            encrypted: None,
            summary: None,
            source: None,
            extra_body,
        }),
        _ => None,
    }
}

fn live_text(node: &Node) -> bool {
    match node {
        Node::Text {
            content,
            signature: None,
            ..
        } => !content.is_empty(),
        Node::Reasoning {
            content,
            summary,
            encrypted: None,
            ..
        } => content
            .as_ref()
            .or(summary.as_ref())
            .is_some_and(|text| !text.is_empty()),
        _ => false,
    }
}

fn apply_delta(node: &mut Node, delta: NodeDelta) {
    match (node, delta) {
        (
            Node::Text {
                content,
                signature,
                citations,
                logprobs,
                ..
            },
            NodeDelta::Text {
                content: fragment,
                signature: next_signature,
                citations: next_citations,
                logprobs: next_scores,
            },
        ) => {
            content.push_str(&fragment);
            if next_signature.is_some() {
                *signature = next_signature;
            }
            for citation in next_citations {
                if !citations.contains(&citation) {
                    citations.push(citation);
                }
            }
            crate::urp::logprobs::append(logprobs, &next_scores);
        }
        (
            Node::Reasoning {
                content,
                encrypted,
                summary,
                source,
                metadata,
                ..
            },
            NodeDelta::Reasoning {
                content: fragment,
                encrypted: next_encrypted,
                summary: next_summary,
                source: next_source,
                metadata: next_metadata,
            },
        ) => {
            if let Some(fragment) = fragment {
                content.get_or_insert_default().push_str(&fragment);
            }
            if let Some(fragment) = next_summary {
                summary.get_or_insert_default().push_str(&fragment);
            }
            if next_encrypted.is_some() {
                *encrypted = next_encrypted;
            }
            if next_source.is_some() {
                *source = next_source;
            }
            *metadata = next_metadata;
        }
        _ => {}
    }
}

/// Writes Gemini data-only SSE frames. Completion requires a canonical terminal event.
pub async fn encode_urp_stream_as_gemini(
    mut rx: mpsc::Receiver<UrpStreamEvent>,
    tx: mpsc::Sender<Event>,
    model: &str,
) -> AppResult<()> {
    let mut encoder = GeminiStreamEncoder::new(model);
    while let Some(event) = rx.recv().await {
        let frames = match encoder.push_event(event) {
            Ok(frames) => frames,
            Err(message) => {
                let body = crate::urp::media::error_body(&message);
                crate::urp::stream_helpers::send_plain_sse_data(&tx, body.to_string()).await?;
                return Err(AppError::new(
                    StatusCode::BAD_GATEWAY,
                    "stream_encode_failed",
                    message,
                )
                .with_downstream_stream_terminal_sent(!tx.is_closed()));
            }
        };
        for frame in frames {
            crate::urp::stream_helpers::send_plain_sse_data(&tx, frame.to_string()).await?;
        }
    }
    if !encoder.terminal {
        let message = "Gemini stream has no canonical terminal event";
        crate::urp::stream_helpers::send_plain_sse_data(
            &tx,
            crate::urp::media::error_body(message).to_string(),
        )
        .await?;
        return Err(
            AppError::new(StatusCode::BAD_GATEWAY, "stream_encode_failed", message)
                .with_downstream_stream_terminal_sent(!tx.is_closed()),
        );
    }
    Ok(())
}
