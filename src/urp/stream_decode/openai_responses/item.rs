//! Stream state of one upstream Responses output item, keyed by its `output_index`.

use crate::urp::stream_helpers::extract_reasoning_parts;
use crate::urp::{
    Citation, Node, OrdinaryRole, ProviderProtocol, ReasoningMetadata, ReasoningTextPart,
    ToolCallType,
};
use serde_json::{Value, json};
use std::collections::{BTreeMap, HashMap};

/// The output item classes that terminal reconciliation pairs (PR6c).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) enum ItemClass {
    Reasoning,
    Message,
    ToolCall,
    ToolResult,
    ProviderItem,
}

impl ItemClass {
    /// The class of a native item `type`. Generated images live in assistant messages.
    pub(super) fn of_type(item_type: &str) -> Option<Self> {
        match item_type {
            "" => None,
            "reasoning" => Some(Self::Reasoning),
            "message" | "image_generation_call" => Some(Self::Message),
            "function_call" | "custom_tool_call" => Some(Self::ToolCall),
            "function_call_output" | "custom_tool_call_output" => Some(Self::ToolResult),
            _ => Some(Self::ProviderItem),
        }
    }
}

/// One message content part. `node_index` is set when its `NodeStart` went out.
#[derive(Debug, Default)]
pub(super) struct ContentSlot {
    pub(super) node_index: Option<u32>,
    pub(super) node: Option<Node>,
    pub(super) done: bool,
    /// `phase` of the part's `response.output_text.done` event.
    pub(super) done_phase: Option<String>,
}

/// Reasoning payload accumulated for the terminal comparison (PR6c).
#[derive(Debug, Default)]
pub(super) struct ReasoningAccumulator {
    content: String,
    /// Summary text that arrived without a `summary_index`.
    summary: String,
    summary_parts: BTreeMap<u64, String>,
    encrypted: Option<Value>,
    pub(super) source: Option<String>,
    pub(super) extra_body: HashMap<String, Value>,
}

impl ReasoningAccumulator {
    pub(super) fn merge_source(&mut self, value: &Value) {
        if let Some(source) = value
            .get("source")
            .and_then(Value::as_str)
            .filter(|source| !source.is_empty())
        {
            self.source = Some(source.to_string());
        }
    }

    pub(super) fn has_typed_output(&self) -> bool {
        !self.content.is_empty() || self.summary_text().is_some() || self.encrypted.is_some()
    }

    pub(super) fn append_text(&mut self, delta: &str) {
        self.content.push_str(delta);
    }

    pub(super) fn append_summary(&mut self, summary_index: Option<u64>, delta: &str) {
        // An empty delta must not create a part: part count decides the summary shape.
        if delta.is_empty() {
            return;
        }
        match summary_index {
            Some(index) => self.summary_parts.entry(index).or_default().push_str(delta),
            None => self.summary.push_str(delta),
        }
    }

    /// A `reasoning_text.done` text fills a slot that no delta filled.
    pub(super) fn complete_text(&mut self, text: &str) {
        if !text.is_empty() && self.content.is_empty() {
            self.content = text.to_string();
        }
    }

    /// A non-empty done text is the complete value of its summary part (PR6c).
    pub(super) fn complete_summary(&mut self, summary_index: Option<u64>, text: &str) {
        if text.is_empty() {
            return;
        }
        match summary_index {
            Some(index) => {
                self.summary_parts.insert(index, text.to_string());
            }
            None => self.summary = text.to_string(),
        }
    }

    /// Merges an item snapshot. A done snapshot (`done`) owns every non-empty field; an added
    /// snapshot only fills empty fields and never supplies encrypted content (PR6c).
    pub(super) fn merge_item_snapshot(&mut self, item: &Value, done: bool) {
        let (text, summary, encrypted) = extract_reasoning_parts(item);
        if !text.is_empty() && (done || self.content.is_empty()) {
            self.content = text;
        }
        if !summary.is_empty() {
            if done {
                self.summary_parts.clear();
                self.summary = summary;
            } else if self.summary.is_empty() && self.summary_parts.is_empty() {
                self.summary = summary;
            }
        }
        if done && !encrypted.is_empty() {
            self.encrypted = Some(Value::String(encrypted));
        }
    }

