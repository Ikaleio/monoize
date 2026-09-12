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
    model: String,
    downstream: DownstreamProtocol,
    resource_scope: Option<urp::MediaResource>,
}

impl HistoryContext {
    pub(super) fn from_request(
        state: &AppState,
        req: &urp::UrpRequest,
        downstream: DownstreamProtocol,
    ) -> Option<Self> {
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
            model: req.model.clone(),
            downstream,
            resource_scope: None,
        })
    }

    pub(super) fn with_resource_scope(mut self, scope: Option<urp::MediaResource>) -> Self {
        self.resource_scope = scope;
        self
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

    pub(super) async fn retain_response(&self, response: &urp::UrpResponse) {
        if !self.store {
            return;
        }
        let successful = match response.extra_body.get("status").and_then(Value::as_str) {
            Some("completed" | "incomplete") => true,
            Some(_) => false,
            None => !matches!(response.finish_reason, Some(urp::FinishReason::Other)),
        };
        if !successful {
            return;
        }
        if encode_response_for_downstream(self.downstream, response, &self.model).is_err() {
            return;
        }
        let mut output = response.output.clone();
        let Ok(resources) = urp::media::resources(&output) else {
            return;
        };
        if !resources.is_empty() {
            let Some(scope) = &self.resource_scope else {
                return;
            };
            if resources
                .iter()
                .any(|resource| !urp::media::resource_matches_scope(resource, scope))
            {
                return;
            }
            urp::media::bind_resources(&mut output, scope);
        }
        let mut nodes = self.input.clone();
        nodes.extend(output);
        self.cache
            .lock()
            .await
            .insert(self.id.clone(), self.scope.clone(), nodes);
    }

    pub(super) async fn finish_response(&self, resp: &mut urp::UrpResponse) {
        self.retain_response(resp).await;
        self.decorate_response(resp);
    }

    pub(super) fn decorate_response(&self, resp: &mut urp::UrpResponse) {
        resp.id = self.id.clone();
        self.decorate_extra(&mut resp.extra_body);
    }

    pub(super) async fn forward_stream(
        self,
        mut rx: mpsc::Receiver<urp::UrpStreamEvent>,
        tx: mpsc::Sender<urp::UrpStreamEvent>,
    ) -> Option<urp::UrpResponse> {
        let mut terminal = None;
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
                    terminal = Some(urp::UrpResponse {
                        id: self.id.clone(),
                        model: self.model.clone(),
                        created_at: None,
                        output: output.clone(),
                        finish_reason: *finish_reason,
                        usage: None,
                        extra_body: extra_body.clone(),
                    });
                    self.decorate_extra(extra_body);
                }
                _ => {}
            }
            if tx.send(event).await.is_err() {
                break;
            }
        }
        terminal
    }
}

pub(super) fn is_managed(req: &urp::UrpRequest) -> bool {
    req.extra_body.contains_key(CONTEXT_KEY)
}

#[cfg(test)]
mod media_history_tests {
    use super::*;

    fn resource(provider: &str) -> urp::MediaResource {
        urp::MediaResource {
            protocol: urp::ProviderProtocol::Responses,
            provider_id: Some(provider.into()),
            channel_id: Some(format!("channel-{provider}")),
            credential_scope: Some(format!("credential-{provider}")),
        }
    }

    fn context(input: Vec<urp::Node>) -> HistoryContext {
        HistoryContext {
            cache: Arc::new(Mutex::new(ResponseHistoryStore::default())),
            id: "resp_history_test".into(),
            scope: "tenant".into(),
            store: true,
            previous: Value::Null,
            input,
            model: "test-model".into(),
            downstream: DownstreamProtocol::Responses,
            resource_scope: Some(resource("A")),
        }
    }

    fn file_input() -> urp::Node {
        serde_json::from_value(json!({"type":"file","role":"user",
            "source":{"type":"file_id","file_id":"file-input"},
            "metadata":{"resource":resource("A")}}))
        .unwrap()
    }

    fn file_output(scope: Option<urp::MediaResource>) -> urp::Node {
        let scope = scope.unwrap_or(urp::MediaResource {
            protocol: urp::ProviderProtocol::Responses,
            provider_id: None,
            channel_id: None,
            credential_scope: None,
        });
        serde_json::from_value(json!({"type":"tool_result","call_id":"call-1","name":"document",
            "tool_type":"function","is_error":false,"content":[{"type":"file",
                "source":{"type":"file_id","file_id":"file-output"},"metadata":{"resource":scope}}]})).unwrap()
    }

    fn response(output: Vec<urp::Node>) -> urp::UrpResponse {
        urp::UrpResponse {
            id: "upstream-id".into(),
            model: "test-model".into(),
            created_at: None,
            output,
            finish_reason: Some(urp::FinishReason::Stop),
            usage: None,
            extra_body: HashMap::new(),
        }
    }

    fn invalid_audio() -> urp::Node {
        serde_json::from_value(json!({"type":"audio","role":"assistant",
            "source":{"type":"base64","media_type":"audio/wav","data":"YXVkaW8="}}))
        .unwrap()
    }

