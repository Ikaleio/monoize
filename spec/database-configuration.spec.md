# Database Storage (Dashboard + Routing) Specification

## 0. Status

- Product name: Monoize.
- Internal protocol name: `URP-Proto`.
- Scope: Multi-backend database abstraction via Sea ORM, supporting SQLite and PostgreSQL.

## 1. Configuration

DB1. The server MUST resolve database DSN by precedence:

1. `MONOIZE_DATABASE_DSN` environment variable, if set and non-empty.
2. `DATABASE_URL` environment variable, if set and non-empty.
3. default DSN (`DB2`).

DB2. The default DSN MUST be `sqlite://./data/monoize.db`.

DB3. If the DSN is a SQLite file path (starts with `sqlite://`, not memory mode), startup MUST create missing parent directories and the database file before opening the connection.

## 2. Supported Backends

DB4. The DSN scheme determines the backend:

- `sqlite://...` or `sqlite::memory:` → SQLite backend.
- `postgres://...` or `postgresql://...` → PostgreSQL backend.
- Any other scheme MUST be rejected with error `unsupported database DSN scheme: {dsn}`.

DB4.1. `sqlite::memory:` and `sqlite://...` DSNs containing `:memory:` or `mode=memory` MUST be treated as SQLite in-memory mode and MUST NOT trigger filesystem directory/file creation.

DB5. Backend selection is determined at startup and is immutable for the lifetime of the process.

## 3. Connection Pool Architecture

### 3.1 SQLite

DB6. SQLite MUST use a split read/write pool architecture:

- Write pool: exactly 1 connection (`max_connections=1`), enforcing single-writer semantics.
- Read pool: 10 connections (`max_connections=10`).
- Both pools: `acquire_timeout=10s`, `connect_timeout=5s`, `sqlx_logging=false`.

DB7. Every physical SQLite connection in both pools MUST execute the following PRAGMAs as part of establishing that connection, including connections that the pool opens after startup:

- `PRAGMA journal_mode=WAL`
- `PRAGMA synchronous=NORMAL`
- `PRAGMA busy_timeout=15000`
- `PRAGMA foreign_keys=ON`
- `PRAGMA cache_size=-65536` (64 MB page cache)
- `PRAGMA mmap_size=268435456` (256 MB memory-mapped I/O)

The implementation MUST configure these values through per-connection SQLx/SeaORM connection options. Executing each PRAGMA once through a pooled `DatabaseConnection` after pool creation is insufficient because it configures only the physical connection selected for that statement. For an in-memory SQLite database, SQLite may report the effective `journal_mode` as `memory`; all connection-local PRAGMAs remain required.

DB8. If the SQLite DSN does not contain a `?` query string, `?mode=rwc` MUST be appended.

### 3.2 PostgreSQL

DB9. PostgreSQL MUST use a single connection pool shared for both reads and writes:

- `max_connections=20`, `acquire_timeout=10s`, `connect_timeout=5s`, `sqlx_logging=false`.

DB10. The same `DatabaseConnection` instance is returned for both `read()` and `write()` accessors.

## 4. DbPool Interface

DB11. `DbPool` MUST expose the following public interface:

- `connect(dsn: &str) -> Result<Self, DbErr>`: Construct from DSN string.
- `read() -> &DatabaseConnection`: Connection for SELECT queries.
- `write() -> &DatabaseConnection`: Connection for INSERT/UPDATE/DELETE/DDL.
- `backend() -> DbBackend`: Returns `DbBackend::Sqlite` or `DbBackend::Postgres`.
- `is_sqlite() -> bool`: True iff backend is SQLite.
- `is_postgres() -> bool`: True iff backend is PostgreSQL.
- `stmt(sql: &str, values: Vec<Value>) -> Statement`: Build a statement with automatic placeholder conversion.

DB12. `DbPool` MUST implement `Clone` (all connections are `Arc`-backed internally by Sea ORM).

## 5. SQL Placeholder Conversion

DB13. All application SQL MUST be written with PostgreSQL-style `$1, $2, ...` placeholders.

DB14. `stmt()` MUST convert each PostgreSQL-style `$N` placeholder to the SQLite numbered placeholder `?N` when `backend == DbBackend::Sqlite`. The decimal digits in `N` MUST be preserved. Repeated occurrences of `$N` MUST continue to address one bind value, and placeholders written out of numeric order MUST retain their original indices. For example, `$2 || $1 || $2` MUST become `?2 || ?1 || ?2` and MUST bind a two-element value vector without adding a third value.

DB15. `stmt()` MUST pass SQL through unchanged when `backend == DbBackend::Postgres`.

## 6. Automatic Schema Migration

DB16. On startup, when the node role is `primary` (`primary-replica-deployment.spec.md` PRP1), after `DbPool::connect()` succeeds, the application MUST acquire the database write guard and call `run_startup_migrations(&*write_guard)`. When the node role is `replica`, no migration MUST run and startup follows the read-only verification in `primary-replica-deployment.spec.md` PRP10.

