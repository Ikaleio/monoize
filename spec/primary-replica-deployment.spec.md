# Primary/Replica Deployment Specification

## 0. Status

- Product name: Monoize.
- Internal protocol name: `URP-Proto`.
- Scope: optional multi-node deployment consisting of exactly one node in role `primary` and zero or more nodes in role `replica`, sharing one PostgreSQL database. Also defines the node-local upstream proxy configuration that applies to every node regardless of role.
- Out of scope: automatic leader election, SQLite-backed replicas, multi-primary writes, cross-region replication, database-level replication tooling.
- Terminology: "primary" is the node role whose process is the only writer of Monoize business tables; "replica" is a node role whose process serves traffic without writing business tables. A deployment has exactly one primary; the assignment is manual (section 9).

## 1. Role selection and validation

PRP1. The node role MUST be resolved from the `MONOIZE_NODE_ROLE` environment variable. Accepted values are `primary` and `replica` (ASCII, case-sensitive). An absent or empty value MUST resolve to `primary`. Any other value MUST stop startup with error `node_role_invalid`.

PRP2. Role and all related settings are resolved once at startup and are immutable for the lifetime of the process. No runtime endpoint MAY change them.

PRP3. A replica node MUST reject a SQLite DSN (`sqlite://...`, `sqlite::memory:`) at startup with error `replica_requires_postgres`.

PRP4. A replica node MUST require `MONOIZE_PRIMARY_INTERNAL_URL`. The value MUST be a valid absolute `http://` or `https://` URL; otherwise startup MUST fail with error `replica_primary_url_required`.

PRP5. A replica node MUST require `MONOIZE_REPLICA_TOKEN`. An absent or empty value MUST stop startup with error `replica_token_required`. A configured value with fewer than 32 Unicode scalar values MUST stop startup with error `replica_token_too_short` on every node role.

PRP6. A primary node with a valid `MONOIZE_REPLICA_TOKEN` MUST mount the metering ingest endpoint defined in section 7. A primary without it runs in single-node compatibility mode: the ingest endpoint MUST NOT be mounted, and requests to its path MUST return 404.

PRP7. Tuning variables, each read once at startup:

| Variable | Default | Constraint | Error on violation |
|---|---|---|---|
| `MONOIZE_CONFIG_POLL_INTERVAL_SECONDS` | `5` | positive integer | `config_poll_interval_invalid` |
| `MONOIZE_METERING_SHIP_INTERVAL_SECONDS` | `10` | positive integer | `metering_ship_interval_invalid` |
| `MONOIZE_METERING_SHIP_BATCH_MAX_ENTRIES` | `500` | integer in `[1, 2000]` | `metering_batch_limit_invalid` |
| `MONOIZE_REPLICA_METERING_SPOOL_DIR` | `./data/replica-metering-spool` | filesystem path | n/a |
| `MONOIZE_REPLICA_METERING_SPOOL_MAX_BYTES` | `536870912` | positive integer | `metering_spool_quota_invalid` |

A malformed value outside the stated constraint MUST stop startup with the listed error code.

## 2. Node-local upstream proxy

PX1. Every node, regardless of role, MAY configure one outbound HTTP proxy via the `MONOIZE_UPSTREAM_PROXY_URL` environment variable. Primary and replica values are independent because the variable is process-local.

PX2. When `MONOIZE_UPSTREAM_PROXY_URL` is set to a non-empty value, it MUST be an absolute URL with scheme `http` or `https`. Any other scheme (including `socks5`) MUST stop startup with error `upstream_proxy_config_invalid`. An absent or empty value MUST mean direct connection.

PX3. When configured, the shared outbound client used for external upstream calls (LLM provider forwarding, models.dev catalog sync) on that node MUST route all its requests through this proxy.

PX4. Internal cluster traffic — the replica metering shipment of section 6 — MUST NOT use the configured upstream proxy. The client performing this traffic MUST be constructed to bypass both the configured proxy and environment-inherited proxies.

PX5. The upstream proxy configuration MUST NOT be stored in `system_settings` and MUST NOT be editable from the dashboard. Changing it requires a process restart.

