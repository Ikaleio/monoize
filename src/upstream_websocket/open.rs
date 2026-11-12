use super::*;

pub(crate) struct UpstreamWsTarget {
    pub(crate) base_url: String,
    pub(crate) headers: Vec<(String, String)>,
    pub(crate) proxy: Option<String>,
    pub(crate) timeout_ms: u64,
    pub(crate) channel_id: String,
    pub(crate) upstream_model: String,
}

pub(crate) enum TurnOpenError {
    /// WS2c: no `101`; the untouched encoded body is returned for HTTP fallback.
    Handshake {
        error: HandshakeError,
        body: Value,
    },
    Send(String),
}

pub(crate) struct ResponsesWsTurnSource {
    pub(super) slot: OwnedMutexGuard<ResponsesWsSlot>,
    pub(super) session: Option<Box<dyn ResponsesWsSession>>,
    pub(super) target: UpstreamWsTarget,
    /// Full create text kept only while WS2h item 3 may replay it.
    pub(super) full_text: String,
    pub(super) full_input: Vec<Value>,
    pub(super) incremental: bool,
    pub(super) reused: bool,
}

pub(super) async fn connect_with_turn_state(
    target: &UpstreamWsTarget,
    turn_state: Option<&str>,
) -> Result<Box<dyn ResponsesWsSession>, HandshakeError> {
    let mut headers = target.headers.clone();
    if let Some(turn_state) = turn_state {
        headers.retain(|(name, _)| !name.eq_ignore_ascii_case(TURN_STATE_HEADER));
        headers.push((TURN_STATE_HEADER.to_string(), turn_state.to_string()));
    }
    connect_responses_websocket(
        &target.base_url,
        &headers,
        target.proxy.as_deref(),
        target.timeout_ms,
    )
    .await
}

/// WS2h items 1–2 and WS18: obtains the lane session for this attempt and sends
/// the create frame (none for a successor).
pub(crate) async fn open_turn_source(
    turn: &ResponsesWsTurn,
    target: UpstreamWsTarget,
    body: Value,
) -> Result<ResponsesWsTurnSource, TurnOpenError> {
    let mut slot = turn.lane.lock_slot().await;
    if channel_rejects_steering(&target.channel_id) {
        turn.state.steer_rejected.store(true, Ordering::Release);
    }
    let same_route = slot.session.is_some()
        && slot.channel_id == target.channel_id
        && slot.upstream_model == target.upstream_model;
    if turn.successor.is_some() {
        if !same_route || slot.stashed.is_empty() {
            return Err(TurnOpenError::Send(
                "the steering successor session is unavailable".to_string(),
            ));
        }
        let session = slot.session.take();
        let full_input = body
            .get("input")
            .and_then(Value::as_array)
            .cloned()
            .unwrap_or_default();
        return Ok(ResponsesWsTurnSource {
            slot,
            session,
            target,
            full_text: String::new(),
            full_input,
            incremental: false,
            reused: false,
        });
    }
    if slot.turn_state_channel != target.channel_id {
        slot.turn_state = None;
        slot.turn_state_channel = target.channel_id.clone();
    }
    // A stashed successor belongs to its WS18 turn only; an ordinary turn must
    // never read its frames, so that session is replaced.
    let same_route = same_route && slot.stashed.is_empty();
    let (session, reused) = match slot.session.take() {
        Some(session) if same_route => (session, true),
        held => {
            if let Some(mut held) = held {
                held.close().await;
            }
            slot.forget_session_state();
            match connect_with_turn_state(&target, slot.turn_state.as_deref()).await {
                Ok(session) => (session, false),
                Err(error) => return Err(TurnOpenError::Handshake { error, body }),
            }
        }
    };
    if !reused && let Some(turn_state) = session.handshake_turn_state() {
        slot.turn_state = Some(turn_state);
    }

    let create = TurnCreate::prepare(&slot, turn, body, reused);
    create.send(slot, session, target, reused).await
}

pub(super) struct TurnCreate {
    pub(super) full_text: String,
    pub(super) full_input: Vec<Value>,
    pub(super) incremental_text: Option<String>,
}

impl TurnCreate {
    pub(super) fn prepare(
        slot: &ResponsesWsSlot,
        turn: &ResponsesWsTurn,
        body: Value,
        reused: bool,
    ) -> Self {
        let mut payload = responses_ws_create_payload(body);
        if let Some(object) = payload.as_object_mut() {
            if turn.warmup {
                object.insert("generate".to_string(), Value::Bool(false));
            }
            if !object.contains_key("client_metadata")
                && let Some(metadata) = turn.client_metadata.clone()
            {
                object.insert("client_metadata".to_string(), metadata);
            }
            if let Some(turn_state) = slot.turn_state.clone() {
                let metadata = object
                    .entry("client_metadata")
                    .or_insert_with(|| Value::Object(serde_json::Map::new()));
                if let Some(metadata) = metadata.as_object_mut()
                    && !metadata.contains_key(TURN_STATE_HEADER)
                {
                    metadata.insert(TURN_STATE_HEADER.to_string(), Value::String(turn_state));
                }
            }
        }
        let full_input = payload
            .get("input")
            .and_then(Value::as_array)
            .cloned()
            .unwrap_or_default();
        let full_text = payload.to_string();
        let incremental_text = if reused {
            incremental_create(slot, turn, &payload, &full_input)
        } else {
            None
        };
        drop(payload);
        Self {
            full_text,
            full_input,
            incremental_text,
        }
    }

