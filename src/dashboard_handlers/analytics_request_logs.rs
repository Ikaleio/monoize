use crate::app::AppState;
use crate::dashboard_handlers::session_helpers::get_current_user;
use crate::error::{AppError, AppResult};
use axum::Json;
use axum::extract::State;
use axum::http::{HeaderMap, StatusCode};
use axum::response::sse::{Event, KeepAlive};
use axum::response::{IntoResponse, Sse};
use chrono::NaiveTime;
use chrono::Utc;
use dashmap::DashMap;
use futures_util::stream;
use serde::Deserialize;
use serde_json::{Value, json};
use std::convert::Infallible;
use std::sync::Arc;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::time::Duration;
use tokio::sync::broadcast::error::RecvError;

#[derive(Debug, Deserialize)]
pub struct RequestLogsQuery {
    #[serde(default = "default_logs_limit")]
    pub limit: i64,
    #[serde(default)]
    pub offset: i64,
    #[serde(default)]
    pub model: Option<String>,
    #[serde(default)]
    pub status: Option<String>,
    #[serde(default)]
    pub api_key_id: Option<String>,
    #[serde(default)]
    pub username: Option<String>,
    #[serde(default)]
    pub search: Option<String>,
    #[serde(default)]
    pub time_from: Option<String>,
    #[serde(default)]
    pub time_to: Option<String>,
}

fn default_logs_limit() -> i64 {
    50
}

pub async fn list_my_request_logs(
    State(state): State<AppState>,
    headers: HeaderMap,
    axum::extract::Query(query): axum::extract::Query<RequestLogsQuery>,
) -> AppResult<impl IntoResponse> {
    let user = get_current_user(&headers, &state).await?;
    let limit = query.limit.clamp(1, 200);
    let offset = query.offset.max(0);
    let (mut logs, total, total_charge_nano_usd) = if user.role.can_manage_users() {
        state
            .user_store
            .list_all_request_logs(
                limit,
                offset,
                query.model.as_deref(),
                query.status.as_deref(),
                query.api_key_id.as_deref(),
                query.username.as_deref(),
                query.search.as_deref(),
                query.time_from.as_deref(),
                query.time_to.as_deref(),
            )
            .await
    } else {
        state
            .user_store
            .list_request_logs_by_user(
                &user.id,
                limit,
                offset,
                query.model.as_deref(),
                query.status.as_deref(),
                query.api_key_id.as_deref(),
                query.search.as_deref(),
                query.time_from.as_deref(),
                query.time_to.as_deref(),
            )
            .await
    }
    .map_err(|e| AppError::new(StatusCode::INTERNAL_SERVER_ERROR, "internal_error", e))?;

    for log in &mut logs {
        if let Some(ref id) = log.provider.id {
            log.provider.name = state.name_caches.get_provider_name(id);
        }
        if let Some(ref id) = log.channel.id {
            log.channel.name = state.name_caches.get_channel_name(id);
        }
        log.user.username = state.name_caches.get_username(&log.user.id);
        if let Some(ref id) = log.api_key.id {
            log.api_key.name = state.name_caches.get_api_key_name(id);
        }
    }

    Ok(Json(json!({
        "data": logs,
        "total": total,
        "total_charge_nano_usd": total_charge_nano_usd,
        "limit": limit,
        "offset": offset,
    })))
}

#[derive(Debug, Deserialize)]
pub struct AnalyticsQuery {
    #[serde(default = "default_analytics_buckets")]
    pub buckets: i64,
    #[serde(default = "default_analytics_range_hours")]
    pub range_hours: i64,
}

fn default_analytics_buckets() -> i64 {
    8
}

fn default_analytics_range_hours() -> i64 {
    24
}

