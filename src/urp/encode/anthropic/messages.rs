//! Request conversation assembly: top-level `system` blocks and the `messages` array.
//!
//! Messages keeps its own boundary rule (MSG-*): consecutive blocks of one wire role share
//! a message, tool results open a `user` message of their own, and native system envelopes
//! are reopened from their control node.

use super::blocks::{encode_request_block, encode_tool_result_block};
use super::reasoning::with_merged_chat_reasoning;
use super::system::{DeferredSystemBlock, encode_system_block, system_tool_change_note};
use crate::urp::encode::merge_extra;
use crate::urp::{
    MESSAGES_SYSTEM_ENVELOPE_EXTRA_KEY, Node, OrdinaryRole, ProviderProtocol, ToolCallType,
    ToolResultContent,
};
use serde_json::{Map, Value};
use std::collections::HashMap;

/// Encodes request nodes into `(system, messages)`.
pub(super) fn encode_conversation(nodes: &[Node]) -> (Vec<Value>, Vec<Value>) {
    let mut conversation = Conversation::default();
    for node in with_merged_chat_reasoning(nodes) {
        conversation.push(&node);
    }
    conversation.finish()
}

struct MessageEnvelope {
    role: OrdinaryRole,
    content: Vec<Value>,
    extra_body: HashMap<String, Value>,
    /// Mid-conversation instruction text rendered inside a user message (MSG-1a).
    system_reminders: Vec<Value>,
}

impl MessageEnvelope {
    fn new(role: OrdinaryRole, extra_body: HashMap<String, Value>) -> Self {
        Self {
            role,
            content: Vec::new(),
            extra_body,
            system_reminders: Vec::new(),
        }
    }

    fn render(self) -> Option<Value> {
        if self.content.is_empty()
            && self.system_reminders.is_empty()
            && (self.role != OrdinaryRole::System || self.extra_body.is_empty())
        {
            return None;
        }

        let mut content = self.content;
        if !self.system_reminders.is_empty() {
            let insert_at = content
                .iter()
                .take_while(|block| {
                    block.get("type").and_then(Value::as_str) == Some("tool_result")
                })
                .count();
            content.splice(insert_at..insert_at, self.system_reminders);
        }
        let role = match self.role {
            OrdinaryRole::Assistant => "assistant",
            OrdinaryRole::System | OrdinaryRole::Developer => {
                if let Some(note) = system_tool_change_note(&content) {
                    content.insert(0, note);
                }
                "system"
            }
            OrdinaryRole::User => "user",
        };
        let mut message = Map::new();
        message.insert("role".to_string(), Value::String(role.to_string()));
        message.insert("content".to_string(), Value::Array(content));
        merge_extra(&mut message, &self.extra_body);
        Some(Value::Object(message))
    }
}

#[derive(Default)]
struct Conversation {
    system: Vec<Value>,
    messages: Vec<Value>,
    pending: Option<MessageEnvelope>,
    /// Wire members of the next message, from its control node.
    pending_envelope_extra: HashMap<String, Value>,
    /// Instruction blocks that arrived after the conversation started.
    deferred_system: Vec<DeferredSystemBlock>,
}

impl Conversation {
    fn push(&mut self, node: &Node) {
        if !self.deferred_system.is_empty() {
            self.place_deferred_system_before(node);
        }
        match node {
            Node::NextDownstreamEnvelopeExtra { extra_body } => {
                self.push_envelope_control(extra_body)
            }
            Node::ToolResult {
                namespace,
                tool_type: ToolCallType::Function,
                call_id,
                content,
                is_error,
                extra_body,
                ..
            } => self.push_tool_result(
                call_id,
                namespace.as_deref(),
                content,
                *is_error,
                extra_body,
            ),
            Node::ToolResult { .. } => {}
            Node::Text {
                role: OrdinaryRole::System | OrdinaryRole::Developer,
                ..
            }
            | Node::ProviderItem {
                role: OrdinaryRole::System | OrdinaryRole::Developer,
                origin_protocol: ProviderProtocol::Messages,
                ..
            } => self.push_instruction(node),
            Node::Text {
                role: OrdinaryRole::User | OrdinaryRole::Assistant,
                ..
            }
            | Node::Image {
                role: OrdinaryRole::User | OrdinaryRole::Assistant,
                ..
            }
            | Node::File {
                role: OrdinaryRole::User | OrdinaryRole::Assistant,
                ..
            }
            | Node::ProviderItem {
                role: OrdinaryRole::User | OrdinaryRole::Assistant,
                ..
            }
            | Node::Reasoning { .. }
            | Node::ToolCall { .. } => self.push_content(node),
            // Instruction media and foreign instruction items have no Messages form; they
            // still end the open message.
            Node::Image {
                role: OrdinaryRole::System | OrdinaryRole::Developer,
                ..
            }
            | Node::File {
                role: OrdinaryRole::System | OrdinaryRole::Developer,
                ..
            }
            | Node::ProviderItem {
                role: OrdinaryRole::System | OrdinaryRole::Developer,
                ..
            } => self.flush_pending(),
            Node::Audio { .. } | Node::Refusal { .. } => {}
        }
        self.attach_deferred_reminders();
    }

    fn finish(mut self) -> (Vec<Value>, Vec<Value>) {
        self.flush_pending();
        self.flush_deferred_system();
        (self.system, self.messages)
    }

    fn flush_pending(&mut self) {
        if let Some(message) = self.pending.take().and_then(MessageEnvelope::render) {
            self.messages.push(message);
        }
    }

