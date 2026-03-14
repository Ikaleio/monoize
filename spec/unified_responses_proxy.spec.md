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
- **Item:** The fundamental unit in URP-Proto `input` and `output` sequences. An `Item` is one of two variants:
  - `Item::Message { role: Role, parts: Vec<Part>, extra_body }` — a conversation message with role and typed content parts.
  - `Item::ToolResult { call_id: String, is_error: bool, content: Vec<ToolResultContent>, extra_body }` — a tool execution result.
- **ToolResultContent:** A typed content entry within `Item::ToolResult.content`. One of: `Text { text: String }`, `Image { source: ImageSource }`, or `File { source: FileSource }`.
- **ItemHeader:** Discriminated header carried by stream `ItemStart` events. One of: `Message { role: Role }` or `ToolResult { call_id: String }`.
- **PhaseZone:** State machine enum governing decoder greedy merging of upstream parts into `Item`s. Values: `Empty`, `InReasoning`, `InContent`, `InAction`.

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

C5. Monoize MUST accept downstream request bodies up to 50 MiB on forwarding endpoints (`/v1/responses`, `/v1/chat/completions`, `/v1/messages`, `/v1/embeddings`). Any framework-default extractor limit smaller than 50 MiB MUST be disabled so that the effective limit remains 50 MiB.

## 5. Forwarding pipeline (normative)

For each downstream request to any forwarding endpoint in §2.2, Monoize MUST execute the following pipeline:

FP1. **Parse:** Parse the downstream request into an internal URP-Proto request. The parser produces a sequence of `Item`s (§1) from the downstream protocol's native representation.

FP2. **Route:** Select an upstream provider for the request according to routing rules (§6).

FP3. **Adapt request:** Convert the URP-Proto request into the selected provider’s upstream request shape (§7).

FP4. **Call upstream:** Send the upstream request. If `stream=true`, Monoize MUST call the upstream in streaming mode.

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

FP4d. For streaming upstream calls, Monoize MUST track terminal-stream evidence in memory during adaptation. At minimum the tracked evidence consists of: whether a literal `[DONE]` sentinel was received, which terminal protocol event was last observed, the terminal finish reason when present, and whether Monoize emitted a synthetic terminal chunk. This evidence is observability-only and MUST NOT change downstream response semantics by itself.

FP5. **Adapt response:** Convert the upstream output (non-streaming or streaming chunks) into URP-Proto `Item`-based output.

FP5a. For streaming upstream calls, Monoize MUST first decode upstream protocol events into a sequence of `UrpStreamEvent` values before producing any downstream SSE frames.

FP5b. The pass-through streaming decoder entrypoint MUST be `stream_upstream_to_urp_events(urp, provider_type, upstream_resp, tx, started_at, runtime_metrics)`.

FP5c. `stream_upstream_to_urp_events` MUST dispatch by `provider_type` to exactly these decoder families:

- `responses` and `grok` → Responses-event decoder;
- `chat_completion` → Chat Completions decoder;
- `messages` → Anthropic Messages decoder;
- `gemini` → Gemini decoder.

`group` is virtual and MUST NOT be streamed directly.

FP6. **Render downstream:** Convert URP-Proto output into the downstream endpoint’s response shape (Responses / Chat Completions / Messages), streaming or non-streaming.

FP6a. For pass-through streaming on `/v1/responses`, `/v1/chat/completions`, and `/v1/messages`, Monoize MUST implement the downstream adapter as a two-stage pipeline connected by an in-memory channel of `UrpStreamEvent` values:

- stage 1: provider-specific decoder consumes upstream SSE and emits `UrpStreamEvent` values;
- stage 2: downstream-specific encoder consumes `UrpStreamEvent` values and emits downstream SSE frames.

FP6b. The pass-through streaming pipeline in FP6a MUST preserve existing routing, billing, logging, and terminal-stream observability semantics. The change in internal streaming representation MUST NOT by itself permit provider fallback after the first downstream byte.

