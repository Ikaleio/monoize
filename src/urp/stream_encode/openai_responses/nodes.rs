//! Node lifecycles: output-item and content-part allocation (RECON-6, RESP-7) and the
//! fragments each `NodeDone` still owes the wire (STR-15).

use super::items::{self, done_content_part, start_content_part};
use super::media::{
    image_generation_call_downstream_event, image_generation_call_event_payload,
    image_generation_call_image_event_payload,
};
use super::reasoning::{self, ReasoningSummaryLifecycle};
use super::wire::{Wire, with_delta_wire_fields};
use super::{ResponsesStreamEncoder, item_id};
use crate::error::AppResult;
use crate::urp::grouping::{GroupKey, Zone};
use crate::urp::stream_helpers::{
    responses_text_delta_payload, sanitize_responses_output_item_for_frame_limit,
};
use crate::urp::{
    Citation, Node, NodeDelta, NodeHeader, OrdinaryRole, ProviderProtocol,
    RESPONSES_CONTENT_PART_SHAPE_KEY, RESPONSES_IMAGE_GENERATION_CALL_EXTRA_KEY,
    RESPONSES_TEXT_DONE_PHASE_EXTRA_KEY, TokenLogprob, ToolCallType, tool_call_arguments_for_wire,
};
use serde_json::{Value, json};
use std::collections::HashMap;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) enum NodeKind {
    /// A content part of a `message` item.
    Part,
    Reasoning,
    ToolCall,
    ToolResult,
    /// A native `image_generation_call` item.
    Image,
    /// A Responses-origin opaque output item.
    Provider,
    /// A control node or a foreign provider item: no wire lifecycle (CTL-5, SACC-9).
    Silent,
}

impl NodeKind {
    fn of(header: &NodeHeader, extra_body: &HashMap<String, Value>) -> Self {
        match header {
            NodeHeader::NextDownstreamEnvelopeExtra => Self::Silent,
            NodeHeader::ProviderItem {
                origin_protocol, ..
            } if *origin_protocol != ProviderProtocol::Responses => Self::Silent,
            NodeHeader::ProviderItem { .. }
                if extra_body.contains_key(RESPONSES_CONTENT_PART_SHAPE_KEY) =>
            {
                Self::Part
            }
            NodeHeader::ProviderItem { .. } => Self::Provider,
            NodeHeader::Image { .. }
                if extra_body.contains_key(RESPONSES_IMAGE_GENERATION_CALL_EXTRA_KEY) =>
            {
                Self::Image
            }
            NodeHeader::Reasoning { .. } => Self::Reasoning,
            NodeHeader::ToolCall { .. } => Self::ToolCall,
            NodeHeader::ToolResult { .. } => Self::ToolResult,
            NodeHeader::Text { .. }
            | NodeHeader::Refusal { .. }
            | NodeHeader::Image { .. }
            | NodeHeader::Audio { .. }
            | NodeHeader::File { .. } => Self::Part,
        }
    }
}

/// The owned `GroupKey` of the last part of the open message.
struct PartKey {
    role: OrdinaryRole,
    zone: Zone,
    phase: Option<String>,
}

impl PartKey {
    fn key(&self) -> GroupKey<'_> {
        GroupKey {
            role: self.role,
            zone: self.zone,
            phase: self.phase.as_deref(),
        }
    }
}

pub(super) struct OpenMessage {
    pub(super) output_index: usize,
    last: PartKey,
}

impl OpenMessage {
    /// RECON-6 continuity plus the phase rule that splits `message` items inside one envelope,
    /// as in the non-stream encoder (RECON-7).
    fn accepts(&self, next: GroupKey<'_>) -> bool {
        let last = self.last.key();
        last.continues_with(next) && last.phase == next.phase
    }
}

/// What earlier deltas of one node carried to the wire (STR-15).
#[derive(Default)]
pub(super) struct Streamed {
    /// Text, refusal, reasoning text, or wire arguments.
    pub(super) text: String,
    pub(super) annotations: usize,
    pub(super) reasoning_part_added: bool,
    pub(super) summary: ReasoningSummaryLifecycle,
    pub(super) encrypted: Option<Value>,
    pub(super) source: Option<String>,
}

