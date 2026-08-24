# Upstream Error Sanitization Specification

## 0. Status

- **Purpose:** Define exactly which parts of an upstream failure are exposed to downstream API clients, which parts are persisted in request logs, which parts each dashboard viewer role may read back, and which parts exist only in the server log.
- **Scope:** Applies to every forwarding endpoint (`/v1/responses`, `/v1/chat/completions`, `/v1/messages`, the Image API, compact, and the Responses WebSocket) for errors derived from upstream attempts, to the request-log persistence of those errors, to the dashboard request-log read paths that return those errors, and to mid-stream downstream error frames.
- **Disclosure tiers.** There are exactly three disclosure tiers:
  1. **Client tier** — downstream API responses and mid-stream frames: masked or generic text only (sections 2, 4, 6).
  2. **Admin tier** — persisted request-log fields as read back by a dashboard user whose role satisfies `request-logs.spec.md` RL-API1 admin access (`admin` or `super_admin`): the full raw upstream detail, bounded only by `TRUNC` (sections 3, 5, 8).
  3. **Non-admin dashboard tier** — persisted request-log fields as read back by any other dashboard user: `MASK` applied at read time (section 8).
  The server tracing log additionally carries the full unbounded raw detail (SAN-3).
- **Reference alignment:** The client tier follows the New API reference implementation (`QuantumNous/new-api`): every client-facing relay error message is masked (`kitutil.MaskSensitiveInfo`), transport failures are replaced by one fixed generic message, and an unparseable upstream error body is never echoed to a client. Monoize deliberately deviates from New API for persisted error-log text: Monoize persists the truncated raw detail so administrators can read the complete upstream error, and enforces masking for non-admin viewers at read time instead of at write time.
- **Relation to other specs:** `monoize-upstream-routing.spec.md` RTA-8/RTA-8a define when the exhausted-routing error is returned; this spec defines its text. `request-logs.spec.md` RL17 defines which attempt fields are persisted and RL-API14 mirrors the read-time disclosure rule; this spec defines the error-text content of those fields. `unified_responses_proxy.spec.md` FP4e renders the client message defined here.

## 1. Definitions

SAN-D1. `MASK(text)` is a pure function on strings. It applies the following four rewrites, in this order, to every non-overlapping match, scanning left to right:

1. **URL masking.** Every substring matching the regex `(http|https)://[^\s/$.?#].[^\s]*` is parsed as a URL. If parsing fails, the substring is kept unchanged. If parsing succeeds, the substring is replaced by `{scheme}://{MASKHOST(host)}{port?}{path'}{query'}` where:
   - `MASKHOST(host)` splits `host` on `.`; if it has fewer than 2 labels the result is `***`; otherwise the result is `***.` followed by the preserved tail. The preserved tail is the last two labels when the last label has length 2 and the second-to-last label has length <= 3 (country-code TLD heuristic, e.g. `co.uk`), otherwise only the last label.
   - `port?` is `:{port}` when the URL carries an explicit port, otherwise empty.
   - `path'`: when the URL path is empty or `/`, `path'` equals the path unchanged; otherwise every `/`-separated non-empty path segment is replaced by `***` and segments are re-joined with `/` after a leading `/`.
   - `query'`: when the URL has no query, `query'` is empty; otherwise every `key=value` pair is replaced by `key=***`, pairs re-joined with `&` after a leading `?`.
2. **Bare domain masking.** Every remaining substring matching `\b(?:[a-zA-Z0-9](?:[a-zA-Z0-9-]{0,61}[a-zA-Z0-9])?\.)+[a-zA-Z]{2,}\b` is replaced by `N` copies of `***.` followed by the preserved tail as in `MASKHOST`, where `N = label_count - tail_label_count`, with a minimum of one `***.` prefix.
3. **IPv4 masking.** Every substring matching `\b(?:\d{1,3}\.){3}\d{1,3}\b` is replaced by `***.***.***.***`.
4. **API-key masking.** Every substring matching `(['"]?)api_key:([^\s'"]+)(['"]?)` is replaced by `{quote}api_key:***{quote}`.

SAN-D2. `DETAIL_LIMIT = 2048`. `TRUNC(text)` equals `text` when `text` contains at most `DETAIL_LIMIT` Unicode scalar values; otherwise it equals the first `DETAIL_LIMIT` scalar values of `text` followed by the literal suffix `... (truncated)`.

