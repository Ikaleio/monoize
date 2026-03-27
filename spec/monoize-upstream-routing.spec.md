# Monoize Upstream Routing Specification

## 0. Status

- Product name: Monoize.
- Internal protocol name: `URP-Proto` (unchanged).
- Scope: provider/channel data model, ordered routing, fail-forward behavior, health checks.

## 1. Design Principles

MUS-1. Routing MUST be provider-centric. `Provider` is the top-level managed unit. `Channel` is a nested transport unit.

MUS-2. Routing MUST use ordered waterfall semantics. Providers are evaluated from index `0` to `N-1`.

MUS-3. Routing MUST fail forward. If one provider is exhausted, routing MUST continue with the next provider.

## 2. Data Model

### 2.1 Channel

A channel record MUST include:

- `id: string`
- `name: string`
- `base_url: string`
- `api_key: string`
- `weight: integer` where `weight >= 0` and default `1`
- `enabled: boolean` default `true`
- `groups: string[]` default `[]`

Runtime-only state MUST be maintained in memory:

- `_healthy: boolean` default `true`
- `_last_success_at: timestamp | null`
- `_passive_samples: sequence<{at_ts: timestamp, failed: boolean}>` (bounded by time-window pruning)

Channel-level passive breaker override fields MAY be present:

- `passive_failure_count_threshold_override: integer? (>= 1)`
- `passive_window_seconds_override: integer? (>= 1)`
- `passive_cooldown_seconds_override: integer? (>= 1)`
- `passive_rate_limit_cooldown_seconds_override: integer? (>= 1)`

### 2.1a Provider Group Semantics

CG-1. `groups` is an array of opaque string labels on the provider. `provider.groups = []` means the provider is public.

CG-2. On create/update, provider `groups` MUST be canonicalized by trimming each element, lowercasing, removing empty strings after trimming, deduplicating, and sorting ascending.

CG-3. If a stored provider row has `groups` absent, null, empty string, or serialized empty array, routing and read models MUST treat it as `[]` for backward compatibility.

### 2.2 Model Entry

A provider model entry MUST include:

- `redirect: string | null`
- `multiplier: number` where `multiplier > 0`

### 2.3 Provider

A provider record MUST include:

- `id: string`
- `name: string`
- `enabled: boolean` default `true`
- `max_retries: integer` default `-1`
- `channel_max_retries: integer` default `0`
- `channel_retry_interval_ms: integer` default `0`
- `circuit_breaker_enabled: boolean` default `true`
- `per_model_circuit_break: boolean` default `false`
- `models: Record<string, ModelEntry>`
- `channels: Channel[]` where `length >= 1`
- `groups: string[]` (default empty; provider-level group labels for routing eligibility)
- `transforms: TransformRuleConfig[]` (ordered, default empty)

Implementation-specific extension:
- `provider_type: enum("responses","chat_completion","messages","gemini")` MUST be present and determines the default upstream request shape.
- `api_type_overrides: ApiTypeOverride[]` (ordered, default empty) MAY be present. Each entry is `{ pattern: string, api_type: enum("responses","chat_completion","messages","gemini") }` where `pattern` uses glob syntax (`*` matches any sequence, `?` matches one character).

### 2.4 API Type Resolution

AT-1. For a given request model, the effective API type MUST be resolved as follows:

1. Iterate `api_type_overrides` in array order.
2. For each entry, test if `pattern` matches the requested model using glob semantics (case-sensitive, anchored).
3. If a match is found, the effective API type is that entry's `api_type`. Stop.
4. If no entry matches (or `api_type_overrides` is empty), the effective API type is `provider_type`.

AT-2. Glob matching MUST use the same semantics as transform model filtering: `*` matches zero or more characters, `?` matches exactly one character, matching is anchored (full string).

AT-3. The effective API type determines the upstream endpoint path and request encoding for that specific request.

### 2.5 Router Configuration

The router subsystem MUST support:

- ordered provider list
- `request_timeout_ms` default `30000`
- health-check config with passive and active sections
- global passive breaker defaults:
  - `passive_failure_count_threshold` default `3`
  - `passive_window_seconds` default `30`
  - `passive_cooldown_seconds` default `60`
  - `passive_rate_limit_cooldown_seconds` default `15`

CFG-1. Provider configuration decoding MUST be fail-fast: invalid serialized provider fields (including `transforms`, `created_at`, `updated_at`) MUST return an explicit error and MUST NOT be silently coerced to defaults.

