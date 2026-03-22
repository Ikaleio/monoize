use crate::auth::AuthState;
use crate::db::DbPool;
use crate::error::{AppError, AppResult};
use crate::handlers::routing::health_key;
use crate::image_transform_cache::ImageTransformCache;
use crate::model_registry::ModelRegistry;
use crate::model_registry_store::ModelRegistryStore;
use crate::monoize_routing::{
    ChannelHealthState, MonoizeRoutingStore, MonoizeRuntimeConfig, probe_channel_completion,
};
use crate::name_cache::NameCaches;
use crate::providers::ProviderStore;
use crate::rate_limit::RateLimiter;
use crate::settings::{SettingsStore, normalize_pricing_model_key};
use crate::transforms::TransformRegistry;
use crate::users::{InsertRequestLog, UserRole, UserStore};
use axum::Router;
use axum::extract::DefaultBodyLimit;
use axum::routing::{get, post, put};
use dashmap::DashMap;
use metrics_exporter_prometheus::PrometheusHandle;
use serde_json::{Value, json};
use std::collections::HashMap;
use std::sync::atomic::AtomicUsize;
use std::sync::{Arc, Once, OnceLock};
use tokio::sync::Mutex;
use tokio::time::sleep;
use tower_http::limit::RequestBodyLimitLayer;
use tower_http::request_id::{MakeRequestUuid, PropagateRequestIdLayer, SetRequestIdLayer};
use tower_http::set_header::SetResponseHeaderLayer;
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
    pub name_caches: NameCaches,
    pub settings_store: SettingsStore,
    pub provider_store: ProviderStore,
    pub monoize_store: MonoizeRoutingStore,
    pub monoize_runtime: Arc<tokio::sync::RwLock<MonoizeRuntimeConfig>>,
    pub channel_health: Arc<Mutex<HashMap<String, ChannelHealthState>>>,
    pub model_registry_store: ModelRegistryStore,
    pub transform_registry: Arc<TransformRegistry>,
    pub auth_rate_limiter: RateLimiter,
    pub log_broadcast: tokio::sync::broadcast::Sender<Vec<InsertRequestLog>>,
    pub sse_connections: Arc<DashMap<String, AtomicUsize>>,
    pub image_transform_cache: Arc<ImageTransformCache>,
}

const ACTIVE_PROBE_CONNECTIVITY_KIND: &str = "active_probe_connectivity";
const ACTIVE_PROBE_SYSTEM_USER: &str = "_monoize_active_probe";

static METRICS_HANDLE: OnceLock<PrometheusHandle> = OnceLock::new();
static METRICS_ERROR: OnceLock<AppError> = OnceLock::new();
static METRICS_INIT: Once = Once::new();

#[derive(Debug, Clone)]
pub struct RuntimeConfig {
    pub listen: String,
    pub metrics_path: String,
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
        let database_dsn = resolve_database_dsn();
        Self {
            listen,
            metrics_path,
            database_dsn,
        }
    }
}

pub async fn load_state() -> AppResult<AppState> {
    load_state_with_runtime(RuntimeConfig::from_env()).await
}