SAN-D3. Every `UpstreamCallError` carries a `source` classification with exactly these values:

- `transport`: the upstream HTTP request could not be sent or its response body could not be read (connection, TLS, DNS, timeout, and body-read failures).
- `structured_body`: the upstream returned a non-2xx status and the response body parsed as JSON containing a non-empty `error.message` string.
- `unparsed_body`: the upstream returned a non-2xx status and a non-empty body without a parseable `error.message`.
- `empty_body`: the upstream returned a non-2xx status and an empty body.
- `internal`: a Monoize-generated diagnostic (missing provider configuration, request-encoding failure, or JSON-decoding failure of a 2xx body).

Constructors default `source` to `transport` for network-kind errors and `internal` for HTTP-kind errors; the non-2xx response path sets `structured_body`, `unparsed_body`, or `empty_body` explicitly.

## 2. Per-attempt error conversion

SAN-1. When a failed upstream attempt is converted to an `AppError` (`upstream_error_to_app`), with `STATUS` being the upstream HTTP status or `502 Bad Gateway` when absent, the client-facing `message` MUST be exactly:

- `source = transport`: the fixed string `failed to request upstream`.
- `source = unparsed_body` or `empty_body`: `upstream status {STATUS}` (the Display form of the status, e.g. `upstream status 502 Bad Gateway`), with no body content.
- `source = structured_body` or `internal`: `upstream status {STATUS}: ` followed by `MASK(raw message)`.

SAN-2. The same `AppError` MUST set `internal_message` to `upstream status {STATUS}: ` followed by `TRUNC(raw message)`. `MASK` MUST NOT be applied to `internal_message`; it is the admin-tier detail and its read-time disclosure is governed by section 8.

SAN-3. Before the conversion in SAN-1, the raw unmasked detail (including transport error text with the full upstream URL and the raw unparsed error body) MUST be written to the server log (tracing, `warn` level) without truncation. The raw unmasked detail MUST NOT appear in any downstream response body or mid-stream frame. Persisted request-log fields carry the `TRUNC`-bounded raw detail per SAN-2, SAN-5, SAN-9, and SAN-10; disclosure of those fields to dashboard viewers is governed by section 8. Request-capture dump files (`request-capture-dumps.spec.md`) are server-local operator artifacts and are exempt.

SAN-4. When a 2xx upstream response embeds a Chat Completions error object (`embedded_chat_completion_error_to_app`), the resulting `AppError.message` MUST be `MASK` of the embedded message, and `AppError.internal_message` MUST be `TRUNC` of the raw embedded message.

## 3. Attempt recording

SAN-5. Each recorded failed attempt (`TriedProvider`) MUST carry two error strings:

- `error`: the persisted internal detail. It MUST equal `AppError.internal_message` when set, otherwise `TRUNC(AppError.message)`. `MASK` MUST NOT be applied to `error` at write time; read-time disclosure is governed by section 8.
- `client_error`: the client-facing text, equal to `MASK(AppError.message)`. `client_error` MUST NOT be serialized into `tried_providers_json`.

`MASK` is applied unconditionally to `client_error` because attempt failures can also originate from response-decoding `AppError`s that do not pass through SAN-1; masking is idempotent, so re-masking an already-sanitized client message is a fixed point.

## 4. Exhausted-routing downstream error

SAN-6. The downstream `message` of the exhausted-routing error (`monoize-upstream-routing.spec.md` RTA-8) MUST be exactly:

- zero recorded attempts: `No available upstream provider for model: {model}`.
- one or more recorded attempts: `All upstream attempts failed for model: {model}. Last error: {client_error of the last recorded attempt}`.

The downstream message MUST NOT contain the attempt count, provider identifiers, channel identifiers, or upstream URLs.

SAN-7. The exhausted-routing error MUST set `internal_message` to:

- zero recorded attempts: equal to the downstream message.
- one or more recorded attempts: `All {n} upstream attempt(s) failed for model: {model}. Last error: {error of the last recorded attempt}`, where `n` is the recorded attempt count and `{error}` is the unmasked internal detail per SAN-5. Consequently the admin-tier request-log message includes the full last-attempt detail.

SAN-8. Under the RTA-8a exception (`upstream_code = "thinking_signature_invalid"`), the downstream message MUST equal the last recorded attempt's `client_error` and `internal_message` MUST equal the last recorded attempt's `error`.

## 5. Request-log persistence

