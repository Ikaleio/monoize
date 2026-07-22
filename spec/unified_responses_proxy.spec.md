# Unified Responses Proxy (Forwarding) Specification

## 0. Status

- **Product name:** Monoize.
- **Target implementation language:** Rust (stable).
- **Primary purpose:** Provide one forwarding proxy that normalizes API differences between downstream request formats and heterogeneous upstream provider APIs.
- **Internal protocol:** Monoize defines an internal JSON protocol named **URP v2**.
  - The authoritative structural contract is `spec/urp-v2-flat-structure.spec.md`.
  - The authoritative transform contract is `spec/urp-transform-system.spec.md`.
- **Scope of proxy features:**
  - Monoize MUST NOT execute tools locally.
  - Monoize MUST NOT persist response objects for later retrieval.
  - Monoize MUST NOT implement Files API, Vector stores API, or any local retrieval or indexing features.
- **Scope of dashboard features:**
  - Monoize MUST keep the dashboard HTTP API under `/api/dashboard/*` for managing users, API tokens, providers, and model registry records.
  - Dashboard UI MUST remove tool-related configuration and MCP-related configuration.

## 1. Terminology

- **Downstream:** The client calling Monoize.
- **Upstream:** A provider endpoint Monoize calls.
- **Provider:** A configured upstream channel.
- **Provider type:** One of `responses`, `chat_completion`, `messages`, `gemini`, `openai_image`, or `group`.
- **URP v2 request:** The canonical internal request object `UrpRequestV2` defined by `spec/urp-v2-flat-structure.spec.md`.
- **URP v2 response:** The canonical internal response object `UrpResponseV2` defined by `spec/urp-v2-flat-structure.spec.md`.
- **Node sequence:** An ordered flat `Vec<Node>` sequence used by URP v2 `input` and `output`.
- **Ordinary node:** A top-level role-bearing node such as `Text`, `Image`, `Audio`, `File`, `Refusal`, `Reasoning`, `ToolCall`, or `ProviderItem`.
- **ToolResult node:** A distinct top-level `Node::ToolResult` with `call_id` correlation.
- **Control node:** The only control node kind is `next_downstream_envelope_extra`.
- **Canonical stream event:** One of `ResponseStart`, `NodeStart`, `NodeDelta`, `NodeDone`, `ResponseDone`, or `Error` as defined by `spec/urp-v2-flat-structure.spec.md`.
- **Consumable envelope:** One concrete downstream or upstream protocol object reconstructed by an encoder from one or more consecutive URP v2 nodes.

## 2. External HTTP API surface

### 2.1 Authentication

A1. Monoize MUST require API-key authentication for all non-dashboard endpoints listed in §2.2. Monoize MUST accept either `Authorization: Bearer <token>` or `x-api-key: <token>` as the downstream authentication header.

A2. Monoize MUST map `<token>` to a tenant identity using dashboard-managed database API keys only.

### 2.1.1 Balance guard

A3. For requests authenticated by dashboard database API keys, Monoize MUST enforce user balance guard before forwarding:

- if `balance_unlimited=false` and `balance_nano_usd <= 0`, return `402 insufficient_balance`;
- if `balance_unlimited=true`, request MAY proceed regardless of balance value.

### 2.2 Endpoints implemented (forwarding)

Monoize MUST implement:

- `POST /v1/responses`
- `POST /v1/responses/compact`
- `POST /v1/chat/completions` (adapter)
- `POST /v1/messages` (adapter)
- `POST /v1/embeddings` (pass-through)
- `GET /v1/models` (model listing)

Alias:

AP1. For every endpoint above, Monoize MUST also accept the same request at `/api` + endpoint path, with identical semantics.

### 2.3 Dashboard API

Monoize MUST implement dashboard endpoints under `/api/dashboard/*`.

The dashboard API is considered a separate subsystem. Its behavior MUST remain consistent with the dashboard specs in `spec/` and with the frontend UI.

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

F1. For URP-based downstream endpoints (`/v1/responses`, `/v1/chat/completions`, `/v1/messages`), Monoize MUST preserve unknown request keys under internal passthrough state and forward them to the upstream request under §7.6.

F2. Known fields are identified by key name only. No type-checking reclassification is performed. If a known key's value has an unexpected type, the decoder handles it on a best-effort basis, for example by ignoring an unparseable typed field and preserving the raw value in passthrough state when possible.

## 4. Runtime Parameters

C1. Monoize MUST NOT read forwarding, auth, provider, or model-registry data from `config.yml` or `config.yaml`.

C2. Monoize MUST resolve database DSN by precedence:

1. `MONOIZE_DATABASE_DSN` environment variable, if set and non-empty.
2. `DATABASE_URL` environment variable, if set and non-empty.
3. default value `sqlite://./data/monoize.db`.

C3. Monoize MUST resolve listen address from `MONOIZE_LISTEN`, default `0.0.0.0:8080`.

C4. Monoize MUST resolve metrics endpoint path from `MONOIZE_METRICS_PATH`, default `/metrics`.

C5. Monoize MUST accept downstream request bodies up to 50 MiB on forwarding endpoints (`/v1/responses`, `/v1/responses/compact`, `/v1/chat/completions`, `/v1/messages`, `/v1/embeddings`). Any framework-default extractor limit smaller than 50 MiB MUST be disabled so that the effective limit remains 50 MiB.

## 5. Forwarding pipeline (normative)

For each downstream request to any forwarding endpoint in §2.2, Monoize MUST execute the following pipeline:

FP1. **Parse downstream request:** Parse the downstream request into `UrpRequestV2`. The parser produces a flat ordered `Vec<Node>` in `request.input`. The parser MUST NOT introduce canonical grouped-message storage.

FP2. **Route:** Select an upstream provider for the request according to routing rules (§6).

FP3. **Adapt upstream request:** Convert `UrpRequestV2` into the selected provider's upstream request shape (§7).

FP4. **Call upstream:** Send the upstream request. If `stream=true`, Monoize MUST call the upstream in streaming mode unless buffered synthetic streaming is selected by the transform rules under `spec/urp-transform-system.spec.md`.

FP4a. If an upstream streaming response terminates with a transport-level decode error before a terminal chunk or protocol terminator is received, Monoize MUST treat that condition as an upstream error. Monoize MUST NOT silently ignore the decode failure and continue emitting a partial downstream stream as though the upstream stream completed successfully.

FP4b. Immediately before Monoize sends an upstream forwarding request for `/v1/responses`, `/v1/chat/completions`, or `/v1/messages`, Monoize MUST emit one lightweight observability log entry that includes the effective upstream request JSON byte length and a summary of user-image payload shape. The log entry MUST NOT include raw prompt text or raw image bytes.

FP4c. The request-shape observability log in FP4b MUST include at minimum:

- request identifier when present;
- downstream model identifier and selected upstream model identifier;
- provider type;
- stream flag;
- upstream path;
- encoded upstream JSON byte length;
- user image part count;
- base64 image part count, where `ImageSource::Base64` parts and `ImageSource::Url` parts carrying `data:*;base64,...` URLs both count as base64 image parts;
- URL image part count for remaining non-`data:` image URLs;
- total base64 character count for user-image parts;
- estimated decoded byte count for user-image parts computed from the base64 payload length.

FP4d. For streaming upstream calls, Monoize MUST track terminal-stream evidence in memory during adaptation. At minimum the tracked evidence consists of whether a literal `[DONE]` sentinel was received, which terminal protocol event was last observed, the terminal finish reason when present, and whether Monoize emitted a synthetic terminal chunk. This evidence is observability-only and MUST NOT change downstream response semantics by itself.

FP4e. If a downstream request has `stream=true` and an upstream attempt returns a non-2xx HTTP response before Monoize receives any upstream SSE frame, Monoize MUST NOT return the upstream HTTP status as the downstream HTTP status. Monoize MUST return a downstream SSE response with HTTP `200` and a protocol-specific terminal error frame:

- `/v1/responses`: one `event: error` frame whose JSON payload has `type = "error"`, `code` equal to the upstream error code when present, and `message` equal to the upstream error message as exposed by Monoize, followed by one plain `data: [DONE]` frame.
- `/v1/chat/completions`: one plain `data:` frame whose JSON payload has an `error` object, with `error.code` equal to the upstream error code when present, followed by one plain `data: [DONE]` frame.
- `/v1/messages`: one `event: error` frame whose JSON payload has `type = "error"` and an `error` object, with `error.type` equal to the upstream error code when present. Monoize MUST NOT append a `[DONE]` frame on `/v1/messages`.

FP4f. The synthetic stream error in FP4e MUST finalize the request log with `status = "error"`, `error_code` equal to the upstream error code when present, `error_http_status` equal to the upstream HTTP status when present, no token usage, and no billing charge.

FP5. **Decode upstream response:**

- for non-streaming calls, decode the upstream response into `UrpResponseV2`;
- for streaming calls, decode the upstream stream into canonical URP v2 stream events before any downstream SSE frame is emitted.

FP5a. Streaming decoders MUST emit canonical URP v2 stream events directly. They MUST NOT first regroup upstream protocol objects into canonical message wrappers.

FP5b. The pass-through streaming decoder entrypoint MUST be `stream_upstream_to_urp_events(urp, provider_type, upstream_resp, tx, started_at, runtime_metrics)`.

FP5c. `stream_upstream_to_urp_events` MUST dispatch by `provider_type` to exactly these decoder families:

- `responses` -> Responses-event decoder;
- `chat_completion` -> Chat Completions decoder;
- `messages` -> Anthropic Messages decoder;
- `gemini` -> Gemini decoder.

`group` is virtual and MUST NOT be streamed directly.

FP6. **Render downstream response:** Convert `UrpResponseV2` or canonical URP v2 stream events into the downstream endpoint's response shape, streaming or non-streaming.

FP6a. For pass-through streaming on `/v1/responses`, `/v1/chat/completions`, and `/v1/messages`, Monoize MUST implement the downstream adapter as a three-stage pipeline connected by in-memory channels of canonical URP v2 stream events:

- stage 1: provider-specific decoder consumes upstream stream data and emits canonical URP v2 stream events;
- stage 2: response-transform stage consumes canonical URP v2 stream events, applies any matching response-phase transforms incrementally, and emits transformed canonical URP v2 stream events;
- stage 3: downstream-specific encoder consumes transformed canonical URP v2 stream events and emits downstream SSE frames.

FP6a-NF. Monoize MUST NOT implement any fast path or raw passthrough that bypasses the three-stage Decoder -> URP -> Encoder pipeline defined in FP6a for any combination of provider type and downstream protocol. All streaming responses MUST flow through stages 1 through 3 of FP6a unconditionally, regardless of whether response-phase transforms are active.

FP6a-CTRL. A provider-specific decoder MAY emit a canonical provider-control stream event for a protocol control event that is not a model output item and is not terminal state. A provider-control event MUST contain the source protocol name, the downstream event name, and the complete JSON payload for that control event. A downstream encoder MUST emit a provider-control event only when a protocol-specific downstream rule explicitly permits that event name. Otherwise, the downstream encoder MUST ignore the provider-control event. Provider-control events MUST NOT create nodes, change `ResponseDone.output`, or affect billing usage.

FP6a1. If a streaming request matches only response-phase transform rules that support incremental canonical event rewriting, Monoize MUST keep the request in pass-through streaming mode and apply those response transforms to each canonical URP v2 stream event after stage 1 decoding and before stage 3 encoding.

FP6a2. The pass-through streaming runtime in FP6a1 MUST preserve transform rule order and scope semantics already used for non-streaming responses. Provider response-phase rules run before global response-phase rules, and global response-phase rules run before API-key response-phase rules, using one mutable transform-state vector per rule chain for the lifetime of the request.

FP6a3. If a streaming request matches at least one enabled response-phase transform rule that requires whole-response mutation rather than incremental canonical event rewriting, or the selected downstream protocol cannot faithfully represent that transform's incremental output, Monoize MUST use buffered synthetic streaming for that request. In that mode Monoize fetches an upstream non-stream response, applies response transforms to `UrpResponseV2`, then emits protocol-correct synthetic downstream SSE.

FP6a4. `response.reasoning_signature.delta` is not part of the OpenAI Responses downstream event set. Monoize MUST NOT emit that event on downstream `/v1/responses` streams.

FP6a4a. If an upstream protocol exposes reasoning-signature state separately from final reasoning nodes, Monoize MAY preserve that state internally through URP v2. Downstream `/v1/responses` SSE MUST surface such state only through canonical completed reasoning items and completed response objects, not through a custom signature-delta event.

FP6b. The pass-through streaming pipeline in FP6a MUST preserve existing routing, billing, logging, and terminal-stream observability semantics. The change in internal streaming representation MUST NOT by itself permit provider fallback after the first downstream byte.

FP6c. The pass-through streaming encoder entrypoint MUST be `encode_urp_stream(downstream, rx, tx, logical_model, sse_max_frame_length)`.

FP6d. `encode_urp_stream` MUST dispatch by downstream protocol to exactly these encoder families:

- `Responses` -> Responses encoder;
- `ChatCompletions` -> Chat Completions encoder;
- `AnthropicMessages` -> Anthropic Messages encoder.

FP6e. The forwarding handler MUST spawn exactly three asynchronous tasks for pass-through streaming after upstream response headers are accepted:

- one decode task that runs `stream_upstream_to_urp_events` and sends canonical URP v2 stream events into an in-memory channel;
- one transform task that applies matching response-phase transforms to each canonical URP v2 stream event and sends the transformed event into a second in-memory channel;
- one encode task that runs `encode_urp_stream` and reads the transformed-event channel to emit downstream SSE.

FP6f. The decode task and encode task in FP6e MUST be joined before request-final logging and billing finalization.

FP6g. The streaming pipeline MUST use `ResponseDone.output` as the authoritative final streamed response state. Monoize MUST NOT require or depend on any helper named `merged_output_items()` in the streaming path.

FP6h. Decoder responsibilities are split-and-forward only. Decoder output MUST be flat nodes or canonical node lifecycle events. Downstream envelope reconstruction belongs only to the encoder.

## 6. Routing rules

R1. Routing MUST follow `spec/database-provider-routing.spec.md`.

R2. Dashboard-managed providers and channels MUST be evaluated in configured provider order with fail-forward semantics.

R3. Streaming fallback MAY occur only before first downstream byte is emitted.

R4. If the dashboard provider list is empty, routing MUST fail with `502 upstream_error`.

## 7. Adapters

### 7.1 URP v2 internal request and response fields

Monoize MUST use internal request fields consistent with Responses create requests, including at minimum:

- `model`
- `input`
- `tools`
- `tool_choice`
- `stream`
- `include`
- `max_output_tokens`
- `parallel_tool_calls`

IN1. `UrpRequestV2.input` contains an ordered flat `Vec<Node>`. `UrpResponseV2.output` contains an ordered flat `Vec<Node>`. Canonical node order is significant.

IN2. Canonical internal conversational storage MUST follow `spec/urp-v2-flat-structure.spec.md`. In particular:

- `Message { role, parts }` is not a URP v2 value and MUST NOT appear in canonical storage;
- ordinary role-bearing content is represented by top-level ordinary nodes;
- `ToolResult` remains a distinct top-level node with `call_id`;
- `next_downstream_envelope_extra` is the only control node.

T0. For internal URP v2 requests, tool descriptors in `tools[]` MUST use Responses-style function-tool objects:

```json
{ "type": "function", "name": "tool_name", "description": "...", "parameters": { "type": "object", "properties": {} } }
```

T1. When a downstream adapter receives non-Responses tool descriptor shapes, for example Chat Completions `{"type":"function","function":{...}}` or Messages `{ "name": "...", "input_schema": ... }`, Monoize MUST normalize them to the internal shape defined by T0 before forwarding.

T1a. Internal URP v2 `tools[]` MAY also contain native Responses non-function tool descriptors. The same-Responses native set includes at least `image_generation`, `namespace`, `tool_search`, and `programmatic_tool_calling`. Monoize MUST preserve each such descriptor as a non-function tool with its typed `type` plus any extra top-level fields carried in `ToolDefinition.extra_body`.

#### 7.1.0a Tool definition compatibility

T1b. The semantic tool fields for request decode, IR storage, and request encode are:

- tool descriptor `type`;
- Chat Completions `function` wrapper and its owned fields;
- tool `name`;
- tool `description`;
- OpenAI `parameters` JSON Schema object;
- OpenAI Responses custom-tool `format` object;
- Anthropic `input_schema` JSON Schema object;
- `strict` when present on a tool descriptor or on the Chat Completions `function` object;
- request-level `tool_choice`;
- request-level `parallel_tool_calls`;
- Anthropic request-level `disable_parallel_tool_use` inside `tool_choice` objects whose `type` is `auto`, `any`, or `tool`.

T1c. Monoize MUST normalize semantic tool fields before any unknown-field fallback is applied. The Anthropic `input_schema` field and OpenAI `parameters` field are the same semantic parameter-schema field in URP v2. Monoize MUST preserve the JSON value of that schema object byte-equivalently after JSON decode and encode, but Monoize MUST NOT be required to validate every JSON Schema keyword.

T1c1. For OpenAI Responses `type = "custom"` tool descriptors, Monoize MUST preserve the descriptor as a flat tool object. The fields `name`, `description`, and `format` MUST remain fields of the emitted custom tool object. Non-semantic custom-tool fields such as `defer_loading` MUST remain at the same tool object layer through decode, IR storage, and encode.

T1c2. For Anthropic Messages tool descriptors, Monoize MUST preserve user-defined tool fields as a flat Anthropic tool object. The fields `name`, `description`, `input_schema`, optional `type = "custom"`, and `strict` MUST remain at the tool object layer. Anthropic tool metadata fields including `cache_control`, `defer_loading`, `allowed_callers`, `input_examples`, and `eager_input_streaming` MUST remain at the same tool object layer and MUST NOT be moved into `input_schema`. Anthropic provider-native built-in or versioned descriptors, including `computer_20251124`, `web_search_20260209`, `web_search_20250305`, and `mcp_toolset`, MUST remain flat non-function descriptors with their explicit `type`, native `name` when present, and config fields in the same tool object.

T1d. `extra_body` fallback for tool definitions is allowed only for fields that are not semantic tool fields for the source protocol layer. A decoder MAY place an unknown or private tool field into `ToolDefinition.extra_body` or a function-owned extra map only if the owning layer is preserved through decode, IR storage, and encode. The owner MUST distinguish at least tool-descriptor scope from Chat Completions `function` scope. A later encoder MUST NOT move a preserved function-owned field to tool-descriptor scope, and MUST NOT move a preserved tool-descriptor field into the Chat Completions `function` object.

T1e. Tool-definition merge precedence is fixed: encoder-owned semantic fields win, and `extra_body` fills absent keys only. When an encoder constructs an outbound tool descriptor, it MUST write all semantic fields required by the target protocol from typed URP state. It MAY then merge preserved `extra_body` keys at the same owner layer. If a preserved `extra_body` key conflicts with an encoder-owned semantic key, Monoize MUST keep the encoder-owned semantic value and MUST drop the conflicting preserved key from that outbound object.

T1f. Request-level tool controls are typed request semantics, not top-level `extra_body` passthrough. Monoize MUST decode and encode `tool_choice` and `parallel_tool_calls` through typed URP request fields. For Anthropic Messages, Monoize MUST decode and encode `tool_choice.disable_parallel_tool_use` as part of tool-choice semantics. Top-level request `extra_body` MAY fill only absent request keys under §7.6, and XF6 through XF6e still apply to top-level request `extra_body`.

