use crate::auth::AuthState;
use crate::billing_rate_store::{BillingRateStore, DbBillingRateRecord};
use crate::captcha::CapVerifier;
use crate::client_ip::TrustedProxyConfig;
use crate::db::DbPool;
use crate::error::{AppError, AppResult};
use crate::exact_decimal::Multiplier;
use crate::handlers::routing::health_key;
use crate::image_transform_cache::ImageTransformCache;
use crate::model_registry_store::ModelRegistryStore;
use crate::monoize_routing::{
    ChannelAffinityBinding, ChannelHealthState, MonoizeRoutingStore, MonoizeRuntimeConfig,
    probe_channel_completion,
};
use crate::node_config::{HttpClients, NodeRole, NodeSettings};
use crate::request_capture::RequestCaptureStore;
use crate::settings::{PricingProfilePattern, SettingsStore, normalize_pricing_model_key};
use crate::transforms::TransformRegistry;
use crate::users::{InsertRequestLog, UserRole, UserStore};
use axum::Router;
use axum::body::Body;
use axum::extract::DefaultBodyLimit;
use axum::http::{Request, header};
use axum::response::IntoResponse;
use axum::routing::{get, post, put};
use dashmap::DashMap;
use metrics_exporter_prometheus::PrometheusHandle;
use serde_json::{Value, json};
use std::collections::HashMap;
use std::sync::atomic::{AtomicBool, AtomicU64, AtomicUsize, Ordering};
use std::sync::{Arc, Once, OnceLock};
use tokio::sync::Mutex;
use tokio::time::sleep;
use tower_http::cors::CorsLayer;
use tower_http::limit::RequestBodyLimitLayer;
use tower_http::request_id::{
    MakeRequestUuid, PropagateRequestIdLayer, RequestId, SetRequestIdLayer,
};
use tower_http::set_header::SetResponseHeaderLayer;
use tower_http::trace::TraceLayer;

#[derive(Clone)]
pub struct RequestLogTaskTracker {
    inner: Arc<RequestLogTaskTrackerInner>,
}

struct RequestLogTaskTrackerInner {
    active: std::sync::Mutex<usize>,
    updates: tokio::sync::watch::Sender<usize>,
}

impl Default for RequestLogTaskTracker {
    fn default() -> Self {
        let (updates, _) = tokio::sync::watch::channel(0);
        Self {
            inner: Arc::new(RequestLogTaskTrackerInner {
                active: std::sync::Mutex::new(0),
                updates,
            }),
        }
    }
}

impl RequestLogTaskTracker {
    pub(crate) fn register(&self) {
        let mut active = self
            .inner
            .active
            .lock()
            .unwrap_or_else(|error| error.into_inner());
        *active = active.saturating_add(1);
        self.inner.updates.send_replace(*active);
    }

    pub(crate) fn complete(&self) {
        let mut active = self
            .inner
            .active
            .lock()
            .unwrap_or_else(|error| error.into_inner());
        if *active == 0 {
            tracing::error!("request-log terminal task tracker underflow");
            return;
        }
        *active -= 1;
        self.inner.updates.send_replace(*active);
    }

    pub fn active_count(&self) -> usize {
        *self
            .inner
            .active
            .lock()
            .unwrap_or_else(|error| error.into_inner())
    }

    pub async fn wait_for_idle(&self) {
        let mut updates = self.inner.updates.subscribe();
        loop {
            if *updates.borrow_and_update() == 0 {
                return;
            }
            if updates.changed().await.is_err() {
                return;
            }
        }
    }
}

struct RequestLogTaskRegistration {
    tracker: RequestLogTaskTracker,
}

impl RequestLogTaskRegistration {
    fn new(tracker: RequestLogTaskTracker) -> Self {
        tracker.register();
        Self { tracker }
    }
}

impl Drop for RequestLogTaskRegistration {
    fn drop(&mut self) {
        self.tracker.complete();
    }
}

pub struct RequestLogLifecycle {
    reservation: crate::db_cache::RequestLogReservation,
    terminal_scheduled: AtomicBool,
    tracker_completed: AtomicBool,
    tracker: RequestLogTaskTracker,
}

impl RequestLogLifecycle {
    pub(crate) fn new(
        reservation: crate::db_cache::RequestLogReservation,
        tracker: RequestLogTaskTracker,
    ) -> Self {
        tracker.register();
        Self {
            reservation,
            terminal_scheduled: AtomicBool::new(false),
            tracker_completed: AtomicBool::new(false),
            tracker,
        }
    }

    pub(crate) fn try_schedule_terminal(&self) -> Option<crate::db_cache::RequestLogReservation> {
        self.terminal_scheduled
            .compare_exchange(false, true, Ordering::AcqRel, Ordering::Acquire)
            .ok()
            .map(|_| self.reservation.clone())
    }

    #[cfg(test)]
    pub(crate) fn terminal_scheduled(&self) -> bool {
        self.terminal_scheduled.load(Ordering::Acquire)
    }

    pub(crate) fn complete_terminal_task(&self) {
        if !self.tracker_completed.swap(true, Ordering::AcqRel) {
            self.tracker.complete();
        }
    }
}

impl Drop for RequestLogLifecycle {
    fn drop(&mut self) {
        if !self.tracker_completed.swap(true, Ordering::AcqRel) {
            self.tracker.complete();
        }
    }
}

#[derive(Clone)]
pub struct AppState {
    pub runtime: Arc<RuntimeConfig>,
    /// Process start instant, captured once when this state is built.
    pub started_at: chrono::DateTime<chrono::Utc>,
    pub auth: AuthState,
    pub http: reqwest::Client,
    pub http_clients: HttpClients,
    pub node: Arc<NodeSettings>,
    pub db_pool: DbPool,
    /// Present on replicas; drives the metering shipment pipeline.
    pub metering: Option<Arc<crate::replica::metering::ReplicaMetering>>,
    /// SHA-256 digest of MONOIZE_REPLICA_TOKEN on primaries with ingest enabled.
    pub metering_token_digest: Option<[u8; 32]>,
    /// Process-local replica heartbeats observed on the primary ingest path.
    pub replica_heartbeats: Arc<DashMap<String, crate::replica::metering::ReplicaHeartbeatRecord>>,
    pub metrics: PrometheusHandle,
    pub user_store: UserStore,
    pub settings_store: SettingsStore,
    pub monoize_store: MonoizeRoutingStore,
    pub monoize_runtime: Arc<tokio::sync::RwLock<MonoizeRuntimeConfig>>,
    pub channel_health: Arc<Mutex<HashMap<String, ChannelHealthState>>>,
    pub channel_affinity: Arc<Mutex<HashMap<String, ChannelAffinityBinding>>>,
    pub routing_config_revision: Arc<AtomicU64>,
    pub settings_update_lock: Arc<Mutex<()>>,
    pub model_registry_store: ModelRegistryStore,
    pub billing_rate_store: BillingRateStore,
    pub transform_registry: Arc<TransformRegistry>,
    pub cap_verifier: CapVerifier,
    pub log_broadcast: tokio::sync::broadcast::Sender<Vec<InsertRequestLog>>,
    pub pending_request_logs: Arc<DashMap<String, InsertRequestLog>>,
    pub request_log_admissions: Arc<DashMap<String, Arc<RequestLogLifecycle>>>,
    pub request_log_tasks: RequestLogTaskTracker,
    pub background_shutdown: Arc<AtomicBool>,
    pub sse_connections: Arc<DashMap<String, Arc<AtomicUsize>>>,
    pub image_transform_cache: Arc<ImageTransformCache>,
    pub request_capture: RequestCaptureStore,
    pub trusted_proxies: TrustedProxyConfig,
}

impl AppState {
    pub fn with_node_role(self, role: NodeRole) -> Self {
        let mut node = (*self.node).clone();
        node.role = role;
        Self {
            node: Arc::new(node),
            ..self
        }
    }
}

const ACTIVE_PROBE_CONNECTIVITY_KIND: &str = "active_probe_connectivity";
const ACTIVE_PROBE_SYSTEM_USER: &str = "_monoize_active_probe";
const DEFAULT_HTTP_BODY_MAX_BYTES: usize = 50 * 1024 * 1024;

static METRICS_HANDLE: OnceLock<PrometheusHandle> = OnceLock::new();
static METRICS_ERROR: OnceLock<AppError> = OnceLock::new();
static METRICS_INIT: Once = Once::new();

#[derive(Debug, Clone)]
pub struct RuntimeConfig {
    pub listen: String,
    pub metrics_path: String,
    pub database_dsn: String,
    pub request_log_spool_dir: Option<std::path::PathBuf>,
    pub node: NodeSettings,
}

impl RuntimeConfig {
    /// Errors carry `(error_code, detail)` and stop startup per PRP1/PRP7/PX2.
    pub fn from_env() -> Result<Self, (&'static str, String)> {
        let listen = std::env::var("MONOIZE_LISTEN")
            .ok()
            .filter(|v| !v.trim().is_empty())
            .unwrap_or_else(|| "0.0.0.0:8080".to_string());
        let metrics_path = std::env::var("MONOIZE_METRICS_PATH")
            .ok()
            .filter(|v| !v.trim().is_empty())
            .unwrap_or_else(|| "/metrics".to_string());
        let database_dsn = resolve_database_dsn();
        let node = NodeSettings::from_env()?;
        Ok(Self {
            listen,
            metrics_path,
            database_dsn,
            request_log_spool_dir: None,
            node,
        })
    }

