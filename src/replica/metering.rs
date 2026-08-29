//! Metering pipeline between replica and primary (`primary-replica-deployment.spec.md`
//! sections 6–7): durable balance-delta spool on replicas, batched shipment over the
//! internal HTTP API, idempotent apply inside one transaction on the primary, and the
//! PRP9 promotion drain.

use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use axum::extract::State;
use axum::http::{HeaderMap, StatusCode};
use axum::response::IntoResponse;
use dashmap::DashMap;
use sea_orm::ConnectionTrait;
use serde::{Deserialize, Serialize};
use serde_json::Value;

use crate::db::DbPool;
use crate::db_cache::{LastUsedBatcher, MeteringSink, RequestLogBatcher, SpoolRequestLog};

pub const METERING_INGEST_PATH: &str = "/internal/replica/metering";
/// Hard per-batch cap enforced by both sides regardless of configuration (I3).
pub const METERING_BATCH_HARD_CAP: usize = 2000;
/// M9: file inside the metering spool directory that persists the replica identity.
pub const REPLICA_IDENTITY_FILE_NAME: &str = "replica-identity";
/// M4a: ship-interval multiples after which a heartbeat is displayed as stale.
pub const HEARTBEAT_STALE_INTERVALS: u32 = 3;
/// M4a: ship-interval multiples after which a heartbeat entry is evicted on read.
pub const HEARTBEAT_EVICT_INTERVALS: u32 = 360;