T1f.1. A specific function or custom `tool_choice` MUST use the Chat-compatible nested shape in URP v2: `{ "type": "function", "function": { "name": <N> } }` or `{ "type": "custom", "custom": { "name": <N> } }`. A Responses request decoder MUST normalize the official flat Responses selectors `{ "type": "function", "name": <N> }` and `{ "type": "custom", "name": <N> }` to those nested URP shapes. A Responses request encoder MUST restore the official flat Responses shapes. A Chat request encoder MUST retain the nested Chat shapes. Responses built-in and named MCP selectors MUST remain flat. A named MCP selector `{ "type": "mcp", "server_label": <S>, "name": <N> }` selects the retained MCP descriptor whose `type` is `mcp` and whose `server_label` is `<S>`; its inner tool `name` MUST NOT be used as the server-descriptor identity.

T1f.2. URP v2 MUST use the Chat-compatible `allowed_tools` wrapper `{ "type": "allowed_tools", "allowed_tools": { "mode": <M>, "tools": [...] } }`. A Responses request decoder MUST normalize the official flat Responses wrapper `{ "type": "allowed_tools", "mode": <M>, "tools": [...] }` to that URP shape. A Responses request encoder MUST restore the flat Responses wrapper. A Chat request encoder MUST retain the nested Chat wrapper. Function and custom selectors inside `tools[]` MUST undergo the same target-specific normalization as top-level selectors. Built-in and named MCP selectors inside `tools[]` MUST remain flat. Provider filtering MUST retain an `allowed_tools` choice when at least one referenced tool remains available, MUST remove references to filtered descriptors, and MUST omit the choice if no referenced tool remains. A provider family that does not support `allowed_tools` MUST omit the choice.

T1f.3. For every target-specific tool-choice conversion, the semantic `type`, semantic nested `name`, canonical `allowed_tools.mode`, canonical `allowed_tools.tools`, and typed request-level `tool_choice` MUST win collisions with unknown selector fields and top-level request `extra_body`. A decoder MUST recursively remove every client-supplied selector member whose key starts with `_monoize_`, including members of the `allowed_tools` wrapper and its inner selectors. A target encoder MUST also recursively remove such members before wire emission. An OpenAI encoder MUST NOT emit Anthropic-only `disable_parallel_tool_use` inside `tool_choice`.

T1g. Provider-native tool descriptors MUST be gated by provider family. When the source family and target family are the same, Monoize MUST preserve provider-native tool descriptors that the target provider type supports, including Responses native tool types carried under T1a and Anthropic native tool types carried through a Messages request. When the source family and target family differ, Monoize MAY emit a provider-native tool descriptor only when this specification or provider-adapter code explicitly supports that target shape. Otherwise Monoize MUST filter that provider-native tool descriptor from the outbound payload for that upstream attempt while keeping the IR value intact for later attempts or downstream response rendering. If a specific `tool_choice` names only a descriptor filtered by this rule, Monoize MUST omit that `tool_choice` from the same outbound attempt.

T1h. Tool-definition compatibility rules describe request forwarding only. They MUST NOT authorize local tool execution. Monoize MUST NOT execute tools locally, as required by TCI2.

Stateful fields:

S1. Monoize MUST reject `background=true` with `400` code `background_not_supported`.

S2. For a downstream Responses request routed to a `type=responses` upstream, Monoize MUST forward `store` unchanged when it is present. For every cross-family attempt, Monoize MUST remove the downstream Responses `store` field before encoding the upstream request.

S3. For a downstream Responses request routed to a `type=responses` upstream, Monoize MUST forward `conversation` and `previous_response_id` unchanged when present. Monoize MUST NOT resolve either field locally. For every cross-family attempt, Monoize MUST remove both fields before encoding the upstream request.

S3a. A successful non-streaming or streaming `type=responses` attempt with a non-empty upstream response id MUST bind that response id to the successful Provider+Channel in the process-memory affinity cache. A later request from the same authenticated tenant with the same logical model and `previous_response_id` MUST use that binding while it remains eligible and unexpired. The binding uses the same 30-minute idle expiration and invalidation rules as channel affinity.

S4. Monoize MUST accept a Responses `input` item with `type = "item_reference"` as a same-Responses provider item. It MAY be forwarded only to a selected `type=responses` upstream. A cross-family attempt MUST remove the item before encoding because Chat Completions and Messages have no equivalent reference object. Monoize MUST NOT resolve the reference locally.

### 7.1.0b Native Responses compaction endpoint

CMP1. `POST /v1/responses/compact` accepts a JSON object with a non-empty string `model` and an `input` member. It MUST apply API-key model redirects, model allow-list checks, multiplier limits, Provider+Channel routing, balance guard, request logging, usage billing, retry policy, and passive health updates in the same order used by non-streaming forwarding requests.

CMP2. A compact request MUST be eligible only for an effective upstream provider of `type=responses`. Monoize MUST call upstream path `POST /v1/responses/compact`. It MUST NOT adapt the compact request to Chat Completions, Messages, Gemini, image, or Replicate request shapes.

CMP3. For the selected Responses attempt, Monoize MUST preserve the native compact request object and its ordered `input` items. It MAY change only `model` to the selected upstream model, remove proxy-only `max_multiplier`, and remove keys whose names begin with `_monoize_`. It MUST NOT remove or reinterpret native `message`, `reasoning`, `function_call`, `function_call_output`, `program`, `program_output`, `tool_search_call`, `tool_search_output`, `additional_tools`, or `compaction` input items.

CMP4. On upstream success, Monoize MUST return the upstream JSON object without changing `id`, `object`, `created_at`, `output`, or `usage`. In particular, `object = "response.compaction"` and every opaque `type = "compaction"` output item MUST remain unchanged.

CMP5. Compact requests are non-streaming. A request with `stream = true` MUST return `400` with code `invalid_request` before upstream dispatch.

CMP6. On `POST /v1/responses`, a same-Responses attempt MUST forward the complete `context_management` array unchanged. A cross-family attempt MUST remove it. A native `compaction` item emitted by normal Responses create, in either non-streaming output or streaming output-item lifecycle, MUST remain an opaque same-Responses ProviderItem and MUST be replayable as later Responses input.

### 7.1.1 URP v2 tool-calling nodes

TCI1. URP v2 `input` and `output` MAY contain tool-calling state represented as nodes:

- **Tool call:** one ordinary `ToolCall` node with fields `call_id`, `name`, `arguments`, and `role = "assistant"`.
- **Tool result:** one distinct top-level `ToolResult` node with fields `call_id`, `is_error`, `content: Vec<ToolResultContent>`, and `extra_body`.

TCI2. Monoize MUST NOT execute tools locally. Tool execution is always performed by the downstream client.

TCI3. When Monoize forwards a request, Monoize MUST forward any tool-calling nodes present in `UrpRequestV2.input` by adapting them into the selected upstream provider's request format under §7.2 through §7.8.

TCI4. Immediately before encoding a stateless URP v2 request to an upstream provider, Monoize MUST remove incomplete tool exchanges from `UrpRequestV2.input`:

- remove every `ToolCall` node whose `call_id` does not appear in at least one `ToolResult` node in the same request;
- remove every `ToolResult` node whose `call_id` does not appear in at least one `ToolCall` node in the same request.

TCI5. A request is stateless for TCI4 unless it is a downstream Responses request routed to a `type=responses` upstream and carries non-null `previous_response_id` or `conversation`. For a stateful same-Responses request, Monoize MUST skip TCI4 so that a `function_call_output` whose matching call exists in upstream state remains in the request.

TCI6. For same-Responses programmatic tool calling, Monoize MUST preserve `type = "programmatic_tool_calling"` as a native tool descriptor. It MUST preserve function-tool `allowed_callers` and `output_schema` fields. It MUST preserve `program` and `program_output` items as Responses ProviderItems, and it MUST preserve the complete `caller` object on correlated `function_call` and `function_call_output` items. Monoize MUST NOT execute the program or the client-owned function call.

TCI7. For same-Responses tool search, Monoize MUST preserve `defer_loading` on function and MCP tool descriptors, native `namespace` and `tool_search` descriptors, and native `tool_search_call`, `tool_search_output`, and `additional_tools` input or output items. These item types MUST use Responses ProviderItems when no typed URP node exists. Monoize MUST NOT execute client-owned tool search calls.

TCI8. For same-Messages programmatic tool calling, Monoize MUST preserve the native versioned `code_execution_*` descriptor, function-tool `allowed_callers`, top-level `container`, `caller` on client `tool_use`, and opaque `server_tool_use` and `code_execution_tool_result` blocks. Monoize MUST NOT execute the code or the client-owned tool call.

TCI9. For same-Messages tool search, Monoize MUST preserve native versioned `tool_search_tool_regex_*` and `tool_search_tool_bm25_*` descriptors, `defer_loading` on deferred tools, opaque `server_tool_use` and `tool_search_tool_result` blocks, and `tool_reference` blocks nested in client `tool_result.content`. Monoize MUST NOT convert a server tool block into a client `tool_use` block.

### 7.1.1a ToolResultContent type

TRC1. `ToolResultContent` is an enum representing typed content within `ToolResult.content`. The variants are:

- `Text { text: String, extra_body: Map<String, JsonValue> }`
- `Image { source: ImageSource, extra_body: Map<String, JsonValue> }`
- `File { source: FileSource, extra_body: Map<String, JsonValue> }`
- `ProviderItem { origin_protocol: ProviderProtocol, item_type: String, body: JsonValue, extra_body: Map<String, JsonValue> }`

TRC2. Each `ToolResult.content` field contains zero or more `ToolResultContent` entries. An empty `content` vector represents a tool result with no output payload.

TRC3. `ImageSource` contains exactly the following variants:

- `Url { url: String, detail: Option<String> }`
- `Base64 { media_type: String, data: String }`
- `FileId { file_id: String, detail: Option<String> }`

TRC4. `FileSource` contains exactly the following variants:

- `Url { url: String }`
- `FileId { file_id: String }`
- `Text { text: String }`
- `Content { content: Vec<JsonValue> }`
- `Base64 { filename: Option<String>, media_type: String, data: String }`

TRC5. An encoder MUST merge `ToolResultContent.extra_body` into the generated nested protocol object without replacing a typed field.

TRC6. A `ToolResultContent::ProviderItem` MAY be encoded only when its `origin_protocol` exactly equals the target provider protocol. A cross-protocol encoder MUST omit it.

### 7.1.1b ProviderItem opaque native items

PI1. `ProviderItem` is an opaque native item, block, or part preservation container. It is not a cross-protocol semantic node.

PI2. `ProviderItem.origin_protocol` MUST be one of `responses`, `chat_completion`, `messages`, `gemini`, `openai_image`, or `replicate`.

PI3. A decoder MUST emit `ProviderItem` only for a native carrier unit that the source adapter cannot decode into a typed URP node. Required cases are:

- Responses unknown `input[]` or `output[]` items, including items with `type = "compaction"`;
- Chat Completions unknown `messages[].content[]` parts and unknown assistant content parts;
- Anthropic Messages unknown `content[]` blocks;
- Gemini unknown `parts[]` parts.

PI4. A `ProviderItem` MUST preserve the native carrier object in `body` without text stringification. The decoder MUST set `origin_protocol` to the exact source protocol and MUST keep the source native `type` string in `item_type` when one exists.

PI5. An encoder MUST replay `ProviderItem.body` only when `ProviderItem.origin_protocol` exactly equals the target provider protocol. Exact protocol equality is required; `responses` and `chat_completion` are different protocols.

PI6. If `ProviderItem.origin_protocol` does not equal the target provider protocol, the encoder MUST ignore that node. It MUST NOT stringify `body`, insert JSON text into any prompt, convert it to `Text`, or disguise it as another typed node.

PI7. ProviderItem same-protocol replay does not expand any `extra_body` passthrough rule. Top-level `extra_body`, node-local `extra_body`, tool-definition `extra_body`, and envelope-control rules remain governed by §7.6.

PI8. Immediately before request-phase transforms run for each upstream attempt, Monoize MUST remove from `UrpRequestV2.input` every downstream-origin `ProviderItem` whose `origin_protocol` differs from the selected upstream provider protocol. A later transform MAY insert a `ProviderItem` only if it sets `origin_protocol` to the intended target provider protocol.

PI9. In streaming, `NodeStart.header.type = "provider_item"` MUST carry `origin_protocol`. A downstream stream encoder MUST reconstruct a ProviderItem lifecycle only for same-protocol ProviderItems. A mismatched ProviderItem stream node MUST be skipped without producing downstream text or a typed replacement node.

### 7.1.2 URP v2 reasoning node

RSN1. URP v2 `output` MAY contain reasoning state represented as one or more ordinary `Reasoning` nodes.

RSN2. `Reasoning.content` is plaintext reasoning text when available.

RSN3. `Reasoning.summary` is plaintext summary text when available.

RSN4. `Reasoning.encrypted` MUST contain opaque provider-supplied reasoning payload when available. Adapters MUST store that value in the typed field `encrypted`. Adapters MUST NOT move that value into `extra_body` under ad hoc keys such as `signature`.

RSN5. `Reasoning.source` MUST contain the exact provider-supplied source identifier when available. If the upstream protocol omits reasoning source, Monoize MUST leave `source` absent rather than inventing a placeholder.

### 7.1.2a URP v2 text phase metadata

TPH1. Every URP v2 `Text` node MAY carry optional field `phase`.

TPH2. `phase` MUST be treated as an unconstrained string. Monoize MAY recognize known values such as `commentary` and `final_answer`, but MUST NOT reject or rewrite unknown values solely because the value is unrecognized.

TPH3. `phase` belongs only to `Text` nodes. Monoize MUST attach `phase` to text semantics, not to tool-call semantics.

TPH4. When Monoize decodes a protocol object whose `phase` is defined at message-item level or text-block level, Monoize MUST copy that `phase` value onto every URP v2 `Text` node produced from that source object.

TPH5. When Monoize re-encodes URP v2 `Text` nodes to a protocol that supports `phase`, Monoize MUST write the text-node `phase` back to the target protocol object at that protocol's supported level, except where a provider-specific upstream request rule in this specification defines a stricter allow-list.

### 7.1.3 Reasoning-control normalization

RC1. Monoize MUST normalize reasoning effort to one of `none`, `minimal`, `low`, `medium`, `high`, `xhigh`, or `max`. A downstream legacy value `minimum` MAY be accepted only as an input alias and MUST normalize to `minimal` before encoding. `xhigh` and `max` are two distinct effort levels; Monoize MUST NOT treat either as an alias of the other.

RC2. Monoize MUST accept reasoning effort hints from any of the following downstream fields:

- Chat Completions style: top-level `reasoning_effort`.
- Responses style: top-level `reasoning.effort`.
- Messages style, legacy: top-level `thinking` object with `type="enabled"`.
- Messages style, adaptive: top-level `thinking` object with `type="adaptive"` combined with `output_config.effort`.

RC3. If multiple sources in RC2 are present, Monoize MUST use this precedence:

1. `reasoning_effort`
2. `reasoning.effort`
3. `thinking`

RC4. When the selected upstream provider type is:

- `chat_completion`: If effort is `none`, Monoize MUST omit the `reasoning_effort` field entirely. For any other effort value, Monoize MUST send normalized effort as `reasoning_effort`, except for DeepSeek models governed by DC5c and the native reasoning-object case below. If the typed request preserves an explicit native `reasoning` object, that object is the single reasoning-control container: Monoize MUST preserve its non-`effort` members, MUST replace a colliding `reasoning.effort` with the normalized typed effort, and MUST omit the top-level `reasoning_effort` shorthand. OpenRouter `reasoning.effort` and `reasoning.max_tokens` are mutually exclusive. Therefore, when normalized typed effort is inserted into a native `reasoning` object, Monoize MUST remove a colliding `max_tokens`; when typed effort is absent, Monoize MAY preserve native `reasoning.max_tokens`. Monoize MUST NOT emit both equivalent effort controls in one request.
- `responses`: If effort is `none`, Monoize MUST omit the `effort` key from the `reasoning` object. The object MAY still contain other keys such as `summary`. For any other effort value, Monoize MUST send normalized effort as `reasoning: { "effort": <level> }`.
- `messages`: Monoize MUST select the encoding based on the upstream model and any explicit downstream Messages controls:
  - For models that support adaptive thinking, a generated config MUST send `thinking: { "type": "adaptive" }`. If normalized reasoning effort is present and is not `none` or `minimal`, Monoize MUST also send `output_config: { "effort": <level> }`, transmitting `low`, `medium`, `high`, `xhigh`, and `max` as distinct values. If effort is absent, Monoize MUST omit generated `output_config`.
  - A model identifier denotes a Claude model when either (a) the lowercased identifier contains the token `claude`, or (b) the lowercased identifier begins with the legacy bare family prefix `opus-`, `sonnet-`, or `haiku-`.
  - A Claude model is non-adaptive only when its identifier provides a parseable family version that satisfies one of these inclusive upper bounds: Opus version <= 4.5; Sonnet version <= 4.5; Haiku version <= 4.5. Version parsing MUST accept both family-first identifiers such as `claude-opus-4-6`, `claude-sonnet-4.7`, and `claude-haiku-4-5`, and version-first legacy identifiers such as `claude-3-5-sonnet`.
  - An 8-digit release-date suffix is not a version component. `claude-sonnet-4-20250514` denotes Sonnet 4.0, and `claude-3-7-sonnet-20250219` denotes Sonnet 3.7.
  - Every other Claude model MUST be treated as adaptive. This includes Opus 4.6 and later, Sonnet 4.6 and later, Fable 5, Mythos 5, Mythos Preview, Claude identifiers whose family version is absent or unparseable, and future Claude families.
  - Every identifier that does not denote a Claude model MUST be treated as non-adaptive by the Messages encoder.
  - For non-adaptive models, Monoize MUST send `thinking: { "type": "enabled", "budget_tokens": N }`, where:
    - `minimal -> N=1024`
    - `low -> N=1024`
    - `medium -> N=4096`
    - `high -> N=16384`
    - `xhigh -> N=32000`
    - `max -> N=32000` (identical budget to `xhigh` on non-adaptive models; the distinction between `xhigh` and `max` is only observable on adaptive-thinking models)
    - absent effort -> N=4096

RC4b. A Messages decoder MUST preserve the complete explicit `thinking` object and the complete explicit `output_config` object independently. This includes `thinking.type`, exact `budget_tokens`, `display`, `output_config.effort`, `output_config.format`, and unknown `output_config` members. Effort without a thinking object and `thinking.type = "disabled"` are valid states and MUST survive a same-Messages round trip. Preservation occurs before outbound validation; preserving an invalid combination does not authorize dispatch under RC4f.

RC4b.1. A Messages decoder MUST decode `output_config.format = { "type": "json_schema", "schema": <S> }` into `ResponseFormat::JsonSchema` with schema `<S>`. Because the canonical OpenAI-compatible representation requires a schema name and the Messages object has no name field, the decoder MUST assign the deterministic internal name `response`. This synthesized name MUST NOT be emitted to a Messages provider. The complete original `output_config` remains authoritative passthrough state for same-Messages replay.

RC4c. A same-Messages encoder MUST preserve an explicit `thinking.display` value of `summarized` or `omitted`. If display was absent, the encoder MUST leave it absent and allow the selected model's documented default. Monoize MUST NOT rewrite omitted to summarized or summarized to omitted. A preserved `display` value combined with `thinking.type = "disabled"` MUST fail RC4f.5 before dispatch.

RC4d. Generated manual thinking is permitted only for models that support `thinking.type = "enabled"`. Opus 4.7, Opus 4.8, Sonnet 5, Fable 5, and Mythos 5 MUST use adaptive thinking. Opus 4.6 and Sonnet 4.6 SHOULD use adaptive thinking although manual budgets remain accepted but deprecated. Opus 4.5, Sonnet 4.5, Haiku 4.5, and earlier Claude 4 models use manual budgets.

