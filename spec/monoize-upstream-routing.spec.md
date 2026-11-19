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

Runtime-only health state MUST be maintained in memory per HSK-1 through HSK-10.

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
- Each channel MUST contain `provider_type: enum("responses","chat_completion","messages","gemini","openai_image","openrouter_image","replicate","system_one","openai_decisions")`; this value determines the channel default upstream request shape.
- Each Channel MUST contain `models: Record<string, ModelEntry>`.
- Provider MUST NOT contain a `models` field.
- `api_type_overrides: ApiTypeOverride[]` (ordered, default empty) MAY be present at provider level. Each entry is `{ pattern: string, api_type: enum("responses","chat_completion","messages","gemini","openai_image","openrouter_image","replicate","system_one","openai_decisions"), chat_dialect?: enum("openai","deepseek","openrouter","vercel","generic") }` where `chat_dialect` is present exactly on `chat_completion` entries (`channel-management.spec.md` CP-INV-17) and `pattern` uses glob syntax (`*` matches any sequence, `?` matches one character).

### 2.4 API Type Resolution

AT-1. For a given request model and selected channel, the effective API type MUST be resolved as follows:

1. Iterate `api_type_overrides` in array order.
2. For each entry, test if `pattern` matches the requested model using glob semantics (case-sensitive, anchored).
3. If a match is found, the effective API type is that entry's `api_type`. Stop.
4. If no entry matches (or `api_type_overrides` is empty), the effective API type is the selected channel's `provider_type`.

AT-2. Glob matching MUST use the same semantics as transform model filtering: `*` matches zero or more characters, `?` matches exactly one character, matching is anchored (full string).

AT-3. The effective API type determines the upstream endpoint path and request encoding for that specific request.

AT-3a. The effective Chat dialect of an attempt whose effective API type is `chat_completion` MUST be the `chat_dialect` of the entry selected in AT-1 step 3 when an entry matched. It MUST otherwise be the selected Channel's `chat_dialect` (`unified_responses_proxy.spec.md` PC0). A matching entry's value MUST take precedence over a `chat_completion` Channel's value.

AT-4. `system_one` and `openai_decisions` are decision API types. A decision endpoint MUST consider only Channels whose effective API type is a decision API type. Every other forwarding endpoint MUST consider only Channels whose effective API type is not a decision API type. Both filters MUST run before the Provider attempt limit (`decision-api.spec.md` DR-R2, DR-R3).

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

CFG-2. A Provider or Channel MAY define these nullable active-probe overrides:

- `active_probe_interval_seconds_override: integer? (>= 1)`
- `active_probe_success_threshold_override: integer? (>= 1)`
- `active_probe_model_override: string?`
- `active_probe_stream_override: boolean | null`

CFG-3. The global probe settings are:

- `monoize_active_probe_interval_seconds`: integer seconds, minimum `1`, default `30`;
- `monoize_active_probe_success_threshold`: integer count, minimum `1`, default `1`;
- `monoize_active_probe_model`: nullable string, default `null`;
- `monoize_active_probe_stream`: boolean, default `true`.

CFG-3a. For each probe setting, the effective value MUST be the first non-null value in this order: Channel override, Provider override, global setting. If the global probe model is null and neither override is set, the effective configured probe model is null. An explicit `false` stream override MUST win over lower-priority values. For completion-family probes (AHS-4), the effective stream value is the scheduler's initial stream flag. It is also the dashboard Channel test's initial flag when the request omits `stream` (CM-CH-TEST-1).

CFG-4. Active probes MUST use the Provider circuit-breaker flag and the eligibility rules in §6.3. The global `monoize_active_probe_enabled` setting and the Provider and Channel `active_probe_enabled_override` fields are removed. They MUST NOT be accepted, returned, persisted, or treated as aliases. Remove the corresponding runtime configuration, entity, decode, store, UI type, form, and localization fields. No startup default or end-to-end seed may set the removed fields.

CFG-4a. The global setting `monoize_honor_rate_limit` is boolean and defaults to `false`. A Provider or Channel MAY define `honor_rate_limit_override: boolean | null`. The effective value for a Channel MUST be the first non-null value in this order: Channel override, Provider override, global setting. An explicit `false` is a value, not an inheritance marker.

CFG-5. Passive breaker effective parameters MUST be resolved per channel with precedence:

1. channel override field (if present and non-null)
2. global passive breaker setting

