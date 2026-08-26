# Channel/Provider Management (Dashboard) Specification

## 0. Status

- Product name: Monoize.
- Internal protocol name: `URP-Proto`.
- Scope: `/api/dashboard/providers*` APIs used by provider/channel management UI.
- Compatibility rule: this migration has no legacy API compatibility. Removed fields MUST NOT be accepted.
- Billing transition: the former Channel flags `allow_missing_usage` and
  `allow_unpriced_server_tools` are removed by migration step
  `m20260901_000048_model_prices_cutover` (`model-pricing.spec.md` §12.2). This file
  describes the post-cutover contract. Until that step ships, the stored columns and
  the pre-deprecation behavior remain in the legacy engine only.

## 1. Data Model

### 1.1 Provider

A provider object MUST include:

- `id: string` (immutable, server-generated, 8-character random string from `[a-z0-9]`)
- `name: string`
- `enabled: boolean`
- `priority: integer` (lower value means earlier routing order)
- `max_retries: integer` (default `-1`)
- `channel_max_retries: integer` (default `0`)
- `channel_retry_interval_ms: integer` (default `0`)
- `circuit_breaker_enabled: boolean` (default `true`)
- `per_model_circuit_break: boolean` (default `false`)
- `channels: Channel[]`
- `transforms: TransformRuleConfig[]` (ordered, default empty)
- `api_type_overrides: ApiTypeOverride[]` (ordered, default empty). Each entry is `{ pattern: string, api_type: enum("responses","chat_completion","messages","gemini","openai_image","replicate") }`.
- `active_probe_enabled_override?: boolean | null`
- `active_probe_interval_seconds_override?: integer | null`
- `active_probe_success_threshold_override?: integer | null`
- `active_probe_model_override?: string | null`
- `request_timeout_ms_override?: integer | null`
- `extra_fields_whitelist?: string[] | null`
- `strip_cross_protocol_nested_extra?: boolean | null`
- `allow_free_when_unpriced_override?: boolean | null` (free-settlement override, `model-pricing.spec.md` §7; `null` inherits the global setting)
- `allow_free_when_missing_usage_override?: boolean | null` (free-settlement override, `model-pricing.spec.md` §7; `null` inherits the global setting)
- `group_ids: string[]` (provider-level group ids for routing eligibility; stored non-empty, see `groups-registry.spec.md` GR-I2)
- `created_at: RFC3339`
- `updated_at: RFC3339`

CM-READ-1. Provider list, Provider detail, model-constrained routing, and active-probe candidate reads MUST return the persisted `strip_cross_protocol_nested_extra` value exactly. `true`, `false`, and `null` MUST remain distinct on every read path.

A provider object MUST NOT include `provider_type`.

### 1.2 Channel

A channel object MUST include:

- `id: string`
- `name: string`
- `provider_type: enum("responses","chat_completion","messages","gemini","openai_image","replicate")`
- `base_url: string`
- `api_key: string` (write-only: MUST NOT be returned by list/get APIs)
- `weight: integer >= 0`
- `enabled: boolean`
- `models: Record<string, { redirect: string | null, multiplier: string }>`

A channel object MUST NOT include `allow_missing_usage` or
`allow_unpriced_server_tools`. Free-settlement behavior is configured globally and per
Provider (`model-pricing.spec.md` §7). A request body containing either removed field
MUST be rejected with HTTP 400 code `invalid_request`.

Runtime projection fields MAY be returned by list/get APIs:

- `_healthy: boolean`
- `_last_success_at: RFC3339 | null`
- `_health_status: enum("healthy","probing","unhealthy")`
- `_unhealthy_models: string[]`. This list MUST be sorted ascending and MUST contain the Channel model keys whose model-specific health status is `unhealthy`. It MUST be empty when `per_model_circuit_break = false`.
- `_probing_models: string[]`. This list MUST be sorted ascending and MUST contain the Channel model keys whose model-specific health status is `probing`. It MUST be empty when `per_model_circuit_break = false`.
- `_cooldown_until: RFC3339 | null`. For a Channel-level breaker, this value is the base health entry cooldown. For a per-model breaker, this value is the latest cooldown among the Channel's currently unhealthy model entries.