// ---------------------------------------------------------------------------
// Wire types
// ---------------------------------------------------------------------------

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct BalanceDelta {
    pub delta_id: String,
    pub kind: String,
    pub user_id: String,
    #[serde(default)]
    pub api_key_id: Option<String>,
    pub amount_nano_usd: String,
    #[serde(default)]
    pub meta_json: Value,
    pub created_at: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct LastUsedPair {
    pub api_key_id: String,
    pub last_used_at: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ReplicaHeartbeat {
    pub id: String,
    pub hostname: String,
    pub listen: String,
    pub version: String,
    pub started_at: String,
    #[serde(default)]
    pub uptime_seconds: u64,
    #[serde(default)]
    pub spool_pending_count: usize,
    #[serde(default)]
    pub spool_pending_bytes: u64,
}

#[derive(Debug, Clone)]
pub struct ReplicaHeartbeatRecord {
    pub heartbeat: ReplicaHeartbeat,
    pub last_seen_unix_ms: i64,
}

#[derive(Debug, Default, Serialize, Deserialize)]
pub struct MeteringBatch {
    #[serde(default)]
    pub replica: Option<ReplicaHeartbeat>,
    #[serde(default)]
    pub request_logs: Vec<SpoolRequestLog>,
    #[serde(default)]
    pub last_used: Vec<LastUsedPair>,
    #[serde(default)]
    pub balance_deltas: Vec<BalanceDelta>,
}

impl MeteringBatch {
    fn total_entries(&self) -> usize {
        self.request_logs.len() + self.last_used.len() + self.balance_deltas.len()
    }

    fn is_empty(&self) -> bool {
        self.total_entries() == 0
    }
}

#[derive(Debug, Serialize, Deserialize)]
pub struct MeteringAck {
    pub applied_request_logs: u64,
    pub applied_last_used: u64,
    pub applied_balance_deltas: u64,
}

pub fn valid_delta_kind(kind: &str) -> bool {
    matches!(kind, "request_charge" | "api_key_charge")
}

pub fn validate_delta(delta: &BalanceDelta) -> Result<(), &'static str> {
    if delta.delta_id.is_empty() || delta.user_id.is_empty() {
        return Err("delta_id and user_id must be non-empty");
    }
    if !valid_delta_kind(&delta.kind) {
        return Err("kind must be request_charge or api_key_charge");
    }
    if delta.amount_nano_usd.trim().parse::<i128>().is_err() {
        return Err("amount_nano_usd must be decimal i128 text");
    }
    if delta.created_at.is_empty() {
        return Err("created_at must be RFC 3339 text");
    }
    Ok(())
}

// ---------------------------------------------------------------------------
// Pending deductions (M3/M7)
// ---------------------------------------------------------------------------

#[derive(Debug, Default)]
pub struct PendingDeductions {
    map: DashMap<String, i128>,
}

impl PendingDeductions {
    pub fn add(&self, subject: &str, amount: i128) {
        *self.map.entry(subject.to_string()).or_insert(0) += amount;
    }

    pub fn subtract(&self, subject: &str, amount: i128) {
        if let Some(mut entry) = self.map.get_mut(subject) {
            *entry -= amount;
            if *entry == 0 {
                drop(entry);
                self.map.remove(subject);
            }
        }
    }

    /// M7: unshipped charge total for one subject (`user_id` or `api_key_id`).
    pub fn outstanding(&self, subject: &str) -> i128 {
        self.map.get(subject).map(|value| *value).unwrap_or(0)
    }
}

fn delta_subject(delta: &BalanceDelta) -> Option<String> {
    match delta.kind.as_str() {
        "request_charge" => Some(delta.user_id.clone()),
        "api_key_charge" => delta.api_key_id.clone(),
        _ => None,
    }
}

// ---------------------------------------------------------------------------
// Replica identity (M9)
// ---------------------------------------------------------------------------

fn parse_uuid_v4(raw: &str) -> Option<String> {
    let parsed = uuid::Uuid::parse_str(raw).ok()?;
    (parsed.get_version_num() == 4).then(|| parsed.hyphenated().to_string())
}

/// M9: resolve the stable replica identity. A non-empty `configured` value
/// (`MONOIZE_REPLICA_ID`) wins and must be a version-4 UUID; otherwise the identity is
/// loaded from `{spool_dir}/replica-identity`, creating it atomically when absent or
/// corrupt. Errors are prefixed with `replica_id_invalid` or `replica_identity_unwritable`.
pub fn resolve_replica_identity(
    configured: Option<&str>,
    spool_dir: &std::path::Path,
) -> Result<String, String> {
    if let Some(raw) = configured.map(str::trim).filter(|value| !value.is_empty()) {
        return parse_uuid_v4(raw).ok_or_else(|| {
            format!("replica_id_invalid: `MONOIZE_REPLICA_ID` must be a UUID v4, got {raw:?}")
        });
    }
    let path = spool_dir.join(REPLICA_IDENTITY_FILE_NAME);
    match std::fs::read_to_string(&path) {
        Ok(content) => {
            if let Some(id) = parse_uuid_v4(content.trim()) {
                return Ok(id);
            }
            tracing::warn!(
                path = %path.display(),
                "replica identity file content is not a UUID v4; generating a new identity"
            );
        }
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => {}
        Err(error) => {
            tracing::warn!(
                path = %path.display(),
                error = %error,
                "replica identity file unreadable; generating a new identity"
            );
        }
    }
    let id = uuid::Uuid::new_v4().hyphenated().to_string();
    std::fs::create_dir_all(spool_dir)
        .map_err(|error| format!("replica_identity_unwritable: create dir: {error}"))?;
    let tmp_path = spool_dir.join(format!(".tmp-identity-{}", uuid::Uuid::new_v4().simple()));
    let write_result = (|| {
        use std::io::Write;
        let mut file = std::fs::File::create(&tmp_path)
            .map_err(|error| format!("replica_identity_unwritable: create temp file: {error}"))?;
        file.write_all(format!("{id}\n").as_bytes())
            .map_err(|error| format!("replica_identity_unwritable: write temp file: {error}"))?;
        file.sync_all()
            .map_err(|error| format!("replica_identity_unwritable: sync temp file: {error}"))?;
        std::fs::rename(&tmp_path, &path)
            .map_err(|error| format!("replica_identity_unwritable: publish identity: {error}"))
    })();
    if let Err(error) = write_result {
        let _ = std::fs::remove_file(&tmp_path);
        return Err(error);
    }
    Ok(id)
}

/// M4a eviction: drop heartbeat entries whose last sighting is older than
/// `HEARTBEAT_EVICT_INTERVALS` ship intervals. Called on each overview read so that
/// entries left behind by replaced replica identities eventually disappear.
pub fn evict_expired_heartbeats(
    map: &DashMap<String, ReplicaHeartbeatRecord>,
    now_unix_ms: i64,
    ship_interval: Duration,
) {
    let evict_after_ms =
        (ship_interval.as_millis() as i64).saturating_mul(HEARTBEAT_EVICT_INTERVALS as i64);
    map.retain(|_, record| now_unix_ms.saturating_sub(record.last_seen_unix_ms) <= evict_after_ms);
}

// ---------------------------------------------------------------------------
// Durable delta spool (M3)
// ---------------------------------------------------------------------------

#[derive(Debug)]
pub struct DeltaSpool {
    dir: PathBuf,
    max_bytes: u64,
    bytes: std::sync::atomic::AtomicU64,
    io_lock: tokio::sync::Mutex<()>,
}

impl DeltaSpool {
    pub fn new(dir: PathBuf, max_bytes: u64) -> Result<Self, String> {
        std::fs::create_dir_all(&dir)
            .map_err(|error| format!("metering_spool_unwritable: create dir: {error}"))?;
        let probe = dir.join(format!(".write-probe-{}", uuid::Uuid::new_v4().simple()));
        match std::fs::OpenOptions::new()
            .write(true)
            .create_new(true)
            .open(&probe)
        {
            Ok(file) => {
                drop(file);
                let _ = std::fs::remove_file(&probe);
            }
            Err(error) => {
                return Err(format!("metering_spool_unwritable: {error}"));
            }
        }
        let mut total = 0u64;
        let entries =
            std::fs::read_dir(&dir).map_err(|error| format!("open delta spool dir: {error}"))?;
        for entry in entries.flatten() {
            let path = entry.path();
            if path.extension().and_then(|ext| ext.to_str()) == Some("json") {
                if let Ok(meta) = entry.metadata() {
                    total += meta.len();
                }
            } else if path.file_name().and_then(|name| name.to_str())
                != Some(REPLICA_IDENTITY_FILE_NAME)
            {
                // Remove stale temporary files from an interrupted previous run; the
                // persisted replica identity (M9b) must survive this cleanup.
                let _ = std::fs::remove_file(&path);
            }
        }
        Ok(Self {
            dir,
            max_bytes: max_bytes.max(1),
            bytes: std::sync::atomic::AtomicU64::new(total),
            io_lock: tokio::sync::Mutex::new(()),
        })
    }

    fn list_json_files(&self) -> Vec<(String, u64)> {
        match std::fs::read_dir(&self.dir) {
            Ok(entries) => entries
                .flatten()
                .filter_map(|entry| {
                    let path = entry.path();
                    if path.extension().and_then(|ext| ext.to_str()) != Some("json") {
                        return None;
                    }
                    let size = entry.metadata().ok()?.len();
                    let name = path.file_name()?.to_str()?.to_string();
                    Some((name, size))
                })
                .collect(),
            Err(_) => Vec::new(),
        }
    }

    pub fn reconstruct_pending_amounts(&self) -> Vec<(String, i128)> {
        let mut names = self.list_json_files();
        names.sort_by(|left, right| left.0.cmp(&right.0));
        let mut amounts = Vec::new();
        for (name, _) in names {
            let path = self.dir.join(&name);
            match std::fs::read(&path) {
                Ok(bytes) => match serde_json::from_slice::<BalanceDelta>(&bytes) {
                    Ok(delta) => {
                        let Some(subject) = delta_subject(&delta) else {
                            continue;
                        };
                        let Ok(amount) = delta.amount_nano_usd.trim().parse::<i128>() else {
                            continue;
                        };
                        amounts.push((subject, amount));
                    }
                    Err(error) => {
                        tracing::warn!(
                            file = %name,
                            error = %error,
                            "skipping unreadable delta spool file during pending reconstruction"
                        );
                    }
                },
                Err(error) => {
                    tracing::warn!(
                        file = %name,
                        error = %error,
                        "skipping unreadable delta spool file during pending reconstruction"
                    );
                }
            }
        }
        amounts
    }

    pub fn pending_files(&self) -> usize {
        self.list_json_files().len()
    }

    /// `(pending file count, total pending bytes)` over the durable spool.
    pub fn pending_stats(&self) -> (usize, u64) {
        let files = self.list_json_files();
        let bytes = files.iter().map(|(_, size)| *size).sum();
        (files.len(), bytes)
    }

    pub fn dir_display(&self) -> String {
        self.dir.display().to_string()
    }

    pub async fn enqueue(&self, delta: &BalanceDelta) -> Result<(), String> {
        let payload = serde_json::to_vec(delta).map_err(|error| error.to_string())?;
        let _io_guard = self.io_lock.lock().await;
        let current = self.bytes.load(Ordering::Acquire);
        if current + payload.len() as u64 > self.max_bytes {
            return Err(format!(
                "replica metering spool quota exhausted ({}/{})",
                current, self.max_bytes
            ));
        }
        let name = format!(
            "{:020}-{}.json",
            chrono::Utc::now().timestamp_millis(),
            uuid::Uuid::new_v4().simple()
        );
        let final_path = self.dir.join(&name);
        let tmp_path = self.dir.join(format!(".tmp-{name}"));
        write_durable(&tmp_path, &payload).await?;
        if let Err(error) = tokio::fs::rename(&tmp_path, &final_path).await {
            let _ = tokio::fs::remove_file(&tmp_path).await;
            return Err(format!("publish delta spool file: {error}"));
        }
        if let Err(error) = sync_dir(&self.dir).await {
            tracing::warn!(error = %error, "delta spool directory sync failed");
        }
        self.bytes.fetch_add(payload.len() as u64, Ordering::AcqRel);
        Ok(())
    }

    pub async fn load_batch(&self, max_entries: usize) -> Vec<(PathBuf, u64, BalanceDelta)> {
        let _io_guard = self.io_lock.lock().await;
        let mut names: Vec<(String, u64)> = self.list_json_files();
        // Oldest first: the timestamp prefix sorts lexicographically.
        names.sort_by(|left, right| left.0.cmp(&right.0));
        let mut loaded = Vec::with_capacity(names.len());
        for (name, size) in names.into_iter().take(max_entries) {
            let path = self.dir.join(&name);
            match tokio::fs::read(&path).await {
                Ok(bytes) => match serde_json::from_slice::<BalanceDelta>(&bytes) {
                    Ok(delta) => loaded.push((path, size, delta)),
                    Err(error) => {
                        tracing::warn!(file = %name, error = %error, "skipping corrupt delta spool file");
                        let _ = tokio::fs::remove_file(&path).await;
                        self.bytes.fetch_sub(size, Ordering::AcqRel);
                    }
                },
                Err(error) => {
                    tracing::warn!(file = %name, error = %error, "reading delta spool file failed");
                }
            }
        }
        loaded
    }

    pub async fn release(&self, files: &[(PathBuf, u64)]) {
        let _io_guard = self.io_lock.lock().await;
        for (path, size) in files {
            match tokio::fs::remove_file(path).await {
                Ok(()) => {}
                Err(error) if error.kind() == std::io::ErrorKind::NotFound => {}
                Err(error) => {
                    tracing::warn!(path = %path.display(), error = %error, "removing shipped delta file failed");
                    continue;
                }
            }
            self.bytes.fetch_sub(*size, Ordering::AcqRel);
        }
    }
}

async fn write_durable(path: &std::path::Path, payload: &[u8]) -> Result<(), String> {
    use tokio::io::AsyncWriteExt;
    let mut file = tokio::fs::File::create(path)
        .await
        .map_err(|error| format!("create temp file: {error}"))?;
    file.write_all(payload)
        .await
        .map_err(|error| format!("write temp file: {error}"))?;
    file.sync_all()
        .await
        .map_err(|error| format!("sync temp file: {error}"))?;
    Ok(())
}

async fn sync_dir(dir: &std::path::Path) -> Result<(), String> {
    let file = tokio::fs::File::open(dir)
        .await
        .map_err(|error| error.to_string())?;
    file.sync_all().await.map_err(|error| error.to_string())
}

// ---------------------------------------------------------------------------
// Replica-side metering context and shipment loop (M4–M6)
// ---------------------------------------------------------------------------

#[derive(Clone, Default)]
struct ShipExtras {
    last_used: Vec<LastUsedPair>,
    deltas: Vec<(PathBuf, u64, BalanceDelta)>,
}

impl ShipExtras {
    fn is_empty(&self) -> bool {
        self.last_used.is_empty() && self.deltas.is_empty()
    }

    fn into_batch(self, request_logs: Vec<SpoolRequestLog>) -> MeteringBatch {
        MeteringBatch {
            replica: None,
            request_logs,
            last_used: self.last_used,
            balance_deltas: self.deltas.into_iter().map(|(_, _, delta)| delta).collect(),
        }
    }

    fn trim_to_fit(&mut self, log_count: usize, hard_cap: usize) -> ShipExtras {
        let mut leftover = ShipExtras {
            last_used: Vec::new(),
            deltas: Vec::new(),
        };
        let mut remaining = hard_cap.saturating_sub(log_count);
        if self.deltas.len() > remaining {
            leftover.deltas = self.deltas.split_off(remaining);
        }
        remaining = remaining.saturating_sub(self.deltas.len());
        if self.last_used.len() > remaining {
            leftover.last_used = self.last_used.split_off(remaining);
        }
        leftover
    }
}

struct BatchSink<'a> {
    metering: &'a ReplicaMetering,
    last_used: &'a LastUsedBatcher,
    extras: Mutex<Option<ShipExtras>>,
    released: AtomicBool,
    deliver_attempted: AtomicBool,
}

