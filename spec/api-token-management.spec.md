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
- `key_prefix: string` (first 12 characters of the full key; display only and never an authentication or cache identity)
- `key: string` (the full key, stored as plaintext and used for authentication)
- `created_at: RFC3339 string`
- `expires_at: RFC3339 string?`
- `last_used_at: RFC3339 string?`
- `enabled: boolean`
- `sub_account_enabled: boolean`
- `sub_account_balance_nano_usd: string`
- `sub_account_balance_usd: string` (computed)
- `model_limits_enabled: boolean`
- `model_limits: string[]`
- `ip_whitelist: string[]`
- `use_user_group: boolean`
- `group_ids: string[]` (ordered group ids, see `groups-registry.spec.md`)
- `max_multiplier: string?` (canonical positive base-10 decimal string)
- `transforms: TransformRuleConfig[]`
- `request_capture_mode: "off" | "capture-all" | "capture-only-abnormal"`
- `request_capture_retention: "5m" | "1h" | "24h" | "7d"` (capture TTL, `request-capture-dumps.spec.md` RCD-C5a)

### 1.2 Group-scoped routing fields

TM-GRP-1. `use_user_group = true` means the key inherits the owning user's single group at
request-authentication time and the stored `group_ids` value is ignored. The legacy
`token_group` and `allowed_groups` columns no longer exist (`groups-registry.spec.md`
GR-D7).

TM-GRP-2. `group_ids` is an **ordered** JSON TEXT array of group ids. Order is significant:
it defines the routing preference order per `database-provider-routing.spec.md` R-GRP-2.

TM-GRP-3. On API key create/update, the server MUST canonicalize `group_ids` per
`groups-registry.spec.md` GR-C1 (trim, drop empties, dedupe preserving first-occurrence
order) and validate it per GR-C2/GR-C3 (at most 32 entries; every id must exist).

TM-GRP-4. On API key create/update, when the effective `use_user_group` value is `false`,
the canonicalized `group_ids` MUST be non-empty; otherwise the mutation MUST be rejected
with HTTP `400` and code `invalid_request`. When the effective `use_user_group` value is
`true`, the stored `group_ids` MUST be replaced by `[]`.

TM-GRP-5. Group selection permission on create/update, applied after canonicalization:

- if the requesting user's role satisfies `can_manage_system()` (`admin` or
  `super_admin`), any registered group id is selectable;
- otherwise every element of `group_ids` MUST reference a group with
  `user_selectable = 1` OR equal the owning user's current `group_id`.

TM-GRP-6. API key create/update requests that violate TM-GRP-3 through TM-GRP-5 MUST be
rejected with HTTP `400` and code `invalid_request`.

TM-GRP-6a. Stored key groups and selectable groups MUST NOT bypass an active subscription.
Forwarding authentication MUST apply the subscription ceiling defined by
`api-key-authentication.spec.md` AKG3 through AKG7 to inherited and explicit key groups.

TM-GRP-7. Stored `group_ids` decoding follows `groups-registry.spec.md` GR-C4; stored
`use_user_group` MUST be integer `0` or `1`, and any other persisted value MUST fail the
read.

TM-IP-1. Every non-empty `ip_whitelist` entry on create or update MUST parse as either an exact IPv4/IPv6 address or an IPv4/IPv6 CIDR network. Any invalid entry MUST reject the mutation with HTTP `400` and code `invalid_request`.

TM-IP-2. The server MUST persist exact addresses and CIDR networks in their canonical string representation. It MUST trim entries, deduplicate canonical duplicates, and preserve exact-address entries as addresses rather than converting them to host-prefix CIDRs.

TM-STORAGE-1. API-key dashboard reads and forwarding authentication MUST fail with a storage error when a persisted `model_limits`, `ip_whitelist`, `transforms`, or `model_redirects` value is malformed JSON, is not an array of the declared element type, or has an incompatible database type. They MUST NOT substitute an empty array.

TM-STORAGE-2. Persisted `enabled`, `sub_account_enabled`, `model_limits_enabled`, and `reasoning_envelope_enabled` values MUST be integer `0` or integer `1`. A null value, incompatible database type, or any other integer MUST fail the read. It MUST NOT be replaced by a default value.

TM-STORAGE-3. `group_ids` compatibility is limited to TM-GRP-7. Any other malformed or wrongly typed persisted `group_ids` or `use_user_group` value MUST fail the read and MUST NOT be treated as unrestricted or inherited access.

