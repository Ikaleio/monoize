# Database Performance Tuning Specification

## 0. Status

- **Purpose:** Reduce SQLite write contention and query latency through in-memory batching, caching, and PRAGMA tuning.
- **Scope:** Applies to the `db_cache` module and its integration with `UserStore`. Flush-to-database behaviors (DPT-LU3/DPT-LU6, DPT-RL4) apply to the `primary` node role; on a `replica` they are replaced by the shipment pipeline of `primary-replica-deployment.spec.md` M4–M5 with buffering semantics preserved (PRP12 there). Replica `ship_via` MUST use the same on-disk `.json` discovery as DPT-RL4 even when the in-memory buffer is empty.
- **Dependencies:** `dashmap` (concurrent hash map), `tokio` (async runtime).

## 1. Module Structure

DPT1. All performance-tuning constructs MUST reside in `src/db_cache.rs`, exported as `pub mod db_cache` from the crate root.

DPT2. The module MUST expose these public runtime types:
- `LastUsedBatcher`
- `RequestLogBatcher`
- `ApiKeyCache`
- `BalanceCache`

The module MUST also expose `RequestLogAdmissionError` so forwarding preflight and terminal finalization can distinguish spool quota exhaustion from spool unavailability.

## 2. LastUsedBatcher

### 2.1 State

DPT-LU1. `LastUsedBatcher` MUST hold a `DashMap<String, DateTime<Utc>>` keyed by `api_key_id`.

DPT-LU2. When `record(api_key_id, timestamp)` is called for an existing entry, the buffer MUST retain the later of the existing and supplied timestamps. A failed flush MUST reinsert the failed timestamp without replacing a newer concurrently recorded timestamp.

DPT-LU2a. The distinct-key capacity MUST be configurable. The default is `10000`, selected by `MONOIZE_LAST_USED_BUFFER_ENTRIES`. When the buffer is full, an update to an existing key remains accepted and a previously unseen key MAY be omitted with a warning because `last_used_at` is non-billing metadata.

### 2.2 Flush

DPT-LU3. `flush(db)` MUST atomically drain all buffered entries (via `retain(|_,_| false)`) and update them through bounded bulk `UPDATE ... CASE` statements within a single write-lock acquisition. The default chunk size is `256`, selected by `MONOIZE_LAST_USED_FLUSH_CHUNK_ENTRIES` and clamped to `[1, 400]` so each portable statement uses at most 800 bound values. Flush MUST execute at most one database round trip per non-empty chunk, not one round trip per key.

DPT-LU4. If one bulk UPDATE fails, the error MUST be logged at `warn` level and every entry in that chunk MUST be returned to the buffer for a later flush. The flush MUST continue processing remaining chunks.

DPT-LU5. If the buffer is empty at flush time, the method MUST return immediately without acquiring a write lock.

### 2.3 Background Task

DPT-LU6. `spawn_flush_task(db, interval)` MUST spawn a tokio task that calls `flush` at the given interval. The interval ticker MUST use `MissedTickBehavior::Delay`.

DPT-LU7. The default flush interval (as configured in `UserStore::spawn_background_tasks`) MUST be 30 seconds.

## 3. RequestLogBatcher

### 3.1 State

DPT-RL1. `RequestLogBatcher` MUST hold at most a configurable number of in-memory spool references. The default is `128`, selected by `MONOIZE_REQUEST_LOG_BUFFER_ENTRIES`.

DPT-RL2. Each accepted entry MUST be serialized to a unique file in the request-log spool before `push` reports success. `RuntimeConfig.request_log_spool_dir: Option<PathBuf>` MUST select the spool directory when it is `Some(path)`. When it is `None`, `RequestLogBatcher` MUST preserve the existing resolution order: use `MONOIZE_REQUEST_LOG_SPOOL_DIR` when that variable exists, otherwise use `./data/request-log-spool`. `RuntimeConfig::from_env()` MUST set this field to `None`; production therefore keeps the existing environment/default behavior. Two `AppState` instances with distinct explicit paths MUST not discover, flush, or account for each other's spool files.

