# Channel/Provider Management (Dashboard) Specification

## 0. Status

- Product name: Monoize.
- Internal protocol name: `URP-Proto`.
- Scope: `/api/dashboard/providers*` APIs used by provider/channel management UI.

## 1. Data Model

### 1.1 Provider

A provider object MUST include:

- `id: string` (immutable, server-generated, 8-character random string from `[a-z0-9]`)
- `name: string`
- `provider_type: enum("responses","chat_completion","messages","gemini")`
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
- `api_type_overrides?: ApiTypeOverride[]` (ordered, default empty) — see §2.4 of `monoize-upstream-routing.spec.md` for resolution semantics. Each entry: `{ pattern: string, api_type: enum("responses","chat_completion","messages","gemini") }`.
- `created_at: RFC3339`
- `updated_at: RFC3339`
- `groups: string[]` (default empty; provider-level group labels for routing eligibility)

### 1.2 Channel

A channel object MUST include:

- `id: string`
- `name: string`
- `base_url: string`
- `api_key: string` (write-only: MUST NOT be returned by list/get APIs)
- `weight: integer >= 0`
- `enabled: boolean`
Runtime projection fields MAY be returned by list/get APIs:

- `_healthy: boolean`
- `_last_success_at: RFC3339 | null`
- `_health_status: enum("healthy","probing","unhealthy")`

Channel-level passive breaker override fields MAY be present:

- `passive_failure_count_threshold_override: integer? (>= 1)`
- `passive_window_seconds_override: integer? (>= 1)`
- `passive_cooldown_seconds_override: integer? (>= 1)`
- `passive_rate_limit_cooldown_seconds_override: integer? (>= 1)`

Provider group routing semantics:

- `provider.groups = []` means the provider is public (accessible to unrestricted callers and callers with `effective_groups == []`, but NOT accessible to callers with non-empty `effective_groups`).
- On create/update, the server MUST canonicalize `groups` by trimming each element, lowercasing, removing empty strings after trimming, deduplicating, and sorting ascending.
- If a stored provider row has `groups` absent, null, empty string, or serialized empty array, read APIs and routing MUST treat it as `[]` for backward compatibility.

## 2. Invariants

CP-INV-1. `channels.length >= 1`.

CP-INV-2. `models` MUST NOT be empty.

CP-INV-3. Every model entry multiplier MUST satisfy `multiplier > 0`.

CP-INV-4. Every channel weight MUST satisfy `weight >= 0`.

CP-INV-5. `provider_type` MUST be one of `responses`, `chat_completion`, `messages`, `gemini`.

CP-INV-6. If `api_type_overrides` is present, every entry's `api_type` MUST be one of `responses`, `chat_completion`, `messages`, `gemini`, and every entry's `pattern` MUST be a non-empty string.

CP-INV-7. Every returned `provider.groups` value MUST already be canonicalized: lowercase, trimmed, non-empty, deduplicated, sorted ascending.

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
  - `provider_type: "responses" | "chat_completion" | "messages" | "gemini"`
  - `enabled?: boolean`
  - `priority?: integer`
  - `max_retries?: integer`
  - `channel_max_retries?: integer`
  - `channel_retry_interval_ms?: integer`
  - `circuit_breaker_enabled?: boolean`
  - `per_model_circuit_break?: boolean`
  - `models: Record<string, { redirect: string | null, multiplier: number }>`
  - `channels: Array<{ id?: string, name: string, base_url: string, api_key: string, weight?: number, enabled?: boolean, passive_failure_count_threshold_override?: integer, passive_window_seconds_override?: integer, passive_cooldown_seconds_override?: integer, passive_rate_limit_cooldown_seconds_override?: integer }>`
  - `groups?: string[]`
  - `api_type_overrides?: Array<{ pattern: string, api_type: "responses" | "chat_completion" | "messages" | "gemini" }>`
- Response: `201` + created provider
- Errors: `400 invalid_request` when invariants fail

### 3.4 Update provider

