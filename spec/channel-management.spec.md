# Channel/Provider Management (Dashboard) Specification

## 0. Status

- Product name: Monoize.
- Internal protocol name: `URP-Proto`.
- Scope: `/api/dashboard/providers*` APIs used by provider/channel management UI.
- Compatibility rule: this migration has no legacy API compatibility. Removed fields MUST NOT be accepted.

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
- `models: Record<string, { redirect: string | null, multiplier: number }>`
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
- `groups: string[]` (default empty; provider-level group labels for routing eligibility)
- `created_at: RFC3339`
- `updated_at: RFC3339`

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
- `supported_models: string[]` sorted ascending

Runtime projection fields MAY be returned by list/get APIs:

- `_healthy: boolean`
- `_last_success_at: RFC3339 | null`
- `_health_status: enum("healthy","probing","unhealthy")`

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

## 2. Invariants

CP-INV-1. `channels.length >= 1`.

CP-INV-2. `models` MUST NOT be empty.

CP-INV-3. Every model entry multiplier MUST satisfy `multiplier > 0`.

CP-INV-4. Every channel weight MUST satisfy `weight >= 0`.

CP-INV-5. Every channel `provider_type` and every `api_type_overrides[].api_type` MUST be one of `responses`, `chat_completion`, `messages`, `gemini`, `openai_image`, `replicate`.

CP-INV-6. Every `api_type_overrides[].pattern` MUST be a non-empty string.

CP-INV-7. Every returned `provider.groups` value MUST be lowercase, trimmed, non-empty, deduplicated, and sorted ascending.

CP-INV-8. Every `channel.supported_models[]` value MUST match a key in the same provider's `models` object.

CP-INV-9. A provider model MAY have zero supporting channels. The UI MUST warn. Routing MUST skip that provider for that model.

CP-INV-10. A channel MAY have an empty `supported_models` list. The UI MUST warn. The channel MUST NOT be eligible for any model route until at least one model is supported.

Provider group routing semantics:

- `provider.groups = []` means the provider is public for unrestricted callers and callers with `effective_groups == []`.
- If `effective_groups` is non-empty, public providers are not eligible.
- On create/update, the server MUST canonicalize `groups`.

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
  - `models: Record<string, { redirect: string | null, multiplier: number }>`
  - `channels: Array<{ id?: string, name: string, provider_type: ProviderType, base_url: string, api_key: string, weight?: number, enabled?: boolean, supported_models?: string[], passive_failure_count_threshold_override?: integer | null, passive_window_seconds_override?: integer | null, passive_cooldown_seconds_override?: integer | null, passive_rate_limit_cooldown_seconds_override?: integer | null, active_probe_enabled_override?: boolean | null, active_probe_interval_seconds_override?: integer | null, active_probe_success_threshold_override?: integer | null, active_probe_model_override?: string | null }>`
  - `groups?: string[]`
  - `api_type_overrides?: ApiTypeOverride[]`
  - `strip_cross_protocol_nested_extra?: boolean | null`
- Response: `201` + created provider
- Errors: `400 invalid_request` when invariants fail

### 3.4 Update provider

- Method/Path: `PUT /api/dashboard/providers/{provider_id}`
- Body: same schema as create except all fields are optional and `provider_type` is forbidden at provider level.
- `id` MUST NOT be accepted in the update body.
- `models` and `channels` are full replacements when present.
- Channel `api_key` behavior:
  - If `api_key` is omitted or empty for an existing channel id, preserve the stored key.
  - If `api_key` is omitted or empty for a new channel id, reject with `400 invalid_request`.
  - If `api_key` is provided and non-empty, replace the stored key.
- Response: updated provider
- Errors: `404 not_found`, `400 invalid_request`

CP-UPD-1. After update, runtime health entries for removed channel ids MUST be removed.

### 3.5 Delete provider

- Method/Path: `DELETE /api/dashboard/providers/{provider_id}`
- Response: `{ "success": true }`
- Errors: `404 not_found`

CP-DEL-1. After delete, runtime health entries for all deleted provider channel ids MUST be removed.

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
  - Return unique model ids sorted ascending.
- Response: `{ "models": string[] }`

### 3.8 Test channel liveness

- Method/Path: `POST /api/dashboard/providers/{provider_id}/channels/{channel_id}/test`
- Body: `{ "model"?: string }`
- If `model` is provided, it MUST be in the channel's `supported_models`.
- If `model` is omitted, use the channel active probe model override, then provider active probe model override, then global probe model, then the first channel-supported provider model in lexicographic order.
- The effective API type is the first matching Provider `api_type_overrides[]` entry for the logical model, otherwise the Channel `provider_type`.
- Replicate channels MUST be rejected for active completion probes.
- On success, clear all health entries for the tested channel.
- Response: `{ "success": boolean, "latency_ms": integer, "model": string, "error": string | null }`

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