### 3.2 Push

DPT-RL3. `push(log)` MUST assign the stable database row UUID, durably publish one spool file by temporary-file write followed by same-directory atomic rename, and then MAY append an in-memory reference under the mutex. It MUST return `Result` so the caller can fail closed when persistence is unavailable.

DPT-RL3a. The spool byte quota MUST be configurable. The default is `536870912`, selected by `MONOIZE_REQUEST_LOG_SPOOL_MAX_BYTES`. One serialized entry MUST be no larger than `8388608` bytes by default, selected by `MONOIZE_REQUEST_LOG_SPOOL_ENTRY_MAX_BYTES`. The terminal-log admission unit is `min(65536, MONOIZE_REQUEST_LOG_SPOOL_ENTRY_MAX_BYTES)` bytes. With default settings, the quota admits `8192` concurrent terminal-log reservations when no durable spool bytes are outstanding. Terminal-log preflight MUST reject `MONOIZE_REQUEST_LOG_SPOOL_ENTRY_MAX_BYTES` values below `4096` with `RequestLogAdmissionError::EntryQuotaTooSmall`; no reservation or admission marker may be created for that configuration.

DPT-RL3b. If accepting an entry would exceed either quota, or its one durable spool write attempt fails, unreserved `push` MUST return an error. It MUST NOT broadcast the log or remove its pending snapshot. `push` is for bounded internal producers, MUST NOT degrade an oversized entry, and MUST NOT retry indefinitely.

DPT-RL3c. `reserve_terminal_log()` MUST atomically reserve one admission unit (DPT-RL3a) against the sum of durable spool bytes and outstanding reservations. Concurrent reservations MUST NOT oversubscribe the spool quota. The reservation MUST preassign one stable spool-row UUID, one stable final `.json` path, and one stable hidden admission-marker path. `reserve_terminal_log()` MUST NOT perform filesystem I/O; `arm_reserved` is the first durable write and fails closed when the spool is not writable. Dropping the final clone of an unarmed reservation MUST remove any marker file at its marker path and release the reservation.

DPT-RL3c-0. Every spool file publication MUST write a temporary file, sync it, and rename it on the blocking thread pool; it MUST NOT block a Tokio worker thread. The publication MUST then complete one spool-directory sync that started after its rename returned. Concurrent publications MUST share directory syncs: at most one directory sync per spool directory runs at a time, and one completed sync satisfies every publication whose rename returned before that sync started. A failed shared sync MUST fail every publication that waited on it. On Windows, directory sync MUST open the directory with write access and `FILE_FLAG_BACKUP_SEMANTICS` before calling `FlushFileBuffers`.

DPT-RL3c-1. Before upstream dispatch, `arm_reserved(fallback_log, reservation)` MUST publish (DPT-RL3c-0) a valid `SpoolRequestLog` fallback at the reservation's marker path. The fallback MUST use the reservation's stable UUID, have terminal `status = "error"`, identify the admitted request, and state that terminal finalization was interrupted. Arming MUST fail unless the reservation belongs to that batcher, is unclaimed, and is unarmed. `push_reserved` MUST fail unless arming completed.

DPT-RL3c-2. Exactly one `push_reserved(log, reservation)` call MAY claim an armed reservation. It MUST encode the terminal log with the reservation's stable UUID. If the encoded entry is larger than the reservation, `push_reserved` MUST grow the reservation to the encoded size against the combined quota before it writes the entry. If growth would oversubscribe the quota, `push_reserved` MUST keep the armed fallback and retry growth until quota becomes available. It MUST then publish (DPT-RL3c-0) the entry at the reservation's stable final path. After claim, a growth, filesystem write, sync, or rename failure MUST leave the armed fallback and reservation active and MUST retry the same stable UUID and final path until the directory sync succeeds. If the rename succeeded and only the directory sync failed, the retry MUST repeat only the directory sync. Retry delay MUST start at `10 ms`, double after each failed attempt, and stop increasing at `1000 ms`. `push_reserved` MUST NOT acquire `flush_lock`. After the directory sync, `push_reserved` MUST remove the armed marker without a further directory sync; DPT-RL10 removes a marker that survives a crash next to its final file. The reservation MUST then convert to the serialized terminal entry's actual byte count and release unused reserved bytes. Terminal publication of one request MUST NOT wait for another request's file write or for `flush`. It MAY wait for one in-progress shared directory sync before the sync that covers its rename starts.

