//! Downstream OpenAI Chat Completions SSE encoder (DC3-DC9, CHAT-*).
//!
//! `ChatStreamEncoder` owns every cross-event state of one downstream stream: the chunk sink
//! with its envelope state, one `NodeState` per announced node, and the finish-reason evidence.
//! A Chat stream carries one assistant message, so every node renders into `choices[0].delta`.

mod error;
mod media;
mod nodes;
mod reasoning;
#[cfg(test)]
mod tests;
mod text;
mod wire;

use crate::error::AppResult;
use crate::error_sanitize::maybe_mask_sensitive_text;
use crate::handlers::routing::now_ts;
use crate::urp::citations::parse_chat_annotations_control;
use crate::urp::encode::openai_chat::{
    chat_completion_id, encode_chat_usage, finish_reason_to_chat, validate_response_nodes,
};
use crate::urp::stream_encode::{StreamEncodeOptions, StreamEncoder};
use crate::urp::tool_signature::SignatureProjection;
use crate::urp::{
    CHAT_CHOICE_EXTRA_BODY_KEY, CHAT_CHUNK_EXTRA_KEY, CHAT_DELTA_EXTRA_BODY_KEY,
    CHAT_ERROR_EVENT_EXTRA_KEY, CHAT_NATIVE_FINISH_REASON_EXTRA_KEY, FinishReason,
    ProviderProtocol, ResponseOutcome, TerminalNode, UrpResponse, UrpStreamEvent, Usage,
    is_internal_key,
};
use axum::response::sse::Event;
use nodes::NodeState;
use serde_json::{Map, Value, json};
use std::collections::HashMap;
use tokio::sync::mpsc;
use wire::{Wire, merge_delta_extra, native_delta_extra, object};

pub(crate) struct ChatStreamEncoder {
    wire: Wire,
    mask_sensitive_info: bool,
    /// DC7: the downstream request set `stream_options.include_usage`.
    include_usage: bool,
    /// RECON-8: the stream replays a complete response. Its top-level members form every chunk
    /// envelope, and its choice members belong to the finish chunk alone, as in the non-stream
    /// body.
    replay: bool,
    /// `ResponseDone`, an `Error`, or a replayed failure ended the stream; later events are
    /// dropped.
    finished: bool,
    nodes: HashMap<u32, NodeState>,
    /// Characters of message content emitted so far (PC7h annotation offsets).
    text_chars: u64,
    next_tool_index: usize,
    /// DC5: the next synthesized reasoning detail index.
    next_detail_index: u64,
    /// CHAT-7: a tool call or a legacy `function_call` reached the wire.
    saw_tool_call: bool,
    saw_function_call: bool,
}

impl ChatStreamEncoder {
    pub(crate) fn new(tx: mpsc::Sender<Event>, options: StreamEncodeOptions) -> Self {
        Self {
            wire: Wire::new(tx, options.logical_model, options.sse_max_frame_length),
            mask_sensitive_info: options.mask_sensitive_info,
            include_usage: options.chat_include_usage,
            replay: false,
            finished: false,
            nodes: HashMap::new(),
            text_chars: 0,
            next_tool_index: 0,
            next_detail_index: 0,
            saw_tool_call: false,
            saw_function_call: false,
        }
    }

    /// DC7b: the first chunk, with the stream identity and the upstream creation time.
    async fn on_response_start(
        &mut self,
        id: String,
        created_at: Option<i64>,
        extra_body: HashMap<String, Value>,
    ) -> AppResult<()> {
        let mut delta = json!({ "role": "assistant", "content": "", "refusal": Value::Null });
        let envelope = if self.replay {
            wire_members(&extra_body)
        } else {
            // DC6, DC6a: the members of the first upstream frame.
            self.wire.stash_choice_extra(&extra_body);
            merge_delta_extra(&mut delta, native_delta_extra(&extra_body));
            extra_body
                .get(CHAT_CHUNK_EXTRA_KEY)
                .and_then(Value::as_object)
                .map(wire_members)
                .unwrap_or_default()
        };
        self.wire.open(
            chat_completion_id(&id),
            created_at.unwrap_or_else(now_ts),
            envelope,
        );
        self.wire.delta(delta).await
    }

    async fn on_response_done(
        &mut self,
        outcome: Option<ResponseOutcome>,
        finish_reason: Option<FinishReason>,
        usage: Option<Usage>,
        output: Vec<TerminalNode>,
        extra_body: HashMap<String, Value>,
    ) -> AppResult<()> {
        self.finished = true;
        if let Some(body) = error::failure_body(outcome.as_ref(), self.mask_sensitive_info) {
            self.wire.send(body).await?;
            return self.wire.done().await;
        }
        // PC7f: native delta members of the terminal frame precede the finish chunk.
        self.wire.stash_delta_extra(
            extra_body
                .get(CHAT_DELTA_EXTRA_BODY_KEY)
                .and_then(Value::as_object)
                .into_iter()
                .flatten(),
        );
        // PC7g / STR-8d: announced entries complete their own node; the rest are new nodes.
        let claimed: Vec<Option<u32>> =
            TerminalNode::resolve(&output, |index| self.nodes.contains_key(&index))
                .map(|(index, _)| index)
                .collect();
        for (index, entry) in claimed.into_iter().zip(&output) {
            match index {
                Some(index) => self.on_node_done(index, &entry.node).await?,
                None => self.emit_unannounced(&entry.node).await?,
            }
        }
        self.wire.flush_delta_extra().await?;
        let native_finish_reason = extra_body
            .get(CHAT_NATIVE_FINISH_REASON_EXTRA_KEY)
            .and_then(Value::as_str);
        let finish_reason = self.finish_reason(finish_reason, native_finish_reason);
        let usage = usage
            .filter(|_| self.include_usage)
            .map(|usage| encode_chat_usage(&usage));
        self.wire
            .terminate(
                finish_reason,
                extra_body
                    .get(CHAT_CHOICE_EXTRA_BODY_KEY)
                    .and_then(Value::as_object),
                usage,
            )
            .await
    }

