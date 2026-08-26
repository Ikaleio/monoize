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
- `_last_success_at: timestamp | null`
- `_passive_failure_timestamps: sequence<timestamp>`

Channel-level passive breaker override fields MAY be present:

- `passive_failure_count_threshold_override: integer? (>= 1)`
- `passive_window_seconds_override: integer? (>= 1)`
- `passive_cooldown_seconds_override: integer? (>= 1)`
- `passive_rate_limit_cooldown_seconds_override: integer? (>= 1)`

### 2.1a Provider Group Semantics

CG-1. `group_ids` is an array of first-class group ids on the provider
(`groups-registry.spec.md`). Every persisted provider row has at least one group id
(GR-I2); there is no "public provider" tier.

CG-2. On create/update, provider `group_ids` MUST be canonicalized and validated per
GR-C1..GR-C3. A canonicalized empty array MUST be stored as `[default_group_id]`.

CG-3. Stored `group_ids` decoding follows GR-C4. A decoded `[]` (possible only on rows
written outside the API) matches no request groups and is therefore never eligible for
API-key traffic.

### 2.2 Channel Model Entry

A Channel model entry MUST include:

- `redirect: string | null`
- `multiplier: string` containing a representable base-10 decimal with at most 9 fractional digits where `multiplier > 0`

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
- `channels: Channel[]` where `length >= 1`
- `group_ids: string[]` (provider-level group ids for routing eligibility; stored non-empty per CG-2)
- `transforms: TransformRuleConfig[]` (ordered, default empty)

Implementation-specific extension:
- A provider MUST NOT contain `provider_type`.
- Each channel MUST contain `provider_type: enum("responses","chat_completion","messages","gemini","openai_image","replicate")`; this value determines the channel default upstream request shape.
- Each Channel MUST contain `models: Record<string, ModelEntry>`.
- Provider MUST NOT contain a `models` field.
- `api_type_overrides: ApiTypeOverride[]` (ordered, default empty) MAY be present at provider level. Each entry is `{ pattern: string, api_type: enum("responses","chat_completion","messages","gemini","openai_image","replicate") }` where `pattern` uses glob syntax (`*` matches any sequence, `?` matches one character).

### 2.4 API Type Resolution

AT-1. For a given request model and selected channel, the effective API type MUST be resolved as follows:

1. Iterate `api_type_overrides` in array order.
2. For each entry, test if `pattern` matches the requested model using glob semantics (case-sensitive, anchored).
3. If a match is found, the effective API type is that entry's `api_type`. Stop.
4. If no entry matches (or `api_type_overrides` is empty), the effective API type is the selected channel's `provider_type`.

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

CFG-1. Provider configuration decoding MUST be fail-fast: invalid serialized provider fields (including `transforms`, `created_at`, `updated_at`) MUST return an explicit error and MUST NOT be silently coerced to defaults. Every persisted Provider or Channel integer-backed boolean MUST be exactly `0` or `1`; any other integer MUST fail decoding.

CFG-2. Each provider MAY define probe override fields:

- `active_probe_enabled_override: boolean?`
- `active_probe_interval_seconds_override: integer? (>= 1)`
- `active_probe_success_threshold_override: integer? (>= 1)`
- `active_probe_model_override: string?`

CFG-3. Probe precedence MUST be provider override first, then global settings fallback.

CFG-3a. Each channel MAY define the same active probe override fields. Active probe precedence MUST be channel override, then provider override, then global settings fallback.

CFG-4. Global active probe settings MUST be treated as defaults. If global `enabled == false`, channels or providers with `active_probe_enabled_override == true` MUST still be active-probed. If a channel or provider resolves `active_probe_enabled_override == false`, that channel MUST remain excluded regardless of global value.

CFG-5. Passive breaker effective parameters MUST be resolved per channel with precedence:

1. channel override field (if present and non-null)
2. global passive breaker setting

