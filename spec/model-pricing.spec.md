# Model Pricing Specification

## 0. Status

- Product name: Monoize.
- Scope:
  - the `model_prices` table and its resolution rules;
  - USD settlement formulas for token, per-request, and tiered billing;
  - billing-group resolution and `monoize_groups.billing_ratio`;
  - free-settlement flags (`allow_free_when_unpriced`, `allow_free_when_missing_usage`)
    and their Provider-level overrides;
  - server-native tool billing through the `tool_prices` system setting;
  - external price sync (models.dev, OpenRouter, new-api) and field locks;
  - billing breakdown version 3;
  - migration from the `billing_rate_records` engine;
  - dashboard APIs and the `/dashboard/models` pricing UI.
- This specification supersedes the pricing-profile and rate-matrix model in
  `metered-billing.spec.md`. That file is deprecated and points here.
- Related specs: `user-billing-and-model-metadata.spec.md` (balance, ledger, admission,
  pricing-key normalization), `channel-management.spec.md` (Channel `multiplier`,
  `redirect`), `groups-registry.spec.md` (group registry), `database-provider-routing.spec.md`
  (Provider selection, `group_rank`), `model-metadata-dashboard.spec.md` (metadata-only
  model records).

### 0.1 Implementation status

MP-S1. This specification defines the implemented behavior. Migration
`m20260826_000048_model_prices` creates the new storage. Migration
`m20260901_000049_model_prices_cutover` removes the legacy engine and activates this
settlement model.

MP-S2. New settlements MUST use `model_prices`. The deprecated rules in
`metered-billing.spec.md` do not govern new settlements.

## 1. Concepts and units

MP-U1. All persisted prices MUST be base-10 decimal strings denominated in USD. A price
string MUST have at most 12 integer digits and at most 9 fractional digits. Exponent
notation, a leading `+`, `NaN`, and infinity are invalid.

MP-U2. Settlement arithmetic MUST use exact decimal or integer arithmetic. No price,
ratio, multiplier, or charge computation may pass through `f32` or `f64`.

MP-U3. Charges settle in nano-USD (`1 USD = 1_000_000_000 nano-USD`) as signed `i128`
integers. Every conversion from a USD decimal to nano-USD MUST truncate toward zero.

MP-U4. Token prices are denominated in USD per 1,000,000 tokens. The nano-USD price of
one token is `trunc(usd_per_1m * 1000)`.

MP-U5. One `model_prices` row exists per model. Lookup is by exact `model_id`. Glob or
pattern matching on `model_id` MUST NOT exist in price resolution.

## 2. Data model

### 2.1 Table `model_prices`

MP-D1. Table `model_prices` MUST contain these columns:

| Column | Type | Meaning |
|---|---|---|
| `model_id` | TEXT PRIMARY KEY | Bare normalized model id (`model-metadata-dashboard.spec.md` NID1) |
| `billing_mode` | TEXT NOT NULL | `per_token`, `per_request`, or `tiered_expr` |
| `input_usd_per_1m` | TEXT NULL | USD per 1M uncached input tokens |
| `output_usd_per_1m` | TEXT NULL | USD per 1M output tokens |
| `cache_read_usd_per_1m` | TEXT NULL | USD per 1M cache-read input tokens |
| `cache_write_usd_per_1m` | TEXT NULL | USD per 1M cache-write (5m or unsplit) tokens |
| `cache_write_1h_usd_per_1m` | TEXT NULL | USD per 1M 1-hour cache-write tokens |
| `reasoning_usd_per_1m` | TEXT NULL | USD per 1M reasoning output tokens |
| `per_request_usd` | TEXT NULL | USD per request (`per_request` mode) |
| `billing_expr` | TEXT NULL | JSON tier table (`tiered_expr` mode, §4.4) |
| `source` | TEXT NOT NULL | `manual`, `models_dev`, `openrouter`, or `new_api` |
| `locked_fields` | TEXT NOT NULL DEFAULT `'[]'` | JSON array of locked column names (§9.6) |
| `raw_json` | TEXT NOT NULL DEFAULT `'{}'` | JSON object with upstream variants (§9.2) |
| `enabled` | INTEGER NOT NULL DEFAULT `1` | `0` or `1` |
| `updated_at` | TEXT NOT NULL | RFC 3339 UTC |

MP-D2. Every non-null price column MUST satisfy MP-U1 and MUST be `>= 0`. A write path
MUST reject a violation with HTTP `400` and code `invalid_request`.

MP-D3. `billing_mode` MUST be one of `per_token`, `per_request`, `tiered_expr`. Any
other value MUST be rejected at write time.