    /// CHAT-7, DC7b: a typed reason wins; without one, emitted calls decide.
    fn finish_reason<'a>(
        &self,
        finish_reason: Option<FinishReason>,
        native: Option<&'a str>,
    ) -> &'a str {
        match finish_reason {
            Some(FinishReason::Other) => {
                native.filter(|reason| !reason.is_empty()).unwrap_or("stop")
            }
            Some(FinishReason::ToolCalls) if self.saw_function_call => "function_call",
            Some(reason) => finish_reason_to_chat(reason),
            None if self.saw_tool_call => "tool_calls",
            None if self.saw_function_call => "function_call",
            None => "stop",
        }
    }

    /// PC7h: citations of a completed text node, in emitted-content coordinates.
    async fn on_provider_control(
        &mut self,
        protocol: &str,
        event_name: &str,
        data: &Value,
    ) -> AppResult<()> {
        if protocol != ProviderProtocol::ChatCompletion.as_str() {
            return Ok(());
        }
        let Some((node_index, citations)) = parse_chat_annotations_control(event_name, data) else {
            return Ok(());
        };
        let offset = match self.nodes.get_mut(&node_index) {
            Some(state) => {
                state.text.add_citations_sent(citations.len());
                state.text.position()
            }
            None => None,
        };
        let annotations = crate::urp::citations::encode(
            &citations,
            ProviderProtocol::ChatCompletion,
            offset.unwrap_or(self.text_chars),
        );
        self.wire
            .delta(object("annotations", Value::Array(annotations)))
            .await
    }

    async fn on_error(
        &mut self,
        code: Option<String>,
        message: String,
        extra_body: HashMap<String, Value>,
    ) -> AppResult<()> {
        self.finished = true;
        // SAN-11 / SAN-CFG5: decoder-origin error text may embed upstream URLs.
        let message = maybe_mask_sensitive_text(&message, self.mask_sensitive_info);
        let payload = error::error_payload(
            extra_body.get(CHAT_ERROR_EVENT_EXTRA_KEY),
            code.as_deref(),
            &message,
            &extra_body,
        );
        self.wire.send(payload).await?;
        self.wire.done().await
    }
}

impl StreamEncoder for ChatStreamEncoder {
    fn signature_projection() -> Option<SignatureProjection> {
        Some(SignatureProjection::default())
    }

    async fn begin_replay(&mut self, response: &UrpResponse) -> AppResult<()> {
        // A failed response replays as its native error alone, as its non-stream body does.
        if let Some(body) = error::failure_body(response.outcome.as_ref(), self.mask_sensitive_info)
        {
            self.finished = true;
            self.wire.send(body).await?;
            return self.wire.done().await;
        }
        if let Err(error) = validate_response_nodes(&response.output) {
            return self.wire.media_error(&error).await;
        }
        self.replay = true;
        Ok(())
    }

    async fn on_event(&mut self, event: UrpStreamEvent) -> AppResult<()> {
        if self.finished {
            return Ok(());
        }
        let message_audio = match &event {
            UrpStreamEvent::NodeDelta { node_index, .. } => self
                .nodes
                .get(node_index)
                .is_some_and(|state| state.message_audio),
            _ => false,
        };
        if let Err(error) = media::validate_event(&event, message_audio) {
            return self.wire.media_error(&error).await;
        }
        match event {
            UrpStreamEvent::ResponseStart {
                id,
                created_at,
                extra_body,
                ..
            } => self.on_response_start(id, created_at, extra_body).await,
            UrpStreamEvent::NodeStart {
                node_index,
                header,
                extra_body,
            } => self.on_node_start(node_index, header, extra_body).await,
            UrpStreamEvent::NodeDelta {
                node_index,
                delta,
                extra_body,
                ..
            } => self.on_node_delta(node_index, delta, extra_body).await,
            UrpStreamEvent::NodeDone {
                node_index, node, ..
            } => self.on_node_done(node_index, &node).await,
            UrpStreamEvent::ResponseDone {
                outcome,
                finish_reason,
                usage,
                output,
                extra_body,
            } => {
                self.on_response_done(outcome, finish_reason, usage, output, extra_body)
                    .await
            }
            UrpStreamEvent::ProviderControl {
                protocol,
                event_name,
                data,
                ..
            } => {
                self.on_provider_control(&protocol, &event_name, &data)
                    .await
            }
            UrpStreamEvent::Error {
                code,
                message,
                extra_body,
            } => self.on_error(code, message, extra_body).await,
        }
    }

    async fn finish(&mut self) -> AppResult<()> {
        Ok(())
    }
}

/// The wire members of an upstream member map; internal members never reach the wire.
fn wire_members<'a>(
    members: impl IntoIterator<Item = (&'a String, &'a Value)>,
) -> Map<String, Value> {
    members
        .into_iter()
        .filter(|(key, _)| !is_internal_key(key))
        .map(|(key, value)| (key.clone(), value.clone()))
        .collect()
}
