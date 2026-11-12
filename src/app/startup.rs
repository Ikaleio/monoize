use super::active_probe::ensure_active_probe_system_user;
use super::active_probe::scheduler::ActiveProbeScheduler;
use super::*;

pub async fn load_state() -> AppResult<AppState> {
    let runtime =
        RuntimeConfig::from_env().map_err(|(code, detail)| startup_error(code, detail))?;
    load_state_with_runtime(runtime).await
}

/// Every startup failure is reported as a 400 `AppError` carrying a stable code.
fn startup_error(code: &str, detail: impl std::fmt::Display) -> AppError {
    AppError::new(
        axum::http::StatusCode::BAD_REQUEST,
        code,
        detail.to_string(),
    )
}

pub async fn load_state_with_runtime(runtime: RuntimeConfig) -> AppResult<AppState> {
    let auth = AuthState::new();
    let is_replica = runtime.node.is_replica();
    runtime
        .node
        .validate_for_dsn(&runtime.database_dsn)
        .map_err(|(code, detail)| startup_error(code, detail))?;
    let trusted_proxies = TrustedProxyConfig::from_env()
        .map_err(|err| startup_error("trusted_proxy_config_invalid", err))?;
    let cap_verifier =
        CapVerifier::from_env().map_err(|err| startup_error("cap_config_invalid", err))?;
    let http_clients = HttpClients::new(runtime.node.upstream_proxy_url.as_deref())
        .map_err(|err| startup_error("http_client_init_failed", err))?;
    let http = http_clients.global_client();

    let db = initialize_database(&runtime, is_replica).await?;

    let (log_broadcast, _) = tokio::sync::broadcast::channel::<Vec<InsertRequestLog>>(64);
    let custom_transform_store = CustomTransformStore::new(db.clone())
        .await
        .map_err(|err| startup_error("custom_transform_store_init_failed", err))?;
    let pending_request_logs = Arc::new(DashMap::new());
    let user_store = UserStore::new_for_role(
        db.clone(),
        log_broadcast.clone(),
        pending_request_logs.clone(),
        runtime.request_log_spool_dir.clone(),
        is_replica,
    )
    .await
    .map_err(|err| startup_error("user_store_init_failed", err))?
    .with_custom_transforms(custom_transform_store.snapshot_handle());
    let settings_store = if is_replica {
        SettingsStore::new_read_only(db.clone()).await
    } else {
        SettingsStore::new(db.clone()).await
    }
    .map_err(|err| startup_error("settings_store_init_failed", err))?;
    let monoize_store = if is_replica {
        MonoizeRoutingStore::new_read_only(db.clone()).await
    } else {
        MonoizeRoutingStore::new(db.clone()).await
    }
    .map_err(|err| startup_error("monoize_store_init_failed", err))?;
    let model_registry_store = ModelRegistryStore::new(db.clone())
        .await
        .map_err(|err| startup_error("model_registry_store_init_failed", err))?;
    let model_price_store = crate::model_price_store::ModelPriceStore::new(db.clone());

    let metrics = METRICS.clone()?;

    let settings_snapshot = settings_store
        .get_all()
        .await
        .map_err(|err| startup_error("settings_store_init_failed", err))?;

    let monoize_runtime = runtime_config_from_settings(&settings_snapshot);
    let channel_health = Arc::new(Mutex::new(HashMap::<String, ChannelHealthState>::new()));
    let channel_affinity = Arc::new(Mutex::new(HashMap::new()));
    let response_history = Arc::new(Mutex::new(
        crate::handlers::responses_history::ResponseHistoryStore::default(),
    ));
    register_upstream_hosts(&runtime, &monoize_store).await;
    let routing_config_revision = Arc::new(AtomicU64::new(0));
    let settings_update_lock = Arc::new(Mutex::new(()));
    let transform_registry = Arc::new(crate::transforms::registry());
    let image_transform_cache = Arc::new(
        ImageTransformCache::from_env()
            .await
            .map_err(|err| startup_error("image_transform_cache_init_failed", err))?,
    );
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
    spawn_affinity_cleanup(
        channel_affinity.clone(),
        response_history.clone(),
        background_shutdown.clone(),
    );
    if !is_replica {
        spawn_recharge_expiry_sweeper(user_store.clone(), background_shutdown.clone());
    }

    let monoize_runtime = Arc::new(tokio::sync::RwLock::new(monoize_runtime));
    if is_replica {
        // E3: fixed-interval, single-row epoch poll driving snapshot rebuilds.
        crate::replica::poll::spawn_config_epoch_poller(
            db.clone(),
            settings_store.clone(),
            monoize_runtime.clone(),
            custom_transform_store.clone(),
            runtime.node.config_poll_interval,
        );
    }
    let request_capture = RequestCaptureStore::new(&runtime.database_dsn)
        .with_db(db.clone())
        .with_runtime(monoize_runtime.clone());
    request_capture.spawn_cleanup_task();
    // PRP11: replicas never run the active-probe scheduler.
    if let Some(user_id) = active_probe_user_id {
        ActiveProbeScheduler {
            store: monoize_store.clone(),
            http_clients: http_clients.clone(),
            runtime: monoize_runtime.clone(),
            health: channel_health.clone(),
            routing_config_revision: routing_config_revision.clone(),
            user_store: user_store.clone(),
            model_price_store: model_price_store.clone(),
            user_id,
            shutdown: background_shutdown.clone(),
            _task_registration: RequestLogTaskRegistration::new(request_log_tasks.clone()),
        }
        .spawn();
    }

    let node = Arc::new(runtime.node.clone());
    let started_at = chrono::Utc::now();
    let metering = initialize_metering(&runtime, &db, &user_store, started_at, is_replica).await?;
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
        db_pool: db,
        metering,
        metering_token_digest,
        replica_heartbeats: Arc::new(DashMap::new()),
        metrics,
        user_store,
        settings_store,
        monoize_store,
        monoize_runtime,
        channel_health,
        response_history,
        channel_affinity,
        routing_config_revision,
        websocket_supported_overlay: Arc::new(DashMap::new()),
        settings_update_lock,
        model_registry_store,
        model_price_store,
        transform_registry,
        cap_verifier,
        custom_transform_store,
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

/// A process can install only one global recorder, so every `load_state_with_runtime`
/// call shares the first install outcome.
static METRICS: LazyLock<AppResult<PrometheusHandle>> = LazyLock::new(|| {
    metrics_exporter_prometheus::PrometheusBuilder::new()
        .install_recorder()
        .map_err(|err| startup_error("metrics_init_failed", err))
});

async fn initialize_database(runtime: &RuntimeConfig, is_replica: bool) -> AppResult<DbPool> {
    let db = DbPool::connect(&runtime.database_dsn)
        .await
        .map_err(|err| startup_error("database_init_failed", err))?;

    if !is_replica {
        let write_guard = db.write().await;
        crate::migration::run_startup_migrations(&write_guard)
            .await
            .map_err(|err| startup_error("database_migration_failed", err))?;
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
                startup_error(code, err)
            })?;
    }

    Ok(db)
}