impl<'a> BatchSink<'a> {
    fn take_extras(&self) -> Option<ShipExtras> {
        self.extras
            .lock()
            .unwrap_or_else(|err| err.into_inner())
            .take()
    }

    fn was_released(&self) -> bool {
        self.released.load(Ordering::Acquire)
    }

    fn deliver_attempted(&self) -> bool {
        self.deliver_attempted.load(Ordering::Acquire)
    }
}

#[async_trait::async_trait]
impl MeteringSink for BatchSink<'_> {
    async fn deliver(&self, entries: &[SpoolRequestLog]) -> Result<(), String> {
        self.deliver_attempted.store(true, Ordering::Release);
        let Some(extras) = self.take_extras() else {
            return Err("metering sink state unavailable".to_string());
        };
        self.metering
            .send_composed(entries.to_vec(), extras, self.last_used)
            .await?;
        self.released.store(true, Ordering::Release);
        Ok(())
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ShipTick {
    Idle,
    Success,
    Failure,
}

pub fn next_consecutive_failures(current: usize, tick: ShipTick) -> usize {
    match tick {
        ShipTick::Failure => current.saturating_add(1),
        ShipTick::Success | ShipTick::Idle => 0,
    }
}

#[derive(Clone)]
pub struct ReplicaHeartbeatSource {
    pub id: String,
    pub hostname: String,
    pub listen: String,
    pub version: String,
    pub started_at: String,
}

#[derive(Clone)]
pub struct ReplicaMetering {
    delta_spool: Arc<DeltaSpool>,
    pending: Arc<PendingDeductions>,
    client: reqwest::Client,
    endpoint: String,
    token: String,
    ship_batch_max: usize,
    heartbeat_source: Option<ReplicaHeartbeatSource>,
    ship_notify: Arc<tokio::sync::Notify>,
}

impl ReplicaMetering {
    pub fn new(
        spool_dir: PathBuf,
        spool_max_bytes: u64,
        primary_url: &str,
        token: &str,
        ship_batch_max: usize,
    ) -> Result<Self, String> {
        let client = reqwest::Client::builder()
            .user_agent("monoize/0.1")
            // PX4/PX8: cluster traffic bypasses every proxy including env-inherited ones.
            .no_proxy()
            .connect_timeout(Duration::from_secs(5))
            .timeout(Duration::from_secs(60))
            .build()
            .map_err(|error| error.to_string())?;
        let endpoint = format!(
            "{}{}",
            primary_url.trim_end_matches('/'),
            METERING_INGEST_PATH
        );
        let delta_spool = DeltaSpool::new(spool_dir, spool_max_bytes)?;
        let pending = PendingDeductions::default();
        for (subject, amount) in delta_spool.reconstruct_pending_amounts() {
            pending.add(&subject, amount);
        }
        Ok(Self {
            delta_spool: Arc::new(delta_spool),
            pending: Arc::new(pending),
            client,
            endpoint,
            token: token.to_string(),
            ship_batch_max: ship_batch_max.clamp(1, METERING_BATCH_HARD_CAP),
            heartbeat_source: None,
            ship_notify: Arc::new(tokio::sync::Notify::new()),
        })
    }

    pub fn with_heartbeat_source(mut self, source: ReplicaHeartbeatSource) -> Self {
        self.heartbeat_source = Some(source);
        self
    }

    fn current_heartbeat(&self) -> Option<ReplicaHeartbeat> {
        let source = self.heartbeat_source.as_ref()?;
        let started_at = chrono::DateTime::parse_from_rfc3339(&source.started_at)
            .ok()
            .map(|value| value.with_timezone(&chrono::Utc));
        let uptime_seconds = started_at
            .and_then(|started| {
                chrono::Utc::now()
                    .signed_duration_since(started)
                    .to_std()
                    .ok()
            })
            .map(|duration| duration.as_secs())
            .unwrap_or(0);
        let (spool_pending_count, spool_pending_bytes) = self.delta_spool.pending_stats();
        Some(ReplicaHeartbeat {
            id: source.id.clone(),
            hostname: source.hostname.clone(),
            listen: source.listen.clone(),
            version: source.version.clone(),
            started_at: source.started_at.clone(),
            uptime_seconds,
            spool_pending_count,
            spool_pending_bytes,
        })
    }

    pub fn pending(&self) -> &PendingDeductions {
        &self.pending
    }

    /// Charge-path entry point on replicas (M3): durable enqueue or fail; the caller
    /// maps failure onto the terminal billing-failure path per MB-C6.
    pub(crate) async fn enqueue_balance_delta_for_request(
        state: &crate::app::AppState,
        kind: &str,
        user_id: &str,
        api_key_id: Option<&str>,
        amount_nano_usd: i128,
        meta_json: &Value,
    ) -> Result<(), String> {
        let Some(metering) = state.metering.as_ref() else {
            return Err("metering pipeline unavailable on this node".to_string());
        };
        metering
            .enqueue_balance_delta(kind, user_id, api_key_id, amount_nano_usd, meta_json)
            .await
    }

    pub fn delta_spool(&self) -> &DeltaSpool {
        &self.delta_spool
    }

    /// M3: durable publish before success, plus atomic pending-counter increment.
    pub async fn enqueue_balance_delta(
        &self,
        kind: &str,
        user_id: &str,
        api_key_id: Option<&str>,
        amount_nano_usd: i128,
        meta_json: &Value,
    ) -> Result<(), String> {
        let delta = BalanceDelta {
            delta_id: uuid::Uuid::new_v4().to_string(),
            kind: kind.to_string(),
            user_id: user_id.to_string(),
            api_key_id: api_key_id.map(str::to_string),
            amount_nano_usd: amount_nano_usd.to_string(),
            meta_json: meta_json.clone(),
            created_at: chrono::Utc::now().to_rfc3339(),
        };
        validate_delta(&delta).map_err(|message| message.to_string())?;
        self.delta_spool.enqueue(&delta).await?;
        if let Some(subject) = delta_subject(&delta) {
            self.pending.add(&subject, amount_nano_usd);
        }
        self.ship_notify.notify_one();
        Ok(())
    }

    async fn post_batch(
        &self,
        request_logs: Vec<SpoolRequestLog>,
        extras: &ShipExtras,
    ) -> Result<(), String> {
        let mut batch = extras.clone().into_batch(request_logs);
        batch.replica = self.current_heartbeat();
        if batch.is_empty() && batch.replica.is_none() {
            return Ok(());
        }
        let response = self
            .client
            .post(&self.endpoint)
            .bearer_auth(&self.token)
            .json(&batch)
            .send()
            .await
            .map_err(|error| format!("metering transport error: {error}"))?;
        let status = response.status();
        if status == StatusCode::TOO_MANY_REQUESTS || status.as_u16() == 413 {
            return Err(format!("primary rejected batch as too large ({status})"));
        }
        if !status.is_success() {
            let body = response.text().await.unwrap_or_default();
            return Err(format!("primary returned {status}: {body}"));
        }
        let _ack: MeteringAck = response
            .json()
            .await
            .map_err(|error| format!("invalid metering ack body: {error}"))?;
        Ok(())
    }

    async fn release_extras(&self, extras: &ShipExtras) {
        let files: Vec<(PathBuf, u64)> = extras
            .deltas
            .iter()
            .map(|(path, size, _)| (path.clone(), *size))
            .collect();
        self.delta_spool.release(&files).await;
        for delta in extras.deltas.iter().map(|(_, _, delta)| delta) {
            if let Some(subject) = delta_subject(delta) {
                let amount = delta.amount_nano_usd.trim().parse::<i128>().unwrap_or(0);
                self.pending.subtract(&subject, amount);
            }
        }
        metrics::counter!("monoize_replica_metering_shipped_total", "result" => "ok").increment(1);
        metrics::gauge!("monoize_replica_metering_pending_entries")
            .set(self.delta_spool.pending_files() as f64);
    }

    fn requeue_last_used(&self, pairs: Vec<LastUsedPair>, last_used: &LastUsedBatcher) {
        for pair in pairs {
            if let Ok(timestamp) = chrono::DateTime::parse_from_rfc3339(&pair.last_used_at) {
                last_used.record_retry(pair.api_key_id, timestamp.with_timezone(&chrono::Utc));
            }
        }
    }

    async fn send_composed(
        &self,
        request_logs: Vec<SpoolRequestLog>,
        mut extras: ShipExtras,
        last_used: &LastUsedBatcher,
    ) -> Result<(), String> {
        let leftover = extras.trim_to_fit(request_logs.len(), METERING_BATCH_HARD_CAP);
        self.requeue_last_used(leftover.last_used, last_used);
        match self.post_batch(request_logs, &extras).await {
            Ok(()) => {
                self.release_extras(&extras).await;
                Ok(())
            }
            Err(error) => {
                self.requeue_last_used(extras.last_used, last_used);
                metrics::counter!("monoize_replica_metering_shipped_total", "result" => "error")
                    .increment(1);
                Err(error)
            }
        }
    }

    /// One M4 tick: at most one POST carrying request logs, last-used pairs, and deltas.
    pub async fn ship_once(
        &self,
        log_batcher: &RequestLogBatcher,
        last_used: &LastUsedBatcher,
    ) -> ShipTick {
        let last_used_pairs = last_used
            .drain_limit(METERING_BATCH_HARD_CAP)
            .into_iter()
            .map(|(id, timestamp)| LastUsedPair {
                api_key_id: id,
                last_used_at: timestamp.to_rfc3339(),
            })
            .collect::<Vec<_>>();
        let delta_files = self.delta_spool.load_batch(self.ship_batch_max).await;
        let extras = ShipExtras {
            last_used: last_used_pairs,
            deltas: delta_files,
        };

        let sink = BatchSink {
            metering: self,
            last_used,
            extras: Mutex::new(Some(extras)),
            released: AtomicBool::new(false),
            deliver_attempted: AtomicBool::new(false),
        };
        let _shipped_logs = log_batcher.ship_via(self.ship_batch_max, &sink).await;
        if sink.was_released() {
            return ShipTick::Success;
        }
        if sink.deliver_attempted() {
            return ShipTick::Failure;
        }

        let leftovers = sink.take_extras().unwrap_or_default();
        if leftovers.is_empty() && self.heartbeat_source.is_none() {
            return ShipTick::Idle;
        }
        match self.send_composed(Vec::new(), leftovers, last_used).await {
            Ok(()) => ShipTick::Success,
            Err(error) => {
                tracing::warn!(error = %error, "replica metering shipment failed");
                ShipTick::Failure
            }
        }
    }

    pub fn spawn_ship_loop(
        self: &Arc<Self>,
        log_batcher: RequestLogBatcher,
        last_used: LastUsedBatcher,
        interval: Duration,
    ) -> tokio::task::JoinHandle<()> {
        let metering = self.clone();
        tokio::spawn(async move {
            let mut ticker = tokio::time::interval(interval);
            ticker.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Delay);
            let mut consecutive_failures = 0usize;
            let log_notify = log_batcher.ship_notify();
            let delta_notify = metering.ship_notify.clone();
            loop {
                tokio::select! {
                    _ = ticker.tick() => {}
                    _ = log_notify.notified() => {}
                    _ = delta_notify.notified() => {}
                }
                let tick = metering.ship_once(&log_batcher, &last_used).await;
                consecutive_failures = next_consecutive_failures(consecutive_failures, tick);
                if consecutive_failures >= 3 {
                    tracing::warn!(
                        consecutive_failures,
                        "replica metering shipments keep failing; data remains durably spooled"
                    );
                }
                metrics::gauge!("monoize_replica_metering_pending_entries")
                    .set(metering.delta_spool.pending_files() as f64);
            }
        })
    }

    /// M6: best-effort single attempt at graceful shutdown.
    pub async fn final_ship(&self, log_batcher: &RequestLogBatcher, last_used: &LastUsedBatcher) {
        let _ = tokio::time::timeout(
            Duration::from_secs(10),
            self.ship_once(log_batcher, last_used),
        )
        .await;
    }
}

