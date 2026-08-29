//! Capture detail API (`request-capture-viewer.spec.md` section 2).

use crate::app::AppState;
use crate::dashboard_handlers::session_helpers::get_current_user;
use crate::error::{AppError, AppResult};
use crate::users::{User, UserRole};
use axum::Json;
use axum::extract::{Path, Query, State};
use axum::http::{HeaderMap, StatusCode};
use axum::response::IntoResponse;
use serde::Deserialize;
use serde_json::{Value, json};
use std::collections::HashMap;

#[derive(Debug, Deserialize)]
pub struct RequestCaptureQuery {
    #[serde(default)]
    pub user_id: Option<String>,
}

/// RCV-A7: denied and absent captures share one indistinguishable response.
fn capture_not_found() -> AppError {
    AppError::new(
        StatusCode::NOT_FOUND,
        "capture_not_found",
        "request capture not found",
    )
}

fn internal_error(message: String) -> AppError {
    AppError::new(StatusCode::INTERNAL_SERVER_ERROR, "internal_error", message)
}

/// RCV-A5/RCV-A6 record access rules.
fn caller_may_view(caller: &User, owner_id: &str, owner: Option<&User>) -> bool {
    if owner_id == caller.id {
        return true;
    }
    match caller.role {
        UserRole::User => false,
        UserRole::Admin | UserRole::SuperAdmin => {
            owner.is_some_and(|owner| owner.role == UserRole::User)
        }
    }
}

/// RCV-A10: `user`-role callers only see API-key-scope transform entries;
/// each attempt gains `hidden_transforms` counting the removed entries.
fn redact_transform_chains_for_user(dump: &mut Value) {
    let Some(attempts) = dump.get_mut("attempts").and_then(Value::as_array_mut) else {
        return;
    };
    for attempt in attempts {
        let Some(attempt_obj) = attempt.as_object_mut() else {
            continue;
        };
        let mut hidden = 0usize;
        if let Some(Value::Array(chain)) = attempt_obj.get_mut("transform_chain") {
            let before = chain.len();
            chain.retain(|entry| entry.get("scope").and_then(Value::as_str) == Some("api_key"));
            hidden = before - chain.len();
        }
        attempt_obj.insert("hidden_transforms".to_string(), json!(hidden));
    }
}

pub async fn get_request_capture(
    State(state): State<AppState>,
    headers: HeaderMap,
    Path(request_id): Path<String>,
    Query(query): Query<RequestCaptureQuery>,
) -> AppResult<impl IntoResponse> {
    let caller = get_current_user(&headers, &state).await?;
    let owner_filter = query
        .user_id
        .as_deref()
        .map(str::trim)
        .filter(|value| !value.is_empty());

    // RCV-A1: candidates ordered newest first by the store query.
    let records = state
        .request_capture
        .list_capture_records(&request_id, owner_filter)
        .await
        .map_err(internal_error)?;

    let mut owners: HashMap<String, Option<User>> = HashMap::new();
    for record in records {
        let owner = match owners.get(&record.user_id) {
            Some(owner) => owner.clone(),
            None => {
                let owner = state
                    .user_store
                    .get_user_by_id(&record.user_id)
                    .await
                    .map_err(internal_error)?;
                owners.insert(record.user_id.clone(), owner.clone());
                owner
            }
        };
        if !caller_may_view(&caller, &record.user_id, owner.as_ref()) {
            continue;
        }
        // RCV-A2/RCV-A3: serve the first authorized candidate, reading the
        // dump from disk only here. RCD-Z7/RCV-A9: a zstd-marked file that
        // cannot decompress within bounds is a distinguishable server error.
        let bytes = match state
            .request_capture
            .read_dump_file(&record.file_name)
            .await
            .map_err(|err| match err {
                crate::request_capture::DumpReadError::Unreadable(message) => AppError::new(
                    StatusCode::INTERNAL_SERVER_ERROR,
                    "capture_dump_unreadable",
                    message,
                ),
                crate::request_capture::DumpReadError::Io(message) => internal_error(message),
            })? {
            Some(bytes) => bytes,
            None => {
                // RCV-A8: the file is gone; drop the stale metadata row and
                // fall through to the next authorized candidate.
                if let Err(err) = state
                    .request_capture
                    .delete_capture_record(&record.file_name)
                    .await
                {
                    tracing::warn!("failed to delete stale capture record: {err}");
                }
                continue;
            }
        };
        let mut dump: Value = serde_json::from_slice(&bytes).map_err(|err| {
            // RCV-A9: an existing but unparseable dump is a server error, not
            // a not-found, so operators can distinguish corruption.
            AppError::new(
                StatusCode::INTERNAL_SERVER_ERROR,
                "capture_dump_unreadable",
                format!("capture dump is not parseable JSON: {err}"),
            )
        })?;
        if caller.role == UserRole::User {
            redact_transform_chains_for_user(&mut dump);
        }
        return Ok(Json(json!({
            "request_id": record.request_id,
            "file_name": record.file_name,
            "created_at": record.created_at,
            "size_bytes": record.size_bytes,
            "owner": {
                "id": record.user_id,
                "username": owner.map(|owner| owner.username),
            },
            "dump": dump,
        })));
    }
    Err(capture_not_found())
}
