# Billing Plan Sliding-Window Specification

## 0. Purpose

Define purchasable billing plans with sliding-window usage limits. A billing plan is
separate from the user's prepaid balance. An eligible API-key request consumes plan
capacity before it consumes prepaid balance.

This specification replaces the recurring balance-grant model. The system MUST NOT keep
`grant_amount_nano_usd`, `schedule`, `users.billing_plan_id`, `users.next_grant_at`, or
`plan_grant` behavior.

## 1. Units and windows

BP-U1. All plan prices, limits, usage, and prepaid balances use signed nano-USD integers.
Persisted amounts MUST use canonical `i128` decimal text. Arithmetic MUST use checked
integer or exact decimal operations. Binary floating-point arithmetic is forbidden.

BP-U2. A plan has four optional limits:

| Field | Window length |
|---|---:|
| `limit_5h_nano_usd` | 18,000 seconds |
| `limit_24h_nano_usd` | 86,400 seconds |
| `limit_7d_nano_usd` | 604,800 seconds |
| `limit_30d_nano_usd` | 2,592,000 seconds |

BP-U3. `null` means that the window is not enforced. A configured limit MUST be greater
than zero. Each plan MUST configure at least one limit.

BP-U4. At instant `T`, usage for a window of `W` seconds is the checked sum of
`billing_plan_usage.amount_nano_usd` for the active subscription where
`created_at > T - W` and `created_at <= T`. A row at exactly `T - W` is outside the window.

BP-U5. Remaining capacity for a configured window is `max(limit - usage, 0)`. The plan's
remaining capacity is the minimum remaining capacity across its configured windows.

BP-U6. At instant `T`, `next_reset_at` for a configured window of `W` seconds is the
minimum `created_at + W` among the usage rows counted by BP-U4. It is `null` when the
window usage is zero. This timestamp identifies the next instant when current usage exits
the sliding window; it does not define a fixed calendar reset.

## 2. Data model

### 2.1 `billing_plans`

| Column | Type | Constraints |
|---|---|---|
| `id` | TEXT | primary key, UUID v4 |
| `name` | TEXT | trimmed length 1..100, case-insensitive unique |
| `description` | TEXT | trimmed length 0..1000 |
| `limit_5h_nano_usd` | TEXT | nullable canonical positive i128 |
| `limit_24h_nano_usd` | TEXT | nullable canonical positive i128 |
| `limit_7d_nano_usd` | TEXT | nullable canonical positive i128 |
| `limit_30d_nano_usd` | TEXT | nullable canonical positive i128 |
| `group_ids` | TEXT | non-empty JSON array of registered group ids |
| `multiplier` | TEXT | canonical decimal greater than zero, at most 9 fractional digits |
| `listed` | INTEGER | `0` or `1` |
| `created_at` | TEXT | RFC 3339 UTC |
| `updated_at` | TEXT | RFC 3339 UTC |

BP-D1. `group_ids` MUST contain 1..32 canonical registered group ids. The write path MUST
trim values, remove empty values, and remove duplicates while preserving the first order.

BP-D2. `multiplier` uses `exact_decimal::Multiplier`. The value `0` is invalid for a plan.
The default value is `1`.

BP-D3. `listed = 1` means that authenticated users can see and purchase the plan.
Changing `listed` MUST NOT disable or change an existing subscription.

### 2.2 `billing_plan_prices`

| Column | Type | Constraints |
|---|---|---|
| `id` | TEXT | primary key, UUID v4 |
| `plan_id` | TEXT | existing `billing_plans.id` |
| `price_nano_usd` | TEXT | canonical i128 greater than zero |
| `duration_seconds` | BIGINT | integer greater than zero |
| `created_at` | TEXT | RFC 3339 UTC |

BP-D4. One plan MUST NOT contain two prices with the same `duration_seconds`. A listed
plan MUST contain at least one price. An unlisted plan MAY contain zero prices.

BP-D5. A price tuple `(price_nano_usd, duration_seconds)` means that a purchase costs the
given prepaid amount and expires exactly `duration_seconds` after the purchase commits.

### 2.3 `billing_plan_subscriptions`

Each row is an immutable purchase snapshot.

| Column | Type |
|---|---|
| `id`, `user_id`, `plan_id`, `price_id` | TEXT |
| `plan_name`, `plan_description`, `group_ids`, `multiplier` | TEXT |
| the four `limit_*_nano_usd` fields | nullable TEXT |
| `price_nano_usd` | TEXT |
| `starts_at`, `expires_at`, `created_at` | RFC 3339 UTC TEXT |

