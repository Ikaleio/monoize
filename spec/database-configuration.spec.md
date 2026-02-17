# Database Storage (Dashboard + Routing) Specification

## 0. Status

- Product name: Monoize.
- Internal protocol name: `URP-Proto`.
- Scope: SQLite schema and startup-time table initialization.

## 1. Configuration

DB1. The server MUST resolve database DSN by precedence:

1. `MONOIZE_DATABASE_DSN` environment variable, if set and non-empty.
2. `DATABASE_URL` environment variable, if set and non-empty.
3. default DSN (`DB2`).

DB2. The default DSN MUST be `sqlite://./data/monoize.db`.

DB3. If the DSN is a SQLite file path (starts with `sqlite://`, not memory mode), startup MUST create missing parent directories and the database file before opening the connection.

## 2. Required Tables

DBT1. On startup, the server MUST ensure the following dashboard/auth tables exist:

- `users`
- `sessions`
- `api_keys`
- `billing_ledger`

DBT2. On startup, the server MUST ensure the following model-registry table exists:

- `model_registry_records`
- `model_metadata_records`

DBT3. On startup, the server MUST ensure the following Monoize routing tables exist:

- `monoize_providers`
- `monoize_provider_models`
- `monoize_channels`

DBT4. On startup, the server MUST also keep legacy provider tables available for compatibility paths:

- `providers`
- `model_mappings`
- `group_members`

## 3. Ownership

DBO1. `users`, `sessions`, and `api_keys` are the source of truth for dashboard user/session/token state.

DBO1.1. `users` MUST include billing fields:

- `balance_nano_usd` (`TEXT`, default `"0"`)
- `balance_unlimited` (`INTEGER`, default `0`)

DBO2. `model_registry_records` is the persistent source of dashboard-managed model registry rows merged into in-memory registry at startup.

DBO2.1. `model_metadata_records` is the persistent source of per-model pricing/capability metadata used by billing and dashboard diagnostics.

DBO3. `monoize_providers`, `monoize_provider_models`, and `monoize_channels` are the primary source of truth for provider/channel routing configuration.

DBO3.1. `billing_ledger` is append-only request charge / admin adjustment history.

DBO4. Legacy provider tables MAY exist for compatibility and dashboard maintenance, but forwarding routing MUST NOT read them.
