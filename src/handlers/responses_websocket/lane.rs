use super::*;

/// WS4b: the public response a relayed event belongs to.
pub(super) fn relay_response_id(value: &Value) -> Option<String> {
    value
        .pointer("/steer/previous_response_id")
        .and_then(Value::as_str)
        .map(str::to_string)
        .or_else(|| crate::upstream_websocket::find_response_id(value).map(str::to_string))
}

/// WS4b: an acknowledgement takes the next number of the response it names;
/// provider-control notifications keep their upstream fields.
pub(super) async fn forward_relay(
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
pub(super) struct PreparedTurn {
    pub(super) request: Map<String, Value>,
    pub(super) full_input: Vec<Value>,
    pub(super) previous: Option<String>,
    pub(super) same_lane_previous: bool,
    pub(super) warmup: bool,
    pub(super) strip_encrypted: bool,
    pub(super) client_metadata: Option<Value>,
    pub(super) successor: Option<(String, String)>,
    /// WS18: `full_input` was cut to the steer input because the history exceeds
    /// the limit, so the turn can never be retained.
    pub(super) oversized: bool,
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
pub(super) struct FinishedTurn {
    pub(super) public_id: String,
    pub(super) controls: Map<String, Value>,
    pub(super) full_input: Vec<Value>,
    pub(super) output: Vec<Value>,
    pub(super) pending_steers: Vec<Value>,
    pub(super) strip_encrypted: bool,
    /// WS17: history exceeds the limit; the vectors above hold nothing retained.
    pub(super) oversized: bool,
}

pub(super) async fn run_lane(
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
pub(super) fn steer_items(input: Value) -> Vec<Value> {
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
