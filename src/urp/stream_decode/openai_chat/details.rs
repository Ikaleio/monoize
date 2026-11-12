//! Chat `reasoning_details[]` entries: one reasoning node per entry (PC8), continued by
//! fragments of the same identity, with the PC7h rules for payloads that arrive after the node
//! completed.

use super::reasoning::snapshot_suffix;
use super::state::ChatStreamState;
use crate::error::AppResult;
use crate::urp::reasoning::{chat_details, chat_reasoning_node_from_detail, detail_metadata};
use crate::urp::{
    CHAT_REASONING_DETAIL_EXTRA_KEY, Node, NodeDelta, NodeHeader, ReasoningMetadata,
    is_internal_key,
};
use serde_json::{Map, Value};
use std::collections::HashMap;

pub(super) struct DetailNode {
    pub(super) node_index: u32,
    pub(super) node: Node,
    /// `NodeDone` was sent, so the node takes no more deltas (PC7h).
    pub(super) done: bool,
}

impl ChatStreamState<'_> {
    pub(super) async fn on_reasoning_details(&mut self, details: &[Value]) -> AppResult<()> {
        for detail in details {
            if is_encrypted_placeholder(detail) {
                self.held_placeholders.push(detail.clone());
                continue;
            }
            if !self.detail_continues_open_node(detail) {
                self.close_text().await?;
            }
            self.push_detail(detail).await?;
        }
        Ok(())
    }

    /// True when a fragment only extends an open node of its identity or carries no payload.
    fn detail_continues_open_node(&self, detail: &Value) -> bool {
        let Some(detail) = detail.as_object() else {
            return true;
        };
        self.details
            .iter()
            .any(|open| !open.done && detail_matches(&open.node, detail))
            || !["text", "summary", "data", "signature"]
                .iter()
                .any(|key| match detail.get(*key) {
                    Some(Value::String(value)) => !value.is_empty(),
                    Some(Value::Null) | None => false,
                    Some(_) => true,
                })
    }

    /// Appends a fragment to the open node it continues, or starts a node for it.
    pub(super) async fn push_detail(&mut self, detail: &Value) -> AppResult<()> {
        let Some(detail) = detail.as_object() else {
            return Ok(());
        };
        let Some(fragment) = chat_reasoning_node_from_detail(detail) else {
            return Ok(());
        };
        let Some((delta, fragment_extra)) = reasoning_delta(&fragment) else {
            return Ok(());
        };
        self.start_response(HashMap::new()).await?;
        if let Some(position) = self.continued_detail(detail) {
            let open = &mut self.details[position];
            if let Some(node) = append_fragment(&open.node, detail) {
                open.node = node;
            }
            let node_index = open.node_index;
            let mut event_extra = fragment_extra;
            event_extra.extend(self.take_delta_extra());
            return self.send_delta(node_index, delta, event_extra).await;
        }
        let node_index = self
            .start_node(NodeHeader::from(&fragment), fragment_extra.clone())
            .await?;
        let mut event_extra = fragment_extra;
        event_extra.extend(self.take_delta_extra());
        self.send_delta(node_index, delta, event_extra).await?;
        self.details.push(DetailNode {
            node_index,
            node: fragment,
            done: false,
        });
        Ok(())
    }

    /// The open node that a text or summary fragment continues: the node of the same identity,
    /// else (CHAT-4d) the latest node when both are unidentified entries of one type and format.
    fn continued_detail(&self, fragment: &Map<String, Value>) -> Option<usize> {
        if !matches!(
            fragment.get("type").and_then(Value::as_str),
            Some("reasoning.text" | "reasoning.summary")
        ) {
            return None;
        }
        self.details
            .iter()
            .position(|open| !open.done && detail_matches(&open.node, fragment))
            .or_else(|| {
                let last = self.details.len().checked_sub(1)?;
                let open = &self.details[last];
                (Some(open.node_index) == self.latest_node_index()
                    && !open.done
                    && detail_raw(&open.node)
                        .is_some_and(|existing| continues_unidentified(&existing, fragment)))
                .then_some(last)
            })
    }

    /// Reconciles a terminal `message.reasoning_details[]` snapshot with the streamed nodes by
    /// entry identity (`type`, `id`, `index`, `tool_call_id`, `format`).
    pub(super) async fn on_terminal_details(&mut self, details: &[Value]) -> AppResult<()> {
        let streamed = self.details.len();
        let mut claimed = vec![false; streamed];
        for detail in details {
            let Some(terminal) = detail.as_object() else {
                continue;
            };
            // One logical entry can span several nodes after a PC7h switch.
            let group: Vec<usize> = (0..streamed)
                .filter(|position| {
                    !claimed[*position] && detail_matches(&self.details[*position].node, terminal)
                })
                .collect();
            let Some(&last) = group.last() else {
                self.push_detail(detail).await?;
                continue;
            };
            for position in &group {
                claimed[*position] = true;
            }
            if self.details[last].done
                && terminal.get("type").and_then(Value::as_str) == Some("reasoning.encrypted")
            {
                // PC7h: terminal data that no completed segment carries becomes a new node.
                let data = encrypted_data(terminal);
                let carried = group.iter().any(|position| {
                    matches!(
                        &self.details[*position].node,
                        Node::Reasoning { encrypted, .. } if encrypted.as_ref() == data
                    )
                });
                if data.is_some() && !carried {
                    self.push_detail(detail).await?;
                }
                continue;
            }
            if let Some(terminal) =
                terminal_detail_for_last_segment(terminal, &group, &self.details)
            {
                self.complete_detail(last, terminal).await?;
            }
        }
        Ok(())
    }

    /// Applies the terminal entry to the last node of its group: the missing suffix or changed
    /// signature as a delta on an open node, or as a new node after a completed one (PC7h).
    async fn complete_detail(
        &mut self,
        position: usize,
        terminal: Map<String, Value>,
    ) -> AppResult<()> {
        let completion = detail_completion(&self.details[position].node, &terminal);
        if self.details[position].done {
            let Some((
                _,
                NodeDelta::Reasoning {
                    content,
                    encrypted,
                    summary,
                    ..
                },
                _,
            )) = completion
            else {
                return Ok(());
            };
            let mut fragment = terminal;
            let payload_key = fragment
                .get("type")
                .and_then(Value::as_str)
                .and_then(payload_member)
                .unwrap_or("text");
            fragment.insert(
                payload_key.to_string(),
                Value::String(content.or(summary).unwrap_or_default()),
            );
            if encrypted.is_none() {
                fragment.remove("signature");
            }
            return self.push_detail(&Value::Object(fragment)).await;
        }
        if let Some((merged, delta, mut event_extra)) = completion {
            event_extra.extend(self.take_delta_extra());
            let node_index = self.details[position].node_index;
            self.send_delta(node_index, delta, event_extra).await?;
            self.details[position].node = merged;
            return Ok(());
        }
        let open = &mut self.details[position];
        let mut merged = detail_raw(&open.node).unwrap_or_default();
        merged.extend(
            terminal
                .into_iter()
                .filter(|(key, _)| !is_internal_key(key)),
        );
        if let Some(node) = chat_reasoning_node_from_detail(&merged) {
            open.node = node;
        }
        Ok(())
    }

    /// PC7h: starts nodes for held placeholders that no entry of the same identity supplied.
    pub(super) async fn release_placeholders(&mut self) -> AppResult<()> {
        for detail in std::mem::take(&mut self.held_placeholders) {
            let supplied = detail.as_object().is_some_and(|detail| {
                self.details
                    .iter()
                    .any(|streamed| detail_matches(&streamed.node, detail))
            });
            if !supplied {
                self.push_detail(&detail).await?;
            }
        }
        Ok(())
    }
}

