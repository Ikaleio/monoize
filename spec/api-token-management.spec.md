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
- `key_hash: string` (Argon2 hash of full key)
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

TM-CREATE-3. The server MUST store an Argon2 hash of the full key in `key_hash`.

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

### 2.5 Delete API key

- **Endpoint:** `DELETE /api/dashboard/tokens/{key_id}`
- **Authorization:** Any authenticated user, but only for keys owned by that user.
- **Response:** `{ "success": true }`

### 2.6 Batch delete API keys

- **Endpoint:** `POST /api/dashboard/tokens/batch-delete`
- **Authorization:** Any authenticated user.
- **Request body:** `{ "ids": string[] }`
- **Behavior:** The server MUST delete only keys owned by the current user.
- **Response:** `{ "success": true, "deleted_count": integer }`