PX6. Per-Channel egress proxy resolution: for one upstream request issued for Channel `c`, the effective proxy is `c.proxy_url` when it is a non-empty custom URL (channel-management.spec.md CP-INV-14), otherwise the node-level `MONOIZE_UPSTREAM_PROXY_URL`, otherwise direct connection. The same resolution applies to active-probe requests for that Channel. If a custom Channel `proxy_url` cannot be used to construct an HTTP client, that Channel's request (including an active-probe) MUST fail closed; it MUST NOT fall back to the node-global or direct client.

PX7. The application MUST cache one HTTP client per distinct effective proxy URL (including the direct case) instead of constructing a client per request. Cache entries are immutable after construction; channel `proxy_url` changes take effect by resolving a different cached client on the next request.

PX8. The metering shipment of section 6 always resolves to the no-proxy internal client regardless of any Channel `proxy_url`.

## 3. Startup behavior

### 3.1 Primary

PRP8. Startup on a primary MUST remain exactly as specified by `database-configuration.spec.md` DB16–DB19 and `unified_responses_proxy.spec.md` C-series: connect, run migrations, ensure defaults, construct stores, spawn background tasks.

PRP9. If the metering spool directory (PRP7) contains durable delta entries left over from an earlier replica life of this data directory, the primary MUST drain them through the idempotent apply routine of section 7 against the local database after migrations complete and before the listener accepts requests. A drain failure MUST stop startup with error `metering_drain_failed`. Request-log spool leftovers need no special handling because the normal flush path consumes them (DPT-RL4).

### 3.2 Replica

PRP10. A replica MUST NOT execute any migration. After `DbPool::connect()`, it MUST evaluate the same applied-versus-embedded version comparison as DB16a–DB16d in read-only form:

- outcome equivalent to DB16b's "migrations needed" state ⇒ startup MUST fail with error `replica_schema_pending`;
- outcome equivalent to DB16c acceptance (rollback binary ahead) ⇒ startup MAY continue;
- fully-applied state ⇒ continue.

PRP11. On a replica the following write-producing startup steps MUST NOT run: `SettingsStore` default insertion and transform-rule-id canonicalization writes, `ensure_active_probe_system_user`, session-expired cleanup deletion, request-log retention deletion, and the active-probe scheduler task.

PRP12. On a replica, `RequestLogBatcher` and `LastUsedBatcher` MUST keep identical buffering semantics, but their periodic flush MUST target the metering shipper (section 6) instead of a local database write. `ApiKeyCache` and `BalanceCache` eviction tasks run unchanged because they only evict in-memory entries.

PRP13. All other initialization order (stores, runtime snapshot build, router assembly, listener bind) MUST be identical between roles.

## 4. Configuration epoch and runtime refresh

E1. The persistent epoch is the single row of `state_records` with `tenant_id = 'monoize'`, `kind = 'config_epoch'`, `id = 'global'`. Its `value` is a base-10 unsigned 64-bit decimal text. A missing row MUST be read as epoch `0`.

E2. The primary MUST increment the epoch by exactly 1 inside the same database transaction that commits each of: a `SettingsStore::update_all` commit, and the pricing-profile-patterns point mutation. The increment MUST be one statement computing `value + 1` inside the transaction.

E3. A replica MUST poll the epoch with exactly one single-row, single-column `SELECT` (the epoch value only) every `MONOIZE_CONFIG_POLL_INTERVAL_SECONDS`. When the observed value differs from the last applied value, the replica MUST rebuild `MonoizeRuntimeConfig` from committed `system_settings` values using the same construction logic as primary publication, then swap it into `monoize_runtime` under the existing snapshot lock. The poll MUST fetch no other rows or columns, and the rebuild MUST run only when the epoch value changed. The rebuild itself performs reads only.

E4. A failed epoch poll (database error or unparseable stored value) MUST log at `warn` level, keep the previous snapshot, and retry on the next tick. It MUST NOT terminate the process. Idle replicas keep polling on the fixed interval; no traffic-adaptive backoff is permitted because it would make configuration propagation latency traffic-dependent.

