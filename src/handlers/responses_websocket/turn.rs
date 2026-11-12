use super::*;

/// How a turn ended downstream.
#[derive(Default)]
pub(super) struct TurnOutcome {
    pub(super) status: Option<String>,
    pub(super) output: Vec<Value>,
    pub(super) appendable: bool,
    pub(super) disconnected: bool,
    pub(super) next_sequence: Option<i64>,
}

pub(super) async fn run_turn(
    conn: &Arc<Connection>,
    key: &LaneKey,
    lane: &Arc<ResponsesWsLane>,
    prepared: PreparedTurn,
    relay: &mut mpsc::UnboundedReceiver<ResponsesWsRelay>,
    sequences: &mut HashMap<String, i64>,
) -> Option<FinishedTurn> {
    let RunningTurn {
        retention: running,
        control_tx,
        control_rx,
        pipeline,
    } = RunningTurn::register(conn, key, lane, &prepared)?;
    let mut outcome = running
        .execute(&prepared, pipeline, control_rx, relay, sequences)
        .await;
    if running.state.local_warmup() && outcome.status.is_none() && !outcome.disconnected {
        outcome = send_local_warmup(conn, key, &running.public_id, &prepared).await;
    }
    if let Some(sequence) = outcome.next_sequence {
        sequences.insert(running.public_id.clone(), sequence);
    }
    if let Ok(mut active) = conn.active.lock()
        && active
            .get(key)
            .is_some_and(|turn| turn.public_id == running.public_id)
    {
        active.remove(key);
    }
    drop(control_tx);
    let mut finished = running.retain(prepared, outcome);
    running
        .finish_followups(&mut finished, relay, sequences)
        .await;
    // WS17: only a successor reads the finished turn; the cache entry holds the rest.
    if !lane.has_successor() {
        finished = None;
    }
    finished
}

pub(super) struct RunningTurn<'a> {
    pub(super) retention: TurnRetention<'a>,
    pub(super) control_tx: Arc<watch::Sender<ResponsesWsControl>>,
    pub(super) control_rx: watch::Receiver<ResponsesWsControl>,
    pub(super) pipeline: ResponsesWsTurn,
}

pub(super) struct TurnRetention<'a> {
    pub(super) conn: &'a Arc<Connection>,
    pub(super) key: &'a LaneKey,
    pub(super) lane: &'a Arc<ResponsesWsLane>,
    pub(super) public_id: String,
    pub(super) state: Arc<ResponsesWsTurnState>,
    pub(super) input_bytes: usize,
}

impl<'a> RunningTurn<'a> {
    pub(super) fn register(
        conn: &'a Arc<Connection>,
        key: &'a LaneKey,
        lane: &'a Arc<ResponsesWsLane>,
        prepared: &PreparedTurn,
    ) -> Option<Self> {
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

        Some(Self {
            pipeline: turn,
            retention: TurnRetention {
                conn,
                key,
                lane,
                public_id,
                state,
                input_bytes: serialized_len(&prepared.full_input),
            },
            control_tx,
            control_rx,
        })
    }
}

impl TurnRetention<'_> {
    pub(super) async fn execute(
        &self,
        prepared: &PreparedTurn,
        turn: ResponsesWsTurn,
        control_rx: watch::Receiver<ResponsesWsControl>,
        relay: &mut mpsc::UnboundedReceiver<ResponsesWsRelay>,
        sequences: &mut HashMap<String, i64>,
    ) -> TurnOutcome {
        let conn = self.conn;
        let key = self.key;
        let public_id = &self.public_id;
        let state = &self.state;
        let input_bytes = self.input_bytes;

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
        match result {
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
        }
    }
}

impl TurnRetention<'_> {
    pub(super) fn retain(
        &self,
        prepared: PreparedTurn,
        outcome: TurnOutcome,
    ) -> Option<FinishedTurn> {
        let conn = self.conn;
        let key = self.key;
        let public_id = &self.public_id;
        let input_bytes = self.input_bytes;
        if outcome.disconnected {
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
        }
    }

    pub(super) async fn finish_followups(
        &self,
        finished: &mut Option<FinishedTurn>,
        relay: &mut mpsc::UnboundedReceiver<ResponsesWsRelay>,
        sequences: &mut HashMap<String, i64>,
    ) {
        let Self {
            conn,
            key,
            lane,
            public_id,
            state,
            input_bytes,
            ..
        } = self;
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
                    conn.set_pending_steers(key, public_id, &pending);
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
    }
}

/// WS6: local warmup answer when no upstream WebSocket can prepare it.
pub(super) async fn send_local_warmup(
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