RC4f. Immediately before sending an upstream Messages request, Monoize MUST validate the fully encoded request body. A `thinking` object is active when `thinking.type` is `enabled` or `adaptive`. If validation fails, Monoize MUST return HTTP 400 with code `invalid_request`, MUST NOT dispatch the upstream request, and MUST NOT alter `max_tokens`, `budget_tokens`, sampling controls, tool choice, or message history to make the request valid.

RC4f.1. Active manual thinking has `thinking.type = "enabled"`. Its `budget_tokens` MUST be an integer greater than or equal to 1024 and MUST be strictly less than the encoded `max_tokens`. This rule applies to explicit Messages controls and to manual controls generated from normalized reasoning effort.

RC4f.2. Active adaptive thinking has `thinking.type = "adaptive"`. Its `thinking` object MUST NOT contain `budget_tokens`. The upstream model MUST satisfy the adaptive-model classification in RC4. Manual thinking MUST be rejected for Opus 4.7 or later, Sonnet 5 or later, Fable 5, and Mythos 5 except Mythos Preview. Manual thinking remains accepted for Opus 4.6, Sonnet 4.6, and Mythos Preview. Fable 5, Mythos 5, and Mythos Preview MUST reject `thinking.type = "disabled"` because adaptive thinking is always on for those models.

RC4f.3. For every active thinking request:

- `temperature` MUST be absent or equal to `1`;
- `top_k` MUST be absent;
- `top_p` MUST be absent or be a number in the inclusive range `[0.95, 1]`;
- `tool_choice.type` MUST NOT equal `any` or `tool`; and
- the final encoded `messages[]` entry MUST NOT have `role = "assistant"`.

RC4f.4. Fable 5, Mythos 5, Mythos Preview, Opus 4.7 or later, and Sonnet 5 or later reject non-default sampling values regardless of whether the request contains an explicit `thinking` object. For every request to one of these models, `temperature` MUST be absent or equal to `1`, `top_p` MUST be absent or equal to `1`, and `top_k` MUST be absent.

RC4f.5. When active adaptive thinking carries `output_config.effort`, its value MUST be one of `low`, `medium`, `high`, `xhigh`, or `max`. The value `xhigh` is valid only for Fable 5, Mythos 5 excluding Mythos Preview, Opus 4.7 or later, and Sonnet 5 or later. For active manual or adaptive thinking, an explicit `display` value MUST equal `summarized` or `omitted`. A `thinking` object with `type = "disabled"` MUST NOT contain `budget_tokens` or `display`.

RC4a. For upstream provider type `responses`, Monoize MUST preserve an explicit typed downstream `reasoning.summary` value byte-for-byte. If the typed downstream request does not carry `reasoning.summary`, Monoize MUST omit that key. Monoize MUST NOT synthesize `reasoning.summary = "detailed"`, `reasoning.summary = "auto"`, or any other summary setting because Responses reasoning summaries require explicit opt-in.

RC4e. When replaying an assistant-history message to an upstream Chat Completions provider, a non-empty `reasoning_details` array is authoritative. Monoize MUST preserve its entries and order, and MUST NOT synthesize scalar `reasoning` or `reasoning_content` aliases from those entries. This rule does not prohibit a downstream Chat response from exposing the simple `reasoning` alias allowed by ENC8.

RC5. If Monoize generated provider-native reasoning-control fields under RC4, Monoize MUST NOT forward conflicting source fields from passthrough state to the same upstream request.

### 7.1.4 Decoder and encoder responsibilities

DER1. Downstream decoders for `/v1/responses`, `/v1/chat/completions`, and `/v1/messages` MUST decode into `UrpRequestV2` before any request-phase transform executes.

DER2. Upstream non-stream decoders MUST decode into `UrpResponseV2` before any response-phase non-stream transform executes.

DER3. Stream decoders MUST emit canonical URP v2 stream events before any response-phase stream transform executes.

DER4. A decoder MUST emit flat nodes in source order. A decoder MUST NOT greedily regroup several upstream semantic units into canonical grouped-message storage.

DER5. A decoder MUST preserve `ToolResult` as a distinct top-level node. A decoder MUST NOT reclassify tool execution output as an ordinary role-bearing node.

DER6. A decoder MUST preserve `next_downstream_envelope_extra` as an opaque control boundary when envelope-level unknown fields do not belong to exactly one emitted node.

DER7. An encoder owns downstream and upstream envelope reconstruction. Encoders MAY group consecutive URP v2 nodes only when the grouping is protocol-correct and consistent with `spec/urp-v2-flat-structure.spec.md`.

DER8. Ordinary role-based rewrite and merge behavior MUST treat `ToolResult` as outside that behavior. Encoders and transforms MUST NOT merge `ToolResult` into an ordinary-node envelope.

DER9. Responses, Chat Completions, and Anthropic Messages decoders MUST be able to parse the canonical non-stream response object emitted by the matching encoder back into an equivalent `UrpResponseV2` without inventing, discarding, or reclassifying protocol-visible reasoning, text, tool-call, or tool-result structure.

DER10. If an encoder emits distinct reasoning summary text, full reasoning content, opaque reasoning payload, reasoning source, or phase metadata, the matching decoder MUST reconstruct those fields into the same URP v2 node shape unless the encoded protocol itself collapses those fields into one externally visible field.

DER11. If an upstream reasoning item or reasoning delta carries a provider source identifier, the decoder MUST copy that value into `Reasoning.source` exactly as provided, subject only to omission of empty strings.

DER12. Streaming reconstruction and buffered synthetic fallback MUST preserve the most recent non-empty upstream reasoning source associated with the reconstructed reasoning node. Monoize MUST NOT replace that upstream value with router, provider, or model defaults.

DER13. If the upstream protocol does not provide a reasoning source value, Monoize MUST leave `Reasoning.source` absent.

### 7.1.5 Encoder-owned protocol reconstruction

ENC1. The Responses encoder MUST reconstruct native Responses output items from flat URP v2 nodes.

ENC2. Each `Reasoning` node that carries a meaningful reasoning payload under PR2g MUST encode as one top-level Responses `reasoning` item. A `Reasoning` node that does not carry a meaningful reasoning payload MUST be omitted under PR2g.

ENC3. Each function `ToolCall` node MUST encode as one top-level Responses `function_call` item. Each custom `ToolCall` node MUST encode as one top-level Responses `custom_tool_call` item. Their correlated `ToolResult` nodes MUST encode as `function_call_output` and `custom_tool_call_output` respectively.

ENC4. Each maximal run of adjacent ordinary nodes that are not `Reasoning`, not `ToolCall`, and not `ProviderItem`, and that share the same `role`, MAY encode as one Responses `message` item.

ENC5. A change in `Text.phase` inside a Responses message run MUST force a new Responses `message` item boundary.

ENC6. The Chat Completions encoder MUST prevent content and `tool_calls` interleaving within one streamed downstream chunk. It MAY reconstruct one downstream assistant message object from several flat assistant nodes for non-stream responses when that reconstruction preserves source order and all tool, reasoning, and text fields.

ENC7. In a non-streaming Chat Completions response, `choices[0].message.content` MUST be a JSON string when the reconstructed assistant content contains only text. If multiple assistant text nodes are merged into one downstream message, Monoize MUST concatenate their text in source order using `"\n\n"` as the separator. If the reconstructed same-protocol Chat output contains one or more `ProviderItem(origin_protocol = "chat_completion")` content parts, Monoize MAY emit `choices[0].message.content` as a content-part array to preserve those native parts.

ENC8. Structured reasoning in Chat Completions responses MUST be encoded in `reasoning_details`, not in `reasoning`. Plaintext reasoning MAY also populate the simple `reasoning` alias where that downstream field already exists.

ENC8a. A Chat Completions stream encoder MUST encode non-empty `Reasoning.content` as `reasoning.text`. It MUST NOT encode that value as `reasoning.summary` unless a configured response transform has already moved the value from `Reasoning.content` to `Reasoning.summary`.

ENC9. The Anthropic Messages encoder MUST reconstruct Anthropic `message` and `content[]` envelopes from flat URP v2 nodes. Block order MUST preserve flat node order after protocol-required grouping.

ENC9a. When decoding a downstream Anthropic Messages request, Monoize MUST store request field `max_tokens` as the URP request field `max_output_tokens`. When encoding an upstream Anthropic Messages request, Monoize MUST always send `max_tokens`. If the downstream request omits an explicit output-token cap, Monoize MUST encode `max_tokens: 64000`. If the downstream request provides an explicit output-token cap, Monoize MUST forward that explicit value unchanged.

ENC10. `ToolResult` remains a distinct top-level semantic unit. The Anthropic Messages encoder MUST render a `ToolResult` node as a distinct `tool_result` protocol object or block container. It MUST NOT rewrite that node as ordinary role-bearing content.

ENC11. Every URP metadata key whose name starts with `_monoize_` is internal adapter state. A wire encoder MUST NOT serialize such a key as a request field, response field, message field, content-part field, tool field, or reasoning field. An encoder MAY inspect an internal key to reconstruct a documented native protocol field, but only that reconstructed native field MAY appear on the wire. Immediately before a same-protocol opaque `ProviderItem.body` is emitted or merged into wire state, the encoder MUST clone that body and recursively remove every object member whose key starts with `_monoize_`, including members inside nested objects and objects contained by arrays. The encoder MUST preserve every other opaque member and MUST NOT mutate the canonical `ProviderItem.body`.

### 7.2 Provider adapter: `type=responses`

PR1. Monoize MUST call the upstream path `POST /v1/responses`.

PR2. For non-streaming responses, Monoize MUST parse the upstream response as a Responses response object and convert it to `UrpResponseV2`.

PR2b. When a non-streaming upstream Responses `output[]` item has `type = "image_generation_call"` and carries non-empty field `result`, Monoize MUST decode that item as one assistant `Image` node with `Image.source = Base64`.

- The decoded `media_type` MUST be derived from `output_format` when present: `png -> image/png`, `webp -> image/webp`, `jpeg -> image/jpeg`.
- If `output_format` is absent or unrecognized, Monoize MUST default the decoded `media_type` to `image/png`.
- Monoize MUST preserve unknown fields from that item in node-local `extra_body`, excluding adapter-consumed keys `type`, `result`, and `output_format`.

PR2c. When decoding Responses `input[]` or `output[]`, Monoize MUST decode every unknown top-level item type as `ProviderItem(origin_protocol = "responses")`. This includes `item_reference`, `compaction`, and future item types. Same-Responses replay preserves the native object; cross-family routing removes it unless a typed mapping exists.

PR2e. When decoding a Responses reasoning item, Monoize MUST decode every `content[]` entry with `type = "reasoning_text"` and string field `text` as plaintext `Reasoning.content`. If several such entries exist, Monoize MUST concatenate their `text` values in source order without an inserted separator. Monoize MAY also accept the legacy non-standard top-level reasoning-item field `text` as a decode-only compatibility input. If both official `content[]` reasoning text and legacy top-level `text` are present, official `content[]` is authoritative.

PR2f. When encoding a downstream Responses reasoning item with non-empty `Reasoning.content`, Monoize MUST encode the plaintext as `content: [{ "type": "reasoning_text", "text": <content> }]`. It MUST NOT encode plaintext reasoning in a top-level `text` field. When `Reasoning.content` is absent or empty, the encoder MUST emit `content: []`.

PR2g. A downstream Responses reasoning item carries a meaningful reasoning payload iff at least one of these conditions is true after all response-phase transforms have run:

- `Reasoning.content` is a non-empty string;
- `Reasoning.summary` is a non-empty string; or
- `Reasoning.encrypted` is non-null and is not an empty string, array, or object.

For non-streaming downstream Responses output, Monoize MUST omit every `Reasoning` node that does not carry a meaningful reasoning payload. For streaming downstream Responses output, Monoize MUST defer `response.output_item.added` for a reasoning node until a meaningful payload is observed. If the node completes without a meaningful payload, Monoize MUST emit no output-item lifecycle and no reasoning child lifecycle for that node, MUST omit the node from `response.completed.response.output`, and MUST NOT consume a downstream `output_index`. Therefore later emitted output items MUST retain contiguous zero-based downstream output indices. A response transform that removes the only encrypted payload from a reasoning node can cause that node to become empty under this rule.

PR2d. When encoding URP v2 to a Responses request or response, Monoize MUST replay a `ProviderItem` as one native Responses `input[]` or `output[]` item only when `origin_protocol = "responses"`. It MUST omit all other ProviderItems.

PR2h. A non-stream Responses decoder MUST preserve the exact top-level `status`, `error`, and `incomplete_details` values. A same-Responses encoder MUST re-emit those values and MUST NOT replace `failed`, `incomplete`, `cancelled`, `queued`, or `in_progress` with generated `completed`, `error:null`, or `incomplete_details:null`. Optional fields absent from the source MUST remain absent unless Monoize is synthesizing a new response object.

PR2a. Responses order preservation:

- When Monoize decodes Responses `input[]` or `output[]`, Monoize MUST process items in source order and preserve that order in the resulting URP v2 node sequence.
- When Monoize encodes URP v2 back to Responses `input[]` or `output[]`, Monoize MUST emit items in encoder-reconstructed URP order.
- Monoize MUST NOT postpone all text emission into one final Responses `message` item if that would reorder text relative to `function_call`, `reasoning`, or future item kinds.
- Monoize MAY merge only contiguous message-compatible nodes into one Responses `message` item.
- Monoize MUST split Responses `message` items when the URP order crosses a non-message item boundary or when adjacent URP v2 `Text` nodes have different `phase` values.

PR3. For streaming, Monoize MUST parse upstream Responses SSE into canonical URP v2 stream events and then encode protocol-correct downstream SSE under §8 when the downstream endpoint is `POST /v1/responses`.

PR3a. When an upstream Responses SSE event has `type = "response.image_generation_call.partial_image"`, Monoize MUST preserve it as a downstream Responses SSE event with the same event name when the downstream request is streaming and the downstream protocol is Responses. The downstream event payload MUST preserve `item_id`, `output_index`, `partial_image_b64`, and `partial_image_index` when those fields are present in the upstream payload. It MUST preserve any other upstream payload fields except `type` and `sequence_number` as event-local fields. Monoize MAY assign a new downstream `sequence_number`.

PR3b. When an upstream Responses SSE event has `type = "response.image_generation_call.in_progress"` or `type = "response.image_generation_call.generating"`, Monoize MUST preserve it as a downstream Responses SSE event with the same event name when the downstream request is streaming and the downstream protocol is Responses. The downstream event payload MUST preserve `item_id` and `output_index` when those fields are present. It MUST preserve any other upstream payload fields except `type` and `sequence_number` as event-local fields. Monoize MAY assign a new downstream `sequence_number`.

PR3c. When an upstream Responses SSE event has `type = "response.image_generation_call.completed"`, Monoize MUST preserve it as a downstream Responses SSE event with the same event name when the downstream request is streaming and the downstream protocol is Responses. The downstream event payload MUST preserve `item_id` and `output_index` when those fields are present. It MUST preserve any other upstream payload fields except `type` and `sequence_number` as event-local fields. Monoize MAY assign a new downstream `sequence_number`.

PR4. When constructing upstream `POST /v1/responses` requests, Monoize MUST emit `tools[]` in Responses-style function-tool shape even if the downstream request used another tool schema.

PR4a. Responses `phase` mapping:

- When decoding Responses `message` items, Monoize MUST read `item.phase` when present and copy it onto every URP v2 `Text` node derived from that message item.
- When encoding Responses `message` items from URP v2 `Text` nodes, Monoize MUST write `phase` on the generated `message` item when the contiguous text-node run shares the same non-null `phase`.
- If a contiguous Responses `message` item contains no URP v2 `Text` nodes, Monoize MUST NOT invent a `phase` value.
- When encoding an upstream Responses create request, Monoize MUST forward a `phase` value on a `message` item or text content part only if the value is exactly `commentary` or exactly `final_answer`. Monoize MUST drop every other `phase` value from the upstream request. This filter applies after downstream decoding and request transforms, and before the HTTP request is sent to the upstream Responses provider.

PR4b. When encoding URP v2 `response_format` to an upstream `type=responses` request:

- `ResponseFormat::Text` MUST encode as `text.format.type = "text"`.
- `ResponseFormat::JsonObject` MUST encode as `text.format.type = "json_object"`.
- `ResponseFormat::JsonSchema` MUST encode as `text.format.type = "json_schema"` with the provided schema object.
- Monoize MUST NOT rewrite `ResponseFormat::JsonObject` into a synthetic `json_schema` object.
- A Responses request decoder MUST read the official `text.format` object into `response_format`. It MUST preserve every other field of the `text` object, including `verbosity`, for same-Responses re-encoding. It MUST NOT discard `text` after attempting to read a non-Responses top-level `response_format` alias.
- If typed `response_format` and preserved passthrough `text.format` collide during encoding, the typed `response_format` MUST replace the preserved `text.format`. Every preserved non-`format` member of `text` MUST remain unchanged.

PR4b.1. A Responses request decoder MUST read `text.verbosity` into typed `UrpRequestV2.verbosity`. A Responses request encoder MUST emit typed verbosity as `text.verbosity`. Unknown `text` siblings MUST survive same-Responses replay, and typed verbosity MUST win a collision with preserved `text.verbosity`. A Responses request encoder MUST omit `UrpRequestV2.stop` because Responses create has no stop-sequence control. Typed `UrpRequestV2.user` MUST emit as top-level `user`.

PR4b.2. Responses `instructions` normalization:

- A Responses request decoder MUST accept `instructions` as either a string or an array of Responses input items.
- A non-empty string `instructions` value MUST decode as one developer `Text` node.
- For array `instructions`, an explicit message role of `system`, `developer`, `user`, or `assistant` MUST remain the corresponding URP role. A message without a role or a bare supported content item MUST decode to developer nodes.
- The decoder MUST map supported instruction content to semantic nodes using the same Responses content mappings as `input[]`: text to `Text`, image content to `Image`, and file content to `File`. It MUST preserve source order.
- The decoder MUST retain the exact original `instructions` JSON value and mark every semantic node derived from it as Responses-instructions provenance using internal metadata under `XTRA-10`.
- A same-Responses request encoder MUST reconstruct the exact retained `instructions` JSON value and MUST NOT duplicate its derived semantic nodes in `input[]`.
- A cross-family encoder MUST consume the derived semantic nodes using their normalized roles. It MUST encode every text, image, or file node that the target protocol supports and MUST NOT discard supported instruction content merely because its source was the Responses `instructions` field.
- If no retained Responses-instructions provenance exists, a Responses encoder MAY promote the first otherwise promotable system or developer text message to string `instructions` under the existing request reconstruction rule.

PR4c. When encoding URP v2 `Reasoning` nodes into upstream `POST /v1/responses` request `input[]` items with `type="reasoning"`, Monoize MUST preserve summary, raw reasoning content, encrypted content, status, and item id as distinct fields.

- If the URP reasoning node carries summary text, Monoize MUST encode `summary` as an array containing one `{ "type": "summary_text", "text": <summary> }` object.
- If the URP reasoning node carries raw reasoning content, Monoize MUST encode `content` as an array containing `{ "type": "reasoning_text", "text": <content> }`. Monoize MUST NOT convert raw content into summary text.
- If the source item explicitly carried an empty `summary` or `content` array, a same-Responses replay MUST preserve that empty array. An encoder MUST NOT invent summary text from raw content.
- Monoize MUST NOT forward URP-internal metadata or non-schema fields such as `source`, `started_at`, or transform markers on an upstream Responses reasoning item.
- Monoize MUST NOT encode a legacy top-level `text` field. Raw reasoning text uses `content[].type = "reasoning_text"`.