The resolved parameters are: `passive_failure_count_threshold`, `passive_window_seconds`, `passive_cooldown_seconds`, `passive_rate_limit_cooldown_seconds`.

CFG-5a. `MONOIZE_CHANNEL_PASSIVE_FAILURE_SAMPLE_MAX_ENTRIES` MUST configure the positive process-local maximum passive-failure threshold per health-state entry. An unset, empty, zero, negative, invalid, or overflowing value MUST use `1024`.

CFG-5b. The effective passive-failure threshold MUST equal `min(resolved passive_failure_count_threshold, MONOIZE_CHANNEL_PASSIVE_FAILURE_SAMPLE_MAX_ENTRIES)`. The effective threshold MUST be at least `1`.

CFG-6. Each provider MAY define a timeout override field:

- `request_timeout_ms_override: integer? (>= 1)` — When set, overrides the global `request_timeout_ms` for all upstream calls made through this provider. Resolution order: provider override → global `request_timeout_ms` setting → 30000ms default.

## 3. Request Routing Parameters

The router MUST read:

- `model: string`
- `max_multiplier: string | null`
- `effective_groups: string[] | null`

`max_multiplier` MAY be supplied by request body field `max_multiplier` or header `X-Max-Multiplier`.

RRP-0. Channel multipliers and request/API-key multiplier ceilings MUST be parsed and compared as exact decimals. They MUST NOT be converted through `f32` or `f64`. JSON responses and stored routing policy MUST use canonical decimal strings.

RRP-1. `effective_groups` is the request-scoped ordered group-id filter produced by `api-key-authentication.spec.md` §4.

RRP-2. If `effective_groups == null` (internal system traffic only), the request is unrestricted by group filtering and may use all enabled providers, subject to the other routing rules.

RRP-3. If `effective_groups != null`, the request is restricted to providers whose `group_ids` overlap `effective_groups` (`database-provider-routing.spec.md` R-GRP-1). Group order defines provider-ordering preference per R-GRP-2.

RRP-4. If `effective_groups == []`, no provider is group-eligible.

## 4. Routing Algorithm

For each request:

RTA-1. Iterate providers in configured order.

RTA-2. Static filter rules for each provider:

- skip if `provider.enabled == false`
- skip if provider is not group-eligible per RRP-1 through RRP-4
- skip if no Channel has a model entry for the requested logical model that satisfies the request `max_multiplier`

RTA-3. Availability pre-check:

- candidate channels are those where `enabled == true`, `weight > 0`, `models` contains the requested logical model, the Channel model multiplier does not exceed request `max_multiplier` when present, and runtime state is healthy/probing-eligible for the requested model (see §6.3 for per-model health keying).
- if `provider.circuit_breaker_enabled == false`, runtime health state MUST be ignored for normal routing eligibility. Disabled or zero-weight channels are still excluded.
- if candidate channels are empty, skip provider.

RTA-4. Execute provider with intra-provider retry:

- rewritten model = the selected Channel model entry `redirect ?? requested model`
- attempt multiplier = the selected Channel model entry `multiplier`
- attempt ordering uses weighted randomization over candidate channels
- total attempt budget:
  - if `max_retries == -1`: unlimited (try all channels × per-channel retries)
  - else: `max_retries + 1` total attempts across all channels
- per-channel attempt limit: `channel_max_retries + 1` (default `0 + 1 = 1`, i.e. one attempt per channel with no intra-channel retry)
- execution is nested: for each channel in weighted order, try up to per-channel limit, then move to next channel, all bounded by total attempt budget
- if the channel becomes unhealthy (breaker trips) during intra-channel retries, remaining retries on that channel MUST be aborted and execution MUST move to the next channel
- after a shared-origin blast defined by RTA-6c, remaining not-yet-attempted Channels of that Provider whose origin key equals the failed Channel's origin key MUST be skipped for this request without consuming the Provider attempt budget
- between intra-channel retry attempts on the same channel, the router MUST sleep for `channel_retry_interval_ms` milliseconds. If `channel_retry_interval_ms == 0` (default), no sleep is inserted.

