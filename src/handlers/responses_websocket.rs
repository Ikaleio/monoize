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

struct LaneHandle {
    jobs: mpsc::UnboundedSender<LaneJob>,
}

struct LaneJob {
    append: bool,
    event: Map<String, Value>,
}

async fn serve_responses_websocket(
    socket: WebSocket,
    state: AppState,
    headers: HeaderMap,
    limits: ResponsesWebsocketLimits,
) {
    let (mut sink, mut stream) = socket.split();
    let (outbound, mut outbound_rx) = mpsc::channel::<Message>(256);
    let writer = tokio::spawn(async move {
        while let Some(message) = outbound_rx.recv().await {
            let close = matches!(message, Message::Close(_));
            // WS15a: a peer that stops reading cannot hold the connection open.
            let sent = tokio::time::timeout(WRITE_DEADLINE, sink.send(message)).await;
            if !matches!(sent, Ok(Ok(()))) || close {
                break;
            }
        }
        let _ = tokio::time::timeout(WRITE_DEADLINE, sink.close()).await;
    });

    let beta = headers
        .get("openai-beta")
        .and_then(|value| value.to_str().ok())
        .unwrap_or("")
        .to_string();
    let v2_header = beta.contains("responses_websockets=2026-02-06");
    let v1_header = beta.contains("responses_websockets=2026-02-04") && !v2_header;
    let connection_request_id = extract_request_id(&headers).unwrap_or_default();
    let conn = Arc::new(Connection {
        state,
        headers,
        limits,
        connection_request_id,
        outbound,
        v1: AtomicBool::new(v1_header),
        closed: AtomicBool::new(false),
        entries: StdMutex::new(Vec::new()),
        lane_order: StdMutex::new(HashMap::new()),
        active: StdMutex::new(HashMap::new()),
        generating: Arc::new(Semaphore::new(MAX_GENERATING_TURNS)),
        accepted_turns: std::sync::atomic::AtomicUsize::new(0),
        shutdown: tokio::sync::Notify::new(),
        lifetime: watch::channel(false).0,
    });

    let mut lanes: HashMap<LaneKey, LaneHandle> = HashMap::new();
    let mut named_lanes = 0usize;
    let mut inbound_bytes = 0usize;
    let mut first_text = true;
    let max_age = tokio::time::sleep(std::time::Duration::from_secs(limits.max_age_secs as u64));
    tokio::pin!(max_age);

    loop {
        let message = tokio::select! {
            message = stream.next() => message,
            () = conn.shutdown.notified() => break,
            _ = &mut max_age => {
                // WS15a: the official 60-minute connection lifetime.
                let _ = tokio::time::timeout(
                    WRITE_DEADLINE,
                    conn.send_error(&None, WsError::connection_limit("60 minutes")),
                )
                .await;
                break;
            }
        };
        let message = match message {
            Some(Ok(message)) => message,
            Some(Err(err)) => {
                tracing::debug!(error = %err, "Responses WebSocket receive failed");
                break;
            }
            None => break,
        };
        let text = match message {
            Message::Text(text) => text,
            Message::Binary(_) => {
                if !conn
                    .send_error(
                        &None,
                        WsError::invalid("binary WebSocket messages are not supported"),
                    )
                    .await
                {
                    break;
                }
                continue;
            }
            Message::Ping(payload) => {
                if conn.outbound.send(Message::Pong(payload)).await.is_err() {
                    break;
                }
                continue;
            }
            Message::Pong(_) => continue,
            Message::Close(_) => break,
        };
        inbound_bytes = inbound_bytes.saturating_add(text.len());
        if inbound_bytes > limits.connection_max_bytes {
            let _ = conn
                .send_error(&None, WsError::connection_limit("inbound byte quota"))
                .await;
            break;
        }
        let event = match serde_json::from_str::<Value>(text.as_str()) {
            Ok(Value::Object(event)) => Ok(event),
            Ok(_) => Err(WsError::invalid("WebSocket event must be a JSON object")),
            Err(err) => Err(WsError::invalid(format!("invalid JSON: {err}"))),
        };
        let kind = event
            .as_ref()
            .ok()
            .and_then(|event| event.get("type"))
            .and_then(Value::as_str)
            .map(str::to_string);
        if first_text {
            first_text = false;
            let valid_first = match kind.as_deref() {
                Some("response.create") => true,
                Some("response.append") => !v2_header,
                _ => false,
            };
            if !valid_first {
                // WS12: the official first-event contract closes the socket.
                let _ = conn
                    .send_error(
                        &None,
                        WsError::new(
                            400,
                            "invalid_websocket_event",
                            FIRST_EVENT_MESSAGE,
                            Some("type"),
                        ),
                    )
                    .await;
                break;
            }
        }
        let event = match event {
            Ok(event) => event,
            Err(error) => {
                if !conn.send_error(&None, error).await {
                    break;
                }
                continue;
            }
        };
        match kind.as_deref() {
            Some(kind @ ("response.create" | "response.append")) => {
                let append = kind == "response.append";
                if append && !v2_header {
                    conn.v1.store(true, Ordering::Release);
                }
                let lane: LaneKey = match event.get("stream_id") {
                    None => None,
                    Some(Value::String(stream_id)) if valid_stream_id(stream_id) => {
                        Some(stream_id.clone())
                    }
                    Some(_) => {
                        let error = WsError::new(
                            400,
                            "invalid_stream_id",
                            "The 'stream_id' field must be a non-empty string with at most 256 characters and may only contain letters, numbers, underscores, hyphens, and periods.",
                            Some("stream_id"),
                        );
                        if !conn.send_error(&None, error).await {
                            break;
                        }
                        continue;
                    }
                };
                if lane.is_some() && !lanes.contains_key(&lane) && named_lanes >= MAX_NAMED_LANES {
                    let error = WsError::new(
                        400,
                        "websocket_stream_limit_reached",
                        "This WebSocket connection has reached its maximum number of distinct stream IDs (32). Reuse an existing stream_id or open a new WebSocket connection.",
                        Some("stream_id"),
                    );
                    if !conn.send_error(&lane, error).await {
                        break;
                    }
                    continue;
                }
                let handle = lanes.entry(lane.clone()).or_insert_with(|| {
                    if lane.is_some() {
                        named_lanes += 1;
                    }
                    if let Ok(mut order) = conn.lane_order.lock() {
                        let next = order.len();
                        order.entry(lane.clone()).or_insert(next);
                    }
                    spawn_lane(conn.clone(), lane.clone())
                });
                let _ = handle.jobs.send(LaneJob { append, event });
            }
            Some("response.interrupt") => {
                if let Some(error) = handle_interrupt(&conn, &event).await
                    && !conn.send_error(&None, error).await
                {
                    break;
                }
            }
            Some("response.steer") => handle_steer(&conn, &event).await,
            Some(other) => {
                if !conn
                    .send_error(
                        &None,
                        WsError::invalid(format!("unsupported WebSocket event type '{other}'")),
                    )
                    .await
                {
                    break;
                }
            }
            None => {
                if !conn
                    .send_error(&None, WsError::invalid("WebSocket event is missing type"))
                    .await
                {
                    break;
                }
            }
        }
    }

    conn.mark_closed();
    if let Ok(active) = conn.active.lock() {
        for turn in active.values() {
            turn.control.send_replace(ResponsesWsControl::Disconnected);
        }
    }
    drop(lanes);
    let _ = tokio::time::timeout(WRITE_DEADLINE, conn.outbound.send(Message::Close(None))).await;
    drop(conn);
    let mut writer = writer;
    if tokio::time::timeout(WRITE_DEADLINE, &mut writer)
        .await
        .is_err()
    {
        writer.abort();
    }
}