pub(super) struct NodeState {
    pub(super) kind: NodeKind,
    pub(super) header: NodeHeader,
    pub(super) start_extra: HashMap<String, Value>,
    /// `None` until the node owns an output item: a reasoning item without payload (PR2g)
    /// or a silent node.
    pub(super) output_index: Option<usize>,
    pub(super) content_index: u32,
    /// The wire lifecycle began: the content part or the own item was added.
    pub(super) visible: bool,
    pub(super) closed: bool,
    deferred_stub: Option<Value>,
    pub(super) streamed: Streamed,
}

/// One downstream output item, addressed by its `output_index`.
pub(super) struct OutputItem {
    /// The item stub, then the accumulated message, then the emitted done item.
    pub(super) item: Value,
    pub(super) added: bool,
    pub(super) done: bool,
    /// Message parts allocated and not yet done.
    open_parts: u32,
    next_content_index: u32,
}

/// Wire coordinates of one node's sub-lifecycle.
struct Coordinates {
    item_id: String,
    output_index: usize,
    content_index: u32,
}

fn push_item(items: &mut Vec<OutputItem>, item: Value) -> usize {
    items.push(OutputItem {
        item,
        added: false,
        done: false,
        open_parts: 0,
        next_content_index: 0,
    });
    items.len() - 1
}

impl ResponsesStreamEncoder {
    pub(super) async fn on_node_start(
        &mut self,
        node_index: u32,
        header: NodeHeader,
        extra_body: HashMap<String, Value>,
    ) -> AppResult<()> {
        let kind = NodeKind::of(&header, &extra_body);
        let part_key = GroupKey::of_header(&header, kind == NodeKind::Image)
            .filter(|_| kind == NodeKind::Part)
            .map(|key| PartKey {
                role: key.role,
                zone: key.zone,
                phase: key.phase.map(str::to_string),
            });
        let joins = matches!(
            (&self.open_message, &part_key),
            (Some(open), Some(key)) if open.accepts(key.key())
        );
        if !joins {
            // Every other node, control nodes included, ends the open message (RECON-6).
            self.close_open_message().await?;
        }
        if matches!(header, NodeHeader::NextDownstreamEnvelopeExtra) {
            items::merge_extra_preserving(&mut self.pending_envelope_extra, &extra_body);
        }
        let mut state = NodeState {
            kind,
            header,
            start_extra: extra_body,
            output_index: None,
            content_index: 0,
            visible: false,
            closed: false,
            deferred_stub: None,
            streamed: Streamed::default(),
        };
        match (kind, part_key) {
            (NodeKind::Silent, _) => {}
            (NodeKind::Part, Some(last)) => self.start_part(&mut state, joins, last),
            _ => self.start_item(&mut state).await?,
        }
        self.nodes.insert(node_index, state);
        Ok(())
    }

    fn start_part(&mut self, state: &mut NodeState, joins: bool, last: PartKey) {
        let output_index = match &self.open_message {
            Some(open) if joins => open.output_index,
            _ => push_item(
                &mut self.items,
                items::item_stub(
                    NodeKind::Part,
                    &state.header,
                    &state.start_extra,
                    &HashMap::new(),
                ),
            ),
        };
        self.open_message = Some(OpenMessage { output_index, last });
        let item = &mut self.items[output_index];
        state.output_index = Some(output_index);
        state.content_index = item.next_content_index;
        item.next_content_index += 1;
        item.open_parts += 1;
    }

    async fn start_item(&mut self, state: &mut NodeState) -> AppResult<()> {
        let envelope_extra = std::mem::take(&mut self.pending_envelope_extra);
        let stub = items::item_stub(
            state.kind,
            &state.header,
            &state.start_extra,
            &envelope_extra,
        );
        let stub = reasoning::added_item_with_duration(stub, self.reasoning_duration_secs());
        if state.kind == NodeKind::Reasoning && !reasoning::item_has_payload(&stub) {
            state.deferred_stub = Some(stub);
            return Ok(());
        }
        let output_index = push_item(&mut self.items, stub);
        state.output_index = Some(output_index);
        state.visible = true;
        self.emit_added(output_index).await
    }

    /// PR2g: a reasoning item becomes visible with its first payload.
    async fn ensure_started(&mut self, node_index: u32) -> AppResult<()> {
        let duration = self.reasoning_duration_secs();
        let Some(state) = self.nodes.get_mut(&node_index) else {
            return Ok(());
        };
        let Some(stub) = state.deferred_stub.take() else {
            return Ok(());
        };
        let stub = reasoning::added_item_with_duration(stub, duration);
        let output_index = push_item(&mut self.items, stub);
        state.output_index = Some(output_index);
        state.visible = true;
        self.emit_added(output_index).await
    }