RTA-4a. Bound-target extra retry. When the current attempt is the request's affinity-hit target (`affinity_hit == true` per AFF-7/AFF-7a), the router MUST allow at most one same-Channel attempt beyond the RTA-4 per-channel limit, so the effective per-channel attempt limit for that attempt is `channel_max_retries + 2`. The extra attempt is authorized if and only if all of the following hold after the most recent failure on that Channel:

1. the failure is same-Channel retryable per RTA-5,
2. the failure class is Transient (the upstream HTTP status is not `429`),
3. the Channel health state for the request's health key (HSK-1/HSK-2) is still healthy,
4. no shared-origin skip (RTA-6c step 3) applies to the attempt,
5. the Provider total attempt budget of RTA-4 is not exhausted.

The extra attempt consumes the Provider total attempt budget normally and MUST honor the RTA-4 `channel_retry_interval_ms` sleep. An attempt that is not the affinity-hit target MUST NOT receive this extra attempt. Purpose: one transient fault on the bound Channel MUST NOT force a Channel switch that discards upstream prompt-cache locality.

RTA-5. Error policy per attempt:

- Every upstream HTTP, timeout, connection, response-decoding, or response-validation error that occurs before the first downstream byte MUST end the current attempt. Monoize MUST continue routing until an attempt succeeds or all eligible attempts are exhausted.
- HTTP `408`, HTTP `429`, HTTP `5xx`, timeout, and connection errors MAY retry the same Channel. Same-Channel retries MUST remain within RTA-4 limits.
- Other upstream errors, including HTTP `400`, `401`, `403`, and `422`, MUST NOT retry the same Channel. They MUST still advance to the next eligible Channel or Provider.
- A Monoize authentication, authorization, balance, request-validation, request-encoding, transform, billing, or internal error MUST stop routing. Monoize MUST NOT classify such an error as an upstream attempt failure.

RTA-5a. A persistent Channel failure is an upstream failure that satisfies at least one condition below:

1. Its HTTP status is one of `401`, `402`, `403`, `404`, `405`, `407`, `410`, `415`, `426`, or `451`.
2. Its structured upstream `code` or `type`, after ASCII lowercase conversion and surrounding whitespace removal, is one of `account_deactivated`, `account_suspended`, `authentication_error`, `billing_hard_limit_reached`, `credit_balance_too_low`, `deployment_not_found`, `insufficient_balance`, `insufficient_quota`, `invalid_api_key`, `model_not_found`, `model_not_supported`, `no_available_account`, `permission_denied`, `quota_exceeded`, or `unsupported_model`.

A persistent Channel failure MUST NOT authorize a same-Channel retry. HTTP `400`, `409`, and `422` without a listed structured signal are request-specific and MUST NOT be persistent Channel failures.

RTA-5b. A structured upstream `code` or `type` equal to `rate_limit_error`, `rate_limit_exceeded`, or `too_many_requests` MUST have the RateLimited passive-failure class. A structured upstream `code` or `type` equal to `overloaded_error`, `server_error`, `service_unavailable`, or `temporarily_unavailable` MUST have the Transient passive-failure class. Signal comparison uses the normalization in RTA-5a. HTTP `429` MUST take the RateLimited class regardless of structured signals. An RTA-5a persistent HTTP status MUST take the Persistent class regardless of structured signals. For all other statuses, a persistent structured signal takes precedence over a Transient HTTP status or structured signal.

RTA-6. On an HTTP `408`, HTTP `429`, HTTP `5xx`, timeout, connection failure, RTA-5a persistent failure, or RTA-5b structured failure, Channel passive health state MUST be updated. A persistent failure MUST mark the relevant health entry unhealthy after its first occurrence. A Transient or RateLimited failure MUST use the threshold in PHS-3.

