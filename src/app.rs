use crate::auth::AuthState;
use crate::config::UnknownFieldPolicy;
use crate::error::{AppError, AppResult};
use crate::model_registry::ModelRegistry;
use crate::model_registry_store::ModelRegistryStore;
use crate::monoize_routing::{
    ChannelHealthState, MonoizeRoutingStore, MonoizeRuntimeConfig, probe_channel_list_models,
};
use crate::providers::ProviderStore;
use crate::settings::SettingsStore;
use crate::transforms::TransformRegistry;
use crate::users::UserStore;
use axum::Router;
use axum::routing::{get, post, put};
use metrics_exporter_prometheus::PrometheusHandle;
use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::{Arc, Once, OnceLock};
use tokio::sync::Mutex;
use tokio::time::sleep;
use tower_http::request_id::{MakeRequestUuid, PropagateRequestIdLayer, SetRequestIdLayer};
use tower_http::trace::TraceLayer;

#[derive(Clone)]
pub struct AppState {
    pub runtime: Arc<RuntimeConfig>,
    pub auth: AuthState,
    pub model_registry: ModelRegistry,
    pub http: reqwest::Client,
    pub metrics: PrometheusHandle,
    pub group_counters: Arc<Mutex<HashMap<String, u64>>>,
    pub user_store: UserStore,
    pub settings_store: SettingsStore,
    pub provider_store: ProviderStore,
    pub monoize_store: MonoizeRoutingStore,
    pub monoize_runtime: MonoizeRuntimeConfig,
    pub channel_health: Arc<Mutex<HashMap<String, ChannelHealthState>>>,
    pub model_registry_store: ModelRegistryStore,
    pub transform_registry: Arc<TransformRegistry>,
}

static METRICS_HANDLE: OnceLock<PrometheusHandle> = OnceLock::new();
static METRICS_ERROR: OnceLock<AppError> = OnceLock::new();
static METRICS_INIT: Once = Once::new();

#[derive(Debug, Clone)]
pub struct RuntimeConfig {
    pub listen: String,
    pub metrics_path: String,
    pub unknown_fields: UnknownFieldPolicy,
    pub database_dsn: String,
}

impl RuntimeConfig {
    pub fn from_env() -> Self {
        let listen = std::env::var("MONOIZE_LISTEN")
            .ok()
            .filter(|v| !v.trim().is_empty())
            .unwrap_or_else(|| "0.0.0.0:8080".to_string());
        let metrics_path = std::env::var("MONOIZE_METRICS_PATH")
            .ok()
            .filter(|v| !v.trim().is_empty())
            .unwrap_or_else(|| "/metrics".to_string());
        let unknown_fields = std::env::var("MONOIZE_UNKNOWN_FIELDS")
            .ok()
            .and_then(|v| parse_unknown_fields_policy(&v))
            .unwrap_or(UnknownFieldPolicy::Preserve);
        let database_dsn = resolve_database_dsn();
        Self {
            listen,
            metrics_path,
            unknown_fields,
            database_dsn,
        }
    }
}

pub async fn load_state() -> AppResult<AppState> {
    load_state_with_runtime(RuntimeConfig::from_env()).await
}

