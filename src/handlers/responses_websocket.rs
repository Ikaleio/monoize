mod connection;
use connection::*;
mod lane;
use lane::*;
mod turn;
use turn::*;
mod bridge;
use bridge::*;
mod controls;
use controls::*;

use super::*;
use crate::upstream_websocket::{
    ResponsesWsControl, ResponsesWsLane, ResponsesWsRelay, ResponsesWsTurn, ResponsesWsTurnState,
    UPSTREAM_KIND_WEBSOCKET,
};
use axum::extract::State;
use axum::extract::ws::{Message, WebSocket, WebSocketUpgrade};
use axum::response::IntoResponse;
use futures_util::{SinkExt, StreamExt};
use serde_json::{Map, Value, json};
use std::collections::{BTreeMap, HashMap};
use std::sync::Mutex as StdMutex;
use std::sync::atomic::{AtomicBool, Ordering};
use tokio::sync::{Semaphore, mpsc, watch};

const DEFAULT_MESSAGE_MAX_BYTES: usize = 50 * 1024 * 1024;
const DEFAULT_CONNECTION_MAX_BYTES: usize = 1024 * 1024 * 1024;
const DEFAULT_MAX_TURNS: usize = 4_096;
const DEFAULT_HISTORY_MAX_ITEMS: usize = 16_384;
const DEFAULT_HISTORY_MAX_BYTES: usize = 32 * 1024 * 1024;
const DEFAULT_MAX_AGE_SECS: usize = 3_600;
/// WS4a: official per-connection concurrency and named-lane bounds.
const MAX_GENERATING_TURNS: usize = 16;
const MAX_NAMED_LANES: usize = 32;
const MAX_STREAM_ID_LEN: usize = 256;
/// WS15a: bound on each downstream write and on the final close.
const WRITE_DEADLINE: std::time::Duration = std::time::Duration::from_secs(30);
const FIRST_EVENT_MESSAGE: &str =
    "Expected a 'response.create' message as the first websocket event.";

#[derive(Clone, Copy, Debug)]
struct ResponsesWebsocketLimits {
    message_max_bytes: usize,
    connection_max_bytes: usize,
    max_turns: usize,
    history_max_items: usize,
    history_max_bytes: usize,
    sse_frame_max_bytes: usize,
    sse_buffer_max_bytes: usize,
    max_age_secs: usize,
}

impl Default for ResponsesWebsocketLimits {
    fn default() -> Self {
        Self {
            message_max_bytes: DEFAULT_MESSAGE_MAX_BYTES,
            connection_max_bytes: DEFAULT_CONNECTION_MAX_BYTES,
            max_turns: DEFAULT_MAX_TURNS,
            history_max_items: DEFAULT_HISTORY_MAX_ITEMS,
            history_max_bytes: DEFAULT_HISTORY_MAX_BYTES,
            sse_frame_max_bytes: DEFAULT_MESSAGE_MAX_BYTES,
            sse_buffer_max_bytes: DEFAULT_MESSAGE_MAX_BYTES,
            max_age_secs: DEFAULT_MAX_AGE_SECS,
        }
    }
}

impl ResponsesWebsocketLimits {
    fn from_env() -> Self {
        let defaults = Self::default();
        Self {
            message_max_bytes: positive_env(
                "MONOIZE_RESPONSES_WS_MESSAGE_MAX_BYTES",
                defaults.message_max_bytes,
            ),
            connection_max_bytes: positive_env(
                "MONOIZE_RESPONSES_WS_CONNECTION_MAX_BYTES",
                defaults.connection_max_bytes,
            ),
            max_turns: positive_env("MONOIZE_RESPONSES_WS_MAX_TURNS", defaults.max_turns),
            history_max_items: positive_env(
                "MONOIZE_RESPONSES_WS_HISTORY_MAX_ITEMS",
                defaults.history_max_items,
            ),
            history_max_bytes: positive_env(
                "MONOIZE_RESPONSES_WS_HISTORY_MAX_BYTES",
                defaults.history_max_bytes,
            ),
            sse_frame_max_bytes: positive_env(
                "MONOIZE_RESPONSES_WS_SSE_FRAME_MAX_BYTES",
                defaults.sse_frame_max_bytes,
            ),
            sse_buffer_max_bytes: positive_env(
                "MONOIZE_RESPONSES_WS_SSE_BUFFER_MAX_BYTES",
                defaults.sse_buffer_max_bytes,
            ),
            max_age_secs: positive_env("MONOIZE_RESPONSES_WS_MAX_AGE_SECS", defaults.max_age_secs),
        }
    }

    fn fits(&self, items: usize, bytes: usize) -> bool {
        items <= self.history_max_items && bytes <= self.history_max_bytes
    }
}

/// `None` is the default lane; `Some(name)` a named `stream_id` lane (WS4a).
type LaneKey = Option<String>;