    /// Test/programmatic construction with default (primary) node settings.
    pub fn with_defaults(listen: &str, metrics_path: &str, database_dsn: String) -> Self {
        Self {
            listen: listen.to_string(),
            metrics_path: metrics_path.to_string(),
            database_dsn,
            request_log_spool_dir: None,
            node: NodeSettings::primary_default(),
        }
    }
}

fn http_body_max_bytes() -> usize {
    http_body_max_bytes_from_raw(std::env::var("MONOIZE_HTTP_BODY_MAX_BYTES").ok().as_deref())
}

fn http_body_max_bytes_from_raw(raw: Option<&str>) -> usize {
    raw.map(str::trim)
        .filter(|value| !value.is_empty())
        .and_then(|value| value.parse::<usize>().ok())
        .filter(|value| *value > 0)
        .unwrap_or(DEFAULT_HTTP_BODY_MAX_BYTES)
}

pub async fn load_state() -> AppResult<AppState> {
    let runtime = RuntimeConfig::from_env().map_err(|(code, detail)| {
        AppError::new(axum::http::StatusCode::BAD_REQUEST, code, detail)
    })?;
    load_state_with_runtime(runtime).await
}

#[allow(clippy::field_reassign_with_default)]
pub async fn load_state_with_runtime(runtime: RuntimeConfig) -> AppResult<AppState> {
    let auth = AuthState::new();
    let is_replica = runtime.node.is_replica();
    runtime
        .node
        .validate_for_dsn(&runtime.database_dsn)
        .map_err(|(code, detail)| {
            AppError::new(axum::http::StatusCode::BAD_REQUEST, code, detail)
        })?;
    let trusted_proxies = TrustedProxyConfig::from_env().map_err(|error| {
        AppError::new(
            axum::http::StatusCode::BAD_REQUEST,
            "trusted_proxy_config_invalid",
            error,
        )
    })?;
    let cap_verifier = CapVerifier::from_env().map_err(|error| {
        AppError::new(
            axum::http::StatusCode::BAD_REQUEST,
            "cap_config_invalid",
            error,
        )
    })?;

    let http_clients =
        HttpClients::new(runtime.node.upstream_proxy_url.as_deref()).map_err(|err| {
            AppError::new(
                axum::http::StatusCode::BAD_REQUEST,
                "http_client_init_failed",
                err,
            )
        })?;
    let http = http_clients.global_client();

    let db = DbPool::connect(&runtime.database_dsn)
        .await
        .map_err(|err| {
            AppError::new(
                axum::http::StatusCode::BAD_REQUEST,
                "database_init_failed",
                err.to_string(),
            )
        })?;

    if !is_replica {
        let _write_guard = db.write().await;
        crate::migration::run_startup_migrations(&*_write_guard)
            .await
            .map_err(|err| {
                AppError::new(
                    axum::http::StatusCode::BAD_REQUEST,
                    "database_migration_failed",
                    err.to_string(),
                )
            })?;
    } else {
        // PRP10: replicas verify schema currency without writing.
        crate::migration::verify_schema_current(db.read())
            .await
            .map_err(|err| {
                let code = if err.to_string().contains("replica_schema_pending") {
                    "replica_schema_pending"
                } else {
                    "database_migration_failed"
                };
                AppError::new(axum::http::StatusCode::BAD_REQUEST, code, err.to_string())
            })?;
    }

    let (log_broadcast, _) = tokio::sync::broadcast::channel::<Vec<InsertRequestLog>>(64);

    let pending_request_logs = Arc::new(DashMap::new());
    let user_store = UserStore::new_for_role(
        db.clone(),
        log_broadcast.clone(),
        pending_request_logs.clone(),
        runtime.request_log_spool_dir.clone(),
        is_replica,
    )
    .await
    .map_err(|err| {
        AppError::new(
            axum::http::StatusCode::BAD_REQUEST,
            "user_store_init_failed",
            err,
        )
    })?;
    let settings_store = if is_replica {
        SettingsStore::new_read_only(db.clone()).await
    } else {
        SettingsStore::new(db.clone()).await
    }
    .map_err(|err| {
        AppError::new(
            axum::http::StatusCode::BAD_REQUEST,
            "settings_store_init_failed",
            err,
        )
    })?;
    let monoize_store = if is_replica {
        MonoizeRoutingStore::new_read_only(db.clone()).await
    } else {
        MonoizeRoutingStore::new(db.clone()).await
    }
    .map_err(|err| {
        AppError::new(
            axum::http::StatusCode::BAD_REQUEST,
            "monoize_store_init_failed",
            err,
        )
    })?;
    let model_registry_store = ModelRegistryStore::new(db.clone()).await.map_err(|err| {
        AppError::new(
            axum::http::StatusCode::BAD_REQUEST,
            "model_registry_store_init_failed",
            err,
        )
    })?;
    let billing_rate_store = BillingRateStore::new(db.clone()).await.map_err(|err| {
        AppError::new(
            axum::http::StatusCode::BAD_REQUEST,
            "billing_rate_store_init_failed",
            err,
        )
    })?;

    let metrics = init_metrics()?;

    let settings_snapshot = settings_store.get_all().await.map_err(|err| {
        AppError::new(
            axum::http::StatusCode::BAD_REQUEST,
            "settings_store_init_failed",
            err,
        )
    })?;

    let monoize_runtime = runtime_config_from_settings(&settings_snapshot);
    let channel_health = Arc::new(Mutex::new(HashMap::<String, ChannelHealthState>::new()));
    let channel_affinity = Arc::new(Mutex::new(HashMap::new()));
    let routing_config_revision = Arc::new(AtomicU64::new(0));
    let settings_update_lock = Arc::new(Mutex::new(()));
    let transform_registry = Arc::new(crate::transforms::registry());
    let image_transform_cache = Arc::new(ImageTransformCache::from_env().await.map_err(|err| {
        AppError::new(
            axum::http::StatusCode::BAD_REQUEST,
            "image_transform_cache_init_failed",
            err,
        )
    })?);
    image_transform_cache
        .as_ref()
        .clone()
        .spawn_cleanup_task(ImageTransformCache::default_cleanup_interval());
    let active_probe_user_id = if !is_replica {
        Some(ensure_active_probe_system_user(&user_store).await?)
    } else {
        None
    };
    let request_log_tasks = RequestLogTaskTracker::default();
    let background_shutdown = Arc::new(AtomicBool::new(false));
    {
        let affinity = channel_affinity.clone();
        let shutdown = background_shutdown.clone();
        tokio::spawn(async move {
            let interval = crate::monoize_routing::channel_affinity_cleanup_interval();
            loop {
                sleep(interval).await;
                if shutdown.load(Ordering::Acquire) {
                    break;
                }
                let now = chrono::Utc::now().timestamp();
                let mut guard = affinity.lock().await;
                crate::monoize_routing::cleanup_channel_affinity(&mut guard, now);
            }
        });
    }

    let probe_store = monoize_store.clone();
    let probe_http_clients = http_clients.clone();
    let monoize_runtime = Arc::new(tokio::sync::RwLock::new(monoize_runtime));
    if is_replica {
        // E3: fixed-interval, single-row epoch poll driving snapshot rebuilds.
        crate::replica::poll::spawn_config_epoch_poller(
            db.clone(),
            settings_store.clone(),
            monoize_runtime.clone(),
            runtime.node.config_poll_interval,
        );
    }
    let request_capture = RequestCaptureStore::new(&runtime.database_dsn).with_db(db.clone());
    request_capture.spawn_cleanup_task(monoize_runtime.clone());
    let probe_runtime = monoize_runtime.clone();
    let probe_health = channel_health.clone();
    let probe_routing_config_revision = routing_config_revision.clone();
    let probe_user_store = user_store.clone();
    let probe_model_registry_store = model_registry_store.clone();
    let probe_billing_rate_store = billing_rate_store.clone();
    let probe_user_id = active_probe_user_id;
    let probe_shutdown = background_shutdown.clone();
    let probe_task_registration = RequestLogTaskRegistration::new(request_log_tasks.clone());
    tokio::spawn(async move {
        let _probe_task_registration = probe_task_registration;
        let Some(probe_user_id) = probe_user_id else {
            // PRP11: replicas never run the active-probe scheduler.
            return;
        };
        'scheduler: loop {
            if probe_shutdown.load(Ordering::Acquire) {
                break;
            }
            sleep(std::time::Duration::from_secs(1)).await;
            if probe_shutdown.load(Ordering::Acquire) {
                break;
            }
            let routing_config_revision = probe_routing_config_revision.load(Ordering::Acquire);
            let providers = match probe_store.list_active_probe_candidates().await {
                Ok(v) => v,
                Err(_) => continue,
            };
            let now = chrono::Utc::now().timestamp();
            let rt_snap = probe_runtime.read().await.clone();
            let pricing_snapshot = match build_active_probe_pricing_snapshot(
                &probe_billing_rate_store,
                &probe_model_registry_store,
                &providers,
                &rt_snap,
            )
            .await
            {
                Ok(snapshot) => snapshot,
                Err(err) => {
                    tracing::warn!(error = %err, "active probe pricing snapshot failed");
                    ActiveProbePricingSnapshot::default()
                }
            };
            for provider in providers {
                if probe_shutdown.load(Ordering::Acquire) {
                    break 'scheduler;
                }
                if !provider.circuit_breaker_enabled {
                    continue;
                }
                for channel in provider.channels {
                    if probe_shutdown.load(Ordering::Acquire) {
                        break 'scheduler;
                    }
                    if channel.provider_type
                        == crate::monoize_routing::MonoizeProviderType::Replicate
                    {
                        continue;
                    }
                    let active_enabled = channel
                        .active_probe_enabled_override
                        .or(provider.active_probe_enabled_override)
                        .unwrap_or(rt_snap.active_enabled);
                    if !active_enabled {
                        continue;
                    }
                    let probe_interval_seconds = channel
                        .active_probe_interval_seconds_override
                        .or(provider.active_probe_interval_seconds_override)
                        .unwrap_or(rt_snap.active_interval_seconds)
                        .max(1);
                    let probe_success_threshold = channel
                        .active_probe_success_threshold_override
                        .or(provider.active_probe_success_threshold_override)
                        .unwrap_or(rt_snap.active_success_threshold)
                        .max(1);
                    let configured_model = channel
                        .active_probe_model_override
                        .clone()
                        .or(provider.active_probe_model_override.clone())
                        .or(rt_snap.active_probe_model.clone());
                    let first_model = channel.models.keys().min().cloned();
                    let probe_model = {
                        let guard = probe_health.lock().await;
                        if provider.per_model_circuit_break {
                            let mut due_models: Vec<String> = channel
                                .models
                                .keys()
                                .filter(|model| {
                                    guard
                                        .get(&health_key(&channel.id, Some(model)))
                                        .is_some_and(|state| {
                                            !state.healthy
                                                && state
                                                    .cooldown_until
                                                    .is_none_or(|until| now >= until)
                                                && state.last_probe_at.is_none_or(|last_probe_at| {
                                                    now.saturating_sub(last_probe_at)
                                                        >= probe_interval_seconds as i64
                                                })
                                        })
                                })
                                .cloned()
                                .collect();
                            due_models.sort();
                            configured_model
                                .as_ref()
                                .filter(|model| due_models.contains(model))
                                .cloned()
                                .or_else(|| due_models.into_iter().next())
                        } else {
                            let state = guard
                                .get(&health_key(&channel.id, None))
                                .cloned()
                                .unwrap_or_else(ChannelHealthState::new);
                            let probe_due = !state.healthy
                                && state.cooldown_until.is_none_or(|until| now >= until)
                                && state.last_probe_at.is_none_or(|last_probe_at| {
                                    now.saturating_sub(last_probe_at)
                                        >= probe_interval_seconds as i64
                                });
                            probe_due
                                .then(|| configured_model.clone().or(first_model.clone()))
                                .flatten()
                        }
                    };
                    let Some(ref model_name) = probe_model else {
                        continue;
                    };
                    if !channel.models.contains_key(model_name) {
                        continue;
                    }

                    let probe_http =
                        match probe_http_clients.for_channel_proxy(channel.proxy_url.as_deref()) {
                            Ok(client) => client,
                            Err(error) => {
                                tracing::warn!(
                                    channel_id = %channel.id,
                                    channel_name = %channel.name,
                                    provider = %provider.name,
                                    error = %error,
                                    "active probe skipped because channel proxy could not be built"
                                );
                                continue;
                            }
                        };

                    let upstream_model = channel
                        .models
                        .get(model_name)
                        .and_then(|entry| entry.redirect.as_deref())
                        .map(str::trim)
                        .filter(|value| !value.is_empty())
                        .unwrap_or(model_name)
                        .to_string();
                    if probe_shutdown.load(Ordering::Acquire) {
                        break 'scheduler;
                    }
                    let request_log_reservation = match probe_user_store
                        .reserve_terminal_request_log()
                    {
                        Ok(reservation) => reservation,
                        Err(error) => {
                            tracing::warn!(
                                channel_id = %channel.id,
                                channel_name = %channel.name,
                                provider = %provider.name,
                                probe_model = %model_name,
                                "active probe skipped because request-log spool admission failed: {error}"
                            );
                            continue;
                        }
                    };
                    let probe_request_id = uuid::Uuid::new_v4().to_string();
                    let probe_created_at = chrono::Utc::now();
                    let probe_fallback = build_active_probe_interrupted_log(
                        &probe_request_id,
                        &probe_user_id,
                        &provider.id,
                        &provider.name,
                        channel.provider_type,
                        channel.models.get(model_name).map(|entry| entry.multiplier),
                        &channel.id,
                        &channel.name,
                        model_name,
                        &upstream_model,
                        probe_created_at,
                    );
                    if let Err(error) = probe_user_store
                        .arm_terminal_request_log(probe_fallback, &request_log_reservation)
                        .await
                    {
                        tracing::warn!(
                            channel_id = %channel.id,
                            channel_name = %channel.name,
                            provider = %provider.name,
                            probe_model = %model_name,
                            "active probe skipped because request-log fallback could not be armed: {error}"
                        );
                        continue;
                    }
                    if probe_shutdown.load(Ordering::Acquire) {
                        if let Err(error) = probe_user_store
                            .cancel_terminal_request_log(&request_log_reservation)
                            .await
                        {
                            tracing::error!(
                                channel_id = %channel.id,
                                channel_name = %channel.name,
                                provider = %provider.name,
                                probe_model = %model_name,
                                "active probe request-log reservation cancellation failed: {error}"
                            );
                        }
                        break 'scheduler;
                    }
                    let probe_started_at = std::time::Instant::now();
                    let probe_outcome = probe_channel_completion(
                        &probe_http,
                        &channel,
                        rt_snap.request_timeout_ms,
                        &upstream_model,
                        channel.provider_type,
                        &provider.api_type_overrides,
                        false,
                    )
                    .await;
                    let ok = probe_outcome.ok;
                    let usage_snapshot = probe_outcome.usage;
                    if !ok {
                        if let Err(error) = probe_user_store
                            .cancel_terminal_request_log(&request_log_reservation)
                            .await
                        {
                            tracing::error!(
                                channel_id = %channel.id,
                                channel_name = %channel.name,
                                provider = %provider.name,
                                probe_model = %model_name,
                                "failed active probe request-log reservation cancellation failed: {error}"
                            );
                            continue;
                        }
                    }
                    if ok
                        && let Err(error) = persist_active_probe_request_log(
                            &probe_user_store,
                            &probe_user_id,
                            provider.id.clone(),
                            provider.name.clone(),
                            channel.provider_type,
                            channel.models.get(model_name).map(|entry| entry.multiplier),
                            channel.id.clone(),
                            channel.name.clone(),
                            model_name.to_string(),
                            upstream_model.clone(),
                            pricing_snapshot.resolve(
                                &upstream_model,
                                model_name,
                                channel.provider_type.as_str(),
                            ),
                            usage_snapshot,
                            probe_started_at.elapsed().as_millis() as u64,
                            probe_request_id,
                            probe_created_at,
                            request_log_reservation,
                        )
                        .await
                    {
                        tracing::error!(
                            channel_id = %channel.id,
                            channel_name = %channel.name,
                            provider = %provider.name,
                            probe_model = %model_name,
                            "active probe request log could not be durably enqueued: {error}"
                        );
                        continue;
                    }
                    tracing::debug!(
                        channel_id = %channel.id,
                        channel_name = %channel.name,
                        provider = %provider.name,
                        probe_model = ?probe_model,
                        probe_interval_seconds,
                        probe_success_threshold,
                        success = ok,
                        "active health probe result"
                    );

                    let mut guard = probe_health.lock().await;
                    if probe_routing_config_revision.load(Ordering::Acquire)
                        != routing_config_revision
                    {
                        continue;
                    }
                    if ok {
                        if provider.per_model_circuit_break {
                            let key = health_key(&channel.id, Some(model_name));
                            if let Some(state) = guard.get_mut(&key) {
                                state.last_probe_at = Some(now);
                                state.probe_success_count =
                                    state.probe_success_count.saturating_add(1);
                                if state.probe_success_count >= probe_success_threshold {
                                    clear_channel_health_state(state, now);
                                }
                            }
                        } else {
                            let key = health_key(&channel.id, None);
                            if !crate::monoize_routing::prepare_channel_health_insert(
                                &mut guard, &key,
                            ) {
                                continue;
                            }
                            let state = guard.entry(key).or_insert_with(ChannelHealthState::new);
                            state.last_probe_at = Some(now);
                            state.probe_success_count = state.probe_success_count.saturating_add(1);
                            if state.probe_success_count >= probe_success_threshold {
                                clear_channel_health_state(state, now);
                            }
                        }
                    } else {
                        let cooldown_seconds = channel
                            .passive_cooldown_seconds_override
                            .unwrap_or(rt_snap.passive_cooldown_seconds)
                            .max(1);
                        if provider.per_model_circuit_break {
                            let key = health_key(&channel.id, Some(model_name));
                            if let Some(state) = guard.get_mut(&key) {
                                state.healthy = false;
                                state.probe_success_count = 0;
                                state.last_probe_at = Some(now);
                                state.cooldown_until = Some(now + cooldown_seconds as i64);
                            }
                        } else {
                            let key = health_key(&channel.id, None);
                            if !crate::monoize_routing::prepare_channel_health_insert(
                                &mut guard, &key,
                            ) {
                                continue;
                            }
                            let state = guard.entry(key).or_insert_with(ChannelHealthState::new);
                            state.healthy = false;
                            state.probe_success_count = 0;
                            state.last_probe_at = Some(now);
                            state.cooldown_until = Some(now + cooldown_seconds as i64);
                        }
                    }
                }
            }
        }
    });

    let node = Arc::new(runtime.node.clone());
    let started_at = chrono::Utc::now();
    let metering = if is_replica {
        let hostname = std::env::var("HOSTNAME")
            .ok()
            .map(|value| value.trim().to_string())
            .filter(|value| !value.is_empty())
            .unwrap_or_else(|| "unknown".to_string());
        let metering = crate::replica::metering::ReplicaMetering::new(
            runtime.node.metering_spool_dir.clone(),
            runtime.node.metering_spool_max_bytes,
            runtime.node.replica_primary_url.as_deref().unwrap_or(""),
            runtime.node.replica_token.as_deref().unwrap_or(""),
            runtime.node.metering_ship_batch_max_entries,
        )
        .and_then(|metering| {
            // M9: the identity persists in the spool directory, so a restarted replica
            // keeps its heartbeat map entry on the primary instead of adding a new one.
            let replica_id = crate::replica::metering::resolve_replica_identity(
                runtime.node.replica_id.as_deref(),
                &runtime.node.metering_spool_dir,
            )?;
            Ok(
                metering.with_heartbeat_source(crate::replica::metering::ReplicaHeartbeatSource {
                    id: replica_id,
                    hostname,
                    listen: runtime.listen.clone(),
                    version: env!("CARGO_PKG_VERSION").to_string(),
                    started_at: started_at.to_rfc3339(),
                }),
            )
        })
        .map_err(|err| {
            let code = [
                "metering_spool_unwritable",
                "replica_id_invalid",
                "replica_identity_unwritable",
            ]
            .into_iter()
            .find(|prefix| err.starts_with(prefix))
            .unwrap_or("metering_init_failed");
            AppError::new(axum::http::StatusCode::BAD_REQUEST, code, err)
        })?;
        let metering = Arc::new(metering);
        metering.spawn_ship_loop(
            user_store.request_log_batcher_clone(),
            user_store.last_used_batcher_clone(),
            runtime.node.metering_ship_interval,
        );
        Some(metering)
    } else {
        // PRP9: a promoted node drains leftover delta spool entries before serving.
        if runtime.node.metering_spool_dir.exists() {
            let spool = crate::replica::metering::DeltaSpool::new(
                runtime.node.metering_spool_dir.clone(),
                runtime.node.metering_spool_max_bytes,
            )
            .map_err(|err| {
                AppError::new(
                    axum::http::StatusCode::BAD_REQUEST,
                    "metering_drain_failed",
                    err,
                )
            })?;
            crate::replica::metering::drain_delta_spool_to_local_db(&db, &spool)
                .await
                .map_err(|err| {
                    AppError::new(
                        axum::http::StatusCode::BAD_REQUEST,
                        "metering_drain_failed",
                        err,
                    )
                })?;
        }
        None
    };
    let metering_token_digest: Option<[u8; 32]> = if !is_replica {
        runtime
            .node
            .replica_token
            .as_deref()
            .filter(|token| !token.is_empty())
            .map(crate::replica::metering::sha256_hex_lower)
    } else {
        None
    };

    Ok(AppState {
        runtime: Arc::new(runtime),
        started_at,
        auth,
        http,
        http_clients,
        node,
        db_pool: db.clone(),
        metering,
        metering_token_digest,
        replica_heartbeats: Arc::new(DashMap::new()),
        metrics,
        user_store,
        settings_store,
        monoize_store,
        monoize_runtime,
        channel_health,
        channel_affinity,
        routing_config_revision,
        settings_update_lock,
        model_registry_store,
        billing_rate_store,
        transform_registry,
        cap_verifier,
        log_broadcast,
        pending_request_logs,
        request_log_admissions: Arc::new(DashMap::new()),
        request_log_tasks,
        background_shutdown,
        sse_connections: Arc::new(DashMap::new()),
        image_transform_cache,
        request_capture,
        trusted_proxies,
    })
}