MP-D4. `locked_fields` MUST decode as a JSON array of strings. Each element MUST be one
of: `billing_mode`, `input_usd_per_1m`, `output_usd_per_1m`, `cache_read_usd_per_1m`,
`cache_write_usd_per_1m`, `cache_write_1h_usd_per_1m`, `reasoning_usd_per_1m`,
`per_request_usd`, `billing_expr`, `enabled`. A write path MUST reject other elements.

MP-D5. `raw_json` MUST be a JSON object string. A malformed persisted value MUST fail
the read with a storage error that identifies the row; it MUST NOT decode as `{}`.

MP-D6. The dashboard UI MAY present new-api-style ratios (for example
`output_usd_per_1m / input_usd_per_1m`) as derived display values. Ratios are not
persisted. Rationale: absolute decimal columns avoid non-terminating ratio quotients
and keep MP-U2 satisfiable.

### 2.2 Table `price_sync_runs`

MP-D7. Table `price_sync_runs` MUST contain these columns:

| Column | Type | Meaning |
|---|---|---|
| `id` | TEXT PRIMARY KEY | UUID v4 |
| `source` | TEXT NOT NULL | `models_dev`, `openrouter`, or `new_api` |
| `status` | TEXT NOT NULL | `running`, `success`, or `failed` |
| `started_at` | TEXT NOT NULL | RFC 3339 UTC |
| `finished_at` | TEXT NULL | RFC 3339 UTC |
| `inserted` | INTEGER NOT NULL DEFAULT `0` | Rows inserted |
| `updated` | INTEGER NOT NULL DEFAULT `0` | Rows updated |
| `skipped` | INTEGER NOT NULL DEFAULT `0` | Rows skipped (locks or manual precedence) |
| `deleted` | INTEGER NOT NULL DEFAULT `0` | Rows deleted |
| `error` | TEXT NULL | Failure detail for `failed` runs |
| `detail_json` | TEXT NOT NULL DEFAULT `'{}'` | Bounded JSON diff summary |

MP-D8. `detail_json` MUST be at most 262144 bytes after serialization. A larger diff
summary MUST be truncated to a prefix list plus a `truncated: true` marker.

### 2.3 Group billing ratio

MP-D9. `monoize_groups` gains column `billing_ratio TEXT NOT NULL DEFAULT '1'`.

MP-D10. `billing_ratio` MUST be a decimal string per MP-U1 and MUST be `>= 0`. The
value `0` makes every request billed through that group free. Group create and update
endpoints accept the optional field `billing_ratio`; a violation MUST be rejected with
HTTP `400` and code `invalid_request`. See `groups-registry.spec.md` GR-D8.

### 2.4 Provider free-settlement overrides

MP-D11. `monoize_providers` gains two nullable columns:

- `allow_free_when_unpriced_override INTEGER NULL` (`0`, `1`, or NULL)
- `allow_free_when_missing_usage_override INTEGER NULL` (`0`, `1`, or NULL)

MP-D12. The Provider API object exposes both fields as `boolean | null`. NULL means
"inherit the global setting". Create and update endpoints accept both fields as
optional `boolean | null`. Read paths MUST return `true`, `false`, and `null` distinctly.

### 2.5 System settings

MP-D13. Three settings are added to `system_settings` and exposed through
`GET/PUT /api/dashboard/settings`:

| Key | Type | Default |
|---|---|---|
| `allow_free_when_unpriced` | boolean | `false` |
| `allow_free_when_missing_usage` | boolean | `false` |
| `tool_prices` | JSON object | §6.4 seed table |

MP-D14. Both boolean settings default to `false`. `false` is the fail-closed state:
missing prices and missing usage reject as defined in §7.

## 3. Price resolution

MP-R1. The pricing key for a request attempt is resolved exactly as defined by
`user-billing-and-model-metadata.spec.md` C1.1 and C1.2: normalize the served
`upstream_model` (strip at most one recognized reasoning-tier suffix); when that key
has no applicable price and the model was redirected, retry with the normalized
requested logical model key.

MP-R2. A model has an applicable price if and only if a `model_prices` row exists with
`model_id` equal to the pricing key and `enabled = 1` and the row is complete under
MP-R3.

MP-R3. Row completeness by `billing_mode`:

- `per_token`: `input_usd_per_1m` is non-null.
- `per_request`: `per_request_usd` is non-null.
- `tiered_expr`: `billing_expr` is non-null and decodes under MP-C10.

MP-R4. An incomplete or disabled row MUST be treated exactly like a missing row.

MP-R5. Null price columns resolve through these fallbacks at computation time:

- `output_usd_per_1m` null → use `input_usd_per_1m`.
- `cache_read_usd_per_1m` null → use `input_usd_per_1m`.
- `cache_write_usd_per_1m` null → use `input_usd_per_1m`.
- `cache_write_1h_usd_per_1m` null → use the resolved `cache_write_usd_per_1m`.
- `reasoning_usd_per_1m` null → use the resolved `output_usd_per_1m`.

MP-R6. Service tier and modality do not select prices. Settlement MUST apply the same
resolved prices for every service tier and every modality. The settled `service_tier`
value is still recorded in the breakdown (§8).

MP-R7. Provider attempt preflight MUST attach the resolved `model_prices` row snapshot
(or its absence) plus the resolved free-settlement flags to the attempt. Settlement of
that attempt MUST reuse the snapshot and MUST NOT repeat resolution.

MP-R8. One forwarding request MUST load candidate `model_prices` rows for all of its
distinct pricing keys in one set-based database query. Per-attempt, per-Provider, and
per-Channel price queries MUST NOT exist.

## 4. Charge computation

### 4.1 Token quantities

MP-C1. Token quantities MUST be read from normalized upstream `Usage` as defined by
`user-billing-and-model-metadata.spec.md` C3, C3a, and C4:

```text
cache_read   = usage.input_details.cache_read_tokens          (default 0)
cache_w_5m   = usage.input_details.cache_creation_5m_tokens   (default 0)
cache_w_1h   = usage.input_details.cache_creation_1h_tokens   (default 0)
cache_w_agg  = usage.input_details.cache_creation_tokens      (default 0)
input_uncached = max(0, input_tokens - cache_read - cache_w_agg)
reasoning    = usage.output_details.reasoning_tokens          (default 0)
output_plain = max(0, output_tokens - reasoning)
```

MP-C2. When `cache_w_agg > cache_w_5m + cache_w_1h`, the unsplit remainder
`cache_w_agg - cache_w_5m - cache_w_1h` MUST be charged at the resolved
`cache_write_usd_per_1m` rate. Splitting an unsplit aggregate between TTL buckets MUST
NOT occur.

### 4.2 `per_token` mode

MP-C3. Token charge in nano-USD:

```text
token_charge_nano =
    trunc(input_uncached * input_usd_per_1m * 1000)
  + trunc(cache_read     * resolved(cache_read_usd_per_1m)     * 1000)
  + trunc(cache_w_5m_or_unsplit * resolved(cache_write_usd_per_1m) * 1000)
  + trunc(cache_w_1h     * resolved(cache_write_1h_usd_per_1m) * 1000)
  + trunc(output_plain   * resolved(output_usd_per_1m)         * 1000)
  + trunc(reasoning      * resolved(reasoning_usd_per_1m)      * 1000)
```

Each line item truncates independently. A line item with quantity `0` MUST be omitted
from the breakdown.

MP-C4. For embeddings responses, `output_tokens = 0` and only input line items apply.

### 4.3 `per_request` mode

MP-C5. `token_charge_nano = trunc(per_request_usd * 1_000_000_000)`. Token quantities
are recorded in the breakdown but do not affect the charge.

### 4.4 `tiered_expr` mode

MP-C6. `billing_expr` MUST decode as a JSON object:

```json
{
  "tiers": [
    {
      "when_input_tokens_lte": 200000,
      "input_usd_per_1m": "1.25",
      "output_usd_per_1m": "10",
      "cache_read_usd_per_1m": "0.31",
      "cache_write_usd_per_1m": "1.625",
      "cache_write_1h_usd_per_1m": null,
      "reasoning_usd_per_1m": null
    },
    { "input_usd_per_1m": "2.5", "output_usd_per_1m": "15" }
  ]
}
```

MP-C7. `tiers` MUST be a non-empty array of at most 8 objects. Every tier except the
last MUST have integer `when_input_tokens_lte >= 1`, strictly increasing across tiers.
The last tier MUST omit `when_input_tokens_lte` (unbounded). Each tier MUST contain a
non-null `input_usd_per_1m`; other price fields are optional and resolve per MP-R5
within the tier.

MP-C8. Tier selection uses the settled `usage.input_tokens`: the applied tier is the
first tier whose `when_input_tokens_lte` is `>= input_tokens`, else the last tier.
Exactly one tier applies to the whole request.

MP-C9. After tier selection, the charge follows MP-C3 with the tier's resolved prices.

MP-C10. A `billing_expr` that violates MP-C6 or MP-C7 MUST be rejected at write time
with HTTP `400` and code `invalid_request`. A persisted violating value makes the row
incomplete (MP-R4).