fn valid_stream_id(stream_id: &str) -> bool {
    !stream_id.is_empty()
        && stream_id.len() <= MAX_STREAM_ID_LEN
        && stream_id
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'_' | b'-' | b'.'))
}

fn spawn_lane(conn: Arc<Connection>, key: LaneKey) -> LaneHandle {
    let (relay_tx, relay_rx) = mpsc::unbounded_channel::<ResponsesWsRelay>();
    let lane = Arc::new(ResponsesWsLane::new(relay_tx));
    let (jobs, jobs_rx) = mpsc::unbounded_channel();
    tokio::spawn(run_lane(conn, key, lane, jobs_rx, relay_rx));
    LaneHandle { jobs }
}

/// WS4b: the public response a relayed event belongs to.
fn relay_response_id(value: &Value) -> Option<String> {
    value
        .pointer("/steer/previous_response_id")
        .and_then(Value::as_str)
        .map(str::to_string)
        .or_else(|| crate::upstream_websocket::find_response_id(value).map(str::to_string))
}

/// WS4b: an acknowledgement takes the next number of the response it names;
/// provider-control notifications keep their upstream fields.
async fn forward_relay(
    conn: &Connection,
    key: &LaneKey,
    mut value: Value,
    sequences: &mut HashMap<String, i64>,
) -> bool {
    let kind = value
        .get("type")
        .and_then(Value::as_str)
        .unwrap_or_default();
    if (kind.starts_with("response.interrupt.") || kind.starts_with("response.steer."))
        && value.is_object()
    {
        let next = relay_response_id(&value).and_then(|id| sequences.get_mut(&id));
        match next {
            Some(next) => {
                value["sequence_number"] = json!(*next);
                *next += 1;
            }
            None => {
                if let Some(object) = value.as_object_mut() {
                    object.remove("sequence_number");
                }
            }
        }
    }
    conn.send(key, value).await
}

/// The input of one turn after WS8/WS10/WS18 assembly.
struct PreparedTurn {
    request: Map<String, Value>,
    full_input: Vec<Value>,
    previous: Option<String>,
    same_lane_previous: bool,
    warmup: bool,
    strip_encrypted: bool,
    client_metadata: Option<Value>,
    successor: Option<(String, String)>,
    /// WS18: `full_input` was cut to the steer input because the history exceeds
    /// the limit, so the turn can never be retained.
    oversized: bool,
}

struct PrepareError {
    error: Box<WsError>,
    evict: Option<String>,
}

impl From<WsError> for PrepareError {
    fn from(error: WsError) -> Self {
        Self {
            error: Box::new(error),
            evict: None,
        }
    }
}

/// A finished turn kept for a possible WS18 successor.
struct FinishedTurn {
    public_id: String,
    controls: Map<String, Value>,
    full_input: Vec<Value>,
    output: Vec<Value>,
    pending_steers: Vec<Value>,
    strip_encrypted: bool,
    /// WS17: history exceeds the limit; the vectors above hold nothing retained.
    oversized: bool,
}

async fn run_lane(
    conn: Arc<Connection>,
    key: LaneKey,
    lane: Arc<ResponsesWsLane>,
    mut jobs: mpsc::UnboundedReceiver<LaneJob>,
    mut relay: mpsc::UnboundedReceiver<ResponsesWsRelay>,
) {
    let mut last: Option<FinishedTurn> = None;
    // WS4b: next sequence number per public response of this lane.
    let mut sequences: HashMap<String, i64> = HashMap::new();
    loop {
        if conn.is_closed() {
            break;
        }
        let successor = lane.take_successor();
        let job = match successor {
            Some(successor) => Err(successor),
            None => {
                let job = tokio::select! {
                    biased;
                    value = relay.recv() => {
                        if let Some(value) = value {
                            forward_relay(&conn, &key, value.value, &mut sequences).await;
                        }
                        continue;
                    }
                    job = jobs.recv() => job,
                };
                match job {
                    Some(job) => Ok(job),
                    None => break,
                }
            }
        };
        if conn.is_closed() {
            break;
        }
        let Ok(permit) = conn.generating.clone().acquire_owned().await else {
            break;
        };
        // Queued turns may wait here past a downstream close; they must not start.
        if conn.is_closed() {
            break;
        }
        let prepared = match job {
            Ok(job) => prepare_job(&conn, &key, job),
            Err(successor) => match last.take() {
                Some(steered) => Ok(prepare_successor(&conn.limits, steered, successor)),
                None => {
                    // No steered turn to continue: drop the upstream successor
                    // instead of letting a later turn read its frames.
                    lane.close().await;
                    continue;
                }
            },
        };
        let prepared = match prepared {
            Ok(prepared) => prepared,
            Err(failure) => {
                if let Some(previous) = failure.evict {
                    conn.remove_entry(&key, Some(&previous));
                }
                let _ = conn.send_error(&key, *failure.error).await;
                continue;
            }
        };
        // WS14/WS15: only started turns count toward the turn quota.
        if conn.accepted_turns.fetch_add(1, Ordering::AcqRel) + 1 > conn.limits.max_turns {
            let _ = conn
                .send_error(&None, WsError::connection_limit("turn quota"))
                .await;
            conn.mark_closed();
            conn.shutdown.notify_one();
            break;
        }
        last = run_turn(&conn, &key, &lane, prepared, &mut relay, &mut sequences).await;
        drop(permit);
    }
    lane.close().await;
}