    /// Indexed summary parts concatenate without a separator (PR6c).
    fn summary_text(&self) -> Option<String> {
        let mut parts = self
            .summary_parts
            .values()
            .filter(|part| !part.is_empty())
            .map(String::as_str)
            .collect::<Vec<_>>();
        if parts.is_empty() {
            return (!self.summary.is_empty()).then(|| self.summary.clone());
        }
        if !self.summary.is_empty() {
            parts.push(&self.summary);
        }
        Some(parts.concat())
    }

    fn summary_part_shapes(&self) -> Option<Vec<ReasoningTextPart>> {
        if self.summary_parts.len() < 2 || !self.summary.is_empty() {
            return None;
        }
        Some(
            self.summary_parts
                .values()
                .filter(|part| !part.is_empty())
                .map(|part| ReasoningTextPart {
                    byte_length: part.len(),
                    extra_body: HashMap::from([("type".to_string(), json!("summary_text"))]),
                })
                .collect(),
        )
    }

    fn node(&self, id: Option<String>) -> Node {
        Node::Reasoning {
            metadata: ReasoningMetadata {
                summary_parts: self.summary_part_shapes(),
                ..Default::default()
            },
            id,
            content: (!self.content.is_empty()).then(|| self.content.clone()),
            encrypted: self.encrypted.clone(),
            summary: self.summary_text(),
            source: self.source.clone(),
            extra_body: self.extra_body.clone(),
        }
    }
}

/// Tool-call slot accumulated from item snapshots and argument deltas (PR6c).
#[derive(Debug)]
pub(super) struct CallAccumulator {
    pub(super) call_id: String,
    pub(super) tool_type: ToolCallType,
    pub(super) namespace: Option<String>,
    pub(super) name: String,
    pub(super) arguments: String,
}

impl CallAccumulator {
    pub(super) fn new(call_id: &str, tool_type: ToolCallType, name: &str, arguments: &str) -> Self {
        Self {
            call_id: call_id.to_string(),
            tool_type,
            namespace: None,
            name: name.to_string(),
            arguments: arguments.to_string(),
        }
    }

    /// A custom event marks the call custom and a missing name fills.
    fn apply_identity(&mut self, custom: bool, name: &str) {
        if custom {
            self.tool_type = ToolCallType::Custom;
        }
        if self.name.is_empty() && !name.is_empty() {
            self.name = name.to_string();
        }
    }

    pub(super) fn apply_delta(&mut self, custom: bool, name: &str, delta: &str) {
        self.apply_identity(custom, name);
        self.arguments.push_str(delta);
    }

    /// A non-empty argument snapshot replaces the delta-accumulated arguments (PR6c).
    pub(super) fn apply_snapshot(&mut self, custom: bool, name: &str, arguments: &str) {
        self.apply_identity(custom, name);
        if !arguments.is_empty() {
            self.arguments = arguments.to_string();
        }
    }

    fn node(&self, id: Option<String>) -> Node {
        Node::ToolCall {
            namespace: self.namespace.clone(),
            signature: None,
            id: id.or_else(|| Some(crate::urp::synthetic_tool_call_id())),
            tool_type: self.tool_type,
            call_id: self.call_id.clone(),
            name: self.name.clone(),
            arguments: self.arguments.clone(),
            extra_body: HashMap::new(),
            cache_breakpoint: None,
        }
    }
}

/// Everything the decoder tracks for one upstream output item.
#[derive(Debug, Default)]
pub(super) struct OutputItem {
    /// Native `type` from `response.output_item.added` or `.done`.
    pub(super) item_type: Option<String>,
    /// Upstream item id. A message without one gets a synthetic id that its nodes share.
    pub(super) id: Option<String>,
    pub(super) role: Option<OrdinaryRole>,
    /// Message `phase` from `response.output_item.added`.
    pub(super) phase: Option<String>,
    /// Item-level wire members, carried by the item's envelope control node.
    pub(super) extra: HashMap<String, Value>,
    /// Raw index of the emitted envelope control node.
    pub(super) control: Option<u32>,
    /// Raw index of the item's own node (every item kind except messages), allocated by the
    /// first event that needs it. `started` records that its `NodeStart` went out.
    pub(super) node_index: Option<u32>,
    pub(super) started: bool,
    pub(super) node_done: bool,
    pub(super) reasoning_text_delta_seen: bool,
    pub(super) reasoning_summary_delta_seen: bool,
    pub(super) arguments_delta_seen: bool,
    /// Annotations keyed by (content index, annotation index).
    pub(super) citations: BTreeMap<(u64, u64), Citation>,
    pub(super) content: BTreeMap<u64, ContentSlot>,
    /// Message content of the `response.output_item.added` snapshot, used when no content
    /// event followed.
    pub(super) added_content: Vec<Node>,
    pub(super) reasoning: ReasoningAccumulator,
    pub(super) call: Option<CallAccumulator>,
}

