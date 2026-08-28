# Recharge System Specification

## 0. Status

- Product name: Monoize.
- Scope:
  - the payment-channel registry (`payment_channels`) and the compile-time
    payment-adapter registry (`epay`, `stripe`);
  - the recharge order state machine (`recharge_orders`);
  - currency conversion between a payment currency and the nano-USD wallet;
  - exactly-once wallet credit on verified payment notification, replay and
    race behavior, expiry, reconciliation, and refunds;
  - the dashboard APIs for recharge channels, recharge orders, and the
    billing-ledger read surface;
  - the `/dashboard/wallet` user page and the `/dashboard/payments` admin page.
- Related specs: `user-billing-and-model-metadata.spec.md` (balance storage B1..B6,
  ledger §6, write-pool concurrency §6a), `api-key-sub-account-billing.spec.md`
  (ledger kind table), `billing-plan-subscriptions.spec.md` (grant ledger rows),
  `request-logs.spec.md` (per-request spend surface), `dashboard-ui-layout.spec.md`
  (navigation, skeleton and SWR contracts), `primary-replica-deployment.spec.md`
  (replica request surface D1), `security-access-control.spec.md` (admin session
  helper).

### 0.1 Implementation status

RC-S1. This specification is implemented. Migration step
`m20260910_000050_recharge_system` (§14) ships with this file, and every rule
in this file is in force.

RC-S2. The implementation change that ships §14 also landed these companion
spec amendments in the same change, so specs and code stay aligned:

1. `dashboard-ui-layout.spec.md` DL5 gained `/dashboard/wallet` between
   `/dashboard/tokens` and `/dashboard/logs`.
2. `dashboard-ui-layout.spec.md` DL6 gained `/dashboard/payments` immediately
   after `/dashboard/plans`.
3. `api-key-sub-account-billing.spec.md` ledger-kind table gained the two rows
   defined by RC-L2.
4. `system-settings-ui.spec.md` gained the `recharge_public_origin` field (§4).

## 1. Concepts and units

RC-U1. The prepaid wallet denomination is nano-USD exactly as defined by
`user-billing-and-model-metadata.spec.md` B1/B2. Recharge MUST NOT introduce a
second stored balance denomination, a per-currency wallet, or a points/credits
unit. Every recharge credits `users.balance_nano_usd`. A recharge MUST NOT modify plan capacity.

RC-U2. All recharge amount arithmetic MUST use exact decimal or checked `i128`
integer arithmetic. No amount, rate, or conversion may pass through `f32` or
`f64` (same rule as `model-pricing.spec.md` MP-U2).

RC-U3. `credit_nano_usd` is the signed `i128` nano-USD amount credited to the
wallet when an order succeeds. A valid order has `credit_nano_usd > 0`.

RC-U4. A payment-currency amount (`pay_amount`) is a base-10 decimal string with
exactly `scale(currency, type_id)` fractional digits (RC-P4), no exponent
notation, no leading `+`, and no leading zeros other than a single `0` before
the decimal point.

RC-U5. `usd_rate` is a base-10 decimal string meaning "payment-currency units
per 1 USD". It MUST parse as a positive decimal with at most 12 integer digits
and at most 9 fractional digits. Example: a CNY channel with `usd_rate = "7.30"`
sells 1 USD of wallet credit for 7.30 CNY.

RC-U6. Order creation computes the payment amount as:

```
pay_amount = ceil_to_scale(credit_usd * usd_rate, scale(currency, type_id))
```

where `credit_usd = credit_nano_usd / 10^9` computed exactly, the
multiplication is exact decimal, and `ceil_to_scale` rounds UP (toward positive
infinity) to `scale` fractional digits. Rounding up guarantees the operator
never receives less than the configured rate. Both `pay_amount` and `usd_rate`
are frozen into the order row at creation time; a later channel-rate edit MUST
NOT change any existing order.

RC-U7. Because `credit_nano_usd > 0`, `usd_rate > 0`, and `ceil_to_scale`
rounds up, the computed `pay_amount` is always at least one smallest payment
unit (`10^-scale`). No zero-amount payment can be constructed.

## 2. Payment-adapter registry

RC-P1. A compile-time payment-adapter registry maps `type_id` to an adapter
implementation. Version 1 contains exactly two `type_id` values: `"epay"` and
`"stripe"`. A `payment_channels.type_id` value outside the registry MUST be
rejected at write time with HTTP `400`, code `invalid_channel_type`.

RC-P2. Every adapter MUST implement:

1. `create_payment(order, channel_config, urls) -> PaymentInitiation` — build
   the provider-side payment and return `{ kind: "redirect", url: string }`.
   Version 1 defines exactly one `kind` value: `"redirect"`. New kinds require
   a revision of this file.
2. `verify_notification(http_method, headers, raw_body, query, channel_config)
   -> VerifiedNotification | SignatureError` — verify authenticity BEFORE any
   database read. `VerifiedNotification` is
   `{ order_id, provider_order_id, result, paid_amount?, paid_currency? }` where
   `result` is one of `success`, `failure`, `expired`.
3. `ack(outcome) -> (http_status, content_type, body)` — render the
   provider-specific acknowledgment for an outcome in
   `{ credited, duplicate, failed_recorded, unknown_order, signature_error }`.

RC-P3. An adapter MAY implement two optional capabilities, each advertised by a
boolean flag:

- `supports_query`: `query_order(order, channel_config) -> success | failure |
  still_pending | unknown` (server-to-provider status pull, §7).
- `supports_refund`: `refund(order, channel_config) -> ok | error` (full-order
  provider-side refund, §8).

Version 1 capability matrix:

| `type_id` | `supports_query` | `supports_refund` |
|-----------|------------------|-------------------|
| `epay`    | false            | false             |
| `stripe`  | false            | true              |

RC-P4. Each adapter constrains `currency` and defines the fractional scale used
by RC-U4/RC-U6:

- `epay`: `currency` MUST be `"CNY"`; `scale = 2`.
- `stripe`: `currency` MUST be a 3-letter uppercase ISO 4217 code; `scale = 0`
  when the currency is in the Stripe zero-decimal set
  `{BIF, CLP, DJF, GNF, JPY, KMF, KRW, MGA, PYG, RWF, UGX, VND, VUV, XAF, XOF,
  XPF}`, otherwise `scale = 2`.

A `currency` outside the adapter's constraint MUST be rejected with HTTP `400`,
code `invalid_currency`.

RC-P5. Each adapter defines a JSON config object stored in
`payment_channels.config_json`, with designated secret fields:

- `epay` config: `gateway_url` (string, `http`/`https` URL, no trailing
  slash), `merchant_id` (non-empty string, the EPay `pid`), `merchant_key`
  (SECRET, non-empty string), `pay_type` (string, MAY be empty; when non-empty
  it is sent as the EPay `type` parameter, e.g. `alipay`, `wxpay`).
- `stripe` config: `secret_key` (SECRET, non-empty string), `webhook_secret`
  (SECRET, non-empty string).

RC-P6. Secret config fields are write-only, following the same rule as Channel
API keys (`dashboard-ui-layout.spec.md` PL5): every admin read endpoint MUST
replace each stored secret field value with the empty string. On
`PUT /api/dashboard/payment-channels/{id}`, an empty-string secret field MUST
keep the stored value; a non-empty value MUST replace it. On `POST` (create),
every secret field MUST be non-empty.

### 2.1 EPay (易支付) protocol binding

RC-E1. `create_payment` builds a GET redirect URL
`{gateway_url}/submit.php?{query}` with parameters: `pid = merchant_id`,
`type = pay_type` (omitted when `pay_type` is empty), `out_trade_no = order.id`,
`notify_url` and `return_url` per §4, `name = "Monoize Recharge"`,
`money = pay_amount`, `sign`, `sign_type = "MD5"`. No provider network call is
made; `provider_order_id` stays null until a notification arrives.

RC-E2. `sign` MUST be computed as lowercase hex
`md5(join("&", sorted "k=v" pairs) + merchant_key)` where the pairs cover every
sent parameter except `sign` and `sign_type`, pairs with empty values are
excluded, and sorting is bytewise ascending on the parameter name.

RC-E3. `verify_notification` accepts GET and POST notifications. It MUST
recompute RC-E2 over the received parameters (excluding `sign`, `sign_type`,
and empty values) and compare with the received `sign` using a constant-time
comparison. A mismatch or a missing `sign` is a `SignatureError`.

RC-E4. A verified EPay notification maps to `VerifiedNotification` as:
`order_id = out_trade_no`, `provider_order_id = trade_no`,
`paid_amount = money`, `paid_currency = "CNY"`, and `result = success` iff
`trade_status == "TRADE_SUCCESS"`, otherwise `result = failure`.

RC-E5. `ack` mapping for `epay`: outcomes `credited`, `duplicate`, and
`failed_recorded` return HTTP `200`, `text/plain`, body exactly `success`
(stops gateway retries); `unknown_order` returns HTTP `200`, `text/plain`,
body exactly `fail`; `signature_error` returns HTTP `400`, `text/plain`, body
exactly `fail`.

### 2.2 Stripe protocol binding

RC-T1. `create_payment` calls the Stripe API with `secret_key` to create a
Checkout Session in `mode = "payment"` with exactly one line item
(`currency = lower(order.pay_currency)`, `unit_amount = pay_amount` expressed
in minor units per RC-P4 scale, `quantity = 1`,
`product_data.name = "Monoize Recharge"`), `client_reference_id = order.id`,
`metadata.order_id = order.id`, and `success_url` / `cancel_url` per §4. It
returns `{ kind: "redirect", url: session.url }`. The created `session.id`
MUST be persisted to the order's `provider_order_id` before the RC-O5 response
returns. A Stripe API error is a create failure (RC-O8).

RC-T2. `verify_notification` accepts POST only. It MUST parse the
`Stripe-Signature` header, recompute HMAC-SHA256 over
`"{timestamp}.{raw_body}"` with `webhook_secret`, compare against every `v1`
signature with constant-time comparison, and reject when no signature matches
or when `|now - timestamp| > 300` seconds. Any of these is a `SignatureError`.

RC-T3. Verified Stripe events map to `VerifiedNotification` as follows;
`order_id = event.data.object.client_reference_id`,
`provider_order_id = event.data.object.id`:

- `checkout.session.completed` with `payment_status == "paid"`, and
  `checkout.session.async_payment_succeeded`: `result = success`,
  `paid_amount = amount_total` converted from minor units to an RC-U4 string,
  `paid_currency = upper(currency)`.
