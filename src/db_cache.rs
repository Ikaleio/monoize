use chrono::{DateTime, Utc};
use dashmap::DashMap;
use std::sync::Arc;
use std::time::{Duration, Instant};
use tokio::sync::Mutex;

use crate::db::DbPool;
use crate::users::{InsertRequestLog, UserBalance};

// ---------------------------------------------------------------------------
// LastUsedBatcher: buffers api_key last_used timestamps, flushes periodically
// ---------------------------------------------------------------------------

#[derive(Debug, Clone)]
pub struct LastUsedBatcher {
    buffer: Arc<DashMap<String, DateTime<Utc>>>,
}

impl LastUsedBatcher {
    pub fn new() -> Self {
        Self {
            buffer: Arc::new(DashMap::new()),
        }
    }

    pub fn record(&self, api_key_id: String, now: DateTime<Utc>) {
        self.buffer.insert(api_key_id, now);
    }

    /// Drain all buffered entries and flush them to DB in a single write lock acquisition.
    pub async fn flush(&self, db: &DbPool) {
        let entries: Vec<(String, DateTime<Utc>)> = {
            let mut drained = Vec::new();
            self.buffer.retain(|k, v| {
                drained.push((k.clone(), *v));
                false
            });
            drained
        };
        if entries.is_empty() {
            return;
        }
        let write = db.write().await;
        use sea_orm::ConnectionTrait;
        for (id, ts) in &entries {
            let sql = "UPDATE api_keys SET last_used_at = $1 WHERE id = $2";
            if let Err(e) = write
                .execute(db.stmt(sql, vec![ts.to_rfc3339().into(), id.clone().into()]))
                .await
            {
                tracing::warn!("last_used_batcher flush error for key {id}: {e}");
            }
        }
    }

    /// Spawn background task that flushes every `interval`.
    pub fn spawn_flush_task(self, db: DbPool, interval: Duration) -> tokio::task::JoinHandle<()> {
        tokio::spawn(async move {
            let mut ticker = tokio::time::interval(interval);
            ticker.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Delay);
            loop {
                ticker.tick().await;
                self.flush(&db).await;
            }
        })
    }
}

// ---------------------------------------------------------------------------
// RequestLogBatcher: buffers InsertRequestLog entries, flushes as batch INSERT
// ---------------------------------------------------------------------------

#[derive(Debug, Clone)]
pub struct RequestLogBatcher {
    buffer: Arc<Mutex<Vec<InsertRequestLog>>>,
    capacity_hint: usize,
}

impl RequestLogBatcher {
    pub fn new(capacity_hint: usize) -> Self {
        Self {
            buffer: Arc::new(Mutex::new(Vec::with_capacity(capacity_hint))),
            capacity_hint,
        }
    }

    pub async fn push(&self, log: InsertRequestLog) {
        let mut buf = self.buffer.lock().await;
        buf.push(log);
    }

