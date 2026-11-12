//! Canonical node lifecycle of a live stream: partial Text and Reasoning, completions, and the
//! in-order cursor that decides when a node's Parts reach the wire (GEM-9).

use super::GeminiStreamEncoder;
use crate::urp::{Node, NodeDelta, NodeHeader};
use serde_json::Value;
use std::collections::HashMap;

/// Canonical state of one announced node.
#[derive(Default)]
pub(super) struct NodeSlot {
    /// Text or Reasoning accumulated from `NodeStart` and deltas; other kinds wait for
    /// `NodeDone`.
    partial: Option<Node>,
    /// The `NodeDone` node while the cursor has not reached it.
    complete: Option<Node>,
    done: bool,
}

impl NodeSlot {
    /// Whether the node holds content that establishes a wire boundary after the previous node.
    fn ready(&self) -> bool {
        self.complete.is_some() || self.partial.as_ref().is_some_and(live_text)
    }
}

impl GeminiStreamEncoder {
    pub(super) fn on_node_start(
        &mut self,
        node_index: u32,
        header: NodeHeader,
        extra_body: HashMap<String, Value>,
    ) -> Result<Vec<Value>, String> {
        if self.nodes.contains_key(&node_index) {
            return Err("Gemini stream received a duplicate node start".into());
        }
        self.nodes.insert(
            node_index,
            NodeSlot {
                partial: partial_node(header, extra_body),
                ..Default::default()
            },
        );
        Ok(Vec::new())
    }

    pub(super) fn on_node_delta(
        &mut self,
        node_index: u32,
        delta: NodeDelta,
        extra_body: HashMap<String, Value>,
    ) -> Result<Vec<Value>, String> {
        let slot = self
            .nodes
            .get_mut(&node_index)
            .filter(|slot| !slot.done)
            .ok_or("Gemini stream delta requires an open canonical node")?;
        if let Some(node) = slot.partial.as_mut() {
            apply_delta(node, delta);
            node.extra_body_mut().extend(extra_body);
        }
        if node_index >= self.next_index {
            return self.flush_available();
        }
        // The cursor already passed this node, so its live text extends its Parts directly.
        let content = match &slot.partial {
            Some(node) => self.wire.emit(node_index, node)?,
            None => None,
        };
        Ok(content
            .into_iter()
            .map(|content| self.frame(content))
            .collect())
    }

    pub(super) fn on_node_done(
        &mut self,
        node_index: u32,
        node: Node,
    ) -> Result<Vec<Value>, String> {
        let slot = self.nodes.entry(node_index).or_default();
        if slot.done {
            return Err("Gemini stream received a duplicate node completion".into());
        }
        slot.done = true;
        slot.partial = None;
        if node_index < self.next_index {
            let content = self.wire.emit(node_index, &node)?;
            return Ok(content
                .into_iter()
                .map(|content| self.frame(content))
                .collect());
        }
        slot.complete = Some(node);
        self.flush_available()
    }

    /// Emits Parts in canonical order. The cursor passes a node once it completed, or once it
    /// emitted live text and the next node is ready, which establishes a wire boundary; a passed
    /// node may still extend its Parts later.
    fn flush_available(&mut self) -> Result<Vec<Value>, String> {
        let mut contents = Vec::new();
        loop {
            let index = self.next_index;
            let Some(slot) = self.nodes.get_mut(&index) else {
                break;
            };
            if let Some(node) = slot.complete.take() {
                // A refusal waits for the terminal snapshot, which decides whether it reports a
                // blocked prompt.
                if !matches!(node, Node::Refusal { .. }) {
                    contents.extend(self.wire.emit(index, &node)?);
                }
                self.next_index += 1;
                continue;
            }
            if let Some(node) = &slot.partial
                && (self.wire.has_emitted(index) || live_text(node))
            {
                contents.extend(self.wire.emit(index, node)?);
            }
            let next_ready = index
                .checked_add(1)
                .and_then(|next| self.nodes.get(&next))
                .is_some_and(NodeSlot::ready);
            if !(self.wire.has_emitted(index) && next_ready) {
                break;
            }
            self.next_index += 1;
        }
        Ok(contents
            .into_iter()
            .map(|content| self.frame(content))
            .collect())
    }
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
            cache_breakpoint: None,
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

/// Unsigned text or thought text that may stream before its node completes.
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
        } => {
            content.as_ref().is_some_and(|text| !text.is_empty())
                || summary.as_ref().is_some_and(|text| !text.is_empty())
        }
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
