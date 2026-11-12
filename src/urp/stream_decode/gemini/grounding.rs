//! Streamed grounding accumulation (GEM-6, PG5b2). Chunks accumulate in arrival order; each
//! support segment is resolved against the decoded output when its frame arrives.

use crate::urp::Node;
use serde_json::{Map, Value};
use std::collections::HashMap;

/// The output position of the first node of each native Part of one frame:
/// `(output node index, text bytes the node held before this frame's fragment)`.
pub(super) type FrameParts = HashMap<usize, (usize, u64)>;

#[derive(Default)]
pub(super) struct StreamGrounding {
    chunks: Vec<Value>,
    supports: Vec<Value>,
    metadata: Map<String, Value>,
}

impl StreamGrounding {
    pub(super) fn ingest(
        &mut self,
        candidate: &Map<String, Value>,
        frame_parts: &FrameParts,
        output: &[Node],
    ) {
        let Some(grounding) = candidate
            .get("groundingMetadata")
            .and_then(Value::as_object)
        else {
            return;
        };
        if let Some(chunks) = grounding.get("groundingChunks").and_then(Value::as_array) {
            self.chunks.extend(chunks.iter().cloned());
        }
        self.metadata.extend(
            grounding
                .iter()
                .filter(|(key, _)| !matches!(key.as_str(), "groundingChunks" | "groundingSupports"))
                .map(|(key, value)| (key.clone(), value.clone())),
        );
        for support in grounding
            .get("groundingSupports")
            .and_then(Value::as_array)
            .into_iter()
            .flatten()
        {
            let mut support = support.clone();
            let Some(segment) = support.get_mut("segment").and_then(Value::as_object_mut) else {
                continue;
            };
            let part = segment
                .get("partIndex")
                .and_then(Value::as_u64)
                .unwrap_or(0) as usize;
            let start = segment
                .get("startIndex")
                .and_then(Value::as_u64)
                .unwrap_or(0);
            let end = segment.get("endIndex").and_then(Value::as_u64);
            let text = segment
                .get("text")
                .and_then(Value::as_str)
                .filter(|text| !text.is_empty());
            let frame = frame_parts.get(&part).copied();
            // Gemini does not document whether streamed segment offsets are relative to the
            // frame Part or to the accumulated text. A segment `text` decides; without it the
            // frame-relative reading is used.
            let resolved = match text {
                Some(text) => resolve_segment(output, frame, start, text),
                None if frame_parts.is_empty() => {
                    self.supports.push(support);
                    continue;
                }
                None => frame.and_then(|(node, offset)| {
                    Some((node, start.checked_add(offset)?, end?.checked_add(offset)?))
                }),
            };
            let Some((node, start, end)) = resolved else {
                continue;
            };
            segment.insert("partIndex".into(), Value::from(node));
            segment.insert("startIndex".into(), Value::from(start));
            segment.insert("endIndex".into(), Value::from(end));
            self.supports.push(support);
        }
    }

    /// The accumulated grounding as a candidate fragment, or `None` when no frame carried any.
    pub(super) fn into_candidate(self) -> Option<Map<String, Value>> {
        if self.chunks.is_empty() && self.supports.is_empty() && self.metadata.is_empty() {
            return None;
        }
        let mut metadata = self.metadata;
        if !self.chunks.is_empty() {
            metadata.insert("groundingChunks".into(), Value::Array(self.chunks));
        }
        if !self.supports.is_empty() {
            metadata.insert("groundingSupports".into(), Value::Array(self.supports));
        }
        Some(Map::from_iter([(
            "groundingMetadata".into(),
            Value::Object(metadata),
        )]))
    }
}

/// Resolves a streamed grounding segment to `(output node index, start, end)` byte offsets by
/// checking which offset reading reproduces the segment text, falling back to a text search.
fn resolve_segment(
    output: &[Node],
    frame: Option<(usize, u64)>,
    start: u64,
    text: &str,
) -> Option<(usize, u64, u64)> {
    let matches = |node: usize, start: u64| -> bool {
        let Some(Node::Text { content, .. }) = output.get(node) else {
            return false;
        };
        let Ok(start) = usize::try_from(start) else {
            return false;
        };
        content
            .get(start..start.saturating_add(text.len()))
            .is_some_and(|slice| slice == text)
    };
    let length = text.len() as u64;
    if let Some((node, offset)) = frame {
        for candidate in [start.checked_add(offset), Some(start)]
            .into_iter()
            .flatten()
        {
            if matches(node, candidate) {
                return Some((node, candidate, candidate + length));
            }
        }
    }
    let preferred = frame.map(|(node, _)| node);
    preferred
        .into_iter()
        .chain(
            (0..output.len())
                .rev()
                .filter(|node| Some(*node) != preferred),
        )
        .find_map(|node| match output.get(node) {
            Some(Node::Text { content, .. }) => content
                .find(text)
                .map(|found| (node, found as u64, found as u64 + length)),
            _ => None,
        })
}
