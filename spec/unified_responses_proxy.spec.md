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

C5. Monoize MUST accept downstream request bodies up to 50 MiB on forwarding endpoints (`/v1/responses`, `/v1/chat/completions`, `/v1/messages`, `/v1/embeddings`). Any framework-default extractor limit smaller than 50 MiB MUST be disabled so that the effective limit remains 50 MiB.

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

T1a. Internal URP v2 `tools[]` MAY also contain native Responses non-function tool descriptors. For `type = "image_generation"`, Monoize MUST preserve the tool descriptor as a non-function tool with its typed `type` plus any extra top-level fields carried in `ToolDefinition.extra_body`.

Stateful fields:

S1. Monoize MUST reject `background=true` with `400` code `background_not_supported`.

S2. Monoize MUST ignore `store` and treat it as absent.

S3. Monoize MUST ignore `conversation` and `previous_response_id` and treat them as absent.

S4. Monoize MUST reject any Responses `input` item with `type = "item_reference"` with `400` code `invalid_request`. The error message MUST state that Monoize is stateless and requires replaying full prior assistant and tool-call items instead of stored item references.

S4. Monoize MUST reject any Responses `input` item with `type = "item_reference"` with `400` code `invalid_request`. The error message MUST state that Monoize is stateless and requires the caller to replay full prior assistant and tool-call items instead of stored item references.

### 7.1.1 URP v2 tool-calling nodes

TCI1. URP v2 `input` and `output` MAY contain tool-calling state represented as nodes:

- **Tool call:** one ordinary `ToolCall` node with fields `call_id`, `name`, `arguments`, and `role = "assistant"`.
- **Tool result:** one distinct top-level `ToolResult` node with fields `call_id`, `is_error`, `content: Vec<ToolResultContent>`, and `extra_body`.

TCI2. Monoize MUST NOT execute tools locally. Tool execution is always performed by the downstream client.

TCI3. When Monoize forwards a request, Monoize MUST forward any tool-calling nodes present in `UrpRequestV2.input` by adapting them into the selected upstream provider's request format under §7.2 through §7.8.

### 7.1.1a ToolResultContent type

TRC1. `ToolResultContent` is an enum representing typed content within `ToolResult.content`. The variants are:

- `Text { text: String }`
- `Image { source: ImageSource }`
- `File { source: FileSource }`

TRC2. Each `ToolResult.content` field contains zero or more `ToolResultContent` entries. An empty `content` vector represents a tool result with no output payload.

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

RC1. Monoize MUST normalize reasoning effort to one of `none`, `minimum`, `low`, `medium`, `high`, `xhigh`, or `max`. `xhigh` and `max` are two distinct effort levels; Monoize MUST NOT treat either as an alias of the other, and MUST NOT silently rewrite one to the other in either direction.

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

- `chat_completion`: If effort is `none`, Monoize MUST omit the `reasoning_effort` field entirely. For any other effort value, Monoize MUST send normalized effort as `reasoning_effort`.
- `responses`: If effort is `none`, Monoize MUST omit the `effort` key from the `reasoning` object. The object MAY still contain other keys such as `summary`. For any other effort value, Monoize MUST send normalized effort as `reasoning: { "effort": <level> }`.
- `messages`: Monoize MUST select the encoding based on the upstream model:
  - For models that support adaptive thinking, Monoize MUST send `thinking: { "type": "adaptive" }` combined with `output_config: { "effort": <level> }`, transmitting the normalized effort string as-is (including `"xhigh"` and `"max"` as distinct values). A model supports adaptive thinking iff its identifier contains a `claude-opus-` or `claude-sonnet-` (or bare `opus-` / `sonnet-`) family segment whose (major, minor) version is >= (4, 6). This covers Claude Opus/Sonnet 4.6, 4.7, 4.8, and any 5.x or later release without requiring per-minor-version maintenance.
  - For all other Anthropic models, Monoize MUST send `thinking: { "type": "enabled", "budget_tokens": N }`, where:
    - `minimum -> N=1024`
    - `low -> N=1024`
    - `medium -> N=4096`
    - `high -> N=16384`
    - `xhigh -> N=32000`
    - `max -> N=32000` (identical budget to `xhigh` on non-adaptive models; the distinction between `xhigh` and `max` is only observable on adaptive-thinking models)

RC4a. For upstream provider type `responses`, Monoize MUST include top-level `reasoning.summary = "detailed"` on the encoded upstream request unless the typed downstream request already carries an explicit `reasoning.summary` value.

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

ENC2. Each `Reasoning` node MUST encode as one top-level Responses `reasoning` item.

ENC3. Each `ToolCall` node MUST encode as one top-level Responses `function_call` item.

ENC4. Each maximal run of adjacent ordinary nodes that are not `Reasoning` and not `ToolCall`, and that share the same `role`, MAY encode as one Responses `message` item.

ENC5. A change in `Text.phase` inside a Responses message run MUST force a new Responses `message` item boundary.

ENC6. The Chat Completions encoder MUST prevent content and `tool_calls` interleaving within one streamed downstream chunk. It MAY reconstruct one downstream assistant message object from several flat assistant nodes for non-stream responses when that reconstruction preserves source order and all tool, reasoning, and text fields.

ENC7. In a non-streaming Chat Completions response, `choices[0].message.content` MUST always be a JSON string. If multiple assistant text nodes are merged into one downstream message, Monoize MUST concatenate their text in source order using `"\n\n"` as the separator.

