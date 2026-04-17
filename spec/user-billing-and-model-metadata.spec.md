# User Billing and Model Metadata Specification

## 0. Status

- Product name: Monoize.
- Scope:
  - user-level prepaid balance and billing on proxy requests;
  - model metadata storage and Models.dev metadata sync;
  - admin-only balance mutation.

## 1. Precision and storage rules

B1. Balance unit MUST be nano-dollar (`1 USD = 1_000_000_000 nano_usd`).

B2. Persistent balance MUST use signed integer nano-dollar string storage in `users.balance_nano_usd` (`TEXT` column), not floating point.

B3. User balance unlimited switch MUST be persisted in `users.balance_unlimited` (`INTEGER` column, `0|1`).

B4. Decimal USD inputs MUST accept up to 9 fractional digits. Values with more than 9 fractional digits MUST be truncated toward zero when converted to nano-dollar.

B5. Balance arithmetic MUST use checked integer operations. Overflow MUST return `500 internal_error`.

## 2. User data model

U1. User read model exposed by dashboard/auth APIs MUST include:

- `balance_nano_usd: string`
- `balance_usd: string`
- `balance_unlimited: boolean`
- `email: string | null`
- `allowed_groups: string[]`

U2. `balance_usd` MUST be computed from `balance_nano_usd` with nano precision and no binary floating conversion.

U3. New users created by register or dashboard create-user MUST default to:

- `balance_nano_usd = "0"`
- `balance_unlimited = false`
- `email = null`
- `allowed_groups = []`

U4. Usernames with prefix `_monoize_` (case-insensitive) are reserved for internal system accounts and MUST NOT be allowed in public register/login flows or admin create/update username operations.

U5. Internal reserved users (`username` prefix `_monoize_`) MUST be excluded from user list APIs and user-count metrics used by dashboard/admin UI.

U6. Monoize active-probe subsystem MUST ensure an internal user `_monoize_active_probe` exists before each probe attempt and MUST force this user to `balance_unlimited = true`.

U7. `email` is an optional field (`TEXT NULL` in SQLite). When set, it MUST be a non-empty string. The server MUST NOT validate email format beyond non-emptiness; the field is used solely for Gravatar URL generation.

U8. Any authenticated user MAY update their own `email` field via `PUT /api/dashboard/auth/me` with optional body field `email: string | null`. Setting `email` to `null` or empty string MUST clear the stored value.

U9. Admin users MAY also update a user's `email` via `PUT /api/dashboard/users/{user_id}` with optional body field `email: string | null`.

U10. Dashboard frontend MUST generate Gravatar URLs from user email using the MD5 hash of the lowercase-trimmed email, per the Gravatar protocol (`https://www.gravatar.com/avatar/{md5}?d=identicon&s={size}`). If the user has no email set, the frontend MUST fall back to displaying the first character of the username as the avatar.

## 3. Admin mutation rules

A1. Only admin/super-admin endpoints MAY mutate user balance fields.

A2. `PUT /api/dashboard/users/{user_id}` MUST accept optional fields:

- `balance_nano_usd: string`
- `balance_usd: string`
- `balance_unlimited: boolean`
- `email: string | null`
- `allowed_groups: string[]`

A2a. `POST /api/dashboard/users` MUST accept optional field `allowed_groups: string[]`. If the field is omitted, the stored value MUST be `[]`.

A2b. `PUT /api/dashboard/users/{user_id}` MUST treat `allowed_groups` as a partial-update field:

- if `allowed_groups` is omitted, the stored value MUST remain unchanged;
- if `allowed_groups` is present, the stored value MUST be replaced by that array.

A2c. Any dashboard/admin write path that persists `users.allowed_groups` MUST canonicalize the array before storage by trimming each element, lowercasing, removing empty strings after trimming, deduplicating, and sorting ascending.

A3. If both `balance_nano_usd` and `balance_usd` are provided, server MUST use `balance_nano_usd`.

A4. Balance mutation by admin MUST write one ledger entry with type `admin_adjustment`.

## 4. Billing eligibility

BE1. Billing applies only when the request is authenticated by database API key (resolved `user_id` exists).

BE2. Requests authenticated only by static config keys MUST NOT be billed.

BE3. Before upstream forwarding, billing eligibility MUST be checked as follows:

- If the authenticated API key has `sub_account_enabled = 1`: check `sub_account_balance_nano > 0`. If not, return HTTP `402` with code `insufficient_balance`. The user's balance is NOT checked.
- Otherwise (API key inherits user balance): if `balance_unlimited = false` and `balance_nano_usd <= 0`, server MUST return HTTP `402` with code `insufficient_balance`.

