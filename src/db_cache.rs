use chrono::{DateTime, Utc};
use dashmap::DashMap;
use std::sync::{
    Arc,
    atomic::{AtomicU64, Ordering},
};
use std::time::{Duration, Instant};
use tokio::sync::{Mutex, Notify};

use crate::db::DbPool;
use crate::users::{InsertRequestLog, REQUEST_LOG_STATUS_PENDING, UserBalance};

// ---------------------------------------------------------------------------
// LastUsedBatcher: buffers api_key last_used timestamps, flushes periodically
// ---------------------------------------------------------------------------

#[derive(Debug, Clone)]
pub struct LastUsedBatcher {
    buffer: Arc<DashMap<String, DateTime<Utc>>>,
    capacity: usize,
    record_lock: Arc<std::sync::Mutex<()>>,
    flush_chunk_entries: usize,
}

impl Default for LastUsedBatcher {
    fn default() -> Self {
        Self::new()
    }
}

impl LastUsedBatcher {
    pub fn new() -> Self {
        Self::with_capacity(positive_env_usize(
            "MONOIZE_LAST_USED_BUFFER_ENTRIES",
            10_000,
        ))
    }

    pub fn with_capacity(capacity: usize) -> Self {
        Self::with_limits(
            capacity,
            positive_env_usize("MONOIZE_LAST_USED_FLUSH_CHUNK_ENTRIES", 256),
        )
    }

    pub fn with_limits(capacity: usize, flush_chunk_entries: usize) -> Self {
        Self {
            buffer: Arc::new(DashMap::new()),
            capacity: capacity.max(1),
            record_lock: Arc::new(std::sync::Mutex::new(())),
            flush_chunk_entries: flush_chunk_entries.clamp(1, 400),
        }
    }

    pub fn record(&self, api_key_id: String, now: DateTime<Utc>) {
        let _guard = self
            .record_lock
            .lock()
            .unwrap_or_else(|err| err.into_inner());
        if let Some(mut existing) = self.buffer.get_mut(&api_key_id) {
            if now > *existing {
                *existing = now;
            }
            return;
        }
        if self.buffer.len() >= self.capacity {
            tracing::warn!(
                capacity = self.capacity,
                "last_used buffer is full; omitting new key metadata"
            );
            return;
        }
        self.buffer.insert(api_key_id, now);
    }

    /// Drain all buffered entries and flush them to DB in a single write lock acquisition.
    pub async fn flush(&self, db: &DbPool) {
        let entries: Vec<(String, DateTime<Utc>)> = {
            let mut drained = Vec::new();
            self.buffer.retain(|k, v| {
                drained.push((k.clone(), *v));
                false
            });
            drained
        };
        if entries.is_empty() {
            return;
        }
        let write = db.write().await;
        use sea_orm::ConnectionTrait;
        let mut failed = Vec::new();
        for chunk in entries.chunks(self.flush_chunk_entries) {
            let (sql, values) = last_used_bulk_update(chunk);
            if let Err(error) = write.execute(db.stmt(&sql, values)).await {
                tracing::warn!(
                    entries = chunk.len(),
                    "last_used_batcher bulk flush error: {error}"
                );
                failed.extend_from_slice(chunk);
            }
        }
        drop(write);
        for (id, timestamp) in failed {
            self.record_retry(id, timestamp);
        }
    }

    /// Spawn background task that flushes every `interval`.
    pub fn spawn_flush_task(self, db: DbPool, interval: Duration) -> tokio::task::JoinHandle<()> {
        tokio::spawn(async move {
            let mut ticker = tokio::time::interval(interval);
            ticker.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Delay);
            loop {
                ticker.tick().await;
                self.flush(&db).await;
            }
        })
    }

    /// Replica shipment path (PRP12): atomically drain all buffered entries without
    /// touching the database. Callers re-insert via `record_retry` on delivery failure.
    pub fn drain(&self) -> Vec<(String, DateTime<Utc>)> {
        self.drain_limit(usize::MAX)
    }

    pub fn drain_limit(&self, max: usize) -> Vec<(String, DateTime<Utc>)> {
        let mut drained = Vec::new();
        if max == 0 {
            return drained;
        }
        self.buffer.retain(|k, v| {
            if drained.len() >= max {
                return true;
            }
            drained.push((k.clone(), *v));
            false
        });
        drained
    }

    pub(crate) fn record_retry(&self, api_key_id: String, timestamp: DateTime<Utc>) {
        let _guard = self
            .record_lock
            .lock()
            .unwrap_or_else(|error| error.into_inner());
        if let Some(mut existing) = self.buffer.get_mut(&api_key_id) {
            if timestamp > *existing {
                *existing = timestamp;
            }
            return;
        }
        if self.buffer.len() >= self.capacity
            && let Some(eviction_key) = self.buffer.iter().next().map(|entry| entry.key().clone())
        {
            self.buffer.remove(&eviction_key);
            tracing::warn!(
                api_key_id = %eviction_key,
                "last_used buffer evicted metadata to retain a failed write for retry"
            );
        }
        self.buffer.insert(api_key_id, timestamp);
    }
}

pub(crate) fn last_used_bulk_update(
    entries: &[(String, DateTime<Utc>)],
) -> (String, Vec<sea_orm::Value>) {
    let mut sql = String::from("UPDATE api_keys SET last_used_at = CASE");
    let mut values = Vec::with_capacity(entries.len().saturating_mul(2));
    let mut id_placeholders = Vec::with_capacity(entries.len());
    for (index, (id, timestamp)) in entries.iter().enumerate() {
        let id_param = index.saturating_mul(2).saturating_add(1);
        let timestamp_param = id_param.saturating_add(1);
        sql.push_str(&format!(" WHEN id = ${id_param} THEN ${timestamp_param}"));
        id_placeholders.push(format!("${id_param}"));
        values.push(id.clone().into());
        values.push(timestamp.to_rfc3339().into());
    }
    sql.push_str(" ELSE last_used_at END WHERE id IN (");
    sql.push_str(&id_placeholders.join(", "));
    sql.push(')');
    (sql, values)
}

// ---------------------------------------------------------------------------
// RequestLogBatcher: buffers InsertRequestLog entries, flushes as batch INSERT
// ---------------------------------------------------------------------------

const REQUEST_LOG_INSERT_COLUMNS: usize = 38;
pub(crate) const REQUEST_LOG_INSERT_CHUNK_ENTRIES: usize = 20;
pub(crate) const REQUEST_LOG_MIN_ENTRY_BYTES: u64 = 4_096;
const REQUEST_LOG_RETRY_INITIAL_DELAY: Duration = Duration::from_millis(10);
const REQUEST_LOG_RETRY_MAX_DELAY: Duration = Duration::from_millis(1_000);
const REQUEST_LOG_RESERVATION_UNARMED: u8 = 0;
const REQUEST_LOG_RESERVATION_ARMING: u8 = 1;
const REQUEST_LOG_RESERVATION_ARMED: u8 = 2;
const REQUEST_LOG_RESERVATION_CLAIMED: u8 = 3;
const REQUEST_LOG_RESERVATION_CONSUMED: u8 = 4;
const REQUEST_LOG_RESERVATION_CANCELING: u8 = 5;
const REQUEST_LOG_UNARMED_MARKER: &[u8] = b"monoize-request-log-reservation\n";
const REQUEST_LOG_INSERT_PREFIX: &str = r#"INSERT INTO request_logs
       (id, request_id, user_id, api_key_id, model, provider_id, upstream_model, channel_id, is_stream,
        input_tokens, output_tokens, cache_read_tokens, cache_creation_tokens, tool_prompt_tokens, reasoning_tokens,
        accepted_prediction_tokens, rejected_prediction_tokens,
        provider_multiplier, charge_nano_usd, status, usage_breakdown_json,
        billing_breakdown_json, error_code, error_message, error_http_status,
        duration_ms, ttfb_ms,
        request_ip, reasoning_effort, tried_providers_json, request_kind,
        effective_provider_type, affinity_hit, affinity_key_hash, affinity_target,
        session_affinity_value,
        created_at, created_at_unix_ms)
       VALUES "#;

#[derive(Debug, Clone)]
struct SpoolFileRef {
    path: std::path::PathBuf,
    bytes: u64,
}

/// Delivery target for replica request-log shipment (PRP12 / M4–M5).
#[async_trait::async_trait]
pub(crate) trait MeteringSink: Send + Sync {
    /// Deliver one batch durably on the receiving side. Returning `Err` MUST mean
    /// nothing was persisted so the caller can retry the identical entries later.
    async fn deliver(&self, entries: &[SpoolRequestLog]) -> Result<(), String>;
}