- `checkout.session.async_payment_failed`: `result = failure`.
- `checkout.session.expired`: `result = expired`.
- Every other event type MUST be acknowledged with HTTP `200` and MUST NOT
  read or write any order state.

RC-T4. `ack` mapping for `stripe`: outcomes `credited`, `duplicate`,
`failed_recorded`, and `unknown_order` return HTTP `200`, `application/json`,
body `{"received":true}`; `signature_error` returns HTTP `400`,
`application/json`, body `{"error":"invalid signature"}`.

## 3. Data model

### 3.1 `payment_channels` table

| Column           | Type    | Constraints                                                        |
|------------------|---------|--------------------------------------------------------------------|
| `id`             | TEXT    | PRIMARY KEY, UUID v4 string                                        |
| `name`           | TEXT    | NOT NULL, unique after `lower(trim(name))`, 1..100 chars trimmed   |
| `type_id`        | TEXT    | NOT NULL, member of the RC-P1 registry                             |
| `enabled`        | INTEGER | NOT NULL, `0` or `1`, default `1`                                  |
| `currency`       | TEXT    | NOT NULL, validated per RC-P4                                      |
| `usd_rate`       | TEXT    | NOT NULL, RC-U5 decimal string                                     |
| `min_credit_usd` | TEXT    | NOT NULL, positive RC-U5-format decimal, default `"1"`             |
| `max_credit_usd` | TEXT    | NOT NULL, positive RC-U5-format decimal, `>= min_credit_usd`, default `"10000"` |
| `config_json`    | TEXT    | NOT NULL, JSON object validated per RC-P5                          |
| `sort_order`     | INTEGER | NOT NULL, default `0`                                              |
| `created_at`     | TEXT    | NOT NULL, RFC 3339 UTC                                             |
| `updated_at`     | TEXT    | NOT NULL, RFC 3339 UTC                                             |

RC-D1. `type_id` and `currency` are immutable after create; a `PUT` that
changes either MUST be rejected with HTTP `400`, code `invalid_request`.
Existing orders snapshot everything they need (RC-D4), so every other channel
field is freely editable.

RC-D2. Deleting a payment channel MUST NOT delete or mutate any
`recharge_orders` row. An order whose `payment_channel_id` no longer resolves
renders from its own snapshot columns. A notification addressed to a deleted
channel id resolves no channel and returns HTTP `404` (RC-N2).

### 3.2 `recharge_orders` table

| Column               | Type | Constraints                                                    |
|----------------------|------|-----------------------------------------------------------------|
| `id`                 | TEXT | PRIMARY KEY, 32 lowercase hex chars (UUID v4 without hyphens)  |
| `user_id`            | TEXT | NOT NULL                                                        |
| `payment_channel_id` | TEXT | NOT NULL (no FK; RC-D2)                                         |
| `channel_type_id`    | TEXT | NOT NULL, snapshot of `type_id` at creation                     |
| `channel_name`       | TEXT | NOT NULL, snapshot of `name` at creation                        |
| `status`             | TEXT | NOT NULL, one of `pending`, `succeeded`, `failed`, `expired`, `refunded` |
| `credit_nano_usd`    | TEXT | NOT NULL, canonical positive `i128` decimal string              |
| `pay_currency`       | TEXT | NOT NULL, snapshot                                              |
| `pay_amount`         | TEXT | NOT NULL, RC-U4 string, snapshot                                |
| `usd_rate`           | TEXT | NOT NULL, snapshot                                              |
| `provider_order_id`  | TEXT | NULL, provider-side identifier                                  |
| `error_code`         | TEXT | NULL, set when `status = failed` (RC-N5, RC-O8)                 |
| `paid_at`            | TEXT | NULL, RFC 3339 UTC, set exactly when the credit commits         |
| `expires_at`         | TEXT | NOT NULL, RFC 3339 UTC                                          |
| `meta_json`          | TEXT | NOT NULL, JSON object, default `{}`                             |
| `created_at`         | TEXT | NOT NULL, RFC 3339 UTC                                          |
| `updated_at`         | TEXT | NOT NULL, RFC 3339 UTC                                          |

Indexes: `(user_id, created_at)`, `(status, expires_at)`, and a non-unique
index on `provider_order_id`.

RC-D3. The 32-hex `id` doubles as the merchant order number sent to the
provider (EPay `out_trade_no`, Stripe `client_reference_id`). It fits EPay's
32-character `out_trade_no` limit and is globally unique.

RC-D4. Snapshot columns (`channel_type_id`, `channel_name`, `pay_currency`,
`pay_amount`, `usd_rate`, `credit_nano_usd`) are written once at creation and
MUST NOT be updated afterward.

RC-D5. The complete state-transition relation. Any transition not listed MUST
NOT occur:

| From        | To          | Trigger                                                            |
|-------------|-------------|--------------------------------------------------------------------|
| `pending`   | `succeeded` | verified success notification (§6) or reconciliation success (§7)  |
| `pending`   | `failed`    | verified failure notification, amount mismatch (RC-N5), user deleted (RC-N9), or adapter create failure (RC-O8) |
| `pending`   | `expired`   | expiry sweeper (§7) or verified `expired` notification             |
| `expired`   | `succeeded` | verified success notification or reconciliation success (late payment; money capture is authoritative) |
| `expired`   | `failed`    | amount mismatch (RC-N5) or user deleted (RC-N9) on a late verified notification |
| `succeeded` | `refunded`  | admin refund (§8)                                                  |