RTA-6b. Other upstream failures MUST NOT update Channel passive health state.

RTA-6a. If `provider.circuit_breaker_enabled == false`, retryable attempt failures MUST NOT trip passive health state and MUST NOT mark the channel unhealthy.

RTA-6c. Shared-origin blast. When a retryable failure's upstream HTTP status is `502`, `503`, or `524`, Monoize MUST treat the failure as a shared-origin blast for that attempt's Provider:

1. The origin key is `lowercase(scheme) + "://" + lowercase(host) + ":" + port`, where `scheme`, `host`, and `port` come from parsing the Channel `base_url`. `port` is the URL's explicit port when present, otherwise the default port for that scheme (`80` for `http`, `443` for `https`). Only `http` and `https` origins are valid. If `base_url` cannot be parsed into such an origin, that Channel has no origin key and MUST NOT share health with any other Channel.
2. Every other Channel of the same Provider that is enabled, has `weight > 0`, and has an equal origin key MUST receive the same Transient passive-failure sample as the failed Channel, using the failed attempt's resolved passive parameters and the same timestamp. Peer updates MUST use the same health keying as the failed attempt (HSK-1 / HSK-2). A peer insert that would exceed HSK-6 MUST be skipped.
3. Remaining same-Channel retries and remaining not-yet-attempted same-origin Channels of that Provider MUST be skipped for the rest of this request.
4. HTTP `408` and HTTP `429` MUST NOT trigger a shared-origin blast. They remain per-Channel.

RTA-6d. A shared-origin blast MUST classify cooldown as Transient (`passive_cooldown_seconds`), not as the rate-limit cooldown.

RTA-7. If all attempts in Provider fail before the first downstream byte, router MUST continue with the next Provider. The status code of an upstream failure MUST NOT stop cross-Provider fail-forward.

RTA-8. If all providers are exhausted for a non-streaming downstream request, return `502` with the sanitized message defined by `spec/upstream-error-sanitization.spec.md` SAN-6, which identifies the exhausted model and MUST NOT expose upstream URLs, attempt counts, or provider/channel identifiers. If the final failed attempt has a non-empty upstream error code, the downstream error `code` and request-log `error_code` MUST equal that upstream code. Otherwise they MUST equal `upstream_error`. The diagnostic `upstream_code` field MUST remain present when an upstream code exists. If all providers are exhausted before the first downstream byte for a streaming downstream request, return the protocol-specific stream error defined by `spec/unified_responses_proxy.spec.md` FP4e with `error.code = "upstream_error"` unless a final upstream error code is available. This rule preserves fail-forward behavior and does not authorize a same-Channel retry for HTTP `400`, `401`, `403`, or `422`.

RTA-8a. RTA-8 has one structured-error exception. If the final failed attempt has `upstream_code = "thinking_signature_invalid"`, Monoize MUST return `error.code = "thinking_signature_invalid"` and MUST use the final attempt's client-facing error text (`spec/upstream-error-sanitization.spec.md` SAN-8) as the downstream message without an `All upstream attempts failed` wrapper. If the final `upstream_status` is a valid HTTP `4xx` status, Monoize MUST return that status. Otherwise Monoize MUST return HTTP `400`. The request log `error_code` and `error_http_status` MUST equal the downstream values; the request log `error_message` MUST equal the persisted internal detail per `spec/upstream-error-sanitization.spec.md` SAN-9 (read-time disclosure per its section 8). Monoize MUST apply this rule only after all eligible Channels and Providers are exhausted; it MUST NOT disable fail-forward.

## 5. Streaming-specific Rule

STRM-1. If downstream streaming has emitted a protocol data or error event, router MUST NOT switch provider/channel for that request. An SSE comment or protocol keep-alive event emitted before the first protocol data or error event does not disable fallback.

STRM-2. Provider/channel fallback is allowed only before the first downstream protocol data or error event is emitted.