#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub struct SpoolRequestLog {
    pub id: String,
    pub request_id: Option<String>,
    pub user_id: String,
    pub api_key_id: Option<String>,
    pub model: String,
    pub provider_id: Option<String>,
    pub upstream_model: Option<String>,
    pub channel_id: Option<String>,
    pub is_stream: bool,
    pub input_tokens: Option<u64>,
    pub output_tokens: Option<u64>,
    pub cache_read_tokens: Option<u64>,
    pub cache_creation_tokens: Option<u64>,
    pub tool_prompt_tokens: Option<u64>,
    pub reasoning_tokens: Option<u64>,
    pub accepted_prediction_tokens: Option<u64>,
    pub rejected_prediction_tokens: Option<u64>,
    pub provider_multiplier: Option<String>,
    pub charge_nano_usd: Option<String>,
    pub status: String,
    pub usage_breakdown_json: Option<serde_json::Value>,
    pub billing_breakdown_json: Option<serde_json::Value>,
    pub error_code: Option<String>,
    pub error_message: Option<String>,
    pub error_http_status: Option<u16>,
    pub duration_ms: Option<u64>,
    pub ttfb_ms: Option<u64>,
    pub request_ip: Option<String>,
    pub reasoning_effort: Option<String>,
    pub tried_providers_json: Option<serde_json::Value>,
    pub request_kind: Option<String>,
    pub effective_provider_type: Option<String>,
    pub affinity_hit: Option<bool>,
    pub affinity_key_hash: Option<String>,
    pub affinity_target: Option<String>,
    pub session_affinity_value: Option<String>,
    pub created_at: String,
    pub created_at_unix_ms: i64,
}

impl SpoolRequestLog {
    fn from_log(id: String, log: &InsertRequestLog) -> Self {
        Self {
            id,
            request_id: log.request_id.clone(),
            user_id: log.user_id.clone(),
            api_key_id: log.api_key_id.clone(),
            model: log.model.clone(),
            provider_id: log.provider_id.clone(),
            upstream_model: log.upstream_model.clone(),
            channel_id: log.channel_id.clone(),
            is_stream: log.is_stream,
            input_tokens: log.input_tokens,
            output_tokens: log.output_tokens,
            cache_read_tokens: log.cache_read_tokens,
            cache_creation_tokens: log.cache_creation_tokens,
            tool_prompt_tokens: log.tool_prompt_tokens,
            reasoning_tokens: log.reasoning_tokens,
            accepted_prediction_tokens: log.accepted_prediction_tokens,
            rejected_prediction_tokens: log.rejected_prediction_tokens,
            provider_multiplier: log.provider_multiplier.as_ref().map(ToString::to_string),
            charge_nano_usd: log.charge_nano_usd.map(|value| value.to_string()),
            status: log.status.clone(),
            usage_breakdown_json: log.usage_breakdown_json.clone(),
            billing_breakdown_json: log.billing_breakdown_json.clone(),
            error_code: log.error_code.clone(),
            error_message: log.error_message.clone(),
            error_http_status: log.error_http_status,
            duration_ms: log.duration_ms,
            ttfb_ms: log.ttfb_ms,
            request_ip: log.request_ip.clone(),
            reasoning_effort: log.reasoning_effort.clone(),
            tried_providers_json: log.tried_providers_json.clone(),
            request_kind: log.request_kind.clone(),
            effective_provider_type: log.effective_provider_type.clone(),
            affinity_hit: log.affinity_hit,
            affinity_key_hash: log.affinity_key_hash.clone(),
            affinity_target: log.affinity_target.clone(),
            session_affinity_value: log.session_affinity_value.clone(),
            created_at: log.created_at.to_rfc3339(),
            created_at_unix_ms: log.created_at.timestamp_millis(),
        }
    }

    pub fn to_insert_log(&self) -> InsertRequestLog {
        let created_at = chrono::DateTime::parse_from_rfc3339(&self.created_at)
            .map(|value| value.with_timezone(&chrono::Utc))
            .unwrap_or_else(|_| {
                chrono::DateTime::from_timestamp_millis(self.created_at_unix_ms)
                    .unwrap_or_else(chrono::Utc::now)
            });
        InsertRequestLog {
            request_id: self.request_id.clone(),
            user_id: self.user_id.clone(),
            api_key_id: self.api_key_id.clone(),
            model: self.model.clone(),
            provider_id: self.provider_id.clone(),
            upstream_model: self.upstream_model.clone(),
            channel_id: self.channel_id.clone(),
            names: crate::users::RequestLogNameSnapshots::default(),
            is_stream: self.is_stream,
            input_tokens: self.input_tokens,
            output_tokens: self.output_tokens,
            cache_read_tokens: self.cache_read_tokens,
            cache_creation_tokens: self.cache_creation_tokens,
            tool_prompt_tokens: self.tool_prompt_tokens,
            reasoning_tokens: self.reasoning_tokens,
            accepted_prediction_tokens: self.accepted_prediction_tokens,
            rejected_prediction_tokens: self.rejected_prediction_tokens,
            provider_multiplier: self
                .provider_multiplier
                .as_deref()
                .and_then(|value| value.parse().ok()),
            charge_nano_usd: self
                .charge_nano_usd
                .as_deref()
                .and_then(|value| value.parse().ok()),
            status: self.status.clone(),
            usage_breakdown_json: self.usage_breakdown_json.clone(),
            billing_breakdown_json: self.billing_breakdown_json.clone(),
            error_code: self.error_code.clone(),
            error_message: self.error_message.clone(),
            error_http_status: self.error_http_status,
            duration_ms: self.duration_ms,
            ttfb_ms: self.ttfb_ms,
            request_ip: self.request_ip.clone(),
            reasoning_effort: self.reasoning_effort.clone(),
            tried_providers_json: self.tried_providers_json.clone(),
            request_kind: self.request_kind.clone(),
            effective_provider_type: self.effective_provider_type.clone(),
            affinity_hit: self.affinity_hit,
            affinity_key_hash: self.affinity_key_hash.clone(),
            affinity_target: self.affinity_target.clone(),
            session_affinity_value: self.session_affinity_value.clone(),
            created_at,
        }
    }
}

fn encode_reserved_spool_entry(
    entry: &SpoolRequestLog,
    max_bytes: u64,
) -> Result<Arc<[u8]>, RequestLogAdmissionError> {
    let complete = serde_json::to_vec(entry)
        .map_err(|error| RequestLogAdmissionError::Unavailable(error.to_string()))?;
    if u64::try_from(complete.len()).unwrap_or(u64::MAX) > max_bytes {
        return Err(RequestLogAdmissionError::EntryTooLarge);
    }
    Ok(Arc::from(complete))
}

fn request_log_insert_values(log: &SpoolRequestLog) -> Vec<sea_orm::Value> {
    use sea_orm::Value as SeaValue;
    vec![
        log.id.clone().into(),
        log.request_id.clone().into(),
        log.user_id.clone().into(),
        log.api_key_id.clone().into(),
        log.model.clone().into(),
        log.provider_id.clone().into(),
        log.upstream_model.clone().into(),
        log.channel_id.clone().into(),
        SeaValue::Int(Some(if log.is_stream { 1 } else { 0 })),
        request_log_u64_value("input_tokens", log.input_tokens, log.request_id.as_deref()),
        request_log_u64_value(
            "output_tokens",
            log.output_tokens,
            log.request_id.as_deref(),
        ),
        request_log_u64_value(
            "cache_read_tokens",
            log.cache_read_tokens,
            log.request_id.as_deref(),
        ),
        request_log_u64_value(
            "cache_creation_tokens",
            log.cache_creation_tokens,
            log.request_id.as_deref(),
        ),
        request_log_u64_value(
            "tool_prompt_tokens",
            log.tool_prompt_tokens,
            log.request_id.as_deref(),
        ),
        request_log_u64_value(
            "reasoning_tokens",
            log.reasoning_tokens,
            log.request_id.as_deref(),
        ),
        request_log_u64_value(
            "accepted_prediction_tokens",
            log.accepted_prediction_tokens,
            log.request_id.as_deref(),
        ),
        request_log_u64_value(
            "rejected_prediction_tokens",
            log.rejected_prediction_tokens,
            log.request_id.as_deref(),
        ),
        log.provider_multiplier.clone().into(),
        log.charge_nano_usd.clone().into(),
        log.status.clone().into(),
        log.usage_breakdown_json
            .as_ref()
            .map(serde_json::Value::to_string)
            .into(),
        log.billing_breakdown_json
            .as_ref()
            .map(serde_json::Value::to_string)
            .into(),
        log.error_code.clone().into(),
        log.error_message.clone().into(),
        log.error_http_status
            .map(|value| SeaValue::BigInt(Some(i64::from(value))))
            .unwrap_or(SeaValue::BigInt(None)),
        request_log_u64_value("duration_ms", log.duration_ms, log.request_id.as_deref()),
        request_log_u64_value("ttfb_ms", log.ttfb_ms, log.request_id.as_deref()),
        log.request_ip.clone().into(),
        log.reasoning_effort.clone().into(),
        log.tried_providers_json
            .as_ref()
            .map(serde_json::Value::to_string)
            .into(),
        log.request_kind.clone().into(),
        log.effective_provider_type.clone().into(),
        log.affinity_hit
            .map(|value| SeaValue::Int(Some(if value { 1 } else { 0 })))
            .unwrap_or(SeaValue::Int(None)),
        log.affinity_key_hash.clone().into(),
        log.affinity_target.clone().into(),
        log.session_affinity_value.clone().into(),
        log.created_at.clone().into(),
        log.created_at_unix_ms.into(),
    ]
}

