# Monoize Database Provider Routing Specification

## 0. Status

- Product name: Monoize.
- Internal protocol name: `URP-Proto`.
- Scope: runtime routing step for forwarding endpoints.

## 1. Inputs

R-IN-1. Routing input MUST include requested `model`.

R-IN-2. Routing input MAY include `max_multiplier`.

R-IN-3. Router MUST read providers from dashboard database in `priority ASC` order.

R-IN-4. Routing input MUST include request-scoped `effective_groups: string[] | null` as resolved by `api-key-authentication.spec.md` §4.

## 2. Provider Evaluation Order

R-ORD-1. Router MUST iterate providers in stored order (waterfall).

R-ORD-2. For each provider, static filtering MUST be applied in this order:

1. `provider.enabled == true`
2. `provider.models` contains requested model
3. if `max_multiplier` exists, `provider.models[model].multiplier <= max_multiplier`
4. provider is group-eligible under R-GRP-1

R-ORD-3. If any rule in R-ORD-2 fails, router MUST continue to next provider.

## 3. Provider Group Eligibility

R-GRP-0. For routing eligibility, `provider.groups` MUST be treated as the provider's canonical string-array label set. `provider.groups = []` means the provider is public. If a stored provider row has `groups` absent, null, empty string, or serialized empty array, routing MUST treat it as `[]` for backward compatibility.

R-GRP-0a. Public providers are always group-eligible.

R-GRP-1. A provider is group-eligible if and only if:

- `effective_groups == null` (unrestricted access), OR
- `provider.groups == []` (public provider), OR
- `intersection(provider.groups, effective_groups)` is non-empty

R-GRP-1a. If `effective_groups == []`, only public providers satisfy the group rule.

## 4. Channel Availability and Retry

R-CH-1. Candidate channels are channels with:

- `enabled == true`
- `weight > 0`
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

R-CH-10. If provider attempts are exhausted, router MUST continue to next provider (fail-forward).

R-CH-11. If all providers are exhausted, router MUST return `502 upstream_error`.

R-CH-12. If `provider.circuit_breaker_enabled == false`, routing MUST ignore runtime health state for that provider and retryable failures MUST NOT trip passive circuit breaking.

## 4. Model Rewriting

R-MDL-1. For selected provider model entry:

- upstream model = `redirect` when non-null and non-empty
- otherwise upstream model = requested model

## 5. Error Classification

R-ERR-1. Non-retryable errors are:

- HTTP `400`, `401`, `403`, `422`

R-ERR-2. Retryable errors are:

- HTTP `429`
- HTTP `5xx`
- timeout
- connection refused/reset

R-ERR-3. For non-retryable errors, router MUST stop immediately and return that error.

R-ERR-4. For retryable errors, router MUST try next channel according to retry budget.

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

R-H-4. Health state entry MUST be marked unhealthy when the count of failed samples within the sliding window (`window_seconds`) reaches `failure_count_threshold`.

R-H-5. If unhealthy is triggered by retryable `429`, cooldown MUST use `rate_limit_cooldown_seconds`; otherwise use `cooldown_seconds`.

R-H-6. Unhealthy state entries MUST be skipped during cooldown.

R-H-7. If active probing is enabled, channels whose cooldown elapsed MUST be probed periodically and recover after success threshold is reached. When `per_model_circuit_break == true`, a successful probe MUST clear all model-specific unhealthy entries for that channel.

R-H-8. If `provider.circuit_breaker_enabled == false`, active probing MUST be skipped for that provider.
