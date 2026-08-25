# Mock Upstream Server Specification

## 0. Status

- **Purpose:** Define the behavior of the development mock upstream at `mock/server.ts`.
- **Scope:** The mock stands in for provider upstreams during local development, SDK
  verification (`sdk-live-responses-contract.spec.md`), and Playground verification
  (`playground.spec.md`). It is not part of the release artifact.

## 1. Runtime

MU1. `bun run server.ts` in `mock/` MUST serve HTTP on the port from `PORT`, defaulting
to `4010`.

MU2. `GET /health` MUST return HTTP 200 with JSON body `{ "ok": true }`.

MU3. Any request not matched by a rule below MUST return HTTP 404 with JSON body
`{ "error": "not found" }`.

## 2. Echo semantics

MU4. Define `echo(text, body)` as `text` plus the suffix `|extra_echo=<v>` when
`body.extra_echo` is a non-empty string, else plus `|unparsed_field=<v>` when
`body.unparsed_field` is a non-empty string, else plus nothing.

MU5. Define the *reasoning trigger*: a request body activates reasoning output iff its
`model` string contains the substring `reasoning`.

## 3. `POST /v1/responses`

MU6. Input text is the concatenation of: a string `input` verbatim; else for each
`input[]` item, string items verbatim plus the `text`/`input_text` fields of `message`
item content parts, in source order.

MU7. Without `stream: true`, the endpoint MUST return HTTP 200 with a completed
`response` object whose `output` is one assistant `message` item containing one
`output_text` part with `echo(input text, body)`.

MU8. With `stream: true` and the reasoning trigger inactive, the endpoint MUST emit
exactly one `response.output_text.delta` frame carrying `echo(input text, body)`,
then `data: [DONE]`.

MU9. With `stream: true` and the reasoning trigger active, the endpoint MUST emit an
OpenAI Responses lifecycle in this order, then `data: [DONE]`:

1. `response.created`
2. `response.output_item.added` with a `reasoning` item (`id = "rs_mock_1"`,
   `summary: []`)
3. `response.reasoning_summary_part.added` (`summary_index = 0`)
4. one `response.reasoning_summary_text.delta` per whitespace-delimited token of
   `Mock summary of: <input text>` (`summary_index = 0`)
5. `response.reasoning_summary_text.done`, `response.reasoning_summary_part.done`
6. `response.output_item.done` with the completed reasoning item (one `summary_text`
   entry holding the full summary text)
7. `response.output_item.added` with a `message` item (`id = "msg_mock_1"`)
8. `response.content_part.added`, one `response.output_text.delta` per
   whitespace-delimited token of `echo(input text, body)`,
   `response.output_text.done`, `response.content_part.done`
9. `response.output_item.done` with the completed message item
10. `response.completed` whose `response.output` contains the completed reasoning and
    message items and whose `response.usage` carries non-zero token counts

Every frame in MU9 MUST carry the event name in both the `event:` line and the `data:`
JSON `type` field.

## 4. `POST /v1/chat/completions`

MU10. Without `stream: true`, the endpoint MUST return the tool-loop responses defined
by the `weather`/`websearch` fixture when both tool names are declared, else one
assistant message whose content is `echo(concatenated string message contents, body)`.

MU11. With `stream: true`, the endpoint MUST emit a role chunk, one content delta per
whitespace-delimited token of `echo(...)`, a terminal chunk with
`finish_reason: "stop"` and usage, then `data: [DONE]`.

MU12. With `stream: true` and the reasoning trigger active, the endpoint MUST emit,
between the role chunk and the first content delta, one
`delta: { "reasoning_content": <token> }` chunk per whitespace-delimited token of
`Mock reasoning about: <concatenated string message contents>`.

## 5. `POST /v1/messages`, `POST /v1/images/generations`, `POST /v1/images/edits`

MU13. `/v1/messages` MUST echo concatenated `text` blocks (stream and non-stream) in
Anthropic Messages format.

MU14. `/v1/images/generations` MUST return one fixed orange 256x256 PNG as `b64_json`
with `revised_prompt = "mock render of: <prompt>"` and non-zero usage.

MU15. `/v1/images/edits` MUST require a file field `image` (else HTTP 400) and return
one fixed teal 256x256 PNG in the same shape as MU14.