    async fn emit_added(&mut self, output_index: usize) -> AppResult<()> {
        let item = &mut self.items[output_index];
        item.added = true;
        let payload = json!({ "output_index": output_index, "item": item.item.clone() });
        self.wire.event("response.output_item.added", payload).await
    }

    /// Opens the content part of `node_index`, and its message item on the first part.
    async fn ensure_part_visible(&mut self, node_index: u32) -> AppResult<()> {
        let Some(state) = self.nodes.get_mut(&node_index) else {
            return Ok(());
        };
        let Some(output_index) = state.output_index.filter(|_| !state.visible) else {
            return Ok(());
        };
        state.visible = true;
        let item = &mut self.items[output_index];
        if let Some(obj) = item.item.as_object_mut() {
            items::merge_json_extra_preserving(obj, &self.pending_envelope_extra);
            items::merge_json_extra(obj, &state.start_extra);
            obj.insert("status".to_string(), json!("in_progress"));
        }
        self.pending_envelope_extra.clear();
        if !item.added {
            item.added = true;
            let payload = json!({ "output_index": output_index, "item": item.item.clone() });
            self.wire
                .event("response.output_item.added", payload)
                .await?;
        }
        let payload = json!({
            "output_index": output_index,
            "content_index": state.content_index,
            "item_id": item_id(&item.item),
            "part": start_content_part(&state.header),
        });
        self.wire
            .event("response.content_part.added", payload)
            .await
    }

    pub(super) async fn on_node_delta(
        &mut self,
        node_index: u32,
        delta: NodeDelta,
        extra_body: HashMap<String, Value>,
    ) -> AppResult<()> {
        if let Some(name) = image_generation_call_downstream_event(&extra_body) {
            let payload = match &delta {
                NodeDelta::Image { source } => Some(image_generation_call_image_event_payload(
                    source,
                    &extra_body,
                )),
                NodeDelta::ProviderItem { data } => {
                    Some(image_generation_call_event_payload(data, &extra_body))
                }
                _ => None,
            };
            if let Some(payload) = payload {
                return self.wire.event(&name, payload).await;
            }
        }
        let Some(kind) = self
            .nodes
            .get(&node_index)
            .filter(|state| !state.closed)
            .map(|state| state.kind)
        else {
            return Ok(());
        };
        if reasoning::delta_has_payload(&delta) {
            self.ensure_started(node_index).await?;
        }
        if kind == NodeKind::Part {
            self.ensure_part_visible(node_index).await?;
        }
        let Some(state) = self.nodes.get_mut(&node_index) else {
            return Ok(());
        };
        let Some(output_index) = state.output_index else {
            // A reasoning item without payload stays invisible (PR2g) but keeps its source.
            if let NodeDelta::Reasoning {
                source: Some(source),
                ..
            } = &delta
                && !source.is_empty()
            {
                state.streamed.source = Some(source.clone());
            }
            return Ok(());
        };
        let item = &mut self.items[output_index];
        let at = Coordinates {
            item_id: item_id(&item.item),
            output_index,
            content_index: state.content_index,
        };
        match delta {
            NodeDelta::ProviderItem { data } => {
                if kind == NodeKind::Provider {
                    items::merge_provider_delta(&mut item.item, &data);
                }
                Ok(())
            }
            delta => emit_delta(&mut self.wire, &at, state, delta, &extra_body).await,
        }
    }

    pub(super) async fn on_node_done(
        &mut self,
        node_index: u32,
        node: &Node,
        done_extra: &HashMap<String, Value>,
    ) -> AppResult<()> {
        if self
            .nodes
            .get(&node_index)
            .is_some_and(|state| !state.closed)
        {
            self.close_node(node_index, node, done_extra, false).await?;
        }
        Ok(())
    }