    pub(super) async fn send(
        self,
        mut slot: OwnedMutexGuard<ResponsesWsSlot>,
        mut session: Box<dyn ResponsesWsSession>,
        target: UpstreamWsTarget,
        reused: bool,
    ) -> Result<ResponsesWsTurnSource, TurnOpenError> {
        let Self {
            full_text,
            full_input,
            incremental_text,
        } = self;
        let incremental = incremental_text.is_some();
        // WS2h item 2: the session still queues pending steering, which a full create
        // already contains, so a full replay must not run on it.
        let sent = if reused && !incremental && slot.pending_steer_parts > 0 {
            Err("pending steering needs a fresh session".to_string())
        } else {
            session
                .send_text(incremental_text.unwrap_or_else(|| full_text.clone()))
                .await
        };
        match sent {
            Ok(()) => Ok(ResponsesWsTurnSource {
                slot,
                session: Some(session),
                target,
                full_text: if reused { full_text } else { String::new() },
                full_input,
                incremental,
                reused,
            }),
            Err(_) if reused => {
                // WS2h item 3: a stale pinned session is replaced transparently.
                session.close().await;
                slot.last_upstream_id = None;
                let mut session = connect_with_turn_state(&target, slot.turn_state.as_deref())
                    .await
                    .map_err(|error| TurnOpenError::Send(error.message))?;
                session
                    .send_text(full_text)
                    .await
                    .map_err(TurnOpenError::Send)?;
                Ok(ResponsesWsTurnSource {
                    slot,
                    session: Some(session),
                    target,
                    full_text: String::new(),
                    full_input,
                    incremental: false,
                    reused: false,
                })
            }
            Err(error) => Err(TurnOpenError::Send(error)),
        }
    }
}

pub(super) fn content_parts(content: &Value) -> usize {
    content.as_array().map_or(1, Vec::len)
}

/// WS2h item 2: content parts a steer input contributes to the encoded history.
pub(super) fn steer_content_parts(input: &Value) -> usize {
    match input {
        Value::Array(items) => items
            .iter()
            .map(|item| item.get("content").map_or(1, content_parts))
            .sum(),
        _ => 1,
    }
}

/// WS2h item 2: the incremental create text, when the encoded input extends the
/// session basis and the turn continues the session's last public response.
pub(super) fn incremental_create(
    slot: &ResponsesWsSlot,
    turn: &ResponsesWsTurn,
    payload: &Value,
    full_input: &[Value],
) -> Option<String> {
    let upstream_id = slot.last_upstream_id.as_ref()?;
    let previous = turn.previous_response_id.as_ref()?;
    if slot.last_public_id.as_ref() != Some(previous) {
        return None;
    }
    let basis = slot.basis.len();
    let replayed = basis.saturating_add(slot.last_output.len());
    if full_input.len() < replayed || full_input[..basis] != slot.basis[..] {
        return None;
    }
    // The replayed previous output must be exactly the upstream's own output
    // items, proven by item id; anything else needs the full create.
    for ((kind, id), item) in slot.last_output.iter().zip(&full_input[basis..replayed]) {
        let id = id.as_deref()?;
        let item_kind = item
            .get("type")
            .and_then(Value::as_str)
            .unwrap_or("message");
        if item_kind != kind || item.get("id").and_then(Value::as_str) != Some(id) {
            return None;
        }
    }
    // The encoder merges adjacent user messages, so pending steering must span
    // whole items by content part count; a merge with new input needs the full create.
    let mut prefix = replayed;
    let mut parts = 0;
    while parts < slot.pending_steer_parts {
        let item = full_input.get(prefix)?;
        if item.get("role").and_then(Value::as_str) != Some("user") {
            return None;
        }
        parts += item.get("content").map_or(1, content_parts);
        prefix += 1;
    }
    if parts != slot.pending_steer_parts {
        return None;
    }
    let mut object = serde_json::Map::new();
    for (key, value) in payload.as_object()? {
        if key != "input" {
            object.insert(key.clone(), value.clone());
        }
    }
    object.insert(
        "input".to_string(),
        Value::Array(full_input[prefix..].to_vec()),
    );
    object.insert(
        "previous_response_id".to_string(),
        Value::String(upstream_id.clone()),
    );
    Some(Value::Object(object).to_string())
}

/// WS2: synthetic warmup frames fed through the stream decoder when a warmup
/// turn cannot reach an upstream WebSocket (WS2c, WS6).
pub(crate) fn local_warmup_frames(model: &str) -> Vec<String> {
    let id = format!("resp_monoize_warmup_{}", uuid::Uuid::new_v4().simple());
    let created_at = chrono::Utc::now().timestamp();
    let response = |status: &str, usage: Value| {
        json!({
            "id": id,
            "object": "response",
            "created_at": created_at,
            "status": status,
            "model": model,
            "output": [],
            "usage": usage,
        })
    };
    vec![
        json!({
            "type": "response.created",
            "sequence_number": 0,
            "response": response("in_progress", Value::Null),
        })
        .to_string(),
        json!({
            "type": "response.completed",
            "sequence_number": 1,
            "response": response(
                "completed",
                json!({"input_tokens": 0, "output_tokens": 0, "total_tokens": 0}),
            ),
        })
        .to_string(),
    ]
}
