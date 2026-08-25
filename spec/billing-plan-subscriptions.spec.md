# Billing Plan Subscriptions Specification

## 0. Purpose

Define recurring balance grants ("subscriptions"): admins define billing plans that periodically
reset a user's balance to a fixed amount and optionally restrict which routing groups the
subscriber may use. A plan is a named record; users reference at most one plan.

## 1. Data model

### 1.1 `billing_plans` table

| Column                   | Type    | Constraints                                    |
| ------------------------ | ------- | ---------------------------------------------- |
| `id`                     | TEXT    | PRIMARY KEY, UUID v4 string                    |
| `name`                   | TEXT    | NOT NULL, unique after `lower(trim(name))`, 1..100 chars after trimming |
| `grant_amount_nano_usd`  | TEXT    | NOT NULL, canonical signed i128 decimal, >= 0   |
| `schedule`               | TEXT    | NOT NULL, 5-field Unix cron (see BP-D4)         |
| `group_ids`              | TEXT    | NOT NULL, JSON array of group ids, default `[]` |
| `enabled`                | INTEGER | NOT NULL, `0` or `1`, default `1`               |
| `created_at`             | TEXT    | NOT NULL, RFC 3339 UTC                          |
| `updated_at`             | TEXT    | NOT NULL, RFC 3339 UTC                          |

BP-D1. `group_ids` references first-class groups (`groups-registry.spec.md`). Every write
path MUST canonicalize and validate it per GR-C1..GR-C3 (trim, drop empties, dedupe
preserving first-occurrence order, at most 32 entries, every id must exist). The empty
array means "no group restriction from this layer".

BP-D4. `schedule` is a 5-field Unix cron expression `minute hour day-of-month month day-of-week`.
Write paths MUST trim the value, split on ASCII whitespace, require exactly five non-empty
fields, and rejoin those fields with a single space. Evaluation timezone is `Asia/Shanghai`.
`0 0 * * *` means 00:00 in `Asia/Shanghai` every day. `day-of-week` uses Unix numbering:
`0` and `7` are Sunday. A value that does not parse, that has the wrong field count, or that
has no fire time after an arbitrary UTC instant MUST be rejected.

BP-D5. Migration `m20260823_000037_billing_plan_cron_schedule` MUST replace
`billing_plans.period_seconds` with `billing_plans.schedule`. Existing rows MUST be converted:

- `60` → `* * * * *`
- `3600` → `0 * * * *`
- `86400` → `0 0 * * *`
- `604800` → `0 0 * * 0`
- any other value → `0 0 * * *`

The `period_seconds` column MUST NOT remain. Existing `users.next_grant_at` values MUST be
left unchanged by the migration.

### 1.2 New `users` columns

| Column           | Type | Constraints                                              |
| ---------------- | ---- | -------------------------------------------------------- |
| `billing_plan_id`| TEXT | NULL; when non-NULL it MUST reference an existing row in `billing_plans` |
| `next_grant_at`  | TEXT | NULL or RFC 3339 UTC                                     |

BP-D2. For every persisted user row, `next_grant_at IS NOT NULL` if and only if
`billing_plan_id IS NOT NULL`. Every write path that changes either column MUST keep both
consistent in the same transaction.

BP-D3. No database-level foreign key constraint is created between `users.billing_plan_id`
and `billing_plans.id`; referential integrity MUST be enforced by write paths. Plan delete
and plan assignment MUST serialize on the plan row so a concurrent assign cannot observe a
deleted plan id (and a concurrent delete cannot drop a plan that just gained an assignee).

## 2. Plan administration API

All endpoints require an authenticated admin (`super_admin` or `admin`) session, identical to
the user management endpoints.

- `GET /api/dashboard/billing-plans` — list all plans ordered by `created_at` ascending.
- `POST /api/dashboard/billing-plans` — create a plan.
- `PUT /api/dashboard/billing-plans/{plan_id}` — update a plan.
- `POST /api/dashboard/billing-plans/{plan_id}/reset` — reset period quota for every eligible subscriber of the plan. Request body is empty.
- `DELETE /api/dashboard/billing-plans/{plan_id}` — delete a plan.

Create request body fields: `name: string`, one of `grant_amount_nano_usd: string` or
`grant_amount_usd: string` (if both are provided, the nano value wins), `schedule: string`,
optional `group_ids: string[]` (omitted = `[]`), optional `enabled: boolean` (omitted = `true`).

Update (`PUT`) request body fields: `name`, `schedule`, and a grant amount are required
(same amount rules as create). Omitted `group_ids` and omitted `enabled` leave the stored
values unchanged (BP-A8).

BP-A1. If `name` (trimmed) already exists on another plan when compared case-insensitively,
the server MUST return HTTP `409` with code `plan_name_exists`. Unique-constraint races MUST
map to the same code, never HTTP `500`.

