use super::*;

type FrameSender = mpsc::Sender<Result<(String, String), AppError>>;

/// WS12b shape of a steer that the routed upstream cannot apply.
fn steering_not_supported(public_response_id: &str, input: Value) -> Value {
    json!({
        "type": "response.steer.failed",
        "steer": {
            "id": format!("steer_{}", uuid::Uuid::new_v4().simple()),
            "previous_response_id": public_response_id,
            "input": input,
        },
        "error": {
            "type": "invalid_request_error",
            "code": "steering_not_supported",
            "message": "The routed upstream does not support steering; resend the input with response.create.",
        },
    })
}

enum FeedFlow {
    Continue,
    Finish,
    Replay,
}

#[derive(Default)]
struct FeederSteering {
    pending_steer_parts: usize,
    pending_steers: VecDeque<Value>,
    sent_steers: VecDeque<Value>,
    accepted_steers: Vec<(String, Value)>,
    successor: bool,
}

/// Reads one turn's upstream frames into the stream decoder (WS2d) and handles
/// interruption (WS2g), session reuse fallback (WS2h), and steering (WS2i).
struct TurnFeeder {
    steering: FeederSteering,
    source: ResponsesWsTurnSource,
    turn: ResponsesWsTurn,
    frames: FrameSender,
    idle: Duration,
    upstream_id: Option<String>,
    terminal_id: Option<String>,
    terminal_output: Vec<(String, Option<String>)>,
    terminal: bool,
    got_frame: bool,
    decided: bool,
    replayed: bool,
    buffered: Vec<(String, String)>,
    interrupt_requested: bool,
    interrupt_sent: bool,
    control_closed: bool,
    connection_seen: bool,
    decoder_gone: bool,
}

pub(crate) fn spawn_turn_feeder(
    source: ResponsesWsTurnSource,
    turn: ResponsesWsTurn,
    frames: FrameSender,
    idle_timeout_ms: u64,
) {
    let feeder = TurnFeeder {
        steering: FeederSteering::default(),
        source,
        turn,
        frames,
        idle: Duration::from_millis(idle_timeout_ms.max(1)),
        upstream_id: None,
        terminal_id: None,
        terminal_output: Vec::new(),
        terminal: false,
        got_frame: false,
        decided: false,
        replayed: false,
        buffered: Vec::new(),
        interrupt_requested: false,
        interrupt_sent: false,
        control_closed: false,
        connection_seen: false,
        decoder_gone: false,
    };
    feeder.turn.state.feeders.fetch_add(1, Ordering::AcqRel);
    tokio::spawn(feeder.run());
}

async fn recv_steer(rx: &mut Option<mpsc::UnboundedReceiver<Value>>) -> Option<Value> {
    match rx {
        Some(rx) => rx.recv().await,
        None => std::future::pending().await,
    }
}

impl TurnFeeder {
    async fn run(mut self) {
        let mut steer_rx = self.turn.steer.lock().ok().and_then(|mut rx| rx.take());
        if self.turn.is_disconnected() || *self.turn.control.borrow() != ResponsesWsControl::Running
        {
            self.interrupt_requested = true;
        }
        let keep = self.read_frames(&mut steer_rx).await;
        self.finish(keep && self.terminal).await;
        if let Ok(mut slot) = self.turn.steer.lock()
            && let Some(rx) = steer_rx
        {
            *slot = Some(rx);
        }
        let state = self.turn.state.clone();
        let lane = self.turn.lane.clone();
        // A stashed successor is read by its own turn, not by the idle watcher.
        if self.source.slot.session.is_some() && self.source.slot.stashed.is_empty() {
            lane.watching.store(true, Ordering::Release);
            tokio::spawn(watch_idle_session(self.source.slot, lane));
        }
        state.feeder_finished();
    }