BP-D6. A subscription is active at `T` when `starts_at <= T` and `expires_at > T`.
A user MUST have at most one active subscription. Expired rows remain queryable.

BP-D7. Plan updates and plan deletion MUST NOT change a subscription snapshot. Plan
deletion MUST delete the current price rows but MUST preserve subscriptions and usage.

### 2.4 `billing_plan_usage`

| Column | Type | Constraints |
|---|---|---|
| `id` | TEXT | primary key, UUID v4 |
| `subscription_id`, `user_id`, `api_key_id` | TEXT | required historical identifiers |
| `request_id` | TEXT | required and unique |
| `group_id` | TEXT | required historical group id |
| `amount_nano_usd` | TEXT | canonical positive i128 |
| `created_at` | TEXT | RFC 3339 UTC |

BP-D8. One terminal request MUST create at most one usage row. User, API key, group, plan,
or subscription deletion MUST NOT delete historical usage rows.

## 3. Destructive schema cutover

BP-M1. Migration `m20260911_000051_billing_plan_sliding_windows` MUST perform these actions:

1. Clear all `users.billing_plan_id` and `users.next_grant_at` values.
2. Drop the old `billing_plans` table and its indexes.
3. Drop `users.billing_plan_id` and `users.next_grant_at`.
4. Create the four tables in section 2 and their indexes.

BP-M2. The migration MUST NOT convert or preserve an old billing plan. Existing prepaid
`users.balance_nano_usd` values and existing `billing_ledger` rows MUST remain unchanged.

BP-M3. The application MUST NOT start the old plan-grant scheduler after this migration.

## 4. Plan administration API

All endpoints in this section require an admin session.

- `GET /api/dashboard/billing-plans`
- `POST /api/dashboard/billing-plans`
- `PUT /api/dashboard/billing-plans/{plan_id}`
- `DELETE /api/dashboard/billing-plans/{plan_id}`

BP-A1. Create and update bodies contain `name`, `description`, the four optional limits,
`group_ids`, `multiplier`, `listed`, and `prices`. Each price contains either
`price_nano_usd` or `price_usd`, plus `duration_seconds`.

BP-A2. The server MUST validate the complete replacement object before changing storage.
An update MUST replace every mutable field and every price row in one transaction.

BP-A3. Invalid names return HTTP 400 `invalid_plan_name`. Duplicate names return HTTP 409
`plan_name_exists`. Invalid limits return HTTP 400 `invalid_plan_limits`. Invalid groups
return HTTP 400 `invalid_plan_groups`. Invalid multipliers return HTTP 400
`invalid_plan_multiplier`. Invalid prices return HTTP 400 `invalid_plan_prices`.

BP-A4. Updating or deleting an unknown plan returns HTTP 404 `not_found`. Deleting a plan
with active subscriptions is allowed because subscriptions contain immutable snapshots.

## 5. Marketplace and purchase API

- `GET /api/dashboard/billing-plans/marketplace` requires any authenticated dashboard user.
- `GET /api/dashboard/billing-plan-subscription` requires any authenticated dashboard user.
- `POST /api/dashboard/billing-plans/{plan_id}/purchase` requires any authenticated dashboard user and body `{ "price_id": string }`.

BP-P1. Marketplace returns only plans where `listed = 1`. It returns every current price
for those plans. It MUST NOT return an unlisted plan.

BP-P2. The subscription endpoint returns `null` when no active subscription exists. When
one exists, it returns the immutable snapshot and one object for each configured window.
Each window object contains `limit_nano_usd`, `used_nano_usd`,
`remaining_nano_usd`, and `next_reset_at`. Amounts are canonical nano-USD integer
strings. `next_reset_at` is an RFC 3339 UTC timestamp or `null` as defined by BP-U6.

BP-P3. Purchase MUST run in one transaction. It MUST lock the user row, re-read the listed
plan and selected price, and reject a second active subscription with HTTP 409
`active_subscription_exists`.

BP-P4. A purchase MUST reject a missing, unlisted, or changed plan/price with HTTP 409
`plan_not_available`. It MUST reject a disabled user with HTTP 403 `user_disabled`.

BP-P5. A finite-balance user MUST have `prepaid_balance >= price_nano_usd`. Otherwise the
purchase returns HTTP 402 `insufficient_balance` and changes no state.

BP-P6. A successful purchase MUST subtract the price from `users.balance_nano_usd`, create
one subscription snapshot, and append one `billing_ledger` row with kind `plan_purchase`.
The ledger delta is `-price_nano_usd`. `balance_after_nano_usd` is the prepaid balance.