// ---------------------------------------------------------------------------
// Primary-side ingest endpoint (I1–I6)
// ---------------------------------------------------------------------------

pub fn sha256_hex_lower(input: &str) -> [u8; 32] {
    use sha2::{Digest, Sha256};
    let digest = Sha256::digest(input.as_bytes());
    let mut out = [0u8; 32];
    out.copy_from_slice(&digest);
    out
}

fn constant_time_eq(left: &[u8], right: &[u8]) -> bool {
    if left.len() != right.len() {
        return false;
    }
    let mut diff = 0u8;
    for (a, b) in left.iter().zip(right.iter()) {
        diff |= a ^ b;
    }
    diff == 0
}

fn metering_error(
    status: StatusCode,
    code: &str,
    message: impl Into<String>,
) -> axum::response::Response {
    (
        status,
        axum::Json(serde_json::json!({ "error": { "code": code, "message": message.into() } })),
    )
        .into_response()
}

/// I2: bearer compared by SHA-256 digest equality (constant-time).
pub(crate) fn verify_ingest_token(headers: &HeaderMap, expected_digest: &[u8; 32]) -> bool {
    let provided = headers
        .get(axum::http::header::AUTHORIZATION)
        .and_then(|value| value.to_str().ok())
        .and_then(|raw| raw.strip_prefix("Bearer "))
        .map(str::trim);
    let Some(provided) = provided else {
        return false;
    };
    constant_time_eq(&sha256_hex_lower(provided), expected_digest)
}

