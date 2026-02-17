# Model Metadata Dashboard Specification

## 0. Status

- Product name: Monoize.
- Scope:
  - dashboard UI page `/dashboard/models` for viewing/editing model metadata;
  - CRUD REST endpoints for `model_metadata_records`;
  - sync-vs-manual priority semantics.

## 1. Data Model

MD1. This spec operates on the existing `model_metadata_records` table (defined in `user-billing-and-model-metadata.spec.md` § 7).

MD2. `model_id` (PK) is the **bare model API name** (e.g. `gpt-4o`, `claude-sonnet-4-20250514`), not prefixed with provider.

MD3. `source` column distinguishes record origin:

| `source` value | Semantics |
|----------------|-----------|
| `models_dev` | Populated or last updated by Models.dev sync |
| `manual` | Created or last updated by admin manual edit |

MD4. All pricing fields are nano-dollar integer strings (same precision as billing spec).

MD5. `raw_json` stores all provider variants from models.dev as `{ "providers": { "openai": {...}, "azure": {...}, ... } }`. This enables the edit UI to let the user switch pricing source.

MD6. `models_dev_provider` indicates which models.dev provider's pricing is currently applied.

## 2. Sync Priority & Merge

SP1. `POST /api/dashboard/model-metadata/sync/models-dev` MUST skip upsert for any row whose current `source = 'manual'`.

SP2. Rows with `source = 'models_dev'` (or no prior row) MUST be upserted normally.

SP3. When models.dev contains the same bare model name under multiple providers:
  - Group all variants by bare model name.
  - Select the variant with the lowest non-zero `input_cost_per_token_nano` as the default.
  - Store all variants in `raw_json.providers` so the user can switch sources in the edit UI.

SP4. Sync MUST first delete all records with `source != 'manual'`, then insert new data. This ensures models removed upstream are also cleaned up. Sync response MUST return `upserted`, `skipped`, and `deleted` counts.

SP5. During sync, canonical `model_id = "auto"` MUST be ignored (not inserted/updated).

SP6. During sync, a grouped model MUST be ignored when **all** variants have missing or non-positive (`<= 0`) `input_cost_per_token_nano`. In other words, at least one variant must have strictly positive input pricing to be eligible.

SP7. During sync, canonical model IDs that end with `-thinking`, `:thinking`, or `-think` MUST be ignored (not inserted/updated).

SP8. Admin MAY explicitly reset a manual record back to sync-managed by updating it with `source = 'models_dev'` via the PUT endpoint, after which subsequent syncs will overwrite it.

## 3. CRUD Endpoints

### 3.1 List model metadata

- Method/Path: `GET /api/dashboard/model-metadata`
- No changes from original spec.

### 3.2 Get single model metadata

- Method/Path: `GET /api/dashboard/model-metadata/{model_id}`
- No changes from original spec.

### 3.3 Upsert model metadata

- Method/Path: `PUT /api/dashboard/model-metadata/{model_id}`
- Auth: admin required.
- Body (all fields optional):

```json
{
  "models_dev_provider": "openai",
  "mode": "chat",
  "input_cost_per_token_nano": "30000",
  "output_cost_per_token_nano": "60000",
  "cache_read_input_cost_per_token_nano": "15000",
  "output_cost_per_reasoning_token_nano": null,
  "max_input_tokens": 128000,
  "max_output_tokens": 16384,
  "max_tokens": 128000
}
```

- If row exists: update provided fields, set `source = 'manual'`, set `updated_at = now()`.
- If row does not exist: insert with provided fields, `source = 'manual'`, `raw_json = '{}'`, `updated_at = now()`.
- Response: `200 OK` with the full updated `ModelMetadataRecord`.
- Errors: `400 invalid_request` if `model_id` path param is empty.

### 3.4 Delete model metadata

- Method/Path: `DELETE /api/dashboard/model-metadata/{model_id}`
- Auth: admin required.
- Response: `200 OK` with `{ "success": true }`.
- Errors: `404 not_found` if record does not exist.

## 4. Dashboard UI

### 4.1 Page location

UI1. Page MUST be accessible at `/dashboard/models`.

UI2. Sidebar navigation MUST include a "Models" entry between "Playground" and "Users" in the admin section.

### 4.2 Layout

UI3. Page MUST follow standard dashboard layout: `PageWrapper`, `text-3xl` heading, motion animations.

UI4. Page heading: "Model Database" (en) / "模型数据库" (zh).

### 4.3 Default view: Compact virtualized list

UI5. Default display MUST be a compact virtualized table (`TableVirtuoso`) with columns:

| Column | Content |
|--------|---------|
| Model | Provider icon (from `models_dev_provider`) + `model_id` (bare name) |
| Input | `input_cost_per_token_nano` formatted as `$X.XX / 1M tokens` |
| Output | `output_cost_per_token_nano` formatted as `$X.XX / 1M tokens` |
| Context | `max_tokens` formatted with `K` suffix |
| Source | Badge showing `models_dev` or `manual` |
| Updated | Relative timestamp |

UI6. Each row MUST be clickable to open an edit dialog.

UI7. Price display: `nano_per_token / 1000` = dollars per 1M tokens. Display up to 4 decimal places.