#[allow(clippy::result_large_err)]
fn init_metrics() -> AppResult<PrometheusHandle> {
    METRICS_INIT.call_once(|| {
        match metrics_exporter_prometheus::PrometheusBuilder::new().install_recorder() {
            Ok(handle) => {
                let _ = METRICS_HANDLE.set(handle);
            }
            Err(err) => {
                let _ = METRICS_ERROR.set(AppError::new(
                    axum::http::StatusCode::BAD_REQUEST,
                    "metrics_init_failed",
                    err.to_string(),
                ));
            }
        }
    });

    if let Some(err) = METRICS_ERROR.get() {
        return Err(err.clone());
    }
    METRICS_HANDLE.get().cloned().ok_or_else(|| {
        AppError::new(
            axum::http::StatusCode::BAD_REQUEST,
            "metrics_init_failed",
            "metrics recorder not available",
        )
    })
}

async fn ensure_active_probe_system_user(user_store: &UserStore) -> AppResult<String> {
    let existing = user_store
        .get_user_by_username(ACTIVE_PROBE_SYSTEM_USER)
        .await
        .map_err(active_probe_user_init_error)?;
    if let Some(user) = existing {
        if !user.balance_unlimited {
            user_store
                .update_user(
                    &user.id,
                    None,
                    None,
                    None,
                    None,
                    None,
                    Some(true),
                    None,
                    None,
                )
                .await
                .map_err(active_probe_user_init_error)?;
        }
        return Ok(user.id);
    }
    let user = user_store
        .create_user(
            ACTIVE_PROBE_SYSTEM_USER,
            &uuid::Uuid::new_v4().to_string(),
            UserRole::User,
            None,
        )
        .await
        .map_err(active_probe_user_init_error)?;
    user_store
        .update_user(
            &user.id,
            None,
            None,
            None,
            None,
            None,
            Some(true),
            None,
            None,
        )
        .await
        .map_err(active_probe_user_init_error)?;
    Ok(user.id)
}

