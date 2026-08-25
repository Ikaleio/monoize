use crate::auth::AuthResult;
use crate::config::ProviderType;
use crate::db::DbPool;
use crate::handlers::DownstreamProtocol;
use crate::monoize_routing::MonoizeRuntimeConfig;
use crate::transforms::TransformRuleConfig;
use crate::users::RequestCaptureMode;
use chrono::{SecondsFormat, Utc};
use sea_orm::ConnectionTrait;
use serde_json::{Value, json};
use std::path::{Path, PathBuf};
use std::sync::Arc;
use tokio::sync::{Mutex, RwLock};
use tokio::task::JoinHandle;
use tokio::time::MissedTickBehavior;

const DEFAULT_MAX_ATTEMPTS: usize = 16;
const DEFAULT_MAX_FRAMES: usize = 4_096;
const DEFAULT_MAX_FRAME_BYTES: usize = 256 * 1024;
const DEFAULT_MAX_SESSION_BYTES: usize = 16 * 1024 * 1024;
const MIN_SESSION_BYTES: usize = 8 * 1024;
const MAX_CAPTURE_IDENTIFIER_BYTES: usize = 256;

#[derive(Clone, Copy, Debug)]
struct RequestCaptureLimits {
    max_attempts: usize,
    max_frames: usize,
    max_frame_bytes: usize,
    max_session_bytes: usize,
}

impl RequestCaptureLimits {
    fn from_env() -> Self {
        Self {
            max_attempts: positive_env(
                "MONOIZE_REQUEST_CAPTURE_MAX_ATTEMPTS",
                DEFAULT_MAX_ATTEMPTS,
            ),
            max_frames: positive_env("MONOIZE_REQUEST_CAPTURE_MAX_FRAMES", DEFAULT_MAX_FRAMES),
            max_frame_bytes: positive_env(
                "MONOIZE_REQUEST_CAPTURE_MAX_FRAME_BYTES",
                DEFAULT_MAX_FRAME_BYTES,
            ),
            max_session_bytes: positive_env(
                "MONOIZE_REQUEST_CAPTURE_MAX_SESSION_BYTES",
                DEFAULT_MAX_SESSION_BYTES,
            )
            .max(MIN_SESSION_BYTES),
        }
    }
}

#[derive(Clone)]
pub(crate) struct SseFrameCapture {
    state: Arc<Mutex<SseFrameCaptureState>>,
    limits: RequestCaptureLimits,
}

#[derive(Debug, Default)]
struct SseFrameCaptureState {
    frames: Vec<String>,
    omitted_frames: usize,
    omitted_bytes: usize,
    retained_bytes: usize,
}

impl SseFrameCapture {
    pub(crate) fn new() -> Self {
        Self::with_limits(RequestCaptureLimits::from_env())
    }

    fn with_limits(limits: RequestCaptureLimits) -> Self {
        Self {
            state: Arc::new(Mutex::new(SseFrameCaptureState::default())),
            limits,
        }
    }

    pub(crate) async fn record(&self, frame: String) {
        let mut state = self.state.lock().await;
        if state.frames.len() >= self.limits.max_frames {
            state.omitted_frames = state.omitted_frames.saturating_add(1);
            state.omitted_bytes = state.omitted_bytes.saturating_add(frame.len());
            return;
        }
        let available = self
            .limits
            .max_session_bytes
            .saturating_sub(state.retained_bytes);
        if available == 0 {
            state.omitted_frames = state.omitted_frames.saturating_add(1);
            state.omitted_bytes = state.omitted_bytes.saturating_add(frame.len());
            return;
        }
        let retained = truncate_utf8(&frame, self.limits.max_frame_bytes.min(available));
        if retained.is_empty() && !frame.is_empty() {
            state.omitted_frames = state.omitted_frames.saturating_add(1);
            state.omitted_bytes = state.omitted_bytes.saturating_add(frame.len());
            return;
        }
        state.omitted_bytes = state
            .omitted_bytes
            .saturating_add(frame.len().saturating_sub(retained.len()));
        state.retained_bytes = state.retained_bytes.saturating_add(retained.len());
        state.frames.push(retained);
    }

    pub(crate) async fn snapshot(&self) -> CapturedSseFrames {
        let state = self.state.lock().await;
        CapturedSseFrames {
            frames: state.frames.clone(),
            truncation: CaptureTruncation {
                omitted_frames: state.omitted_frames,
                omitted_bytes: state.omitted_bytes,
                retained_bytes: state.retained_bytes,
                retained_frames: state.frames.len(),
                ..CaptureTruncation::default()
            },
        }
    }

    #[cfg(test)]
    pub(crate) async fn captured_frames(&self) -> Vec<String> {
        self.state.lock().await.frames.clone()
    }
}

tokio::task_local! {
    static CURRENT_SSE_CAPTURE: SseFrameCapture;
}

pub(crate) async fn capture_sse_frame(frame: String) {
    if let Ok(capture) = CURRENT_SSE_CAPTURE.try_with(Clone::clone) {
        capture.record(frame).await;
    }
}

pub(crate) async fn with_sse_capture<F, T>(capture: SseFrameCapture, future: F) -> T
where
    F: std::future::Future<Output = T>,
{
    CURRENT_SSE_CAPTURE.scope(capture, future).await
}

pub(crate) fn spawn_with_sse_capture<F, T>(future: F) -> JoinHandle<T>
where
    F: std::future::Future<Output = T> + Send + 'static,
    T: Send + 'static,
{
    let capture = CURRENT_SSE_CAPTURE.try_with(Clone::clone).ok();
    tokio::spawn(async move {
        if let Some(capture) = capture {
            CURRENT_SSE_CAPTURE.scope(capture, future).await
        } else {
            future.await
        }
    })
}

#[derive(Clone)]
pub struct RequestCaptureStore {
    dump_dir: Arc<PathBuf>,
    limits: RequestCaptureLimits,
    db: Option<DbPool>,
}