    fn push_envelope_control(&mut self, extra_body: &HashMap<String, Value>) {
        self.flush_pending();
        for (key, value) in extra_body {
            if key != MESSAGES_SYSTEM_ENVELOPE_EXTRA_KEY {
                self.pending_envelope_extra
                    .insert(key.clone(), value.clone());
            }
        }
        if opens_native_system_envelope(extra_body) {
            self.pending = Some(MessageEnvelope::new(
                OrdinaryRole::System,
                std::mem::take(&mut self.pending_envelope_extra),
            ));
        }
    }

    fn push_tool_result(
        &mut self,
        call_id: &str,
        namespace: Option<&str>,
        content: &[ToolResultContent],
        is_error: bool,
        extra_body: &HashMap<String, Value>,
    ) {
        let closes_pending = self.pending.as_ref().is_some_and(|existing| {
            existing.role != OrdinaryRole::User
                || existing
                    .content
                    .iter()
                    .any(|block| block.get("type").and_then(Value::as_str) != Some("tool_result"))
        });
        if closes_pending {
            self.flush_pending();
        }
        let block = encode_tool_result_block(call_id, namespace, content, is_error, extra_body);
        self.open_message(OrdinaryRole::User).content.push(block);
    }

    fn push_content(&mut self, node: &Node) {
        let Some(role) = message_role_for_node(node) else {
            return;
        };
        if self
            .pending
            .as_ref()
            .is_some_and(|existing| existing.role != role)
        {
            self.flush_pending();
        }
        if let Some(block) = encode_request_block(node) {
            self.open_message(role).content.push(block);
        }
    }

    /// Places one system or developer block: inside an open native system envelope, at the
    /// top-level `system` before the conversation starts, and deferred afterwards.
    fn push_instruction(&mut self, node: &Node) {
        if let Some(message) = self
            .pending
            .as_mut()
            .filter(|message| message.role == OrdinaryRole::System)
        {
            if let Some(block) = encode_system_block(node) {
                message.content.push(block);
            }
        } else if !self.messages.is_empty() || self.pending.is_some() {
            self.flush_pending();
            if let Some(block) = encode_system_block(node) {
                let text = match node {
                    Node::Text { content, .. } => Some(content.clone()),
                    _ => None,
                };
                self.deferred_system
                    .push(DeferredSystemBlock { block, text });
            }
        } else if let Some(block) = encode_system_block(node) {
            self.system.push(block);
        }
    }

    /// Returns the open message, opening one of `role` with the pending envelope members.
    fn open_message(&mut self, role: OrdinaryRole) -> &mut MessageEnvelope {
        let extra_body = &mut self.pending_envelope_extra;
        self.pending
            .get_or_insert_with(|| MessageEnvelope::new(role, std::mem::take(extra_body)))
    }

    /// Emits deferred instructions as a `system` message unless `node` opens a user message
    /// that can carry them as reminders.
    fn place_deferred_system_before(&mut self, node: &Node) {
        let target_role = match node {
            Node::NextDownstreamEnvelopeExtra { extra_body } => {
                opens_native_system_envelope(extra_body).then_some(OrdinaryRole::System)
            }
            Node::ToolResult {
                tool_type: ToolCallType::Function,
                ..
            } => Some(OrdinaryRole::User),
            _ => message_role_for_node(node),
        };
        let reminders_fit_user_message = self
            .deferred_system
            .iter()
            .all(|deferred| deferred.text.is_some());
        if target_role.is_some_and(|role| role != OrdinaryRole::User || !reminders_fit_user_message)
        {
            self.flush_deferred_system();
        }
    }

    fn flush_deferred_system(&mut self) {
        if self.deferred_system.is_empty() {
            return;
        }
        let mut message = MessageEnvelope::new(OrdinaryRole::System, HashMap::new());
        message.content = self
            .deferred_system
            .drain(..)
            .map(|deferred| deferred.block)
            .collect();
        if let Some(message) = message.render() {
            self.messages.push(message);
        }
    }

    /// Moves deferred instruction text into the open user message when every deferred block
    /// is text; Anthropic rejects a `system` message that precedes a `user` message.
    fn attach_deferred_reminders(&mut self) {
        let Some(message) = self
            .pending
            .as_mut()
            .filter(|message| message.role == OrdinaryRole::User)
        else {
            return;
        };
        if self.deferred_system.is_empty()
            || self
                .deferred_system
                .iter()
                .any(|deferred| deferred.text.is_none())
        {
            return;
        }
        message.system_reminders.extend(
            self.deferred_system
                .drain(..)
                .map(DeferredSystemBlock::into_reminder),
        );
    }
}

fn opens_native_system_envelope(extra_body: &HashMap<String, Value>) -> bool {
    extra_body.get(MESSAGES_SYSTEM_ENVELOPE_EXTRA_KEY) == Some(&Value::Bool(true))
}

/// Wire role of the message that carries a content node; `None` for nodes that never
/// occupy a `user` or `assistant` message.
fn message_role_for_node(node: &Node) -> Option<OrdinaryRole> {
    match node {
        Node::Text { role, .. }
        | Node::Image { role, .. }
        | Node::File { role, .. }
        | Node::ProviderItem {
            role,
            origin_protocol: ProviderProtocol::Messages,
            ..
        } => match role {
            OrdinaryRole::System | OrdinaryRole::Developer => None,
            OrdinaryRole::User | OrdinaryRole::Assistant => Some(*role),
        },
        Node::Reasoning { .. } | Node::ToolCall { .. } => Some(OrdinaryRole::Assistant),
        Node::ToolResult { .. }
        | Node::NextDownstreamEnvelopeExtra { .. }
        | Node::Audio { .. }
        | Node::Refusal { .. }
        | Node::ProviderItem { .. } => None,
    }
}