pub(crate) fn request_log_insert_chunk<'a>(
    logs: impl ExactSizeIterator<Item = &'a SpoolRequestLog>,
) -> (String, Vec<sea_orm::Value>) {
    use std::fmt::Write as _;

    let row_count = logs.len();
    debug_assert!(row_count > 0 && row_count <= REQUEST_LOG_INSERT_CHUNK_ENTRIES);
    let mut sql = String::from(REQUEST_LOG_INSERT_PREFIX);
    let mut values = Vec::with_capacity(row_count.saturating_mul(REQUEST_LOG_INSERT_COLUMNS));
    for (row_index, log) in logs.enumerate() {
        if row_index > 0 {
            sql.push_str(", ");
        }
        sql.push('(');
        let first_bind = row_index
            .saturating_mul(REQUEST_LOG_INSERT_COLUMNS)
            .saturating_add(1);
        for column_index in 0..REQUEST_LOG_INSERT_COLUMNS {
            if column_index > 0 {
                sql.push_str(", ");
            }
            write!(sql, "${}", first_bind.saturating_add(column_index))
                .expect("writing to String cannot fail");
        }
        sql.push(')');
        let row_values = request_log_insert_values(log);
        debug_assert_eq!(row_values.len(), REQUEST_LOG_INSERT_COLUMNS);
        values.extend(row_values);
    }
    sql.push_str(" ON CONFLICT(id) DO NOTHING");
    debug_assert_eq!(values.len(), row_count * REQUEST_LOG_INSERT_COLUMNS);
    (sql, values)
}

#[derive(Debug, Clone, thiserror::Error)]
pub enum RequestLogAdmissionError {
    #[error("request log spool quota exhausted")]
    QuotaExhausted,
    #[error(
        "request log spool entry quota {configured} bytes is below the minimum {minimum} bytes"
    )]
    EntryQuotaTooSmall { configured: u64, minimum: u64 },
    #[error("request log entry exceeds spool entry quota")]
    EntryTooLarge,
    #[error("request log spool unavailable: {0}")]
    Unavailable(String),
    #[error("request log reservation is invalid or already consumed")]
    InvalidReservation,
    #[error("request log fallback must have a terminal status")]
    InvalidFallback,
}

#[derive(Clone, Debug)]
pub struct RequestLogReservation {
    inner: Arc<RequestLogReservationInner>,
}

#[derive(Debug)]
struct RequestLogReservationInner {
    admitted_total: Arc<AtomicU64>,
    spool_bytes: Arc<AtomicU64>,
    bytes: u64,
    state: std::sync::atomic::AtomicU8,
    armed_bytes: AtomicU64,
    marker_path: std::sync::Mutex<Option<std::path::PathBuf>>,
    spool_dir: std::path::PathBuf,
    stable_id: Option<String>,
    final_path: Option<std::path::PathBuf>,
}

impl RequestLogReservation {
    fn claim(&self, owner: &Arc<AtomicU64>) -> Result<(), RequestLogAdmissionError> {
        if !Arc::ptr_eq(&self.inner.admitted_total, owner) {
            return Err(RequestLogAdmissionError::InvalidReservation);
        }
        self.inner
            .state
            .compare_exchange(
                REQUEST_LOG_RESERVATION_ARMED,
                REQUEST_LOG_RESERVATION_CLAIMED,
                Ordering::AcqRel,
                Ordering::Acquire,
            )
            .map(|_| ())
            .map_err(|_| RequestLogAdmissionError::InvalidReservation)
    }

    fn begin_arm(&self, owner: &Arc<AtomicU64>) -> Result<(), RequestLogAdmissionError> {
        if !Arc::ptr_eq(&self.inner.admitted_total, owner)
            || self.inner.stable_id.is_none()
            || self.inner.final_path.is_none()
        {
            return Err(RequestLogAdmissionError::InvalidReservation);
        }
        self.inner
            .state
            .compare_exchange(
                REQUEST_LOG_RESERVATION_UNARMED,
                REQUEST_LOG_RESERVATION_ARMING,
                Ordering::AcqRel,
                Ordering::Acquire,
            )
            .map(|_| ())
            .map_err(|_| RequestLogAdmissionError::InvalidReservation)
    }

    fn finish_arm(&self, actual_bytes: u64) {
        self.inner
            .armed_bytes
            .store(actual_bytes, Ordering::Release);
        self.inner
            .state
            .store(REQUEST_LOG_RESERVATION_ARMED, Ordering::Release);
    }

    fn abort_arm(&self) {
        let _ = self.inner.state.compare_exchange(
            REQUEST_LOG_RESERVATION_ARMING,
            REQUEST_LOG_RESERVATION_UNARMED,
            Ordering::AcqRel,
            Ordering::Acquire,
        );
    }

    fn begin_cancel(&self, owner: &Arc<AtomicU64>) -> Result<(), RequestLogAdmissionError> {
        if !Arc::ptr_eq(&self.inner.admitted_total, owner) {
            return Err(RequestLogAdmissionError::InvalidReservation);
        }
        self.inner
            .state
            .compare_exchange(
                REQUEST_LOG_RESERVATION_ARMED,
                REQUEST_LOG_RESERVATION_CANCELING,
                Ordering::AcqRel,
                Ordering::Acquire,
            )
            .map(|_| ())
            .map_err(|_| RequestLogAdmissionError::InvalidReservation)
    }

    fn abort_cancel(&self) {
        let _ = self.inner.state.compare_exchange(
            REQUEST_LOG_RESERVATION_CANCELING,
            REQUEST_LOG_RESERVATION_ARMED,
            Ordering::AcqRel,
            Ordering::Acquire,
        );
    }

    fn finish_cancel(&self) {
        self.inner
            .marker_path
            .lock()
            .unwrap_or_else(|error| error.into_inner())
            .take();
        self.inner.armed_bytes.store(0, Ordering::Release);
        self.inner
            .state
            .store(REQUEST_LOG_RESERVATION_CONSUMED, Ordering::Release);
        self.inner
            .admitted_total
            .fetch_sub(self.inner.bytes, Ordering::AcqRel);
    }

    fn consume(&self, actual_bytes: u64) {
        self.inner.remove_marker();
        if self
            .inner
            .state
            .swap(REQUEST_LOG_RESERVATION_CONSUMED, Ordering::AcqRel)
            != REQUEST_LOG_RESERVATION_CONSUMED
            && self.inner.bytes > actual_bytes
        {
            self.inner
                .admitted_total
                .fetch_sub(self.inner.bytes - actual_bytes, Ordering::AcqRel);
        }
    }
}

impl Drop for RequestLogReservationInner {
    fn drop(&mut self) {
        match self.state.load(Ordering::Acquire) {
            REQUEST_LOG_RESERVATION_CONSUMED => {}
            REQUEST_LOG_RESERVATION_ARMED
            | REQUEST_LOG_RESERVATION_CLAIMED
            | REQUEST_LOG_RESERVATION_CANCELING
                if self.armed_bytes.load(Ordering::Acquire) > 0 =>
            {
                if !self.promote_fallback() {
                    tracing::error!(
                        stable_id = self.stable_id.as_deref().unwrap_or("<missing>"),
                        "armed request-log reservation dropped before terminal persistence; durable fallback remains reserved"
                    );
                }
            }
            _ => {
                self.remove_marker();
                self.admitted_total.fetch_sub(self.bytes, Ordering::AcqRel);
            }
        }
    }
}

impl RequestLogReservationInner {
    fn remove_marker(&self) {
        let marker = self
            .marker_path
            .lock()
            .unwrap_or_else(|error| error.into_inner())
            .take();
        let Some(marker) = marker else {
            return;
        };
        match std::fs::remove_file(&marker) {
            Ok(()) => {
                if let Err(error) = sync_directory(&self.spool_dir) {
                    tracing::warn!(path = %self.spool_dir.display(), "sync request-log spool after admission marker removal failed: {error}");
                }
            }
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => {}
            Err(error) => {
                tracing::warn!(path = %marker.display(), "remove request-log admission marker failed: {error}")
            }
        }
    }