    #[tokio::test]
    async fn retained_input_and_output_keep_the_successful_resource_scope() {
        let history = context(vec![file_input()]);
        let mut resp = response(vec![file_output(None)]);
        history.finish_response(&mut resp).await;
        let cache = history.cache.lock().await;
        let entry = cache.entries.get(&history.id).unwrap();
        let resources = urp::media::resources(&entry.nodes).unwrap();
        assert_eq!(resources.len(), 2);
        for scope in &resources {
            assert!(urp::media::resource_matches_scope(scope, &resource("A")));
            assert!(!urp::media::resource_matches_scope(scope, &resource("B")));
            assert_eq!(scope.credential_scope, resource("A").credential_scope);
        }
        assert_eq!(resp.id, history.id);
        assert_eq!(resp.extra_body["store"], true);
    }

    #[tokio::test]
    async fn output_scope_is_never_rebound_to_the_successful_attempt() {
        let history = context(vec![file_input()]);
        let mut resp = response(vec![file_output(Some(resource("B")))]);
        history.finish_response(&mut resp).await;
        assert!(history.cache.lock().await.entries.is_empty());
        assert_eq!(
            urp::media::resources(&resp.output).unwrap(),
            vec![resource("B")]
        );
    }

    #[tokio::test]
    async fn invalid_output_is_decorated_without_retention() {
        let history = context(vec![file_input()]);
        let mut resp = response(vec![invalid_audio()]);
        history.finish_response(&mut resp).await;
        assert_eq!(resp.id, history.id);
        assert_eq!(resp.extra_body["store"], true);
        assert!(history.cache.lock().await.entries.is_empty());
    }

    #[tokio::test]
    async fn compound_document_input_scope_survives_history_retention() {
        let mut input = urp::decode::anthropic::decode_request(&json!({
            "model":"test-model", "messages":[{"role":"user", "content":[
                {"type":"document", "source":{"type":"content", "content":[
                    {"type":"image", "source":{"type":"file", "file_id":"file-nested"}}
                ]}}
            ]}]
        }))
        .unwrap()
        .input;
        let scope_a = urp::MediaResource {
            protocol: urp::ProviderProtocol::Messages,
            ..resource("A")
        };
        let scope_b = urp::MediaResource {
            protocol: urp::ProviderProtocol::Messages,
            ..resource("B")
        };
        urp::media::bind_resources(&mut input, &scope_a);
        let history = context(input).with_resource_scope(Some(scope_a.clone()));
        history
            .retain_response(&response(vec![urp::Node::assistant_text("done")]))
            .await;
        let cache = history.cache.lock().await;
        let resources = urp::media::resources(&cache.entries[&history.id].nodes).unwrap();
        assert_eq!(resources.len(), 1);
        assert!(urp::media::resource_matches_scope(&resources[0], &scope_a));
        assert!(!urp::media::resource_matches_scope(&resources[0], &scope_b));
    }

    #[tokio::test]
    async fn live_stream_commits_only_after_successful_encoding() {
        for invalid_early in [false, true] {
            let history = context(vec![file_input()]);
            let output = vec![urp::Node::assistant_text("done")];
            let (tx, rx) = mpsc::channel(8);
            tx.send(urp::UrpStreamEvent::ResponseStart {
                id: "upstream-id".into(),
                model: "test-model".into(),
                usage: None,
                extra_body: HashMap::new(),
            })
            .await
            .unwrap();
            if invalid_early {
                tx.send(urp::UrpStreamEvent::NodeDone {
                    node_index: 0,
                    node: invalid_audio(),
                    usage: None,
                    extra_body: HashMap::new(),
                })
                .await
                .unwrap();
            }
            tx.send(urp::UrpStreamEvent::ResponseDone {
                output,
                finish_reason: Some(urp::FinishReason::Stop),
                usage: None,
                extra_body: HashMap::new(),
            })
            .await
            .unwrap();
            drop(tx);
            let (history_tx, history_rx) = mpsc::channel(8);
            let pending = history
                .clone()
                .forward_stream(rx, history_tx)
                .await
                .unwrap();
            assert!(history.cache.lock().await.entries.is_empty());
            let (wire_tx, _wire_rx) = mpsc::channel(64);
            let result = urp::stream_encode::encode_urp_stream(
                DownstreamProtocol::Responses,
                history_rx,
                wire_tx,
                "test-model",
                Instant::now(),
                None,
                false,
            )
            .await;
            if result.is_ok() {
                history.retain_response(&pending).await;
            }
            assert_eq!(result.is_err(), invalid_early);
            assert_eq!(history.cache.lock().await.entries.is_empty(), invalid_early);
        }
    }

    #[tokio::test]
    async fn synthetic_stream_failure_does_not_commit_decorated_output() {
        for invalid in [false, true] {
            let history = context(vec![file_input()]);
            let mut resp = response(vec![if invalid {
                invalid_audio()
            } else {
                urp::Node::assistant_text("done")
            }]);
            history.decorate_response(&mut resp);
            assert!(history.cache.lock().await.entries.is_empty());
            let (tx, _rx) = mpsc::channel(64);
            let result = urp::stream_encode::emit_synthetic_stream_from_urp_response(
                DownstreamProtocol::Responses,
                "test-model",
                &resp,
                None,
                None,
                tx,
            )
            .await;
            if result.is_ok() {
                history.retain_response(&resp).await;
            }
            assert_eq!(result.is_err(), invalid);
            assert_eq!(history.cache.lock().await.entries.is_empty(), invalid);
        }
    }
}
