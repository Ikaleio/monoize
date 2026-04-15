# URP v2 Flat Structure Specification

## 0. Status

- Version: `2.0.0`
- Product name: Monoize
- Internal protocol name: `URP v2`
- Scope: canonical flat request and response storage, canonical flat streaming events, unknown-field passthrough, cross-family nested-field stripping, control-node semantics, and downstream envelope reconstruction invariants.

## 1. Terminology

- **Node sequence**: An ordered array of URP v2 `Node` values.
- **Ordinary node**: A role-bearing top-level node that represents one conversational, media, refusal, reasoning, tool-call, or provider-item unit.
- **ToolResult node**: A distinct top-level node that represents one completed tool result correlated by `call_id`.
- **Control node**: A top-level node with no user-visible content. In this spec the only control node kind is `next_downstream_envelope_extra`.
- **Consumable envelope**: One concrete downstream protocol object created by an encoder from one or more consecutive URP nodes. Examples include one Responses `message` item, one Chat Completions `message`, one Anthropic `message`, or one Anthropic `tool_result` block container.
- **Protocol family**: One of these exact families:
  - `responses`: downstream `/v1/responses` or upstream provider type `responses`
  - `chat_completion`: downstream `/v1/chat/completions` or upstream provider type `chat_completion`
  - `messages`: downstream `/v1/messages` or upstream provider type `messages`
- **Cross-family hop**: Any encode step whose source family differs from its target family. Any hop involving `gemini` or `openai_image` is cross-family.

## 2. Canonical non-stream objects

URPV2-1. The canonical internal request object MUST be:

```text
UrpRequestV2 {
  model: String,
  input: Vec<Node>,
  stream?: bool,
  temperature?: number,
  top_p?: number,
  max_output_tokens?: integer,
  reasoning?: ReasoningConfig,
  tools?: Vec<ToolDefinition>,
  tool_choice?: ToolChoice,
  response_format?: ResponseFormat,
  user?: String,
  ...extra_body
}
```

URPV2-2. The canonical internal response object MUST be:

```text
UrpResponseV2 {
  id: String,
  model: String,
  output: Vec<Node>,
  finish_reason?: FinishReason,
  usage?: Usage,
  ...extra_body
}
```

URPV2-3. `input` and `output` MUST be ordered `Vec<Node>` sequences. Node order is canonical.

URPV2-4. `Message { role, parts }` is not a URP v2 value and MUST NOT appear in canonical storage.

URPV2-5. A decoder, transform, or encoder MUST NOT infer a canonical grouped message boundary from adjacency alone. Grouping exists only inside the encoder that is building one concrete downstream protocol object.

URPV2-6. Top-level request and response objects MUST support unknown-field passthrough through flattened `extra_body`.

URPV2-7. If a key exists in both a typed top-level field and top-level `extra_body`, the typed field value MUST win.

URPV2-8. The flat redesign changes only canonical conversational storage. `usage`, `finish_reason`, model selection fields, and top-level request controls remain top-level request or response fields and are not represented as nodes.

## 3. Canonical node model

URPV2-9. `Node` MUST be the discriminated union below.

```text
Node =
  | Text {
      type: "text",
      role: OrdinaryRole,
      content: String,
      phase?: String,
      ...extra_body
    }
  | Image {
      type: "image",
      role: OrdinaryRole,
      source: ImageSource,
      ...extra_body
    }
  | Audio {
      type: "audio",
      role: OrdinaryRole,
      source: AudioSource,
      ...extra_body
    }
  | File {
      type: "file",
      role: OrdinaryRole,
      source: FileSource,
      ...extra_body
    }
  | Refusal {
      type: "refusal",
      role: "assistant",
      content: String,
      ...extra_body
    }
  | Reasoning {
      type: "reasoning",
      role: "assistant",
      content?: String,
      summary?: String,
      encrypted?: JsonValue,
      source?: String,
      ...extra_body
    }
  | ToolCall {
      type: "tool_call",
      role: "assistant",
      call_id: String,
      name: String,
      arguments: String,
      ...extra_body
    }
  | ProviderItem {
      type: "provider_item",
      role: OrdinaryRole,
      item_type: String,
      body: JsonValue,
      ...extra_body
    }
  | ToolResult {
      type: "tool_result",
      call_id: String,
      is_error?: bool,
      content: Vec<ToolResultContent>,
      ...extra_body
    }
  | NextDownstreamEnvelopeExtra {
      type: "next_downstream_envelope_extra",
      ...extra_body
    }
```

URPV2-10. `OrdinaryRole` MUST be one of `system`, `developer`, `user`, or `assistant`.