    fn promote_fallback(&self) -> bool {
        let final_path = match self.final_path.as_ref() {
            Some(path) => path,
            None => return false,
        };
        let marker = self
            .marker_path
            .lock()
            .unwrap_or_else(|error| error.into_inner())
            .take();
        let visible = if final_path.exists() {
            if let Some(marker) = marker.as_ref() {
                match std::fs::remove_file(marker) {
                    Ok(()) => {}
                    Err(error) if error.kind() == std::io::ErrorKind::NotFound => {}
                    Err(error) => {
                        tracing::warn!(path = %marker.display(), "remove duplicate armed request-log marker failed: {error}");
                        return false;
                    }
                }
            }
            true
        } else if let Some(marker) = marker.as_ref() {
            match std::fs::rename(marker, final_path) {
                Ok(()) => true,
                Err(error) => {
                    tracing::warn!(
                        marker = %marker.display(),
                        final_path = %final_path.display(),
                        "promote armed request-log fallback failed: {error}"
                    );
                    false
                }
            }
        } else {
            false
        };
        if !visible {
            return false;
        }

        let actual_bytes = match std::fs::metadata(final_path) {
            Ok(metadata) if metadata.is_file() => metadata.len(),
            Ok(_) => return false,
            Err(error) => {
                tracing::warn!(path = %final_path.display(), "stat promoted request-log fallback failed: {error}");
                return false;
            }
        };
        self.spool_bytes.fetch_add(actual_bytes, Ordering::AcqRel);
        if let Err(error) = sync_directory(&self.spool_dir) {
            tracing::warn!(path = %self.spool_dir.display(), "sync promoted request-log fallback failed: {error}");
        } else if self.bytes > actual_bytes {
            self.admitted_total
                .fetch_sub(self.bytes - actual_bytes, Ordering::AcqRel);
        }
        self.state
            .store(REQUEST_LOG_RESERVATION_CONSUMED, Ordering::Release);
        true
    }
}

fn request_log_u64_value(
    field: &'static str,
    value: Option<u64>,
    request_id: Option<&str>,
) -> sea_orm::Value {
    match value {
        Some(value) => match i64::try_from(value) {
            Ok(value) => sea_orm::Value::BigInt(Some(value)),
            Err(_) => {
                tracing::warn!(
                    field,
                    value,
                    request_id = request_id.unwrap_or("<missing>"),
                    "request log scalar exceeds i64 and will be stored as null"
                );
                sea_orm::Value::BigInt(None)
            }
        },
        None => sea_orm::Value::BigInt(None),
    }
}

#[derive(Clone)]
pub struct RequestLogBatcher {
    buffer: Arc<Mutex<Vec<SpoolFileRef>>>,
    flush_lock: Arc<Mutex<()>>,
    memory_capacity: usize,
    spool_dir: Arc<std::path::PathBuf>,
    spool_max_bytes: u64,
    spool_entry_max_bytes: u64,
    spool_bytes: Arc<AtomicU64>,
    admitted_bytes: Arc<AtomicU64>,
    spool_healthy: Arc<std::sync::atomic::AtomicBool>,
    spool_error: Arc<std::sync::Mutex<Option<String>>>,
    broadcast: tokio::sync::broadcast::Sender<Vec<InsertRequestLog>>,
    pending_snapshots: Arc<DashMap<String, InsertRequestLog>>,
    ship_notify: Arc<Notify>,
}

impl RequestLogBatcher {
    pub fn new(
        capacity_hint: usize,
        broadcast: tokio::sync::broadcast::Sender<Vec<InsertRequestLog>>,
        pending_snapshots: Arc<DashMap<String, InsertRequestLog>>,
    ) -> Self {
        Self::new_with_spool_dir(capacity_hint, None, broadcast, pending_snapshots)
    }

    pub fn new_with_spool_dir(
        capacity_hint: usize,
        spool_dir_override: Option<std::path::PathBuf>,
        broadcast: tokio::sync::broadcast::Sender<Vec<InsertRequestLog>>,
        pending_snapshots: Arc<DashMap<String, InsertRequestLog>>,
    ) -> Self {
        let memory_capacity =
            positive_env_usize("MONOIZE_REQUEST_LOG_BUFFER_ENTRIES", capacity_hint.max(1));
        let spool_dir = spool_dir_override.unwrap_or_else(|| {
            std::env::var("MONOIZE_REQUEST_LOG_SPOOL_DIR")
                .map(std::path::PathBuf::from)
                .unwrap_or_else(|_| std::path::PathBuf::from("./data/request-log-spool"))
        });
        let spool_max_bytes =
            positive_env_u64("MONOIZE_REQUEST_LOG_SPOOL_MAX_BYTES", 512 * 1024 * 1024);
        let spool_entry_max_bytes =
            positive_env_u64("MONOIZE_REQUEST_LOG_SPOOL_ENTRY_MAX_BYTES", 8 * 1024 * 1024);
        Self::new_with_limits(
            memory_capacity,
            spool_dir,
            spool_max_bytes,
            spool_entry_max_bytes,
            broadcast,
            pending_snapshots,
        )
    }

    pub fn new_with_limits(
        memory_capacity: usize,
        spool_dir: std::path::PathBuf,
        spool_max_bytes: u64,
        spool_entry_max_bytes: u64,
        broadcast: tokio::sync::broadcast::Sender<Vec<InsertRequestLog>>,
        pending_snapshots: Arc<DashMap<String, InsertRequestLog>>,
    ) -> Self {
        let spool_entry_max_bytes = spool_entry_max_bytes.max(1);
        let initialization = initialize_spool(&spool_dir, spool_entry_max_bytes);
        let (spool_bytes, spool_error) = match initialization {
            Ok(bytes) => (bytes, None),
            Err(error) => (0, Some(error)),
        };
        Self {
            buffer: Arc::new(Mutex::new(Vec::with_capacity(memory_capacity.max(1)))),
            flush_lock: Arc::new(Mutex::new(())),
            memory_capacity: memory_capacity.max(1),
            spool_dir: Arc::new(spool_dir),
            spool_max_bytes: spool_max_bytes.max(1),
            spool_entry_max_bytes,
            spool_bytes: Arc::new(AtomicU64::new(spool_bytes)),
            admitted_bytes: Arc::new(AtomicU64::new(spool_bytes)),
            spool_healthy: Arc::new(std::sync::atomic::AtomicBool::new(spool_error.is_none())),
            spool_error: Arc::new(std::sync::Mutex::new(spool_error)),
            broadcast,
            pending_snapshots,
            ship_notify: Arc::new(Notify::new()),
        }
    }

    pub(crate) fn ship_notify(&self) -> Arc<Notify> {
        self.ship_notify.clone()
    }

    pub fn can_accept_terminal_log(&self) -> bool {
        self.reserve_terminal_log().is_ok()
    }

    pub fn reserve_terminal_log(&self) -> Result<RequestLogReservation, RequestLogAdmissionError> {
        if self.spool_entry_max_bytes < REQUEST_LOG_MIN_ENTRY_BYTES {
            return Err(RequestLogAdmissionError::EntryQuotaTooSmall {
                configured: self.spool_entry_max_bytes,
                minimum: REQUEST_LOG_MIN_ENTRY_BYTES,
            });
        }
        if let Err(error) = std::fs::create_dir_all(&*self.spool_dir) {
            self.mark_spool_error(error.to_string());
            return Err(RequestLogAdmissionError::Unavailable(error.to_string()));
        }
        let stable_id = uuid::Uuid::new_v4().to_string();
        let stable_name = format!(
            "{:020}-{}",
            chrono::Utc::now().timestamp_millis(),
            uuid::Uuid::parse_str(&stable_id)
                .expect("generated request-log UUID parses")
                .simple()
        );
        let final_path = self.spool_dir.join(format!("{stable_name}.json"));
        let marker = self.spool_dir.join(format!(".admission-{stable_name}"));
        let reservation = self.reserve_bytes_with_target(
            self.spool_entry_max_bytes,
            REQUEST_LOG_RESERVATION_UNARMED,
            Some(stable_id),
            Some(final_path),
            Some(marker.clone()),
        )?;
        match write_admission_marker(&self.spool_dir, &marker) {
            Ok(()) => {}
            Err(error) => {
                self.mark_spool_error(error.clone());
                return Err(RequestLogAdmissionError::Unavailable(error));
            }
        }
        self.spool_healthy
            .store(true, std::sync::atomic::Ordering::Release);
        *self
            .spool_error
            .lock()
            .unwrap_or_else(|error| error.into_inner()) = None;
        Ok(reservation)
    }

