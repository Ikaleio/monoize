//! Assistant text segments with their citations and logprobs, refusal, and generated audio.

use super::state::ChatStreamState;
use crate::error::AppResult;
use crate::urp::{
    AudioSource, CHAT_MESSAGE_AUDIO_EXTRA_KEY, Citation, MediaMetadata, Node, NodeDelta,
    NodeHeader, OrdinaryRole, ProviderProtocol, TokenLogprob, is_internal_key,
    synthetic_message_id,
};
use serde_json::{Map, Value};
use std::collections::HashMap;

#[derive(Default)]
pub(super) struct TextState {
    /// All streamed message text across segments; a terminal snapshot extends it by suffix.
    pub(super) all: String,
    /// Byte offset in `all` where the open segment starts.
    pub(super) segment_start: usize,
    pub(super) open: Option<u32>,
    /// Logprobs and citations of the open segment.
    pub(super) logprobs: Option<Vec<TokenLogprob>>,
    pub(super) citations: Vec<Citation>,
}

pub(super) struct RefusalState {
    pub(super) node_index: u32,
    content: String,
    logprobs: Option<Vec<TokenLogprob>>,
}

impl RefusalState {
    pub(super) fn into_node(self) -> Node {
        Node::Refusal {
            logprobs: self.logprobs,
            id: None,
            content: self.content,
            extra_body: HashMap::new(),
        }
    }
}

/// Message `audio` members accumulated across deltas: `data` and `transcript` concatenate,
/// other members take their latest value.
pub(super) struct AudioState {
    pub(super) node_index: u32,
    fields: Map<String, Value>,
}

impl AudioState {
    /// The audio node, or `None` when no member arrived. `mime` is the requested format's type.
    pub(super) fn into_node(self, mime: Option<&str>) -> Option<Node> {
        let mut node = crate::urp::decode::openai_chat::parse_chat_message_audio_node(
            &Value::Object(self.fields),
        )?;
        if let Node::Audio {
            source: AudioSource::Base64 { media_type, .. },
            ..
        } = &mut node
            && let Some(mime) = mime
        {
            *media_type = mime.to_string();
        }
        Some(node)
    }
}