fn input_items(input: Option<&Value>) -> Result<Vec<Value>, WsError> {
    match input {
        None | Some(Value::Null) => Ok(Vec::new()),
        Some(Value::Array(items)) => Ok(items.clone()),
        Some(Value::String(text)) => Ok(vec![json!({"role": "user", "content": text})]),
        Some(object @ Value::Object(_)) => Ok(vec![object.clone()]),
        Some(_) => Err(WsError::invalid(
            "Responses input must be a string, object, or array",
        )),
    }
}

fn concat_history(
    limits: &ResponsesWebsocketLimits,
    entry: &CacheEntry,
    incoming: Vec<Value>,
) -> Result<Vec<Value>, WsError> {
    let items = entry
        .input
        .len()
        .saturating_add(entry.output.len())
        .saturating_add(entry.pending_steers.len())
        .saturating_add(incoming.len());
    if items > limits.history_max_items {
        return Err(WsError::history_limit());
    }
    let mut full = Vec::with_capacity(items);
    full.extend(entry.input.iter().cloned());
    full.extend(entry.output.iter().cloned());
    full.extend(entry.pending_steers.iter().cloned());
    full.extend(incoming);
    if serialized_len(&full) > limits.history_max_bytes {
        return Err(WsError::history_limit());
    }
    Ok(full)
}

/// WS5: `include` is absent or an array of strings; null means absent.
fn normalize_include(request: &mut Map<String, Value>) -> Result<(), WsError> {
    match request.get("include") {
        None => Ok(()),
        Some(Value::Null) => {
            request.remove("include");
            Ok(())
        }
        Some(Value::Array(values)) if values.iter().all(Value::is_string) => Ok(()),
        Some(_) => Err(WsError::invalid_request(
            "include must be an array of strings",
            "include",
        )),
    }
}

/// WS5a: request encrypted reasoning for replay; true when the client did not
/// ask for it, so it must be stripped downstream. `include` is normalized.
fn ensure_encrypted_reasoning(request: &mut Map<String, Value>) -> bool {
    let include = request
        .entry("include")
        .or_insert_with(|| Value::Array(Vec::new()));
    let Some(include) = include.as_array_mut() else {
        return false;
    };
    if include
        .iter()
        .any(|value| value.as_str() == Some("reasoning.encrypted_content"))
    {
        return false;
    }
    include.push(json!("reasoning.encrypted_content"));
    true
}

fn finish_request(
    limits: &ResponsesWebsocketLimits,
    mut controls: Map<String, Value>,
    full_input: &[Value],
) -> Result<Map<String, Value>, WsError> {
    controls.insert("input".to_string(), Value::Array(full_input.to_vec()));
    controls.insert("stream".to_string(), Value::Bool(true));
    if !limits.fits(full_input.len(), serialized_len(&controls)) {
        return Err(WsError::history_limit());
    }
    Ok(controls)
}

fn prepare_job(
    conn: &Connection,
    key: &LaneKey,
    job: LaneJob,
) -> Result<PreparedTurn, PrepareError> {
    let limits = &conn.limits;
    let mut event = job.event;
    if job.append {
        // WS10: legacy append continues the lane's appendable entry.
        if !event.contains_key("input") {
            return Err(WsError::invalid("response.append is missing input").into());
        }
        let mut incoming = input_items(event.get("input"))?;
        conn.resolve_item_references(&mut incoming);
        let entries = conn
            .entries
            .lock()
            .map_err(|_| WsError::invalid("connection state is unavailable"))?;
        let Some(entry) = entries
            .iter()
            .find(|entry| entry.lane == *key && entry.appendable)
        else {
            return Err(WsError::previous_response_not_found("<append>").into());
        };
        let previous = entry.public_id.clone();
        let evict = |error: WsError| PrepareError {
            error: Box::new(error),
            evict: Some(previous.clone()),
        };
        let full_input = concat_history(limits, entry, incoming).map_err(evict)?;
        let mut controls = entry.controls.clone();
        // A warmup entry never requested encrypted reasoning (WS5a).
        let strip_encrypted = ensure_encrypted_reasoning(&mut controls) || entry.strip_encrypted;
        drop(entries);
        let client_metadata = controls.get("client_metadata").cloned();
        let request = finish_request(limits, controls, &full_input).map_err(evict)?;
        return Ok(PreparedTurn {
            request,
            full_input,
            previous: Some(previous),
            same_lane_previous: true,
            warmup: false,
            strip_encrypted,
            client_metadata,
            successor: None,
            oversized: false,
        });
    }

    let previous = match event.remove("previous_response_id") {
        None | Some(Value::Null) => None,
        Some(Value::String(previous)) => Some(previous),
        Some(_) => {
            return Err(WsError::invalid_request(
                "previous_response_id must be a string",
                "previous_response_id",
            )
            .into());
        }
    };
    // WS7: any rejection of a same-lane continuation evicts the referenced entry.
    prepare_create(conn, key, event, previous.clone()).map_err(|error| PrepareError {
        error: Box::new(error),
        evict: previous.filter(|previous| conn.entry_lane(previous).as_ref() == Some(key)),
    })
}

