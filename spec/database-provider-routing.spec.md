# Monoize Database Provider Routing Specification

## 0. Status

- Product name: Monoize.
- Internal protocol name: `URP-Proto`.
- Scope: runtime routing step for forwarding endpoints.

## 1. Inputs

R-IN-1. Routing input MUST include requested `model`.

R-IN-2. Routing input MAY include `max_multiplier`.

R-IN-2a. `max_multiplier` and Channel model multipliers MUST use exact decimal values sourced from decimal strings. Routing comparisons MUST NOT use binary floating point.

R-IN-3. Router MUST read providers from dashboard database in `priority ASC` order.

R-IN-4. Routing input MUST include request-scoped `effective_groups: string[] | null` (an ordered list of group ids) as resolved by `api-key-authentication.spec.md` §4. `null` occurs only for system-originated internal traffic.

R-IN-5. Request routing MUST select Provider and Channel rows through queries constrained by the resolved logical model, enabled Provider, enabled Channel, and Channel weight greater than zero. It MUST NOT load disabled or zero-weight routing rows and MUST NOT load model entries for unrelated logical models.

R-IN-6. Logical-model suffix resolution MAY query the candidate model names before Provider selection. A candidate MUST be considered available only when an enabled Provider offers it through an enabled Channel whose weight is greater than zero. The lookup MUST NOT reconstruct the Provider-Channel-Model graph for each candidate name. A database error from this lookup MUST abort the forwarding request with HTTP `500`; suffix resolution MUST NOT silently use the unnormalized model after a failed lookup.

R-IN-7. One Provider-selection operation MUST load its matching Provider, Channel, and model-entry data with a bounded query count independent of the number of Providers and Channels. A cross-request full-table routing cache is not required and MUST NOT be introduced by this rule.

## 2. Provider Evaluation Order

R-ORD-1. Router MUST iterate providers in stored order (waterfall).

R-ORD-2. For each provider, static filtering MUST be applied in this order:

1. `provider.enabled == true`
2. provider is group-eligible under R-GRP-1
3. at least one Channel contains the requested logical model in `channel.models`
4. if `max_multiplier` exists, at least one such Channel satisfies `channel.models[model].multiplier <= max_multiplier`

R-ORD-3. If any rule in R-ORD-2 fails, router MUST continue to next provider.

## 3. Provider Group Eligibility and Group-Order Priority

R-GRP-0. For routing eligibility, `provider.group_ids` MUST be treated as the provider's group-id set (`groups-registry.spec.md`). Every provider row stores at least one group id (GR-I2). Stored-value decoding follows GR-C4: absent, null, empty string, or serialized empty array decode as `[]`; any other malformed JSON, non-string array element, or database type mismatch MUST fail Provider decoding.

R-GRP-1. A provider is group-eligible if and only if:

- `effective_groups == null` (internal system traffic), OR
- `intersection(provider.group_ids, effective_groups)` is non-empty.

R-GRP-1a. If `effective_groups == []`, no provider is group-eligible.

R-GRP-2. Group order defines routing preference. For a non-null `effective_groups`, define
`group_rank(provider) = min { i : effective_groups[i] ∈ provider.group_ids }`. Before
attempt collection, group-eligible providers MUST be stably re-ordered by
`group_rank ASC`; within one rank the R-IN-3 order (`priority ASC`, `created_at ASC`) is
preserved. When `effective_groups == null`, every provider has rank `0` and R-IN-3 order
applies unchanged.

R-GRP-3. R-GRP-2 affects only attempt ordering. Channel affinity promotion
(`monoize-upstream-routing.spec.md` §AFF) applies after group-order sorting and MAY move a
bound attempt ahead of a lower-rank provider.

## 4. Channel Availability and Retry

R-CH-1. Candidate channels are channels with:

- `enabled == true`
- `weight > 0`
- `channel.models` contains the requested logical model
- when `max_multiplier` exists, `channel.models[model].multiplier <= max_multiplier`
- runtime health state is healthy or probing-eligible for the requested model (respecting per-model health keying when `per_model_circuit_break == true`)