`failed` and `refunded` are terminal. `succeeded` transitions only to
`refunded`.

### 3.3 Ledger rows

RC-L1. `billing_ledger` is the single record of every wallet movement caused by
recharge. No parallel recharge-ledger table exists.

RC-L2. Two new ledger kinds:

| `kind`            | Delta sign | Meaning                                     |
|-------------------|-----------|----------------------------------------------|
| `recharge`        | positive  | Wallet credited by a succeeded recharge order |
| `recharge_refund` | negative  | Wallet debited by a refunded recharge order   |

RC-L3. A `recharge` ledger row MUST set
`idempotency_key = "recharge:{order_id}"`; a `recharge_refund` row MUST set
`idempotency_key = "recharge_refund:{order_id}"`. The existing partial unique
index `uidx_billing_ledger_idempotency_key`
(migration `m20260823_000033_billing_ledger_delta_dedupe`) therefore rejects a
second credit or a second refund for the same order at the storage layer.

RC-L4. `meta_json` of a `recharge` row MUST contain: `order_id`,
`payment_channel_id`, `channel_type_id`, `pay_currency`, `pay_amount`,
`usd_rate`, and `provider_order_id` (null allowed). A `recharge_refund` row
MUST contain the same fields plus `actor_user_id` (the admin who triggered the
refund) and `manual: boolean` (RC-R4).

## 4. Settings and environment

RC-G1. New system setting `recharge_public_origin` (string, default empty),
persisted and edited through the existing `GET/PUT /api/dashboard/settings`
flow. A non-empty value MUST be an absolute `http` or `https` origin with no
path, query, fragment, or trailing slash (example: `https://api.example.com`).
A malformed value MUST be rejected with HTTP `400`, code `invalid_request`.

RC-G2. Derived URLs:

- `notify_url = {recharge_public_origin}/api/pay/notify/{payment_channel_id}`
- `return_url = {recharge_public_origin}/dashboard/wallet?order_id={order_id}`
  (EPay `return_url`, Stripe `success_url`)
- `cancel_url = {recharge_public_origin}/dashboard/wallet?order_id={order_id}&canceled=1`
  (Stripe `cancel_url`)

RC-G3. While `recharge_public_origin` is empty, order creation MUST be rejected
with HTTP `409`, code `recharge_origin_unset`. Channel CRUD remains allowed.

RC-G4. Environment variables, each parsed as a positive integer with fallback
to its default on unset, empty, malformed, zero, or negative values:

- `MONOIZE_RECHARGE_ORDER_TTL_SECS` — order lifetime, default `3600`.
- `MONOIZE_RECHARGE_TICK_INTERVAL_SECS` — sweeper tick interval, default `60`.
- `MONOIZE_RECHARGE_MAX_PENDING_ORDERS` — per-user open-order cap, default `10`.

## 5. Order creation

RC-O1. Endpoint: `POST /api/dashboard/recharge/orders`. Authentication: any
authenticated dashboard user (the same session policy as
`GET /api/dashboard/auth/me`). The order's `user_id` is always the
authenticated user; a caller MUST NOT create an order for another user.

RC-O2. Request body: `payment_channel_id: string`, and one of
`credit_nano_usd: string` or `credit_usd: string` (when both are present the
nano value wins, mirroring `user-billing-and-model-metadata.spec.md` A3).

RC-O3. Validation order and error codes (first failure wins):

1. Unknown `payment_channel_id` → HTTP `404`, code `not_found`.
2. Channel `enabled = 0` → HTTP `409`, code `channel_disabled`.
3. `recharge_public_origin` empty → HTTP `409`, code `recharge_origin_unset`.
4. Amount missing, non-canonical, or `<= 0` → HTTP `400`, code
   `invalid_amount`.
5. Amount outside `[min_credit_usd, max_credit_usd]` (inclusive, compared in
   nano-USD after exact conversion) → HTTP `400`, code `amount_out_of_range`.
6. The user already has `>= MONOIZE_RECHARGE_MAX_PENDING_ORDERS` orders with
   `status = pending` → HTTP `429`, code `too_many_pending_orders`.

RC-O4. The pending-order count and the insert MUST execute in one write transaction while the user row is locked. On success the server inserts one `recharge_orders` row with
`status = pending`, `expires_at = now + MONOIZE_RECHARGE_ORDER_TTL_SECS`, and
the RC-D4 snapshots, then calls the adapter's `create_payment`.

RC-O5. Response body on success: `{ "order": <RC-A3 order object>,
"payment": { "kind": "redirect", "url": "<provider url>" } }`, HTTP `200`.

RC-O6. Order creation MUST NOT mutate `users.balance_nano_usd` and MUST NOT
append any ledger row. Balance changes happen only in §6 and §8 transactions.

RC-O7. Order creation MUST NOT create a request-log row (recharge traffic is
not proxy traffic; same principle as `balance-compatibility-api.spec.md`
BC-R4).

RC-O8. If `create_payment` fails (provider API error, config error), the
server MUST transition the just-created order `pending → failed` with
`error_code = "payment_init_failed"` and return HTTP `502`, code
`payment_init_failed`. The failed order row remains for audit.

## 6. Notification processing — exactly-once credit