DPT-RL3c-3. Dropping the final clone of an armed or claimed reservation before terminal completion MUST preserve the fallback by atomically promoting its marker to the stable final `.json` path. If promotion succeeds, quota accounting MUST convert the reservation to the fallback file's actual byte count. If promotion fails, the marker MUST remain on disk and the process MUST retain the conservative full reservation.

DPT-RL3c-4. `cancel_reserved(reservation)` MAY cancel an armed but unclaimed reservation for a producer whose completed outcome is specified not to generate a row. Cancellation MUST exclusively transition the reservation out of the claimable state, then retry until the marker is absent and the spool directory sync succeeds. Retry delay MUST start at `10 ms`, double after each failed attempt, and stop increasing at `1000 ms`. It MUST mark the reservation consumed and release its full quota only after durable cancellation. An unarmed, claimed, consumed, or foreign reservation MUST be rejected. The caller MUST await cancellation before discarding the lifecycle.

DPT-RL3d. `push(log)` without a preflight reservation is reserved for internal producers such as active probes and tests. It MUST atomically reserve the serialized entry's actual byte count before writing and MUST obey the same combined quota.

DPT-RL3e. `arm_reserved` and `push_reserved` MUST serialize the complete entry. They MUST NOT replace the entry with a compact representation or generate `error_code = "request_log_payload_truncated"`. If the complete fallback encoding exceeds the reservation, `arm_reserved` MUST return `RequestLogAdmissionError::EntryTooLarge`. If the complete terminal encoding exceeds `MONOIZE_REQUEST_LOG_SPOOL_ENTRY_MAX_BYTES` or the spool quota, `push_reserved` MUST return `RequestLogAdmissionError::EntryTooLarge`. An `arm_reserved` size failure MUST occur before the reservation enters the armed state. A `push_reserved` size failure MUST leave the armed fallback available for durable promotion. The operation MUST NOT persist a partial terminal entry.

DPT-RL3f. Before a final rename makes a spool file visible to `flush`, the producer MUST add the file's byte count to `spool_bytes`. If that rename fails, the producer MUST subtract the same byte count. The file's bytes MUST remain covered by `admitted_bytes` through its reservation until the reservation converts to the actual byte count. `flush` MAY therefore select and delete a published file at any time after its rename. `flush_lock` MUST serialize only `flush` and `ship_via` with each other; `push` and `push_reserved` MUST NOT acquire it.

### 3.3 Flush

DPT-RL4. `flush(db)` MUST select a bounded batch of durable spool files and open one database transaction. It MUST partition the selected entries into consecutive chunks of at most `20` rows. Each non-empty chunk MUST execute exactly one multi-row `INSERT INTO request_logs (...) VALUES (...), (...) ON CONFLICT(id) DO NOTHING` statement. Each row MUST bind exactly the `42` request-log columns, so one statement binds at most `840` values and remains below the portable SQLite `999`-variable ceiling. Placeholder numbering MUST be contiguous across every row in one statement and MUST restart at `$1` for each chunk. The transaction MUST commit only after every chunk succeeds.

DPT-RL5. Each INSERT MUST use the stable UUID stored in the spool entry and the log entry's captured `created_at` value instead of flush time.

DPT-RL6. If transaction begin, any INSERT, or commit fails, the selected spool files MUST remain available for a later retry and the failure MUST be logged at `warn` level.