`GET /api/dashboard/providers` MAY also return `model_runtime_statuses` on each Provider. Each entry MUST have this schema:

```json
{
  "model": "string",
  "availability_status": "healthy | degraded | unavailable",
  "eligible_channel_count": 0,
  "available_channel_count": 0,
  "breaker_channels": [
    {
      "channel_id": "string",
      "channel_name": "string",
      "cooldown_until": "RFC3339 | null"
    }
  ],
  "pricing_status": "complete | partial | missing",
  "unpriced_channels": [
    {
      "channel_id": "string",
      "channel_name": "string"
    }
  ]
}
```

The array MUST contain one entry for each distinct logical model key in the Provider's Channel model maps. The array and each nested Channel array MUST be sorted ascending by model or Channel name, with Channel id as the tie breaker.

For availability counts, an eligible Channel MUST be enabled, have positive weight, and contain the logical model. A Channel is unavailable only while the relevant health entry status is `unhealthy`. A `probing` Channel remains available to routing. If the circuit breaker is disabled, every eligible Channel is available. `availability_status` MUST be `unavailable` when at least one eligible Channel exists and none is available, `degraded` when some but not all eligible Channels are available, and `healthy` otherwise.

For pricing status, the denominator is every Channel model entry for the logical model, including disabled and zero-weight Channels. An entry is priced when its pricing key resolves to an applicable price under `model-pricing.spec.md` MP-R1..MP-R4. `pricing_status` MUST be `missing` when every such entry is unpriced, `partial` when only some entries are unpriced, and `complete` when none is unpriced.

Channel-level passive breaker override fields MAY be present:

- `passive_failure_count_threshold_override: integer? (>= 1)`
- `passive_window_seconds_override: integer? (>= 1)`
- `passive_cooldown_seconds_override: integer? (>= 1)`
- `passive_rate_limit_cooldown_seconds_override: integer? (>= 1)`

Channel-level active probe override fields MAY be present:

- `active_probe_enabled_override: boolean?`
- `active_probe_interval_seconds_override: integer? (>= 1)`
- `active_probe_success_threshold_override: integer? (>= 1)`
- `active_probe_model_override: string?`

Channel-level affinity override fields MAY be present:

- `affinity_enabled_override: boolean?`
- `affinity_idle_ttl_seconds_override: integer? (>= 1)`
- `affinity_failback_mode_override: enum("sticky","prefer_higher_priority")?`
- `affinity_failback_delay_seconds_override: integer? (>= 0)`

Channel egress proxy field MAY be present:

- `proxy_url: string | null`. `null`, absent, or empty means "follow global": the channel's upstream requests use the node-local proxy from `MONOIZE_UPSTREAM_PROXY_URL` (`primary-replica-deployment.spec.md` PX-series). A non-empty value is a custom absolute `http://` or `https://` proxy URL used only by this Channel's upstream requests.

Channel static upstream header map MAY be present:

- `extra_headers: Record<string, string> | null`. `null`, absent, or `{}` are equivalent and mean "no extra headers". Otherwise each entry is a static HTTP header injected into every upstream request issued for this Channel, applied after authentication and protocol-specific headers.

Channel automatic session affinity flag MAY be present:

- `session_affinity_auto: boolean | null`. `true` enables automatic session affinity. `false` disables it. `null` or absent selects the URL-based default in CM-AFF-0. When effective automatic session affinity is enabled, every proxied upstream request issued for this Channel MUST carry an `x-session-affinity` header per CM-AFF-1 through CM-AFF-2.

## 2. Invariants

CP-INV-1. `channels.length >= 1`.

CP-INV-2. At least one Channel MUST have a non-empty `models` object.

CP-INV-3. Every Channel model entry `multiplier` MUST be a base-10 decimal string with at most 9 fractional digits, MUST be representable by the server decimal type, and MUST satisfy `multiplier >= 0`. Exponent notation, a leading `+`, `NaN`, and infinity are invalid. The server MUST parse, compare, persist, and return the multiplier without converting it through `f32` or `f64`.

CP-INV-3a. Read responses MUST return each multiplier as a canonical decimal string without exponent notation or trailing fractional zeroes. The canonical representation of one is `"1"`. The canonical representation of zero is `"0"`.