fn active_probe_user_init_error(error: String) -> AppError {
    AppError::new(
        axum::http::StatusCode::INTERNAL_SERVER_ERROR,
        "active_probe_user_init_failed",
        error,
    )
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct ActiveProbeRateResolution {
    pricing_profile: String,
    pricing_model: String,
    input_rate_nano: i128,
    output_rate_nano: i128,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct ActiveProbeCharge {
    prompt_charge_nano: i128,
    completion_charge_nano: i128,
    base_charge_nano: i128,
    final_charge_nano: i128,
}

fn is_dimensionless_probe_rate(rate: &DbBillingRateRecord, usage_class: &str) -> bool {
    rate.rate_kind == "token"
        && rate.usage_class == usage_class
        && rate.unit == "token"
        && rate.modality.is_none()
        && rate.cache_ttl.is_none()
        && rate
            .context_tier
            .as_deref()
            .is_none_or(|tier| tier == "default")
        && rate
            .service_tier
            .as_deref()
            .is_none_or(|tier| tier == "default")
}

fn first_dimensionless_probe_rate(
    rates: &[DbBillingRateRecord],
    usage_class: &str,
) -> Result<Option<i128>, String> {
    let Some(rate) = rates
        .iter()
        .find(|rate| is_dimensionless_probe_rate(rate, usage_class))
    else {
        return Ok(None);
    };
    let price = rate.unit_price_nano()?;
    if price < 0 || price.to_string() != rate.unit_price_nano_usd {
        return Err(format!(
            "non-canonical or negative unit_price_nano_usd for billing rate {}",
            rate.id
        ));
    }
    Ok(Some(price))
}

fn resolve_active_probe_rates_for_model(
    patterns: &[PricingProfilePattern],
    metadata_profiles: &HashMap<String, String>,
    candidate_rates: &[DbBillingRateRecord],
    pricing_model: &str,
    provider_type: &str,
) -> Result<Option<ActiveProbeRateResolution>, String> {
    let mut candidate_profiles = Vec::new();
    if let Some(profile) =
        crate::billing_rate_store::select_pricing_profile(patterns, pricing_model)
    {
        candidate_profiles.push(profile.to_string());
    }
    if let Some(metadata_profile) = metadata_profiles.get(pricing_model)
        && !candidate_profiles.contains(metadata_profile)
    {
        candidate_profiles.push(metadata_profile.clone());
    }

    for pricing_profile in candidate_profiles {
        let rates = candidate_rates
            .iter()
            .filter(|rate| {
                rate.pricing_profile == pricing_profile
                    && rate
                        .provider_type
                        .as_deref()
                        .is_none_or(|value| value == provider_type)
                    && rate.model_pattern.as_deref().is_none_or(|pattern| {
                        crate::billing_rate_store::glob_matches(pattern, pricing_model)
                    })
            })
            .cloned()
            .collect::<Vec<_>>();
        let input_rate_nano = first_dimensionless_probe_rate(&rates, "input_uncached")?;
        let output_rate_nano = first_dimensionless_probe_rate(&rates, "output")?;
        if let (Some(input_rate_nano), Some(output_rate_nano)) = (input_rate_nano, output_rate_nano)
        {
            return Ok(Some(ActiveProbeRateResolution {
                pricing_profile,
                pricing_model: pricing_model.to_string(),
                input_rate_nano,
                output_rate_nano,
            }));
        }
    }
    Ok(None)
}

#[derive(Debug, Clone, Default)]
struct ActiveProbePricingSnapshot {
    reasoning_suffix_map: HashMap<String, String>,
    resolutions: HashMap<(String, String), Result<Option<ActiveProbeRateResolution>, String>>,
}

impl ActiveProbePricingSnapshot {
    fn resolve(
        &self,
        upstream_model: &str,
        logical_model: &str,
        provider_type: &str,
    ) -> Result<Option<ActiveProbeRateResolution>, String> {
        let normalized_upstream_model =
            normalize_pricing_model_key(upstream_model, &self.reasoning_suffix_map);
        let upstream = self
            .resolutions
            .get(&(normalized_upstream_model.clone(), provider_type.to_string()))
            .cloned()
            .unwrap_or(Ok(None))?;
        if upstream.is_some() {
            return Ok(upstream);
        }
        let normalized_logical_model =
            normalize_pricing_model_key(logical_model, &self.reasoning_suffix_map);
        if normalized_logical_model == normalized_upstream_model {
            return Ok(None);
        }
        self.resolutions
            .get(&(normalized_logical_model, provider_type.to_string()))
            .cloned()
            .unwrap_or(Ok(None))
    }
}

async fn build_active_probe_pricing_snapshot(
    billing_rate_store: &BillingRateStore,
    model_registry_store: &ModelRegistryStore,
    providers: &[crate::monoize_routing::MonoizeProvider],
    runtime: &MonoizeRuntimeConfig,
) -> Result<ActiveProbePricingSnapshot, String> {
    let reasoning_suffix_map = runtime.reasoning_suffix_map.clone();
    let patterns = runtime.pricing_profile_model_patterns.clone();
    let mut pairs = std::collections::HashSet::new();
    for provider in providers {
        for channel in &provider.channels {
            if channel.provider_type == crate::monoize_routing::MonoizeProviderType::Replicate {
                continue;
            }
            let configured_model = channel
                .active_probe_model_override
                .as_ref()
                .or(provider.active_probe_model_override.as_ref())
                .or(runtime.active_probe_model.as_ref())
                .cloned();
            let logical_model = configured_model.or_else(|| channel.models.keys().min().cloned());
            let Some(logical_model) = logical_model else {
                continue;
            };
            if !channel.models.contains_key(&logical_model) {
                continue;
            }
            let upstream_model = channel
                .models
                .get(&logical_model)
                .and_then(|entry| entry.redirect.as_deref())
                .map(str::trim)
                .filter(|value| !value.is_empty())
                .unwrap_or(&logical_model);
            let provider_type = channel.provider_type.as_str().to_string();
            pairs.insert((
                normalize_pricing_model_key(upstream_model, &reasoning_suffix_map),
                provider_type.clone(),
            ));
            pairs.insert((
                normalize_pricing_model_key(&logical_model, &reasoning_suffix_map),
                provider_type,
            ));
        }
    }
    let mut models = pairs
        .iter()
        .map(|(model, _)| model.clone())
        .collect::<Vec<_>>();
    models.sort();
    models.dedup();
    let metadata_profiles = model_registry_store
        .list_model_metadata_pricing_profiles(&models)
        .await?;
    let mut profiles = Vec::new();
    for model in &models {
        if let Some(profile) = crate::billing_rate_store::select_pricing_profile(&patterns, model) {
            profiles.push(profile.to_string());
        }
        if let Some(profile) = metadata_profiles.get(model) {
            profiles.push(profile.clone());
        }
    }
    profiles.sort();
    profiles.dedup();
    let mut provider_types = pairs
        .iter()
        .map(|(_, provider_type)| provider_type.clone())
        .collect::<Vec<_>>();
    provider_types.sort();
    provider_types.dedup();
    let candidate_rates = billing_rate_store
        .list_candidate_rates_for_profiles_and_provider_types(&profiles, &provider_types)
        .await?;
    let resolutions = pairs
        .into_iter()
        .map(|(model, provider_type)| {
            let resolution = resolve_active_probe_rates_for_model(
                &patterns,
                &metadata_profiles,
                &candidate_rates,
                &model,
                &provider_type,
            );
            ((model, provider_type), resolution)
        })
        .collect();
    Ok(ActiveProbePricingSnapshot {
        reasoning_suffix_map,
        resolutions,
    })
}

fn calculate_active_probe_charge(
    prompt_tokens: u64,
    completion_tokens: u64,
    pricing: &ActiveProbeRateResolution,
    provider_multiplier: Multiplier,
) -> Result<ActiveProbeCharge, String> {
    let prompt_charge_nano = i128::from(prompt_tokens)
        .checked_mul(pricing.input_rate_nano)
        .ok_or_else(|| "active probe prompt charge overflow".to_string())?;
    let completion_charge_nano = i128::from(completion_tokens)
        .checked_mul(pricing.output_rate_nano)
        .ok_or_else(|| "active probe completion charge overflow".to_string())?;
    let base_charge_nano = prompt_charge_nano
        .checked_add(completion_charge_nano)
        .ok_or_else(|| "active probe base charge overflow".to_string())?;
    let final_charge_nano = provider_multiplier
        .checked_scale_i128(base_charge_nano)
        .ok_or_else(|| "active probe multiplier charge overflow".to_string())?;
    Ok(ActiveProbeCharge {
        prompt_charge_nano,
        completion_charge_nano,
        base_charge_nano,
        final_charge_nano,
    })
}

fn build_probe_usage_breakdown(prompt_tokens: u64, completion_tokens: u64) -> Value {
    json!({
        "version": 1,
        "input": {
            "total_tokens": prompt_tokens,
            "uncached_tokens": prompt_tokens,
            "text_tokens": prompt_tokens,
            "cached_tokens": 0,
            "cache_creation_tokens": null,
            "audio_tokens": null,
            "image_tokens": null
        },
        "output": {
            "total_tokens": completion_tokens,
            "non_reasoning_tokens": completion_tokens,
            "text_tokens": completion_tokens,
            "reasoning_tokens": null,
            "audio_tokens": null,
            "image_tokens": null
        },
        "raw_usage_extra": {
            "source": "active_probe"
        }
    })
}

#[allow(clippy::too_many_arguments)]
fn build_probe_billing_breakdown(
    provider_id: &str,
    logical_model: &str,
    upstream_model: &str,
    provider_multiplier: Multiplier,
    prompt_tokens: u64,
    completion_tokens: u64,
    pricing: &ActiveProbeRateResolution,
    charge: ActiveProbeCharge,
) -> Value {
    json!({
        "version": 2,
        "currency": "nano_usd",
        "logical_model": logical_model,
        "upstream_model": upstream_model,
        "provider_id": provider_id,
        "pricing_profile": pricing.pricing_profile,
        "pricing_model": pricing.pricing_model,
        "provider_multiplier": provider_multiplier,
        "token_line_items": [
            {
                "usage_class": "input_uncached",
                "unit": "token",
                "unit_price_nano": pricing.input_rate_nano.to_string(),
                "quantity": prompt_tokens,
                "charge_nano": charge.prompt_charge_nano.to_string()
            },
            {
                "usage_class": "output",
                "unit": "token",
                "unit_price_nano": pricing.output_rate_nano.to_string(),
                "quantity": completion_tokens,
                "charge_nano": charge.completion_charge_nano.to_string()
            }
        ],
        "meter_line_items": [],
        "tier": {
            "context_tier": null,
            "service_tier": null
        },
        "base_charge_nano": charge.base_charge_nano.to_string(),
        "final_charge_nano": charge.final_charge_nano.to_string()
    })
}

#[allow(clippy::too_many_arguments)]
fn build_active_probe_interrupted_log(
    request_id: &str,
    user_id: &str,
    provider_id: &str,
    provider_name: &str,
    provider_type: crate::monoize_routing::MonoizeProviderType,
    provider_multiplier: Option<Multiplier>,
    channel_id: &str,
    channel_name: &str,
    logical_model: &str,
    upstream_model: &str,
    created_at: chrono::DateTime<chrono::Utc>,
) -> InsertRequestLog {
    InsertRequestLog {
        request_id: Some(request_id.to_string()),
        user_id: user_id.to_string(),
        api_key_id: None,
        model: logical_model.to_string(),
        provider_id: Some(provider_id.to_string()),
        upstream_model: Some(upstream_model.to_string()),
        channel_id: Some(channel_id.to_string()),
        names: crate::users::RequestLogNameSnapshots {
            username: Some(ACTIVE_PROBE_SYSTEM_USER.to_string()),
            api_key_name: None,
            provider_name: Some(provider_name.to_string()),
            channel_name: Some(channel_name.to_string()),
        },
        is_stream: false,
        input_tokens: None,
        output_tokens: None,
        cache_read_tokens: None,
        cache_creation_tokens: None,
        tool_prompt_tokens: None,
        reasoning_tokens: None,
        accepted_prediction_tokens: None,
        rejected_prediction_tokens: None,
        provider_multiplier: Some(provider_multiplier.unwrap_or(Multiplier::ONE)),
        charge_nano_usd: None,
        status: crate::users::REQUEST_LOG_STATUS_ERROR.to_string(),
        usage_breakdown_json: None,
        billing_breakdown_json: None,
        error_code: Some("active_probe_interrupted".to_string()),
        error_message: Some(
            "active probe ended before terminal request-log persistence".to_string(),
        ),
        error_http_status: Some(axum::http::StatusCode::INTERNAL_SERVER_ERROR.as_u16()),
        duration_ms: None,
        ttfb_ms: None,
        request_ip: None,
        reasoning_effort: None,
        tried_providers_json: None,
        request_kind: Some(ACTIVE_PROBE_CONNECTIVITY_KIND.to_string()),
        effective_provider_type: Some(provider_type.as_str().to_string()),
        affinity_hit: None,
        affinity_key_hash: None,
        affinity_target: None,
        session_affinity_value: None,
        created_at,
    }
}

#[allow(clippy::too_many_arguments)]
async fn persist_active_probe_request_log(
    user_store: &UserStore,
    user_id: &str,
    provider_id: String,
    provider_name: String,
    provider_type: crate::monoize_routing::MonoizeProviderType,
    provider_multiplier: Option<Multiplier>,
    channel_id: String,
    channel_name: String,
    logical_model: String,
    upstream_model: String,
    pricing_resolution: Result<Option<ActiveProbeRateResolution>, String>,
    usage_snapshot: Option<Value>,
    duration_ms: u64,
    request_id: String,
    created_at: chrono::DateTime<chrono::Utc>,
    reservation: crate::db_cache::RequestLogReservation,
) -> Result<(), String> {
    let provider_multiplier = provider_multiplier.unwrap_or(Multiplier::ONE);
    let parsed_prompt_tokens = usage_snapshot
        .as_ref()
        .and_then(|v| v.get("prompt_tokens"))
        .and_then(|v| v.as_u64());
    let parsed_completion_tokens = usage_snapshot
        .as_ref()
        .and_then(|v| v.get("completion_tokens"))
        .and_then(|v| v.as_u64());
    let usage_tokens = parsed_prompt_tokens.zip(parsed_completion_tokens);
    let (charge_nano_usd, billing_breakdown_json) =
        if let Some((prompt_tokens, completion_tokens)) = usage_tokens {
            match pricing_resolution {
                Ok(Some(pricing)) => match calculate_active_probe_charge(
                    prompt_tokens,
                    completion_tokens,
                    &pricing,
                    provider_multiplier,
                ) {
                    Ok(charge) => (
                        Some(charge.final_charge_nano),
                        Some(build_probe_billing_breakdown(
                            &provider_id,
                            &logical_model,
                            &upstream_model,
                            provider_multiplier,
                            prompt_tokens,
                            completion_tokens,
                            &pricing,
                            charge,
                        )),
                    ),
                    Err(err) => {
                        tracing::warn!(
                            logical_model,
                            upstream_model,
                            error = %err,
                            "active probe charge calculation failed"
                        );
                        (None, None)
                    }
                },
                Ok(None) => (None, None),
                Err(err) => {
                    tracing::warn!(
                        logical_model,
                        upstream_model,
                        error = %err,
                        "active probe pricing resolution failed"
                    );
                    (None, None)
                }
            }
        } else {
            (None, None)
        };

    let usage_breakdown_json = usage_tokens.map(|(prompt_tokens, completion_tokens)| {
        build_probe_usage_breakdown(prompt_tokens, completion_tokens)
    });

    let log = InsertRequestLog {
        request_id: Some(request_id),
        user_id: user_id.to_string(),
        api_key_id: None,
        model: logical_model,
        provider_id: Some(provider_id),
        upstream_model: Some(upstream_model),
        channel_id: Some(channel_id),
        names: crate::users::RequestLogNameSnapshots {
            username: Some(ACTIVE_PROBE_SYSTEM_USER.to_string()),
            api_key_name: None,
            provider_name: Some(provider_name),
            channel_name: Some(channel_name),
        },
        is_stream: false,
        input_tokens: usage_tokens.map(|(prompt_tokens, _)| prompt_tokens),
        output_tokens: usage_tokens.map(|(_, completion_tokens)| completion_tokens),
        cache_read_tokens: usage_tokens.map(|_| 0),
        cache_creation_tokens: None,
        tool_prompt_tokens: None,
        reasoning_tokens: None,
        accepted_prediction_tokens: None,
        rejected_prediction_tokens: None,
        provider_multiplier: Some(provider_multiplier),
        charge_nano_usd,
        status: "success".to_string(),
        usage_breakdown_json,
        billing_breakdown_json,
        error_code: None,
        error_message: None,
        error_http_status: None,
        duration_ms: Some(duration_ms),
        ttfb_ms: None,
        request_ip: None,
        reasoning_effort: None,
        tried_providers_json: None,
        request_kind: Some(ACTIVE_PROBE_CONNECTIVITY_KIND.to_string()),
        effective_provider_type: Some(provider_type.as_str().to_string()),
        affinity_hit: None,
        affinity_key_hash: None,
        affinity_target: None,
        session_affinity_value: None,
        created_at,
    };

    user_store
        .finalize_reserved_request_log(log, reservation)
        .await
}

#[cfg(test)]
mod active_probe_billing_tests {
    use super::*;

    #[test]
    fn active_probe_fallback_is_a_complete_terminal_error_snapshot() {
        let created_at = chrono::Utc::now();
        let log = build_active_probe_interrupted_log(
            "d0925a9e-5384-4f1e-b9b1-d96464644700",
            "user-probe",
            "provider-1",
            "OpenAI",
            crate::monoize_routing::MonoizeProviderType::Responses,
            Some(Multiplier::parse("1.25").expect("valid multiplier")),
            "channel-1",
            "primary",
            "gpt-5",
            "gpt-5-2026-08-07",
            created_at,
        );

        assert_eq!(
            log.request_id.as_deref(),
            Some("d0925a9e-5384-4f1e-b9b1-d96464644700")
        );
        assert_eq!(log.status, crate::users::REQUEST_LOG_STATUS_ERROR);
        assert_eq!(log.error_code.as_deref(), Some("active_probe_interrupted"));
        assert_eq!(
            log.request_kind.as_deref(),
            Some(ACTIVE_PROBE_CONNECTIVITY_KIND)
        );
        assert_eq!(log.provider_id.as_deref(), Some("provider-1"));
        assert_eq!(log.channel_id.as_deref(), Some("channel-1"));
        assert_eq!(log.created_at, created_at);
    }

    #[test]
    fn http_body_limit_accepts_only_positive_usize_values() {
        assert_eq!(http_body_max_bytes_from_raw(Some(" 1234 ")), 1234);
        for raw in [None, Some(""), Some("0"), Some("-1"), Some("invalid")] {
            assert_eq!(
                http_body_max_bytes_from_raw(raw),
                DEFAULT_HTTP_BODY_MAX_BYTES
            );
        }
        let overflow = format!("{}0", usize::MAX);
        assert_eq!(
            http_body_max_bytes_from_raw(Some(&overflow)),
            DEFAULT_HTTP_BODY_MAX_BYTES
        );
    }

    fn rate(
        id: &str,
        usage_class: &str,
        unit_price_nano_usd: &str,
        modality: Option<&str>,
    ) -> DbBillingRateRecord {
        DbBillingRateRecord {
            id: id.to_string(),
            source: "test".to_string(),
            pricing_profile: "test-profile".to_string(),
            model_pattern: Some("test-model".to_string()),
            provider_type: None,
            rate_kind: "token".to_string(),
            usage_class: usage_class.to_string(),
            unit: "token".to_string(),
            unit_price_nano_usd: unit_price_nano_usd.to_string(),
            context_tier: None,
            service_tier: None,
            modality: modality.map(str::to_string),
            cache_ttl: None,
            match_json: json!({}),
            priority: 0,
            enabled: true,
            raw_json: json!({}),
            updated_at: chrono::Utc::now(),
        }
    }

    #[test]
    fn probe_rate_selection_uses_first_dimensionless_row() {
        let rates = vec![
            rate("dimensioned", "input_uncached", "999", Some("image")),
            rate("first", "input_uncached", "1001", None),
            rate("second", "input_uncached", "2000", None),
        ];
        assert_eq!(
            first_dimensionless_probe_rate(&rates, "input_uncached").unwrap(),
            Some(1001)
        );
    }

    #[test]
    fn probe_bulk_snapshot_preserves_profile_and_model_fallback_order() {
        let patterns = vec![PricingProfilePattern {
            pattern: "upstream-*".to_string(),
            pricing_profile: "settings-profile".to_string(),
        }];
        let metadata =
            HashMap::from([("logical-model".to_string(), "metadata-profile".to_string())]);
        let mut input = rate("input", "input_uncached", "11", None);
        input.pricing_profile = "metadata-profile".to_string();
        input.model_pattern = Some("logical-*".to_string());
        input.provider_type = Some("responses".to_string());
        let mut output = input.clone();
        output.id = "output".to_string();
        output.usage_class = "output".to_string();
        let candidate_rates = vec![input, output];
        let upstream = resolve_active_probe_rates_for_model(
            &patterns,
            &metadata,
            &candidate_rates,
            "upstream-model",
            "responses",
        )
        .unwrap();
        assert!(upstream.is_none());
        let logical = resolve_active_probe_rates_for_model(
            &patterns,
            &metadata,
            &candidate_rates,
            "logical-model",
            "responses",
        )
        .unwrap()
        .expect("metadata profile resolves");
        assert_eq!(logical.pricing_profile, "metadata-profile");
        assert_eq!(logical.input_rate_nano, 11);
    }

    #[test]
    fn probe_charge_uses_exact_multiplier_and_checked_arithmetic() {
        let pricing = ActiveProbeRateResolution {
            pricing_profile: "test-profile".to_string(),
            pricing_model: "test-model".to_string(),
            input_rate_nano: 1000,
            output_rate_nano: 2000,
        };
        let charge =
            calculate_active_probe_charge(1, 1, &pricing, Multiplier::parse("1.001").unwrap())
                .unwrap();
        assert_eq!(charge.base_charge_nano, 3000);
        assert_eq!(charge.final_charge_nano, 3003);

        let overflowing = ActiveProbeRateResolution {
            input_rate_nano: i128::MAX,
            ..pricing
        };
        assert!(calculate_active_probe_charge(2, 0, &overflowing, Multiplier::ONE).is_err());
    }
}

fn resolve_database_dsn() -> String {
    std::env::var("MONOIZE_DATABASE_DSN")
        .ok()
        .filter(|v| !v.trim().is_empty())
        .or_else(|| {
            std::env::var("DATABASE_URL")
                .ok()
                .filter(|v| !v.trim().is_empty())
        })
        .unwrap_or_else(|| "sqlite://./data/monoize.db".to_string())
}

fn clear_channel_health_state(state: &mut ChannelHealthState, now: i64) {
    state.healthy = true;
    state.cooldown_until = None;
    state.last_success_at = Some(now);
    state.probe_success_count = 0;
    state.last_probe_at = None;
}

async fn canonical_request_id_middleware(
    mut request: axum::extract::Request,
    next: axum::middleware::Next,
) -> axum::response::Response {
    let canonical = request
        .headers()
        .get("x-request-id")
        .and_then(|value| value.to_str().ok())
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .and_then(|value| axum::http::HeaderValue::from_str(value).ok())
        .unwrap_or_else(|| {
            axum::http::HeaderValue::from_str(&uuid::Uuid::new_v4().to_string())
                .expect("UUID request id is a valid header value")
        });
    request.headers_mut().insert(
        axum::http::header::HeaderName::from_static("x-request-id"),
        canonical.clone(),
    );
    request.extensions_mut().insert(RequestId::new(canonical));
    next.run(request).await
}

/// Shared construction logic for the settings-derived runtime snapshot (E3): the
/// primary publishes it after mutations and the replica rebuilds it on epoch change.
#[allow(clippy::field_reassign_with_default)]
pub(crate) fn runtime_config_from_settings(
    settings_snapshot: &crate::settings::SystemSettings,
) -> MonoizeRuntimeConfig {
    let mut runtime = MonoizeRuntimeConfig::default();
    runtime.passive_failure_count_threshold =
        settings_snapshot.monoize_passive_failure_threshold.max(1);
    runtime.passive_cooldown_seconds = settings_snapshot.monoize_passive_cooldown_seconds.max(1);
    runtime.passive_window_seconds = settings_snapshot.monoize_passive_window_seconds.max(1);
    runtime.passive_rate_limit_cooldown_seconds = settings_snapshot
        .monoize_passive_rate_limit_cooldown_seconds
        .max(1);
    runtime.active_enabled = settings_snapshot.monoize_active_probe_enabled;
    runtime.active_interval_seconds = settings_snapshot
        .monoize_active_probe_interval_seconds
        .max(1);
    runtime.active_success_threshold = settings_snapshot
        .monoize_active_probe_success_threshold
        .max(1);
    runtime.active_probe_model = settings_snapshot.monoize_active_probe_model.clone();
    runtime.global_transforms = settings_snapshot.global_transforms.clone();
    let _ = runtime.set_global_model_redirects(settings_snapshot.global_model_redirects.clone());
    runtime.reasoning_suffix_map = settings_snapshot.reasoning_suffix_map.clone();
    runtime.pricing_profile_model_patterns =
        settings_snapshot.pricing_profile_model_patterns.clone();
    runtime.codex_model_ids = settings_snapshot.codex_model_ids.clone();
    runtime.request_timeout_ms = settings_snapshot.monoize_request_timeout_ms.max(1);
    runtime.stream_idle_timeout_ms = settings_snapshot.monoize_stream_idle_timeout_ms.max(1);
    runtime.enable_estimated_billing = settings_snapshot.monoize_enable_estimated_billing;
    runtime.extra_fields_whitelist = settings_snapshot.monoize_extra_fields_whitelist.clone();
    runtime.strip_cross_protocol_nested_extra =
        settings_snapshot.monoize_strip_cross_protocol_nested_extra;
    runtime.request_capture_enabled = settings_snapshot.monoize_request_capture_enabled;
    runtime.request_capture_retention_days = settings_snapshot
        .monoize_request_capture_retention_days
        .max(1);
    runtime.mask_sensitive_info = settings_snapshot.monoize_mask_sensitive_info;
    runtime.affinity_enabled = settings_snapshot.monoize_affinity_enabled;
    runtime.affinity_idle_ttl_seconds = settings_snapshot.monoize_affinity_idle_ttl_seconds.max(1);
    runtime.affinity_failback_mode = settings_snapshot.monoize_affinity_failback_mode;
    runtime.affinity_failback_delay_seconds =
        settings_snapshot.monoize_affinity_failback_delay_seconds;
    runtime
}

/// D1: replica nodes answer every non-API path with this error instead of mounting
/// the dashboard or the SPA.
async fn replica_disabled_fallback() -> axum::response::Response {
    (
        axum::http::StatusCode::NOT_FOUND,
        axum::Json(json!({
            "error": {
                "code": "replica_dashboard_disabled",
                "message": "dashboard and frontend are served by the primary node"
            }
        })),
    )
        .into_response()
}

pub fn build_app(state: AppState) -> Router {
    let metrics_path = state.runtime.metrics_path.clone();
    let trusted_proxies = state.trusted_proxies.clone();
    let http_body_max_bytes = http_body_max_bytes();
    let is_replica = state.node.is_replica();
    let root_api_router = build_root_api_router(&metrics_path);
    let dashboard_api_router = build_dashboard_api_router();
    let csp = ContentSecurityPolicy::new(state.cap_verifier.api_origin());

    let mut app = Router::<AppState>::new()
        .merge(root_api_router.clone())
        .merge(build_balance_compatibility_router());
    if is_replica {
        // D1/D2: API-only surface; /v1/** and /metrics stay local.
        app = app.fallback(replica_disabled_fallback);
    } else {
        let api_router = root_api_router.clone().merge(dashboard_api_router);
        app = app.nest("/api", api_router);
        if state.metering_token_digest.is_some() {
            // PRP6/I1: ingest mounted only when a replica token is configured.
            app = app.route(
                crate::replica::metering::METERING_INGEST_PATH,
                post(crate::replica::metering::ingest_metering_handler),
            );
        }
        app = app.fallback(crate::frontend::frontend_fallback);
    }
    app.with_state(state)
        .layer(axum::middleware::from_fn_with_state(
            trusted_proxies,
            crate::client_ip::canonical_client_ip_middleware,
        ))
        .layer(DefaultBodyLimit::disable())
        .layer(PropagateRequestIdLayer::new(
            axum::http::header::HeaderName::from_static("x-request-id"),
        ))
        .layer(axum::middleware::from_fn(canonical_request_id_middleware))
        .layer(SetRequestIdLayer::new(
            axum::http::header::HeaderName::from_static("x-request-id"),
            MakeRequestUuid,
        ))
        .layer(TraceLayer::new_for_http())
        .layer(RequestBodyLimitLayer::new(http_body_max_bytes))
        // Security headers
        .layer(SetResponseHeaderLayer::overriding(
            axum::http::header::HeaderName::from_static("x-content-type-options"),
            axum::http::HeaderValue::from_static("nosniff"),
        ))
        .layer(SetResponseHeaderLayer::overriding(
            axum::http::header::HeaderName::from_static("x-frame-options"),
            axum::http::HeaderValue::from_static("DENY"),
        ))
        .layer(axum::middleware::from_fn_with_state(
            csp,
            content_security_policy_middleware,
        ))
}

#[derive(Clone)]
struct ContentSecurityPolicy {
    connect_src: String,
}

impl ContentSecurityPolicy {
    fn new(cap_origin: Option<String>) -> Self {
        let connect_src = match cap_origin {
            Some(origin) => format!("'self' {origin}"),
            None => "'self'".to_string(),
        };
        Self { connect_src }
    }
}

#[derive(Clone)]
#[allow(dead_code)]
pub(crate) struct CspNonce(pub String);

async fn content_security_policy_middleware(
    axum::extract::State(config): axum::extract::State<ContentSecurityPolicy>,
    mut request: Request<Body>,
    next: axum::middleware::Next,
) -> axum::response::Response {
    let nonce = uuid::Uuid::new_v4().simple().to_string();
    request.extensions_mut().insert(CspNonce(nonce.clone()));
    let mut response = next.run(request).await;
    let policy = format!(
        "default-src 'self'; script-src 'self' 'nonce-{nonce}'; style-src 'self' 'unsafe-inline' https://fonts.googleapis.com https://fontsapi.zeoseven.com; img-src 'self' data: https://www.gravatar.com; connect-src {}; font-src 'self' https://fonts.gstatic.com https://fontsapi.zeoseven.com; worker-src 'self' blob:; frame-ancestors 'none'",
        config.connect_src
    );
    if let Ok(value) = header::HeaderValue::from_str(&policy) {
        response
            .headers_mut()
            .insert(header::CONTENT_SECURITY_POLICY, value);
    }
    response
}

fn build_v1_router() -> Router<AppState> {
    Router::new()
        .route("/v1/models", get(crate::handlers::list_models))
        .route(
            "/v1/responses",
            get(crate::handlers::responses_websocket).post(crate::handlers::create_response),
        )
        .route(
            "/v1/responses/compact",
            post(crate::handlers::compact_response),
        )
        .route(
            "/v1/chat/completions",
            post(crate::handlers::create_chat_completions),
        )
        .route("/v1/embeddings", post(crate::handlers::create_embeddings))
        .route("/v1/messages", post(crate::handlers::create_messages))
        .route(
            "/v1/images/generations",
            post(crate::handlers::image_api::create_image_generation),
        )
        .route(
            "/v1/images/edits",
            post(crate::handlers::image_api::create_image_edit),
        )
        .layer(CorsLayer::very_permissive())
}

fn build_root_api_router(metrics_path: &str) -> Router<AppState> {
    build_v1_router()
        .route(metrics_path, get(crate::dashboard_handlers::get_metrics))
        .route(
            "/presets/providers",
            get(crate::dashboard_handlers::get_provider_presets),
        )
        .route(
            "/presets/apikeys",
            get(crate::dashboard_handlers::get_apikey_presets),
        )
}

fn build_balance_compatibility_router() -> Router<AppState> {
    Router::new()
        .route("/api/codex/usage", get(crate::handlers::codex_usage))
        .route("/user/balance", get(crate::handlers::deepseek_user_balance))
        .layer(CorsLayer::very_permissive())
}

fn build_dashboard_api_router() -> Router<AppState> {
    Router::new()
        .route(
            "/dashboard/auth/register",
            post(crate::dashboard_handlers::register),
        )
        .route(
            "/dashboard/auth/login",
            post(crate::dashboard_handlers::login),
        )
        .route(
            "/dashboard/captcha/challenge",
            post(crate::dashboard_handlers::create_captcha_challenge),
        )
        .route(
            "/dashboard/captcha/redeem",
            post(crate::dashboard_handlers::redeem_captcha_challenge),
        )
        .route(
            "/dashboard/auth/logout",
            post(crate::dashboard_handlers::logout),
        )
        .route("/dashboard/auth/me", get(crate::dashboard_handlers::get_me))
        .route(
            "/dashboard/auth/me",
            put(crate::dashboard_handlers::update_me),
        )
        .route(
            "/dashboard/auth/password",
            put(crate::dashboard_handlers::change_password),
        )
        .route(
            "/dashboard/users",
            get(crate::dashboard_handlers::list_users),
        )
        .route(
            "/dashboard/users",
            post(crate::dashboard_handlers::create_user),
        )
        .route(
            "/dashboard/users/{user_id}",
            get(crate::dashboard_handlers::get_user),
        )
        .route(
            "/dashboard/users/{user_id}",
            put(crate::dashboard_handlers::update_user),
        )
        .route(
            "/dashboard/users/{user_id}",
            axum::routing::delete(crate::dashboard_handlers::delete_user),
        )
        .route(
            "/dashboard/billing-plans",
            get(crate::dashboard_handlers::list_billing_plans)
                .post(crate::dashboard_handlers::create_billing_plan),
        )
        .route(
            "/dashboard/billing-plans/{plan_id}",
            put(crate::dashboard_handlers::update_billing_plan)
                .delete(crate::dashboard_handlers::delete_billing_plan),
        )
        .route(
            "/dashboard/billing-plans/{plan_id}/reset",
            post(crate::dashboard_handlers::reset_billing_plan),
        )
        .route(
            "/dashboard/tokens",
            get(crate::dashboard_handlers::list_my_api_keys),
        )
        .route(
            "/dashboard/tokens",
            post(crate::dashboard_handlers::create_api_key),
        )
        .route(
            "/dashboard/tokens/batch-delete",
            post(crate::dashboard_handlers::batch_delete_api_keys),
        )
        .route(
            "/dashboard/tokens/{key_id}",
            get(crate::dashboard_handlers::get_api_key),
        )
        .route(
            "/dashboard/tokens/{key_id}",
            put(crate::dashboard_handlers::update_api_key),
        )
        .route(
            "/dashboard/tokens/{key_id}",
            axum::routing::delete(crate::dashboard_handlers::delete_api_key),
        )
        .route(
            "/dashboard/tokens/{key_id}/transfer",
            post(crate::dashboard_handlers::transfer_to_sub_account),
        )
        .route(
            "/dashboard/settings",
            get(crate::dashboard_handlers::get_settings),
        )
        .route(
            "/dashboard/settings",
            put(crate::dashboard_handlers::update_settings),
        )
        .route(
            "/dashboard/settings/public",
            get(crate::dashboard_handlers::get_public_settings),
        )
        .route(
            "/dashboard/stats",
            get(crate::dashboard_handlers::get_dashboard_stats),
        )
        .route(
            "/dashboard/config",
            get(crate::dashboard_handlers::get_config_overview),
        )
        .route(
            "/dashboard/groups",
            get(crate::dashboard_handlers::list_dashboard_groups),
        )
        .route(
            "/dashboard/groups",
            post(crate::dashboard_handlers::create_group),
        )
        .route(
            "/dashboard/groups/reorder",
            post(crate::dashboard_handlers::reorder_groups),
        )
        .route(
            "/dashboard/groups/{group_id}",
            put(crate::dashboard_handlers::update_group),
        )
        .route(
            "/dashboard/groups/{group_id}",
            axum::routing::delete(crate::dashboard_handlers::delete_group),
        )
        .route(
            "/dashboard/providers",
            get(crate::dashboard_handlers::list_providers),
        )
        .route(
            "/dashboard/providers",
            post(crate::dashboard_handlers::create_provider),
        )
        .route(
            "/dashboard/providers/reorder",
            post(crate::dashboard_handlers::reorder_providers),
        )
        .route(
            "/dashboard/providers/{provider_id}",
            get(crate::dashboard_handlers::get_provider),
        )
        .route(
            "/dashboard/providers/{provider_id}",
            put(crate::dashboard_handlers::update_provider),
        )
        .route(
            "/dashboard/providers/{provider_id}",
            axum::routing::delete(crate::dashboard_handlers::delete_provider),
        )
        .route(
            "/dashboard/transforms/registry",
            get(crate::dashboard_handlers::get_transform_registry),
        )
        // Model registry API routes
        .route(
            "/dashboard/models",
            get(crate::dashboard_handlers::list_models),
        )
        .route(
            "/dashboard/models",
            post(crate::dashboard_handlers::create_model),
        )
        .route(
            "/dashboard/models/{model_id}",
            get(crate::dashboard_handlers::get_model),
        )
        .route(
            "/dashboard/models/{model_id}",
            put(crate::dashboard_handlers::update_model),
        )
        .route(
            "/dashboard/models/{model_id}",
            axum::routing::delete(crate::dashboard_handlers::delete_model),
        )
        .route(
            "/dashboard/model-metadata",
            get(crate::dashboard_handlers::list_model_metadata),
        )
        .route(
            "/dashboard/model-metadata/batch-delete",
            post(crate::dashboard_handlers::batch_delete_model_metadata),
        )
        .route(
            "/dashboard/marketplace/models",
            get(crate::dashboard_handlers::list_marketplace_models),
        )
        .route(
            "/dashboard/model-metadata/sync/models-dev",
            post(crate::dashboard_handlers::sync_model_metadata_models_dev),
        )
        .route(
            "/dashboard/billing-rates",
            get(crate::dashboard_handlers::list_billing_rates),
        )
        .route(
            "/dashboard/billing-rates/sync/catalog",
            post(crate::dashboard_handlers::sync_billing_rates_catalog),
        )
        .route(
            "/dashboard/billing-rates/{*id}",
            put(crate::dashboard_handlers::upsert_billing_rate)
                .delete(crate::dashboard_handlers::delete_billing_rate),
        )
        .route(
            "/dashboard/pricing-profile-patterns",
            get(crate::dashboard_handlers::get_pricing_profile_patterns)
                .put(crate::dashboard_handlers::update_pricing_profile_patterns),
        )
        .route(
            "/dashboard/model-metadata/{*model_id}",
            get(crate::dashboard_handlers::get_model_metadata)
                .put(crate::dashboard_handlers::upsert_model_metadata)
                .delete(crate::dashboard_handlers::delete_model_metadata),
        )
        .route(
            "/dashboard/providers/{provider_id}/channels/{channel_id}/test",
            post(crate::dashboard_handlers::test_channel),
        )
        .route(
            "/dashboard/fetch-channel-models",
            post(crate::dashboard_handlers::fetch_channel_models),
        )
        .route(
            "/dashboard/request-logs/stream",
            get(crate::dashboard_handlers::stream_request_logs),
        )
        .route(
            "/dashboard/request-logs",
            get(crate::dashboard_handlers::list_my_request_logs),
        )
        .route(
            "/dashboard/request-captures/{request_id}",
            get(crate::dashboard_handlers::get_request_capture),
        )
        .route(
            "/dashboard/analytics",
            get(crate::dashboard_handlers::get_dashboard_analytics),
        )
        .route(
            "/dashboard/me/live-usage",
            get(crate::dashboard_handlers::get_my_live_usage),
        )
        .route(
            "/dashboard/admin/overview",
            get(crate::dashboard_handlers::get_admin_overview),
        )
}
