//! Downstream Chat chunk emission (DC6, DC6a, DC7b, DC8).

use crate::error::{AppError, AppResult};
use crate::payload::sse::WireEvent as Event;
use crate::urp::stream_helpers::{
    send_plain_sse_data, send_plain_sse_json, split_json_value_by_string_patch,
};
use crate::urp::{
    CHAT_CHOICE_EXTRA_BODY_KEY, CHAT_DELTA_EXTRA_BODY_KEY, TokenLogprob, is_internal_key,
};
use axum::http::StatusCode;
use serde_json::{Map, Value, json};
use std::collections::HashMap;
use tokio::sync::mpsc;

/// The chunk sink of one downstream Chat stream. Every chunk carries the stream identity and
/// the upstream top-level chunk members (DC6a). Pending choice members ride on the next chunk
/// that has a choice (DC6); pending delta members (CTL-5) join the next delta that takes them.
pub(super) struct Wire {
    tx: mpsc::Sender<Event>,
    max_frame_length: Option<usize>,
    model: String,
    id: String,
    created: i64,
    envelope_extra: Map<String, Value>,
    pending_choice_extra: Map<String, Value>,
    pending_delta_extra: HashMap<String, Value>,
}

impl Wire {
    pub(super) fn new(
        tx: mpsc::Sender<Event>,
        model: String,
        max_frame_length: Option<usize>,
    ) -> Self {
        Self {
            tx,
            max_frame_length,
            model,
            id: String::new(),
            created: 0,
            envelope_extra: Map::new(),
            pending_choice_extra: Map::new(),
            pending_delta_extra: HashMap::new(),
        }
    }

    /// DC7b: the identity and envelope members every later chunk carries.
    pub(super) fn open(&mut self, id: String, created: i64, envelope_extra: Map<String, Value>) {
        self.id = id;
        self.created = created;
        self.envelope_extra = envelope_extra;
    }

    /// DC6: the unknown choice members of the upstream frame behind `event_extra`.
    pub(super) fn stash_choice_extra(&mut self, event_extra: &HashMap<String, Value>) {
        if let Some(choice_extra) = event_extra
            .get(CHAT_CHOICE_EXTRA_BODY_KEY)
            .and_then(Value::as_object)
        {
            self.pending_choice_extra.extend(
                choice_extra
                    .iter()
                    .filter(|(key, _)| !is_internal_key(key))
                    .map(|(key, value)| (key.clone(), value.clone())),
            );
        }
    }