DPT-RL7. After a successful commit, the selected spool files MUST be deleted. An ambiguous commit outcome is safe because retries use `ON CONFLICT(id) DO NOTHING` with the stable UUID.

DPT-RL7a. Removing a committed spool file MUST subtract its size from `spool_bytes` and `admitted_bytes` without unsigned wraparound. If a file was published outside the current batcher's accounting window, subtraction MUST stop at zero. Such a file MUST NOT make later admission fail with a false quota-exhausted state.

### 3.4 Background Task

DPT-RL8. `spawn_flush_task(db, interval)` MUST spawn a tokio task that calls `flush` at the given interval. The interval ticker MUST use `MissedTickBehavior::Delay`.

DPT-RL9. The default flush interval (as configured in `UserStore::spawn_background_tasks`) MUST be 2 seconds.

### 3.5 Crash recovery

DPT-RL10. Spool files left by an abrupt process exit MUST be discovered and retried by a later process using the same spool directory. Memory-buffer capacity MUST NOT limit recovery of on-disk entries. Startup MUST delete legacy constant-content and unarmed write-probe admission markers. Startup MUST parse every armed admission marker as a bounded `SpoolRequestLog`; if its stable final path is absent, startup MUST atomically promote the marker to that path and sync the directory. If the final path already exists, startup MUST retain the final file and remove the duplicate marker. Startup MUST fail without deleting a non-legacy admission marker that is neither a valid fallback nor an already-published final row.

## 4. ApiKeyCache

### 4.1 State

DPT-AK1. `ApiKeyCache` MUST hold a `DashMap<String, CachedApiKeyEntry>` keyed by the complete API key string. The first 12 characters MUST NOT be used as a cache identity.

DPT-AK2. Each `CachedApiKeyEntry` MUST contain:
- `api_key: ApiKey` (the full API key record)
- `user: User` (the owning user record)
- `cached_at: Instant` (wall-clock timestamp at insertion)
- `generation: u64` (the cache invalidation generation observed before the database read)

DPT-AK3. The TTL (as configured in `UserStore::new`) MUST be 60 seconds.

DPT-AK3a. Entry capacity MUST be configurable. The default is `10000`, selected by `MONOIZE_API_KEY_CACHE_CAPACITY`. Insertion at capacity MUST evict an existing entry before publishing the new entry.

### 4.2 Lookup

DPT-AK4. `get(key)` MUST return `Some((ApiKey, User))` if and only if:
1. An entry exists for the complete key, AND
2. `cached_at.elapsed() <= ttl`.

DPT-AK5. If an entry exists but `cached_at.elapsed() > ttl`, the cache MUST remove the entry only if the currently stored entry is still expired at removal time (conditional remove), and then return `None`.

### 4.3 Security Invariant

DPT-AK6. A cache hit MUST NOT bypass full-key plaintext verification. The caller (`validate_api_key`) MUST still compare the supplied complete key with `cached_key.key` on every cache-hit path.

DPT-AK7. A cache hit MUST additionally verify that `cached_key.enabled == true`, `cached_user.enabled == true`, and the key is not expired (`expires_at > now` or `expires_at` is None). If any check fails, the entry MUST be invalidated and the caller MUST fall through to the database path.

### 4.4 Insertion

DPT-AK8. A cache-miss database read MUST capture the current invalidation generation before reading. The result MUST be inserted only while that generation remains current.

DPT-AK8a. After insertion, the cache MUST read the generation again. If it changed, the cache MUST conditionally remove the entry whose stored generation equals the stale generation and report insertion failure. The caller MUST repeat database validation instead of returning that stale result.

### 4.5 Invalidation

DPT-AK9. The cache MUST maintain reverse indexes from API-key ID and user ID to complete-token cache keys. The following invalidation methods MUST exist:
- `invalidate_by_key_id(key_id)`: Remove entries named by the API-key-ID reverse index.
- `invalidate_by_user_id(user_id)`: Remove entries named by the user-ID reverse index.
- `invalidate_by_key_ids(key_ids)`: Remove entries named by the API-key-ID reverse indexes.
- `invalidate(key)`: Remove the entry for the complete API key.
- `invalidate_all()`: Clear the entire cache.