FP6c. The pass-through streaming encoder entrypoint MUST be `encode_urp_stream(downstream, rx, tx, logical_model, sse_max_frame_length)`.

FP6d. `encode_urp_stream` MUST dispatch by downstream protocol to exactly these encoder families:

- `Responses` → Responses encoder;
- `ChatCompletions` → Chat Completions encoder;
- `AnthropicMessages` → Anthropic Messages encoder.

FP6e. The forwarding handler MUST spawn exactly two asynchronous tasks for pass-through streaming after upstream response headers are accepted:

- one decode task that runs `stream_upstream_to_urp_events` and sends `UrpStreamEvent` values into an in-memory channel;
- one encode task that runs `encode_urp_stream` and reads the same channel to emit downstream SSE.

FP6f. The decode task and encode task in FP6e MUST be joined before request-final logging and billing finalization.

FP6g. The streaming pipeline MUST use `ResponseDone.outputs` as the authoritative final streamed response state. Monoize MUST NOT require or depend on any helper named `merged_output_items()` in the streaming path.

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

IN1. The `input` field contains a `Vec<Item>`. Each `Item` is either `Item::Message` (a conversation message with `role`, `parts`, and `extra_body`) or `Item::ToolResult` (a tool execution result with `call_id`, `is_error`, `content: Vec<ToolResultContent>`, and `extra_body`). See §1 for type definitions.

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

TCI1. URP-Proto `input` MAY contain tool-calling state represented as `Item` variants:

- **Tool call (function call):** Represented as a `Part::ToolCall` within an `Item::Message`. Each tool call part carries `call_id: String`, `name: String`, and `arguments: String` (JSON-encoded).

- **Tool result (function call output):** Represented as `Item::ToolResult` with fields:
  - `call_id: String` — correlates to the originating tool call.
  - `is_error: bool` — indicates whether the tool execution failed.
  - `content: Vec<ToolResultContent>` — typed result content (see §7.1.1a).
  - `extra_body: HashMap<String, Value>` — preserved unknown fields per §7.7 XF4.

TCI2. Monoize MUST NOT execute tools locally. Tool execution is always performed by the downstream client.

TCI3. When Monoize forwards a request, Monoize MUST forward any tool-calling `Item`s present in URP-Proto `input` by adapting them into the selected upstream provider's request format (§7.2–§7.8).

### 7.1.1a ToolResultContent type

TRC1. `ToolResultContent` is an enum representing typed content within `Item::ToolResult.content`. The variants are:

- `Text { text: String }` — plain text result.
- `Image { source: ImageSource }` — image result with provider-appropriate source reference.
- `File { source: FileSource }` — file/document result with provider-appropriate source reference.

TRC2. Each `Item::ToolResult.content` field contains zero or more `ToolResultContent` entries. An empty `content` vector represents a tool result with no output payload.

### 7.1.2 URP-Proto (internal) reasoning item

RSN1. URP-Proto `output` MAY contain a reasoning item represented as:

```json
{ "type": "reasoning", "text": "...", "signature": "..." }
```

RSN2. `text` MUST represent human-readable reasoning text (if available).

RSN3. `signature` MUST contain an opaque provider-supplied string that can be used to correlate or verify reasoning (if available).

RSN4. Inside URP `Part::Reasoning`, the provider-supplied opaque reasoning signature MUST be stored in the typed field `encrypted`. Adapters MUST NOT move that value into `extra_body` under ad-hoc keys such as `signature` when the value semantically represents the reasoning signature payload.

### 7.1.2a URP-Proto text phase metadata

TPH1. Every URP text part MAY carry optional field `phase`.

TPH2. `phase` MUST be treated as an unconstrained string. Monoize MAY recognize known values such as `commentary` and `final_answer`, but MUST NOT reject or rewrite unknown values solely because the value is unrecognized.

TPH3. `phase` MUST follow text semantics rather than tool-call semantics. Monoize MUST attach `phase` to text parts, not to tool-call parts.

TPH4. When Monoize decodes a protocol object whose `phase` is defined at message-item level or text-block level, Monoize MUST copy that `phase` value onto every URP text part produced from that source object.

