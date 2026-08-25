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

MB-D3a. `unit_price_nano_usd` MUST be non-negative and representable as `i128`. Create, update, sync, and metadata-mirror paths MUST reject a negative or malformed rate before persistence.

MB-D4. `match_json` and `raw_json` MUST be JSON object strings. Decoding a persisted value that is malformed JSON or is not a JSON object MUST return a storage error that identifies the billing-rate row and column. A get, list, or matching-rate query MUST propagate that error; it MUST NOT replace the value with `{}`, omit the row, or treat the row as an unconditional rate. Create, update, and catalog-sync paths MUST reject an explicit non-object value before persistence. An omitted value MAY default to `{}` before persistence.

MB-D4a. Migration `m20260809_000030_normalize_billing_json_nulls` MUST evaluate `billing_rate_records.match_json` and `billing_rate_records.raw_json` independently. For each column, it MUST replace the stored value with `{}` if and only if removing leading and trailing JSON whitespace (`U+0009`, `U+000A`, `U+000D`, and `U+0020`) yields the exact string `null`. It MUST leave every other value unchanged, including malformed JSON, arrays, objects, strings, booleans, numbers, and values containing non-JSON whitespace. SQLite and PostgreSQL MUST apply these predicates identically. The down migration MUST be a no-op because the original whitespace cannot be reconstructed. Runtime decoding MUST continue to satisfy MB-D4 after this migration.

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

MB-P2a. Pattern matching MUST compare ASCII bytes without recursion. It MUST use `O(1)` call-stack space and `O(1)` auxiliary space for every pattern and value length. Its worst-case comparison count MUST be `O(pattern_bytes * value_bytes)`. A pattern or value with at least `200000` bytes and a pattern containing multiple `*` operators MUST not cause call-stack growth proportional to input length.

MB-P3. Pricing-profile selection MUST use the first pattern whose `pattern` matches the normalized pricing model key.

MB-P4. If no pattern matches, the request has no billable pricing.

MB-P5. Migration `m20260619_000020_default_pricing_profile` MUST rename stored pricing profile value `legacy` to `default` in `billing_rate_records.pricing_profile` and in the `pricing_profile_model_patterns` system setting. Runtime pricing selection MUST NOT treat `legacy` as an alias for `default`.

MB-P6. When the selected profile has no complete eligible rate matrix for a normalized pricing model, Monoize MAY try one additional fallback profile from `model_metadata_records.models_dev_provider` for the same normalized model. The fallback MUST be used only when it differs from the selected profile. The fallback MUST use the same `provider_type`, `model_pattern`, context-tier, meter-rate, and completeness rules as the selected profile. Monoize MUST persist the profile that actually matched in `billing_breakdown_json`.

MB-P7. When MB-P6 yields more than one candidate profile, rate rows for all candidate profiles MUST be loaded in one set-based database query for that model and provider type.

MB-P8. Provider-attempt preflight MUST attach the complete selected rate-matrix snapshot to the attempt. Settlement of that attempt MUST reuse the attached snapshot and MUST NOT repeat profile selection, metadata lookup, or rate-row lookup.

MB-P9. One active-probe scheduler tick MUST read the reasoning-suffix map once and pricing-profile patterns once. It MUST bulk-read metadata pricing profiles at most once and candidate Billing Rate rows at most once for all probe candidate model and Provider-type pairs in that tick.

MB-P10. Active-probe pricing resolution MUST preserve ordered profile precedence: Settings pattern first, model metadata second. Within one profile it MUST preserve Billing Rate priority order, Provider-type matching, model-pattern matching, and dimensionless token-rate selection.

MB-P11. Each active-probe request-log task MUST receive its pricing resolution from the scheduler-tick snapshot. The task MUST NOT query Settings, model metadata, or Billing Rates.

MB-P12. One forwarding request MUST clone the reasoning-suffix map and pricing-profile patterns once from the process runtime snapshot before pricing its eligible attempts. It MUST execute no `system_settings` query for those values. It MUST bulk-read metadata pricing profiles at most once and candidate Billing Rate rows at most once for all distinct normalized model and effective Provider-type pairs in that request. Pricing resolution for each attempt MUST then use only this request-local snapshot. The runtime MUST NOT execute pricing queries per attempt, Provider, or Channel.

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

MB-R9. Preflight MUST parse `unit_price_nano_usd` for every candidate row as a canonical non-negative `i128` string. One malformed, non-canonical, or negative candidate row MUST make the matrix incomplete.

