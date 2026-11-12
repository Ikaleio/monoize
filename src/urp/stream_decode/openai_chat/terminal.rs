//! The terminal `choices[0].message` snapshot (PC7f) and the completion of the stream.

use super::chunk::unknown_members;
use super::content::{block_is_tool_call_like, content_blocks, text_block_text};
use super::details::detail_text_len;
use super::reasoning::snapshot_suffix;
use super::state::ChatStreamState;
use super::tool_calls::legacy_function_call_as_tool_call;
use crate::error::AppResult;
use crate::handlers::usage::{increment_estimated_output_tokens, latest_stream_usage_snapshot};
use crate::urp::decode::openai_chat::normalize_tool_call_finish;
use crate::urp::{
    CHAT_DELTA_EXTRA_BODY_KEY, CHAT_REASONING_SURFACE_REASONING,
    CHAT_REASONING_SURFACE_REASONING_CONTENT, CHAT_REASONING_SURFACE_REASONING_OPAQUE, Node,
    TerminalNode, UrpStreamEvent,
};
use serde_json::{Map, Value};

/// Message members that the snapshot handlers own; every other non-internal member reaches
/// one downstream delta before the terminal chunk (PC7f).
const KNOWN_MESSAGE_MEMBERS: &[&str] = &[
    "role",
    "content",
    "reasoning",
    "reasoning_content",
    "reasoning_details",
    "reasoning_opaque",
    "tool_calls",
    "function_call",
    "refusal",
    "phase",
];