BE4. The legacy `ensure_quota_before_forward` per-call quota check MUST NOT exist. Sub-account billing replaces it entirely (see `api-key-sub-account-billing.spec.md`).

## 5. Charge calculation

C1. Charge requires both:

- upstream response usage (`input_tokens`, `output_tokens`), and
- model metadata pricing resolved under C1.2, with non-null `input_cost_per_token_nano` and `output_cost_per_token_nano`.

C1.1. Served upstream model resolution for request execution and billing metadata:

- if provider model mapping has non-empty `redirect`, Monoize MUST send that `redirect` upstream and MUST record it as `upstream_model`;
- otherwise Monoize MUST use the requested logical model as `upstream_model`.

C1.2. Pricing model resolution for billing:

- Before each pricing lookup candidate in this section, Monoize MUST normalize that candidate to a `pricing_model_key` by removing at most one recognized reasoning-tier suffix from the end of the model ID. If no recognized suffix matches, `pricing_model_key` MUST equal the original candidate.
- Recognized reasoning-tier suffixes MUST use the same suffix set and longest-suffix-first matching rule as `reasoning_suffix_map` plus the built-in effort suffixes defined in `model-metadata-dashboard.spec.md` § 8.
- Monoize MUST first look up pricing for the normalized `upstream_model` key derived from C1.1.
- If `upstream_model` came from a non-empty `redirect` and that normalized lookup does not yield complete pricing, Monoize MUST retry pricing lookup with the normalized requested logical model key.
- If the normalized requested logical model key equals the normalized `upstream_model` key, Monoize MUST NOT perform a second lookup.
- If neither lookup yields complete pricing, the request has no billable pricing.

C2. Base charge formula (nano-dollar):

```
input_charge = input_tokens * input_cost_per_token_nano
output_charge = output_tokens * output_cost_per_token_nano
base_charge = input_charge + output_charge + cache_creation_charge
```

(where `cache_creation_charge` defaults to `0` when not applicable).

C3. `usage.input_tokens` on the internal `Usage` model MUST be interpreted as an aggregate/inclusive prompt total. That is, `input_tokens` MUST be the sum of base-rate prompt tokens, cache-read prompt tokens, and cache-creation prompt tokens. Cache-class counters (`cache_read_tokens`, `cache_creation_tokens`) are refinements of that total, not disjoint additive buckets.

C3-i. Upstream providers whose native usage field is already aggregate/inclusive (for example OpenAI Chat Completions and OpenAI Responses) MUST map their prompt total directly to `input_tokens`.

C3-ii. Upstream providers whose native usage field excludes cache buckets (for example Anthropic Messages, where the wire `input_tokens` is the non-cached remainder and `cache_read_input_tokens` / `cache_creation_input_tokens` are reported as disjoint buckets) MUST be normalized at decode time so that the internal `Usage.input_tokens` equals `wire_input_tokens + cache_read_input_tokens + cache_creation_input_tokens`. The native wire semantics MUST be reconstructed at encode time by subtracting cache buckets back out (saturating at zero) before writing `input_tokens` to any downstream Anthropic-format response, SSE `message_start`, or SSE `message_delta` payload.

C3-iii. With C3-i and C3-ii in effect, all billing and logging code paths MUST treat `Usage.input_tokens` uniformly as aggregate/inclusive. Provider-type branching on the interpretation of `input_tokens` MUST NOT exist in billing computation, usage-breakdown construction, or request-log projection.

C3-iv. If `usage.input_details.cache_read_tokens` is present and metadata provides `cache_read_input_cost_per_token_nano`, input charge MUST be:

```
input_charge =
  (input_tokens - cache_read_tokens) * input_cost_per_token_nano
  + cache_read_tokens * cache_read_input_cost_per_token_nano
```

C3a. If `usage.input_details.cache_creation_tokens` is present and metadata provides `cache_creation_input_cost_per_token_nano`, an additional charge MUST be added:

```
cache_creation_charge = cache_creation_tokens * cache_creation_input_cost_per_token_nano
```

When C3a applies, the input charge computed in C2/C3 MUST first exclude `cache_creation_tokens` from the base-rate input bucket before adding `cache_creation_charge`. Formally:

``` 
base_rate_input_tokens = input_tokens - cache_read_tokens - cache_creation_tokens
input_charge =
  base_rate_input_tokens * input_cost_per_token_nano
  + cache_read_tokens * cache_read_input_cost_per_token_nano
  + cache_creation_tokens * cache_creation_input_cost_per_token_nano
```