The resolved parameters are `passive_failure_count_threshold`, `passive_window_seconds`, `passive_cooldown_seconds`, and `passive_rate_limit_cooldown_seconds`. The threshold is a count; the other parameters are waits in seconds. For health recovery, resolve `passive_rate_limit_cooldown_seconds` only when the Channel's effective honor-rate-limit setting is `true` (CFG-4a).

CFG-5a. `MONOIZE_CHANNEL_PASSIVE_FAILURE_SAMPLE_MAX_ENTRIES` MUST configure the positive process-local maximum passive-failure threshold per health-state entry. An unset, empty, zero, negative, invalid, or overflowing value MUST use `1024`.

CFG-5b. The effective passive-failure threshold MUST equal `min(resolved passive_failure_count_threshold, MONOIZE_CHANNEL_PASSIVE_FAILURE_SAMPLE_MAX_ENTRIES)`. The effective threshold MUST be at least `1`.

CFG-6. Each provider MAY define a timeout override field:

- `request_timeout_ms_override: integer? (>= 1)` — When set, overrides the global `request_timeout_ms` for all upstream calls made through this provider. Resolution order: provider override → global `request_timeout_ms` setting → 30000ms default.

CFG-6a. Let `T` be the resolved `request_timeout_ms` of an attempt. Upstream HTTP forwarding calls MUST apply these deadlines:

1. A call whose complete 2xx body Monoize reads as one JSON value (non-stream forwarding, including multipart image edits) MUST complete sending, response headers, and the full body within `T` ms.
2. A call whose 2xx body Monoize consumes incrementally (SSE stream decoding, non-stream collection of an upstream stream, and every image-capable dispatch) MUST receive the response headers within `H = max(10 × T, 600000)` ms. For a non-2xx response, `H` also bounds the read of the error body.
3. For a call of item 2, Monoize MUST NOT apply a total deadline to the 2xx body. The only body bound is `monoize_stream_idle_timeout_ms`: each wait for the next SSE event, WebSocket frame, or JSON body chunk MUST end with `upstream_idle_timeout` when it exceeds that value. A stream that keeps delivering data within the idle bound MAY last indefinitely.
4. A deadline failure of item 1 or item 2 is a transport failure (`upstream-error-sanitization.spec.md` SAN-D3 `transport`).

Active probes and dashboard Channel liveness tests keep one total deadline of `T` ms (AHS-3b) and are outside CFG-6a.

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

- candidate Channels MUST have `enabled == true` and `weight > 0`. `models` MUST contain the requested logical model, and its multiplier MUST NOT exceed the request `max_multiplier` when present.
- if `provider.circuit_breaker_enabled == true`, the effective health status of the requested model's key (HSK-1, HSK-2, HSK-10) MUST NOT be `unhealthy`. An elapsed probe due time MUST NOT make an unhealthy key eligible. HSK-7 still applies at capacity.
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
- before every dispatch, including intra-Channel retries, the router MUST exclude a Channel whose relevant key is effectively `unhealthy` when the Provider circuit breaker is enabled. A skipped Channel MUST NOT consume an attempt.
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
- A target-capability failure MUST end only the current attempt (RTA-5e).
- A Monoize authentication, authorization, balance, request-validation, request-encoding, transform, billing, or internal error other than RTA-5e MUST stop routing. Monoize MUST NOT classify such an error as an upstream attempt failure.

RTA-5a. A persistent Channel failure is an upstream failure that satisfies at least one condition below:

1. Its HTTP status is one of `401`, `402`, `403`, `404`, `405`, `407`, `410`, `415`, `426`, or `451`.
2. Its structured upstream `code` or `type`, after ASCII lowercase conversion and surrounding whitespace removal, is one of `account_deactivated`, `account_suspended`, `authentication_error`, `billing_hard_limit_reached`, `credit_balance_too_low`, `deployment_not_found`, `insufficient_balance`, `insufficient_quota`, `invalid_api_key`, `model_not_found`, `model_not_supported`, `no_available_account`, `permission_denied`, `quota_exceeded`, or `unsupported_model`.

A persistent Channel failure MUST NOT authorize a same-Channel retry. HTTP `400`, `409`, and `422` without a listed structured signal are request-specific and MUST NOT be persistent Channel failures.

