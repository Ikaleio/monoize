//! Metering pipeline between replica and primary (`primary-replica-deployment.spec.md`
//! sections 6–7): durable balance-delta spool on replicas, batched shipment over the
//! internal HTTP API, idempotent apply inside one transaction on the primary, and the
//! PRP9 promotion drain.

mod primary;
mod spool;
pub(crate) use primary::ingest_metering_handler;
pub use primary::{apply_metering_batch, drain_delta_spool_to_local_db, sha256_hex_lower};
pub use spool::DeltaSpool;

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