BP-A2. If `schedule` fails BP-D4, the server MUST return HTTP `400` with code `invalid_schedule`.

BP-A3. Invalid amounts (missing, non-canonical nano string, unparsable USD, negative, overflow)
MUST return HTTP `400` with code `invalid_grant_amount`.

BP-A4. Delete of a plan referenced by at least one user MUST return HTTP `409` with code
`plan_in_use`. Delete of zero-reference plans MUST succeed and leave user balances unchanged.

BP-A5. Update of a nonexistent plan MUST return HTTP `404` with code `not_found`.

BP-A6. Editing any plan field affects only future grant evaluations. Existing
`users.next_grant_at` anchors MUST NOT be shifted by plan edits.

BP-A7. If `name` (trimmed) is empty or longer than 100 characters, the server MUST return
HTTP `400` with code `invalid_plan_name`.

BP-A8. On `PUT`, omitted `enabled` MUST leave the stored enabled flag unchanged, and omitted
`group_ids` MUST leave the stored group list unchanged. On `POST`, omitted
`group_ids` is `[]` and omitted `enabled` is `true`.

BP-A9. Reset of a nonexistent plan MUST return HTTP `404` with code `not_found`.

BP-A10. Reset of an existing plan MUST return HTTP `200` with JSON
`{ "success": true, "reset_count": N }` where `N` is the number of users whose grant
committed in this request. `N = 0` is valid (no eligible subscribers). The plan row
MUST NOT change. A disabled plan is still resettable.

BP-A11. A user is eligible for reset of plan P if and only if ALL of the following hold
at grant time: `billing_plan_id = P.id`, `next_grant_at IS NOT NULL`, `enabled = 1`,
`balance_unlimited = 0`. Disabled users, unlimited users, unassigned users, and users
assigned to a different plan MUST NOT change balance and MUST NOT receive a ledger row.

BP-A12. For each eligible user, reset MUST execute the same atomic grant as BP-G3 with
`execution_now` equal to the reset request time, even when `next_grant_at` is still in
the future and even when `P.enabled = 0`. After a successful per-user grant,
`next_grant_at` is the first fire of `P.schedule` strictly after `execution_now`, so
the next scheduler tick MUST NOT grant that user again until that new anchor. The
ledger `meta_json` MUST include `plan_id`, `plan_name`, and `"source": "admin_reset"`.

BP-A13. A per-user grant failure MUST NOT abort remaining eligible users. `reset_count`
counts only committed grants. Each committed grant MUST invalidate that user's
in-process balance cache before the handler returns (BP-G4).

## 3. Plan assignment

Assignment happens through `PUT /api/dashboard/users/{user_id}` with the new optional field
`billing_plan_id: string | null` (absent = no change; `null` = unassign).

BP-S1. Assigning a plan MUST, in the same transaction as the rest of the user update:
set `next_grant_at` to the first fire of `P.schedule` strictly after `assignment_time`
in timezone `Asia/Shanghai` (stored as RFC 3339 UTC); and, when every
immediate-grant predicate below is true, reset `balance_nano_usd` to
`P.grant_amount_nano_usd` (absolute reset, checked i128) and append one `billing_ledger`
row with `kind = "plan_grant"` using the same fields as BP-G3.

Immediate-grant predicates (all required):
- the user's `enabled` after this update is `1`;
- the user's `balance_unlimited` after this update is `0`;
- the assigned plan has `enabled = 1`;
- the same update does not set `balance_nano_usd` or `balance_usd`.

If any predicate is false, assignment MUST still set `next_grant_at` and MUST NOT change
the user's current balance.

BP-S2. Unassigning (`null`) MUST clear `next_grant_at` to NULL in the same transaction.
The user's current balance MUST NOT change.

BP-S3. Assigning a plan id that does not exist MUST fail the whole update with HTTP `400`,
code `invalid_billing_plan`.

BP-S4. Reassigning from plan P1 to plan P2 MUST apply BP-S1 using P2, including the
immediate grant when BP-S1 predicates hold.

## 4. Grant execution

BP-G1. A background scheduler runs one tick every
`MONOIZE_PLAN_GRANT_TICK_INTERVAL_SECS` seconds (default `60`; invalid or non-positive values
fall back to the default). The first tick runs immediately when background tasks start.

BP-G2. In each tick, every user row satisfying ALL of the following MUST receive one grant:
`billing_plan_id IS NOT NULL`, `balance_unlimited = 0`, `enabled = 1`, joined plan has
`enabled = 1`, and `next_grant_at <= now`.

BP-G3. One grant for user u assigned plan P MUST execute atomically in a single transaction:
lock the user row (row lock on PostgreSQL; single-writer serialization suffices on SQLite),
re-read all BP-G2 conditions from the locked row, set
`balance_nano_usd := P.grant_amount_nano_usd` (absolute reset, checked i128),
`next_grant_at` to the first fire of `P.schedule` strictly after `execution_now` in
`Asia/Shanghai` (RFC 3339 UTC), `updated_at := execution_now`, and append
one `billing_ledger` row with `kind = "plan_grant"`,
`delta_nano_usd = new_balance - old_balance`, `balance_after_nano_usd = new_balance`,
`meta_json = {"plan_id": ..., "plan_name": ...}`.