RTA-5b. A structured upstream `code` or `type` equal to `rate_limit_error`, `rate_limit_exceeded`, or `too_many_requests` MUST have the RateLimited passive-failure class. A structured upstream `code` or `type` equal to `overloaded_error`, `server_error`, `service_unavailable`, or `temporarily_unavailable` MUST have the Transient passive-failure class. Signal comparison uses the normalization in RTA-5a. HTTP `429` MUST take the RateLimited class regardless of structured signals. An RTA-5a persistent HTTP status MUST take the Persistent class regardless of structured signals. For all other statuses, a persistent structured signal takes precedence over a Transient HTTP status or structured signal.

RTA-5c. If the effective Channel proxy cannot construct an HTTP client, Monoize MUST fail that attempt with `upstream_proxy_config_invalid`, record it in `tried_providers`, and advance to the next eligible Channel or Provider. It MUST NOT fall back to a direct connection, retry the same Channel, or record a passive-health failure sample.

RTA-5d. Forwarding attempts and active probes MUST use the same Retry-After parser. Inputs are the response headers and the wall-clock time at parsing. The parser MUST use the first applicable case below, and clamp a present result to at most `3600` seconds:

1. A non-negative integer `retry-after-ms` value gives `ceil(value / 1000)` seconds.
2. Otherwise, a non-negative integer `retry-after` value gives that number of seconds.
3. Otherwise, a valid `retry-after` HTTP-date gives `max(0, date_s - now_s)`, where `date_s` and `now_s` are whole Unix seconds for the HTTP-date and the parse time, respectively.
4. Otherwise, the result is absent.

Forwarding attempts MUST use the headers of non-stream HTTP error responses and pre-commit stream error responses whose HTTP headers are available. In-stream terminal errors have no Retry-After. Active probes MUST use the headers of their own HTTP response, if available.

RTA-5e. A target-capability failure is a rejection in `media-transport.spec.md` MT20a while preparing a selected forwarding attempt. It MUST occur before any upstream bytes are sent for that attempt. Count the failed attempt against the existing RTA-4 Provider attempt budget and record exactly one `tried_providers` entry with its existing error code and message (RL17, SAN-5). Do not retry the same Channel, including with a bound-target extra retry under RTA-4a. Do not record a passive health sample, trigger a shared-origin blast, or clear affinity. Advance to the next eligible Channel or Provider under the existing attempt limits and order. A pre-dispatch error outside MT20a MUST NOT take this path merely because it has the same status or code.

RTA-6. On an HTTP `408`, HTTP `429`, HTTP `5xx`, timeout, connection failure, RTA-5a persistent failure, or RTA-5b structured failure, Channel passive health state MUST be updated. A persistent failure MUST mark the relevant health entry unhealthy after its first occurrence. A Transient or RateLimited failure MUST use the threshold in PHS-3.

RTA-6b. Other upstream failures MUST NOT update Channel passive health state.

RTA-6a. If `provider.circuit_breaker_enabled == false`, retryable attempt failures MUST NOT trip passive health state and MUST NOT mark the channel unhealthy.

RTA-6c. Shared-origin blast. When a retryable failure's upstream HTTP status is `502`, `503`, or `524`, Monoize MUST treat the failure as a shared-origin blast for that attempt's Provider:

1. The origin key is `lowercase(scheme) + "://" + lowercase(host) + ":" + port`, where `scheme`, `host`, and `port` come from parsing the Channel `base_url`. `port` is the URL's explicit port when present, otherwise the default port for that scheme (`80` for `http`, `443` for `https`). Only `http` and `https` origins are valid. If `base_url` cannot be parsed into such an origin, that Channel has no origin key and MUST NOT share health with any other Channel.
2. Every other Channel of the same Provider that is enabled, has `weight > 0`, and has an equal origin key MUST receive the same Transient passive-failure sample as the failed Channel, using the failed attempt's resolved passive parameters and the same timestamp. Peer updates MUST use the same health keying as the failed attempt (HSK-1 / HSK-2). A peer insert that would exceed HSK-6 MUST be skipped.
3. Remaining same-Channel retries and remaining not-yet-attempted same-origin Channels of that Provider MUST be skipped for the rest of this request.
4. HTTP `408` and HTTP `429` MUST NOT trigger a shared-origin blast. They remain per-Channel.

RTA-6d. A shared-origin blast MUST classify the passive sample as Transient, not RateLimited. Its initial recovery wait MUST use the non-rate-limit branch for the affected key's recovery mode (PHS-4, PHS-5).

RTA-7. If all attempts in Provider fail before the first downstream byte, router MUST continue with the next Provider. The status code of an upstream failure MUST NOT stop cross-Provider fail-forward.