URPV2-11. `tool` is not an ordinary role in URP v2. Tool execution output is represented only by `ToolResult`.

URPV2-12. `ToolResultContent` MUST be the discriminated union below.

```text
ToolResultContent =
  | Text { type: "text", text: String, ...extra_body }
  | Image { type: "image", source: ImageSource, ...extra_body }
  | File { type: "file", source: FileSource, ...extra_body }
```

URPV2-13. Sources MUST use the exact typed shapes below.

```text
ImageSource =
  | Url { type: "url", url: String, detail?: String }
  | Base64 { type: "base64", media_type: String, data: String }

AudioSource =
  | Url { type: "url", url: String }
  | Base64 { type: "base64", media_type: String, data: String }

FileSource =
  | Url { type: "url", url: String }
  | Base64 {
      type: "base64",
      filename?: String,
      media_type: String,
      data: String
    }
```

### 3.1 Ordinary node invariants

ORD-1. Every ordinary node MUST carry `role` directly on the node. No ordinary node may be nested under a message wrapper.

ORD-2. `Reasoning`, `ToolCall`, and `Refusal` nodes MUST use `role = "assistant"`.

ORD-3. `Text`, `Image`, `Audio`, `File`, and `ProviderItem` nodes MAY use any `OrdinaryRole`.

ORD-4. `Text.phase` belongs only to `Text` nodes.

ORD-5. `Text.phase`, when present, MUST be treated as an unconstrained string. The decoder MUST preserve unknown values byte-for-byte. The encoder MUST NOT rewrite or drop a non-empty `phase` value solely because the value is not recognized locally.

ORD-6. A decoder MUST emit one ordinary node for each source-order semantic unit that the upstream protocol exposes. The decoder MUST NOT first merge several ordinary units into a logical message envelope.

ORD-7. Ordinary node `extra_body` stores unknown fields that belong to exactly that ordinary node's protocol object.

ORD-8. If a key exists in both an ordinary node typed field and that node's `extra_body`, the typed field value MUST win.

### 3.2 Reasoning invariants

RSN-1. `Reasoning.content` is plaintext reasoning text. `Reasoning.summary` is plaintext summary text. The two fields are semantically distinct.

RSN-2. A `Reasoning` node MAY carry `content`, `summary`, both, or neither.

RSN-3. If both `content` and `summary` are present, canonical URP storage MUST preserve both. One field MUST NOT overwrite the other.

RSN-4. `Reasoning.encrypted` is an opaque provider payload. URP MUST store it in the typed field `encrypted`. URP MUST NOT move that value into `extra_body` under ad hoc keys such as `signature`.

RSN-5. `Reasoning.source` is the exact provider-supplied source identifier when the provider sends one.

RSN-6. If upstream omits reasoning source, URP MUST leave `source` absent. URP MUST NOT invent a fallback source such as a router name, provider name, or model identifier.

RSN-7. If upstream sends an empty-string reasoning source, URP MUST treat it as absent.

RSN-8. Distinct `Reasoning` nodes are order-significant. URP MUST preserve their relative order exactly.

### 3.3 Tool-call and tool-result invariants

TCL-1. `ToolCall.call_id` MUST be non-empty.

TCL-2. `ToolCall.arguments` MUST be a JSON-encoded string. If a source protocol delivers structured arguments as a JSON object or array, the decoder MUST serialize that structured value to JSON text before storing it in `arguments`.

TR-1. `ToolResult` is a distinct top-level node. It MUST NOT carry `role`.

TR-2. `ToolResult.call_id` MUST be non-empty and MUST correlate to the originating `ToolCall.call_id` byte-for-byte.

TR-3. A terminal node sequence MUST NOT contain two distinct `ToolResult` nodes with the same `call_id`.

TR-4. `ToolResult.content` order is canonical. The encoder MUST preserve that order when rendering a protocol that supports multimodal tool results.

TR-5. `ToolResult.is_error` defaults to `false` when absent.

TR-6. `ToolResult.extra_body` stores unknown fields that belong to the protocol object representing that one tool result.

TR-7. If a key exists in both a `ToolResult` typed field and `ToolResult.extra_body`, the typed field value MUST win.

TR-8. `ToolResultContent.extra_body` stores unknown fields that belong to exactly that one content entry inside the tool result.

TR-9. If a key exists in both a `ToolResultContent` typed field and that entry's `extra_body`, the typed field value MUST win.

### 3.4 Control-node invariants

CTL-1. `NextDownstreamEnvelopeExtra` is the only control node kind in URP v2.

