# Admin Dashboard Spec

## Scope

This spec defines the admin-only system dashboard: the backend endpoint
`GET /api/dashboard/admin/overview` and the frontend page `/dashboard/admin`.
The page presents system status, user usage ranking, model/channel health, and
replica (从机) status when applicable.

## Backend

AD-1. `GET /api/dashboard/admin/overview` MUST require an authenticated
dashboard admin session (`session_helpers::require_admin`). Non-admin requests
MUST be rejected per the shared admin-session policy.

AD-2. The response MUST be a JSON object with exactly these top-level fields (`node`, `replica`, `system`, `today`, `users_ranking`, `channel_health`):

- `node`: object:
  - `role`: `"primary"` or `"replica"` (from the runtime node role);
  - `version`: the compiled package version string (`CARGO_PKG_VERSION`);
  - `started_at`: RFC 3339 process start timestamp (captured once at startup);
  - `uptime_seconds`: integer seconds elapsed since process start;
  - `listen`: the configured listen address;
  - `metrics_path`: the configured metrics path;
  - `database_backend`: `"sqlite"` or `"postgres"`;
  - `database_dsn_redacted`: the database DSN with credentials redacted
    (same redaction as the existing config-overview endpoint);
  - `upstream_proxy_url`: the node-global egress proxy URL, or null.
- `replica`: object:
  - `ingest_enabled`: boolean; true when the node is a primary configured with
    a replica token (`metering_token_digest.is_some()`);
  - `spool_pending_count`: integer; 0 on primaries; on replicas the number of
    unsent durable metering spool files, when the replica metering pipeline is
    present;
  - `spool_pending_bytes`: integer; 0 on primaries; on replicas the total byte
    size of unsent durable metering spool files, when the replica metering
    pipeline is present.
  - `replicas`: array of objects, one per replica that has sent at least one
    heartbeat to this primary and has not been evicted per
    `primary-replica-deployment.spec.md` M4a (entries older than
    `360 * MONOIZE_METERING_SHIP_INTERVAL_SECONDS` are removed on read;
    the array is empty on replica nodes and on primaries with ingest
    disabled). Each object:
    - `id`: string, the stable replica deployment identity
      (`primary-replica-deployment.spec.md` M9);
    - `hostname`: string;
    - `listen`: string, the replica listen address;
    - `version`: string;
    - `started_at`: RFC 3339;
    - `last_seen_at`: RFC 3339 of the most recent heartbeat;
    - `uptime_seconds`: integer;
    - `spool_pending_count`: integer;
    - `spool_pending_bytes`: integer;
    - `stale`: boolean; true when `now - last_seen_at` is greater than
      `3 * MONOIZE_METERING_SHIP_INTERVAL_SECONDS`.
- `today`: object:
  - `calls`: integer COUNT of request-log rows with
    `created_at_unix_ms >= UTC calendar-day start` (same instant as
    `GET /api/dashboard/analytics` `today_calls`);
  - `cost_nano_usd`: nano-dollar integer string SUM of canonical in-range
    `charge_nano_usd` (same aggregation as analytics `today_cost_nano_usd`).
- `system`: object:
  - `pending_request_logs`: integer count of in-memory pending request-log
    snapshots;
  - `sse_connections`: integer count of active request-log SSE connections
    (sum of per-session counters);
  - `channel_health_entries`: integer count of tracked channel health states;
  - `channel_affinity_entries`: integer count of channel affinity bindings;
  - `routing_config_revision`: unsigned 64-bit integer as a decimal string.
- `users_ranking`: array of at most 20 objects ordered by
  `cost_nano_usd DESC`, then `call_count DESC`, then `username ASC`:
  - `user_id`: string;
  - `username`: string or null;
  - `call_count`: integer;
  - `cost_nano_usd`: nano-dollar integer string.
  The aggregation window MUST be the last 24 hours ending now, computed from
  `request_logs.created_at_unix_ms >= now - 24h` and only over rows whose
  `created_at_unix_ms` is not null. Charge decoding MUST follow the existing
  analytics aggregate rules (RL-analytics).
