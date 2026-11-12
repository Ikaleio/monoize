//! The Parts one Gemini stream put on the wire, and the reconciliation of later node versions
//! with them (GEM-9). An emitted Part cannot be amended, replaced, moved, or retracted; only an
//! unsigned plain text Part may grow by a suffix.

use crate::urp::decode::gemini::{append_fragment, decode_part};
use crate::urp::encode::gemini::{content_extra, encode_response_node_parts};
use crate::urp::{GEMINI_CONTENT_EXTRA_KEY, Node, OrdinaryRole, is_internal_key};
use serde_json::{Map, Value, json};
use std::collections::BTreeMap;
use std::ops::Bound::{Excluded, Unbounded};

/// The Parts on the wire for one node.
struct EmittedNode {
    /// Every Part emitted for the node in full form: accumulated text, not fragments.
    parts: Vec<Value>,
    /// Native Content members of the node's frames.
    envelope: Map<String, Value>,
    /// The node's first fragment continued the previous node's plain fragment of the same kind,
    /// so a client merged both into one Part.
    merged: bool,
}

/// Emitted Parts keyed in wire order. A streamed node uses its node index; a node first emitted
/// at the terminal boundary uses a key after every emitted key.
#[derive(Default)]
pub(super) struct WireState {
    emitted: BTreeMap<u32, EmittedNode>,
    /// `(thought, key)` of the last plain text fragment on the wire.
    last_fragment: Option<(bool, u32)>,
}

impl WireState {
    pub(super) fn has_emitted(&self, key: u32) -> bool {
        self.emitted.contains_key(&key)
    }

    pub(super) fn is_empty(&self) -> bool {
        self.emitted.is_empty()
    }

    pub(super) fn keys(&self) -> impl Iterator<Item = u32> + '_ {
        self.emitted.keys().copied()
    }

    /// The key of a node first emitted at the terminal boundary.
    pub(super) fn next_key(&self) -> u32 {
        self.emitted
            .last_key_value()
            .map_or(0, |(key, _)| key.saturating_add(1))
    }

    /// Reconciles `node` with the Parts emitted under `key`. Returns the frame `content` that
    /// carries what the wire lacks, or `None` when the wire already holds the node.
    pub(super) fn emit(&mut self, key: u32, node: &Node) -> Result<Option<Value>, String> {
        let parts = encode_response_node_parts(node)?;
        let later_emitted = self
            .emitted
            .range((Excluded(key), Unbounded))
            .next()
            .is_some();
        let previous = self.emitted.get(&key);
        let fragments = match previous {
            None if parts.is_empty() => return Ok(None),
            None if later_emitted => {
                return Err("Gemini cannot insert a Part before a later emitted Part".into());
            }
            None => parts.clone(),
            Some(_) if parts.is_empty() => {
                return Err("Gemini cannot delete a Part that was already emitted".into());
            }
            Some(previous) if previous.parts == parts => return Ok(None),
            Some(_) if later_emitted => {
                return Err("Gemini cannot modify a Part before a later emitted Part".into());
            }
            Some(previous) => vec![suffix_fragment(previous, &parts)?],
        };
        let envelope: Map<String, Value> = content_extra(node)
            .into_iter()
            .filter(|(key, _)| !is_internal_key(key) && !matches!(key.as_str(), "role" | "parts"))
            .collect();
        let merged = self.track_fragments(key, &fragments, envelope.is_empty());
        let mut content = envelope.clone();
        content.insert("role".into(), json!("model"));
        content.insert("parts".into(), Value::Array(fragments));
        let entry = self.emitted.entry(key).or_insert_with(|| EmittedNode {
            parts: Vec::new(),
            envelope: Map::new(),
            merged: false,
        });
        entry.parts = parts;
        entry.envelope = envelope;
        entry.merged |= merged;
        Ok(Some(Value::Object(content)))
    }

    /// Records the last plain fragment on the wire. Returns whether the first fragment continues
    /// the plain fragment of another node, which a client merges with it. Frames with native
    /// Content members never merge.
    fn track_fragments(&mut self, key: u32, fragments: &[Value], plain_envelope: bool) -> bool {
        let mut merged = false;
        for (index, part) in fragments.iter().enumerate() {
            let kind = plain_envelope.then(|| plain_fragment_kind(part)).flatten();
            if index == 0
                && let Some(kind) = kind
                && self
                    .last_fragment
                    .is_some_and(|(previous, owner)| previous == kind && owner != key)
            {
                merged = true;
            }
            self.last_fragment = kind.map(|kind| (kind, key));
        }
        merged
    }

    /// Rebases terminal grounding supports from non-stream Part positions to the Parts a client
    /// assembles from the emitted fragments, where adjacent plain fragments merge (GEM-6, PG5b).
    pub(super) fn rebase_grounding(
        &self,
        candidate: &mut Map<String, Value>,
    ) -> Result<(), String> {
        let Some(supports) = candidate
            .get_mut("groundingMetadata")
            .and_then(|metadata| metadata.get_mut("groundingSupports"))
            .and_then(Value::as_array_mut)
        else {
            return Ok(());
        };
        // For each emitted Part: (assembled Part index, text bytes before it in that Part).
        let mut positions = Vec::new();
        let mut last: Option<Node> = None;
        let mut assembled = 0usize;
        for emitted in self.emitted.values() {
            for (index, part) in emitted.parts.iter().enumerate() {
                let mut node = decode_part(part, OrdinaryRole::Assistant)?;
                if index == 0 && !emitted.envelope.is_empty() {
                    node.extra_body_mut().insert(
                        GEMINI_CONTENT_EXTRA_KEY.into(),
                        Value::Object(emitted.envelope.clone()),
                    );
                }
                let prefix = match &last {
                    Some(Node::Text { content, .. }) => content.len() as u64,
                    _ => 0,
                };
                if index == 0
                    && last
                        .as_mut()
                        .is_some_and(|last| append_fragment(last, &mut node))
                {
                    positions.push((assembled - 1, prefix));
                } else {
                    last = Some(node);
                    assembled += 1;
                    positions.push((assembled - 1, 0));
                }
            }
        }
        for support in supports {
            let Some(segment) = support.get_mut("segment").and_then(Value::as_object_mut) else {
                continue;
            };
            let native_part = segment
                .get("partIndex")
                .and_then(Value::as_u64)
                .unwrap_or(0) as usize;
            let (part, offset) = positions
                .get(native_part)
                .ok_or("Gemini grounding references a Part that was not emitted")?;
            let start = segment
                .get("startIndex")
                .and_then(Value::as_u64)
                .unwrap_or(0)
                .checked_add(*offset)
                .ok_or("Gemini grounding start index overflow")?;
            let end = segment
                .get("endIndex")
                .and_then(Value::as_u64)
                .and_then(|end| end.checked_add(*offset))
                .ok_or("Gemini grounding end index is invalid")?;
            segment.insert("partIndex".into(), Value::from(*part));
            segment.insert("startIndex".into(), Value::from(start));
            segment.insert("endIndex".into(), Value::from(end));
        }
        Ok(())
    }
}