RTA-8. If all providers are exhausted before `COMMIT` (`unified_responses_proxy.spec.md` FP4e) for a forwarding endpoint, the terminal `AppError` MUST be, with `LAST` being the final failed attempt:

- status: `LAST.upstream_status` when it is in `400..=599`; otherwise `502`. An attempt that failed without an upstream HTTP status records the local status of its failure (`502` for transport and adapter failures).
- `code` and request-log `error_code`: the non-empty upstream code of `LAST`; otherwise `upstream_error`.
- `type`: the non-empty upstream type of `LAST`; otherwise derived from the status: 401 → `authentication_error`, 403 → `permission_error`, 404 → `not_found_error`, 429 → `rate_limit_error`, other `4xx` → `invalid_request_error`, other statuses → `server_error`. `POST /v1/messages` applies SE1c on top of this value.
- `param`: the upstream param of `LAST`, otherwise `null`.
- `message`: the sanitized client text of `LAST` defined by `spec/upstream-error-sanitization.spec.md` SAN-6. It MUST NOT contain upstream URLs, attempt counts, provider/channel identifiers, or the text `All upstream attempts failed`.

With zero recorded attempts the error is status `502`, code `upstream_error`, type `server_error`, and the SAN-6 zero-attempt message. Request-log `error_http_status` MUST equal the status. The body and framing are defined by `unified_responses_proxy.spec.md` SE1. If exhaustion happens after `COMMIT`, the same fields are rendered as the SE2 in-stream error.

RTA-8b. Channel credential failures. If `LAST.upstream_status` is `401`, `403`, or `407`, the failure belongs to the Channel credential or permission, not to the client. RTA-8 and RTA-8a do not apply. The terminal `AppError` MUST be status `502`, `code = "upstream_error"`, `type = "server_error"` (Messages: `api_error` through SE1c), `param = null`, and the RTA-8 `message`. The upstream code and type MUST NOT appear in `code` or `type`, because clients treat them as a login failure. The body MUST carry the diagnostics `upstream_status` (the original status) and `upstream_code` (the non-empty code reported by the upstream, omitted only when the upstream reported none; a reported value of `upstream_error` is retained). Request-log `error_code` and `error_http_status` MUST equal the downstream values; `tried_providers` keeps the original status and code. A WebSocket error frame uses the same status, code, and type. A 401 for the client's own Monoize key is a local failure and stays `401`. Every other upstream status, including 400, 404, 409, 413, 422, 429, and every `5xx`, follows RTA-8.

RTA-8a. If the final failed attempt has `upstream_code = "thinking_signature_invalid"` and its status is not a valid HTTP `4xx` status, Monoize MUST return HTTP `400` instead of the RTA-8 status. The request log `error_code` and `error_http_status` MUST equal the downstream values; the request log `error_message` MUST equal the persisted internal detail per `spec/upstream-error-sanitization.spec.md` SAN-8 and SAN-9.

RTA-8c. When routing is exhausted and the last failed attempt is a target-capability failure, return that attempt's existing local HTTP status and error code. Use its existing message as the client error under SAN-5 and SAN-6 without substituting a generic routing error. If the last failed attempt has another class, its existing exhaustion rule still applies. The terminal request log MUST use the returned status and code.

## 5. Streaming-specific Rule

STRM-1. If downstream streaming has emitted a protocol data or error event, router MUST NOT switch provider/channel for that request. An SSE comment or protocol keep-alive event emitted before the first protocol data or error event does not disable fallback.

STRM-2. Provider/channel fallback is allowed only before the first downstream protocol data or error event is emitted.

STRM-2a. Pre-output window. After an upstream streaming attempt returns HTTP `2xx`, or completes the WebSocket `response.create` send under WS2d, the attempt stays fallback-eligible until the stream decoder yields its first output event. An output event is every canonical stream event other than `ResponseStart` and `ProviderControl`. Provider-control events include `codex.rate_limits`, `codex.response.metadata`, `response.metadata`, and Messages `ping`. Monoize MUST buffer `ResponseStart` and `ProviderControl` and MUST NOT emit any downstream frame derived from them (for example Responses `response.created` and `response.in_progress`) before the window closes. Buffered events of a failed attempt MUST be discarded. When the window closes, Monoize MUST emit the buffered events before the first output event, in decoder order. The decoder idle timeout (`monoize_stream_idle_timeout_ms`) bounds the window. The buffer holds at most 256 events. A 257th non-output event MUST NOT close the window and MUST NOT count as output: the attempt ends as a pre-output failure with code `upstream_control_overflow` and the Transient class, its buffered events are discarded, and STRM-2b applies.