pub async fn load_state_with_runtime(runtime: RuntimeConfig) -> AppResult<AppState> {
    let auth = AuthState::new();

    let http = reqwest::Client::builder()
        .user_agent("monoize/0.1")
        .build()
        .map_err(|err| {
            AppError::new(
                axum::http::StatusCode::BAD_REQUEST,
                "http_client_init_failed",
                err.to_string(),
            )
        })?;

    ensure_sqlite_file(&runtime.database_dsn).map_err(|err| {
        AppError::new(
            axum::http::StatusCode::BAD_REQUEST,
            "database_init_failed",
            err,
        )
    })?;

    let pool = sqlx::sqlite::SqlitePoolOptions::new()
        .max_connections(5)
        .connect(&runtime.database_dsn)
        .await
        .map_err(|err| {
            AppError::new(
                axum::http::StatusCode::BAD_REQUEST,
                "database_init_failed",
                err.to_string(),
            )
        })?;

    let user_store = UserStore::new(pool.clone()).await.map_err(|err| {
        AppError::new(
            axum::http::StatusCode::BAD_REQUEST,
            "user_store_init_failed",
            err,
        )
    })?;
    let settings_store = SettingsStore::new(pool.clone()).await.map_err(|err| {
        AppError::new(
            axum::http::StatusCode::BAD_REQUEST,
            "settings_store_init_failed",
            err,
        )
    })?;
    let provider_store = ProviderStore::new(pool.clone()).await.map_err(|err| {
        AppError::new(
            axum::http::StatusCode::BAD_REQUEST,
            "provider_store_init_failed",
            err,
        )
    })?;
    let monoize_store = MonoizeRoutingStore::new(pool.clone())
        .await
        .map_err(|err| {
            AppError::new(
                axum::http::StatusCode::BAD_REQUEST,
                "monoize_store_init_failed",
                err,
            )
        })?;
    let model_registry_store = ModelRegistryStore::new(pool).await.map_err(|err| {
        AppError::new(
            axum::http::StatusCode::BAD_REQUEST,
            "model_registry_store_init_failed",
            err,
        )
    })?;

    let metrics = init_metrics()?;

    let model_registry = ModelRegistry::new();
    let db_records = model_registry_store
        .list_enabled_models()
        .await
        .map_err(|err| {
            AppError::new(
                axum::http::StatusCode::BAD_REQUEST,
                "model_registry_db_load_failed",
                err,
            )
        })?;
    model_registry.replace_db_records(db_records).await;

    let monoize_runtime = MonoizeRuntimeConfig::default();
    let channel_health = Arc::new(Mutex::new(HashMap::new()));
    let transform_registry = Arc::new(crate::transforms::registry());

    if monoize_runtime.active_enabled {
        let probe_store = monoize_store.clone();
        let probe_http = http.clone();
        let probe_runtime = monoize_runtime.clone();
        let probe_health = channel_health.clone();
        tokio::spawn(async move {
            loop {
                sleep(std::time::Duration::from_secs(
                    probe_runtime.active_interval_seconds,
                ))
                .await;
                let providers = match probe_store.list_providers().await {
                    Ok(v) => v,
                    Err(_) => continue,
                };
                let now = chrono::Utc::now().timestamp();
                for provider in providers {
                    for channel in provider.channels {
                        let probe_due = {
                            let guard = probe_health.lock().await;
                            let state = guard
                                .get(&channel.id)
                                .cloned()
                                .unwrap_or_else(ChannelHealthState::new);
                            if state.healthy {
                                false
                            } else if let Some(until) = state.cooldown_until {
                                now >= until
                            } else {
                                true
                            }
                        };
                        if !probe_due {
                            continue;
                        }

                        let ok = if probe_runtime.active_method == "list_models" {
                            probe_channel_list_models(
                                &probe_http,
                                &channel,
                                probe_runtime.request_timeout_ms,
                            )
                            .await
                        } else {
                            probe_channel_list_models(
                                &probe_http,
                                &channel,
                                probe_runtime.request_timeout_ms,
                            )
                            .await
                        };

                        let mut guard = probe_health.lock().await;
                        let state = guard
                            .entry(channel.id.clone())
                            .or_insert_with(ChannelHealthState::new);
                        if ok {
                            state.probe_success_count = state.probe_success_count.saturating_add(1);
                            if state.probe_success_count >= probe_runtime.active_success_threshold {
                                state.healthy = true;
                                state.failure_count = 0;
                                state.cooldown_until = None;
                                state.last_success_at = Some(now);
                                state.probe_success_count = 0;
                            }
                        } else {
                            state.healthy = false;
                            state.probe_success_count = 0;
                            state.cooldown_until =
                                Some(now + probe_runtime.passive_cooldown_seconds as i64);
                        }
                    }
                }
            }
        });
    }

    Ok(AppState {
        runtime: Arc::new(runtime),
        auth,
        model_registry,
        http,
        metrics,
        group_counters: Arc::new(Mutex::new(HashMap::new())),
        user_store,
        settings_store,
        provider_store,
        monoize_store,
        monoize_runtime,
        channel_health,
        model_registry_store,
        transform_registry,
    })
}

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