/// One `request_capture_records` row (RCD-M1).
#[derive(Clone, Debug)]
pub struct CaptureRecordRow {
    pub file_name: String,
    pub request_id: String,
    pub user_id: String,
    pub api_key_id: String,
    pub created_at: String,
    pub created_at_unix_ms: i64,
    pub size_bytes: i64,
}

#[derive(Clone)]
pub(crate) struct RequestCaptureSession {
    store: RequestCaptureStore,
    request_id: Option<String>,
    created_at: chrono::DateTime<Utc>,
    api_key_id: String,
    user_id: String,
    downstream_protocol: DownstreamProtocol,
    is_stream: bool,
    mode: RequestCaptureMode,
    attempts: Arc<Mutex<Vec<Value>>>,
    limits: RequestCaptureLimits,
    truncation: Arc<Mutex<CaptureTruncation>>,
}

#[derive(Clone, Debug, Default)]
struct CaptureTruncation {
    omitted_attempts: usize,
    omitted_frames: usize,
    omitted_bytes: usize,
    retained_bytes: usize,
    retained_frames: usize,
}

#[derive(Clone, Debug)]
pub(crate) struct CapturedSseFrames {
    frames: Vec<String>,
    truncation: CaptureTruncation,
}

impl CaptureTruncation {
    fn to_json(&self) -> Value {
        json!({
            "truncated": self.omitted_attempts > 0 || self.omitted_frames > 0 || self.omitted_bytes > 0,
            "omitted_attempts": self.omitted_attempts,
            "omitted_frames": self.omitted_frames,
            "omitted_bytes": self.omitted_bytes,
            "retained_bytes": self.retained_bytes,
            "retained_frames": self.retained_frames,
        })
    }
}

/// RCD-D3a: list the transform rules that apply to an attempt, in application
/// order (provider, then global, then API key), using the same applicability
/// predicate as `transforms::apply_transforms` minus the phase filter: entries
/// record their phase instead of being filtered by it. Rule `config` payloads
/// are intentionally not recorded.
pub(crate) fn build_transform_chain(
    provider_rules: &[TransformRuleConfig],
    global_rules: &[TransformRuleConfig],
    api_key_rules: &[TransformRuleConfig],
    match_model: &str,
) -> Value {
    let mut chain = Vec::new();
    for (scope, rules) in [
        ("provider", provider_rules),
        ("global", global_rules),
        ("api_key", api_key_rules),
    ] {
        for rule in rules {
            if !rule.enabled {
                continue;
            }
            if let Some(patterns) = &rule.models
                && !patterns
                    .iter()
                    .any(|pattern| crate::transforms::model_glob_match(pattern, match_model))
            {
                continue;
            }
            chain.push(json!({
                "scope": scope,
                "transform": crate::transforms::canonical_transform_id(&rule.transform),
                "phase": rule.phase,
            }));
        }
    }
    Value::Array(chain)
}

#[allow(clippy::too_many_arguments)]
pub(crate) fn build_attempt_dump(
    attempt_number: u32,
    provider_id: &str,
    channel_id: Option<&str>,
    provider_type: ProviderType,
    logical_model: &str,
    upstream_model: &str,
    upstream_path: &str,
    raw_input: Value,
    transformed_urp_request: &crate::urp::UrpRequest,
    upstream_request: Value,
    downstream_response: Option<Value>,
    downstream_sse_frames: Option<CapturedSseFrames>,
    transform_chain: Value,
    error: Option<Value>,
) -> Value {
    let (downstream_sse_frames, frame_truncation) = downstream_sse_frames
        .map(|capture| (Some(capture.frames), capture.truncation))
        .unwrap_or((None, CaptureTruncation::default()));
    json!({
        "attempt_number": attempt_number,
        "provider_id": provider_id,
        "channel_id": channel_id,
        "provider_type": provider_type_name(provider_type),
        "logical_model": logical_model,
        "upstream_model": upstream_model,
        "upstream_path": upstream_path,
        "raw_input": raw_input,
        "transformed_urp_request": transformed_urp_request,
        "upstream_request": upstream_request,
        "downstream_response": downstream_response,
        "downstream_sse_frames": downstream_sse_frames,
        "downstream_sse_frames_truncation": frame_truncation.to_json(),
        "transform_chain": transform_chain,
        "error": error,
    })
}

impl RequestCaptureStore {
    pub fn new(database_dsn: &str) -> Self {
        Self {
            dump_dir: Arc::new(data_dir_from_database_dsn(database_dsn).join("dumps")),
            limits: RequestCaptureLimits::from_env(),
            db: None,
        }
    }

    /// Attach the database pool used for `request_capture_records` metadata
    /// rows (RCD-M3). Without a pool, dumps are written but no metadata row
    /// is recorded, so the capture stays unreachable through the detail API.
    pub fn with_db(mut self, db: DbPool) -> Self {
        self.db = Some(db);
        self
    }

    pub fn dump_dir(&self) -> &Path {
        &self.dump_dir
    }

    pub(crate) async fn maybe_start_session(
        &self,
        runtime: &RwLock<MonoizeRuntimeConfig>,
        auth: &AuthResult,
        request_id: Option<String>,
        downstream_protocol: DownstreamProtocol,
        is_stream: bool,
    ) -> Option<RequestCaptureSession> {
        let rt = runtime.read().await;
        if !rt.request_capture_enabled || !auth.request_capture_mode.should_start_capture() {
            return None;
        }
        let api_key_id = auth.api_key_id.clone()?;
        let user_id = auth.user_id.clone()?;
        let (request_id, omitted_request_id_bytes) = match request_id {
            Some(request_id) => {
                let retained = truncate_utf8(&request_id, MAX_CAPTURE_IDENTIFIER_BYTES);
                let omitted = request_id.len().saturating_sub(retained.len());
                (Some(retained), omitted)
            }
            None => (None, 0),
        };
        let initial_truncation = CaptureTruncation {
            omitted_bytes: omitted_request_id_bytes,
            ..CaptureTruncation::default()
        };
        Some(RequestCaptureSession {
            store: self.clone(),
            request_id,
            created_at: Utc::now(),
            api_key_id,
            user_id,
            downstream_protocol,
            is_stream,
            mode: auth.request_capture_mode,
            attempts: Arc::new(Mutex::new(Vec::new())),
            limits: self.limits,
            truncation: Arc::new(Mutex::new(initial_truncation)),
        })
    }

