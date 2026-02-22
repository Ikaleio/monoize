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
- `status: string` (`"pending"`, `"success"`, or `"error"`)
- `usage_breakdown_json: object?` (normalized per-request usage detail snapshot; persisted as JSON text in DB)
- `billing_breakdown_json: object?` (per-request pricing and charge breakdown snapshot at billing time; persisted as JSON text in DB)
- `error_code: string?` (error code for failed requests, e.g. `upstream_error`)
- `error_message: string?` (error message for failed requests)
- `error_http_status: integer?` (HTTP status returned to downstream client for failed requests)
- `duration_ms: integer?` (wall-clock time from request start to upstream response)
- `ttfb_ms: integer?` (time from request start to first byte/chunk from upstream; null for non-streaming)
- `request_ip: string?` (client IP address extracted from `x-forwarded-for` header or socket peer)
- `tried_providers_json: object[]?` (array of `{ provider_id, channel_id, error }` objects recording providers/channels that were attempted and failed before the final result; persisted as JSON text in DB; null when no fallback occurred)
- `request_kind: string?` (classification of log source; null for normal client requests. `"active_probe_connectivity"` for active health-probe connectivity tests)
- `created_at: RFC3339 string`

### 1.2 Enriched fields (computed at query time, not stored)

When returning request log rows via the dashboard API, the following fields are JOINed from related tables:

- `username: string?` (from `users.username` via `user_id`)
- `api_key_name: string?` (from `api_keys.name` via `api_key_id`)
- `channel_name: string?` (from `monoize_channels.name` via `channel_id`)
- `provider_name: string?` (from `monoize_providers.name` via `provider_id`)

## 2. Recording rules

RL1. For every API-key-authenticated proxy request (`user_id` is present), the system MUST create exactly one lifecycle request-log row.

RL1a. Before the first upstream attempt is sent, the lifecycle row MUST be present with `status = "pending"`.

RL1b. The lifecycle row MUST transition from `"pending"` to exactly one terminal status:

- `"success"` when the downstream client received a normal API response payload (including truncated/cutoff completion cases such as `finish_reason = "length"`, and including cases where the downstream client disconnected mid-stream after partial delivery),
- `"error"` only when the request ends with an API error response.

RL1c. Terminal logging MUST update the existing pending row for the same request identity (request ID + user scope). If no pending row exists (legacy/backward-compatible path), the terminal row MAY be inserted directly.

RL1d. Creating or updating `pending` status MUST NOT trigger any extra billing call. Request billing execution count MUST remain identical to pre-pending behavior (at most once per billable request outcome).

RL1e. When all provider attempts are exhausted (including the case where zero attempts exist), the pending row MUST still transition to `"error"`. The absence of a `last_failed_attempt` MUST NOT prevent finalization.

RL1f. On server startup, all request-log rows with `status = "pending"` MUST be transitioned to `status = "error"` with `error_code = "server_shutdown"` and `error_message = "interrupted by server restart"`. This cleanup MUST execute before the HTTP listener begins accepting connections.

RL1g. On receipt of SIGINT or SIGTERM, the server MUST initiate graceful shutdown: stop accepting new connections, allow in-flight requests to drain, then transition any remaining `"pending"` rows to `"error"` with the same fields as RL1f before process exit.

RL1h. For pass-through streaming requests, if the downstream client disconnects (the response channel closes) before the upstream stream completes, the stream adapter MUST stop consuming upstream events at the next iteration boundary. The request MUST finalize as `status = "success"` with whatever usage was accumulated up to the point of disconnection, and billing MUST execute normally on that accumulated usage.

RL1i. When a provider attempt is selected (upstream call succeeds or streaming begins), the pending row MUST be updated with `provider_id`, `channel_id`, `upstream_model`, and `provider_multiplier` immediately, before response processing or streaming starts. This update MUST NOT trigger billing and MUST NOT change the row's `status`.

