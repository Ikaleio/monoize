use crate::auth::AuthResult;
use crate::config::ProviderType;
use crate::db::DbPool;
use crate::handlers::DownstreamProtocol;
use crate::monoize_routing::MonoizeRuntimeConfig;
use crate::transforms::TransformRuleConfig;
use crate::users::{RequestCaptureMode, RequestCaptureRetention};
use chrono::{SecondsFormat, Utc};
use sea_orm::ConnectionTrait;
use serde_json::{Value, json};
use std::path::{Path, PathBuf};
use std::sync::Arc;
use tokio::sync::{Mutex, RwLock};
use tokio::task::JoinHandle;
use tokio::time::MissedTickBehavior;

mod session;
mod store;

const DEFAULT_MAX_ATTEMPTS: usize = 16;
const DEFAULT_MAX_FRAMES: usize = 4_096;
const DEFAULT_MAX_FRAME_BYTES: usize = 256 * 1024;
const DEFAULT_MAX_SESSION_BYTES: usize = 16 * 1024 * 1024;
const MIN_SESSION_BYTES: usize = 8 * 1024;
const MAX_CAPTURE_IDENTIFIER_BYTES: usize = 256;
/// RCD-Z1: dumps are zstd frames at this compression level.
const ZSTD_COMPRESSION_LEVEL: i32 = 3;
/// RCD-Z1/RCD-Z6: zstd frame magic number used for read-path format detection.
const ZSTD_MAGIC: [u8; 4] = [0x28, 0xB5, 0x2F, 0xFD];
/// RCD-R5a: fixed deletion horizon for dump files without a metadata row.
const ORPHAN_FILE_HORIZON_SECS: u64 = 86_400;
/// RCD-R3/RCD-R5a/RCD-R7: bound for dynamic `IN` lists and victim batches.
const CLEANUP_BATCH_SIZE: usize = 400;

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
}

tokio::task_local! {
    static CURRENT_SSE_CAPTURE: SseFrameCapture;
}

pub(crate) fn current_sse_capture() -> Option<SseFrameCapture> {
    CURRENT_SSE_CAPTURE.try_with(Clone::clone).ok()
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
    let capture = current_sse_capture();
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
    runtime: Option<Arc<RwLock<MonoizeRuntimeConfig>>>,
    ready: tokio::sync::broadcast::Sender<CaptureReady>,
}

#[derive(Clone, Debug, serde::Serialize)]
pub struct CaptureReady {
    pub request_id: String,
    pub user_id: String,
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
    pub expires_at_unix_ms: i64,
}

/// Dump read failure classification (RCD-Z7 / RCV-A9): `Unreadable` marks a
/// zstd-prefixed file that cannot be decompressed within bounds, which the
/// capture detail API maps to `capture_dump_unreadable`; `Io` is any other
/// filesystem failure and maps to `internal_error`.
#[derive(Debug)]
pub enum DumpReadError {
    Io(String),
    Unreadable(String),
}

impl std::fmt::Display for DumpReadError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Io(message) | Self::Unreadable(message) => f.write_str(message),
        }
    }
}

/// RCD-D2b: the `downstream_protocol` value recorded in a dump. Image API
/// ingress is not a `DownstreamProtocol` (its handlers reuse the Responses
/// forwarding pipeline internally), so capture keeps its own protocol tag.
#[derive(Clone, Copy, Debug)]
pub(crate) enum CaptureDownstreamProtocol {
    Responses,
    ChatCompletions,
    AnthropicMessages,
    ImageGenerations,
    ImageEdits,
}

impl From<DownstreamProtocol> for CaptureDownstreamProtocol {
    fn from(protocol: DownstreamProtocol) -> Self {
        match protocol {
            DownstreamProtocol::Responses => Self::Responses,
            DownstreamProtocol::ChatCompletions => Self::ChatCompletions,
            DownstreamProtocol::AnthropicMessages => Self::AnthropicMessages,
        }
    }
}

