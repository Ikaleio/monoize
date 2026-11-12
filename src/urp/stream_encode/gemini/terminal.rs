//! The terminal snapshot of a live stream (GEM-9, GEM-10, STR-8d): `ResponseDone.output`
//! reconciled with the Parts already on the wire, then the terminal frame.

use super::GeminiStreamEncoder;
use crate::urp::encode::gemini::{
    encode_response_checked, encode_response_node_parts, is_prompt_block_response,
};
use crate::urp::{
    FinishReason, Node, ResponseOutcome, ResponseStatus, TerminalNode, UrpResponse, Usage,
};
use serde_json::Value;
use std::collections::{HashMap, HashSet};

impl GeminiStreamEncoder {
    pub(super) fn on_response_done(
        &mut self,
        outcome: Option<ResponseOutcome>,
        finish_reason: Option<FinishReason>,
        usage: Option<Usage>,
        output: Vec<TerminalNode>,
        extra_body: HashMap<String, Value>,
    ) -> Result<Vec<Value>, String> {
        if outcome.as_ref().is_some_and(|outcome| {
            matches!(
                outcome.status,
                ResponseStatus::InProgress | ResponseStatus::Queued
            )
        }) {
            return Err("Gemini terminal output cannot have an active response outcome".into());
        }
        // GEM-10: a completed snapshot without a finish reason ends with STOP.
        let finish_reason = finish_reason.or_else(|| {
            outcome
                .as_ref()
                .is_none_or(|outcome| outcome.status == ResponseStatus::Completed)
                .then_some(FinishReason::Stop)
        });
        let claims: Vec<Option<u32>> =
            TerminalNode::resolve(&output, |index| self.nodes.contains_key(&index))
                .map(|(claim, _)| claim)
                .collect();
        let response = UrpResponse {
            outcome,
            id: self.id.clone(),
            model: self.options.logical_model.clone(),
            created_at: None,
            output: TerminalNode::into_nodes(output),
            finish_reason,
            usage,
            extra_body,
        };
        let mut terminal = encode_response_checked(&response, &self.options.logical_model)?;
        if terminal.get("error").is_some() {
            return Ok(vec![terminal]);
        }
        self.check_claims(&claims)?;
        let mut frames = Vec::new();
        if is_prompt_block_response(&response) {
            if !self.wire.is_empty() {
                return Err("Gemini cannot retract emitted content for a prompt block".into());
            }
        } else {
            for content in self.emit_terminal_nodes(&claims, &response.output)? {
                frames.push(self.frame(content));
            }
        }
        if let Some(candidate) = terminal
            .get_mut("candidates")
            .and_then(Value::as_array_mut)
            .and_then(|candidates| candidates.first_mut())
            .and_then(Value::as_object_mut)
        {
            self.wire.rebase_grounding(candidate)?;
            candidate.remove("content");
        }
        frames.push(terminal);
        Ok(frames)
    }

    /// Every node with Parts on the wire stays in the snapshot in wire order, because an emitted
    /// Part cannot be retracted or moved.
    fn check_claims(&self, claims: &[Option<u32>]) -> Result<(), String> {
        let mut previous = None;
        for index in claims
            .iter()
            .flatten()
            .copied()
            .filter(|index| self.wire.has_emitted(*index))
        {
            if previous.is_some_and(|previous| previous > index) {
                return Err("Gemini cannot reorder Parts that were already emitted".into());
            }
            previous = Some(index);
        }
        let claimed: HashSet<u32> = claims.iter().flatten().copied().collect();
        if self.wire.keys().any(|key| !claimed.contains(&key)) {
            return Err("Gemini cannot delete a Part that was already emitted".into());
        }
        Ok(())
    }

    /// Emits the terminal nodes in order. A node that claims Parts on the wire extends or keeps
    /// them; any other node is new on the wire and may only follow the last claimed one.
    fn emit_terminal_nodes(
        &mut self,
        claims: &[Option<u32>],
        output: &[Node],
    ) -> Result<Vec<Value>, String> {
        // Taken before emission: a new node's wire key may equal a later entry's node index.
        let on_wire: Vec<Option<u32>> = claims
            .iter()
            .map(|claim| claim.filter(|index| self.wire.has_emitted(*index)))
            .collect();
        let last_claim = on_wire.iter().rposition(Option::is_some);
        let mut contents = Vec::new();
        for (position, (claim, node)) in on_wire.into_iter().zip(output).enumerate() {
            if let Some(index) = claim {
                contents.extend(self.wire.emit(index, node)?);
            } else if last_claim.is_some_and(|last| position < last) {
                if !encode_response_node_parts(node)?.is_empty() {
                    return Err("Gemini cannot insert a Part before a later emitted Part".into());
                }
            } else {
                let key = self.wire.next_key();
                contents.extend(self.wire.emit(key, node)?);
            }
        }
        Ok(contents)
    }
}
