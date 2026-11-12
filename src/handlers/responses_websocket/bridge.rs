use super::*;

/// Converts one turn's pipeline SSE body into WebSocket events (WS4, WS12a,
/// WS12c, WS19, WS5a).
pub(super) struct TurnBridge {
    pub(super) oversized: bool,
    pub(super) conn: Arc<Connection>,
    pub(super) key: LaneKey,
    pub(super) public_id: String,
    pub(super) control: watch::Receiver<ResponsesWsControl>,
    pub(super) state: Arc<ResponsesWsTurnState>,
    pub(super) strip_encrypted: bool,
    pub(super) input_items: usize,
    pub(super) input_bytes: usize,
}

#[derive(Default)]
pub(super) struct BridgeTracker {
    pub(super) created: Option<Value>,
    pub(super) done_items: BTreeMap<u64, Value>,
    pub(super) next_sequence: Option<i64>,
    /// WS4b: this response's relays waiting for its lifecycle barrier.
    pub(super) held: Vec<ResponsesWsRelay>,
    pub(super) outcome: TurnOutcome,
    pub(super) failed: bool,
}

pub(super) enum BridgeFlow {
    Continue,
    Disconnected,
}

#[derive(Default)]
pub(super) struct BridgeInput {
    pub(super) buffer: Vec<u8>,
    pub(super) tracker: BridgeTracker,
    pub(super) interrupt_pending: bool,
    pub(super) control_closed: bool,
}

impl BridgeInput {
    pub(super) fn should_interrupt(&self) -> bool {
        self.interrupt_pending
            && self.tracker.created.is_some()
            && self.tracker.outcome.status.is_none()
            && !self.tracker.failed
    }
}

pub(super) enum BridgeStep {
    Continue,
    Interrupt,
    End(TurnOutcome),
}

impl TurnBridge {
    pub(super) async fn run(
        mut self,
        response: Response,
        relay: &mut mpsc::UnboundedReceiver<ResponsesWsRelay>,
        sequences: &mut HashMap<String, i64>,
    ) -> TurnOutcome {
        let mut body = response.into_body().into_data_stream();
        let mut input = BridgeInput::default();
        loop {
            let chunk = tokio::select! {
                biased;
                changed = self.control.changed(), if !input.control_closed => {
                    match changed {
                        Err(_) => input.control_closed = true,
                        Ok(()) => {
                            match *self.control.borrow_and_update() {
                                ResponsesWsControl::Disconnected => {
                                    return TurnOutcome { disconnected: true, ..TurnOutcome::default() };
                                }
                                ResponsesWsControl::Interrupt
                                    if self.state.upstream_kind() != UPSTREAM_KIND_WEBSOCKET =>
                                {
                                    input.interrupt_pending = true;
                                }
                                _ => {}
                            }
                        }
                    }
                    if input.should_interrupt() {
                        return self.emulate_interrupt(body, input.tracker).await;
                    }
                    continue;
                }
                value = relay.recv() => {
                    if let Some(value) = value
                        && !self.accept_relay(value, &mut input.tracker, sequences).await
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
                    return self.read_failure(input.tracker).await;
                }
            };
            match self.process_chunk(&chunk, &mut input).await {
                BridgeStep::Continue => {}
                BridgeStep::Interrupt => return self.emulate_interrupt(body, input.tracker).await,
                BridgeStep::End(outcome) => return outcome,
            }
        }
        self.finish_stream(input).await
    }

    pub(super) async fn read_failure(&self, mut tracker: BridgeTracker) -> TurnOutcome {
        if tracker.outcome.status.is_none() && !tracker.failed {
            let _ = self
                .conn
                .send_error(
                    &self.key,
                    WsError::new(502, "server_error", "upstream SSE body read failed", None),
                )
                .await;
        }
        let _ = self.release_relays(&mut tracker, true).await;
        tracker.outcome.status = None;
        tracker.outcome.next_sequence = tracker.next_sequence;
        tracker.outcome
    }