### 4.5 Final charge

MP-C11. Final charge:

```text
base_charge_nano  = token_charge_nano + tool_charge_nano        (§6)
final_charge_nano = trunc(base_charge_nano * channel_multiplier * group_billing_ratio)
```

`channel_multiplier` is the selected Channel model entry `multiplier`
(`channel-management.spec.md` CP-INV-3, `>= 0`). `group_billing_ratio` is resolved by
§5. Both multiplications use exact decimal arithmetic with one final truncation toward
zero.

MP-C12. `channel_multiplier = 0` and `group_billing_ratio = 0` are valid explicit
zero-charge configurations. They MUST settle a normal breakdown with
`final_charge_nano = 0` and `free_reason = null`.

MP-C13. Settlement, ledger append, negative-balance semantics, and stream lifecycle
rules remain governed by `user-billing-and-model-metadata.spec.md` §6.

## 5. Billing group resolution

MP-G1. The billing group of a request is the group actually used for routing: for the
selected Provider, with the request's non-null ordered `effective_groups`, compute
`group_rank(provider) = min { i : effective_groups[i] ∈ provider.group_ids }`
(`database-provider-routing.spec.md` R-GRP-2). The billing group id is
`effective_groups[group_rank]`.

MP-G2. `group_billing_ratio` is the `billing_ratio` of the billing group's
`monoize_groups` row, read from the routing snapshot attached at preflight (MP-R7).

MP-G3. When `effective_groups == null` (system-originated internal traffic),
`billing_group_id = null` and `group_billing_ratio = 1`.

MP-G4. The user's `group_id`, `effective_groups[0]`, and any maximum-ratio selection
MUST NOT be used to pick the billing group. The breakdown persists `billing_group_id`
(§8) so the applied group is auditable.

MP-G5. When a Channel-affinity binding promotes an attempt
(`database-provider-routing.spec.md` R-AFF), MP-G1 still applies to the Provider that
actually served the request.

## 6. Tool billing (`tool_prices`)

### 6.1 Value schema

MP-T1. The `tool_prices` setting is a JSON object. Each key is a server-native usage
class (for example `web_search`, `code_interpreter_duration`). Each value is one of:

- a JSON number or decimal string: USD per 1000 calls (new-api compatible shorthand);
- an object:

```json
{ "usd": "0.03", "per": "1k_calls", "minimum_units": 5 }
```

MP-T2. `usd` MUST satisfy MP-U1 and `>= 0`. `per` MUST be one of `1k_calls`, `minute`,
`session`. `minimum_units` is an optional integer `>= 1` and is valid only when `per`
is `minute` or `session`. A violation MUST be rejected at write time with HTTP `400`
and code `invalid_request`.

MP-T3. A shorthand number value is equivalent to `{ "usd": <value>, "per": "1k_calls" }`.

### 6.2 Quantity sources

MP-T4. A usage class is actually used when at least one of these holds:

- normalized upstream usage (`Usage.extra_body`) contains a positive authoritative
  quantity for that class;
- decoded terminal URP output contains a matching provider-native tool item.

MP-T5. Quantity per unit kind:

- `1k_calls`: the authoritative call count when present, else the count of matching
  decoded provider-native tool items.
- `minute`: the authoritative billed-minute quantity from upstream usage. Local
  wall-clock measurement MUST NOT be used.
- `session`: the authoritative session count from upstream usage.

MP-T6. When `minimum_units` is present, the billed quantity is
`max(actual_quantity, minimum_units)` and applies only when the class is actually used
with a positive authoritative quantity.

### 6.3 Charge and fail-open rule

MP-T7. Tool charge per class:

- `1k_calls`: `trunc(count * usd * 1_000_000_000 / 1000)`
- `minute` and `session`: `trunc(billed_quantity * usd * 1_000_000_000)`

`tool_charge_nano` is the sum over all actually used classes.

MP-T8. Tool billing is fail-open. An actually used class settles with zero tool charge
when either condition holds:

- `tool_prices` has no entry for the class;
- the entry requires an authoritative quantity (`minute`, `session`) and upstream usage
  does not provide one.

Each such class MUST be listed once in breakdown `unpriced_tool_classes` (§8), in
request descriptor order. The request MUST NOT be rejected for a missing tool price.
The legacy `allow_unpriced_server_tools` Channel flag does not exist in this model.

MP-T9. Tool charges apply in every `billing_mode`, including `per_request`.

### 6.4 Seed values

MP-T10. On first startup after the cutover migration, `tool_prices` MUST be seeded
with exactly:

```json
{
  "web_search": "10",
  "x_search": "5",
  "file_search_tool_call": "2.5",
  "code_execution": "5",
  "code_interpreter_duration": { "usd": "0.0015", "per": "minute", "minimum_units": 5 },
  "code_execution_duration": { "usd": "0.000833333", "per": "minute", "minimum_units": 5 },
  "code_interpreter_session": { "usd": "0.03", "per": "session" }
}
```

An existing `tool_prices` setting MUST NOT be overwritten by startup seeding.

## 7. Free-settlement flags

### 7.1 Resolution

MP-F1. For a selected attempt, each flag resolves as:

```text
effective(flag) = provider.<flag>_override  if non-null
                  else global setting <flag>
```

for `allow_free_when_unpriced` and `allow_free_when_missing_usage` independently.

### 7.2 Behavior matrix

MP-F2. Unpriced model (MP-R2 fails for both pricing keys of an attempt):

| `effective(allow_free_when_unpriced)` | Behavior |
|---|---|
| `false` | The attempt is not billable. When no candidate attempt is billable, reject with HTTP `403` and code `model_pricing_required` before the balance gate. |
| `true` | The attempt is billable. Settlement writes a full breakdown with `token_charge_nano = 0`, tool charges per §6, and `free_reason = "unpriced"`. |

MP-F3. Missing usage (billable-success response without normalized upstream usage):

| `effective(allow_free_when_missing_usage)` | Behavior |
|---|---|
| `false` | Non-stream and buffered synthetic stream: reject before response delivery with HTTP `403` and code `usage_required`. Pass-through stream: settle from the byte estimate (input `ceil(request_utf8_bytes / 4)`, output `ceil(visible_output_utf8_bytes / 4)`) with breakdown `estimated = true`. |
| `true` | Settle with zero token quantities, `token_charge_nano = 0`, and `free_reason = "missing_usage"`. Tool charges per §6 still apply when tool usage is observable. |

MP-F4. Present upstream usage always takes precedence: neither flag changes a charge
when a complete price row and normalized usage exist.

MP-F5. When both conditions occur (unpriced model and missing usage), the unpriced rule
MP-F2 applies first. A settled free breakdown records `free_reason = "unpriced"`.

MP-F6. `admin` and `super_admin` roles are not exempt: pricing requirements apply to
every role. Only these flags, `channel_multiplier = 0`, or `group_billing_ratio = 0`
produce a free settlement.

MP-F7. The per-Channel `allow_missing_usage` flag does not exist in this model. The
cutover migration removes the column (§12).

## 8. Billing breakdown version 3

MP-B1. A settled request MUST persist `billing_breakdown_json` with this schema:

```json
{
  "version": 3,
  "billing_mode": "per_token",
  "pricing_model_key": "gpt-4o",
  "price_row_model_id": "gpt-4o",
  "applied_tier_index": null,
  "token_line_items": [
    { "usage_class": "input_uncached", "quantity": 1200, "usd_per_1m": "2.5", "charge_nano": "3000" }
  ],
  "tool_line_items": [
    { "usage_class": "web_search", "quantity": 2, "per": "1k_calls", "usd": "10", "charge_nano": "20000000" }
  ],
  "unpriced_tool_classes": [],
  "service_tier": null,
  "billing_group_id": "3e9c...",
  "group_billing_ratio": "1",
  "channel_multiplier": "1",
  "base_charge_nano": "3000",
  "final_charge_nano": "3000",
  "free_reason": null,
  "estimated": false
}
```

MP-B2. `token_line_items[].usage_class` values are `input_uncached`, `cache_read`,
`cache_write`, `cache_write_1h`, `output`, `reasoning_output`. Zero-quantity line items
are omitted (MP-C3).

MP-B3. `price_row_model_id` is the `model_prices.model_id` of the applied row, or
`null` for a `free_reason = "unpriced"` settlement. `applied_tier_index` is the
zero-based selected tier for `tiered_expr` mode, else `null`.

MP-B4. `free_reason` domain is `null`, `"unpriced"`, `"missing_usage"`.

MP-B5. `estimated = true` only for the pass-through byte-estimate path of MP-F3.

MP-B6. Version 2 breakdowns remain readable in stored request logs. New settlements
MUST write only version 3 after the cutover migration.

## 9. External price sync

### 9.1 Sources

MP-Y1. Three sync sources exist:

| `source` | Endpoint | Auth |
|---|---|---|
| `models_dev` | `GET https://models.dev/api.json` | none |
| `openrouter` | `GET https://openrouter.ai/api/v1/models` | none |
| `new_api` | `GET {configured_base_url}/api/pricing` | optional bearer token |

