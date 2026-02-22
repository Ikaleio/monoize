# Unified Responses Proxy (Forwarding) Specification

## 0. Status

- **Product name:** Monoize.
- **Target implementation language:** Rust (stable).
- **Primary purpose:** Provide a single forwarding proxy that normalizes API differences between downstream request formats and heterogeneous upstream provider APIs.
- **Internal protocol:** Monoize defines an internal JSON protocol named **Universal Responses Protocol (URP-Proto)**.
  - **URP-Proto v0** request and response schemas are derived from the OpenAI **Responses API** create request/response schemas used by this server.
- **Scope of proxy features:**
  - Monoize MUST NOT execute tools locally.
  - Monoize MUST NOT persist response objects for later retrieval.
  - Monoize MUST NOT implement Files API, Vector stores API, or any local retrieval/indexing features.
- **Scope of dashboard features:**
  - Monoize MUST keep the dashboard HTTP API under `/api/dashboard/*` for managing users, API tokens, providers, and model registry records.
  - Dashboard UI MUST remove tool-related configuration and MCP-related configuration.

## 1. Terminology

- **Downstream:** The client calling Monoize.
- **Upstream:** A provider endpoint Monoize calls.
- **Provider:** A configured upstream channel.
- **Provider type:** One of `responses`, `chat_completion`, `messages`, `gemini`, `grok`, or `group`.
- **URP-Proto request:** A Responses-create-compatible JSON request (Monoize internal).
- **URP-Proto response:** A Responses-compatible JSON response object (Monoize internal).

## 2. External HTTP API surface

### 2.1 Authentication

A1. Monoize MUST require `Authorization: Bearer <token>` for all non-dashboard endpoints listed in §2.2.

A2. Monoize MUST map `<token>` to a tenant identity using dashboard-managed database API keys only.

### 2.1.1 Balance guard

A3. For requests authenticated by dashboard database API keys, Monoize MUST enforce user balance guard before forwarding:

- if `balance_unlimited=false` and `balance_nano_usd <= 0`, return `402 insufficient_balance`;
- if `balance_unlimited=true`, request MAY proceed regardless of balance value.

### 2.2 Endpoints implemented (forwarding)

Monoize MUST implement:

- `POST /v1/responses`
- `POST /v1/chat/completions` (adapter)
- `POST /v1/messages` (adapter)
- `POST /v1/embeddings` (pass-through)
- `GET /v1/models` (model listing)

Alias:

AP1. For every endpoint above, Monoize MUST also accept the same request at `/api` + endpoint path, with identical semantics.

### 2.3 Dashboard API

Monoize MUST implement dashboard endpoints under `/api/dashboard/*`.

The dashboard API is considered a separate subsystem; its behavior MUST remain consistent with the dashboard specs in `spec/` and with the frontend UI.

### 2.4 Provider presets

PP1. `GET /api/dashboard/presets/providers` MUST include at least these provider preset IDs:

- `openai_official`
- `anthropic_claude`
- `deepseek`
- `google_gemini`
- `xai_grok`

PP2. The preset `google_gemini` MUST use native Gemini base URL:

- `https://generativelanguage.googleapis.com/v1beta`

PP3. The preset `xai_grok` MUST use xAI base URL:

- `https://api.x.ai`

## 3. Unknown fields policy

F1. For URP-based downstream endpoints (`/v1/responses`, `/v1/chat/completions`, `/v1/messages`), Monoize MUST preserve unknown request keys under an internal `extra` map and forward them to the upstream request (§7.7).

F2. Known fields are identified by key name only. No type-checking reclassification is performed. If a known key's value has an unexpected type, the decoder handles it on a best-effort basis (e.g. ignoring unparseable values and leaving them in `extra`).

## 4. Runtime Parameters

C1. Monoize MUST NOT read forwarding/auth/provider/model-registry data from `config.yml` or `config.yaml`.

C2. Monoize MUST resolve database DSN by precedence:

1. `MONOIZE_DATABASE_DSN` environment variable, if set and non-empty.
2. `DATABASE_URL` environment variable, if set and non-empty.
3. default value `sqlite://./data/monoize.db`.

C3. Monoize MUST resolve listen address from `MONOIZE_LISTEN`, default `0.0.0.0:8080`.

C4. Monoize MUST resolve metrics endpoint path from `MONOIZE_METRICS_PATH`, default `/metrics`.

## 5. Forwarding pipeline (normative)

For each downstream request to any forwarding endpoint in §2.2, Monoize MUST execute the following pipeline:

FP1. **Parse:** Parse the downstream request into an internal URP-Proto request.

FP2. **Route:** Select an upstream provider for the request according to routing rules (§6).

FP3. **Adapt request:** Convert the URP-Proto request into the selected provider’s upstream request shape (§7).

FP4. **Call upstream:** Send the upstream request. If `stream=true`, Monoize MUST call the upstream in streaming mode.

FP5. **Adapt response:** Convert the upstream output (non-streaming or streaming chunks) into URP-Proto output.

FP6. **Render downstream:** Convert URP-Proto output into the downstream endpoint’s response shape (Responses / Chat Completions / Messages), streaming or non-streaming.

## 6. Routing rules

R1. Routing MUST follow `spec/database-provider-routing.spec.md`.

R2. Dashboard-managed providers/channels MUST be evaluated in configured provider order with fail-forward semantics.

R3. Streaming fallback MAY occur only before first downstream byte is emitted.

R4. If dashboard provider list is empty, routing MUST fail with `502 upstream_error`.

## 7. Adapters

### 7.1 URP-Proto (internal) request fields

Monoize MUST accept URP-Proto request fields consistent with Responses create requests, including at minimum:

- `model`
- `input`
- `tools`
- `tool_choice`
- `stream`
- `include`
- `max_output_tokens`
- `parallel_tool_calls`

T0. For internal URP-Proto requests, tool descriptors in `tools[]` MUST use Responses-style function-tool objects:

```json
{ "type": "function", "name": "tool_name", "description": "...", "parameters": { "type": "object", "properties": {} } }
```

T1. When a downstream adapter receives non-Responses tool descriptor shapes (for example Chat Completions `{"type":"function","function":{...}}` or Messages `{ "name": "...", "input_schema": ... }`), Monoize MUST normalize them to the internal URP-Proto shape defined by T0 before forwarding.

Stateful fields:

S1. Monoize MUST reject `background=true` with `400` code `background_not_supported`.

S2. Monoize MUST ignore `store` (treat it as absent).

S3. Monoize MUST ignore `conversation` and `previous_response_id` (treat them as absent).

### 7.1.1 URP-Proto (internal) tool-calling items

TCI1. URP-Proto `input` MAY contain non-message items that represent tool-calling state, using the following JSON objects:

- **Function call (tool call) input/output item:**

```json
{ "type": "function_call", "call_id": "call_x", "name": "tool_name", "arguments": "{\"k\":\"v\"}" }
```

- **Function call output (tool result) input item:**

```json
{ "type": "function_call_output", "call_id": "call_x", "output": "{\"result\":true}" }
```

TCI2. Monoize MUST NOT execute tools locally. Tool execution is always performed by the downstream client.

TCI3. When Monoize forwards a request, Monoize MUST forward any tool-calling items present in URP-Proto `input` by adapting them into the selected upstream provider’s request format (§7.2–§7.8).

### 7.1.2 URP-Proto (internal) reasoning item

RSN1. URP-Proto `output` MAY contain a reasoning item represented as:

```json
{ "type": "reasoning", "text": "...", "signature": "..." }
```

RSN2. `text` MUST represent human-readable reasoning text (if available).

RSN3. `signature` MUST contain an opaque provider-supplied string that can be used to correlate or verify reasoning (if available).

### 7.1.3 Reasoning-control normalization