DPT-AK9a. Every explicit invalidation MUST increment the cache generation before removing entries. A database result read before that increment MUST NOT be published afterward.

DPT-AK10. Invalidation MUST be called on the following mutation paths:

| Mutation | Invalidation Method |
|---|---|
| `delete_api_key(id)` | `invalidate_by_key_id(id)` |
| `update_api_key(key_id, input)` | `invalidate_by_key_id(key_id)` |
| `batch_delete_api_keys(ids)` | `invalidate_by_key_ids(ids)` |
| `delete_user(id)` | `invalidate_by_user_id(id)` |
| `update_user(id, ..., any persisted field changed, ...)` | `invalidate_by_user_id(id)` |
| `update_last_login(id)` | `invalidate_by_user_id(id)` after the database write succeeds |
| `decrement_api_key_quota(api_key_id)` | `invalidate_by_key_id(api_key_id)` |

DPT-AK11. `update_user` MUST invalidate the API key cache whenever the update modifies any persisted user field.

DPT-AK12. `decrement_api_key_quota(api_key_id)` MUST invalidate API key cache entries for that key via `invalidate_by_key_id(api_key_id)` after the quota update executes.

DPT-AK13. `ApiKeyCache` MUST provide a background eviction task that periodically removes expired entries using `retain`.

DPT-AK14. Explicit ID/user invalidation MUST NOT scan the complete-token cache. Eviction and every removal path MUST remove corresponding reverse-index membership. Stale empty reverse-index sets MUST be removed.

DPT-AK15. `delete_user(id)` MUST execute the following operations in one write transaction: lock the matching user row, reject a missing user, delete that user, verify that exactly one user row was deleted, and commit. It MUST NOT query or materialize the deleted user's API-key IDs. After commit it MUST call `ApiKeyCache::invalidate_by_user_id(id)` and `BalanceCache::invalidate(id)` and return `Result<(), String>`.

## 5. BalanceCache

### 5.1 State

DPT-BC1. `BalanceCache` MUST hold a `DashMap<String, CachedBalanceEntry>` keyed by `user_id`.

DPT-BC2. Each `CachedBalanceEntry` MUST contain:
- `balance: UserBalance`
- `cached_at: Instant`
- `generation: u64`

DPT-BC3. The TTL (as configured in `UserStore::new`) MUST be 30 seconds.

DPT-BC3a. Entry capacity MUST be configurable. The default is `10000`, selected by `MONOIZE_BALANCE_CACHE_CAPACITY`. Insertion at capacity MUST evict an existing entry before publishing the new entry.

### 5.2 Lookup

DPT-BC4. `get(user_id)` MUST return `Some(UserBalance)` if and only if:
1. An entry exists for the given user_id, AND
2. `cached_at.elapsed() <= ttl`.

DPT-BC5. If an entry exists but `cached_at.elapsed() > ttl`, the cache MUST remove the entry only if the currently stored entry is still expired at removal time (conditional remove), and then return `None`.

### 5.3 Insertion

DPT-BC6. `get_user_balance(user_id)` MUST check the cache first. On cache miss, it MUST capture the current invalidation generation, query the database, and insert the result only while that generation remains current.

DPT-BC6a. After insertion, the cache MUST read the generation again. If it changed, the cache MUST conditionally remove the entry whose stored generation equals the stale generation and `get_user_balance` MUST repeat the database read.

### 5.4 Invalidation

DPT-BC7. The following invalidation methods MUST exist:
- `invalidate(user_id)`: Remove the entry for the given user_id.
- `invalidate_all()`: Clear the entire cache.

DPT-BC7a. Every explicit invalidation MUST increment the cache generation before removing entries. A database result read before that increment MUST NOT be published afterward.

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

