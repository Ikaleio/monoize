# Database Performance Tuning Specification

## 0. Status

- **Purpose:** Reduce SQLite write contention and query latency through in-memory batching, caching, and PRAGMA tuning.
- **Scope:** Applies to the `db_cache` module and its integration with `UserStore`.
- **Dependencies:** `dashmap` (concurrent hash map), `tokio` (async runtime).

## 1. Module Structure

DPT1. All performance-tuning constructs MUST reside in `src/db_cache.rs`, exported as `pub mod db_cache` from the crate root.

DPT2. The module MUST expose exactly four public types:
- `LastUsedBatcher`
- `RequestLogBatcher`
- `ApiKeyCache`
- `BalanceCache`

## 2. LastUsedBatcher

### 2.1 State

DPT-LU1. `LastUsedBatcher` MUST hold a `DashMap<String, DateTime<Utc>>` keyed by `api_key_id`.

DPT-LU2. When `record(api_key_id, timestamp)` is called, the entry for `api_key_id` MUST be unconditionally overwritten with the new timestamp. If the key already exists, the previous value is replaced. No deduplication or ordering guarantee is required beyond last-write-wins.

### 2.2 Flush

DPT-LU3. `flush(db)` MUST atomically drain all buffered entries (via `retain(|_,_| false)`) and execute one `UPDATE api_keys SET last_used_at = $1 WHERE id = $2` per drained entry within a single write-lock acquisition.

DPT-LU4. If an individual UPDATE fails, the error MUST be logged at `warn` level. The flush MUST continue processing remaining entries (no short-circuit on partial failure).

DPT-LU5. If the buffer is empty at flush time, the method MUST return immediately without acquiring a write lock.

### 2.3 Background Task

DPT-LU6. `spawn_flush_task(db, interval)` MUST spawn a tokio task that calls `flush` at the given interval. The interval ticker MUST use `MissedTickBehavior::Delay`.

DPT-LU7. The default flush interval (as configured in `UserStore::spawn_background_tasks`) MUST be 30 seconds.

## 3. RequestLogBatcher

### 3.1 State

DPT-RL1. `RequestLogBatcher` MUST hold a `Mutex<Vec<InsertRequestLog>>` with a configurable initial capacity hint.

DPT-RL2. The default capacity hint (as configured in `UserStore::new`) MUST be 128.

### 3.2 Push

DPT-RL3. `push(log)` MUST append the `InsertRequestLog` to the internal buffer under the mutex.

### 3.3 Flush

DPT-RL4. `flush(db)` MUST atomically drain the buffer (via `std::mem::replace` with a fresh `Vec` of the same capacity hint) and execute one `INSERT INTO request_logs (...) VALUES (...)` per drained entry within a single write-lock acquisition.

DPT-RL5. Each INSERT MUST generate a new UUID `id` and set `created_at` to `Utc::now()` at flush time.

DPT-RL6. If an individual INSERT fails, the error MUST be logged at `warn` level. The flush MUST continue processing remaining entries.

DPT-RL7. If the buffer is empty at flush time, the method MUST return immediately without acquiring a write lock.

### 3.4 Background Task

DPT-RL8. `spawn_flush_task(db, interval)` MUST spawn a tokio task that calls `flush` at the given interval. The interval ticker MUST use `MissedTickBehavior::Delay`.

DPT-RL9. The default flush interval (as configured in `UserStore::spawn_background_tasks`) MUST be 2 seconds.

### 3.5 Data Loss Window

DPT-RL10. Between `push` and the next `flush`, request log entries exist only in memory. If the process crashes before flush, those entries are lost. The maximum data loss window equals the flush interval (2 seconds by default).

## 4. ApiKeyCache

### 4.1 State

DPT-AK1. `ApiKeyCache` MUST hold a `DashMap<String, CachedApiKeyEntry>` keyed by the API key prefix (first 12 characters of the key string).

DPT-AK2. Each `CachedApiKeyEntry` MUST contain:
- `api_key: ApiKey` (the full API key record)
- `user: User` (the owning user record)
- `cached_at: Instant` (wall-clock timestamp at insertion)

DPT-AK3. The TTL (as configured in `UserStore::new`) MUST be 60 seconds.