STRM-3. A streaming request MUST write or refresh affinity only after the upstream stream completes without terminal error.

STRM-4. If a partial stream later fails with a breaker-relevant terminal failure — an in-stream terminal error event classified by RTA-5a, RTA-5b, or RTA-6, or a stream adapter failure whose error code starts with `upstream_` (idle timeout, stream decode failure, protocol error, missing stream terminal) — Monoize MUST record exactly one passive health failure for the serving Channel using the attempt's health key. HTTP `429` and RTA-5b rate-limit signals have the RateLimited class. An RTA-5a signal has the Persistent class and MUST trip immediately. Other qualifying events and adapter failures have the Transient class. A mid-stream failure MUST NOT trigger a shared-origin blast and MUST NOT mark peer Channels. An event outside these classifications MUST NOT update health. An adapter failure whose error code does not start with `upstream_` (internal transform or encode failure) MUST NOT update health and MUST NOT clear the affinity binding.

STRM-4a. After recording the STRM-4 sample, Monoize MUST clear the request's affinity binding if and only if the serving attempt is the request's affinity-hit target (`affinity_hit == true`) and the Channel health state for the attempt's health key is unhealthy after the sample (AFF-9 conditions 1 and 3). In every other case the binding MUST remain stored.

## 5.1 Channel Affinity

AFF-1. Affinity MUST be stored in process memory only.

AFF-1a. Global affinity settings MUST be:

- `monoize_affinity_enabled: boolean` (default `true`)
- `monoize_affinity_idle_ttl_seconds: integer >= 1` (default `1800`)
- `monoize_affinity_failback_mode: enum("sticky", "prefer_higher_priority")` (default `"sticky"`)
- `monoize_affinity_failback_delay_seconds: integer >= 0` (default `300`)

AFF-1b. Each Channel MAY define these nullable override fields:

- `affinity_enabled_override: boolean?`
- `affinity_idle_ttl_seconds_override: integer?`
- `affinity_failback_mode_override: enum("sticky", "prefer_higher_priority")?`
- `affinity_failback_delay_seconds_override: integer?`

For each field, a non-null Channel value MUST replace the matching global value for that Channel. A null Channel value MUST inherit the global value. Therefore `affinity_enabled_override = true` MAY enable affinity for one Channel when `monoize_affinity_enabled = false`, and `affinity_enabled_override = false` MUST disable affinity for one Channel when `monoize_affinity_enabled = true`.

AFF-2. On every successful binding write or refresh, `last_used_at` MUST equal `now` and `expires_at` MUST equal the saturating sum `now + effective affinity_idle_ttl_seconds`. A binding MUST be expired when `now >= expires_at`.

AFF-3. The affinity cache key MUST include:

- forwarding API key id if present, otherwise authenticated user id if present
- logical model
- explicit stable metadata field value if present
- otherwise a hash of the normalized input prefix

AFF-4. Explicit stable metadata fields are Responses `previous_response_id`, session, conversation, thread, and user-like fields from request metadata or extra body. `previous_response_id` takes precedence over other explicit fields. Per-request ids, including downstream `request_id`, MUST NOT be used as affinity keys.

AFF-5. The fallback input-prefix hash MUST hash normalized request input only. It MUST consider at most the first 8 input nodes and at most the first 16384 bytes of their normalized JSON/text material. The implementation MUST stop serialization when it reaches the byte limit. It MUST NOT serialize or allocate material after that limit. Raw affinity material MUST NOT be persisted.

AFF-5a. The input-prefix hash MUST be deterministic: two requests whose decoded input nodes are equal MUST produce byte-identical hash material within one process lifetime and across process restarts. Node serialization for this hash MUST emit passthrough extra fields of a node, and of its nested tool-result content parts, in ascending lexicographic byte order of their keys. Container iteration order that is not defined by the data itself MUST NOT influence the hash.