PR4c.1. When decoding downstream Responses `input[]`, Monoize MUST decode an item with `type = "reasoning"` into one URP v2 `Reasoning` node using the same field mapping as non-streaming Responses `output[]` reasoning items:

- `id` maps to `Reasoning.id`;
- `encrypted_content` maps to `Reasoning.encrypted`;
- `summary[]` maps to `Reasoning.summary`;
- `content[]` entries with `type = "reasoning_text"` map to `Reasoning.content` under PR2e;
- legacy top-level `text`, when present without official reasoning text content, maps to `Reasoning.content`.

If the downstream `input[]` reasoning item omits `id`, Monoize MUST leave `Reasoning.id` absent. Monoize MUST NOT synthesize an id while decoding request-side replay items.

PR4c.2. OpenAI Responses returns `encrypted_content` by default for stateless or zero-data-retention requests. Monoize MAY include legacy value `reasoning.encrypted_content` in the top-level `include` array for providers that still require it. If Monoize appends that value to an existing array, it MUST append it only when the exact string is absent.

PR4c.2a. When encoding an upstream `POST /v1/responses` request whose replayed `input[]` history contains tool-calling nodes, Monoize MUST preserve plaintext RawCoT reasoning items from open-source reasoning models. A reasoning item MAY be dropped only when a decoder or transform has explicitly marked it as downstream-only presentation text with `Reasoning.extra_body["_monoize_reasoning_downstream_only_presentation"] = true`. The presence or absence of `encrypted_content` alone MUST NOT cause a raw `content[].reasoning_text` item to be dropped.

PR4c.2b. When encoding a replayed URP v2 `Reasoning` node with non-empty `encrypted` into an upstream `POST /v1/responses` request `input[]` item, Monoize MUST emit that item only if the node has a stable upstream Responses reasoning item id. A stable id is present iff either `Reasoning.id` is non-empty or `Reasoning.extra_body.id` is a non-empty string. If no stable id is present, Monoize MUST drop the replayed reasoning node. Monoize MUST NOT synthesize a new `rs_...` id for encrypted reasoning replay because the encrypted payload is bound to the original upstream item id.

PR4c.3. When an authenticated API key has `reasoning_envelope_enabled = true`, Monoize MUST wrap every downstream-visible encrypted reasoning payload produced by `/v1/responses`, `/v1/chat/completions`, or `/v1/messages` in a Monoize reasoning envelope before the payload is emitted to the downstream client. The wrap step MUST occur before any response-phase transform (provider, global, or API-key) observes the corresponding `Reasoning` node, reasoning `NodeDelta`, reasoning `NodeStart`, reasoning `NodeDone`, or `ResponseDone.output` reasoning entry. After PR4c.3 wrapping, the only encrypted reasoning value visible to response-phase transforms is the `mz2.` envelope string.

PR4c.4. The Monoize reasoning envelope string format MUST be:

- prefix: `mz2.`;
- suffix: unpadded base64url encoding of a UTF-8 JSON object;
- JSON object fields:
  - `v = 2`;
  - `provider_type`: the upstream provider type string that produced the encrypted payload;
  - `model`: the upstream model string that produced the encrypted payload;
  - `item_id`: the upstream reasoning item id when known, otherwise `null`;
  - `payload`: the original encrypted reasoning payload as a JSON value.

PR4c.5. Monoize MUST apply PR4c.3 to all downstream surfaces that can carry encrypted reasoning, including non-stream terminal responses, streaming reasoning deltas, streaming `output_item.added`, streaming `output_item.done`, streaming `response.completed`, Chat Completions `reasoning_details[]`, and Anthropic Messages `thinking.signature` or `redacted_thinking.data`.

PR4c.5a. When applying PR4c.3 to a streaming reasoning delta, if the canonical URP stream event carries `reasoning_item_id` or `item_id` in event extra state, Monoize MUST store that value as the mz2 envelope `item_id`. Monoize MUST NOT emit an mz2 envelope with `item_id = null` when the stream event contains a non-empty reasoning item id.

PR4c.5b. When decoding a downstream Responses `response.output_item.added` event whose `item.type = "reasoning"` and whose item contains both non-empty `id` and `encrypted_content`, Monoize MUST preserve that `id` in the canonical stream event that carries item-level `encrypted_content`. When applying PR4c.3 to that event, Monoize MUST store the same id as the mz2 envelope `item_id`. The downstream `response.output_item.added.item.id` and the decoded mz2 envelope `item_id` MUST be equal.

PR4c.6. Before sending an upstream request, Monoize MUST inspect replayed URP `Reasoning.encrypted` values. If the value is an `mz2.` envelope and `reasoning_envelope_enabled = true`, Monoize MUST unwrap and forward the original `payload` only when both `provider_type` and `model` equal the selected upstream provider type and upstream model for the current attempt. If either value differs, Monoize MUST drop that replayed reasoning node from the upstream request.

PR4c.7. If `reasoning_envelope_enabled = false`, Monoize MUST NOT wrap newly produced downstream encrypted reasoning payloads. If a downstream request nevertheless replays an `mz2.` envelope, Monoize MAY unwrap it before upstream encoding, but MUST NOT enforce the provider/model mismatch drop defined by PR4c.6.

PR4c.8. Monoize MUST accept legacy `mz1.<item_id>.<payload>` reasoning signatures as replay input. When forwarding such a value to a Responses upstream, Monoize MUST set the reasoning item id to `<item_id>` and forward only `<payload>` as `encrypted_content`.

PR4d. When encoding URP v2 ordinary nodes into upstream `POST /v1/responses` request `input[]` messages, Monoize MUST choose content block types by message role:

- `role="user"` content MUST use request or input block types such as `input_text`, `input_image`, and `input_file`.
- `role="assistant"` message content MUST use only current Response output-message block types, including `output_text` and `refusal`. A native `image_generation_call` MUST remain a top-level output item and MUST NOT be rewritten as non-schema `output_image` message content. A file or image without a current Responses assistant-message content shape MUST remain a same-Responses provider item or be omitted on an incompatible cross-family replay.
- `role="assistant"` text or refusal history MUST NOT be encoded as `input_text`.

PR5. When parsing upstream Responses SSE, Monoize MUST support canonical Responses event payloads where:

- text deltas are carried in `delta` for `response.output_text.delta`;
- tool-call items are nested under `item` for `response.output_item.added` and `response.output_item.done`;
- argument deltas identify the call via `output_index`, not necessarily `call_id`, for `response.function_call_arguments.delta`.

PR5d. For a streamed Responses reasoning item, `response.reasoning_text.delta` and `response.reasoning_text.done` MUST update the same canonical `Reasoning.content` represented by terminal item `content[]` entries with `type = "reasoning_text"`. A terminal `response.output_item.done.item` or `response.completed.response.output[]` snapshot that carries reasoning text only in official `content[]` form MUST preserve that text in `NodeDone.node` and `ResponseDone.output`.

PR5c. When parsing upstream Responses SSE, Monoize MAY receive official image-generation tool events outside the `response.*` namespace.

- For `image_generation.completed`, if the payload carries non-empty `b64_json` or non-empty `result`, Monoize MUST decode that payload as one assistant `Image` node with `Image.source = Base64`.
- Monoize MUST treat `response.image_generation.completed` as an alias of `image_generation.completed`.
- The decoded media type MUST be derived from `output_format` using the same mapping as PR2b, defaulting to `image/png`.
- For `image_generation.partial_image`, Monoize MAY ignore the event for canonical URP node emission. Ignoring that event MUST NOT be treated as a stream error.
- Monoize MUST treat `response.image_generation.partial_image` as an alias of `image_generation.partial_image`.
- An `Image` node decoded from a Responses `image_generation_call` MUST retain the complete native top-level item in same-protocol node-local state. A same-Responses encoder MUST reconstruct the top-level `image_generation_call` item, including its id, status, result, and unknown fields. It MUST NOT place that node inside a `message.content[]` array.

PR5a. Responses stream node reconstruction:

- When upstream emits `response.output_item.added` for a top-level Responses output item of type `reasoning` or `function_call`, Monoize MUST emit a canonical `NodeStart` before any canonical `NodeDelta` derived from that output item.
- Monoize MUST decode top-level Responses `reasoning` output items as one assistant `Reasoning` node.
- Monoize MUST decode top-level Responses `function_call` output items as one assistant `ToolCall` node.
- The Responses streaming decoder MUST maintain one reconstruction slot per logical upstream output item. At minimum, the slot key space MUST include upstream `output_index`, non-empty item `id`, and non-empty function-call `call_id`.
- For one reconstruction slot, upstream `response.output_item.added`, child delta events, `response.output_item.done`, and `response.completed.response.output[]` MUST merge into one canonical node or message item sequence. They MUST NOT create a second logical output item solely because one source has a partial field set and another source has the terminal field set.
- If a `response.output_item.done` event completes a `reasoning` or `function_call` item after a prior `response.output_item.added`, the decoder MUST emit exactly one canonical `NodeDone` for the node that was started by that added event.
- If a `response.output_item.done` event completes a `reasoning` or `function_call` item without a prior `response.output_item.added`, the decoder MUST synthesize the missing `NodeStart` before emitting the terminal `NodeDone`.

PR5b. Responses stream index normalization:

- Upstream Responses `output_index` and `content_index` are upstream protocol coordinates and MUST NOT be reused as URP `node_index` by assumption alone.
- The Responses streaming decoder MUST assign URP `node_index` values sequentially starting from `0` in first-seen node order.
- The Responses streaming decoder MUST maintain enough mapping state to correlate later upstream deltas with the correct URP `node_index` and with the encoder-local downstream coordinates required by the target protocol.
- During downstream Responses re-encoding, Monoize MUST continue to emit `output_index` and `content_index` coordinates required by the Responses wire protocol, but those coordinates are encoder-local and MUST NOT alter URP `node_index` semantics.

PR5d. A same-Responses stream MUST preserve the non-empty upstream response `id` and integer `created_at` from `response.created` or `response.in_progress`. The downstream `response.created`, `response.in_progress`, and terminal Responses object MUST use that preserved identity. Unknown and optional fields present in the upstream start Responses object MUST survive downstream start-envelope reconstruction. Optional start-object fields absent upstream MUST remain absent. If no upstream start Responses object exists before the first output event, Monoize MAY synthesize the start object and its identity.

PR6. If upstream Responses streaming does not emit `response.output_text.delta` but emits assistant message text inside `response.output_item.added` or `response.output_item.done`, Monoize MUST reconstruct semantically equivalent downstream text streaming from the recovered URP v2 `Text` nodes.

PR6a. For streaming translation from upstream `type=responses` to downstream `POST /v1/chat/completions` or `POST /v1/messages`, Monoize MUST support completion-only fallback:

- If upstream sends `response.completed` with `output[]` items and Monoize has not emitted a given output class from earlier granular events, Monoize MUST synthesize the missing downstream stream items from `response.completed.output[]`.
- Output classes are assistant text, function or tool calls, and reasoning.
- Output classes also include assistant image outputs recovered from Responses `image_generation_call` items.
- This fallback MUST only fill classes that were missing in the live stream. It MUST NOT duplicate classes already emitted from earlier upstream stream events.

PR6b. Responses streaming phase preservation:

- When parsing upstream Responses SSE, Monoize MUST track active message-item context by `output_index` or equivalent synthetic index.
- If the active output item is a `message` item with field `phase`, Monoize MUST associate subsequent derived `Text` node state with that same `phase` value.
- When Monoize emits Responses-style downstream SSE, it MUST NOT merge text across distinct `phase` values or across reasoning or tool-call boundaries.
- When Monoize synthesizes missing downstream events from `response.completed.output[]`, it MUST preserve both output order and `phase` metadata from the completed payload.
- Unknown SSE event names, unknown item types, and unknown fields MUST NOT terminate streaming adaptation solely because they are unknown.
- Unknown Responses output item types in streaming, including `type = "compaction"`, MUST decode as one `ProviderItem(origin_protocol = "responses")` slot. The slot MUST produce at most one logical `ProviderItem` in `NodeDone.node` and `ResponseDone.output`.
- When Monoize emits downstream `/v1/responses` SSE from flat URP v2 nodes, it MUST emit canonical Responses output-item boundaries derived by encoder reconstruction, not raw upstream boundaries.
- For downstream `/v1/responses` SSE, every `response.output_item.done` event MUST have exactly one earlier matching `response.output_item.added` event with the same `output_index`.
- Monoize MUST emit `response.content_part.added` and `response.content_part.done` for content-bearing Responses parts. This includes message parts and a reasoning item's `content[]` entry with `type="reasoning_text"`. A RawCoT lifecycle MUST use one reasoning node for output-item, content-part, reasoning-text delta, content-part done, and output-item done events.

PR6c. Final Responses-stream object synthesis:

- If the upstream Responses stream terminates without one of `response.completed`, `response.incomplete`, `response.failed`, or `response.cancelled`, Monoize MUST emit a terminal canonical `Error`. EOF or `[DONE]` without a protocol terminal response event MUST NOT synthesize `status="completed"` or `finish_reason="stop"`.
- If the upstream Responses stream already emitted `response.completed`, Monoize MUST forward at most one corresponding `ResponseDone`. Monoize MUST NOT emit a duplicate synthetic terminal response after forwarding the upstream terminal response.
- `response.incomplete`, `response.failed`, and `response.cancelled` MUST preserve the exact response status, `incomplete_details`, and `error` state in `ResponseDone.extra_body` or a stricter typed equivalent. A downstream Responses encoder MUST emit the matching terminal event name and MUST NOT rewrite it to `response.completed`.
- The synthesized or forwarded `ResponseDone.output` value MUST be the complete terminal flat node sequence.
- If upstream `response.completed.response.output[]` is present, Monoize MUST merge it into the decoder's reconstruction slots before emitting `ResponseDone`. The merge key MUST prefer non-empty item `id` or non-empty function-call `call_id`. If a completed item omits `id`, Monoize MAY copy an item id from the stream slot at the same array position only when both items have the same output item class. If no identity key matches, Monoize MAY treat the completed array position as an `output_index` match only when the accumulated slot has the same output item class. If the completed array position points to a different output item class, Monoize MUST NOT copy that slot's item id or treat that position alone as same-slot evidence. In that case Monoize MUST first try an unmatched accumulated slot with the same output item class; if no such slot can be merged, Monoize MUST append the terminal item as completed-only state.
- During that merge, a missing field or an empty string in one source MAY be filled from a non-empty value in another source. If both sources provide a non-empty typed semantic field for the same slot and the values differ, Monoize MUST treat the upstream stream as inconsistent.
- For a reasoning slot with multiple `response.reasoning_summary_text.*` parts, Monoize MUST aggregate summary text by `summary_index` and MUST concatenate non-empty parts without injecting a separator before comparing the stream accumulator with `response.completed.response.output[].summary[]`.
- If `response.output_item.done` carries a reasoning item snapshot after Monoize has already accumulated non-empty reasoning text, summary, or encrypted content from prior events for the same slot, Monoize MUST NOT overwrite those accumulated fields with the done snapshot. The done snapshot MAY fill fields that are still missing or empty.
- On such an inconsistent terminal merge, Monoize MUST emit a terminal canonical `Error` event with code `responses_terminal_conflict`, and downstream `/v1/responses` MUST terminate as `response.failed` followed by the plain `[DONE]` sentinel. Monoize MUST NOT emit a successful `ResponseDone` for that stream.
- When accumulated assistant text is empty, Monoize MUST NOT synthesize an empty assistant `Text` node solely to carry that empty string.
- When Monoize emits downstream `response.completed` from `ResponseDone.output`, the `response.output` array MUST be encoded with the same Responses encoder logic used for non-streaming responses so that top-level `reasoning`, `message`, `function_call`, and same-protocol `ProviderItem` items remain in canonical Responses output positions rather than being nested incorrectly inside `message.content[]`.
- If Monoize synthesizes missing assistant text stream events because upstream omitted text deltas, it MUST synthesize one downstream text segment per recovered text-bearing node run in output order. Each synthesized segment MUST preserve `phase` metadata and MUST allocate fresh encoder-local coordinates instead of reusing URP `node_index` as a wire coordinate.

PR7. For URP v2 `ToolResult` nodes with non-string multimodal output, Monoize MUST preserve multimodal output parts when forwarding to Responses upstream:

- text parts as `input_text`;
- image parts as `input_image`;
- file or document parts as `input_file`.

PR8. For upstream Responses requests, Monoize MUST parse `function_call_output.output` whether it is:

- string text; or
- an array or object content payload with `input_text`, `input_image`, and `input_file`.

Parsed data MUST become one URP v2 `ToolResult` node without dropping image or file parts.

PR8a. Monoize MUST decode Responses `custom_tool_call` items into `ToolCall(tool_type = "custom")` using `input` as the canonical freeform argument string. It MUST decode `custom_tool_call_output` into `ToolResult(tool_type = "custom")`. Responses encoding MUST restore those exact item types. Responses streaming MUST map `response.custom_tool_call_input.delta` and `response.custom_tool_call_input.done` to the same canonical tool-argument delta lifecycle used for function calls, and MUST restore the custom event names when encoding a custom call.

### 7.3 Provider adapter: `type=chat_completion`

PC1. Monoize MUST call the upstream path `POST /v1/chat/completions`.

PC2. Monoize MUST convert `UrpRequestV2.input` nodes into chat `messages[]` as follows:

- ordinary nodes become chat messages or message content in source order using encoder-owned grouping;
- assistant function and custom `ToolCall` nodes become assistant `tool_calls[]` entries, grouping consecutive tool-call nodes when needed to preserve parallel calls;
- top-level `ToolResult` nodes become chat `role="tool"` messages.

PC2a. Chat `phase` mapping:

- Monoize MUST accept optional extension field `phase` on chat assistant messages.
- When decoding a chat assistant message, Monoize MUST copy `phase` onto every URP v2 `Text` node derived from that assistant message.
- When encoding URP v2 assistant text nodes back to chat messages, Monoize MUST split assistant messages whenever flattening would combine text with different `phase` values or combine text across reasoning or tool-call boundaries.

PC2.1. Input coercion for chat adapter:

- If downstream `input` is a string, Monoize MUST treat it as one user message.
- If downstream `input` is an object with message-like fields `role` and `content` but without explicit `type`, Monoize MUST treat it as one message input object.
- If downstream `input` is an array containing message-like objects without `type`, Monoize MUST treat each such object as one message input object.

PC2.2. Content-block extra preservation for chat adapter:

- When encoding URP v2 nodes to upstream chat `messages[].content[]` blocks, Monoize MUST merge each node's `extra_body` into the generated block object, subject to the precedence rule in §7.6.
- Monoize MAY collapse a single text block to scalar string `content` only when that block has no extra fields beyond adapter-generated keys.
- If a single text block contains any extra field, for example `cache_control`, Monoize MUST keep array or block form and MUST preserve that extra field in the encoded block.

PC2.3. Chat content-block compatibility decode:

- For upstream `type=chat_completion` request and response bodies, Monoize MUST accept assistant `content` encoded as an array of block objects instead of scalar string text.
- When an assistant `content[]` block has `type="text"` or `type="output_text"` and non-empty `text`, Monoize MUST decode that block into a URP assistant `Text` node in array order.
- When an assistant `content[]` block has `type="tool_call"`, `type="function_call"`, or `type="tool_use"`, Monoize MUST decode that block into a URP assistant `ToolCall` node in array order.
- For a `tool_call`, `function_call`, or `tool_use` content block, Monoize MUST resolve fields as follows:
  - `call_id` from `call_id`, else `id`;
  - `name` from `name`, else `function.name`;
  - `arguments` from `arguments`, else `function.arguments`, else `input`, else `function.input`, else `args`, else `function.args`.
