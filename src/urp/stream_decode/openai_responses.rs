//! Upstream OpenAI Responses stream decoding: SSE events, or JSON frames from an upstream
//! WebSocket, into URP stream events.

mod calls;
mod content;
mod errors;
mod frame;
mod gate;
mod images;
mod item;
mod items;
mod merge;
mod nodes;
mod reasoning;
mod state;
mod terminal;

use crate::error::{AppError, AppResult};
use crate::handlers::{StreamRuntimeMetrics, UrpRequest as HandlerUrpRequest};
use crate::urp::UrpStreamEvent;
use axum::http::StatusCode;
use eventsource_stream::Eventsource;
use futures_util::StreamExt;
use serde_json::Value;
use state::{Flow, ResponsesStreamDecoder};
use std::collections::HashMap;
use std::sync::Arc;
use std::time::{Duration, Instant};
use tokio::sync::{Mutex, mpsc};

pub(crate) async fn stream_responses_to_urp_events(
    urp: &HandlerUrpRequest,
    pending_request_envelope_extra: Option<HashMap<String, Value>>,
    upstream_resp: reqwest::Response,
    tx: mpsc::Sender<UrpStreamEvent>,
    started_at: Option<Instant>,
    runtime_metrics: Option<Arc<Mutex<StreamRuntimeMetrics>>>,
    idle_timeout_ms: u64,
) -> AppResult<()> {
    let (frame_tx, frame_rx) = mpsc::channel(32);
    let feeder = tokio::spawn(feed_sse_frames(upstream_resp, frame_tx));
    let result = stream_responses_frames_to_urp_events(
        urp,
        pending_request_envelope_extra,
        frame_rx,
        tx,
        started_at,
        runtime_metrics,
        idle_timeout_ms,
    )
    .await;
    feeder.abort();
    result
}

/// Decodes Responses JSON frames supplied by an upstream WebSocket feeder
/// (`crate::upstream_websocket`, WS2d) with the same decoder as SSE events.
pub(crate) async fn stream_responses_frames_to_urp_events(
    urp: &HandlerUrpRequest,
    pending_request_envelope_extra: Option<HashMap<String, Value>>,
    mut frames: mpsc::Receiver<AppResult<(String, String)>>,
    tx: mpsc::Sender<UrpStreamEvent>,
    started_at: Option<Instant>,
    runtime_metrics: Option<Arc<Mutex<StreamRuntimeMetrics>>>,
    idle_timeout_ms: u64,
) -> AppResult<()> {
    let mut decoder = ResponsesStreamDecoder::new(
        urp,
        pending_request_envelope_extra,
        tx,
        started_at,
        runtime_metrics,
    );
    let idle_timeout = Duration::from_millis(idle_timeout_ms.max(1));
    loop {
        let (event_name, data) = match tokio::time::timeout(idle_timeout, frames.recv()).await {
            Ok(Some(Ok(frame))) => frame,
            Ok(Some(Err(error))) => {
                decoder.release_held(None, None).await;
                return Err(error);
            }
            Ok(None) => break,
            Err(_) => {
                decoder.release_held(None, None).await;
                return Err(AppError::new(
                    StatusCode::GATEWAY_TIMEOUT,
                    "upstream_idle_timeout",
                    format!("upstream stream idle for {idle_timeout_ms}ms without data"),
                ));
            }
        };
        match decoder.on_frame(event_name, data).await {
            Flow::Continue => {}
            Flow::End => break,
            Flow::Stop => return Ok(()),
        }
    }
    decoder.finish().await;
    Ok(())
}

async fn feed_sse_frames(
    upstream_resp: reqwest::Response,
    frame_tx: mpsc::Sender<AppResult<(String, String)>>,
) {
    let mut stream = upstream_resp.bytes_stream().eventsource();
    while let Some(event) = stream.next().await {
        let frame = event
            .map(|event| (event.event, event.data))
            .map_err(|error| {
                AppError::new(
                    StatusCode::BAD_GATEWAY,
                    "upstream_stream_decode_failed",
                    crate::error::sse_error_chain(&error),
                )
            });
        let failed = frame.is_err();
        if frame_tx.send(frame).await.is_err() || failed {
            break;
        }
    }
}
