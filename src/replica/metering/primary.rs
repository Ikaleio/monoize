use super::*;

struct DeltaChargeAccount {
    sub_account: Option<(String, i128)>,
    missing_key: bool,
}

async fn lock_delta_charge_account(
    db: &DbPool,
    tx: &sea_orm::DatabaseTransaction,
    delta: &BalanceDelta,
) -> Result<DeltaChargeAccount, String> {
    let mut sub_account: Option<(String, i128)> = None;
    let mut missing_key = false;
    if delta.kind == "api_key_charge" {
        let api_key_id = delta.api_key_id.as_deref().unwrap_or_default();
        let lock_suffix = if db.is_sqlite() { "" } else { " FOR UPDATE" };
        let row = tx
            .query_one(db.stmt(
                &format!("SELECT user_id, sub_account_enabled, sub_account_balance_nano FROM api_keys WHERE id = $1{lock_suffix}"),
                vec![api_key_id.into()],
            ))
            .await
            .map_err(|error| error.to_string())?;
        match row {
            None => missing_key = true,
            Some(row) => {
                let owner: String = row
                    .try_get("", "user_id")
                    .map_err(|error| error.to_string())?;
                if owner != delta.user_id {
                    return Err("replica charge API key owner mismatch".to_string());
                }
                let enabled = row
                    .try_get::<Option<i32>>("", "sub_account_enabled")
                    .map_err(|error| error.to_string())?
                    .unwrap_or(0)
                    != 0;
                if enabled {
                    let raw = row
                        .try_get::<Option<String>>("", "sub_account_balance_nano")
                        .map_err(|error| error.to_string())?
                        .unwrap_or_else(|| "0".to_string());
                    let current = raw
                        .parse::<i128>()
                        .map_err(|_| "malformed stored sub-account balance".to_string())?;
                    sub_account = Some((api_key_id.to_string(), current));
                }
            }
        }
    }

    Ok(DeltaChargeAccount {
        sub_account,
        missing_key,
    })
}

async fn insert_delta_ledger(
    db: &DbPool,
    tx: &sea_orm::DatabaseTransaction,
    delta: &BalanceDelta,
    allocation: &crate::users::PlanChargeAllocation,
    balance_after: Option<i128>,
) -> Result<(), String> {
    let ledger_meta = crate::users::plan_charge_meta(&delta.meta_json, allocation);
    let conflict_clause =
        "ON CONFLICT(idempotency_key) WHERE idempotency_key IS NOT NULL DO NOTHING";
    let sql = format!(
        "INSERT INTO billing_ledger (id, user_id, kind, delta_nano_usd, balance_after_nano_usd, meta_json, created_at, idempotency_key) \
         VALUES ($1, $2, $3, $4, $5, $6, $7, $8) {conflict_clause}"
    );
    let insert_result = tx
        .execute(db.stmt(
            &sql,
            vec![
                delta.delta_id.clone().into(),
                delta.user_id.clone().into(),
                delta.kind.clone().into(),
                (-allocation.fallback_nano_usd).to_string().into(),
                balance_after.map(|value| value.to_string()).into(),
                ledger_meta.to_string().into(),
                delta.created_at.clone().into(),
                delta.delta_id.clone().into(),
            ],
        ))
        .await
        .map_err(|error| error.to_string())?;
    if insert_result.rows_affected() == 0 {
        return Err("replica delta idempotency raced with another apply".to_string());
    }

    Ok(())
}

async fn update_delta_request_log(
    db: &DbPool,
    tx: &sea_orm::DatabaseTransaction,
    request_id: &str,
    allocation: &crate::users::PlanChargeAllocation,
) -> Result<(), String> {
    let breakdown = tx
        .query_one(db.stmt(
            "SELECT billing_breakdown_json FROM request_logs WHERE request_id = $1",
            vec![request_id.into()],
        ))
        .await
        .map_err(|error| error.to_string())?
        .and_then(|row| {
            row.try_get::<Option<String>>("", "billing_breakdown_json")
                .ok()
                .flatten()
        })
        .and_then(|raw| serde_json::from_str::<Value>(&raw).ok())
        .map(|mut value| {
            if let Some(object) = value.as_object_mut() {
                let pre_plan = object
                    .get("final_charge_nano")
                    .cloned()
                    .unwrap_or(Value::Null);
                object.insert("pre_plan_charge_nano".to_string(), pre_plan);
                object.insert(
                    "final_charge_nano".to_string(),
                    Value::String(allocation.adjusted_charge_nano_usd.to_string()),
                );
                object.insert(
                    "plan".to_string(),
                    serde_json::json!({
                        "subscription_id": allocation.subscription_id,
                        "plan_id": allocation.plan_id,
                        "multiplier": allocation.multiplier,
                        "covered_nano": allocation.plan_covered_nano_usd.to_string(),
                        "fallback_nano": allocation.fallback_nano_usd.to_string(),
                    }),
                );
            }
            value.to_string()
        });
    tx.execute(db.stmt(
        "UPDATE request_logs SET charge_nano_usd = $1, billing_breakdown_json = COALESCE($2, billing_breakdown_json) WHERE request_id = $3",
        vec![allocation.adjusted_charge_nano_usd.to_string().into(), breakdown.into(), request_id.into()],
    ))
    .await
    .map_err(|error| error.to_string())?;
    Ok(())
}

