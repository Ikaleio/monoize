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

AD-2. The response MUST be a JSON object with exactly these top-level fields (`node`, `replica`, `system`, `spend`, `users_ranking`, `channel_health`):

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
- `spend`: object:
  - `window`: one of `"24h"`, `"3d"`, `"7d"`, `"14d"`, `"30d"`;
  - `window_hours`: integer hours corresponding to `window`
    (24, 72, 168, 336, 720);
  - `time_from`: RFC 3339 inclusive lower bound (`now - window_hours`);
  - `time_to`: RFC 3339 exclusive upper bound (`now`);
  - `calls`: integer COUNT of request-log rows with
    `created_at_unix_ms >= time_from` and `created_at_unix_ms < time_to`
    and `created_at_unix_ms IS NOT NULL`;
  - `cost_nano_usd`: nano-dollar integer string SUM of canonical in-range
    `charge_nano_usd` (same aggregation as analytics).
    The window is a rolling interval ending at request time. It MUST NOT use
    UTC calendar-day midnight as the start.
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
  - `window_calls`: integer COUNT of in-window request-log rows for this
    `channel_id` (same rolling window as `spend`);
  - `window_cost_nano_usd`: nano-dollar integer string SUM of those rows'
    canonical `charge_nano_usd`.

AD-2a. `GET /api/dashboard/admin/overview` MUST accept an optional
`spend_window` query parameter. Allowed values are exactly `24h`, `3d`,
`7d`, `14d`, and `30d`. An omitted parameter MUST default to `24h`. An
empty string or any other value MUST return HTTP 400, code
`invalid_spend_window`, and `param = "spend_window"` before executing any
usage-aggregation database query.

AD-2b. The in-memory channel health state stores unix seconds. The endpoint MUST
multiply `last_success_at`, `cooldown_until`, and `last_probe_at` by 1000 before
serialization, so that every AD-2 health timestamp is in unix milliseconds.
`cooldown_active` MUST be true exactly when `cooldown_until` is later than the
request time.

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

ADF-2. The page MUST render inside the dashboard main pane (`dashboard-ui-layout.spec.md`
DL7). It MUST NOT render an additional `main` element, an internal vertical scroll
container, or a fixed footer. It MUST render, in order:

1. the shared `PageHeader` with the localized title and description. Its action area
   MUST contain an outline Refresh button and the muted localized text "Auto refresh
   10s". The button MUST revalidate the overview SWR key and MUST be disabled while the
   revalidation it started is in flight. The 10-second poll MUST NOT disable the button;
2. the Model/channel health card, spanning the full content width;
3. a grid. At viewport widths at or above `lg`, the grid MUST have 12 columns. The left
   column MUST occupy 7 columns and contain User usage ranking. The right column MUST
   occupy 5 columns and contain System status followed by Replica status. Each column
   MUST stack independently.

At viewport widths below `lg`, the cards MUST use this single-column order:
Model/channel health, User usage ranking, System status, Replica status.

ADF-2a. Each card MUST render a `CardTitle`, a localized `CardDescription`, and its
content. The System status, User usage ranking, and Replica status descriptions MUST be
one sentence each; the health card description is the ADF-5 summary. A card title MUST
NOT contain an icon. A card description MUST NOT repeat a value shown inside the card.

ADF-3. The System status card MUST render key-value rows in this order: node role,
version, uptime (humanized, e.g. `2d 4h 12m`), started at, listen address, metrics
path, database (backend and redacted DSN), egress proxy (URL or `—`), pending request
logs, SSE connections, routing config revision, tracked health entries, and affinity
bindings. Values that are technical identifiers (version, listen address, metrics
path, database, egress proxy, routing revision) MUST use the monospace font. Counts
and durations MUST use the body font with tabular numerals.

ADF-4. The User usage ranking card title MUST state the 24-hour window. Its description
MUST state the sort order and the 20-user limit without repeating the window. The card
MUST render a semantic table built from the shared `Table` primitives with columns:
rank, username (or user id in monospace when username is null), call count, and cost
formatted as USD with 6 fractional digits. Call count and cost MUST be end-aligned in
both header and body cells. Rows MUST be ordered as the endpoint returns them and MUST
use `user_id` as their key. The table MUST NOT have a bounded viewport or virtualized
rows, because AD-2 bounds the ranking to 20 rows. An empty ranking MUST render the
localized no-usage state.