E5. Provider/channel routing rows are not part of the epoch contract: replicas read them fresh from the shared database on demand, subject only to the existing cache TTLs.

## 5. Replica request surface

D1. A replica is an API-only node. It MUST NOT mount any `/api/dashboard/**` route and MUST NOT serve frontend static assets. Any request to `/api/dashboard/**` or to a non-API UI path MUST return HTTP 404 with JSON body code `replica_dashboard_disabled`.

D2. Forwarding endpoints (`/v1/**`), the metrics endpoint, and health paths MUST be served locally by the replica against the shared database's read path.

D3. Dashboard administration happens exclusively on the primary node.

## 6. Metering pipeline (replica → primary)

### 6.1 Data classes

M1. Three data classes ship from replica to primary:

1. request logs — the existing durable spool files produced by DPT-RL3*;
2. last-used updates — `{api_key_id, last_used_at}` pairs buffered by `LastUsedBatcher`;
3. balance deltas — billing charge events produced on the replica.

M2. A balance delta record is `{delta_id, kind, user_id, api_key_id?, amount_nano_usd, meta_json, created_at}` where:

- `delta_id` is one UUID v4 generated at enqueue time;
- `kind` is `request_charge` or `api_key_charge` (sub-account);
- `user_id` identifies the owning user; `api_key_id` is present iff `kind = api_key_charge`;
- `amount_nano_usd` is the charge magnitude as decimal signed-128 text;
- `created_at` is RFC 3339.

M3. Before the charge path reports success on a replica, the delta MUST be durably published as one JSON file in `MONOIZE_REPLICA_METERING_SPOOL_DIR` using temporary-file write followed by same-directory atomic rename. If publication fails or the combined spool size would exceed `MONOIZE_REPLICA_METERING_SPOOL_MAX_BYTES`, enqueue MUST fail and terminal billing finalization MUST treat the request as a billing failure consistent with MB-C6. A successful enqueue MUST also atomically add `amount_nano_usd` to the in-memory pending-deduction counter keyed by `user_id` (kind `request_charge`) or `api_key_id` (kind `api_key_charge`).

M3a. Replica startup MUST create `MONOIZE_REPLICA_METERING_SPOOL_DIR` if it is absent and MUST write then delete one probe file in that directory. A create, write, or permission failure MUST stop startup with error `metering_spool_unwritable`. A bind-mounted spool directory MUST be writable by the process user; a root-owned mount that the non-root process cannot write MUST fail this probe rather than accept traffic.

### 6.2 Ship loop

M4. The replica MUST run one ship loop that POSTs at most one JSON batch per iteration to `POST {MONOIZE_PRIMARY_INTERNAL_URL}/internal/replica/metering` with header `Authorization: Bearer {MONOIZE_REPLICA_TOKEN}`. The loop MUST iterate at least every `MONOIZE_METERING_SHIP_INTERVAL_SECONDS`. It MUST also iterate as soon as a request-log spool file is published or a balance delta is durably enqueued (M4b), coalescing wakes that arrive while a POST is in flight into the next iteration. The batch is composed as:

1. the oldest durable request-log spool files, at most `MONOIZE_METERING_SHIP_BATCH_MAX_ENTRIES`, discovered from on-disk `.json` files even when the in-memory buffer is empty (same discovery as `db-performance-tuning.spec.md` DPT-RL4 `load_spool_batch`);
2. pending deltas, at most `MONOIZE_METERING_SHIP_BATCH_MAX_ENTRIES`;
3. currently buffered last-used pairs, filling remaining capacity.

Total entries across the three arrays MUST be at most 2000 (I3). Entries that do not fit MUST remain buffered or spooled for the next tick. Every tick, including a tick whose three arrays are empty, MUST include a `replica` heartbeat object (M4a) and MUST POST.

M4b. Publishing a terminal request-log spool file or durably enqueuing a balance delta MUST wake the ship loop without waiting for the next interval tick. The ship loop remains the only POST issuer.

