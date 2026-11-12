# Mock Upstream Server Specification

## 0. Status

- **Purpose:** Define the behavior of the development mock upstream at `mock/server.ts`.
- **Scope:** The mock stands in for provider upstreams during local development,
  Playground verification (`playground.spec.md`), and differential smoke runs
  (`differential-smoke.spec.md`). It is not part of the release artifact.

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

MU5. Define the _reasoning trigger_: a request body activates reasoning output iff its
`model` string contains the substring `reasoning`.

MU5a. Text tokenization MUST preserve whitespace. Concatenating token deltas MUST
reproduce the complete source text, including leading and trailing whitespace.
Generated Responses and Messages text replies MUST report `input_tokens = 8` and
`output_tokens = 16`. Responses replies MUST also report `total_tokens = 24`.
Chat replies MUST report `prompt_tokens = 8`, `completion_tokens = 16`, and
`total_tokens = 24`. A Messages stream starts with `output_tokens = 0` and
reports the final cumulative `output_tokens = 16` in `message_delta`.
These counts are deterministic fixtures, not measured token counts.

## 3. `POST /v1/responses`

MU6. Input text is the concatenation of: a string `input` verbatim; else for each
`input[]` item, string items verbatim plus the `text`/`input_text` fields of `message`
item content parts, in source order.

MU7. Without `stream: true`, the endpoint MUST return HTTP 200 with a completed
`response` object whose `output` is one assistant `message` item containing one
`output_text` part with `echo(input text, body)` and whose `usage` follows MU5a.

MU8. With `stream: true` and the reasoning trigger inactive, the endpoint MUST emit
the following lifecycle, then `data: [DONE]`:

1. `response.created` with `status: "in_progress"` and empty `output`.
2. `response.output_item.added` with an in-progress assistant message
   (`id = "msg_mock_1"`, `output_index = 0`, empty `content`).
3. `response.content_part.added` with an empty `output_text` part
   (`content_index = 0`).
4. One `response.output_text.delta` per token of `echo(input text, body)`.
5. `response.output_text.done` and `response.content_part.done` with the full text.
6. `response.output_item.done` with the completed message.
7. `response.completed` with `status: "completed"`, the completed message in
   `response.output`, and MU5a usage in `response.usage`.

Each content event MUST carry the same `item_id`, `output_index`, and
`content_index` as its corresponding start event. Every JSON event MUST contain
a monotonically increasing `sequence_number`, starting at `0`.

MU9. With `stream: true` and the reasoning trigger active, the endpoint MUST emit an
OpenAI Responses lifecycle in this order, then `data: [DONE]`:

1. `response.created`
2. `response.output_item.added` with a `reasoning` item (`id = "rs_mock_1"`,
   `summary: []`)
3. `response.reasoning_summary_part.added` (`summary_index = 0`)
4. one `response.reasoning_summary_text.delta` per MU5a token of
   `Mock summary of: <input text>` (`summary_index = 0`)
5. `response.reasoning_summary_text.done`, `response.reasoning_summary_part.done`
6. `response.output_item.done` with the completed reasoning item (one `summary_text`
   entry holding the full summary text)
7. `response.output_item.added` with a `message` item (`id = "msg_mock_1"`)
8. `response.content_part.added`, one `response.output_text.delta` per
   MU5a token of `echo(input text, body)`,
   `response.output_text.done`, `response.content_part.done`
9. `response.output_item.done` with the completed message item
10. `response.completed` whose `response.output` contains the completed reasoning and
    message items and whose `response.usage` carries non-zero token counts

MU8 and MU9 MUST use the same message lifecycle and MU5a usage. The reasoning
item occupies `output_index = 0`, so the MU9 message occupies `output_index = 1`.
Every frame in MU8 and MU9 MUST carry the event name in both the `event:` line and
the `data:` JSON `type` field. MU9 sequence numbers follow MU8's rule.

## 3a. `GET /v1/responses` WebSocket

MU9a. `GET /v1/responses` MUST accept a WebSocket upgrade. After 101, each inbound text
frame MUST be one JSON object.

MU9b. A `response.create` object with `generate=false` is warmup. The mock MUST send
`response.created` then `response.completed` for one synthetic response whose `id`
starts with `resp_`, whose `output` is `[]`, and whose `model` equals the request
model. The mock MUST NOT require `stream`.

MU9c. Any other `response.create` MUST emit the same Responses JSON objects as the
matching `POST /v1/responses` SSE lifecycle (MU8 or MU9), each as one text frame,
without SSE `event:` wrapping and without a `[DONE]` frame. Exactly one
`response.completed` MUST terminate each generated response.

## 4. `POST /v1/chat/completions`

MU10. Without `stream: true`, the endpoint MUST return the tool-loop responses defined
by the `weather`/`websearch` fixture when both tool names are declared, else one
assistant message whose content is `echo(concatenated string message contents, body)`.
Every response, including every tool-loop turn, MUST carry MU5a usage.