/// The reasoning delta that carries a fragment node's payload, with the node's extras.
fn reasoning_delta(node: &Node) -> Option<(NodeDelta, HashMap<String, Value>)> {
    let Node::Reasoning {
        metadata,
        id,
        content,
        encrypted,
        summary,
        source,
        extra_body,
    } = node
    else {
        return None;
    };
    let delta = NodeDelta::Reasoning {
        metadata: ReasoningMetadata {
            item_id: id.clone(),
            ..metadata.clone()
        },
        content: content.clone(),
        encrypted: encrypted.clone(),
        summary: summary.clone(),
        source: source.clone(),
    };
    Some((delta, extra_body.clone()))
}

/// The node of `existing` extended by a text or summary fragment of the same entry.
fn append_fragment(existing: &Node, fragment: &Map<String, Value>) -> Option<Node> {
    let mut accumulated = detail_raw(existing)?;
    let payload_key = payload_member(fragment.get("type").and_then(Value::as_str)?)?;
    let mut payload = accumulated
        .get(payload_key)
        .and_then(Value::as_str)
        .unwrap_or_default()
        .to_string();
    payload.push_str(
        fragment
            .get(payload_key)
            .and_then(Value::as_str)
            .unwrap_or_default(),
    );
    accumulated.extend(fragment.clone());
    accumulated.insert(payload_key.to_string(), Value::String(payload));
    chat_reasoning_node_from_detail(&accumulated)
}