CP-INV-3b. `multiplier = 0` is a valid explicit free configuration for that Channel model entry. It settles a normal billing breakdown with `final_charge_nano = 0` (`model-pricing.spec.md` MP-C12). It is not a free-settlement flag and does not bypass pricing-row requirements.

CP-INV-4. Every channel weight MUST satisfy `weight >= 0`.

CP-INV-5. Every channel `provider_type` and every `api_type_overrides[].api_type` MUST be one of `responses`, `chat_completion`, `messages`, `gemini`, `openai_image`, `replicate`.

CP-INV-6. Every `api_type_overrides[].pattern` MUST be a non-empty string.

CP-INV-7. Every returned `provider.group_ids` value MUST be trimmed, deduplicated preserving first-occurrence order, and reference existing groups at write time (`groups-registry.spec.md` GR-C1..GR-C3).

CP-INV-8. Channel model keys MUST be non-empty after trimming and unique within that Channel.

CP-INV-9. Provider MUST NOT contain a `models` field. Provider-level model selection, redirect, and multiplier state are obsolete and MUST NOT be accepted or returned.

CP-INV-10. A Channel MAY have an empty `models` object. The UI MUST warn. The Channel MUST NOT be eligible for any model route until at least one model entry exists.

CP-INV-11. Every non-null `affinity_idle_ttl_seconds_override` MUST be between `1` and `2147483647`, inclusive.

CP-INV-12. Every non-null `affinity_failback_delay_seconds_override` MUST be between `0` and `2147483647`, inclusive.

CP-INV-13. Every non-null `affinity_failback_mode_override` MUST equal `"sticky"` or `"prefer_higher_priority"`.

CP-INV-14. Every non-empty `proxy_url` MUST be an absolute URL with scheme `http` or `https`. Any other value MUST be rejected with HTTP 400 code `invalid_request`. `null`, absent, and empty are equivalent and mean follow-global.

CP-INV-15. Every non-null `extra_headers` value MUST satisfy all of the following; violations MUST be rejected with HTTP 400 code `invalid_request`:
- at most 16 entries;
- every key is trimmed non-empty, at most 128 characters, and consists only of HTTP field-name characters matching `[!#$%&'*+\-.^_\x60|~0-9A-Za-z]+`;
- keys are unique after lowercasing;
- no key equals, case-insensitively, one of `authorization`, `host`, `content-length`, `content-type`, `transfer-encoding`, `connection`, `keep-alive`, `upgrade`, `expect`, `te`, `trailer`;
- every value is at most 4096 characters and contains neither CR (`0x0D`) nor LF (`0x0A`).

CP-INV-15a. On persist, the server MUST trim keys and serialize the map as JSON with keys sorted ascending by byte order. An entry whose trimmed key is empty MUST cause rejection under CP-INV-15. Values MUST NOT be trimmed.

CM-AFF-0. A **direct Cloudflare Workers AI Channel** has a `base_url` that satisfies all of these conditions:
1. The URL parses successfully.
2. The URL scheme is `https`.
3. The URL host is exactly `api.cloudflare.com`.
4. After removal of one optional trailing slash, the URL path is exactly `/client/v4/accounts/{account_id}/ai` or `/client/v4/accounts/{account_id}/ai/v1`, where `{account_id}` is non-empty.
5. The URL has no query and no fragment.

The Channel's effective automatic session affinity MUST use this order:
1. If `session_affinity_auto` is `true`, enable it.
2. If `session_affinity_auto` is `false`, disable it.
3. If `session_affinity_auto` is `null` or absent, enable it only for a direct Cloudflare Workers AI Channel.

CM-HDR-1. Every upstream request issued for a Channel (proxy traffic and the liveness probe of §3.8) MUST send the Channel's persisted `extra_headers` entries in addition to the authentication and protocol-specific headers. When an entry name collides with an authentication or protocol-specific header, the request MUST be rejected at configuration time by CP-INV-15 rather than silently overridden at runtime.

CM-USAGE-1. Missing-usage settlement and unpriced-model settlement are governed by
`model-pricing.spec.md` §7 through the global settings `allow_free_when_missing_usage`
and `allow_free_when_unpriced` plus the Provider-level overrides. Channels carry no
usage or tool-pricing billing flags.