CFG-2. Each provider MAY define probe override fields:

- `active_probe_enabled_override: boolean?`
- `active_probe_interval_seconds_override: integer? (>= 1)`
- `active_probe_success_threshold_override: integer? (>= 1)`
- `active_probe_model_override: string?`

CFG-3. Probe precedence MUST be provider override first, then global settings fallback.

CFG-4. Global active probe settings MUST be treated as defaults. If global `enabled == false`, providers with `active_probe_enabled_override == true` MUST still be active-probed. Providers with `active_probe_enabled_override == false` MUST remain excluded regardless of global value.

CFG-5. Passive breaker effective parameters MUST be resolved per channel with precedence:

1. channel override field (if present and non-null)
2. global passive breaker setting

The resolved parameters are: `passive_failure_count_threshold`, `passive_window_seconds`, `passive_cooldown_seconds`, `passive_rate_limit_cooldown_seconds`.

CFG-6. Each provider MAY define a timeout override field:

- `request_timeout_ms_override: integer? (>= 1)` — When set, overrides the global `request_timeout_ms` for all upstream calls made through this provider. Resolution order: provider override → global `request_timeout_ms` setting → 30000ms default.

## 3. Request Routing Parameters

The router MUST read:

- `model: string`
- `max_multiplier: number | null`
- `effective_groups: string[] | null`

`max_multiplier` MAY be supplied by request body field `max_multiplier` or header `X-Max-Multiplier`.

RRP-1. `effective_groups` is the request-scoped group filter produced by `api-key-authentication.spec.md` §4.

RRP-2. If `effective_groups == null`, the request is unrestricted by group filtering and may use all enabled providers, subject to the other routing rules.

RRP-3. If `effective_groups != null`, the request is restricted to providers whose `groups` match. When `effective_groups` is non-empty, public providers (groups=[]) are NOT eligible — only providers whose `groups` explicitly overlap with `effective_groups` are eligible. When `effective_groups` is empty (`[]`), only public providers (groups=[]) are eligible.

RRP-4. If `effective_groups == []`, only public providers are group-eligible.

## 4. Routing Algorithm

For each request:

RTA-1. Iterate providers in configured order.

RTA-2. Static filter rules for each provider:

- skip if `provider.enabled == false`
- skip if requested model does not exist in `provider.models`
- skip if `max_multiplier` is present and `provider.models[model].multiplier > max_multiplier`
- skip if provider is not group-eligible per RRP-1 through RRP-4

RTA-3. Availability pre-check:

- candidate channels are those where `enabled == true`, `weight > 0`, and runtime state is healthy/probing-eligible for the requested model (see §6.3 for per-model health keying).
- if `provider.circuit_breaker_enabled == false`, runtime health state MUST be ignored for normal routing eligibility. Disabled or zero-weight channels are still excluded.
- if candidate channels are empty, skip provider.

RTA-4. Execute provider with intra-provider retry:

- rewritten model = `redirect ?? requested model`
- attempt ordering uses weighted randomization over candidate channels
- total attempt budget:
  - if `max_retries == -1`: unlimited (try all channels × per-channel retries)
  - else: `max_retries + 1` total attempts across all channels
- per-channel attempt limit: `channel_max_retries + 1` (default `0 + 1 = 1`, i.e. one attempt per channel with no intra-channel retry)
- execution is nested: for each channel in weighted order, try up to per-channel limit, then move to next channel, all bounded by total attempt budget
- if the channel becomes unhealthy (breaker trips) during intra-channel retries, remaining retries on that channel MUST be aborted and execution MUST move to the next channel
- between intra-channel retry attempts on the same channel, the router MUST sleep for `channel_retry_interval_ms` milliseconds. If `channel_retry_interval_ms == 0` (default), no sleep is inserted.

RTA-5. Error policy per attempt:

- non-retryable client errors (`400`, `401`, `403`, `422`) MUST stop immediately and return error to downstream
- retryable errors (`429`, `5xx`, timeout, connection refused) MUST advance to next channel attempt

RTA-6. On retryable attempt failure, channel passive health state MUST be updated.

RTA-6a. If `provider.circuit_breaker_enabled == false`, retryable attempt failures MUST NOT trip passive health state and MUST NOT mark the channel unhealthy.

RTA-7. If all attempts in provider fail, router MUST continue with next provider.

RTA-8. If all providers are exhausted, return `502` with message indicating no available upstream provider for requested model.

## 5. Streaming-specific Rule