ADF-5. The Model/channel health card header MUST render, on its first row, the title,
the summary "{total} channels · {unhealthy} unhealthy", and at the inline end the
segmented spend-window control. `unhealthy` counts channels whose derived status is
unhealthy or cooling-down. The control MUST use the literal ASCII labels `24h`, `3d`,
`7d`, `14d`, and `30d`, in that order, default to `24h`, expose `aria-pressed` on each
button, and render labels at 0.875rem or larger. These labels MUST NOT be translated.
The selection MUST be held only in React component state. The first row MUST wrap so
the control remains usable on narrow widths. The second row MUST show the process-wide
`spend.cost_nano_usd` (USD, 2 fractional digits), `spend.calls`, and a localized note
that the window is rolling, ends now, and does not reset at midnight.

ADF-5a. The health rows MUST render through the `DataList` primitives
(`frontend-design-system.spec.md` §6.1). In wide mode, the columns MUST be, in order:
Channel (channel name, and provider name on a second muted line), Weight (end-aligned),
Affinity (`auto` as plain text when auto session affinity is enabled, `—` otherwise),
Status, Spend (end-aligned; selected-window cost with 2 fractional digits, and the call
count on a second muted line), and Last probe (end-aligned timestamp or `—`). At a
content width of 1118 CSS pixels, every column MUST be visible without horizontal scrolling.

ADF-5b. Status MUST derive from `cooldown_active`, `enabled`, and `healthy`, in this order.
The client MUST use the server-computed `cooldown_active` (AD-2b) instead of comparing
`cooldown_until` with the client clock. `cooldown_active = true` renders cooling-down
(`StatusBadge variant="warning"`);
otherwise `enabled = false` renders disabled (`Badge variant="secondary"`); otherwise
`healthy` renders healthy (`StatusBadge variant="success"`) or unhealthy
(`StatusBadge variant="destructive"`). When `unhealthy_models` is non-empty, the status
cell MUST list those model ids in monospace `text-error-foreground`, truncated to one
line with the full list in the `title` attribute.

ADF-5c. The health list MUST be virtualized with `Virtuoso`, using
`virtualDataListComponents` and the DL7b main pane as `customScrollParent`. Its height
MUST equal the height of its rendered rows. In wide mode, its header row MUST be sticky
at the top of the main pane. Each row MUST use `channel_id` as its virtual item key. An
empty `channel_health` array MUST render the localized no-channels state.

ADF-6. The Replica status card MUST render key-value rows for node role, replica ingest
(`StatusBadge variant="success"` when enabled, muted text when disabled), spool pending
count, and spool pending bytes. On a replica node, it MUST render only node role and the
two spool rows. When the node is a primary and ingest is disabled, the card MUST state
that no replica token is configured and there is nothing to monitor. When ingest is
enabled, the card MUST list every object in `replica.replicas` with hostname (or id),
listen address, a live (`StatusBadge variant="success"`) or stale
(`StatusBadge variant="warning"`) badge, and key-value rows for version, uptime,
last-seen time, spool pending files, and spool pending bytes. An enabled ingest with an
empty `replicas` array MUST state that no replica has heartbeated yet.

ADF-7. Data fetching MUST use SWR with a 10-second refresh interval. The first load MUST
render a skeleton with the ready layout: page header, a full-width card, and the
two-column grid. A failure without cached data MUST render the shared `QueryError`; its
retry MUST revalidate the overview SWR key, MUST be disabled while pending, and MUST NOT
show the raw error message. A failed refresh with cached data MUST keep the cached page
and render `QueryError` with `stale` above the health card. Mutations do not exist on
this page. The SWR cache key MUST include the selected `spend_window`. The hook MUST set
`keepPreviousData: true`. A spend-window switch MUST NOT show a skeleton. While a fetch
for a newly selected window is in flight, only the health list MUST dim (`opacity-60`);
the header, totals, and spend-window control MUST remain fully opaque. Dim MUST apply
only when `data.spend.window` differs from the selected window, so a same-window
10-second refresh does not dim.

ADF-8. The page MUST NOT throw when any optional field is missing. Missing timestamps
and missing values MUST render as `—`.