pub(crate) async fn ingest_metering_handler(
    State(state): State<crate::app::AppState>,
    headers: HeaderMap,
    body: axum::body::Bytes,
) -> axum::response::Response {
    let Some(expected) = state.metering_token_digest else {
        return metering_error(
            StatusCode::NOT_FOUND,
            "not_found",
            "ingest endpoint disabled",
        );
    };
    if !verify_ingest_token(&headers, &expected) {
        return metering_error(
            StatusCode::UNAUTHORIZED,
            "replica_auth_failed",
            "invalid replica token",
        );
    }
    let batch: MeteringBatch = match serde_json::from_slice(&body) {
        Ok(batch) => batch,
        Err(error) => {
            return metering_error(
                StatusCode::UNPROCESSABLE_ENTITY,
                "metering_batch_invalid",
                error.to_string(),
            );
        }
    };
    if batch.total_entries() > METERING_BATCH_HARD_CAP {
        return metering_error(
            StatusCode::PAYLOAD_TOO_LARGE,
            "metering_batch_too_large",
            format!("batch exceeds hard cap of {METERING_BATCH_HARD_CAP} entries"),
        );
    }
    for pair in &batch.last_used {
        if pair.api_key_id.is_empty() || pair.last_used_at.is_empty() {
            return metering_error(
                StatusCode::UNPROCESSABLE_ENTITY,
                "metering_batch_invalid",
                "last_used entries require api_key_id and last_used_at",
            );
        }
    }
    for delta in &batch.balance_deltas {
        if let Err(message) = validate_delta(delta) {
            return metering_error(
                StatusCode::UNPROCESSABLE_ENTITY,
                "metering_batch_invalid",
                message,
            );
        }
        if delta.kind == "api_key_charge" && delta.api_key_id.as_deref().unwrap_or("").is_empty() {
            return metering_error(
                StatusCode::UNPROCESSABLE_ENTITY,
                "metering_batch_invalid",
                "api_key_charge requires api_key_id",
            );
        }
    }

    if let Some(replica) = batch.replica.clone() {
        if replica.id.trim().is_empty() {
            return metering_error(
                StatusCode::UNPROCESSABLE_ENTITY,
                "metering_batch_invalid",
                "replica.id must be non-empty",
            );
        }
        let now_ms = chrono::Utc::now().timestamp_millis();
        state.replica_heartbeats.insert(
            replica.id.clone(),
            ReplicaHeartbeatRecord {
                heartbeat: replica,
                last_seen_unix_ms: now_ms,
            },
        );
    }

    match apply_metering_batch(&state.db_pool, &state.user_store, &batch).await {
        Ok(ack) => {
            if !batch.request_logs.is_empty() {
                let rows = batch
                    .request_logs
                    .iter()
                    .map(SpoolRequestLog::to_insert_log)
                    .collect::<Vec<_>>();
                let _ = state.log_broadcast.send(rows);
            }
            axum::Json(ack).into_response()
        }
        Err(error) => {
            tracing::warn!(error = %error, "metering batch apply failed; replica will retry");
            metering_error(
                StatusCode::INTERNAL_SERVER_ERROR,
                "metering_apply_failed",
                error,
            )
        }
    }
}