DPT-BC10. After a balance mutation commits and its same-process invalidation completes, later cache reads MUST NOT return a value read before that invalidation. The TTL remains 30 seconds for changes made outside this process.

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
- eviction task for `BalanceCache` (30s interval),
- expired-session cleanup task using the DPT-US10 interval.

DPT-US3. `flush_all_batchers()` MUST be called during application shutdown. It MUST flush both `LastUsedBatcher` and `RequestLogBatcher` to ensure buffered data is persisted.

DPT-US4. `flush_all_batchers()` MUST be called in two shutdown paths:
1. Inside the `shutdown_signal` handler (after signal receipt, before graceful shutdown completes).
2. After `axum::serve` returns (to catch any data buffered during the drain period).

### 6.3 validate_api_key Integration

DPT-US5. `validate_api_key(key)` MUST follow this execution path:
1. If `key.len() < 12`, return `None`.
2. Check `ApiKeyCache::get(key)` using the complete key.
3. On cache hit: verify `enabled`, `user.enabled`, `expires_at`, and `key != cached_key.key` (plaintext equality). If all pass, call `last_used_batcher.record(...)` and return the cached result. If any check fails, invalidate the cache entry and MUST immediately revalidate via the database path in the same call; cache validation failure alone MUST NOT produce an authentication error response.
4. On cache miss: capture the cache generation, query the database by complete key equality, verify enabled/expired/key-equality/user, and insert into `ApiKeyCache` only if the generation remains current.
5. If publication fails because the generation changed, repeat step 2. Otherwise call `last_used_batcher.record(...)` and return.

### 6.4 Request Log Integration

DPT-US6. `insert_request_log_pending()`, `update_pending_request_log_channel()`, and `update_pending_request_log_usage()` MUST be no-op stubs (return `Ok(())` immediately with no DB interaction).

DPT-US7. `finalize_request_log(log)` and `insert_request_log(log)` MUST push the `InsertRequestLog` to `RequestLogBatcher` instead of performing direct DB writes.

DPT-US8. `cleanup_pending_request_logs()` MUST remain functional and continue to transition any `status = "pending"` rows to `status = "error"`. This handles the edge case where the process crashes after a previous version created pending rows, or during the data-loss window between `push` and `flush`.

### 6.5 Session Retention

DPT-US9. `cleanup_expired_sessions()` MUST execute one set-based `DELETE FROM sessions WHERE expires_at <= threshold` statement and return the number of deleted rows. The threshold MUST be the current UTC time encoded with the same RFC3339 representation used by `create_session`. The cleanup MUST NOT first query or materialize session rows.

DPT-US10. `UserStore` construction MUST complete one DPT-US9 cleanup before returning. `spawn_background_tasks()` MUST repeat DPT-US9 after each configured interval. `MONOIZE_SESSION_CLEANUP_INTERVAL_SECONDS` MUST select a positive whole-second interval. Its default MUST be `3600`; a missing, empty, zero, negative, invalid, or overflowing value MUST select the default.

## 7. Concurrency Properties

DPT-C1. `LastUsedBatcher` and `ApiKeyCache` use `DashMap` for lock-free concurrent reads and sharded writes. No contention between readers and writers except on the same shard.

DPT-C2. `RequestLogBatcher` uses `tokio::sync::Mutex` for the buffer. The `push` operation holds the lock only for the duration of `Vec::push`. The `flush` operation holds the lock only for the duration of `std::mem::replace` (buffer swap), then releases it before executing DB writes.

DPT-C3. `BalanceCache` uses `DashMap` with the same concurrency properties as DPT-C1.

## 8. Request-Path Read Caches

This section adds process-local caches for reads that every forwarded request performs before upstream dispatch. It does not change §3.

### 8.1 SnapshotCache

DPT-SC1. `db_cache` MUST expose `SnapshotCache<T>`. It holds at most one published snapshot `Arc<T>`, the `Instant` at which the load of that snapshot started, the invalidation generation captured before that load, a monotonically increasing invalidation generation, and at most one in-flight load.

