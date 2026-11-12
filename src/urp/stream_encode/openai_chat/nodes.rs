//! Node lifecycles: shape selection at `NodeStart`, deltas, and STR-15 completion.

use super::ChatStreamEncoder;
use super::media::is_message_audio;
use super::reasoning::{ReasoningFields, ReasoningProgress};
use super::text::TextProgress;
use crate::error::AppResult;
use crate::urp::encode::openai_chat::encode_assistant_chat_message_from_nodes;
use crate::urp::stream_helpers::{
    chat_delta_path_custom_tool_input, chat_delta_path_function_call_arguments,
    chat_delta_path_tool_arguments,
};
use crate::urp::{
    CHAT_LEGACY_FUNCTION_CALL_EXTRA_KEY, Node, NodeDelta, NodeHeader, ProviderProtocol,
    ToolCallType, is_internal_key,
};
use serde_json::{Value, json};
use std::collections::HashMap;

/// Cross-event state of one node.
#[derive(Default)]
pub(super) struct NodeState {
    pub(super) text: TextProgress,
    reasoning: ReasoningProgress,
    call: Option<CallShape>,
    /// Call arguments emitted so far.
    arguments: String,
    /// `NodeStart` selected the native `message.audio` shape.
    pub(super) message_audio: bool,
    /// The whole-node fragment of a media, provider, or control node went out.
    emitted: bool,
}

/// The native shape of a call, fixed when its header goes out (CHAT-7a).
#[derive(Clone, Copy)]
enum CallShape {
    /// The deprecated assistant `function_call` member (PC2.9).
    LegacyFunction,
    Tool {
        index: usize,
        tool_type: ToolCallType,
    },
}

impl CallShape {
    fn header(self, call_id: &str, name: &str) -> Value {
        match self {
            Self::LegacyFunction => json!({ "function_call": { "name": name, "arguments": "" } }),
            Self::Tool {
                index,
                tool_type: ToolCallType::Function,
            } => json!({ "tool_calls": [{
                "index": index,
                "id": call_id,
                "type": "function",
                "function": { "name": name, "arguments": "" }
            }] }),
            Self::Tool {
                index,
                tool_type: ToolCallType::Custom,
            } => json!({ "tool_calls": [{
                "index": index,
                "id": call_id,
                "type": "custom",
                "custom": { "name": name, "input": "" }
            }] }),
        }
    }

    fn arguments(self) -> (Value, fn(&mut Value, &str)) {
        match self {
            Self::LegacyFunction => (
                json!({ "function_call": { "arguments": "" } }),
                chat_delta_path_function_call_arguments,
            ),
            Self::Tool {
                index,
                tool_type: ToolCallType::Function,
            } => (
                json!({ "tool_calls": [{ "index": index, "function": { "arguments": "" } }] }),
                chat_delta_path_tool_arguments,
            ),
            Self::Tool {
                index,
                tool_type: ToolCallType::Custom,
            } => (
                json!({ "tool_calls": [{ "index": index, "custom": { "input": "" } }] }),
                chat_delta_path_custom_tool_input,
            ),
        }
    }
}

fn is_legacy_function_call(tool_type: ToolCallType, extra_body: &HashMap<String, Value>) -> bool {
    tool_type == ToolCallType::Function
        && extra_body
            .get(CHAT_LEGACY_FUNCTION_CALL_EXTRA_KEY)
            .and_then(Value::as_bool)
            == Some(true)
}

fn node_extra_body(node: &Node) -> &HashMap<String, Value> {
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
        | Node::NextDownstreamEnvelopeExtra { extra_body } => extra_body,
    }
}

impl ChatStreamEncoder {
    pub(super) async fn on_node_start(
        &mut self,
        node_index: u32,
        header: NodeHeader,
        extra_body: HashMap<String, Value>,
    ) -> AppResult<()> {
        let mut state = self.nodes.remove(&node_index).unwrap_or_default();
        let result = self.start_node(&mut state, &header, &extra_body).await;
        self.nodes.insert(node_index, state);
        result
    }

    pub(super) async fn on_node_delta(
        &mut self,
        node_index: u32,
        delta: NodeDelta,
        extra_body: HashMap<String, Value>,
    ) -> AppResult<()> {
        self.wire.stash_choice_extra(&extra_body);
        let mut state = self.nodes.remove(&node_index).unwrap_or_default();
        let result = match delta {
            NodeDelta::Text {
                logprobs,
                citations,
                content,
                ..
            } => {
                self.on_text_delta(&mut state.text, content, citations, logprobs, &extra_body)
                    .await
            }
            NodeDelta::Refusal { logprobs, content } => {
                self.on_refusal_delta(&mut state.text, content, logprobs, &extra_body)
                    .await
            }
            NodeDelta::Reasoning {
                metadata,
                content,
                encrypted,
                summary,
                source,
            } => {
                let fields = ReasoningFields {
                    metadata: &metadata,
                    item_id: metadata.item_id.as_deref(),
                    content: content.as_deref(),
                    summary: summary.as_deref(),
                    encrypted: encrypted.as_ref(),
                    source: source.as_deref(),
                };
                self.on_reasoning_delta(&mut state.reasoning, fields, &extra_body)
                    .await
            }
            NodeDelta::ToolCallArguments { arguments } => match state.call {
                Some(shape) => {
                    state.arguments.push_str(&arguments);
                    self.send_arguments(shape, &arguments, &extra_body).await
                }
                None => Ok(()),
            },
            // Generated audio goes out whole at `NodeDone`; other payload deltas have no Chat form.
            NodeDelta::Image { .. }
            | NodeDelta::Audio { .. }
            | NodeDelta::File { .. }
            | NodeDelta::ProviderItem { .. } => Ok(()),
        };
        self.nodes.insert(node_index, state);
        result
    }

