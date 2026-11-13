//! Upstream Gemini `streamGenerateContent` SSE stream to URP stream events
//! (gemini-codec.spec.md GEM-8, GEM-10).

mod candidate;
mod grounding;
mod state;

use crate::error::AppResult;
use crate::handlers::{StreamRuntimeMetrics, UrpRequest as HandlerUrpRequest};
use crate::payload::sse::{Frames, SseError};
use crate::urp::UrpStreamEvent;
use state::{Flow, GeminiStreamState};
use std::sync::Arc;
use tokio::sync::{Mutex, mpsc};

pub(crate) async fn stream_gemini_to_urp_events(
    urp: &HandlerUrpRequest,
    upstream_resp: reqwest::Response,
    tx: mpsc::Sender<UrpStreamEvent>,
    started_at: Option<std::time::Instant>,
    runtime_metrics: Option<Arc<Mutex<StreamRuntimeMetrics>>>,
    idle_timeout_ms: u64,
) -> AppResult<()> {
    let mut state = GeminiStreamState::new(urp, tx, started_at, runtime_metrics);
    let mut stream = Frames::new(upstream_resp, idle_timeout_ms, false);
    loop {
        let event = match stream.next().await {
            Ok(Some(event)) => event,
            Ok(None) => break,
            Err(SseError::Idle(_)) if state.completed() => break,
            Err(error) => return Err(error.into_app()),
        };
        match state.on_sse_data(event.data).await? {
            Flow::Continue => {}
            Flow::Done => break,
        }
    }
    state.finish().await
}