RC-N1. Route: `/api/pay/notify/{payment_channel_id}`, methods GET and POST,
mounted only on primary nodes (replicas mount no such route, consistent with
`primary-replica-deployment.spec.md` D1). The route is outside dashboard
session authentication and outside API-key authentication. Authenticity comes
exclusively from adapter signature verification (RC-P2 item 2). Processing a
notification MUST NOT create a request-log row.

RC-N2. An unknown `payment_channel_id` MUST return HTTP `404` with an empty
body, before reading the request body and before any adapter call.

RC-N3. The adapter verifies the notification BEFORE any order lookup. A
`SignatureError` MUST return the adapter's `signature_error` ack and MUST NOT
read or write any order or balance state.

RC-N4. A verified notification whose `order_id` resolves no `recharge_orders`
row, or resolves an order whose `payment_channel_id` differs from the route's
channel id, MUST return the `unknown_order` ack and MUST NOT write anything.

RC-N5. Both payment adapters MUST return `paid_amount` and `paid_currency` for a verified success notification. If either field is absent, Monoize MUST roll back, leave the order unchanged, and return HTTP `500` so the provider retries. Amount check, evaluated before the success transition:
`paid_currency` MUST equal the order's `pay_currency` and `paid_amount` MUST
be numerically equal to the order's `pay_amount`. On mismatch with the order
in `pending` or `expired`, the server MUST transition the order to `failed`
with `error_code = "amount_mismatch"`, record the received values under
`meta_json.mismatch`, MUST NOT credit any balance, and MUST return the
`failed_recorded` ack (stopping provider retries; the operator resolves the
money difference manually). On mismatch with the order already `succeeded` or
`refunded`, the server returns the `duplicate` ack and writes nothing.

RC-N6. `result = success` with the amount check passed executes ONE credit
transaction on the billing write path
(`user-billing-and-model-metadata.spec.md` §6a: the single-connection write
pool on SQLite; `SELECT ... FOR UPDATE` row locks on PostgreSQL):

1. Lock the order row and re-read `status`.
2. If `status ∈ {succeeded, refunded}`: commit nothing, return the `duplicate`
   ack. This makes provider replays idempotent.
3. If `status = failed`: record the verified result under
   `meta_json.late_notification` for audit, credit nothing, and return the
   `failed_recorded` ack (`failed` is terminal per RC-D5).
4. If `status ∈ {pending, expired}`: set `status = succeeded`,
   `paid_at = now`, `provider_order_id` (when provided), `updated_at = now`;
   lock the user row; compute
   `new_balance = balance_nano_usd + credit_nano_usd` with checked `i128`
   arithmetic; write `new_balance`; append one `billing_ledger` row with
   `kind = "recharge"`, `delta_nano_usd = +credit_nano_usd`,
   `balance_after_nano_usd = new_balance`, the RC-L3 idempotency key, and the
   RC-L4 meta; commit.
5. After commit: invalidate the in-process balance cache entry for the user,
   then return the `credited` ack.

RC-N7. A user with `balance_unlimited = true` still receives step 4's finite
balance credit and ledger row (the stored finite value stays meaningful, per
`balance-compatibility-api.spec.md` BC-D5). Recharge never changes
`balance_unlimited`.

RC-N8. The order row lock plus the write-pool serialization is the primary
exactly-once mechanism; two concurrent success notifications for one order
serialize, and the second observes `succeeded` and takes the `duplicate`
branch. The RC-L3 unique idempotency key is the independent second barrier: if
a ledger insert conflicts, the transaction MUST roll back completely (order
stays in its prior state, balance unchanged) and the handler MUST return the
`duplicate` ack.

RC-N9. A deleted user: if the locked user row is absent in step 4, the
transaction MUST roll back, the order MUST stay in its prior state, and the
handler returns the `failed_recorded` ack after separately transitioning the
order to `failed` with `error_code = "user_deleted"`.

RC-N10. A checked-arithmetic overflow or storage error in step 4 MUST roll
back the transaction, leave the order `pending`/`expired`, and return HTTP
`500` with an empty body (NOT a success ack), so the provider retries and the
credit is not silently lost.

RC-N11. `result = failure` with the order in `pending` transitions it to
`failed` with `error_code = "provider_failure"` and returns the
`failed_recorded` ack. With the order in any other state it returns the
`duplicate` ack and writes nothing. `result = expired` with the order in
`pending` transitions it to `expired`; in any other state it writes nothing.

## 7. Expiry and reconciliation

RC-X1. A background sweeper runs one tick every
`MONOIZE_RECHARGE_TICK_INTERVAL_SECS` seconds on primary nodes only. The first
tick runs when background tasks start.

RC-X2. Each tick transitions every order with `status = pending` and
`expires_at <= now` to `expired`. Expiry writes no ledger row and mutates no
balance.

RC-X3. Expiry does not forfeit paid money: a later verified success
notification (or reconciliation success) on an `expired` order credits the
wallet via the unchanged RC-N6 transaction (RC-D5 row `expired → succeeded`).

RC-X4. For each adapter with `supports_query = true`, each tick additionally
calls `query_order` at most once per order for orders with `status = pending`
and `created_at <= now - 60s`. A `success` answer executes the RC-N6
transaction (the amount check RC-N5 is skipped; the provider's stored order is
queried by our own `order_id`, so the amount is ours by construction). A
`failure` answer transitions `pending → failed` with
`error_code = "provider_failure"`. `still_pending` and `unknown` write
nothing. Version 1 has no adapter with this capability; the rule binds future
adapters.

## 8. Refunds