// ---------------------------------------------------------------------------
// Idempotent apply (I4–I5), also reused by the PRP9 promotion drain
// ---------------------------------------------------------------------------

const LAST_USED_CHUNK_ENTRIES: usize = 256;

pub async fn apply_metering_batch(
    db: &DbPool,
    user_store: &crate::users::UserStore,
    batch: &MeteringBatch,
) -> Result<MeteringAck, String> {
    use sea_orm::{ConnectionTrait, TransactionTrait};

    let write = db.write().await;
    let tx = write.begin().await.map_err(|error| error.to_string())?;

    let mut applied_request_logs = 0u64;
    let mut applied_balance_deltas = 0u64;
    let mut apply_error: Option<String> = None;

    for chunk in batch
        .request_logs
        .chunks(crate::db_cache::REQUEST_LOG_INSERT_CHUNK_ENTRIES)
    {
        let (sql, values) = crate::db_cache::request_log_insert_chunk(chunk.iter());
        match tx.execute(db.stmt(&sql, values)).await {
            Ok(outcome) => applied_request_logs += outcome.rows_affected(),
            Err(error) => {
                apply_error = Some(error.to_string());
                break;
            }
        }
    }

    if apply_error.is_none() {
        for chunk in batch.last_used.chunks(LAST_USED_CHUNK_ENTRIES) {
            let pairs = chunk
                .iter()
                .filter_map(|pair| {
                    chrono::DateTime::parse_from_rfc3339(&pair.last_used_at)
                        .ok()
                        .map(|parsed| (pair.api_key_id.clone(), parsed.with_timezone(&chrono::Utc)))
                })
                .collect::<Vec<_>>();
            if pairs.is_empty() {
                continue;
            }
            let (sql, values) = crate::db_cache::last_used_bulk_update(&pairs);
            if let Err(error) = tx.execute(db.stmt(&sql, values)).await {
                apply_error = Some(error.to_string());
                break;
            }
        }
    }

    if apply_error.is_none() {
        for delta in &batch.balance_deltas {
            match apply_balance_delta(db, user_store, &tx, delta).await {
                Ok(count) => applied_balance_deltas += count,
                Err(error) => {
                    apply_error = Some(error);
                    break;
                }
            }
        }
    }

    if let Some(error) = apply_error {
        if let Err(rollback_error) = tx.rollback().await {
            tracing::warn!("metering apply rollback error: {rollback_error}");
        }
        return Err(error);
    }

    tx.commit().await.map_err(|error| error.to_string())?;
    metrics::counter!("monoize_primary_metering_applied_total").increment(applied_balance_deltas);
    Ok(MeteringAck {
        applied_request_logs,
        applied_last_used: batch.last_used.len() as u64,
        applied_balance_deltas,
    })
}