DB16a. `run_startup_migrations` MUST compare the complete set of applied version names in `seaql_migrations` with the ordered version names returned by the embedded `Migrator::migrations()` list.

DB16b. If every applied version is embedded in the current binary, `run_startup_migrations` MUST run `Migrator::up(db, None)` on the connection passed to the wrapper and propagate its result. This includes a new database and a database with pending embedded migrations.

DB16c. If one or more applied versions are not embedded in the current binary, `run_startup_migrations` MUST return success without running `Migrator::up` if and only if both conditions are true:

1. every embedded version is present in the applied-version set;
2. every non-embedded applied version compares lexicographically greater than the last embedded version.

When this exception is used, the application MUST emit a warning that identifies the last embedded version and every non-embedded applied version. This exception permits a rollback binary to start after a newer binary applied only strictly later migrations; it does not reverse or re-run a migration.

DB16d. `run_startup_migrations` MUST return `DbErr` without running `Migrator::up` when a non-embedded applied version exists and either an embedded version is not applied or a non-embedded version is lexicographically less than or equal to the last embedded version. An empty, duplicate, or non-strictly-ordered embedded migration list MUST also return `DbErr`. Application startup MUST map this error to `database_migration_failed` and stop initialization.

DB17. The migration system is defined in `src/migration/` per the `initial-seaorm-migration.spec.md`.

## 7. Required Tables

DBT1. On startup, the server MUST ensure the following dashboard/auth tables exist:

- `users`
- `sessions`
- `api_keys`
- `billing_ledger`

DBT2. On startup, the server MUST ensure the following model-registry tables exist:

- `model_registry_records`
- `model_metadata_records`

DBT3. On startup, the server MUST ensure the following Monoize routing tables exist:

- `monoize_providers`
- `monoize_channels`
- `monoize_channel_models`

DBT4. On startup, the server MUST NOT create legacy provider tables:

- `providers`
- `model_mappings`
- `group_members`

DBT5. On startup, the server MUST also create utility tables:

- `request_logs`
- `system_settings`
- `state_records`
- `file_bytes`

## 8. Ownership

DBO1. `users`, `sessions`, and `api_keys` are the source of truth for dashboard user/session/token state.

DBO1.1. `users` MUST include billing fields:

- `balance_nano_usd` (`TEXT`, default `"0"`)
- `balance_unlimited` (`INTEGER`, default `0`)

DBO1.2. Every persisted user or API-key balance MUST be parsed as a signed `i128` before it is returned or used. A missing or malformed balance MUST return an explicit internal storage error. Dashboard reads MUST NOT silently replace malformed balance data with zero.

DBO2. `model_registry_records` is the authoritative source of dashboard-managed model registry rows. Startup MUST NOT enumerate it to construct an in-memory full-table mirror.

DBO2.1. `model_metadata_records` is the persistent source of per-model pricing/capability metadata used by billing and dashboard diagnostics.

DBO3. `monoize_providers`, `monoize_channels`, and `monoize_channel_models` are the primary source of truth for provider/channel routing configuration.

DBO3.0. `monoize_provider_models` MUST NOT exist after migration. Channel model rows are the only persistent owner of logical model, redirect, and multiplier configuration.

DBO3.1. `billing_ledger` is append-only request charge / admin adjustment history. A ledger row MUST remain after its referenced user or API key is deleted. `billing_ledger.user_id` stores the historical user identifier and MUST NOT have a cascading foreign key to `users`.

DBO4. Legacy provider tables MUST NOT exist after the Provider/Channel model-routing migration completes.

## 9. Store Initialization

DB18. All store constructors MUST accept `DbPool` and use `db.read()` for queries, `db.write()` for mutations.

DB19. Application initialization order:

1. `DbPool::connect(&runtime.database_dsn)`
2. Acquire `db.write()` and call `run_startup_migrations(&*write_guard)` — apply embedded migrations or accept only the DB16c forward-compatible rollback state
3. Construct stores: `UserStore`, `SettingsStore`, `MonoizeRoutingStore`, `ModelRegistryStore`

## 10. Cross-Backend SQL Compatibility

DB20. All SQL statements MUST be compatible with both SQLite and PostgreSQL. Specifically:

- Use `$N` placeholders (converted to `?` for SQLite by `stmt()`).
- Use `ON CONFLICT ... DO UPDATE SET col=excluded.col` for upserts (supported by both).
- Store dates as RFC 3339 TEXT strings.
- Store i128 nano-USD values as TEXT strings.
- Store booleans as INTEGER `0/1`.
- Logical integer fields decoded into Rust `i64`, including request-log token/timing counters and model-metadata maximum-token counters, MUST map to SQLite `INTEGER` and PostgreSQL `BIGINT`. PostgreSQL `INTEGER`/`INT4` MUST NOT back a Rust `i64` field.
- Use `TEXT`, `INTEGER`, `REAL`, `BLOB` logical types only. Logical `REAL` MUST map to SQLite `REAL` and PostgreSQL `DOUBLE PRECISION`; PostgreSQL `REAL`/`FLOAT4` MUST NOT back a Rust `f64` field.