STRM-2b. If the decoder yields a terminal `Error` event, or ends without an output event (EOF, `[DONE]`, idle timeout, decode failure, or WebSocket close), while the window is open, Monoize MUST end that attempt as an upstream attempt failure. Monoize MUST record one `tried_providers` entry for the attempt, MUST record the capture attempt dump with the error under `request-capture-dumps.spec.md`, and MUST apply the RTA-4, RTA-4a, and RTA-5 retry rules. The passive-failure class comes from the STRM-4 classification of the error code, error type, and synthesized HTTP status. Adapter failures whose code starts with `upstream_` have the Transient class. The synthesized status MUST NOT trigger an RTA-6c shared-origin blast. If no eligible attempt remains, RTA-8 applies: before `COMMIT` Monoize returns the native JSON error (FP4e, SE1), and after `COMMIT` the downstream stream ends with the SE2 error event. A frame that carries a `status` in `400..=599` sets the downstream status. That status is also the attempt's recorded `tried_providers.upstream_status`, so an explicit `403` or `407` stays `403` or `407`. A frame without one uses the decoder's synthesized status, except that the code or type `authentication_error`, `invalid_api_key`, `permission_error`, or `permission_denied` yields `401` (RTA-8b applies), and a synthesized `400` becomes `429` for `rate_limit_exceeded` or `rate_limit_error` and `500` for `server_error`, `api_error`, or `overloaded_error`. The status of a frame never sets `AppError.upstream_status` and never triggers an RTA-6c shared-origin blast.

STRM-2c. A `ResponseDone` event that arrives as the first output event closes the window and commits the attempt. After the window closes, STRM-1 and STRM-4 govern the stream.

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

HSK-5. Each upstream attempt and active probe MUST capture the routing configuration revision used to create it. After the revision advances, an attempt or probe with an older revision MUST NOT create or update Channel health state. This includes a Provider create, update, delete, or reorder, a Channel enable change (CM-CH-EN-1), and a health-setting mutation (R-H-9).

HSK-6. `MONOIZE_CHANNEL_HEALTH_MAX_ENTRIES` MUST configure the positive process-local health-map limit. An unset, empty, zero, negative, or invalid value MUST use `10000`.

HSK-7. A health update MUST NOT increase the map beyond HSK-6. At capacity, a new key MUST NOT be inserted or evict an existing key. Every missing health key MUST be treated as ineligible until an entry slot becomes available. Capacity checks MUST be constant-time and MUST NOT scan the health map.

HSK-8. The only Channel health display statuses are `healthy`, `unhealthy`, and `disabled`. Display status MUST be `disabled` if and only if `Channel.enabled = false`; a runtime health key itself is never `disabled`. Otherwise the display status MUST reflect effective runtime health. `no_traffic` is a separate Provider-page badge, not a health status (PL18b). The former `probing` status is removed from runtime state, API enums, UI, and documentation.

HSK-9. Each runtime health key MUST store the following state. Timestamps are wall-clock instants expressed in whole Unix seconds. Waits in the recovery formulas are also in whole seconds.

- `healthy: boolean`, initially `true`;
- `last_success_at: timestamp | null`, initially null, the most recent successful attempt or test that made or kept the key healthy;
- `passive_failure_timestamps: sequence<timestamp>`, initially empty, the queue maintained by PHS-1;
- `probe_success_count: integer`, initially `0`, the number of consecutive successful active probes for the current trip;
- `last_probe_at: timestamp | null`, initially null, the time of the most recent applied probe result for the current trip;
- `trip_id: u64 | null`, initially null, the unique identity of the current trip. Assign each new trip a value from one process-local, monotonically increasing counter. A trip id MUST NOT be reused within the process, including after a health-map clear. A healthy key has no current trip id;
- `trip_time: timestamp | null`, initially null, the wall-clock time of the current healthy-to-unhealthy transition, denoted by `t`;
- `trip_rate_limited: boolean`, initially `false`, whether the failure that caused the current trip was RateLimited (`rl`);
- `trip_retry_after_seconds: integer | null`, initially null, the parsed Retry-After of that failure (`ra`, `0..=3600` seconds, RTA-5d);
- `recovery_mode: enum("probe","timed") | null`, initially null, the mode selected for the current trip (PHS-4);
- `next_recovery_at: timestamp | null`, initially null, the next probe due time in `probe` mode or the time when the key becomes healthy in `timed` mode.