M4a. The `replica` heartbeat object is not counted toward the 2000-entry cap. Its schema is:

```json
{
  "id": "uuid v4 string, resolved per M9; stable across process restarts of one replica deployment",
  "hostname": "string",
  "listen": "the replica MONOIZE_LISTEN value",
  "version": "CARGO_PKG_VERSION",
  "started_at": "RFC 3339 process start",
  "uptime_seconds": 0,
  "spool_pending_count": 0,
  "spool_pending_bytes": 0
}
```

A primary that authenticates an ingest request containing `replica` MUST upsert that replica into a process-local heartbeat map keyed by `id`, recording `last_seen_at = now`, before applying the batch. Heartbeat recording MUST NOT require a non-empty data array. The map is not persisted across primary restarts. Each time the map is read for the admin overview response, every entry with `now - last_seen_at > 360 * MONOIZE_METERING_SHIP_INTERVAL_SECONDS` MUST first be removed from the map; removed entries MUST NOT appear in that response.

M5. Spool files, buffered last-used pairs, pending deltas, and their pending-deduction counters MUST only be released after an HTTP 200 response. Any non-200 response or transport error MUST retain everything unchanged for the next tick. After 3 consecutive failed ticks the replica MUST log a `warn` naming the consecutive-failure count, repeated per subsequent failure. A heartbeat-only tick that receives HTTP 200 is a successful tick.

M6. At graceful shutdown the replica MUST make one best-effort final ship attempt; leftover data persists on disk and ships after restart.

### 6.3 Replica-side balance preflight

M7. Balance preflight on a replica MUST compute `effective_balance = persisted_balance - pending_deductions[subject]` where `persisted_balance` comes from the existing cache/read path and `subject` follows M2 keying. The insufficient-balance decision and HTTP mapping (402 `insufficient_balance`) MUST match `ensure_user_can_spend` / `ensure_sub_account_can_spend`. Unlimited balances bypass subtraction.

M8. Because preflight subtracts locally unshipped charges, overspend during a primary outage is bounded by in-flight concurrency, not by shipment delay.

### 6.4 Replica identity

M9. A replica MUST resolve exactly one identity UUID at startup, before the first ship-loop tick, in this order:

1. When `MONOIZE_REPLICA_ID` is set to a non-empty value, the value MUST parse as a UUID whose RFC 4122 version field equals 4 (hyphenated or simple hex form; case-insensitive). On parse success, the canonical lowercase hyphenated form is the identity; the identity file of step 2 is neither read nor written. On parse failure or version mismatch, startup MUST stop with error `replica_id_invalid`.
2. Otherwise the replica MUST read the identity file `{MONOIZE_REPLICA_METERING_SPOOL_DIR}/replica-identity`. When the file exists and its whitespace-trimmed content parses as a UUID with version 4, the canonical lowercase hyphenated form is the identity and the file is left unchanged.
3. Otherwise (file absent, unreadable, or content not a version-4 UUID) the replica MUST generate one new UUID v4, persist it to the identity file by writing a temporary file in the same directory, fsyncing it, then atomically renaming it onto `replica-identity`, and use the generated value as the identity. A directory-create, write, sync, or rename failure MUST stop startup with error `replica_identity_unwritable`.

M9a. The identity file content written by M9 step 3 is exactly the 36-character lowercase hyphenated UUID followed by one `\n` (37 bytes). Readers tolerate surrounding ASCII whitespace per M9 step 2.

M9b. The spool-directory startup cleanup (the M3a construction path that deletes non-`.json` leftovers) MUST NOT delete a file named `replica-identity`.

M9c. The M4a heartbeat `id` MUST equal the resolved identity. Consequently the `id` is stable across process restarts for one replica data directory (or for one `MONOIZE_REPLICA_ID` value), and a restarted replica upserts its existing heartbeat map entry on the primary instead of creating an additional entry.

## 7. Metering ingest API (primary)

I1. Route: `POST /internal/replica/metering`, mounted iff PRP6 conditions hold. It is outside dashboard-session auth.

