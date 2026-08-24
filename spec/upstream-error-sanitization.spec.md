# Upstream Error Sanitization Specification

## 0. Status

- **Purpose:** Define exactly which parts of an upstream failure are exposed to downstream API clients, which parts are persisted in request logs, and which parts exist only in the server log.
- **Scope:** Applies to every forwarding endpoint (`/v1/responses`, `/v1/chat/completions`, `/v1/messages`, the Image API, compact, and the Responses WebSocket) for errors derived from upstream attempts, to the request-log persistence of those errors, and to mid-stream downstream error frames.
- **Reference alignment:** The disclosure policy follows the New API reference implementation (`QuantumNous/new-api`): every client-facing relay error message is masked (`kitutil.MaskSensitiveInfo`), transport failures are replaced by one fixed generic message, an unparseable upstream error body is never echoed to a client, persisted error-log text is masked, and full raw detail is written to the server log only.
- **Relation to other specs:** `monoize-upstream-routing.spec.md` RTA-8/RTA-8a define when the exhausted-routing error is returned; this spec defines its text. `request-logs.spec.md` RL17 defines which attempt fields are persisted; this spec defines the sanitization of their error text. `unified_responses_proxy.spec.md` FP4e renders the message defined here.

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

SAN-2. The same `AppError` MUST set `internal_message` to `upstream status {STATUS}: ` followed by `TRUNC(MASK(raw message))`.

SAN-3. Before the conversion in SAN-1, the raw unmasked detail (including transport error text with the full upstream URL and the raw unparsed error body) MUST be written to the server log (tracing, `warn` level). The raw unmasked detail MUST NOT appear in any downstream response body and MUST NOT be persisted in any request-log field. Request-capture dump files (`request-capture-dumps.spec.md`) are server-local operator artifacts and are exempt.

SAN-4. When a 2xx upstream response embeds a Chat Completions error object (`embedded_chat_completion_error_to_app`), the resulting `AppError.message` MUST be `MASK` of the embedded message.

## 3. Attempt recording

SAN-5. Each recorded failed attempt (`TriedProvider`) MUST carry two error strings:

- `error`: the persisted internal detail. It MUST equal `AppError.internal_message` when set, otherwise `TRUNC(MASK(AppError.message))`.
- `client_error`: the client-facing text, equal to `MASK(AppError.message)`. `client_error` MUST NOT be serialized into `tried_providers_json`.

`MASK` is applied unconditionally in both fields because attempt failures can also originate from response-decoding `AppError`s that do not pass through SAN-1.

## 4. Exhausted-routing downstream error

SAN-6. The downstream `message` of the exhausted-routing error (`monoize-upstream-routing.spec.md` RTA-8) MUST be exactly:

- zero recorded attempts: `No available upstream provider for model: {model}`.
- one or more recorded attempts: `All upstream attempts failed for model: {model}. Last error: {client_error of the last recorded attempt}`.

The downstream message MUST NOT contain the attempt count, provider identifiers, channel identifiers, or upstream URLs.

SAN-7. The exhausted-routing error MUST set `internal_message` to:

- zero recorded attempts: equal to the downstream message.
- one or more recorded attempts: `All {n} upstream attempt(s) failed for model: {model}. Last error: {error of the last recorded attempt}`, where `n` is the recorded attempt count.

SAN-8. Under the RTA-8a exception (`upstream_code = "thinking_signature_invalid"`), the downstream message MUST equal the last recorded attempt's `client_error` and `internal_message` MUST equal the last recorded attempt's `error`.

## 5. Request-log persistence

SAN-9. A terminal error request-log row MUST persist `error_message = AppError.internal_message` when set, otherwise `AppError.message`. The streaming terminal-error log path MUST apply the same rule. Consequently a persisted `error_message` MAY differ from the downstream client message but MUST NOT contain unmasked URLs, bare domains, IPv4 addresses, or `api_key:` values originating from upstream error text.

SAN-10. Each `tried_providers_json` entry's `error` field MUST equal the attempt's `error` string per SAN-5 (masked, truncated internal detail).

## 6. Mid-stream error frames

SAN-11. When a downstream stream encoder renders a `UrpStreamEvent::Error` into a downstream frame (Chat Completions terminal error `data:` frame, Anthropic Messages `error` event, or Responses `response.failed` payload), the rendered `message` string MUST be `MASK(event message)`. This rule covers decoder-origin mid-stream failures whose text never passes through SAN-1.

## 7. Diagnostic fields

SAN-12. The structured diagnostic fields `upstream_status`, `upstream_code`, `upstream_type`, and `upstream_param` remain exposed downstream unchanged, as required by RTA-8. They carry enumerated upstream error metadata, not free-form infrastructure text.