#[allow(clippy::field_reassign_with_default)]
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

    let db = DbPool::connect(&runtime.database_dsn)
        .await
        .map_err(|err| {
            AppError::new(
                axum::http::StatusCode::BAD_REQUEST,
                "database_init_failed",
                err.to_string(),
            )
        })?;

    {
        use sea_orm_migration::MigratorTrait;
        let _write_guard = db.write().await;
        crate::migration::Migrator::up(&*_write_guard, None)
            .await
            .map_err(|err| {
                AppError::new(
                    axum::http::StatusCode::BAD_REQUEST,
                    "database_migration_failed",
                    err.to_string(),
                )
            })?;
    }

    let (log_broadcast, _) = tokio::sync::broadcast::channel::<Vec<InsertRequestLog>>(64);

    let user_store = UserStore::new(db.clone(), log_broadcast.clone())
        .await
        .map_err(|err| {
            AppError::new(
                axum::http::StatusCode::BAD_REQUEST,
                "user_store_init_failed",
                err,
            )
        })?;
    let name_caches = NameCaches::init(db.read()).await.map_err(|err| {
        AppError::new(
            axum::http::StatusCode::BAD_REQUEST,
            "name_cache_init_failed",
            err,
        )
    })?;
    let settings_store = SettingsStore::new(db.clone()).await.map_err(|err| {
        AppError::new(
            axum::http::StatusCode::BAD_REQUEST,
            "settings_store_init_failed",
            err,
        )
    })?;
    let provider_store = ProviderStore::new(db.clone()).await.map_err(|err| {
        AppError::new(
            axum::http::StatusCode::BAD_REQUEST,
            "provider_store_init_failed",
            err,
        )
    })?;
    let monoize_store = MonoizeRoutingStore::new(db.clone()).await.map_err(|err| {
        AppError::new(
            axum::http::StatusCode::BAD_REQUEST,
            "monoize_store_init_failed",
            err,
        )
    })?;
    let model_registry_store = ModelRegistryStore::new(db).await.map_err(|err| {
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

    let settings_snapshot = settings_store.get_all().await.map_err(|err| {
        AppError::new(
            axum::http::StatusCode::BAD_REQUEST,
            "settings_store_init_failed",
            err,
        )
    })?;

    let mut monoize_runtime = MonoizeRuntimeConfig::default();
    monoize_runtime.passive_failure_count_threshold =
        settings_snapshot.monoize_passive_failure_threshold.max(1);
    monoize_runtime.passive_cooldown_seconds =
        settings_snapshot.monoize_passive_cooldown_seconds.max(1);
    monoize_runtime.passive_window_seconds =
        settings_snapshot.monoize_passive_window_seconds.max(1);
    monoize_runtime.passive_rate_limit_cooldown_seconds = settings_snapshot
        .monoize_passive_rate_limit_cooldown_seconds
        .max(1);
    monoize_runtime.active_enabled = settings_snapshot.monoize_active_probe_enabled;
    monoize_runtime.active_interval_seconds = settings_snapshot
        .monoize_active_probe_interval_seconds
        .max(1);
    monoize_runtime.active_success_threshold = settings_snapshot
        .monoize_active_probe_success_threshold
        .max(1);
    monoize_runtime.active_probe_model = settings_snapshot.monoize_active_probe_model.clone();
    monoize_runtime.request_timeout_ms = settings_snapshot.monoize_request_timeout_ms.max(1);
    let channel_health = Arc::new(Mutex::new(HashMap::new()));
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
    let _ = ensure_active_probe_system_user(&user_store).await;

    let probe_store = monoize_store.clone();
    let probe_http = http.clone();
    let monoize_runtime = Arc::new(tokio::sync::RwLock::new(monoize_runtime));
    let probe_runtime = monoize_runtime.clone();
    let probe_health = channel_health.clone();
    let probe_user_store = user_store.clone();
    let probe_model_registry_store = model_registry_store.clone();
    let probe_settings_store = settings_store.clone();
    tokio::spawn(async move {
        loop {
            sleep(std::time::Duration::from_secs(1)).await;
            let providers = match probe_store.list_providers().await {
                Ok(v) => v,
                Err(_) => continue,
            };
            let now = chrono::Utc::now().timestamp();
            let rt_snap = probe_runtime.read().await.clone();
            for provider in providers {
                if !provider.circuit_breaker_enabled {
                    continue;
                }
                let active_enabled = provider
                    .active_probe_enabled_override
                    .unwrap_or(rt_snap.active_enabled);
                if !active_enabled {
                    continue;
                }
                let probe_interval_seconds = provider
                    .active_probe_interval_seconds_override
                    .unwrap_or(rt_snap.active_interval_seconds)
                    .max(1);
                let probe_success_threshold = provider
                    .active_probe_success_threshold_override
                    .unwrap_or(rt_snap.active_success_threshold)
                    .max(1);

                for channel in provider.channels {
                    let probe_due = {
                        let guard = probe_health.lock().await;
                        let states = if provider.per_model_circuit_break {
                            channel_health_keys(&guard, &channel.id)
                                .into_iter()
                                .filter_map(|key| guard.get(&key).cloned())
                                .collect::<Vec<_>>()
                        } else {
                            vec![guard
                                .get(&health_key(&channel.id, None))
                                .cloned()
                                .unwrap_or_else(ChannelHealthState::new)]
                        };
                        let unhealthy_states: Vec<ChannelHealthState> =
                            states.into_iter().filter(|state| !state.healthy).collect();
                        if unhealthy_states.is_empty() {
                            false
                        } else if !unhealthy_states.iter().any(|state| {
                            state
                                .cooldown_until
                                .map(|until| now >= until)
                                .unwrap_or(true)
                        }) {
                            false
                        } else if let Some(last_probe_at) =
                            unhealthy_states.iter().filter_map(|state| state.last_probe_at).max()
                        {
                            now.saturating_sub(last_probe_at) >= probe_interval_seconds as i64
                        } else {
                            true
                        }
                    };
                    if !probe_due {
                        continue;
                    }

                    let configured_model = provider
                        .active_probe_model_override
                        .clone()
                        .or(rt_snap.active_probe_model.clone());
                    let first_model = provider.models.keys().next().cloned();
                    let probe_model = configured_model.clone().or(first_model.clone());
                    let Some(ref model_name) = probe_model else {
                        continue;
                    };
                    let active_probe_user_id =
                        ensure_active_probe_system_user(&probe_user_store).await;
                    let probe_started_at = std::time::Instant::now();
                    let (ok, usage_snapshot) = probe_channel_completion(
                        &probe_http,
                        &channel,
                        rt_snap.request_timeout_ms,
                        model_name,
                        provider.provider_type,
                        &provider.api_type_overrides,
                    )
                    .await;
                    spawn_active_probe_request_log(
                        probe_user_store.clone(),
                        probe_model_registry_store.clone(),
                        probe_settings_store.clone(),
                        active_probe_user_id.clone(),
                        provider.id.clone(),
                        provider.name.clone(),
                        provider
                            .models
                            .get(model_name)
                            .map(|entry| entry.multiplier),
                        channel.id.clone(),
                        channel.name.clone(),
                        model_name.to_string(),
                        usage_snapshot,
                        probe_started_at.elapsed().as_millis() as u64,
                        ok,
                    );
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
                    if ok {
                        if provider.per_model_circuit_break {
                            let keys = channel_health_keys(&guard, &channel.id);
                            let mut reached_threshold = false;
                            for key in &keys {
                                if let Some(state) = guard.get_mut(key) {
                                    state.last_probe_at = Some(now);
                                    state.probe_success_count =
                                        state.probe_success_count.saturating_add(1);
                                    reached_threshold |=
                                        state.probe_success_count >= probe_success_threshold;
                                }
                            }
                            if reached_threshold {
                                for key in keys {
                                    if let Some(state) = guard.get_mut(&key) {
                                        clear_channel_health_state(state, now);
                                    }
                                }
                            }
                        } else {
                            let state = guard
                                .entry(health_key(&channel.id, None))
                                .or_insert_with(ChannelHealthState::new);
                            state.last_probe_at = Some(now);
                            state.probe_success_count =
                                state.probe_success_count.saturating_add(1);
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
                            for key in channel_health_keys(&guard, &channel.id) {
                                if let Some(state) = guard.get_mut(&key) {
                                    state.healthy = false;
                                    state.probe_success_count = 0;
                                    state.last_probe_at = Some(now);
                                    state.cooldown_until = Some(now + cooldown_seconds as i64);
                                }
                            }
                        } else {
                            let state = guard
                                .entry(health_key(&channel.id, None))
                                .or_insert_with(ChannelHealthState::new);
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

    Ok(AppState {
        runtime: Arc::new(runtime),
        auth,
        model_registry,
        http,
        metrics,
        group_counters: Arc::new(Mutex::new(HashMap::new())),
        user_store,
        name_caches,
        settings_store,
        provider_store,
        monoize_store,
        monoize_runtime,
        channel_health,
        model_registry_store,
        transform_registry,
        auth_rate_limiter: RateLimiter::new(10, std::time::Duration::from_secs(60)),
        log_broadcast,
        sse_connections: Arc::new(DashMap::new()),
        image_transform_cache,
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

async fn ensure_active_probe_system_user(user_store: &UserStore) -> Option<String> {
    let existing = match user_store
        .get_user_by_username(ACTIVE_PROBE_SYSTEM_USER)
        .await
    {
        Ok(v) => v,
        Err(err) => {
            tracing::warn!("failed to query active probe system user: {err}");
            return None;
        }
    };
    if let Some(user) = existing {
        if !user.balance_unlimited {
            if let Err(err) = user_store
                .update_user(&user.id, None, None, None, None, None, Some(true), None)
                .await
            {
                tracing::warn!("failed to set active probe system user unlimited balance: {err}");
            }
        }
        return Some(user.id);
    }
    match user_store
        .create_user(
            ACTIVE_PROBE_SYSTEM_USER,
            &uuid::Uuid::new_v4().to_string(),
            UserRole::User,
        )
        .await
    {
        Ok(user) => {
            if let Err(err) = user_store
                .update_user(&user.id, None, None, None, None, None, Some(true), None)
                .await
            {
                tracing::warn!("failed to set active probe system user unlimited balance: {err}");
            }
            Some(user.id)
        }
        Err(err) => {
            tracing::warn!("failed to create active probe system user: {err}");
            None
        }
    }
}

fn parse_pricing_i128(raw: Option<String>) -> Option<i128> {
    raw.and_then(|v| v.parse::<i128>().ok())
}

fn scale_charge_with_multiplier(base_nano: i128, provider_multiplier: f64) -> Option<i128> {
    if !provider_multiplier.is_finite() || provider_multiplier < 0.0 {
        return None;
    }

    const SCALE: i128 = 1_000_000_000;
    let multiplier_repr = format!("{provider_multiplier:.18}");
    let mut parts = multiplier_repr.split('.');
    let whole = parts.next().unwrap_or("0").parse::<i128>().ok()?;
    let frac_raw = parts.next().unwrap_or("0");
    let mut frac_nano = String::with_capacity(9);
    for ch in frac_raw.chars().take(9) {
        frac_nano.push(ch);
    }
    while frac_nano.len() < 9 {
        frac_nano.push('0');
    }
    let frac = frac_nano.parse::<i128>().ok()?;

    let multiplier_nano = whole.checked_mul(SCALE)?.checked_add(frac)?;
    base_nano.checked_mul(multiplier_nano)?.checked_div(SCALE)
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
    provider_name: String,
    upstream_model: String,
    provider_multiplier: f64,
    prompt_tokens: u64,
    completion_tokens: u64,
    input_rate_nano: i128,
    output_rate_nano: i128,
    final_charge_nano: i128,
) -> Value {
    let prompt_charge = i128::from(prompt_tokens)
        .checked_mul(input_rate_nano)
        .unwrap_or_default();
    let completion_charge = i128::from(completion_tokens)
        .checked_mul(output_rate_nano)
        .unwrap_or_default();
    let base_charge = prompt_charge
        .checked_add(completion_charge)
        .unwrap_or_default();
    json!({
        "version": 1,
        "currency": "nano_usd",
        "logical_model": upstream_model,
        "upstream_model": upstream_model,
        "provider_id": provider_name,
        "provider_multiplier": provider_multiplier,
        "input": {
            "total_tokens": prompt_tokens,
            "cached_tokens": 0,
            "billed_uncached_tokens": prompt_tokens,
            "billed_cached_tokens": 0,
            "unit_price_nano": input_rate_nano.to_string(),
            "cached_unit_price_nano": null,
            "uncached_charge_nano": prompt_charge.to_string(),
            "cached_charge_nano": "0",
            "total_charge_nano": prompt_charge.to_string()
        },
        "output": {
            "total_tokens": completion_tokens,
            "reasoning_tokens": 0,
            "billed_non_reasoning_tokens": completion_tokens,
            "billed_reasoning_tokens": 0,
            "unit_price_nano": output_rate_nano.to_string(),
            "reasoning_unit_price_nano": null,
            "non_reasoning_charge_nano": completion_charge.to_string(),
            "reasoning_charge_nano": "0",
            "total_charge_nano": completion_charge.to_string()
        },
        "base_charge_nano": base_charge.to_string(),
        "final_charge_nano": final_charge_nano.to_string()
    })
}

#[allow(clippy::too_many_arguments)]
fn spawn_active_probe_request_log(
    user_store: UserStore,
    model_registry_store: ModelRegistryStore,
    settings_store: SettingsStore,
    user_id: Option<String>,
    provider_id: String,
    provider_name: String,
    provider_multiplier: Option<f64>,
    channel_id: String,
    _channel_name: String,
    model: String,
    usage_snapshot: Option<Value>,
    duration_ms: u64,
    status_ok: bool,
) {
    let Some(user_id) = user_id else {
        return;
    };
    let provider_multiplier = provider_multiplier.unwrap_or(1.0);
    tokio::spawn(async move {
        let parsed_prompt_tokens = usage_snapshot
            .as_ref()
            .and_then(|v| v.get("prompt_tokens"))
            .and_then(|v| v.as_u64());
        let parsed_completion_tokens = usage_snapshot
            .as_ref()
            .and_then(|v| v.get("completion_tokens"))
            .and_then(|v| v.as_u64());
        let usage_tokens = parsed_prompt_tokens.zip(parsed_completion_tokens);
        let reasoning_suffix_map = settings_store
            .get_reasoning_suffix_map()
            .await
            .unwrap_or_default();
        let pricing_model_key = normalize_pricing_model_key(&model, &reasoning_suffix_map);

        let (charge_nano_usd, billing_breakdown_json) = if status_ok {
            if let Some((prompt_tokens, completion_tokens)) = usage_tokens {
                match model_registry_store
                    .get_model_metadata(&pricing_model_key)
                    .await
                {
                    Ok(Some(meta)) => {
                        let input_rate =
                            parse_pricing_i128(meta.input_cost_per_token_nano).unwrap_or_default();
                        let output_rate =
                            parse_pricing_i128(meta.output_cost_per_token_nano).unwrap_or_default();
                        let base_charge = i128::from(prompt_tokens)
                            .checked_mul(input_rate)
                            .and_then(|v| {
                                v.checked_add(
                                    i128::from(completion_tokens).checked_mul(output_rate)?,
                                )
                            })
                            .unwrap_or_default();
                        let final_charge =
                            scale_charge_with_multiplier(base_charge, provider_multiplier)
                                .unwrap_or(base_charge);
                        let billing = build_probe_billing_breakdown(
                            provider_name.clone(),
                            model.clone(),
                            provider_multiplier,
                            prompt_tokens,
                            completion_tokens,
                            input_rate,
                            output_rate,
                            final_charge,
                        );
                        (Some(final_charge), Some(billing))
                    }
                    Ok(None) | Err(_) => (None, None),
                }
            } else {
                (None, None)
            }
        } else {
            (None, None)
        };

        let usage_breakdown_json = if status_ok {
            usage_tokens.map(|(prompt_tokens, completion_tokens)| {
                build_probe_usage_breakdown(prompt_tokens, completion_tokens)
            })
        } else {
            None
        };

        let log = InsertRequestLog {
            request_id: None,
            user_id,
            api_key_id: None,
            model: model.clone(),
            provider_id: Some(provider_id),
            upstream_model: Some(model),
            channel_id: Some(channel_id),
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
            status: if status_ok {
                "success".to_string()
            } else {
                "error".to_string()
            },
            usage_breakdown_json,
            billing_breakdown_json,
            error_code: if status_ok {
                None
            } else {
                Some("active_probe_failed".to_string())
            },
            error_message: if status_ok {
                None
            } else {
                Some("active probe connectivity test failed".to_string())
            },
            error_http_status: None,
            duration_ms: Some(duration_ms),
            ttfb_ms: None,
            request_ip: None,
            reasoning_effort: None,
            tried_providers_json: None,
            request_kind: Some(ACTIVE_PROBE_CONNECTIVITY_KIND.to_string()),
            created_at: chrono::Utc::now(),
        };

        if let Err(err) = user_store.insert_request_log(log).await {
            tracing::warn!("failed to insert active probe request log: {err}");
        }
    });
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

fn channel_health_keys(
    health: &HashMap<String, ChannelHealthState>,
    channel_id: &str,
) -> Vec<String> {
    let prefix = format!("{channel_id}::");
    health
        .keys()
        .filter(|key| key.as_str() == channel_id || key.starts_with(&prefix))
        .cloned()
        .collect()
}

pub fn build_app(state: AppState) -> Router {
    let metrics_path = state.runtime.metrics_path.clone();
    let root_api_router = build_root_api_router(&metrics_path);
    let dashboard_api_router = build_dashboard_api_router();
    let api_router = root_api_router.clone().merge(dashboard_api_router);
    Router::<AppState>::new()
        .merge(root_api_router)
        .nest("/api", api_router)
        .fallback(crate::frontend::frontend_fallback)
        .with_state(state)
        .layer(DefaultBodyLimit::disable())
        .layer(SetRequestIdLayer::new(
            axum::http::header::HeaderName::from_static("x-request-id"),
            MakeRequestUuid,
        ))
        .layer(PropagateRequestIdLayer::new(
            axum::http::header::HeaderName::from_static("x-request-id"),
        ))
        .layer(TraceLayer::new_for_http())
        // 50 MiB body size limit
        .layer(RequestBodyLimitLayer::new(50 * 1024 * 1024))
        // Security headers
        .layer(SetResponseHeaderLayer::overriding(
            axum::http::header::HeaderName::from_static("x-content-type-options"),
            axum::http::HeaderValue::from_static("nosniff"),
        ))
        .layer(SetResponseHeaderLayer::overriding(
            axum::http::header::HeaderName::from_static("x-frame-options"),
            axum::http::HeaderValue::from_static("DENY"),
        ))
        .layer(SetResponseHeaderLayer::overriding(
            axum::http::header::HeaderName::from_static("content-security-policy"),
            axum::http::HeaderValue::from_static("default-src 'self'; script-src 'self'; style-src 'self' 'unsafe-inline' https://fonts.googleapis.com https://fontsapi.zeoseven.com; img-src 'self' data: https://www.gravatar.com; connect-src 'self'; font-src 'self' https://fonts.gstatic.com https://fontsapi.zeoseven.com; frame-ancestors 'none'"),
        ))
}

fn build_root_api_router(metrics_path: &str) -> Router<AppState> {
    Router::new()
        .route("/v1/models", get(crate::handlers::list_models))
        .route("/v1/responses", post(crate::handlers::create_response))
        .route(
            "/v1/chat/completions",
            post(crate::handlers::create_chat_completions),
        )
        .route("/v1/embeddings", post(crate::handlers::create_embeddings))
        .route("/v1/messages", post(crate::handlers::create_messages))
        .route(metrics_path, get(crate::handlers::metrics))
        .route(
            "/presets/providers",
            get(crate::dashboard_handlers::get_provider_presets),
        )
        .route(
            "/presets/apikeys",
            get(crate::dashboard_handlers::get_apikey_presets),
        )
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
            "/dashboard/auth/logout",
            post(crate::dashboard_handlers::logout),
        )
        .route("/dashboard/auth/me", get(crate::dashboard_handlers::get_me))
        .route(
            "/dashboard/auth/me",
            put(crate::dashboard_handlers::update_me),
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
            "/dashboard/analytics",
            get(crate::dashboard_handlers::get_dashboard_analytics),
        )
}
