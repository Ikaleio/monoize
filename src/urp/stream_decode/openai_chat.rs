//! Upstream Chat Completions SSE stream to URP stream events.

mod chunk;
mod content;
mod details;
mod errors;
mod reasoning;
mod state;
mod terminal;
#[cfg(test)]
mod tests;
mod text;
mod tool_calls;

use crate::error::AppResult;
use crate::handlers::usage::{StreamChunkMetrics, record_stream_chunk_metrics};
use crate::handlers::{StreamRuntimeMetrics, UrpRequest as HandlerUrpRequest};
use crate::payload::sse::Frames;
use crate::urp::UrpStreamEvent;
use state::{ChatStreamState, Flow};
use std::sync::Arc;
use tokio::sync::{Mutex, mpsc};

pub(crate) async fn stream_chat_to_urp_events(
    urp: &HandlerUrpRequest,
    upstream_resp: reqwest::Response,
    tx: mpsc::Sender<UrpStreamEvent>,
    started_at: Option<std::time::Instant>,
    runtime_metrics: Option<Arc<Mutex<StreamRuntimeMetrics>>>,
    idle_timeout_ms: u64,
) -> AppResult<()> {
    let mut state = ChatStreamState::new(urp, tx, started_at, runtime_metrics);
    let result = decode_events(&mut state, upstream_resp, idle_timeout_ms).await;
    // Visible output bytes ride along with the next event's single metrics lock; whatever is
    // still pending when decoding stops (on any exit path) is flushed here.
    record_stream_chunk_metrics(
        &state.runtime_metrics,
        StreamChunkMetrics {
            visible_output_bytes: state.pending_visible_output_bytes,
            ..StreamChunkMetrics::default()
        },
    )
    .await;
    result
}

async fn decode_events(
    state: &mut ChatStreamState<'_>,
    upstream_resp: reqwest::Response,
    idle_timeout_ms: u64,
) -> AppResult<()> {
    let mut stream = Frames::new(upstream_resp, idle_timeout_ms, false);
    while let Some(event) = stream.next().await.map_err(|error| error.into_app())? {
        let flow = state.on_sse_event(event).await?;
        match flow {
            Flow::Continue => {}
            Flow::Done => break,
            Flow::Stop => return Ok(()),
        }
    }
    state.finish().await
}
