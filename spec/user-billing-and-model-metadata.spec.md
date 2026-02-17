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

U2. `balance_usd` MUST be computed from `balance_nano_usd` with nano precision and no binary floating conversion.

U3. New users created by register or dashboard create-user MUST default to:

- `balance_nano_usd = "0"`
- `balance_unlimited = false`

## 3. Admin mutation rules

A1. Only admin/super-admin endpoints MAY mutate user balance fields.

A2. `PUT /api/dashboard/users/{user_id}` MUST accept optional fields:

- `balance_nano_usd: string`
- `balance_usd: string`
- `balance_unlimited: boolean`

A3. If both `balance_nano_usd` and `balance_usd` are provided, server MUST use `balance_nano_usd`.

A4. Balance mutation by admin MUST write one ledger entry with type `admin_adjustment`.

## 4. Billing eligibility

BE1. Billing applies only when the request is authenticated by database API key (resolved `user_id` exists).

BE2. Requests authenticated only by static config keys MUST NOT be billed.

BE3. Before upstream forwarding, if `balance_unlimited = false` and `balance_nano_usd <= 0`, server MUST return:

- HTTP `402`
- code `insufficient_balance`

## 5. Charge calculation

C1. Charge requires both:

- upstream response usage (`prompt_tokens`, `completion_tokens`), and
- model metadata pricing for the served upstream model (`input_cost_per_token_nano`, `output_cost_per_token_nano`).

C1.1. Served upstream model resolution for billing:

- if provider model mapping has non-empty `redirect`, billing MUST use `redirect` as `upstream_model`;
- otherwise billing MUST use the requested logical model.

C2. Base charge formula (nano-dollar):

```
prompt_charge = prompt_tokens * input_cost_per_token_nano
completion_charge = completion_tokens * output_cost_per_token_nano
base_charge = prompt_charge + completion_charge
```

C3. If `usage.cached_tokens` is present and metadata provides `cache_read_input_cost_per_token_nano`, prompt charge MUST be:

```
prompt_charge =
  (prompt_tokens - cached_tokens) * input_cost_per_token_nano
  + cached_tokens * cache_read_input_cost_per_token_nano
```

C4. If `usage.reasoning_tokens` is present and metadata provides `output_cost_per_reasoning_token_nano`, completion charge MUST be:

```
completion_charge =
  (completion_tokens - reasoning_tokens) * output_cost_per_token_nano
  + reasoning_tokens * output_cost_per_reasoning_token_nano
```

C5. Final charge MUST multiply by provider model multiplier and truncate toward zero:

```
final_charge_nano = trunc(base_charge * provider_multiplier)
```

C6. If any required pricing field is missing, charge MUST be skipped for that request and a warning MUST be logged.

C7. For embeddings responses, billing MUST treat usage as:

- `prompt_tokens = usage.prompt_tokens`
- `completion_tokens = 0`

## 6. Billing execution and ledger

L1. Billing deduction MUST run after successful non-stream proxy response decode.

L2. For pass-through streaming requests, billing MAY be skipped when usage cannot be determined from stream payload.

L2a. Requests that return a normal model response payload (including truncated/cutoff completions such as `finish_reason = "length"`) MUST be treated as billable-success requests, not failed requests.

L2b. Requests that terminate as API errors (`4xx`/`5xx` error response) MUST NOT be billed.

L3. On successful deduction, server MUST append a ledger row with:

- `user_id`
- `kind = "request_charge"`
- `delta_nano_usd` (negative value)
- `balance_after_nano_usd`
- `meta_json` (at minimum model, provider_id, prompt/completion/reasoning/cached tokens)

L4. If deduction fails because resulting balance would be negative, server MUST return:

- HTTP `402`
- code `insufficient_balance`

and MUST NOT write deduction.

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

S3. For each `provider/model` pair, the `model_id` stored in the database MUST be `"{provider_id}/{model_id}"` (e.g. `"openai/gpt-4o"`).

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
4. Build the upstream models URL as `GET {trim_trailing_slash(channel.base_url)}/v1/models`.
   The request MUST include `Authorization: Bearer {channel.api_key}`.
5. Parse the response as OpenAI-compatible `{ data: [{ id: string, ... }] }`.
6. Return a JSON object containing:
   - `provider_id: string`
   - `provider_name: string`
   - `models: string[]` (list of model IDs from the response)

UF2. On upstream fetch or parse failure, endpoint MUST return `502` with code `upstream_fetch_failed`.

UF3. Request timeout for the upstream call MUST be 15 seconds.