    pub fn spawn_cleanup_task(&self, runtime: Arc<RwLock<MonoizeRuntimeConfig>>) {
        let store = self.clone();
        tokio::spawn(async move {
            if let Err(err) = store.cleanup_expired(runtime.clone()).await {
                tracing::warn!("failed to cleanup request capture dumps at startup: {err}");
            }
            let mut interval = tokio::time::interval(std::time::Duration::from_secs(3600));
            interval.set_missed_tick_behavior(MissedTickBehavior::Delay);
            loop {
                interval.tick().await;
                if let Err(err) = store.cleanup_expired(runtime.clone()).await {
                    tracing::warn!("failed to cleanup request capture dumps: {err}");
                }
            }
        });
    }

    async fn cleanup_expired(
        &self,
        runtime: Arc<RwLock<MonoizeRuntimeConfig>>,
    ) -> Result<(), String> {
        let retention_days = runtime.read().await.request_capture_retention_days.max(1);
        // RCD-R6: metadata-row deletion failure must not stop file cleanup.
        if let Some(db) = self.db.as_ref() {
            let cutoff_unix_ms = (Utc::now()
                - chrono::Duration::days(i64::try_from(retention_days).unwrap_or(i64::MAX)))
            .timestamp_millis();
            if let Err(err) = db
                .write()
                .await
                .execute(db.stmt(
                    "DELETE FROM request_capture_records WHERE created_at_unix_ms < $1",
                    vec![cutoff_unix_ms.into()],
                ))
                .await
            {
                tracing::warn!("failed to cleanup request capture metadata rows: {err}");
            }
        }
        let dump_dir = self.dump_dir.clone();
        tokio::task::spawn_blocking(move || cleanup_expired_sync(&dump_dir, retention_days))
            .await
            .map_err(|err| err.to_string())?
    }

    /// RCV-A1: candidate records for one request id, newest first. When
    /// `user_id` is supplied only that owner's records are considered.
    pub async fn list_capture_records(
        &self,
        request_id: &str,
        user_id: Option<&str>,
    ) -> Result<Vec<CaptureRecordRow>, String> {
        let Some(db) = self.db.as_ref() else {
            return Ok(Vec::new());
        };
        let mut sql = "SELECT file_name, request_id, user_id, api_key_id, created_at, \
             created_at_unix_ms, size_bytes FROM request_capture_records WHERE request_id = $1"
            .to_string();
        let mut values: Vec<sea_orm::Value> = vec![request_id.into()];
        if let Some(user_id) = user_id {
            sql.push_str(" AND user_id = $2");
            values.push(user_id.into());
        }
        sql.push_str(" ORDER BY created_at_unix_ms DESC, file_name DESC");
        let rows = db
            .read()
            .query_all(db.stmt(&sql, values))
            .await
            .map_err(|err| err.to_string())?;
        rows.into_iter()
            .map(|row| {
                Ok(CaptureRecordRow {
                    file_name: row.try_get("", "file_name").map_err(|e| e.to_string())?,
                    request_id: row.try_get("", "request_id").map_err(|e| e.to_string())?,
                    user_id: row.try_get("", "user_id").map_err(|e| e.to_string())?,
                    api_key_id: row.try_get("", "api_key_id").map_err(|e| e.to_string())?,
                    created_at: row.try_get("", "created_at").map_err(|e| e.to_string())?,
                    created_at_unix_ms: row
                        .try_get("", "created_at_unix_ms")
                        .map_err(|e| e.to_string())?,
                    size_bytes: row.try_get("", "size_bytes").map_err(|e| e.to_string())?,
                })
            })
            .collect()
    }

    /// RCV-A3: on-demand dump read on a blocking-capable executor. Returns
    /// `Ok(None)` when the file no longer exists (RCV-A8 stale record).
    pub async fn read_dump_file(&self, file_name: &str) -> Result<Option<Vec<u8>>, String> {
        // Defense in depth: recorded file names never contain separators, but
        // reject any that would escape the dump directory.
        if file_name.contains('/') || file_name.contains('\\') || file_name.contains("..") {
            return Err("invalid capture dump file name".to_string());
        }
        let path = self.dump_dir.join(file_name);
        tokio::task::spawn_blocking(move || match std::fs::read(&path) {
            Ok(bytes) => Ok(Some(bytes)),
            Err(err) if err.kind() == std::io::ErrorKind::NotFound => Ok(None),
            Err(err) => Err(err.to_string()),
        })
        .await
        .map_err(|err| err.to_string())?
    }

    /// RCV-A8: drop a stale metadata row whose dump file no longer exists.
    pub async fn delete_capture_record(&self, file_name: &str) -> Result<(), String> {
        let Some(db) = self.db.as_ref() else {
            return Ok(());
        };
        db.write()
            .await
            .execute(db.stmt(
                "DELETE FROM request_capture_records WHERE file_name = $1",
                vec![file_name.into()],
            ))
            .await
            .map_err(|err| err.to_string())?;
        Ok(())
    }

    /// RCD-M3: upsert one metadata row immediately after a dump write.
    async fn insert_capture_record(&self, record: &CaptureRecordRow) -> Result<(), String> {
        let Some(db) = self.db.as_ref() else {
            return Ok(());
        };
        db.write()
            .await
            .execute(db.stmt(
                "INSERT INTO request_capture_records \
                 (file_name, request_id, user_id, api_key_id, created_at, created_at_unix_ms, size_bytes) \
                 VALUES ($1, $2, $3, $4, $5, $6, $7) \
                 ON CONFLICT (file_name) DO UPDATE SET \
                 request_id = excluded.request_id, user_id = excluded.user_id, \
                 api_key_id = excluded.api_key_id, created_at = excluded.created_at, \
                 created_at_unix_ms = excluded.created_at_unix_ms, size_bytes = excluded.size_bytes",
                vec![
                    record.file_name.as_str().into(),
                    record.request_id.as_str().into(),
                    record.user_id.as_str().into(),
                    record.api_key_id.as_str().into(),
                    record.created_at.as_str().into(),
                    record.created_at_unix_ms.into(),
                    record.size_bytes.into(),
                ],
            ))
            .await
            .map_err(|err| err.to_string())?;
        Ok(())
    }
}

