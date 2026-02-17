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
- `provider_type: enum("responses","chat_completion","messages")`
- `enabled: boolean`
- `priority: integer` (lower value means earlier routing order)
- `max_retries: integer` (default `-1`)
- `models: Record<string, { redirect: string | null, multiplier: number }>`
- `channels: Channel[]`
- `created_at: RFC3339`
- `updated_at: RFC3339`

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
- `_failure_count: integer`
- `_last_success_at: RFC3339 | null`
- `_health_status: enum("healthy","probing","unhealthy")`

## 2. Invariants

CP-INV-1. `channels.length >= 1`.

CP-INV-2. `models` MUST NOT be empty.

CP-INV-3. Every model entry multiplier MUST satisfy `multiplier > 0`.

CP-INV-4. Every channel weight MUST satisfy `weight >= 0`.

CP-INV-5. `provider_type` MUST be concrete (`responses`, `chat_completion`, or `messages`).

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
  - `provider_type: "responses" | "chat_completion" | "messages"`
  - `enabled?: boolean`
  - `priority?: integer`
  - `max_retries?: integer`
  - `models: Record<string, { redirect: string | null, multiplier: number }>`
  - `channels: Array<{ id?: string, name: string, base_url: string, api_key: string, weight?: number, enabled?: boolean }>`
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

### 3.5 Delete provider

- Method/Path: `DELETE /api/dashboard/providers/{provider_id}`
- Response: `{ "success": true }`
- Errors: `404 not_found`

### 3.6 Reorder providers

- Method/Path: `POST /api/dashboard/providers/reorder`
- Body: `{ "provider_ids": string[] }`
- Semantics: provider at index `i` MUST be assigned priority `i`
- Response: `{ "success": true }`
- Errors:
  - `400 invalid_request` if array is empty
  - `400 invalid_request` if ids are duplicated or missing existing providers

## 4. Security

CP-SEC-1. `api_key` MUST be accepted in create/update payloads.

CP-SEC-2. `api_key` MUST NOT be returned in any read response.