- If the resolved `arguments` value is not a string, Monoize MUST serialize it as JSON.
- Any other object content block in `messages[].content[]` or assistant `content[]` MUST decode as `ProviderItem(origin_protocol = "chat_completion")`.
- When encoding to `type=chat_completion`, Monoize MUST replay only `ProviderItem` nodes whose `origin_protocol = "chat_completion"` as native `messages[].content[]` parts. Other ProviderItems MUST be omitted.

PC2.4. Assistant message history preservation for chat adapter:

- When decoding downstream Chat Completions history, if one assistant message contains both non-empty `content` and `tool_calls`, Monoize MUST preserve both within the same assistant turn by emitting flat URP v2 nodes in source order rather than discarding either surface.
- When encoding that assistant turn back to an upstream `type=chat_completion` request, Monoize MAY encode the assistant text or refusal content and the assistant `tool_calls` on the same upstream `messages[]` element when the target protocol supports that combined form.

PC2.5. Chat Completions file and audio input content:

- A content part `{type:"file",file:{file_id:<id>}}` MUST decode as `FileSource::FileId` with `_monoize_file_id_origin = "openai"`.
- A content part `{type:"file",file:{file_data:<base64>,filename?:<name>}}` MUST decode as `FileSource::Base64`. The decoder MUST preserve the optional filename. Because Chat does not carry a media type, the canonical media type MUST be `application/octet-stream`.
- A content part `{type:"input_audio",input_audio:{data:<base64>,format:"wav"|"mp3"}}` MUST decode as `AudioSource::Base64` with media type `audio/wav` or `audio/mpeg` respectively.
- A Chat encoder MUST encode an OpenAI-origin `FileSource::FileId` using the nested `file.file_id` shape and a `FileSource::Base64` using nested `file.file_data` plus optional `file.filename`. It MUST omit `FileSource::Url`, `FileSource::Text`, and `FileSource::Content` because Chat has no native mapping for those source variants. It MUST NOT invent a bracketed text marker for an unsupported file.
- A Chat encoder MUST encode `AudioSource::Base64` as `input_audio` when the media type maps to `wav` or `mp3`. It MUST omit an audio URL and any unsupported audio media type.
- Responses create and Messages have no current input-audio content mapping. Their encoders MUST omit `Audio` nodes. Gemini MAY encode canonical audio using its native inline-data or file-data surface.

PC2.6. Responses `input_file.file_data` and Chat `file.file_data` are base64 string fields and do not define a sibling `media_type` field. A Responses or Chat encoder MUST NOT emit `media_type` beside these native file-data fields.

PC2.7. A Chat request decoder MUST map top-level `stop`, `verbosity`, and `user` into typed `UrpRequestV2.stop`, `UrpRequestV2.verbosity`, and `UrpRequestV2.user`. A Chat encoder MUST restore scalar-versus-array `stop` shape, emit top-level `verbosity`, and emit top-level `user`. Typed fields MUST win collisions with `extra_body`.

PC2.8. Chat assistant audio envelope preservation:

- A Chat decoder that receives a non-null assistant `message.audio` object MUST emit one `ProviderItem` with `origin_protocol = "chat_completion"`, `item_type = "audio"`, and `body` equal to the complete audio object. The provider item MUST carry an internal marker that identifies it as a Chat message-level audio field.
- A Chat request or response encoder MUST consume that marked provider item as the enclosing message's `audio` field. It MUST NOT place the audio object in `messages[].content[]` or `choices[].message.content[]`.
- A marked audio provider item MUST make an audio-only assistant message consumable even when `content = null`. The same-family encoded message MUST contain `audio` and MUST contain `content = null` for a response or an empty request content value when the request schema requires content.
- A non-Chat encoder MUST omit the marked provider item under `PI5` and `PI6`. The internal marker MUST NOT appear on the wire.

PC2.9. Deprecated Chat function-call lifecycle preservation:

- Each object in a Chat request's deprecated top-level `functions[]` array MUST decode as one semantic function `ToolDefinition`. The decoder MUST copy `name`, `description`, `parameters`, and `strict` into `FunctionDefinition`, preserve any other function-owned fields in `FunctionDefinition.extra_body`, and attach an internal legacy-function-definition marker to the enclosing tool definition.
- A Chat request's deprecated top-level `function_call` control MUST decode into semantic `tool_choice` when top-level `tool_choice` is absent. A string value MUST decode as the corresponding `ToolChoice::Mode`. An object with `name = N` MUST decode as the semantic specific choice `{type:"function",function:{name:N}}`. Unknown fields from the deprecated object MUST remain available in internal request metadata for same-Chat replay. If both `tool_choice` and `function_call` are present, `tool_choice` MUST control the semantic field and the decoder MUST NOT attach legacy-choice provenance.
- A Chat request encoder MUST partition semantic tools by provenance. It MUST emit a marked legacy function definition only in `functions[]` and an unmarked Chat-valid tool definition only in `tools[]`; it MUST NOT emit one semantic definition in both arrays. When semantic `tool_choice` carries legacy-choice request provenance, the encoder MUST emit only `function_call`; otherwise it MUST emit only `tool_choice`. A marked legacy specific choice MUST restore its unknown deprecated-object fields, while semantic `name` wins a collision.
- Responses, Messages, and other non-Chat encoders MUST ignore the internal legacy-definition and legacy-choice markers and encode the normalized semantic tools and tool choice using their target-family shapes.
- An assistant `function_call = {name, arguments}` object MUST decode as one `ToolCall` with `tool_type = function`, `name` and `arguments` copied exactly, deterministic `call_id = "legacy_function:" + name`, and an internal legacy-function-call marker.
- A request message with `role = "function"`, `name = N`, and `content = C` MUST decode as one `ToolResult` with `tool_type = function`, `call_id = "legacy_function:" + N`, text content `C`, and an internal legacy-function-result marker containing `N`.
- A Chat encoder MUST restore a marked legacy call as `function_call` rather than `tool_calls[]`. It MUST restore a marked legacy result as `role = "function"`, `name`, and `content` rather than `role = "tool"` and `tool_call_id`.
- A Chat stream decoder MUST normalize `delta.function_call` and terminal `message.function_call` through the same marked `ToolCall` lifecycle. A Chat stream encoder MUST emit marked call chunks under `delta.function_call`, preserve argument-fragment order, and use terminal `finish_reason = "function_call"`.
- Legacy markers are internal metadata under `XTRA-10`. They MUST remain available until target encoding and MUST NOT appear on the wire.

PC3. Tool descriptor normalization:

- For `type=chat_completion` upstreams, Monoize MUST ensure upstream `tools[]` contains only Chat-valid tool descriptors.
- A URP tool with `type = "function"` MUST encode as a Chat tool descriptor with shape `{ "type": "function", "function": { ... } }`.
- A URP tool with `type = "custom"` MUST encode as a Chat tool descriptor with shape `{ "type": "custom", "custom": { ... } }`.
- For Chat custom tools, unknown fields owned by the nested `custom` object MUST remain inside that nested `custom` object after re-encoding.
- For Chat custom tools, unknown fields owned by the top-level tool descriptor MAY be emitted only at the top-level Chat tool descriptor layer.
- A URP tool with `type` other than `"function"` or `"custom"` MUST NOT be emitted as a raw Chat tool descriptor unless a later rule explicitly defines a Chat-valid encoding for that tool type.

PC4. Monoize MUST convert chat-completions non-stream output into `UrpResponseV2` output nodes.

PC5. Monoize MUST convert chat-completions streaming deltas into canonical URP v2 stream events.

PC6. Tool-calling, non-stream:

- If the upstream chat-completions response contains `choices[0].message.tool_calls[]`, Monoize MUST convert each entry into one URP assistant `ToolCall` node using:
  - `call_id = tool_calls[i].id`
  - `name = tool_calls[i].function.name`
  - `arguments = tool_calls[i].function.arguments`, string. If the upstream sends a JSON object, Monoize MUST serialize it as JSON.

- If `tool_calls[i].type = "custom"`, Monoize MUST instead create `ToolCall(tool_type = "custom")` using `custom.name` and `custom.input`. If `custom.input` is not a string, Monoize MUST serialize it as JSON text.

PC7. Tool-calling, stream:

- If the upstream chat-completions stream contains `choices[0].delta.tool_calls[]`, Monoize MUST convert the deltas into canonical URP v2 stream events such that downstream Responses, Messages, and Chat Completions encoders can emit protocol-correct tool-call lifecycles.
- The stream decoder MUST recognize both `type="function"` with `function.arguments` and `type="custom"` with `custom.input`; the stream encoder MUST restore the matching nested member and discriminator.

PC7a. For downstream `POST /v1/chat/completions` translated from upstream `type=chat_completion` streaming:

- If upstream already emitted at least one terminal chunk with non-null `choices[0].finish_reason`, Monoize MUST NOT append an additional synthetic terminal chat chunk with a different `finish_reason`.
- Monoize MUST preserve upstream terminal finish semantics, for example `tool_calls`, so downstream clients can continue tool loops correctly.

PC7b. If an upstream `type=chat_completion` stream emits any tool-call presence, either tool-call header chunks or argument deltas via `choices[0].delta.tool_calls[]`, in a turn, but emits terminal `choices[0].finish_reason = "stop"`, Monoize MUST normalize downstream terminal finish reason to `tool_calls` for `POST /v1/chat/completions`.

PC7c. For downstream `POST /v1/chat/completions` translated from upstream `type=chat_completion` streaming, every non-terminal downstream chunk that carries assistant deltas, including `content`, `reasoning_details`, and `tool_calls`, MUST emit `choices[0].finish_reason = null`.

PC7d. For downstream `POST /v1/chat/completions` translated from upstream `type=chat_completion` streaming, Monoize MUST emit at most one downstream chunk with non-null `choices[0].finish_reason`, and that terminal chunk MUST be emitted only after the last downstream assistant delta of the turn.

PC7e. For upstream `type=chat_completion` streaming chunks that carry assistant state as `choices[0].delta.content[]` block arrays:

- Monoize MUST decode text blocks and tool-call blocks in block-array order using the same field mapping as PC2.3.
- Monoize MUST decode unknown block-array objects as `ProviderItem(origin_protocol = "chat_completion")` in block-array order.
- If one streamed assistant turn contains both text blocks and at least one tool-call block, downstream `POST /v1/chat/completions` streaming MUST preserve the assistant text deltas that precede the tool-call deltas, MUST emit the tool-call deltas after those text deltas, and MUST terminate the turn with `finish_reason="tool_calls"`.

PC7f. For upstream `type=chat_completion` streaming chunks that carry assistant state in terminal `choices[0].message` snapshots instead of incremental `choices[0].delta.tool_calls[]`:

- Monoize MUST decode the snapshot message using the same text and tool-call compatibility rules as PC2.3.
- If the snapshot contains assistant tool calls that were not previously emitted as downstream deltas, Monoize MUST emit equivalent downstream tool-call deltas before the terminal downstream chunk.
- If the snapshot contains assistant text or reasoning suffix bytes that were not previously emitted as downstream deltas, Monoize MUST emit those missing suffix deltas before the terminal downstream chunk.
- A non-internal unknown field on the terminal message snapshot MUST appear exactly once on a downstream `choices[0].delta` object before the terminal chunk. A field whose name starts with `_monoize_` MUST NOT appear on the downstream wire.

PC7g. When a downstream Chat Completions stream encoder reconstructs missing output from `ResponseDone.output`, duplicate suppression MUST be keyed by URP `node_index`. Emission evidence for node index `K` MUST NOT suppress a terminal-only node at a different index `J`, even when both nodes are tool calls or both nodes carry the same reasoning surface type. The encoder MUST emit every terminal-only Chat-visible node in `ResponseDone.output` order before the terminal finish chunk.

PC8. Reasoning, non-stream and stream:

- Monoize MUST parse upstream Chat Completions reasoning from `choices[0].message.reasoning_details[]` and `choices[0].message.reasoning`.
- For `reasoning_details[]`, Monoize MUST interpret entries as follows:
  - `type="reasoning.text"`: `text` contributes to `Reasoning.content`.
  - `type="reasoning.encrypted"`: `data` contributes to `Reasoning.encrypted`.
  - `type="reasoning.summary"`: `summary` contributes to `Reasoning.summary` and to the simple `reasoning` alias when no `reasoning.text` content is available.
  - `type="reasoning.server_tool_call"`: the complete native entry is preserved for same-Chat replay; it does not become a local tool call.
- Every detail is one ordered URP `Reasoning` node. The decoder MUST preserve repeated detail types, `id`, `format`, `index`, `reasoning.text.signature`, server-tool fields, and unknown entry-local fields. It MUST NOT merge or deduplicate details.
- For streaming, Monoize MUST apply the same mapping to `choices[0].delta.reasoning_details[]` deltas in arrival order.
- Monoize MUST store parsed reasoning as URP v2 `Reasoning` nodes.
- `_monoize_chat_reasoning_detail`, `_monoize_chat_reasoning_surface`, and every other `_monoize_` key are internal metadata. They MUST NOT appear as fields of an upstream `messages[]` object. A same-Chat encoder MUST consume `_monoize_chat_reasoning_detail` to reconstruct the raw `reasoning_details[]` entry, preserving its native fields, then omit the wrapper key.
- Backward compatibility: if `reasoning` and `reasoning_details` are absent, Monoize MUST still accept legacy `reasoning_content` and `reasoning_opaque` from upstream chat outputs.

PC9. For an upstream `type=chat_completion` request with `stream=true`, the Chat Completions encoder MUST set `stream_options.include_usage = true` when the outbound request body does not already contain that field. For `stream=false` or absent, it MUST NOT synthesize `stream_options`; non-streaming responses already carry usage in the response object.

### 7.4 Provider adapter: `type=messages`

PM1. Monoize MUST call the upstream path `POST /v1/messages`.

PM2. Monoize MUST convert `UrpRequestV2.input` nodes into Messages `messages[]` using encoder-owned reconstruction of role and content blocks.

PM2a. Messages `phase` mapping:

- Monoize MUST accept optional extension field `phase` on Anthropic `text` blocks.
- When decoding an Anthropic `text` block, Monoize MUST copy `phase` onto the URP v2 `Text` node derived from that block.
- When encoding a URP v2 `Text` node to an Anthropic `text` block, Monoize MUST write `phase` on that block when the text node carries non-null `phase`.

PM2b. For downstream `POST /v1/messages` request parsing, Monoize MUST decode ordinary `messages[].content[]` blocks with `type = "image"` into role-bearing URP `Image` nodes. The decoder MUST support the Anthropic image source shapes below:

- `source: { type: "base64", media_type: <media type>, data: <raw base64> }` -> `ImageSource::Base64`;
- `source: { type: "url", url: <url> }` -> `ImageSource::Url`;
- `source: { type: "file", file_id: <file id> }` -> `ImageSource::FileId`.

PM2c. For downstream `POST /v1/messages` request parsing, Monoize MUST decode ordinary `messages[].content[]` blocks with `type = "document"` or `type = "file"` into role-bearing URP `File` nodes when a supported file source is present.

PM2c.1. The Messages adapter MUST decode and encode the following document source shapes:

- `{ type: "url", url: <url> }` as `FileSource::Url`;
- `{ type: "base64", media_type: <media type>, data: <raw base64> }` as `FileSource::Base64`;
- `{ type: "file", file_id: <file id> }` as `FileSource::FileId`;
- `{ type: "text", media_type: "text/plain", data: <text> }` as `FileSource::Text`;
- `{ type: "content", content: <content block array> }` as `FileSource::Content`.

PM2c.2. A Messages encoder MUST NOT add `filename` to a document source of type `base64`. When `_monoize_file_id_origin = "messages"`, a Messages encoder MUST encode `ImageSource::FileId` and `FileSource::FileId` as nested Anthropic sources with `type = "file"` and the original `file_id`. It MUST omit a typed file identifier whose origin marker is absent or belongs to another protocol family under `URPV2-13b`.

PM2d. For downstream `POST /v1/messages` request parsing and upstream `type=messages` response parsing, Monoize MUST decode any unknown `content[]` block object as `ProviderItem(origin_protocol = "messages")`. This rule applies to both ordinary message content and the top-level `system` block array. When encoding to `type=messages`, Monoize MUST replay only `ProviderItem` nodes whose `origin_protocol = "messages"` as native `content[]` or `system[]` blocks in source order. Other ProviderItems MUST be omitted.

PM2.1. Input coercion for Messages adapter:

- If downstream `input` is a string, Monoize MUST treat it as one user text message.
- If downstream `input` is an object with message-like fields `role` and `content` but without explicit `type`, Monoize MUST treat it as one message input object.
- If downstream `input` is an array containing message-like objects without `type`, Monoize MUST treat each such object as one message input object.

PM3. Monoize MUST convert Messages output into `UrpResponseV2` output nodes.

PM4. Tool-calling:

- When the upstream Messages output contains `tool_use` blocks, Monoize MUST convert each block into one URP assistant `ToolCall` node.
- When a downstream Messages request contains `tool_result` blocks, Monoize MUST convert them into top-level URP `ToolResult` nodes.
- When encoding a request to an upstream Messages provider, Monoize MUST NOT emit a `text` content block whose `text` value is the empty string unless that block carries at least one non-semantic extra field.
- When encoding two or more consecutive URP `ToolResult` nodes to an upstream Messages provider, Monoize MUST encode them as consecutive `tool_result` blocks inside one user `messages[]` entry. Monoize MUST NOT split consecutive `ToolResult` nodes into multiple adjacent user `messages[]` entries.
- When a user `messages[]` entry contains one or more encoded `tool_result` blocks, those blocks MUST appear before any non-`tool_result` content block in that same user entry.

PM4.1. When parsing downstream Messages `tool_result.content`, Monoize MUST support:

- string text content; and
- block-array content where blocks may include `text`, `image`, and `document`.

PM4.2. For PM4.1 block-array content, Monoize MUST map blocks to `ToolResult.content` entries:

- `text` -> text entry;
- `image` -> image entry;
- `document` -> file entry.

PM4.3. When parsing upstream Messages assistant output, Monoize MUST support multimodal output blocks `image` and `document` in addition to `text`, `thinking`, and `tool_use`.

PM4.4. When encoding a request to an upstream `type=messages` provider, Monoize MUST NOT emit an upstream request field named `response_format`. If the URP request carries `ResponseFormat::JsonSchema`, Monoize MUST encode it as `output_config.format = { "type": "json_schema", "schema": <S> }`. When no explicit Messages `output_config.format` passthrough object exists, the generated Messages format object MUST NOT contain the OpenAI-only schema `name`, `description`, or `strict` members. `ResponseFormat::Text` and `ResponseFormat::JsonObject` have no Messages equivalent and MUST NOT produce an `output_config.format` member.

PM4.4.1. When an explicit Messages `output_config` and typed `response_format` are both present, all non-`format` members of the explicit `output_config` MUST remain unchanged. A typed `ResponseFormat::JsonSchema` MUST replace the semantic `type` and `schema` members of the preserved `format` object while retaining its other same-Messages passthrough members. A typed `ResponseFormat::Text` or `ResponseFormat::JsonObject` MUST remove the preserved `format` member. Thus typed state wins every semantic format collision while unknown `output_config` siblings survive.