DB21. Request-log storage MUST use a single canonical table schema across SQLite and PostgreSQL. PostgreSQL-specific shadow columns for type-specialized mirrors are forbidden. If an older PostgreSQL database still contains such shadow columns, migrations MUST remove them while preserving canonical data.

### 10.1 Backend parity tests

DB-T1. SQLite database-semantic tests MUST run without external services. PostgreSQL parity tests MUST use `MONOIZE_TEST_POSTGRES_DSN` when that environment variable is present and non-empty, and MUST skip without failure when it is absent. PostgreSQL parity tests MUST isolate their fixtures in a transaction or temporary tables and MUST NOT mutate persistent application tables.

## 11. Settings Mutation Ordering

DB22. The process MUST serialize every dashboard settings mutation, including the dedicated `pricing_profile_model_patterns` endpoint, from its initial read or validation through database writes and publication to `monoize_runtime`, using one process-local settings-update lock.

DB23. A settings update MUST read its base state after every earlier settings update in the same process has published its runtime snapshot. An earlier update's snapshot MUST NOT overwrite a later update's snapshot.

DB23a. One dashboard settings update MUST write all changed `system_settings` rows in one database transaction. If any row write or commit fails, no row from that update may remain committed and `monoize_runtime` MUST remain unchanged.

DB23b. After the transaction commits, Monoize MUST construct and publish `monoize_runtime` from the committed values before returning success. The supported writer model is one `primary`-role process per deployment, defined by `unified_responses_proxy.spec.md` C6; replicas obtain equivalent snapshots via `primary-replica-deployment.spec.md` E3.

DB23c. `monoize_runtime` MUST contain the committed `reasoning_suffix_map`, `pricing_profile_model_patterns`, and `codex_model_ids`. A forwarding request MUST clone the suffix and pricing values from one runtime read and MUST NOT query `system_settings` for either value. `GET /v1/models` MUST clone `codex_model_ids` from the runtime snapshot and MUST NOT load all settings. The dedicated pricing-pattern mutation MUST publish its committed pattern list before returning success.

DB23d. Authentication code that needs only `session_ttl_days`, and API-key creation code that needs only `api_key_max_per_user`, MUST execute one point lookup for that key. It MUST NOT call `get_all()` or parse unrelated setting payloads. A missing, malformed, or non-positive point value MUST resolve to `7` and `1000`, respectively. A database error from either point lookup MUST return HTTP `500`; authentication MUST NOT create a session and API-key creation MUST NOT insert a key after that error.

DB23e. `GET /api/dashboard/settings/public` MUST execute one set-based query restricted to `registration_enabled`, `captcha_enabled`, `site_name`, `site_description`, and `api_base_url`. It MUST return defaults for any missing row and MUST NOT load or parse unrelated settings rows. If a persisted `registration_enabled` or `captcha_enabled` row exists, its value MUST be exactly the boolean text `true` or `false`; any other value MUST return a storage error and MUST NOT be interpreted as enabled.

DB23f. `GET /api/dashboard/stats` MUST obtain `my_api_keys_count` with `COUNT(*) WHERE user_id = ?`. It MUST NOT load or deserialize the user's API-key rows to compute the count.

DB23g. `PUT /api/dashboard/settings` MUST reject `session_ttl_days <= 0` and `api_key_max_per_user <= 0` with HTTP `400` and code `invalid_request`. A committed value for either setting MUST be a positive signed integer.

DB23h. `POST /api/dashboard/auth/logout` MUST return success only after the current session delete commits. A session-delete database error MUST return HTTP `500`; the endpoint MUST NOT report a successful logout for a session that may remain valid.

DB24. `system_settings` MUST persist `monoize_affinity_enabled`, `monoize_affinity_idle_ttl_seconds`, `monoize_affinity_failback_mode`, and `monoize_affinity_failback_delay_seconds`. Missing rows MUST resolve to `true`, `1800`, `"sticky"`, and `300`, respectively.

DB24i. `system_settings` MUST persist `monoize_mask_sensitive_info`. A missing row MUST resolve to `true`. `monoize_runtime.mask_sensitive_info` MUST equal the committed value after every settings publication (DB23b).

DB24a. `system_settings` MUST persist `codex_model_ids` as a JSON array of strings. A missing, invalid, or non-array value MUST resolve to `[]`. Writes MUST use the canonical ordered array defined by `spec/unified_responses_proxy.spec.md` DMO3a.

DB24b. `system_settings` MUST persist `global_model_redirects` as a JSON array
of `ModelRedirectRule` objects defined by `spec/api-key-model-redirects.spec.md`.
A missing, invalid, or non-array value MUST resolve to `[]`.

DB25. `monoize_channels` MUST persist nullable `affinity_enabled_override`, `affinity_idle_ttl_seconds_override`, `affinity_failback_mode_override`, and `affinity_failback_delay_seconds_override` columns.