- `channel_health`: array of objects, one per channel known to the routing
  store, ordered by provider priority ascending then channel name ascending:
  - `provider_id`, `provider_name`, `channel_id`, `channel_name`: strings;
  - `enabled`: boolean;
  - `weight`: integer;
  - `session_affinity_auto`: boolean;
  - `healthy`: boolean (true when no health state is tracked for the channel
    id or any `{channel_id}::{model}` key);
  - `last_success_at`: unix-milliseconds integer or null;
  - `cooldown_until`: unix-milliseconds integer or null;
  - `probe_success_count`: integer;
  - `last_probe_at`: unix-milliseconds integer or null;
  - `unhealthy_models`: array of model id strings whose per-model health key
    is unhealthy or in cooldown; empty when `per_model_circuit_break` is
    false or every model key is healthy;
  - `today_calls`: integer COUNT of today's request-log rows for this
    `channel_id` (UTC calendar-day start, same window as `today`);
  - `today_cost_nano_usd`: nano-dollar integer string SUM of those rows'
    canonical `charge_nano_usd`.

AD-3. The endpoint MUST NOT expose credentials: no channel API keys, no
provider API keys, no database passwords, no replica tokens.

AD-4. The endpoint MUST return HTTP 200 with the full object even when some
subsystems are absent (e.g. no replica metering pipeline, no channels): absent
collections MUST be empty arrays and absent counts MUST be zero.

AD-5. The user usage ranking query MUST aggregate per user in SQL
(`GROUP BY rl.user_id`) with a `LIMIT` of 20 and MUST join the users table for
usernames. It MUST NOT load raw request-log rows into application memory.

## Frontend

ADF-1. `/dashboard/admin` MUST be reachable only through a nav item rendered
exclusively for admin-role sessions (same role predicate as the existing admin
nav items). Direct navigation by a non-admin MUST show an unauthorized/empty
state without calling the admin endpoint.

ADF-2. The page MUST render four sections in order: System status, User usage
ranking, Model/channel health, and Replica status. Each section MUST be a
card.

ADF-3. System status card MUST show: node role, version, uptime (humanized,
e.g. `2d 4h 12m`), listen address, metrics path, database backend, redacted
DSN, and upstream proxy presence.

ADF-4. User usage ranking card MUST render a table of at most 20 rows with
columns: rank, username (or user id when username is null), call count, and
cost formatted as USD with 6 fractional digits. Rows MUST be ordered as the
endpoint returns them.

ADF-5. Model/channel health card MUST render one row per channel with columns:
provider name, channel name, enabled, weight, auto session affinity, health
status (healthy/unhealthy/cooling-down), last probe time, today's calls, and
today's cost formatted as USD with 2 fractional digits. Health status MUST
derive from `healthy` and `cooldown_until`: a channel with
`cooldown_until > now` renders as cooling-down regardless of `healthy`. When
`unhealthy_models` is non-empty, the status cell MUST list those model ids.
The card header MUST also show the process-wide `today.cost_nano_usd` and
`today.calls` totals.

ADF-6. Replica status card MUST render the node role, ingest-enabled state,
spool pending count, and spool pending bytes. When the node is a primary and
ingest is disabled, the card MUST state that no replica token is configured
and there is nothing to monitor. When ingest is enabled, the card MUST list
every object in `replica.replicas` with hostname, listen address, version,
uptime, last-seen time, spool pending files/bytes, and a stale/live badge.
An enabled ingest with an empty `replicas` array MUST state that no replica
has heartbeated yet.

ADF-7. Data fetching MUST use SWR with a 10-second refresh interval, skeleton
fallbacks while loading, and an error state with retry on failure. Mutations
do not exist on this page.

ADF-8. The page MUST NOT throw when any optional field is missing; missing
timestamps MUST render as `-`.