impl RequestCaptureSession {
    pub(crate) async fn push_attempt(&self, mut attempt: Value) {
        let mut attempts = self.attempts.lock().await;
        let mut truncation = self.truncation.lock().await;
        if attempts.len() >= self.limits.max_attempts {
            truncation.omitted_attempts = truncation.omitted_attempts.saturating_add(1);
            truncation.omitted_frames = truncation.omitted_frames.saturating_add(
                attempt
                    .get("downstream_sse_frames")
                    .and_then(Value::as_array)
                    .map_or(0, Vec::len),
            );
            truncation.omitted_bytes = truncation
                .omitted_bytes
                .saturating_add(serialized_len(&attempt));
            return;
        }
        let mut additionally_omitted_frames = 0_usize;
        let mut additionally_omitted_bytes = 0_usize;
        let retained_frame_count = if let Some(frames) = attempt
            .get_mut("downstream_sse_frames")
            .and_then(Value::as_array_mut)
        {
            let available = self
                .limits
                .max_frames
                .saturating_sub(truncation.retained_frames);
            if frames.len() > available {
                let omitted = frames.split_off(available);
                additionally_omitted_frames = omitted.len();
                additionally_omitted_bytes = omitted
                    .iter()
                    .filter_map(Value::as_str)
                    .map(str::len)
                    .sum::<usize>();
            }
            frames.len()
        } else {
            0
        };
        if additionally_omitted_frames > 0
            && let Some(metadata) = attempt
                .get_mut("downstream_sse_frames_truncation")
                .and_then(Value::as_object_mut)
        {
            let prior_frames = metadata
                .get("omitted_frames")
                .and_then(Value::as_u64)
                .unwrap_or(0);
            let prior_bytes = metadata
                .get("omitted_bytes")
                .and_then(Value::as_u64)
                .unwrap_or(0);
            metadata.insert(
                "omitted_frames".to_string(),
                json!(prior_frames.saturating_add(additionally_omitted_frames as u64)),
            );
            metadata.insert(
                "omitted_bytes".to_string(),
                json!(prior_bytes.saturating_add(additionally_omitted_bytes as u64)),
            );
            metadata.insert("truncated".to_string(), Value::Bool(true));
        }
        if let Some(frame_meta) = attempt.get("downstream_sse_frames_truncation") {
            truncation.omitted_frames = truncation.omitted_frames.saturating_add(
                frame_meta
                    .get("omitted_frames")
                    .and_then(Value::as_u64)
                    .unwrap_or(0) as usize,
            );
            truncation.omitted_bytes = truncation.omitted_bytes.saturating_add(
                frame_meta
                    .get("omitted_bytes")
                    .and_then(Value::as_u64)
                    .unwrap_or(0) as usize,
            );
        }
        let attempt_bytes = serialized_len(&attempt);
        let remaining = self
            .limits
            .max_session_bytes
            .saturating_sub(truncation.retained_bytes);
        let retained_attempt = if attempt_bytes <= remaining {
            truncation.retained_frames = truncation
                .retained_frames
                .saturating_add(retained_frame_count);
            attempt
        } else {
            truncation.omitted_frames = truncation
                .omitted_frames
                .saturating_add(retained_frame_count);
            let placeholder = truncated_attempt_placeholder(&attempt, attempt_bytes);
            let placeholder_bytes = serialized_len(&placeholder);
            truncation.omitted_bytes = truncation
                .omitted_bytes
                .saturating_add(attempt_bytes.saturating_sub(placeholder_bytes.min(attempt_bytes)));
            if placeholder_bytes > remaining {
                truncation.omitted_attempts = truncation.omitted_attempts.saturating_add(1);
                truncation.omitted_bytes =
                    truncation.omitted_bytes.saturating_add(placeholder_bytes);
                return;
            }
            placeholder
        };
        truncation.retained_bytes = truncation
            .retained_bytes
            .saturating_add(serialized_len(&retained_attempt));
        attempts.push(retained_attempt);
    }