    pub async fn arm_reserved(
        &self,
        fallback_log: InsertRequestLog,
        reservation: &RequestLogReservation,
    ) -> Result<(), RequestLogAdmissionError> {
        if fallback_log.status == REQUEST_LOG_STATUS_PENDING {
            return Err(RequestLogAdmissionError::InvalidFallback);
        }
        let stable_id = reservation
            .inner
            .stable_id
            .as_ref()
            .ok_or(RequestLogAdmissionError::InvalidReservation)?
            .clone();
        let marker = reservation
            .inner
            .marker_path
            .lock()
            .unwrap_or_else(|error| error.into_inner())
            .clone()
            .ok_or(RequestLogAdmissionError::InvalidReservation)?;
        let entry = SpoolRequestLog::from_log(stable_id, &fallback_log);
        let encoded = encode_reserved_spool_entry(&entry, reservation.inner.bytes)?;
        let encoded_len = u64::try_from(encoded.len()).unwrap_or(u64::MAX);
        reservation.begin_arm(&self.admitted_bytes)?;
        let tmp = self.new_spool_admission_temp_path();
        let result = write_spool_file(&self.spool_dir, &tmp, &marker, encoded).await;
        match result {
            Ok(()) => {
                reservation.finish_arm(encoded_len);
                self.spool_healthy
                    .store(true, std::sync::atomic::Ordering::Release);
                *self
                    .spool_error
                    .lock()
                    .unwrap_or_else(|error| error.into_inner()) = None;
                Ok(())
            }
            Err(error) => {
                reservation.abort_arm();
                self.mark_spool_error(error.clone());
                Err(RequestLogAdmissionError::Unavailable(error))
            }
        }
    }

    pub async fn cancel_reserved(
        &self,
        reservation: &RequestLogReservation,
    ) -> Result<(), RequestLogAdmissionError> {
        reservation.begin_cancel(&self.admitted_bytes)?;
        let Some(marker) = reservation
            .inner
            .marker_path
            .lock()
            .unwrap_or_else(|error| error.into_inner())
            .clone()
        else {
            reservation.abort_cancel();
            return Err(RequestLogAdmissionError::InvalidReservation);
        };
        let mut retry_delay = REQUEST_LOG_RETRY_INITIAL_DELAY;
        loop {
            let marker_for_write = marker.clone();
            let spool_dir = (*self.spool_dir).clone();
            let result = tokio::task::spawn_blocking(move || {
                match std::fs::remove_file(&marker_for_write) {
                    Ok(()) => {}
                    Err(error) if error.kind() == std::io::ErrorKind::NotFound => {}
                    Err(error) => return Err(error.to_string()),
                }
                sync_directory(&spool_dir)
            })
            .await
            .map_err(|error| error.to_string())
            .and_then(|result| result);
            match result {
                Ok(()) => break,
                Err(error) => {
                    self.mark_spool_error(error.clone());
                    tracing::warn!(
                        path = %marker.display(),
                        retry_delay_ms = retry_delay.as_millis(),
                        "durable request-log reservation cancellation failed; retrying: {error}"
                    );
                    tokio::time::sleep(retry_delay).await;
                    retry_delay =
                        std::cmp::min(retry_delay.saturating_mul(2), REQUEST_LOG_RETRY_MAX_DELAY);
                }
            }
        }
        reservation.finish_cancel();
        self.spool_healthy
            .store(true, std::sync::atomic::Ordering::Release);
        *self
            .spool_error
            .lock()
            .unwrap_or_else(|error| error.into_inner()) = None;
        Ok(())
    }

    pub fn ensure_log_capacity(&self) -> Result<(), RequestLogAdmissionError> {
        drop(self.reserve_terminal_log()?);
        Ok(())
    }

    pub async fn push(&self, log: InsertRequestLog) -> Result<(), RequestLogAdmissionError> {
        let entry = SpoolRequestLog::from_log(uuid::Uuid::new_v4().to_string(), &log);
        let encoded = serde_json::to_vec(&entry)
            .map_err(|error| RequestLogAdmissionError::Unavailable(error.to_string()))?;
        let encoded_len = u64::try_from(encoded.len()).unwrap_or(u64::MAX);
        if encoded_len > self.spool_entry_max_bytes {
            return Err(RequestLogAdmissionError::EntryTooLarge);
        }
        let reservation = self.reserve_bytes(encoded_len)?;
        self.push_encoded_once(log, Arc::from(encoded), encoded_len, reservation)
            .await
    }

    pub async fn push_reserved(
        &self,
        log: InsertRequestLog,
        reservation: RequestLogReservation,
    ) -> Result<(), RequestLogAdmissionError> {
        let stable_id = reservation
            .inner
            .stable_id
            .clone()
            .ok_or(RequestLogAdmissionError::InvalidReservation)?;
        let entry = SpoolRequestLog::from_log(stable_id, &log);
        let encoded = encode_reserved_spool_entry(&entry, reservation.inner.bytes)?;
        let encoded_len = u64::try_from(encoded.len()).unwrap_or(u64::MAX);
        self.push_encoded_until_durable(log, encoded, encoded_len, reservation)
            .await
    }

    async fn push_encoded_once(
        &self,
        log: InsertRequestLog,
        encoded: Arc<[u8]>,
        encoded_len: u64,
        reservation: RequestLogReservation,
    ) -> Result<(), RequestLogAdmissionError> {
        reservation.claim(&self.admitted_bytes)?;
        let path = self.new_spool_path();
        let _spool_guard = self.flush_lock.lock().await;
        let tmp = self.new_spool_temp_path();
        let result = write_spool_file(&*self.spool_dir, &tmp, &path, encoded).await;
        if let Err(error) = result {
            self.mark_spool_error(error.clone());
            return Err(RequestLogAdmissionError::Unavailable(error));
        }
        self.complete_durable_push(log, path, encoded_len, reservation)
            .await;
        Ok(())
    }

    async fn push_encoded_until_durable(
        &self,
        log: InsertRequestLog,
        encoded: Arc<[u8]>,
        encoded_len: u64,
        reservation: RequestLogReservation,
    ) -> Result<(), RequestLogAdmissionError> {
        reservation.claim(&self.admitted_bytes)?;
        let path = reservation
            .inner
            .final_path
            .clone()
            .ok_or(RequestLogAdmissionError::InvalidReservation)?;
        let mut retry_delay = REQUEST_LOG_RETRY_INITIAL_DELAY;
        loop {
            let spool_guard = self.flush_lock.lock().await;
            let tmp = self.new_spool_temp_path();
            let result = write_spool_file(&*self.spool_dir, &tmp, &path, encoded.clone()).await;
            match result {
                Ok(()) => {
                    self.complete_durable_push(log, path, encoded_len, reservation)
                        .await;
                    drop(spool_guard);
                    return Ok(());
                }
                Err(error) => {
                    drop(spool_guard);
                    self.mark_spool_error(error.clone());
                    tracing::warn!(
                        request_id = log.request_id.as_deref().unwrap_or("<missing>"),
                        retry_delay_ms = retry_delay.as_millis(),
                        "request log durable spool write failed; retrying: {error}"
                    );
                    tokio::time::sleep(retry_delay).await;
                    retry_delay =
                        std::cmp::min(retry_delay.saturating_mul(2), REQUEST_LOG_RETRY_MAX_DELAY);
                }
            }
        }
    }

    fn new_spool_path(&self) -> std::path::PathBuf {
        self.spool_dir.join(format!(
            "{:020}-{}.json",
            chrono::Utc::now().timestamp_millis(),
            uuid::Uuid::new_v4().simple()
        ))
    }

    fn new_spool_temp_path(&self) -> std::path::PathBuf {
        self.spool_dir
            .join(format!(".tmp-{}", uuid::Uuid::new_v4().simple()))
    }

    fn new_spool_admission_temp_path(&self) -> std::path::PathBuf {
        self.spool_dir
            .join(format!(".admission-tmp-{}", uuid::Uuid::new_v4().simple()))
    }

    async fn complete_durable_push(
        &self,
        log: InsertRequestLog,
        path: std::path::PathBuf,
        encoded_len: u64,
        reservation: RequestLogReservation,
    ) {
        self.spool_bytes.fetch_add(encoded_len, Ordering::AcqRel);
        reservation.consume(encoded_len);
        self.spool_healthy
            .store(true, std::sync::atomic::Ordering::Release);
        *self
            .spool_error
            .lock()
            .unwrap_or_else(|error| error.into_inner()) = None;
        if log.status != REQUEST_LOG_STATUS_PENDING
            && let Some(request_id) = log.request_id.as_deref()
        {
            self.pending_snapshots.remove(request_id);
        }
        let _ = self.broadcast.send(vec![log.clone()]);
        self.ship_notify.notify_one();
        let mut buf = self.buffer.lock().await;
        if buf.len() < self.memory_capacity {
            buf.push(SpoolFileRef {
                path,
                bytes: encoded_len,
            });
        }
    }

    fn reserve_bytes(&self, bytes: u64) -> Result<RequestLogReservation, RequestLogAdmissionError> {
        self.reserve_bytes_with_target(bytes, REQUEST_LOG_RESERVATION_ARMED, None, None, None)
    }