MP-Y2. The `new_api` source is configured through system settings
`price_sync_new_api_base_url` (string, default empty = disabled) and
`price_sync_new_api_token` (string, default empty). The token MUST NOT be returned by
settings read APIs once set; reads return `""` for an unset token and `"__set__"`
otherwise.

MP-Y2a. Settings writes handle the token field as: value `"__set__"` keeps the stored
token unchanged; value `""` clears the stored token; any other string replaces the
stored token. This makes a read-modify-write settings round trip lossless.

MP-Y3. Fetch timeout is 30 seconds. A fetch or parse failure finalizes the
`price_sync_runs` row with `status = "failed"` and returns HTTP `502` with code
`upstream_fetch_failed`.

### 9.2 models.dev mapping

MP-Y4. models.dev responses are keyed by provider id, each with a `models` object.
Sync normalizes every model id to the bare canonical form
(`model-metadata-dashboard.spec.md` NID1) and groups variants by canonical id.

MP-Y5. Cost mapping to `model_prices` (all values parsed as exact decimal strings from
the JSON tokens, never through binary floating point):

| models.dev field | column |
|---|---|
| `cost.input` | `input_usd_per_1m` |
| `cost.output` | `output_usd_per_1m` |
| `cost.cache_read` | `cache_read_usd_per_1m` |
| `cost.cache_write` | `cache_write_usd_per_1m` |
| `cost.reasoning` | `reasoning_usd_per_1m` |

`billing_mode = "per_token"`. Missing cost subfields store NULL.

MP-Y6. All grouped variants MUST be stored in `raw_json` as
`{ "providers": { "<provider_id>": <variant model JSON>, ... } }` with every cost value
kept as its exact decimal string.

MP-Y7. Variant selection for the applied price uses this priority:

1. **Official provider preference.** If the canonical `model_id` matches a family in
   the MP-Y8 table and the mapped provider's variant has strictly positive
   `cost.input`, select that variant.
2. **Highest-price fallback.** Otherwise select the variant with the highest strictly
   positive `cost.input`. This prevents resale losses when downstream Channels route
   to the most expensive supplier.

A grouped model where every variant has missing or non-positive `cost.input` MUST be
skipped.

MP-Y8. Official family→provider table. The match is a case-insensitive prefix test on
the canonical `model_id`, evaluated top to bottom; the first matching row wins.
`o<digit>` means the letter `o` followed by an ASCII digit.

| `model_id` prefix | models.dev provider |
|---|---|
| `gpt-`, `o<digit>`, `chatgpt-` | `openai` |
| `claude-` | `anthropic` |
| `gemini-`, `gemma-` | `google` |
| `grok-` | `xai` |
| `deepseek-` | `deepseek` |
| `mistral-`, `codestral-`, `pixtral-`, `ministral-`, `magistral-`, `devstral-` | `mistral` |
| `qwen`, `qwq-`, `qvq-` | `alibaba` |
| `llama-` | `llama` |
| `command-` | `cohere` |
| `kimi-`, `moonshot-` | `moonshotai` |
| `glm-` | `zhipuai` |
| `minimax-` | `minimax` |
| `step-` | `stepfun` |
| `sonar` | `perplexity` |
| `solar-` | `upstage` |
| `phi-` | `azure` |
| `mimo-` | `xiaomi` |
| `mercury` | `inception` |

This table is maintained here. Adding or removing a family requires a spec update in
the same change.

MP-Y9. Sync skip rules: canonical `model_id = "auto"` and canonical ids ending with
`-thinking`, `:thinking`, or `-think` MUST be skipped.

MP-Y10. models.dev sync also upserts metadata (limits, `mode`, `raw_json`) into
`model_metadata_records` per `model-metadata-dashboard.spec.md` §2. Metadata rows no
longer carry prices.

### 9.3 OpenRouter mapping

MP-Y11. OpenRouter returns `{ "data": [{ "id", "pricing": { "prompt", "completion",
"input_cache_read", "input_cache_write", ... } }] }` with prices in USD per token as
decimal strings. Mapping:

| OpenRouter field | column | conversion |
|---|---|---|
| `pricing.prompt` | `input_usd_per_1m` | `value * 1_000_000` exact |
| `pricing.completion` | `output_usd_per_1m` | `value * 1_000_000` exact |
| `pricing.input_cache_read` | `cache_read_usd_per_1m` | `value * 1_000_000` exact |
| `pricing.input_cache_write` | `cache_write_usd_per_1m` | `value * 1_000_000` exact |

Model ids are normalized per NID1 (`vendor/model` → bare `model`). A model with
non-positive `pricing.prompt` and non-positive `pricing.completion` is skipped.

