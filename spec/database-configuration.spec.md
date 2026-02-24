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

DB7. On both SQLite pools, the following PRAGMAs MUST be executed at connection time:

- `PRAGMA journal_mode=WAL`
- `PRAGMA busy_timeout=5000`
- `PRAGMA foreign_keys=ON`

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

DB14. `stmt()` MUST convert `$N` placeholders to `?` when `backend == DbBackend::Sqlite`. The conversion replaces any `$` followed by one or more ASCII digits with a single `?`.

DB15. `stmt()` MUST pass SQL through unchanged when `backend == DbBackend::Postgres`.

## 6. Automatic Schema Migration

DB16. On startup, after `DbPool::connect()` succeeds, the application MUST run `Migrator::up(db.write(), None)` to apply all pending Sea ORM migrations.

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
- `monoize_provider_models`
- `monoize_channels`

DBT4. On startup, the server MUST also create legacy provider tables:

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

DBO2. `model_registry_records` is the persistent source of dashboard-managed model registry rows merged into in-memory registry at startup.

DBO2.1. `model_metadata_records` is the persistent source of per-model pricing/capability metadata used by billing and dashboard diagnostics.

DBO3. `monoize_providers`, `monoize_provider_models`, and `monoize_channels` are the primary source of truth for provider/channel routing configuration.

DBO3.1. `billing_ledger` is append-only request charge / admin adjustment history.

DBO4. Legacy provider tables MAY exist for compatibility and dashboard maintenance, but forwarding routing MUST NOT read them.

## 9. Store Initialization

DB18. All store constructors MUST accept `DbPool` and use `db.read()` for queries, `db.write()` for mutations.

DB19. Application initialization order:

1. `DbPool::connect(&runtime.database_dsn)`
2. `Migrator::up(db.write(), None)` — auto-migrate
3. Construct stores: `UserStore`, `SettingsStore`, `ProviderStore`, `MonoizeRoutingStore`, `ModelRegistryStore`

## 10. Cross-Backend SQL Compatibility

DB20. All SQL statements MUST be compatible with both SQLite and PostgreSQL. Specifically:

- Use `$N` placeholders (converted to `?` for SQLite by `stmt()`).
- Use `ON CONFLICT ... DO UPDATE SET col=excluded.col` for upserts (supported by both).
- Store dates as RFC 3339 TEXT strings.
- Store i128 nano-USD values as TEXT strings.
- Store booleans as INTEGER `0/1`.
- Use `TEXT`, `INTEGER`, `REAL`, `BLOB` logical types only.