ENC8. Structured reasoning in Chat Completions responses MUST be encoded in `reasoning_details`, not in `reasoning`. Plaintext reasoning MAY also populate the simple `reasoning` alias where that downstream field already exists.

ENC9. The Anthropic Messages encoder MUST reconstruct Anthropic `message` and `content[]` envelopes from flat URP v2 nodes. Block order MUST preserve flat node order after protocol-required grouping.

ENC9a. When encoding an upstream Anthropic Messages request, Monoize MUST always send `max_tokens`. If the downstream request omits an explicit output-token cap, Monoize MUST encode `max_tokens: 64000`. If the downstream request provides an explicit output-token cap, Monoize MUST forward that explicit value unchanged.

ENC10. `ToolResult` remains a distinct top-level semantic unit. The Anthropic Messages encoder MUST render a `ToolResult` node as a distinct `tool_result` protocol object or block container. It MUST NOT rewrite that node as ordinary role-bearing content.

### 7.2 Provider adapter: `type=responses`

PR1. Monoize MUST call the upstream path `POST /v1/responses`.

PR2. For non-streaming responses, Monoize MUST parse the upstream response as a Responses response object and convert it to `UrpResponseV2`.

PR2b. When a non-streaming upstream Responses `output[]` item has `type = "image_generation_call"` and carries non-empty field `result`, Monoize MUST decode that item as one assistant `Image` node with `Image.source = Base64`.

- The decoded `media_type` MUST be derived from `output_format` when present: `png -> image/png`, `webp -> image/webp`, `jpeg -> image/jpeg`.
- If `output_format` is absent or unrecognized, Monoize MUST default the decoded `media_type` to `image/png`.
- Monoize MUST preserve unknown fields from that item in node-local `extra_body`, excluding adapter-consumed keys `type`, `result`, and `output_format`.

PR2a. Responses order preservation:

- When Monoize decodes Responses `input[]` or `output[]`, Monoize MUST process items in source order and preserve that order in the resulting URP v2 node sequence.
- When Monoize encodes URP v2 back to Responses `input[]` or `output[]`, Monoize MUST emit items in encoder-reconstructed URP order.
- Monoize MUST NOT postpone all text emission into one final Responses `message` item if that would reorder text relative to `function_call`, `reasoning`, or future item kinds.
- Monoize MAY merge only contiguous message-compatible nodes into one Responses `message` item.
- Monoize MUST split Responses `message` items when the URP order crosses a non-message item boundary or when adjacent URP v2 `Text` nodes have different `phase` values.

PR3. For streaming, Monoize MUST parse upstream Responses SSE into canonical URP v2 stream events and then encode protocol-correct downstream SSE under §8 when the downstream endpoint is `POST /v1/responses`.

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

PR4c. When encoding URP v2 `Reasoning` nodes into upstream `POST /v1/responses` request `input[]` items with `type="reasoning"`, Monoize MUST always include field `summary`.

- If the URP reasoning node carries summary text, Monoize MUST encode `summary` as an array containing one `{ "type": "summary_text", "text": <summary> }` object.
- If the URP reasoning node carries no summary text but carries plaintext reasoning content, Monoize MUST encode that plaintext content into `summary` as one `{ "type": "summary_text", "text": <content> }` object.
- If the URP reasoning node carries neither summary text nor plaintext reasoning content, Monoize MUST encode `summary` as `[]`.
- This request-side requirement applies even when the same reasoning node also carries `content` and or `encrypted`.
- Monoize MUST NOT forward URP-internal or provider-origin reasoning metadata such as `source` on upstream request `input[]` reasoning items unless the upstream Responses request schema explicitly supports that field.
- Monoize MUST NOT encode field `text` on upstream request `input[]` reasoning items. Plain reasoning text, when preserved at all, MUST be represented through `summary`.

PR4c.1. When decoding downstream Responses `input[]`, Monoize MUST decode an item with `type = "reasoning"` into one URP v2 `Reasoning` node using the same field mapping as non-streaming Responses `output[]` reasoning items:

- `id` maps to `Reasoning.id`;
- `encrypted_content` maps to `Reasoning.encrypted`;
- `summary[]` maps to `Reasoning.summary`;
- `text`, when present, maps to `Reasoning.content`.

PR4c.2. When encoding an upstream `POST /v1/responses` request, Monoize MUST include `reasoning.encrypted_content` in the top-level `include` array. If the downstream request already supplied an `include` array, Monoize MUST append `reasoning.encrypted_content` only when that exact string is absent.

PR4c.3. When an authenticated API key has `reasoning_envelope_enabled = true`, Monoize MUST wrap every downstream-visible encrypted reasoning payload produced by `/v1/responses`, `/v1/chat/completions`, or `/v1/messages` in a Monoize reasoning envelope before the payload is emitted to the downstream client.

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

PR4c.6. Before sending an upstream request, Monoize MUST inspect replayed URP `Reasoning.encrypted` values. If the value is an `mz2.` envelope and `reasoning_envelope_enabled = true`, Monoize MUST unwrap and forward the original `payload` only when both `provider_type` and `model` equal the selected upstream provider type and upstream model for the current attempt. If either value differs, Monoize MUST drop that replayed reasoning node from the upstream request.