I2. Authentication: bearer token compared by SHA-256 digest equality (constant-time comparison). Mismatch MUST return HTTP 401 code `replica_auth_failed`.

I3. Body schema:

```json
{
  "replica": { "id": "...", "hostname": "...", "listen": "...", "version": "...", "started_at": "...", "uptime_seconds": 0, "spool_pending_count": 0, "spool_pending_bytes": 0 },
  "request_logs": ["SpoolRequestLog objects per DPT-RL3"],
  "last_used": [{"api_key_id": "...", "last_used_at": "RFC 3339"}],
  "balance_deltas": [one object per M2]
}
```

All three arrays MAY be empty. `replica` MAY be omitted by older replicas; when present it MUST be recorded per M4a and MUST NOT count toward the entry cap. If total entries across the three arrays exceed 2000, the endpoint MUST return HTTP 413 code `metering_batch_too_large` without partial apply. Any per-entry schema violation MUST return HTTP 422 code `metering_batch_invalid` without partial apply.

I4. The entire batch MUST apply inside one database transaction:

1. request logs via the existing chunked multi-row insert with `ON CONFLICT(id) DO NOTHING` (chunk rules of DPT-RL4);
2. last-used via the existing bulk `UPDATE ... CASE` statement;
3. each balance delta via one `INSERT INTO billing_ledger (..., idempotency_key, ...) VALUES (...) ON CONFLICT(idempotency_key) DO NOTHING` with `idempotency_key = delta_id`, then, iff that statement inserted one row, the balance update of I5.

Commit MUST precede the HTTP 200 response. The response body MUST be `{"applied_request_logs": N, "applied_last_used": N, "applied_balance_deltas": N}` counting actually-inserted rows and accepted pairs. Any transaction error MUST roll back every statement of the batch and return HTTP 500 code `metering_apply_failed`; the replica retains and retries the batch unchanged.

I4a. After a successful ingest commit whose batch contained one or more request logs, the primary MUST broadcast those request-log entries on the process-local request-log SSE stream used by `request-logs.spec.md` RL1c-0. Dashboard clients MUST observe replica-originated terminal rows through that stream without waiting for a later list fetch. Name snapshots on that broadcast MAY be empty; the next `GET` list query still JOINs names per `request-logs.spec.md` section 1.2.

I5. Balance update per newly-inserted delta:

- kind `request_charge`: decrement `users.balance_nano_usd` by `amount_nano_usd`, allowing a negative result; an unlimited owner MUST receive no balance update while the delta still counts as applied;
- kind `api_key_charge`: decrement `api_keys.sub_account_balance_nano` for a sub-account-enabled key, allowing negative; if the key is not sub-account-enabled, the update falls back to the owning user row exactly like `charge_sub_account_balance_nano`.

Delta application MUST NOT fail due to insufficient funds; synchronous overdraft rejection belongs exclusively to the replica preflight (M7).

I6. Idempotency window is permanent: `billing_ledger.idempotency_key` values persist under DBO3.1 retention. Replaying an already-applied batch MUST change nothing and return the same success shape.

## 8. Schema change

SC1. Migration `m20260823_000033_billing_ledger_delta_dedupe` MUST add nullable TEXT column `idempotency_key` to `billing_ledger` plus one partial unique index over it restricted to rows where it is not null, identically on SQLite and PostgreSQL. Pre-existing rows keep NULL. Writers other than ingest apply leave the column NULL. The down migration MUST drop the index then the column.

SC2. `state_records` gains no schema change; the config epoch row (E1) is created lazily by the first settings mutation.

SC3. Migration `m20260823_000034_channel_egress_proxy` MUST add nullable TEXT column `proxy_url` to `monoize_channels`, defaulting to NULL (follow-global) for all existing rows, identically on SQLite and PostgreSQL. The down migration MUST drop the column.

## 9. Manual failover

F1. Promotion = stop the replica process, set `MONOIZE_NODE_ROLE=primary`, start. PRP9 drains leftover deltas before the listener accepts requests; the node then operates as the sole writer.