fn parse_unknown_fields_policy(value: &str) -> Option<UnknownFieldPolicy> {
    match value.trim().to_ascii_lowercase().as_str() {
        "reject" => Some(UnknownFieldPolicy::Reject),
        "ignore" => Some(UnknownFieldPolicy::Ignore),
        "preserve" => Some(UnknownFieldPolicy::Preserve),
        _ => None,
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

fn ensure_sqlite_file(dsn: &str) -> Result<(), String> {
    let dsn = dsn.trim();
    if !dsn.starts_with("sqlite://") {
        return Ok(());
    }
    if dsn.contains(":memory:") || dsn.contains("mode=memory") {
        return Ok(());
    }
    let path_part = dsn.trim_start_matches("sqlite://");
    let path_part = path_part.split('?').next().unwrap_or("");
    if path_part.is_empty() {
        return Ok(());
    }
    let path = PathBuf::from(path_part);
    if let Some(parent) = path.parent() {
        if !parent.as_os_str().is_empty() {
            std::fs::create_dir_all(parent)
                .map_err(|err| format!("sqlite_dir_create_failed: {err}"))?;
        }
    }
    if !path.exists() {
        std::fs::File::create(&path).map_err(|err| format!("sqlite_file_create_failed: {err}"))?;
    }
    Ok(())
}

pub fn build_app(state: AppState) -> Router {
    let metrics_path = state.runtime.metrics_path.clone();
    let api_router = build_api_router(&metrics_path);
    Router::<AppState>::new()
        .merge(api_router.clone())
        .nest("/api", api_router)
        .fallback(crate::frontend::frontend_fallback)
        .with_state(state)
        .layer(SetRequestIdLayer::new(
            axum::http::header::HeaderName::from_static("x-request-id"),
            MakeRequestUuid,
        ))
        .layer(PropagateRequestIdLayer::new(
            axum::http::header::HeaderName::from_static("x-request-id"),
        ))
        .layer(TraceLayer::new_for_http())
}

fn build_api_router(metrics_path: &str) -> Router<AppState> {
    Router::new()
        .route("/v1/responses", post(crate::handlers::create_response))
        .route(
            "/v1/chat/completions",
            post(crate::handlers::create_chat_completions),
        )
        .route("/v1/embeddings", post(crate::handlers::create_embeddings))
        .route("/v1/messages", post(crate::handlers::create_messages))
        .route(metrics_path, get(crate::handlers::metrics))
        // Dashboard API routes
        .route(
            "/dashboard/auth/register",
            post(crate::dashboard_handlers::register),
        )
        .route(
            "/dashboard/auth/login",
            post(crate::dashboard_handlers::login),
        )
        .route(
            "/dashboard/auth/logout",
            post(crate::dashboard_handlers::logout),
        )
        .route("/dashboard/auth/me", get(crate::dashboard_handlers::get_me))
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
        .route(
            "/presets/providers",
            get(crate::dashboard_handlers::get_provider_presets),
        )
        .route(
            "/presets/apikeys",
            get(crate::dashboard_handlers::get_apikey_presets),
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
            "/dashboard/model-metadata/sync/models-dev",
            post(crate::dashboard_handlers::sync_model_metadata_models_dev),
        )
        .route(
            "/dashboard/model-metadata/{*model_id}",
            get(crate::dashboard_handlers::get_model_metadata)
                .put(crate::dashboard_handlers::upsert_model_metadata)
                .delete(crate::dashboard_handlers::delete_model_metadata),
        )
        .route(
            "/dashboard/providers/{provider_id}/fetch-models",
            post(crate::dashboard_handlers::fetch_provider_models),
        )
        .route(
            "/dashboard/fetch-channel-models",
            post(crate::dashboard_handlers::fetch_channel_models),
        )
        .route(
            "/dashboard/request-logs",
            get(crate::dashboard_handlers::list_my_request_logs),
        )
}
