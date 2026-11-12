use super::*;

pub(super) const TURN_STATE_HEADER: &str = "x-codex-turn-state";
pub(crate) const UPSTREAM_KIND_WEBSOCKET: u8 = 1;
pub(crate) const UPSTREAM_KIND_HTTP: u8 = 2;

/// WS2h: one lane's pinned upstream session and its continuation basis.
#[derive(Default)]
pub(crate) struct ResponsesWsSlot {
    pub(super) session: Option<Box<dyn ResponsesWsSession>>,
    pub(super) channel_id: String,
    pub(super) upstream_model: String,
    pub(super) last_upstream_id: Option<String>,
    pub(super) last_public_id: Option<String>,
    pub(super) basis: Vec<Value>,
    /// `(type, id)` of each item in the last upstream terminal `output`; the
    /// incremental create must find these ids right after `basis` (WS2h item 2).
    pub(super) last_output: Vec<(String, Option<String>)>,
    /// Number of input items of steers the upstream left pending after `last_output`;
    /// the upstream prepends them itself on an incremental create.
    pub(super) pending_steer_parts: usize,
    pub(super) turn_state: Option<String>,
    pub(super) turn_state_channel: String,
    pub(super) stashed: Vec<String>,
    /// Upstream response id -> public response id for every response read on
    /// this lane, so relayed acknowledgements can be rewritten (WS19).
    pub(super) public_ids: HashMap<String, String>,
}

impl ResponsesWsSlot {
    pub(super) fn forget_session_state(&mut self) {
        self.session = None;
        self.last_upstream_id = None;
        self.last_public_id = None;
        self.basis.clear();
        self.last_output.clear();
        self.pending_steer_parts = 0;
        self.stashed.clear();
    }
}

/// WS4b: one event the handler must order into a response's egress stream.
pub(crate) struct ResponsesWsRelay {
    pub(crate) value: Value,
    /// Read after the upstream terminal of its response, so it must not reach
    /// the client before that terminal does.
    pub(crate) post_terminal: bool,
}

/// WS2i: a stashed automatic successor waiting for its WS18 turn.
pub(crate) struct ResponsesWsSuccessor {
    pub(crate) steer_inputs: Vec<Value>,
    pub(crate) channel_id: String,
    pub(crate) upstream_model: String,
}

pub(crate) struct ResponsesWsLane {
    pub(super) slot: Arc<AsyncMutex<ResponsesWsSlot>>,
    pub(super) relay: mpsc::UnboundedSender<ResponsesWsRelay>,
    pub(super) successor: std::sync::Mutex<Option<ResponsesWsSuccessor>>,
    /// WS2h item 6: an idle watcher holds the slot between turns until woken.
    pub(super) watching: AtomicBool,
    pub(super) wake: tokio::sync::Notify,
}

impl ResponsesWsLane {
    /// `relay` receives acknowledgement events that bypass the stream decoder.
    pub(crate) fn new(relay: mpsc::UnboundedSender<ResponsesWsRelay>) -> Self {
        Self {
            slot: Arc::new(AsyncMutex::new(ResponsesWsSlot::default())),
            relay,
            successor: std::sync::Mutex::new(None),
            watching: AtomicBool::new(false),
            wake: tokio::sync::Notify::new(),
        }
    }

    pub(crate) fn take_successor(&self) -> Option<ResponsesWsSuccessor> {
        self.successor.lock().ok()?.take()
    }

    pub(crate) fn has_successor(&self) -> bool {
        self.successor
            .lock()
            .is_ok_and(|successor| successor.is_some())
    }

    pub(super) async fn lock_slot(&self) -> OwnedMutexGuard<ResponsesWsSlot> {
        if self.watching.load(Ordering::Acquire) {
            self.wake.notify_one();
        }
        self.slot.clone().lock_owned().await
    }