RL2. Requests authenticated only by static config keys MUST NOT generate request logs.

RL3. Terminal log finalization (`pending -> success/error`) MUST be fire-and-forget (spawned asynchronously) and MUST NOT block the response to the client.

RL3a. Initial `pending` row creation MAY be executed before upstream forwarding starts to guarantee deterministic lifecycle transition.

RL4. For non-streaming requests, the log MUST include token usage from the upstream response. `ttfb_ms` MUST be null.

RL5. For streaming requests where response transforms require buffering (synthetic stream), the log MUST include token usage. `ttfb_ms` MUST record the time from `started_at` to the point where the upstream response body is received.

RL6. For pass-through streaming requests, `ttfb_ms` MUST record the time from `started_at` to the point where the first chunk is received from upstream.

RL6a. For pass-through streaming requests where usage cannot be extracted from streamed events, token usage fields MAY be omitted (set to null).

RL6b. For pass-through streaming requests where usage is extracted while the lifecycle row is still `status = "pending"`, Monoize MUST incrementally update the pending row usage fields (`prompt_tokens`, `completion_tokens`, `cached_tokens`, `reasoning_tokens`, `usage_breakdown_json`) using the latest cumulative usage snapshot.

RL6c. RL6b incremental pending updates MUST NOT execute billing deduction and MUST NOT replace terminal finalization (`pending -> success/error`).

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

RL17. When a request triggers waterfall fail-forward (one or more provider/channel attempts fail with retryable errors before a final result), `tried_providers_json` MUST record each failed attempt as `{ provider_id, channel_id, error }`. The array MUST be ordered chronologically (first attempt first). When no fallback occurred, the field MUST be null.

RL18. Active probe connectivity tests that can incur upstream token cost MUST be persisted as request logs with `request_kind = "active_probe_connectivity"`.

RL19. For active probe logs, `api_key_id` MUST be null and UI token column label MUST be rendered as a localized "Connectivity Test" string.

## 3. Dashboard endpoint

### 3.1 List request logs

- **Endpoint:** `GET /api/dashboard/request-logs`
- **Authorization:** Any authenticated dashboard user.
- **Query parameters:**
  - `limit: integer` (default 50, clamped to [1, 200])
  - `offset: integer` (default 0, clamped to >= 0)
  - `model: string?` (filter by model name; supports comma-separated list for multi-model OR matching, e.g. `"gpt-4o, gpt-5"`. Each entry is trimmed and matched via substring.)
  - `status: string?` (filter by status, exact match: `"pending"`, `"success"`, or `"error"`)
  - `api_key_id: string?` (filter by specific API key ID)
  - `username: string?` (filter by username, exact match via JOIN on `users.username`; only effective when the caller has admin role — non-admin callers ignore this parameter)
- `search: string?` (full-text search across model, upstream_model, request_id, request_ip)
  - `time_from: string?` (ISO 8601 / RFC 3339 timestamp; inclusive lower bound on `created_at`)
  - `time_to: string?` (ISO 8601 / RFC 3339 timestamp; exclusive upper bound on `created_at`)
- **Response:**

```json
{
  "data": EnrichedRequestLogRow[],
  "total": integer,
  "total_charge_nano_usd": string,
  "limit": integer,
  "offset": integer
}
```

Where `EnrichedRequestLogRow` = `RequestLogRow` + `username` + `api_key_name` + `channel_name` + `provider_name`.

RL-API6. `total_charge_nano_usd` MUST equal the SUM of `charge_nano_usd` across all rows matching the active filters (not just the current page). Rows with null `charge_nano_usd` MUST be treated as 0. The value MUST be a string representation of a non-negative integer (nano-dollar).

RL-API1. When the authenticated user has role `super_admin` or `admin`, the endpoint MUST return logs for ALL users. Otherwise, it MUST return only logs belonging to the current authenticated user.

RL-API2. Results MUST be ordered by `created_at DESC` (most recent first).