impl OutputItem {
    pub(super) fn role(&self) -> OrdinaryRole {
        self.role.unwrap_or(OrdinaryRole::Assistant)
    }

    pub(super) fn is_reasoning(&self) -> bool {
        self.item_type.as_deref() == Some("reasoning")
    }

    /// Records a non-empty upstream item id.
    pub(super) fn set_id(&mut self, id: Option<&str>) {
        if let Some(id) = id.filter(|id| !id.is_empty()) {
            self.id = Some(id.to_string());
        }
    }

    /// The id shared by the item's message nodes.
    pub(super) fn message_id(&mut self) -> String {
        self.id
            .get_or_insert_with(crate::urp::synthetic_message_id)
            .clone()
    }

    pub(super) fn citations_for(&self, content_index: u64) -> Vec<Citation> {
        self.citations
            .iter()
            .filter(|((part, _), _)| *part == content_index)
            .map(|(_, citation)| citation.clone())
            .collect()
    }

    pub(super) fn record_part_citations(&mut self, part: &Value, content_index: u64) {
        let Some(annotations) = part.get("annotations").and_then(Value::as_array) else {
            return;
        };
        for (annotation_index, annotation) in (0u64..).zip(annotations) {
            self.citations.insert(
                (content_index, annotation_index),
                Citation::decode(annotation.clone(), ProviderProtocol::Responses),
            );
        }
    }

    pub(super) fn emitted_any(&self) -> bool {
        self.started || self.content.values().any(|slot| slot.node_index.is_some())
    }

    /// Whether the item's nodes are message content parts rather than one item node.
    pub(super) fn is_message(&self) -> bool {
        match self.item_type.as_deref() {
            Some(item_type) => item_type == "message",
            None => !self.content.is_empty(),
        }
    }

    /// The class from the native type, else from what the stream accumulated.
    pub(super) fn class(&self) -> Option<ItemClass> {
        if let Some(item_type) = &self.item_type {
            return ItemClass::of_type(item_type);
        }
        if !self.content.is_empty() {
            Some(ItemClass::Message)
        } else if self.call.is_some() {
            Some(ItemClass::ToolCall)
        } else {
            self.reasoning
                .has_typed_output()
                .then_some(ItemClass::Reasoning)
        }
    }

    /// The node of a tool-call item, or of a reasoning item with payload, as the stream
    /// accumulated it. Other items keep no accumulated node: their terminal snapshot is their
    /// final state.
    pub(super) fn accumulated_node(&self) -> Option<Node> {
        if let Some(call) = &self.call {
            return Some(call.node(self.id.clone()));
        }
        self.reasoning
            .has_typed_output()
            .then(|| self.reasoning.node(self.id.clone()))
    }

    /// The reconstruction slot for the terminal merge (PR6c): a message's envelope control and
    /// content in content-index order, or the item's accumulated node.
    pub(super) fn accumulated_nodes(&self) -> Vec<Node> {
        if !self.is_message() {
            return self.accumulated_node().into_iter().collect();
        }
        let mut nodes: Vec<Node> = if self.content.is_empty() {
            self.added_content.clone()
        } else {
            self.content
                .values()
                .filter_map(|slot| slot.node.clone())
                .collect()
        };
        if !nodes.is_empty() && !self.extra.is_empty() {
            let control = Node::NextDownstreamEnvelopeExtra {
                extra_body: self.extra.clone(),
            };
            nodes.insert(0, control);
        }
        nodes
    }
}