    /// WS17: releases the encoded history of an evicted entry. Late notifications
    /// stay attributable, and a queued pending steer still forces a fresh session.
    pub(crate) async fn forget_continuation(self: &Arc<Self>) {
        let mut slot = self.lock_slot().await;
        slot.last_upstream_id = None;
        slot.last_public_id = None;
        slot.basis = Vec::new();
        slot.last_output = Vec::new();
        if slot.session.is_some() && slot.stashed.is_empty() {
            self.watching.store(true, Ordering::Release);
            tokio::spawn(watch_idle_session(slot, self.clone()));
        }
    }

    pub(crate) async fn close(&self) {
        let mut slot = self.lock_slot().await;
        if let Some(mut session) = slot.session.take() {
            session.close().await;
        }
        slot.forget_session_state();
    }
}

/// WS2h item 6: reads a pinned session between turns so late notifications
/// reach the client attributed to their own response.
pub(super) async fn watch_idle_session(
    mut slot: OwnedMutexGuard<ResponsesWsSlot>,
    lane: Arc<ResponsesWsLane>,
) {
    while let Some(session) = slot.session.as_mut() {
        tokio::select! {
            biased;
            _ = lane.wake.notified() => break,
            next = session.next_text() => {
                let keep = match next {
                    Ok(Some(text)) => relay_idle_frame(&mut slot, &lane, &text),
                    Ok(None) | Err(_) => false,
                };
                if !keep {
                    if let Some(mut session) = slot.session.take() {
                        session.close().await;
                    }
                    slot.forget_session_state();
                    break;
                }
            }
        }
    }
    lane.watching.store(false, Ordering::Release);
}

/// Returns false when the frame shows the session is no longer idle-consistent.
pub(super) fn relay_idle_frame(
    slot: &mut ResponsesWsSlot,
    lane: &ResponsesWsLane,
    text: &str,
) -> bool {
    let Ok(mut value) = serde_json::from_str::<Value>(text) else {
        return false;
    };
    let kind = value
        .get("type")
        .and_then(Value::as_str)
        .unwrap_or_default()
        .to_string();
    let ack = kind.starts_with("response.interrupt.") || kind.starts_with("response.steer.");
    if !ack && (kind.starts_with("response.") || kind == "error") {
        return false;
    }
    capture_turn_state(slot, &value);
    map_known_response_ids(&mut value, &slot.public_ids);
    let _ = lane.relay.send(ResponsesWsRelay {
        value,
        post_terminal: true,
    });
    true
}

pub(super) fn capture_turn_state(slot: &mut ResponsesWsSlot, value: &Value) {
    if value.get("type").and_then(Value::as_str) == Some("codex.response.metadata")
        && let Some(turn_state) = value
            .get("headers")
            .and_then(|headers| headers.get(TURN_STATE_HEADER))
            .and_then(Value::as_str)
    {
        slot.turn_state = Some(turn_state.to_string());
    }
}

/// WS19: rewrites `response_id` and `previous_response_id` members naming a
/// known upstream response to that response's public id.
pub(super) fn map_known_response_ids(value: &mut Value, public_ids: &HashMap<String, String>) {
    match value {
        Value::Object(object) => {
            for (key, child) in object.iter_mut() {
                if matches!(key.as_str(), "response_id" | "previous_response_id")
                    && let Some(public) = child.as_str().and_then(|id| public_ids.get(id))
                {
                    *child = Value::String(public.clone());
                } else {
                    map_known_response_ids(child, public_ids);
                }
            }
        }
        Value::Array(items) => items
            .iter_mut()
            .for_each(|item| map_known_response_ids(item, public_ids)),
        _ => {}
    }
}

/// The first `response_id` member at any depth (e.g. `timing_metrics.response_id`).
pub(crate) fn find_response_id(value: &Value) -> Option<&str> {
    match value {
        Value::Object(object) => object
            .get("response_id")
            .and_then(Value::as_str)
            .or_else(|| object.values().find_map(find_response_id)),
        Value::Array(items) => items.iter().find_map(find_response_id),
        _ => None,
    }
}

