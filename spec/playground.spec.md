# Playground Specification

## 0. Status

- Product name: Monoize.
- Scope: interactive chat-completion playground accessible at `/dashboard/playground`.

## 1. Purpose

The Playground provides a UI for composing arbitrary chat-completion message histories, sending them to the local Monoize instance's `/api/v1/chat/completions` endpoint, and viewing streamed model output inline. It is primarily a debugging and testing tool.

## 2. Authentication

PG-AUTH1. The Playground MUST authenticate against the Monoize proxy using a user-owned API key (prefix `sk-`), NOT the dashboard session token.

PG-AUTH2. The user MUST select an API key from their existing API keys (fetched via the dashboard `/api/dashboard/tokens` endpoint). If the user has no API keys, the Playground MUST display a prompt to create one.

PG-AUTH3. The selected API key identifier MUST be persisted in `localStorage` under the key `playground_api_key_id` so it survives page reloads.

## 3. Model Selection

PG-MODEL1. The model field MUST be a free-text input (combobox-style), since the set of routable models is determined by provider configuration and may not be enumerable from the frontend.

PG-MODEL2. The selected model MUST be persisted in `localStorage` under the key `playground_model`.

## 4. Message History

PG-MSG1. The message list MUST be an ordered array of objects with shape `{ role: string, content: string }`.

PG-MSG2. Supported `role` values for user-composed messages: `"system"`, `"user"`, `"assistant"`.

PG-MSG3. Each message row MUST provide:
- A role selector (select/dropdown).
- A multi-line text area for content.
- A delete button to remove the row.

PG-MSG4. The user MUST be able to add a new message row at the bottom of the list.

PG-MSG5. Default initial state: one `system` message (empty content) and one `user` message (empty content).

PG-MSG6. Messages with empty `content` MUST be excluded from the request payload (but the row remains visible in the UI for editing).

## 5. Request Execution

PG-REQ1. On "Send", the frontend MUST issue `POST /api/v1/chat/completions` with:
```json
{
  "model": "<selected model>",
  "messages": [ ...non-empty messages... ],
  "stream": true
}
```

PG-REQ2. The `Authorization` header MUST be `Bearer <full API key value>`.

PG-REQ3. Streaming MUST use the SSE protocol (`text/event-stream`). The client parses `data:` lines, accumulates `choices[0].delta.content` fragments, and renders them in real-time.

PG-REQ4. A `[data: DONE]` sentinel or stream closure terminates accumulation.

PG-REQ5. While streaming, the "Send" button MUST change to a "Stop" button that aborts the in-flight request via `AbortController`.

PG-REQ6. After streaming completes (or is aborted), the accumulated assistant response MUST be appended to the message list as a new `assistant` message row, and an empty `user` message row MUST be appended after it for continued conversation.

## 6. Parameter Controls

PG-PARAM1. The following optional parameters MUST be exposed:
- `temperature`: number input, range `[0, 2]`, default `1`, step `0.1`.
- `max_tokens`: number input, minimum `1`, no default (omitted if empty).

PG-PARAM2. These parameters are included in the request body only when explicitly set by the user.

## 7. Error Handling

PG-ERR1. If the request fails (non-2xx response or network error), the error MUST be displayed inline as a new `assistant` message row with content prefixed by `[Error] `. A subsequent empty `user` message row MUST be appended for continued conversation. No toast notification is shown for request errors.

PG-ERR2. If the user aborts the request, any accumulated partial response is appended as a normal `assistant` message. No error message is shown for user-initiated aborts.

## 8. UI Layout

PG-UI1. The page MUST follow the standard dashboard page patterns: `PageWrapper`, `text-3xl font-bold tracking-tight` heading, `motion` entry animations.

PG-UI2. Layout: single-column. Top: model selector + API key selector + parameter controls. Middle: scrollable message list. Bottom: action buttons (Add Message, Send/Stop).

PG-UI3. The assistant's streaming response MUST be rendered below the message list in a visually distinct area (e.g., card with different background) that updates in real-time as tokens arrive.

## 9. Constraints

PG-C1. The Playground MUST NOT modify any backend state; it only reads API keys and sends proxy requests.

PG-C2. The Playground MUST NOT store or cache API key secret values beyond what the API returns (the `key` field from the API key list, which is only the prefix).

PG-C3. Since the full API key secret is only available at creation time, the user MUST paste/enter the full `sk-...` key value manually. The API key selector assists by showing which key names are available, but the actual secret must be provided by the user. This value is stored in `localStorage` under `playground_api_key`.
