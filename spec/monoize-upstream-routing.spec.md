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

Runtime-only state MUST be maintained in memory:

- `_healthy: boolean` default `true`
- `_failure_count: integer` default `0`
- `_last_success_at: timestamp | null`

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
- `models: Record<string, ModelEntry>`
- `channels: Channel[]` where `length >= 1`
- `transforms: TransformRuleConfig[]` (ordered, default empty)

Implementation-specific extension:

- `provider_type: enum("responses","chat_completion","messages","gemini","grok")` MAY be present and determines upstream request shape.

### 2.4 Router Configuration

The router subsystem MUST support:

- ordered provider list
- `request_timeout_ms` default `30000`
- health-check config with passive and active sections

CFG-1. Provider configuration decoding MUST be fail-fast: invalid serialized provider fields (including `transforms`, `created_at`, `updated_at`) MUST return an explicit error and MUST NOT be silently coerced to defaults.

CFG-2. Each provider MAY define probe override fields:

- `active_probe_enabled_override: boolean?`
- `active_probe_interval_seconds_override: integer? (>= 1)`
- `active_probe_success_threshold_override: integer? (>= 1)`
- `active_probe_model_override: string?`

CFG-3. Probe precedence MUST be provider override first, then global settings fallback.

CFG-4. Global active probe settings MUST be treated as defaults. If global `enabled == false`, providers with `active_probe_enabled_override == true` MUST still be active-probed. Providers with `active_probe_enabled_override == false` MUST remain excluded regardless of global value.

## 3. Request Routing Parameters

The router MUST read:

- `model: string`
- `max_multiplier: number | null`

`max_multiplier` MAY be supplied by request body field `max_multiplier` or header `X-Max-Multiplier`.

## 4. Routing Algorithm

For each request:

RTA-1. Iterate providers in configured order.

RTA-2. Static filter rules for each provider:

- skip if `provider.enabled == false`
- skip if requested model does not exist in `provider.models`
- skip if `max_multiplier` is present and `provider.models[model].multiplier > max_multiplier`

RTA-3. Availability pre-check:

- candidate channels are those where `enabled == true`, `weight > 0`, and runtime state is healthy/probing-eligible.
- if candidate channels are empty, skip provider.

RTA-4. Execute provider with intra-provider retry:

- rewritten model = `redirect ?? requested model`
- attempt ordering uses weighted randomization over candidate channels
- attempts count:
  - if `max_retries == -1`: up to all candidate channels
  - else: `min(max_retries + 1, candidate_count)`

RTA-5. Error policy per attempt:

- non-retryable client errors (`400`, `401`, `403`, `422`) MUST stop immediately and return error to downstream
- retryable errors (`429`, `5xx`, timeout, connection refused) MUST advance to next channel attempt

RTA-6. On retryable attempt failure, channel passive health state MUST be updated.

RTA-7. If all attempts in provider fail, router MUST continue with next provider.

RTA-8. If all providers are exhausted, return `502` with message indicating no available upstream provider for requested model.

## 5. Streaming-specific Rule

STRM-1. If downstream streaming has already emitted any bytes, router MUST NOT switch provider/channel for that request.

STRM-2. Provider/channel fallback is allowed only before first downstream byte is emitted.

## 6. Health Check

### 6.1 Passive

- `failure_threshold` default `3`
- `cooldown_seconds` default `60`

PHS-1. When consecutive retryable failures reach threshold, channel MUST become unhealthy.

PHS-2. Unhealthy channel MUST not receive normal traffic during cooldown.

### 6.2 Active

- `enabled` default `true`
- `interval_seconds` default `30`
- `method` default `completion`
- `probe_model` default `null` (when null, use provider first model)
- `success_threshold` default `1`

AHS-1. Active probing MUST target unhealthy channels whose cooldown has elapsed.

AHS-2. Channel MUST return to healthy only after reaching success threshold.

AHS-3. When `method` is `completion`, probe MUST send a minimal completion request using `probe_model` when configured; otherwise it MUST use the provider's first model from its model map. If no model can be resolved, probing for that provider/channel MUST be skipped.

AHS-4. The completion probe request MUST use `max_tokens: 1` and a minimal single-user-message payload to minimize cost and latency.

AHS-5. Probe results MUST be logged at debug level with channel ID, provider name, probe model, and success/failure status.

AHS-6. Probe scheduler MUST enforce provider-level probe interval independently. A channel that is probe-eligible MUST be skipped until `now - last_probe_at >= effective_interval_seconds`.

## 7. Dashboard Requirements

UI-1. Providers page MUST be provider-centric and editable without exposing `api_key` values in read responses.

UI-2. Provider list MUST support priority reordering.

UI-3. Provider editor MUST include:

- provider enable toggle
- per-model redirect and multiplier table
- channel table (name, base URL, weight, enabled)
- channel runtime indicator (healthy/probing/unhealthy)
- max_retries setting
