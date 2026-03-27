# Pre-Redirect (API Key Model Rewrite)

## Purpose

Allow per-API-key regex-based model name rewriting that executes **before** routing,
billing, model-limit checks, and all other request processing. Designed for
Claude Code custom model scenarios where the client sends model names like
`claude-opus-4-6-20250610` but the gateway should route and bill as `gpt-5.4`.

## Data Model

### `ModelRedirectRule`

| Field     | Type   | Constraints                                       |
|-----------|--------|---------------------------------------------------|
| `pattern` | string | Non-empty regex. Matched against the full model string using `^(pattern)$` anchoring. |
| `replace` | string | Non-empty literal target model name.              |

### Storage

- Column `model_redirects` on `api_keys` table, type `TEXT`, default `'[]'`.
- JSON-serialized `Vec<ModelRedirectRule>`.
- Added via migration `m20260327_000010_api_key_model_redirects`.

## Behavior

### Preconditions

- The request has passed authentication (API key validated).
- The request body has been decoded into a `UrpRequest` (model field extracted).

### Execution

1. Let `rules` = the API key's `model_redirects` list (possibly empty).
2. Let `original_model` = `urp_request.model`.
3. For each `rule` in `rules` (order preserved):
   a. Compile `rule.pattern` as a regex (case-sensitive).
   b. If `original_model` matches `^(rule.pattern)$`:
      - Set `urp_request.model = rule.replace`.
      - **Stop** (first match wins).
4. If no rule matched, `urp_request.model` is unchanged.

### Postconditions

- The (possibly rewritten) model name is used for:
  - `ensure_model_allowed` check
  - Transform matching (`transform_match_model`)
  - Routing (`build_monoize_attempts`)
  - Billing (`logical_model`)
  - Request logging (`model` field)
  - Response `model` field rewriting

### Execution Order in Handler

```
auth_tenant()
ensure_balance_before_forward()
ensure_quota_before_forward()
decode_urp_request()           ← model extracted here
apply_model_redirects()        ← NEW: rewrite model here
ensure_model_allowed()         ← sees rewritten model
... routing, billing, etc.     ← all see rewritten model
```

For `/v1/images/generations` and `/v1/images/edits`, the handler MUST rewrite the extracted `model` value before `ensure_model_allowed` and before building the internal URP subrequests that feed routing and billing.

### Constraints

- Maximum 32 rules per API key.
- Each `pattern` must be a valid Rust regex (the `regex` crate).
- Invalid patterns are rejected at create/update time with a 400 error.
- Empty `pattern` or empty `replace` is rejected.

## API Surface

### Create API Key

`POST /api/dashboard/api-keys`

Request body gains optional field:

```json
{
  "model_redirects": [
    { "pattern": ".*opus.*", "replace": "gpt-5.4" },
    { "pattern": ".*haiku.*", "replace": "gpt-5.4-mini" }
  ]
}
```

Default: `[]` (no redirects).

### Update API Key

`PUT /api/dashboard/api-keys/:id`

Request body gains optional field:

```json
{
  "model_redirects": [
    { "pattern": ".*opus.*", "replace": "gpt-5.4" }
  ]
}
```

When present, replaces the entire list. When absent, the field is unchanged.

### Get / List API Keys

Response includes:

```json
{
  "model_redirects": [
    { "pattern": ".*opus.*", "replace": "gpt-5.4" }
  ]
}
```

The dashboard backend uses the JSON field name `model_redirects` in create,
update, get, list, and create-response payloads.

## Frontend

FR-1. The dashboard API key create dialog in `frontend/src/pages/api-keys.tsx` MUST expose a section labeled `Model Redirects` after the transform editor.

FR-2. The dashboard API key edit dialog in `frontend/src/pages/api-keys.tsx` MUST expose the same `Model Redirects` section after the transform editor.

FR-3. The `Model Redirects` section MUST render the current `model_redirects` array as an ordered list of rows. Each row MUST contain:

- one text input bound to `pattern`
- a visual `→` separator
- one text input bound to `replace`
- one remove control that deletes that row

FR-4. The `Model Redirects` section MUST include an add control that appends a new row with `{ pattern: "", replace: "" }`.

FR-5. When an API key is opened in the edit dialog, the frontend MUST initialize the dialog state from `key.model_redirects`, or `[]` if the field is absent.

FR-6. When the create form state is reset, the frontend MUST reset `model_redirects` state to `[]`.

FR-7. When the frontend submits create or update requests, it MUST include `model_redirects` only as the ordered list of rows whose trimmed `pattern` and trimmed `replace` are both non-empty. Rows with an empty trimmed `pattern` or empty trimmed `replace` MUST be omitted from the request payload.

## Error Cases

| Condition                    | HTTP | Code                    | Message                                      |
|------------------------------|------|-------------------------|----------------------------------------------|
| Invalid regex in `pattern`   | 400  | `invalid_request`       | `"invalid model redirect pattern: {detail}"` |
| Empty `pattern`              | 400  | `invalid_request`       | `"model redirect pattern must not be empty"` |
| Empty `replace`              | 400  | `invalid_request`       | `"model redirect replace must not be empty"` |
| More than 32 rules           | 400  | `invalid_request`       | `"too many model redirect rules (max 32)"`   |
