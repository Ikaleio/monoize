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
use futures_util::{StreamExt, stream};
use serde::Deserialize;
use serde_json::{Value, json};
use std::collections::BTreeMap;
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

fn validate_request_log_model_filter(query: &RequestLogsQuery) -> AppResult<()> {
    crate::users::UserStore::validate_request_log_model_filter(query.model.as_deref()).map_err(
        |message| {
            AppError::new(
                StatusCode::BAD_REQUEST,
                "request_log_model_filter_too_many_terms",
                message,
            )
            .with_param("model")
        },
    )
}

fn validate_request_log_time_filters(query: &RequestLogsQuery) -> AppResult<()> {
    let parse = |name: &'static str, value: Option<&str>| {
        value
            .map(|value| {
                chrono::DateTime::parse_from_rfc3339(value).map_err(|_| {
                    AppError::new(
                        StatusCode::BAD_REQUEST,
                        "invalid_time_filter",
                        format!("{name} must be an RFC 3339 timestamp"),
                    )
                    .with_param(name)
                })
            })
            .transpose()
    };
    let time_from = parse("time_from", query.time_from.as_deref())?;
    let time_to = parse("time_to", query.time_to.as_deref())?;
    if time_from.zip(time_to).is_some_and(|(from, to)| from >= to) {
        return Err(AppError::new(
            StatusCode::BAD_REQUEST,
            "invalid_time_filter",
            "time_from must be earlier than time_to",
        )
        .with_param("time_from"));
    }
    Ok(())
}

#[cfg(test)]
mod request_log_query_tests {
    use super::{
        RequestLogsQuery, SseConnectionGuard, validate_request_log_model_filter,
        validate_request_log_time_filters,
    };
    use dashmap::DashMap;
    use std::sync::Arc;
    use std::sync::atomic::{AtomicUsize, Ordering};

    fn query(time_from: Option<&str>, time_to: Option<&str>) -> RequestLogsQuery {
        RequestLogsQuery {
            limit: 50,
            offset: 0,
            model: None,
            status: None,
            api_key_id: None,
            username: None,
            search: None,
            time_from: time_from.map(str::to_string),
            time_to: time_to.map(str::to_string),
        }
    }

    #[test]
    fn malformed_and_reversed_time_filters_are_bad_requests() {
        let malformed = validate_request_log_time_filters(&query(Some("bad"), None)).unwrap_err();
        assert_eq!(malformed.status, axum::http::StatusCode::BAD_REQUEST);
        assert_eq!(malformed.code, "invalid_time_filter");
        assert_eq!(malformed.param.as_deref(), Some("time_from"));

        let reversed = validate_request_log_time_filters(&query(
            Some("2024-01-02T00:00:00Z"),
            Some("2024-01-01T00:00:00Z"),
        ))
        .unwrap_err();
        assert_eq!(reversed.status, axum::http::StatusCode::BAD_REQUEST);
        assert_eq!(reversed.param.as_deref(), Some("time_from"));
    }

    #[test]
    fn over_limit_model_filter_is_a_bad_request() {
        let mut query = query(None, None);
        query.model = Some(
            (0..33)
                .map(|term| format!("model-{term}"))
                .collect::<Vec<_>>()
                .join(","),
        );
        let error = validate_request_log_model_filter(&query).unwrap_err();
        assert_eq!(error.status, axum::http::StatusCode::BAD_REQUEST);
        assert_eq!(error.code, "request_log_model_filter_too_many_terms");
        assert_eq!(error.param.as_deref(), Some("model"));
    }

    #[test]
    fn sse_counter_entry_is_removed_without_decrementing_a_replacement() {
        let connections = Arc::new(DashMap::new());
        let counter = Arc::new(AtomicUsize::new(1));
        connections.insert("user-1".to_string(), counter.clone());
        let replacement = Arc::new(AtomicUsize::new(1));
        connections.insert("user-1".to_string(), replacement.clone());
        drop(SseConnectionGuard {
            user_id: "user-1".to_string(),
            connections: connections.clone(),
            counter,
        });
        let current = connections.get("user-1").expect("replacement remains");
        assert!(Arc::ptr_eq(current.value(), &replacement));
        assert_eq!(current.value().load(Ordering::Relaxed), 1);
    }
}

