# Metered Billing Specification

## 0. Status

- Product name: Monoize.
- Scope:
  - billing-rate matrix storage;
  - pricing-profile selection;
  - token, cache, context-tier, modality, and server-native-meter charging;
  - dashboard APIs for billing-rate administration.

## 1. Data Model

MB-D1. Billing rates MUST be stored in table `billing_rate_records`.

MB-D2. `billing_rate_records` MUST contain these columns:

- `id: TEXT PRIMARY KEY`
- `source: TEXT`
- `pricing_profile: TEXT`
- `model_pattern: TEXT NULL`
- `provider_type: TEXT NULL`
- `rate_kind: TEXT`
- `usage_class: TEXT`
- `unit: TEXT`
- `unit_price_nano_usd: TEXT`
- `context_tier: TEXT NULL`
- `service_tier: TEXT NULL`
- `modality: TEXT NULL`
- `cache_ttl: TEXT NULL`
- `match_json: TEXT`
- `priority: INTEGER`
- `enabled: INTEGER`
- `raw_json: TEXT`
- `updated_at: TEXT`

MB-D3. `unit_price_nano_usd` MUST be an integer string denominated in nano-USD per one `unit`.

MB-D4. `match_json` and `raw_json` MUST be JSON object strings. Invalid JSON MUST be treated as `{}` when reading legacy rows.

MB-D5. `model_metadata_records` MUST continue to store model capabilities, limits, Models.dev raw data, and legacy token prices. Billing computation MUST read `billing_rate_records`. Metadata writes and Models.dev sync MAY mirror token prices into `billing_rate_records`.

## 2. Pricing Profiles

MB-P1. System setting `pricing_profile_model_patterns` MUST store an ordered array of objects:

```json
[{ "pattern": "gpt-*", "pricing_profile": "openai" }]
```

MB-P1a. The default `pricing_profile_model_patterns` value MUST be exactly:

```json
[
  { "pattern": "gpt-image-*", "pricing_profile": "openai" },
  { "pattern": "text-embedding-*", "pricing_profile": "openai" },
  { "pattern": "gpt-*", "pricing_profile": "openai" },
  { "pattern": "o*", "pricing_profile": "openai" },
  { "pattern": "claude-*", "pricing_profile": "anthropic" },
  { "pattern": "gemini-*", "pricing_profile": "google" },
  { "pattern": "grok-*", "pricing_profile": "xai" },
  { "pattern": "*", "pricing_profile": "default" }
]
```

MB-P1b. The profile name `default` denotes the fallback pricing profile for model names that do not match a more specific provider profile rule. It MUST NOT imply legacy billing behavior.

MB-P2. Pattern matching MUST use case-insensitive glob semantics with `*` matching zero or more characters and `?` matching exactly one character.

MB-P3. Pricing-profile selection MUST use the first pattern whose `pattern` matches the normalized pricing model key.

MB-P4. If no pattern matches, the request has no billable pricing.

MB-P5. Migration `m20260619_000020_default_pricing_profile` MUST rename stored pricing profile value `legacy` to `default` in `billing_rate_records.pricing_profile` and in the `pricing_profile_model_patterns` system setting. Runtime pricing selection MUST NOT treat `legacy` as an alias for `default`.

MB-P6. When the selected profile has no complete eligible rate matrix for a normalized pricing model, Monoize MAY try one additional fallback profile from `model_metadata_records.models_dev_provider` for the same normalized model. The fallback MUST be used only when it differs from the selected profile. The fallback MUST use the same `provider_type`, `model_pattern`, context-tier, meter-rate, and completeness rules as the selected profile. Monoize MUST persist the profile that actually matched in `billing_breakdown_json`.

## 3. Rate Selection

MB-R1. A rate row is eligible for a request only when all of these predicates are true:

- `enabled = 1`;
- `pricing_profile` equals the selected pricing profile;
- `provider_type` is null or equals the effective upstream provider type;
- `model_pattern` is null or matches the normalized pricing model key using MB-P2.

MB-R2. Eligible rows MUST be ordered by `priority DESC, id ASC`. The first matching row for a class and dimension set is the applied row.

MB-R3. `rate_kind = "token"` rows charge token quantities. `rate_kind = "meter"` rows charge non-token quantities.

MB-R4. `usage_class` for token rows MUST support at least:

- `input_uncached`
- `input_cached`
- `cache_write_5m`
- `cache_write_1h`
- `cache_read`
- `output`
- `reasoning_output`

MB-R5. The context tier domain is `default`, `short`, `long`.

MB-R6. If any eligible row for a pricing model has `context_tier` other than null or `default`, then the matrix MUST provide either:

- an authoritative upstream usage/service field that selects the tier, or
- `match_json.context_threshold_tokens` as an integer threshold.

MB-R7. If a tiered matrix has no deterministic tier selector under MB-R6, preflight MUST reject the request with HTTP `403` and code `model_pricing_required`.

MB-R8. For a context-tiered matrix, every non-default context tier present for a requested token class MUST have a matching rate for that token class. Missing tier rows MUST reject with HTTP `403` and code `model_pricing_required`.