    async fn read_frames(&mut self, steer_rx: &mut Option<mpsc::UnboundedReceiver<Value>>) -> bool {
        'feed: {
            for text in std::mem::take(&mut self.source.slot.stashed) {
                match self.feed_frame(text).await {
                    FeedFlow::Finish => break 'feed true,
                    FeedFlow::Continue | FeedFlow::Replay => {}
                }
            }
            loop {
                let Some(session) = self.source.session.as_mut() else {
                    break 'feed false;
                };
                let next = tokio::select! {
                    biased;
                    changed = self.turn.control.changed(), if !self.interrupt_requested && !self.control_closed => {
                        match changed {
                            // The handler drops the sender when the downstream turn
                            // ends; that is not an interruption.
                            Err(_) => self.control_closed = true,
                            Ok(()) => {
                                if *self.turn.control.borrow() != ResponsesWsControl::Running {
                                    self.interrupt_requested = true;
                                    self.send_interrupt_if_ready().await;
                                }
                            }
                        }
                        continue;
                    }
                    changed = self.turn.connection_closed.changed(), if !self.connection_seen => {
                        if changed.is_err() || *self.turn.connection_closed.borrow() {
                            self.connection_seen = true;
                            if self.terminal {
                                // Only steering follow-ups remain; nobody can receive them.
                                break 'feed false;
                            }
                            self.interrupt_requested = true;
                            self.send_interrupt_if_ready().await;
                        }
                        continue;
                    }
                    steer = recv_steer(steer_rx), if steer_rx.is_some() => {
                        match steer {
                            Some(steer) => {
                                self.steering.pending_steers.push_back(steer);
                                self.send_steers_if_ready().await;
                            }
                            None => *steer_rx = None,
                        }
                        continue;
                    }
                    next = timeout(self.idle, session.next_text()) => next,
                };
                match next {
                    Err(_) => {
                        if !self.terminal {
                            self.fail(
                                StatusCode::GATEWAY_TIMEOUT,
                                "upstream_idle_timeout",
                                format!(
                                    "upstream stream idle for {}ms without data",
                                    self.idle.as_millis()
                                ),
                            )
                            .await;
                        }
                        break 'feed false;
                    }
                    Ok(Ok(Some(text))) => match self.feed_frame(text).await {
                        FeedFlow::Continue => {}
                        FeedFlow::Finish => break 'feed true,
                        FeedFlow::Replay => {
                            if !self.replay().await {
                                break 'feed false;
                            }
                        }
                    },
                    Ok(Ok(None)) | Ok(Err(_)) => {
                        if !self.got_frame && self.source.reused && !self.replayed {
                            if self.replay().await {
                                continue;
                            }
                            break 'feed false;
                        }
                        if !self.terminal {
                            self.fail(
                                StatusCode::BAD_GATEWAY,
                                "upstream_stream_decode_failed",
                                "upstream WebSocket closed before a terminal event".to_string(),
                            )
                            .await;
                        }
                        break 'feed false;
                    }
                }
            }
        }
    }

    async fn fail(&mut self, status: StatusCode, code: &str, message: String) {
        let _ = self
            .frames
            .send(Err(AppError::new(status, code.to_string(), message)))
            .await;
    }

    fn public_id_for(&self, upstream_id: &str) -> Option<String> {
        self.source.slot.public_ids.get(upstream_id).cloned()
    }

    fn relay(&self, value: Value) {
        let _ = self.turn.lane.relay.send(ResponsesWsRelay {
            value,
            post_terminal: self.terminal,
        });
    }

    async fn feed_frame(&mut self, text: String) -> FeedFlow {
        self.got_frame = true;
        let Ok(mut value) = serde_json::from_str::<Value>(&text) else {
            if !self.terminal {
                self.forward(String::new(), text).await;
            }
            return FeedFlow::Continue;
        };
        let kind = value
            .get("type")
            .and_then(Value::as_str)
            .unwrap_or_default()
            .to_string();
        capture_turn_state(&mut self.source.slot, &value);
        if !kind.starts_with("response.")
            && kind != "error"
            && let Some(id) = find_response_id(&value)
            && self.source.slot.public_ids.contains_key(id)
            && self.upstream_id.as_deref() != Some(id)
        {
            // A late notification of an earlier response on this session keeps
            // its own public id and never enters this turn's decoder.
            map_known_response_ids(&mut value, &self.source.slot.public_ids);
            self.relay(value);
            return FeedFlow::Continue;
        }
        match kind.as_str() {
            "response.interrupt.accepted" | "response.interrupt.failed" => {
                self.interrupt_acknowledgement(&kind, value).await;
                return FeedFlow::Continue;
            }
            "response.steer.accepted" | "response.steer.pending" | "response.steer.failed" => {
                return self.steer_acknowledgement(&kind, value);
            }
            _ => {}
        }
        if self.reject_unsupported_steer(&kind, &value) {
            return self.after_frame();
        }
        if self.terminal {
            return self.post_terminal_frame(&kind, value, text);
        }
        if !self.decided && self.source.reused && !self.replayed {
            let incremental_miss = kind == "error"
                && self.source.incremental
                && value.pointer("/error/code").and_then(Value::as_str)
                    == Some("previous_response_not_found");
            if incremental_miss {
                return FeedFlow::Replay;
            }
            if !kind.starts_with("response.") && kind != "error" {
                self.buffered.push((kind, text));
                return FeedFlow::Continue;
            }
            self.decided = true;
            for (kind, text) in std::mem::take(&mut self.buffered) {
                self.forward(kind, text).await;
            }
        }
        if kind == "response.created"
            && let Some(id) = value.pointer("/response/id").and_then(Value::as_str)
        {
            self.upstream_id = Some(id.to_string());
            self.source
                .slot
                .public_ids
                .insert(id.to_string(), self.turn.public_response_id.clone());
            self.send_interrupt_if_ready().await;
            self.send_steers_if_ready().await;
        }
        self.capture_terminal(&kind, &value);
        drop(value);
        self.forward(kind, text).await;
        self.after_frame()
    }

    async fn interrupt_acknowledgement(&mut self, kind: &str, mut value: Value) {
        if kind == "response.interrupt.failed"
            && value.get("response_id").and_then(Value::as_str) == self.upstream_id.as_deref()
        {
            self.signal_discard_requested(false).await;
        }
        if !self.turn.is_disconnected() {
            if let Some(public) = value
                .get("response_id")
                .and_then(Value::as_str)
                .and_then(|id| self.public_id_for(id))
            {
                value["response_id"] = Value::String(public);
            }
            self.relay(value);
        }
    }

    fn steer_acknowledgement(&mut self, kind: &str, mut value: Value) -> FeedFlow {
        let steer_id = value
            .pointer("/steer/id")
            .and_then(Value::as_str)
            .map(str::to_string);
        if kind == "response.steer.accepted" {
            let input = self.steering.sent_steers.pop_front().unwrap_or(Value::Null);
            if let Some(steer_id) = steer_id {
                self.steering.accepted_steers.push((steer_id, input));
            }
        } else {
            let was_accepted = steer_id.as_ref().is_some_and(|steer_id| {
                self.steering
                    .accepted_steers
                    .iter()
                    .any(|(id, _)| id == steer_id)
            });
            if was_accepted {
                if kind == "response.steer.pending"
                    && let Some(position) = self
                        .steering
                        .accepted_steers
                        .iter()
                        .position(|(id, _)| Some(id) == steer_id.as_ref())
                {
                    // The upstream prepends this input to the next create;
                    // full replays must contain it too (WS7, WS2h item 2).
                    let (_, input) = self.steering.accepted_steers.remove(position);
                    self.steering.pending_steer_parts += steer_content_parts(&input);
                    if let Ok(mut pending) = self.turn.state.pending_steers.lock() {
                        pending.push(input);
                    }
                }
                self.steering
                    .accepted_steers
                    .retain(|(id, _)| Some(id) != steer_id.as_ref());
            } else {
                // A rejection without prior acceptance answers the oldest
                // unacknowledged sent steer.
                self.steering.sent_steers.pop_front();
            }
        }
        if let Some(public) = value
            .pointer("/steer/previous_response_id")
            .and_then(Value::as_str)
            .and_then(|id| self.public_id_for(id))
        {
            value["steer"]["previous_response_id"] = Value::String(public);
        }
        self.relay(value);
        if self.terminal && self.steers_resolved() {
            FeedFlow::Finish
        } else {
            FeedFlow::Continue
        }
    }

    fn reject_unsupported_steer(&mut self, kind: &str, value: &Value) -> bool {
        if kind == "error"
            && value.get("response").is_none()
            && !self.steering.sent_steers.is_empty()
            && matches!(
                value.pointer("/error/code").and_then(Value::as_str),
                Some(
                    "invalid_request_type" | "unsupported_request_type" | "unsupported_event_type"
                )
            )
        {
            // WS2i: an upstream without steering rejects the event type while its
            // response keeps streaming; this error belongs to the steer.
            let input = self.steering.sent_steers.pop_front().unwrap_or(Value::Null);
            self.turn
                .state
                .steer_rejected
                .store(true, Ordering::Release);
            if let Ok(mut channels) = STEERING_REJECTING_CHANNELS.lock() {
                channels.insert(self.source.target.channel_id.clone());
            }
            self.relay(steering_not_supported(&self.turn.public_response_id, input));
            return true;
        }
        false
    }

    fn post_terminal_frame(&mut self, kind: &str, mut value: Value, text: String) -> FeedFlow {
        if kind == "response.created" && !self.steering.accepted_steers.is_empty() {
            self.source.slot.stashed.push(text);
            self.steering.successor = true;
            return FeedFlow::Finish;
        }
        if !kind.starts_with("response.") && kind != "error" {
            map_known_response_ids(&mut value, &self.source.slot.public_ids);
            self.relay(value);
        }
        FeedFlow::Continue
    }

    fn capture_terminal(&mut self, kind: &str, value: &Value) {
        let terminal = matches!(
            kind,
            "response.completed"
                | "response.failed"
                | "response.incomplete"
                | "response.cancelled"
                | "error"
        );
        if terminal {
            self.terminal = true;
            self.terminal_id = value
                .pointer("/response/id")
                .and_then(Value::as_str)
                .map(str::to_string)
                .or_else(|| self.upstream_id.clone());
            self.terminal_output = value
                .pointer("/response/output")
                .and_then(Value::as_array)
                .map(|items| {
                    items
                        .iter()
                        .map(|item| {
                            (
                                item.get("type")
                                    .and_then(Value::as_str)
                                    .unwrap_or("message")
                                    .to_string(),
                                item.get("id").and_then(Value::as_str).map(str::to_string),
                            )
                        })
                        .collect()
                })
                .unwrap_or_default();
        }
    }

    fn after_frame(&self) -> FeedFlow {
        if self.terminal && self.steers_resolved() {
            FeedFlow::Finish
        } else {
            FeedFlow::Continue
        }
    }

    fn steers_resolved(&self) -> bool {
        self.steering.accepted_steers.is_empty() && self.steering.sent_steers.is_empty()
    }

    async fn forward(&mut self, kind: String, text: String) {
        if self.decoder_gone {
            return;
        }
        if self.frames.send(Ok((kind, text))).await.is_err() {
            // The pipeline stopped reading; stop upstream work but keep reading
            // to the terminal so the session stays reusable.
            self.decoder_gone = true;
            self.interrupt_requested = true;
            self.send_interrupt_if_ready().await;
        }
    }

    async fn send_interrupt_if_ready(&mut self) {
        if !self.interrupt_requested || self.interrupt_sent || self.terminal {
            return;
        }
        let (Some(upstream_id), Some(session)) =
            (self.upstream_id.clone(), self.source.session.as_mut())
        else {
            return;
        };
        self.interrupt_sent = true;
        let frame = json!({
            "type": "response.interrupt",
            "response_id": upstream_id,
            "mode": "discard_partial_items",
        });
        match timeout(self.idle, session.send_text(frame.to_string())).await {
            Ok(Ok(())) => self.signal_discard_requested(true).await,
            Ok(Err(error)) => {
                tracing::warn!(%error, "upstream WebSocket interrupt send failed; draining usage")
            }
            Err(_) => tracing::warn!("upstream WebSocket interrupt send timed out; draining usage"),
        }
    }

    /// WS2g: while true, the decoder treats the terminal `output` as authoritative
    /// (partial items discarded upstream). Sent on interrupt send, not on
    /// acknowledgement, because some upstreams never acknowledge.
    async fn signal_discard_requested(&mut self, requested: bool) {
        if self.decoder_gone {
            return;
        }
        let _ = self
            .frames
            .send(Ok((
                crate::urp::RESPONSES_DISCARD_REQUESTED_FRAME.to_string(),
                requested.to_string(),
            )))
            .await;
    }

    async fn send_steers_if_ready(&mut self) {
        let (Some(upstream_id), Some(session)) =
            (self.upstream_id.clone(), self.source.session.as_mut())
        else {
            return;
        };
        while let Some(input) = self.steering.pending_steers.pop_front() {
            let frame = json!({
                "type": "response.steer",
                "previous_response_id": upstream_id,
                "input": input,
            });
            match timeout(self.idle, session.send_text(frame.to_string())).await {
                Ok(Ok(())) => self.steering.sent_steers.push_back(input),
                _ => {
                    let _ = self.turn.lane.relay.send(ResponsesWsRelay {
                        value: json!({
                            "type": "response.steer.failed",
                            "steer": {
                                "id": format!("steer_{}", uuid::Uuid::new_v4().simple()),
                                "previous_response_id": self.turn.public_response_id,
                                "input": input,
                            },
                            "error": {
                                "type": "invalid_request_error",
                                "code": "response_not_active",
                                "message": "This response no longer accepts steering input.",
                            },
                        }),
                        post_terminal: false,
                    });
                }
            }
        }
    }

    /// WS2h item 3: replace the reused session and resend the full create once.
    async fn replay(&mut self) -> bool {
        self.replayed = true;
        if let Some(mut session) = self.source.session.take() {
            session.close().await;
        }
        self.source.slot.last_upstream_id = None;
        let turn_state = self.source.slot.turn_state.clone();
        let mut session =
            match connect_with_turn_state(&self.source.target, turn_state.as_deref()).await {
                Ok(session) => session,
                Err(error) => {
                    self.fail(
                        StatusCode::BAD_GATEWAY,
                        "upstream_websocket_handshake_failed",
                        error.message,
                    )
                    .await;
                    return false;
                }
            };
        if let Some(turn_state) = session.handshake_turn_state() {
            self.source.slot.turn_state = Some(turn_state);
        }
        let text = std::mem::take(&mut self.source.full_text);
        if let Err(error) = session.send_text(text).await {
            self.fail(
                StatusCode::BAD_GATEWAY,
                "upstream_websocket_send_failed",
                error,
            )
            .await;
            return false;
        }
        self.source.session = Some(session);
        self.source.incremental = false;
        self.buffered.clear();
        self.decided = false;
        self.got_frame = false;
        self.upstream_id = None;
        true
    }

    async fn finish(&mut self, keep: bool) {
        let slot = &mut self.source.slot;
        if keep && !self.turn.is_disconnected() {
            slot.session = self.source.session.take();
            slot.channel_id = self.source.target.channel_id.clone();
            slot.upstream_model = self.source.target.upstream_model.clone();
            slot.last_upstream_id = self.terminal_id.clone();
            slot.last_public_id = Some(self.turn.public_response_id.clone());
            slot.basis = std::mem::take(&mut self.source.full_input);
            slot.last_output = std::mem::take(&mut self.terminal_output);
            slot.pending_steer_parts = self.steering.pending_steer_parts;
        } else {
            if let Some(mut session) = self.source.session.take() {
                session.close().await;
            }
            slot.forget_session_state();
            self.steering.successor = false;
        }
        if self.steering.successor
            && let Ok(mut successor) = self.turn.lane.successor.lock()
        {
            *successor = Some(ResponsesWsSuccessor {
                steer_inputs: std::mem::take(&mut self.steering.accepted_steers)
                    .into_iter()
                    .map(|(_, input)| input)
                    .collect(),
                channel_id: self.source.target.channel_id.clone(),
                upstream_model: self.source.target.upstream_model.clone(),
            });
        }
    }
}