fn prepare_create(
    conn: &Connection,
    key: &LaneKey,
    mut event: Map<String, Value>,
    previous: Option<String>,
) -> Result<PreparedTurn, WsError> {
    let limits = &conn.limits;
    if previous.is_some()
        && event
            .get("conversation")
            .is_some_and(|value| !value.is_null())
    {
        return Err(WsError::invalid_request(
            "conversation and previous_response_id are mutually exclusive",
            "previous_response_id",
        ));
    }
    event.remove("type");
    event.remove("stream_id");
    let warmup = matches!(event.remove("generate"), Some(Value::Bool(false)));
    normalize_include(&mut event)?;
    if event.get("background").and_then(Value::as_bool) == Some(true) {
        return Err(WsError::new(
            400,
            "background_not_supported",
            "background not supported",
            Some("background"),
        ));
    }
    let mut incoming = input_items(event.remove("input").as_ref())?;
    conn.resolve_item_references(&mut incoming);
    let (full_input, same_lane_previous) = match previous.as_deref() {
        Some(previous) => {
            let entries = conn
                .entries
                .lock()
                .map_err(|_| WsError::invalid("connection state is unavailable"))?;
            let Some(entry) = entries.iter().find(|entry| entry.public_id == previous) else {
                return Err(WsError::previous_response_not_found(previous));
            };
            let same_lane = entry.lane == *key;
            (concat_history(limits, entry, incoming)?, same_lane)
        }
        None => (incoming, false),
    };
    let strip_encrypted = !warmup && ensure_encrypted_reasoning(&mut event);
    let client_metadata = event.get("client_metadata").cloned();
    let request = finish_request(limits, event, &full_input)?;
    Ok(PreparedTurn {
        request,
        full_input,
        previous,
        same_lane_previous,
        warmup,
        strip_encrypted,
        client_metadata,
        successor: None,
        oversized: false,
    })
}

/// WS18: steer input as Responses input items.
fn steer_items(input: Value) -> Vec<Value> {
    match input {
        Value::String(text) => vec![json!({"role": "user", "content": text})],
        Value::Array(items) => items,
        other => vec![other],
    }
}

/// WS18: the request of an automatic steering successor. The upstream already
/// generates it, so history limits decide retention (WS17), never admission; an
/// oversized history is not materialized.
fn prepare_successor(
    limits: &ResponsesWebsocketLimits,
    steered: FinishedTurn,
    successor: crate::upstream_websocket::ResponsesWsSuccessor,
) -> PreparedTurn {
    let mut steers = steered.pending_steers;
    steers.extend(successor.steer_inputs.into_iter().flat_map(steer_items));
    let oversized = steered.oversized
        || !limits.fits(
            steered
                .full_input
                .len()
                .saturating_add(steered.output.len())
                .saturating_add(steers.len()),
            serialized_len(&steered.full_input)
                .saturating_add(serialized_len(&steered.output))
                .saturating_add(serialized_len(&steers)),
        );
    let full_input = if oversized {
        steers
    } else {
        let mut full_input = steered.full_input;
        full_input.extend(steered.output);
        full_input.extend(steers);
        full_input
    };
    let client_metadata = steered.controls.get("client_metadata").cloned();
    let mut request = steered.controls;
    request.insert("input".to_string(), Value::Array(full_input.clone()));
    request.insert("stream".to_string(), Value::Bool(true));
    PreparedTurn {
        request,
        full_input,
        previous: Some(steered.public_id),
        same_lane_previous: false,
        warmup: false,
        strip_encrypted: steered.strip_encrypted,
        client_metadata,
        successor: Some((successor.channel_id, successor.upstream_model)),
        oversized,
    }
}

/// How a turn ended downstream.
#[derive(Default)]
struct TurnOutcome {
    status: Option<String>,
    output: Vec<Value>,
    appendable: bool,
    disconnected: bool,
    next_sequence: Option<i64>,
}