CM-USAGE-2. When normalized upstream usage is present, no free-settlement flag changes
that usage or the resulting charge (`model-pricing.spec.md` MP-F4).

CM-USAGE-3. Server-native tool billing is fail-open per `model-pricing.spec.md` MP-T8:
an actually used usage class without a usable `tool_prices` entry settles with zero
tool charge and is recorded in breakdown `unpriced_tool_classes`.

CM-AFF-1. If the Channel `extra_headers` contains an explicit `x-session-affinity` entry, that value MUST be sent verbatim and client passthrough (CM-AFF-1a), request-body identifiers (CM-AFF-1b), and automatic derivation (CM-AFF-2) MUST NOT run.

CM-AFF-1a. When effective automatic session affinity is enabled and CM-AFF-1 does not apply, and the incoming client request carries a session-affinity-style header, the gateway MUST pass that client value through as the upstream `x-session-affinity` header. The client headers are read in this order and the first present, non-empty one wins:
1. `session_id` (codex-style header; matched case-insensitively);
2. `session-id` (hyphenated alias; nginx default configs drop underscore header names);
3. `x-session-id`;
4. `x-session-affinity` (the header itself, sent by clients that already compute affinity).
The value MUST be trimmed, restricted to printable ASCII characters (`0x20..=0x7E`), and truncated to 128 characters. If nothing remains after restriction, the header is treated as absent and the rules below continue.

CM-AFF-1b. When effective automatic session affinity is enabled and neither CM-AFF-1 nor CM-AFF-1a produced a value, the gateway MUST use a stable conversation identifier from the decoded request. Scan the following sources in order and take the first present, non-empty value after the same printable-ASCII restriction and 128-character truncation as CM-AFF-1a:
1. `extra_body` keys `session_id`, `session`, `conversation_id`, `conversation`, `thread_id`, `thread`;
2. `extra_body.metadata` with the same keys as (1);
3. `extra_body.user_id`, then `extra_body.metadata.user_id`;
4. `req.user`.
A source value MAY be a JSON string or integer; other JSON types are skipped. The raw identifier string is used (not a `key:value` encoding) so a header `session_id` and a body `session_id` with the same uuid produce the same upstream header. `previous_response_id` MUST NOT be used: that field changes every Responses turn and would split one conversation across instances.

CM-AFF-2. When effective automatic session affinity is enabled and none of CM-AFF-1, CM-AFF-1a, or CM-AFF-1b produced a value, the gateway MUST derive the header value for each proxied request as follows:
1. If the upstream body contains a non-empty string `prompt_cache_key`, the value MUST be that string trimmed, restricted to printable ASCII characters (`0x20..=0x7E`), and truncated to 128 characters. If nothing remains after restriction, fall through to rule 2.
2. Otherwise the value MUST be `"mono-"` followed by the first 16 lowercase hex characters of the SHA-256 digest of the canonical JSON serialization (`serde_json`, key-sorted) of the object:
   - `instructions`: the decoded request `extra_body.instructions` value when present, else null;
   - `head`: an array of at most the first 2 decoded input nodes, each replaced by a canonical identity object that includes the node's kind, role when the node has a role, and content identity (text content; image/audio/file source; tool-call name and arguments; tool-result content). The canonical object MUST omit `id`, `extra_body`, `cache_control`, and any tool-definition array.
The derivation MUST NOT include `tools` or `functions`. It MUST be a pure function of `instructions` plus the first two input nodes: identical heads yield identical values; appending further input nodes MUST NOT change the value; adding, removing, or reordering tool definitions MUST NOT change the value.
When a decoded request is not available, the gateway MUST apply the same hash to the encoded upstream body using `instructions` (string or absent), `system` (when present), and the first at most 2 entries of `messages` else `input`, and MUST still omit `tools` and `functions`.

CM-AFF-3. Automatic session affinity applies only to proxied traffic. Liveness probes (§3.8) MUST NOT send derived values or client passthrough values; explicit static `extra_headers` entries continue to apply there under CM-HDR-1.

