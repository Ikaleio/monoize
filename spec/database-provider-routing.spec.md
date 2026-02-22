# Monoize Database Provider Routing Specification

## 0. Status

- Product name: Monoize.
- Internal protocol name: `URP-Proto`.
- Scope: runtime routing step for forwarding endpoints.

## 1. Inputs

R-IN-1. Routing input MUST include requested `model`.

R-IN-2. Routing input MAY include `max_multiplier`.

R-IN-3. Router MUST read providers from dashboard database in `priority ASC` order.

## 2. Provider Evaluation Order

R-ORD-1. Router MUST iterate providers in stored order (waterfall).

R-ORD-2. For each provider, static filtering MUST be applied in this order:

1. `provider.enabled == true`
2. `provider.models` contains requested model
3. if `max_multiplier` exists, `provider.models[model].multiplier <= max_multiplier`

R-ORD-3. If any rule in R-ORD-2 fails, router MUST continue to next provider.

## 3. Channel Availability and Retry

R-CH-1. Candidate channels are channels with:

- `enabled == true`
- `weight > 0`
- runtime state healthy or probing-eligible

R-CH-2. If no candidate channels exist, router MUST continue to next provider.

R-CH-3. Attempt count per provider MUST be:

- all candidate channels when `max_retries == -1`
- otherwise `min(max_retries + 1, candidate_channel_count)`

R-CH-4. Channel attempt order MUST use weighted randomization by `weight`.

R-CH-5. On successful attempt, router MUST return immediately.

R-CH-6. If provider attempts are exhausted, router MUST continue to next provider (fail-forward).

R-CH-7. If all providers are exhausted, router MUST return `502 upstream_error`.

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

R-H-1. Passive threshold and cooldown defaults:

- `failure_threshold = 3`
- `cooldown_seconds = 60`
- `window_seconds = 30`
- `min_samples = 20`
- `failure_rate_threshold = 0.6`
- `rate_limit_cooldown_seconds = 15`

R-H-2. Effective passive breaker parameters MUST be resolved per channel: channel override first, global setting fallback.

R-H-3. Channel MUST be marked unhealthy when either consecutive transient failures reach `failure_threshold`, or windowed failure rate reaches `failure_rate_threshold` with at least `min_samples`.

R-H-4. If unhealthy is triggered by retryable `429`, cooldown MUST use `rate_limit_cooldown_seconds`; otherwise use `cooldown_seconds`.

R-H-5. Unhealthy channels MUST be skipped during cooldown.

R-H-6. If active probing is enabled, channels whose cooldown elapsed MUST be probed periodically and recover after success threshold is reached.