MU11. With `stream: true`, the endpoint MUST emit a role chunk, one content delta per
MU5a token of `echo(...)`, a terminal chunk with
`finish_reason: "stop"` and usage, then `data: [DONE]`.

MU12. With `stream: true` and the reasoning trigger active, the endpoint MUST emit,
between the role chunk and the first content delta, one
`delta: { "reasoning_content": <token> }` chunk per MU5a token of
`Mock reasoning about: <concatenated string message contents>`.

## 5. Messages and images

MU13. `POST /v1/messages` MUST echo concatenated message text in Anthropic
Messages format. A string `message.content` contributes that string. An array
contributes each `type: "text"` block's string `text`, in source order.
The endpoint MUST apply MU4 to that text. A non-stream reply MUST contain one
assistant text block, `stop_reason: "end_turn"`, `stop_sequence: null`, and MU5a usage.

MU13a. A streaming Messages reply MUST emit this sequence:

1. `message_start` with an empty assistant message, `stop_reason: null`,
   `stop_sequence: null`, and initial MU5a usage.
2. `content_block_start` with an empty text block at index `0`.
3. One `content_block_delta` with `delta.type: "text_delta"` per token of MU13 text.
4. `content_block_stop` for the text block.
5. `message_delta` with `delta: { stop_reason: "end_turn", stop_sequence: null }`
   and final cumulative MU5a usage.
6. `message_stop`.

Every frame MUST carry the event name in both `event:` and the JSON `type` field.
The stream MUST NOT append `[DONE]`.

MU13b. With the reasoning trigger, the Messages stream MUST emit a thinking block
before the text block. The thinking block occupies index `0`; the text block
occupies index `1`. The thinking block MUST start with empty `thinking`, emit one
`thinking_delta` per token of `Mock reasoning about: <concatenated message text>`,
then emit `signature_delta` with `signature: "mock-thinking-signature"` and
`content_block_stop`. Non-stream Messages replies remain text-only.

MU13c. `POST /v1/messages/count_tokens` MUST return HTTP 200 with exactly
`{ "input_tokens": ceil(utf8_byte_length(text) / 4) }`. Concatenate system text
first, then all message text, without separators. Each text input follows MU13's
string-or-text-block rule. Missing text contributes an empty string.
The endpoint MUST NOT count models, tools, image blocks, or JSON syntax.

MU14. `/v1/images/generations` without `stream: true` MUST return one fixed orange
256x256 PNG as `b64_json` with `revised_prompt = "mock render of: <prompt>"` and
non-zero usage.

MU14a. `/v1/images/generations` with `stream: true` MUST return SSE: one
`image_generation.partial_image` frame per requested `partial_images` (integer,
default `0`, capped at `3`) with 0-based `partial_image_index` and the fixed PNG as
`b64_json`, then one `image_generation.completed` frame with the fixed PNG as
`b64_json`, `output_format: "png"`, and the MU14 usage object, then `data: [DONE]`.
Every frame MUST carry the event name in both the `event:` line and the `data:` JSON
`type` field.

MU15. `/v1/images/edits` MUST require at least one file part named `image` or
`image[]` (else HTTP 400) and, without text field `stream` equal to `true`, return
one fixed teal 256x256 PNG in the same shape as MU14.

MU15a. `/v1/images/edits` with text field `stream` equal to `true` MUST return SSE in
the MU14a shape with event names `image_edit.partial_image` /
`image_edit.completed`, the teal PNG, and `partial_images` read from the same-named
text field.

## 6. `POST /v1/systemone`

MU16. The body MUST be a JSON object. `questions` MUST be an object. A violation
MUST return HTTP 400. Otherwise the endpoint MUST return HTTP 200 with a JSON
object that contains `model`, `answers`, and `usage`.

MU17. `answers` MUST contain one property for each key of `questions`, in source
key order. The value depends on that question's `type`:

- `noul`: `{ "type": "noul", "noul": 0.5 }`.
- `choice`: `choice` is the first key of object `criteria`. `n` is the key count,
  or `1` when `criteria` is missing or empty. `probabilities` maps each criteria
  key to `1/n`. An empty criteria object yields `choice = null` and
  `probabilities = {}`. `confidence` is `1/n`.
- `score`: `n` is the number of keys in object `legend`, or the length of array
  `legend`. A missing legend uses `n = 1` and `legend = {}`. `score` is
  `(n - 1) / 2`. `probabilities` maps each legend key to `1/n`. Array legend keys
  are the decimal indexes `"0"`, `"1"`, and so on. `confidence` is `1/n`. The
  answer includes `legend`.
- Any other `type`: `{ "type": <type> }`.

MU18. `usage.input_tokens` MUST equal `ceil(utf8_byte_length / 4)`. The measured
text is the JSON encoding of an object whose keys are `state` then `questions`,
with no extra whitespace. A missing `state` is JSON `null`.
`usage.output_tokens` MUST equal the number of keys in `questions`.

MU19. When the request `model` string is exactly `jev-latest`, the response
`model` MUST be `jev-1.13.0`. Otherwise the response `model` MUST equal the
request `model` string. A missing `model` MUST become an empty string.