    pub(crate) async fn persist_with_result(
        &self,
        upstream_usage: Option<&crate::urp::Usage>,
        upstream_error_seen: bool,
    ) {
        let mut attempts = self.attempts.lock().await.clone();
        if attempts.is_empty() {
            return;
        }
        if !self
            .mode
            .should_persist(upstream_usage, upstream_error_seen)
        {
            return;
        }
        let mut truncation = self.truncation.lock().await.clone();
        let encoded = loop {
            let payload = json!({
                "version": 2,
                "request_id": self.request_id,
                "created_at": self.created_at.to_rfc3339_opts(SecondsFormat::Millis, true),
                "api_key_id": self.api_key_id,
                "user_id": self.user_id,
                "downstream_protocol": downstream_protocol_name(self.downstream_protocol),
                "is_stream": self.is_stream,
                "attempts": &attempts,
                "capture_truncation": truncation.to_json(),
            });
            let encoded = match serde_json::to_vec(&payload) {
                Ok(encoded) => encoded,
                Err(error) => {
                    tracing::warn!("failed to encode request capture dump: {error}");
                    return;
                }
            };
            if encoded.len() <= self.limits.max_session_bytes {
                break encoded;
            }
            if attempts.len() == 1 {
                let original_bytes = serialized_len(&attempts[0]);
                let placeholder = truncated_attempt_placeholder(&attempts[0], original_bytes);
                let placeholder_bytes = serialized_len(&placeholder);
                if placeholder_bytes >= original_bytes {
                    tracing::warn!(
                        max_session_bytes = self.limits.max_session_bytes,
                        "bounded request capture envelope exceeds configured session byte limit"
                    );
                    return;
                }
                attempts[0] = placeholder;
                truncation.omitted_bytes = truncation
                    .omitted_bytes
                    .saturating_add(original_bytes.saturating_sub(placeholder_bytes));
                truncation.retained_bytes = truncation
                    .retained_bytes
                    .saturating_sub(original_bytes)
                    .saturating_add(placeholder_bytes);
                continue;
            }
            let Some(removed) = attempts.pop() else {
                tracing::warn!(
                    max_session_bytes = self.limits.max_session_bytes,
                    "request capture envelope exceeds configured session byte limit"
                );
                return;
            };
            let removed_bytes = serialized_len(&removed);
            truncation.omitted_attempts = truncation.omitted_attempts.saturating_add(1);
            truncation.omitted_bytes = truncation.omitted_bytes.saturating_add(removed_bytes);
            truncation.retained_bytes = truncation.retained_bytes.saturating_sub(removed_bytes);
        };
        let size_bytes = encoded.len() as i64;
        let file_name = match self
            .store
            .write_dump(self.request_id.as_deref(), self.created_at, encoded)
            .await
        {
            Ok(file_name) => file_name,
            Err(err) => {
                tracing::warn!("failed to write request capture dump: {err}");
                return;
            }
        };
        // RCD-M3: only sessions with a request id get a metadata row; RCD-M4:
        // an insert failure keeps the dump file and the client response intact.
        if let Some(request_id) = self.request_id.as_deref() {
            let record = CaptureRecordRow {
                file_name,
                request_id: request_id.to_string(),
                user_id: self.user_id.clone(),
                api_key_id: self.api_key_id.clone(),
                created_at: self.created_at.to_rfc3339_opts(SecondsFormat::Millis, true),
                created_at_unix_ms: self.created_at.timestamp_millis(),
                size_bytes,
            };
            if let Err(err) = self.store.insert_capture_record(&record).await {
                tracing::warn!("failed to record request capture metadata: {err}");
            }
        }
    }
}

impl RequestCaptureStore {
    async fn write_dump(
        &self,
        request_id: Option<&str>,
        created_at: chrono::DateTime<Utc>,
        bytes: Vec<u8>,
    ) -> Result<String, String> {
        let dump_dir = self.dump_dir.clone();
        let prefix = request_id_prefix(request_id);
        let timestamp = created_at.format("%Y%m%dT%H%M%S%3fZ").to_string();
        let filename = format!("{prefix}_{timestamp}.json");
        let written_filename = filename.clone();
        tokio::task::spawn_blocking(move || {
            std::fs::create_dir_all(&*dump_dir).map_err(|err| err.to_string())?;
            let final_path = dump_dir.join(filename);
            let tmp_path = final_path.with_extension(format!(
                "json.tmp.{}",
                uuid::Uuid::new_v4().to_string().replace('-', "")
            ));
            std::fs::write(&tmp_path, bytes).map_err(|err| err.to_string())?;
            std::fs::rename(&tmp_path, &final_path).map_err(|err| err.to_string())?;
            Ok::<(), String>(())
        })
        .await
        .map_err(|err| err.to_string())??;
        Ok(written_filename)
    }
}

fn request_id_prefix(request_id: Option<&str>) -> String {
    let Some(request_id) = request_id.filter(|value| !value.is_empty()) else {
        return "unknown".to_string();
    };
    let sanitized: String = request_id
        .chars()
        .take(8)
        .map(|ch| {
            if ch.is_ascii_alphanumeric() || ch == '-' || ch == '_' {
                ch
            } else {
                '_'
            }
        })
        .collect();
    if sanitized.is_empty() {
        "unknown".to_string()
    } else {
        sanitized
    }
}

fn downstream_protocol_name(protocol: DownstreamProtocol) -> &'static str {
    match protocol {
        DownstreamProtocol::Responses => "responses",
        DownstreamProtocol::ChatCompletions => "chat_completions",
        DownstreamProtocol::AnthropicMessages => "anthropic_messages",
    }
}

fn provider_type_name(provider_type: ProviderType) -> &'static str {
    match provider_type {
        ProviderType::Responses => "responses",
        ProviderType::ChatCompletion => "chat_completion",
        ProviderType::Messages => "messages",
        ProviderType::Gemini => "gemini",
        ProviderType::OpenaiImage => "openai_image",
        ProviderType::Replicate => "replicate",
        ProviderType::Group => "group",
    }
}

fn cleanup_expired_sync(dump_dir: &Path, retention_days: u64) -> Result<(), String> {
    if !dump_dir.exists() {
        return Ok(());
    }
    let cutoff = std::time::SystemTime::now()
        .checked_sub(std::time::Duration::from_secs(
            retention_days.saturating_mul(86_400),
        ))
        .unwrap_or(std::time::UNIX_EPOCH);
    for entry in std::fs::read_dir(dump_dir).map_err(|err| err.to_string())? {
        let entry = entry.map_err(|err| err.to_string())?;
        let metadata = entry.metadata().map_err(|err| err.to_string())?;
        if !metadata.is_file() {
            continue;
        }
        let Ok(modified) = metadata.modified() else {
            continue;
        };
        if modified < cutoff {
            std::fs::remove_file(entry.path()).map_err(|err| err.to_string())?;
        }
    }
    Ok(())
}

fn data_dir_from_database_dsn(dsn: &str) -> PathBuf {
    sqlite_file_path_from_dsn(dsn)
        .and_then(|path| path.parent().map(Path::to_path_buf))
        .unwrap_or_else(|| PathBuf::from("./data"))
}

fn sqlite_file_path_from_dsn(dsn: &str) -> Option<PathBuf> {
    let raw = dsn.strip_prefix("sqlite://")?;
    if raw.contains(":memory:") || raw.starts_with(":memory:") || raw.contains("mode=memory") {
        return None;
    }
    let path_part = raw.split('?').next().unwrap_or(raw);
    if path_part.is_empty() {
        return None;
    }
    Some(PathBuf::from(path_part))
}