MB-R10. A complete matrix MUST contain dimensionless fallback rows for `input_uncached` and `output`. A dimensionless fallback row has `modality = null`, `cache_ttl = null`, and `service_tier` equal to null or `default`. For a non-tiered matrix, its `context_tier` MUST equal null or `default`. For a context-tiered matrix, each tier required by MB-R8 MUST contain such a fallback row. Preflight MUST reject a matrix that lacks one of these rows.

MB-R11. Settlement MUST select the service tier from the non-empty `service_tier` field in the actual upstream response envelope. For a Responses stream, the field is `response.service_tier`. For a Chat Completions stream, the field is the top-level `service_tier`. For a Messages stream, the field is `message.service_tier`. A request field or `Usage.extra_body.service_tier` MUST NOT select the settled service tier.

MB-R12. If the settled service tier is absent or equals `default`, a rate row with `service_tier` equal to null or `default` is eligible. If the settled service tier has any other value, only a rate row with the same non-null `service_tier` is eligible. A null or `default` row MUST NOT act as a fallback for a different settled service tier.

MB-R13. For a non-default settled service tier, the matrix MUST contain matching dimensionless `input_uncached` and `output` token rows. It MUST also contain a matching meter row for each requested server-native usage class. If one of these rows is absent, settlement MUST fail with HTTP `403` and code `model_pricing_required`.

## 4. Token Billing

MB-T1. Token quantities MUST be read from normalized upstream `Usage`. Monoize MUST NOT estimate token quantities when upstream usage is available.

MB-T1a. `Usage.input_tokens` and `Usage.output_tokens` MUST be inclusive totals before billing starts. If a provider reports tool-result prompt tokens or reasoning tokens as disjoint counters, its decoder MUST add those counters to the corresponding total with checked integer addition while retaining the counters in `input_details` or `output_details`. Billing MUST NOT add a retained detail counter to an inclusive total a second time.

MB-T2. `Usage.input_tokens` is the aggregate prompt total. The uncached input quantity is:

```text
input_uncached = input_tokens - cache_read_tokens - cache_creation_tokens
```

with saturation at zero.

MB-T3. `cache_read_tokens` MUST charge against `usage_class = "cache_read"` when the quantity is non-zero. A rate row with `usage_class = "input_cached"` is an accepted alias for the same quantity.

MB-T3a. When eligible rows for cached input have non-null `modality` and `Usage.input_details.cache_read_modality_breakdown` is present, billing MUST use that breakdown. Billing MUST NOT derive the cached modality split from aggregate `cache_read_tokens` or from total input modality counts. When the breakdown is absent, billing MUST use the first matching dimensionless `cache_read` or `input_cached` row; if neither exists, billing MUST use the dimensionless `input_uncached` fallback row.

MB-T4. `cache_creation_5m_tokens` MUST charge against `usage_class = "cache_write_5m"` and `cache_ttl = "5m"` when the quantity is non-zero.

MB-T5. `cache_creation_1h_tokens` MUST charge against `usage_class = "cache_write_1h"` and `cache_ttl = "1h"` when the quantity is non-zero.

MB-T6. If `cache_creation_tokens > 0`, both 5-minute and 1-hour cache-write rates are eligible, and `cache_creation_5m_tokens = cache_creation_1h_tokens = 0`, billing MUST reject with HTTP `403` and code `model_pricing_required`. Monoize MUST NOT split aggregate cache-creation usage between 5-minute and 1-hour buckets.

MB-T6a. For each cache-read or cache-write bucket whose selected dimensions have no matching specialized rate, billing MUST charge that bucket with the matching dimensionless `input_uncached` fallback rate. An unsplit aggregate cache-write bucket for which MB-T6 does not select a TTL-specific rate MUST be charged once with that fallback rate.

MB-T7. Output tokens excluding reasoning tokens MUST charge against `usage_class = "output"`.

MB-T8. Reasoning output tokens MUST charge against `usage_class = "reasoning_output"` when the quantity is non-zero and a matching rate exists. If no matching reasoning rate exists, those tokens MUST be included in the base output bucket.

MB-T9. When eligible rows for a token class have non-null `modality` and a modality breakdown is present, billing MUST charge each non-zero modality quantity using its matching modality row. The modality quantities used for that token class MUST sum exactly to the quantity billed for that token class. When the breakdown is absent, billing MUST charge the aggregate quantity with the first matching dimensionless row.

MB-T10. For `gpt-image-2`, Monoize MUST bill text/image input tokens, cached input tokens, and image output tokens from upstream usage. Monoize MUST add an output-item fixed fee only when `billing_rate_records` contains an enabled meter row for that fee.

## 5. Meter Billing

MB-M1. Server-native tool meter charges MUST be based only on:

- authoritative provider usage counters in `Usage.extra_body`, or
- decoded native provider events represented in URP output nodes.

MB-M2. Monoize MUST NOT charge a server-native tool from local wall-clock measurement.

MB-M3. Duration, session, and billed-minute meters MUST require an authoritative upstream billed quantity. If the request enabled such a meter class and upstream usage does not provide the billed quantity, billing MUST reject with HTTP `403` and code `model_pricing_required`.

MB-M4. Call-count meters MAY use decoded native provider events when no authoritative provider usage counter exists.

MB-M5. If a request enables a server-native tool and no eligible meter rate exists for its `usage_class`, preflight MUST reject the request with HTTP `403` and code `model_pricing_required`.

MB-M6. For each meter `usage_class`, billing MUST apply only the first eligible row under MB-R2 whose context tier, service tier, modality, and cache-TTL dimensions match the settled usage. A lower-priority duplicate row for the same class and selected dimensions MUST NOT create another line item.

MB-M7. Pass-through streaming MUST retain the decoded terminal URP output nodes until settlement. When authoritative provider meter usage is absent, MB-M4 MUST count call meters from those retained nodes using the same rule as non-stream settlement.

## 6. Charge Formula

MB-C1. Base charge is:

```text
base_charge = sum(token_line_items.charge_nano) + sum(meter_line_items.charge_nano)
```

MB-C2. Final charge is:

```text
final_charge = trunc(base_charge * provider_multiplier)
```

`provider_multiplier` MUST be an exact positive decimal sourced from a decimal string. Multiplication MUST use checked decimal/integer arithmetic and truncate toward zero without conversion through `f32` or `f64`.

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

MB-C5. A billable non-stream response or buffered synthetic stream without normalized usage MUST be rejected before delivery when the selected Channel has `allow_missing_usage = false`. A pass-through stream without terminal normalized usage MUST settle from an estimate whose input quantity is `ceil(serialized_upstream_request_utf8_bytes / 4)` and whose output quantity is `ceil(decoded_visible_output_utf8_bytes / 4)` when the selected Channel has `allow_missing_usage = false`; the resulting billing snapshot MUST contain `estimated = true`. When the selected Channel has `allow_missing_usage = true`, each of these missing-usage cases MUST instead settle with normalized input and output token quantities of zero and a total charge of zero. Present upstream usage MUST always take precedence over this Channel flag.

MB-C6. Once pass-through stream bytes have been delivered, a settlement error MUST NOT be converted into a successful zero-charge snapshot. Monoize MUST finalize the request log as an explicit billing failure containing the billing error code. The server MUST NOT claim that an error response was delivered downstream after the terminal stream event has already been sent.

## 7. Dashboard APIs

MB-A1. Admin endpoint `GET /api/dashboard/billing-rates` MUST return all billing-rate rows ordered by `pricing_profile ASC, priority DESC, id ASC`.

MB-A2. Admin endpoint `PUT /api/dashboard/billing-rates/{id}` MUST upsert one billing-rate row.

MB-A2a. If the request body omits `source`, the upserted row MUST use `source = "manual"`, even when a row with the same `id` already exists from `source = "catalog"` or `source = "models_dev"`.

MB-A2b. A billing-rate partial upsert MUST preserve omitted fields at the database write boundary. Concurrent partial upserts to distinct fields MUST NOT restore omitted fields from a stale pre-update snapshot on SQLite or PostgreSQL.

MB-A3. Admin endpoint `DELETE /api/dashboard/billing-rates/{id}` MUST delete one billing-rate row.

MB-A4. Admin endpoint `POST /api/dashboard/billing-rates/sync/catalog` MUST sync the bundled catalog. Manual rows with the same `id` MUST take precedence over catalog rows.

MB-A4a. Catalog sync MUST read the protected manual id set once and insert every non-protected catalog row through set-based statements split into fixed-size chunks. It MUST NOT issue one database round trip per catalog row. Every chunk MUST remain below the portable SQLite bound-parameter limit, and PostgreSQL MUST use the same chunking semantics.

MB-A4b. Bulk pricing-profile, Provider-type, and model-metadata lookup methods MUST split dynamic input sets into portable fixed-size chunks when necessary. A caller-controlled or database-sized input set MUST NOT exceed a backend bind-variable limit. Results from all chunks MUST preserve the method's documented deterministic order.

MB-A5. Admin endpoint `GET /api/dashboard/pricing-profile-patterns` MUST return the ordered profile-pattern setting.

MB-A6. Admin endpoint `PUT /api/dashboard/pricing-profile-patterns` MUST replace the ordered profile-pattern setting after rejecting empty `pattern` or `pricing_profile` strings.