CTL-2. A control node carries no typed payload besides `type`. Every other field on the control node belongs to its flattened `extra_body`.

CTL-3. A control node does not represent user-visible content, a tool result, usage, or finish state.

CTL-4. A decoder MUST emit `NextDownstreamEnvelopeExtra` immediately before the first URP node derived from a protocol envelope when that protocol envelope contains unknown fields that do not belong to exactly one emitted ordinary node or `ToolResult` node.

CTL-5. A control node applies only to the next consumable envelope opened by the downstream encoder.

CTL-6. A control node is an explicit envelope boundary. When an encoder encounters a control node, it MUST flush any currently open consumable envelope before buffering that control node for later consumption.

CTL-7. Consecutive control nodes before one consumable envelope are legal. The encoder MUST merge their `extra_body` maps in source order. If the same key appears more than once, the later control node value MUST win.

CTL-8. When the encoder opens the next consumable envelope, it MUST merge the buffered control-node map into that envelope object. If a key exists in both the control-node map and a typed field generated by the adapter for that envelope, the typed field value MUST win.

CTL-9. A control node MUST NOT generate an empty downstream envelope by itself.

CTL-10. A valid terminal node sequence MUST NOT end with `NextDownstreamEnvelopeExtra`.

CTL-11. If a decoder or encoder reaches terminal end-of-sequence or end-of-stream while a control node remains unmatched, it MUST discard that control node and MUST NOT emit an empty envelope or an error solely because the node was unmatched.

## 4. Unknown-field passthrough and cross-family stripping

XTRA-1. URP v2 MUST preserve unknown top-level request and response fields in top-level `extra_body`.

XTRA-2. URP v2 MUST preserve unknown node-local fields in the owning ordinary node, `ToolResult`, or `ToolResultContent` `extra_body`.

XTRA-3. URP v2 MUST preserve unknown envelope-level fields with `NextDownstreamEnvelopeExtra` rather than inventing a synthetic message wrapper.

XTRA-4. Cross-family stripping applies only to nested passthrough state. Top-level request and response `extra_body` are not nested and are not removed by this rule.

XTRA-5. Immediately before an encode step into a different protocol family, the runtime MUST:

1. clear `extra_body` on every ordinary node;
2. clear `extra_body` on every `ToolResult` node;
3. clear `extra_body` on every `ToolResultContent` entry; and
4. remove every `NextDownstreamEnvelopeExtra` control node.

XTRA-6. After XTRA-5, later transforms or adapters for the target family MAY add new target-family passthrough fields.

XTRA-7. On a same-family encode step, the runtime MUST preserve node `extra_body`, `ToolResultContent.extra_body`, and control nodes.

## 5. Canonical flat streaming events

STR-1. The canonical URP v2 streaming representation MUST be:

```text
UrpStreamEventV2 =
  | ResponseStart {
      id: String,
      model: String,
      ...extra_body
    }
  | NodeStart {
      node_index: u32,
      header: NodeHeader,
      ...extra_body
    }
  | NodeDelta {
      node_index: u32,
      delta: NodeDelta,
      usage?: Usage,
      ...extra_body
    }
  | NodeDone {
      node_index: u32,
      node: Node,
      usage?: Usage,
      ...extra_body
    }
  | ResponseDone {
      finish_reason?: FinishReason,
      usage?: Usage,
      output: Vec<Node>,
      ...extra_body
    }
  | Error {
      code?: String,
      message: String,
      ...extra_body
    }
```

STR-2. `NodeHeader` MUST be the discriminated union below.

```text
NodeHeader =
  | Text { role: OrdinaryRole, phase?: String }
  | Image { role: OrdinaryRole }
  | Audio { role: OrdinaryRole }
  | File { role: OrdinaryRole }
  | Refusal { role: "assistant" }
  | Reasoning { role: "assistant" }
  | ToolCall { role: "assistant", call_id: String, name: String }
  | ProviderItem { role: OrdinaryRole, item_type: String }
  | ToolResult { call_id: String }
  | NextDownstreamEnvelopeExtra
```

STR-3. `NodeDelta` MUST be the discriminated union below.

```text
NodeDelta =
  | Text { content: String }
  | Reasoning {
      content?: String,
      summary?: String,
      encrypted?: JsonValue,
      source?: String
    }
  | Refusal { content: String }
  | ToolCallArguments { arguments: String }
  | Image { source: ImageSource }
  | Audio { source: AudioSource }
  | File { source: FileSource }
  | ProviderItem { data: JsonValue }
```

STR-4. `node_index` is a URP-local index. A decoder MUST assign `node_index` values sequentially starting at `0` in first-seen node order. A decoder MUST NOT reuse an upstream protocol index as a URP `node_index` by assumption alone.