    pub(super) async fn process_chunk(
        &mut self,
        chunk: &[u8],
        input: &mut BridgeInput,
    ) -> BridgeStep {
        let limits = self.conn.limits;
        if input.buffer.len().saturating_add(chunk.len()) > limits.sse_buffer_max_bytes {
            let _ = self
                .conn
                .send_error(&self.key, WsError::parser_limit())
                .await;
            return BridgeStep::End(TurnOutcome::default());
        }
        input.buffer.extend_from_slice(chunk);
        let frames = match drain_sse_data(&mut input.buffer, false, limits.sse_frame_max_bytes) {
            Ok(frames) => frames,
            Err(error) => {
                let _ = self.conn.send_error(&self.key, error).await;
                return BridgeStep::End(TurnOutcome::default());
            }
        };
        for data in frames {
            if let BridgeFlow::Disconnected = self.forward_event(data, &mut input.tracker).await {
                return BridgeStep::End(TurnOutcome {
                    disconnected: true,
                    ..TurnOutcome::default()
                });
            }
            if !self.release_relays(&mut input.tracker, false).await {
                return BridgeStep::End(TurnOutcome {
                    disconnected: true,
                    ..TurnOutcome::default()
                });
            }
            if input.should_interrupt() {
                return BridgeStep::Interrupt;
            }
        }
        BridgeStep::Continue
    }

    pub(super) async fn finish_stream(&mut self, mut input: BridgeInput) -> TurnOutcome {
        match drain_sse_data(
            &mut input.buffer,
            true,
            self.conn.limits.sse_frame_max_bytes,
        ) {
            Ok(frames) => {
                for data in frames {
                    if let BridgeFlow::Disconnected =
                        self.forward_event(data, &mut input.tracker).await
                    {
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
        if !self.release_relays(&mut input.tracker, true).await {
            return TurnOutcome {
                disconnected: true,
                ..TurnOutcome::default()
            };
        }
        if input.tracker.failed {
            input.tracker.outcome.status = None;
        }
        input.tracker.outcome.next_sequence = input.tracker.next_sequence;
        input.tracker.outcome
    }

    pub(super) fn retainable(&self, output: &[Value]) -> bool {
        !self.oversized
            && self.conn.limits.fits(
                self.input_items.saturating_add(output.len()),
                self.input_bytes.saturating_add(serialized_len(output)),
            )
    }

    /// WS4b: orders a relay into the egress of the response it names. This
    /// response's relays wait for their lifecycle barrier; an earlier response's
    /// go out now, numbered from that response's own counter.
    pub(super) async fn accept_relay(
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

    pub(super) fn ended(tracker: &BridgeTracker) -> bool {
        tracker.outcome.status.is_some() || tracker.failed
    }

    /// WS4b: sends held relays whose barrier passed. At the end of the stream the
    /// rest go out, unless the response never reached the client.
    pub(super) async fn release_relays(&self, tracker: &mut BridgeTracker, finished: bool) -> bool {
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
    pub(super) async fn forward_relay(
        &self,
        mut value: Value,
        tracker: &mut BridgeTracker,
    ) -> bool {
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

    pub(super) async fn forward_event(
        &mut self,
        data: String,
        tracker: &mut BridgeTracker,
    ) -> BridgeFlow {
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
    pub(super) async fn emulate_interrupt(
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
pub(super) fn rewrite_response_ids(value: &mut Value, public_id: &str) {
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

pub(super) fn strip_item_encrypted(item: &mut Value) {
    if item.get("type").and_then(Value::as_str) == Some("reasoning")
        && let Some(object) = item.as_object_mut()
    {
        object.remove("encrypted_content");
    }
}

pub(super) fn strip_encrypted_reasoning(event: &mut Value) {
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

pub(super) fn drain_sse_data(
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

pub(super) fn parse_sse_data_frame(frame: &[u8]) -> Option<String> {
    let frame = String::from_utf8_lossy(frame);
    let data = frame
        .lines()
        .filter_map(|line| line.strip_prefix("data:"))
        .map(|line| line.strip_prefix(' ').unwrap_or(line))
        .collect::<Vec<_>>();
    (!data.is_empty()).then(|| data.join("\n"))
}
