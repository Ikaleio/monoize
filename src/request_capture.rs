use crate::auth::AuthResult;
use crate::config::ProviderType;
use crate::handlers::DownstreamProtocol;
use crate::monoize_routing::MonoizeRuntimeConfig;
use chrono::{SecondsFormat, Utc};
use serde_json::{Value, json};
use std::path::{Path, PathBuf};
use std::sync::Arc;
use tokio::sync::{Mutex, RwLock};
use tokio::task::JoinHandle;
use tokio::time::MissedTickBehavior;

pub(crate) type SseFrameCapture = Arc<Mutex<Vec<String>>>;

tokio::task_local! {
    static CURRENT_SSE_CAPTURE: SseFrameCapture;
}

pub(crate) async fn capture_sse_frame(frame: String) {
    if let Ok(capture) = CURRENT_SSE_CAPTURE.try_with(Clone::clone) {
        capture.lock().await.push(frame);
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
            with_sse_capture(capture, future).await
        } else {
            future.await
        }
    })
}

#[derive(Clone)]
pub struct RequestCaptureStore {
    dump_dir: Arc<PathBuf>,
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
    attempts: Arc<Mutex<Vec<Value>>>,
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
    downstream_sse_frames: Option<Vec<String>>,
    error: Option<Value>,
) -> Value {
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
        "error": error,
    })
}

impl RequestCaptureStore {
    pub fn new(database_dsn: &str) -> Self {
        Self {
            dump_dir: Arc::new(data_dir_from_database_dsn(database_dsn).join("dumps")),
        }
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
        if !rt.request_capture_enabled || !auth.request_capture_enabled {
            return None;
        }
        let api_key_id = auth.api_key_id.clone()?;
        let user_id = auth.user_id.clone()?;
        Some(RequestCaptureSession {
            store: self.clone(),
            request_id,
            created_at: Utc::now(),
            api_key_id,
            user_id,
            downstream_protocol,
            is_stream,
            attempts: Arc::new(Mutex::new(Vec::new())),
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
        let dump_dir = self.dump_dir.clone();
        tokio::task::spawn_blocking(move || cleanup_expired_sync(&dump_dir, retention_days))
            .await
            .map_err(|err| err.to_string())?
    }
}

impl RequestCaptureSession {
    pub(crate) async fn push_attempt(&self, attempt: Value) {
        self.attempts.lock().await.push(attempt);
    }

    pub(crate) async fn persist(&self) {
        let attempts = self.attempts.lock().await.clone();
        if attempts.is_empty() {
            return;
        }
        let payload = json!({
            "version": 1,
            "request_id": self.request_id,
            "created_at": self.created_at.to_rfc3339_opts(SecondsFormat::Millis, true),
            "api_key_id": self.api_key_id,
            "user_id": self.user_id,
            "downstream_protocol": downstream_protocol_name(self.downstream_protocol),
            "is_stream": self.is_stream,
            "attempts": attempts,
        });
        if let Err(err) = self
            .store
            .write_dump(self.request_id.as_deref(), self.created_at, payload)
            .await
        {
            tracing::warn!("failed to write request capture dump: {err}");
        }
    }
}

impl RequestCaptureStore {
    async fn write_dump(
        &self,
        request_id: Option<&str>,
        created_at: chrono::DateTime<Utc>,
        payload: Value,
    ) -> Result<(), String> {
        let dump_dir = self.dump_dir.clone();
        let prefix = request_id_prefix(request_id);
        let timestamp = created_at.format("%Y%m%dT%H%M%S%3fZ").to_string();
        let filename = format!("{prefix}_{timestamp}.json");
        tokio::task::spawn_blocking(move || {
            std::fs::create_dir_all(&*dump_dir).map_err(|err| err.to_string())?;
            let final_path = dump_dir.join(filename);
            let tmp_path = final_path.with_extension(format!(
                "json.tmp.{}",
                uuid::Uuid::new_v4().to_string().replace('-', "")
            ));
            let bytes = serde_json::to_vec_pretty(&payload).map_err(|err| err.to_string())?;
            std::fs::write(&tmp_path, bytes).map_err(|err| err.to_string())?;
            std::fs::rename(&tmp_path, &final_path).map_err(|err| err.to_string())?;
            Ok::<(), String>(())
        })
        .await
        .map_err(|err| err.to_string())?
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

#[cfg(test)]
mod tests {
    use super::*;
    use crate::auth::AuthResult;
    use crate::monoize_routing::MonoizeRuntimeConfig;
    use crate::users::UserRole;
    use tokio::sync::RwLock;

    fn test_auth(request_capture_enabled: bool) -> AuthResult {
        AuthResult {
            tenant_id: "tenant-1".to_string(),
            user_id: Some("user-1".to_string()),
            username: None,
            user_role: UserRole::User,
            api_key_id: Some("key-1".to_string()),
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
            request_capture_enabled,
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
                    &test_auth(true),
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
                    &test_auth(false),
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
                    &test_auth(true),
                    Some("req_12345678".to_string()),
                    DownstreamProtocol::Responses,
                    false,
                )
                .await
                .is_some()
        );
    }
}