CM-AFF-4. When effective automatic session affinity is enabled and any of CM-AFF-1, CM-AFF-1a, CM-AFF-1b, or CM-AFF-2 produced a value for a proxied request, the gateway MUST record that exact value in `request_logs.session_affinity_value` on the request's terminal log row (`request-logs.spec.md` §1.1). Requests whose effective automatic session affinity is disabled, liveness probes, and requests that produced no value MUST store null.

Provider group routing semantics:

- A provider is eligible for a request when `provider.group_ids` overlaps the request's `effective_groups` (`database-provider-routing.spec.md` R-GRP-1).
- On create/update, the server MUST canonicalize and validate `group_ids` per `groups-registry.spec.md` GR-C1..GR-C3, and MUST store a canonicalized empty array as `[default_group_id]` (GR-I2).

## 3. Endpoints

All endpoints require an authenticated dashboard admin session.

### 3.1 List providers

- Method/Path: `GET /api/dashboard/providers`
- Response: `Provider[]`, ordered by `priority ASC`

### 3.2 Get provider

- Method/Path: `GET /api/dashboard/providers/{provider_id}`
- Response: `Provider`
- Errors: `404 not_found`

### 3.3 Create provider

- Method/Path: `POST /api/dashboard/providers`
- Body:
  - `name: string`
  - `enabled?: boolean`
  - `priority?: integer`
  - `max_retries?: integer`
  - `channel_max_retries?: integer`
  - `channel_retry_interval_ms?: integer`
  - `circuit_breaker_enabled?: boolean`
  - `per_model_circuit_break?: boolean`
  - `channels: Array<{ id?: string, name: string, provider_type: ProviderType, base_url: string, api_key: string, weight?: number, enabled?: boolean, models: Record<string, { redirect: string | null, multiplier: string }>, passive_failure_count_threshold_override?: integer | null, passive_window_seconds_override?: integer | null, passive_cooldown_seconds_override?: integer | null, passive_rate_limit_cooldown_seconds_override?: integer | null, active_probe_enabled_override?: boolean | null, active_probe_interval_seconds_override?: integer | null, active_probe_success_threshold_override?: integer | null, active_probe_model_override?: string | null, affinity_enabled_override?: boolean | null, affinity_idle_ttl_seconds_override?: integer | null, affinity_failback_mode_override?: "sticky" | "prefer_higher_priority" | null, affinity_failback_delay_seconds_override?: integer | null, extra_headers?: Record<string, string> | null, session_affinity_auto?: boolean | null }>`
  - `group_ids?: string[]`
  - `api_type_overrides?: ApiTypeOverride[]`
  - `strip_cross_protocol_nested_extra?: boolean | null`
  - `allow_free_when_unpriced_override?: boolean | null`
  - `allow_free_when_missing_usage_override?: boolean | null`
- Response: `201` + created provider
- Errors: `400 invalid_request` when invariants fail

### 3.4 Update provider

- Method/Path: `PUT /api/dashboard/providers/{provider_id}`
- Body: same schema as create except all fields are optional and `provider_type` is forbidden at provider level.
- `id` MUST NOT be accepted in the update body.
- `channels` is a full replacement when present. Each Channel `models` object is a full replacement for that Channel.
- `models` at Provider level is an unknown field and MUST be rejected.
- Channel `api_key` behavior:
  - If `api_key` is omitted or empty for an existing channel id, preserve the stored key.
  - If `api_key` is omitted or empty for a new channel id, reject with `400 invalid_request`.
  - If `api_key` is provided and non-empty, replace the stored key.
- Response: updated provider
- Errors: `404 not_found`, `400 invalid_request`

CP-UPD-1. After update, runtime health and affinity entries for every channel id in the pre-update or post-update provider MUST be removed.

CP-UPD-2. After update completes, in-flight work created from an older provider configuration MUST NOT recreate the removed runtime health or affinity entries.

### 3.5 Delete provider

- Method/Path: `DELETE /api/dashboard/providers/{provider_id}`
- Response: `{ "success": true }`
- Errors: `404 not_found`

CP-DEL-1. After delete, runtime health and affinity entries for all deleted provider channel ids MUST be removed.

CP-DEL-2. After delete completes, in-flight work created before deletion MUST NOT recreate runtime health or affinity entries for a deleted channel.