    /// Completes the lifecycle of `node_index` with its final `node`. `incomplete` marks a node
    /// the terminal snapshot of a non-completed response discarded (STR3p).
    pub(super) async fn close_node(
        &mut self,
        node_index: u32,
        node: &Node,
        done_extra: &HashMap<String, Value>,
        incomplete: bool,
    ) -> AppResult<()> {
        let Some(kind) = self.nodes.get(&node_index).map(|state| state.kind) else {
            return Ok(());
        };
        // A replayed control node carries its wire members only on the complete node.
        if let Node::NextDownstreamEnvelopeExtra { extra_body } = node {
            items::merge_extra_preserving(&mut self.pending_envelope_extra, extra_body);
        }
        if kind == NodeKind::Reasoning && reasoning::node_has_payload(node) {
            self.ensure_started(node_index).await?;
        }
        if kind == NodeKind::Part {
            self.ensure_part_visible(node_index).await?;
        }
        let Some(state) = self.nodes.get_mut(&node_index) else {
            return Ok(());
        };
        state.closed = true;
        let Some(output_index) = state.output_index else {
            return Ok(());
        };
        match kind {
            NodeKind::Silent => Ok(()),
            NodeKind::Part => {
                self.finish_part(node_index, output_index, node, done_extra, incomplete)
                    .await
            }
            _ => {
                self.finish_item(node_index, output_index, node, incomplete)
                    .await
            }
        }
    }

    async fn finish_part(
        &mut self,
        node_index: u32,
        output_index: usize,
        node: &Node,
        done_extra: &HashMap<String, Value>,
        incomplete: bool,
    ) -> AppResult<()> {
        let Some(state) = self.nodes.get(&node_index) else {
            return Ok(());
        };
        let at = Coordinates {
            item_id: item_id(&self.items[output_index].item),
            output_index,
            content_index: state.content_index,
        };
        match node {
            Node::Text {
                logprobs,
                citations,
                content,
                ..
            } => {
                let text = TextDone {
                    logprobs,
                    citations,
                    content,
                    phase: done_extra
                        .get(RESPONSES_TEXT_DONE_PHASE_EXTRA_KEY)
                        .and_then(Value::as_str),
                };
                finish_text(&mut self.wire, &at, &state.streamed, text).await?;
            }
            Node::Refusal { content, .. } => {
                finish_refusal(&mut self.wire, &at, &state.streamed.text, content).await?;
            }
            _ => {}
        }
        if let Some(part) = done_content_part(node) {
            let payload = json!({
                "output_index": output_index,
                "content_index": at.content_index,
                "item_id": at.item_id,
                "part": part.clone(),
            });
            self.wire
                .event("response.content_part.done", payload)
                .await?;
            items::set_content_part(
                &mut self.items[output_index].item,
                at.content_index as usize,
                part,
            );
        }
        let item = &mut self.items[output_index];
        item.open_parts = item.open_parts.saturating_sub(1);
        let open = self
            .open_message
            .as_ref()
            .is_some_and(|open| open.output_index == output_index);
        // STR3r: an open message stays open for later parts until its item-done signal.
        if item.open_parts == 0 && !open {
            self.emit_message_done(output_index, incomplete).await?;
        }
        Ok(())
    }

    async fn finish_item(
        &mut self,
        node_index: u32,
        output_index: usize,
        node: &Node,
        incomplete: bool,
    ) -> AppResult<()> {
        let duration = self.reasoning_duration_secs();
        let Some(state) = self.nodes.get_mut(&node_index) else {
            return Ok(());
        };
        let item_id = item_id(&self.items[output_index].item);
        match node {
            Node::Reasoning { .. } => {
                reasoning::finish(
                    &mut self.wire,
                    &item_id,
                    output_index,
                    &mut state.streamed,
                    node,
                )
                .await?;
            }
            Node::ToolCall { .. } => {
                finish_arguments(&mut self.wire, &item_id, output_index, state, node).await?;
            }
            _ => {}
        }
        let item = &mut self.items[output_index];
        let mut done = items::done_item(state.kind, &item.item, node);
        if incomplete && matches!(state.kind, NodeKind::ToolCall | NodeKind::ToolResult) {
            done["status"] = json!("incomplete");
        }
        let done = sanitize_responses_output_item_for_frame_limit(
            &reasoning::item_with_duration(done, Some(duration)),
            self.wire.max_frame_length(),
        );
        item.item = done.clone();
        item.done = true;
        let payload = json!({ "output_index": output_index, "item": done });
        self.wire.event("response.output_item.done", payload).await
    }

    /// Closes the open message. A message with an open part completes with that part.
    pub(super) async fn close_open_message(&mut self) -> AppResult<()> {
        let Some(open) = self.open_message.take() else {
            return Ok(());
        };
        if self.items[open.output_index].open_parts == 0 {
            self.emit_message_done(open.output_index, false).await?;
        }
        Ok(())
    }

