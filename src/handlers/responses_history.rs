use super::*;

const CONTEXT_KEY: &str = "_monoize_response_history";
const MAX_NODES: usize = 4096;
const MAX_ENTRY_BYTES: usize = 32 * 1024 * 1024;
const MAX_CACHE_BYTES: usize = 128 * 1024 * 1024;
const MAX_ENTRIES: usize = 1024;
const TTL: Duration = Duration::from_secs(30 * 60);

struct HistoryEntry {
    scope: String,
    nodes: Vec<urp::Node>,
    bytes: usize,
    inserted_at: Instant,
}

#[derive(Default)]
pub(crate) struct ResponseHistoryStore {
    entries: HashMap<String, HistoryEntry>,
    bytes: usize,
}

impl ResponseHistoryStore {
    pub(crate) fn cleanup(&mut self) {
        self.entries
            .retain(|_, entry| entry.inserted_at.elapsed() < TTL);
        self.bytes = self.entries.values().map(|entry| entry.bytes).sum();
    }

    fn insert(&mut self, id: String, scope: String, nodes: Vec<urp::Node>) {
        self.cleanup();
        let Ok(encoded) = serde_json::to_vec(&nodes) else {
            return;
        };
        let bytes = encoded.len();
        if nodes.len() > MAX_NODES || bytes > MAX_ENTRY_BYTES {
            return;
        }
        if let Some(previous) = self.entries.remove(&id) {
            self.bytes -= previous.bytes;
        }
        while self.entries.len() >= MAX_ENTRIES || self.bytes + bytes > MAX_CACHE_BYTES {
            let oldest = self
                .entries
                .iter()
                .min_by_key(|(_, entry)| entry.inserted_at)
                .map(|(id, _)| id.clone());
            let Some(oldest) = oldest else { break };
            if let Some(entry) = self.entries.remove(&oldest) {
                self.bytes -= entry.bytes;
            }
        }
        self.bytes += bytes;
        self.entries.insert(
            id,
            HistoryEntry {
                scope,
                nodes,
                bytes,
                inserted_at: Instant::now(),
            },
        );
    }
}

fn invalid(message: &str, param: &str, code: &str) -> AppError {
    let mut error = AppError::new(StatusCode::BAD_REQUEST, code, message);
    error.param = Some(param.to_string());
    error
}

pub(super) async fn prepare(
    state: &AppState,
    auth: &crate::auth::AuthResult,
    req: &mut urp::UrpRequest,
) -> AppResult<()> {
    let scope = json!([
        auth.tenant_id,
        auth.api_key_id,
        auth.internal_source.map(|source| source.request_kind())
    ])
    .to_string();
    let previous = req
        .extra_body
        .remove("previous_response_id")
        .unwrap_or(Value::Null);
    if !previous.is_null() {
        let id = previous.as_str().ok_or_else(|| {
            invalid(
                "previous_response_id must be a string",
                "previous_response_id",
                "invalid_request",
            )
        })?;
        if req
            .extra_body
            .get("conversation")
            .is_some_and(|value| !value.is_null())
        {
            return Err(invalid(
                "conversation and previous_response_id are mutually exclusive",
                "previous_response_id",
                "invalid_request",
            ));
        }
        let mut cache = state.response_history.lock().await;
        cache.cleanup();
        let mut nodes = cache
            .entries
            .get(id)
            .filter(|entry| entry.scope == scope)
            .map(|entry| entry.nodes.clone())
            .ok_or_else(|| {
                invalid(
                    "the previous response is not available",
                    "previous_response_id",
                    "previous_response_not_found",
                )
            })?;
        nodes.append(&mut req.input);
        req.input = nodes;
    }
    let store = match req.extra_body.remove("store") {
        None | Some(Value::Null) => true,
        Some(Value::Bool(store)) => store,
        _ => {
            return Err(invalid(
                "store must be a boolean",
                "store",
                "invalid_request",
            ));
        }
    };
    req.extra_body
        .insert("store".to_string(), Value::Bool(false));
    req.extra_body.insert(
        CONTEXT_KEY.to_string(),
        json!({
            "id": format!("resp_monoize_{}", uuid::Uuid::new_v4().simple()),
            "scope": scope, "store": store, "previous_response_id": previous,
        }),
    );
    Ok(())
}