PM4.5. A Messages request decoder MUST map `stop_sequences` to `UrpRequestV2.stop = Multiple(values)` and `metadata.user_id` to typed `UrpRequestV2.user`. It MUST preserve all non-`user_id` `metadata` members in `extra_body.metadata`. A Messages encoder MUST map `StopControl::Single(s)` to `stop_sequences = [s]`, map `StopControl::Multiple(values)` to the same array, and map typed `user` to `metadata.user_id`. It MUST merge preserved metadata siblings into the emitted object, and typed `user` MUST win a collision with preserved `metadata.user_id`. A Messages encoder MUST omit typed `verbosity`.

PM5. Reasoning:

- When the upstream Messages output contains a `thinking` block, Monoize MUST convert it into one URP `Reasoning` node. The block's non-empty `thinking` string is provider-supplied summarized thinking and MUST map to `Reasoning.summary`; `Reasoning.content` MUST remain absent. The block's non-empty `signature` string MUST map to `Reasoning.encrypted` under PM5b.
- If a decoded Messages `thinking` block contains non-empty summarized thinking and its signature does not recover a non-empty reasoning item id under PM5b, the decoder MUST set `Reasoning.extra_body["_monoize_reasoning_downstream_only_presentation"] = true`. A Responses request encoder MUST omit a reasoning node with this marker because the summarized Messages presentation is not a Responses RawCoT replay item. A same-Messages encoder MUST ignore the marker and reconstruct the original thinking block. If PM5b recovers a non-empty reasoning item id, the decoder MUST NOT set this marker solely because the block uses the Messages shape.
- A regular `thinking` block with `thinking = ""` and a non-empty `signature` is omitted thinking, not redacted thinking. Monoize MUST retain it as one `Reasoning` node with absent content and summary and with the signature in `encrypted`.
- When an upstream Messages stream contains a non-empty `thinking_delta.thinking` fragment, Monoize MUST emit `NodeDelta::Reasoning` with that fragment in `summary` and with `content` absent. The same delta event MUST carry `extra_body["_monoize_summary_from_messages_thinking"] = true`. The terminal `Reasoning.summary` MUST equal the ordered concatenation of those fragments.
- When the upstream Messages output contains a `redacted_thinking` block, Monoize MUST convert it into one URP `Reasoning` node with:
  - `content` absent,
  - `encrypted` set to the block's `data` value (preserved verbatim, as a JSON string when the wire value is a string),
  - `extra_body["_monoize_reasoning_kind"]` set to the string `"redacted_thinking"` so that a later encode step targeting Messages can reconstruct the original block type.

PM5a. For downstream `POST /v1/messages` request parsing, Monoize MUST accept assistant content blocks of type `thinking` and `redacted_thinking` inside `messages[].content[]` and decode them into URP `Reasoning` nodes using the same field mapping as PM5. An input `thinking` block whose `thinking` string is empty and whose `signature` is present MUST still be decoded into a `Reasoning` node with `content` and `summary` absent and `encrypted` set from `signature`.

PM5b. Reasoning identifier transport through Anthropic Messages uses a **signature sigil** rather than a custom content-block field. The sigil wraps the opaque payload the upstream attached to a `thinking` or `redacted_thinking` block.

Sigil format:

```
mz1.<item_id>.<original_signature>
```

- The literal prefix is `mz1.` (version 1 of the sigil format).
- `<item_id>` is the reasoning item identifier to transport (for example an OpenAI Responses `rs_...` id). It MUST be non-empty and MUST NOT contain a `.`.
- `<original_signature>` is the raw opaque payload produced by the originating upstream.

Decoder behavior (for both downstream request parsing and upstream response parsing):

1. If an Anthropic `thinking.signature` or `redacted_thinking.data` field begins with `mz1.`, Monoize MUST split the string on the first `.` after the prefix, set `Reasoning.id` to the left segment, and set `Reasoning.encrypted` to the right segment (the original signature payload).
2. If either sigil segment is empty, Monoize MUST treat the field as an opaque non-sigil signature: `Reasoning.id` MUST remain `None` and `Reasoning.encrypted` MUST be set to the full original string.
3. If the field does not begin with `mz1.`, Monoize MUST store the full value in `Reasoning.encrypted` without interpretation.

Encoder behavior uses two directional modes:

- **Downstream response direction** (Monoize → client of `/v1/messages`): When a `Reasoning` node carries both a non-empty `Reasoning.id` and a non-empty `Reasoning.encrypted`, Monoize MUST attach the sigil to the emitted `thinking.signature` (or `redacted_thinking.data`). When `Reasoning.id` is absent or empty, Monoize MUST emit the original signature verbatim.
- **Upstream request direction** (Monoize → `type=messages` upstream): Monoize MUST detect any sigil-prefixed signature inside the URP node, strip the `mz1.<id>.` prefix, and forward only `<original_signature>`. The real Anthropic API MUST NEVER see a sigil-prefixed signature, because Anthropic's own signature validation would reject it.

A sigil-prefixed signature MUST NOT be wrapped a second time. The sigil helpers are idempotent: wrapping a value that already starts with `mz1.` returns the value unchanged.

PM5c. For every URP `Reasoning` node, Monoize MUST preserve `Reasoning.id` whenever it is present in URP storage. In particular, `Reasoning.id` MUST NOT be dropped by any Anthropic Messages decode step, encode step, or transform unless a spec rule in this file explicitly authorizes that drop.

PM5d. When encoding a URP v2 request to an upstream `type=messages` provider, Monoize MUST render URP `Reasoning` nodes inside `messages[].content[]` using the same decision rule as DM5.1 and the same invariants as DM5.2 and DM5.3. This applies to both non-streaming and streaming upstream request construction. Monoize MUST NOT emit any Anthropic content block that violates DM5.3.

PM5e. When encoding a URP v2 request to an upstream `type=responses` provider, Monoize MUST preserve `Reasoning.id` as the upstream reasoning item `id`. If `Reasoning.id` is a non-empty string, the upstream `reasoning` item `id` MUST equal that string byte-for-byte. Monoize MUST NOT synthesize a fresh `rs_...` identifier solely because the URP node originated from a different protocol family. This rule is required because OpenAI Responses `encrypted_content` is cryptographically bound to the reasoning item `id`, and regenerating the id invalidates the signature.

PM6. Monoize MUST convert Messages streaming deltas into canonical URP v2 stream events.

PM6.1. Messages message-envelope unknown fields MUST decode into `NextDownstreamEnvelopeExtra` immediately before the first node derived from that message. They MUST NOT be copied into the first content block node. Block-local fields such as `cache_control`, `citations`, and `caller` remain only on that block node.

PM6.2. Monoize MUST preserve all current Anthropic stop reasons: `end_turn`, `max_tokens`, `stop_sequence`, `tool_use`, `pause_turn`, `refusal`, and `model_context_window_exceeded`. A same-Messages response MUST re-emit the exact source reason. In particular, `pause_turn`, `refusal`, and context-window exhaustion MUST NOT become `end_turn`.

PM6.3. `message_delta.usage` is a partial cumulative update. Missing or null counters MUST retain the latest prior cumulative counter; a present counter replaces that counter. Multiple `message_delta` events MUST merge without producing multiple canonical terminal responses. Monoize MUST emit exactly one `ResponseDone` after `message_stop` or other proven terminal evidence.

PM6a. For Messages streaming, unknown `content_block_start.content_block` block types MUST decode as `ProviderItem(origin_protocol = "messages")`. Every `content_block_delta.delta` object for that block MUST decode as an ordered `NodeDelta::ProviderItem` value. Downstream Messages stream encoding MUST emit a ProviderItem content-block lifecycle only for same-protocol ProviderItems, and it MUST replay each ordered ProviderItem delta as the native `content_block_delta.delta` object without reclassifying the block as a typed client `tool_use`. For an opaque block such as `server_tool_use`, ordered `input_json_delta.partial_json` fragments MUST assemble `NodeDone.node.body.input` and the corresponding `ResponseDone.output` body input. A valid assembled JSON string MUST become its JSON value; it MUST NOT remain a quoted JSON string.

PM6b. A Messages stream decoder MUST accumulate usage across `message_start.message.usage` and every later `message_delta.usage` object. For each usage counter, a missing or JSON `null` value in a later event MUST retain the most recent numeric value for that counter. A numeric value in a later event MUST replace the prior value for that counter because Anthropic stream usage counters are cumulative, not incremental. In particular, a `message_delta.usage` object that contains only `output_tokens` MUST retain `input_tokens`, cache-read tokens, cache-creation tokens, cache-creation TTL splits, tool-prompt tokens, and output-token detail counters from `message_start.message.usage`. The decoder MUST map `output_tokens_details.thinking_tokens` to `Usage.output_details.reasoning_tokens`. The decoder MUST apply the aggregate `Usage.input_tokens` normalization in C3-ii only after merging the wire counters.

PM6c. A Messages stream decoder MUST NOT emit canonical `ResponseDone` merely because it receives a `message_delta`. It MUST retain the most recent non-null `message_delta.delta.stop_reason` and the most recent present `message_delta.delta.stop_sequence` value, and it MUST continue consuming later `message_delta` events so that later cumulative usage is observed. The decoder MUST store those retained terminal values in `ResponseDone.extra_body.stop_reason` and `ResponseDone.extra_body.stop_sequence`, respectively. The decoder MUST emit exactly one `ResponseDone` when `message_stop` is received. If the upstream byte stream closes without `message_stop`, the decoder MAY emit exactly one `ResponseDone` only when a prior `message_delta` supplied a non-null stop reason or when an explicit terminal `[DONE]` sentinel was received. An unmarked end-of-file MUST NOT be converted to a successful `ResponseDone`.

PM7. Messages `tool_choice` normalization:

- For downstream `POST /v1/messages`, Monoize MUST normalize Anthropic-style `tool_choice` values into URP-compatible `tool_choice` before forwarding.
- At minimum, Monoize MUST support:
  - `{ "type": "auto" }` -> `"auto"`
  - `{ "type": "any" }` -> `"required"`
  - `{ "type": "tool", "name": "<N>" }` -> `{ "type": "function", "function": { "name": "<N>" } }`
- If an Anthropic Messages `tool_choice` object has boolean `disable_parallel_tool_use` and `type` is `auto`, `any`, or `tool`, Monoize MUST preserve that flag as request-level tool-choice semantics. It MUST NOT store the flag in any tool descriptor or tool descriptor `extra_body`.
- When a preserved Anthropic `any` choice is encoded back to a Messages upstream, Monoize MUST emit `{ "type": "any" }`, not `{ "type": "required" }`. If `disable_parallel_tool_use` was present, the emitted object MUST include the same boolean flag.
- When a preserved Anthropic named `tool` choice is encoded back to a Messages upstream, Monoize MUST emit `{ "type": "tool", "name": "<N>" }`. If `disable_parallel_tool_use` was present, the emitted object MUST include the same boolean flag.
- For OpenAI-compatible upstream requests, `parallel_tool_calls` is a top-level request field. Monoize MUST emit it at the request object top level when the canonical request carries a boolean value, and MUST NOT nest it under `tools[]`, `function`, `custom`, or any other tool descriptor object.

PM8. When calling a `type=messages` upstream, Monoize MUST send HTTP header `anthropic-version` with value `2023-06-01`.

PM8d. When an encoded `type=messages` upstream request contains an `image` or `document` block whose `source.type = "file"` and whose source has a non-empty `file_id`, or a `container_upload` block with a non-empty `file_id`, Monoize MUST also send HTTP header `anthropic-beta` with value `files-api-2025-04-14`. Monoize MUST NOT add this beta header solely because an unrelated string field happens to equal a file identifier.

PM8a. When decoding Anthropic Messages usage, Monoize MUST map cache usage as follows:

- wire `cache_read_input_tokens` -> `Usage.input_details.cache_read_tokens`;
- wire `cache_creation_input_tokens` -> `Usage.input_details.cache_creation_tokens`;
- wire `usage.cache_creation.ephemeral_5m_input_tokens` -> `Usage.input_details.cache_creation_5m_tokens`;
- wire `usage.cache_creation.ephemeral_1h_input_tokens` -> `Usage.input_details.cache_creation_1h_tokens`.
- wire `usage.output_tokens_details.thinking_tokens` -> `Usage.output_details.reasoning_tokens`.

The decoder MUST remove the recognized `thinking_tokens` member from usage passthrough state. It MUST preserve every unrecognized sibling inside `output_tokens_details` under `Usage.extra_body.output_tokens_details`. A Messages encoder MUST merge those preserved siblings with the typed reasoning count and MUST write the typed count as native `output_tokens_details.thinking_tokens`; the typed value MUST overwrite a colliding preserved value. It MUST NOT emit the legacy flat `reasoning_output_tokens` extension when the native nested field can represent the count.

PM8b. If Anthropic usage contains aggregate `cache_creation_input_tokens` but omits the structured `usage.cache_creation` 5-minute and 1-hour split, Monoize MUST preserve the aggregate in `cache_creation_tokens` and MUST NOT infer the split.

PM8c. When decoding any provider usage object, Monoize MUST map an authoritative cached-input modality split to `Usage.input_details.cache_read_modality_breakdown` if the provider supplies one. Monoize MUST NOT infer this split from aggregate cached tokens or from total input modality counts.

PM9. For downstream `POST /v1/messages` streaming responses synthesized or translated by Monoize, Monoize MUST emit Anthropic-compatible message envelope events in this order:

1. `message_start`
2. zero or more `content_block_start` / `content_block_delta` / `content_block_stop`
3. one `message_delta`
4. `message_stop`

PM9a. For PM9 streams, each emitted content block lifecycle MUST use one integer `index`. The `content_block_start` indices MUST equal `0, 1, ..., N - 1` in content block emission order, where `N` is the number of emitted content blocks in the stream. Every `content_block_delta` and `content_block_stop` for a block MUST use the same `index` as that block's `content_block_start`. Skipped URP nodes, buffered URP nodes, and non-content control nodes MUST NOT create gaps in the emitted index sequence.

PM10. For PM9 streams, `message_start.message` MUST include at least:

- `id`
- `type = "message"`
- `role = "assistant"`
- `model`
- `content` as an array
- `stop_reason = null`
- `stop_sequence = null`
- `usage` object with token counters when available, or zeros when unavailable

PM11. For PM9 streams, `message_delta.delta.stop_reason` MUST be:

- the exact preserved upstream Messages stop reason when `ResponseDone.extra_body.stop_reason` is a non-empty string;
- otherwise `"tool_use"` if any tool-use block was emitted;
- otherwise the protocol mapping of the canonical finish reason, with `"end_turn"` as the fallback.

PM11a. For PM9 streams, `message_delta.delta.stop_sequence` MUST equal `ResponseDone.extra_body.stop_sequence` when that member is present. When the member is absent, the emitted value MUST be JSON `null`.

PM12. For PM9 streams, `message_delta.usage` token counters MUST be cumulative within the stream. Multiple upstream `message_delta` events MUST still produce exactly one downstream terminal `message_delta` and one downstream `message_stop`, derived from the single canonical `ResponseDone` required by PM6c.

PM12a. A Messages encoder MUST render both `message_start.message.usage` and terminal `message_delta.usage` from the complete latest canonical `Usage` snapshot. The rendered object MUST preserve `input_tokens`, `output_tokens`, cache-read tokens, aggregate cache-creation tokens, cache-creation 5-minute and 1-hour splits, tool-prompt tokens, reasoning tokens, accepted-prediction tokens, rejected-prediction tokens, and every non-internal unknown usage field. Reasoning tokens MUST use native `output_tokens_details.thinking_tokens`. A recognized typed `Usage` field MUST overwrite a colliding key from `Usage.extra_body`. A key whose name starts with `_monoize_` MUST NOT appear in the wire usage object.

### 7.5 Provider adapter: `type=gemini`

PG1. Monoize MUST call Gemini native endpoints under base URL `https://generativelanguage.googleapis.com` using the API version path selected by provider configuration, default `v1beta`.

PG2. For non-streaming requests, Monoize MUST call:

- `POST /<version>/models/{upstream_model}:generateContent`

PG3. For streaming requests, Monoize MUST call:

- `POST /<version>/models/{upstream_model}:streamGenerateContent?alt=sse`

PG4. Monoize MUST encode URP v2 requests to Gemini native request fields:

- `contents[]` for conversation turns;
- `systemInstruction` for leading system or developer instruction;
- `generationConfig` for temperature, top_p, and max_output_tokens;
- `tools[]` and `toolConfig.functionCallingConfig` for tool definitions and tool choice.

PG4a. When encoding a URP `ToolResult` node into Gemini `functionResponse`, Monoize MUST set `functionResponse.name` to the tool function name, not the URP `call_id`. Monoize MAY recover that function name from preserved metadata or from the corresponding earlier URP `ToolCall` node.

PG5. Monoize MUST decode Gemini responses from `candidates[].content.parts[]` and convert them to URP v2 nodes, including:

- text nodes;
- tool or function call nodes;
- reasoning or thought nodes and signatures when provided.

PG6. Monoize MUST map Gemini usage metadata to URP usage fields using:

- `promptTokenCount -> input_tokens`
- `candidatesTokenCount -> output_tokens`
- `thoughtsTokenCount -> output_details.reasoning_tokens` when present
- `cachedContentTokenCount -> input_details.cache_read_tokens` when present
- `cacheCreationTokenCount -> input_details.cache_creation_tokens` when present
- `toolPromptInputTokenCount -> input_details.tool_prompt_tokens` when present
- `acceptedPredictionOutputTokenCount -> output_details.accepted_prediction_tokens` when present
- `rejectedPredictionOutputTokenCount -> output_details.rejected_prediction_tokens` when present

PG7. Monoize MUST preserve unknown Gemini request and response fields in URP v2 passthrough state according to §3 and §7.6.

PG8. Unknown Gemini `parts[]` entries MUST decode as `ProviderItem(origin_protocol = "gemini")`. A Gemini encoder MUST replay such parts only when the target provider protocol is `gemini`.

### 7.6 Extra field forwarding

XF1. For any downstream endpoint in §2.2, Monoize MUST preserve unknown fields according to §3 and store them in the owning URP v2 passthrough surface.

XF2. When constructing an upstream request body from a URP v2 request, Monoize MUST insert every key-value pair from top-level request `extra_body` as a top-level JSON key in the upstream request body unless that key is already present in the upstream request body due to adapter logic.

XF3. Adapter-generated keys MUST take precedence over keys from passthrough state. Passthrough state MUST NOT overwrite adapter-generated keys.

XF4. Node-local unknown fields:

- When decoding a downstream request, Monoize MUST preserve unknown fields on individual content blocks into the corresponding URP node `extra_body` or `ToolResultContent.extra_body`.
- When encoding an upstream request, Monoize MUST merge each node or tool-result-content `extra_body` map into the generated protocol object, subject to the same precedence rule as XF3.
- This applies to all content-block types, including `text`, `image`, `document` or `file`, `thinking`, `tool_use`, and `tool_result` inner content blocks.

XF4a. Envelope-level unknown fields:

- When decoding protocol objects whose unknown fields belong to one downstream or upstream envelope rather than to exactly one emitted ordinary node or `ToolResult` node, Monoize MUST preserve those fields as `next_downstream_envelope_extra`.
- When encoding protocol objects from URP v2, Monoize MUST apply buffered `next_downstream_envelope_extra` state to the next consumable envelope exactly once, subject to the same precedence rule as XF3.

XF5. Usage-level unknown fields:

- When parsing upstream usage objects, Monoize MUST populate the URP `Usage` struct's structured fields from recognized provider-specific fields. Any unrecognized fields MUST be captured into `Usage.extra_body`.
- When encoding downstream usage objects for any downstream endpoint, Monoize MUST merge `Usage.extra_body` into the generated usage JSON. Unless an adapter-specific rule states otherwise, a preserved upstream field overwrites an adapter-generated default for the same key.