    /// Drain buffer and batch-insert into DB.
    pub async fn flush(&self, db: &DbPool) {
        let entries: Vec<InsertRequestLog> = {
            let mut buf = self.buffer.lock().await;
            if buf.is_empty() {
                return;
            }
            let drained = std::mem::replace(&mut *buf, Vec::with_capacity(self.capacity_hint));
            drained
        };
        if entries.is_empty() {
            return;
        }

        let write = db.write().await;
        use sea_orm::ConnectionTrait;
        use sea_orm::Value as SeaValue;

        for log in &entries {
            let id = uuid::Uuid::new_v4().to_string();
            let now = Utc::now().to_rfc3339();
            let sql = r#"INSERT INTO request_logs
                   (id, request_id, user_id, api_key_id, model, provider_id, upstream_model, channel_id, is_stream,
                    input_tokens, output_tokens, cache_read_tokens, cache_creation_tokens, tool_prompt_tokens, reasoning_tokens,
                    accepted_prediction_tokens, rejected_prediction_tokens,
                    provider_multiplier, charge_nano_usd, status, usage_breakdown_json,
                    billing_breakdown_json, error_code, error_message, error_http_status,
                    duration_ms, ttfb_ms, request_ip, reasoning_effort, tried_providers_json, request_kind, created_at)
                   VALUES ($1, $2, $3, $4, $5, $6, $7, $8, $9, $10, $11, $12, $13, $14, $15, $16, $17, $18, $19, $20, $21, $22, $23, $24, $25, $26, $27, $28, $29, $30, $31, $32)"#;
            let values = vec![
                id.into(),
                log.request_id.clone().into(),
                log.user_id.clone().into(),
                log.api_key_id.clone().into(),
                log.model.clone().into(),
                log.provider_id.clone().into(),
                log.upstream_model.clone().into(),
                log.channel_id.clone().into(),
                SeaValue::Int(Some(if log.is_stream { 1 } else { 0 })),
                log.input_tokens
                    .map(|v| SeaValue::BigInt(Some(v as i64)))
                    .unwrap_or(SeaValue::BigInt(None)),
                log.output_tokens
                    .map(|v| SeaValue::BigInt(Some(v as i64)))
                    .unwrap_or(SeaValue::BigInt(None)),
                log.cache_read_tokens
                    .map(|v| SeaValue::BigInt(Some(v as i64)))
                    .unwrap_or(SeaValue::BigInt(None)),
                log.cache_creation_tokens
                    .map(|v| SeaValue::BigInt(Some(v as i64)))
                    .unwrap_or(SeaValue::BigInt(None)),
                log.tool_prompt_tokens
                    .map(|v| SeaValue::BigInt(Some(v as i64)))
                    .unwrap_or(SeaValue::BigInt(None)),
                log.reasoning_tokens
                    .map(|v| SeaValue::BigInt(Some(v as i64)))
                    .unwrap_or(SeaValue::BigInt(None)),
                log.accepted_prediction_tokens
                    .map(|v| SeaValue::BigInt(Some(v as i64)))
                    .unwrap_or(SeaValue::BigInt(None)),
                log.rejected_prediction_tokens
                    .map(|v| SeaValue::BigInt(Some(v as i64)))
                    .unwrap_or(SeaValue::BigInt(None)),
                log.provider_multiplier
                    .map(|v| SeaValue::Double(Some(v)))
                    .unwrap_or(SeaValue::Double(None)),
                log.charge_nano_usd.map(|v| v.to_string()).into(),
                log.status.clone().into(),
                log.usage_breakdown_json
                    .as_ref()
                    .map(serde_json::Value::to_string)
                    .into(),
                log.billing_breakdown_json
                    .as_ref()
                    .map(serde_json::Value::to_string)
                    .into(),
                log.error_code.clone().into(),
                log.error_message.clone().into(),
                log.error_http_status
                    .map(|v| SeaValue::BigInt(Some(i64::from(v))))
                    .unwrap_or(SeaValue::BigInt(None)),
                log.duration_ms
                    .map(|v| SeaValue::BigInt(Some(v as i64)))
                    .unwrap_or(SeaValue::BigInt(None)),
                log.ttfb_ms
                    .map(|v| SeaValue::BigInt(Some(v as i64)))
                    .unwrap_or(SeaValue::BigInt(None)),
                log.request_ip.clone().into(),
                log.reasoning_effort.clone().into(),
                log.tried_providers_json
                    .as_ref()
                    .map(serde_json::Value::to_string)
                    .into(),
                log.request_kind.clone().into(),
                now.into(),
            ];
            if let Err(e) = write.execute(db.stmt(sql, values)).await {
                tracing::warn!("request_log_batcher flush error: {e}");
            }
        }
    }

    /// Spawn background task that flushes every `interval`.
    pub fn spawn_flush_task(self, db: DbPool, interval: Duration) -> tokio::task::JoinHandle<()> {
        tokio::spawn(async move {
            let mut ticker = tokio::time::interval(interval);
            ticker.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Delay);
            loop {
                ticker.tick().await;
                self.flush(&db).await;
            }
        })
    }
}

// ---------------------------------------------------------------------------
// ApiKeyCache: caches validated API key lookups, invalidated on mutation
// ---------------------------------------------------------------------------

use crate::users::{ApiKey, User};

#[derive(Clone)]
struct CachedApiKeyEntry {
    api_key: ApiKey,
    user: User,
    cached_at: Instant,
}

/// Caches successful `validate_api_key` results keyed by key prefix (first 12 chars).
/// Entries expire after `ttl`. Mutations to api_keys table must call `invalidate_*`.
#[derive(Debug, Clone)]
pub struct ApiKeyCache {
    // Keyed by key_prefix (12 chars)
    cache: Arc<DashMap<String, CachedApiKeyEntry>>,
    ttl: Duration,
}

