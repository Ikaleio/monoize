//! Text and refusal content with token scores and citations (CHAT-2, PC7h, STR-15).

use super::ChatStreamEncoder;
use super::wire::object;
use crate::error::AppResult;
use crate::urp::stream_helpers::chat_delta_path_content;
use crate::urp::{Citation, Node, OrdinaryRole, ProviderProtocol, TokenLogprob};
use serde_json::{Value, json};
use std::collections::HashMap;

/// The emitted part of one text or refusal node.
#[derive(Default)]
pub(super) struct TextProgress {
    sent: String,
    /// Prefix of `sent`, in bytes and tokens, that emitted token scores cover.
    scored_bytes: usize,
    scored_tokens: usize,
    /// Character position of the node in the emitted message content (PC7h).
    offset: Option<u64>,
    citations_sent: usize,
}

impl TextProgress {
    /// Records `content` as emitted and returns the scores that may accompany it: scores that
    /// continue the scored prefix and, for non-empty content, cover exactly `content`.
    fn push<'a>(
        &mut self,
        content: &str,
        scores: &'a Option<Vec<TokenLogprob>>,
    ) -> Option<&'a [TokenLogprob]> {
        let fully_scored_before = self.sent.len() == self.scored_bytes;
        self.sent.push_str(content);
        let scores = scores.as_deref().filter(|scores| !scores.is_empty())?;
        let bytes: Vec<u8> = scores
            .iter()
            .flat_map(|entry| token_bytes(entry).iter().copied())
            .collect();
        let unscored = self.sent.as_bytes().get(self.scored_bytes..)?;
        if !unscored.starts_with(&bytes)
            || (!content.is_empty() && (!fully_scored_before || bytes != content.as_bytes()))
        {
            return None;
        }
        self.scored_bytes += bytes.len();
        self.scored_tokens += scores.len();
        Some(scores)
    }

    /// The node scores past the emitted scored prefix, when that prefix agrees with them.
    fn unsent_scores<'a>(&self, scores: Option<&'a [TokenLogprob]>) -> Option<&'a [TokenLogprob]> {
        let scores = scores?;
        let sent = scores.get(..self.scored_tokens)?;
        let sent_bytes: usize = sent.iter().map(|entry| token_bytes(entry).len()).sum();
        (sent_bytes == self.scored_bytes)
            .then(|| &scores[self.scored_tokens..])
            .filter(|rest| !rest.is_empty())
    }

    /// PC7h: the node position in the emitted content, fixed by its first fragment.
    fn offset(&mut self, emitted_chars: u64) -> u64 {
        *self.offset.get_or_insert(emitted_chars)
    }

    pub(super) fn position(&self) -> Option<u64> {
        self.offset
    }

    pub(super) fn add_citations_sent(&mut self, count: usize) {
        self.citations_sent += count;
    }
}

fn token_bytes(entry: &TokenLogprob) -> &[u8] {
    entry
        .score
        .bytes
        .as_deref()
        .unwrap_or(entry.score.token.as_bytes())
}

fn patch_refusal(chunk: &mut Value, content: &str) {
    chunk["choices"][0]["delta"]["refusal"] = json!(content);
}

impl ChatStreamEncoder {
    pub(super) async fn on_text_delta(
        &mut self,
        progress: &mut TextProgress,
        content: String,
        citations: Vec<Citation>,
        logprobs: Option<Vec<TokenLogprob>>,
        event_extra: &HashMap<String, Value>,
    ) -> AppResult<()> {
        let offset = progress.offset(self.text_chars);
        self.text_chars += content.chars().count() as u64;
        let mut delta = json!({ "content": "" });
        if !citations.is_empty() {
            delta["annotations"] = Value::Array(crate::urp::citations::encode(
                &citations,
                ProviderProtocol::ChatCompletion,
                offset,
            ));
            progress.add_citations_sent(citations.len());
        }
        let delta = self.wire.with_event_extra(delta, event_extra);
        let scores = progress.push(&content, &logprobs);
        self.wire
            .text(delta, &content, chat_delta_path_content, scores)
            .await
    }

    pub(super) async fn on_refusal_delta(
        &mut self,
        progress: &mut TextProgress,
        content: String,
        logprobs: Option<Vec<TokenLogprob>>,
        event_extra: &HashMap<String, Value>,
    ) -> AppResult<()> {
        let delta = self
            .wire
            .with_event_extra(json!({ "refusal": "" }), event_extra);
        let scores = progress.push(&content, &logprobs);
        self.wire.text(delta, &content, patch_refusal, scores).await
    }

    /// STR-15 for a text or refusal node: the content suffix, token scores, and citations that
    /// no delta carried. Content that diverged from the emitted prefix cannot be retracted.
    pub(super) async fn complete_text(
        &mut self,
        progress: &mut TextProgress,
        node: &Node,
    ) -> AppResult<()> {
        let (content, citations, field, patch) = match node {
            Node::Text {
                role: OrdinaryRole::Assistant,
                content,
                citations,
                ..
            } => (
                content,
                citations.as_slice(),
                "content",
                chat_delta_path_content as fn(&mut Value, &str),
            ),
            Node::Refusal { content, .. } => (
                content,
                &[] as &[Citation],
                "refusal",
                patch_refusal as fn(&mut Value, &str),
            ),
            _ => return Ok(()),
        };
        let Some(suffix) = content.strip_prefix(progress.sent.as_str()) else {
            return Ok(());
        };
        let scores = node.token_scores();
        let unsent_scores = progress.unsent_scores(scores);
        let offset = progress.offset(self.text_chars);
        if field == "content" {
            self.text_chars += suffix.chars().count() as u64;
        }
        // Unsent scores ride on the suffix only when they cover exactly the suffix.
        let suffix_scores = unsent_scores.filter(|_| progress.scored_bytes == progress.sent.len());
        if !suffix.is_empty() {
            let delta = self.wire.with_pending_extra(json!({ field: "" }));
            self.wire.text(delta, suffix, patch, suffix_scores).await?;
        }
        if let Some(rest) = unsent_scores
            && (suffix.is_empty() || suffix_scores.is_none())
        {
            self.wire
                .text(json!({ field: "" }), "", patch, Some(rest))
                .await?;
        }
        progress.sent.push_str(suffix);
        if let (Some(scores), Some(_)) = (scores, unsent_scores) {
            progress.scored_bytes = content.len();
            progress.scored_tokens = scores.len();
        }
        if let Some(unsent) = citations
            .get(progress.citations_sent..)
            .filter(|unsent| !unsent.is_empty())
        {
            let annotations =
                crate::urp::citations::encode(unsent, ProviderProtocol::ChatCompletion, offset);
            progress.citations_sent = citations.len();
            if !annotations.is_empty() {
                self.wire
                    .delta(object("annotations", Value::Array(annotations)))
                    .await?;
            }
        }
        Ok(())
    }
}
