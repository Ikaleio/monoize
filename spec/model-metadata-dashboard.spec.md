# Model Metadata Dashboard Specification

## 0. Status

- Product name: Monoize.
- Scope:
  - `model_metadata_records` as a **metadata-only** store (capabilities, limits,
    mode, raw models.dev variants);
  - CRUD REST endpoints for `model_metadata_records`;
  - model-id normalization and suffix-based reasoning-effort resolution.
- Price storage, price editing, price sync selection, and billing enforcement are
  governed by `model-pricing.spec.md`. The former Billing Profiles and Advanced Rates
  tabs, the `billing_rate_records` CRUD endpoints, and the pricing-profile pattern
  endpoints do not exist in this model; migration step
  `m20260901_000049_model_prices_cutover` removes their storage
  (`model-pricing.spec.md` §12.2).

## 1. Data Model

MD1. This spec operates on `model_metadata_records`
(`user-billing-and-model-metadata.spec.md` § 7).

MD2. `model_id` (PK) is the **bare model API name** (e.g. `gpt-4o`,
`claude-sonnet-4-20250514`), not prefixed with provider.

MD3. `source` column distinguishes record origin:

| `source` value | Semantics |
|----------------|-----------|
| `models_dev` | Populated or last updated by Models.dev sync |
| `manual` | Created or last updated by admin manual edit |

MD4. After the cutover migration, `model_metadata_records` carries no price columns.
Metadata rows store `models_dev_provider`, `mode`, token limits, `raw_json`, `source`,
and `updated_at`. Billing computation reads `model_prices`
(`model-pricing.spec.md` §3) and MUST NOT read `model_metadata_records`.

MD5. `raw_json` stores all provider variants from models.dev as
`{ "providers": { "openai": {...}, "azure": {...}, ... } }`. Every value inside a
variant's `cost` object MUST be stored and returned as its exact decimal string rather
than a JSON number. This lets the pricing UI switch price sources without JavaScript
binary-floating-point conversion.

MD6. `models_dev_provider` indicates which models.dev provider variant is currently
applied for metadata (and, through `model-pricing.spec.md` MP-Y7, for the synced
price).

## 2. Models.dev sync (metadata part)

SP1. The models.dev sync run is defined by `model-pricing.spec.md` §9.2. One run
updates `model_prices` (prices) and `model_metadata_records` (metadata) from one
fetched snapshot.

SP2. Metadata upsert skips rows whose current `source = 'manual'`. Rows with
`source = 'models_dev'` (or no prior row) are upserted normally.

SP3. Variant grouping, official-provider preference, and the highest-price fallback
are defined by `model-pricing.spec.md` MP-Y7 and the MP-Y8 official family→provider
table. The metadata row's `models_dev_provider` MUST equal the provider of the variant
selected there. All variants are stored in `raw_json.providers`.

SP4. Metadata sync MUST first delete all records with `source != 'manual'`, then
insert new data, so models removed upstream are cleaned up.

SP5. Skip rules follow `model-pricing.spec.md` MP-Y9 (`auto`, thinking-suffix ids).

SP6. Limit and mode mapping from the selected models.dev variant:

| models.dev field | DB column |
|------------------|-----------|
| (provider key) | `models_dev_provider` |
| `limit.context` | `max_tokens` |
| `limit.input` | `max_input_tokens` |
| `limit.output` | `max_output_tokens` |
| `family` contains `"embed"` (case-insensitive) in any grouped variant | `mode` = `"embedding"` |
| otherwise | `mode` = `"chat"` |
| (entire model JSON per variant) | `raw_json.providers` |

SP7. Admin MAY reset a manual record back to sync-managed by updating it with
`source = 'models_dev'` via the PUT endpoint, after which later syncs overwrite it.

## 3. CRUD Endpoints

### 3.1 List model metadata

- Method/Path: `GET /api/dashboard/model-metadata`
- Auth: admin required.
- Response: rows ordered by `model_id ASC`.

### 3.2 Get single model metadata