#[derive(Clone)]
pub(super) struct HistoryContext {
    cache: Arc<Mutex<ResponseHistoryStore>>,
    id: String,
    scope: String,
    store: bool,
    previous: Value,
    input: Vec<urp::Node>,
}

impl HistoryContext {
    pub(super) fn from_request(state: &AppState, req: &urp::UrpRequest) -> Option<Self> {
        let context = req.extra_body.get(CONTEXT_KEY)?;
        let store = context.get("store")?.as_bool()?;
        let mut input = if store { req.input.clone() } else { Vec::new() };
        input.retain_mut(|node| {
            node.extra_body_mut()
                .get(urp::RESPONSES_INSTRUCTION_NODE_EXTRA_KEY)
                .and_then(Value::as_bool)
                != Some(true)
        });
        Some(Self {
            cache: state.response_history.clone(),
            id: context.get("id")?.as_str()?.to_string(),
            scope: context.get("scope")?.as_str()?.to_string(),
            store,
            previous: context
                .get("previous_response_id")
                .cloned()
                .unwrap_or(Value::Null),
            input,
        })
    }

    fn decorate_extra(&self, extra: &mut HashMap<String, Value>) {
        extra.insert("previous_response_id".to_string(), self.previous.clone());
        extra.insert("store".to_string(), Value::Bool(self.store));
        if let Some(source) = extra
            .get_mut(urp::RESPONSES_STREAM_START_SOURCE_EXTRA_KEY)
            .and_then(Value::as_object_mut)
        {
            source.insert("previous_response_id".to_string(), self.previous.clone());
            source.insert("store".to_string(), Value::Bool(self.store));
        }
    }

    async fn retain(
        &self,
        output: &[urp::Node],
        finish: Option<urp::FinishReason>,
        extra: &HashMap<String, Value>,
    ) {
        if !self.store {
            return;
        }
        let successful = match extra.get("status").and_then(Value::as_str) {
            Some("completed" | "incomplete") => true,
            Some(_) => false,
            None => !matches!(finish, Some(urp::FinishReason::Other)),
        };
        if !successful {
            return;
        }
        let mut nodes = self.input.clone();
        nodes.extend_from_slice(output);
        self.cache
            .lock()
            .await
            .insert(self.id.clone(), self.scope.clone(), nodes);
    }

    pub(super) async fn finish_response(&self, resp: &mut urp::UrpResponse) {
        self.retain(&resp.output, resp.finish_reason, &resp.extra_body)
            .await;
        resp.id = self.id.clone();
        self.decorate_extra(&mut resp.extra_body);
    }

    pub(super) async fn forward_stream(
        self,
        mut rx: mpsc::Receiver<urp::UrpStreamEvent>,
        tx: mpsc::Sender<urp::UrpStreamEvent>,
    ) {
        while let Some(mut event) = rx.recv().await {
            match &mut event {
                urp::UrpStreamEvent::ResponseStart { id, extra_body, .. } => {
                    *id = self.id.clone();
                    self.decorate_extra(extra_body);
                }
                urp::UrpStreamEvent::ResponseDone {
                    output,
                    finish_reason,
                    extra_body,
                    ..
                } => {
                    self.retain(output, *finish_reason, extra_body).await;
                    self.decorate_extra(extra_body);
                }
                _ => {}
            }
            if tx.send(event).await.is_err() {
                break;
            }
        }
    }
}

pub(super) fn is_managed(req: &urp::UrpRequest) -> bool {
    req.extra_body.contains_key(CONTEXT_KEY)
}