    pub(super) async fn on_node_done(&mut self, node_index: u32, node: &Node) -> AppResult<()> {
        let mut state = self.nodes.remove(&node_index).unwrap_or_default();
        let result = self.complete_node(&mut state, node).await;
        self.nodes.insert(node_index, state);
        result
    }

    /// STR-8b: a node first seen in `ResponseDone.output` gets its whole lifecycle there.
    pub(super) async fn emit_unannounced(&mut self, node: &Node) -> AppResult<()> {
        let mut state = NodeState::default();
        self.start_node(&mut state, &NodeHeader::from(node), node_extra_body(node))
            .await?;
        self.complete_node(&mut state, node).await
    }

    /// Selects the native shape of a node from its header and `NodeStart` members.
    async fn start_node(
        &mut self,
        state: &mut NodeState,
        header: &NodeHeader,
        extra_body: &HashMap<String, Value>,
    ) -> AppResult<()> {
        match header {
            NodeHeader::NextDownstreamEnvelopeExtra => {
                // CTL-5: the members modify the next delta.
                state.emitted |= extra_body.keys().any(|key| !is_internal_key(key));
                self.wire.stash_delta_extra(extra_body);
            }
            NodeHeader::ToolCall {
                tool_type,
                call_id,
                name,
                ..
            } => {
                let legacy = is_legacy_function_call(*tool_type, extra_body);
                self.open_call(state, *tool_type, legacy, call_id, name, extra_body)
                    .await?;
            }
            NodeHeader::Text {
                phase: Some(phase), ..
            } => self.wire.set_delta_member("phase", json!(phase)),
            NodeHeader::Audio { .. } => state.message_audio = is_message_audio(extra_body),
            _ => {}
        }
        Ok(())
    }

    /// STR-15: every fragment of `node` that no earlier event emitted. Completing a node again
    /// with the same content emits nothing.
    async fn complete_node(&mut self, state: &mut NodeState, node: &Node) -> AppResult<()> {
        match node {
            Node::Text { .. } | Node::Refusal { .. } => {
                self.complete_text(&mut state.text, node).await
            }
            Node::Reasoning { .. } => self.complete_reasoning(&mut state.reasoning, node).await,
            Node::ToolCall {
                tool_type,
                call_id,
                name,
                arguments,
                extra_body,
                ..
            } => {
                let shape = match state.call {
                    Some(shape) => shape,
                    None => {
                        let legacy = is_legacy_function_call(*tool_type, extra_body);
                        self.open_call(state, *tool_type, legacy, call_id, name, &HashMap::new())
                            .await?
                    }
                };
                // Arguments that diverged from the emitted prefix cannot be retracted.
                let Some(suffix) = arguments
                    .strip_prefix(state.arguments.as_str())
                    .filter(|suffix| !suffix.is_empty())
                else {
                    return Ok(());
                };
                self.send_arguments(shape, suffix, &HashMap::new()).await?;
                state.arguments.push_str(suffix);
                Ok(())
            }
            Node::Image { .. } | Node::File { .. } | Node::Audio { .. } if !state.emitted => {
                state.emitted = true;
                let delta = encode_assistant_chat_message_from_nodes(std::slice::from_ref(node));
                self.wire.delta(Value::Object(delta)).await
            }
            Node::ProviderItem {
                origin_protocol: ProviderProtocol::ChatCompletion,
                ..
            } if !state.emitted => {
                state.emitted = true;
                let delta = encode_assistant_chat_message_from_nodes(std::slice::from_ref(node));
                let delta = self.wire.with_pending_extra(Value::Object(delta));
                self.wire.delta(delta).await
            }
            Node::NextDownstreamEnvelopeExtra { extra_body } if !state.emitted => {
                state.emitted = true;
                self.wire.stash_delta_extra(extra_body);
                Ok(())
            }
            _ => Ok(()),
        }
    }

    /// Emits a call header and fixes the call shape (CHAT-7, CHAT-7a, PC2.9).
    async fn open_call(
        &mut self,
        state: &mut NodeState,
        tool_type: ToolCallType,
        legacy: bool,
        call_id: &str,
        name: &str,
        event_extra: &HashMap<String, Value>,
    ) -> AppResult<CallShape> {
        let shape = if legacy {
            self.saw_function_call = true;
            CallShape::LegacyFunction
        } else {
            self.saw_tool_call = true;
            let index = self.next_tool_index;
            self.next_tool_index += 1;
            CallShape::Tool { index, tool_type }
        };
        state.call = Some(shape);
        let delta = self
            .wire
            .with_event_extra(shape.header(call_id, name), event_extra);
        self.wire.delta(delta).await?;
        Ok(shape)
    }

    async fn send_arguments(
        &mut self,
        shape: CallShape,
        arguments: &str,
        event_extra: &HashMap<String, Value>,
    ) -> AppResult<()> {
        let (template, patch) = shape.arguments();
        let delta = self.wire.with_event_extra(template, event_extra);
        self.wire.delta_string(delta, arguments, patch).await
    }
}