/// The native entry of a detail node, rebuilt from its typed payload and entry metadata.
fn detail_raw(node: &Node) -> Option<Map<String, Value>> {
    let Node::Reasoning {
        id,
        content,
        summary,
        encrypted,
        source,
        extra_body,
        ..
    } = node
    else {
        return None;
    };
    let native = extra_body
        .get(CHAT_REASONING_DETAIL_EXTRA_KEY)?
        .as_object()?;
    chat_details(
        content.as_deref(),
        summary.as_deref(),
        encrypted.as_ref(),
        id.as_deref(),
        source.as_deref(),
        native,
    )
    .into_iter()
    .next()
}

fn payload_member(detail_type: &str) -> Option<&'static str> {
    match detail_type {
        "reasoning.text" => Some("text"),
        "reasoning.summary" => Some("summary"),
        "reasoning.encrypted" => Some("data"),
        _ => None,
    }
}

fn has_identity(detail: &Map<String, Value>) -> bool {
    detail
        .get("id")
        .and_then(Value::as_str)
        .is_some_and(|id| !id.is_empty())
        || detail.get("index").and_then(Value::as_u64).is_some()
}

/// Whether a fragment or terminal entry has the identity of an identified streamed entry.
fn identity_matches(existing: &Map<String, Value>, other: &Map<String, Value>) -> bool {
    has_identity(existing)
        && existing.get("type") == other.get("type")
        && ["id", "index", "tool_call_id"]
            .iter()
            .all(|key| existing.get(*key) == other.get(*key))
        && match (existing.get("format"), other.get("format")) {
            (Some(left), Some(right)) => left == right,
            _ => true,
        }
}

fn detail_matches(existing: &Node, other: &Map<String, Value>) -> bool {
    detail_raw(existing).is_some_and(|existing| identity_matches(&existing, other))
}

/// CHAT-4d fallback: a fragment without `id` and `index` continues the latest node when that
/// node is an unidentified entry of the same type and format.
fn continues_unidentified(existing: &Map<String, Value>, fragment: &Map<String, Value>) -> bool {
    !has_identity(existing)
        && !has_identity(fragment)
        && existing.get("type") == fragment.get("type")
        && existing.get("format") == fragment.get("format")
}

/// Non-empty encrypted `data` of an entry.
fn encrypted_data(detail: &Map<String, Value>) -> Option<&Value> {
    detail.get("data").filter(|data| match data {
        Value::Null => false,
        Value::String(data) => !data.is_empty(),
        _ => true,
    })
}

/// PC7h: a `reasoning.encrypted` entry whose data has not arrived yet.
fn is_encrypted_placeholder(detail: &Value) -> bool {
    detail.get("type").and_then(Value::as_str) == Some("reasoning.encrypted")
        && match detail.get("data") {
            None | Some(Value::Null) => true,
            Some(Value::String(data)) => data.is_empty(),
            Some(_) => false,
        }
}

/// PC7h: an unsigned Anthropic thinking entry stays open, because its signature must reach the
/// same node before a Messages downstream closes the block.
pub(super) fn detail_awaits_signature(node: &Node) -> bool {
    let Node::Reasoning {
        encrypted,
        source,
        extra_body,
        ..
    } = node
    else {
        return false;
    };
    encrypted.is_none()
        && source
            .as_deref()
            .is_some_and(|source| source.starts_with("anthropic"))
        && extra_body
            .get(CHAT_REASONING_DETAIL_EXTRA_KEY)
            .and_then(|detail| detail.get("type"))
            .and_then(Value::as_str)
            == Some("reasoning.text")
}