    /// CTL-5: wire members for the next delta. Internal members never reach the wire.
    pub(super) fn stash_delta_extra<'a>(
        &mut self,
        members: impl IntoIterator<Item = (&'a String, &'a Value)>,
    ) {
        self.pending_delta_extra.extend(
            members
                .into_iter()
                .filter(|(key, _)| !is_internal_key(key))
                .map(|(key, value)| (key.clone(), value.clone())),
        );
    }

    pub(super) fn set_delta_member(&mut self, key: &str, value: Value) {
        self.pending_delta_extra.insert(key.to_string(), value);
    }

    /// Moves the pending delta members into `delta`; typed members of `delta` win.
    pub(super) fn with_pending_extra(&mut self, delta: Value) -> Value {
        self.take_delta_extra(delta, &mut Map::new())
    }

    /// Moves the pending delta members and `event_extra` (the event's own native delta members,
    /// which win) into `delta`. Typed members of `delta` win over both.
    pub(super) fn take_delta_extra(
        &mut self,
        mut delta: Value,
        event_extra: &mut Map<String, Value>,
    ) -> Value {
        let mut extra = std::mem::take(&mut self.pending_delta_extra);
        extra.extend(std::mem::take(event_extra));
        merge_delta_extra(&mut delta, extra);
        delta
    }

    /// [`Self::take_delta_extra`] with the native delta members of one event.
    pub(super) fn with_event_extra(
        &mut self,
        delta: Value,
        event_extra: &HashMap<String, Value>,
    ) -> Value {
        self.take_delta_extra(delta, &mut native_delta_extra(event_extra))
    }

    fn chunk(&self, delta: Value) -> Value {
        let mut chunk = json!({
            "id": self.id,
            "object": "chat.completion.chunk",
            "created": self.created,
            "model": self.model,
            "choices": [{ "index": 0, "delta": {}, "finish_reason": Value::Null }]
        });
        chunk["choices"][0]["delta"] = delta;
        chunk
    }

    /// Sends one payload. A chunk takes the envelope members and the pending choice members;
    /// generated members win.
    pub(super) async fn send(&mut self, mut payload: Value) -> AppResult<()> {
        if let Some(obj) = payload.as_object_mut()
            && obj.get("object").and_then(Value::as_str) == Some("chat.completion.chunk")
        {
            for (key, value) in &self.envelope_extra {
                obj.entry(key.clone()).or_insert_with(|| value.clone());
            }
            if let Some(choice) = obj
                .get_mut("choices")
                .and_then(Value::as_array_mut)
                .and_then(|choices| choices.first_mut())
                .and_then(Value::as_object_mut)
            {
                for (key, value) in std::mem::take(&mut self.pending_choice_extra) {
                    choice.entry(key).or_insert(value);
                }
            }
        }
        send_plain_sse_json(&self.tx, &payload).await
    }

    pub(super) async fn delta(&mut self, delta: Value) -> AppResult<()> {
        let chunk = self.chunk(delta);
        self.send(chunk).await
    }

    /// Sends `content` through `patch` into the chunk of `delta`, split across frames under the
    /// frame limit.
    pub(super) async fn delta_string(
        &mut self,
        delta: Value,
        content: &str,
        patch: fn(&mut Value, &str),
    ) -> AppResult<()> {
        let chunk = self.chunk(delta);
        for chunk in split_json_value_by_string_patch(chunk, content, patch, self.max_frame_length)
        {
            self.send(chunk).await?;
        }
        Ok(())
    }

    /// A content or refusal fragment with its token scores; a split frame never repeats a
    /// token. Annotations of `delta` follow in their own chunk (PC7h).
    pub(super) async fn text(
        &mut self,
        mut delta: Value,
        content: &str,
        patch: fn(&mut Value, &str),
        scores: Option<&[TokenLogprob]>,
    ) -> AppResult<()> {
        let annotations = delta
            .as_object_mut()
            .and_then(|delta| delta.remove("annotations"));
        match scores {
            Some(scores) => {
                let field = if delta.get("refusal").is_some() {
                    "refusal"
                } else {
                    "content"
                };
                let mut chunk = self.chunk(delta);
                chunk["choices"][0]["logprobs"] =
                    object(field, crate::urp::logprobs::encode_openai(scores));
                patch(&mut chunk, content);
                if self
                    .max_frame_length
                    .is_some_and(|limit| chunk.to_string().len() + 8 > limit)
                {
                    for (text, scores) in crate::urp::logprobs::fragments(scores) {
                        let mut part = chunk.clone();
                        patch(&mut part, if content.is_empty() { "" } else { &text });
                        part["choices"][0]["logprobs"][field] =
                            crate::urp::logprobs::encode_openai(&scores);
                        self.send(part).await?;
                    }
                } else {
                    self.send(chunk).await?;
                }
            }
            None => self.delta_string(delta, content, patch).await?,
        }
        if let Some(annotations) =
            annotations.filter(|value| value.as_array().is_some_and(|values| !values.is_empty()))
        {
            self.delta(object("annotations", annotations)).await?;
        }
        Ok(())
    }

    /// Sends the pending delta members in an empty delta, when there are any.
    pub(super) async fn flush_delta_extra(&mut self) -> AppResult<()> {
        if self.pending_delta_extra.is_empty() {
            return Ok(());
        }
        let delta = self.with_pending_extra(json!({}));
        self.delta(delta).await
    }

    pub(super) fn has_pending_delta_extra(&self) -> bool {
        !self.pending_delta_extra.is_empty()
    }

    /// DC9: the empty-delta finish chunk, the CHAT-8 usage chunk, and `[DONE]`. Choice members
    /// that no delta chunk carried go out in an empty delta first.
    pub(super) async fn terminate(
        &mut self,
        finish_reason: &str,
        choice_extra: Option<&Map<String, Value>>,
        usage: Option<Value>,
    ) -> AppResult<()> {
        if !self.pending_choice_extra.is_empty() {
            self.delta(json!({})).await?;
        }
        let mut finish = self.chunk(json!({}));
        finish["choices"][0]["finish_reason"] = json!(finish_reason);
        if let Some(choice) = finish["choices"][0].as_object_mut() {
            for (key, value) in choice_extra.into_iter().flatten() {
                if !is_internal_key(key) {
                    choice.entry(key.clone()).or_insert_with(|| value.clone());
                }
            }
        }
        self.send(finish).await?;
        if let Some(usage) = usage {
            let mut chunk = json!({
                "id": self.id,
                "object": "chat.completion.chunk",
                "created": self.created,
                "model": self.model,
                "choices": [],
            });
            chunk["usage"] = usage;
            self.send(chunk).await?;
        }
        self.done().await
    }

    /// The single `[DONE]` sentinel (CHAT-6).
    pub(super) async fn done(&mut self) -> AppResult<()> {
        send_plain_sse_data(&self.tx, "[DONE]").await
    }

    /// Ends the stream with the native `unsupported_media` error. Always returns `Err`, marked as
    /// terminal-sent when the frames reached an open receiver (FP6j).
    pub(super) async fn media_error(&mut self, message: &str) -> AppResult<()> {
        self.send(crate::urp::media::error_body(message)).await?;
        self.done().await?;
        Err(
            AppError::new(StatusCode::BAD_GATEWAY, "unsupported_media", message)
                .with_downstream_stream_terminal_sent(!self.tx.is_closed()),
        )
    }
}

/// `{ key: value }`, moving `value`.
pub(super) fn object(key: &str, value: Value) -> Value {
    Value::Object(Map::from_iter([(key.to_string(), value)]))
}

/// The native delta members of one event (DC6).
pub(super) fn native_delta_extra(event_extra: &HashMap<String, Value>) -> Map<String, Value> {
    event_extra
        .get(CHAT_DELTA_EXTRA_BODY_KEY)
        .and_then(Value::as_object)
        .cloned()
        .unwrap_or_default()
}

/// Adds the non-internal members of `extra` that `delta` does not define.
pub(super) fn merge_delta_extra(
    delta: &mut Value,
    extra: impl IntoIterator<Item = (String, Value)>,
) {
    let Some(delta) = delta.as_object_mut() else {
        return;
    };
    for (key, value) in extra {
        if !is_internal_key(&key) && !delta.contains_key(&key) {
            delta.insert(key, value);
        }
    }
}