### 9.4 new-api mapping

MP-Y12. new-api `GET /api/pricing` returns entries with `model_name`, `quota_type`,
`model_ratio`, `completion_ratio`, `model_price`. Conversion:

- `quota_type = 0` (ratio billing): `billing_mode = "per_token"`,
  `input_usd_per_1m = model_ratio * 2` (ratio `1` equals USD 2 per 1M tokens),
  `output_usd_per_1m = input_usd_per_1m * completion_ratio`.
- `quota_type = 1` (fixed price): `billing_mode = "per_request"`,
  `per_request_usd = model_price`.

All arithmetic is exact decimal; results truncate to 9 fractional digits.

### 9.5 Apply semantics

MP-Y13. A sync run applies per-source ownership: it upserts rows whose `source` equals
the run's source or that do not exist. It MUST NOT modify a row whose `source` is
`manual` or a different sync source; such rows are counted in `skipped`.

MP-Y14. Within an upserted row, a column listed in `locked_fields` MUST keep its stored
value. The sync MUST still refresh `raw_json` and `updated_at`.

MP-Y15. A `models_dev` sync run deletes rows with `source = "models_dev"` whose
canonical model id is absent from the fetched snapshot. Other sources do not delete.

MP-Y16. Each apply run inserts one `price_sync_runs` row at start (`status =
"running"`) and finalizes it with counts. Row writes use set-based statements in
fixed-size chunks below the portable SQLite bound-parameter limit; PostgreSQL uses the
same chunking.

### 9.6 Field locks

MP-Y17. A dashboard upsert (§10) that changes a price column adds that column name to
`locked_fields` and sets `source = "manual"` only when the row was previously absent;
an existing synced row keeps its `source`, gaining only the lock entries.

MP-Y18. A dashboard upsert MAY replace `locked_fields` explicitly to remove locks.
Removing every lock from a synced row re-enables full sync updates for it.

## 10. Dashboard APIs

All endpoints require an authenticated admin session unless stated otherwise.

MP-A1. `GET /api/dashboard/model-prices` returns all rows ordered by `model_id ASC`.

MP-A2. `PUT /api/dashboard/model-prices/{model_id}` upserts one row. The body accepts
every mutable column as optional; omitted fields keep stored values; explicit `null`
clears a nullable column. Validation per §2.1. Lock semantics per MP-Y17/MP-Y18. The
route uses the Axum wildcard `{*model_id}` form and strips one leading `/`.

MP-A3. `DELETE /api/dashboard/model-prices/{model_id}` deletes one row; `404
not_found` when absent.

MP-A4. `GET /api/dashboard/model-prices/unpriced` returns
`{ "models": string[] }`: the sorted distinct logical model names available for routing
(`database-provider-routing.spec.md` R-MDL-3 source set) whose pricing key resolves to
no applicable price under MP-R2. The computation runs on request-local data with a
bounded query count.

MP-A5. `GET /api/dashboard/price-sync/runs?limit=N` returns the most recent
`price_sync_runs` rows ordered by `started_at DESC`, default limit 20, maximum 100.

MP-A6. `POST /api/dashboard/price-sync/{source}/preview` fetches the source and
returns the computed diff without writing `model_prices` rows:

```json
{ "source": "models_dev", "insert": 12, "update": 3, "skip": 5, "delete": 1,
  "changes": [ { "model_id": "gpt-4o", "kind": "update", "fields": ["input_usd_per_1m"] } ] }
```

`changes` is truncated to at most 500 entries plus `"truncated": true`.

MP-A7. `POST /api/dashboard/price-sync/{source}/apply` performs MP-Y13..MP-Y16 and
returns the finalized run row.

MP-A8. `tool_prices`, `allow_free_when_unpriced`, `allow_free_when_missing_usage`,
`price_sync_new_api_base_url`, and `price_sync_new_api_token` are read and written
through the existing `GET/PUT /api/dashboard/settings` endpoints.

MP-A9. Group `billing_ratio` is read through `GET /api/dashboard/groups` and written
through the group create/update endpoints (`groups-registry.spec.md` §2).

## 11. Dashboard UI

MP-UI1. The page at `/dashboard/models` contains exactly five tabs in this order:

1. `Model Pricing` (模型定价)
2. `Unpriced Models` (未定价模型)
3. `Tool Prices` (工具价格)
4. `Upstream Sync` (上游同步)
5. `Group Pricing` (分组定价)

MP-UI2. Every tab uses SWR for data loading, renders a skeleton while loading, and
applies optimistic updates for user-triggered mutations.