HSK-10. An absent key has status `healthy` for status reads, subject to the separate routing capacity check in HSK-7. For an existing key, its effective status at read time is `unhealthy` exactly when `healthy = false` and either `recovery_mode = probe` or read time is before `next_recovery_at` in `timed` mode. The effective status is `healthy` otherwise. An elapsed timed trip is no longer current, even if the expired trip data has not yet been cleared from memory. When the Provider circuit breaker is off, routing and Channel availability ignore these runtime values (RTA-3, CM-HEALTH-2).

### 6.2 Passive

- `failure_count_threshold` default `3`
- `window_seconds` default `30`
- `cooldown_seconds` default `60`
- `rate_limit_cooldown_seconds` default `15`

PHS-1. On each breaker-relevant failure, the health state entry MUST prune failure timestamps older than `window_seconds` from the front of its queue. It MUST append `now` only when the queue length is below the effective threshold from CFG-5b.

PHS-2. A successful attempt MUST NOT append a passive sample. If `provider.circuit_breaker_enabled == false`, success and failure handling MUST return without inserting or modifying a Channel health entry.

PHS-3. Only the passive failure rules may transition a key from healthy to unhealthy. On a Persistent failure, a currently healthy key MUST become unhealthy immediately. On a Transient or RateLimited failure, a currently healthy key MUST become unhealthy when its failure-timestamp queue length reaches the effective threshold. Evaluate current health per HSK-10. Failure-count evaluation MUST read the queue length in constant time and MUST NOT scan the queue. The queue length MUST NOT exceed the effective threshold.

PHS-4. At a PHS-3 transition the router MUST write `healthy = false`, allocate a new `trip_id` (HSK-9), record `t`, `rl`, and `ra`, reset `probe_success_count` to `0` and `last_probe_at` to null, and set the mode and `next_recovery_at`. The mode MUST be `probe` only on the primary and only for a Channel whose `provider_type != replicate`. The mode MUST be `timed` on replicas and for `replicate` Channels on either node. Use Channel `provider_type`, not a model API type override. Set `rl = true` exactly when the triggering failure is RateLimited (HTTP `429` or an RTA-5b rate-limit signal under its classification rules), and set `ra` to the RTA-5d result. A failure while the key is effectively unhealthy MUST NOT replace the recorded trip or its recovery time.

PHS-5. Let `honor` be the Channel's effective value from CFG-4a, and `interval` its effective interval from CFG-3a in seconds. If `honor && rl`, the initial wait in either mode MUST be `max(passive_rate_limit_cooldown_seconds, ra)` with absent `ra` treated as `0`. Otherwise the initial wait MUST be `interval` in `probe` mode or `passive_cooldown_seconds` in `timed` mode. Resolve the two passive waits per CFG-5 (RTA-6c for shared-origin samples). The runtime MUST NOT consult `passive_rate_limit_cooldown_seconds` for recovery when `honor = false`. Set `next_recovery_at = t + initial_wait`, adding the wait in seconds. For `timed` mode, routing and every status read MUST treat the key as healthy at and after that time without waiting for a background task. For `probe` mode, reaching that time only makes the key due for a probe (AHS-1).

PHS-6. A successful forwarding attempt subject to PHS-2 and HSK-5 MUST immediately make the affected key healthy. Set `healthy = true`, `last_success_at = now`, `probe_success_count = 0`, `trip_rate_limited = false`, and set `last_probe_at`, `trip_id`, `trip_time`, `trip_retry_after_seconds`, `recovery_mode`, and `next_recovery_at` to null. A successful dashboard test uses the same reset for the key selected in CM-CH-TEST-1. A timed recovery does not record a successful attempt or probe.

PHS-7. For each health write from a forwarding attempt, active probe, or dashboard test, `now` MUST be the wall-clock time when applying that update. It MUST NOT be a timestamp captured at dispatch or at the beginning of a scheduler tick. Use one write-time value per failed forwarding attempt, including its shared-origin samples under RTA-6c.

### 6.3 Active

AHS-1. A health key is due exactly when its effective health is `unhealthy`, its `recovery_mode = probe`, and `now >= next_recovery_at`. Each tick MUST schedule one probe for each eligible due key, subject to AHS-1a, AHS-1b, AHS-3, AHS-9, AHS-11, and `request-logs.spec.md` RL3c and RL18e through RL18g. If the health map contains no unhealthy entry in `probe` mode, the tick MUST perform no database query.