AFF-6. The affinity value MUST contain `(provider_id, channel_id, bound_at, last_used_at, expires_at)`.

AFF-7. If an affinity hit points to a Provider+Channel that is still eligible for the request, routing MAY jump directly to that attempt before normal provider-order attempts. This jump consumes the normal provider/channel attempt budget.

AFF-7a. Under effective failback mode `"sticky"`, an eligible unexpired binding MUST remain first even when an earlier Provider has recovered.

AFF-7b. Under effective failback mode `"prefer_higher_priority"`, Monoize MUST use normal waterfall order instead of moving the bound attempt first when both conditions hold:

- `now - bound_at >= effective affinity_failback_delay_seconds`
- the normal eligible-attempt list contains an attempt from a different Provider before the bound attempt

Attempts from the same Provider that happen to precede the bound Channel after weighted randomization MUST NOT trigger failback.

AFF-7c. If AFF-7b uses normal waterfall order, request logging MUST set `affinity_hit = false` and retain the prior binding target in `affinity_target`. A successful attempt MUST replace or refresh the binding with that attempt as target and MUST set `bound_at` to the success time. If normal waterfall returns to the prior bound target after earlier attempts fail, `bound_at` MUST also reset to the success time.

AFF-7d. A successful request that used the bound target through AFF-7 or AFF-7a MUST update `last_used_at` and MUST preserve `bound_at`.

AFF-8. If the bound Provider+Channel is stale, affinity-disabled, disabled, zero weight, group-ineligible, multiplier-ineligible, or does not support the logical model, the binding MUST be cleared and normal waterfall routing MUST begin from the first provider.

AFF-8a. If the bound Provider+Channel is merely unhealthy or otherwise absent from this request's eligible attempt list, the binding MUST remain stored. Routing MUST NOT jump to that target for this request. Waterfall MUST continue from the first eligible attempt. When the bound target is eligible again, AFF-7 MUST apply to the retained binding.

AFF-9. A breaker-relevant failure defined by RTA-5a, RTA-5b, RTA-6, or STRM-4 MUST clear the request's affinity binding if and only if all of the following hold:

1. the failed attempt is the request's affinity-hit target (`affinity_hit == true` per AFF-7/AFF-7a),
2. the failure is not a shared-origin blast per RTA-6c,
3. after the RTA-6 passive-failure sample is recorded, the Channel health state for the attempt's health key (HSK-1/HSK-2) is unhealthy.

In every other case the binding MUST remain stored: a failure on an attempt that is not the affinity-hit target MUST NOT clear the binding, a sub-threshold Transient or RateLimited failure on the bound target MUST NOT clear the binding, and a shared-origin blast MUST NOT clear the binding. When `provider.circuit_breaker_enabled == false`, no unhealthy transition exists, so breaker-relevant failures MUST NOT clear the binding. Other upstream errors MUST NOT clear affinity by themselves. A successful fallback attempt on an affinity-enabled Channel MUST replace the binding per AFF-7c and AFF-10, so the binding always tracks the most recent successful Channel. Purpose: sub-threshold transient failures MUST NOT discard upstream prompt-cache locality.

AFF-10. A successful non-stream request MUST write or refresh affinity after success only when the successful Channel's effective `affinity_enabled` is true.

AFF-11. After a successful `type=responses` attempt on an affinity-enabled Channel, Monoize MUST write an additional affinity binding keyed by authenticated tenant, logical model, and the non-empty upstream response id. A subsequent Responses request that sends that id as `previous_response_id` MUST resolve the additional binding under AFF-7 through AFF-9. A successful Responses stream MUST write this binding only after successful terminal completion. If the successful attempt was an affinity hit and retained the same target, the response-id binding MUST inherit the source binding's `bound_at`; otherwise the response-id binding MUST set `bound_at` to the success time.