fn positive_env(name: &str, default: usize) -> usize {
    std::env::var(name)
        .ok()
        .and_then(|value| value.parse::<usize>().ok())
        .filter(|value| *value > 0)
        .unwrap_or(default)
}

fn serialized_len(value: &Value) -> usize {
    serde_json::to_vec(value)
        .map(|bytes| bytes.len())
        .unwrap_or(usize::MAX)
}

fn truncate_utf8(value: &str, max_bytes: usize) -> String {
    if value.len() <= max_bytes {
        return value.to_string();
    }
    let mut end = max_bytes.min(value.len());
    while end > 0 && !value.is_char_boundary(end) {
        end -= 1;
    }
    value[..end].to_string()
}

#[cfg(test)]
fn bound_frames(
    frames: Vec<String>,
    limits: RequestCaptureLimits,
) -> (Vec<String>, CaptureTruncation) {
    let mut retained = Vec::with_capacity(frames.len().min(limits.max_frames));
    let mut metadata = CaptureTruncation::default();
    for frame in frames {
        if retained.len() >= limits.max_frames
            || metadata.retained_bytes >= limits.max_session_bytes
        {
            metadata.omitted_frames = metadata.omitted_frames.saturating_add(1);
            metadata.omitted_bytes = metadata.omitted_bytes.saturating_add(frame.len());
            continue;
        }
        let max_bytes = limits.max_frame_bytes.min(
            limits
                .max_session_bytes
                .saturating_sub(metadata.retained_bytes),
        );
        let bounded = truncate_utf8(&frame, max_bytes);
        if bounded.len() < frame.len() {
            metadata.omitted_bytes = metadata
                .omitted_bytes
                .saturating_add(frame.len().saturating_sub(bounded.len()));
        }
        metadata.retained_bytes = metadata.retained_bytes.saturating_add(bounded.len());
        retained.push(bounded);
    }
    metadata.retained_frames = retained.len();
    (retained, metadata)
}

fn truncated_attempt_placeholder(attempt: &Value, original_bytes: usize) -> Value {
    let field = |name: &str| bounded_attempt_field(attempt, name);
    let prior_frame_metadata = attempt
        .get("downstream_sse_frames_truncation")
        .cloned()
        .unwrap_or_else(|| CaptureTruncation::default().to_json());
    let retained_frames = attempt
        .get("downstream_sse_frames")
        .and_then(Value::as_array);
    let additionally_omitted_frames = retained_frames.map_or(0, Vec::len);
    let additionally_omitted_bytes = retained_frames
        .into_iter()
        .flatten()
        .filter_map(Value::as_str)
        .map(str::len)
        .sum::<usize>();
    let prior_omitted_frames = prior_frame_metadata
        .get("omitted_frames")
        .and_then(Value::as_u64)
        .unwrap_or(0);
    let prior_omitted_bytes = prior_frame_metadata
        .get("omitted_bytes")
        .and_then(Value::as_u64)
        .unwrap_or(0);
    let frame_truncation = json!({
        "truncated": prior_omitted_frames > 0
            || prior_omitted_bytes > 0
            || additionally_omitted_frames > 0
            || additionally_omitted_bytes > 0,
        "omitted_attempts": 0,
        "omitted_frames": prior_omitted_frames.saturating_add(additionally_omitted_frames as u64),
        "omitted_bytes": prior_omitted_bytes.saturating_add(additionally_omitted_bytes as u64),
        "retained_bytes": 0,
        "retained_frames": 0,
    });
    json!({
        "attempt_number": field("attempt_number"),
        "provider_id": field("provider_id"),
        "channel_id": field("channel_id"),
        "provider_type": field("provider_type"),
        "logical_model": field("logical_model"),
        "upstream_model": field("upstream_model"),
        "upstream_path": field("upstream_path"),
        "raw_input": null,
        "transformed_urp_request": null,
        "upstream_request": null,
        "downstream_response": null,
        "downstream_sse_frames": null,
        "downstream_sse_frames_truncation": frame_truncation,
        "transform_chain": null,
        "error": bounded_attempt_error(attempt),
        "capture_truncation": {
            "truncated": true,
            "reason": "attempt_bytes",
            "original_bytes": original_bytes,
        }
    })
}

fn bounded_attempt_field(attempt: &Value, name: &str) -> Value {
    match attempt.get(name) {
        Some(Value::String(value)) => {
            Value::String(truncate_utf8(value, MAX_CAPTURE_IDENTIFIER_BYTES))
        }
        Some(value) => value.clone(),
        None => Value::Null,
    }
}