PR4c.7. If `reasoning_envelope_enabled = false`, Monoize MUST NOT wrap newly produced downstream encrypted reasoning payloads. If a downstream request nevertheless replays an `mz2.` envelope, Monoize MAY unwrap it before upstream encoding, but MUST NOT enforce the provider/model mismatch drop defined by PR4c.6.

PR4c.8. Monoize MUST accept legacy `mz1.<item_id>.<payload>` reasoning signatures as replay input. When forwarding such a value to a Responses upstream, Monoize MUST set the reasoning item id to `<item_id>` and forward only `<payload>` as `encrypted_content`.

PR4d. When encoding URP v2 ordinary nodes into upstream `POST /v1/responses` request `input[]` messages, Monoize MUST choose content block types by message role:

- `role="user"` content MUST use request or input block types such as `input_text`, `input_image`, and `input_file`.
- `role="assistant"` content MUST use assistant or output block types such as `output_text`, `output_image`, and `output_file`.
- `role="assistant"` text or refusal history MUST NOT be encoded as `input_text`.

PR5. When parsing upstream Responses SSE, Monoize MUST support canonical Responses event payloads where:

- text deltas are carried in `delta` for `response.output_text.delta`;
- tool-call items are nested under `item` for `response.output_item.added` and `response.output_item.done`;
- argument deltas identify the call via `output_index`, not necessarily `call_id`, for `response.function_call_arguments.delta`.

PR5c. When parsing upstream Responses SSE, Monoize MAY receive official image-generation tool events outside the `response.*` namespace.

- For `image_generation.completed`, if the payload carries non-empty `b64_json`, Monoize MUST decode that payload as one assistant `Image` node with `Image.source = Base64`.
- The decoded media type MUST be derived from `output_format` using the same mapping as PR2b, defaulting to `image/png`.
- For `image_generation.partial_image`, Monoize MAY ignore the event for canonical URP node emission. Ignoring that event MUST NOT be treated as a stream error.

PR5a. Responses stream node reconstruction:

- When upstream emits `response.output_item.added` for a top-level Responses output item of type `reasoning` or `function_call`, Monoize MUST emit a canonical `NodeStart` before any canonical `NodeDelta` derived from that output item.
- Monoize MUST decode top-level Responses `reasoning` output items as one assistant `Reasoning` node.
- Monoize MUST decode top-level Responses `function_call` output items as one assistant `ToolCall` node.

PR5b. Responses stream index normalization:

- Upstream Responses `output_index` and `content_index` are upstream protocol coordinates and MUST NOT be reused as URP `node_index` by assumption alone.
- The Responses streaming decoder MUST assign URP `node_index` values sequentially starting from `0` in first-seen node order.
- The Responses streaming decoder MUST maintain enough mapping state to correlate later upstream deltas with the correct URP `node_index` and with the encoder-local downstream coordinates required by the target protocol.
- During downstream Responses re-encoding, Monoize MUST continue to emit `output_index` and `content_index` coordinates required by the Responses wire protocol, but those coordinates are encoder-local and MUST NOT alter URP `node_index` semantics.

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
- When Monoize emits downstream `/v1/responses` SSE from flat URP v2 nodes, it MUST emit canonical Responses output-item boundaries derived by encoder reconstruction, not raw upstream boundaries.
- For downstream `/v1/responses` SSE, every `response.output_item.done` event MUST have exactly one earlier matching `response.output_item.added` event with the same `output_index`.
- Monoize MUST emit `response.content_part.added` and `response.content_part.done` only for Responses `message` items. Monoize MUST NOT encode `Reasoning` or `ToolCall` nodes as `message.content[]` stream parts.

PR6c. Final Responses-stream object synthesis:

- If the upstream Responses stream terminates without a `response.completed` event, Monoize MUST synthesize exactly one terminal `ResponseDone` from the accumulated stream state.
- If the upstream Responses stream already emitted `response.completed`, Monoize MUST forward at most one corresponding `ResponseDone`. Monoize MUST NOT emit a duplicate synthetic terminal response after forwarding the upstream terminal response.
- The synthesized or forwarded `ResponseDone.output` value MUST be the complete terminal flat node sequence.
- When accumulated assistant text is empty, Monoize MUST NOT synthesize an empty assistant `Text` node solely to carry that empty string.
- When Monoize emits downstream `response.completed` from `ResponseDone.output`, the `response.output` array MUST be encoded with the same Responses encoder logic used for non-streaming responses so that top-level `reasoning`, `message`, and `function_call` items remain in canonical Responses output positions rather than being nested incorrectly inside `message.content[]`.
- If Monoize synthesizes missing assistant text stream events because upstream omitted text deltas, it MUST synthesize one downstream text segment per recovered text-bearing node run in output order. Each synthesized segment MUST preserve `phase` metadata and MUST allocate fresh encoder-local coordinates instead of reusing URP `node_index` as a wire coordinate.

PR7. For URP v2 `ToolResult` nodes with non-string multimodal output, Monoize MUST preserve multimodal output parts when forwarding to Responses upstream:

- text parts as `input_text`;
- image parts as `input_image`;
- file or document parts as `input_file`.

PR8. For upstream Responses requests, Monoize MUST parse `function_call_output.output` whether it is:

- string text; or
- an array or object content payload with `input_text`, `input_image`, and `input_file`.

Parsed data MUST become one URP v2 `ToolResult` node without dropping image or file parts.

### 7.3 Provider adapter: `type=chat_completion`

PC1. Monoize MUST call the upstream path `POST /v1/chat/completions`.