R-CH-2. If no candidate channels exist, router MUST continue to next provider.

R-CH-3. Total attempt budget per provider MUST be:

- unlimited when `max_retries == -1`
- otherwise `max_retries + 1`

R-CH-4. Per-channel attempt limit MUST be `channel_max_retries + 1` (default 1, no intra-channel retry).

R-CH-5. Between same-channel retry attempts, the router MUST sleep for `channel_retry_interval_ms` milliseconds. If `channel_retry_interval_ms == 0`, no sleep is inserted.

R-CH-6. Channel attempt order MUST use weighted randomization by `weight`.

R-CH-7. Execution is nested: for each channel in weighted order, try up to per-channel limit, then move to next channel. All attempts are bounded by total attempt budget.

R-CH-8. If the channel becomes unhealthy (breaker trips) during intra-channel retries, remaining retries on that channel MUST be aborted immediately.

R-CH-9. On successful attempt, router MUST return immediately.

R-CH-10. If Provider attempts are exhausted before the first downstream byte, router MUST continue to the next Provider. An upstream HTTP status MUST NOT stop this fail-forward transition.

R-CH-11. If all providers are exhausted, router MUST return `502 upstream_error`.

R-CH-12. If `provider.circuit_breaker_enabled == false`, routing MUST ignore runtime health state for that provider and retryable failures MUST NOT trip passive circuit breaking.

R-CH-13. `monoize_channel_models` MUST have an index whose leading column is `model_name` so R-IN-5 does not require a full model-entry scan.

## 3.1 Provider mutations

R-MUT-1. Provider creation, initial Channel creation, and initial Channel model-entry creation MUST commit in one database transaction. Any failure MUST leave none of those rows persisted.

R-MUT-1a. If Provider creation omits `priority`, the assigned priority MUST equal `0` when the Provider table is empty and MUST otherwise equal the current maximum priority plus `1`. SQLite and PostgreSQL MUST decode the aggregate through the same signed 64-bit SQL type. PostgreSQL MUST serialize this automatic assignment against concurrent Provider priority mutations. An aggregate decode error or signed 32-bit overflow MUST abort the transaction.

R-MUT-2. A Provider partial update MUST write only the fields present in that update. Concurrent partial updates to distinct Provider fields MUST NOT restore omitted fields from an earlier snapshot.

R-MUT-3. When a Provider update replaces `channels`, the Provider-field update and complete Channel replacement MUST commit in one database transaction.

R-MUT-4. Provider reorder MUST read only Provider ids. Validation and all priority writes MUST execute in one transaction. Priority writes MUST use one set-based statement; the query count MUST NOT increase with Provider count.

R-MUT-4a. Reordering an empty Provider table with an empty `provider_ids` list MUST succeed without issuing a priority update. An empty list MUST fail exact-set validation when at least one Provider exists.

R-MUT-5. `list_providers()` MUST reconstruct all Providers, Channels, and Channel model entries with exactly three set-based queries. `get_provider(id)` MUST use exactly three queries constrained to that Provider. Neither method may issue a query inside a Provider or Channel loop.

R-MUT-6. Complete Channel replacement MUST insert Channel rows and Channel model-entry rows with set-based statements split into fixed-size chunks. The operation MUST NOT issue one database round trip per Channel or per model entry. Every chunk MUST remain below the portable SQLite bound-parameter limit, and PostgreSQL MUST use the same chunking semantics.

R-MUT-7. `MONOIZE_PROVIDER_REORDER_MAX_IDS` MUST configure the positive maximum Provider ids accepted by reorder. Missing, empty, zero, negative, invalid, or overflowing values MUST use `199`; values above `199` MUST be clamped to `199`. Provider reorder MUST reject a larger input before starting a transaction. The same limit MUST apply on SQLite and PostgreSQL so the one set-based priority statement uses at most 399 bound values.

R-MIG-1. Startup transform-id canonicalization MUST scan Provider rows in `id ASC` keyset batches of at most `199`. Each batch read and its set-based updates MUST commit in one transaction before the next batch starts. Memory use MUST be `O(199)` and MUST NOT depend on total Provider count. The completion marker MUST be written in a separate final transaction after every batch commits.