### 4.4 Search and filter

UI8. Page MUST include a search input that filters by `model_id` substring (client-side).

### 4.5 Edit dialog — provider source switcher

UI9. When `raw_json.providers` contains multiple entries, the edit dialog MUST show a provider selector listing available providers with their pricing.

UI10. Selecting a provider MUST auto-fill all pricing and limit fields from that provider's data in `raw_json.providers[provider]`.

UI11. The user MAY further edit the auto-filled values. Any save always sets `source = 'manual'`.

### 4.6 Actions

UI12. Page header:
- "Sync Models.dev" button: triggers sync, shows loading, toast with upserted/skipped counts.
- "Add Model" button: opens create dialog.

UI13. Edit dialog MUST include a "Delete" action.

UI14. After any mutation, the model list MUST revalidate via SWR.

### 4.7 Loading state

UI15. Skeleton placeholders while loading.

### 4.8 Billing integration note

UI16. Billing queries `model_metadata_records` by `upstream_model` (bare name). PK is now bare name, so billing matches correctly.

## 5. Invariants

INV1. `source = 'manual'` whenever created or updated via PUT endpoint.

INV2. Sync MUST NOT modify records where `source = 'manual'`.

INV3. `model_id` is the primary key, bare model API name, MUST be unique.

INV4. Price fields nullable; billing **blocks** requests to models with missing input/output cost.

## 6. Billing Enforcement

BE1. `build_monoize_attempts()` MUST filter out any attempt whose `upstream_model` has no pricing data in `model_metadata_records` (i.e. row does not exist, or `input_cost_per_token_nano` is null, or `output_cost_per_token_nano` is null).

BE2. If ALL attempts for a request are filtered out due to missing pricing, the system MUST return HTTP 403 with error code `model_pricing_required` and a message listing the blocked model name(s).

BE3. `maybe_charge_response()` MUST return an error (HTTP 403 `model_pricing_required`) if pricing lookup fails. This is a defense-in-depth check — BE1 should already prevent this path from being reached.

BE4. The Provider dashboard page MUST display a visible warning badge on any `ProviderCard` whose models include entries not present (or not fully priced) in `model_metadata_records`. The badge MUST show the count of unpriced models.

- Unpriced counting target MUST be `redirect` model when `redirect` is non-empty; otherwise the logical model key.

BE5. The billing enforcement check uses a per-request cache to avoid redundant pricing lookups when multiple attempts share the same `upstream_model`.

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
  - During `sync_from_models_dev`, when grouping variants by model name.
  - During migration on startup (existing records with `/` in `model_id`).

NID3. When normalization produces duplicate `model_id` values, the most recently updated record wins.

NID4. Dashboard CRUD routes for model metadata MUST use Axum wildcard `{*model_id}` to support model IDs that may contain `/` (e.g. user-created records). The handler MUST strip a leading `/` from the captured path if present.

## 8. Suffix-Based Reasoning Effort Resolution

### 8.1 Reasoning effort value domain

RE1. Valid `reasoning_effort` values: `none`, `minimum`, `low`, `medium`, `high`, `xhigh`, `max`.

RE2. `max` is a URP-level alias: at decode time it MUST be mapped to the suffix `-xhigh` (i.e. the value sent upstream is `xhigh`, but users may specify `max` and the system treats it as equivalent to `xhigh`).

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

RE6. The setting is editable in the dashboard Settings page.

### 8.3 Model resolution algorithm

RE7. When `collect_provider_attempts` looks up `urp.model` in `provider.models`:
  1. **Exact match**: If `provider.models` contains `urp.model`, use it directly. No suffix processing.
  2. **Suffix resolution**: If no exact match, iterate `reasoning_suffix_map` entries (longest suffix first). For each suffix, check if `urp.model` ends with that suffix. If yes:
     - `base_model = urp.model` with the suffix removed.
     - Look up `base_model` in `provider.models`.
     - If found, use the base model entry AND set `reasoning_effort` to the mapped value.
  3. **No match**: If neither exact nor suffix match, skip this provider (existing behavior).

RE8. When a suffix match resolves to a base model, the resolved `reasoning_effort` value MUST be injected into the URP request's `reasoning.effort` field (typed flow) before the request is encoded for the upstream provider. If the user already specified `reasoning_effort` explicitly in the request body, the explicit value takes precedence over the suffix-derived value.

RE9. Billing uses the **base model**'s pricing from `model_metadata_records`. The suffix model itself does not need a separate pricing entry.

### 8.4 Billing: reasoning token fallback

RE10. In `calculate_charge_nano`, when `reasoning_tokens > 0` and `output_cost_per_reasoning_token_nano` is `None`, the system MUST fall back to `output_cost_per_token_nano` for reasoning tokens (i.e. charge all completion tokens at the output rate).

This is already the existing behavior (the `else` branch charges `completion_tokens * output_cost_per_token_nano` which includes reasoning tokens). No change needed.

## 9. Migration

MIG1. On startup, existing records with `model_id` containing `/` (e.g. `openai/gpt-4o`) MUST be migrated to bare name via NID1 normalization. When duplicates arise after stripping, keep the most recently updated record.