XF5a. Usage alias acceptance and canonicalization:

- For each provider adapter, Monoize MUST accept all observed upstream aliases for one semantic usage metric and map them to one canonical URP field.
- Alias handling MUST be deterministic. When multiple aliases for the same semantic metric are present simultaneously, Monoize MUST apply a fixed precedence order per adapter implementation.
- Monoize MUST NOT sum alias variants unless the provider contract explicitly defines additive semantics for those fields.

XF5b. Usage forwarding boundary:

- Monoize MUST interpret only recognized usage fields into typed URP structured fields.
- Monoize MUST preserve but MUST NOT reinterpret unknown usage fields. Unknown usage fields are forwarded through `Usage.extra_body` without semantic transformation.

XF5b.1. For Chat Completions and Responses usage, an unknown member nested inside `prompt_tokens_details`, `completion_tokens_details`, `input_tokens_details`, or `output_tokens_details` MUST remain nested under the same detail-object key in `Usage.extra_body`. A decoder MUST NOT flatten that member into the top-level usage namespace. On encode, the adapter MUST merge preserved unknown detail members into the corresponding generated detail object, and recognized typed counters MUST overwrite colliding preserved values.

XF5c. Usage round-trip preservation:

- For adapters that support opaque usage fields, decode then encode through Monoize MUST preserve unknown usage fields and values in downstream usage payloads.
- If an adapter cannot represent a preserved field due to protocol limits, this loss MUST be adapter-specific and explicitly documented in the adapter section.

XF5d. Monoize usage extension field registry:

- When Monoize must encode a URP usage concept into a provider format that lacks a native field for that concept, Monoize MUST emit a Monoize extension field name for that provider format.
- Every Monoize usage extension field name emitted by an encoder MUST be accepted by the corresponding decoder as a recognized alias and MUST therefore NOT remain inside `Usage.extra_body` after decode.
- The following extension field names are reserved for Monoize usage encoding:
  - Messages or Anthropic usage object extensions:
    - `tool_prompt_input_tokens`
    - `cache_creation_5m_input_tokens`
    - `cache_creation_1h_input_tokens`
    - `reasoning_output_tokens` (decode-only legacy alias; current Messages encoding uses native `output_tokens_details.thinking_tokens`)
    - `accepted_prediction_output_tokens`
    - `rejected_prediction_output_tokens`
  - Gemini `usageMetadata` extensions:
    - `cacheCreationTokenCount`
    - `toolPromptInputTokenCount`
    - `acceptedPredictionOutputTokenCount`
    - `rejectedPredictionOutputTokenCount`
    - `reasoningOutputTokenCount`
  - Chat Completions usage detail aliases accepted for symmetry:
    - `tool_prompt_input_tokens` inside `prompt_tokens_details` or `input_tokens_details`
    - `accepted_prediction_output_tokens` inside `completion_tokens_details` or `output_tokens_details`
    - `rejected_prediction_output_tokens` inside `completion_tokens_details` or `output_tokens_details`
  - Responses usage detail aliases accepted for symmetry:
    - `tool_prompt_input_tokens` inside `input_tokens_details` or `prompt_tokens_details`
    - `accepted_prediction_output_tokens` inside `output_tokens_details` or `completion_tokens_details`
    - `rejected_prediction_output_tokens` inside `output_tokens_details` or `completion_tokens_details`
- These names are Monoize-defined extensions, not claims about native upstream contracts. If an upstream provider later adopts one of these names with incompatible semantics, Monoize MUST treat that as a spec-level conflict requiring explicit review.

### 7.6.1 Extra field upstream whitelist

XF6. When constructing an upstream request body from a URP v2 request, Monoize MUST filter top-level request `extra_body` keys against a per-provider-type whitelist before inserting them into the upstream request body. Keys not present in the effective whitelist MUST be dropped from the upstream request body. The intermediate filter MUST retain every `_monoize_` internal adapter-state key until target encoding, independent of the provider whitelist. Retention is not wire permission: the target encoder MUST consume or omit each internal key under `ENC11`.

XF6a. Default whitelists per provider type:

- `chat_completion`: `audio`, `frequency_penalty`, `function_call`, `functions`, `logit_bias`, `logprobs`, `top_logprobs`, `max_completion_tokens`, `max_tokens`, `metadata`, `moderation`, `n`, `presence_penalty`, `prompt_cache_options`, `safety_identifier`, `seed`, `service_tier`, `stop`, `stream_options`, `store`, `web_search_options`, `parallel_tool_calls`, `debug`, `image_config`, `modalities`, `cache_control`, `top_k`, `top_a`, `min_p`, `repetition_penalty`, `prediction`, `prompt_cache_key`, `prompt_cache_retention`, `route`, `structured_outputs`, `verbosity`, `models`, `provider`, `plugins`, `session_id`, `stop_server_tools_when`, `trace`, `thinking`, `include_reasoning`, `user_id`.
- `responses`: `background`, `context_management`, `conversation`, `include`, `instructions`, `metadata`, `max_tool_calls`, `moderation`, `parallel_tool_calls`, `previous_response_id`, `prompt`, `prompt_cache_key`, `prompt_cache_options`, `prompt_cache_retention`, `safety_identifier`, `service_tier`, `store`, `stream_options`, `text`, `top_logprobs`, `truncation`.
- `messages`: `cache_control`, `container`, `fallbacks`, `max_tokens`, `metadata`, `output_config`, `service_tier`, `stop_sequences`, `top_k`, `inference_geo`.
- `gemini`: `generationConfig`, `safetySettings`, `cachedContent`, `labels`.

XF6b. Each dashboard-managed provider MAY carry an optional `extra_fields_whitelist` override, JSON array of strings stored in the `monoize_providers` table. When present, the effective whitelist is the union of the default whitelist and the override list. When absent, only the default whitelist applies.

XF6c. If `extra_fields_whitelist` contains the single entry `"*"`, Monoize MUST skip whitelist filtering entirely for that provider, forwarding all top-level request `extra_body` keys unconditionally.

XF6d. Whitelist filtering applies only to top-level request `extra_body` keys. Node-local `extra_body`, `ToolResultContent.extra_body`, envelope-control nodes, and usage `extra_body` are not subject to this whitelist.

XF6e. Whitelist filtering MUST occur after request-phase transforms and before upstream request encoding. The filter runs inside `encode_request_for_provider`, immediately before dispatching to the provider-specific encoder.

### 7.7 Downstream adapter: `POST /v1/chat/completions`

DC1. Monoize MUST parse the downstream request as a Chat Completions create request and convert it into `UrpRequestV2`.

DC1a. URP v2 represents one assistant candidate. A downstream Chat Completions request MAY omit `n` or set `n = 1`. Monoize MUST reject `n = 0`, `n > 1`, a non-integer value, or a non-numeric value with HTTP 400. Monoize MUST NOT forward a request that can produce multiple choices and then silently retain only `choices[0]`.

DC2. Monoize MUST forward using the pipeline in §5.

DC3. Monoize MUST render the result as a Chat Completions response, non-stream or SSE stream, based on the downstream request.

DC4. Tool-calling:

- For non-streaming responses, if `UrpResponseV2.output` contains one or more assistant `ToolCall` nodes, Monoize MUST render them into `choices[0].message.tool_calls[]`.
- For streaming responses, Monoize MUST stream tool calls using `choices[0].delta.tool_calls[]` in a semantically equivalent manner, including parallel tool calls when multiple calls are present.
- For downstream requests, Monoize MUST parse:
  - `role="tool"` messages into top-level URP `ToolResult` nodes; and
  - assistant messages with `tool_calls[]` into assistant URP `ToolCall` nodes.

DC4a. Non-stream Chat reconstruction:

- When rendering a non-streaming Chat Completions response, Monoize MUST reconstruct one `choices[0].message` object from the flat assistant node sequence when the downstream response shape requires one assistant message object.
- That reconstructed object MUST preserve the full assistant text sequence, all tool calls, reasoning fields, and preserved unknown fields. Monoize MUST NOT silently discard later assistant segments.

DC4b. A same-Chat non-streaming response MUST preserve the upstream integer `created` timestamp through `UrpResponseV2.created_at`. The Chat encoder MUST generate a new timestamp only when `created_at` is absent.

DC5. Reasoning:

- If `UrpResponseV2.output` contains `Reasoning` nodes, Monoize MUST:
  - render non-stream output to Chat Completions as:
    - `choices[0].message.reasoning` from URP reasoning `content` when present, otherwise from URP reasoning `summary`; and
    - `choices[0].message.reasoning_details[]` using OpenRouter reasoning item types `reasoning.summary`, `reasoning.text`, `reasoning.encrypted`, and `reasoning.server_tool_call`.
  - preserve the distinction between summary text and full reasoning content. If one URP reasoning node carries both `summary` and `content`, Monoize MUST render summary text as `type="reasoning.summary"` detail entries and full reasoning content as `type="reasoning.text"` detail entries.
  - preserve every distinct encrypted reasoning payload as its own `type="reasoning.encrypted"` entry in `choices[0].message.reasoning_details[]`. Monoize MUST NOT collapse those entries to only the first encrypted payload.
  - preserve detail order and every entry's `id`, `format`, `index`, `signature`, server-tool fields, and unknown entry-local fields. Repeated detail types and byte-identical entries MUST remain repeated; Monoize MUST NOT deduplicate them.
  - for streaming, emit `choices[0].delta.reasoning_details[]` chunks as reasoning deltas become available.
  - preserve reasoning stream lifecycle. Each reasoning delta MUST be emitted as one chat chunk in arrival order, MAY interleave with text or tool-call chunks, and MUST terminate with the final finish chunk and `[DONE]`.
- Backward compatibility for downstream Chat Completions requests: Monoize MUST parse assistant-message reasoning from both OpenRouter fields `reasoning` and `reasoning_details` and legacy fields `reasoning_content` and `reasoning_opaque`.

DC5a. For DeepSeek V4 thinking-mode tool loops, an assistant message that contains tool calls and non-empty `reasoning_content` MUST replay that `reasoning_content` byte-for-byte on the next outbound DeepSeek V4 Chat request. A no-tool prior turn MAY omit it. Monoize MUST preserve provenance so this DeepSeek replay field is not confused with an OpenRouter scalar alias.

DC5b. A post-start OpenRouter stream chunk with top-level `error`, or a non-stream choice with `error` and `finish_reason="error"`, is terminal failure state. Monoize MUST NOT convert it to `finish_reason="stop"`, emit a successful terminal chunk, or bill it as a successful completion. Monoize MUST preserve a string or numeric `error.code` as its decimal/string representation. If the top-level error object omits `code` or `type`, Monoize MUST use `error.metadata.provider_code` or `error.metadata.error_type`, respectively, when those values are non-empty JSON scalars. A top-level `code` or `type` wins a collision with its metadata fallback. When a same-Chat stream encoder retains the original error chunk, it MUST use that chunk as the replay base and materialize missing canonical `message`, `code`, `type`, and `param` members in the native top-level or choice-local `error` object. Existing native direct members MUST win those insertions. Before replay, the encoder MUST reject incoming `_monoize_` members at the retained chunk, choice, error, and error-metadata owner layers while preserving other unknown provider fields. DeepSeek `finish_reason="insufficient_system_resource"` likewise MUST NOT normalize to `stop`.

DC5c. For a Chat Completions upstream model identifier containing `deepseek`, Monoize MUST use the current DeepSeek thinking controls. Reasoning effort `none` MUST encode `thinking.type = "disabled"` and omit `reasoning_effort`. Every enabled effort MUST encode `thinking.type = "enabled"`; `minimal`, `low`, `medium`, and `high` MUST encode `reasoning_effort = "high"`, while `xhigh` and `max` MUST encode `reasoning_effort = "max"`. A legacy `minimum` input is normalized to `minimal` before this mapping. The maximum output-token field MUST be `max_tokens`, not `max_completion_tokens`.

DC6. If the selected upstream provider type is `chat_completion` and the upstream response contains additional non-standard fields inside `choices[0]`, `choices[0].delta`, or `choices[0].message`, other than fields explicitly mapped by DC4 through DC5, Monoize MUST preserve those fields in the downstream response, streaming or non-streaming, for `POST /v1/chat/completions`. In streaming, a choice-level field such as DeepSeek `choices[0].logprobs` on a non-terminal token frame MUST be preserved on a non-terminal downstream choice frame; it MUST NOT be delayed until the terminal frame, moved below `delta`, or discarded.

DC7. For downstream `POST /v1/chat/completions` streaming responses, when cumulative stream usage counters are available, Monoize MUST emit one separate usage chunk after the empty-delta finish chunk and before `[DONE]`. The usage chunk MUST set `choices` to `[]` and MUST contain the cumulative `usage` object. The empty-delta finish chunk MUST NOT contain a non-null `usage` object.

DC8. For downstream `POST /v1/chat/completions` streaming responses, Monoize MUST emit SSE as data-only frames. Every assistant chunk MUST be encoded as `data: {json}` with no named `event:` line, and successful stream termination MUST emit exactly one terminal `data: [DONE]` sentinel.

DC9. For downstream `POST /v1/chat/completions` streaming responses, Monoize MUST preserve these externally visible lifecycle guarantees even though canonical internal state is flat:

- exactly one plain `[DONE]` sentinel;
- exactly one terminal empty-delta finish chunk;
- when cumulative usage is available, exactly one separate `choices=[]` usage chunk after the finish chunk and immediately before `[DONE]`;
- no streamed chunk may co-pack `content` and `tool_calls` in the same assistant delta;
- if tool-call deltas were emitted in the turn, terminal `finish_reason` MUST be `tool_calls`;
- OpenRouter-compatible reasoning fields, including `reasoning_details`, MUST remain available downstream.

### 7.8 Downstream adapter: `POST /v1/messages`

DM1. Monoize MUST parse the downstream request as a Messages create request and convert it into `UrpRequestV2`.

DM1.1. For Messages downstream requests, Monoize MUST recognize and forward `parallel_tool_calls` when present and boolean-typed.

DM2. Monoize MUST forward using the pipeline in §5.

DM3. Monoize MUST render the result as a Messages response, non-stream or SSE stream, based on the downstream request.

DM4. Tool-calling:

- For non-streaming responses, if `UrpResponseV2.output` contains assistant `ToolCall` nodes, Monoize MUST render them as Messages `tool_use` content blocks.
- For streaming responses, Monoize MUST stream tool calls as `content_block_start` blocks with `type="tool_use"` and tool input deltas.
- For streaming responses translated from Responses upstream events, if any accumulated output item is a function call, the downstream Messages terminal `message_delta.delta.stop_reason` MUST be `"tool_use"` even when the upstream `response.completed.status` is `"completed"` and the `response.completed.response.output` snapshot omits that function call.
- For downstream requests, Monoize MUST parse `tool_result` blocks into top-level URP `ToolResult` nodes.

DM4.1. For downstream `tool_result` blocks that carry block-array content, Monoize MUST preserve image and file payloads when routing through URP v2 and when encoding to eligible upstream formats.

DM5. Reasoning:

- If `UrpResponseV2.output` contains `Reasoning` nodes, Monoize MUST render them as Messages `thinking` content blocks with:
  - `thinking` from URP `content` when present, otherwise from URP `summary`;
  - `signature` from URP `encrypted` when the downstream Messages block schema requires that field.

DM5.1. When rendering a URP `Reasoning` node to an Anthropic Messages content block (downstream response, upstream request as assistant history, or streaming equivalents thereof), Monoize MUST select the wire shape by the following ordered decision rule. The same rule applies to both non-streaming and streaming Anthropic encoders.

1. If `Reasoning.extra_body["_monoize_reasoning_kind"] == "redacted_thinking"` and `Reasoning.encrypted` is present, emit `{"type": "redacted_thinking", "data": <encrypted>}`.
2. Otherwise, if `Reasoning.content` or `Reasoning.summary` is a non-empty string, emit `{"type": "thinking", "thinking": <text>, "signature"?: <encrypted>}` where `<text>` is `Reasoning.content` when present, otherwise `Reasoning.summary`.
3. Otherwise, if `Reasoning.encrypted` is non-empty, emit the valid omitted-thinking shape `{"type": "thinking", "thinking": "", "signature": <encrypted>}`.
4. Otherwise omit the node. Monoize MUST NOT emit a non-standard field such as `encrypted_thinking`, and MUST NOT reinterpret omitted thinking as `redacted_thinking`.

DM5.2. Reasoning item id transport through Anthropic Messages uses the signature sigil defined in PM5b. When rendering a URP `Reasoning` node to an Anthropic Messages content block under DM5.1 case 1, case 2, or case 3:

- If the encoding direction is **downstream** (the block is part of a response body Monoize returns to a `/v1/messages` client), and `Reasoning.id` and `Reasoning.encrypted` are both non-empty, Monoize MUST wrap the signature payload in the sigil `mz1.<Reasoning.id>.<original_signature>` and write that wrapped value into `thinking.signature` or `redacted_thinking.data`.
- If the encoding direction is **upstream** (the block is part of a request body Monoize sends to a `type=messages` provider), Monoize MUST strip any sigil prefix from the signature payload before emission so that the upstream receives only `<original_signature>`. Monoize MUST NOT emit any extension field such as `_monoize_item_id` on an upstream-facing block.
- Monoize MUST NOT attach a sigil when `Reasoning.id` or `Reasoning.encrypted` is absent.

DM5.3. Invariants for Anthropic Messages thinking block encoding, covering both downstream response rendering and upstream request assistant-history rendering, streaming and non-streaming:

1. Every emitted `thinking` block MUST contain a `thinking` field that is a JSON string.
2. Every emitted `redacted_thinking` block MUST contain a `data` field.
3. A `thinking` block with `thinking = ""` is valid only when it carries a non-empty `signature`; this is Anthropic's omitted-thinking representation.
4. Monoize MUST NOT emit extension field `encrypted_thinking` on any Anthropic content block. The field is not part of the Anthropic wire contract.
5. Anthropic streaming reasoning lifecycles are opened for a URP `Reasoning` node emitted under DM5.1 case 1, 2, or 3. A node that falls into case 4 MUST NOT trigger a content-block lifecycle.

DM5a. For downstream `POST /v1/messages` streaming responses, one URP `Reasoning` node MUST be rendered as one Anthropic `thinking` block lifecycle, except for the adjacent Chat detail case defined in DM5a.1. For each incremental `NodeDelta::Reasoning`, Monoize MUST emit non-empty `content` as `thinking_delta.thinking`. If `content` is absent or empty, Monoize MUST emit non-empty `summary` as `thinking_delta.thinking` only when the delta carries either `_monoize_summary_from_messages_thinking = true` under PM5 or `_monoize_summary_from_plaintext_reasoning = true` under `spec/urp-transform-system.spec.md` PRTS-9. An unmarked summary delta MUST NOT be replayed into a Messages thinking lifecycle because the same reasoning node MAY also carry authoritative raw content. If plaintext reasoning and signature are present, Monoize MUST emit `thinking_delta` first, then `signature_delta`, then `content_block_stop` for the same block index. Monoize MUST preserve upstream delta granularity for emitted reasoning text, emitted summary text, and signatures. Monoize MUST NOT artificially merge multiple upstream reasoning deltas into one larger synthetic delta. It MAY split one upstream delta only when required by the configured SSE maximum frame length.