#[derive(Clone)]
pub(crate) struct RequestCaptureSession {
    store: RequestCaptureStore,
    request_id: Option<String>,
    created_at: chrono::DateTime<Utc>,
    api_key_id: String,
    user_id: String,
    downstream_protocol: CaptureDownstreamProtocol,
    is_stream: bool,
    mode: RequestCaptureMode,
    retention: RequestCaptureRetention,
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

/// One part of a captured `multipart/form-data` body (RCD-D16). Ingress
/// captures (RCD-D4a) may lack a filename or a wire `Content-Type`; upstream
/// captures (RCD-D6a) always carry both because the encoder sets them.
pub(crate) enum CapturedMultipartPart {
    Text {
        name: String,
        text: String,
    },
    File {
        name: String,
        filename: Option<String>,
        content_type: Option<String>,
        data_base64: String,
        byte_length: usize,
    },
}

/// RCD-D16: serialize captured multipart parts into the multipart capture
/// object recorded in `raw_input` (RCD-D4a) or `upstream_request` (RCD-D6a).
pub(crate) fn multipart_capture_object(parts: &[CapturedMultipartPart]) -> Value {
    let parts: Vec<Value> = parts
        .iter()
        .map(|part| match part {
            CapturedMultipartPart::Text { name, text } => json!({
                "name": name,
                "text": text,
            }),
            CapturedMultipartPart::File {
                name,
                filename,
                content_type,
                data_base64,
                byte_length,
            } => json!({
                "name": name,
                "filename": filename,
                "part_content_type": content_type,
                "byte_length": byte_length,
                "data_base64": data_base64,
            }),
        })
        .collect();
    json!({
        "content_type": "multipart/form-data",
        "parts": parts,
    })
}

/// RCD-D6a/OIU-E5g: build the multipart capture object for a sent
/// `openai_image` edit form from the exact fields the form was built from,
/// so the capture can never diverge from the wire body.
pub(crate) fn multipart_capture_object_from_upstream_fields(
    fields: &[crate::urp::encode::openai_image::MultipartField],
) -> Value {
    use crate::urp::encode::openai_image::MultipartField;
    use base64::Engine as _;
    let parts: Vec<CapturedMultipartPart> = fields
        .iter()
        .map(|field| match field {
            MultipartField::Text { name, value } => CapturedMultipartPart::Text {
                name: name.clone(),
                text: value.clone(),
            },
            MultipartField::File {
                name,
                filename,
                content_type,
                bytes,
            } => CapturedMultipartPart::File {
                name: name.clone(),
                filename: Some(filename.clone()),
                content_type: Some(content_type.clone()),
                data_base64: base64::engine::general_purpose::STANDARD.encode(bytes),
                byte_length: bytes.len(),
            },
        })
        .collect();
    multipart_capture_object(&parts)
}

/// RCD-D10c: serialize a stream-collected URP response in the RCD-D10a shape
/// `{finish_reason?, usage?, output, ...extra_body}`. The collected
/// `UrpResponse` keeps the terminal event's `extra_body` unchanged, so this
/// reproduces the reconstruction a pass-through stream tap would have stored.
pub(crate) fn reconstructed_response_json(resp: &crate::urp::UrpResponse) -> Value {
    let mut object = serde_json::Map::new();
    if let Some(finish_reason) = &resp.finish_reason {
        object.insert("finish_reason".to_string(), json!(finish_reason));
    }
    if let Some(usage) = &resp.usage {
        object.insert("usage".to_string(), json!(usage));
    }
    object.insert("output".to_string(), json!(resp.output));
    for (key, value) in &resp.extra_body {
        object.insert(key.clone(), value.clone());
    }
    Value::Object(object)
}

pub(crate) struct AttemptDumpInput<'a> {
    pub(crate) attempt_number: u32,
    pub(crate) provider_id: &'a str,
    pub(crate) channel_id: Option<&'a str>,
    pub(crate) provider_type: ProviderType,
    pub(crate) logical_model: &'a str,
    pub(crate) upstream_model: &'a str,
    pub(crate) upstream_path: &'a str,
    pub(crate) raw_input: Value,
    pub(crate) transformed_urp_request: &'a crate::urp::UrpRequest,
    pub(crate) upstream_request: Value,
    pub(crate) downstream_response: Option<Value>,
    pub(crate) reconstructed_urp_response: Option<Value>,
    pub(crate) downstream_sse_frames: Option<CapturedSseFrames>,
    pub(crate) transform_chain: Value,
    pub(crate) error: Option<Value>,
}