AFF-12. `MONOIZE_CHANNEL_AFFINITY_MAX_ENTRIES` MUST configure the positive process-local affinity-map limit. An unset, empty, zero, negative, or invalid value MUST use `4096`. At capacity, an existing key MUST remain refreshable and a new key MUST be rejected without eviction. Capacity checks MUST be constant-time and MUST NOT scan the map.

AFF-13. Lookup MUST inspect only the requested binding. It MUST remove that binding when expired and MUST NOT scan the complete affinity map on each request.

AFF-13a. `MONOIZE_CHANNEL_AFFINITY_CLEANUP_INTERVAL_SECONDS` MUST configure the positive background cleanup interval in seconds. An unset, empty, zero, negative, invalid, or overflowing value MUST use `60`.

AFF-13b. One background cleanup tick MUST retain only bindings whose `expires_at` is greater than the tick timestamp. Its work MUST be `O(current binding count)`, which is bounded by AFF-12. Request lookup, insertion, and refresh MUST NOT invoke this complete-map cleanup.

AFF-14. A provider update or delete MUST remove affinity bindings for every channel in the affected provider. An upstream attempt created before the completed provider mutation MUST NOT write or refresh affinity afterward.

AFF-15. A successful settings mutation that changes any global affinity setting MUST advance the routing configuration revision and clear all process-local affinity bindings before the updated settings response is returned. An upstream attempt created under the earlier revision MUST NOT recreate a cleared binding.

## 6. Health Check

### 6.1 Health State Keying

HSK-1. When `provider.per_model_circuit_break == false` (default), health state MUST be keyed by `channel_id` alone. All models sharing a channel share one health state.

HSK-1a. When `provider.circuit_breaker_enabled == false`, health state MAY still exist in memory from prior configuration, but routing MUST ignore it and passive updates MUST NOT create new unhealthy state for that provider.

HSK-2. When `provider.per_model_circuit_break == true`, health state MUST be keyed by `(channel_id, logical_model)` where `logical_model` is the request model after any API-key or global pre-redirect rule has been applied and before provider-model redirect is resolved. A circuit break for model A on channel X MUST NOT affect model B on the same channel.

HSK-3. Eligibility filtering (RTA-3) MUST use the appropriate health key when determining whether a channel is healthy for a given request model.

HSK-4. A provider update MUST remove runtime health state for every channel in both the pre-update and post-update provider. A provider delete MUST remove runtime health state for every deleted channel.

HSK-5. Each upstream attempt and active probe MUST capture the routing-configuration revision used to create it. After a provider create, update, delete, or reorder increments that revision, an attempt or probe with an older revision MUST NOT create or update channel health state.

HSK-6. `MONOIZE_CHANNEL_HEALTH_MAX_ENTRIES` MUST configure the positive process-local health-map limit. An unset, empty, zero, negative, or invalid value MUST use `10000`.

HSK-7. A health update MUST NOT increase the map beyond HSK-6. At capacity, a new key MUST NOT be inserted or evict an existing key. Every missing health key MUST be treated as ineligible until an entry slot becomes available. Capacity checks MUST be constant-time and MUST NOT scan the health map.

### 6.2 Passive

- `failure_count_threshold` default `3`
- `window_seconds` default `30`
- `cooldown_seconds` default `60`
- `rate_limit_cooldown_seconds` default `15`

PHS-1. On each breaker-relevant failure, the health state entry MUST prune failure timestamps older than `window_seconds` from the front of its queue. It MUST append `now` only when the queue length is below the effective threshold from CFG-5b.

PHS-2. A successful attempt MUST NOT append a passive sample. If `provider.circuit_breaker_enabled == false`, success and failure handling MUST return without inserting or modifying a Channel health entry.

PHS-3. A Persistent failure MUST make the health state entry unhealthy immediately. Otherwise the entry MUST become unhealthy when its failure-timestamp queue length reaches the effective threshold. Failure-count evaluation MUST read the queue length in constant time and MUST NOT scan the queue. The queue length MUST NOT exceed the effective threshold.