PC2. Monoize MUST convert `UrpRequestV2.input` nodes into chat `messages[]` as follows:

- ordinary nodes become chat messages or message content in source order using encoder-owned grouping;
- assistant `ToolCall` nodes become assistant `tool_calls[]` entries, grouping consecutive tool-call nodes when needed to preserve parallel calls;
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

PC2.4. Assistant message history preservation for chat adapter:

- When decoding downstream Chat Completions history, if one assistant message contains both non-empty `content` and `tool_calls`, Monoize MUST preserve both within the same assistant turn by emitting flat URP v2 nodes in source order rather than discarding either surface.
- When encoding that assistant turn back to an upstream `type=chat_completion` request, Monoize MAY encode the assistant text or refusal content and the assistant `tool_calls` on the same upstream `messages[]` element when the target protocol supports that combined form.

PC3. Tool descriptor normalization:

- For `type=chat_completion` upstreams, Monoize MUST ensure upstream `tools[]` contains only `type=function` tool descriptors.
- For each URP tool with `type != "function"`, Monoize MUST convert it into a `type=function` tool with `function.name = <tool.type>` and a permissive JSON schema.

PC4. Monoize MUST convert chat-completions non-stream output into `UrpResponseV2` output nodes.

PC5. Monoize MUST convert chat-completions streaming deltas into canonical URP v2 stream events.

PC6. Tool-calling, non-stream:

- If the upstream chat-completions response contains `choices[0].message.tool_calls[]`, Monoize MUST convert each entry into one URP assistant `ToolCall` node using:
  - `call_id = tool_calls[i].id`
  - `name = tool_calls[i].function.name`
  - `arguments = tool_calls[i].function.arguments`, string. If the upstream sends a JSON object, Monoize MUST serialize it as JSON.

PC7. Tool-calling, stream:

- If the upstream chat-completions stream contains `choices[0].delta.tool_calls[]`, Monoize MUST convert the deltas into canonical URP v2 stream events such that downstream Responses, Messages, and Chat Completions encoders can emit protocol-correct tool-call lifecycles.

PC7a. For downstream `POST /v1/chat/completions` translated from upstream `type=chat_completion` streaming:

- If upstream already emitted at least one terminal chunk with non-null `choices[0].finish_reason`, Monoize MUST NOT append an additional synthetic terminal chat chunk with a different `finish_reason`.
- Monoize MUST preserve upstream terminal finish semantics, for example `tool_calls`, so downstream clients can continue tool loops correctly.

PC7b. If an upstream `type=chat_completion` stream emits any tool-call presence, either tool-call header chunks or argument deltas via `choices[0].delta.tool_calls[]`, in a turn, but emits terminal `choices[0].finish_reason = "stop"`, Monoize MUST normalize downstream terminal finish reason to `tool_calls` for `POST /v1/chat/completions`.

PC7c. For downstream `POST /v1/chat/completions` translated from upstream `type=chat_completion` streaming, every non-terminal downstream chunk that carries assistant deltas, including `content`, `reasoning_details`, and `tool_calls`, MUST emit `choices[0].finish_reason = null`.

PC7d. For downstream `POST /v1/chat/completions` translated from upstream `type=chat_completion` streaming, Monoize MUST emit at most one downstream chunk with non-null `choices[0].finish_reason`, and that terminal chunk MUST be emitted only after the last downstream assistant delta of the turn.

PC7e. For upstream `type=chat_completion` streaming chunks that carry assistant state as `choices[0].delta.content[]` block arrays:

- Monoize MUST decode text blocks and tool-call blocks in block-array order using the same field mapping as PC2.3.
- If one streamed assistant turn contains both text blocks and at least one tool-call block, downstream `POST /v1/chat/completions` streaming MUST preserve the assistant text deltas that precede the tool-call deltas, MUST emit the tool-call deltas after those text deltas, and MUST terminate the turn with `finish_reason="tool_calls"`.

PC7f. For upstream `type=chat_completion` streaming chunks that carry assistant state in terminal `choices[0].message` snapshots instead of incremental `choices[0].delta.tool_calls[]`:

- Monoize MUST decode the snapshot message using the same text and tool-call compatibility rules as PC2.3.
- If the snapshot contains assistant tool calls that were not previously emitted as downstream deltas, Monoize MUST emit equivalent downstream tool-call deltas before the terminal downstream chunk.
- If the snapshot contains assistant text or reasoning suffix bytes that were not previously emitted as downstream deltas, Monoize MUST emit those missing suffix deltas before the terminal downstream chunk.

PC8. Reasoning, non-stream and stream:

- Monoize MUST parse upstream Chat Completions reasoning from `choices[0].message.reasoning_details[]` and `choices[0].message.reasoning`.
- For `reasoning_details[]`, Monoize MUST interpret entries as follows:
  - `type="reasoning.text"`: `text` contributes to `Reasoning.content`.
  - `type="reasoning.encrypted"`: `data` contributes to `Reasoning.encrypted`.
  - `type="reasoning.summary"`: `summary` contributes to `Reasoning.summary` and to the simple `reasoning` alias when no `reasoning.text` content is available.
- For streaming, Monoize MUST apply the same mapping to `choices[0].delta.reasoning_details[]` deltas in arrival order.
- Monoize MUST store parsed reasoning as URP v2 `Reasoning` nodes.
- Backward compatibility: if `reasoning` and `reasoning_details` are absent, Monoize MUST still accept legacy `reasoning_content` and `reasoning_opaque` from upstream chat outputs.

