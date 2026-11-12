use super::*;

struct LaneHandle {
    jobs: mpsc::UnboundedSender<LaneJob>,
}

pub(super) struct LaneJob {
    pub(super) append: bool,
    pub(super) event: Map<String, Value>,
}

pub(super) async fn serve_responses_websocket(
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

    let mut reader = ConnectionReader {
        conn: conn.clone(),
        v2_header,
        lanes: HashMap::new(),
        named_lanes: 0,
        inbound_bytes: 0,
        first_text: true,
    };
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
        if !reader.receive_message(message).await {
            break;
        }
    }

    conn.mark_closed();
    if let Ok(active) = conn.active.lock() {
        for turn in active.values() {
            turn.control.send_replace(ResponsesWsControl::Disconnected);
        }
    }
    drop(reader);
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

/// Reader-side state of one connection. Each method returns false when the
/// connection must close.
struct ConnectionReader {
    conn: Arc<Connection>,
    v2_header: bool,
    lanes: HashMap<LaneKey, LaneHandle>,
    named_lanes: usize,
    inbound_bytes: usize,
    first_text: bool,
}

impl ConnectionReader {
    async fn receive_message(&mut self, message: Message) -> bool {
        match message {
            Message::Text(text) => self.receive_text(text.as_str()).await,
            Message::Binary(_) => {
                self.conn
                    .send_error(
                        &None,
                        WsError::invalid("binary WebSocket messages are not supported"),
                    )
                    .await
            }
            Message::Ping(payload) => self
                .conn
                .outbound
                .send(Message::Pong(payload))
                .await
                .is_ok(),
            Message::Pong(_) => true,
            Message::Close(_) => false,
        }
    }

    async fn receive_text(&mut self, text: &str) -> bool {
        self.inbound_bytes = self.inbound_bytes.saturating_add(text.len());
        if self.inbound_bytes > self.conn.limits.connection_max_bytes {
            let _ = self
                .conn
                .send_error(&None, WsError::connection_limit("inbound byte quota"))
                .await;
            return false;
        }
        let event = match serde_json::from_str::<Value>(text) {
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
        if self.first_text {
            self.first_text = false;
            let valid_first = match kind.as_deref() {
                Some("response.create") => true,
                Some("response.append") => !self.v2_header,
                _ => false,
            };
            if !valid_first {
                // WS12: the official first-event contract closes the socket.
                let _ = self
                    .conn
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
                return false;
            }
        }
        match event {
            Ok(event) => self.dispatch_event(kind.as_deref(), event).await,
            Err(error) => self.conn.send_error(&None, error).await,
        }
    }

    async fn dispatch_event(&mut self, kind: Option<&str>, event: Map<String, Value>) -> bool {
        match kind {
            Some(kind @ ("response.create" | "response.append")) => {
                self.enqueue_turn(kind == "response.append", event).await
            }
            Some("response.interrupt") => match handle_interrupt(&self.conn, &event).await {
                Some(error) => self.conn.send_error(&None, error).await,
                None => true,
            },
            Some("response.steer") => {
                handle_steer(&self.conn, &event).await;
                true
            }
            Some(other) => {
                self.conn
                    .send_error(
                        &None,
                        WsError::invalid(format!("unsupported WebSocket event type '{other}'")),
                    )
                    .await
            }
            None => {
                self.conn
                    .send_error(&None, WsError::invalid("WebSocket event is missing type"))
                    .await
            }
        }
    }

    async fn enqueue_turn(&mut self, append: bool, event: Map<String, Value>) -> bool {
        if append && !self.v2_header {
            self.conn.v1.store(true, Ordering::Release);
        }
        let lane: LaneKey = match event.get("stream_id") {
            None => None,
            Some(Value::String(stream_id)) if valid_stream_id(stream_id) => Some(stream_id.clone()),
            Some(_) => {
                let error = WsError::new(
                    400,
                    "invalid_stream_id",
                    "The 'stream_id' field must be a non-empty string with at most 256 characters and may only contain letters, numbers, underscores, hyphens, and periods.",
                    Some("stream_id"),
                );
                return self.conn.send_error(&None, error).await;
            }
        };
        if lane.is_some() && !self.lanes.contains_key(&lane) && self.named_lanes >= MAX_NAMED_LANES
        {
            let error = WsError::new(
                400,
                "websocket_stream_limit_reached",
                "This WebSocket connection has reached its maximum number of distinct stream IDs (32). Reuse an existing stream_id or open a new WebSocket connection.",
                Some("stream_id"),
            );
            return self.conn.send_error(&lane, error).await;
        }
        let handle = self.lanes.entry(lane.clone()).or_insert_with(|| {
            if lane.is_some() {
                self.named_lanes += 1;
            }
            if let Ok(mut order) = self.conn.lane_order.lock() {
                let next = order.len();
                order.entry(lane.clone()).or_insert(next);
            }
            spawn_lane(self.conn.clone(), lane.clone())
        });
        let _ = handle.jobs.send(LaneJob { append, event });
        true
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
