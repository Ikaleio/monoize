//! Node-to-block scheduling (DM9a, MSG-5): blocks follow node order, one block is open at a
//! time, and a node whose block cannot open yet waits with its accumulated payload.

use super::MessagesStreamEncoder;
use super::blocks::BlockPayload;
use super::lifecycle;
use crate::error::AppResult;
use crate::urp::{Node, NodeDelta, NodeHeader, TerminalNode};
use serde_json::Value;
use std::collections::HashMap;

/// The wire state of one announced node that has a Messages block.
pub(super) enum NodeState {
    /// The block has not started; the payload accumulates the node's deltas.
    Waiting(BlockPayload),
    /// The block is open at wire index `index`; the payload holds what the wire carries.
    Open { index: u32, payload: BlockPayload },
    /// The node completed; its block waits for the blocks of earlier nodes.
    Ready(BlockPayload),
    /// The block ended, or the previous block absorbed the node (DM5a.1).
    Closed,
}

impl MessagesStreamEncoder {
    pub(super) async fn on_node_start(
        &mut self,
        node_index: u32,
        header: NodeHeader,
        extra_body: HashMap<String, Value>,
    ) -> AppResult<()> {
        if matches!(header, NodeHeader::NextDownstreamEnvelopeExtra) {
            self.add_envelope_extra(&extra_body);
            return Ok(());
        }
        self.saw_refusal |= matches!(header, NodeHeader::Refusal { .. });
        let Some(payload) = BlockPayload::from_header(&header, &extra_body) else {
            return Ok(());
        };
        self.ensure_message_start().await?;
        self.saw_tool_use |= payload.is_tool_use();
        self.nodes
            .entry(node_index)
            .or_insert(NodeState::Waiting(payload));
        if node_index == self.next_node {
            self.open_next().await?;
        }
        Ok(())
    }

    pub(super) async fn on_node_delta(
        &mut self,
        node_index: u32,
        delta: NodeDelta,
        extra_body: HashMap<String, Value>,
    ) -> AppResult<()> {
        match self.nodes.get_mut(&node_index) {
            Some(NodeState::Waiting(payload) | NodeState::Open { payload, .. }) => {
                payload.merge_delta_metadata(&delta);
            }
            Some(NodeState::Ready(_) | NodeState::Closed) => {
                if matches!(&delta, NodeDelta::Text { citations, .. } if !citations.is_empty()) {
                    let message = "Messages cannot append citations to a closed text block.";
                    return self.wire.media_error(message.to_string()).await;
                }
                return Ok(());
            }
            None => return Ok(()),
        }
        // PM6e: buffering would lose this delta's estimate, so the next block opens now.
        if node_index == self.next_node
            && !self.replay
            && extra_body.contains_key("estimated_tokens")
            && matches!(
                self.nodes.get(&node_index),
                Some(NodeState::Waiting(BlockPayload::Thinking { metadata, .. }))
                    if !metadata.redacted
            )
        {
            self.open_block(node_index).await?;
        }
        match self.nodes.get_mut(&node_index) {
            Some(NodeState::Open { index, payload }) => {
                lifecycle::emit_delta(&self.wire, *index, payload, &delta, &extra_body).await?;
                payload.absorb_delta(&delta, true);
            }
            Some(NodeState::Waiting(payload)) => payload.absorb_delta(&delta, false),
            _ => {}
        }
        // DM5.3 item 5: a waiting thinking block opens with its first text or signature.
        if node_index == self.next_node {
            self.open_next().await?;
        }
        Ok(())
    }

    pub(super) async fn on_node_done(&mut self, node_index: u32, node: Node) -> AppResult<()> {
        if let Node::NextDownstreamEnvelopeExtra { extra_body } = &node {
            // A replayed control node carries its wire members only on the complete node.
            self.add_envelope_extra(extra_body);
            return self.skip_node(node_index).await;
        }
        self.saw_refusal |= matches!(node, Node::Refusal { .. });
        match self.nodes.get(&node_index) {
            Some(NodeState::Open { .. }) => self.close_block(node_index, &node).await,
            Some(NodeState::Closed) => Ok(()),
            Some(NodeState::Waiting(_) | NodeState::Ready(_)) | None => {
                self.complete_node(node_index, &node).await
            }
        }
    }

    /// STR-15 fragments, DM5a.1 absorption, and `content_block_stop` of the open block.
    async fn close_block(&mut self, node_index: u32, node: &Node) -> AppResult<()> {
        let Some(NodeState::Open { index, payload }) =
            self.nodes.insert(node_index, NodeState::Closed)
        else {
            return Ok(());
        };
        lifecycle::emit_suffix(&self.wire, index, &payload, node).await?;
        let following = node_index.saturating_add(1);
        let absorbed = self.absorb_signature(index, &payload, following).await?;
        self.wire.block_stop(index).await?;
        if node_index == self.next_node {
            self.next_node += 1;
        }
        if absorbed && following == self.next_node {
            self.next_node += 1;
        }
        self.flush_ready().await?;
        self.open_next().await
    }

    /// DM5a.1: OpenRouter sends plaintext and encrypted reasoning as adjacent detail entries.
    /// Messages needs both in one thinking block, so the closing block takes the signature of
    /// the waiting next node, which then forms no block. URP keeps both nodes.
    async fn absorb_signature(
        &mut self,
        index: u32,
        current: &BlockPayload,
        following: u32,
    ) -> AppResult<bool> {
        let Some(NodeState::Waiting(next)) = self.nodes.get(&following) else {
            return Ok(false);
        };
        if !current.takes_signature_of(next) {
            return Ok(false);
        }
        let Some(signature) = next.wire_signature() else {
            return Ok(false);
        };
        self.wire.signature(index, &signature).await?;
        self.nodes.insert(following, NodeState::Closed);
        Ok(true)
    }

