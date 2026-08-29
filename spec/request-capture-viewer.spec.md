# Request Capture Viewer Specification

## 0. Status

- **Purpose:** Expose persisted request-capture dumps (`request-capture-dumps.spec.md`) to dashboard users through a per-request detail API and a capture viewer dialog on the logs page.
- **Scope:** Dashboard API `GET /api/dashboard/request-captures/{request_id}`, the `has_capture` indicator on request-log list rows, and the frontend capture viewer UI.
- **Compatibility:** Applies only to version-2 dumps that have a `request_capture_records` row. Version-1 dumps written before this feature have no metadata row and are not viewable (RCD-D2a).

## 1. Logs list capture indicator

RCV-L1. Every request-log row returned by `GET /api/dashboard/request-logs` MUST include `has_capture: boolean`.

RCV-L2. `has_capture` MUST be `true` iff at least one `request_capture_records` row exists with `request_id = row.request_id AND user_id = row.user_id`. The list query MUST compute this with an indexed `EXISTS` subquery (or an equivalent indexed lookup) against `request_capture_records` only. The list path MUST NOT open, stat, or read any dump file.

RCV-L3. In-memory SSE snapshots (`request-logs.spec.md` RL1a-1, FL46) are emitted before capture persistence completes, so SSE-delivered rows MUST carry `has_capture: false`.

RCV-L4. Because of RCV-L3, while SSE is connected the logs page MUST schedule one trailing-debounced newest-page SWR revalidation (debounce interval 1500 ms) whenever at least one SSE-delivered row with a terminal status (`success`, `error`, `client_gone`) arrives. This revalidation refreshes `has_capture` without user interaction. Pending-only SSE batches MUST NOT schedule it.

## 2. Capture detail API

### 2.1 Endpoint

- **Endpoint:** `GET /api/dashboard/request-captures/{request_id}`
- **Authorization:** Any authenticated dashboard user; per-record access rules in section 2.2.
- **Query parameters:**
  - `user_id: string?` — optional owner filter. When present, only capture records with that `user_id` are considered. Access rules are unchanged by this parameter.

RCV-A1. The handler MUST resolve candidate records by `request_id` (and `user_id` when supplied) from `request_capture_records`, ordered by `created_at_unix_ms DESC`, then `file_name DESC`.

RCV-A2. The handler MUST select the first candidate record the caller is authorized to view (section 2.2) and serve that record's dump file.

RCV-A3. The dump file MUST be read from disk on demand, only inside this handler. The read MUST occur on a blocking-capable executor.

RCV-A4. Success response body:

```json
{
  "request_id": string,
  "file_name": string,
  "created_at": "RFC3339 string",
  "size_bytes": integer,
  "owner": { "id": string, "username": string | null },
  "dump": object
}
```

`dump` is the parsed dump-file JSON after section 2.3 redaction.

### 2.2 Access rules

RCV-A5. A caller with role `user` is authorized only for records whose `user_id` equals the caller's user id.

RCV-A6. A caller with role `admin` or `super_admin` is authorized for a record iff:

1. the record's `user_id` equals the caller's user id, or
2. the record's owner user row exists and has role `user`.

An admin-role caller is therefore never authorized for another `admin`'s or `super_admin`'s records, and a record whose owner row was deleted is authorized only for its own user id.

RCV-A7. When no candidate record exists, or no candidate record is authorized, the endpoint MUST return HTTP `404` with code `capture_not_found`. Denied and absent captures MUST be indistinguishable in status code, error code, and message.

RCV-A8. When the selected record's dump file no longer exists on disk, the handler MUST delete that stale `request_capture_records` row, then continue with the next authorized candidate; if none remains, it MUST return the RCV-A7 response.

RCV-A9. When the dump file exists but is not parseable JSON, the endpoint MUST return HTTP `500` with code `capture_dump_unreadable`.

### 2.3 Transform-chain redaction

RCV-A10. When the caller's role is `user`, the handler MUST rewrite each attempt in `dump.attempts[]` before responding:

- `transform_chain` (when it is an array) MUST retain only entries with `scope == "api_key"`,
- the attempt MUST gain `hidden_transforms: integer` equal to the number of removed entries.

RCV-A11. When the caller's role is `admin` or `super_admin`, `transform_chain` MUST be returned unmodified and `hidden_transforms` MUST be `0` when the field is emitted.

