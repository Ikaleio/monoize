use super::*;

/// WS12a: returns an error frame for an invalid mode; otherwise signals the target.
pub(super) async fn handle_interrupt(
    conn: &Connection,
    event: &Map<String, Value>,
) -> Option<WsError> {
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
pub(super) async fn handle_steer(conn: &Connection, event: &Map<String, Value>) {
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