async fn register_upstream_hosts(runtime: &RuntimeConfig, monoize_store: &MonoizeRoutingStore) {
    // SAN-D1 step 2: request logs and early errors can name any configured host.
    if let Some(proxy_url) = runtime.node.upstream_proxy_url.as_deref() {
        crate::error_sanitize::register_configured_url(proxy_url);
    }
    match monoize_store.list_providers().await {
        Ok(providers) => {
            for channel in providers.iter().flat_map(|provider| &provider.channels) {
                crate::error_sanitize::register_configured_url(&channel.base_url);
                if let Some(proxy_url) = channel.proxy_url.as_deref() {
                    crate::error_sanitize::register_configured_url(proxy_url);
                }
            }
        }
        Err(err) => tracing::warn!(error = %err, "failed to load configured upstream hosts"),
    }
}

fn spawn_affinity_cleanup(
    affinity: Arc<Mutex<HashMap<String, ChannelAffinityBinding>>>,
    history: Arc<Mutex<crate::handlers::responses_history::ResponseHistoryStore>>,
    shutdown: Arc<AtomicBool>,
) {
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
            drop(guard);
            history.lock().await.cleanup();
        }
    });
}

/// RC-X1/RC-X2 (`recharge-system.spec.md`): primary-only expiry sweeper; the first
/// tick runs when background tasks start.
fn spawn_recharge_expiry_sweeper(user_store: UserStore, shutdown: Arc<AtomicBool>) {
    tokio::spawn(async move {
        let interval = std::time::Duration::from_secs(crate::recharge::tick_interval_secs());
        loop {
            if shutdown.load(Ordering::Acquire) {
                break;
            }
            if let Err(error) = user_store.expire_due_recharge_orders().await {
                tracing::warn!(error, "recharge expiry sweep failed");
            }
            sleep(interval).await;
        }
    });
}

/// Replicas build the metering shipment pipeline; primaries drain any delta spool
/// left behind from a previous life as a replica.
async fn initialize_metering(
    runtime: &RuntimeConfig,
    db: &DbPool,
    user_store: &UserStore,
    started_at: chrono::DateTime<chrono::Utc>,
    is_replica: bool,
) -> AppResult<Option<Arc<crate::replica::metering::ReplicaMetering>>> {
    if !is_replica {
        // PRP9: a promoted node drains leftover delta spool entries before serving.
        if runtime.node.metering_spool_dir.exists() {
            let spool = crate::replica::metering::DeltaSpool::new(
                runtime.node.metering_spool_dir.clone(),
                runtime.node.metering_spool_max_bytes,
            )
            .map_err(|err| startup_error("metering_drain_failed", err))?;
            crate::replica::metering::drain_delta_spool_to_local_db(db, user_store, &spool)
                .await
                .map_err(|err| startup_error("metering_drain_failed", err))?;
        }
        return Ok(None);
    }

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
        startup_error(code, err)
    })?;
    let metering = Arc::new(metering);
    metering.spawn_ship_loop(
        user_store.request_log_batcher_clone(),
        user_store.last_used_batcher_clone(),
        runtime.node.metering_ship_interval,
    );
    Ok(Some(metering))
}