### 4.2 Lookup

DPT-AK4. `get(prefix)` MUST return `Some((ApiKey, User))` if and only if:
1. An entry exists for the given prefix, AND
2. `cached_at.elapsed() <= ttl`.

DPT-AK5. If an entry exists but `cached_at.elapsed() > ttl`, the cache MUST remove the entry only if the currently stored entry is still expired at removal time (conditional remove), and then return `None`.

### 4.3 Security Invariant

DPT-AK6. A cache hit MUST NOT bypass full-key plaintext verification. The caller (`validate_api_key`) MUST still compare `key != cached_key.key` (plaintext equality) on every cache-hit path to prevent prefix-collision attacks.

DPT-AK7. A cache hit MUST additionally verify that `cached_key.enabled == true`, `cached_user.enabled == true`, and the key is not expired (`expires_at > now` or `expires_at` is None). If any check fails, the entry MUST be invalidated and the caller MUST fall through to the database path.

### 4.4 Insertion

DPT-AK8. `insert(prefix, api_key, user)` MUST store the entry with `cached_at = Instant::now()`.

### 4.5 Invalidation

DPT-AK9. The following invalidation methods MUST exist:
- `invalidate_by_key_id(key_id)`: Remove all entries where `entry.api_key.id == key_id`.
- `invalidate_by_user_id(user_id)`: Remove all entries where `entry.api_key.user_id == user_id`.
- `invalidate_by_key_ids(key_ids)`: Remove all entries where `entry.api_key.id` is in `key_ids`.
- `invalidate_by_prefix(prefix)`: Remove the entry for the given key prefix.
- `invalidate_all()`: Clear the entire cache.

DPT-AK10. Invalidation MUST be called on the following mutation paths:

| Mutation | Invalidation Method |
|---|---|
| `delete_api_key(id)` | `invalidate_by_key_id(id)` |
| `update_api_key(key_id, input)` | `invalidate_by_key_id(key_id)` |
| `batch_delete_api_keys(ids)` | `invalidate_by_key_ids(ids)` |
| `delete_user(id)` | `invalidate_by_user_id(id)` |
| `update_user(id, ..., any persisted field changed, ...)` | `invalidate_by_user_id(id)` |
| `decrement_api_key_quota(api_key_id)` | `invalidate_by_key_id(api_key_id)` |

DPT-AK11. `update_user` MUST invalidate the API key cache whenever the update modifies any persisted user field.

DPT-AK12. `decrement_api_key_quota(api_key_id)` MUST invalidate API key cache entries for that key via `invalidate_by_key_id(api_key_id)` after the quota update executes.

DPT-AK13. `ApiKeyCache` MUST provide a background eviction task that periodically removes expired entries using `retain`.

## 5. BalanceCache

### 5.1 State

DPT-BC1. `BalanceCache` MUST hold a `DashMap<String, CachedBalanceEntry>` keyed by `user_id`.

DPT-BC2. Each `CachedBalanceEntry` MUST contain:
- `balance: UserBalance`
- `cached_at: Instant`

DPT-BC3. The TTL (as configured in `UserStore::new`) MUST be 30 seconds.

### 5.2 Lookup

DPT-BC4. `get(user_id)` MUST return `Some(UserBalance)` if and only if:
1. An entry exists for the given user_id, AND
2. `cached_at.elapsed() <= ttl`.

DPT-BC5. If an entry exists but `cached_at.elapsed() > ttl`, the cache MUST remove the entry only if the currently stored entry is still expired at removal time (conditional remove), and then return `None`.

### 5.3 Insertion

DPT-BC6. `get_user_balance(user_id)` MUST check the cache first. On cache miss, it MUST query the database and insert the result into the cache before returning.

### 5.4 Invalidation

DPT-BC7. The following invalidation methods MUST exist:
- `invalidate(user_id)`: Remove the entry for the given user_id.
- `invalidate_all()`: Clear the entire cache.

DPT-BC8. Invalidation MUST be called on the following mutation paths:

| Mutation | Invalidation Method |
|---|---|
| `charge_user_balance_nano_inner(user_id, ...)` | `invalidate(user_id)` — after transaction commit |
| `admin_adjust_user_balance(user_id, ...)` | `invalidate(user_id)` — after transaction commit |
| `update_user(id, ..., balance_nano_usd=Some(_), ...)` | `invalidate(id)` |
| `update_user(id, ..., balance_unlimited=Some(_), ...)` | `invalidate(id)` |
| `delete_user(id)` | `invalidate(id)` |

DPT-BC9. `update_user` MUST invalidate the balance cache only when `balance_nano_usd` or `balance_unlimited` is being changed. Other user field updates MUST NOT trigger balance cache invalidation.

### 5.5 Staleness Bound

DPT-BC10. The maximum staleness of a cached balance is bounded by the TTL (30 seconds). A user who receives a deposit or is charged will see the updated balance within at most 30 seconds, or immediately if the cache is explicitly invalidated by a mutation on the same process.

DPT-BC11. `BalanceCache` MUST provide a background eviction task that periodically removes expired entries using `retain`.

## 6. UserStore Integration

### 6.1 Construction

DPT-US1. `UserStore::new(db)` MUST construct all four subsystems:
- `LastUsedBatcher::new()`
- `RequestLogBatcher::new(128)`
- `ApiKeyCache::new(Duration::from_secs(60))`
- `BalanceCache::new(Duration::from_secs(30))`

### 6.2 Lifecycle

DPT-US2. `spawn_background_tasks()` MUST be called after `UserStore` construction (during application startup, after `load_state()`). It MUST spawn:
- flush task for `LastUsedBatcher` (30s interval),
- flush task for `RequestLogBatcher` (2s interval),
- eviction task for `ApiKeyCache` (30s interval),
- eviction task for `BalanceCache` (30s interval).

DPT-US3. `flush_all_batchers()` MUST be called during application shutdown. It MUST flush both `LastUsedBatcher` and `RequestLogBatcher` to ensure buffered data is persisted.

DPT-US4. `flush_all_batchers()` MUST be called in two shutdown paths:
1. Inside the `shutdown_signal` handler (after signal receipt, before graceful shutdown completes).
2. After `axum::serve` returns (to catch any data buffered during the drain period).

### 6.3 validate_api_key Integration

DPT-US5. `validate_api_key(key)` MUST follow this execution path:
1. If `key.len() < 12`, return `None`.
2. Extract `prefix = key[..12]`.
3. Check `ApiKeyCache::get(prefix)`.
4. On cache hit: verify `enabled`, `user.enabled`, `expires_at`, and `key != cached_key.key` (plaintext equality). If all pass, call `last_used_batcher.record(...)` and return the cached result. If any check fails, invalidate the cache entry and MUST immediately revalidate via the database path in the same call; cache validation failure alone MUST NOT produce an authentication error response.
5. On cache miss: query DB for API key by prefix, verify enabled/expired/key-equality/user, call `last_used_batcher.record(...)`, insert into `ApiKeyCache`, and return.

### 6.4 Request Log Integration

DPT-US6. `insert_request_log_pending()`, `update_pending_request_log_channel()`, and `update_pending_request_log_usage()` MUST be no-op stubs (return `Ok(())` immediately with no DB interaction).

DPT-US7. `finalize_request_log(log)` and `insert_request_log(log)` MUST push the `InsertRequestLog` to `RequestLogBatcher` instead of performing direct DB writes.

DPT-US8. `cleanup_pending_request_logs()` MUST remain functional and continue to transition any `status = "pending"` rows to `status = "error"`. This handles the edge case where the process crashes after a previous version created pending rows, or during the data-loss window between `push` and `flush`.

## 7. Concurrency Properties

DPT-C1. `LastUsedBatcher` and `ApiKeyCache` use `DashMap` for lock-free concurrent reads and sharded writes. No contention between readers and writers except on the same shard.

DPT-C2. `RequestLogBatcher` uses `tokio::sync::Mutex` for the buffer. The `push` operation holds the lock only for the duration of `Vec::push`. The `flush` operation holds the lock only for the duration of `std::mem::replace` (buffer swap), then releases it before executing DB writes.

DPT-C3. `BalanceCache` uses `DashMap` with the same concurrency properties as DPT-C1.
