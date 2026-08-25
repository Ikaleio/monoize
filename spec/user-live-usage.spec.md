# User Live Usage Specification

## 0. Status

- **Purpose:** Expose the authenticated dashboard user's own rolling 60-second request-log
  usage aggregate for the sidebar user-center dropdown (`dashboard-ui-layout.spec.md` DL3d).
- **Scope:** `GET /api/dashboard/me/live-usage` and its storage-layer aggregate.

## 1. Endpoint contract

LU-1. The server MUST expose `GET /api/dashboard/me/live-usage`.

LU-2. Authorization: any authenticated dashboard session user, validated by the same
session mechanism as other `/api/dashboard/*` endpoints. A missing or invalid session
MUST return HTTP `401` with code `unauthorized`. A disabled user MUST return HTTP `403`.

LU-3. Scope: the aggregate MUST cover only request-log rows whose `user_id` equals the
authenticated user's id. The endpoint MUST NOT accept a `user_id` query parameter or any
other filter parameter. An `admin` or `super_admin` caller receives the same
own-rows-only aggregate as any other user; this endpoint never exposes other users' data.

LU-4. Window: the aggregate window is exactly the last 60 seconds relative to the
server's current time at query execution: rows with
`created_at_unix_ms >= now_ms - 60000 AND created_at_unix_ms < now_ms`.
`window_seconds = 60` is a server constant; the endpoint MUST NOT accept a client-supplied
window. Legacy rows with `created_at_unix_ms IS NULL` are excluded (every such row
predates the current process by more than 60 seconds by construction, per
`request-logs.spec.md` RL-S2c/RL-S2d).

LU-5. Rows of every `status` (`success`, `error`, `client_gone`, and any other persisted
terminal status) count toward the aggregate. In-memory SSE-only `pending` snapshots are
not database rows and do not count.

LU-6. Response: HTTP `200` with this exact JSON object:

```json
{
  "window_seconds": 60,
  "rpm": integer,
  "tpm": integer,
  "input_tokens": integer,
  "output_tokens": integer,
  "cache_read_tokens": integer,
  "cache_hit_rate": number | null
}
```

- `rpm` = COUNT of matching rows.
- `input_tokens` = SUM of `COALESCE(input_tokens, 0)` over matching rows.
- `output_tokens` = SUM of `COALESCE(output_tokens, 0)` over matching rows.
- `cache_read_tokens` = SUM of `COALESCE(cache_read_tokens, 0)` over matching rows.
- `tpm` = `input_tokens + output_tokens`, computed with checked integer arithmetic;
  overflow MUST return an internal storage error, not a wrapped value.
- `cache_hit_rate` = `cache_read_tokens / input_tokens` as an IEEE-754 double when
  `input_tokens > 0`; otherwise exactly `null`. The value is a ratio (under
  `request-logs.spec.md` RL15a it lies in `[0, 1]`), not a percentage.

LU-7. An empty window (no matching rows) MUST return
`rpm = 0`, `tpm = 0`, `input_tokens = 0`, `output_tokens = 0`, `cache_read_tokens = 0`,
and `cache_hit_rate = null`.

LU-8. Aggregation MUST execute in one SQL statement on both SQLite and PostgreSQL
(`COUNT` plus token `SUM`s cast to a 64-bit integer). The implementation MUST NOT load
individual request-log rows into application memory to compute these values. The range
predicate MUST compare `created_at_unix_ms` directly per `request-logs.spec.md` RL-S2b so
the `(user_id, created_at_unix_ms DESC)` index applies.

## 2. Frontend data layer

LU-9. The frontend MUST expose the endpoint through `api.getMyLiveUsage()` in
`frontend/src/lib/api.ts` and the SWR hook `useLiveUsage` in `frontend/src/lib/swr.ts`
with the canonical key `SWR_KEYS.LIVE_USAGE = "/dashboard/me/live-usage"`.

LU-10. `useLiveUsage` MUST use `refreshInterval: 10000` (10 seconds). The hook is mounted
only while the user-center dropdown content is open (`dashboard-ui-layout.spec.md` DL3d),
so polling runs only while the menu is open and stops when it closes. Reopening the menu
renders the cached value immediately and revalidates without requiring close/reopen.

LU-11. The dropdown MUST format `cache_hit_rate` as a percentage with at most 1
fractional digit (trailing `.0` removed). A `null` rate MUST render as an em dash (`—`),
never as `0%`. `rpm` and `tpm` MUST render as locale-grouped integers.
