# User Billing and Model Metadata Specification

## 0. Status

- Product name: Monoize.
- Scope:
  - user-level balance and post-response billing on proxy requests;
  - model metadata storage and Models.dev metadata sync;
  - admin-only balance mutation.

## 1. Precision and storage rules

B1. Balance unit MUST be nano-dollar (`1 USD = 1_000_000_000 nano_usd`).

B2. Persistent balance MUST use signed integer nano-dollar string storage in `users.balance_nano_usd` (`TEXT` column), not floating point.

B3. User balance unlimited switch MUST be persisted in `users.balance_unlimited` (`INTEGER` column, `0|1`).

B4. A persisted `balance_nano_usd` value that is not a signed base-10 `i128` integer MUST produce an explicit storage error. A read path MUST NOT substitute zero for an invalid persisted balance.

B4. Decimal USD inputs MUST accept up to 9 fractional digits. Values with more than 9 fractional digits MUST be truncated toward zero when converted to nano-dollar.

B5. Balance arithmetic MUST use checked integer operations. Overflow MUST return `500 internal_error`.

B6. Settlement of an admitted request MAY make a finite user or API-key sub-account balance negative. Monoize MUST NOT reserve or pre-deduct an estimated request cost before upstream forwarding.

## 2. User data model

U1. User read model exposed by dashboard/auth APIs MUST include:

- `balance_nano_usd: string`
- `balance_usd: string`
- `balance_unlimited: boolean`
- `email: string | null`
- `group_id: string` (the user's single group id, see `groups-registry.spec.md`)
- `billing_plan_id: string | null`
- `next_grant_at: string | null` (RFC 3339)
- `billing_plan: object | null` as defined by `spec/billing-plan-subscriptions.spec.md` BP-U2

U2. `balance_usd` MUST be computed from `balance_nano_usd` with nano precision and no binary floating conversion.

U2.1. Nano-dollar formatting MUST cover the complete signed `i128` domain, including `i128::MIN`, without panic or lossy conversion.

U3. New users created by register or dashboard create-user MUST default to:

- `balance_nano_usd = "0"`
- `balance_unlimited = false`
- `email = null`
- `group_id = <default group id>` (`groups-registry.spec.md` GR-D2)

U4. Usernames with prefix `_monoize_` (case-insensitive) are reserved for internal system accounts and MUST NOT be allowed in public register/login flows or admin create/update username operations.

U5. Internal reserved users (`username` prefix `_monoize_`) MUST be excluded from user list APIs and user-count metrics used by dashboard/admin UI.

U6. Monoize active-probe subsystem MUST ensure an internal user `_monoize_active_probe` exists before each probe attempt and MUST force this user to `balance_unlimited = true`.

U7. `email` is an optional field (`TEXT NULL` in SQLite). When set, it MUST be a non-empty string. The server MUST NOT validate email format beyond non-emptiness; the field is used solely for Gravatar URL generation.

U8. Any authenticated user MAY update their own `email` field via `PUT /api/dashboard/auth/me` with optional body field `email: string | null`. Setting `email` to `null` or empty string MUST clear the stored value.

U9. Admin users MAY also update a user's `email` via `PUT /api/dashboard/users/{user_id}` with optional body field `email: string | null`.

U10. Dashboard frontend MUST generate Gravatar URLs from user email using the MD5 hash of the lowercase-trimmed email, per the Gravatar protocol (`https://www.gravatar.com/avatar/{md5}?d=identicon&s={size}`). If the user has no email set, the frontend MUST fall back to displaying the first character of the username as the avatar.

U11. Public registration MUST read the `registration_enabled` setting and call one atomic `UserStore` registration operation. One process-local async critical section shared by every `UserStore` clone MUST contain the non-internal user count, first-user role decision, duplicate-username check, and user insert. If the count is zero, that operation MUST create exactly one concurrent caller as `super_admin` even when registration is disabled. Every later caller MUST be created as `user` only when registration is enabled; otherwise it MUST perform no insert and report `registration_disabled`. A present but malformed persisted `registration_enabled` value MUST abort registration with a storage error before the atomic operation and MUST NOT be interpreted as enabled.

U12. Username and password syntax validation MUST finish before the atomic registration operation. The operation MUST report a duplicate username separately from storage failure so the endpoint preserves HTTP `409 username_exists`.

U13. A mutation of `users.password_hash` or `users.role` MUST delete every `sessions` row for that user in the same transaction. A transition of `users.enabled` to `false` MUST do the same. A later transition of `users.enabled` to `true` MUST NOT restore a deleted session.

U14. Any authenticated user MAY change their password through `PUT /api/dashboard/auth/password`. The request body MUST contain `current_password: string` and `new_password: string`. The endpoint MUST reject a `new_password` shorter than 8 characters with HTTP `400` and code `invalid_password`. The endpoint MUST reject an incorrect `current_password` with HTTP `401` and code `invalid_current_password`.

U15. A successful self-service password change MUST update `users.password_hash`, delete every existing session for the user, and insert one replacement session in one transaction. The replacement session MUST use the configured session TTL. The response MUST return the replacement token and user object with the same schema as login. The response MUST also replace the `monoize_session` cookie. The old bearer token and all other old session tokens MUST fail authentication after the transaction commits.

U16. The authenticated user settings page MUST provide current-password, new-password, and confirmation fields. The page MUST prevent submission when the new password is shorter than 8 characters or differs from the confirmation. After a successful response, the page MUST keep the replacement session active, clear all three password fields, and report that other sessions were signed out.

## 3. Admin mutation rules

A1. Only admin/super-admin endpoints MAY mutate user balance fields.

A2. `PUT /api/dashboard/users/{user_id}` MUST accept optional fields:

- `balance_nano_usd: string`
- `balance_usd: string`
- `balance_unlimited: boolean`
- `email: string | null`
- `group_id: string`

A2a. `POST /api/dashboard/users` MUST accept optional field `group_id: string`. If the field is omitted, the stored value MUST be the default group id.

A2b. `PUT /api/dashboard/users/{user_id}` MUST treat `group_id` as a partial-update field:

- if `group_id` is omitted, the stored value MUST remain unchanged;
- if `group_id` is present, the stored value MUST be replaced by that value.

A2c. Any dashboard/admin write path that persists `users.group_id` MUST trim the value and validate that it references an existing `monoize_groups` row; a violation MUST be rejected with HTTP `400` and code `invalid_request`.

A3. If both `balance_nano_usd` and `balance_usd` are provided, server MUST use `balance_nano_usd`.

A4. Balance mutation by admin MUST write one ledger entry with type `admin_adjustment`.

A5. `PUT /api/dashboard/users/{user_id}` MUST apply ordinary user fields, `balance_nano_usd`, `balance_unlimited`, and the A4 ledger row in one database transaction through one `UserStore` operation. Password hashing, balance parsing, group serialization, and other deterministic validation MUST finish before the transaction begins. A database or ledger failure MUST roll back every field change. Cache invalidation MUST occur only after commit; API-key cache entries for the user MUST be invalidated for any persisted field change, and the balance cache entry MUST additionally be invalidated when either balance field changes.

## 4. Billing eligibility

BE1. Billing applies only when the request is authenticated by database API key (resolved `user_id` exists).

BE2. Requests authenticated only by static config keys MUST NOT be billed.

BE3. Before upstream forwarding, billing eligibility MUST be checked as follows:

- If the authenticated API key has `sub_account_enabled = 1`: check `sub_account_balance_nano > 0`. If not, return HTTP `402` with code `insufficient_balance`. The user's balance is NOT checked.
- Otherwise (API key inherits user balance): if `balance_unlimited = false` and `balance_nano_usd <= 0`, server MUST return HTTP `402` with code `insufficient_balance`.

BE3.1. BE3 is an admission gate, not a final-cost affordability check. Multiple requests admitted while the same balance is positive MAY settle to a negative balance. Once the persisted balance is non-positive, a later request MUST fail BE3 until an administrator or transfer makes the balance positive again.

BE3.2. The finite user-balance admission read MUST query the database. It MUST NOT authorize a request from a cached positive balance after a prior settlement has committed a non-positive balance.

BE4. The legacy `ensure_quota_before_forward` per-call quota check MUST NOT exist. Sub-account billing replaces it entirely (see `api-key-sub-account-billing.spec.md`).

BE5. Monoize MUST determine whether selected candidate attempts have billable pricing before enforcing the pre-forward balance gate. If no candidate attempt has billable pricing under C1.2, Monoize MUST reject the request with HTTP `403` and code `model_pricing_required` before the balance gate. This rule applies to all roles, including `admin` and `super_admin`.

## 5. Charge calculation

C1. Charge requires both:

- normalized upstream response usage, and
- a complete applicable `billing_rate_records` matrix resolved under C1.2.

C1a. `model_metadata_records` MAY store legacy token prices and model limits. Metadata writes and Models.dev sync MUST mirror present token prices into `billing_rate_records`, but billing computation MUST read `billing_rate_records`.

C1.1. Served upstream model resolution for request execution and billing metadata:

- if the selected Channel model mapping has non-empty `redirect`, Monoize MUST send that `redirect` upstream and MUST record it as `upstream_model`;
- otherwise Monoize MUST use the requested logical model as `upstream_model`.

C1.2. Pricing model resolution for billing:

- Before each pricing lookup candidate in this section, Monoize MUST normalize that candidate to a `pricing_model_key` by removing at most one recognized reasoning-tier suffix from the end of the model ID. If no recognized suffix matches, `pricing_model_key` MUST equal the original candidate.
- Recognized reasoning-tier suffixes MUST use the same suffix set and longest-suffix-first matching rule as `reasoning_suffix_map` plus the built-in effort suffixes defined in `model-metadata-dashboard.spec.md` § 8.
- For each candidate key, Monoize MUST select the pricing profile via ordered `pricing_profile_model_patterns` from `metered-billing.spec.md` § 2.
- Monoize MUST first look up eligible rate rows for the normalized `upstream_model` key derived from C1.1.
- If `upstream_model` came from a non-empty `redirect` and that normalized lookup does not yield complete rates, Monoize MUST retry rate lookup with the normalized requested logical model key.
- If the normalized requested logical model key equals the normalized `upstream_model` key, Monoize MUST NOT perform a second lookup.
- If neither lookup yields complete rates, the request has no billable pricing.

C2. Base charge formula (nano-dollar):

```
base_charge = sum(token_line_items.charge_nano) + sum(meter_line_items.charge_nano)
```

Token line items and meter line items are defined by `metered-billing.spec.md`.

C3. `usage.input_tokens` on the internal `Usage` model MUST be interpreted as an aggregate/inclusive prompt total. That is, `input_tokens` MUST be the sum of base-rate prompt tokens, cache-read prompt tokens, and cache-creation prompt tokens. Cache-class counters (`cache_read_tokens`, `cache_creation_tokens`) are refinements of that total, not disjoint additive buckets.

C3-i. Upstream providers whose native usage field is already aggregate/inclusive (for example OpenAI Chat Completions and OpenAI Responses) MUST map their prompt total directly to `input_tokens`.

C3-ii. Upstream providers whose native usage field excludes cache buckets (for example Anthropic Messages, where the wire `input_tokens` is the non-cached remainder and `cache_read_input_tokens` / `cache_creation_input_tokens` are reported as disjoint buckets) MUST be normalized at decode time so that the internal `Usage.input_tokens` equals `wire_input_tokens + cache_read_input_tokens + cache_creation_input_tokens`. The native wire semantics MUST be reconstructed at encode time by subtracting cache buckets back out (saturating at zero) before writing `input_tokens` to any downstream Anthropic-format response, SSE `message_start`, or SSE `message_delta` payload.

C3-iii. With C3-i and C3-ii in effect, all billing and logging code paths MUST treat `Usage.input_tokens` uniformly as aggregate/inclusive. Provider-type branching on the interpretation of `input_tokens` MUST NOT exist in billing computation, usage-breakdown construction, or request-log projection.

C3-iii-a. If a provider reports tool-result prompt tokens as a disjoint counter, decode MUST add that counter to `Usage.input_tokens` and MUST also preserve it in `usage.input_details.tool_prompt_tokens`. Billing MUST NOT add the detail counter a second time.

C3-iv. If `usage.input_details.cache_read_tokens` is present and a matching `cache_read` rate exists, input charge MUST be:

```
input_charge =
  (input_tokens - cache_read_tokens) * input_uncached_rate
  + cache_read_tokens * cache_read_rate
```

C3a. If `usage.input_details.cache_creation_tokens` is present, billing MUST handle cache-write tokens according to `metered-billing.spec.md` § 4:

```
cache_creation_tokens = cache_creation_5m_tokens + cache_creation_1h_tokens
```

When cache-write billing applies, the input charge computed in C2/C3 MUST first exclude `cache_creation_tokens` from the base-rate input bucket before adding cache-write line items. Formally:

``` 
base_rate_input_tokens = input_tokens - cache_read_tokens - cache_creation_tokens
input_charge =
  base_rate_input_tokens * input_cost_per_token_nano
  + cache_read_tokens * cache_read_input_cost_per_token_nano
  + cache_creation_5m_tokens * cache_write_5m_rate
  + cache_creation_1h_tokens * cache_write_1h_rate
```

The implementation MUST clamp each billable bucket at zero after subtraction. Monoize MUST NOT charge the same input token once at the base input rate and again at a cache-write rate. If a rate matrix requires both 5-minute and 1-hour cache-write classes and upstream usage provides only aggregate cache creation, Monoize MUST reject billing with HTTP `403` and code `model_pricing_required`.

C4. If `usage.output_details.reasoning_tokens` is present and a matching `reasoning_output` rate exists, output charge MUST be:

```
output_charge =
  (output_tokens - reasoning_tokens) * output_rate
  + reasoning_tokens * reasoning_output_rate
```

`Usage.output_tokens` MUST be an aggregate/inclusive output total. If a provider reports visible candidate tokens and reasoning/thinking tokens as disjoint counters, decode MUST set `Usage.output_tokens` to their checked sum and MUST preserve the reasoning counter in `usage.output_details.reasoning_tokens`. If a provider already reports an inclusive output total, decode MUST use it directly. Billing MUST subtract the reasoning detail once before applying the base output rate and MUST NOT bill the same output token twice.

C5. Final charge MUST multiply by the selected Channel model multiplier and truncate toward zero:

```
final_charge_nano = trunc(base_charge * channel_model_multiplier)
```

C6. If C1.2 yields no billable rates, Monoize MUST reject the request with HTTP `403` and code `model_pricing_required`.

C6.1. `build_monoize_attempts()` SHOULD prevent C6 from being reached by filtering unbillable attempts before upstream forwarding.

C6.2. If C6 is reached during post-response billing, Monoize MUST NOT write any charge ledger row for that request.

C6.3. Missing pricing or missing meter rates MUST NOT be bypassed by `admin` or `super_admin`.

C7. For embeddings responses, billing MUST treat usage as:

- `input_tokens = usage.input_tokens`
- `output_tokens = 0`

## 6. Billing execution and ledger

L1. Billing deduction MUST run after successful non-stream proxy response decode.

L2. For pass-through streaming requests, Monoize MUST continue consuming the upstream stream until one of these conditions is true:

- upstream sends a protocol terminal frame that contains usage;
- upstream sends a protocol terminal frame that proves no more usage can arrive;
- upstream closes the response body;
- the configured upstream stream idle timeout is reached.

L2.1. A downstream client disconnect MUST NOT by itself stop upstream stream consumption. After downstream disconnect, Monoize MAY discard encoded SSE frames that can no longer be delivered, but MUST continue decoding upstream events for usage, terminal diagnostics, request logging, request capture, and billing.

L2.2. If upstream terminal usage is received after downstream disconnect, Monoize MUST bill from that upstream usage using the same charge calculation as a normally completed stream.

L2.3. A billable successful response MUST NOT silently become free because authoritative usage is absent. Non-stream handling MUST reject such a response before delivering it when no deterministic estimate exists. Pass-through streaming MUST maintain an input/output estimate while forwarding; if terminal usage is absent, it MUST settle from that estimate and mark the billing breakdown as estimated.

L2.4. A pass-through stream has already committed its HTTP status before post-response settlement. A settlement storage or pricing error therefore MUST be recorded as a billing failure and MUST NOT be replaced with a successful zero-charge computation. This rule does not permit an insufficient-balance error during settlement because L4 requires admitted requests to settle into negative balances.

L2a. Requests that return a normal model response payload (including truncated/cutoff completions such as `finish_reason = "length"`) MUST be treated as billable-success requests, not failed requests. A request whose downstream client disconnected after admission but whose upstream attempt still produced that normal payload MUST still be billed; its request-log status is `"client_gone"` per `request-logs.spec.md` RL1h, which is not an API-error failure under L2b.

L2b. Requests that terminate as API errors (`4xx`/`5xx` error response) MUST NOT be billed.

L3. On successful deduction, server MUST append a ledger row with:

- `user_id`
- `kind = "request_charge"` (user balance) or `kind = "api_key_charge"` (sub-account)
- `delta_nano_usd` (negative value)
- `balance_after_nano_usd`
- `meta_json` (at minimum request_id, model, provider_id, prompt/completion/reasoning/cached tokens; sub-account charges MUST also include `api_key_id`)

L4. Settlement of an admitted request MUST subtract the complete final charge with checked arithmetic even when the resulting finite balance is negative. Settlement MUST append the corresponding ledger row in the same transaction. HTTP `402 insufficient_balance` applies only to the pre-forward BE3 admission gate and MUST NOT be produced merely because settlement crosses zero.

## 6a. Billing concurrency control

LC1. The application MUST use two SQLite pools against the same DSN: a read pool (`max_connections = 10`) and a write pool (`max_connections = 1`).

LC2. All balance-mutating operations (request charges and admin adjustments) MUST execute on the write pool.

LC3. Balance reads used for eligibility and analytics MAY execute on the read pool.

LC4. The write pool's single connection is the required serialization mechanism for billing writes; an additional application-level billing mutex MUST NOT be required.

LC4.1. PostgreSQL balance mutations MUST lock every user and API-key balance row that participates in the mutation with `SELECT ... FOR UPDATE` in deterministic user-then-API-key order. Each mutation MUST compute balances, write balances, and append all ledger rows in one transaction.

LC5. The billing charge path (`charge_user_balance_nano`) MUST execute a single attempt and MUST NOT include an explicit retry loop. Error behavior for non-transient failures remains unchanged. This clause scopes the primary-role synchronous path; a replica node enqueues balance deltas instead per `primary-replica-deployment.spec.md` M3.

## 7. Model metadata store

M1. Server MUST persist model metadata in table `model_metadata_records`.

M2. Primary key MUST be `model_id`.

M3. Table MUST contain at least:

- `model_id: TEXT`
- `models_dev_provider: TEXT`
- `mode: TEXT`
- `input_cost_per_token_nano: TEXT NULL`
- `output_cost_per_token_nano: TEXT NULL`
- `cache_read_input_cost_per_token_nano: TEXT NULL`
- `cache_creation_input_cost_per_token_nano: TEXT NULL`
- `output_cost_per_reasoning_token_nano: TEXT NULL`
- `max_input_tokens: INTEGER NULL`
- `max_output_tokens: INTEGER NULL`
- `max_tokens: INTEGER NULL`
- `raw_json: TEXT`
- `source: TEXT`
- `updated_at: TEXT`

M4. Price fields in this table MUST use nano-dollar integer strings.

## 8. Models.dev sync

S1. Admin endpoint `POST /api/dashboard/model-metadata/sync/models-dev` MUST fetch the
complete URL stored in `models_dev_base_url`, with default value
`https://models.dev/api.json`.

S1a. `models_dev_base_url` MUST be an absolute URL using `http` or `https` and MUST contain a
host. The server MUST normalize trailing slashes. For backward compatibility, a stored URL
whose path is empty MAY be normalized by appending `/api.json`; a non-empty custom path MUST
be preserved.

S2. The response is a JSON object keyed by provider ID, each containing a `models` object keyed by model ID.

S3. For each `provider/model` pair, sync MUST normalize the upstream model name to the canonical bare `model_id` used by billing lookups (for example `"openai/gpt-4o"` stores as `"gpt-4o"`). The source provider identity for the chosen sync variant MUST be stored separately in `models_dev_provider`.

S4. Cost fields in models.dev are denominated in USD per 1M tokens. Conversion to nano-dollar per token:

```
nano_per_token = trunc(cost_per_1m * 1_000_000_000 / 1_000_000) = trunc(cost_per_1m * 1000)
```

S5. Field mapping from models.dev model object to `model_metadata_records`:

| models.dev field | DB column |
|------------------|-----------|
| (provider key) | `models_dev_provider` |
| `cost.input` | `input_cost_per_token_nano` (after S4 conversion) |
| `cost.output` | `output_cost_per_token_nano` (after S4 conversion) |
| `cost.cache_read` | `cache_read_input_cost_per_token_nano` (after S4 conversion) |
| `cost.cache_write` | `cache_creation_input_cost_per_token_nano` (after S4 conversion) |
| `cost.reasoning` | `output_cost_per_reasoning_token_nano` (after S4 conversion) |
| `limit.context` | `max_tokens` |
| `limit.input` | `max_input_tokens` |
| `limit.output` | `max_output_tokens` |
| `family` contains `"embed"` (case-insensitive) in any grouped provider variant for the canonical model | `mode` = `"embedding"` |
| otherwise | `mode` = `"chat"` |
| (entire model JSON) | `raw_json` |

S6. Sync MUST upsert records by `model_id`.

S7. Sync response MUST return at least:

- `success: true`
- `upserted: number`
- `fetched_at: RFC3339 string`

S8. On fetch/parse failure, endpoint MUST return `502` with `upstream_fetch_failed`.

S9. The metadata sync subsystem MUST expose only `POST /api/dashboard/model-metadata/sync/models-dev`.

## 9. Metadata query API

Q1. Admin endpoint `GET /api/dashboard/model-metadata` MUST list stored metadata rows with
all rows whose `source = "manual"` first, followed by all other rows; each group MUST be
ordered by `model_id ASC`.

Q2. `GET /api/dashboard/model-metadata/{model_id}` MUST return single row or `404 not_found`.

## 10. Upstream model list fetch

UF1. Admin endpoint `POST /api/dashboard/fetch-channel-models` MUST accept:

- `provider_type: responses | chat_completion | messages | gemini | openai_image | replicate`
- `base_url: string`
- `api_key: string`

UF2. For `responses`, `chat_completion`, `messages`, `openai_image`, and `replicate`, the endpoint MUST:

1. Let `base = trim_trailing_slash(base_url)`.
2. Build the upstream models URL as:
   - `GET {base}/models` when `base` ends with `/v1`;
   - otherwise `GET {base}/v1/models`.
3. Include `Authorization: Bearer {api_key}`.
4. Parse OpenAI-compatible `{ data: [{ id: string, ... }] }`.

UF3. For `gemini`, the endpoint MUST call the Gemini model-list API using `api_key` and parse model names as model IDs after removing a leading `models/` prefix.

UF4. The response MUST be `{ models: string[] }` ordered lexicographically with duplicate IDs removed.

UF5. On upstream fetch or parse failure, endpoint MUST return `502` with code `upstream_fetch_failed`.

UF3. Request timeout for the upstream call MUST be 15 seconds.