// ---------------------------------------------------------------------------
// Primary-side ingest endpoint (I1–I6)
// ---------------------------------------------------------------------------

pub fn sha256_hex_lower(input: &str) -> [u8; 32] {
    use sha2::{Digest, Sha256};
    let digest = Sha256::digest(input.as_bytes());
    let mut out = [0u8; 32];
    out.copy_from_slice(&digest);
    out
}

fn constant_time_eq(left: &[u8], right: &[u8]) -> bool {
    if left.len() != right.len() {
        return false;
    }
    let mut diff = 0u8;
    for (a, b) in left.iter().zip(right.iter()) {
        diff |= a ^ b;
    }
    diff == 0
}

fn metering_error(
    status: StatusCode,
    code: &str,
    message: impl Into<String>,
) -> axum::response::Response {
    (
        status,
        axum::Json(serde_json::json!({ "error": { "code": code, "message": message.into() } })),
    )
        .into_response()
}

/// I2: bearer compared by SHA-256 digest equality (constant-time).
pub(crate) fn verify_ingest_token(headers: &HeaderMap, expected_digest: &[u8; 32]) -> bool {
    let provided = headers
        .get(axum::http::header::AUTHORIZATION)
        .and_then(|value| value.to_str().ok())
        .and_then(|raw| raw.strip_prefix("Bearer "))
        .map(str::trim);
    let Some(provided) = provided else {
        return false;
    };
    constant_time_eq(&sha256_hex_lower(provided), expected_digest)
}

pub(crate) async fn ingest_metering_handler(
    State(state): State<crate::app::AppState>,
    headers: HeaderMap,
    body: axum::body::Bytes,
) -> axum::response::Response {
    let Some(expected) = state.metering_token_digest else {
        return metering_error(
            StatusCode::NOT_FOUND,
            "not_found",
            "ingest endpoint disabled",
        );
    };
    if !verify_ingest_token(&headers, &expected) {
        return metering_error(
            StatusCode::UNAUTHORIZED,
            "replica_auth_failed",
            "invalid replica token",
        );
    }
    let batch: MeteringBatch = match serde_json::from_slice(&body) {
        Ok(batch) => batch,
        Err(error) => {
            return metering_error(
                StatusCode::UNPROCESSABLE_ENTITY,
                "metering_batch_invalid",
                error.to_string(),
            );
        }
    };
    if batch.total_entries() > METERING_BATCH_HARD_CAP {
        return metering_error(
            StatusCode::PAYLOAD_TOO_LARGE,
            "metering_batch_too_large",
            format!("batch exceeds hard cap of {METERING_BATCH_HARD_CAP} entries"),
        );
    }
    for pair in &batch.last_used {
        if pair.api_key_id.is_empty() || pair.last_used_at.is_empty() {
            return metering_error(
                StatusCode::UNPROCESSABLE_ENTITY,
                "metering_batch_invalid",
                "last_used entries require api_key_id and last_used_at",
            );
        }
    }
    for delta in &batch.balance_deltas {
        if let Err(message) = validate_delta(delta) {
            return metering_error(
                StatusCode::UNPROCESSABLE_ENTITY,
                "metering_batch_invalid",
                message,
            );
        }
        if delta.kind == "api_key_charge" && delta.api_key_id.as_deref().unwrap_or("").is_empty() {
            return metering_error(
                StatusCode::UNPROCESSABLE_ENTITY,
                "metering_batch_invalid",
                "api_key_charge requires api_key_id",
            );
        }
    }

    if let Some(replica) = batch.replica.clone() {
        if replica.id.trim().is_empty() {
            return metering_error(
                StatusCode::UNPROCESSABLE_ENTITY,
                "metering_batch_invalid",
                "replica.id must be non-empty",
            );
        }
        let now_ms = chrono::Utc::now().timestamp_millis();
        state.replica_heartbeats.insert(
            replica.id.clone(),
            ReplicaHeartbeatRecord {
                heartbeat: replica,
                last_seen_unix_ms: now_ms,
            },
        );
    }

    match apply_metering_batch(&state.db_pool, &state.user_store, &batch).await {
        Ok(ack) => {
            if !batch.request_logs.is_empty() {
                let rows = batch
                    .request_logs
                    .iter()
                    .map(SpoolRequestLog::to_insert_log)
                    .collect::<Vec<_>>();
                let _ = state.log_broadcast.send(rows);
            }
            axum::Json(ack).into_response()
        }
        Err(error) => {
            tracing::warn!(error = %error, "metering batch apply failed; replica will retry");
            metering_error(
                StatusCode::INTERNAL_SERVER_ERROR,
                "metering_apply_failed",
                error,
            )
        }
    }
}