DM5a.1. If ordered Chat `reasoning_details[]` decoding produces a `reasoning.text` `Reasoning` node with non-empty plaintext followed immediately by a `reasoning.encrypted` `Reasoning` node with a non-empty encrypted payload and no plaintext, every Messages encoder MUST render that adjacent pair as one Anthropic `thinking` block. The block's `thinking` value MUST equal the plaintext from the first node. The block's `signature` value MUST be derived from the encrypted payload and item id of the second node under DM5.2. This rule applies to downstream non-stream responses, upstream non-stream request assistant history, and streaming equivalents. The decoder-owned URP nodes remain distinct for same-Chat replay under MAP-19 through MAP-21.

DM5a.2. For the streaming form of DM5a.1, the encoder MUST emit the plaintext as `thinking_delta`, then the encrypted payload as `signature_delta`, then one `content_block_stop`, all with the same block index.

DM5b. For downstream `POST /v1/messages` streaming responses, a non-redacted thinking lifecycle MUST start with `content_block_start.content_block = {"type":"thinking","thinking":"","signature":""}`. Monoize MUST then emit zero or more `thinking_delta` events, exactly one `signature_delta` when a non-empty signature exists, and one `content_block_stop`. For omitted thinking, it MUST emit zero `thinking_delta` events and one `signature_delta`. The signature payload MUST appear in the delta, not in the start stub.

DM6. For downstream `POST /v1/messages` streaming responses synthesized or translated from non-messages upstream event formats, Monoize MUST set `message_delta.usage` from cumulative stream usage counters when available.

DM7. For downstream `POST /v1/messages` SSE streams, Monoize MUST emit an SSE `event:` line whose value exactly equals the payload `type` field for every named Messages event, including `message_start`, `content_block_start`, `content_block_delta`, `content_block_stop`, `message_delta`, `message_stop`, `ping`, and `error`.

DM7c. When encoding canonical `Error` as a Messages `event: error`, Monoize MUST place the provider error type at `error.type` and the message at `error.message`. A non-empty preserved `error_type`, preserved `type`, or nested preserved `error.type` MUST take precedence over the generic canonical code when selecting `error.type`, in that order. Other non-internal preserved error members MUST remain nested inside `error`; the semantic `type` and `message` values MUST win collisions. No `_monoize_` member may appear on the wire.

DM7a. When an upstream Messages stream emits `ping`, the Messages decoder MAY emit a provider-control stream event under FP6a-CTRL so internal stream state can observe that the upstream connection is alive. The downstream Messages encoder MUST NOT re-emit an upstream-derived `ping` provider-control event. Upstream-derived `ping` events MUST NOT open a content block, close a content block, mutate `ResponseDone.output`, or produce a `[DONE]` sentinel.

DM7b. For downstream `POST /v1/messages` SSE streams, Monoize MUST provide downstream keep-alive independently of upstream events. The downstream keep-alive event MUST be an Anthropic Messages `ping` event with SSE `event: ping` and JSON payload `{"type":"ping"}`. This downstream keep-alive MUST be generated by the downstream response layer only after no downstream frame has been sent for the configured keep-alive interval. A downstream keep-alive `ping` MUST NOT open a content block, close a content block, mutate `ResponseDone.output`, or produce a `[DONE]` sentinel.

DM8. For downstream `POST /v1/messages` SSE streams, every content block index MUST be emitted in strict non-interleaved lifecycle order:

- exactly one `content_block_start` for that index;
- zero or more `content_block_delta` events for that index;
- exactly one `content_block_stop` for that index;
- no events for a later-emitted block may appear between `content_block_start` and `content_block_stop` of an earlier-emitted block.

DM8a. For downstream `POST /v1/messages` SSE streams, the set of emitted content block indices MUST be exactly the contiguous zero-based integer range `0..N`, where `N` is the number of emitted content blocks. The ordering of first `content_block_start` events MUST be ascending by one from `0` with no skipped value.

DM9. For downstream `POST /v1/messages` SSE streams, Monoize MUST NOT emit duplicate `content_block_start`, duplicate `content_block_stop`, or duplicate final-content replays for a block whose streamed deltas already carried the same text, thinking, or input-json bytes.

DM9a. For downstream `POST /v1/messages` SSE streams, when the encoder receives a `NodeStart` for node index `K`, `K` is the next unclosed visible Messages block, and no earlier visible block is open, the encoder MUST emit `content_block_start` for `K` immediately. For every later `NodeDelta` for that open node, the encoder MUST emit exactly one same-kind `content_block_delta` frame, except for configured SSE frame-length splitting. When it receives `NodeDone` for that open node, the encoder MUST emit `content_block_stop` without replaying already emitted content. If a node cannot be opened immediately because an earlier visible block has not closed or because a terminal-only fallback item has no incremental lifecycle, the encoder MAY buffer that node and emit a complete lifecycle later under DM8 and DM9.

DM10. For downstream `POST /v1/messages` successful SSE streams, Monoize MUST terminate with `message_stop` and MUST NOT append any additional `data: [DONE]` sentinel.

DM11. For downstream `POST /v1/messages`, flat canonical storage MUST NOT weaken these externally visible lifecycle guarantees:

- `message_start` first;
- `message_delta` before `message_stop`;
- block lifecycles remain non-interleaved;
- `thinking_delta` precedes `signature_delta` for one thinking block;
- cumulative usage semantics remain explicit and testable.

### 7.9 Downstream endpoint: `POST /v1/embeddings`

DE1. Monoize MUST authenticate and apply pre-forward balance guard exactly as other forwarding endpoints.

DE2. Monoize MUST route Provider and Channel candidates using the same Channel model-map matching rules as chat completions.

DE3. Monoize MUST call upstream path `POST /v1/embeddings` for every selected provider attempt, regardless of provider type.

DE4. Monoize MUST forward request JSON as pass-through payload except for replacing outbound `model` with the selected `upstream_model`.

DE5. Monoize MUST require request field `input` to be either:

- one string; or
- an array whose every element is a string.

DE6. Monoize MUST parse upstream usage from `usage.prompt_tokens` and `usage.total_tokens`.

DE7. Billing tokens for embeddings MUST be:

- `input_tokens = usage.prompt_tokens`
- `output_tokens = 0`

DE8. Downstream response body MUST preserve upstream payload structure except top-level `model` MUST be rewritten to the logical model requested by the client.

DE8a. For downstream `POST /v1/responses` non-stream responses, Monoize MUST preserve top-level upstream response fields such as `service_tier` through canonical decode and re-encode. Monoize MUST NOT replace a present upstream `service_tier` value with a synthesized default.

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

DMO4. `data` MUST contain only logical model keys for which at least one enabled Provider has at least one Channel where `enabled == true`, `weight > 0`, and `models` contains the logical model.

DMO5. If a logical model key appears in two or more providers, `data` MUST include exactly one item for that key.

DMO6. `data` MUST be sorted by `id` in ascending lexicographic order.

DMO7. Runtime health state MUST NOT affect `GET /v1/models`.

DMO8. If the authenticated API key has `model_limits_enabled = true` and `model_limits` is non-empty, Monoize MUST filter `data` to include only models whose `id` is present in the `model_limits` list. If `model_limits_enabled` is false or `model_limits` is empty, no filtering is applied.

## 8. Streaming requirements, Responses downstream

When the downstream endpoint is `POST /v1/responses` with `stream=true`, Monoize MUST respond using SSE and MUST emit the externally visible Responses lifecycle from canonical URP v2 stream events and terminal `ResponseDone.output`.

STR0. If a downstream request has `stream=true`, terminal errors use the protocol-specific HTTP 200 SSE error contract in FP4e and SE1 even when the upstream failure occurs before the first upstream SSE frame. Monoize MUST preserve the upstream error fields in that SSE error object and MUST NOT emit a successful terminal response event.

STR0a. If Monoize has already returned a downstream SSE response and a later upstream or adapter error occurs, Monoize MUST report that error using the downstream protocol's terminal stream error shape. STR0 does not apply after downstream SSE headers have been committed.

At minimum the downstream Responses stream MUST support:

- `response.created`
- `response.in_progress`
- `response.output_item.added`
- `response.output_text.delta`, zero or more
- `response.output_text.done`
- `response.output_item.done`
- `response.completed` or `response.failed`

STR1. Each SSE event `data` MUST be one JSON object whose event metadata lives at the top level. Monoize MUST NOT wrap the protocol payload inside an extra `data` object.

```json
{ "type": "response.created", "sequence_number": 1, ... }
```

STR2. `sequence_number` MUST be monotonically increasing starting from 1 within one response stream.

STR3. For downstream `POST /v1/responses` streams synthesized from non-responses upstream event formats, Monoize MUST include a `usage` object in the terminal `response.completed` payload when cumulative stream usage counters are available.

STR3a. For downstream `POST /v1/responses` streams, every SSE payload MUST include a top-level string field `type` whose value exactly equals the SSE `event:` name.

STR3b. For downstream `POST /v1/responses` SSE, Monoize MUST emit the canonical OpenAI event fields for the selected event family plus Monoize-required top-level `type` and `sequence_number`. Monoize MUST NOT add ad hoc top-level fields such as wrapper `data`, `response_id`, or duplicate text aliases when those fields are not part of the canonical event schema.

STR3c. For downstream `POST /v1/responses` successful SSE streams, Monoize MUST emit exactly one terminal `data: [DONE]` sentinel after the final JSON event payload. The sentinel MUST be a plain `data:` frame and MUST NOT be emitted as a named SSE event.

STR3c.1. For downstream streaming responses emitted by `POST /v1/responses` or `POST /v1/chat/completions`, Monoize MUST configure an SSE heartbeat with an interval of 15 seconds. Each heartbeat MUST be an SSE comment frame whose comment text is `heartbeat`. A heartbeat MUST NOT contain a `data:` line, MUST NOT contain an `event:` line, MUST NOT increment Responses `sequence_number`, and MUST NOT count as a Chat Completions `[DONE]` sentinel. The heartbeat exists only to keep downstream HTTP intermediaries from treating an otherwise-valid idle stream as inactive after Monoize has started the downstream SSE response. For `POST /v1/messages`, downstream keep-alive is specified by DM7b instead of this comment-frame heartbeat.

STR3d. For downstream `POST /v1/responses` reasoning streams, Monoize MUST preserve the distinction between reasoning summary text and raw reasoning text. Reasoning summary lifecycle events MUST use the OpenAI names `response.reasoning_summary_text.delta`, `response.reasoning_summary_text.done`, and `response.reasoning_summary_part.done`. Raw reasoning text lifecycle events MUST use the OpenAI names `response.reasoning_text.delta` and `response.reasoning_text.done`. Monoize MUST NOT emit the legacy custom event family `response.reasoning.delta` or `response.reasoning.done`.

STR3e. `response.created` and `response.in_progress` payloads MUST carry the Responses object under top-level field `response`. The nested response object MUST use field name `created_at`, not `created`.

STR3f. Every downstream `/v1/responses` SSE payload that contains a Responses object, `response.created`, `response.in_progress`, `response.completed`, and `response.failed` when present, MUST encode that nested object with canonical Responses object field names. In particular, the timestamp field MUST be `created_at`.

STR3fa. When a downstream `/v1/responses` SSE payload contains a nested completed Responses object, Monoize MUST preserve top-level upstream response fields such as `service_tier`. Monoize MUST NOT force `service_tier = "auto"` when the terminal upstream response carried a different value.

STR3g. For downstream `/v1/responses` message text streaming:

- `response.content_part.added` MUST include `output_index`, `content_index`, `item_id`, and `part`.
- The first content part in a message item MUST use `content_index = 0`.
- For `part.type = "output_text"`, the added-event part payload MUST include `annotations: []` and `text: ""`.
- `response.output_text.delta` MUST include `output_index`, `content_index`, `item_id`, `delta`, and `logprobs`, where `logprobs` is `null` when unavailable.
- `response.output_text.done` MUST include `output_index`, `content_index`, `item_id`, `text`, and `logprobs`, where `text` is the full aggregated content for that output-text part.

STR3h. For downstream `/v1/responses` function-call streaming, after the last `response.function_call_arguments.delta` for a function-call item and before that item's `response.output_item.done`, Monoize MUST emit exactly one `response.function_call_arguments.done` containing the full aggregated `arguments` plus `call_id`, `item_id`, `name`, and `output_index`.

STR3i. For downstream `/v1/responses` reasoning-summary streaming, Monoize MUST emit `response.reasoning_summary_part.added` before the first `response.reasoning_summary_text.delta` for that summary part.

STR3j. Downstream `/v1/responses` SSE MUST obey nested lifecycle ordering. For each `output_index`, every child lifecycle belonging to that output item MUST close before Monoize emits `response.output_item.done` for that same `output_index`. In particular:

- reasoning summary `.done` events MUST precede the reasoning item's `response.output_item.done`;
- message `response.output_text.done` and `response.content_part.done` MUST precede the message item's `response.output_item.done`;
- function-call `response.function_call_arguments.done` MUST precede the function-call item's `response.output_item.done`.

STR3k. For downstream `/v1/responses` SSE translated from upstream Responses streams, Monoize MUST emit at most one `response.output_item.added` and `response.output_item.done` lifecycle per logical downstream output item. If a message, reasoning item, or function-call item has already been streamed before `response.completed`, Monoize MUST NOT synthesize a second duplicate lifecycle for that same logical item when the terminal response snapshot arrives.

STR3k.1. For the duplicate-suppression rule in STR3k, two message items MUST be treated as the same logical output item when all of the following conditions hold:

- both items have `type = "message"`;
- both items have the same role, using `"assistant"` when `role` is absent;
- both items have the same non-empty ordered text content after reading each `content[]` part's `text` or `refusal` string;
- the two item-level `phase` values are equal, or at least one item omits item-level `phase`.

In that case, different message `id` values MUST NOT cause Monoize to emit a second output item lifecycle or a second `response.completed.response.output[]` entry.

STR3l. Downstream `/v1/responses` SSE MUST preserve externally visible item identity continuity. In particular, deltas and terminal item payloads for the same logical output item MUST use the same `item_id`.

STR3m. Downstream `/v1/responses` SSE MUST preserve `phase` on reconstructed message items and text deltas when that metadata is present in URP v2 `Text` nodes.

STR3n. `ResponseDone.output` is the only authoritative terminal flat state used to reconstruct the final downstream `response.completed.response.output` array. Monoize MUST NOT duplicate final outputs by replaying items that were already emitted as completed downstream lifecycles.

STR3o. For downstream `/v1/responses` reasoning items, Monoize MUST preserve item-local `duration` when that field is present on the reconstructed reasoning item. If a streamed reasoning item completes without item-local `duration`, Monoize MUST synthesize integer-second `duration` on that reasoning item before any downstream event can cause a completed or non-last reasoning item to be rendered without duration. At minimum this means the reasoning item carried by `response.output_item.added`, the reasoning item carried by `response.output_item.done`, and the matching item in `response.completed.response.output` MUST all contain `duration` once Monoize can infer that the upstream reasoning item represents completed or terminal reasoning state. The synthesized value MUST be non-negative. The synthesized value SHOULD use elapsed time from the downstream request start when the upstream does not provide item-local duration. Monoize MUST NOT delay or pace SSE frames to fabricate reasoning duration.

### 8.1 Canonical internal stream events

STR4. URP v2 internally represents streaming with the canonical event set defined by `spec/urp-v2-flat-structure.spec.md`:

- `ResponseStart`
- `NodeStart`
- `NodeDelta`
- `NodeDone`
- `ResponseDone`
- `Error`

STR5. `NodeStart.header` MUST use the canonical `NodeHeader` union defined by `spec/urp-v2-flat-structure.spec.md`.

STR6. `NodeDelta.delta` MUST use the canonical `NodeDelta` union defined by `spec/urp-v2-flat-structure.spec.md`.

STR7. `node_index` values MUST be assigned sequentially starting from `0` within one response stream. `node_index` is URP-local and MUST NOT be copied from upstream protocol indices by assumption alone.

STR8. `NodeDone.node` MUST contain the complete terminal node for that `node_index`.

STR9. `ResponseDone.output` MUST contain the complete terminal ordered flat node sequence.

STR10. Downstream stream encoders for Responses, Chat Completions, and Messages own protocol-visible envelope reconstruction from `NodeStart`, `NodeDelta`, `NodeDone`, and `ResponseDone.output`.

STR11. Flat canonical storage MUST NOT weaken downstream protocol lifecycle guarantees. Responses output-item and content-part lifecycles, Chat terminal finish-chunk semantics, and Messages block-lifecycle ordering remain explicit encoder obligations and remain testable at the wire level.

## 9. Stream error termination

When a streaming request, `stream=true`, encounters an error, either before any upstream data is received, for example no available provider, or mid-stream, for example upstream connection failure, Monoize MUST:

1. Emit a protocol-appropriate error event to the downstream client.
2. Emit the protocol-appropriate terminal sentinel for that downstream endpoint.
3. Close the SSE connection.

### 9.1 Pre-stream errors

SE1. If an error occurs before any data has been streamed, Monoize MUST return an SSE response, not a JSON error response, containing:

- for `POST /v1/chat/completions`: one `data` event with an OpenAI-compatible error JSON object, followed by `data: [DONE]`;
- for `POST /v1/responses`: one named `event: error` whose JSON payload contains top-level `type = "error"`, `sequence_number`, `code`, `message`, and `param`, followed by `data: [DONE]`;
- for `POST /v1/messages`: one named `event: error` with an Anthropic-compatible error object, then close the SSE stream without appending `data: [DONE]`.

For `POST /v1/responses`, Monoize MUST NOT emit `response.failed` for the SE1 pre-stream error case. The `response.failed` event is reserved for a Responses response object that has failed after the Responses stream exists.

SE1a. If the pre-stream error was caused by an upstream HTTP error response and that response body contains JSON field `error.code`, `error.type`, or `error.param`, Monoize MUST preserve those values on the downstream error object using `upstream_code`, `upstream_type`, and `upstream_param` for non-stream JSON errors, and MUST use `upstream_code` as the top-level `code` of downstream Responses `event: error` SSE payloads.

SE1b. A non-stream JSON error returned by `POST /v1/messages` MUST use the Anthropic envelope `{ "type": "error", "error": { "type": <error type>, "message": <message> }, "request_id": <request id> }`. If Monoize has a request identifier, it MUST emit the same value in the top-level `request_id` member and the HTTP `request-id` response header. Upstream diagnostic members required by SE1a MAY remain additional members of the nested `error` object. Chat Completions and Responses JSON error envelopes are unchanged.

### 9.2 Mid-stream errors

SE2. If an error occurs after streaming has begun, for example upstream disconnects or parse failure, Monoize MUST emit a protocol-appropriate error event, then:

- for downstream `POST /v1/chat/completions` and `POST /v1/responses`, emit exactly one terminal `data: [DONE]` sentinel and close the stream;
- for downstream `POST /v1/messages`, close the stream without appending `data: [DONE]`.

SE3. If the downstream channel sender is already closed, client disconnected, Monoize MAY silently discard the error event.

SE3a. For downstream `POST /v1/responses`, Monoize-generated `event: error` SSE payloads MUST NOT nest the error under an `error` object. The fields `type`, `sequence_number`, `code`, `message`, and `param` MUST be top-level fields of the SSE JSON payload.

SE4. If an upstream Responses stream emits an `error` event or `response.failed` event, Monoize MUST treat that event as terminal. Monoize MUST NOT consume or forward any later upstream `response.completed` event or `response.failed` event for that request, and MUST NOT synthesize a successful `ResponseDone` after the error. For downstream `POST /v1/responses`, the externally visible terminal JSON event MUST be `response.failed`. If the upstream failure contains `type`, `code`, `message`, or `param`, Monoize MUST preserve those fields in `response.failed.response.error`.