RL-API3. `total` MUST reflect the count of logs matching all active filters, not the page size.

RL-API4. Filter parameters are combined with AND logic.

RL-API5. For admin users applying `username` filter, rows with `request_kind = "active_probe_connectivity"` MUST remain included regardless of username value.

### 3.2 Admin-visible vs user-visible fields

The API returns the same enriched schema for all users. The frontend controls column visibility:

- **Admin-only columns:** `username`, `channel` (display text uses `provider_name` when available, otherwise falls back to `provider_id`; tooltip shows channel name and upstream model context)
- **All users see:** `created_at`, `request_id`, `model` (with ModelBadge), `api_key_name`, `duration_ms`/`ttfb_ms`/`is_stream` (merged badge group), `prompt_tokens`, `completion_tokens`, `charge_nano_usd`, `status`, `request_ip`, and error tooltip details (`error_code`, `error_message`, `error_http_status`) when `status = "error"`.

## 4. Storage

RL-S1. Request logs MUST be stored in table `request_logs`.

RL-S2. The table MUST have a composite index on `(user_id, created_at DESC)` for efficient pagination.

RL-S3. The `user_id` foreign key MUST cascade on delete.

RL-S4. New columns (`request_id`, `channel_id`, `ttfb_ms`, `request_ip`, `usage_breakdown_json`, `billing_breakdown_json`, `error_code`, `error_message`, `error_http_status`, `tried_providers_json`) MUST be added via `ALTER TABLE ADD COLUMN` statements in the migration logic. All new columns are nullable to preserve backward compatibility with existing rows.

RL-S5. `request_kind` MUST be added as a nullable `TEXT` column via migration logic, with null as backward-compatible default for existing rows.

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
  - **Status filter**: dropdown with options `All`, `Pending`, `Success`, `Error`.
  - **Token filter**: dropdown listing all of the user's API keys by name; selecting one filters by `api_key_id`.
  - **Username filter** (admin only): text input defaulting to the current user's username; applied on Enter or blur. Non-admin users do not see this control.
  - **Time range filter**: dropdown with preset options `All Time`, `Last 1 Hour`, `Last 24 Hours`, `Last 7 Days`, `Last 30 Days`, `Today`, `Yesterday`, `This Month`, `Last Month`. Selecting a preset computes `time_from` / `time_to` as ISO 8601 strings in the browser's local timezone and sends them as query parameters to the API.

FL7a. The filter-control area MUST display the total charge sum for the current filter conditions. The value MUST be formatted as regular USD currency with 6 fractional digits (e.g. `$1.234567`). The label MUST use the i18n key `requestLogs.totalCost`. The element MUST be displayed in the summary area (top-right) alongside the existing "Showing X-Y of Z" text.

FL8. Column order (left to right): `created_at`, `request_id` (with adjacent status indicator), `model` (ModelBadge), `api_key_name`, `[username]` (admin), `[channel]` (admin, with tooltip showing provider context), `duration/ttfb/stream` (merged badges), `prompt_tokens` (input), `completion_tokens` (output), `charge_nano_usd` (cost), `request_ip`.

FL9. For the admin channel column display value:

- If `provider_name` is non-empty, UI MUST render `provider_name` as the primary text.
- Else if `provider_id` is non-empty, UI MUST render `provider_id`.
- Else UI MUST render `-`.
- On hover, the tooltip MUST show `channel_name` (or `channel_id` as fallback) when available, and upstream model when it differs from the requested model.

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

FL25. The `charge_nano_usd` (Cost) column displayed value MUST use regular USD currency formatting with exactly 6 fractional digits (for example: `$0.000123`), and MUST NOT use threshold shorthand (for example: `<$0.0001`).

FL25a. The Cost column MUST NOT truncate visible cell text. The table layout MUST allow this column to expand with content when needed (while preserving horizontal overflow/scroll behavior for narrow viewports).