BP-G4. After a grant commits, the in-process user balance cache entry for that user MUST be
invalidated before the tick proceeds to other work.

BP-G5. Catch-up rule: if multiple schedule fire times elapsed while the scheduler was not
running, exactly ONE grant executes per due user per tick. `next_grant_at` is the first fire
of `P.schedule` strictly after `execution_now`. Missed fire times are not multiplied and not
replayed.

BP-G6. Users with `balance_unlimited = 1` and disabled plans never receive grants and never
produce `plan_grant` ledger rows. Disabled users are skipped entirely.

BP-G7. Grant amounts of `0` are valid; they reset the balance to `"0"` and still produce a
ledger row.

BP-G8. A user who has an assigned enabled plan, is enabled, is not unlimited, and has never
received a `plan_grant` ledger row MUST receive one grant on the next scheduler tick even
when `next_grant_at` is still in the future. That grant MUST reset `balance_nano_usd` as in
BP-G3 and MUST NOT change `next_grant_at`. This recovers subscribers assigned before BP-S1
applied the first grant at assignment time.

## 5. Group composition

BP-R1. When a user references an enabled plan P with non-empty `P.group_ids`, request
authorization filters the base group list (the API key's ordered group ids, or the user's
single group; `api-key-authentication.spec.md` AKG5) to the members of `P.group_ids`,
preserving base order. An empty `P.group_ids` contributes no restriction. The filtered
result MAY be `[]`, in which case zero providers are group-eligible.

BP-R2. A disabled plan, or a missing plan row, contributes NO restriction (treated exactly as
"no plan") for group computation. Per BP-G6 it also receives no grants.

BP-R3. The composition output MUST be deduplicated preserving first-occurrence order; it
MUST NOT be lowercased or sorted (group ids are opaque, order is routing preference).

## 6. Dashboard response surface

BP-U1. User responses returned by dashboard endpoints MUST include
`billing_plan_id: Option<String>` and `next_grant_at: Option<String>` (RFC 3339; both are
`null` when no plan is assigned).

BP-U2. Every dashboard user JSON object from login, register,
`GET /api/dashboard/auth/me`, `PUT /api/dashboard/auth/me`,
`GET /api/dashboard/stats` field `current_user`, `POST /api/dashboard/users`,
`GET /api/dashboard/users`, `GET /api/dashboard/users/{user_id}`, and
`PUT /api/dashboard/users/{user_id}` MUST include `billing_plan` as JSON `null` or:

```json
{
  "id": "plan-uuid",
  "name": "starter",
  "grant_amount_nano_usd": "10000000000",
  "grant_amount_usd": "10",
  "schedule": "0 0 * * *",
  "group_ids": [],
  "enabled": true
}
```

`billing_plan` MUST be the current `billing_plans` row for `billing_plan_id`.
If `billing_plan_id` is null, `billing_plan` MUST be null. If `billing_plan_id` is
non-null and no matching plan row exists, `billing_plan` MUST be null.
Non-admin callers MUST receive this object from auth/me and MUST NOT be required
to call `GET /api/dashboard/billing-plans`.

BP-U3. `GET /api/dashboard/users` MUST include these additional fields on every listed user:

- `today_calls: integer` — COUNT of `request_logs` rows with that `user_id` and
  `created_at_unix_ms >= UTC calendar-day start` (`today_start` equals
  `UTC date of now at 00:00:00.000`, the same instant used by
  `GET /api/dashboard/analytics`);
- `today_cost_nano_usd: string` — SUM of canonical in-range `charge_nano_usd` for
  those rows. Aggregation MUST follow `spec/request-logs.spec.md` RL-S2e;
- `today_cost_usd: string` — `format_nano_to_usd(today_cost_nano_usd)` with no
  binary floating conversion.

A listed user with no matching logs MUST have `today_calls = 0` and
`today_cost_nano_usd = "0"`. These three fields MUST be omitted from login,
register, me, stats `current_user`, create, get-by-id, and update responses.

BP-U4. `GET /api/dashboard/users` MUST compute BP-U3 with one set-based
`GROUP BY user_id` query. It MUST NOT issue one request-log query per listed user.
It MUST resolve `billing_plan` objects with at most one billing-plan list (or an
equivalent set-based join), not one plan lookup per listed user.

BP-U5. A canonical stored charge outside the signed `i128` domain, or a per-user
or cross-user sum outside that domain, MUST return HTTP `500` with code
`internal_error`. Non-canonical stored charge text MUST not contribute to
`today_cost_nano_usd` and MUST still contribute to `today_calls`.