PHS-4. When unhealthy is triggered by HTTP `429` or an RTA-5b rate-limit signal, cooldown MUST use `rate_limit_cooldown_seconds`. Otherwise cooldown MUST use `cooldown_seconds`.

PHS-5. Unhealthy state entries MUST NOT receive normal traffic while `now < cooldown_until`.

PHS-6. On successful attempts, the health state entry MUST be restored to healthy immediately: `healthy := true`, `cooldown_until := None`, `probe_success_count := 0`, `last_probe_at := None`.

### 6.3 Active

- `enabled` default `true`
- `interval_seconds` default `30`
- `method` default `completion`
- `probe_model` default `null` (when null, use the first Channel model key)
- `success_threshold` default `1`

AHS-1. Active probing MUST target unhealthy channels whose cooldown has elapsed.

AHS-1a. If `provider.circuit_breaker_enabled == false`, active probing MUST be skipped for that provider.

AHS-1b. Active-probe candidate loading MUST exclude a disabled Provider, a disabled Channel, and a Channel whose weight is zero. A Provider with no enabled positive-weight Channel MUST be excluded.

AHS-2. Channel MUST return to healthy only after reaching success threshold.

AHS-3. When `method` is `completion`, probe MUST send a minimal completion request using the resolved probe model. Resolution order is Channel probe model override, Provider probe model override, global probe model, then the first Channel model key in lexicographic order. The upstream request uses that Channel entry's redirect when non-empty. If no Channel model can be resolved, probing for that Provider/Channel MUST be skipped.

AHS-4. The completion probe request MUST use `max_tokens: 16` and a minimal single-user-message payload to minimize cost and latency.

AHS-5. Probe results MUST be logged at debug level with channel ID, provider name, probe model, and success/failure status.

AHS-6. Probe scheduler MUST enforce provider-level probe interval independently. A channel that is probe-eligible MUST be skipped until `now - last_probe_at >= effective_interval_seconds`.

AHS-7. When `per_model_circuit_break == true`, the active-probe scheduler MUST select one probe-due unhealthy logical model and send the probe with that model's Channel mapping. A successful probe MUST increment and, after the success threshold, clear only that model's health entry. A failed probe MUST update only that model's health entry. The scheduler MUST NOT clear or extend cooldown for sibling model entries.

AHS-8. Active probe failure cooldown MUST use the effective `passive_cooldown_seconds` for the channel (channel override first, global fallback), consistent with passive breaker resolution.

AHS-9. Replicate channels MUST be skipped by active probing.

## 7. Dashboard Requirements

UI-1. Providers page MUST be provider-centric and editable without exposing `api_key` values in read responses.

UI-2. Provider list MUST support priority reordering.

UI-3. Provider editor MUST include:

- provider enable toggle
- Channel master list (name, type, base URL, weight, enabled, model count)
- selected Channel model redirect and multiplier editor
- channel runtime indicator (healthy/probing/unhealthy)
- max_retries setting
- channel_max_retries setting
- channel_retry_interval_ms setting
- circuit_breaker_enabled toggle
- per_model_circuit_break toggle

UI-4. Override fields (provider-level probe overrides, channel-level breaker overrides, timeout override) MUST display the effective global default value as placeholder text when the override is not set. Leaving a field empty MUST mean "inherit from global settings".

UI-5. Nullable boolean overrides MUST use a three-value selector (inherit / enabled / disabled). When "inherit" is selected, the selector label MUST include the effective global boolean value (e.g. "Inherit Global (Enabled)").

UI-6. Provider model fetching MUST NOT exist as a provider-level action. Each channel editor MUST expose model fetching using the channel's type, base URL, and API key.

UI-7. A Channel model fetch confirmation MUST add selected models to that Channel `models` object with `redirect = null` and `multiplier = "1"`. It MUST preserve existing entries that remain selected and MUST NOT mutate sibling Channels.
