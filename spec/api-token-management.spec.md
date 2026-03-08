# Dashboard API Token (API Key) Management Specification

## 0. Status

- **Purpose:** Define dashboard-managed API keys used to authenticate forwarding endpoints.
- **Scope:** Applies to `/api/dashboard/tokens*` endpoints.

## 1. Data model

### 1.1 API key

An API key row has:

- `id: string`
- `user_id: string`
- `name: string`
- `key_prefix: string` (first 12 characters of the full key)
- `key_hash: string` (reserved; currently not used for runtime validation)
- `key: string` (the full key, stored for display)
- `created_at: RFC3339 string`
- `expires_at: RFC3339 string?`
- `last_used_at: RFC3339 string?`
- `enabled: boolean`
- `quota_remaining: integer?`
- `quota_unlimited: boolean`
- `model_limits_enabled: boolean`
- `model_limits: string[]`
- `ip_whitelist: string[]`
- `group: string`
- `max_multiplier: number?`
- `transforms: TransformRuleConfig[]`

## 2. Endpoints

All endpoints in this spec require an authenticated dashboard session.

### 2.1 List my API keys

- **Endpoint:** `GET /api/dashboard/tokens`
- **Authorization:** Any authenticated user.
- **Response:** `APIKey[]` for the current user.

### 2.2 Get API key

- **Endpoint:** `GET /api/dashboard/tokens/{key_id}`
- **Authorization:** Any authenticated user, but only for keys owned by that user.
- **Errors:** `404 not_found` if the key does not exist or is not owned by the user.

### 2.3 Create API key

- **Endpoint:** `POST /api/dashboard/tokens`
- **Authorization:** Any authenticated user.
- **Request body:** fields:
  - `name: string`
  - `expires_in_days: integer?`
  - `quota: integer?`
  - `quota_unlimited: boolean` (default true)
  - `model_limits_enabled: boolean` (default false)
  - `model_limits: string[]` (default empty)
  - `ip_whitelist: string[]` (default empty)
  - `group: string` (default `"default"`)
  - `max_multiplier: number?` (default null)
  - `transforms: TransformRuleConfig[]` (default empty)
- **Response:** The created key object including the full key string.

TM-CREATE-1. The generated full key MUST start with the literal prefix `sk-`.

TM-CREATE-2. The server MUST compute `key_prefix` as the first 12 characters of the full key.

TM-CREATE-3. The server MUST persist `key_hash` as a reserved compatibility field. Runtime token validation semantics are defined in `api-key-authentication.spec.md`.

TM-CREATE-4. After successful key creation, there is no required cache invalidation side-effect because the new key does not exist in cache yet.

### 2.4 Update API key

- **Endpoint:** `PUT /api/dashboard/tokens/{key_id}`
- **Authorization:** Any authenticated user, but only for keys owned by that user.
- **Request body:** partial update with optional fields:
  - `name`
  - `enabled`
  - `quota`
  - `quota_unlimited`
  - `model_limits_enabled`
  - `model_limits`
  - `ip_whitelist`
  - `group`
  - `max_multiplier`
  - `transforms`
  - `expires_at` (RFC3339 string or null)
- **Errors:** `404 not_found` if the key does not exist or is not owned by the user.

TM-UPD-1. A successful API key update MUST invalidate in-memory API key cache entries for the updated key id before returning the response.

### 2.4a API-key transform safety boundary

TM-TF-1. API key `transforms` are user-scoped request/response shaping rules. They MUST NOT act as routing, pricing, or upstream service-tier controls.

TM-TF-2. The server MUST reject API key create/update requests whose `transforms` array contains any rule outside the allowed API-key transform subset defined by TM-TF-3 and TM-TF-4.

TM-TF-3. Allowed API-key request-phase transforms are exactly:

- `inject_system_prompt`
- `system_to_developer_role`
- `merge_consecutive_roles`
- `append_empty_user_message`
- `compress_user_message_images`
- `auto_cache_system`
- `auto_cache_tool_use`
- `auto_cache_user_id`

TM-TF-4. Allowed API-key response-phase transforms are exactly:

- `strip_reasoning`
- `reasoning_to_think_xml`
- `think_xml_to_reasoning`

TM-TF-5. API key `transforms` MUST NOT include transforms that can modify routing, upstream model selection, upstream pricing tier, request execution mode, output token ceiling, or arbitrary provider passthrough fields. This forbidden set includes at minimum:

- `set_field`
- `remove_field`
- `force_stream`
- `override_max_tokens`
- `reasoning_effort_to_budget`
- `reasoning_effort_to_model_suffix`

TM-TF-6. Requests rejected by TM-TF-2 through TM-TF-5 MUST return HTTP `400` with code `invalid_request`. The error response body MUST include a human-readable message identifying the disallowed transform name.

TM-TF-7. Runtime enforcement MUST be defensive: when an API key row is loaded from storage, the server MUST discard any transform rules that are not permitted by TM-TF-3 and TM-TF-4 before attaching them to the authenticated context.

TM-TF-8. Admin bypass: Users with role `super_admin` or `admin` (as determined by `UserRole::can_manage_system()`) are exempt from TM-TF-2 through TM-TF-5. For admin users, `validate_api_key_transforms` MUST accept any transform, and `sanitize_api_key_transforms` MUST preserve all transforms without filtering.

TM-TF-9. When an API key create or update request is rejected by the server (including but not limited to transform validation failures), the frontend MUST display the server error message to the user via a toast notification. Silent failure is not acceptable.

### 2.5 Delete API key

- **Endpoint:** `DELETE /api/dashboard/tokens/{key_id}`
- **Authorization:** Any authenticated user, but only for keys owned by that user.
- **Response:** `{ "success": true }`

TM-DEL-1. A successful API key delete MUST invalidate in-memory API key cache entries for the deleted key id before returning the response.

### 2.6 Batch delete API keys

- **Endpoint:** `POST /api/dashboard/tokens/batch-delete`
- **Authorization:** Any authenticated user.
- **Request body:** `{ "ids": string[] }`
- **Behavior:** The server MUST delete only keys owned by the current user.
- **Response:** `{ "success": true, "deleted_count": integer }`

TM-BATCH-1. A successful batch delete MUST invalidate in-memory API key cache entries for all deleted key ids before returning the response.

## 3. Runtime quota cache coherence

TM-Q1. Any operation that decrements `quota_remaining` for an API key MUST invalidate in-memory API key cache entries for that key id in the same process before returning.