PC9. For all upstream `type=chat_completion` requests, regardless of `stream` value, the Chat Completions encoder MUST unconditionally set `stream_options.include_usage = true` when the outbound request body does not already include `stream_options.include_usage`. This ensures usage data is always returned by the upstream provider and prevents billing leaks.

### 7.4 Provider adapter: `type=messages`

PM1. Monoize MUST call the upstream path `POST /v1/messages`.

PM2. Monoize MUST convert `UrpRequestV2.input` nodes into Messages `messages[]` using encoder-owned reconstruction of role and content blocks.

PM2a. Messages `phase` mapping:

- Monoize MUST accept optional extension field `phase` on Anthropic `text` blocks.
- When decoding an Anthropic `text` block, Monoize MUST copy `phase` onto the URP v2 `Text` node derived from that block.
- When encoding a URP v2 `Text` node to an Anthropic `text` block, Monoize MUST write `phase` on that block when the text node carries non-null `phase`.

PM2b. For downstream `POST /v1/messages` request parsing, Monoize MUST decode ordinary `messages[].content[]` blocks with `type = "image"` into role-bearing URP `Image` nodes. The decoder MUST support both Anthropic image source shapes below:

- `source: { type: "base64", media_type: <media type>, data: <raw base64> }` -> `ImageSource::Base64`;
- `source: { type: "url", url: <url> }` -> `ImageSource::Url`.

PM2c. For downstream `POST /v1/messages` request parsing, Monoize MUST decode ordinary `messages[].content[]` blocks with `type = "document"` or `type = "file"` into role-bearing URP `File` nodes when a supported file source is present.

PM2.1. Input coercion for Messages adapter:

- If downstream `input` is a string, Monoize MUST treat it as one user text message.
- If downstream `input` is an object with message-like fields `role` and `content` but without explicit `type`, Monoize MUST treat it as one message input object.
- If downstream `input` is an array containing message-like objects without `type`, Monoize MUST treat each such object as one message input object.

PM3. Monoize MUST convert Messages output into `UrpResponseV2` output nodes.

PM4. Tool-calling:

- When the upstream Messages output contains `tool_use` blocks, Monoize MUST convert each block into one URP assistant `ToolCall` node.
- When a downstream Messages request contains `tool_result` blocks, Monoize MUST convert them into top-level URP `ToolResult` nodes.

PM4.1. When parsing downstream Messages `tool_result.content`, Monoize MUST support:

- string text content; and
- block-array content where blocks may include `text`, `image`, and `document`.

PM4.2. For PM4.1 block-array content, Monoize MUST map blocks to `ToolResult.content` entries:

- `text` -> text entry;
- `image` -> image entry;
- `document` -> file entry.

PM4.3. When parsing upstream Messages assistant output, Monoize MUST support multimodal output blocks `image` and `document` in addition to `text`, `thinking`, and `tool_use`.

PM4.4. When encoding a request to an upstream `type=messages` provider, Monoize MUST NOT synthesize an upstream request field named `response_format`. If the downstream URP request carries `response_format`, Monoize MUST omit it unless Anthropic later defines an official request field with that exact meaning.

PM5. Reasoning:

- When the upstream Messages output contains a `thinking` block, Monoize MUST convert it into one URP `Reasoning` node.
- When the upstream Messages output contains a `redacted_thinking` block, Monoize MUST convert it into one URP `Reasoning` node with:
  - `content` absent,
  - `encrypted` set to the block's `data` value (preserved verbatim, as a JSON string when the wire value is a string),
  - `extra_body["_monoize_reasoning_kind"]` set to the string `"redacted_thinking"` so that a later encode step targeting Messages can reconstruct the original block type.

PM5a. For downstream `POST /v1/messages` request parsing, Monoize MUST accept assistant content blocks of type `thinking` and `redacted_thinking` inside `messages[].content[]` and decode them into URP `Reasoning` nodes using the same field mapping as PM5. An input `thinking` block whose `thinking` string is empty and whose `signature` is present MUST still be decoded into a `Reasoning` node with `content` absent and `encrypted` set from `signature`.

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

PM7. Messages `tool_choice` normalization:

- For downstream `POST /v1/messages`, Monoize MUST normalize Anthropic-style `tool_choice` values into URP-compatible `tool_choice` before forwarding.
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
- `content` as an array
- `stop_reason = null`
- `stop_sequence = null`
- `usage` object with token counters when available, or zeros when unavailable

PM11. For PM9 streams, `message_delta.delta.stop_reason` MUST be:

- `"tool_use"` if any tool-use block was emitted;
- otherwise `"end_turn"`.

PM12. For PM9 streams, `message_delta.usage` token counters MUST be cumulative within the stream.

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
- When encoding downstream usage objects for any downstream endpoint, Monoize MUST merge `Usage.extra_body` into the generated usage JSON, overwriting adapter-generated keys when present. The upstream's full detail objects take precedence over synthesized defaults.

XF5a. Usage alias acceptance and canonicalization:

- For each provider adapter, Monoize MUST accept all observed upstream aliases for one semantic usage metric and map them to one canonical URP field.
- Alias handling MUST be deterministic. When multiple aliases for the same semantic metric are present simultaneously, Monoize MUST apply a fixed precedence order per adapter implementation.
- Monoize MUST NOT sum alias variants unless the provider contract explicitly defines additive semantics for those fields.

