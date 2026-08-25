use crate::app::AppState;
use crate::dashboard_handlers::session_helpers::require_admin;
use crate::error::{AppError, AppResult};
use crate::handlers::routing::health_key;
use axum::Json;
use axum::extract::State;
use axum::http::{HeaderMap, StatusCode};
use chrono::{NaiveTime, Utc};
use serde_json::{Value, json};
use std::collections::HashMap;

/// security-access-control.spec.md SAC-1..SAC-5: metrics expose runtime topology
/// and therefore use the same authorization boundary as the admin dashboard.
pub async fn get_metrics(
    State(state): State<AppState>,
    headers: HeaderMap,
) -> AppResult<impl axum::response::IntoResponse> {
    require_admin(&headers, &state).await?;
    Ok(state.metrics.render())
}

/// admin-dashboard.spec.md AD-1..AD-5: one admin-only aggregate snapshot of
/// node/system status, replica state, user usage ranking, and channel health.
pub async fn get_admin_overview(
    State(state): State<AppState>,
    headers: HeaderMap,
) -> AppResult<Json<Value>> {
    require_admin(&headers, &state).await?;

    let now = chrono::Utc::now();
    let started_at = state.started_at;
    let uptime_seconds = now
        .signed_duration_since(started_at)
        .to_std()
        .map(|duration| duration.as_secs())
        .unwrap_or(0);

    let database_backend = if state.db_pool.is_sqlite() {
        "sqlite"
    } else if state.db_pool.is_postgres() {
        "postgres"
    } else {
        "unknown"
    };
    let dsn_redacted = redact_dsn(&state.runtime.database_dsn);
    let role = state.node.role.as_str();

    let (spool_pending_count, spool_pending_bytes) = match (role, state.metering.as_ref()) {
        ("replica", Some(metering)) => {
            let (count, bytes) = metering.delta_spool().pending_stats();
            (count, bytes)
        }
        _ => (0usize, 0u64),
    };

    let sse_connections: usize = state
        .sse_connections
        .iter()
        .map(|entry| entry.value().load(std::sync::atomic::Ordering::Relaxed))
        .sum::<usize>()
        .min(usize::MAX);

    let ranking_window_from = (now - chrono::Duration::hours(24)).to_rfc3339();
    let ranking = state
        .user_store
        .get_users_usage_ranking(&ranking_window_from, &now.to_rfc3339(), 20)
        .await
        .map_err(|e| AppError::new(StatusCode::INTERNAL_SERVER_ERROR, "internal_error", e))?;
    let users_ranking: Vec<Value> = ranking
        .into_iter()
        .map(|row| {
            json!({
                "user_id": row.user_id,
                "username": row.username,
                "call_count": row.call_count,
                "cost_nano_usd": row.cost_nano_usd.to_string(),
            })
        })
        .collect();

    let today_start = Utc::now()
        .date_naive()
        .and_time(NaiveTime::MIN)
        .and_utc()
        .to_rfc3339();
    let (today_calls, today_cost_nano_usd) = state
        .user_store
        .get_today_usage_totals(&today_start)
        .await
        .map_err(|e| AppError::new(StatusCode::INTERNAL_SERVER_ERROR, "internal_error", e))?;
    let channel_today: HashMap<String, crate::users::ChannelTodayUsage> = state
        .user_store
        .get_channels_today_usage(&today_start)
        .await
        .map_err(|e| AppError::new(StatusCode::INTERNAL_SERVER_ERROR, "internal_error", e))?
        .into_iter()
        .map(|row| (row.channel_id.clone(), row))
        .collect();

    let providers = state
        .monoize_store
        .list_providers()
        .await
        .map_err(|e| AppError::new(StatusCode::INTERNAL_SERVER_ERROR, "internal_error", e))?;
    let health = state.channel_health.lock().await;
    let now_ms = crate::handlers::routing::now_ts();
    let mut channel_health: Vec<Value> = Vec::new();
    for provider in providers {
        for channel in provider.channels {
            let base_entry = health.get(&channel.id);
            let mut healthy = base_entry.map(|entry| entry.healthy).unwrap_or(true);
            let mut cooldown_until = base_entry.and_then(|entry| entry.cooldown_until);
            let mut last_success_at = base_entry.and_then(|entry| entry.last_success_at);
            let mut last_probe_at = base_entry.and_then(|entry| entry.last_probe_at);
            let mut probe_success_count = base_entry
                .map(|entry| entry.probe_success_count)
                .unwrap_or(0);
            let mut unhealthy_models: Vec<String> = Vec::new();
            if provider.per_model_circuit_break {
                for model in channel.models.keys() {
                    let Some(entry) = health.get(&health_key(&channel.id, Some(model))) else {
                        continue;
                    };
                    healthy &= entry.healthy;
                    if entry.cooldown_until.is_some_and(|until| until > now_ms) || !entry.healthy {
                        unhealthy_models.push(model.clone());
                    }
                    cooldown_until = match (cooldown_until, entry.cooldown_until) {
                        (Some(left), Some(right)) => Some(left.max(right)),
                        (Some(value), None) | (None, Some(value)) => Some(value),
                        (None, None) => None,
                    };
                    last_success_at = match (last_success_at, entry.last_success_at) {
                        (Some(left), Some(right)) => Some(left.max(right)),
                        (Some(value), None) | (None, Some(value)) => Some(value),
                        (None, None) => None,
                    };
                    last_probe_at = match (last_probe_at, entry.last_probe_at) {
                        (Some(left), Some(right)) => Some(left.max(right)),
                        (Some(value), None) | (None, Some(value)) => Some(value),
                        (None, None) => None,
                    };
                    probe_success_count = probe_success_count.max(entry.probe_success_count);
                }
                unhealthy_models.sort();
            }
            let today = channel_today.get(&channel.id);
            channel_health.push(json!({
                "provider_id": provider.id,
                "provider_name": provider.name,
                "channel_id": channel.id,
                "channel_name": channel.name,
                "enabled": channel.enabled,
                "weight": channel.weight,
                "session_affinity_auto": channel.session_affinity_auto.unwrap_or(false),
                "healthy": healthy,
                "last_success_at": last_success_at,
                "cooldown_until": cooldown_until,
                "probe_success_count": probe_success_count,
                "last_probe_at": last_probe_at,
                "cooldown_active": cooldown_until.is_some_and(|until| until > now_ms),
                "unhealthy_models": unhealthy_models,
                "today_calls": today.map(|row| row.today_calls).unwrap_or(0),
                "today_cost_nano_usd": today
                    .map(|row| row.today_cost_nano_usd.to_string())
                    .unwrap_or_else(|| "0".to_string()),
            }));
        }
    }
    drop(health);

    let stale_after_ms = (state.node.metering_ship_interval.as_millis() as i64)
        .saturating_mul(crate::replica::metering::HEARTBEAT_STALE_INTERVALS as i64);
    let now_unix_ms = now.timestamp_millis();
    crate::replica::metering::evict_expired_heartbeats(
        &state.replica_heartbeats,
        now_unix_ms,
        state.node.metering_ship_interval,
    );
    let mut replicas: Vec<Value> = state
        .replica_heartbeats
        .iter()
        .map(|entry| {
            let record = entry.value();
            let stale = now_unix_ms.saturating_sub(record.last_seen_unix_ms) > stale_after_ms;
            json!({
                "id": record.heartbeat.id,
                "hostname": record.heartbeat.hostname,
                "listen": record.heartbeat.listen,
                "version": record.heartbeat.version,
                "started_at": record.heartbeat.started_at,
                "last_seen_at": chrono::DateTime::<chrono::Utc>::from_timestamp_millis(record.last_seen_unix_ms)
                    .unwrap_or(now)
                    .to_rfc3339(),
                "uptime_seconds": record.heartbeat.uptime_seconds,
                "spool_pending_count": record.heartbeat.spool_pending_count,
                "spool_pending_bytes": record.heartbeat.spool_pending_bytes,
                "stale": stale,
            })
        })
        .collect();
    replicas.sort_by(|left, right| {
        let left_host = left.get("hostname").and_then(Value::as_str).unwrap_or("");
        let right_host = right.get("hostname").and_then(Value::as_str).unwrap_or("");
        left_host.cmp(right_host)
    });

    Ok(Json(json!({
        "node": {
            "role": role,
            "version": env!("CARGO_PKG_VERSION"),
            "started_at": started_at.to_rfc3339(),
            "uptime_seconds": uptime_seconds,
            "listen": state.runtime.listen,
            "metrics_path": state.runtime.metrics_path,
            "database_backend": database_backend,
            "database_dsn_redacted": dsn_redacted,
            "upstream_proxy_url": state.node.upstream_proxy_url,
        },
        "replica": {
            "ingest_enabled": state.metering_token_digest.is_some(),
            "spool_pending_count": spool_pending_count,
            "spool_pending_bytes": spool_pending_bytes,
            "replicas": replicas,
        },
        "system": {
            "pending_request_logs": state.pending_request_logs.len(),
            "sse_connections": sse_connections,
            "channel_health_entries": state.channel_health.lock().await.len(),
            "channel_affinity_entries": state.channel_affinity.lock().await.len(),
            "routing_config_revision": state.routing_config_revision
                .load(std::sync::atomic::Ordering::Relaxed)
                .to_string(),
        },
        "today": {
            "calls": today_calls,
            "cost_nano_usd": today_cost_nano_usd.to_string(),
        },
        "users_ranking": users_ranking,
        "channel_health": channel_health,
    })))
}

fn redact_dsn(dsn: &str) -> String {
    if let Some(at_pos) = dsn.find('@')
        && let Some(scheme_end) = dsn.find("://")
    {
        return format!("{}://***@{}", &dsn[..scheme_end], &dsn[at_pos + 1..]);
    }
    if dsn.starts_with("sqlite") {
        return dsn.to_string();
    }
    "***".to_string()
}