    /// `incomplete`: the response ended without completing (STR3p).
    pub(super) async fn emit_message_done(
        &mut self,
        output_index: usize,
        incomplete: bool,
    ) -> AppResult<()> {
        let max_frame_length = self.wire.max_frame_length();
        let item = &mut self.items[output_index];
        if !item.added || item.done {
            return Ok(());
        }
        if incomplete && item.item.get("status").and_then(Value::as_str) == Some("in_progress") {
            item.item["status"] = json!("incomplete");
        }
        let done = sanitize_responses_output_item_for_frame_limit(
            &items::complete_item(item.item.clone()),
            max_frame_length,
        );
        item.item = done.clone();
        item.done = true;
        let payload = json!({ "output_index": output_index, "item": done });
        self.wire.event("response.output_item.done", payload).await
    }
}

fn arguments_event_names(custom: bool) -> (&'static str, &'static str, &'static str) {
    if custom {
        (
            "response.custom_tool_call_input.delta",
            "response.custom_tool_call_input.done",
            "input",
        )
    } else {
        (
            "response.function_call_arguments.delta",
            "response.function_call_arguments.done",
            "arguments",
        )
    }
}

/// The wire deltas of one payload fragment of a visible node; `streamed` records what they
/// carried (STR-15).
async fn emit_delta(
    wire: &mut Wire,
    at: &Coordinates,
    state: &mut NodeState,
    delta: NodeDelta,
    extra_body: &HashMap<String, Value>,
) -> AppResult<()> {
    let streamed = &mut state.streamed;
    match delta {
        NodeDelta::Text {
            logprobs,
            citations,
            content,
            ..
        } => {
            text_delta(
                wire, at, streamed, &content, &citations, &logprobs, extra_body,
            )
            .await
        }
        NodeDelta::Refusal { content, .. } => {
            streamed.text.push_str(&content);
            let template = json!({
                "item_id": at.item_id,
                "output_index": at.output_index,
                "content_index": at.content_index,
            });
            wire.delta(
                "response.refusal.delta",
                with_delta_wire_fields(template, extra_body),
                &content,
            )
            .await
        }
        NodeDelta::Reasoning {
            metadata,
            content,
            encrypted,
            summary,
            source,
        } => {
            let delta = reasoning::ReasoningDelta {
                metadata: &metadata,
                content: content.as_deref(),
                summary: summary.as_deref(),
                encrypted,
                source: source.as_deref(),
            };
            reasoning::delta(
                wire,
                &at.item_id,
                at.output_index,
                streamed,
                delta,
                extra_body,
            )
            .await
        }
        NodeDelta::ToolCallArguments { arguments } => {
            let custom = matches!(
                state.header,
                NodeHeader::ToolCall {
                    tool_type: ToolCallType::Custom,
                    ..
                }
            );
            let arguments = if custom {
                arguments
            } else {
                tool_call_arguments_for_wire(arguments)
            };
            streamed.text.push_str(&arguments);
            let template = json!({ "item_id": at.item_id, "output_index": at.output_index });
            wire.delta(
                arguments_event_names(custom).0,
                with_delta_wire_fields(template, extra_body),
                &arguments,
            )
            .await
        }
        NodeDelta::ProviderItem { .. }
        | NodeDelta::Image { .. }
        | NodeDelta::Audio { .. }
        | NodeDelta::File { .. } => Ok(()),
    }
}

async fn text_delta(
    wire: &mut Wire,
    at: &Coordinates,
    streamed: &mut Streamed,
    content: &str,
    citations: &[Citation],
    logprobs: &Option<Vec<TokenLogprob>>,
    extra_body: &HashMap<String, Value>,
) -> AppResult<()> {
    let annotations = crate::urp::citations::encode(citations, ProviderProtocol::Responses, 0);
    let first = streamed.annotations;
    streamed.annotations += annotations.len();
    for (offset, annotation) in annotations.into_iter().enumerate() {
        emit_annotation(wire, at, first + offset, annotation).await?;
    }
    // STR3g: an annotation-only delta carries no text delta.
    if content.is_empty() && !citations.is_empty() {
        return Ok(());
    }
    streamed.text.push_str(content);
    let payload = with_delta_wire_fields(
        responses_text_delta_payload(
            extra_body.get("phase").and_then(Value::as_str),
            &json!({ "id": at.item_id }),
            at.output_index as u64,
            u64::from(at.content_index),
        ),
        extra_body,
    );
    wire.text_delta(
        payload,
        content,
        crate::urp::logprobs::valid(logprobs, content),
    )
    .await
}

