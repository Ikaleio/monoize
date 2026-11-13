use crate::auth::AuthState;
use crate::captcha::CapVerifier;
use crate::client_ip::TrustedProxyConfig;
use crate::custom_transforms::CustomTransformStore;
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
use crate::settings::{SettingsStore, normalize_pricing_model_key};
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
use std::sync::{Arc, LazyLock};
use tokio::sync::Mutex;
use tokio::time::sleep;
use tower_http::cors::CorsLayer;
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
    pub(crate) response_history:
        Arc<Mutex<crate::handlers::responses_history::ResponseHistoryStore>>,
    pub channel_affinity: Arc<Mutex<HashMap<String, ChannelAffinityBinding>>>,
    pub routing_config_revision: Arc<AtomicU64>,
    /// Replica-only overlay for Channel `websocket_supported` when the DB value is NULL.
    pub websocket_supported_overlay: Arc<DashMap<String, bool>>,
    pub settings_update_lock: Arc<Mutex<()>>,
    pub model_registry_store: ModelRegistryStore,
    pub model_price_store: crate::model_price_store::ModelPriceStore,
    pub transform_registry: Arc<TransformRegistry>,
    pub cap_verifier: CapVerifier,
    pub custom_transform_store: CustomTransformStore,
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

mod active_probe;
mod config;
mod routing;
mod startup;

pub use config::RuntimeConfig;
pub(crate) use config::runtime_config_from_settings;
#[cfg(embed_frontend)]
pub(crate) use routing::CspNonce;
pub use routing::build_app;
pub use startup::{load_state, load_state_with_runtime};
