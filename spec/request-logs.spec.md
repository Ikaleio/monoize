# Request Logs Specification

## 0. Status

- **Purpose:** Record and expose per-request metadata for all API-key-authenticated proxy requests.
- **Scope:** Applies to all forwarding endpoints (responses, chat completions, messages, embeddings) and the dashboard request-logs API.

## 1. Data model

### 1.1 Request log row

A request log row has:

- `id: string` (UUID)
- `request_id: string` (the `x-request-id` header assigned by tower-http `SetRequestIdLayer`)
- `user_id: string`
- `api_key_id: string?`
- `model: string` (logical model requested by the client)
- `provider_id: string?`
- `upstream_model: string?`
- `channel_id: string?` (the channel that ultimately served the request)
- `is_stream: boolean`
- `prompt_tokens: integer?`
- `completion_tokens: integer?`
- `cached_tokens: integer?`
- `reasoning_tokens: integer?`
- `provider_multiplier: float?`
- `charge_nano_usd: string?` (nano-dollar integer string)
- `status: string` (`"success"` or `"error"`)
- `usage_breakdown_json: object?` (normalized per-request usage detail snapshot; persisted as JSON text in DB)
- `billing_breakdown_json: object?` (per-request pricing and charge breakdown snapshot at billing time; persisted as JSON text in DB)
- `error_code: string?` (error code for failed requests, e.g. `upstream_error`)
- `error_message: string?` (error message for failed requests)
- `error_http_status: integer?` (HTTP status returned to downstream client for failed requests)
- `duration_ms: integer?` (wall-clock time from request start to upstream response)
- `ttfb_ms: integer?` (time from request start to first byte/chunk from upstream; null for non-streaming)
- `request_ip: string?` (client IP address extracted from `x-forwarded-for` header or socket peer)
- `created_at: RFC3339 string`

### 1.2 Enriched fields (computed at query time, not stored)

When returning request log rows via the dashboard API, the following fields are JOINed from related tables:

- `username: string?` (from `users.username` via `user_id`)
- `api_key_name: string?` (from `api_keys.name` via `api_key_id`)
- `channel_name: string?` (from `monoize_channels.name` via `channel_id`)

## 2. Recording rules

RL1. A request log MUST be inserted for every terminal proxy request outcome (success or error) that is authenticated by a database API key (`user_id` is present).

RL1a. `status` MUST be `"success"` when the downstream client received a normal API response payload (including truncated/cutoff completion cases such as `finish_reason = "length"`), and MUST be `"error"` only when the request ends with an API error response.

RL2. Requests authenticated only by static config keys MUST NOT generate request logs.

RL3. Log insertion MUST be fire-and-forget (spawned asynchronously) and MUST NOT block the response to the client.

RL4. For non-streaming requests, the log MUST include token usage from the upstream response. `ttfb_ms` MUST be null.

RL5. For streaming requests where response transforms require buffering (synthetic stream), the log MUST include token usage. `ttfb_ms` MUST record the time from `started_at` to the point where the upstream response body is received.

RL6. For pass-through streaming requests, `ttfb_ms` MUST record the time from `started_at` to the point where the first chunk is received from upstream.

RL6a. For pass-through streaming requests where usage cannot be extracted from streamed events, token usage fields MAY be omitted (set to null).

RL7. The `duration_ms` field MUST measure wall-clock time from the start of request processing (after auth) to the point where the upstream response is received.

RL8. The `request_id` field MUST be populated from the `x-request-id` header set by the tower-http middleware.

RL9. The `request_ip` field MUST be extracted from the `x-forwarded-for` header (first IP), falling back to `x-real-ip`, then omitted if neither is present.

RL10. The `channel_id` field MUST record the ID of the channel that ultimately served the request.

RL11. For non-streaming requests and synthetic-stream requests (where usage is available and billing is executed), `charge_nano_usd` in `request_logs` MUST equal the computed request charge persisted by the billing subsystem for the same request.

RL12. For pass-through streaming requests where usage is unavailable and billing is skipped, `charge_nano_usd` MAY be null.

RL13. For pass-through streaming requests where usage is extracted from streamed events, the log MUST persist extracted usage fields and `charge_nano_usd` MUST equal the computed request charge persisted by the billing subsystem for that request.

RL14. For failed requests (`status = "error"`):