RC-R1. Endpoint: `POST /api/dashboard/recharge/orders/{order_id}/refund`.
Authentication: admin session (`session_helpers::require_admin`, same policy
as `security-access-control.spec.md` SAC-4). Non-admin callers receive the
standard `401`/`403` mapping.

RC-R2. Version 1 supports full-order refunds only. There is no partial-refund
amount parameter.

RC-R3. Preconditions: the order MUST have `status = succeeded`, otherwise HTTP
`409`, code `invalid_order_state`.

RC-R4. Provider interaction depends on the adapter capability:

- `supports_refund = true` (stripe): the server first calls the adapter's
  `refund`. Only after the provider confirms does the RC-R5 transaction run. A
  provider refund error returns HTTP `502`, code `refund_failed`, and writes
  nothing. The ledger meta records `manual = false`.
- `supports_refund = false` (epay): the request body MUST contain
  `manual: true`, by which the admin asserts the money was returned out of
  band; otherwise HTTP `400`, code `manual_refund_required`. The RC-R5
  transaction then runs. The ledger meta records `manual = true`.

RC-R5. The refund transaction, on the billing write path:

1. Lock the order row; re-check `status = succeeded` (a concurrent refund
   observes `refunded` and returns HTTP `409`, code `invalid_order_state`).
2. Set `status = refunded`, `updated_at = now`.
3. Lock the user row; compute
   `new_balance = balance_nano_usd - credit_nano_usd` with checked `i128`
   arithmetic. The result MAY be negative (consistent with
   `user-billing-and-model-metadata.spec.md` B6/L4: debt is representable).
4. Append one `billing_ledger` row with `kind = "recharge_refund"`,
   `delta_nano_usd = -credit_nano_usd`, `balance_after_nano_usd = new_balance`,
   the RC-L3 idempotency key, and the RC-L4 meta.
5. Commit, then invalidate the user's balance cache entry.

A refund of a deleted user's order MUST return HTTP `409`, code
`invalid_order_state`, and write nothing.

RC-R6. Provider-initiated refunds (for example a Stripe refund created in the
Stripe dashboard) are NOT synchronized in version 1: per RC-T3, non-checkout
events are acknowledged and ignored. Operators reconcile such refunds through
this endpoint.

## 9. Dashboard APIs

### 9.1 User-facing

RC-A1. `GET /api/dashboard/recharge/channels` — any authenticated user.
Returns `{ "channels": [...] }` containing only rows with `enabled = 1`,
ordered by `sort_order ASC`, then `created_at ASC`. Each object exposes
exactly: `id`, `name`, `type_id`, `currency`, `usd_rate`, `min_credit_usd`,
`max_credit_usd`, `pay_scale` (the RC-P4 scale integer). `config_json` MUST
NOT appear in any form.

RC-A2. `GET /api/dashboard/recharge/orders` — role-scoped exactly like
`request-logs.spec.md` RL-API1: `admin`/`super_admin` see all users' orders
and MAY filter with `username`; role `user` sees only own orders and the
`username` parameter is ignored. Query parameters: `limit` (default 20, max
100), `offset` (default 0), `status` (optional, one of the §3.2 status values;
anything else → HTTP `400`, code `invalid_request`), `username` (optional,
admin only). Response: `{ "orders": [...], "total": <matching count> }`,
ordered `created_at DESC`, then `id DESC`.

RC-A3. Each order object contains: `id`, `user_id`, `username` (joined; null
after user deletion), `payment_channel_id`, `channel_type_id`, `channel_name`,
`status`, `credit_nano_usd`, `credit_usd` (canonical U2 formatting),
`pay_currency`, `pay_amount`, `usd_rate`, `provider_order_id`, `error_code`,
`paid_at`, `expires_at`, `created_at`. `username` is included only for admin
callers.

RC-A4. `GET /api/dashboard/recharge/orders/{order_id}` — returns one RC-A3
object. Role `user` receives HTTP `404`, code `not_found`, for another user's
order (existence is not disclosed).

RC-A5. `GET /api/dashboard/ledger` — the billing-ledger read surface.
Role-scoped like RC-A2. Query parameters: `limit` (default 20, max 100),
`offset` (default 0), `kinds` (optional comma-separated list; each entry MUST
match `^[a-z_]{1,64}$`, otherwise HTTP `400`, code `invalid_request`; entries
that match no stored kind simply select nothing), `username` (optional, admin
only). Response: `{ "entries": [...], "total": <matching count> }`, ordered
`created_at DESC`, then `id DESC`. Each entry contains: `id`, `user_id`,
`username` (admin callers only), `kind`, `delta_nano_usd`, `delta_usd`
(canonical U2 formatting), `balance_after_nano_usd` (null allowed),
`meta_json` (parsed object), `created_at`.

### 9.2 Admin payment-channel CRUD

All endpoints require an admin session (RC-R1 policy).

- `GET /api/dashboard/payment-channels` — all rows ordered by `sort_order ASC`,
  `created_at ASC`, with RC-P6 secret masking applied to `config_json`.
- `POST /api/dashboard/payment-channels` — create. Body fields: `name`,
  `type_id`, `currency`, `usd_rate`, optional `min_credit_usd`,
  `max_credit_usd`, `enabled` (default `true`), `sort_order` (default `0`),
  and `config` (object, validated per RC-P5).