async fn run_turn(
    conn: &Arc<Connection>,
    key: &LaneKey,
    lane: &Arc<ResponsesWsLane>,
    prepared: PreparedTurn,
    relay: &mut mpsc::UnboundedReceiver<ResponsesWsRelay>,
    sequences: &mut HashMap<String, i64>,
) -> Option<FinishedTurn> {
    let public_id = format!("resp_monoize_{}", uuid::Uuid::new_v4().simple());
    let (control_tx, control_rx) = watch::channel(ResponsesWsControl::Running);
    let control_tx = Arc::new(control_tx);
    let (steer_tx, steer_rx) = mpsc::unbounded_channel();
    let state = Arc::new(ResponsesWsTurnState::default());
    if let Ok(mut active) = conn.active.lock() {
        active.insert(
            key.clone(),
            ActiveTurn {
                public_id: public_id.clone(),
                control: control_tx.clone(),
                steer: steer_tx,
                state: state.clone(),
            },
        );
    }
    // The reader sets `closed` before signalling registered turns, so a turn
    // registered after that signal observes `closed` here.
    if conn.is_closed() {
        if let Ok(mut active) = conn.active.lock() {
            active.remove(key);
        }
        return None;
    }
    let turn = ResponsesWsTurn {
        control: control_rx.clone(),
        steer: Arc::new(StdMutex::new(Some(steer_rx))),
        lane: lane.clone(),
        state: state.clone(),
        public_response_id: public_id.clone(),
        previous_response_id: prepared.previous.clone(),
        warmup: prepared.warmup,
        successor: prepared.successor.clone(),
        client_metadata: prepared.client_metadata.clone(),
        encrypted_include_added: prepared.strip_encrypted,
        connection_closed: conn.lifetime.subscribe(),
    };

    // WS11: every turn has its own request id; the upgrade id only correlates.
    let request_id = uuid::Uuid::new_v4().to_string();
    let mut headers = conn.headers.clone();
    if let Ok(value) = axum::http::HeaderValue::from_str(&request_id) {
        headers.insert("x-request-id", value);
    }
    tracing::debug!(
        connection_request_id = %conn.connection_request_id,
        request_id = %request_id,
        public_response_id = %public_id,
        "Responses WebSocket turn"
    );

    let input_bytes = serialized_len(&prepared.full_input);
    let bridge = TurnBridge {
        conn: conn.clone(),
        key: key.clone(),
        public_id: public_id.clone(),
        control: control_rx,
        state: state.clone(),
        strip_encrypted: prepared.strip_encrypted,
        input_items: prepared.full_input.len(),
        oversized: prepared.oversized,
        input_bytes,
    };
    let result = super::create_response_prefer_websocket(
        conn.state.clone(),
        headers,
        Value::Object(prepared.request.clone()),
        turn,
    )
    .await;
    let mut outcome = match result {
        Err(err) => {
            let _ = conn.send_error(key, WsError::from_app(&err)).await;
            TurnOutcome::default()
        }
        Ok(response) if !response.status().is_success() => {
            let status = response.status().as_u16();
            let body = axum::body::to_bytes(response.into_body(), 1024 * 1024)
                .await
                .unwrap_or_default();
            let value = serde_json::from_slice::<Value>(&body).unwrap_or(Value::Null);
            let _ = conn
                .send_error(key, WsError::from_error_value(&value, Some(status)))
                .await;
            TurnOutcome::default()
        }
        Ok(response) => bridge.run(response, relay, sequences).await,
    };
    if state.local_warmup() && outcome.status.is_none() && !outcome.disconnected {
        outcome = send_local_warmup(conn, key, &public_id, &prepared).await;
    }
    if let Some(sequence) = outcome.next_sequence {
        sequences.insert(public_id.clone(), sequence);
    }
    if let Ok(mut active) = conn.active.lock()
        && active
            .get(key)
            .is_some_and(|turn| turn.public_id == public_id)
    {
        active.remove(key);
    }
    drop(control_tx);

    let mut finished = if outcome.disconnected {
        None
    } else {
        match outcome.status.as_deref() {
            Some("completed" | "incomplete" | "cancelled") => {
                let mut controls = prepared.request;
                controls.remove("input");
                controls.remove("stream");
                let fits = !prepared.oversized
                    && conn.limits.fits(
                        prepared
                            .full_input
                            .len()
                            .saturating_add(outcome.output.len()),
                        input_bytes.saturating_add(serialized_len(&outcome.output)),
                    );
                if fits {
                    conn.set_entry(CacheEntry {
                        lane: key.clone(),
                        public_id: public_id.clone(),
                        input: prepared.full_input.clone(),
                        output: outcome.output.clone(),
                        pending_steers: Vec::new(),
                        controls: controls.clone(),
                        strip_encrypted: prepared.strip_encrypted,
                        appendable: outcome.appendable,
                    });
                } else {
                    // WS17: forwarded normally but no longer continuable.
                    conn.remove_entry(key, None);
                }
                Some(FinishedTurn {
                    public_id: public_id.clone(),
                    controls,
                    full_input: if fits {
                        prepared.full_input
                    } else {
                        Vec::new()
                    },
                    output: if fits { outcome.output } else { Vec::new() },
                    pending_steers: Vec::new(),
                    strip_encrypted: prepared.strip_encrypted,
                    oversized: !fits,
                })
            }
            _ => {
                if prepared.same_lane_previous
                    && let Some(previous) = prepared.previous.as_deref()
                {
                    conn.remove_entry(key, Some(previous));
                }
                None
            }
        }
    };
    if state.upstream_kind() == UPSTREAM_KIND_WEBSOCKET {
        // WS2i: the feeder may still be reading steering follow-ups; their
        // acknowledgements continue this response's sequence.
        let feeders = state.wait_feeders();
        tokio::pin!(feeders);
        loop {
            tokio::select! {
                biased;
                value = relay.recv() => match value {
                    Some(value) => {
                        forward_relay(conn, key, value.value, sequences).await;
                    }
                    None => break,
                },
                () = &mut feeders => break,
            }
        }
        // WS7: steering left pending for required input belongs to the history
        // the next continuation replays.
        let pending: Vec<Value> = state
            .take_pending_steers()
            .into_iter()
            .flat_map(steer_items)
            .collect();
        if !pending.is_empty()
            && let Some(finished) = finished.as_mut()
            && !finished.oversized
        {
            let fits = conn.limits.fits(
                finished
                    .full_input
                    .len()
                    .saturating_add(finished.output.len())
                    .saturating_add(pending.len()),
                input_bytes
                    .saturating_add(serialized_len(&finished.output))
                    .saturating_add(serialized_len(&pending)),
            );
            if fits {
                conn.set_pending_steers(key, &public_id, &pending);
                finished.pending_steers = pending;
            } else {
                conn.remove_entry(key, None);
                finished.full_input = Vec::new();
                finished.output = Vec::new();
                finished.oversized = true;
            }
        }
        // WS17: an evicted entry takes its encoded history with it.
        if finished.as_ref().is_some_and(|finished| finished.oversized) {
            lane.forget_continuation().await;
        }
    }
    // WS17: only a successor reads the finished turn; the cache entry holds the rest.
    if !lane.has_successor() {
        finished = None;
    }
    finished
}

/// WS6: local warmup answer when no upstream WebSocket can prepare it.
async fn send_local_warmup(
    conn: &Connection,
    key: &LaneKey,
    public_id: &str,
    prepared: &PreparedTurn,
) -> TurnOutcome {
    let created_at = chrono::Utc::now().timestamp();
    let mut response = Map::new();
    response.insert("id".to_string(), json!(public_id));
    response.insert("object".to_string(), json!("response"));
    response.insert("created_at".to_string(), json!(created_at));
    response.insert("status".to_string(), json!("in_progress"));
    response.insert("completed_at".to_string(), Value::Null);
    response.insert(
        "model".to_string(),
        prepared
            .request
            .get("model")
            .cloned()
            .unwrap_or(Value::Null),
    );
    response.insert("output".to_string(), json!([]));
    response.insert("error".to_string(), Value::Null);
    response.insert("incomplete_details".to_string(), Value::Null);
    response.insert("previous_response_id".to_string(), json!(prepared.previous));
    response.insert("usage".to_string(), Value::Null);
    for field in [
        "instructions",
        "tools",
        "tool_choice",
        "temperature",
        "top_p",
        "reasoning",
        "text",
        "max_output_tokens",
        "max_tool_calls",
        "parallel_tool_calls",
        "store",
        "metadata",
        "prompt_cache_key",
        "safety_identifier",
        "service_tier",
        "truncation",
        "include",
    ] {
        if let Some(value) = prepared.request.get(field) {
            response.insert(field.to_string(), value.clone());
        }
    }
    let created = json!({
        "type": "response.created",
        "sequence_number": 0,
        "response": Value::Object(response.clone()),
    });
    response.insert("status".to_string(), json!("completed"));
    response.insert("completed_at".to_string(), json!(created_at));
    let appendable = conn.is_v1();
    let completed = json!({
        "type": if appendable { "response.done" } else { "response.completed" },
        "sequence_number": 1,
        "response": Value::Object(response),
    });
    if !conn.send(key, created).await || !conn.send(key, completed).await {
        return TurnOutcome {
            disconnected: true,
            ..TurnOutcome::default()
        };
    }
    TurnOutcome {
        status: Some("completed".to_string()),
        output: Vec::new(),
        appendable,
        disconnected: false,
        next_sequence: Some(2),
    }
}