/// The terminal entry as seen by the last node of its group, with the payload of the earlier
/// segments removed. `None` when the snapshot does not extend the earlier segments.
fn terminal_detail_for_last_segment(
    terminal: &Map<String, Value>,
    group: &[usize],
    details: &[DetailNode],
) -> Option<Map<String, Value>> {
    let mut terminal = terminal.clone();
    let Some((_, earlier)) = group.split_last() else {
        return Some(terminal);
    };
    if earlier.is_empty() {
        return Some(terminal);
    }
    let payload_key = terminal
        .get("type")
        .and_then(Value::as_str)
        .and_then(payload_member)?;
    let prior: String = earlier
        .iter()
        .filter_map(|position| detail_raw(&details[*position].node))
        .filter_map(|raw| {
            raw.get(payload_key)
                .and_then(Value::as_str)
                .map(str::to_string)
        })
        .collect();
    let rest = terminal
        .get(payload_key)?
        .as_str()?
        .strip_prefix(prior.as_str())?
        .to_string();
    terminal.insert(payload_key.to_string(), Value::String(rest));
    Some(terminal)
}

/// The node merged with its terminal entry, plus the delta and event extras that carry what
/// the stream lacked. `None` when the terminal entry adds no payload.
fn detail_completion(
    existing: &Node,
    terminal: &Map<String, Value>,
) -> Option<(Node, NodeDelta, HashMap<String, Value>)> {
    let existing_raw = detail_raw(existing)?;
    let payload_key = payload_member(terminal.get("type").and_then(Value::as_str)?)?;
    let mut merged_raw = existing_raw.clone();
    merged_raw.extend(
        terminal
            .iter()
            .filter(|(key, _)| !is_internal_key(key))
            .map(|(key, value)| (key.clone(), value.clone())),
    );
    let merged = chat_reasoning_node_from_detail(&merged_raw)?;
    let (content, encrypted, summary, payload) = match payload_key {
        "text" => {
            let suffix = snapshot_suffix(
                str_member(&existing_raw, "text"),
                str_member(&merged_raw, "text"),
            )
            .map(str::to_string);
            // PC8: a signature first seen (or changed) in the snapshot has no other delta.
            let signature = match (existing, &merged) {
                (
                    Node::Reasoning { encrypted: old, .. },
                    Node::Reasoning { encrypted: new, .. },
                ) if new != old => new.clone(),
                _ => None,
            };
            if suffix.is_none() && signature.is_none() {
                return None;
            }
            let payload = Value::String(suffix.clone().unwrap_or_default());
            (suffix, signature, None, payload)
        }
        "data" => {
            // A terminal snapshot replaces encrypted data atomically.
            let data = encrypted_data(terminal)?;
            if matches!(existing, Node::Reasoning { encrypted, .. } if encrypted.as_ref() == Some(data))
            {
                return None;
            }
            (None, Some(data.clone()), None, data.clone())
        }
        _ => {
            let suffix = snapshot_suffix(
                str_member(&existing_raw, "summary"),
                str_member(&merged_raw, "summary"),
            )?
            .to_string();
            let payload = Value::String(suffix.clone());
            (None, None, Some(suffix), payload)
        }
    };
    let mut delta_detail: Map<String, Value> = terminal
        .iter()
        .filter(|(key, _)| !is_internal_key(key))
        .map(|(key, value)| (key.clone(), value.clone()))
        .collect();
    delta_detail.insert(payload_key.to_string(), payload);
    let extra_body = HashMap::from([(
        CHAT_REASONING_DETAIL_EXTRA_KEY.to_string(),
        Value::Object(detail_metadata(&delta_detail)),
    )]);
    let delta = NodeDelta::Reasoning {
        metadata: ReasoningMetadata {
            item_id: terminal
                .get("id")
                .and_then(Value::as_str)
                .map(str::to_string),
            ..Default::default()
        },
        content,
        encrypted,
        summary,
        source: terminal
            .get("format")
            .and_then(Value::as_str)
            .map(str::to_string),
    };
    Some((merged, delta, extra_body))
}

fn str_member<'m>(raw: &'m Map<String, Value>, key: &str) -> &'m str {
    raw.get(key).and_then(Value::as_str).unwrap_or_default()
}

/// Characters of reasoning text and summary in the detail nodes, for output estimation.
pub(super) fn detail_text_len(details: &[DetailNode]) -> usize {
    details
        .iter()
        .map(|detail| match &detail.node {
            Node::Reasoning {
                content, summary, ..
            } => content.as_deref().map_or(0, str::len) + summary.as_deref().map_or(0, str::len),
            _ => 0,
        })
        .sum()
}