/// Apply one replica charge atomically. The pre-check avoids replaying plan usage. The
/// unique ledger key remains the database idempotency anchor for concurrent applies.
async fn apply_balance_delta(
    db: &DbPool,
    user_store: &crate::users::UserStore,
    tx: &sea_orm::DatabaseTransaction,
    delta: &BalanceDelta,
) -> Result<u64, String> {
    let already_applied = tx
        .query_one(db.stmt(
            "SELECT id FROM billing_ledger WHERE idempotency_key = $1",
            vec![delta.delta_id.clone().into()],
        ))
        .await
        .map_err(|error| error.to_string())?
        .is_some();
    if already_applied {
        return Ok(0);
    }
    let amount = delta
        .amount_nano_usd
        .trim()
        .parse::<i128>()
        .map_err(|_| "amount_nano_usd must be decimal i128 text".to_string())?;
    let request_id = delta
        .meta_json
        .get("request_id")
        .and_then(Value::as_str)
        .filter(|value| !value.trim().is_empty())
        .ok_or_else(|| "replica charge metadata is missing request_id".to_string())?;
    let billing_group_id = delta
        .meta_json
        .get("billing_group_id")
        .and_then(Value::as_str);
    let user = user_store
        .lock_user_balance_tx(tx, &delta.user_id)
        .await
        .map_err(|error| error.message)?;
    let now = chrono::Utc::now();

    let mut sub_account: Option<(String, i128)> = None;
    let mut missing_key = false;
    if delta.kind == "api_key_charge" {
        let api_key_id = delta.api_key_id.as_deref().unwrap_or_default();
        let lock_suffix = if db.is_sqlite() { "" } else { " FOR UPDATE" };
        let row = tx
            .query_one(db.stmt(
                &format!("SELECT user_id, sub_account_enabled, sub_account_balance_nano FROM api_keys WHERE id = $1{lock_suffix}"),
                vec![api_key_id.into()],
            ))
            .await
            .map_err(|error| error.to_string())?;
        match row {
            None => missing_key = true,
            Some(row) => {
                let owner: String = row
                    .try_get("", "user_id")
                    .map_err(|error| error.to_string())?;
                if owner != delta.user_id {
                    return Err("replica charge API key owner mismatch".to_string());
                }
                let enabled = row
                    .try_get::<Option<i32>>("", "sub_account_enabled")
                    .map_err(|error| error.to_string())?
                    .unwrap_or(0)
                    != 0;
                if enabled {
                    let raw = row
                        .try_get::<Option<String>>("", "sub_account_balance_nano")
                        .map_err(|error| error.to_string())?
                        .unwrap_or_else(|| "0".to_string());
                    let current = raw
                        .parse::<i128>()
                        .map_err(|_| "malformed stored sub-account balance".to_string())?;
                    sub_account = Some((api_key_id.to_string(), current));
                }
            }
        }
    }

    let bypass = missing_key || (sub_account.is_none() && user.unlimited);
    let allocation = if bypass {
        crate::users::PlanChargeAllocation {
            adjusted_charge_nano_usd: amount,
            plan_covered_nano_usd: 0,
            fallback_nano_usd: 0,
            subscription_id: None,
            plan_id: None,
            multiplier: None,
        }
    } else {
        user_store
            .allocate_plan_charge_tx(
                tx,
                &delta.user_id,
                delta.api_key_id.as_deref(),
                billing_group_id,
                request_id,
                amount,
                now,
            )
            .await
            .map_err(|error| error.message)?
    };

    let balance_after = if let Some((api_key_id, current)) = sub_account {
        let next = checked_sub_allow_negative(current, allocation.fallback_nano_usd)?;
        if allocation.fallback_nano_usd > 0 {
            tx.execute(db.stmt(
                "UPDATE api_keys SET sub_account_balance_nano = $1 WHERE id = $2",
                vec![next.to_string().into(), api_key_id.into()],
            ))
            .await
            .map_err(|error| error.to_string())?;
        }
        Some(next)
    } else if bypass {
        None
    } else {
        let next = checked_sub_allow_negative(user.balance, allocation.fallback_nano_usd)?;
        if allocation.fallback_nano_usd > 0 {
            tx.execute(db.stmt(
                "UPDATE users SET balance_nano_usd = $1, updated_at = $2 WHERE id = $3",
                vec![
                    next.to_string().into(),
                    now.to_rfc3339().into(),
                    delta.user_id.clone().into(),
                ],
            ))
            .await
            .map_err(|error| error.to_string())?;
        }
        Some(next)
    };

    let ledger_meta = crate::users::plan_charge_meta(&delta.meta_json, &allocation);
    let conflict_clause =
        "ON CONFLICT(idempotency_key) WHERE idempotency_key IS NOT NULL DO NOTHING";
    let sql = format!(
        "INSERT INTO billing_ledger (id, user_id, kind, delta_nano_usd, balance_after_nano_usd, meta_json, created_at, idempotency_key) \
         VALUES ($1, $2, $3, $4, $5, $6, $7, $8) {conflict_clause}"
    );
    let insert_result = tx
        .execute(db.stmt(
            &sql,
            vec![
                delta.delta_id.clone().into(),
                delta.user_id.clone().into(),
                delta.kind.clone().into(),
                (-allocation.fallback_nano_usd).to_string().into(),
                balance_after.map(|value| value.to_string()).into(),
                ledger_meta.to_string().into(),
                delta.created_at.clone().into(),
                delta.delta_id.clone().into(),
            ],
        ))
        .await
        .map_err(|error| error.to_string())?;
    if insert_result.rows_affected() == 0 {
        return Err("replica delta idempotency raced with another apply".to_string());
    }

    let breakdown = tx
        .query_one(db.stmt(
            "SELECT billing_breakdown_json FROM request_logs WHERE request_id = $1",
            vec![request_id.into()],
        ))
        .await
        .map_err(|error| error.to_string())?
        .and_then(|row| {
            row.try_get::<Option<String>>("", "billing_breakdown_json")
                .ok()
                .flatten()
        })
        .and_then(|raw| serde_json::from_str::<Value>(&raw).ok())
        .map(|mut value| {
            if let Some(object) = value.as_object_mut() {
                let pre_plan = object
                    .get("final_charge_nano")
                    .cloned()
                    .unwrap_or(Value::Null);
                object.insert("pre_plan_charge_nano".to_string(), pre_plan);
                object.insert(
                    "final_charge_nano".to_string(),
                    Value::String(allocation.adjusted_charge_nano_usd.to_string()),
                );
                object.insert(
                    "plan".to_string(),
                    serde_json::json!({
                        "subscription_id": allocation.subscription_id,
                        "plan_id": allocation.plan_id,
                        "multiplier": allocation.multiplier,
                        "covered_nano": allocation.plan_covered_nano_usd.to_string(),
                        "fallback_nano": allocation.fallback_nano_usd.to_string(),
                    }),
                );
            }
            value.to_string()
        });
    tx.execute(db.stmt(
        "UPDATE request_logs SET charge_nano_usd = $1, billing_breakdown_json = COALESCE($2, billing_breakdown_json) WHERE request_id = $3",
        vec![allocation.adjusted_charge_nano_usd.to_string().into(), breakdown.into(), request_id.into()],
    ))
    .await
    .map_err(|error| error.to_string())?;
    Ok(1)
}