AHS-1a. The scheduler MUST NOT probe any Channel whose `provider.circuit_breaker_enabled = false`. Every Provider with `circuit_breaker_enabled = true` is subject to AHS-1, AHS-1b, and AHS-9 with no separate enable switch.

AHS-1b. Active-probe candidate loading MUST exclude a disabled Provider, a disabled Channel, and a Channel whose weight is zero. A Provider with no enabled positive-weight Channel MUST be excluded.

AHS-2. On an accepted successful probe (AHS-10), increment the key's consecutive success count by `1`. If it reaches the effective success threshold (CFG-3a), reset the key per PHS-6 without changing the passive failure queue. Otherwise the key MUST remain unhealthy; set `last_probe_at = now` and `next_recovery_at = now + interval`, with `interval` as in PHS-5. The previously recorded trip MUST NOT change on a below-threshold success.

AHS-3. For a Channel-level breaker, resolve the configured probe model per CFG-3a. Select it if it is a key of the Channel's `models`; otherwise select the lexicographically smallest Channel model key. An invalid higher-precedence value falls back to that smallest key, not a lower-precedence configured value. For a per-model breaker, select each due key's own logical model. Skip a probe if there is no selected model key in `models`. A dashboard Channel test without an explicit model MUST use the Channel-level selection rule even when `per_model_circuit_break = true`.

AHS-3a. The scheduler and dashboard Channel test MUST resolve API type and Chat dialect from the selected logical model (AT-1 and AT-3a). The initial upstream model MUST be the selected entry's non-empty `redirect`, or the logical model when the redirect is empty or null. Neither a redirect nor a transform's change to the outbound model may change the selected API type, Chat dialect, or the logical model used for transform matching.

AHS-3b. The scheduler and dashboard Channel test MUST use Provider `request_timeout_ms_override` when non-null, else the global `request_timeout_ms` (CFG-6). This is one total timeout in milliseconds for the entire probe request.

AHS-4. For a probe whose effective API type is `responses`, `chat_completion`, `messages`, or `gemini`, the scheduler and dashboard Channel test MUST build one `UrpRequestV2` with a single user text message `hi`, maximum output tokens `16`, and the initial stream flag from CFG-3a or CM-CH-TEST-1. No downstream request, Monoize authentication context, or API-key transform chain participates. They MUST process the probe in this order:

1. Apply the forwarding attempt shaping that does not require a downstream request (`urp-transform-system.spec.md` PIPE-1 steps 4–7). Set the request model per AHS-3a. Use the same cross-family nested-extra strip setting and precedence as forwarding. Do not synthesize downstream passthrough fields or client authentication context.
2. Apply the Provider request-phase transforms, then the global request-phase transforms, in rule order within each scope. Match rules against the selected logical model and supply the selected effective API type as transform context (PIPE-1c, TF-3, TF-4, TF-10, TF-11). Transforms MAY change the URP request, including its maximum output tokens and stream flag, as for forwarding. Do not restore the initial token limit or stream flag after transforms.
3. Encode the transformed URP request with the forwarding encoder for the effective API type and Chat dialect. Apply the forwarding extra-fields whitelist after transforms and before protocol-specific encoding (`unified_responses_proxy.spec.md` XF6e). Use the resulting upstream request mode for dispatch and decoding.
4. Use the forwarding HTTP request construction for the encoded request, including endpoint and URL joining, upstream authentication, Channel extra headers, OpenCode probe session headers, and Channel proxy (CM-HDR-1, CM-AFF-3, PX6). Always use HTTP, never WebSocket (`unified_responses_proxy.spec.md` WS2f).

Do not apply response-phase transforms. Do not keep separately constructed completion request bodies or a separate raw SSE reader for these API types.

AHS-4a. For the completion-family probes in AHS-4, report success under exactly the applicable condition below, subject to the one total timeout in AHS-3b:

- Streaming: the HTTP status is 2xx and the same URP stream decoder used by forwarding (`unified_responses_proxy.spec.md` FP5 through FP5c) reaches its terminal response event without emitting an error event. An HTTP 2xx status without a successful terminal decode is not sufficient.
- Non-streaming: the HTTP status is 2xx and the same URP response decoder used by forwarding accepts the body.

