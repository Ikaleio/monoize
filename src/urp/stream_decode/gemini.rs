//! Upstream Gemini `streamGenerateContent` SSE stream to URP stream events
//! (gemini-codec.spec.md GEM-8, GEM-10).

mod candidate;
mod grounding;
mod state;

use crate::error::{AppError, AppResult};
use crate::handlers::{StreamRuntimeMetrics, UrpRequest as HandlerUrpRequest};
use crate::urp::UrpStreamEvent;
use axum::http::StatusCode;
use eventsource_stream::Eventsource;
use futures_util::StreamExt;
use state::{Flow, GeminiStreamState, decode_error};
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
    let idle_timeout = std::time::Duration::from_millis(idle_timeout_ms.max(1));
    let mut stream = upstream_resp.bytes_stream().eventsource();
    loop {
        let event = match tokio::time::timeout(idle_timeout, stream.next()).await {
            Ok(Some(event)) => event,
            Ok(None) => break,
            // A finish reason already established completion; trailing silence is not a failure.
            Err(_) if state.completed() => break,
            Err(_) => {
                return Err(AppError::new(
                    StatusCode::GATEWAY_TIMEOUT,
                    "upstream_idle_timeout",
                    format!("upstream stream idle for {idle_timeout_ms}ms without data"),
                ));
            }
        };
        let event = event.map_err(|error| decode_error(crate::error::sse_error_chain(&error)))?;
        match state.on_sse_data(&event.data).await? {
            Flow::Continue => {}
            Flow::Done => break,
        }
    }
    state.finish().await
}