    /// A node completed before its block opened. Its complete block waits for earlier blocks.
    async fn complete_node(&mut self, node_index: u32, node: &Node) -> AppResult<()> {
        let Some(payload) = BlockPayload::from_node(node) else {
            self.nodes.remove(&node_index);
            return self.skip_node(node_index).await;
        };
        self.nodes.insert(node_index, NodeState::Ready(payload));
        self.flush_ready().await?;
        self.open_next().await
    }

    /// A completed node without a block. Only the next node moves the cursor; a later one
    /// leaves the cursor to the terminal flush.
    async fn skip_node(&mut self, node_index: u32) -> AppResult<()> {
        if node_index == self.next_node {
            self.next_node += 1;
            self.flush_ready().await?;
        }
        Ok(())
    }

    /// Opens the waiting block of the next node once it can stream. A replay opens no block
    /// before its node completes.
    async fn open_next(&mut self) -> AppResult<()> {
        if self.replay {
            return Ok(());
        }
        if matches!(
            self.nodes.get(&self.next_node),
            Some(NodeState::Waiting(payload)) if payload.opens_live()
        ) {
            self.open_block(self.next_node).await?;
        }
        Ok(())
    }

    /// Starts the waiting block of `node_index` and sends what it accumulated.
    async fn open_block(&mut self, node_index: u32) -> AppResult<()> {
        let Some(state) = self.nodes.get_mut(&node_index) else {
            return Ok(());
        };
        let payload = match std::mem::replace(state, NodeState::Closed) {
            NodeState::Waiting(payload) => payload,
            other => {
                *state = other;
                return Ok(());
            }
        };
        let index = self.next_block_index;
        self.next_block_index += 1;
        self.wire.block_start(index, payload.start_block()).await?;
        lifecycle::emit_accumulated(&self.wire, index, &payload).await?;
        *state = NodeState::Open { index, payload };
        Ok(())
    }

    /// Emits the complete blocks that wait at the cursor, in node order.
    async fn flush_ready(&mut self) -> AppResult<()> {
        while matches!(self.nodes.get(&self.next_node), Some(NodeState::Ready(_))) {
            if let Some(NodeState::Ready(payload)) =
                self.nodes.insert(self.next_node, NodeState::Closed)
            {
                self.emit_block(&payload).await?;
            }
            self.next_node += 1;
        }
        Ok(())
    }

    /// The complete lifecycle of a block at the next wire index (PM9a).
    async fn emit_block(&mut self, payload: &BlockPayload) -> AppResult<()> {
        self.ensure_message_start().await?;
        let index = self.next_block_index;
        self.next_block_index += 1;
        self.saw_tool_use |= payload.is_tool_use();
        lifecycle::emit_complete(&self.wire, index, payload).await
    }

    /// Ends the open block with the last fragments of its terminal entry (STR-15). A block not
    /// yet on the wire takes its terminal entry; a node without one is not part of the final
    /// response (STR-8e). Then every remaining block follows in node order.
    pub(super) async fn finish_blocks(&mut self, output: &[TerminalNode]) -> AppResult<()> {
        let resolved: Vec<Option<u32>> =
            TerminalNode::resolve(output, |index| self.nodes.contains_key(&index))
                .map(|(node_index, _)| node_index)
                .collect();
        let terminal: HashMap<u32, &Node> = resolved
            .iter()
            .zip(output)
            .filter_map(|(node_index, entry)| Some(((*node_index)?, &entry.node)))
            .collect();
        let mut unfinished: Vec<u32> = self
            .nodes
            .iter()
            .filter(|(_, state)| !matches!(state, NodeState::Closed))
            .map(|(node_index, _)| *node_index)
            .collect();
        unfinished.sort_unstable();
        for node_index in unfinished {
            let complete = terminal.get(&node_index).copied();
            if let Some(NodeState::Open { index, payload }) =
                self.nodes.insert(node_index, NodeState::Closed)
            {
                if let Some(node) = complete {
                    lifecycle::emit_suffix(&self.wire, index, &payload, node).await?;
                }
                self.wire.block_stop(index).await?;
            } else if let Some(payload) = complete.and_then(BlockPayload::from_node) {
                self.nodes.insert(node_index, NodeState::Ready(payload));
            }
        }
        self.flush_all().await?;
        // STR-8b: an entry no announced node claims is a node first seen at the terminal.
        for (node_index, entry) in resolved.into_iter().zip(output) {
            if node_index.is_none()
                && let Some(payload) = BlockPayload::from_node(&entry.node)
            {
                self.emit_block(&payload).await?;
            }
        }
        Ok(())
    }

    /// Emits every complete block in node order, past nodes that never completed.
    async fn flush_all(&mut self) -> AppResult<()> {
        loop {
            self.flush_ready().await?;
            let Some(next) = self
                .nodes
                .iter()
                .filter(|(_, state)| matches!(state, NodeState::Ready(_)))
                .map(|(node_index, _)| *node_index)
                .min()
            else {
                return Ok(());
            };
            self.next_node = next;
        }
    }
}