BP-P7. A user with `balance_unlimited = 1` MAY purchase a plan. The purchase creates the
subscription and a zero-delta `plan_purchase` row. It does not change prepaid balance.

## 6. Group eligibility

BP-G1. A request can use plan capacity only when all conditions are true:

1. Authentication used an API key.
2. The user has an active subscription at settlement time.
3. The selected attempt has a non-null `billing_group_id`.
4. The selected `billing_group_id` is in the subscription snapshot `group_ids`.

BP-G2. The plan MUST NOT filter or change API-key routing groups. A request that uses a
group outside the subscription remains routable and uses prepaid or sub-account balance.

BP-G3. Dashboard Playground traffic has no API key and MUST NOT use plan capacity.

## 7. Charge allocation

BP-C1. Existing model, tool, Channel, and group pricing first computes
`settled_charge_nano_usd`. For an eligible request, compute:

```
adjusted_charge = trunc(settled_charge_nano_usd * subscription.multiplier)
```

For an ineligible request, `adjusted_charge = settled_charge_nano_usd`.

BP-C2. `request_logs.charge_nano_usd`, the request ledger metadata, and the returned billing
breakdown MUST use `adjusted_charge`. The breakdown MUST also contain the plan id,
subscription id, multiplier, plan-covered amount, and prepaid-covered amount when a plan
was eligible.

BP-C3. At terminal settlement, the primary MUST lock the active subscription and balance
state. It MUST recompute current window usage inside the transaction.

BP-C4. Let `R` be the remaining plan capacity from BP-U5. The plan-covered amount is
`min(adjusted_charge, R)`. The fallback amount is
`adjusted_charge - plan_covered_amount`.

BP-C5. If the plan-covered amount is positive, the transaction MUST append one
`billing_plan_usage` row. If the fallback amount is positive, it MUST deduct that amount
from the API-key sub-account when enabled. Otherwise it MUST deduct prepaid balance.

BP-C6. The transaction MUST append one `billing_ledger` request row. Its delta is the
negative fallback amount. Its `balance_after_nano_usd` is the fallback balance after debit.
Its metadata MUST contain the total adjusted charge and the plan-covered amount. A fully
plan-covered request therefore writes a zero-delta request ledger row.

BP-C7. Terminal settlement MAY make a prepaid or sub-account balance negative. The request
already consumed upstream resources. Checked integer overflow MUST fail closed with HTTP
500 and must commit no local charge state.

BP-C8. Pre-forward admission succeeds when at least one applicable source can spend:

- `balance_unlimited = 1`; or
- an eligible active subscription has positive remaining capacity in every configured window; or
- the applicable prepaid or sub-account balance is greater than zero.

Otherwise admission returns HTTP 402 `insufficient_balance`.

BP-C9. A subscription that expires between admission and settlement provides no capacity.
The terminal charge falls back to the applicable balance.

BP-C10. The replica metering path MUST preserve BP-C1 through BP-C9. Pending replica
charges MUST reduce admission capacity conservatively before the primary acknowledges them.

## 8. Prepaid balance separation

BP-B1. `users.balance_nano_usd` is the prepaid balance. Recharge, recharge refund,
admin adjustment, and sub-account transfer operations MUST change only prepaid balance.

BP-B2. Plan capacity is computed only from the subscription snapshot and
`billing_plan_usage`. Recharge MUST NOT increase plan capacity. Plan expiry MUST NOT change
prepaid balance.

BP-B3. Wallet ledger filters MUST include `plan_purchase`. They MUST NOT include the removed
`plan_grant` kind.

## 9. Dashboard behavior

BP-UI1. `/dashboard/plans` is an admin page. It MUST list and edit the plan name,
description, four optional limits, groups, multiplier, listed state, and all price tuples.

BP-UI2. Plan mutations MUST use SWR optimistic updates, render skeletons while loading, and
roll back on failure without closing a form that contains invalid data.

BP-UI3. `/dashboard/wallet` MUST show prepaid balance separately from the active plan. It
MUST show each configured limit, current sliding-window usage, remaining capacity, and the
subscription expiry time.

BP-UI4. `/dashboard/wallet` MUST list marketplace plans and price options. Purchase MUST
show the selected prepaid price and duration before confirmation. A successful purchase
MUST revalidate the session user, active subscription, marketplace, and ledger caches.

BP-UI5. An admin viewing `/dashboard/wallet` MUST request orders and ledger rows with the
admin's own username filter. The personal wallet MUST NOT display other users' rows.