XF5b. Usage forwarding boundary:

- Monoize MUST interpret only recognized usage fields into typed URP structured fields.
- Monoize MUST preserve but MUST NOT reinterpret unknown usage fields. Unknown usage fields are forwarded through `Usage.extra_body` without semantic transformation.

XF5c. Usage round-trip preservation:

- For adapters that support opaque usage fields, decode then encode through Monoize MUST preserve unknown usage fields and values in downstream usage payloads.
- If an adapter cannot represent a preserved field due to protocol limits, this loss MUST be adapter-specific and explicitly documented in the adapter section.

XF5d. Monoize usage extension field registry:

- When Monoize must encode a URP usage concept into a provider format that lacks a native field for that concept, Monoize MUST emit a Monoize extension field name for that provider format.
- Every Monoize usage extension field name emitted by an encoder MUST be accepted by the corresponding decoder as a recognized alias and MUST therefore NOT remain inside `Usage.extra_body` after decode.
- The following extension field names are reserved for Monoize usage encoding:
  - Messages or Anthropic usage object extensions:
    - `tool_prompt_input_tokens`
    - `reasoning_output_tokens`
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

XF6. When constructing an upstream request body from a URP v2 request, Monoize MUST filter top-level request `extra_body` keys against a per-provider-type whitelist before inserting them into the upstream request body. Keys not present in the effective whitelist MUST be dropped from the upstream request body.

XF6a. Default whitelists per provider type:

- `chat_completion`: `frequency_penalty`, `logit_bias`, `logprobs`, `top_logprobs`, `max_completion_tokens`, `max_tokens`, `metadata`, `presence_penalty`, `seed`, `stop`, `stream_options`, `parallel_tool_calls`, `debug`, `image_config`, `modalities`, `cache_control`, `top_k`, `top_a`, `min_p`, `repetition_penalty`, `prediction`, `route`, `structured_outputs`, `verbosity`, `models`, `provider`, `plugins`, `session_id`, `trace`.
- `responses`: `background`, `context_management`, `conversation`, `include`, `instructions`, `metadata`, `max_tool_calls`, `parallel_tool_calls`, `previous_response_id`, `prompt`, `prompt_cache_key`, `prompt_cache_retention`, `safety_identifier`, `service_tier`, `store`, `text`, `top_logprobs`, `truncation`.
- `messages`: `max_tokens`, `metadata`, `output_config`, `service_tier`, `stop_sequences`, `top_k`, `inference_geo`.
- `gemini`: `generationConfig`, `safetySettings`, `cachedContent`, `labels`.

XF6b. Each dashboard-managed provider MAY carry an optional `extra_fields_whitelist` override, JSON array of strings stored in the `monoize_providers` table. When present, the effective whitelist is the union of the default whitelist and the override list. When absent, only the default whitelist applies.

XF6c. If `extra_fields_whitelist` contains the single entry `"*"`, Monoize MUST skip whitelist filtering entirely for that provider, forwarding all top-level request `extra_body` keys unconditionally.

XF6d. Whitelist filtering applies only to top-level request `extra_body` keys. Node-local `extra_body`, `ToolResultContent.extra_body`, envelope-control nodes, and usage `extra_body` are not subject to this whitelist.

XF6e. Whitelist filtering MUST occur after request-phase transforms and before upstream request encoding. The filter runs inside `encode_request_for_provider`, immediately before dispatching to the provider-specific encoder.

### 7.7 Downstream adapter: `POST /v1/chat/completions`

DC1. Monoize MUST parse the downstream request as a Chat Completions create request and convert it into `UrpRequestV2`.

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

DC5. Reasoning:

- If `UrpResponseV2.output` contains `Reasoning` nodes, Monoize MUST:
  - render non-stream output to Chat Completions as:
    - `choices[0].message.reasoning` from URP reasoning `content` when present, otherwise from URP reasoning `summary`; and
    - `choices[0].message.reasoning_details[]` using OpenRouter reasoning item types `reasoning.summary`, `reasoning.text`, and or `reasoning.encrypted`.
  - preserve the distinction between summary text and full reasoning content. If one URP reasoning node carries both `summary` and `content`, Monoize MUST render summary text as `type="reasoning.summary"` detail entries and full reasoning content as `type="reasoning.text"` detail entries.
  - preserve every distinct encrypted reasoning payload as its own `type="reasoning.encrypted"` entry in `choices[0].message.reasoning_details[]`. Monoize MUST NOT collapse those entries to only the first encrypted payload.
  - for streaming, emit `choices[0].delta.reasoning_details[]` chunks as reasoning deltas become available.
  - preserve reasoning stream lifecycle. Each reasoning delta MUST be emitted as one chat chunk in arrival order, MAY interleave with text or tool-call chunks, and MUST terminate with the final finish chunk and `[DONE]`.
- Backward compatibility for downstream Chat Completions requests: Monoize MUST parse assistant-message reasoning from both OpenRouter fields `reasoning` and `reasoning_details` and legacy fields `reasoning_content` and `reasoning_opaque`.

DC6. If the selected upstream provider type is `chat_completion` and the upstream response contains additional non-standard fields inside `choices[0].delta` or `choices[0].message`, other than fields explicitly mapped by DC4 through DC5, Monoize MUST preserve those fields in the downstream response, streaming or non-streaming, for `POST /v1/chat/completions`.