The implementation MUST clamp each billable bucket at zero after subtraction. Monoize MUST NOT charge the same input token once at the base input rate and again at the cache-creation rate. The single aggregate/inclusive formula above applies uniformly to all provider types, because all upstream usage is normalized per C3-ii before billing.

C4. If `usage.output_details.reasoning_tokens` is present and metadata provides `output_cost_per_reasoning_token_nano`, output charge MUST be:

```
output_charge =
  (output_tokens - reasoning_tokens) * output_cost_per_token_nano
  + reasoning_tokens * output_cost_per_reasoning_token_nano
```

`output_tokens` in C4 MUST likewise be treated according to provider semantics. If a provider defines reasoning tokens as a subtype of the reported output total, Monoize MUST subtract them before applying the base output rate and then add the reasoning-rate subtotal. Monoize MUST NOT bill the same output token once at the base output rate and again at the reasoning rate.

C5. Final charge MUST multiply by provider model multiplier and truncate toward zero:

```
final_charge_nano = trunc(base_charge * provider_multiplier)
```

C6. If C1.2 yields no billable pricing, Monoize MUST reject the request with HTTP `403` and code `model_pricing_required`.

C6.1. `build_monoize_attempts()` SHOULD prevent C6 from being reached by filtering unbillable attempts before upstream forwarding.

C6.2. If C6 is reached during post-response billing, Monoize MUST NOT write any charge ledger row for that request.

C7. For embeddings responses, billing MUST treat usage as:

- `input_tokens = usage.input_tokens`
- `output_tokens = 0`

## 6. Billing execution and ledger

L1. Billing deduction MUST run after successful non-stream proxy response decode.

L2. For pass-through streaming requests, billing MAY be skipped when usage cannot be determined from stream payload.

L2a. Requests that return a normal model response payload (including truncated/cutoff completions such as `finish_reason = "length"`) MUST be treated as billable-success requests, not failed requests.

L2b. Requests that terminate as API errors (`4xx`/`5xx` error response) MUST NOT be billed.

L3. On successful deduction, server MUST append a ledger row with:

- `user_id`
- `kind = "request_charge"` (user balance) or `kind = "api_key_charge"` (sub-account)
- `delta_nano_usd` (negative value)
- `balance_after_nano_usd`
- `meta_json` (at minimum model, provider_id, prompt/completion/reasoning/cached tokens; sub-account charges MUST also include `api_key_id`)

L4. If deduction fails because resulting balance would be negative, server MUST return:

- HTTP `402`
- code `insufficient_balance`

and MUST NOT write deduction.

## 6a. Billing concurrency control

LC1. The application MUST use two SQLite pools against the same DSN: a read pool (`max_connections = 10`) and a write pool (`max_connections = 1`).

LC2. All balance-mutating operations (request charges and admin adjustments) MUST execute on the write pool.

LC3. Balance reads used for eligibility and analytics MAY execute on the read pool.

LC4. The write pool's single connection is the required serialization mechanism for billing writes; an additional application-level billing mutex MUST NOT be required.

LC5. The billing charge path (`charge_user_balance_nano`) MUST execute a single attempt and MUST NOT include an explicit retry loop. Error behavior for non-transient failures remains unchanged.

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

S1. Admin endpoint `POST /api/dashboard/model-metadata/sync/models-dev` MUST fetch:

- `https://models.dev/api.json`

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

Q1. Admin endpoint `GET /api/dashboard/model-metadata` MUST list stored metadata rows ordered by `model_id ASC`.

Q2. `GET /api/dashboard/model-metadata/{model_id}` MUST return single row or `404 not_found`.

## 10. Upstream model list fetch

UF1. Admin endpoint `POST /api/dashboard/providers/{provider_id}/fetch-models` MUST:

1. Look up the provider by `provider_id` from `MonoizeRoutingStore`.
2. If the provider has no channels, return `400` with code `no_channels`.
3. Pick the first enabled channel (or the first channel if none are enabled).
4. Let `base = trim_trailing_slash(channel.base_url)`. Build the upstream models URL as:
   - `GET {base}/models` when `base` ends with `/v1`;
   - otherwise `GET {base}/v1/models`.
   This rule MUST produce exactly one `/v1` segment before `/models`.
   The request MUST include `Authorization: Bearer {channel.api_key}`.
5. Parse the response as OpenAI-compatible `{ data: [{ id: string, ... }] }`.
6. Return a JSON object containing:
   - `provider_id: string`
   - `provider_name: string`
   - `models: string[]` (list of model IDs from the response)

UF2. On upstream fetch or parse failure, endpoint MUST return `502` with code `upstream_fetch_failed`.

UF3. Request timeout for the upstream call MUST be 15 seconds.