/// WS7: one lane's retained response.
struct CacheEntry {
    lane: LaneKey,
    public_id: String,
    input: Vec<Value>,
    output: Vec<Value>,
    /// Steer inputs the upstream left pending for required input (WS7).
    pending_steers: Vec<Value>,
    /// The turn's normalized request without `input`.
    controls: Map<String, Value>,
    strip_encrypted: bool,
    appendable: bool,
}

struct ActiveTurn {
    public_id: String,
    control: Arc<watch::Sender<ResponsesWsControl>>,
    steer: mpsc::UnboundedSender<Value>,
    state: Arc<ResponsesWsTurnState>,
}

struct Connection {
    state: AppState,
    headers: HeaderMap,
    limits: ResponsesWebsocketLimits,
    connection_request_id: String,
    outbound: mpsc::Sender<Message>,
    v1: AtomicBool,
    closed: AtomicBool,
    entries: StdMutex<Vec<CacheEntry>>,
    /// WS8a: lane creation order for `item_reference` resolution.
    lane_order: StdMutex<HashMap<LaneKey, usize>>,
    /// WS5a: request models whose responses contained reasoning items.
    active: StdMutex<HashMap<LaneKey, ActiveTurn>>,
    generating: Arc<Semaphore>,
    /// WS14: started turns; rejected events never count.
    accepted_turns: std::sync::atomic::AtomicUsize,
    /// Wakes the reader when a lane ends the connection (WS15).
    shutdown: tokio::sync::Notify,
    /// WS2g: observed by upstream feeders after their turn left `active`.
    lifetime: watch::Sender<bool>,
}

#[derive(Debug)]
struct WsError {
    status: u16,
    error_type: String,
    code: String,
    message: String,
    param: Option<String>,
}

impl WsError {
    fn new(status: u16, code: &str, message: impl Into<String>, param: Option<&str>) -> Self {
        Self {
            status,
            error_type: default_error_type(status).to_string(),
            code: code.to_string(),
            message: message.into(),
            param: param.map(str::to_string),
        }
    }

    fn invalid(message: impl Into<String>) -> Self {
        Self::new(400, "invalid_websocket_event", message, None)
    }

    fn invalid_request(message: &str, param: &str) -> Self {
        Self::new(400, "invalid_request", message, Some(param))
    }

    fn previous_response_not_found(id: &str) -> Self {
        Self::new(
            400,
            "previous_response_not_found",
            format!("Previous response with id '{id}' not found."),
            Some("previous_response_id"),
        )
    }

    fn connection_limit(quota: &str) -> Self {
        Self::new(
            400,
            "websocket_connection_limit_reached",
            format!(
                "Responses websocket connection limit reached ({quota}). Create a new websocket connection to continue."
            ),
            None,
        )
    }

    fn history_limit() -> Self {
        Self::new(
            400,
            "websocket_history_limit_exceeded",
            "WebSocket continuation history limit exceeded",
            Some("input"),
        )
    }

    fn parser_limit() -> Self {
        Self::new(
            502,
            "websocket_sse_limit_exceeded",
            "upstream SSE frame exceeded the WebSocket bridge limit",
            None,
        )
    }

    fn from_app(err: &AppError) -> Self {
        let status = err.upstream_status.unwrap_or(err.status.as_u16());
        Self {
            status,
            error_type: err
                .upstream_type
                .clone()
                .unwrap_or_else(|| err.error_type.clone()),
            code: err
                .upstream_code
                .clone()
                .unwrap_or_else(|| err.code.clone()),
            message: err.message.clone(),
            param: err.upstream_param.clone().or_else(|| err.param.clone()),
        }
    }

    /// WS12c: a native `{"error":{...}}` body or an SSE `error` event (flat or nested).
    fn from_error_value(value: &Value, http_status: Option<u16>) -> Self {
        let error = value
            .get("error")
            .filter(|error| error.is_object())
            .unwrap_or(value);
        let text = |key: &str| error.get(key).and_then(Value::as_str).map(str::to_string);
        let error_type = text("type").filter(|kind| kind != "error");
        let status = http_status
            .or_else(|| {
                value
                    .get("status")
                    .and_then(Value::as_u64)
                    .map(|s| s as u16)
            })
            .or_else(|| {
                error
                    .get("status")
                    .and_then(Value::as_u64)
                    .map(|s| s as u16)
            })
            .unwrap_or_else(|| status_for_error_type(error_type.as_deref()));
        Self {
            status,
            error_type: error_type.unwrap_or_else(|| default_error_type(status).to_string()),
            code: text("code").unwrap_or_else(|| default_error_type(status).to_string()),
            message: text("message").unwrap_or_else(|| "upstream error".to_string()),
            param: text("param"),
        }
    }