DC7. For downstream `POST /v1/chat/completions` streaming responses synthesized from non-chat upstream event formats, Monoize MUST include a `usage` object in the terminal chat chunk when cumulative stream usage counters are available.

DC8. For downstream `POST /v1/chat/completions` streaming responses, Monoize MUST emit SSE as data-only frames. Every assistant chunk MUST be encoded as `data: {json}` with no named `event:` line, and successful stream termination MUST emit exactly one terminal `data: [DONE]` sentinel.

DC9. For downstream `POST /v1/chat/completions` streaming responses, Monoize MUST preserve these externally visible lifecycle guarantees even though canonical internal state is flat:

- exactly one plain `[DONE]` sentinel;
- exactly one terminal empty-delta finish chunk;
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
- For downstream requests, Monoize MUST parse `tool_result` blocks into top-level URP `ToolResult` nodes.

DM4.1. For downstream `tool_result` blocks that carry block-array content, Monoize MUST preserve image and file payloads when routing through URP v2 and when encoding to eligible upstream formats.

DM5. Reasoning:

- If `UrpResponseV2.output` contains `Reasoning` nodes, Monoize MUST render them as Messages `thinking` content blocks with:
  - `thinking` from URP `content` when present, otherwise from URP `summary`;
  - `signature` from URP `encrypted` when the downstream Messages block schema requires that field.

DM5.1. When rendering a URP `Reasoning` node to an Anthropic Messages content block (downstream response, upstream request as assistant history, or streaming equivalents thereof), Monoize MUST select the wire shape by the following ordered decision rule. The same rule applies to both non-streaming and streaming Anthropic encoders.

1. If `Reasoning.content` is a non-empty string, or `Reasoning.summary` is a non-empty string, emit `{"type": "thinking", "thinking": <text>, "signature"?: <encrypted>}` where `<text>` is `Reasoning.content` when present, otherwise `Reasoning.summary`. Include `"signature"` only when `Reasoning.encrypted` is a non-empty string or a non-null JSON value that can be represented as the Anthropic `signature` field.
2. Otherwise, if `Reasoning.extra_body["_monoize_reasoning_kind"] == "redacted_thinking"` and `Reasoning.encrypted` is present, emit `{"type": "redacted_thinking", "data": <encrypted>}`.
3. Otherwise the node MUST be omitted from the rendered content array. Monoize MUST NOT emit an Anthropic `thinking` block with an empty `thinking` string, MUST NOT emit a non-standard field name such as `encrypted_thinking`, and MUST NOT emit a `redacted_thinking` block from reasoning data whose kind marker is not present.

DM5.2. Reasoning item id transport through Anthropic Messages uses the signature sigil defined in PM5b. When rendering a URP `Reasoning` node to an Anthropic Messages content block under DM5.1 case 1 or case 2:

- If the encoding direction is **downstream** (the block is part of a response body Monoize returns to a `/v1/messages` client), and `Reasoning.id` and `Reasoning.encrypted` are both non-empty, Monoize MUST wrap the signature payload in the sigil `mz1.<Reasoning.id>.<original_signature>` and write that wrapped value into `thinking.signature` or `redacted_thinking.data`.
- If the encoding direction is **upstream** (the block is part of a request body Monoize sends to a `type=messages` provider), Monoize MUST strip any sigil prefix from the signature payload before emission so that the upstream receives only `<original_signature>`. Monoize MUST NOT emit any extension field such as `_monoize_item_id` on an upstream-facing block.
- Monoize MUST NOT attach a sigil when `Reasoning.id` or `Reasoning.encrypted` is absent.

DM5.3. Invariants for Anthropic Messages thinking block encoding, covering both downstream response rendering and upstream request assistant-history rendering, streaming and non-streaming:

1. Every emitted `thinking` block MUST contain a `thinking` field that is a JSON string.
2. Every emitted `redacted_thinking` block MUST contain a `data` field.
3. Monoize MUST NOT emit an Anthropic reasoning-related content block whose only reasoning payload is an empty `thinking` string, regardless of whether `signature` is present.
4. Monoize MUST NOT emit extension field `encrypted_thinking` on any Anthropic content block. The field is not part of the Anthropic wire contract.
5. Anthropic streaming reasoning lifecycles are only opened for a URP `Reasoning` node that would be emitted under DM5.1 case 1 or case 2. A node that falls into DM5.1 case 3 MUST NOT trigger `content_block_start` for a `thinking` block.

DM5a. For downstream `POST /v1/messages` streaming responses, one URP `Reasoning` node MUST be rendered as one Anthropic `thinking` block lifecycle. If both plaintext reasoning and signature are present, Monoize MUST emit `thinking_delta` first, then `signature_delta`, then `content_block_stop` for the same block index. Monoize MUST preserve upstream delta granularity for reasoning text and signatures. It MUST NOT artificially split one upstream delta into smaller chunks, and it MUST NOT artificially merge multiple upstream deltas into one larger synthetic delta.

DM5b. For downstream `POST /v1/messages` streaming responses, when a reasoning node carries a non-empty signature, Monoize MUST NOT emit `content_block_start.content_block.signature = ""`. The start payload and any later `signature_delta` MUST preserve the actual non-empty signature payload.

DM6. For downstream `POST /v1/messages` streaming responses synthesized or translated from non-messages upstream event formats, Monoize MUST set `message_delta.usage` from cumulative stream usage counters when available.

