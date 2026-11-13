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
use crate::payload::sse::{FrameData, Frames};
use crate::urp::UrpStreamEvent;
use axum::http::StatusCode;
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
    let mut frames = Frames::new(upstream_resp, idle_timeout_ms, false);
    let mut decoder = ResponsesStreamDecoder::new(
        urp,
        pending_request_envelope_extra,
        tx,
        started_at,
        runtime_metrics,
    );
    loop {
        let frame = match frames.next().await {
            Ok(Some(frame)) => frame,
            Ok(None) => break,
            Err(error) => {
                decoder.release_held(None, None).await;
                return Err(error.into_app());
            }
        };
        match decoder.on_frame(frame.event, frame.data).await {
            Flow::Continue => {}
            Flow::End => break,
            Flow::Stop => return Ok(()),
        }
    }
    decoder.finish().await;
    Ok(())
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
        match decoder.on_frame(event_name, FrameData::from(data)).await {
            Flow::Continue => {}
            Flow::End => break,
            Flow::Stop => return Ok(()),
        }
    }
    decoder.finish().await;
    Ok(())
}