/// Converts one turn's pipeline SSE body into WebSocket events (WS4, WS12a,
/// WS12c, WS19, WS5a).
struct TurnBridge {
    oversized: bool,
    conn: Arc<Connection>,
    key: LaneKey,
    public_id: String,
    control: watch::Receiver<ResponsesWsControl>,
    state: Arc<ResponsesWsTurnState>,
    strip_encrypted: bool,
    input_items: usize,
    input_bytes: usize,
}

#[derive(Default)]
struct BridgeTracker {
    created: Option<Value>,
    done_items: BTreeMap<u64, Value>,
    next_sequence: Option<i64>,
    /// WS4b: this response's relays waiting for its lifecycle barrier.
    held: Vec<ResponsesWsRelay>,
    outcome: TurnOutcome,
    failed: bool,
}

enum BridgeFlow {
    Continue,
    Disconnected,
}

impl TurnBridge {
    async fn run(
        mut self,
        response: Response,
        relay: &mut mpsc::UnboundedReceiver<ResponsesWsRelay>,
        sequences: &mut HashMap<String, i64>,
    ) -> TurnOutcome {
        let mut body = response.into_body().into_data_stream();
        let mut buffer = Vec::new();
        let mut tracker = BridgeTracker::default();
        let mut interrupt_pending = false;
        let mut control_closed = false;
        let limits = self.conn.limits;

        loop {
            let chunk = tokio::select! {
                biased;
                changed = self.control.changed(), if !control_closed => {
                    match changed {
                        Err(_) => control_closed = true,
                        Ok(()) => {
                            let control = *self.control.borrow_and_update();
                            match control {
                                ResponsesWsControl::Disconnected => {
                                    return TurnOutcome { disconnected: true, ..TurnOutcome::default() };
                                }
                                ResponsesWsControl::Interrupt
                                    if self.state.upstream_kind() != UPSTREAM_KIND_WEBSOCKET =>
                                {
                                    interrupt_pending = true;
                                }
                                _ => {}
                            }
                        }
                    }
                    if interrupt_pending && tracker.created.is_some() && tracker.outcome.status.is_none() && !tracker.failed {
                        return self.emulate_interrupt(body, tracker).await;
                    }
                    continue;
                }
                value = relay.recv() => {
                    if let Some(value) = value
                        && !self.accept_relay(value, &mut tracker, sequences).await
                    {
                        return TurnOutcome { disconnected: true, ..TurnOutcome::default() };
                    }
                    continue;
                }
                chunk = body.next() => match chunk {
                    Some(chunk) => chunk,
                    None => break,
                },
            };
            let chunk = match chunk {
                Ok(chunk) => chunk,
                Err(err) => {
                    tracing::debug!(error = %err, "Responses WebSocket SSE bridge read failed");
                    if tracker.outcome.status.is_none() && !tracker.failed {
                        let _ = self
                            .conn
                            .send_error(
                                &self.key,
                                WsError::new(
                                    502,
                                    "server_error",
                                    "upstream SSE body read failed",
                                    None,
                                ),
                            )
                            .await;
                    }
                    let _ = self.release_relays(&mut tracker, true).await;
                    tracker.outcome.status = None;
                    tracker.outcome.next_sequence = tracker.next_sequence;
                    return tracker.outcome;
                }
            };
            if buffer.len().saturating_add(chunk.len()) > limits.sse_buffer_max_bytes {
                let _ = self
                    .conn
                    .send_error(&self.key, WsError::parser_limit())
                    .await;
                return TurnOutcome::default();
            }
            buffer.extend_from_slice(&chunk);
            let frames = match drain_sse_data(&mut buffer, false, limits.sse_frame_max_bytes) {
                Ok(frames) => frames,
                Err(error) => {
                    let _ = self.conn.send_error(&self.key, error).await;
                    return TurnOutcome::default();
                }
            };
            for data in frames {
                if let BridgeFlow::Disconnected = self.forward_event(data, &mut tracker).await {
                    return TurnOutcome {
                        disconnected: true,
                        ..TurnOutcome::default()
                    };
                }
                if !self.release_relays(&mut tracker, false).await {
                    return TurnOutcome {
                        disconnected: true,
                        ..TurnOutcome::default()
                    };
                }
                if interrupt_pending
                    && tracker.created.is_some()
                    && tracker.outcome.status.is_none()
                    && !tracker.failed
                {
                    return self.emulate_interrupt(body, tracker).await;
                }
            }
        }
        match drain_sse_data(&mut buffer, true, limits.sse_frame_max_bytes) {
            Ok(frames) => {
                for data in frames {
                    if let BridgeFlow::Disconnected = self.forward_event(data, &mut tracker).await {
                        return TurnOutcome {
                            disconnected: true,
                            ..TurnOutcome::default()
                        };
                    }
                }
            }
            Err(error) => {
                let _ = self.conn.send_error(&self.key, error).await;
                return TurnOutcome::default();
            }
        }
        if !self.release_relays(&mut tracker, true).await {
            return TurnOutcome {
                disconnected: true,
                ..TurnOutcome::default()
            };
        }
        if tracker.failed {
            tracker.outcome.status = None;
        }
        tracker.outcome.next_sequence = tracker.next_sequence;
        tracker.outcome
    }

    fn retainable(&self, output: &[Value]) -> bool {
        !self.oversized
            && self.conn.limits.fits(
                self.input_items.saturating_add(output.len()),
                self.input_bytes.saturating_add(serialized_len(output)),
            )
    }

    /// WS4b: orders a relay into the egress of the response it names. This
    /// response's relays wait for their lifecycle barrier; an earlier response's
    /// go out now, numbered from that response's own counter.
    async fn accept_relay(
        &self,
        relay: ResponsesWsRelay,
        tracker: &mut BridgeTracker,
        sequences: &mut HashMap<String, i64>,
    ) -> bool {
        match relay_response_id(&relay.value) {
            Some(id) if id == self.public_id => {
                if tracker.created.is_none() || (relay.post_terminal && !Self::ended(tracker)) {
                    tracker.held.push(relay);
                    true
                } else {
                    self.forward_relay(relay.value, tracker).await
                }
            }
            _ => forward_relay(&self.conn, &self.key, relay.value, sequences).await,
        }
    }