async fn emit_annotation(
    wire: &mut Wire,
    at: &Coordinates,
    annotation_index: usize,
    annotation: Value,
) -> AppResult<()> {
    let payload = json!({
        "item_id": at.item_id,
        "output_index": at.output_index,
        "content_index": at.content_index,
        "annotation_index": annotation_index,
        "annotation": annotation,
    });
    wire.event("response.output_text.annotation.added", payload)
        .await
}

struct TextDone<'a> {
    logprobs: &'a Option<Vec<TokenLogprob>>,
    citations: &'a [Citation],
    content: &'a str,
    /// `phase` of the upstream done event (STR3m).
    phase: Option<&'a str>,
}

/// STR-15: the text suffix and annotations no delta carried, then `output_text.done`.
async fn finish_text(
    wire: &mut Wire,
    at: &Coordinates,
    streamed: &Streamed,
    text: TextDone<'_>,
) -> AppResult<()> {
    let item = json!({ "id": at.item_id });
    let (output_index, content_index) = (at.output_index as u64, u64::from(at.content_index));
    if let Some(suffix) = text
        .content
        .strip_prefix(streamed.text.as_str())
        .filter(|suffix| !suffix.is_empty())
    {
        // Scores align with the whole text only; a suffix of streamed text carries none.
        let scores = streamed
            .text
            .is_empty()
            .then(|| crate::urp::logprobs::valid(text.logprobs, text.content))
            .flatten();
        let payload = responses_text_delta_payload(None, &item, output_index, content_index);
        wire.text_delta(payload, suffix, scores).await?;
    }
    let annotations = crate::urp::citations::encode(text.citations, ProviderProtocol::Responses, 0);
    for (index, annotation) in annotations
        .into_iter()
        .enumerate()
        .skip(streamed.annotations)
    {
        emit_annotation(wire, at, index, annotation).await?;
    }
    let mut payload = responses_text_delta_payload(text.phase, &item, output_index, content_index);
    payload["text"] = json!(text.content);
    payload["logprobs"] = crate::urp::logprobs::encode_openai(
        crate::urp::logprobs::valid(text.logprobs, text.content).unwrap_or_default(),
    );
    wire.event("response.output_text.done", payload).await
}

async fn finish_refusal(
    wire: &mut Wire,
    at: &Coordinates,
    streamed: &str,
    content: &str,
) -> AppResult<()> {
    let coordinates = json!({
        "item_id": at.item_id,
        "output_index": at.output_index,
        "content_index": at.content_index,
    });
    if let Some(suffix) = content
        .strip_prefix(streamed)
        .filter(|suffix| !suffix.is_empty())
    {
        wire.delta("response.refusal.delta", coordinates.clone(), suffix)
            .await?;
    }
    let mut payload = coordinates;
    payload["refusal"] = json!(content);
    wire.event("response.refusal.done", payload).await
}

/// STR3h: the argument suffix no delta carried, then exactly one arguments done event.
async fn finish_arguments(
    wire: &mut Wire,
    item_id: &str,
    output_index: usize,
    state: &NodeState,
    node: &Node,
) -> AppResult<()> {
    let (
        Node::ToolCall {
            tool_type,
            arguments,
            ..
        },
        NodeHeader::ToolCall { call_id, name, .. },
    ) = (node, &state.header)
    else {
        return Ok(());
    };
    let custom = *tool_type == ToolCallType::Custom;
    let arguments = if custom {
        arguments.clone()
    } else {
        tool_call_arguments_for_wire(arguments)
    };
    let (delta_name, done_name, field) = arguments_event_names(custom);
    if let Some(suffix) = arguments
        .strip_prefix(state.streamed.text.as_str())
        .filter(|suffix| !suffix.is_empty())
    {
        let template = json!({ "item_id": item_id, "output_index": output_index });
        wire.delta(delta_name, template, suffix).await?;
    }
    let payload = json!({
        (field): arguments,
        "call_id": call_id,
        "item_id": item_id,
        "name": name,
        "output_index": output_index,
    });
    wire.event(done_name, payload).await
}