DPT-SC2. `get_or_load(load)` MUST return the published snapshot without database I/O if and only if its generation equals the current generation and no more than the TTL has elapsed since its load started. A hit MUST clone only the `Arc`.

DPT-SC3. On a miss, all callers that observe the same generation MUST share one in-flight load. Only one `load` closure runs. Every waiter receives the same `Ok(Arc<T>)` or the same `Err(String)`. A failed load MUST NOT be published or retained; the next miss MUST start a new load. No lock MUST be held across an `.await`.

DPT-SC4. A load MUST capture the generation before its first read. It MUST publish its result only if the generation is unchanged at publication. Callers that waited on that load MAY use an unpublished result.

DPT-SC5. `invalidate()` MUST increment the generation and then drop the published snapshot. After `invalidate()` returns, a later `get_or_load` call in the same process MUST NOT return a snapshot, or join a load, whose captured generation precedes that increment.

### 8.2 TTL

DPT-RC1. The constant `REQUEST_PATH_CACHE_TTL` MUST equal `1000 ms`. It is the TTL of the routing catalog (DPT-RC2), the model-price cache (DPT-RC5), and the group billing-ratio cache (DPT-RC8). A change committed by another process MUST become visible to this process no later than `1000 ms` after the start of the first read that follows the commit.

### 8.3 Routing catalog

DPT-RC2. `MonoizeRoutingStore` MUST own one `SnapshotCache` of the routing catalog. One catalog load MUST execute the `list_providers` reads (providers, channels, channel models: three statements). The catalog MUST retain only providers with `enabled = 1`, and in each provider only channels with `enabled = 1 AND weight > 0`. Providers MUST be ordered by `priority ASC, created_at ASC`, and channels by `created_at ASC`. The catalog MUST also hold the sorted, distinct set of model names of the retained channels.

DPT-RC3. These reads MUST be served from the catalog:
- `available_model_names(candidates)` MUST return the candidates that are in the catalog model set.
- `list_available_model_names()` MUST return the catalog model set in ascending order.
- `list_providers_for_model(model)` MUST return `Arc<[MonoizeProvider]>`. It contains each catalog provider that has at least one retained channel whose `models` contains `model`, in catalog order. Each returned provider MUST contain only those channels. The `models` map of each returned channel MUST contain only the `model` entry. The projection for one model MUST be built at most once per catalog snapshot and shared by `Arc`. Only models in the catalog model set are memoized; other models MUST return an empty slice.

`list_providers`, `get_provider`, `provider_count`, and `list_active_probe_candidates` MUST continue to read the database directly.

DPT-RC4. The routing catalog MUST be invalidated after the write commits and before the method returns success, for each of these writes to `monoize_providers`, `monoize_channels`, or `monoize_channel_models`:

| Write | Invalidation point |
|---|---|
| `MonoizeRoutingStore::create_provider` | after commit, before reading back the provider |
| `MonoizeRoutingStore::update_provider` | after commit, before reading back the provider |
| `MonoizeRoutingStore::delete_provider` | after the delete affects one row |
| `MonoizeRoutingStore::reorder_providers` | after commit |
| `MonoizeRoutingStore::remember_channel_websocket_supported` | after commit, when a row changed |
| `UserStore::delete_group` (GR-X3 rewrites `monoize_providers.group_ids`) | the admin group-delete handler MUST call `MonoizeRoutingStore::invalidate_routing_catalog()` after `delete_group` succeeds and before it responds |

Startup canonicalization in `MonoizeRoutingStore::new` completes before the store is shared. The cache is empty at that point.

### 8.4 Model-price cache