    fn ended(tracker: &BridgeTracker) -> bool {
        tracker.outcome.status.is_some() || tracker.failed
    }

    /// WS4b: sends held relays whose barrier passed. At the end of the stream the
    /// rest go out, unless the response never reached the client.
    async fn release_relays(&self, tracker: &mut BridgeTracker, finished: bool) -> bool {
        if tracker.held.is_empty() {
            return true;
        }
        if tracker.created.is_none() {
            if finished {
                tracker.held.clear();
            }
            return true;
        }
        let ended = finished || Self::ended(tracker);
        let (release, keep): (Vec<_>, Vec<_>) = std::mem::take(&mut tracker.held)
            .into_iter()
            .partition(|relay| ended || !relay.post_terminal);
        tracker.held = keep;
        for relay in release {
            if !self.forward_relay(relay.value, tracker).await {
                return false;
            }
        }
        true
    }

    /// Interleaves a relayed acknowledgement into this response's sequence.
    async fn forward_relay(&self, mut value: Value, tracker: &mut BridgeTracker) -> bool {
        let kind = value
            .get("type")
            .and_then(Value::as_str)
            .unwrap_or_default();
        if (kind.starts_with("response.interrupt.") || kind.starts_with("response.steer."))
            && let Some(object) = value.as_object_mut()
        {
            let sequence = tracker.next_sequence.unwrap_or(0);
            object.insert("sequence_number".to_string(), json!(sequence));
            tracker.next_sequence = Some(sequence + 1);
        }
        self.conn.send(&self.key, value).await
    }

    async fn forward_event(&mut self, data: String, tracker: &mut BridgeTracker) -> BridgeFlow {
        if data == "[DONE]" || tracker.failed {
            return BridgeFlow::Continue;
        }
        let Ok(mut event) = serde_json::from_str::<Value>(&data) else {
            return BridgeFlow::Continue;
        };
        let kind = event
            .get("type")
            .and_then(Value::as_str)
            .unwrap_or_default()
            .to_string();
        if let Some(sequence) = event.get("sequence_number").and_then(Value::as_i64) {
            // One sequence per public response: relayed acknowledgements are
            // interleaved, so pipeline events are renumbered after them.
            let next = tracker.next_sequence.unwrap_or(sequence);
            event["sequence_number"] = json!(next);
            tracker.next_sequence = Some(next + 1);
        }
        if kind == "error" {
            tracker.failed = true;
            let error = WsError::from_error_value(&event, None);
            return if self.conn.send_error(&self.key, error).await {
                BridgeFlow::Continue
            } else {
                BridgeFlow::Disconnected
            };
        }
        if tracker.outcome.status.is_some() {
            // Nothing of this response follows its terminal event.
            return BridgeFlow::Continue;
        }
        let has_response = event.get("response").is_some_and(Value::is_object);
        if !has_response {
            rewrite_response_ids(&mut event, &self.public_id);
        }
        match kind.as_str() {
            "response.created" => tracker.created = event.get("response").cloned(),
            "response.output_item.done" => {
                if let (Some(index), Some(item)) = (
                    event.get("output_index").and_then(Value::as_u64),
                    event.get("item"),
                ) {
                    tracker.done_items.insert(index, item.clone());
                }
            }
            "response.completed"
            | "response.incomplete"
            | "response.cancelled"
            | "response.failed" => {
                let response = event.get("response");
                let status = response
                    .and_then(|response| response.get("status"))
                    .and_then(Value::as_str)
                    .map(str::to_string)
                    .unwrap_or_else(|| kind.trim_start_matches("response.").to_string());
                let output = response
                    .and_then(|response| response.get("output"))
                    .and_then(Value::as_array)
                    .cloned()
                    .unwrap_or_default();
                if kind == "response.failed" {
                    tracker.failed = true;
                } else {
                    let retainable = self.retainable(&output);
                    // WS4: v1 `response.done` marks an appendable response.
                    if kind == "response.completed" && self.conn.is_v1() && retainable {
                        event["type"] = json!("response.done");
                        tracker.outcome.appendable = true;
                    }
                }
                tracker.outcome.status = Some(status);
                tracker.outcome.output = output;
            }
            _ => {}
        }
        if self.strip_encrypted {
            strip_encrypted_reasoning(&mut event);
        }
        if self.conn.send(&self.key, event).await {
            BridgeFlow::Continue
        } else {
            BridgeFlow::Disconnected
        }
    }

    /// WS12a emulation for HTTP and non-Responses upstreams (Codex contract).
    async fn emulate_interrupt(
        self,
        mut body: axum::body::BodyDataStream,
        tracker: BridgeTracker,
    ) -> TurnOutcome {
        let mut response = tracker.created.unwrap_or_else(|| json!({}));
        let output: Vec<Value> = tracker.done_items.into_values().collect();
        response["status"] = json!("completed");
        response["completed_at"] = json!(chrono::Utc::now().timestamp());
        response["usage"] = Value::Null;
        response["output"] = Value::Array(output.clone());
        let sequence = tracker.next_sequence.unwrap_or(0);
        let appendable = self.conn.is_v1() && self.retainable(&output);
        let accepted = json!({
            "type": "response.interrupt.accepted",
            "response_id": self.public_id,
            "sequence_number": sequence,
        });
        let mut completed = json!({
            "type": if appendable { "response.done" } else { "response.completed" },
            "sequence_number": sequence + 1,
            "response": response,
        });
        if self.strip_encrypted {
            strip_encrypted_reasoning(&mut completed);
        }
        let delivered =
            self.conn.send(&self.key, accepted).await && self.conn.send(&self.key, completed).await;
        // L2.1: the pipeline keeps settling usage and the request log.
        tokio::spawn(async move { while body.next().await.is_some() {} });
        TurnOutcome {
            status: Some("completed".to_string()),
            output,
            appendable,
            disconnected: !delivered,
            next_sequence: Some(sequence + 2),
        }
    }
}

/// WS19: every `response_id` member of a non-response event names the public id.
fn rewrite_response_ids(value: &mut Value, public_id: &str) {
    match value {
        Value::Object(object) => {
            for (key, child) in object.iter_mut() {
                if key == "response_id" && child.is_string() {
                    *child = Value::String(public_id.to_string());
                } else {
                    rewrite_response_ids(child, public_id);
                }
            }
        }
        Value::Array(items) => {
            for item in items {
                rewrite_response_ids(item, public_id);
            }
        }
        _ => {}
    }
}