A request-phase transform or encoding error MUST send no upstream bytes and MUST be logged at `warn` level with the error. It counts as a failed probe with `rl' = false` and no Retry-After, so a scheduler result accepted under AHS-10 uses the `interval` branch of AHS-8. Classify upstream failures under AHS-8 and RTA-5d.

AHS-4b. When the effective API type is `openai_image`, `openrouter_image`, `system_one`, or `openai_decisions`, use the native minimal non-streaming request instead of AHS-4 and AHS-4a. Do not apply transforms or consult the effective stream setting for these types. The native request shapes are specified by OIU-RT3, ORI-11, DR-C2, and CM-CH-TEST-1. Replicate Channels stay in timed recovery (AHS-9).

AHS-5. Probe results MUST be logged at debug level with channel ID, provider name, probe model, and success/failure status.

AHS-6. *(Removed — `next_recovery_at` is the only probe due-time gate. See AHS-1.)*

AHS-7. Each due health key for a Channel with `per_model_circuit_break = true` MUST be evaluated separately. Each result MUST affect only the health key that the probe targeted. It MUST NOT modify the base Channel key or any sibling model key.

AHS-8. On an accepted failed probe (AHS-10), the key MUST remain unhealthy. Set `probe_success_count = 0` and `last_probe_at = now`. If `honor` is true (CFG-4a) and the probe response is RateLimited (HTTP `429` or an RTA-5b rate-limit signal in its body, including a decoded upstream error event), set `next_recovery_at = now + max(passive_rate_limit_cooldown_seconds, ra')`, where `ra'` is the RTA-5d result of that probe, or `0` when absent. In every other case set `next_recovery_at = now + interval` (PHS-5). Use neither the original trip's Retry-After nor the timed recovery wait for a probe failure. The recorded trip MUST NOT change.

AHS-9. The scheduler MUST run only on a primary (PRP11). It MUST skip every Channel whose `provider_type = replicate` (PHS-4).

AHS-10. Each dispatch MUST capture the health key, its `trip_id`, and the routing configuration revision. On completion, apply the result only if the current revision matches the captured revision and the same key still exists, is effectively unhealthy, and has the captured `trip_id`. Do not use `trip_time` as the identity: two trips in the same second MUST have different ids. The scheduler MUST test this guard and apply the health write as one operation relative to other updates. A discarded result MUST NOT update, create, or recreate a health entry.

AHS-11. A tick MUST have at most `8` probes in flight. No two probes may be in flight for the same health key. A tick MUST wait for all of its probes to finish before the next tick starts. Shutdown dispatch and drain rules remain RL3c and RL18e through RL18g in `request-logs.spec.md`.

AHS-12. A tick's pricing snapshot MUST cover every logical model probed in that tick and include existing candidate price rows for the redirected and logical model keys used by `request-logs.spec.md` RL18a. For a completion-family probe, take usage for RL18 from the decoded URP response or the stream decoder's terminal usage, not from a separate interpretation of the native body or event frames. Logging and settlement MUST follow RL18 through RL18g, including when AHS-10 discards a health update.

## 7. Dashboard Requirements

UI-1. Providers page MUST be provider-centric and editable without exposing `api_key` values in read responses.

UI-2. Provider list MUST support priority reordering.

UI-3. Provider editor MUST include:

- provider enable toggle
- Channel master list (name, type, base URL, weight, enabled, model count)
- selected Channel model redirect and multiplier editor
- Channel runtime indicator (`healthy`, `unhealthy`, or `disabled`); show the separate `no_traffic` badge per PL18b of `dashboard-ui-layout.spec.md`
- max_retries setting
- channel_max_retries setting
- channel_retry_interval_ms setting
- circuit_breaker_enabled toggle
- per_model_circuit_break toggle

UI-4. An unset probe, breaker, or timeout override field MUST display the value inherited under CFG-3a, CFG-4a, CFG-5, or CFG-6, as applicable. An empty field means inherit.

UI-5. Nullable boolean overrides MUST use a three-value selector (inherit / on / off). The inherit label MUST identify the source and inherited value, such as "Inherit Global (On)" or "Inherit Provider (Off)".

UI-6. Provider model fetching MUST NOT exist as a provider-level action. Each channel editor MUST expose model fetching using the channel's type, base URL, and API key.

UI-7. A Channel model fetch confirmation MUST add selected models to that Channel `models` object with `redirect = null` and `multiplier = "1"`. It MUST preserve existing entries that remain selected and MUST NOT mutate sibling Channels.
