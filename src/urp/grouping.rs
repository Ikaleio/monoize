//! The message-compatible envelope boundary of RECON-6, shared by non-stream
//! and stream encoders (RSTRUCT-5).

use super::{
    Node, NodeHeader, OrdinaryRole, RESPONSES_IMAGE_GENERATION_CALL_EXTRA_KEY, is_internal_key,
};
use serde_json::Value;
use std::collections::HashMap;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Zone {
    Reasoning,
    Content,
    Action,
}

/// The members of an ordinary node that decide its envelope membership.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct GroupKey<'a> {
    pub role: OrdinaryRole,
    pub zone: Zone,
    pub phase: Option<&'a str>,
}

impl<'a> GroupKey<'a> {
    /// `None` for a `ToolResult` and for a control node, which always end the open envelope.
    pub fn of_node(node: &'a Node) -> Option<Self> {
        let (zone, phase) = match node {
            Node::Reasoning { .. } => (Zone::Reasoning, None),
            Node::Image { extra_body, .. }
                if extra_body.contains_key(RESPONSES_IMAGE_GENERATION_CALL_EXTRA_KEY) =>
            {
                (Zone::Action, None)
            }
            Node::Text { phase, .. } => (Zone::Content, phase.as_deref()),
            Node::Image { .. } | Node::Audio { .. } | Node::File { .. } | Node::Refusal { .. } => {
                (Zone::Content, None)
            }
            Node::ToolCall { .. } | Node::ProviderItem { .. } => (Zone::Action, None),
            Node::ToolResult { .. } | Node::NextDownstreamEnvelopeExtra { .. } => return None,
        };
        Some(Self {
            role: node.role()?,
            zone,
            phase,
        })
    }

    /// The key of a node announced by `header`. `image_generation_call` states whether an
    /// `Image` header belongs to a Responses image-generation item, which the header cannot
    /// express itself.
    pub fn of_header(header: &'a NodeHeader, image_generation_call: bool) -> Option<Self> {
        let assistant = OrdinaryRole::Assistant;
        let (role, zone, phase) = match header {
            NodeHeader::Reasoning { .. } => (assistant, Zone::Reasoning, None),
            NodeHeader::Image { role, .. } if image_generation_call => (*role, Zone::Action, None),
            NodeHeader::Text { role, phase, .. } => (*role, Zone::Content, phase.as_deref()),
            NodeHeader::Image { role, .. }
            | NodeHeader::Audio { role, .. }
            | NodeHeader::File { role, .. } => (*role, Zone::Content, None),
            NodeHeader::Refusal { .. } => (assistant, Zone::Content, None),
            NodeHeader::ToolCall { .. } => (assistant, Zone::Action, None),
            NodeHeader::ProviderItem { role, .. } => (*role, Zone::Action, None),
            NodeHeader::ToolResult { .. } | NodeHeader::NextDownstreamEnvelopeExtra => return None,
        };
        Some(Self { role, zone, phase })
    }

    /// Whether a node with key `next` joins the envelope whose last node has key `self`.
    pub fn continues_with(self, next: GroupKey<'_>) -> bool {
        let phase_boundary =
            self.zone == Zone::Content && next.zone == Zone::Content && self.phase != next.phase;
        let zone_boundary = match next.zone {
            Zone::Reasoning => matches!(self.zone, Zone::Content | Zone::Action),
            Zone::Content => self.zone == Zone::Action,
            Zone::Action => false,
        };
        self.role == next.role && !phase_boundary && !zone_boundary
    }
}

/// Whether `next` shares the envelope of the ordinary node `previous`.
pub fn continues_message(previous: &Node, next: &Node) -> bool {
    match (GroupKey::of_node(previous), GroupKey::of_node(next)) {
        (Some(previous), Some(next)) => previous.continues_with(next),
        _ => false,
    }
}

/// One consumable envelope over a borrowed node sequence.
#[derive(Debug)]
pub enum Envelope<'a> {
    /// A maximal run of ordinary nodes that share one wire message.
    Message {
        role: OrdinaryRole,
        /// The `id` of the first node when that node is content; the wire message id.
        id: Option<&'a str>,
        nodes: &'a [Node],
        /// Wire members of the control nodes that precede the run (CTL-7, CTL-8).
        extra: HashMap<String, Value>,
    },
    ToolResult {
        node: &'a Node,
        /// Members of the control nodes that precede the result. The node's own extras win.
        extra: HashMap<String, Value>,
    },
}

/// Splits `nodes` into envelopes under RECON-6. A control node that no envelope follows is
/// discarded (CTL-11).
pub fn envelopes(nodes: &[Node]) -> Vec<Envelope<'_>> {
    struct OpenRun<'a> {
        start: usize,
        last: GroupKey<'a>,
        extra: HashMap<String, Value>,
    }

    fn message<'a>(nodes: &'a [Node], extra: HashMap<String, Value>) -> Envelope<'a> {
        Envelope::Message {
            role: nodes[0].role().unwrap_or(OrdinaryRole::Assistant),
            id: message_id(&nodes[0]),
            nodes,
            extra,
        }
    }

    let mut envelopes = Vec::new();
    let mut pending: HashMap<String, Value> = HashMap::new();
    let mut run: Option<OpenRun<'_>> = None;
    for (index, node) in nodes.iter().enumerate() {
        let key = GroupKey::of_node(node);
        if let Some(open) = run.as_mut()
            && let Some(key) = key
            && open.last.continues_with(key)
        {
            open.last = key;
            continue;
        }
        if let Some(open) = run.take() {
            envelopes.push(message(&nodes[open.start..index], open.extra));
        }
        match (key, node) {
            (Some(key), _) => {
                let mut extra = std::mem::take(&mut pending);
                extra.retain(|key, _| !is_internal_key(key));
                run = Some(OpenRun {
                    start: index,
                    last: key,
                    extra,
                });
            }
            (None, Node::NextDownstreamEnvelopeExtra { extra_body }) => {
                pending.extend(extra_body.iter().map(|(k, v)| (k.clone(), v.clone())));
            }
            (None, _) => envelopes.push(Envelope::ToolResult {
                node,
                extra: std::mem::take(&mut pending),
            }),
        }
    }
    if let Some(open) = run {
        envelopes.push(message(&nodes[open.start..], open.extra));
    }
    envelopes
}

fn message_id(node: &Node) -> Option<&str> {
    match node {
        Node::Text { id, .. }
        | Node::Image { id, .. }
        | Node::Audio { id, .. }
        | Node::File { id, .. }
        | Node::Refusal { id, .. } => id.as_deref(),
        _ => None,
    }
}