fn strip_item_encrypted(item: &mut Value) {
    if item.get("type").and_then(Value::as_str) == Some("reasoning")
        && let Some(object) = item.as_object_mut()
    {
        object.remove("encrypted_content");
    }
}

fn strip_encrypted_reasoning(event: &mut Value) {
    if let Some(item) = event.get_mut("item") {
        strip_item_encrypted(item);
    }
    if let Some(output) = event
        .get_mut("response")
        .and_then(|response| response.get_mut("output"))
        .and_then(Value::as_array_mut)
    {
        output.iter_mut().for_each(strip_item_encrypted);
    }
}

/// WS12a: returns an error frame for an invalid mode; otherwise signals the target.
async fn handle_interrupt(conn: &Connection, event: &Map<String, Value>) -> Option<WsError> {
    if event
        .get("mode")
        .is_some_and(|mode| mode.as_str() != Some("discard_partial_items"))
    {
        return Some(WsError::new(
            400,
            "invalid_interrupt",
            "response.interrupt requires mode discard_partial_items",
            Some("mode"),
        ));
    }
    let response_id = event.get("response_id").and_then(Value::as_str);
    let lane: LaneKey = event
        .get("stream_id")
        .and_then(Value::as_str)
        .map(str::to_string);
    let not_in_progress = {
        let active = conn.active.lock().ok()?;
        if active.is_empty() {
            return None;
        }
        let target = match response_id {
            Some(id) => active.values().find(|turn| turn.public_id == id),
            None => active.get(&lane),
        };
        match target {
            Some(turn) => {
                if *turn.control.borrow() == ResponsesWsControl::Running {
                    turn.control.send_replace(ResponsesWsControl::Interrupt);
                }
                None
            }
            None => response_id.map(str::to_string),
        }
    };
    if let Some(response_id) = not_in_progress {
        let _ = conn
            .send(
                &None,
                json!({
                    "type": "response.interrupt.failed",
                    "response_id": response_id,
                    "error": {
                        "type": "invalid_request_error",
                        "code": "response_not_in_progress",
                        "message": "This response is no longer in progress.",
                        "param": "response_id",
                    },
                }),
            )
            .await;
    }
    None
}

fn valid_steer_input(input: &Value) -> bool {
    match input {
        Value::String(text) => !text.is_empty(),
        Value::Array(items) => {
            !items.is_empty()
                && items.iter().all(|item| {
                    item.get("role").and_then(Value::as_str) == Some("user")
                        && item
                            .get("type")
                            .and_then(Value::as_str)
                            .is_none_or(|kind| kind == "message")
                })
        }
        _ => false,
    }
}

/// WS12b: steering is forwarded only to an upstream WebSocket source.
async fn handle_steer(conn: &Connection, event: &Map<String, Value>) {
    let previous = event
        .get("previous_response_id")
        .and_then(Value::as_str)
        .map(str::to_string);
    let input = event.get("input").cloned().unwrap_or(Value::Null);
    let valid = previous.is_some()
        && valid_steer_input(&input)
        && event
            .keys()
            .all(|key| matches!(key.as_str(), "type" | "previous_response_id" | "input"));
    let target = previous.as_deref().and_then(|previous| {
        let active = conn.active.lock().ok()?;
        active
            .iter()
            .find(|(_, turn)| turn.public_id == previous)
            .map(|(lane, turn)| {
                let forwardable = turn.state.upstream_kind() == UPSTREAM_KIND_WEBSOCKET
                    && !turn.state.steer_rejected();
                (lane.clone(), forwardable, turn.steer.clone())
            })
    });
    let (lane, code) = match (valid, target) {
        (false, target) => (
            target.map(|(lane, ..)| lane).unwrap_or(None),
            "invalid_input",
        ),
        (true, Some((lane, true, steer))) => {
            if steer.send(input.clone()).is_ok() {
                return;
            }
            (lane, "response_not_active")
        }
        (true, Some((lane, ..))) => (lane, "steering_not_supported"),
        (true, None) => match previous.as_deref().and_then(|id| conn.entry_lane(id)) {
            Some(lane) => (lane, "response_not_active"),
            None => (None, "response_not_found"),
        },
    };
    let message = match code {
        "invalid_input" => "Use only the supported event fields and user message input.",
        "steering_not_supported" => {
            "The routed upstream does not support steering; resend the input with response.create."
        }
        "response_not_active" => "This response no longer accepts steering input.",
        _ => "The target response must still be available on the same WebSocket connection.",
    };
    let _ = conn
        .send(
            &lane,
            json!({
                "type": "response.steer.failed",
                "steer": {
                    "id": format!("steer_{}", uuid::Uuid::new_v4().simple()),
                    "previous_response_id": previous,
                    "input": input,
                },
                "error": {
                    "type": "invalid_request_error",
                    "code": code,
                    "message": message,
                },
            }),
        )
        .await;
}

fn drain_sse_data(
    buffer: &mut Vec<u8>,
    eof: bool,
    max_frame_bytes: usize,
) -> Result<Vec<String>, WsError> {
    let mut frames = Vec::new();
    loop {
        let boundary = buffer.windows(2).position(|window| window == b"\n\n");
        let Some(boundary) = boundary else {
            break;
        };
        if boundary.saturating_add(2) > max_frame_bytes {
            return Err(WsError::parser_limit());
        }
        let frame = buffer.drain(..boundary + 2).collect::<Vec<_>>();
        if let Some(data) = parse_sse_data_frame(&frame) {
            frames.push(data);
        }
    }
    if buffer.len() > max_frame_bytes {
        return Err(WsError::parser_limit());
    }
    if eof && !buffer.is_empty() {
        let frame = std::mem::take(buffer);
        if let Some(data) = parse_sse_data_frame(&frame) {
            frames.push(data);
        }
    }
    Ok(frames)
}

fn parse_sse_data_frame(frame: &[u8]) -> Option<String> {
    let frame = String::from_utf8_lossy(frame);
    let data = frame
        .lines()
        .filter_map(|line| line.strip_prefix("data:"))
        .map(|line| line.strip_prefix(' ').unwrap_or(line))
        .collect::<Vec<_>>();
    (!data.is_empty()).then(|| data.join("\n"))
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
