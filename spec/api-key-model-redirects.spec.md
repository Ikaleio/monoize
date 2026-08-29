# Pre-Redirect (API Key and Global Model Rewrite)

## Purpose

Allow ordered regex-based model name rewriting at API-key and global scope. Rewriting
executes before routing, billing, model-limit checks, and all other request processing.
An API-key rule overrides a global rule for the same request.

## Data Model

### `ModelRedirectRule`

| Field     | Type   | Constraints                                       |
|-----------|--------|---------------------------------------------------|
| `pattern` | string | Non-empty Rust regex with a maximum UTF-8 length of 256 bytes. Matched against the full model string using `^(pattern)$` anchoring. |
| `replace` | string | Non-empty literal target model name.              |

### Storage

- Column `model_redirects` on `api_keys` table, type `TEXT`, default `'[]'`.
- JSON-serialized `Vec<ModelRedirectRule>`.
- Added via migration `m20260327_000010_api_key_model_redirects`.
- System setting `global_model_redirects`, stored as a JSON-serialized
  `Vec<ModelRedirectRule>`.
- A missing, invalid, non-array, or constraint-violating `global_model_redirects` setting resolves to `[]`.

## Behavior

### Preconditions

- The request has passed authentication (API key validated).
- The request body has been decoded into a `UrpRequest` (model field extracted).

### Execution

1. Let `api_key_rules` = the API key's `model_redirects` list (possibly empty).
2. Let `global_rules` = the current system setting `global_model_redirects`
   (possibly empty).
3. Let `original_model` = `urp_request.model`.
4. For each precompiled `rule` in `api_key_rules` (order preserved):
   a. If `original_model` matches `^(rule.pattern)$`:
      - Set `urp_request.model = rule.replace`.
      - **Stop** (first match wins).
5. If no API-key rule matched, repeat step 4 using `global_rules`.
6. If no rule in either scope matched, `urp_request.model` is unchanged.

The replacement value MUST NOT be processed by any later rule. At most one
pre-redirect is applied to one request.

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
apply_model_redirects()        ← API-key rules, then global rules
ensure_model_allowed()         ← sees rewritten model
... routing, billing, etc.     ← all see rewritten model
```

The execution order applies to `POST /v1/responses`, `POST /v1/chat/completions`,
`POST /v1/messages`, `POST /v1/embeddings`, `POST /v1/responses/compact`,
`POST /v1/images/generations`, `POST /v1/images/edits`, and every `/api` alias
of those endpoints.

For `/v1/images/generations` and `/v1/images/edits`, the handler MUST rewrite
the extracted `model` value before `ensure_model_allowed` and before building
the internal URP subrequests that feed routing and billing.

### Constraints

- Maximum 32 rules per API key.
- Maximum 32 global rules.
- Each `pattern` has a maximum UTF-8 length of 256 bytes.
- Each `pattern` must be a valid Rust regex (the `regex` crate).
- Invalid patterns are rejected at create/update time with a 400 error.
- Empty `pattern` or empty `replace` is rejected.
- API-key create and update validation MUST compile every accepted pattern.
- Loading an API key from storage MUST compile every persisted pattern before the key can authenticate a forwarding request.
- Loading or updating `global_model_redirects` MUST compile every accepted pattern before publishing the runtime snapshot.
- A forwarding request MUST reuse the compiled API-key and global patterns. It MUST NOT compile a model redirect pattern.

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

### Get System Settings

`GET /api/dashboard/settings` includes:

```json
{
  "global_model_redirects": [
    { "pattern": "claude-.*", "replace": "gpt-5.6-sol" }
  ]
}
```

The default is `[]`.

### Update System Settings

`PUT /api/dashboard/settings` accepts optional field:

```json
{
  "global_model_redirects": [
    { "pattern": "claude-.*", "replace": "gpt-5.6-sol" }
  ]
}
```

When present, the field replaces the complete ordered global rule list. When
absent, the stored list is unchanged. A successful update MUST publish the new
list to the forwarding runtime before returning. A process restart MUST load
the stored list before accepting forwarding requests.

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

FR-8. `/dashboard/admin-settings` MUST expose a `Global Model Redirects` card
bound to `global_model_redirects` in the existing settings load and save flow.

FR-9. The global card MUST render the ordered rule list. Each row MUST contain
a visible label and text input for `pattern`, a visual arrow, a visible label
and text input for `replace`, and an accessible remove control.

FR-10. The global card MUST include an add control that appends
`{ pattern: "", replace: "" }` to the local settings draft.

FR-11. At viewport widths below `640px`, each global rule row MUST stack its
pattern and replacement controls vertically without horizontal overflow. At
widths of `640px` and above, the pattern and replacement controls MUST occupy
two columns.

FR-11a. At viewport widths of `640px` and above, the visual arrow MUST occupy an input-height alignment lane. Its vertical center MUST equal both input centers.

FR-12. Saving global settings MUST omit rows whose trimmed `pattern` or trimmed
`replace` is empty. The remaining row order MUST be preserved.

FR-13. Loading settings MUST render the existing settings-page skeleton until
the `global_model_redirects` value is available. A save MUST use the existing
optimistic settings mutation and error-toast behavior.

## Error Cases

| Condition                    | HTTP | Code                    | Message                                      |
|------------------------------|------|-------------------------|----------------------------------------------|
| Invalid regex in `pattern`   | 400  | `invalid_request`       | `"invalid model redirect pattern: {detail}"` |
| Empty `pattern`              | 400  | `invalid_request`       | `"model redirect pattern must not be empty"` |
| Empty `replace`              | 400  | `invalid_request`       | `"model redirect replace must not be empty"` |
| `pattern` longer than 256 bytes | 400 | `invalid_request`    | `"model redirect pattern exceeds 256 bytes"` |
| More than 32 rules           | 400  | `invalid_request`       | `"too many model redirect rules (max 32)"`   |