    fn reserve_bytes_with_target(
        &self,
        bytes: u64,
        initial_state: u8,
        stable_id: Option<String>,
        final_path: Option<std::path::PathBuf>,
        marker_path: Option<std::path::PathBuf>,
    ) -> Result<RequestLogReservation, RequestLogAdmissionError> {
        if bytes == 0 || bytes > self.spool_entry_max_bytes || bytes > self.spool_max_bytes {
            return Err(RequestLogAdmissionError::EntryTooLarge);
        }
        loop {
            let admitted = self.admitted_bytes.load(Ordering::Acquire);
            if admitted.saturating_add(bytes) > self.spool_max_bytes {
                return Err(RequestLogAdmissionError::QuotaExhausted);
            }
            if self
                .admitted_bytes
                .compare_exchange(
                    admitted,
                    admitted.saturating_add(bytes),
                    Ordering::AcqRel,
                    Ordering::Acquire,
                )
                .is_ok()
            {
                return Ok(RequestLogReservation {
                    inner: Arc::new(RequestLogReservationInner {
                        admitted_total: self.admitted_bytes.clone(),
                        spool_bytes: self.spool_bytes.clone(),
                        bytes,
                        state: std::sync::atomic::AtomicU8::new(initial_state),
                        armed_bytes: AtomicU64::new(0),
                        marker_path: std::sync::Mutex::new(marker_path),
                        spool_dir: (*self.spool_dir).clone(),
                        stable_id,
                        final_path,
                    }),
                });
            }
        }
    }

    async fn requeue_front(&self, mut entries: Vec<SpoolFileRef>) {
        let mut buf = self.buffer.lock().await;
        entries.append(&mut *buf);
        entries.truncate(self.memory_capacity);
        *buf = entries;
    }

    fn mark_spool_error(&self, error: String) {
        self.spool_healthy
            .store(false, std::sync::atomic::Ordering::Release);
        *self
            .spool_error
            .lock()
            .unwrap_or_else(|state| state.into_inner()) = Some(error);
    }

    /// Drain buffer and batch-insert into DB.
    pub async fn flush(&self, db: &DbPool) {
        let _flush_guard = self.flush_lock.lock().await;
        let buffered: Vec<SpoolFileRef> = {
            let mut buf = self.buffer.lock().await;
            std::mem::replace(&mut *buf, Vec::with_capacity(self.memory_capacity))
        };
        let entries = match load_spool_batch(
            &self.spool_dir,
            buffered,
            self.memory_capacity,
            self.spool_entry_max_bytes,
        )
        .await
        {
            Ok(entries) => entries,
            Err(error) => {
                self.mark_spool_error(error.clone());
                tracing::warn!("request_log_batcher spool read error: {error}");
                return;
            }
        };
        if entries.is_empty() {
            return;
        }
        let entry_refs = entries
            .iter()
            .map(|(entry, _)| entry.clone())
            .collect::<Vec<_>>();

        let write = db.write().await;
        use sea_orm::{ConnectionTrait, TransactionTrait};

        let tx = match write.begin().await {
            Ok(tx) => tx,
            Err(e) => {
                tracing::warn!("request_log_batcher flush begin tx error: {e}");
                self.requeue_front(entry_refs).await;
                return;
            }
        };

        let mut insert_failed = false;
        for chunk in entries.chunks(REQUEST_LOG_INSERT_CHUNK_ENTRIES) {
            let (sql, values) = request_log_insert_chunk(chunk.iter().map(|(_, log)| log));
            if let Err(e) = tx.execute(db.stmt(&sql, values)).await {
                tracing::warn!("request_log_batcher flush error: {e}");
                insert_failed = true;
                break;
            }
        }

        if insert_failed {
            if let Err(e) = tx.rollback().await {
                tracing::warn!("request_log_batcher rollback error: {e}");
            }
            self.requeue_front(entry_refs).await;
            return;
        }

        if let Err(e) = tx.commit().await {
            tracing::warn!("request_log_batcher commit error: {e}");
            self.requeue_front(entry_refs).await;
            return;
        }

        let mut deletion_failed = Vec::new();
        for (entry, _) in entries {
            match tokio::fs::remove_file(&entry.path).await {
                Ok(()) => {
                    atomic_saturating_sub(&self.spool_bytes, entry.bytes);
                    atomic_saturating_sub(&self.admitted_bytes, entry.bytes);
                }
                Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
                    atomic_saturating_sub(&self.spool_bytes, entry.bytes);
                    atomic_saturating_sub(&self.admitted_bytes, entry.bytes);
                }
                Err(error) => {
                    tracing::warn!(path = %entry.path.display(), "remove committed request log spool file failed: {error}");
                    deletion_failed.push(entry);
                }
            }
        }
        if !deletion_failed.is_empty() {
            self.requeue_front(deletion_failed).await;
        }
    }

    /// Replica shipment path (PRP12 / M4–M5): select the oldest durable entries, hand
    /// them to `sink`, and only release spool files and quota accounting after the sink
    /// reports success. On failure every entry is requeued in its original order.
    /// Serialized by `flush_lock`, so it cannot interleave with a DB flush on this node.
    pub(crate) async fn ship_via(&self, max_entries: usize, sink: &dyn MeteringSink) -> usize {
        let _flush_guard = self.flush_lock.lock().await;
        let selected: Vec<SpoolFileRef> = {
            let mut buf = self.buffer.lock().await;
            let take = max_entries.min(buf.len());
            buf.drain(..take).collect()
        };
        let selected_backup: Vec<SpoolFileRef> = selected
            .iter()
            .map(|entry| SpoolFileRef {
                path: entry.path.clone(),
                bytes: entry.bytes,
            })
            .collect();
        let entries = match load_spool_batch(
            &self.spool_dir,
            selected,
            max_entries,
            self.spool_entry_max_bytes,
        )
        .await
        {
            Ok(entries) => entries,
            Err(error) => {
                // At least one file became unreadable: put the refs back unchanged.
                tracing::warn!("request_log_batcher ship read error: {error}");
                self.requeue_front(selected_backup).await;
                return 0;
            }
        };
        if entries.is_empty() {
            return 0;
        }
        let entry_refs = entries
            .iter()
            .map(|(entry, _)| entry.clone())
            .collect::<Vec<_>>();
        let payloads = entries
            .iter()
            .map(|(_, log)| log.clone())
            .collect::<Vec<_>>();
        match sink.deliver(&payloads).await {
            Ok(()) => {}
            Err(error) => {
                tracing::warn!(
                    entries = payloads.len(),
                    "metering sink rejected request-log batch: {error}"
                );
                self.requeue_front(entry_refs).await;
                return 0;
            }
        }
        let mut delivered = 0usize;
        let mut deletion_failed = Vec::new();
        for (entry, _) in entries {
            match tokio::fs::remove_file(&entry.path).await {
                Ok(()) => {
                    atomic_saturating_sub(&self.spool_bytes, entry.bytes);
                    atomic_saturating_sub(&self.admitted_bytes, entry.bytes);
                    delivered += 1;
                }
                Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
                    atomic_saturating_sub(&self.spool_bytes, entry.bytes);
                    atomic_saturating_sub(&self.admitted_bytes, entry.bytes);
                    delivered += 1;
                }
                Err(error) => {
                    tracing::warn!(path = %entry.path.display(), "remove shipped request log spool file failed: {error}");
                    deletion_failed.push(entry);
                }
            }
        }
        if !deletion_failed.is_empty() {
            self.requeue_front(deletion_failed).await;
        }
        delivered
    }

    /// Spawn background task that flushes every `interval`.
    pub fn spawn_flush_task(self, db: DbPool, interval: Duration) -> tokio::task::JoinHandle<()> {
        tokio::spawn(async move {
            let mut ticker = tokio::time::interval(interval);
            ticker.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Delay);
            loop {
                ticker.tick().await;
                self.flush(&db).await;
            }
        })
    }
}

fn atomic_saturating_sub(counter: &AtomicU64, amount: u64) {
    let _ = counter.fetch_update(Ordering::AcqRel, Ordering::Acquire, |current| {
        Some(current.saturating_sub(amount))
    });
}

// ---------------------------------------------------------------------------
// ApiKeyCache: caches validated API key lookups, invalidated on mutation
// ---------------------------------------------------------------------------

use crate::users::{ApiKey, User};

#[derive(Clone)]
struct CachedApiKeyEntry {
    api_key: ApiKey,
    user: User,
    cached_at: Instant,
    generation: u64,
}

/// Caches successful `validate_api_key` results keyed by the complete API key.
/// Entries expire after `ttl`. Mutations to api_keys table must call `invalidate_*`.
#[derive(Debug, Clone)]
pub struct ApiKeyCache {
    cache: Arc<DashMap<String, CachedApiKeyEntry>>,
    key_id_index: Arc<DashMap<String, std::collections::HashSet<String>>>,
    user_id_index: Arc<DashMap<String, std::collections::HashSet<String>>>,
    generation: Arc<AtomicU64>,
    ttl: Duration,
    capacity: usize,
    mutation_lock: Arc<std::sync::Mutex<()>>,
}

impl std::fmt::Debug for CachedApiKeyEntry {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("CachedApiKeyEntry")
            .field("api_key_id", &self.api_key.id)
            .field("user_id", &self.user.id)
            .finish()
    }
}

impl ApiKeyCache {
    pub fn new(ttl: Duration) -> Self {
        Self::with_capacity(
            ttl,
            positive_env_usize("MONOIZE_API_KEY_CACHE_CAPACITY", 10_000),
        )
    }

