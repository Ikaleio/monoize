pub mod anthropic;
#[cfg(test)]
pub mod gemini;
pub mod openai_chat;
pub mod openai_responses;

use crate::error::AppResult;
use crate::handlers::DownstreamProtocol;
use crate::payload::sse::WireEvent as Event;
use crate::urp::tool_signature::SignatureProjection;
use crate::urp::{UrpResponse, UrpStreamEvent, stream_events};
use serde_json::Value;
use std::time::Instant;
use tokio::sync::mpsc;

/// Downstream encoding options of one live stream or one replay.
#[derive(Debug, Clone)]
pub(crate) struct StreamEncodeOptions {
    pub logical_model: String,
    pub sse_max_frame_length: Option<usize>,
    pub mask_sensitive_info: bool,
    /// The downstream Chat request's `stream_options.include_usage` (DC7).
    pub chat_include_usage: bool,
    /// STR3t: request controls a synthesized Responses object echoes.
    pub responses_request_echo: Option<Value>,
    /// Start of the downstream stream. Responses reasoning durations are measured from it.
    pub started_at: Instant,
    /// Replay only: the reasoning duration a replayed Responses stream reports.
    pub replay_reasoning_duration_secs: Option<u64>,
}

/// One downstream protocol stream encoder (RSTRUCT-2, RSTRUCT-3). It owns its SSE sender and
/// all cross-event state. `run` and `replay` are its only drivers.
pub(crate) trait StreamEncoder: Send {
    /// The CTRL-8a signature projection applied to every event before the encoder sees it.
    /// `None` for a protocol that carries call signatures natively.
    fn signature_projection() -> Option<SignatureProjection>
    where
        Self: Sized;

    /// Replay only, before the first event. A response the downstream protocol cannot encode
    /// returns `Err` after the encoder emitted its native error, marked with
    /// `with_downstream_stream_terminal_sent` when that frame was delivered (FP6j).
    fn begin_replay(
        &mut self,
        response: &UrpResponse,
    ) -> impl Future<Output = AppResult<()>> + Send {
        let _ = response;
        async { Ok(()) }
    }

    fn on_event(&mut self, event: UrpStreamEvent) -> impl Future<Output = AppResult<()>> + Send;

    /// Called once after the last event, also when the input ended without `ResponseDone`.
    fn finish(&mut self) -> impl Future<Output = AppResult<()>> + Send;
}

struct Driver<E> {
    encoder: E,
    projection: Option<SignatureProjection>,
}

impl<E: StreamEncoder> Driver<E> {
    fn new(encoder: E) -> Self {
        Self {
            encoder,
            projection: E::signature_projection(),
        }
    }

    async fn feed(&mut self, event: UrpStreamEvent) -> AppResult<()> {
        let Some(projection) = self.projection.as_mut() else {
            return self.encoder.on_event(event).await;
        };
        projection.push(event);
        while let Some(event) = projection.next_ready() {
            self.encoder.on_event(event).await?;
        }
        Ok(())
    }

    async fn end(mut self) -> AppResult<()> {
        if let Some(projection) = self.projection.as_mut() {
            projection.finish();
            while let Some(event) = projection.next_ready() {
                self.encoder.on_event(event).await?;
            }
        }
        self.encoder.finish().await
    }
}

/// Encodes a live URP event stream.
pub(crate) async fn run<E: StreamEncoder>(
    encoder: E,
    mut rx: mpsc::Receiver<UrpStreamEvent>,
) -> AppResult<()> {
    let mut driver = Driver::new(encoder);
    while let Some(event) = rx.recv().await {
        driver.feed(event).await?;
    }
    driver.end().await
}

/// Encodes the STR-14 replay of a complete response (RECON-8).
pub(crate) async fn replay<E: StreamEncoder>(
    mut encoder: E,
    response: UrpResponse,
) -> AppResult<()> {
    encoder.begin_replay(&response).await?;
    let mut driver = Driver::new(encoder);
    for event in stream_events::events_from_response(response) {
        driver.feed(event).await?;
    }
    driver.end().await
}

pub(crate) async fn encode_urp_stream(
    downstream: DownstreamProtocol,
    rx: mpsc::Receiver<UrpStreamEvent>,
    tx: mpsc::Sender<Event>,
    options: StreamEncodeOptions,
) -> AppResult<()> {
    match downstream {
        DownstreamProtocol::Responses => {
            run(
                openai_responses::ResponsesStreamEncoder::new(tx, options),
                rx,
            )
            .await
        }
        DownstreamProtocol::ChatCompletions => {
            run(openai_chat::ChatStreamEncoder::new(tx, options), rx).await
        }
        DownstreamProtocol::AnthropicMessages => {
            run(anthropic::MessagesStreamEncoder::new(tx, options), rx).await
        }
    }
}

/// The synthetic downstream stream of a non-stream upstream response (RECON-8).
pub(crate) async fn replay_urp_response(
    downstream: DownstreamProtocol,
    response: UrpResponse,
    tx: mpsc::Sender<Event>,
    options: StreamEncodeOptions,
) -> AppResult<()> {
    match downstream {
        DownstreamProtocol::Responses => {
            replay(
                openai_responses::ResponsesStreamEncoder::new(tx, options),
                response,
            )
            .await
        }
        DownstreamProtocol::ChatCompletions => {
            replay(openai_chat::ChatStreamEncoder::new(tx, options), response).await
        }
        DownstreamProtocol::AnthropicMessages => {
            replay(anthropic::MessagesStreamEncoder::new(tx, options), response).await
        }
    }
}