impl ChatStreamState<'_> {
    pub(super) async fn on_terminal_message(
        &mut self,
        message: &Map<String, Value>,
    ) -> AppResult<()> {
        if self.phase.is_none() {
            self.phase = message
                .get("phase")
                .and_then(Value::as_str)
                .map(str::to_string);
        }
        let carries_reasoning = [
            "reasoning_details",
            "reasoning",
            "reasoning_content",
            "reasoning_opaque",
        ]
        .iter()
        .any(|key| match message.get(*key) {
            Some(Value::String(value)) => !value.is_empty(),
            Some(Value::Array(value)) => !value.is_empty(),
            _ => false,
        });
        if carries_reasoning || !self.held_placeholders.is_empty() {
            self.close_text().await?;
        }
        self.delta_extra
            .extend(unknown_members(message, KNOWN_MESSAGE_MEMBERS));
        self.on_terminal_reasoning(message).await?;
        self.release_placeholders().await?;
        self.close_reasoning(true).await?;
        self.on_terminal_output(message).await
    }

    /// Structured details reconcile by entry identity. Scalar reasoning applies only to a
    /// stream without details: text extends by suffix, opaque data replaces atomically.
    async fn on_terminal_reasoning(&mut self, message: &Map<String, Value>) -> AppResult<()> {
        if let Some(details) = message
            .get("reasoning_details")
            .and_then(Value::as_array)
            .filter(|details| !details.is_empty())
        {
            return self.on_terminal_details(details).await;
        }
        if !self.details.is_empty() {
            return Ok(());
        }
        let scalar = [
            (
                "reasoning_content",
                CHAT_REASONING_SURFACE_REASONING_CONTENT,
            ),
            ("reasoning", CHAT_REASONING_SURFACE_REASONING),
        ]
        .into_iter()
        .find_map(|(key, surface)| {
            message
                .get(key)
                .and_then(Value::as_str)
                .filter(|value| !value.is_empty())
                .map(|value| (value, surface))
        });
        if let Some((snapshot, surface)) = scalar
            && let Some(suffix) = snapshot_suffix(&self.reasoning.text, snapshot)
        {
            self.push_reasoning_text(suffix, None, Some(surface))
                .await?;
        }
        if let Some(snapshot) = message
            .get("reasoning_opaque")
            .and_then(Value::as_str)
            .filter(|value| !value.is_empty())
            && let Some(suffix) = self.reasoning.apply_encrypted_snapshot(snapshot)
        {
            self.push_reasoning_encrypted(
                &Value::String(suffix),
                None,
                Some(CHAT_REASONING_SURFACE_REASONING_OPAQUE),
            )
            .await?;
        }
        Ok(())
    }

    async fn on_terminal_output(&mut self, message: &Map<String, Value>) -> AppResult<()> {
        match message.get("content") {
            Some(Value::String(text)) => {
                if let Some(suffix) = snapshot_suffix(&self.text.all, text) {
                    self.push_text(suffix, None).await?;
                }
            }
            Some(content) => {
                if let Some(blocks) = content_blocks(content) {
                    self.on_terminal_blocks(blocks).await?;
                }
            }
            None => {}
        }
        if let Some(tool_calls) = message.get("tool_calls").and_then(Value::as_array) {
            for (position, tool_call) in tool_calls.iter().enumerate() {
                self.on_terminal_tool_call(tool_call, position, false)
                    .await?;
            }
        }
        if let Some(function_call) = message
            .get("function_call")
            .and_then(legacy_function_call_as_tool_call)
        {
            self.on_terminal_tool_call(&function_call, 0, true).await?;
        }
        Ok(())
    }

    /// Text blocks extend the streamed text by suffix. Tool-call blocks reconcile by call
    /// identity. The other blocks reconcile by their position among the message's other blocks:
    /// the first `n` correspond to the `n` content items already streamed.
    async fn on_terminal_blocks(&mut self, blocks: &[Value]) -> AppResult<()> {
        let streamed_items = self.provider_items.len();
        let mut item_position = 0usize;
        let mut snapshot_text = String::new();
        for (position, block) in blocks.iter().enumerate() {
            let block_obj = block.as_object();
            let text = match block {
                Value::String(text) => Some(text.as_str()),
                _ => block_obj.and_then(text_block_text),
            };
            if let Some(text) = text {
                snapshot_text.push_str(text);
                if let Some(suffix) = snapshot_suffix(&self.text.all, &snapshot_text) {
                    self.push_text(suffix, None).await?;
                }
                continue;
            }
            let Some(block_obj) = block_obj else {
                continue;
            };
            if block_is_tool_call_like(block_obj) {
                self.on_terminal_tool_call(block, position, false).await?;
                continue;
            }
            item_position += 1;
            if item_position > streamed_items {
                self.start_content_item(block_obj).await?;
            }
        }
        Ok(())
    }

    /// Completes the stream after `[DONE]` or the end of the upstream body: `NodeDone` for every
    /// open node, then `ResponseDone` with the terminal entry of every announced node.
    pub(super) async fn finish(&mut self) -> AppResult<()> {
        if !self.terminal_seen {
            return self
                .terminal_error(
                    "upstream_stream_missing_terminal",
                    "upstream Chat Completions stream ended before a terminal finish_reason",
                    None,
                    None,
                )
                .await;
        }
        if !self.held_placeholders.is_empty() {
            self.close_text().await?;
            self.release_placeholders().await?;
        }
        // Choice members of earlier chunks that no event carried yield to the terminal choice.
        if let Some(delta_extra) = self.take_delta_extra().remove(CHAT_DELTA_EXTRA_BODY_KEY) {
            self.terminal_extra
                .insert(CHAT_DELTA_EXTRA_BODY_KEY.to_string(), delta_extra);
        }
        let usage = match self.latest_usage.take() {
            Some(usage) => Some(usage),
            None => latest_stream_usage_snapshot(&self.runtime_metrics).await,
        };
        let estimated_chars = self.text.all.len()
            + self.reasoning.text.len()
            + self.reasoning.summary.len()
            + detail_text_len(&self.details);
        increment_estimated_output_tokens(&self.runtime_metrics, estimated_chars as u64).await;

        let mut indices = Vec::new();
        let mut nodes = Vec::new();
        for (node_index, node, done) in self.take_terminal_nodes() {
            if !done {
                self.send_done(node_index, node.clone()).await?;
            }
            indices.push(node_index);
            nodes.push(node);
        }
        let finish_reason = self
            .finish_reason
            .map(|reason| normalize_tool_call_finish(reason, &nodes));
        let output = indices
            .into_iter()
            .zip(nodes)
            .map(|(node_index, node)| TerminalNode::streamed(node_index, node))
            .collect();
        let extra_body = std::mem::take(&mut self.terminal_extra);
        self.send(UrpStreamEvent::ResponseDone {
            outcome: None,
            finish_reason,
            usage,
            output,
            extra_body,
        })
        .await
    }

    /// Every announced node in index order, with whether its `NodeDone` was already sent.
    fn take_terminal_nodes(&mut self) -> Vec<(u32, Node, bool)> {
        let mut nodes: Vec<(u32, Node, bool)> = self
            .closed
            .drain(..)
            .map(|(node_index, node)| (node_index, node, true))
            .collect();
        if let Some(node_index) = self.reasoning.open.take() {
            nodes.push((node_index, self.reasoning.take_segment(), false));
        }
        nodes.extend(
            self.details
                .drain(..)
                .map(|detail| (detail.node_index, detail.node, detail.done)),
        );
        if let Some(node_index) = self.text.open.take() {
            nodes.push((node_index, self.take_text_segment(), false));
        }
        nodes.extend(
            std::mem::take(&mut self.tool_calls)
                .into_nodes()
                .map(|(node_index, node)| (node_index, node, false)),
        );
        nodes.extend(
            self.provider_items
                .drain(..)
                .map(|(node_index, node)| (node_index, node, false)),
        );
        if let Some(refusal) = self.refusal.take() {
            nodes.push((refusal.node_index, refusal.into_node(), false));
        }
        if let Some(audio) = self.audio.take() {
            let node_index = audio.node_index;
            if let Some(node) = audio.into_node(self.audio_mime) {
                nodes.push((node_index, node, false));
            }
        }
        nodes.sort_by_key(|(node_index, ..)| *node_index);
        nodes
    }
}
