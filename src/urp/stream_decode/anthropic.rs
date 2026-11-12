//! Upstream Anthropic Messages SSE stream to URP stream events.

mod blocks;
mod json_input;
mod state;
mod usage;

use crate::error::{AppError, AppResult};
use crate::handlers::{StreamRuntimeMetrics, UrpRequest as HandlerUrpRequest};
use crate::urp::UrpStreamEvent;
use crate::urp::decode::split_extra;
use axum::http::StatusCode;
use eventsource_stream::Eventsource;
use futures_util::StreamExt;
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
    let idle_timeout = std::time::Duration::from_millis(idle_timeout_ms.max(1));
    let mut stream = upstream_resp.bytes_stream().eventsource();
    let mut terminal_event = None;
    while let Some(event) = tokio::time::timeout(idle_timeout, stream.next())
        .await
        .map_err(|_| {
            AppError::new(
                StatusCode::GATEWAY_TIMEOUT,
                "upstream_idle_timeout",
                format!("upstream stream idle for {idle_timeout_ms}ms without data"),
            )
        })?
    {
        let flow = match event {
            Ok(event) => state.on_sse_event(event).await,
            Err(error) => {
                state
                    .protocol_error(
                        "upstream_stream_decode_failed",
                        crate::error::sse_error_chain(&error),
                        HashMap::new(),
                    )
                    .await
            }
        };
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