TPH5. When Monoize re-encodes URP text parts to a protocol that supports `phase`, Monoize MUST write the text part `phase` back to the target protocol object at that protocol's supported level.

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

### 7.1.5 Greedy Phase-Zone Merging

GZ1. ALL decoders MUST merge upstream protocol items into URP `Item`s using the PhaseZone state machine.

GZ2. PhaseZone has four states: `Empty`, `InReasoning`, `InContent`, `InAction`. Each decoder instance initializes PhaseZone to `Empty` at the start of each response.

GZ3. Parts are classified into three groups:

- **Reasoning-like:** `Reasoning`.
- **Content-like:** `Text`, `Image`, `Audio`, `File`, `Refusal`.
- **Action-like:** `ToolCall`.

GZ4. Transition rules for assistant `Item`s (current zone × part class → action):

| Current Zone  | Reasoning-like             | Content-like                | Action-like        |
|---------------|----------------------------|-----------------------------|--------------------|
| `Empty`       | → `InReasoning`            | → `InContent`               | → `InAction`       |
| `InReasoning` | stay in `InReasoning`      | → `InContent`               | → `InAction`       |
| `InContent`   | FLUSH, → `InReasoning`     | stay in `InContent`          | → `InAction`       |
| `InAction`    | FLUSH, → `InReasoning`     | FLUSH, → `InContent`        | stay in `InAction` |

FLUSH means: finalize the current `Item::Message`, emit it, and start a new `Item::Message` before appending the incoming part.

GZ5. A role change between consecutive upstream items MUST always trigger FLUSH, regardless of part class.

GZ6. After greedy merging, every assistant `Item::Message` MUST conform to the part-order constraint: `[Reasoning*] [ContentLike?] [ActionLike*]`.

GZ7. No decoder MUST preserve upstream item boundaries. Greedy merging always applies; upstream message/item boundaries are discarded.

GZ8. For provider streaming decoders, the `ResponseDone.outputs` payload MUST satisfy GZ1-GZ7 exactly as the corresponding non-streaming decoder would for the same completed upstream response object.

GZ9. For provider streaming decoders that forward granular upstream events before the terminal object is available, Monoize MAY emit intermediate `UrpStreamEvent` values that still reflect upstream event boundaries. This allowance applies only to pre-terminal stream events. It MUST NOT relax GZ8 for `ResponseDone.outputs`.

### 7.1.6 Encoder Splitting

ES1. The Responses encoder MUST split a merged `Item::Message` back into native Responses output items: `reasoning` items for Reasoning-like parts, `message` items for Content-like parts, and `function_call` items for Action-like parts. A single merged `Item::Message` with reasoning, text, and tool calls produces three or more native Responses items.

ES2. The Chat Completions encoder MUST prevent content/tool-call interleaving within a single chat message by splitting `Item::Message` items at content-to-action and action-to-content boundaries.

ES2a. When rendering a non-streaming Chat Completions response, Monoize MUST merge all assistant `Item::Message` outputs back into one `choices[0].message` object. The merged object MUST preserve the full assistant content sequence, all tool calls, reasoning fields, and preserved unknown fields. Monoize MUST NOT silently discard later assistant message segments.

ES3. The Anthropic Messages encoder MUST merge consecutive assistant `Item::Message` items into a single `messages[]` entry with `role="assistant"`, concatenating their content blocks.

### 7.7 Extra field forwarding

XF1. For any downstream endpoint in §2.2, Monoize MUST preserve unknown fields according to §3 and store them in the internal URP request field named `extra`.

XF2. When constructing an upstream request body from a URP request, Monoize MUST insert every key-value pair from `extra` as a top-level JSON key in the upstream request body, **unless** that key is already present in the upstream request body due to adapter logic.

XF3. Adapter-generated keys MUST take precedence over keys from `extra` (i.e. `extra` MUST NOT overwrite adapter-generated keys).

XF4. Content-block-level unknown fields:

- When decoding a downstream request, Monoize MUST preserve unknown fields on individual content blocks (e.g. `cache_control` on a text block) into the corresponding URP part's `extra_body`.
- When encoding an upstream request, Monoize MUST merge each URP part's `extra_body` into the generated content-block JSON object, subject to the same precedence rule as XF3 (adapter-generated keys take precedence).
- This applies to all content-block types: `text`, `image`, `document`/`file`, `thinking`, `tool_use`, and `tool_result` blocks.
  This applies to system blocks, regular message content blocks, tool-result inner content blocks, and response content blocks.

XF4a. Item-level unknown fields:

- When decoding protocol objects that correspond to a URP `Item` (for example Responses `message` items or Chat `assistant` messages), Monoize MUST preserve unknown fields on that protocol object in the corresponding `Item::Message.extra_body` or `Item::ToolResult.extra_body`.
- When encoding protocol objects from a URP `Item`, Monoize MUST merge the `Item` variant's `extra_body` into every generated protocol object derived from that `Item`, subject to the same precedence rule as XF3.

XF5. Usage-level unknown fields:

 When parsing upstream usage objects, Monoize MUST populate the URP `Usage` struct's structured fields (`input_tokens`, `output_tokens`, `input_details`, `output_details`) from recognized provider-specific fields. Any unrecognized fields MUST be captured into `Usage.extra_body`.
 When encoding downstream usage objects for any downstream endpoint (`/v1/chat/completions`, `/v1/responses`, `/v1/messages`), Monoize MUST merge `Usage.extra_body` into the generated usage JSON, overwriting adapter-generated keys when present (the upstream's full detail objects take precedence over synthesized defaults).

XF5a. Usage alias acceptance and canonicalization:

- For each provider adapter, Monoize MUST accept all observed upstream aliases for a semantic usage metric (for example cache creation/write aliases) and map them to one canonical URP field.
- Alias handling MUST be deterministic. When multiple aliases for the same semantic metric are present simultaneously, Monoize MUST apply a fixed precedence order per adapter implementation.
- Monoize MUST NOT sum alias variants unless the provider contract explicitly defines additive semantics for those fields.

XF5b. Usage forwarding boundary:

- Monoize MUST interpret only recognized usage fields into typed URP structured fields.
- Monoize MUST preserve but MUST NOT reinterpret unknown usage fields. Unknown usage fields are forwarded through `Usage.extra_body` without semantic transformation.

XF5c. Usage round-trip preservation:

- For adapters that support opaque usage fields, decode→encode through Monoize MUST preserve unknown usage fields and values in downstream usage payloads.
- If an adapter cannot represent a preserved field due to protocol limits, this loss MUST be adapter-specific and explicitly documented in the adapter section.

XF5d. Monoize usage extension field registry:

- When Monoize must encode a URP usage concept into a provider format that lacks a native field for that concept, Monoize MUST emit a Monoize extension field name for that provider format.
- Every Monoize usage extension field name emitted by an encoder MUST be accepted by the corresponding decoder as a recognized alias and MUST therefore NOT remain inside `Usage.extra_body` after decode.
- The following extension field names are reserved for Monoize usage encoding:
  - Messages / Anthropic usage object extensions:
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
    - `tool_prompt_input_tokens` inside `prompt_tokens_details` / `input_tokens_details`
    - `accepted_prediction_output_tokens` inside `completion_tokens_details` / `output_tokens_details`
    - `rejected_prediction_output_tokens` inside `completion_tokens_details` / `output_tokens_details`
  - Responses usage detail aliases accepted for symmetry:
    - `tool_prompt_input_tokens` inside `input_tokens_details` / `prompt_tokens_details`
    - `accepted_prediction_output_tokens` inside `output_tokens_details` / `completion_tokens_details`
    - `rejected_prediction_output_tokens` inside `output_tokens_details` / `completion_tokens_details`
- These names are Monoize-defined extensions, not claims about native upstream contracts. If an upstream provider later adopts one of these names with incompatible semantics, Monoize MUST treat that as a spec-level conflict requiring explicit review.

### 7.2 Provider adapter: `type=responses`

PR1. Monoize MUST call the upstream path `POST /v1/responses`.

PR2. For non-streaming, Monoize MUST parse the upstream response as a Responses response object and convert it to URP-Proto output.

PR2a. Responses item order preservation:

- When Monoize decodes Responses `input[]` or `output[]`, Monoize MUST process items in source order and preserve that order in the resulting URP message/part sequence.
- When Monoize encodes URP back to Responses `input[]` or `output[]`, Monoize MUST emit items in URP order.
- Monoize MUST NOT postpone all text emission into one final `message` item if that would reorder text relative to `function_call`, `reasoning`, or future item kinds.
- Monoize MAY merge only contiguous message-compatible parts into one Responses `message` item.
- Monoize MUST split Responses `message` items when the URP order crosses a non-message item boundary or when adjacent URP text parts have different `phase` values.

PR3. For streaming, Monoize MUST parse upstream SSE and convert it into Monoize downstream SSE format (§8) if the downstream endpoint is `POST /v1/responses`.

PR4. When constructing upstream `POST /v1/responses` requests, Monoize MUST emit `tools[]` in Responses-style function-tool shape (`type/name/parameters`) even if the downstream request used another tool schema.

PR4a. Responses `phase` mapping:

- When decoding Responses `message` items, Monoize MUST read `item.phase` when present and copy it onto every URP text part derived from that `message` item.
- When encoding Responses `message` items from URP text parts, Monoize MUST write `phase` on the generated `message` item when the contiguous run of text parts shares the same non-null `phase`.
- If a contiguous Responses `message` item contains no URP text parts, Monoize MUST NOT invent a `phase` value.

PR5. When parsing upstream Responses SSE, Monoize MUST support canonical Responses event payloads where:

- text deltas are carried in `delta` for `response.output_text.delta`;
- tool-call items are nested under `item` for `response.output_item.added` / `response.output_item.done`;
- argument deltas identify the call via `output_index` (not necessarily `call_id`) for `response.function_call_arguments.delta`.

PR5a. Responses stream item-start reconstruction:

- When upstream emits `response.output_item.added` for a top-level Responses output item of type `reasoning` or `function_call`, Monoize MUST emit a URP `ItemStart` before any URP `Delta` derived from that output item.
- For such top-level `reasoning` and `function_call` outputs, Monoize MUST also allocate and emit a URP `PartStart` before forwarding any URP `Delta` derived from the same output item because upstream Responses does not guarantee a preceding `response.content_part.added` event for those item types.
- Monoize MUST decode top-level Responses `reasoning` and `function_call` output items as assistant URP message items containing one reasoning part or one tool-call part, respectively.

PR5b. Responses stream index normalization:

- Upstream Responses `output_index` and `content_index` are upstream protocol coordinates and MUST NOT be reused as URP `item_index` or `part_index` values by assumption alone.
- The Responses streaming decoder MUST assign URP `item_index` values sequentially starting from 0 in first-seen output-item order, satisfying STR6 even when greedy regrouping later collapses multiple upstream output items into one final `ResponseDone.outputs` item.
- The Responses streaming decoder MUST assign URP `part_index` values from its own sequential namespace and maintain a mapping from upstream coordinates to URP part indices for all later `Delta`/`PartDone` events.
- During downstream Responses re-encoding, Monoize MUST continue to emit upstream-facing `output_index` / `content_index` coordinates required by the Responses wire protocol, but those coordinates are adapter-local and MUST NOT alter URP stream-index semantics.

PR6. If upstream Responses streaming does not emit `response.output_text.delta` but emits assistant message text inside `response.output_item.added` and/or `response.output_item.done`, Monoize MUST reconstruct semantically equivalent downstream text streaming from those message items.

PR6b. Responses streaming phase preservation:

- When parsing upstream Responses SSE, Monoize MUST track the active output item context by `output_index` or equivalent synthetic index.
- If the active output item is a `message` item with field `phase`, Monoize MUST associate subsequent text deltas for that item with the same `phase` value.
- When Monoize emits Responses-style downstream SSE, it MUST NOT merge text across distinct `phase` values or across tool-call boundaries.
- When Monoize synthesizes missing downstream events from `response.completed.output[]`, it MUST preserve both item order and `phase` metadata from the completed payload.
- Unknown SSE event names, unknown item types, and unknown fields MUST NOT terminate streaming adaptation solely because they are unknown.

PR6c. Final Responses-stream object synthesis:

- If the upstream Responses stream terminates without a `response.completed` event, Monoize MUST synthesize exactly one terminal `UrpStreamEvent::ResponseDone` from the accumulated stream state.
- If the upstream Responses stream already emitted `response.completed`, Monoize MUST forward at most one corresponding `UrpStreamEvent::ResponseDone`. Monoize MUST NOT emit a duplicate synthetic terminal response after forwarding the upstream terminal response.
- The synthesized or forwarded `ResponseDone.outputs` value MUST use greedy regrouping per GZ8.
- When the accumulated assistant text is empty, Monoize MUST NOT synthesize an empty assistant text `Item::Message` solely to carry that empty string.
- When Monoize emits downstream `response.completed` from `ResponseDone.outputs`, the `response.output` array MUST be encoded with the same Responses encoder logic used for non-streaming responses so that top-level `reasoning`, `message`, and `function_call` items remain in canonical Responses output positions rather than being nested inside `message.content[]`.

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

PC2a. Chat `phase` mapping:

- Monoize MUST accept optional extension field `phase` on chat assistant messages.
- When decoding a chat assistant message, Monoize MUST copy `phase` onto every URP text part derived from that assistant message.
- When encoding URP assistant text parts back to chat messages, Monoize MUST split assistant messages whenever flattening would combine text with different `phase` values or combine text across tool-call boundaries.
- Monoize MUST merge the `Item::Message` variant's `extra_body` into every emitted chat message segment created from one URP `Item::Message`.

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

PM2a. Messages `phase` mapping:

- Monoize MUST accept optional extension field `phase` on Anthropic `text` blocks.
- When decoding an Anthropic `text` block, Monoize MUST copy `phase` onto the URP text part derived from that block.
- When encoding a URP text part to an Anthropic `text` block, Monoize MUST write `phase` on that block when the URP text part carries non-null `phase`.

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

- `promptTokenCount -> input_tokens`
- `candidatesTokenCount -> output_tokens`
- `thoughtsTokenCount -> output_details.reasoning_tokens` when present.
- `cachedContentTokenCount -> input_details.cache_read_tokens` when present.
- `cacheCreationTokenCount -> input_details.cache_creation_tokens` when present.
- `toolPromptInputTokenCount -> input_details.tool_prompt_tokens` when present.
- `acceptedPredictionOutputTokenCount -> output_details.accepted_prediction_tokens` when present.
- `rejectedPredictionOutputTokenCount -> output_details.rejected_prediction_tokens` when present.

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

- `input_tokens = usage.prompt_tokens`
- `output_tokens = 0`

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
- `response.output_item.added` (at least one output item)
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

### 8.1 Internal URP stream events

STR4. URP-Proto internally represents stream boundaries using `ItemStart` and `ItemDone` events:

- `ItemStart` carries:
  - `item_index: u32` — zero-based index of this item in the output sequence.
  - `header: ItemHeader` — discriminated header identifying the item variant.

- `ItemDone` carries:
  - `item_index: u32` — matching index from the corresponding `ItemStart`.
  - `item: Item` — the fully assembled `Item`.

STR5. `ItemHeader` is an enum with two variants:
- `Message { role: Role }` — indicates the item is an `Item::Message` with the given role.
- `ToolResult { call_id: String }` — indicates the item is an `Item::ToolResult` for the given call.

STR6. Every `ItemStart` event MUST be followed by exactly one `ItemDone` event with the same `item_index` before another `ItemStart` may be emitted. `item_index` values MUST be assigned sequentially starting from 0 within a single response stream.

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