DPT-RC5. `ModelPriceStore` MUST cache, per `model_id`, the applicable row or its absence. A row is applicable when `enabled` is true and the row is complete (MP-R2, MP-R4). `list_applicable_by_model_ids(ids)` MUST return a map from `model_id` to `Arc<ModelPriceRecord>` that contains only applicable rows. It MUST serve each id whose entry has the current generation and whose read started no more than `1000 ms` before. It MUST fetch all other distinct ids with one `list_by_model_ids` call (MP-R8 chunking). It MUST capture the generation before that read and cache the results only under that generation. A served entry MUST have the current generation. The cache MUST hold at most `4096` entries. Before an insert at capacity, the cache MUST remove every entry that it cannot serve. If the cache is still full, the new entry MUST NOT be cached.

DPT-RC6. The model-price cache MUST be invalidated (generation increment, then clear) by `upsert`, `delete`, `bulk_upsert_synced`, and `delete_by_ids_with_source` before they return. This includes an error return after any statement ran, because chunked sync writes can commit partially. Price sync (MP-Y13 to MP-Y16) writes only through these methods.

DPT-RC7. The forwarding price snapshot (MP-R8) MUST use `list_applicable_by_model_ids`. `list`, `get`, and `list_by_model_ids` remain uncached for dashboard and active-probe callers.

### 8.5 Group billing-ratio cache

DPT-RC8. `UserStore` MUST own one `SnapshotCache` of `group_id -> billing_ratio` for every `monoize_groups` row. One load executes `SELECT id, billing_ratio FROM monoize_groups`. `group_billing_ratios()` MUST return the shared map. The forwarding path MUST read it only when at least one attempt has a billing group. `create_group`, `update_group`, `reorder_groups`, and `delete_group` MUST invalidate this cache after their write succeeds and before they return.

DPT-RC9. With warm caches, building attempts for a forwarded request MUST NOT execute SQL for model availability, providers, channels, channel models, model prices, or group billing ratios. A write through an owning store in this process MUST be visible to every request that starts after the write returns.

### 8.6 Spend preflight

DPT-BP1. `UserStore::ensure_user_can_spend(user_id)` MUST read the balance through `BalanceCache`. On a hit, it decides from the cached `UserBalance`. On a miss, it captures the generation, reads the `users` row once, calls `insert_if_current`, and decides from the value it read, even if publication failed. It MUST NOT repeat the read because the generation changed. The decision is unchanged (BP-C8): `balance_unlimited` admits; `balance_nano_usd > 0` admits; otherwise the result is `InsufficientBalance`. A missing row is `NotFound`.

DPT-BP2. `UserStore` MUST own a second `BalanceCache`, `sub_account_balance_cache`. Its key is `api_key_id`. Its TTL is 30 seconds, and its capacity follows DPT-BC3a. In an entry, `user_id` is the owning user, `balance_nano_usd` is `api_keys.sub_account_balance_nano`, and `balance_unlimited` is false. `ensure_sub_account_can_spend(api_key_id)` MUST follow DPT-BP1 with `SELECT user_id, sub_account_balance_nano FROM api_keys WHERE id = $1`. A balance `> 0` admits; otherwise the result is `InsufficientBalance`. A missing row is `NotFound`. `spawn_background_tasks` MUST start its eviction task with a 30-second interval on both roles.

DPT-BP3. These mutations MUST invalidate the listed caches after commit and before they return:

| Mutation | Invalidation |
|---|---|
| `charge_sub_account_balance_nano` (sub-account branch) | `sub_account_balance_cache.invalidate(api_key_id)` |
| `transfer_to_sub_account` | `sub_account_balance_cache.invalidate(api_key_id)` |
| `update_api_key` | `sub_account_balance_cache.invalidate(key_id)` |
| `batch_delete_api_keys` | `sub_account_balance_cache.invalidate(id)` for each deleted id |
| `delete_user` | `sub_account_balance_cache.invalidate_all()` |
| `apply_metering_batch` (primary apply of replica deltas) | `balance_cache.invalidate(user_id)`, and `sub_account_balance_cache.invalidate(api_key_id)` when the delta names a key, for every delta in the committed batch |

DPT-BP4. Replica preflight (`primary-replica-deployment.spec.md` M7) MUST keep reading persisted balances without these caches.