    fn frame(&self) -> Value {
        json!({
            "type": "error",
            "status": self.status,
            "error": {
                "type": self.error_type,
                "code": self.code,
                "message": self.message,
                "param": self.param,
            }
        })
    }
}

fn default_error_type(status: u16) -> &'static str {
    if (400..500).contains(&status) {
        "invalid_request_error"
    } else {
        "server_error"
    }
}

fn status_for_error_type(error_type: Option<&str>) -> u16 {
    match error_type {
        Some("invalid_request_error") => 400,
        Some("authentication_error") => 401,
        Some("permission_error") => 403,
        Some("not_found_error") => 404,
        Some("rate_limit_error") => 429,
        _ => 500,
    }
}

impl Connection {
    fn is_closed(&self) -> bool {
        self.closed.load(Ordering::Acquire)
    }

    fn mark_closed(&self) {
        self.closed.store(true, Ordering::Release);
        self.lifetime.send_replace(true);
    }

    fn is_v1(&self) -> bool {
        self.v1.load(Ordering::Acquire)
    }

    /// WS4a: tags `stream_id` for named lanes and queues the text frame.
    async fn send(&self, lane: &LaneKey, mut value: Value) -> bool {
        if let (Some(stream_id), Some(object)) = (lane, value.as_object_mut()) {
            object.insert("stream_id".to_string(), Value::String(stream_id.clone()));
        }
        self.outbound
            .send(Message::Text(value.to_string().into()))
            .await
            .is_ok()
    }

    async fn send_error(&self, lane: &LaneKey, error: WsError) -> bool {
        self.send(lane, error.frame()).await
    }

    fn set_entry(&self, entry: CacheEntry) {
        let Ok(mut entries) = self.entries.lock() else {
            return;
        };
        match entries
            .iter_mut()
            .find(|current| current.lane == entry.lane)
        {
            Some(current) => *current = entry,
            None => entries.push(entry),
        }
    }

    fn set_pending_steers(&self, lane: &LaneKey, public_id: &str, items: &[Value]) {
        if let Ok(mut entries) = self.entries.lock()
            && let Some(entry) = entries
                .iter_mut()
                .find(|entry| entry.lane == *lane && entry.public_id == public_id)
        {
            entry.pending_steers = items.to_vec();
        }
    }

    fn remove_entry(&self, lane: &LaneKey, public_id: Option<&str>) {
        if let Ok(mut entries) = self.entries.lock() {
            entries.retain(|entry| {
                entry.lane != *lane || public_id.is_some_and(|id| entry.public_id != id)
            });
        }
    }

    fn entry_lane(&self, public_id: &str) -> Option<LaneKey> {
        let entries = self.entries.lock().ok()?;
        entries
            .iter()
            .find(|entry| entry.public_id == public_id)
            .map(|entry| entry.lane.clone())
    }

    /// WS8a: replaces connection-retained `item_reference` inputs.
    fn resolve_item_references(&self, items: &mut [Value]) {
        let Ok(entries) = self.entries.lock() else {
            return;
        };
        let order = self
            .lane_order
            .lock()
            .map(|order| order.clone())
            .unwrap_or_default();
        let mut ordered: Vec<&CacheEntry> = entries.iter().collect();
        ordered.sort_by_key(|entry| {
            (
                entry.lane.is_some(),
                order.get(&entry.lane).copied().unwrap_or(usize::MAX),
            )
        });
        for item in items.iter_mut() {
            if item.get("type").and_then(Value::as_str) != Some("item_reference") {
                continue;
            }
            let Some(id) = item.get("id").and_then(Value::as_str) else {
                continue;
            };
            let found = ordered.iter().find_map(|entry| {
                entry
                    .input
                    .iter()
                    .chain(entry.output.iter())
                    .find(|candidate| candidate.get("id").and_then(Value::as_str) == Some(id))
            });
            if let Some(found) = found {
                *item = found.clone();
            }
        }
    }
}

pub async fn responses_websocket(
    State(state): State<AppState>,
    headers: HeaderMap,
    ws: WebSocketUpgrade,
) -> AppResult<Response> {
    auth_tenant(&headers, &state).await?;
    let limits = ResponsesWebsocketLimits::from_env();
    Ok(ws
        .max_message_size(limits.message_max_bytes)
        .max_frame_size(limits.message_max_bytes)
        .on_upgrade(move |socket| serve_responses_websocket(socket, state, headers, limits))
        .into_response())
}

fn serialized_len(value: &(impl serde::Serialize + ?Sized)) -> usize {
    serde_json::to_vec(value)
        .map(|bytes| bytes.len())
        .unwrap_or(usize::MAX)
}

fn positive_env(name: &str, default: usize) -> usize {
    std::env::var(name)
        .ok()
        .and_then(|value| value.parse::<usize>().ok())
        .filter(|value| *value > 0)
        .unwrap_or(default)
}