F2. Demotion = stop the primary process, set `MONOIZE_NODE_ROLE=replica` plus the PRP4/PRP5 variables, start; PRP10 gates startup on schema currency.

F3. While the primary is unavailable, replicas MUST continue serving `/v1/**` traffic; charges accumulate durably (M3) and preflight follows M7–M8.

## 10. Observability

O1. Every replica MUST export Prometheus counter `monoize_replica_metering_shipped_total{result="ok"|"error"}` and gauge `monoize_replica_metering_pending_entries`. The primary MUST export counter `monoize_primary_metering_applied_total`.

## 11. Cross-specification revisions

XR1. `unified_responses_proxy.spec.md` C6 writer exclusivity applies to the primary role; replicas are non-writing processes whose telemetry reaches business tables only through section 7.

XR2. `database-configuration.spec.md` DB16 runs on the primary role only; replicas follow PRP10 read-only verification. DB23b publication happens on the primary; replicas obtain equivalent snapshots via E3.

XR3. `db-performance-tuning.spec.md` DPT-LU3/DPT-LU6 flush-to-database behavior and DPT-RL4 flush-to-database behavior apply to the primary role; on replicas they are replaced by M4–M5 with buffering semantics preserved (PRP12).

XR4. `user-billing-and-model-metadata.spec.md` LC5 single-attempt semantics apply to the primary synchronous charge path; the replica charge path is enqueue-or-fail (M3) without retry loops inside the request lifecycle.

## 12. Test matrix

T1. Config validation: each error code in PRP1/PRP3–PRP7/PX2 has one unit test asserting the exact code.

T1a. Delta spool construction: a writable directory accepts `DeltaSpool::new`; a directory the process cannot write MUST return an error whose text begins with `metering_spool_unwritable`.

T2. Ingest idempotency (SQLite in-memory): replaying one batch twice yields exactly one ledger row per delta, one net balance effect, identical response counts; duplicate `request_logs` ids are no-ops; last-used keeps the later timestamp.

T3. Ingest semantics: unlimited owner skips balance update but counts applied; sub-account delta updates `sub_account_balance_nano`; negative result allowed; batch >2000 returns 413 without partial state.

T4. Shipper against a mock primary: HTTP 200 deletes shipped spool files and clears buffers/counters; HTTP 500 retains everything; transport error retains everything; consecutive-failure warn appears at the third failure.

T4a. Request-log shipment discovers on-disk `.json` spool files even when the in-memory buffer is empty and deletes them only after the sink reports success.

T5. Epoch: primary mutation increments epoch within its transaction; replica poll observes change and swaps snapshot; failed poll keeps prior snapshot.

T6. Replica surface: `/api/dashboard/**` and `/` return 404 `replica_dashboard_disabled` on a replica; `/v1/**` and `/metrics` are served locally; no dashboard route exists in the router. `/metrics` MUST enforce `security-access-control.spec.md` SAC-1 through SAC-5 instead of returning `replica_dashboard_disabled`.

T7. Promotion drain: a data directory with leftover delta spool entries started as primary applies them before serving and then serves with empty spool.

T8. PostgreSQL parity: SC1 migration and T2/T3 scenarios run against `MONOIZE_TEST_POSTGRES_DSN` when provided and skip otherwise (DB-T1 rules).

T9. Replica identity (M9): first resolution in an empty spool directory creates `replica-identity` containing one version-4 UUID plus `\n`; a second resolution over the same directory returns the identical identity; `DeltaSpool` construction over the same directory preserves the file and a subsequent resolution still returns the identical identity; a corrupt identity file is replaced by a newly generated identity; a valid `MONOIZE_REPLICA_ID` yields its canonical lowercase hyphenated form without creating the file; a non-UUID or non-version-4 `MONOIZE_REPLICA_ID` yields an error whose text begins with `replica_id_invalid`.

T10. Heartbeat eviction (M4a): with ship interval `s`, a map entry with `now - last_seen_at > 360 * s` is removed by the overview read path while an entry with `now - last_seen_at <= 360 * s` is retained.