    pub fn with_capacity(ttl: Duration, capacity: usize) -> Self {
        Self {
            cache: Arc::new(DashMap::new()),
            key_id_index: Arc::new(DashMap::new()),
            user_id_index: Arc::new(DashMap::new()),
            generation: Arc::new(AtomicU64::new(0)),
            ttl,
            capacity: capacity.max(1),
            mutation_lock: Arc::new(std::sync::Mutex::new(())),
        }
    }

    pub fn get(&self, key: &str) -> Option<(ApiKey, User)> {
        let entry = self.cache.get(key)?;
        if entry.cached_at.elapsed() > self.ttl {
            drop(entry);
            let _guard = self
                .mutation_lock
                .lock()
                .unwrap_or_else(|err| err.into_inner());
            if self
                .cache
                .get(key)
                .is_some_and(|entry| entry.cached_at.elapsed() > self.ttl)
            {
                self.remove_key_locked(key);
            }
            return None;
        }
        Some((entry.api_key.clone(), entry.user.clone()))
    }

    pub(crate) fn current_generation(&self) -> u64 {
        self.generation.load(Ordering::Acquire)
    }

    pub(crate) fn insert_if_current(
        &self,
        key: String,
        generation: u64,
        api_key: ApiKey,
        user: User,
    ) -> bool {
        if self.current_generation() != generation {
            return false;
        }
        let _guard = self
            .mutation_lock
            .lock()
            .unwrap_or_else(|err| err.into_inner());
        if self.current_generation() != generation {
            return false;
        }
        if !self.cache.contains_key(&key) && self.cache.len() >= self.capacity {
            let eviction_key = { self.cache.iter().next().map(|entry| entry.key().clone()) };
            if let Some(eviction_key) = eviction_key {
                self.remove_key_locked(&eviction_key);
            }
        }
        if self.cache.contains_key(&key) {
            self.remove_key_locked(&key);
        }
        let cache_key = key.clone();
        let api_key_id = api_key.id.clone();
        let user_id = api_key.user_id.clone();
        self.cache.insert(
            key,
            CachedApiKeyEntry {
                api_key,
                user,
                cached_at: Instant::now(),
                generation,
            },
        );
        self.key_id_index
            .entry(api_key_id)
            .or_default()
            .insert(cache_key.clone());
        self.user_id_index
            .entry(user_id)
            .or_default()
            .insert(cache_key.clone());
        if self.current_generation() == generation {
            return true;
        }
        if self
            .cache
            .get(&cache_key)
            .is_some_and(|entry| entry.generation == generation)
        {
            self.remove_key_locked(&cache_key);
        }
        false
    }

    fn begin_invalidation(&self) {
        self.generation.fetch_add(1, Ordering::AcqRel);
    }

    pub fn invalidate_by_key_id(&self, key_id: &str) {
        self.begin_invalidation();
        let _guard = self
            .mutation_lock
            .lock()
            .unwrap_or_else(|err| err.into_inner());
        let keys = self
            .key_id_index
            .remove(key_id)
            .map(|(_, keys)| keys)
            .unwrap_or_default();
        for key in keys {
            self.remove_key_locked(&key);
        }
    }

    /// Invalidate all keys belonging to a user.
    pub fn invalidate_by_user_id(&self, user_id: &str) {
        self.begin_invalidation();
        let _guard = self
            .mutation_lock
            .lock()
            .unwrap_or_else(|err| err.into_inner());
        let keys = self
            .user_id_index
            .remove(user_id)
            .map(|(_, keys)| keys)
            .unwrap_or_default();
        for key in keys {
            self.remove_key_locked(&key);
        }
    }

    /// Invalidate entries matching any of the given key IDs.
    pub fn invalidate_by_key_ids(&self, key_ids: &[String]) {
        self.begin_invalidation();
        let _guard = self
            .mutation_lock
            .lock()
            .unwrap_or_else(|err| err.into_inner());
        for key_id in key_ids {
            let keys = self
                .key_id_index
                .remove(key_id)
                .map(|(_, keys)| keys)
                .unwrap_or_default();
            for key in keys {
                self.remove_key_locked(&key);
            }
        }
    }

    pub fn invalidate(&self, key: &str) {
        self.begin_invalidation();
        let _guard = self
            .mutation_lock
            .lock()
            .unwrap_or_else(|err| err.into_inner());
        self.remove_key_locked(key);
    }

    /// Remove all entries.
    pub fn invalidate_all(&self) {
        self.begin_invalidation();
        let _guard = self
            .mutation_lock
            .lock()
            .unwrap_or_else(|err| err.into_inner());
        self.cache.clear();
        self.key_id_index.clear();
        self.user_id_index.clear();
    }

    pub fn spawn_eviction_task(self, interval: Duration) -> tokio::task::JoinHandle<()> {
        tokio::spawn(async move {
            let mut ticker = tokio::time::interval(interval);
            ticker.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Delay);
            loop {
                ticker.tick().await;
                let expired = self
                    .cache
                    .iter()
                    .filter(|entry| entry.cached_at.elapsed() > self.ttl)
                    .map(|entry| entry.key().clone())
                    .collect::<Vec<_>>();
                let _guard = self
                    .mutation_lock
                    .lock()
                    .unwrap_or_else(|err| err.into_inner());
                for key in expired {
                    if self
                        .cache
                        .get(&key)
                        .is_some_and(|entry| entry.cached_at.elapsed() > self.ttl)
                    {
                        self.remove_key_locked(&key);
                    }
                }
            }
        })
    }

    fn remove_key_locked(&self, key: &str) {
        let Some((_, entry)) = self.cache.remove(key) else {
            return;
        };
        remove_index_member(&self.key_id_index, &entry.api_key.id, key);
        remove_index_member(&self.user_id_index, &entry.api_key.user_id, key);
    }
}

// ---------------------------------------------------------------------------
// BalanceCache: caches user balance lookups, invalidated on charge/adjust
// ---------------------------------------------------------------------------

#[derive(Clone, Debug)]
struct CachedBalanceEntry {
    balance: UserBalance,
    cached_at: Instant,
    generation: u64,
}

/// Caches `get_user_balance` results keyed by user_id.
/// Entries expire after `ttl`. Balance mutations must call `invalidate`.
#[derive(Debug, Clone)]
pub struct BalanceCache {
    cache: Arc<DashMap<String, CachedBalanceEntry>>,
    generation: Arc<AtomicU64>,
    ttl: Duration,
    capacity: usize,
    mutation_lock: Arc<std::sync::Mutex<()>>,
}

impl BalanceCache {
    pub fn new(ttl: Duration) -> Self {
        Self::with_capacity(
            ttl,
            positive_env_usize("MONOIZE_BALANCE_CACHE_CAPACITY", 10_000),
        )
    }

    pub fn with_capacity(ttl: Duration, capacity: usize) -> Self {
        Self {
            cache: Arc::new(DashMap::new()),
            generation: Arc::new(AtomicU64::new(0)),
            ttl,
            capacity: capacity.max(1),
            mutation_lock: Arc::new(std::sync::Mutex::new(())),
        }
    }

    pub fn get(&self, user_id: &str) -> Option<UserBalance> {
        let entry = self.cache.get(user_id)?;
        if entry.cached_at.elapsed() > self.ttl {
            drop(entry);
            let _guard = self
                .mutation_lock
                .lock()
                .unwrap_or_else(|err| err.into_inner());
            self.cache
                .remove_if(user_id, |_, v| v.cached_at.elapsed() > self.ttl);
            return None;
        }
        Some(entry.balance.clone())
    }

    pub(crate) fn current_generation(&self) -> u64 {
        self.generation.load(Ordering::Acquire)
    }

    pub(crate) fn insert_if_current(
        &self,
        user_id: String,
        generation: u64,
        balance: UserBalance,
    ) -> bool {
        if self.current_generation() != generation {
            return false;
        }
        let _guard = self
            .mutation_lock
            .lock()
            .unwrap_or_else(|err| err.into_inner());
        if self.current_generation() != generation {
            return false;
        }
        if !self.cache.contains_key(&user_id) && self.cache.len() >= self.capacity {
            let eviction_key = { self.cache.iter().next().map(|entry| entry.key().clone()) };
            if let Some(eviction_key) = eviction_key {
                self.cache.remove(&eviction_key);
            }
        }
        let cache_key = user_id.clone();
        self.cache.insert(
            user_id,
            CachedBalanceEntry {
                balance,
                cached_at: Instant::now(),
                generation,
            },
        );
        if self.current_generation() == generation {
            return true;
        }
        self.cache
            .remove_if(&cache_key, |_, entry| entry.generation == generation);
        false
    }

    pub fn invalidate(&self, user_id: &str) {
        self.generation.fetch_add(1, Ordering::AcqRel);
        let _guard = self
            .mutation_lock
            .lock()
            .unwrap_or_else(|err| err.into_inner());
        self.cache.remove(user_id);
    }

