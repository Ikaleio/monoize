use crate::app::AppState;
use crate::dashboard_handlers::session_helpers::get_current_user;
use crate::error::{AppError, AppResult};
use crate::users::PerformanceTargetRaw;
use axum::Json;
use axum::extract::State;
use axum::http::{HeaderMap, StatusCode};
use axum::response::IntoResponse;
use chrono::{Duration, SecondsFormat, Utc};
use serde_json::{Value, json};
use std::collections::{HashMap, HashSet};

const BRICK_COUNT: i64 = 24;
const WINDOW_HOURS: i64 = 24;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum BrickStatus {
    Empty,
    Up,
    Degraded,
    Down,
}

impl BrickStatus {
    fn as_str(self) -> &'static str {
        match self {
            Self::Empty => "empty",
            Self::Up => "up",
            Self::Degraded => "degraded",
            Self::Down => "down",
        }
    }
}

fn brick_status(finished_count: i64, success_count: i64) -> BrickStatus {
    if finished_count <= 0 {
        return BrickStatus::Empty;
    }
    let rate = (success_count as f64) / (finished_count as f64);
    if rate >= 0.99 {
        BrickStatus::Up
    } else if rate >= 0.95 {
        BrickStatus::Degraded
    } else {
        BrickStatus::Down
    }
}

fn build_bricks(raw: &PerformanceTargetRaw, brick_count: i64) -> Vec<Value> {
    let mut finished = vec![0i64; brick_count as usize];
    let mut success = vec![0i64; brick_count as usize];
    for row in &raw.hour_buckets {
        let idx = row.hour_idx.clamp(0, brick_count - 1) as usize;
        finished[idx] = finished[idx].saturating_add(row.finished_count);
        success[idx] = success[idx].saturating_add(row.success_count);
    }
    (0..brick_count as usize)
        .map(|i| {
            json!({
                "index": i,
                "status": brick_status(finished[i], success[i]).as_str(),
            })
        })
        .collect()
}

fn target_json(
    id: &str,
    name: Option<&str>,
    raw: &PerformanceTargetRaw,
    brick_count: i64,
) -> Value {
    let mut obj = serde_json::Map::new();
    obj.insert("id".to_string(), Value::String(id.to_string()));
    if let Some(name) = name {
        obj.insert("name".to_string(), Value::String(name.to_string()));
    }
    obj.insert(
        "bricks".to_string(),
        Value::Array(build_bricks(raw, brick_count)),
    );
    obj.insert(
        "avg_ttft_ms".to_string(),
        raw.avg_ttft_ms.map(Value::from).unwrap_or(Value::Null),
    );
    obj.insert(
        "avg_tps".to_string(),
        raw.avg_tps.map(Value::from).unwrap_or(Value::Null),
    );
    Value::Object(obj)
}

pub async fn get_dashboard_performance(
    State(state): State<AppState>,
    headers: HeaderMap,
) -> AppResult<impl IntoResponse> {
    let _user = get_current_user(&headers, &state).await?;

    let settings = state
        .settings_store
        .get_all()
        .await
        .map_err(|e| AppError::new(StatusCode::INTERNAL_SERVER_ERROR, "internal_error", e))?;

    let configured_group_ids = settings.dashboard_performance_group_ids;
    let configured_model_ids = settings.dashboard_performance_model_ids;
    let now = Utc::now();
    let time_from = now - Duration::hours(WINDOW_HOURS);
    let time_to_unix_ms = now.timestamp_millis();
    let time_from_unix_ms = time_from.timestamp_millis();
    let time_from_rfc3339 = time_from.to_rfc3339_opts(SecondsFormat::Millis, true);
    let time_to_rfc3339 = now.to_rfc3339_opts(SecondsFormat::Millis, true);

    if configured_group_ids.is_empty() && configured_model_ids.is_empty() {
        return Ok(Json(json!({
            "groups": [],
            "models": [],
            "brick_count": BRICK_COUNT,
            "window_hours": WINDOW_HOURS,
            "time_from": time_from_rfc3339,
            "time_to": time_to_rfc3339,
        })));
    }

    let registry_groups = state
        .user_store
        .list_groups()
        .await
        .map_err(|e| AppError::new(StatusCode::INTERNAL_SERVER_ERROR, "internal_error", e))?;
    let group_name_by_id: HashMap<String, String> = registry_groups
        .into_iter()
        .map(|g| (g.id, g.name))
        .collect();

    let providers = state
        .monoize_store
        .list_providers()
        .await
        .map_err(|e| AppError::new(StatusCode::INTERNAL_SERVER_ERROR, "internal_error", e))?;

    let mut provider_ids_by_group: HashMap<String, Vec<String>> = HashMap::new();
    for provider in &providers {
        for group_id in &provider.group_ids {
            provider_ids_by_group
                .entry(group_id.clone())
                .or_default()
                .push(provider.id.clone());
        }
    }

    let mut groups_json = Vec::new();
    let mut seen_groups = HashSet::new();
    for group_id in &configured_group_ids {
        if !seen_groups.insert(group_id.clone()) {
            continue;
        }
        let Some(name) = group_name_by_id.get(group_id) else {
            continue;
        };
        let provider_ids = provider_ids_by_group
            .get(group_id)
            .map(Vec::as_slice)
            .unwrap_or(&[]);
        let raw = state
            .user_store
            .get_performance_target_stats(
                time_from_unix_ms,
                time_to_unix_ms,
                BRICK_COUNT,
                Some(provider_ids),
                None,
            )
            .await
            .map_err(|e| AppError::new(StatusCode::INTERNAL_SERVER_ERROR, "internal_error", e))?;
        groups_json.push(target_json(
            group_id,
            Some(name.as_str()),
            &raw,
            BRICK_COUNT,
        ));
    }

    let mut models_json = Vec::new();
    for model_id in &configured_model_ids {
        let raw = state
            .user_store
            .get_performance_target_stats(
                time_from_unix_ms,
                time_to_unix_ms,
                BRICK_COUNT,
                None,
                Some(model_id.as_str()),
            )
            .await
            .map_err(|e| AppError::new(StatusCode::INTERNAL_SERVER_ERROR, "internal_error", e))?;
        models_json.push(target_json(model_id, None, &raw, BRICK_COUNT));
    }

    Ok(Json(json!({
        "groups": groups_json,
        "models": models_json,
        "brick_count": BRICK_COUNT,
        "window_hours": WINDOW_HOURS,
        "time_from": time_from_rfc3339,
        "time_to": time_to_rfc3339,
    })))
}

#[cfg(test)]
mod tests {
    use super::{BrickStatus, brick_status};

    #[test]
    fn brick_status_thresholds_match_dh9b() {
        assert_eq!(brick_status(0, 0), BrickStatus::Empty);
        assert_eq!(brick_status(100, 99), BrickStatus::Up);
        assert_eq!(brick_status(100, 98), BrickStatus::Degraded);
        assert_eq!(brick_status(100, 95), BrickStatus::Degraded);
        assert_eq!(brick_status(100, 94), BrickStatus::Down);
        assert_eq!(brick_status(1, 1), BrickStatus::Up);
        assert_eq!(brick_status(1, 0), BrickStatus::Down);
    }
}