/// The fragment that extends an emitted unsigned text Part to `parts`: the same Part shape with
/// only the new text suffix. A late signature travels on the suffix Part.
fn suffix_fragment(previous: &EmittedNode, parts: &[Value]) -> Result<Value, String> {
    let ([old], [new]) = (previous.parts.as_slice(), parts) else {
        return Err("Gemini cannot replace Parts that were already emitted".into());
    };
    let old_signed = old.get("thoughtSignature").is_some();
    if !old_signed && new.get("thoughtSignature").is_some() && previous.merged {
        return Err(
            "Gemini cannot attach a late signature after its text merged with an earlier Part"
                .into(),
        );
    }
    let shape = |part: &Value| {
        let mut shape = part.clone();
        if let Some(obj) = shape.as_object_mut() {
            obj.remove("text");
            obj.remove("thoughtSignature");
        }
        shape
    };
    match (
        old.get("text").and_then(Value::as_str),
        new.get("text").and_then(Value::as_str),
    ) {
        (Some(prefix), Some(text))
            if !old_signed && text.starts_with(prefix) && shape(old) == shape(new) =>
        {
            let mut fragment = new.clone();
            fragment["text"] = json!(&text[prefix.len()..]);
            Ok(fragment)
        }
        _ => Err("Gemini cannot replace a Part that was already emitted".into()),
    }
}

/// `Some(thought)` for a Part that holds only text (and the thought flag): a fragment a client
/// appends to an adjacent fragment of the same kind.
fn plain_fragment_kind(part: &Value) -> Option<bool> {
    let part = part.as_object()?;
    part.get("text")?.as_str()?;
    let thought = part.get("thought").and_then(Value::as_bool) == Some(true);
    part.keys()
        .all(|key| key == "text" || (thought && key == "thought"))
        .then_some(thought)
}