fn bounded_attempt_error(attempt: &Value) -> Value {
    let Some(error) = attempt.get("error").and_then(Value::as_object) else {
        return Value::Null;
    };
    let mut bounded = serde_json::Map::new();
    for key in ["code", "message", "status"] {
        if let Some(value) = error.get(key) {
            bounded.insert(
                key.to_string(),
                match value {
                    Value::String(value) => {
                        Value::String(truncate_utf8(value, MAX_CAPTURE_IDENTIFIER_BYTES))
                    }
                    value if value.is_number() || value.is_boolean() || value.is_null() => {
                        value.clone()
                    }
                    _ => Value::Null,
                },
            );
        }
    }
    Value::Object(bounded)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::auth::AuthResult;
    use crate::monoize_routing::MonoizeRuntimeConfig;
    use crate::users::UserRole;
    use tempfile::TempDir;
    use tokio::sync::RwLock;

    fn test_auth(request_capture_mode: RequestCaptureMode) -> AuthResult {
        AuthResult {
            tenant_id: "tenant-1".to_string(),
            user_id: Some("user-1".to_string()),
            username: None,
            user_role: UserRole::User,
            api_key_id: Some("key-1".to_string()),
            api_key_name: Some("test key".to_string()),
            max_multiplier: None,
            transforms: Vec::new(),
            model_redirects: Vec::new(),
            effective_groups: None,
            model_limits_enabled: false,
            model_limits: Vec::new(),
            ip_whitelist: Vec::new(),
            sub_account_enabled: false,
            sub_account_balance_nano: "0".to_string(),
            reasoning_envelope_enabled: true,
            request_capture_mode,
        }
    }

    #[test]
    fn default_dsn_maps_to_data_dumps() {
        let store = RequestCaptureStore::new("sqlite://./data/monoize.db");
        assert_eq!(store.dump_dir(), Path::new("./data/dumps"));
    }

    #[test]
    fn non_file_dsn_falls_back_to_default_data_dir() {
        let store = RequestCaptureStore::new("postgres://localhost/db");
        assert_eq!(store.dump_dir(), Path::new("./data/dumps"));
    }

    #[test]
    fn request_id_prefix_sanitizes_path_characters() {
        assert_eq!(request_id_prefix(Some("../evil42")), "___evil4");
        assert_eq!(request_id_prefix(Some("abc-DEF_1")), "abc-DEF_");
    }

    #[tokio::test]
    async fn maybe_start_session_requires_global_and_api_key_switches() {
        let store = RequestCaptureStore::new("sqlite://./data/monoize.db");
        let runtime = RwLock::new(MonoizeRuntimeConfig {
            request_capture_enabled: false,
            ..MonoizeRuntimeConfig::default()
        });
        assert!(
            store
                .maybe_start_session(
                    &runtime,
                    &test_auth(RequestCaptureMode::CaptureAll),
                    Some("req_12345678".to_string()),
                    DownstreamProtocol::Responses,
                    false,
                )
                .await
                .is_none()
        );

        let runtime = RwLock::new(MonoizeRuntimeConfig {
            request_capture_enabled: true,
            ..MonoizeRuntimeConfig::default()
        });
        assert!(
            store
                .maybe_start_session(
                    &runtime,
                    &test_auth(RequestCaptureMode::Off),
                    Some("req_12345678".to_string()),
                    DownstreamProtocol::Responses,
                    false,
                )
                .await
                .is_none()
        );

        assert!(
            store
                .maybe_start_session(
                    &runtime,
                    &test_auth(RequestCaptureMode::CaptureAll),
                    Some("req_12345678".to_string()),
                    DownstreamProtocol::Responses,
                    false,
                )
                .await
                .is_some()
        );
    }

    #[test]
    fn frames_are_bounded_on_utf8_boundaries_with_metadata() {
        let limits = RequestCaptureLimits {
            max_attempts: 2,
            max_frames: 2,
            max_frame_bytes: 5,
            max_session_bytes: 8,
        };
        let (frames, metadata) = bound_frames(
            vec!["ééé".to_string(), "abcd".to_string(), "omitted".to_string()],
            limits,
        );
        assert_eq!(frames, vec!["éé".to_string(), "abcd".to_string()]);
        assert_eq!(metadata.retained_bytes, 8);
        assert_eq!(metadata.omitted_frames, 1);
        assert_eq!(metadata.omitted_bytes, 9);
    }

    #[tokio::test]
    async fn spawned_children_share_one_sse_byte_quota() {
        let active = SseFrameCapture::with_limits(RequestCaptureLimits {
            max_attempts: 1,
            max_frames: 8,
            max_frame_bytes: 8,
            max_session_bytes: 8,
        });

        CURRENT_SSE_CAPTURE
            .scope(active.clone(), async {
                let first = spawn_with_sse_capture(async {
                    capture_sse_frame("aaaaaaaa".to_string()).await;
                });
                let second = spawn_with_sse_capture(async {
                    capture_sse_frame("bbbbbbbb".to_string()).await;
                });
                first.await.unwrap();
                second.await.unwrap();
            })
            .await;

        let captured = active.snapshot().await;
        assert_eq!(captured.frames.len(), 1);
        assert_eq!(captured.frames.iter().map(String::len).sum::<usize>(), 8);
        assert_eq!(captured.truncation.omitted_frames, 1);
        assert_eq!(captured.truncation.omitted_bytes, 8);
    }

    #[tokio::test]
    async fn persisting_a_dump_records_capture_metadata_reachable_through_store_queries() {
        use crate::migration::Migrator;
        use sea_orm_migration::MigratorTrait;

        let temp = TempDir::new().expect("temporary directory");
        let db = crate::db::DbPool::connect("sqlite::memory:")
            .await
            .expect("db connects");
        {
            let write = db.write().await;
            Migrator::up(&*write, None).await.expect("migrates");
        }
        let store = RequestCaptureStore {
            dump_dir: Arc::new(temp.path().join("dumps")),
            limits: RequestCaptureLimits::from_env(),
            db: Some(db.clone()),
        };
        let runtime = RwLock::new(MonoizeRuntimeConfig {
            request_capture_enabled: true,
            ..MonoizeRuntimeConfig::default()
        });
        let session = store
            .maybe_start_session(
                &runtime,
                &test_auth(RequestCaptureMode::CaptureAll),
                Some("req_meta_1".to_string()),
                DownstreamProtocol::Responses,
                false,
            )
            .await
            .expect("capture starts");

        let chain = build_transform_chain(
            &[],
            &[serde_json::from_value(json!({
                "transform": "force_stream",
                "phase": "request",
                "config": {}
            }))
            .expect("rule parses")],
            &[],
            "gpt-test",
        );
        session
            .push_attempt(json!({
                "attempt_number": 1,
                "provider_id": "prov-1",
                "channel_id": "ch-1",
                "provider_type": "responses",
                "logical_model": "gpt-test",
                "upstream_model": "gpt-test",
                "upstream_path": "/v1/responses",
                "raw_input": {"input": "hi"},
                "transformed_urp_request": {},
                "upstream_request": {},
                "downstream_response": {"ok": true},
                "downstream_sse_frames": null,
                "downstream_sse_frames_truncation": CaptureTruncation::default().to_json(),
                "transform_chain": chain,
                "error": null
            }))
            .await;
        session.persist_with_result(None, false).await;

        let records = store
            .list_capture_records("req_meta_1", None)
            .await
            .expect("records query succeeds");
        assert_eq!(records.len(), 1);
        let record = &records[0];
        assert_eq!(record.request_id, "req_meta_1");
        assert_eq!(record.user_id, "user-1");
        assert_eq!(record.api_key_id, "key-1");
        assert!(record.size_bytes > 0);

        let bytes = store
            .read_dump_file(&record.file_name)
            .await
            .expect("dump read succeeds")
            .expect("dump exists");
        assert_eq!(bytes.len() as i64, record.size_bytes);
        let payload: Value = serde_json::from_slice(&bytes).expect("dump is JSON");
        assert_eq!(payload["version"], 2);
        assert_eq!(
            payload["attempts"][0]["transform_chain"],
            json!([{"scope": "global", "transform": "stream_force", "phase": "request"}])
        );

        // Owner filter excludes non-matching users.
        assert!(
            store
                .list_capture_records("req_meta_1", Some("someone-else"))
                .await
                .expect("filtered query succeeds")
                .is_empty()
        );

        // RCV-A8 support: a missing dump reads as None and the stale row can
        // be deleted.
        std::fs::remove_file(store.dump_dir().join(&record.file_name)).expect("file removed");
        assert!(
            store
                .read_dump_file(&record.file_name)
                .await
                .expect("read succeeds")
                .is_none()
        );
        store
            .delete_capture_record(&record.file_name)
            .await
            .expect("record deleted");
        assert!(
            store
                .list_capture_records("req_meta_1", None)
                .await
                .expect("records query succeeds")
                .is_empty()
        );
    }

    #[tokio::test]
    async fn cleanup_deletes_expired_metadata_rows_with_dump_files() {
        use crate::migration::Migrator;
        use sea_orm::ConnectionTrait;
        use sea_orm_migration::MigratorTrait;

        let temp = TempDir::new().expect("temporary directory");
        let db = crate::db::DbPool::connect("sqlite::memory:")
            .await
            .expect("db connects");
        {
            let write = db.write().await;
            Migrator::up(&*write, None).await.expect("migrates");
        }
        let store = RequestCaptureStore {
            dump_dir: Arc::new(temp.path().join("dumps")),
            limits: RequestCaptureLimits::from_env(),
            db: Some(db.clone()),
        };
        let old_ms = (Utc::now() - chrono::Duration::days(365)).timestamp_millis();
        let fresh_ms = Utc::now().timestamp_millis();
        for (file_name, created_ms) in [("old.json", old_ms), ("fresh.json", fresh_ms)] {
            db.write()
                .await
                .execute(db.stmt(
                    "INSERT INTO request_capture_records (file_name, request_id, user_id, api_key_id, created_at, created_at_unix_ms, size_bytes) VALUES ($1, 'req-x', 'user-1', 'key-1', '2026-01-01T00:00:00Z', $2, 1)",
                    vec![file_name.into(), created_ms.into()],
                ))
                .await
                .expect("row inserted");
        }
        let runtime = Arc::new(RwLock::new(MonoizeRuntimeConfig {
            request_capture_enabled: true,
            request_capture_retention_days: 7,
            ..MonoizeRuntimeConfig::default()
        }));
        store
            .cleanup_expired(runtime)
            .await
            .expect("cleanup succeeds");
        let remaining = store
            .list_capture_records("req-x", None)
            .await
            .expect("records query succeeds");
        assert_eq!(remaining.len(), 1);
        assert_eq!(remaining[0].file_name, "fresh.json");
    }

    #[tokio::test]
    async fn persisted_dump_uses_the_checked_compact_bytes_and_retains_a_placeholder() {
        let temp = TempDir::new().expect("temporary directory");
        let limits = RequestCaptureLimits {
            max_attempts: 1,
            max_frames: 8,
            max_frame_bytes: 1024,
            max_session_bytes: MIN_SESSION_BYTES,
        };
        let store = RequestCaptureStore {
            dump_dir: Arc::new(temp.path().join("dumps")),
            limits,
            db: None,
        };
        let runtime = RwLock::new(MonoizeRuntimeConfig {
            request_capture_enabled: true,
            ..MonoizeRuntimeConfig::default()
        });
        let session = store
            .maybe_start_session(
                &runtime,
                &test_auth(RequestCaptureMode::CaptureAll),
                Some("request-id".repeat(100)),
                DownstreamProtocol::Responses,
                true,
            )
            .await
            .expect("capture starts");
        session
            .push_attempt(json!({
                "attempt_number": 1,
                "provider_id": "provider".repeat(100),
                "channel_id": "channel".repeat(100),
                "provider_type": "responses",
                "logical_model": "model".repeat(100),
                "upstream_model": "upstream".repeat(100),
                "upstream_path": "/v1/responses",
                "raw_input": "x".repeat(MIN_SESSION_BYTES * 2),
                "transformed_urp_request": null,
                "upstream_request": null,
                "downstream_response": null,
                "downstream_sse_frames": ["retained-frame"],
                "downstream_sse_frames_truncation": {
                    "truncated": true,
                    "omitted_attempts": 0,
                    "omitted_frames": 2,
                    "omitted_bytes": 7,
                    "retained_bytes": 14,
                    "retained_frames": 1
                },
                "error": null
            }))
            .await;
        session.persist_with_result(None, false).await;

        let path = std::fs::read_dir(store.dump_dir())
            .expect("dump directory exists")
            .next()
            .expect("one dump exists")
            .expect("dump entry reads")
            .path();
        let bytes = std::fs::read(path).expect("dump reads");
        assert!(bytes.len() <= MIN_SESSION_BYTES);
        let payload: Value = serde_json::from_slice(&bytes).expect("dump is JSON");
        assert_eq!(bytes, serde_json::to_vec(&payload).unwrap());
        assert_eq!(payload["attempts"].as_array().unwrap().len(), 1);
        assert!(
            payload["capture_truncation"]["truncated"]
                .as_bool()
                .unwrap()
        );
        let frame_meta = &payload["attempts"][0]["downstream_sse_frames_truncation"];
        assert_eq!(frame_meta["omitted_frames"], 3);
        assert_eq!(frame_meta["omitted_bytes"], 21);
        assert_eq!(frame_meta["retained_frames"], 0);
        assert_eq!(frame_meta["retained_bytes"], 0);
    }
}