#[derive(Default)]
pub(crate) struct ResponsesWsTurnState {
    pub(super) upstream_kind: AtomicU8,
    pub(super) local_warmup: AtomicBool,
    pub(super) steer_rejected: AtomicBool,
    pub(super) feeders: std::sync::atomic::AtomicUsize,
    pub(super) feeder_done: tokio::sync::Notify,
    /// Inputs of accepted steers answered with `response.steer.pending`.
    pub(super) pending_steers: std::sync::Mutex<Vec<Value>>,
}

impl ResponsesWsTurnState {
    pub(crate) fn take_pending_steers(&self) -> Vec<Value> {
        self.pending_steers
            .lock()
            .map(|mut pending| std::mem::take(&mut *pending))
            .unwrap_or_default()
    }
}

/// WS2i: Channels whose upstream WebSocket rejected `response.steer`.
pub(super) static STEERING_REJECTING_CHANNELS: std::sync::LazyLock<
    std::sync::Mutex<HashSet<String>>,
> = std::sync::LazyLock::new(Default::default);

pub(super) fn channel_rejects_steering(channel_id: &str) -> bool {
    STEERING_REJECTING_CHANNELS
        .lock()
        .is_ok_and(|channels| channels.contains(channel_id))
}

impl ResponsesWsTurnState {
    /// WS2i: waits until every upstream feeder of this turn finished reading,
    /// including steering follow-ups after the terminal.
    pub(crate) async fn wait_feeders(&self) {
        loop {
            let notified = self.feeder_done.notified();
            tokio::pin!(notified);
            notified.as_mut().enable();
            if self.feeders.load(Ordering::Acquire) == 0 {
                return;
            }
            notified.await;
        }
    }

    pub(super) fn feeder_finished(&self) {
        self.feeders.fetch_sub(1, Ordering::AcqRel);
        self.feeder_done.notify_waiters();
    }
    /// WS12b: steering must fail locally for this turn's upstream.
    pub(crate) fn steer_rejected(&self) -> bool {
        self.steer_rejected.load(Ordering::Acquire)
    }

    pub(crate) fn upstream_kind(&self) -> u8 {
        self.upstream_kind.load(Ordering::Acquire)
    }

    pub(crate) fn set_upstream_kind(&self, kind: u8) {
        self.upstream_kind.store(kind, Ordering::Release);
    }

    pub(crate) fn local_warmup(&self) -> bool {
        self.local_warmup.load(Ordering::Acquire)
    }

    pub(crate) fn set_local_warmup(&self) {
        self.local_warmup.store(true, Ordering::Release);
    }
}

/// WS2a: per-turn context the WebSocket handler passes to the Responses pipeline.
#[derive(Clone)]
pub(crate) struct ResponsesWsTurn {
    pub(crate) control: watch::Receiver<ResponsesWsControl>,
    pub(crate) steer: Arc<std::sync::Mutex<Option<mpsc::UnboundedReceiver<Value>>>>,
    pub(crate) lane: Arc<ResponsesWsLane>,
    pub(crate) state: Arc<ResponsesWsTurnState>,
    pub(crate) public_response_id: String,
    pub(crate) previous_response_id: Option<String>,
    pub(crate) warmup: bool,
    /// WS18: Channel id and upstream model of the session the successor reads.
    pub(crate) successor: Option<(String, String)>,
    pub(crate) client_metadata: Option<Value>,
    /// WS5a: Monoize appended `reasoning.encrypted_content` to `include`.
    pub(crate) encrypted_include_added: bool,
    /// True once the downstream connection closed; outlives the turn's own
    /// control so post-terminal reading still observes disconnects (WS2g).
    pub(crate) connection_closed: watch::Receiver<bool>,
}

impl ResponsesWsTurn {
    pub(crate) fn is_disconnected(&self) -> bool {
        *self.control.borrow() == ResponsesWsControl::Disconnected
            || *self.connection_closed.borrow()
    }
}