STR-5. For each streamed `node_index`, there MUST be exactly one `NodeStart` and exactly one `NodeDone`. Every `NodeDelta` for that `node_index` MUST occur after `NodeStart` and before `NodeDone`.

STR-6. `ToolResult` and `NextDownstreamEnvelopeExtra` nodes MUST have zero `NodeDelta` events.

STR-7. `NodeDone.node` MUST contain the complete terminal node for that `node_index`.

STR-8. `ResponseDone.output` MUST contain the complete terminal ordered node sequence.

STR-9. `ResponseDone.output` is the authoritative final streamed response state. Downstream stream reconstruction, synthetic terminal event synthesis, and post-stream transforms MUST use `ResponseDone.output` rather than any ad hoc merged helper state.

STR-10. Stream decoders MUST emit flat nodes directly. They MUST NOT pre-group stream state into message envelopes before entering the URP event channel.

STR-11. Downstream stream encoders own logical envelope reconstruction from `NodeStart`, `NodeDelta`, `NodeDone`, and `ResponseDone.output`.

### 5.1 Delta accumulation invariants

SACC-1. For `NodeDelta::Text`, terminal `Text.content` is the ordered concatenation of all `content` fragments for that `node_index`.

SACC-2. For `NodeDelta::ToolCallArguments`, terminal `ToolCall.arguments` is the ordered concatenation of all `arguments` fragments for that `node_index`.

SACC-3. For `NodeDelta::Reasoning.content`, terminal `Reasoning.content` is the ordered concatenation of all non-null `content` fragments for that `node_index`.

SACC-4. For `NodeDelta::Reasoning.summary`, terminal `Reasoning.summary` is the ordered concatenation of all non-null `summary` fragments for that `node_index`.

SACC-5. If a streamed `Reasoning.encrypted` payload is emitted incrementally and each delta `encrypted` value is a string, terminal `Reasoning.encrypted` is the ordered concatenation of those string fragments.

SACC-6. If a provider supplies a non-string `Reasoning.encrypted` payload, the decoder MUST emit that value only in `NodeDone.node` and `ResponseDone.output`, or in exactly one `NodeDelta`. The decoder MUST NOT split a non-string JSON value across several deltas.

SACC-7. For `NodeDelta::Reasoning.source`, the terminal `Reasoning.source` value is the most recent non-empty `source` value seen for that `node_index`.

SACC-8. An empty-string `Reasoning.source` delta MUST be ignored.

## 6. Encoder-owned logical envelope reconstruction

RECON-1. URP v2 canonical storage is only the flat node sequence. Message grouping, output-item grouping, and block grouping are encoder responsibilities.

RECON-2. An encoder MAY place consecutive ordinary nodes into one consumable envelope only when all conditions below hold:

1. every grouped value is an ordinary node;
2. no `ToolResult` node lies inside the grouped run;
3. no `NextDownstreamEnvelopeExtra` node lies inside the grouped run;
4. grouping preserves original node order exactly; and
5. grouping does not violate a protocol-specific boundary rule in this section.

RECON-3. A `ToolResult` node is always its own top-level semantic unit. An encoder MUST NOT merge a `ToolResult` node into an ordinary-node envelope.

RECON-4. A control node is not itself emitted downstream. Its only effect is to modify the next consumable envelope under CTL-5 through CTL-8.

### 6.1 Responses stability and reconstruction

RESP-1. The Responses encoder MUST reconstruct canonical Responses output items from flat nodes.

RESP-2. Each `Reasoning` node MUST encode as one top-level Responses `reasoning` item.

RESP-3. Each `ToolCall` node MUST encode as one top-level Responses `function_call` item.

RESP-4. Each maximal run of adjacent ordinary nodes that are not `Reasoning` and not `ToolCall`, and that share the same `role`, MAY encode as one Responses `message` item.

RESP-5. A change in `Text.phase` value inside a Responses `message` run MUST force a new Responses `message` item boundary.

RESP-6. `NextDownstreamEnvelopeExtra` applies to the next Responses output item envelope created under RESP-2 through RESP-4.

RESP-7. Responses streaming output MUST preserve canonical event lifecycle semantics:

1. `response.created` occurs before any output-item lifecycle event;
2. every emitted `response.output_item.done` has exactly one earlier matching `response.output_item.added` with the same `output_index`;
3. `response.content_part.added` and `response.content_part.done` are emitted only for Responses `message` items;
4. `response.completed` reflects the same reconstructed `response.output` ordering used by the terminal item lifecycle; and
5. the stream terminates with exactly one plain `data: [DONE]` sentinel.