impl ChatStreamState<'_> {
    async fn text_node(&mut self) -> AppResult<u32> {
        if let Some(node_index) = self.text.open {
            return Ok(node_index);
        }
        let header = NodeHeader::Text {
            citations: Vec::new(),
            signature: None,
            id: None,
            role: OrdinaryRole::Assistant,
            phase: self.phase.clone(),
        };
        let node_index = self.start_node(header, HashMap::new()).await?;
        self.text.open = Some(node_index);
        Ok(node_index)
    }

    /// Appends assistant text to the open text segment, opening one when needed.
    pub(super) async fn push_text(
        &mut self,
        text: &str,
        logprobs: Option<Vec<TokenLogprob>>,
    ) -> AppResult<()> {
        if text.is_empty() {
            return Ok(());
        }
        crate::urp::logprobs::append(&mut self.text.logprobs, &logprobs);
        let node_index = self.text_node().await?;
        self.text.all.push_str(text);
        self.pending_visible_output_bytes += text.len() as u64;
        let extra_body = self.take_delta_extra();
        self.send_delta(
            node_index,
            NodeDelta::Text {
                logprobs,
                citations: Vec::new(),
                signature: None,
                content: text.to_string(),
            },
            extra_body,
        )
        .await
    }

    pub(super) async fn on_annotations(&mut self, annotations: &[Value]) -> AppResult<()> {
        let citations =
            crate::urp::citations::decode(annotations.to_vec(), ProviderProtocol::ChatCompletion);
        let completed_chars = self
            .text
            .all
            .get(..self.text.segment_start)
            .unwrap_or_default()
            .chars()
            .count() as u64;
        let (live, completed) = place_chat_citations(citations, completed_chars, &mut self.closed);
        for (node_index, citation) in completed {
            self.send(crate::urp::citations::chat_annotations_control(
                node_index,
                &[citation],
            ))
            .await?;
        }
        // Citations that all belong to completed segments must not open an empty node.
        if annotations.is_empty() || !live.is_empty() {
            let node_index = self.text_node().await?;
            self.text.citations.extend(live.iter().cloned());
            self.send_delta(
                node_index,
                NodeDelta::Text {
                    logprobs: None,
                    citations: live,
                    signature: None,
                    content: String::new(),
                },
                HashMap::new(),
            )
            .await?;
        }
        Ok(())
    }

    pub(super) async fn push_refusal(
        &mut self,
        refusal: &str,
        logprobs: Option<Vec<TokenLogprob>>,
    ) -> AppResult<()> {
        let node_index = match self.refusal.as_ref().map(|state| state.node_index) {
            Some(node_index) => node_index,
            None => {
                self.start_node(NodeHeader::Refusal { id: None }, HashMap::new())
                    .await?
            }
        };
        let state = self.refusal.get_or_insert_with(|| RefusalState {
            node_index,
            content: String::new(),
            logprobs: None,
        });
        crate::urp::logprobs::append(&mut state.logprobs, &logprobs);
        state.content.push_str(refusal);
        let extra_body = self.take_delta_extra();
        self.send_delta(
            node_index,
            NodeDelta::Refusal {
                logprobs,
                content: refusal.to_string(),
            },
            extra_body,
        )
        .await
    }

    /// Accumulates message audio; its payload reaches events only with `NodeDone`.
    pub(super) async fn on_audio(&mut self, audio: &Map<String, Value>) -> AppResult<()> {
        let node_index = match self.audio.as_ref().map(|state| state.node_index) {
            Some(node_index) => node_index,
            None => {
                let header = NodeHeader::Audio {
                    metadata: MediaMetadata {
                        media_type: self.audio_mime.map(str::to_string),
                        ..Default::default()
                    },
                    id: None,
                    role: OrdinaryRole::Assistant,
                };
                let shape =
                    HashMap::from([(CHAT_MESSAGE_AUDIO_EXTRA_KEY.to_string(), Value::Bool(true))]);
                self.start_node(header, shape).await?
            }
        };
        let fields = &mut self
            .audio
            .get_or_insert_with(|| AudioState {
                node_index,
                fields: Map::new(),
            })
            .fields;
        for (key, value) in audio {
            if let ("data" | "transcript", Value::String(chunk)) = (key.as_str(), value) {
                if let Some(Value::String(accumulated)) = fields.get_mut(key) {
                    accumulated.push_str(chunk);
                } else {
                    fields.insert(key.clone(), Value::String(chunk.clone()));
                }
            } else if !is_internal_key(key) {
                fields.insert(key.clone(), value.clone());
            }
        }
        Ok(())
    }

    /// PC7h: completes the open text segment.
    pub(super) async fn close_text(&mut self) -> AppResult<()> {
        let Some(node_index) = self.text.open.take() else {
            return Ok(());
        };
        let node = self.take_text_segment();
        self.text.segment_start = self.text.all.len();
        self.closed.push((node_index, node.clone()));
        self.send_done(node_index, node).await
    }

    /// The open text segment as a node; takes the segment's logprobs and citations.
    pub(super) fn take_text_segment(&mut self) -> Node {
        Node::Text {
            logprobs: self.text.logprobs.take(),
            citations: std::mem::take(&mut self.text.citations),
            signature: None,
            id: Some(synthetic_message_id()),
            role: OrdinaryRole::Assistant,
            content: self
                .text
                .all
                .get(self.text.segment_start..)
                .unwrap_or_default()
                .to_string(),
            phase: self.phase.clone(),
            cache_breakpoint: None,
            extra_body: HashMap::new(),
        }
    }
}

/// PC7h: Chat annotation ranges are message-global and URP text citations are node-local.
/// `completed_chars` is the character length of the completed segments. Returns the citations
/// of the open segment, then the node-local citations whose range lies wholly inside a
/// completed segment; those also join that segment's terminal node.
fn place_chat_citations(
    citations: Vec<Citation>,
    completed_chars: u64,
    closed: &mut [(u32, Node)],
) -> (Vec<Citation>, Vec<(u32, Citation)>) {
    if completed_chars == 0 {
        return (citations, Vec::new());
    }
    let mut live = Vec::with_capacity(citations.len());
    let mut completed = Vec::new();
    for mut citation in citations {
        match &mut citation.answer_range {
            Some(range) if range.end <= completed_chars && range.start < completed_chars => {
                let mut segment_end = 0u64;
                let owner = closed.iter_mut().find_map(|(node_index, node)| {
                    let Node::Text {
                        content, citations, ..
                    } = node
                    else {
                        return None;
                    };
                    let segment_start = segment_end;
                    segment_end += content.chars().count() as u64;
                    (range.start < segment_end).then_some((
                        *node_index,
                        segment_start,
                        segment_end,
                        citations,
                    ))
                });
                if let Some((node_index, segment_start, segment_end, target)) = owner {
                    range.start -= segment_start;
                    range.end = range.end.min(segment_end) - segment_start;
                    target.push(citation.clone());
                    completed.push((node_index, citation));
                }
            }
            Some(range) => {
                range.start = range.start.saturating_sub(completed_chars);
                range.end = range.end.saturating_sub(completed_chars);
                live.push(citation);
            }
            None => live.push(citation),
        }
    }
    (live, completed)
}