SAN-9. A terminal error request-log row MUST persist `error_message = AppError.internal_message` when set, otherwise `AppError.message`. The streaming terminal-error log path MUST apply the same rule. Consequently a persisted `error_message` MAY contain raw upstream URLs, bare domains, IPv4 addresses, and `api_key:` values, bounded only by `TRUNC`. The stored value is the admin-tier text; disclosure to dashboard viewers is governed by section 8.

SAN-10. Each `tried_providers_json` entry's `error` field MUST equal the attempt's `error` string per SAN-5 (unmasked, truncated internal detail).

## 6. Mid-stream error frames

SAN-11. When a downstream stream encoder renders a `UrpStreamEvent::Error` into a downstream frame (Chat Completions terminal error `data:` frame, Anthropic Messages `error` event, or Responses `response.failed` payload), the rendered `message` string MUST be `MASK(event message)`. This rule covers decoder-origin mid-stream failures whose text never passes through SAN-1.

## 7. Diagnostic fields

SAN-12. The structured diagnostic fields `upstream_status`, `upstream_code`, `upstream_type`, and `upstream_param` remain exposed downstream unchanged, as required by RTA-8. They carry enumerated upstream error metadata, not free-form infrastructure text.

## 8. Read-time disclosure of persisted error detail

The request-log read surfaces are `GET /api/dashboard/request-logs` (REST list) and `GET /api/dashboard/request-logs/stream` (SSE). Both surfaces serialize `error_message` (as `error.message`) and `tried_providers[].error`.

SAN-13. When the authenticated dashboard caller's role satisfies the RL-API1 admin predicate (`admin` or `super_admin`), both read surfaces MUST return `error.message` and every `tried_providers[].error` exactly as stored: the full raw detail bounded only by `TRUNC`, with no `MASK` applied.

SAN-14. For every other authenticated dashboard caller, both read surfaces MUST replace, before serialization, `error.message` with `MASK(stored error_message)` and each `tried_providers[].error` with `MASK(stored error)`. The replacement applies to REST list rows, to the initial SSE pending batch, and to every live SSE `log_batch` row. The stored row MUST NOT be modified.

SAN-15. SAN-14 operates on the stored text, which is `TRUNC`-bounded at write time. Because `MASK` is idempotent, applying SAN-14 to a historical row whose stored text was masked at write time (rows persisted before this policy) yields the stored text unchanged.

SAN-16. No non-dashboard API may return persisted request-log error text. The forwarding endpoints return only the client-tier messages defined in sections 2, 4, and 6.

## 9. System setting: mask sensitive info

SAN-CFG1. System settings MUST include `monoize_mask_sensitive_info: boolean`.

SAN-CFG2. The default value of `monoize_mask_sensitive_info` MUST be `true`.

SAN-CFG3. `monoize_runtime` MUST publish `mask_sensitive_info` equal to the committed `monoize_mask_sensitive_info`. Forwarding and dashboard request-log read paths that apply `MASK` MUST read this runtime value and MUST NOT query `system_settings` per request.

SAN-CFG4. When `mask_sensitive_info` is `true`, sections 2, 3 (`client_error`), 4, 6, and 8 apply unchanged.

SAN-CFG5. When `mask_sensitive_info` is `false`:

1. Every call site that would apply `MASK` (SAN-1 structured/internal client text, SAN-4, SAN-5 `client_error`, SAN-11, SAN-14) MUST leave the text unchanged (identity).
2. SAN-1 transport client message MUST be `upstream status {STATUS}: ` followed by `TRUNC(raw message)` instead of the fixed string `failed to request upstream`.
3. SAN-1 `unparsed_body` client message MUST be `upstream status {STATUS}: ` followed by `TRUNC(raw message)` instead of status-only text.
4. SAN-1 `empty_body` client message remains `upstream status {STATUS}`.
5. SAN-14 MUST NOT run: non-admin dashboard callers receive the stored admin-tier text verbatim.
6. SAN-2, SAN-5 `error`, SAN-7, SAN-9, and SAN-10 remain unchanged (persisted detail stays unmasked and `TRUNC`-bounded).
7. SAN-3 server tracing remains unchanged.

SAN-CFG6. Changing `monoize_mask_sensitive_info` via `PUT /api/dashboard/settings` MUST take effect for subsequent forwarding and dashboard reads after the settings transaction publishes `monoize_runtime` (DB23b). Already-persisted request-log rows are not rewritten.
