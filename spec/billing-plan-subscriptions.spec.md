# Billing Plan Subscriptions Specification

## 0. Purpose

Define recurring balance grants ("subscriptions"): admins define billing plans that periodically
reset a user's balance to a fixed amount and optionally restrict which channel groups the
subscriber may use. A plan is a named record; users reference at most one plan.

## 1. Data model

### 1.1 `billing_plans` table

| Column                   | Type    | Constraints                                    |
| ------------------------ | ------- | ---------------------------------------------- |
| `id`                     | TEXT    | PRIMARY KEY, UUID v4 string                    |
| `name`                   | TEXT    | NOT NULL, UNIQUE, 1..100 chars after trimming   |
| `grant_amount_nano_usd`  | TEXT    | NOT NULL, canonical signed i128 decimal, >= 0   |
| `period_seconds`         | BIGINT  | NOT NULL, > 0                                   |
| `allowed_groups`         | TEXT    | NOT NULL, JSON array of strings, default `[]`   |
| `enabled`                | INTEGER | NOT NULL, `0` or `1`, default `1`               |
| `created_at`             | TEXT    | NOT NULL, RFC 3339 UTC                          |
| `updated_at`             | TEXT    | NOT NULL, RFC 3339 UTC                          |

BP-D1. `allowed_groups` MUST be canonicalized on every write path exactly like
`users.allowed_groups`: trim each element, lowercase, drop empties, deduplicate, sort ascending.
The empty array means "no group restriction from this layer".

### 1.2 New `users` columns

| Column           | Type | Constraints                                              |
| ---------------- | ---- | -------------------------------------------------------- |
| `billing_plan_id`| TEXT | NULL; when non-NULL it MUST reference an existing row in `billing_plans` |
| `next_grant_at`  | TEXT | NULL or RFC 3339 UTC                                     |

BP-D2. For every persisted user row, `next_grant_at IS NOT NULL` if and only if
`billing_plan_id IS NOT NULL`. Every write path that changes either column MUST keep both
consistent in the same transaction.

BP-D3. No database-level foreign key constraint is created between `users.billing_plan_id`
and `billing_plans.id`; referential integrity MUST be enforced by write paths.

## 2. Plan administration API

All endpoints require an authenticated admin (`super_admin` or `admin`) session, identical to
the user management endpoints.

- `GET /api/dashboard/billing-plans` — list all plans ordered by `created_at` ascending.
- `POST /api/dashboard/billing-plans` — create a plan.
- `PUT /api/dashboard/billing-plans/{plan_id}` — update a plan.
- `DELETE /api/dashboard/billing-plans/{plan_id}` — delete a plan.

Create/update request body fields: `name: string`, one of `grant_amount_nano_usd: string` or
`grant_amount_usd: string` (if both are provided, the nano value wins), `period_seconds: integer`,
`allowed_groups: string[]`, optional `enabled: boolean` (default `true` on create).

BP-A1. If `name` (trimmed) already exists on another plan, the server MUST return HTTP `409`
with code `plan_name_exists`.

BP-A2. If `period_seconds <= 0`, the server MUST return HTTP `400` with code `invalid_period`.
Non-integer values are rejected at deserialization.

BP-A3. Invalid amounts MUST return HTTP `400` with code `invalid_grant_amount`.

BP-A4. Delete of a plan referenced by at least one user MUST return HTTP `409` with code
`plan_in_use`. Delete of zero-reference plans MUST succeed and leave user balances unchanged.

BP-A5. Update of a nonexistent plan MUST return HTTP `404` with code `not_found`.

BP-A6. Editing any plan field affects only future grant evaluations. Existing
`users.next_grant_at` anchors MUST NOT be shifted by plan edits.

## 3. Plan assignment

Assignment happens through `PUT /api/dashboard/users/{user_id}` with the new optional field
`billing_plan_id: string | null` (absent = no change; `null` = unassign).

BP-S1. Assigning a plan MUST set `next_grant_at = assignment_time + period_seconds` of the
target plan in the same transaction as the rest of the user update. The user's current balance
MUST NOT change at assignment time.

BP-S2. Unassigning (`null`) MUST clear `next_grant_at` to NULL in the same transaction.
The user's current balance MUST NOT change.

BP-S3. Assigning a plan id that does not exist MUST fail the whole update with HTTP `400`,
code `invalid_billing_plan`.

BP-S4. Reassigning from plan P1 to plan P2 MUST set `next_grant_at = now + P2.period_seconds`.

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
`next_grant_at := execution_now + P.period_seconds`, `updated_at := execution_now`, and append
one `billing_ledger` row with `kind = "plan_grant"`,
`delta_nano_usd = new_balance - old_balance`, `balance_after_nano_usd = new_balance`,
`meta_json = {"plan_id": ..., "plan_name": ...}`.

BP-G4. After a grant commits, the in-process user balance cache entry for that user MUST be
invalidated before the tick proceeds to other work.

BP-G5. Catch-up rule: if multiple period boundaries elapsed while the scheduler was not
running, exactly ONE grant executes per due user per tick, anchored forward from
`execution_now` (`next_grant_at := execution_now + period_seconds`). Missed periods are not
multiplied and not replayed.

BP-G6. Users with `balance_unlimited = 1` and disabled plans never receive grants and never
produce `plan_grant` ledger rows. Disabled users are skipped entirely.

BP-G7. Grant amounts of `0` are valid; they reset the balance to `"0"` and still produce a
ledger row.

## 5. Group composition

BP-R1. When a user references an enabled plan P, request authorization computes effective
groups as the intersection of three layers:
`user.allowed_groups ∩ P.allowed_groups ∩ api_key.allowed_groups`.
Each layer's empty array means "unrestricted at this layer". The result is `None` (fully
unrestricted) if and only if all three layers are unrestricted; otherwise the result is
`Some(intersection)`, possibly `Some([])` when the layers share no group.

BP-R2. A disabled plan, or a missing plan row, contributes NO restriction (treated exactly as
"no plan") for group computation. Per BP-G6 it also receives no grants.

BP-R3. Canonicalization rules of every layer are identical to BP-D1. The composition output
MUST be canonical (sorted, deduplicated).

## 6. Dashboard response surface

BP-U1. User responses returned by dashboard endpoints MUST include
`billing_plan_id: Option<String>` and `next_grant_at: Option<String>` (RFC 3339; both are
`null` when no plan is assigned).