FL26. Hovering the `charge_nano_usd` (Cost) cell MUST show billing breakdown details sourced from `billing_breakdown_json`, including per-class expression `unit_price × token_count` and subtotal, plus multiplier/base/final charge.

FL27. Hovering the `prompt_tokens` (Input) and `completion_tokens` (Output) cells MUST show usage breakdown details sourced from `usage_breakdown_json`, including subtype token counts when available (for example: text, cached, cache creation/read, image, audio, reasoning).

FL28. For rows with `status = "error"`, hovering the request-id/status indicator MUST show error details from `error_code`, `error_message`, and `error_http_status` when present.

FL29. When `tried_providers_json` is non-empty, the request-id tooltip MUST additionally display the list of tried providers/channels with their error messages, separated from the main error details by a visual divider.

FL30. For rows where `request_kind = "active_probe_connectivity"` and `api_key_name` is null, the Token column MUST display a localized i18n label meaning "Connectivity Test".

FL31. The rightmost `request_ip` column MUST keep a trailing right inset equal to the leading left inset of the first (`created_at`) column, so IP text does not visually touch the table's right boundary.

FL32. Tooltip overlays for request-log table detail cells (request-id, model, token, channel, duration, input/output, cost) MUST render in a portal layer attached to `document.body` so overlay width/position is not constrained by table/cell/container layout width or overflow clipping.

FL33. On coarse-pointer devices (touch-first), those tooltip overlays MUST open on tap and close on outside tap, while preserving hover behavior on fine-pointer devices.

FL34. The `model` column MUST use a minimum width of 13.5 rem as its baseline and MUST be allowed to expand with content when long model identifiers are present.

FL35. In the logs table, model badge text in the `model` column MUST NOT be forcibly truncated. On narrow viewports, overflow MUST be handled by the table/container horizontal scrolling behavior rather than wrapping or clipping model badge text.

FL36. In the request-id status indicator, status-color mapping MUST be:

- `pending`: blue lamp,
- `success`: green lamp,
- `error`: red lamp.

FL37. The logs page MUST auto-refresh the newest page periodically so that `pending` rows can transition to terminal status without manual refresh.

FL38. While any tooltip-detail overlay in the request-logs table is open (request-id, model, token, channel, duration, input/output, cost):

- The periodic auto-refresh poll defined in FL37 MUST be paused (`isPaused` returns `true`).
- Any data updates that arrive from in-flight requests (started before the tooltip opened) or from SWR revalidation triggers (e.g. `revalidateOnFocus` when the browser tab regains focus) MUST be buffered and MUST NOT cause the table row list to re-render.
- When all tooltip overlays close, buffered data MUST be flushed and applied to the visible table immediately, and periodic polling MUST resume at the normal interval.
- This guarantee MUST hold on both fine-pointer (desktop hover) and coarse-pointer (mobile tap) devices.

FL39. The time-range filter popover MUST contain three vertical sections in this order: preset row, manual datetime inputs, single-month calendar.

FL40. The preset row MUST be horizontally scrollable when content overflows and MUST reserve scrollbar gutter space to avoid layout jump while scrolling.

FL41. The popover content width MUST equal the rendered calendar width for the currently displayed month. The datetime input rows MUST NOT expand popover width beyond calendar width.

FL42. The manual datetime inputs MUST be stacked in two rows (`from` then `to`) and accept second-precision format `yyyy-MM-dd HH:mm:ss`.

FL43. Time-range selection MUST be bidirectionally synchronized:

- selecting a preset MUST update manual inputs and calendar selection,
- selecting calendar range or committing manual inputs MUST activate the matching fixed preset (`today`, `yesterday`, `this_month`, `last_month`) when and only when the selected range matches that preset, otherwise no preset is active.

FL44. Active preset buttons (including `All Time`) MUST use a high-contrast foreground/background pair so text remains legible in both light and dark themes.