RC1. Monoize MUST normalize reasoning effort to one of `none`, `minimum`, `low`, `medium`, `high`, or `xhigh`. The value `max` MUST be accepted as an alias for `xhigh` at decode time. When the selected upstream provider type is `messages`, `xhigh` MUST be mapped back to `max` for encoding.

RC2. Monoize MUST accept reasoning effort hints from any of the following downstream fields:

- Chat Completions style: top-level `reasoning_effort`.
- Responses style: top-level `reasoning.effort`.
- Messages style (legacy): top-level `thinking` object with `type="enabled"` (budget-based mapping is defined in RC4).
- Messages style (adaptive): top-level `thinking` object with `type="adaptive"` combined with `output_config.effort` (see RC4).

RC3. If multiple sources in RC2 are present, Monoize MUST use this precedence:

1. `reasoning_effort`
2. `reasoning.effort`
3. `thinking`

RC4. When the selected upstream provider type is:

- `chat_completion`: Monoize MUST send normalized effort as `reasoning_effort`.
- `responses`: Monoize MUST send normalized effort as `reasoning: { "effort": <level> }`.
- `messages`: Monoize MUST select the encoding based on the upstream model:
  - For models that support adaptive thinking (Claude Opus 4.6+, Sonnet 4.6+, and any future Claude model with major version ≥ 5): Monoize MUST send `thinking: { "type": "adaptive" }` combined with `output_config: { "effort": <level> }`.
  - For all other Anthropic models: Monoize MUST send `thinking: { "type": "enabled", "budget_tokens": N }`, where:
    - `low -> N=1024`
    - `medium -> N=4096`
    - `high -> N=16384`

RC5. If Monoize generated provider-native reasoning-control fields under RC4, Monoize MUST NOT forward conflicting source fields from `extra` to the same upstream request.

### 7.7 Extra field forwarding

XF1. For any downstream endpoint in §2.2, Monoize MUST preserve unknown fields according to §3 and store them in the internal URP request field named `extra`.

XF2. When constructing an upstream request body from a URP request, Monoize MUST insert every key-value pair from `extra` as a top-level JSON key in the upstream request body, **unless** that key is already present in the upstream request body due to adapter logic.

XF3. Adapter-generated keys MUST take precedence over keys from `extra` (i.e. `extra` MUST NOT overwrite adapter-generated keys).

XF4. Content-block-level unknown fields:

- When decoding a downstream request, Monoize MUST preserve unknown fields on individual content blocks (e.g. `cache_control` on a text block) into the corresponding URP part's `extra_body`.
- When encoding an upstream request, Monoize MUST merge each URP part's `extra_body` into the generated content-block JSON object, subject to the same precedence rule as XF3 (adapter-generated keys take precedence).
- This applies to all content-block types: `text`, `image`, `document`/`file`, `thinking`, `tool_use`, and `tool_result` blocks.
 This applies to system blocks, regular message content blocks, tool-result inner content blocks, and response content blocks.