DM7. For downstream `POST /v1/messages` SSE streams, Monoize MUST emit an SSE `event:` line whose value exactly equals the payload `type` field for every named Messages event, including `message_start`, `content_block_start`, `content_block_delta`, `content_block_stop`, `message_delta`, `message_stop`, `ping`, and `error`.

DM8. For downstream `POST /v1/messages` SSE streams, every content block index MUST be emitted in strict non-interleaved lifecycle order:

- exactly one `content_block_start` for that index;
- zero or more `content_block_delta` events for that index;
- exactly one `content_block_stop` for that index;
- no events for a later-emitted block may appear between `content_block_start` and `content_block_stop` of an earlier-emitted block.

DM9. For downstream `POST /v1/messages` SSE streams, Monoize MUST NOT emit duplicate `content_block_start`, duplicate `content_block_stop`, or duplicate final-content replays for a block whose streamed deltas already carried the same text, thinking, or input-json bytes.

DM10. For downstream `POST /v1/messages` successful SSE streams, Monoize MUST terminate with `message_stop` and MUST NOT append any additional `data: [DONE]` sentinel.

DM11. For downstream `POST /v1/messages`, flat canonical storage MUST NOT weaken these externally visible lifecycle guarantees:

- `message_start` first;
- `message_delta` before `message_stop`;
- block lifecycles remain non-interleaved;
- `thinking_delta` precedes `signature_delta` for one thinking block;
- cumulative usage semantics remain explicit and testable.

### 7.9 Downstream endpoint: `POST /v1/embeddings`

DE1. Monoize MUST authenticate and apply pre-forward balance guard exactly as other forwarding endpoints.

DE2. Monoize MUST route provider and channel candidates using the same provider model-map matching rules as chat completions.

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

DMO4. `data` MUST contain the set union of logical model keys across all configured providers.

DMO5. If a logical model key appears in two or more providers, `data` MUST include exactly one item for that key.

DMO6. `data` MUST be sorted by `id` in ascending lexicographic order.

DMO7. If the authenticated API key has `model_limits_enabled = true` and `model_limits` is non-empty, Monoize MUST filter `data` to include only models whose `id` is present in the `model_limits` list. If `model_limits_enabled` is false or `model_limits` is empty, no filtering is applied.

## 8. Streaming requirements, Responses downstream

When the downstream endpoint is `POST /v1/responses` with `stream=true`, Monoize MUST respond using SSE and MUST emit the externally visible Responses lifecycle from canonical URP v2 stream events and terminal `ResponseDone.output`.

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

STR3c.1. For every downstream streaming response emitted by `POST /v1/responses`, `POST /v1/chat/completions`, or `POST /v1/messages`, Monoize MUST configure an SSE heartbeat with an interval of 15 seconds. Each heartbeat MUST be an SSE comment frame whose comment text is `heartbeat`. A heartbeat MUST NOT contain a `data:` line, MUST NOT contain an `event:` line, MUST NOT increment Responses `sequence_number`, MUST NOT count as a Chat Completions `[DONE]` sentinel, and MUST NOT count as an Anthropic Messages protocol event. The heartbeat exists only to keep downstream HTTP intermediaries from treating an otherwise-valid idle stream as inactive after Monoize has started the downstream SSE response.

STR3d. For downstream `POST /v1/responses` reasoning streams, Monoize MUST preserve the distinction between official reasoning summary events and Monoize-specific raw reasoning events. Official summary lifecycle events MUST use the OpenAI names `response.reasoning_summary_text.delta`, `response.reasoning_summary_text.done`, and `response.reasoning_summary_part.done`. Raw reasoning content is not an official OpenAI Responses event family. When Monoize emits it as an extension, it MUST use the custom event family `response.reasoning.delta` and `response.reasoning.done`.

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

STR3k. For downstream `/v1/responses` SSE translated from upstream Responses streams, Monoize MUST emit at most one `response.output_item.added` and `response.output_item.done` lifecycle per logical downstream output item. If a message or function-call item has already been streamed before `response.completed`, Monoize MUST NOT synthesize a second duplicate lifecycle for that same logical item when the terminal response snapshot arrives.

STR3l. Downstream `/v1/responses` SSE MUST preserve externally visible item identity continuity. In particular, deltas and terminal item payloads for the same logical output item MUST use the same `item_id`.

STR3m. Downstream `/v1/responses` SSE MUST preserve `phase` on reconstructed message items and text deltas when that metadata is present in URP v2 `Text` nodes.

STR3n. `ResponseDone.output` is the only authoritative terminal flat state used to reconstruct the final downstream `response.completed.response.output` array. Monoize MUST NOT duplicate final outputs by replaying items that were already emitted as completed downstream lifecycles.

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
- for `POST /v1/responses`: one named `event: error` with a sequence-numbered error payload, followed by `data: [DONE]`;
- for `POST /v1/messages`: one named `event: error` with an Anthropic-compatible error object, then close the SSE stream without appending `data: [DONE]`.

### 9.2 Mid-stream errors

SE2. If an error occurs after streaming has begun, for example upstream disconnects or parse failure, Monoize MUST emit a protocol-appropriate error event, then:

- for downstream `POST /v1/chat/completions` and `POST /v1/responses`, emit exactly one terminal `data: [DONE]` sentinel and close the stream;
- for downstream `POST /v1/messages`, close the stream without appending `data: [DONE]`.

SE3. If the downstream channel sender is already closed, client disconnected, Monoize MAY silently discard the error event.