## 4.1 Channel Affinity and Failback

R-AFF-1. Every eligible Channel attempt MUST resolve effective affinity settings by taking each non-null Channel override before the matching global setting.

R-AFF-2. A successful attempt whose effective affinity enabled value is false MUST NOT create or refresh a binding to that Channel.

R-AFF-3. A binding whose target Channel resolves effective affinity enabled value false MUST be removed during targeted lookup.

R-AFF-4. `"sticky"` mode MUST keep an eligible bound attempt ahead of normal Provider order.

R-AFF-5. `"prefer_higher_priority"` mode MUST restore normal Provider order after the effective failback delay only when a different eligible Provider precedes the bound Provider. Weighted Channel order inside the bound Provider MUST NOT trigger this transition.

R-AFF-6. An affinity-prioritized attempt and a normal-order failback attempt MUST remain subject to R-CH-3 through R-CH-8.

## 4. Model Rewriting

R-MDL-1. For the selected Channel model entry:

- upstream model = `redirect` when non-null and non-empty
- otherwise upstream model = requested model

R-MDL-2. The attempt `model_multiplier` MUST equal the selected Channel model entry multiplier. Two Channels in one Provider MAY use different redirect and multiplier values for the same logical model.

R-MDL-3. The available-model-name query used by `/v1/models` MUST return sorted distinct logical model names from Channels that satisfy all of: Provider enabled, Channel enabled, and Channel weight greater than zero. It MUST use one set-based database query and MUST NOT hydrate Provider or Channel objects.

## 5. Error Classification

R-ERR-1. Every upstream HTTP, timeout, connection, response-decoding, or response-validation error before the first downstream byte MUST fail the current attempt. The router MUST continue routing until an attempt succeeds or all eligible attempts are exhausted.

R-ERR-2. Same-Channel retry errors are:

- HTTP `408`
- HTTP `429`
- HTTP `5xx`
- timeout
- connection refused/reset

R-ERR-3. For a same-Channel retry error, the router MAY retry that Channel within R-CH-3 through R-CH-8.

R-ERR-4. For any other upstream error, the router MUST NOT retry the same Channel. The router MUST advance to the next eligible Channel or Provider.

R-ERR-5. A Monoize authentication, authorization, balance, request-validation, request-encoding, transform, billing, or internal error MUST stop routing.

R-ERR-6. Persistent Channel failure statuses and structured signals, RateLimited structured signals, and Transient structured signals MUST equal RTA-5a and RTA-5b in `monoize-upstream-routing.spec.md`. Persistent failures MUST NOT retry the same Channel and MUST make the relevant health entry unhealthy after the first occurrence. HTTP `400`, `409`, and `422` without a listed signal MUST NOT update Channel health.

## 6. Streaming Constraint

R-STR-1. For streaming requests, router MAY switch channel/provider only before first downstream byte is emitted.

R-STR-2. After first downstream byte emission, channel/provider switching MUST NOT occur.

## 7. Health State

R-H-1. Passive breaker defaults:

- `failure_count_threshold = 3`
- `window_seconds = 30`
- `cooldown_seconds = 60`
- `rate_limit_cooldown_seconds = 15`

R-H-2. Effective passive breaker parameters MUST be resolved per channel: channel override first, global setting fallback.

R-H-3. Health state MUST be keyed by `channel_id` when `per_model_circuit_break == false`, or by `(channel_id, logical_model)` when `per_model_circuit_break == true`.

R-H-4. A Persistent failure defined by R-ERR-6 MUST mark the health state entry unhealthy immediately. A Transient or RateLimited failure MUST mark it unhealthy when the count of failed samples within the sliding window (`window_seconds`) reaches the effective threshold defined by R-H-15.

R-H-5. If unhealthy is triggered by retryable `429`, cooldown MUST use `rate_limit_cooldown_seconds`; otherwise use `cooldown_seconds`.

R-H-6. Unhealthy state entries MUST be skipped during cooldown.