impl std::fmt::Debug for CachedApiKeyEntry {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("CachedApiKeyEntry")
            .field("api_key_id", &self.api_key.id)
            .field("user_id", &self.user.id)
            .finish()
    }
}

impl ApiKeyCache {
    pub fn new(ttl: Duration) -> Self {
        Self {
            cache: Arc::new(DashMap::new()),
            ttl,
        }
    }

    pub fn get(&self, prefix: &str) -> Option<(ApiKey, User)> {
        let entry = self.cache.get(prefix)?;
        if entry.cached_at.elapsed() > self.ttl {
            drop(entry);
            self.cache
                .remove_if(prefix, |_, v| v.cached_at.elapsed() > self.ttl);
            return None;
        }
        Some((entry.api_key.clone(), entry.user.clone()))
    }

    pub fn insert(&self, prefix: String, api_key: ApiKey, user: User) {
        self.cache.insert(
            prefix,
            CachedApiKeyEntry {
                api_key,
                user,
                cached_at: Instant::now(),
            },
        );
    }

    /// Invalidate a single key by its ID (scans all entries).
    pub fn invalidate_by_key_id(&self, key_id: &str) {
        self.cache.retain(|_, v| v.api_key.id != key_id);
    }

    /// Invalidate all keys belonging to a user.
    pub fn invalidate_by_user_id(&self, user_id: &str) {
        self.cache.retain(|_, v| v.api_key.user_id != user_id);
    }

    /// Invalidate entries matching any of the given key IDs.
    pub fn invalidate_by_key_ids(&self, key_ids: &[String]) {
        let key_id_set: std::collections::HashSet<&str> =
            key_ids.iter().map(String::as_str).collect();
        self.cache
            .retain(|_, v| !key_id_set.contains(v.api_key.id.as_str()));
    }

    pub fn invalidate_by_prefix(&self, prefix: &str) {
        self.cache.remove(prefix);
    }

    /// Remove all entries.
    pub fn invalidate_all(&self) {
        self.cache.clear();
    }

    pub fn spawn_eviction_task(self, interval: Duration) -> tokio::task::JoinHandle<()> {
        tokio::spawn(async move {
            let mut ticker = tokio::time::interval(interval);
            ticker.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Delay);
            loop {
                ticker.tick().await;
                let ttl = self.ttl;
                self.cache.retain(|_, v| v.cached_at.elapsed() <= ttl);
            }
        })
    }
}

// ---------------------------------------------------------------------------
// BalanceCache: caches user balance lookups, invalidated on charge/adjust
// ---------------------------------------------------------------------------

#[derive(Clone, Debug)]
struct CachedBalanceEntry {
    balance: UserBalance,
    cached_at: Instant,
}

/// Caches `get_user_balance` results keyed by user_id.
/// Entries expire after `ttl`. Balance mutations must call `invalidate`.
#[derive(Debug, Clone)]
pub struct BalanceCache {
    cache: Arc<DashMap<String, CachedBalanceEntry>>,
    ttl: Duration,
}

impl BalanceCache {
    pub fn new(ttl: Duration) -> Self {
        Self {
            cache: Arc::new(DashMap::new()),
            ttl,
        }
    }

    pub fn get(&self, user_id: &str) -> Option<UserBalance> {
        let entry = self.cache.get(user_id)?;
        if entry.cached_at.elapsed() > self.ttl {
            drop(entry);
            self.cache
                .remove_if(user_id, |_, v| v.cached_at.elapsed() > self.ttl);
            return None;
        }
        Some(entry.balance.clone())
    }

    pub fn insert(&self, user_id: String, balance: UserBalance) {
        self.cache.insert(
            user_id,
            CachedBalanceEntry {
                balance,
                cached_at: Instant::now(),
            },
        );
    }

    pub fn invalidate(&self, user_id: &str) {
        self.cache.remove(user_id);
    }

    pub fn invalidate_all(&self) {
        self.cache.clear();
    }

    pub fn spawn_eviction_task(self, interval: Duration) -> tokio::task::JoinHandle<()> {
        tokio::spawn(async move {
            let mut ticker = tokio::time::interval(interval);
            ticker.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Delay);
            loop {
                ticker.tick().await;
                let ttl = self.ttl;
                self.cache.retain(|_, v| v.cached_at.elapsed() <= ttl);
            }
        })
    }
}