fn checked_sub_allow_negative(current: i128, amount: i128) -> Result<i128, String> {
    // I5: negative results are allowed; only genuine i128 overflow aborts the batch.
    current
        .checked_sub(amount)
        .ok_or_else(|| "balance overflow".to_string())
}

/// PRP9: drain leftover delta spool entries directly into the local database when a
/// former replica starts as the primary. Runs after migrations, before serving traffic.
pub async fn drain_delta_spool_to_local_db(
    db: &DbPool,
    user_store: &crate::users::UserStore,
    spool: &DeltaSpool,
) -> Result<(), String> {
    loop {
        let files = spool.load_batch(METERING_BATCH_HARD_CAP).await;
        if files.is_empty() {
            return Ok(());
        }
        let batch = MeteringBatch {
            replica: None,
            request_logs: Vec::new(),
            last_used: Vec::new(),
            balance_deltas: files.iter().map(|(_, _, delta)| delta.clone()).collect(),
        };
        let ack = apply_metering_batch(db, user_store, &batch)
            .await
            .map_err(|error| {
                format!(
                    "promotion drain failed: {error}; spool entries preserved at {}",
                    spool.dir_display()
                )
            })?;
        // Every entry is now either applied or already present server-side under the
        // same idempotency key, so releasing all of them is safe.
        let refs: Vec<(PathBuf, u64)> = files
            .iter()
            .map(|(path, size, _)| (path.clone(), *size))
            .collect();
        let _ = ack.applied_balance_deltas;
        spool.release(&refs).await;
        if files.len() < METERING_BATCH_HARD_CAP {
            return Ok(());
        }
    }
}
