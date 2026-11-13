//! Upstream Anthropic Messages SSE stream to URP stream events.

mod blocks;
mod json_input;
mod state;
mod usage;

use crate::error::AppResult;
use crate::handlers::{StreamRuntimeMetrics, UrpRequest as HandlerUrpRequest};
use crate::payload::sse::Frames;
use crate::urp::UrpStreamEvent;
use crate::urp::decode::split_extra;
use serde_json::Value;
use state::{AnthropicMessagesStreamState, Flow};
use std::collections::HashMap;
use std::sync::Arc;
use tokio::sync::{Mutex, mpsc};

pub(crate) async fn stream_messages_to_urp_events(
    urp: &HandlerUrpRequest,
    upstream_resp: reqwest::Response,
    tx: mpsc::Sender<UrpStreamEvent>,
    started_at: Option<std::time::Instant>,
    runtime_metrics: Option<Arc<Mutex<StreamRuntimeMetrics>>>,
    idle_timeout_ms: u64,
) -> AppResult<()> {
    let mut state = AnthropicMessagesStreamState::new(urp, tx, started_at, runtime_metrics);
    let mut stream = Frames::new(upstream_resp, idle_timeout_ms, false);
    let mut terminal_event = None;
    while let Some(event) = stream.next().await.map_err(|error| error.into_app())? {
        let flow = state.on_sse_event(event).await;
        match flow {
            Flow::Continue => {}
            Flow::Terminal(name) => {
                terminal_event = Some(name);
                break;
            }
            Flow::Stop => return Ok(()),
        }
    }
    state.finish(terminal_event).await;
    Ok(())
}

/// The wire members of a JSON object except `ignored` and internal keys.
fn object_without_keys(value: &Value, ignored: &[&str]) -> HashMap<String, Value> {
    value
        .as_object()
        .map_or_else(HashMap::new, |object| split_extra(object, ignored))
}