- `PUT /api/dashboard/payment-channels/{id}` — update any mutable field
  (RC-D1); omitted fields keep stored values; secret handling per RC-P6.
- `DELETE /api/dashboard/payment-channels/{id}` — delete; RC-D2 governs
  surviving orders. Delete always succeeds for an existing row.

RC-A6. CRUD validation error codes: duplicate name (case-insensitive, races
included) → HTTP `409`, code `channel_name_exists`; unknown `type_id` → HTTP
`400`, code `invalid_channel_type`; RC-P4 violation → HTTP `400`, code
`invalid_currency`; malformed `usd_rate`, `min_credit_usd`, `max_credit_usd`,
or `min > max` → HTTP `400`, code `invalid_rate`; RC-P5 config violation →
HTTP `400`, code `invalid_channel_config`; unknown id on `PUT`/`DELETE` →
HTTP `404`, code `not_found`.

## 10. Wallet page (`/dashboard/wallet`)

RC-W1. `/dashboard/wallet` is a main-navigation page for every authenticated
user (RC-S2 amendment 1). It renders, top to bottom: a page heading, one wallet
stage that contains the prepaid balance and recharge controls, a plan-capacity
card, and one activity card. The activity card contains `orders` and `ledger`
tabs. `orders` is selected on first render.

RC-W2. The balance region reads the session user object (`balance_usd`,
`balance_unlimited`) exactly like `dashboard-ui-layout.spec.md` DL3a/US2,
without extra fetches. It labels this value as prepaid balance. A balance value
change replaces the old value with the new value in the same bounded region.
The balance region MUST use the wallet ink surface and wallet foreground tokens
in both light and dark themes.

RC-W2a. The plan-capacity card MUST load `GET /api/dashboard/billing-plan-subscription` and `GET /api/dashboard/billing-plans/marketplace` through SWR. It MUST render skeleton content while either request loads. When a subscription is active, it MUST show its name, description, expiry, eligible groups, and every configured sliding-window remaining value. When no subscription is active, it MUST show every listed plan price and allow purchase. A successful purchase MUST revalidate the subscription, session user, and ledger caches without a page close or reload.

RC-W3. The recharge card:

- loads channels from RC-A1 through an SWR hook and renders skeleton rows
  while loading (AGENTS.md §4);
- when zero enabled channels exist, renders a localized empty state and no
  amount controls;
- renders a channel selector, preset amount buttons for `5`, `10`, `25`, `50`,
  and `100` USD, and one custom-amount input. A preset outside the selected
  channel's `[min_credit_usd, max_credit_usd]` MUST be disabled;
- shows the computed `pay_amount` and `pay_currency` for the entered amount
  using RC-U6 with `BigInt`/exact-decimal arithmetic before submission;
- on submit, POSTs RC-O2, optimistically inserts the returned `pending` order
  at the top of the orders SWR cache, and navigates the browser to
  `payment.url` (top-level navigation, not an iframe);
- surfaces every RC-O3/RC-O8 error code as a localized toast and keeps the
  entered state intact.

RC-W4. The recharge-orders section lists the caller's RC-A2 orders (newest
first) in the activity card's `orders` tab. Each order item shows created time
(FL2 format), channel name, credit (`credit_usd`), payment (`pay_amount` +
`pay_currency`), status badge, and order id (first 8 chars, full id in a
tooltip). While any loaded own order has `status = pending`, the SWR hook MUST
poll with `refreshInterval = 5000`; when none is pending, polling MUST stop. A
`?order_id=` query parameter (RC-G2 return URL) highlights the matching item;
state correctness MUST rely on polling, never on return-URL parameters.

RC-W5. The activity card's `ledger` tab lists the caller's RC-A5 entries. Each
ledger item shows created time, kind (localized label), delta (`delta_usd`,
signed, semantic success color for positive and semantic destructive color for
negative), and balance after. The default `kinds` filter sent by the page is exactly
`recharge,recharge_refund,admin_adjustment,plan_purchase,sub_account_transfer_out,sub_account_transfer_in,sub_account_refund,sub_account_debt_transfer,sub_account_delete_settlement,admin_sub_account_adjustment`
(every non-per-request kind); a kind filter control MAY narrow it. Per-request
charges (`request_charge`, `api_key_charge`) are intentionally excluded here —
they remain on `/dashboard/logs` (§12).

RC-W6. Both activity tabs remain mounted after the page renders. They render
skeleton rows while loading and load further pages with `limit`/`offset`
paging. A completed recharge MUST appear (order `succeeded`, ledger `recharge`
row, updated sidebar balance via session-user revalidation) without a page
close/reopen: the pending-order poll that observes the terminal status MUST
also revalidate the ledger cache and the session user cache.

RC-W7. The wallet page MUST use mobile-first layout. The balance and recharge
regions stack at widths below the `lg` breakpoint and render in two columns at
or above `lg`. Order and ledger items MUST remain readable without horizontal
page scrolling.

RC-W8. Wallet page entry, balance replacement, amount selection, pay-preview
replacement, plan-meter updates, activity-tab changes, and activity-item entry
MUST use non-linear spring transitions. A tab change MUST move the entering
content horizontally by at most 32 CSS pixels and MUST keep the activity card
in the document flow. If `prefers-reduced-motion: reduce` matches, these
transitions MUST use no x-offset, y-offset, scale, or rotation animation.

## 11. Payments admin page (`/dashboard/payments`)