TM-STORAGE-4. A present, non-null `request_capture_mode` MUST equal `"off"`, `"capture-all"`, or `"capture-only-abnormal"`. Any other value or incompatible database type MUST fail the read instead of falling back to `request_capture_enabled` or `"off"`. An absent or null value retains the `"off"` compatibility behavior defined by `request-capture-dumps.spec.md` RCD-C8.

TM-STORAGE-7. A present, non-null `request_capture_retention` MUST equal `"5m"`, `"1h"`, `"24h"`, or `"7d"`. Any other value or incompatible database type MUST fail the read. An absent or null value reads as `"24h"` (`request-capture-dumps.spec.md` RCD-C5b).

TM-STORAGE-5. Every selected API-key and owning-user column MUST propagate an incompatible database type as a storage error. In particular, a failed `api_keys.key` decode MUST NOT become an empty token and a failed nullable `users.email` decode MUST NOT become null.

TM-STORAGE-6. Every query that decodes a complete API-key record MUST select the owning user's role in the same point or set-based JOIN as `owner_role`. API-key row decoding MUST NOT issue a fallback user query when `owner_role` is absent, null, or malformed.

## 2. Endpoints

All endpoints in this spec require an authenticated dashboard session.

### 2.1 List my API keys

- **Endpoint:** `GET /api/dashboard/tokens`
- **Authorization:** Any authenticated user.
- **Response:** `APIKey[]` for the current user.

### 2.1a List a user's API keys (admin)

- **Endpoint:** `GET /api/dashboard/users/{user_id}/api-keys`
- **Authorization:** `admin` or `super_admin`; other roles receive `403 forbidden`.
- **Response:** `APIKey[]` owned by `user_id`, with the same item shape and order as §2.1.
- **Errors:** `404 not_found` if the user does not exist.

TM-ADM-LIST-1. Every item's `key` field MUST be the empty string. The endpoint MUST NOT return any user's full key; `key_prefix` identifies the key.