- Method/Path: `GET /api/dashboard/model-metadata/{model_id}`
- Response: single row or `404 not_found`.

### 3.3 Upsert model metadata

- Method/Path: `PUT /api/dashboard/model-metadata/{model_id}`
- Auth: admin required.
- Body (all fields optional):

```json
{
  "source": "manual",
  "models_dev_provider": "openai",
  "mode": "chat",
  "max_input_tokens": 128000,
  "max_output_tokens": 16384,
  "max_tokens": 128000
}
```

- `source` MAY be `manual` or `models_dev`. An omitted `source` resolves to `manual`.
  Another value is invalid.
- If row exists: update only fields present in the JSON object. Set `source` to the
  resolved source. Set `updated_at = now()`. An omitted field preserves its stored
  value. An explicitly null nullable field clears its stored value.
- If row does not exist: insert with provided fields, the resolved source,
  `raw_json = '{}'`, and `updated_at = now()`.
- Response: `200 OK` with the full updated record.
- Errors: `400 invalid_request` if the `model_id` path param is empty. Price fields
  are unknown fields on this endpoint and MUST be rejected with `400 invalid_request`;
  prices are edited through `PUT /api/dashboard/model-prices/{model_id}`
  (`model-pricing.spec.md` MP-A2).

### 3.4 Delete model metadata

- Method/Path: `DELETE /api/dashboard/model-metadata/{model_id}`
- Auth: admin required.
- Response: `200 OK` with `{ "success": true }`.
- Errors: `404 not_found` if the record does not exist.
- Deleting metadata MUST NOT delete the model's `model_prices` row.

## 4. Dashboard UI

UI1. The `/dashboard/models` page is defined by `model-pricing.spec.md` §11. There is
no standalone metadata table page.

UI2. Metadata fields (`mode`, `max_tokens`, `max_input_tokens`, `max_output_tokens`,
`models_dev_provider`) are shown and edited inside the model pricing sheet
(`model-pricing.spec.md` MP-UI3) in a section separated from the price fields.
Metadata edits persist through §3.3; price edits persist through
`model-pricing.spec.md` MP-A2.

UI3. When `raw_json.providers` contains multiple variants, the pricing sheet MUST show
a provider-variant selector. Selecting a variant auto-fills price fields (exact
decimal strings from `raw_json`) and limit fields from that variant. The user MAY edit
the auto-filled values before saving.

UI4. All variant price handling in the UI keeps decimal strings end to end; values
MUST NOT pass through JavaScript `Number`, `parseFloat`, `toFixed`, or binary
floating-point arithmetic.

## 5. Invariants

INV1. A PUT request with omitted `source` stores `source = 'manual'`. A PUT request
with `source = 'models_dev'` stores `source = 'models_dev'` and enables SP7.

INV2. Sync MUST NOT modify metadata records where `source = 'manual'`.

INV3. `model_id` is the primary key, bare model API name, unique.

## 6. Billing Enforcement

BE1. Attempt filtering, `model_pricing_required` rejection, the free-settlement flags,
and the Provider dashboard pricing badges are defined by `model-pricing.spec.md`
(MP-F2, MP-A4) and `channel-management.spec.md` (`model_runtime_statuses.pricing_status`).
`model_metadata_records` plays no role in billing enforcement.

## 7. Model ID Normalization

NID1. **Canonical form**: `model_id` MUST be normalized in this order:
  1. Take the last segment after splitting on `/`.
  2. Optionally strip a provider prefix in either `provider--model` or `provider.model` form, but ONLY when `provider` is a known provider identifier.
  3. Lowercase the result.
  - `openai/gpt-4o` → `gpt-4o`
  - `accounts/fireworks/models/llama-v3p1-405b-instruct` → `llama-v3p1-405b-instruct`
  - `anthropic--claude-4.5-opus` → `claude-4.5-opus`
  - `xxxxx/anthropic.claude-opus-4.6` → `claude-opus-4.6`
  - `flux.1-dev` → `flux.1-dev` (no known provider prefix; preserve)
  - `GPT-4o` → `gpt-4o`
  - `claude-sonnet-4-20250514` → `claude-sonnet-4-20250514` (no `/`, unchanged except lowercase)