pub async fn get_dashboard_analytics(
    State(state): State<AppState>,
    headers: HeaderMap,
    axum::extract::Query(query): axum::extract::Query<AnalyticsQuery>,
) -> AppResult<impl IntoResponse> {
    let user = get_current_user(&headers, &state).await?;
    let buckets = query.buckets.clamp(1, 48);
    let range_hours = query.range_hours.clamp(1, 720);

    let now = Utc::now();
    let time_to = now.to_rfc3339();
    let time_from = (now - chrono::Duration::hours(range_hours)).to_rfc3339();
    let today_start = now
        .date_naive()
        .and_time(NaiveTime::MIN)
        .and_utc()
        .to_rfc3339();

    let bucket_width_days = (range_hours as f64) / (buckets as f64) / 24.0;

    let user_id_filter: Option<String> = if user.role.can_manage_users() {
        None
    } else {
        Some(user.id.clone())
    };

    let raw = state
        .user_store
        .get_dashboard_analytics(
            user_id_filter.as_deref(),
            &time_from,
            &time_to,
            &today_start,
            buckets,
            bucket_width_days,
        )
        .await
        .map_err(|e| AppError::new(StatusCode::INTERNAL_SERVER_ERROR, "internal_error", e))?;

    let range_ms = (range_hours as f64) * 3600.0 * 1000.0;
    let time_from_ms = now.timestamp_millis() as f64 - range_ms;
    let bucket_width_ms = range_ms / (buckets as f64);

    let mut bucket_labels: Vec<String> = Vec::with_capacity(buckets as usize);
    for i in 0..buckets {
        let ms = time_from_ms + (i as f64) * bucket_width_ms;
        let secs = (ms / 1000.0) as i64;
        let dt = chrono::DateTime::from_timestamp(secs, 0).unwrap_or(now);
        let label = dt.format("%m-%d %H:00").to_string();
        bucket_labels.push(label);
    }

    let mut cost_by_model_buckets: Vec<serde_json::Map<String, Value>> =
        (0..buckets).map(|_| serde_json::Map::new()).collect();
    let mut calls_by_model_buckets: Vec<serde_json::Map<String, Value>> =
        (0..buckets).map(|_| serde_json::Map::new()).collect();
    let mut calls_by_provider_buckets: Vec<serde_json::Map<String, Value>> =
        (0..buckets).map(|_| serde_json::Map::new()).collect();

    for row in &raw.model_buckets {
        let idx = row.bucket_idx.clamp(0, buckets - 1) as usize;
        *cost_by_model_buckets[idx]
            .entry(&row.model)
            .or_insert(json!(0)) = json!(
            cost_by_model_buckets[idx]
                .get(&row.model)
                .and_then(|v| v.as_i64())
                .unwrap_or(0)
                + row.cost_nano
        );
        *calls_by_model_buckets[idx]
            .entry(&row.model)
            .or_insert(json!(0)) = json!(
            calls_by_model_buckets[idx]
                .get(&row.model)
                .and_then(|v| v.as_i64())
                .unwrap_or(0)
                + row.call_count
        );
    }

    for row in &raw.provider_buckets {
        let idx = row.bucket_idx.clamp(0, buckets - 1) as usize;
        *calls_by_provider_buckets[idx]
            .entry(&row.provider_label)
            .or_insert(json!(0)) = json!(
            calls_by_provider_buckets[idx]
                .get(&row.provider_label)
                .and_then(|v| v.as_i64())
                .unwrap_or(0)
                + row.call_count
        );
    }

    let response_buckets: Vec<Value> = (0..buckets as usize)
        .map(|i| {
            json!({
                "label": bucket_labels[i],
                "cost_by_model": cost_by_model_buckets[i],
                "calls_by_model": calls_by_model_buckets[i],
                "calls_by_provider": calls_by_provider_buckets[i],
            })
        })
        .collect();

    Ok(Json(json!({
        "buckets": response_buckets,
        "time_from": time_from,
        "time_to": time_to,
        "total_cost_nano_usd": raw.total_cost_nano_usd,
        "total_calls": raw.total_calls,
        "today_cost_nano_usd": raw.today_cost_nano_usd,
        "today_calls": raw.today_calls,
    })))
}

/// Guard that decrements the per-user SSE connection counter on drop,
/// ensuring no counter leaks even if the stream is abruptly cancelled.
struct SseConnectionGuard {
    user_id: String,
    connections: Arc<DashMap<String, AtomicUsize>>,
}

impl Drop for SseConnectionGuard {
    fn drop(&mut self) {
        if let Some(entry) = self.connections.get(&self.user_id) {
            let prev = entry.value().fetch_sub(1, Ordering::Relaxed);
            if prev <= 1 {
                drop(entry);
                self.connections.remove(&self.user_id);
            }
        }
    }
}

const MAX_SSE_CONNECTIONS_PER_USER: usize = 5;

pub async fn stream_request_logs(
    State(state): State<AppState>,
    headers: HeaderMap,
) -> AppResult<impl IntoResponse> {
    let user = get_current_user(&headers, &state).await?;
    let is_admin = user.role.can_manage_users();
    let user_id = user.id;

    // Enforce per-user SSE connection limit
    let entry = state
        .sse_connections
        .entry(user_id.clone())
        .or_insert_with(|| AtomicUsize::new(0));
    let current = entry.value().fetch_add(1, Ordering::Relaxed);
    drop(entry);
    if current >= MAX_SSE_CONNECTIONS_PER_USER {
        // Undo the speculative increment
        if let Some(e) = state.sse_connections.get(&user_id) {
            e.value().fetch_sub(1, Ordering::Relaxed);
        }
        return Err(AppError::new(
            StatusCode::TOO_MANY_REQUESTS,
            "too_many_sse_connections",
            "Too many concurrent SSE connections",
        ));
    }

    let guard = SseConnectionGuard {
        user_id: user_id.clone(),
        connections: state.sse_connections.clone(),
    };

    let name_caches = state.name_caches.clone();
    let api_key_cache = state.user_store.api_key_cache.clone();
    let receiver = state.log_broadcast.subscribe();
    let stream = stream::unfold(
        (
            receiver,
            name_caches,
            api_key_cache,
            is_admin,
            user_id,
            guard,
        ),
        |(mut receiver, name_caches, api_key_cache, is_admin, user_id, guard)| async move {
            loop {
                match receiver.recv().await {
                    Ok(batch) => {
                        let filtered: Vec<_> = if is_admin {
                            batch
                        } else {
                            batch
                                .into_iter()
                                .filter(|log| log.user_id == user_id)
                                .collect()
                        };
                        if filtered.is_empty() {
                            continue;
                        }
                        let enriched_batch: Vec<_> = filtered
                            .iter()
                            .map(|log| name_caches.enrich_log(log, &api_key_cache))
                            .collect();
                        let event = match serde_json::to_string(&enriched_batch) {
                            Ok(payload) => Event::default().event("log_batch").data(payload),
                            Err(_) => Event::default().event("resync").data("{}"),
                        };
                        return Some((
                            Ok::<Event, Infallible>(event),
                            (
                                receiver,
                                name_caches,
                                api_key_cache,
                                is_admin,
                                user_id,
                                guard,
                            ),
                        ));
                    }
                    Err(RecvError::Lagged(_)) => {
                        let event = Event::default().event("resync").data("{}");
                        return Some((
                            Ok::<Event, Infallible>(event),
                            (
                                receiver,
                                name_caches,
                                api_key_cache,
                                is_admin,
                                user_id,
                                guard,
                            ),
                        ));
                    }
                    Err(RecvError::Closed) => return None,
                }
            }
        },
    );

    Ok(Sse::new(stream)
        .keep_alive(KeepAlive::new().interval(Duration::from_secs(15)))
        .into_response())
}