STRM-1. If downstream streaming has already emitted any bytes, router MUST NOT switch provider/channel for that request.

STRM-2. Provider/channel fallback is allowed only before first downstream byte is emitted.

## 6. Health Check

### 6.1 Health State Keying

HSK-1. When `provider.per_model_circuit_break == false` (default), health state MUST be keyed by `channel_id` alone. All models sharing a channel share one health state.

HSK-1a. When `provider.circuit_breaker_enabled == false`, health state MAY still exist in memory from prior configuration, but routing MUST ignore it and passive updates MUST NOT create new unhealthy state for that provider.

HSK-2. When `provider.per_model_circuit_break == true`, health state MUST be keyed by `(channel_id, logical_model)` where `logical_model` is the pre-redirect requested model. A circuit break for model A on channel X MUST NOT affect model B on the same channel.

HSK-3. Eligibility filtering (RTA-3) MUST use the appropriate health key when determining whether a channel is healthy for a given request model.

### 6.2 Passive

- `failure_count_threshold` default `3`
- `window_seconds` default `30`
- `cooldown_seconds` default `60`
- `rate_limit_cooldown_seconds` default `15`

PHS-1. On each retryable failure (transient or rate-limited), the health state entry MUST append one sample `{at_ts: now, failed: true}` and MUST prune samples older than `window_seconds`.

PHS-2. On each successful attempt, the health state entry MUST append one sample `{at_ts: now, failed: false}` and MUST prune samples older than `window_seconds`.

PHS-3. The health state entry MUST become unhealthy when the count of failed samples within the current window reaches `failure_count_threshold`.

PHS-4. When unhealthy is triggered by a retryable `429` failure, cooldown MUST use `rate_limit_cooldown_seconds`. Otherwise cooldown MUST use `cooldown_seconds`.

PHS-5. Unhealthy state entries MUST NOT receive normal traffic while `now < cooldown_until`.

PHS-6. On successful attempts, the health state entry MUST be restored to healthy immediately: `healthy := true`, `cooldown_until := None`, `probe_success_count := 0`, `last_probe_at := None`.

### 6.3 Active

- `enabled` default `true`
- `interval_seconds` default `30`
- `method` default `completion`
- `probe_model` default `null` (when null, use provider first model)
- `success_threshold` default `1`

AHS-1. Active probing MUST target unhealthy channels whose cooldown has elapsed.

AHS-1a. If `provider.circuit_breaker_enabled == false`, active probing MUST be skipped for that provider.

AHS-2. Channel MUST return to healthy only after reaching success threshold.

AHS-3. When `method` is `completion`, probe MUST send a minimal completion request using `probe_model` when configured; otherwise it MUST use the provider's first model from its model map. If no model can be resolved, probing for that provider/channel MUST be skipped.

AHS-4. The completion probe request MUST use `max_tokens: 16` and a minimal single-user-message payload to minimize cost and latency.

AHS-5. Probe results MUST be logged at debug level with channel ID, provider name, probe model, and success/failure status.

AHS-6. Probe scheduler MUST enforce provider-level probe interval independently. A channel that is probe-eligible MUST be skipped until `now - last_probe_at >= effective_interval_seconds`.

AHS-7. When `per_model_circuit_break == true`, a successful active probe MUST clear unhealthy state for ALL model-specific health entries associated with the probed channel. Active probing does not probe each model individually.

AHS-8. Active probe failure cooldown MUST use the effective `passive_cooldown_seconds` for the channel (channel override first, global fallback), consistent with passive breaker resolution.

## 7. Dashboard Requirements

UI-1. Providers page MUST be provider-centric and editable without exposing `api_key` values in read responses.

UI-2. Provider list MUST support priority reordering.

UI-3. Provider editor MUST include:

- provider enable toggle
- per-model redirect and multiplier table
- channel table (name, base URL, weight, enabled)
- channel runtime indicator (healthy/probing/unhealthy)
- max_retries setting
- channel_max_retries setting
- channel_retry_interval_ms setting
- circuit_breaker_enabled toggle
- per_model_circuit_break toggle

UI-4. Override fields (provider-level probe overrides, channel-level breaker overrides, timeout override) MUST display the effective global default value as placeholder text when the override is not set. Leaving a field empty MUST mean "inherit from global settings".

UI-5. Nullable boolean overrides MAY use a switch plus a separate reset-to-inherit affordance instead of a three-value selector. When inherited, the UI MUST display the effective global boolean value.