RCV-A12. Redaction applies uniformly to own and foreign captures: it depends only on the caller's role.

## 3. Frontend entry point

RCV-F1. The logs table first column merges time and request id (`request-logs.spec.md` FL8): line 1 is `created_at`, line 2 is the request-id fragment with its status indicator.

RCV-F2. For rows with `has_capture == true`, line 2 of the first column MUST additionally render one square icon button, right-aligned within the column, using the lucide-react `ScanSearch` glyph. Rows with `has_capture != true` MUST NOT render the button.

RCV-F3. The button MUST have an accessible localized label (`requestLogs.capture.open`), a minimum 24x24 px hit target inside the dense table row, and MUST be reachable by keyboard.

RCV-F4. Activating the button MUST open the capture viewer dialog for that row, passing the row's `request_id` and `user.id`.

## 4. Capture viewer dialog

### 4.1 Data fetching

RCV-F5. The capture dump MUST be fetched through an SWR hook keyed by `(request_id, user_id)`. The SWR key MUST be `null` while the dialog is closed, so no fetch occurs before the dialog opens. Closing the dialog keeps the cache entry; reopening the same row MUST reuse the cached dump per normal SWR semantics.

RCV-F6. While the fetch is loading, the dialog body MUST render a skeleton fallback. A fetch error MUST render a localized error state with a retry action; it MUST NOT render an empty body.

### 4.2 Structure

RCV-F7. The dialog MUST use the shared `Dialog` primitive so it inherits the popup motion contract (`frontend-popup-motion.spec.md` PM3-PM13) without page-level animation code.

RCV-F8. The dialog header MUST show, when available: the full `request_id` (monospace), dump `created_at`, downstream protocol, stream/non-stream badge, and — when more than one attempt exists — an attempt selector listing `attempt_number`, provider id, and upstream model per attempt. Selecting an attempt switches all section 4.3 content to that attempt.

RCV-F9. The dialog MUST show a compact transform-chain strip for the selected attempt: one chip per `transform_chain` entry, in stored order, showing the transform id, with scope and phase distinguishable (label or badge styling). When `hidden_transforms > 0`, the strip MUST append one localized chip stating the number of hidden system transforms. When `transform_chain` is `[]` or `null` and `hidden_transforms` is absent or `0`, the strip MUST render a localized empty label.

RCV-F10. The content area MUST be a tab bar with these tabs for the selected attempt, in this order:

1. **Downstream request** (`raw_input`) — the request body received from the client,
2. **Upstream request** (`upstream_request`) — the body actually sent upstream,
3. **URP** (`transformed_urp_request`) — the transformed intermediate request,
4. **Response** — the non-streaming result of the attempt (RCV-F10a),
5. **Output Stream** — the captured downstream SSE frames (RCV-F10b).

RCV-F10a. The Response tab renders one JSON object chosen by this precedence:

1. `downstream_response` when non-null (non-streaming and buffered synthetic-stream attempts),
2. otherwise `reconstructed_urp_response` when non-null (the URP parser's non-stream reconstruction of a pass-through stream, `request-capture-dumps.spec.md` RCD-D10a),
3. otherwise the attempt `error` object when non-null,
4. otherwise a localized empty state: `requestLogs.capture.responseEmptyStream` when `downstream_sse_frames` is a non-null array (directing the user to the Output Stream tab), else `requestLogs.capture.empty`.

The Response tab MUST NOT render raw SSE frames.

RCV-F10b. The Output Stream tab trigger MUST be rendered iff the selected attempt's `downstream_sse_frames` is a non-null array. Its content is the frame list defined by RCV-F16 with the virtualization and highlighting rules of RCV-F16a and RCV-F16b. Dumps written before `reconstructed_urp_response` existed keep a working Output Stream tab and show the RCV-F10a empty state on the Response tab.

RCV-F11. Each tab MUST provide a copy button that copies the tab's full underlying content to the clipboard: pretty-printed JSON (2-space indent) for object tabs (for the Response tab, the object selected by RCV-F10a), newline-joined frame data for the Output Stream tab. Copy success MUST show transient localized feedback.

### 4.3 Content rendering

RCV-F12. JSON content MUST render as a collapsible tree with token-level syntax coloring using existing theme tokens: keys in the info color family, strings in the success color family, numbers in the warning color family, booleans and null in muted/destructive styling. All payload text MUST use the monospace font.

RCV-F13. The tree MUST support multi-level collapse: every object and array node with children is independently toggleable. Nodes at depth >= 3, arrays with more than 20 entries, and object values with more than 50 keys MUST start collapsed. Collapsed nodes MUST show a summary (`{…} n keys` / `[…] n items`).

RCV-F14. A string leaf longer than 400 characters MUST render truncated with a localized expand/collapse control; expanding never mutates the underlying data.

RCV-F14a. Inline image preview. A string leaf is image-bearing iff one of these holds:

1. it starts with `data:image/` followed by a media subtype and `;base64,`, or
2. it is at least 64 characters long and starts with one of these base64 image signatures: `iVBORw0KGgo` (PNG, preview media type `image/png`), `/9j/` (JPEG, `image/jpeg`), `R0lGOD` (GIF, `image/gif`), `UklGR` (WEBP, `image/webp`).

For every image-bearing string leaf, the JSON tree MUST render one inline image preview directly below the (RCV-F14-truncated) string text, built as an `img` element whose source is the string itself for case 1, or `data:{media_type};base64,{string}` for case 2. The preview MUST be bounded to at most 160 px height with preserved aspect ratio, MUST have a localized accessible alt text (`requestLogs.capture.imagePreview`), and MUST NOT mutate the underlying dump data. A payload the browser cannot decode MUST collapse the preview element without breaking the surrounding tree. Detection MUST inspect only the string prefix (no full-string decode before render).

RCV-F14b. Multipart capture objects (`request-capture-dumps.spec.md` RCD-D16) render through the same JSON tree: no dedicated widget is required. Their `data_base64` file-part values are covered by RCV-F14a case 2 when the bytes are an image, and by RCV-F14 truncation otherwise.

RCV-F15. Expand/collapse transitions MUST use non-linear easing from the project motion tokens (`easeOutExpo` enter, `easeInOutQuart` exit) via `framer-motion`; durations MUST be within `0.16s`-`0.30s`. Linear easing MUST NOT be used.

RCV-F16. The Output Stream view MUST render one row per captured frame, monospace, each row independently expandable when its content exceeds one line, with the frame index visible. When the attempt records frame truncation (`downstream_sse_frames_truncation.truncated == true`), the list MUST show a localized truncation notice with the omitted counts.

RCV-F16a. Virtualization: the frame list MUST be rendered through `react-virtuoso` inside a fixed-height scroll container, so only visible rows (plus overscan) are mounted regardless of frame count. Expanding a row changes only that row's measured height; it MUST NOT force rendering of off-screen rows.

RCV-F16b. Syntax highlighting: expanded frame content MUST be highlighted by a single-pass tokenizer with `O(n)` cost in the frame length and no backtracking. The tokenizer distinguishes: SSE field names at line starts (`event`, `data`, `id`, `retry`, terminated by `:`), JSON object keys, JSON string values, numbers, and `true`/`false`/`null` literals inside `data:` payloads. Token colors MUST reuse the RCV-F12 theme families (keys/fields info, strings success, numbers warning, booleans and null muted/destructive). Highlighting rules:

1. a frame longer than 4096 characters MUST render as plain unhighlighted text,
2. collapsed one-line previews MUST render as plain text (no tokenization cost for collapsed rows),
3. tokenization for a row MUST run only while that row is mounted (RCV-F16a) and MUST be memoized per frame string.

RCV-F16c. The highlighted output MUST preserve the frame text exactly: concatenating the rendered tokens reproduces the input string.

### 4.4 Responsiveness and accessibility

RCV-F17. On viewports narrower than the `sm` breakpoint, the dialog MUST occupy at least 92% of viewport width and its content area MUST remain vertically scrollable; tab triggers MUST remain reachable without horizontal viewport overflow.

RCV-F18. On `sm` and wider viewports, the dialog width MUST be capped (max-w-4xl) with an internal scroll area, per `frontend-popup-motion.spec.md` PM14.

RCV-F19. Tabs, the attempt selector, copy buttons, and collapse toggles MUST be operable by keyboard, and interactive controls MUST have accessible names.

RCV-F19a. If a JSON-tree collapse label wraps, its Chevron MUST align with the first rendered text line. A one-line label and its Chevron MUST share one visual vertical center.

## 5. Localization

RCV-F20. All user-facing strings introduced by this feature MUST use i18n keys under `requestLogs.capture.*` with translations in `en`, `ja`, `zh`, and `zh-TW`.