### 3.6 Reorder providers

- Method/Path: `POST /api/dashboard/providers/reorder`
- Body: `{ "provider_ids": string[] }`
- Semantics: provider at index `i` MUST be assigned priority `i`
- Response: `{ "success": true }`
- Errors:
  - `400 invalid_request` if array is empty
  - `400 invalid_request` if ids are duplicated or missing existing providers

### 3.7 Fetch channel models

- Method/Path: `POST /api/dashboard/fetch-channel-models`
- Body: `{ "provider_type": ProviderType, "base_url": string, "api_key"?: string, "provider_id"?: string, "channel_id"?: string }`
- Semantics:
  - If `api_key` is present and non-empty after trimming, the request MUST use that key.
  - If `api_key` is omitted or empty, `provider_id` and `channel_id` MUST both be present.
  - If `api_key` is omitted or empty and `provider_id` plus `channel_id` identify an existing Channel, the request MUST use the stored Channel `api_key`.
  - If `api_key` is omitted or empty and no stored Channel key can be resolved, return `400 invalid_input`.
  - The request body `provider_type` and `base_url` are the source of truth for the upstream request. They MAY differ from the stored Channel values when the editor has unsaved changes.
  - For `responses`, `chat_completion`, `messages`, `openai_image`, and `replicate`, call `GET {base}/v1/models` with bearer authentication.
  - For `gemini`, call Gemini list models with `x-goog-api-key`.
  - Read both successful and non-successful upstream response bodies through the bounded discovery reader defined by RRB-UD1 through RRB-UD6 in `spec/runtime-resource-bounds.spec.md`.
  - Parse a successful upstream response as JSON only after the bounded reader returns the complete body.
  - Return unique model ids sorted ascending.
- Response: `{ "models": string[] }`
- Errors:
  - `502 upstream_discovery_response_too_large` when a declared or streamed upstream body exceeds `MONOIZE_UPSTREAM_DISCOVERY_MAX_BYTES`.
  - `502 upstream_fetch_failed` when the upstream request, bounded body read, upstream status, or JSON decode fails for another reason.

### 3.8 Test channel liveness

- Method/Path: `POST /api/dashboard/providers/{provider_id}/channels/{channel_id}/test`
- Body: `{ "model"?: string, "stream"?: boolean }`
- If `stream` is omitted, it MUST default to `true`.
- If `model` is provided, it MUST be a key in the Channel `models` object.
- If `model` is omitted, use the Channel active probe model override, then Provider active probe model override, then global probe model, then the first Channel model key in lexicographic order.
- The global probe model and request timeout MUST be read from one `monoize_runtime` snapshot. This endpoint MUST NOT read the settings database.
- The upstream probe model MUST be the selected Channel model entry `redirect` when non-empty, otherwise the logical model key.
- The effective API type is the first matching Provider `api_type_overrides[]` entry for the logical model, otherwise the Channel `provider_type`.
- Replicate channels MUST be rejected for active completion probes.
- If `stream = true`, Responses, Chat Completions, Messages, and Gemini tests MUST send their protocol's streaming request form. A streaming test MUST read the upstream stream until a protocol terminal event is observed. HTTP success without a valid terminal event MUST return `success = false` and `error_code = "upstream_stream_missing_terminal"`.
- If `stream = true` for an OpenAI Image or Replicate Channel, return `400 invalid_request` because the liveness test does not define a streaming form for those Channel types.
- If `per_model_circuit_break = true`, a successful test MUST reset only the `{channel_id}::{resolved logical model}` health key. It MUST NOT reset the base Channel key or any sibling model key. If this model key does not exist and capacity remains, insert and reset this model key.
- If `per_model_circuit_break = false`, a successful test MUST reset only the base Channel health key. If this key does not exist and capacity remains, insert and reset it.
- A successful test MUST NOT inspect or mutate any other health-map key. Its health-map work MUST be `O(1)` and independent of Channel model count and global health entry count.
- Response: `{ "success": boolean, "latency_ms": integer, "model": string, "stream": boolean, "http_status": integer | null, "error_code": string | null, "error_type": string | null, "error": string | null }`
- On probe failure, `error` MUST identify the upstream outcome rather than a generic sentence:
  - HTTP non-2xx: `upstream returned {status} {reason}: {body}` where `{status}` is the
    numeric status (e.g. `500`, `503`), `{reason}` is the canonical reason phrase when
    known (e.g. `Service Unavailable`), and `{body}` is the upstream response body
    truncated to at most 512 bytes. An empty body omits the `: {body}` suffix.
  - Transport/connection failure: `connection failed: {detail}` using the underlying
    client error text.
  - Stream decode or terminal failure: a concrete sentence that identifies the invalid event, decode failure, connection failure, idle timeout, or missing terminal condition.
  - `error` MUST NOT be the undifferentiated string
    `upstream returned non-2xx status or connection failed`.