- `charge_nano_usd` MUST be null.
- `billing_breakdown_json` MUST be null.
- `error_code` and `error_message` MUST be populated when available.
- `error_http_status` MUST store the HTTP status returned to the client.

RL15. For successful requests where usage exists, `usage_breakdown_json` MUST persist a request-time snapshot of usage details. The snapshot MUST include `input.total_tokens` and `output.total_tokens`, and SHOULD include subtype token counts when present (for example: cached, cache creation/read, reasoning, audio, image, text).

RL16. For successful requests where billing is executed, `billing_breakdown_json` MUST persist the request-time pricing snapshot used for billing. The snapshot MUST include at least:

- unit prices used for each billed token class,
- token quantities used in each billed class,
- per-class subtotal charges,
- provider multiplier,
- base charge and final charge.

## 3. Dashboard endpoint

### 3.1 List request logs

- **Endpoint:** `GET /api/dashboard/request-logs`
- **Authorization:** Any authenticated dashboard user.
- **Query parameters:**
  - `limit: integer` (default 50, clamped to [1, 200])
  - `offset: integer` (default 0, clamped to >= 0)
  - `model: string?` (filter by model name; supports comma-separated list for multi-model OR matching, e.g. `"gpt-4o, gpt-5"`. Each entry is trimmed and matched via substring.)
  - `status: string?` (filter by status, exact match: `"success"` or `"error"`)
  - `api_key_id: string?` (filter by specific API key ID)
  - `username: string?` (filter by username, exact match via JOIN on `users.username`; only effective when the caller has admin role — non-admin callers ignore this parameter)
  - `search: string?` (full-text search across model, upstream_model, request_id, request_ip)
- **Response:**

```json
{
  "data": EnrichedRequestLogRow[],
  "total": integer,
  "limit": integer,
  "offset": integer
}
```

Where `EnrichedRequestLogRow` = `RequestLogRow` + `username` + `api_key_name` + `channel_name`.

RL-API1. When the authenticated user has role `super_admin` or `admin`, the endpoint MUST return logs for ALL users. Otherwise, it MUST return only logs belonging to the current authenticated user.

RL-API2. Results MUST be ordered by `created_at DESC` (most recent first).

RL-API3. `total` MUST reflect the count of logs matching all active filters, not the page size.

RL-API4. Filter parameters are combined with AND logic.

### 3.2 Admin-visible vs user-visible fields

The API returns the same enriched schema for all users. The frontend controls column visibility:

- **Admin-only columns:** `username`, `channel` (display text uses `channel_name` when available, otherwise falls back to `channel_id`; tooltip keeps provider/channel context)
- **All users see:** `created_at`, `request_id`, `model` (with ModelBadge), `api_key_name`, `duration_ms`/`ttfb_ms`/`is_stream` (merged badge group), `prompt_tokens`, `completion_tokens`, `charge_nano_usd`, `status`, `request_ip`, and error tooltip details (`error_code`, `error_message`, `error_http_status`) when `status = "error"`.

## 4. Storage

RL-S1. Request logs MUST be stored in table `request_logs`.

RL-S2. The table MUST have a composite index on `(user_id, created_at DESC)` for efficient pagination.

RL-S3. The `user_id` foreign key MUST cascade on delete.

RL-S4. New columns (`request_id`, `channel_id`, `ttfb_ms`, `request_ip`, `usage_breakdown_json`, `billing_breakdown_json`, `error_code`, `error_message`, `error_http_status`) MUST be added via `ALTER TABLE ADD COLUMN` statements in the migration logic. All new columns are nullable to preserve backward compatibility with existing rows.

## 5. Frontend display

### 5.1 Format

FL1. The logs page MUST use a compact list format (dense table rows) with horizontal scrolling for overflow.

FL2. The `created_at` field MUST be displayed as a localized timestamp in format `YYYY-MM-DD HH:mm:ss` using the browser's local timezone.

FL3. The model column MUST use the `ModelBadge` component (same as Provider page).

FL4. The `duration_ms`, `ttfb_ms`, and `is_stream` fields MUST be merged into a single cell with adjacent rounded badges: `[总用时] [首字时间] [流]` (where 首字时间 and 流 badges are only shown when applicable).

FL5. The `api_key_name` column header MUST be "Token" (referring to the API key name, not the literal token value).

FL6. The multiplier column from the old layout MUST be removed (multiplier is already shown inside ModelBadge on the Provider page).