// ---------------------------------------------------------------------------
// Idempotent apply (I4–I5), also reused by the PRP9 promotion drain
// ---------------------------------------------------------------------------

const LAST_USED_CHUNK_ENTRIES: usize = 256;

pub async fn apply_metering_batch(
    db: &DbPool,
    user_store: &crate::users::UserStore,
    batch: &MeteringBatch,
) -> Result<MeteringAck, String> {
    use sea_orm::{ConnectionTrait, TransactionTrait};

    let write = db.write().await;
    let tx = write.begin().await.map_err(|error| error.to_string())?;

    let mut applied_request_logs = 0u64;
    let mut applied_balance_deltas = 0u64;
    let mut apply_error: Option<String> = None;

    for chunk in batch
        .request_logs
        .chunks(crate::db_cache::REQUEST_LOG_INSERT_CHUNK_ENTRIES)
    {
        let (sql, values) = crate::db_cache::request_log_insert_chunk(chunk.iter());
        match tx.execute(db.stmt(&sql, values)).await {
            Ok(outcome) => applied_request_logs += outcome.rows_affected(),
            Err(error) => {
                apply_error = Some(error.to_string());
                break;
            }
        }
    }

    if apply_error.is_none() {
        for chunk in batch.last_used.chunks(LAST_USED_CHUNK_ENTRIES) {
            let pairs = chunk
                .iter()
                .filter_map(|pair| {
                    chrono::DateTime::parse_from_rfc3339(&pair.last_used_at)
                        .ok()
                        .map(|parsed| (pair.api_key_id.clone(), parsed.with_timezone(&chrono::Utc)))
                })
                .collect::<Vec<_>>();
            if pairs.is_empty() {
                continue;
            }
            let (sql, values) = crate::db_cache::last_used_bulk_update(&pairs);
            if let Err(error) = tx.execute(db.stmt(&sql, values)).await {
                apply_error = Some(error.to_string());
                break;
            }
        }
    }

    if apply_error.is_none() {
        for delta in &batch.balance_deltas {
            match apply_balance_delta(db, user_store, &tx, delta).await {
                Ok(count) => applied_balance_deltas += count,
                Err(error) => {
                    apply_error = Some(error);
                    break;
                }
            }
        }
    }

    if let Some(error) = apply_error {
        if let Err(rollback_error) = tx.rollback().await {
            tracing::warn!("metering apply rollback error: {rollback_error}");
        }
        return Err(error);
    }

    tx.commit().await.map_err(|error| error.to_string())?;
    // DPT-BP3: primary spend preflight reads balances through these caches.
    for delta in &batch.balance_deltas {
        user_store.balance_cache.invalidate(&delta.user_id);
        if let Some(api_key_id) = delta.api_key_id.as_deref() {
            user_store.sub_account_balance_cache.invalidate(api_key_id);
        }
    }
    metrics::counter!("monoize_primary_metering_applied_total").increment(applied_balance_deltas);
    Ok(MeteringAck {
        applied_request_logs,
        applied_last_used: batch.last_used.len() as u64,
        applied_balance_deltas,
    })
}