NID2. Normalization MUST be applied:
  - During models.dev sync, when grouping variants by model name.
  - During migration on startup (existing records with `/` in `model_id`).

NID3. When normalization produces duplicate `model_id` values, the most recently updated record wins.

NID4. Dashboard CRUD routes for model metadata and model prices MUST use Axum wildcard `{*model_id}` to support model IDs that may contain `/` (e.g. user-created records). The handler MUST strip a leading `/` from the captured path if present.

## 8. Suffix-Based Reasoning Effort Resolution

### 8.1 Reasoning effort value domain

RE1. Valid `reasoning_effort` values: `none`, `minimum`, `low`, `medium`, `high`, `xhigh`, `max`. `xhigh` and `max` are two distinct effort levels and MUST NOT be aliased to each other.

RE2. The built-in suffix table maps each `-<effort>` suffix to its own identical effort string (e.g. `-max -> max`, `-xhigh -> xhigh`). Monoize MUST NOT collapse `-max` to `xhigh` at suffix-resolution time.

### 8.2 Global suffix → effort mapping

RE3. A global setting `reasoning_suffix_map` stores a JSON object mapping string suffixes to reasoning effort values.

Default value:
```json
{
  "-thinking": "high",
  "-reasoning": "high",
  "-nothinking": "none"
}
```

RE4. Suffixes are matched **longest-first** against the end of the model name.

RE5. The setting is stored in `system_settings` table under key `reasoning_suffix_map` and exposed via the existing `GET/PUT /api/dashboard/settings` endpoints.

RE5a. Startup and every successful settings mutation MUST publish `reasoning_suffix_map` into the process runtime snapshot. Forwarding suffix resolution MUST read that snapshot and MUST NOT query `system_settings` per request.

RE6. The setting is editable in the dashboard Settings page.

RE6a. The default provider-level suffix transform used for Anthropic/OpenRouter compatibility SHOULD map wildcard `*` to `-thinking` (not `-{effort}`), so suffix resolution keeps model IDs on supported aliases.

### 8.3 Model resolution algorithm

RE7. When `collect_provider_attempts` looks up `urp.model` in each `channel.models`:
  1. **Exact match**: If `channel.models` contains `urp.model`, use it directly. No suffix processing.
  2. **Suffix resolution**: If no exact match, iterate `reasoning_suffix_map` entries (longest suffix first). For each suffix, check if `urp.model` ends with that suffix. If yes:
     - `base_model = urp.model` with the suffix removed.
     - Look up `base_model` in `channel.models`.
     - If found, use that Channel model entry AND set `reasoning_effort` to the mapped value.
  3. **No match**: If neither exact nor suffix match, skip this Channel.

RE8. When a suffix match resolves to a base model, the resolved `reasoning_effort` value MUST be injected into the URP request's `reasoning.effort` field (typed flow) before the request is encoded for the upstream provider. If the user already specified `reasoning_effort` explicitly in the request body, the explicit value takes precedence over the suffix-derived value.

RE9. Billing and any other model-pricing identification path use the **base model**'s pricing key. When a model ID ends with a recognized reasoning-tier suffix, Monoize MUST strip that suffix (longest suffix first, at most one suffix removed) before the `model_prices` lookup (`model-pricing.spec.md` MP-R1). The suffix model itself does not need a separate pricing entry.

### 8.4 Billing: reasoning token pricing

RE10. Reasoning-token pricing follows `model-pricing.spec.md` MP-R5: a null
`reasoning_usd_per_1m` bills reasoning tokens at the resolved output rate.

## 9. Migration

MIG1. On startup, existing records with `model_id` containing `/` (e.g. `openai/gpt-4o`) MUST be migrated to bare name via NID1 normalization. When duplicates arise after stripping, keep the most recently updated record.