FL7. The top of the page MUST include a search bar and filter controls:
  - **Model filter**: text input accepting comma-separated model names (e.g. `gpt-4o, gpt-5`); applied on Enter or blur.
  - **Status filter**: dropdown with options `All`, `Success`, `Error`.
  - **Token filter**: dropdown listing all of the user's API keys by name; selecting one filters by `api_key_id`.
  - **Username filter** (admin only): text input defaulting to the current user's username; applied on Enter or blur. Non-admin users do not see this control.

FL8. Column order (left to right): `created_at`, `request_id` (with adjacent status indicator), `model` (ModelBadge), `api_key_name`, `[username]` (admin), `[channel]` (admin, with tooltip showing provider context), `duration/ttfb/stream` (merged badges), `prompt_tokens` (input), `completion_tokens` (output), `charge_nano_usd` (cost), `request_ip`.

FL9. For the admin channel column display value:

- If `channel_name` is non-empty, UI MUST render `channel_name` as the primary text.
- Else if `channel_id` is non-empty, UI MUST render `channel_id`.
- Else UI MUST render `-`.

FL10. The request logs table body MUST use virtualized rendering via `react-virtuoso` (`TableVirtuoso`) instead of rendering all loaded rows as plain DOM rows.

FL11. The request logs page MUST remove explicit previous/next pagination buttons. Additional rows MUST be loaded by scroll-to-end (infinite loading).

FL12. Infinite loading MUST fetch in backend-paginated chunks using `limit=100` and `offset = loaded_row_count` semantics, and MUST stop requesting when `loaded_row_count >= total`.

FL13. The virtualized table viewport MUST occupy the remaining page height below the header + filter controls (using a flexible layout) so the first screen shows as many rows as possible.

FL14. The filter-control area second row MUST include an IP visibility toggle button at the far right:

- The button MUST be a square icon button using an eye/eye-off glyph.
- Initial state MUST be "hidden".
- When hidden, request IP cell text MUST remain present but rendered with a Gaussian blur effect.
- When shown, the blur MUST be removed immediately.

FL15. The table MUST use compact column spacing:

- Header and body cells MUST use reduced horizontal/vertical padding suitable for dense log browsing.
- Columns MUST use content-oriented widths (instead of evenly stretched wide columns) to avoid large unused horizontal gaps between adjacent fields.

FL16. Token-count columns (`prompt_tokens` / `completion_tokens`) MUST keep compact widths suitable for short integer values (commonly up to 7 digits), and should avoid consuming excess horizontal space from adjacent columns.

FL17. The `duration/ttfb/stream` merged column MUST use compact badge spacing and width so that token-count columns remain visually closer to it (reduced horizontal gap).

FL18. Left-side leading columns (`created_at`, `request_id`) MUST use compact widths and reduced horizontal padding.

FL19. The first visible column (`created_at`) MUST keep a small left inset from the table edge to avoid text touching the border.

FL20. The status indicator MUST be rendered directly adjacent to the request ID text inside the same `request_id` cell (near-zero gap), and columns to the right SHOULD use reduced left padding to keep the layout left-compacted.

FL21. The `api_key_name` (Token) column MUST use a narrow width and truncated text display to avoid occupying excessive horizontal space.

FL22. The merged `duration/ttfb/stream` column MUST remain narrowly sized with minimal horizontal cell padding and compact badges, and MUST NOT reserve excess blank width when values are short.

FL23. The admin `channel` column MUST use a narrow width with aggressive truncation for long channel names, to minimize horizontal space usage.

FL24. On desktop dashboard layouts, the logs table SHOULD fit within the page content width without horizontal scrolling; the `request_ip` column MUST use narrow width with truncated text display.

FL25. The `charge_nano_usd` (Cost) column MAY use compact width, but its displayed value MUST keep regular currency formatting (no threshold shorthand such as `<$0.0001`).

FL26. Hovering the `charge_nano_usd` (Cost) cell MUST show billing breakdown details sourced from `billing_breakdown_json`, including per-class expression `unit_price × token_count` and subtotal, plus multiplier/base/final charge.

FL27. Hovering the `prompt_tokens` (Input) and `completion_tokens` (Output) cells MUST show usage breakdown details sourced from `usage_breakdown_json`, including subtype token counts when available (for example: text, cached, cache creation/read, image, audio, reasoning).

FL28. For rows with `status = "error"`, hovering the request-id/status indicator MUST show error details from `error_code`, `error_message`, and `error_http_status` when present.