    pub fn invalidate_all(&self) {
        self.generation.fetch_add(1, Ordering::AcqRel);
        let _guard = self
            .mutation_lock
            .lock()
            .unwrap_or_else(|err| err.into_inner());
        self.cache.clear();
    }

    pub fn spawn_eviction_task(self, interval: Duration) -> tokio::task::JoinHandle<()> {
        tokio::spawn(async move {
            let mut ticker = tokio::time::interval(interval);
            ticker.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Delay);
            loop {
                ticker.tick().await;
                let _guard = self
                    .mutation_lock
                    .lock()
                    .unwrap_or_else(|err| err.into_inner());
                let ttl = self.ttl;
                self.cache.retain(|_, v| v.cached_at.elapsed() <= ttl);
            }
        })
    }
}

fn remove_index_member(
    index: &DashMap<String, std::collections::HashSet<String>>,
    index_key: &str,
    cache_key: &str,
) {
    let remove_empty = if let Some(mut members) = index.get_mut(index_key) {
        members.remove(cache_key);
        members.is_empty()
    } else {
        false
    };
    if remove_empty {
        index.remove_if(index_key, |_, members| members.is_empty());
    }
}

fn initialize_spool(spool_dir: &std::path::Path, max_entry_bytes: u64) -> Result<u64, String> {
    std::fs::create_dir_all(spool_dir).map_err(|error| error.to_string())?;
    let mut directory_changed = false;
    for entry in std::fs::read_dir(spool_dir).map_err(|error| error.to_string())? {
        let entry = entry.map_err(|error| error.to_string())?;
        let path = entry.path();
        let Some(file_name) = path.file_name().and_then(|value| value.to_str()) else {
            continue;
        };
        if file_name.starts_with(".tmp-") || file_name.starts_with(".admission-tmp-") {
            if entry
                .metadata()
                .map_err(|error| error.to_string())?
                .is_file()
            {
                std::fs::remove_file(&path).map_err(|error| error.to_string())?;
                directory_changed = true;
            }
            continue;
        }
        let Some(stable_name) = file_name.strip_prefix(".admission-") else {
            continue;
        };
        let metadata = entry.metadata().map_err(|error| error.to_string())?;
        if !metadata.is_file() {
            continue;
        }
        if metadata.len() > max_entry_bytes {
            return Err(format!(
                "request-log admission marker {} exceeds entry quota",
                path.display()
            ));
        }
        let encoded = std::fs::read(&path).map_err(|error| error.to_string())?;
        if encoded == REQUEST_LOG_UNARMED_MARKER {
            std::fs::remove_file(&path).map_err(|error| error.to_string())?;
            directory_changed = true;
            continue;
        }
        serde_json::from_slice::<SpoolRequestLog>(&encoded).map_err(|error| {
            format!(
                "request-log admission marker {} is not recoverable: {error}",
                path.display()
            )
        })?;
        let final_path = spool_dir.join(format!("{stable_name}.json"));
        if final_path.exists() {
            std::fs::remove_file(&path).map_err(|error| error.to_string())?;
        } else {
            std::fs::rename(&path, &final_path).map_err(|error| error.to_string())?;
        }
        directory_changed = true;
    }
    if directory_changed {
        sync_directory(spool_dir)?;
    }

    let mut bytes = 0_u64;
    for entry in std::fs::read_dir(spool_dir).map_err(|error| error.to_string())? {
        let entry = entry.map_err(|error| error.to_string())?;
        let path = entry.path();
        if path.extension().and_then(|value| value.to_str()) != Some("json") {
            continue;
        }
        let metadata = entry.metadata().map_err(|error| error.to_string())?;
        if metadata.is_file() {
            bytes = bytes.saturating_add(metadata.len());
        }
    }
    Ok(bytes)
}

fn write_admission_marker(
    spool_dir: &std::path::Path,
    marker: &std::path::Path,
) -> Result<(), String> {
    use std::io::Write;
    std::fs::create_dir_all(spool_dir).map_err(|error| error.to_string())?;
    let nonce = uuid::Uuid::new_v4().simple();
    let tmp = spool_dir.join(format!(".admission-tmp-{nonce}"));
    let result = (|| {
        let mut file = std::fs::OpenOptions::new()
            .write(true)
            .create_new(true)
            .open(&tmp)
            .map_err(|error| error.to_string())?;
        file.write_all(REQUEST_LOG_UNARMED_MARKER)
            .map_err(|error| error.to_string())?;
        file.sync_all().map_err(|error| error.to_string())?;
        std::fs::rename(&tmp, &marker).map_err(|error| error.to_string())?;
        sync_directory(spool_dir)?;
        Ok::<(), String>(())
    })();
    if let Err(error) = result {
        let _ = std::fs::remove_file(&tmp);
        let _ = std::fs::remove_file(&marker);
        return Err(error);
    }
    Ok(())
}

#[cfg(not(windows))]
fn sync_directory(directory: &std::path::Path) -> Result<(), String> {
    std::fs::File::open(directory)
        .and_then(|directory| directory.sync_all())
        .map_err(|error| error.to_string())
}

#[cfg(windows)]
fn sync_directory(directory: &std::path::Path) -> Result<(), String> {
    use std::os::windows::fs::OpenOptionsExt;

    const FILE_FLAG_BACKUP_SEMANTICS: u32 = 0x0200_0000;
    std::fs::OpenOptions::new()
        .write(true)
        .custom_flags(FILE_FLAG_BACKUP_SEMANTICS)
        .open(directory)
        .and_then(|directory| directory.sync_all())
        .map_err(|error| error.to_string())
}

async fn write_spool_file(
    spool_dir: &std::path::Path,
    tmp: &std::path::Path,
    path: &std::path::Path,
    encoded: Arc<[u8]>,
) -> Result<(), String> {
    let spool_dir = spool_dir.to_path_buf();
    let tmp = tmp.to_path_buf();
    let path = path.to_path_buf();
    tokio::task::spawn_blocking(move || {
        use std::io::Write;
        std::fs::create_dir_all(&spool_dir).map_err(|error| error.to_string())?;
        let result = (|| {
            let mut file = std::fs::OpenOptions::new()
                .write(true)
                .create_new(true)
                .open(&tmp)
                .map_err(|error| error.to_string())?;
            file.write_all(&encoded)
                .map_err(|error| error.to_string())?;
            file.sync_all().map_err(|error| error.to_string())?;
            std::fs::rename(&tmp, &path).map_err(|error| error.to_string())?;
            sync_directory(&spool_dir)?;
            Ok::<(), String>(())
        })();
        if result.is_err() {
            let _ = std::fs::remove_file(&tmp);
        }
        result
    })
    .await
    .map_err(|error| error.to_string())?
}

async fn load_spool_batch(
    spool_dir: &std::path::Path,
    buffered: Vec<SpoolFileRef>,
    max_entries: usize,
    max_entry_bytes: u64,
) -> Result<Vec<(SpoolFileRef, SpoolRequestLog)>, String> {
    let mut paths = buffered
        .into_iter()
        .map(|entry| entry.path)
        .collect::<std::collections::BTreeSet<_>>();
    let mut directory = tokio::fs::read_dir(spool_dir)
        .await
        .map_err(|error| error.to_string())?;
    while paths.len() < max_entries {
        let Some(entry) = directory
            .next_entry()
            .await
            .map_err(|error| error.to_string())?
        else {
            break;
        };
        let path = entry.path();
        if path.extension().and_then(|value| value.to_str()) == Some("json") {
            paths.insert(path);
        }
    }
    let mut entries = Vec::with_capacity(paths.len().min(max_entries));
    for path in paths.into_iter().take(max_entries) {
        let metadata = match tokio::fs::metadata(&path).await {
            Ok(metadata) if metadata.is_file() => metadata,
            Ok(_) => continue,
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => continue,
            Err(error) => return Err(format!("read {} metadata: {error}", path.display())),
        };
        if metadata.len() > max_entry_bytes {
            return Err(format!(
                "spool entry {} exceeds entry quota",
                path.display()
            ));
        }
        let raw = tokio::fs::read(&path)
            .await
            .map_err(|error| format!("read {}: {error}", path.display()))?;
        let log = serde_json::from_slice::<SpoolRequestLog>(&raw)
            .map_err(|error| format!("decode {}: {error}", path.display()))?;
        entries.push((
            SpoolFileRef {
                path,
                bytes: metadata.len(),
            },
            log,
        ));
    }
    Ok(entries)
}

fn positive_env_usize(name: &str, default: usize) -> usize {
    std::env::var(name)
        .ok()
        .and_then(|value| value.parse::<usize>().ok())
        .filter(|value| *value > 0)
        .unwrap_or(default)
}

fn positive_env_u64(name: &str, default: u64) -> u64 {
    std::env::var(name)
        .ok()
        .and_then(|value| value.parse::<u64>().ok())
        .filter(|value| *value > 0)
        .unwrap_or(default)
}