TM-ADM-LIST-2. The endpoint is read-only. Mutating another user's key through §2.4 remains forbidden by TM-QUERY-2.

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
  - `sub_account_enabled: boolean` (default false)
  - `sub_account_balance_nano_usd: string` (default `"0"`; an explicit value is admin only)
  - `model_limits_enabled: boolean` (default false)
  - `model_limits: string[]` (default empty)
  - `ip_whitelist: string[]` (default empty)
  - `use_user_group: boolean` (default true, meaning inherit the owning user's group)
  - `group_ids: string[]` (default empty; required non-empty when `use_user_group` is false)
  - `max_multiplier: string?` (default null)
  - `transforms: TransformRuleConfig[]` (default empty)
  - `request_capture_mode: "off" | "capture-all" | "capture-only-abnormal"` (default `"off"`)
  - `request_capture_retention: "5m" | "1h" | "24h" | "7d"` (default `"24h"`)
- **Response:** The created key object including the full key string.

TM-CREATE-1. The generated full key MUST start with the literal prefix `sk-`.

TM-CREATE-1a. A present `expires_in_days` below `1` MUST be rejected with HTTP `400`, code `invalid_request`, and `param = "expires_in_days"`. An absent value creates a key without expiry.

TM-CREATE-2. The server MUST compute `key_prefix` as the first 12 characters of the full key.

TM-CREATE-3. The server MUST persist the full key as plaintext in `api_keys.key`. The `api_keys` table MUST NOT contain a hash, digest, or encrypted copy of the full key. Runtime token validation semantics are defined in `api-key-authentication.spec.md`.

TM-CREATE-3a. The database MUST index `api_keys.key` for complete-token lookup.

TM-CREATE-3b. Migration `m20260824_000039_api_keys_plaintext_only` MUST delete `idx_api_keys_key_hash` and the `api_keys.key_hash` column when they exist. It MUST create `idx_api_keys_key` on `api_keys.key`. Its rollback path MUST NOT restore `key_hash`.

TM-CREATE-4. After successful key creation, there is no required cache invalidation side-effect because the new key does not exist in cache yet.

TM-CREATE-5. `POST /api/dashboard/tokens` MUST read only the `api_key_max_per_user` setting. One process-local async critical section shared by every `UserStore` clone MUST contain both `COUNT(*) WHERE user_id = $1` and the key insert. If the count is at least the supplied positive limit, the store MUST perform no insert and the endpoint MUST return HTTP `403` with code `max_api_keys_reached`. Concurrent create requests in one process MUST never commit more than the configured number of keys for one user.

### 2.4 Update API key

- **Endpoint:** `PUT /api/dashboard/tokens/{key_id}`
- **Authorization:** Any authenticated user, but only for keys owned by that user.
- **Request body:** partial update with optional fields:
  - `name`
  - `enabled`
  - `sub_account_enabled`
  - `sub_account_balance_nano_usd` (admin only)
  - `expected_sub_account_balance_nano_usd` (admin only; see `api-key-sub-account-billing.spec.md` SA-ADM5)
  - `model_limits_enabled`
  - `model_limits`
  - `ip_whitelist`
  - `use_user_group`
  - `group_ids`
  - `max_multiplier`
  - `transforms`
  - `request_capture_mode`
  - `request_capture_retention`
  - `expires_at` (RFC3339 string or null)
- **Errors:** `404 not_found` if the key does not exist or is not owned by the user.

TM-UPD-0. `expires_at` distinguishes absent from `null`: an absent field keeps the stored expiry, `null` clears it (the key never expires), and an RFC3339 string replaces it. Any other string MUST be rejected with HTTP `400` and code `invalid_request`.

TM-UPD-1. A successful API key update MUST invalidate in-memory API key cache entries for the updated key id before returning the response.

### 2.4a API-key transform safety boundary

TM-TF-1. API key `transforms` are user-scoped request/response shaping rules. They MUST NOT act as routing, pricing, or upstream service-tier controls.

TM-TF-2. The server MUST reject API key create/update requests whose `transforms` array contains any rule outside the allowed API-key transform subset defined by TM-TF-3 and TM-TF-4.

TM-TF-3. Allowed API-key request-phase transforms are exactly:

- `prompt_inject_system`
- `role_system_to_developer`
- `role_merge_consecutive`
- `prompt_append_empty_user`
- `image_compress_input`
- `image_enable_openai_generation_tool`
- `prompt_strip_anthropic_billing_header`
- `cache_anthropic_auto`
- `cache_anthropic_system`
- `cache_anthropic_tool_use`
- `cache_openai_tool_use`
- `cache_user_id`
- `cache_openai_prompt`
- `field_alias_reserved_tool_names`
- `field_custom_tools_to_function`

TM-TF-4. Allowed API-key response-phase transforms are exactly:

- `reasoning_strip_output`
- `reasoning_strip_encrypted`
- `reasoning_to_think_xml`
- `reasoning_from_think_xml`
- `stream_split_sse_frames`
- `reasoning_content_to_summary`
- `reasoning_inject_content_field`
- `reasoning_summary_to_raw_cot`
- `reasoning_summary_heading`
- `image_markdown_to_output`
- `image_output_to_markdown`
- `image_compress_output`
- `field_alias_reserved_tool_names`
- `field_custom_tools_to_function`

TM-TF-5. API key `transforms` MUST NOT include transforms that can modify routing, upstream model selection, upstream pricing tier, request execution mode, output token ceiling, or arbitrary provider passthrough fields. This forbidden set includes at minimum:

- `field_set`
- `field_remove`
- `stream_force`
- `field_override_max_tokens`
- `reasoning_effort_to_budget`
- `reasoning_effort_to_model_suffix`

TM-TF-6. Requests rejected by TM-TF-2 through TM-TF-5 MUST return HTTP `400` with code `invalid_request`. The error response body MUST include a human-readable message identifying the disallowed transform name.

TM-TF-7. Runtime enforcement MUST be defensive: when an API key row is loaded from storage, the server MUST discard any transform rules that are not permitted by TM-TF-3 and TM-TF-4 before attaching them to the authenticated context.

TM-TF-8. Admin bypass: Users with role `super_admin` or `admin` (as determined by `UserRole::can_manage_system()`) are exempt from TM-TF-2 through TM-TF-5. For admin users, `validate_api_key_transforms` MUST accept any transform, and `sanitize_api_key_transforms` MUST preserve all transforms without filtering.

TM-TF-9. When an API key create or update request is rejected by the server (including but not limited to transform validation failures), the frontend MUST show a localized message: an inline field error when the error's `param` names a dialog field, otherwise a toast with the `apiErrors.<code>` text or the generic request-failed text. It MUST NOT display the raw server `message`. Silent failure is not acceptable.

TM-TF-10. The dashboard transform registry consumed by the API key editor MUST carry explicit transform scope metadata. The API key editor MUST list only transforms whose scope includes `api_key`; transforms that are unavailable for API keys MUST be hidden rather than displayed and rejected later.

### 2.5 Delete API key

- **Endpoint:** `DELETE /api/dashboard/tokens/{key_id}`
- **Authorization:** Any authenticated user, but only for keys owned by that user.
- **Response:** `{ "success": true }`

TM-DEL-1. A successful API key delete MUST invalidate in-memory API key cache entries for the deleted key id before returning the response.

TM-DEL-2. Monoize MUST NOT maintain an API-key name cache. A successful user delete MUST invalidate authentication-cache entries for the deleted user's keys through the user-id reverse index before returning the response.

### 2.6 Batch delete API keys

- **Endpoint:** `POST /api/dashboard/tokens/batch-delete`
- **Authorization:** Any authenticated user.
- **Request body:** `{ "ids": string[] }`
- **Behavior:** The server MUST delete only keys owned by the current user.
- **Response:** `{ "success": true, "deleted_count": integer }`

TM-BATCH-1. A successful batch delete MUST invalidate in-memory API key cache entries for all deleted key ids before returning the response.

TM-BATCH-2. Batch-delete ownership filtering MUST be performed by a set-based database query constrained by both current-user id and requested key ids. It MUST NOT list every key owned by the user or issue one ownership query per requested id.

TM-BATCH-3. `MONOIZE_API_KEY_BATCH_DELETE_MAX_IDS` MUST configure the positive maximum number of ids accepted by one batch-delete request or store call. Missing, zero, invalid, negative, or overflowing values MUST use `400`. A parsed value above `400` MUST be clamped to `400` so every supported SQLite build remains below its bind-variable limit.

TM-BATCH-4. The HTTP endpoint MUST reject a request containing more than TM-BATCH-3 ids with HTTP `400` before an ownership query. Store ownership filtering and transactional batch deletion MUST independently reject an input above the same limit. Every dynamic `IN` list in these paths MUST therefore contain at most 400 values.

## Startup Transform-ID Migration

TM-MIG-1. API-key transform-rule id canonicalization MUST use the persistent `system_settings` marker `migration.api_key_transform_rule_ids.v2`. When the marker value is `complete`, startup MUST perform only the marker point query and MUST NOT scan `api_keys`. Canonicalization MUST use the map defined by `urp-transform-system.spec.md` TF-17.

TM-MIG-2. When the marker is absent, one process MUST process API-key transform rows in ascending-ID keyset batches of at most 300 rows. Each batch MUST select, canonicalize, update changed rows through at most one CASE statement, and commit before the next batch begins. Memory use and one transaction's row count MUST remain bounded by 300. Invalid transform JSON MUST remain unchanged and MUST NOT prevent progress. The completion marker MUST be written in a separate final transaction only after every batch commits; that final transaction MUST also delete the obsolete `system_settings` row `key = "migration.api_key_transform_rule_ids.v1"`. A restart before the marker is written MAY re-read canonicalized rows and MUST converge without changing their values.

TM-QUERY-1. The create limit check MUST use `COUNT(*) WHERE user_id = current_user_id`; it MUST NOT decode every API-key row. The count and insert MUST satisfy TM-CREATE-5.

TM-QUERY-2. Get, update, and delete ownership checks MUST query by both `id` and `user_id`; they MUST NOT list every API key owned by the current user.

## 3. Runtime sub-account cache coherence

TM-Q1. Any operation that mutates `sub_account_balance_nano` for an API key MUST invalidate in-memory API key cache entries for that key id in the same process before returning.

TM-Q2. Sub-account billing behavior is defined in `api-key-sub-account-billing.spec.md`.

## 4. Dashboard balance controls

TM-UI1. The create dialog MUST render the initial sub-account balance input only when the authenticated user's role is `admin` or `super_admin` AND the dialog's sub-account switch is on. The input MUST accept a non-negative USD decimal with at most 9 fractional digits, MUST show the converted value as "= {nano} nano-USD" below it, and MUST convert to `sub_account_balance_nano_usd` with exact integer arithmetic (`BigInt`); it MUST NOT pass the value through JavaScript `Number`, `parseInt`, `parseFloat`, or `toFixed`. An invalid value MUST show an inline field error and block submit. The edit dialog MUST NOT render a balance input, and an edit mutation MUST NOT include `sub_account_balance_nano_usd`; balance changes go through TM-UI1a. When an edit disables sub-account billing, the server consolidates the locked current balance (`api-key-sub-account-billing.spec.md` SA-API6).

TM-UI1a. For `admin` and `super_admin` users, each key row with `sub_account_enabled = true` MUST show an "Adjust balance" action next to the transfer action. It opens a separate dialog that shows the current balance, a signed USD decimal input (at most 9 fractional digits, exact `BigInt` conversion) for the new balance, and a preview "current → new". When the new balance is lower, the dialog MUST state that the difference is returned to the owner's wallet. Submit sends exactly `{ sub_account_balance_nano_usd, expected_sub_account_balance_nano_usd }`, where the expected value is the balance the dialog displayed. On `409 balance_changed` the dialog MUST stay open, revalidate the key list, display the fresh current balance, and show the localized message "balance changed (now $X), confirm and retry". On success the dialog closes, the row is updated from the response, the key list is revalidated, and the session user is refreshed.

TM-UI2. A non-admin create or update mutation MUST omit `sub_account_balance_nano_usd` from its JSON request body.

TM-UI3. The create and edit dialogs MUST render a `request_capture_retention` select with exactly the options `5m`, `1h`, `24h`, and `7d` (localized labels, stored values verbatim). The select MUST be rendered iff the dialog's `request_capture_mode` value is not `"off"`. When hidden, the mutation still submits the current draft value, so toggling the mode does not reset retention. The create dialog's initial value MUST be `"24h"`.

TM-UI4. The create and edit dialogs MUST render one shared form component (`frontend-design-system.spec.md` DS67). The expiry field renders in both modes. Create renders a whole-days input that accepts empty (never expires) or an integer ≥ 1 and submits `expires_in_days`; any other value shows an inline field error and blocks submit. Edit renders a date picker plus a "Never expires" checkbox and submits `expires_at` only when the value changed: the end of the selected local day as an RFC3339 string, or `null` when "Never expires" is checked. Every other field MUST render with the same label, help text, and control in both modes. When `model_limits_enabled` is true, `model_limits` MUST be edited with `ModelMultiSelect` (DS63) with `allowCustom = true`; its options MUST be the `model_id` values from `GET /api/dashboard/marketplace/models`. The mutation MUST submit the selected IDs in display order.

TM-UI5. The key dialog MUST be a `<form>` whose primary button is `type="submit"`, so Enter in a single-line input submits. Its basic section MUST contain, in order: name, expiry, groups (TM-UI9), and the sub-account switch (plus the TM-UI1 balance input). All other fields MUST live in one "Advanced settings" section that is collapsed by default in both modes. The collapsed header MUST show "N configured", where N counts advanced items that differ from their create defaults: reasoning-envelope protection off, request capture not `"off"`, model limits on, a non-empty IP allowlist, a max multiplier, a non-empty transform chain, and at least one non-empty model redirect. The section MUST expand automatically when submit finds an invalid field inside it. The transform editor MUST render only for `admin` and `super_admin` users, with `TransformChainEditor layer="apiKey"`; non-admins MUST NOT see a transform section. The dialog MUST pass `dirty` to `DialogContent` when the draft differs from its initial value.

TM-UI6. The transfer dialog MUST validate the USD amount inline (positive, at most 9 fractional digits). It MUST show the preview "account $A → $A−x; key $B → $B+x" (the account side reads "unlimited" for a `balance_unlimited` user). The transfer button MUST be disabled while the amount is invalid or, for a finite-balance user, exceeds the account balance. On success the frontend MUST set the row's sub-account balance from `api_key_balance_nano_usd`, revalidate the key list, and refresh the session user. `402 insufficient_balance` MUST show a localized message.

TM-UI7. The list balance cell MUST display an enabled sub-account balance as `$` + `formatUsdDecimal(sub_account_balance_usd, 2)`.

TM-UI8. `/dashboard/tokens` MUST show one read-only "API base URL" line with a `CopyButton` directly under the page header, and the create-result panel MUST repeat it next to the new key. The line MUST render the label, the URL in `font-mono`, and the copy action on one line in viewports ≥ 640 CSS px; it MUST NOT render a bordered box. The value is the public setting `api_base_url`; when that is empty, the line MUST show `window.location.origin` labelled "current site address (not configured by the admin)".

TM-UI9. The group switch MUST read "Follow account group". Its help text MUST state that the key uses the account's current group, naming that group when known, and that turning it off allows an ordered list of groups.