XF5. Usage-level unknown fields:

 When parsing upstream usage objects, Monoize MUST capture unknown fields (e.g. `cache_creation_input_tokens`, `cache_write_tokens`, `prompt_tokens_details` sub-fields) into the URP `Usage.extra_body`.
 When encoding downstream usage objects for any downstream endpoint (`/v1/chat/completions`, `/v1/responses`, `/v1/messages`), Monoize MUST merge `Usage.extra_body` into the generated usage JSON, overwriting adapter-generated keys when present (the upstream's full detail objects take precedence over synthesized defaults).

### 7.2 Provider adapter: `type=responses`

PR1. Monoize MUST call the upstream path `POST /v1/responses`.

PR2. For non-streaming, Monoize MUST parse the upstream response as a Responses response object and convert it to URP-Proto output.

PR3. For streaming, Monoize MUST parse upstream SSE and convert it into Monoize downstream SSE format (§8) if the downstream endpoint is `POST /v1/responses`.

PR4. When constructing upstream `POST /v1/responses` requests, Monoize MUST emit `tools[]` in Responses-style function-tool shape (`type/name/parameters`) even if the downstream request used another tool schema.

PR5. When parsing upstream Responses SSE, Monoize MUST support canonical Responses event payloads where:

- text deltas are carried in `delta` for `response.output_text.delta`;
- tool-call items are nested under `item` for `response.output_item.added` / `response.output_item.done`;
- argument deltas identify the call via `output_index` (not necessarily `call_id`) for `response.function_call_arguments.delta`.

PR6. If upstream Responses streaming does not emit `response.output_text.delta` but emits assistant message text inside `response.output_item.added` and/or `response.output_item.done`, Monoize MUST reconstruct semantically equivalent downstream text streaming from those message items.

PR6a. For streaming translation from upstream `type=responses` (or `type=grok`, which uses Responses event shape) to downstream `POST /v1/chat/completions` or `POST /v1/messages`, Monoize MUST support completion-only fallback:

- If upstream sends `response.completed` with `output[]` items and Monoize has not emitted a given output class from earlier granular events, Monoize MUST synthesize the missing downstream stream items from `response.completed.output[]`.
- Output classes are:
  - assistant text (`type="message"` output text parts),
  - function/tool calls (`type="function_call"`),
  - reasoning (`type="reasoning"`).
- This fallback MUST only fill classes that were missing in the live stream; it MUST NOT duplicate classes already emitted from earlier upstream stream events.

PR7. For URP-Proto `type="function_call_output"` items with non-string `output`, Monoize MUST preserve multimodal output parts when forwarding to Responses upstream:

- text parts as `input_text`;
- image parts as `input_image`;
- file/document parts as `input_file`.

PR8. For upstream Responses requests, Monoize MUST parse `function_call_output.output` whether it is:

- string text; or
- an array/object content payload with `input_text`/`input_image`/`input_file`.

Parsed data MUST become a URP tool result message (`role=tool`, `type="function_call_output"` item + sibling multimodal parts) without dropping image/file parts.

### 7.3 Provider adapter: `type=chat_completion`

PC1. Monoize MUST call the upstream path `POST /v1/chat/completions`.

PC2. Monoize MUST convert URP-Proto `input` items into chat `messages[]` as described:

- URP `message` items become chat messages with the same `role` and text content.
- URP `function_call` items become assistant `tool_calls[]` entries (grouping consecutive items to preserve parallel calls).
- URP `function_call_output` items become chat `role="tool"` messages.

PC2.1. Input coercion for chat adapter:

- If URP `input` is a string, Monoize MUST treat it as one user message.
- If URP `input` is an object with message-like fields (`role`, `content`) but without explicit `type`, Monoize MUST treat it as one message item.
- If URP `input` is an array containing message-like objects without `type`, Monoize MUST treat each such object as a message item.

PC2.2. Content-block extra preservation for chat adapter:

- When encoding URP message parts to upstream chat `messages[].content[]` blocks, Monoize MUST merge each part's `extra_body` into the generated block object (same precedence rule as XF3).
- Monoize MAY collapse a single text block to scalar string `content` only when that block has no extra fields beyond adapter-generated keys.
- If a single text block contains any extra field (for example `cache_control`), Monoize MUST keep array/block form and MUST preserve that extra field in the encoded block.

PC3. Tool descriptor normalization:

- For `type=chat_completion` upstreams, Monoize MUST ensure upstream `tools[]` contains only `type=function` tool descriptors.
- For each URP tool with `type != "function"`, Monoize MUST convert it into a `type=function` tool with `function.name = <tool.type>` and a permissive JSON schema.

PC4. Monoize MUST convert chat-completions non-stream output into URP-Proto output items.

PC5. Monoize MUST convert chat-completions streaming deltas into URP-Proto output deltas.

PC6. Tool-calling (non-stream):

- If the upstream chat-completions response contains `choices[0].message.tool_calls[]`, Monoize MUST convert each entry into a URP-Proto `output` item with `type="function_call"` using:
  - `call_id = tool_calls[i].id`
  - `name = tool_calls[i].function.name`
  - `arguments = tool_calls[i].function.arguments` (string; if the upstream sends a JSON object, Monoize MUST serialize it as JSON)

PC7. Tool-calling (stream):

- If the upstream chat-completions stream contains `choices[0].delta.tool_calls[]`, Monoize MUST convert the deltas into a semantically equivalent URP-Proto stream such that:
  - the downstream Responses stream (if applicable) includes `response.function_call_arguments.delta` events; and
  - the downstream Messages/Chat-Completions stream (if applicable) includes tool-call deltas in their native formats.

PC7a. For downstream `POST /v1/chat/completions` translated from upstream `type=chat_completion` streaming:

- If upstream already emitted at least one terminal chunk with non-null `choices[0].finish_reason`, Monoize MUST NOT append an additional synthetic terminal chat chunk with a different `finish_reason`.
- Monoize MUST preserve upstream terminal finish semantics (for example `tool_calls`) so downstream clients can continue ReACT/tool loops correctly.

PC7b. If an upstream `type=chat_completion` stream emits any tool-call deltas (`choices[0].delta.tool_calls[]`) in a turn, but emits terminal `choices[0].finish_reason = "stop"`, Monoize MUST normalize downstream terminal finish reason to `tool_calls` for `POST /v1/chat/completions`.

PC8. Reasoning (non-stream and stream):

- Monoize MUST parse upstream Chat Completions reasoning from `choices[0].message.reasoning_details[]` and `choices[0].message.reasoning`.
- For `reasoning_details[]`, Monoize MUST interpret entries as follows:
  - `type="reasoning.text"`: `text` contributes to reasoning text; `signature` contributes to reasoning signature when present.
  - `type="reasoning.encrypted"`: `data` contributes to reasoning signature payload.
  - `type="reasoning.summary"`: `summary` contributes to reasoning text when no `reasoning.text` content is available.
- For streaming, Monoize MUST apply the same mapping to `choices[0].delta.reasoning_details[]` deltas in arrival order.
- Monoize MUST store parsed reasoning in URP-Proto as a `type="reasoning"` output item (§7.1.2).
- Backward compatibility: if `reasoning` / `reasoning_details` are absent, Monoize MUST still accept legacy `reasoning_content` / `reasoning_opaque` from upstream chat outputs.

PC9. For upstream `type=chat_completion` requests with `stream=true`, Monoize MUST request in-stream usage by setting `stream_options.include_usage = true` when the request does not already include `stream_options.include_usage`.

### 7.4 Provider adapter: `type=messages`

PM1. Monoize MUST call the upstream path `POST /v1/messages`.

PM2. Monoize MUST convert URP-Proto `input` items into Messages `messages[]` (role + text blocks).

PM2.1. Input coercion for Messages adapter:

- If URP `input` is a string, Monoize MUST treat it as one user text message.
- If URP `input` is an object with message-like fields (`role`, `content`) but without explicit `type`, Monoize MUST treat it as one message item.
- If URP `input` is an array containing message-like objects without `type`, Monoize MUST treat each such object as a message item.

PM3. Monoize MUST convert Messages output into URP-Proto output items.

PM4. Tool-calling:

- When the upstream Messages output contains `tool_use` blocks, Monoize MUST convert each block into a URP-Proto `output` item with `type="function_call"`.
- When a downstream Messages request contains `tool_result` blocks, Monoize MUST convert them into URP-Proto `input` items with `type="function_call_output"`.

PM4.1. When parsing downstream Messages `tool_result.content`, Monoize MUST support:

- string text content; and
- block-array content where blocks may include `text`, `image`, and `document`.

PM4.2. For PM4.1 block-array content, Monoize MUST map blocks to URP multipart tool result siblings:

- `text` -> text part;
- `image` -> image part;
- `document` -> file part.

PM4.3. When parsing upstream Messages assistant output, Monoize MUST support multimodal output blocks `image` and `document` in addition to `text` / `thinking` / `tool_use`.

PM5. Reasoning:

- When the upstream Messages output contains a `thinking` block, Monoize MUST convert it into a URP-Proto `output` item with `type="reasoning"` (§7.1.2).

PM6. Monoize MUST convert Messages streaming deltas into URP-Proto output deltas.

PM7. Messages `tool_choice` normalization:

- For downstream `POST /v1/messages`, Monoize MUST normalize Anthropic-style `tool_choice` values into URP-Proto-compatible `tool_choice` before forwarding.
- At minimum, Monoize MUST support:
  - `{ "type": "auto" }` -> `"auto"`
  - `{ "type": "any" }` -> `"required"`
  - `{ "type": "tool", "name": "<N>" }` -> `{ "type": "function", "function": { "name": "<N>" } }`

PM8. When calling a `type=messages` upstream, Monoize MUST send HTTP header `anthropic-version` with value `2023-06-01`.

PM9. For downstream `POST /v1/messages` streaming responses synthesized or translated by Monoize, Monoize MUST emit Anthropic-compatible message envelope events in this order:

1. `message_start`
2. zero or more `content_block_start` / `content_block_delta` / `content_block_stop`
3. one `message_delta`
4. `message_stop`

PM10. For PM9 streams, `message_start.message` MUST include at least:

- `id`
- `type = "message"`
- `role = "assistant"`
- `model`
- `content` (array)
- `stop_reason = null`
- `stop_sequence = null`
- `usage` object with token counters when available (or zeros when unavailable)

PM11. For PM9 streams, `message_delta.delta.stop_reason` MUST be:

- `"tool_use"` if any tool-use block was emitted;
- otherwise `"end_turn"`.

PM12. For PM9 streams, `message_delta.usage` token counters MUST be cumulative within the stream.

### 7.5 Provider adapter: `type=gemini`

PG1. Monoize MUST call Gemini native endpoints under base URL `https://generativelanguage.googleapis.com` using API version path selected by provider configuration (default `v1beta`).

PG2. For non-streaming requests, Monoize MUST call:

- `POST /<version>/models/{upstream_model}:generateContent`

PG3. For streaming requests, Monoize MUST call:

- `POST /<version>/models/{upstream_model}:streamGenerateContent?alt=sse`

PG4. Monoize MUST encode URP requests to Gemini native request fields:

- `contents[]` for conversation turns;
- `systemInstruction` for leading system/developer instruction;
- `generationConfig` for temperature/top_p/max_output_tokens;
- `tools[]` and `toolConfig.functionCallingConfig` for tool definitions and tool choice.

PG5. Monoize MUST decode Gemini responses from `candidates[].content.parts[]` and convert them to URP output parts, including:

- text parts;
- tool/function call parts;
- reasoning/thought parts and signatures when provided.

PG6. Monoize MUST map Gemini usage metadata to URP usage fields using:

- `promptTokenCount -> prompt_tokens`
- `candidatesTokenCount -> completion_tokens`
- `thoughtsTokenCount -> reasoning_tokens` when present.

PG7. Monoize MUST preserve unknown Gemini request/response fields in URP `extra_body` according to §3 and §7.7.

### 7.6 Provider adapter: `type=grok`

PX1. Monoize MUST call xAI Grok native Responses endpoint:

- `POST /v1/responses`

PX2. Monoize MUST encode URP requests to xAI Responses request fields (`model`, `input`, `tools`, `tool_choice`, `stream`, `parallel_tool_calls`, and optional provider-specific extras).

PX3. Monoize MUST decode xAI Responses output union items into URP output parts, including:

- message text;
- function/tool calls;
- reasoning items;
- encrypted reasoning content when available.

PX4. Monoize MUST support xAI function-call lifecycle fields:

- tool call item type `function_call` with `call_id`, `name`, and `arguments`;
- tool result input item type `function_call_output` with `call_id` and `output`.

PX5. Monoize MUST map xAI usage fields to URP usage fields, including reasoning and cached token counters when available.

PX6. Monoize MUST preserve unknown xAI request/response fields in URP `extra_body` according to §3 and §7.7.

### 7.7 Downstream adapter: `POST /v1/chat/completions`

DC1. Monoize MUST parse the downstream request as a Chat Completions create request and convert it into URP-Proto.

DC2. Monoize MUST forward using the pipeline in §5.

DC3. Monoize MUST render the result as a Chat Completions response (non-stream or SSE stream) based on the downstream request.

DC4. Tool-calling:

- For non-streaming responses, if URP-Proto `output` contains one or more `type="function_call"` items, Monoize MUST render them into `choices[0].message.tool_calls[]`.
- For streaming responses, Monoize MUST stream tool calls using `choices[0].delta.tool_calls[]` in a semantically equivalent manner (including parallel tool calls when multiple calls are present).
- For downstream requests, Monoize MUST parse:
  - `role="tool"` messages into URP-Proto `type="function_call_output"` input items; and
  - assistant messages with `tool_calls[]` into URP-Proto `type="function_call"` input items.

DC5. Reasoning:

- If URP-Proto `output` contains a `type="reasoning"` item, Monoize MUST:
  - render non-stream output to Chat Completions as:
    - `choices[0].message.reasoning` from URP reasoning `text`; and
    - `choices[0].message.reasoning_details[]` using OpenRouter reasoning item types (`reasoning.text` and/or `reasoning.encrypted`).
  - for streaming, emit `choices[0].delta.reasoning_details[]` chunks as reasoning deltas become available.
  - preserve reasoning stream lifecycle: each reasoning delta MUST be emitted as one chat chunk in arrival order, MAY interleave with text/tool-call chunks, and MUST terminate with the final finish chunk and `[DONE]`.
- Backward compatibility for downstream Chat Completions requests: Monoize MUST parse assistant-message reasoning from both OpenRouter fields (`reasoning`, `reasoning_details`) and legacy fields (`reasoning_content`, `reasoning_opaque`).

DC6. If the selected upstream provider type is `chat_completion` and the upstream response contains additional non-standard fields inside `choices[0].delta` or `choices[0].message` (other than fields explicitly mapped by DC4–DC5), Monoize MUST preserve those fields in the downstream response (streaming or non-streaming) for `POST /v1/chat/completions`.

DC7. For downstream `POST /v1/chat/completions` streaming responses synthesized from non-chat upstream event formats, Monoize MUST include a `usage` object in the terminal chat chunk when cumulative stream usage counters are available.

### 7.8 Downstream adapter: `POST /v1/messages`

DM1. Monoize MUST parse the downstream request as a Messages create request and convert it into URP-Proto.

DM1.1. For Messages downstream requests, Monoize MUST recognize and forward `parallel_tool_calls` when present and boolean-typed.

DM2. Monoize MUST forward using the pipeline in §5.

DM3. Monoize MUST render the result as a Messages response (non-stream or SSE stream) based on the downstream request.

DM4. Tool-calling:

- For non-streaming responses, if URP-Proto `output` contains `type="function_call"` items, Monoize MUST render them as Messages `tool_use` content blocks.
- For streaming responses, Monoize MUST stream tool calls as `content_block_start` blocks with `type="tool_use"` and tool input deltas.
- For downstream requests, Monoize MUST parse `tool_result` blocks into URP-Proto `type="function_call_output"` input items.

DM4.1. For downstream `tool_result` blocks that carry block-array content, Monoize MUST preserve image/file payloads when routing through URP-Proto and when encoding to eligible upstream formats.

DM5. Reasoning:

- If URP-Proto `output` contains a `type="reasoning"` item, Monoize MUST render it as a Messages `thinking` content block with:
  - `thinking = text`
  - `signature = signature`

DM6. For downstream `POST /v1/messages` streaming responses synthesized or translated from non-messages upstream event formats, Monoize MUST set `message_delta.usage` from cumulative stream usage counters when available.

### 7.9 Downstream endpoint: `POST /v1/embeddings`

DE1. Monoize MUST authenticate and apply pre-forward balance guard exactly as other forwarding endpoints.

DE2. Monoize MUST route provider/channel candidates using the same provider model map matching rules as chat completions.

DE3. Monoize MUST call upstream path `POST /v1/embeddings` for every selected provider attempt, regardless of provider type.

DE4. Monoize MUST forward request JSON as pass-through payload, except:

- replace outbound `model` with the selected `upstream_model`.

DE5. Monoize MUST require request field `input` to be either:

- one string; or
- an array whose every element is a string.

DE6. Monoize MUST parse upstream usage from `usage.prompt_tokens` and `usage.total_tokens`.

DE7. Billing tokens for embeddings MUST be:

- `prompt_tokens = usage.prompt_tokens`
- `completion_tokens = 0`

DE8. Downstream response body MUST preserve upstream payload structure, except top-level `model` MUST be rewritten to the logical model requested by the client.

DE9. Embeddings endpoint is non-streaming only.

### 7.10 Downstream endpoint: `GET /v1/models`

DMO1. Monoize MUST require forwarding bearer authentication for `GET /v1/models` exactly as defined in §2.1 and `spec/api-key-authentication.spec.md`.

DMO2. Monoize MUST load provider definitions from the dashboard-managed provider routing store.

DMO3. The response body MUST be JSON with the shape:

```json
{
  "object": "list",
  "data": [
    {
      "id": "<logical_model>",
      "object": "model",
      "created": 0,
      "owned_by": "monoize"
    }
  ]
}
```

DMO4. `data` MUST contain the set union of logical model keys across all configured providers.

DMO5. If a logical model key appears in 2 or more providers, `data` MUST include exactly one item for that key.

DMO6. `data` MUST be sorted by `id` in ascending lexicographic order.

DMO7. If the authenticated API key has `model_limits_enabled = true` and `model_limits` is non-empty, Monoize MUST filter `data` to include only models whose `id` is present in the `model_limits` list. If `model_limits_enabled` is false or `model_limits` is empty, no filtering is applied.

## 8. Streaming requirements (Responses downstream)

When the downstream endpoint is `POST /v1/responses` with `stream=true`, Monoize MUST respond using SSE and MUST emit:

- `response.created`
- `response.in_progress`
- `response.output_item.added` (at least one message item)
- `response.output_text.delta` (zero or more)
- `response.output_text.done`
- `response.output_item.done`
- `response.completed` or `response.failed`

STR1. Each SSE event `data` MUST be a JSON object containing:

```json
{ "sequence_number": 1, "data": { ... } }
```

STR2. `sequence_number` MUST be monotonically increasing starting from 1 within a single response stream.

STR3. For downstream `POST /v1/responses` streams synthesized from non-responses upstream event formats, Monoize MUST include a `usage` object in the terminal `response.completed` payload when cumulative stream usage counters are available.

## 9. Stream error termination

When a streaming request (`stream=true`) encounters an error — either before any upstream data is received (e.g. no available provider) or mid-stream (e.g. upstream connection failure) — Monoize MUST:

1. Emit a protocol-appropriate error event to the downstream client.
2. Emit a final `data: [DONE]` SSE event.
3. Close the SSE connection.

### 9.1 Pre-stream errors

SE1. If an error occurs before any data has been streamed (e.g. routing failure, no available provider), Monoize MUST return an SSE response (not a JSON error response) containing:

- For `POST /v1/chat/completions`: one `data` event with an OpenAI-compatible error JSON object, followed by `data: [DONE]`.
- For `POST /v1/responses`: one named `event: error` with a sequence-numbered error payload, followed by `data: [DONE]`.
- For `POST /v1/messages`: one named `event: error` with an Anthropic-compatible error object, followed by `data: [DONE]`.

### 9.2 Mid-stream errors

SE2. If an error occurs after streaming has begun (e.g. upstream disconnects, parse failure), Monoize MUST emit a protocol-appropriate error event followed by `data: [DONE]` on the existing SSE connection, then close it.

SE3. If the downstream channel sender is already closed (client disconnected), Monoize MAY silently discard the error event.