RC-M1. `/dashboard/payments` is an admin-navigation page (RC-S2 amendment 2)
with two tabs: Channels and Orders.

RC-M2. The Channels tab lists every §9.2 row (name, `type_id`, currency,
`usd_rate`, credit bounds, enabled switch) and offers create, edit, and delete
dialogs. The enabled switch applies an SWR optimistic update and rolls back
with a toast on error. Secret config inputs render empty with a localized
"stored, enter to replace" placeholder (RC-P6). The create/edit dialog derives
its config fields from the RC-P5 schema of the selected `type_id`. Delete
opens a confirmation dialog naming the channel.

RC-M3. The Orders tab renders the admin view of RC-A2 (all users) with
`status` and `username` filters and these columns: created time (FL2 format),
username, channel name, credit (`credit_usd`), payment (`pay_amount` +
`pay_currency`), status badge, and order id (first 8 chars; the tooltip shows
the full id and, when set, `error_code`). Each row has a refund action enabled
only when `status = succeeded`. The refund action opens a confirmation dialog;
for a `supports_refund = false` channel the dialog contains the RC-R4 manual
acknowledgment checkbox and blocks confirm until checked. Mutations
optimistically update the orders cache and roll back on error with the server
message.

## 12. Placement relative to request logs

RC-Q1. Recharge orders and ledger entries MUST NOT be fused into
`/dashboard/logs`. `/dashboard/logs` renders per-proxy-request rows under the
`request-logs.spec.md` FL contract (fixed 44px rows, token/timing columns,
SSE `pending` lifecycle). Wallet events have a different schema (kind, signed
delta, balance-after, order state machine), a different lifecycle (webhook and
polling driven, no SSE), and a volume lower by orders of magnitude. Mixing
them would break the FL column contract and force nullable token fields onto
non-request rows.

RC-Q2. The division of billing visibility is:

- per-request spend: `/dashboard/logs` (`charge_nano_usd` column and filtered
  `total_charge_nano_usd` per FL7a) — unchanged;
- wallet-level movements (recharge, refunds, admin adjustments, plan grants,
  sub-account transfers): `/dashboard/wallet` ledger section (RC-W5), backed
  by `billing_ledger`, which per-request charges also write — the ledger is
  the reconciliation superset, the two pages are disjoint default views of it.

RC-Q3. `GET /api/dashboard/request-logs` and its SSE stream MUST NOT emit
recharge rows, and `GET /api/dashboard/ledger` MUST NOT join request-log data.

## 13. Security

RC-C1. Signature comparisons in RC-E3 and RC-T2 MUST be constant-time.

RC-C2. Secret config values MUST NOT appear in any HTTP response (RC-P6), any
log line, any ledger `meta_json`, or any order `meta_json`.

RC-C3. The notify route MUST NOT reflect request input into responses; every
response body is one of the fixed RC-E5/RC-T4 strings or empty.

RC-C4. RC-A1 is the only non-admin channel read, and it exposes no
`config_json` field. Notification processing never trusts client-supplied
amounts for crediting: the credited value is always the stored
`credit_nano_usd` (RC-N5 only verifies, never substitutes).

## 14. Migration

RC-V1. Migration step `m20260910_000050_recharge_system` creates
`payment_channels` and `recharge_orders` with the §3 columns and indexes. It
MUST NOT alter `billing_ledger` (the `idempotency_key` column and unique index
already exist) and MUST NOT alter `users`.

RC-V2. The step is purely additive; `down` drops the two tables.

## 15. Test matrix

Automated tests shipped with the implementation MUST cover at least:

- T1. RC-U6 conversion: `credit_usd = "10"`, `usd_rate = "7.30"`, CNY →
  `pay_amount = "73.00"`; `usd_rate = "7.333333333"` → `pay_amount = "73.34"`
  (ceiling); JPY via stripe scale 0 → integer string.
- T2. Order creation validation: each RC-O3 code, including the pending cap.
- T3. EPay sign round-trip: RC-E2 signing then RC-E3 verification; a mutated
  `money` parameter fails verification.
- T4. Stripe signature: valid `v1` HMAC accepted; stale timestamp (> 300 s)
  rejected; wrong secret rejected.
- T5. Exactly-once: two sequential verified success notifications for one
  order produce exactly one balance credit and one ledger row; the second
  returns the `duplicate` ack.
- T6. Race: two concurrent success notifications (write-pool serialized)
  produce exactly one credit; direct ledger insert with a duplicated
  idempotency key aborts and rolls back the order transition (RC-N8).
- T7. Amount mismatch: notification with a different `money` transitions the
  order to `failed` with `error_code = "amount_mismatch"` and credits nothing.
- T8. Expiry: sweeper expires a stale `pending` order; a later verified
  success on the `expired` order credits once (RC-X3).
- T9. Refund: full flow debits `credit_nano_usd` into a possibly negative
  balance, writes `recharge_refund` with `manual` flag; a second refund
  attempt returns `invalid_order_state`.
- T10. Role scoping: role `user` sees only own orders/ledger entries; the
  `username` filter is ignored for role `user` and honored for admins.
- T11. Secret masking: channel `GET` returns empty secret fields; `PUT` with
  empty secret keeps the stored value; `PUT` with a new value replaces it.
- T12. Notify surface: unknown channel id → 404; invalid signature → adapter
  `signature_error` ack with no state change; unknown order → `unknown_order`
  ack with no state change.