## 4. Security

CP-SEC-1. `api_key` MUST be accepted in create/update payloads.

CP-SEC-2. `api_key` MUST NOT be returned in any read response.

## 5. Dashboard Frontend Interaction

CP-FE-1. Provider card move, edit, and delete actions MUST be invokable with a single tap or click. Tooltip visibility MUST NOT be required before the action runs.

CP-FE-2. Provider card move, edit, and delete icon buttons MUST have a hit target of at least `44px` by `44px` below the `sm` breakpoint. They MAY use a smaller hit target at `sm` and wider breakpoints.

CP-FE-3. Native HTML drag-and-drop reordering of provider cards MUST be enabled only when the browser matches `(pointer: fine)`. On coarse-pointer devices, provider reordering MUST remain available through the move up and move down buttons.

CP-FE-4. While a provider child popup is open or is closing from a user action, the parent provider dialog MUST NOT treat that child popup interaction as an outside-click dismissal.

CP-FE-5. Saving a provider child editor popup MUST update the parent provider draft and close only that child popup. It MUST NOT open the parent provider unsaved-changes confirmation.

CP-FE-6. Saving from the provider unsaved-changes confirmation MUST invoke the provider create or update operation at most once for the same tap or click sequence. That same sequence MUST NOT reopen the unsaved-changes confirmation.

CP-FE-7. The null choice for `session_affinity_auto` MUST use a label that identifies URL-based automatic selection. It MUST NOT use the global-inheritance label used by unrelated nullable settings.

CP-FE-8. The Channel liveness-test dialog MUST contain a `Stream response` checkbox. The checkbox MUST be checked whenever a stream-capable Channel test dialog opens. It MUST be disabled and unchecked for a Channel type whose liveness test does not define streaming.

CP-FE-9. The Channel liveness-test dialog MUST expose adjacent `Sequential test` and `Concurrent test` buttons. Sequential test MUST await each model before starting the next model. Concurrent test MUST start one request for every Channel model without a client-side concurrency limit. Both actions MUST update each model row as its request completes.

CP-FE-10. A failed model row MUST render the concrete `error` value inline. The row MUST NOT require hover to read the error. The UI MAY truncate the initial display only when it supplies an expand action and a copy action for the complete value.

CP-FE-11. A Provider overview model badge MUST use the following severity order: red for `availability_status = unavailable`; yellow for `availability_status = degraded` or `pricing_status != complete`; default styling otherwise. Color MUST NOT be the only state signal. The badge detail overlay MUST separate breaker details from pricing details.

CP-FE-12. A Provider overview model-status detail overlay MUST open on hover or focus for a fine pointer and on tap for a coarse pointer. It MUST list every `breaker_channels` Channel name and every `unpriced_channels` Channel name. A red badge MUST state that zero of the eligible Channels are available.

CP-FE-13. A Channel `unhealthy` status detail overlay MUST identify a Channel-level breaker when `per_model_circuit_break = false`. When `per_model_circuit_break = true`, it MUST list `_unhealthy_models`. A Channel `probing` status detail overlay MUST list `_probing_models`. Each overlay MUST show `_cooldown_until` when present.

CP-FE-14. The Provider editor MUST expose `allow_free_when_unpriced_override` and `allow_free_when_missing_usage_override` as three-state selectors with options `Inherit global`, `On`, and `Off`, mapping to `null`, `true`, and `false`. Each description MUST state which global setting the `Inherit global` option follows.