MP-UI3. Model Pricing tab: a virtualized table (`TableVirtuoso`) with columns Model,
Mode, Input $/1M, Output $/1M, Source, Status (enabled + lock count), Updated. A row
click opens a pricing sheet (drawer) with one section per `billing_mode` selected by a
mode switcher: per-token price fields, per-request price field, and a tiered editor
for `billing_expr`. All price inputs are decimal strings; conversion and validation
MUST NOT pass values through JavaScript `Number` or `parseFloat`.

MP-UI4. Unpriced Models tab: renders MP-A4 results with a per-row action that opens
the pricing sheet pre-filled with the model id.

MP-UI5. Tool Prices tab: an editor for the `tool_prices` object. Each row has usage
class, USD price, a unit selector (`1k_calls`, `minute`, `session`), and a
`minimum_units` input enabled only for `minute` and `session`. Invalid decimal or
negative prices are blocked before submission.

MP-UI6. Upstream Sync tab: one card per source (models.dev, OpenRouter, new-api) with
last-run status from MP-A5, a preview action rendering the MP-A6 diff, and an apply
action. The new-api card exposes the base-URL and token settings.

MP-UI6a. Applying the models.dev source MUST update prices and metadata from the same
fetched snapshot. A separate metadata-sync action or endpoint MUST NOT exist.

MP-UI7. Group Pricing tab: lists registry groups with an editable `billing_ratio`
column persisting through the group update endpoint.

MP-UI8. The system settings page exposes `allow_free_when_unpriced` and
`allow_free_when_missing_usage` as two switches in the billing/health settings
category. Each description states the fail-closed default.

MP-UI9. The Provider editor dialog exposes both Provider overrides as three-state
selectors (`Inherit global`, `On`, `Off`) mapping to `null`, `true`, `false`.

## 12. Migration

### 12.1 Step 1 (additive): `m20260826_000048_model_prices`

MP-M1. Step 1 creates `model_prices` and `price_sync_runs` per §2, adds
`monoize_groups.billing_ratio TEXT NOT NULL DEFAULT '1'`, and adds the two nullable
Provider override columns per MP-D11. It MUST NOT drop or alter any existing column.
`down()` reverses exactly these additions.

MP-M2. After step 1, SQLite and PostgreSQL schemas are identical in column names,
nullability, and defaults.

### 12.2 Step 2 (cutover): `m20260901_000049_model_prices_cutover`

MP-M3. Step 2 converts legacy manual rules: every `billing_rate_records` row with
`source = "manual"`, `enabled = 1`, `rate_kind = "token"`, a non-null `model_pattern`
containing no `*` and no `?`, and null `context_tier`, `service_tier`, `modality`
(null or `"default"` accepted for `context_tier` and `service_tier`) maps to a
`model_prices` row keyed by the normalized `model_pattern`:

| legacy `usage_class` | column |
|---|---|
| `input_uncached` | `input_usd_per_1m` |
| `output` | `output_usd_per_1m` |
| `cache_read`, `input_cached` | `cache_read_usd_per_1m` |
| `cache_write_5m` | `cache_write_usd_per_1m` |
| `cache_write_1h` | `cache_write_1h_usd_per_1m` |
| `reasoning_output` | `reasoning_usd_per_1m` |

Price conversion: `usd_per_1m = unit_price_nano_usd / 1000` exact decimal. Converted
rows get `billing_mode = "per_token"`, `source = "manual"`, and `locked_fields`
containing every populated price column. When two legacy rows map to the same column
of the same model, the higher `priority` (then lower `id`) wins. An existing
`model_prices` row for the same `model_id` keeps its values; the legacy rule is
discarded.

MP-M4. Every other legacy rule (glob patterns, provider-type-scoped rows, tiered rows,
meter rows, catalog and models_dev rows) is discarded. Operators re-sync through §9.

MP-M5. Step 2 then drops table `billing_rate_records`, drops Channel columns
`allow_missing_usage` and `allow_unpriced_server_tools`, deletes the
`pricing_profile_model_patterns` system-settings row, and drops the
`model_metadata_records` price columns (`input_cost_per_token_nano`,
`output_cost_per_token_nano`, `cache_read_input_cost_per_token_nano`,
`cache_creation_input_cost_per_token_nano`, `output_cost_per_reasoning_token_nano`).
No compatibility alias, view, or duplicate column remains.

MP-M6. `down()` for step 2 recreates the dropped schema empty. Discarded rule data is
not reconstructible.

MP-M7. The settlement engine, routing preflight, dashboards, and request-log
projection switch from `billing_rate_records` to `model_prices` in the same change
that ships step 2.
