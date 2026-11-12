//! Candidate members accumulated across stream frames: merged metadata (finish reason, token
//! scores, continuation token), per-frame citation metadata, and grounding (GEM-6, PG5b1).

use super::grounding::{FrameParts, StreamGrounding};
use crate::urp::decode::gemini::candidate_extra;
use crate::urp::{GEMINI_CANDIDATE_EXTRA_KEY, Node, TokenLogprob, citations, logprobs};
use serde_json::{Map, Value, json};
use std::collections::HashMap;

#[derive(Default)]
pub(super) struct CandidateAccumulator {
    /// Latest value of every non-content candidate member; `logprobsResult` fragments append.
    metadata: Map<String, Value>,
    /// The `citationMetadata` of each frame, applied in arrival order at the terminal boundary.
    citation_frames: Vec<Map<String, Value>>,
    grounding: StreamGrounding,
}

impl CandidateAccumulator {
    pub(super) fn metadata(&self) -> &Map<String, Value> {
        &self.metadata
    }

    /// Records one frame's selected candidate. Members no typed field owns accumulate under
    /// [`GEMINI_CANDIDATE_EXTRA_KEY`] in the response extras.
    pub(super) fn ingest(
        &mut self,
        candidate: &Map<String, Value>,
        frame_parts: &FrameParts,
        output: &[Node],
        extra_body: &mut HashMap<String, Value>,
    ) {
        if let Some(metadata) = candidate.get("citationMetadata") {
            self.citation_frames.push(Map::from_iter([(
                "citationMetadata".to_string(),
                metadata.clone(),
            )]));
        }
        self.grounding.ingest(candidate, frame_parts, output);
        self.merge_metadata(candidate, output);
        let metadata = candidate_extra(candidate);
        if !metadata.is_empty()
            && let Some(stored) = stored_candidate_extra(extra_body)
        {
            stored.extend(metadata);
        }
    }

    fn merge_metadata(&mut self, candidate: &Map<String, Value>, output: &[Node]) {
        for (key, value) in candidate {
            if key == "content" {
                continue;
            }
            if key != "logprobsResult" {
                self.metadata.insert(key.clone(), value.clone());
                continue;
            }
            // A cumulative score snapshot replaces earlier fragments; a per-frame fragment
            // appends to them.
            let text: String = output
                .iter()
                .filter_map(|node| match node {
                    Node::Text { content, .. } => Some(content.as_str()),
                    _ => None,
                })
                .collect();
            let tokens: String = value
                .get("chosenCandidates")
                .and_then(Value::as_array)
                .into_iter()
                .flatten()
                .filter_map(|score| score.get("token").and_then(Value::as_str))
                .collect();
            if tokens == text || !self.metadata.contains_key(key) {
                self.metadata.insert(key.clone(), value.clone());
            } else if let (Some(previous), Some(next)) = (
                self.metadata.get_mut(key).and_then(Value::as_object_mut),
                value.as_object(),
            ) {
                for (field, value) in next {
                    if !matches!(field.as_str(), "chosenCandidates" | "topCandidates") {
                        previous.insert(field.clone(), value.clone());
                    } else if let Some(values) = value.as_array()
                        && let Some(stored) = previous
                            .entry(field.clone())
                            .or_insert_with(|| Value::Array(Vec::new()))
                            .as_array_mut()
                    {
                        stored.extend(values.iter().cloned());
                    }
                }
            }
        }
    }

    /// Applies accumulated citations and grounding to the terminal output. Grounding
    /// presentation metadata replaces the per-frame copy in the response extras.
    pub(super) fn attach_citations(
        &mut self,
        output: &mut [Node],
        extra_body: &mut HashMap<String, Value>,
    ) {
        for candidate in std::mem::take(&mut self.citation_frames) {
            citations::attach_gemini(&candidate, output);
        }
        let Some(candidate) = std::mem::take(&mut self.grounding).into_candidate() else {
            return;
        };
        citations::attach_gemini(&candidate, output);
        let Some(stored) = stored_candidate_extra(extra_body) else {
            return;
        };
        match citations::gemini_metadata_extra(&candidate) {
            Some(metadata) => {
                stored.insert("groundingMetadata".into(), metadata);
            }
            None => {
                stored.remove("groundingMetadata");
            }
        }
    }

    /// Applies the merged candidate token scores to the terminal output.
    pub(super) fn attach_logprobs(&self, output: &mut [Node]) {
        logprobs::attach_gemini(&self.metadata, output);
    }
}

fn stored_candidate_extra(
    extra_body: &mut HashMap<String, Value>,
) -> Option<&mut Map<String, Value>> {
    extra_body
        .entry(GEMINI_CANDIDATE_EXTRA_KEY.into())
        .or_insert_with(|| json!({}))
        .as_object_mut()
}

/// Token scores a frame's candidate supplies for each of its answer text Parts, keyed by the
/// native Part index.
pub(super) fn frame_text_scores(
    candidate: &Map<String, Value>,
    parts: &[Value],
) -> HashMap<usize, Vec<TokenLogprob>> {
    if !candidate.contains_key("logprobsResult") {
        return HashMap::new();
    }
    let mut indices = Vec::new();
    let mut nodes = Vec::new();
    for (index, part) in parts.iter().enumerate() {
        if part.get("thought").and_then(Value::as_bool) != Some(true)
            && let Some(text) = part.get("text").and_then(Value::as_str)
        {
            indices.push(index);
            nodes.push(Node::assistant_text(text));
        }
    }
    logprobs::attach_gemini(candidate, &mut nodes);
    indices
        .into_iter()
        .zip(nodes)
        .filter_map(|(index, node)| match node {
            Node::Text {
                logprobs: Some(scores),
                ..
            } => Some((index, scores)),
            _ => None,
        })
        .collect()
}