RESP-8. The Responses encoder MUST preserve externally visible addressing and lifecycle semantics for `response_id`, `item_id`, `output_index`, `content_index`, and item `status`. These fields are encoder-owned output coordinates derived from the flat node order.

RESP-9. Responses external reasoning behavior remains stable:

1. summary text remains distinct from full reasoning text;
2. opaque reasoning payload remains typed reasoning data rather than plain text;
3. `source` is preserved when present upstream; and
4. `source` is omitted when upstream omitted it.

RESP-10. Downstream `/v1/responses` streaming MUST NOT introduce a custom `response.reasoning_signature.delta` event. Opaque reasoning state is surfaced only through canonical reasoning item events and terminal response state.

### 6.2 Anthropic Messages stability and reconstruction

MSG-1. The Messages encoder MUST reconstruct one Anthropic `message` envelope whose `content[]` block order matches flat-node order after applying protocol-required role and block mapping.

MSG-2. Each emitted Messages `content` block index MUST equal that block's final zero-based position in the reconstructed `content[]` array.

MSG-3. Messages streaming output MUST preserve this exact lifecycle order:

1. `message_start` first;
2. for each content block, `content_block_start`, then zero or more `content_block_delta`, then `content_block_stop`;
3. `message_delta` after the final `content_block_stop`;
4. `message_stop` last.

MSG-4. A Messages stream MUST NOT append `[DONE]`.

MSG-5. Content-block lifecycles MUST NOT interleave. At most one content block may be open at a time.

MSG-6. `Reasoning` nodes MUST reconstruct Anthropic `thinking` blocks. If the stream exposes both thinking text and signature state, `thinking_delta` MUST occur before `signature_delta`, and both MUST occur before that block's `content_block_stop`.

MSG-7. `ToolCall` nodes MUST reconstruct Anthropic `tool_use` blocks. Streamed tool input JSON remains block-scoped and index-scoped.

MSG-8. `ToolResult` nodes MUST reconstruct Anthropic `tool_result` blocks as distinct tool-result protocol objects. They MUST NOT be rewritten as ordinary role-bearing nodes.

### 6.3 OpenRouter-compatible Chat stability and reconstruction

CHAT-1. The Chat Completions encoder MUST preserve OpenRouter-compatible chat behavior. The flat redesign MUST NOT reduce the downstream contract to plain OpenAI Chat Completions.

CHAT-2. In non-stream chat responses, `choices[0].message.content` MUST remain a JSON string. If several text nodes are merged into one downstream assistant message, the encoder MUST concatenate them in source order with `"\n\n"` between adjacent text segments.

CHAT-3. Structured reasoning in chat responses MUST remain in `reasoning_details`. Plaintext assistant reasoning, when emitted as a simple scalar field, remains `message.reasoning` for non-stream and `delta.reasoning` only when the downstream protocol already defines that exact field for plain text.

CHAT-4. Chat `reasoning_details[]` entries MUST preserve the OpenRouter-compatible discriminated union:

1. `{ "type": "reasoning.summary", "summary": ... }`
2. `{ "type": "reasoning.text", "text": ... }`
3. `{ "type": "reasoning.encrypted", "data": ... }`

CHAT-5. Opaque encrypted reasoning payloads MUST appear only in `reasoning_details[]` entries with `type = "reasoning.encrypted"` and field `data`.

CHAT-6. Streaming chat output remains data-only SSE and terminates with exactly one `[DONE]` sentinel.

CHAT-7. If streamed chat output emits tool-call deltas, terminal `finish_reason` semantics for that downstream stream remain `tool_calls`.

CHAT-8. Final usage chunk semantics remain externally stable. A terminal usage chunk may occur once before `[DONE]` with empty `choices`.

CHAT-9. SSE comment lines and post-start chunk-shaped error payloads remain representable downstream. The flat URP redesign MUST NOT remove support for those externally visible chat stream forms.

## 7. Validity summary

VALID-1. A valid terminal URP v2 sequence is an ordered `Vec<Node>` with no `Message { role, parts }` wrapper.

VALID-2. A valid terminal sequence MUST NOT end with `NextDownstreamEnvelopeExtra`.

VALID-3. `ToolResult` remains a distinct top-level node variant and MUST NOT be reclassified as an ordinary role-bearing node.

VALID-4. Terminal stream state is authoritative. `ResponseDone.output` is the final flat node sequence.

VALID-5. Decoder complexity is minimized by emitting flat nodes only. Encoder complexity owns all logical envelope reconstruction.