- Method/Path: `PUT /api/dashboard/providers/{provider_id}`
- Body: same schema as create except `id` (immutable), all fields optional, full replacement for `models`/`channels` when provided
- `id` MUST NOT be accepted in the update body. The provider id is immutable after creation.
- Channel `api_key` behavior on update:
  - If `api_key` is omitted or empty string for a channel whose `id` matches an existing channel under this provider, the existing `api_key` MUST be preserved.
  - If `api_key` is omitted or empty string for a **new** channel (no matching `id` in the existing provider), the request MUST be rejected with `400 invalid_request`.
  - If `api_key` is provided and non-empty, it MUST replace the stored value.
- Response: updated provider
- Errors: `404 not_found`, `400 invalid_request`

CP-UPD-1. After a successful provider update, runtime `channel_health` entries whose channel ids are no longer present in the updated provider's `channels` set MUST be removed from the in-memory health map before the response is returned.

### 3.5 Delete provider

- Method/Path: `DELETE /api/dashboard/providers/{provider_id}`
- Response: `{ "success": true }`
- Errors: `404 not_found`

CP-DEL-1. After a successful provider deletion, runtime `channel_health` entries for all channels that belonged to the deleted provider MUST be removed from the in-memory health map before the success response is returned.

### 3.6 Reorder providers

- Method/Path: `POST /api/dashboard/providers/reorder`
- Body: `{ "provider_ids": string[] }`
- Semantics: provider at index `i` MUST be assigned priority `i`
- Response: `{ "success": true }`
- Errors:
  - `400 invalid_request` if array is empty
  - `400 invalid_request` if ids are duplicated or missing existing providers

### 3.7 Test channel liveness

- Method/Path: `POST /api/dashboard/providers/{provider_id}/channels/{channel_id}/test`
- Body (optional):
  - `model?: string` — If provided, test with this specific model. If omitted, use the provider's configured active probe model override, falling back to global probe model, falling back to the provider's first model key.
- Semantics: Sends a minimal request to the channel using the effective API type resolved for the probe model (see §2.4 of `monoize-upstream-routing.spec.md`):
  - `chat_completion`: `POST /v1/chat/completions` with `{ "model", "max_tokens": 16, "messages": [{"role":"user","content":"hi"}] }`
  - `messages`: `POST /v1/messages` with `{ "model", "max_tokens": 16, "messages": [{"role":"user","content":"hi"}] }` and header `anthropic-version: 2023-06-01`
  - `responses`: `POST /v1/responses` with `{ "model", "max_output_tokens": 16, "input": [{"type":"message","role":"user","content":[{"type":"input_text","text":"hi"}]}] }`
  - `gemini`: `POST /v1beta/models/{model}:generateContent` with `{ "contents": [{"role":"user","parts":[{"text":"hi"}]}], "generationConfig": {"maxOutputTokens": 16} }` and header `x-goog-api-key: <channel api key>`
  Measures wall-clock time from request start to response completion.
  - **Health side-effect**: If `success` is `true`, the channel's health state MUST be reset to healthy. Specifically: `healthy := true`, `cooldown_until := None`, `last_success_at := now`, `probe_success_count := 0`, `last_probe_at := None`. When `per_model_circuit_break == true`, this MUST clear ALL model-specific health entries for the tested channel. This allows manual testing to recover an unhealthy channel without waiting for the active probe cycle.
- Response: `200`
  ```json
  {
    "success": boolean,
    "latency_ms": integer,
    "model": string,
    "error": string | null
  }
  ```
  - `success`: `true` if the upstream returned a 2xx status.
  - `latency_ms`: Wall-clock milliseconds from request send to response body received.
  - `model`: The model name used for the probe.
  - `error`: If `success` is `false`, a human-readable error string. `null` otherwise.
- Errors:
  - `404 not_found` if provider or channel does not exist
  - `400 invalid_request` if provider has no models and no model is specified

## 4. Security

CP-SEC-1. `api_key` MUST be accepted in create/update payloads.

CP-SEC-2. `api_key` MUST NOT be returned in any read response.