pub(crate) fn build_attempt_dump(input: AttemptDumpInput<'_>) -> Value {
    let AttemptDumpInput {
        attempt_number,
        provider_id,
        channel_id,
        provider_type,
        logical_model,
        upstream_model,
        upstream_path,
        raw_input,
        transformed_urp_request,
        upstream_request,
        downstream_response,
        reconstructed_urp_response,
        downstream_sse_frames,
        transform_chain,
        error,
    } = input;
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
        "reconstructed_urp_response": reconstructed_urp_response,
        "downstream_sse_frames": downstream_sse_frames,
        "downstream_sse_frames_truncation": frame_truncation.to_json(),
        "transform_chain": transform_chain,
        "error": error,
    })
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

fn downstream_protocol_name(protocol: CaptureDownstreamProtocol) -> &'static str {
    match protocol {
        CaptureDownstreamProtocol::Responses => "responses",
        CaptureDownstreamProtocol::ChatCompletions => "chat_completions",
        CaptureDownstreamProtocol::AnthropicMessages => "anthropic_messages",
        CaptureDownstreamProtocol::ImageGenerations => "image_generations",
        CaptureDownstreamProtocol::ImageEdits => "image_edits",
    }
}

fn provider_type_name(provider_type: ProviderType) -> &'static str {
    match provider_type {
        ProviderType::Responses => "responses",
        ProviderType::ChatCompletion => "chat_completion",
        ProviderType::Messages => "messages",
        ProviderType::Gemini => "gemini",
        ProviderType::OpenaiImage => "openai_image",
        ProviderType::OpenrouterImage => "openrouter_image",
        ProviderType::Replicate => "replicate",
        ProviderType::SystemOne => "system_one",
        ProviderType::OpenaiDecisions => "openai_decisions",
        ProviderType::Group => "group",
    }
}

/// RCD-R5/RCD-R5a: direct regular files with a modification time older than
/// `horizon`. Subdirectories are never entered; entries without a valid UTF-8
/// name cannot match a metadata row and are skipped.
fn list_files_older_than(
    dump_dir: &Path,
    horizon: std::time::SystemTime,
) -> Result<Vec<String>, String> {
    if !dump_dir.exists() {
        return Ok(Vec::new());
    }
    let mut names = Vec::new();
    for entry in std::fs::read_dir(dump_dir).map_err(|err| err.to_string())? {
        let entry = entry.map_err(|err| err.to_string())?;
        let metadata = entry.metadata().map_err(|err| err.to_string())?;
        if !metadata.is_file() {
            continue;
        }
        let Ok(modified) = metadata.modified() else {
            continue;
        };
        if modified >= horizon {
            continue;
        }
        if let Ok(name) = entry.file_name().into_string() {
            names.push(name);
        }
    }
    Ok(names)
}

/// RCD-Z7: bounded zstd decompression. Fails when the frame is invalid or
/// its decompressed size exceeds `max_bytes`.
fn decompress_bounded(bytes: &[u8], max_bytes: usize) -> Result<Vec<u8>, String> {
    use std::io::Read;
    let decoder = zstd::stream::read::Decoder::new(bytes)
        .map_err(|err| format!("invalid zstd frame: {err}"))?;
    let mut out = Vec::new();
    // Reading one byte past the bound distinguishes "exactly at the bound"
    // from "over the bound" without decompressing the whole oversized frame.
    decoder
        .take(max_bytes as u64 + 1)
        .read_to_end(&mut out)
        .map_err(|err| format!("zstd decompression failed: {err}"))?;
    if out.len() > max_bytes {
        return Err(format!(
            "decompressed capture dump exceeds the {max_bytes}-byte session bound"
        ));
    }
    Ok(out)
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
        "reconstructed_urp_response": null,
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