## 4. Token Billing

MB-T1. Token quantities MUST be read from normalized upstream `Usage`. Monoize MUST NOT estimate token quantities when upstream usage is available.

MB-T2. `Usage.input_tokens` is the aggregate prompt total. The uncached input quantity is:

```text
input_uncached = input_tokens - cache_read_tokens - cache_creation_tokens
```

with saturation at zero.

MB-T3. `cache_read_tokens` MUST charge against `usage_class = "cache_read"` when the quantity is non-zero. A rate row with `usage_class = "input_cached"` is an accepted alias for the same quantity.

MB-T3a. When eligible rows for cached input have non-null `modality`, billing MUST require `Usage.input_details.cache_read_modality_breakdown`. Billing MUST NOT derive the cached modality split from aggregate `cache_read_tokens` or from total input modality counts.

MB-T4. `cache_creation_5m_tokens` MUST charge against `usage_class = "cache_write_5m"` and `cache_ttl = "5m"` when the quantity is non-zero.

MB-T5. `cache_creation_1h_tokens` MUST charge against `usage_class = "cache_write_1h"` and `cache_ttl = "1h"` when the quantity is non-zero.

MB-T6. If `cache_creation_tokens > 0`, both 5-minute and 1-hour cache-write rates are eligible, and `cache_creation_5m_tokens = cache_creation_1h_tokens = 0`, billing MUST reject with HTTP `403` and code `model_pricing_required`. Monoize MUST NOT split aggregate cache-creation usage between 5-minute and 1-hour buckets.

MB-T7. Output tokens excluding reasoning tokens MUST charge against `usage_class = "output"`.

MB-T8. Reasoning output tokens MUST charge against `usage_class = "reasoning_output"` when the quantity is non-zero and a matching rate exists. If no matching reasoning rate exists, those tokens MUST be included in the base output bucket.

MB-T9. When eligible rows for a token class have non-null `modality`, billing MUST require a modality breakdown for that class and MUST charge each non-zero modality quantity using its matching modality row. The modality quantities used for a token class MUST sum exactly to the quantity billed for that token class.

MB-T10. For `gpt-image-2`, Monoize MUST bill text/image input tokens, cached input tokens, and image output tokens from upstream usage. Monoize MUST add an output-item fixed fee only when `billing_rate_records` contains an enabled meter row for that fee.

## 5. Meter Billing

MB-M1. Server-native tool meter charges MUST be based only on:

- authoritative provider usage counters in `Usage.extra_body`, or
- decoded native provider events represented in URP output nodes.

MB-M2. Monoize MUST NOT charge a server-native tool from local wall-clock measurement.

MB-M3. Duration, session, and billed-minute meters MUST require an authoritative upstream billed quantity. If the request enabled such a meter class and upstream usage does not provide the billed quantity, billing MUST reject with HTTP `403` and code `model_pricing_required`.

MB-M4. Call-count meters MAY use decoded native provider events when no authoritative provider usage counter exists.

MB-M5. If a request enables a server-native tool and no eligible meter rate exists for its `usage_class`, preflight MUST reject the request with HTTP `403` and code `model_pricing_required`.

## 6. Charge Formula

MB-C1. Base charge is:

```text
base_charge = sum(token_line_items.charge_nano) + sum(meter_line_items.charge_nano)
```

MB-C2. Final charge is:

```text
final_charge = trunc(base_charge * provider_multiplier)
```

MB-C3. If any required rate is missing, the request MUST be rejected for all roles, including `admin` and `super_admin`.

MB-C4. A successful billing snapshot MUST persist `billing_breakdown_json` with:

- `version = 2`
- `token_line_items[]`
- `meter_line_items[]`
- selected `context_tier`
- selected `service_tier`
- `provider_multiplier`
- `base_charge_nano`
- `final_charge_nano`

## 7. Dashboard APIs

MB-A1. Admin endpoint `GET /api/dashboard/billing-rates` MUST return all billing-rate rows ordered by `pricing_profile ASC, priority DESC, id ASC`.

MB-A2. Admin endpoint `PUT /api/dashboard/billing-rates/{id}` MUST upsert one billing-rate row.

MB-A2a. If the request body omits `source`, the upserted row MUST use `source = "manual"`, even when a row with the same `id` already exists from `source = "catalog"` or `source = "models_dev"`.

MB-A3. Admin endpoint `DELETE /api/dashboard/billing-rates/{id}` MUST delete one billing-rate row.

MB-A4. Admin endpoint `POST /api/dashboard/billing-rates/sync/catalog` MUST sync the bundled catalog. Manual rows with the same `id` MUST take precedence over catalog rows.

MB-A5. Admin endpoint `GET /api/dashboard/pricing-profile-patterns` MUST return the ordered profile-pattern setting.

MB-A6. Admin endpoint `PUT /api/dashboard/pricing-profile-patterns` MUST replace the ordered profile-pattern setting after rejecting empty `pattern` or `pricing_profile` strings.