R-H-7. If active probing is enabled, channels whose cooldown elapsed MUST be probed periodically and recover after success threshold is reached. When `per_model_circuit_break == true`, a successful probe MUST clear all model-specific unhealthy entries for that channel.

R-H-8. If `provider.circuit_breaker_enabled == false`, active probing MUST be skipped for that provider.

R-H-9. Every successful mutation of a passive-health or active-probe system setting MUST advance the routing configuration revision and clear all runtime Channel health entries.

R-H-10. A request or active probe captured under an older routing configuration revision MUST NOT publish a health-state update after the revision advances.

R-H-11. Each active-probe scheduler tick MUST load only enabled Providers with `circuit_breaker_enabled = true` and at least one enabled Channel whose weight is greater than zero. It MUST load only enabled, positive-weight Channel rows. Loading their Providers, Channels, and Channel model entries MUST use a bounded query count independent of the number of Providers and Channels.

R-H-12. `MONOIZE_CHANNEL_HEALTH_MAX_ENTRIES` MUST configure the positive process-local Channel health entry limit. An unset, empty, zero, negative, or invalid value MUST use `10000`.

R-H-13. The process-local Channel health map MUST NOT exceed its configured entry limit. At capacity, a new health key MUST NOT be inserted or evict an existing key. Every missing health key MUST be treated as ineligible until an entry slot becomes available. Capacity checks MUST be constant-time and MUST NOT scan the health map.

R-H-14. Active-probe evaluation of a Channel with per-model circuit breaking MUST derive health keys from that Channel's configured model set. It MUST perform point lookups for those keys and MUST NOT scan the complete Channel health map.

R-H-15. `MONOIZE_CHANNEL_PASSIVE_FAILURE_SAMPLE_MAX_ENTRIES` MUST configure the positive process-local maximum passive-failure threshold per health-state entry. An unset, empty, zero, negative, invalid, or overflowing value MUST use `1024`. The effective threshold MUST equal the smaller of this limit and the resolved Channel threshold and MUST be at least `1`.

R-H-16. Before a passive success update or failure-count evaluation, the runtime MUST remove breaker-relevant failure timestamps older than the resolved window from the front of the queue. The queue length MUST NOT exceed the effective threshold. Failure-count evaluation MUST use the queue length and MUST NOT scan the queue. A successful request MUST NOT append a queue element. If the Provider circuit breaker is disabled, request success and failure handling MUST NOT insert or modify a health entry.

R-AFF-7. `MONOIZE_CHANNEL_AFFINITY_MAX_ENTRIES` MUST configure the positive process-local Channel affinity binding limit. An unset, empty, zero, negative, or invalid value MUST use `4096`.

R-AFF-8. At capacity, an existing affinity key MUST remain refreshable and a new affinity key MUST be rejected without eviction. Capacity checks MUST be constant-time and MUST NOT scan the affinity map.

R-AFF-9. Every binding MUST store an explicit expiration timestamp computed from the successful attempt's effective idle TTL. A hot lookup MUST inspect and, when expired, remove only the requested key in constant time.

R-AFF-10. `MONOIZE_CHANNEL_AFFINITY_CLEANUP_INTERVAL_SECONDS` MUST configure a positive background cleanup interval and MUST default to `60` for a missing, empty, zero, negative, invalid, or overflowing value. A cleanup tick MAY scan only the bounded affinity map and MUST NOT run as part of request lookup, insertion, or refresh.

## 8. Provider Dashboard Pricing Availability

R-DASH-1. A Provider list request MUST collect all unique `(normalized_model, effective_provider_type)` pricing pairs before evaluating billable coverage.

R-DASH-2. The request MUST load metadata pricing profiles for all unique normalized models in one set query and MUST load candidate billing-rate rows for all candidate profiles and effective Provider types in one set query.

R-DASH-3. After the two set queries in R-DASH-2, pricing-profile selection, Provider-type filtering, model-pattern matching, and rate-matrix completeness checks MUST run in process memory. The request MUST NOT execute a metadata or billing-rate query per model, Channel, Provider, or pair.

R-DASH-4. Provider dashboard pricing availability MUST use request-local data only. It MUST NOT create a cross-request full-table metadata or billing-rate cache.