pub async fn list_my_request_logs(
    State(state): State<AppState>,
    headers: HeaderMap,
    axum::extract::Query(query): axum::extract::Query<RequestLogsQuery>,
) -> AppResult<impl IntoResponse> {
    validate_request_log_model_filter(&query)?;
    let user = get_current_user(&headers, &state).await?;
    validate_request_log_time_filters(&query)?;
    let is_admin = user.role.can_manage_users();
    let limit = query.limit.clamp(1, 200);
    let offset = query.offset.max(0);
    let (mut logs, total, total_charge_nano_usd) = if is_admin {
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

    // RL-API14 / SAN-14: only admins may read the stored full error detail.
    // SAN-CFG5 item 5: skipped entirely when masking is disabled.
    if !is_admin && state.monoize_runtime.read().await.mask_sensitive_info {
        for log in &mut logs {
            log.mask_error_detail_for_non_admin();
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

    let mut cost_by_model_buckets: Vec<BTreeMap<String, i128>> =
        (0..buckets).map(|_| BTreeMap::new()).collect();
    let mut calls_by_model_buckets: Vec<BTreeMap<String, i64>> =
        (0..buckets).map(|_| BTreeMap::new()).collect();
    let mut calls_by_provider_buckets: Vec<BTreeMap<String, i64>> =
        (0..buckets).map(|_| BTreeMap::new()).collect();

    for row in &raw.model_buckets {
        let idx = row.bucket_idx.clamp(0, buckets - 1) as usize;
        let cost = cost_by_model_buckets[idx]
            .entry(row.model.clone())
            .or_insert(0);
        *cost = cost.checked_add(row.cost_nano).ok_or_else(|| {
            AppError::new(
                StatusCode::INTERNAL_SERVER_ERROR,
                "internal_error",
                "analytics cost aggregate overflow",
            )
        })?;
        let calls = calls_by_model_buckets[idx]
            .entry(row.model.clone())
            .or_insert(0);
        *calls = calls.checked_add(row.call_count).ok_or_else(|| {
            AppError::new(
                StatusCode::INTERNAL_SERVER_ERROR,
                "internal_error",
                "analytics call count overflow",
            )
        })?;
    }

    for row in &raw.provider_buckets {
        let idx = row.bucket_idx.clamp(0, buckets - 1) as usize;
        let calls = calls_by_provider_buckets[idx]
            .entry(row.provider_label.clone())
            .or_insert(0);
        *calls = calls.checked_add(row.call_count).ok_or_else(|| {
            AppError::new(
                StatusCode::INTERNAL_SERVER_ERROR,
                "internal_error",
                "analytics call count overflow",
            )
        })?;
    }

    let response_buckets: Vec<Value> = (0..buckets as usize)
        .map(|i| {
            json!({
                "label": bucket_labels[i],
                "cost_by_model": cost_by_model_buckets[i]
                    .iter()
                    .map(|(model, cost)| (model.clone(), Value::String(cost.to_string())))
                    .collect::<serde_json::Map<String, Value>>(),
                "calls_by_model": calls_by_model_buckets[i],
                "calls_by_provider": calls_by_provider_buckets[i],
            })
        })
        .collect();

    Ok(Json(json!({
        "buckets": response_buckets,
        "time_from": time_from,
        "time_to": time_to,
        "total_cost_nano_usd": raw.total_cost_nano_usd.to_string(),
        "total_calls": raw.total_calls,
        "today_cost_nano_usd": raw.today_cost_nano_usd.to_string(),
        "today_calls": raw.today_calls,
    })))
}

/// Guard that decrements the per-user SSE connection counter on drop,
/// ensuring no counter leaks even if the stream is abruptly cancelled.
struct SseConnectionGuard {
    user_id: String,
    connections: Arc<DashMap<String, Arc<AtomicUsize>>>,
    counter: Arc<AtomicUsize>,
}

impl Drop for SseConnectionGuard {
    fn drop(&mut self) {
        if self.counter.fetch_sub(1, Ordering::AcqRel) == 1 {
            self.connections.remove_if(&self.user_id, |_, current| {
                Arc::ptr_eq(current, &self.counter) && current.load(Ordering::Acquire) == 0
            });
        }
    }
}

fn max_sse_connections_per_user() -> usize {
    std::env::var("MONOIZE_REQUEST_LOG_SSE_MAX_CONNECTIONS_PER_USER")
        .ok()
        .and_then(|value| value.parse::<usize>().ok())
        .filter(|value| *value > 0)
        .unwrap_or(5)
}

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
        .or_insert_with(|| Arc::new(AtomicUsize::new(0)));
    let counter = entry.value().clone();
    let current = counter.fetch_add(1, Ordering::AcqRel);
    drop(entry);
    if current >= max_sse_connections_per_user() {
        if counter.fetch_sub(1, Ordering::AcqRel) == 1 {
            state.sse_connections.remove_if(&user_id, |_, current| {
                Arc::ptr_eq(current, &counter) && current.load(Ordering::Acquire) == 0
            });
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
        counter,
    };

    let receiver = state.log_broadcast.subscribe();
    let runtime = state.monoize_runtime.clone();

    let mut initial_pending: Vec<_> = state
        .pending_request_logs
        .iter()
        .filter(|entry| is_admin || entry.value().user_id == user_id)
        .map(|entry| entry.value().clone())
        .collect();
    initial_pending.sort_by(|a, b| b.created_at.cmp(&a.created_at));
    let initial_event = if initial_pending.is_empty() {
        Event::default().comment("ready")
    } else {
        // RL-API14 / SAN-14: non-admin SSE rows carry masked detail unless
        // masking is disabled (SAN-CFG5 item 5).
        let mask_batch = !is_admin && runtime.read().await.mask_sensitive_info;
        let enriched_batch: Vec<_> = initial_pending
            .into_iter()
            .map(|log| {
                let mut row = log.to_request_log_row();
                if mask_batch {
                    row.mask_error_detail_for_non_admin();
                }
                row
            })
            .collect();
        match serde_json::to_string(&enriched_batch) {
            Ok(payload) => Event::default().event("log_batch").data(payload),
            Err(_) => Event::default().event("resync").data("{}"),
        }
    };

    let live_stream = stream::unfold(
        (receiver, is_admin, user_id, guard, runtime),
        |(mut receiver, is_admin, user_id, guard, runtime)| async move {
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
                        // RL-API14 / SAN-14 with the SAN-CFG5 item 5 gate,
                        // re-read per batch so a settings change applies to
                        // subsequent frames of an open connection.
                        let mask_batch = !is_admin && runtime.read().await.mask_sensitive_info;
                        let enriched_batch: Vec<_> = filtered
                            .into_iter()
                            .map(|log| {
                                let mut row = log.to_request_log_row();
                                if mask_batch {
                                    row.mask_error_detail_for_non_admin();
                                }
                                row
                            })
                            .collect();
                        let event = match serde_json::to_string(&enriched_batch) {
                            Ok(payload) => Event::default().event("log_batch").data(payload),
                            Err(_) => Event::default().event("resync").data("{}"),
                        };
                        return Some((
                            Ok::<Event, Infallible>(event),
                            (receiver, is_admin, user_id, guard, runtime),
                        ));
                    }
                    Err(RecvError::Lagged(_)) => {
                        let event = Event::default().event("resync").data("{}");
                        return Some((
                            Ok::<Event, Infallible>(event),
                            (receiver, is_admin, user_id, guard, runtime),
                        ));
                    }
                    Err(RecvError::Closed) => return None,
                }
            }
        },
    );
    let stream =
        stream::once(async move { Ok::<Event, Infallible>(initial_event) }).chain(live_stream);

    Ok(Sse::new(stream)
        .keep_alive(KeepAlive::new().interval(Duration::from_secs(15)))
        .into_response())
}