/// Apply one replica charge atomically. The pre-check avoids replaying plan usage. The
/// unique ledger key remains the database idempotency anchor for concurrent applies.
async fn apply_balance_delta(
    db: &DbPool,
    user_store: &crate::users::UserStore,
    tx: &sea_orm::DatabaseTransaction,
    delta: &BalanceDelta,
) -> Result<u64, String> {
    let already_applied = tx
        .query_one(db.stmt(
            "SELECT id FROM billing_ledger WHERE idempotency_key = $1",
            vec![delta.delta_id.clone().into()],
        ))
        .await
        .map_err(|error| error.to_string())?
        .is_some();
    if already_applied {
        return Ok(0);
    }
    let amount = delta
        .amount_nano_usd
        .trim()
        .parse::<i128>()
        .map_err(|_| "amount_nano_usd must be decimal i128 text".to_string())?;
    let request_id = delta
        .meta_json
        .get("request_id")
        .and_then(Value::as_str)
        .filter(|value| !value.trim().is_empty())
        .ok_or_else(|| "replica charge metadata is missing request_id".to_string())?;
    let billing_group_id = delta
        .meta_json
        .get("billing_group_id")
        .and_then(Value::as_str);
    let user = user_store
        .lock_user_balance_tx(tx, &delta.user_id)
        .await
        .map_err(|error| error.message)?;
    let now = chrono::Utc::now();

    let DeltaChargeAccount {
        sub_account,
        missing_key,
    } = lock_delta_charge_account(db, tx, delta).await?;

    let bypass = missing_key || (sub_account.is_none() && user.unlimited);
    let allocation = if bypass {
        crate::users::PlanChargeAllocation {
            adjusted_charge_nano_usd: amount,
            plan_covered_nano_usd: 0,
            fallback_nano_usd: 0,
            subscription_id: None,
            plan_id: None,
            multiplier: None,
        }
    } else {
        user_store
            .allocate_plan_charge_tx(
                tx,
                crate::users::PlanChargeRequest {
                    user_id: &delta.user_id,
                    api_key_id: delta.api_key_id.as_deref(),
                    billing_group_id,
                    request_id,
                    settled_charge_nano_usd: amount,
                    now,
                },
            )
            .await
            .map_err(|error| error.message)?
    };

    let balance_after = if let Some((api_key_id, current)) = sub_account {
        let next = checked_sub_allow_negative(current, allocation.fallback_nano_usd)?;
        if allocation.fallback_nano_usd > 0 {
            tx.execute(db.stmt(
                "UPDATE api_keys SET sub_account_balance_nano = $1 WHERE id = $2",
                vec![next.to_string().into(), api_key_id.into()],
            ))
            .await
            .map_err(|error| error.to_string())?;
        }
        Some(next)
    } else if bypass {
        None
    } else {
        let next = checked_sub_allow_negative(user.balance, allocation.fallback_nano_usd)?;
        if allocation.fallback_nano_usd > 0 {
            tx.execute(db.stmt(
                "UPDATE users SET balance_nano_usd = $1, updated_at = $2 WHERE id = $3",
                vec![
                    next.to_string().into(),
                    now.to_rfc3339().into(),
                    delta.user_id.clone().into(),
                ],
            ))
            .await
            .map_err(|error| error.to_string())?;
        }
        Some(next)
    };

    insert_delta_ledger(db, tx, delta, &allocation, balance_after).await?;

    update_delta_request_log(db, tx, request_id, &allocation).await?;
    Ok(1)
}

fn checked_sub_allow_negative(current: i128, amount: i128) -> Result<i128, String> {
    // I5: negative results are allowed; only genuine i128 overflow aborts the batch.
    current
        .checked_sub(amount)
        .ok_or_else(|| "balance overflow".to_string())
}

/// PRP9: drain leftover delta spool entries directly into the local database when a
/// former replica starts as the primary. Runs after migrations, before serving traffic.
pub async fn drain_delta_spool_to_local_db(
    db: &DbPool,
    user_store: &crate::users::UserStore,
    spool: &DeltaSpool,
) -> Result<(), String> {
    loop {
        let files = spool.load_batch(METERING_BATCH_HARD_CAP).await;
        if files.is_empty() {
            return Ok(());
        }
        let batch = MeteringBatch {
            replica: None,
            request_logs: Vec::new(),
            last_used: Vec::new(),
            balance_deltas: files.iter().map(|(_, _, delta)| delta.clone()).collect(),
        };
        let ack = apply_metering_batch(db, user_store, &batch)
            .await
            .map_err(|error| {
                format!(
                    "promotion drain failed: {error}; spool entries preserved at {}",
                    spool.dir_display()
                )
            })?;
        // Every entry is now either applied or already present server-side under the
        // same idempotency key, so releasing all of them is safe.
        let refs: Vec<(PathBuf, u64)> = files
            .iter()
            .map(|(path, size, _)| (path.clone(), *size))
            .collect();
        let _ = ack.applied_balance_deltas;
        spool.release(&refs).await;
        if files.len() < METERING_BATCH_HARD_CAP {
            return Ok(());
        }
    }
}
