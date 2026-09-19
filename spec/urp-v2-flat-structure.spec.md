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
- **Provider protocol**: One of `responses`, `chat_completion`, `messages`, `gemini`, `openai_image`, or `replicate`.
- **Same-protocol hop**: An encode step whose target provider protocol exactly equals the source protocol recorded on an opaque provider item.

## 2. Canonical non-stream objects

URPV2-S1. Typed URP fields are the sole authority for represented semantics. Decoders MUST extract recognized semantics before preserving unknown fields. Internal metadata MUST NOT provide an alternative value for a represented semantic field.

URPV2-S2. Encoders MUST honor changes and deletions made by transforms. Native replay metadata MAY preserve unknown fields, field placement, and array boundaries. It MUST NOT restore deleted text, controls, identifiers, or payloads. A semantic change MUST invalidate incompatible replay shape.

URPV2-S3. `ReasoningConfig` MUST represent `effort`, `summary`, thinking mode, exact token budget, and thinking display as typed optional fields. Omission differs from an explicit disabled mode. Summary modes remain separate from response summary text. Unknown native configuration members MAY remain protocol-scoped metadata after recognized fields are removed.

URPV2-S4. `Reasoning` nodes and deltas MUST use typed presentation and transport metadata for redaction kind, replay eligibility, text presentation, and late item identity. Presentation metadata MUST reference the current typed text rather than contain another text copy. It MUST NOT be encoded as an unknown wire field.

URPV2-S5. Stream start usage and citation updates MUST use typed URP fields. Gemini signatures attached to ordinary text MUST have a typed association with that text. Runtime authentication context MUST remain separate from provider request extras.

URPV2-S6. The rules URPV2-S1 through URPV2-S5 take precedence over historical native-replay precedence requirements. They apply to non-stream, streamed, synthetic-stream, and assistant-history encoders.

URPV2-S7. Tool calls and tool results MUST expose an optional typed `namespace`. Calls MUST also expose an optional typed `signature`.
Responses `namespace` and Messages `toolset_name` map to this namespace. Gemini function-call signatures map to the typed signature.
Tool results MUST expose an optional typed `name` for protocols that identify the executed function by name.
Headers and temporary adapter forms MUST preserve these fields. Absence MUST prevent replay metadata from restoring a removed value.

URPV2-S8. Tool definitions MUST expose optional typed `namespace`, `tools`, `origin_protocol`, and `config` fields.
Namespace children reside only in `tools`. Provider-native built-in configuration resides in `config`, with its exact protocol in `origin_protocol`.
Encoders MUST emit provider-native configuration only for the recorded protocol. Function and custom definitions retain their existing typed semantics.
Provider-executed calls and results without shared execution semantics remain origin-scoped `ProviderItem` nodes. They MUST NOT become client function calls.

URPV2-S9. Image, audio, and file nodes MUST carry typed `MediaMetadata`, including optional `signature` and `media_type`.
Media headers and temporary adapter forms MUST preserve it. Gemini Part signatures and fileData MIME types map to these fields.
Chat generated audio data maps to an Audio node. Its reference identifier, transcript, and expiry reside in optional typed media metadata fields.
An audio reference without media data MAY remain an origin-scoped ProviderItem.

URPV2-S10. `NodeHeader::ProviderItem.body` MUST carry an optional initial native body. A decoder MUST NOT duplicate it in internal extras.
Shared stream accumulation MUST initialize the provider node from this body and apply later provider deltas to it.

URPV2-S11. Each special codec feature MUST have non-stream and streaming protocol-to-URP-to-protocol fixtures and URP-to-protocol-to-URP assertions.
Fixtures MUST assert canonical ownership, semantic values, terminal state, and protocol-scoped passthrough where applicable.
Request-only features MUST run with both stream values. Unsupported native features MUST have explicit omission or origin-scoped preservation assertions.
Mutation fixtures MUST prove that typed changes and deletions win over retained wire shape and unknown fields.

URPV2-S12. Affinity fingerprints MUST include typed namespaces, signatures, media metadata, citations, and reasoning metadata that change request semantics.
Provider tool filtering MUST retain a native tool only for its recorded origin protocol. Gemini allowed-function lists MUST survive filtering.
Native non-stream error envelopes MUST fail decoding. A valid Responses response with terminal error state follows PR2h in `unified_responses_proxy.spec.md`.
Native streaming errors MUST produce an Error event and MUST NOT produce successful terminal output.
Error replay metadata MUST NOT restore an obsolete typed error code or message.

URPV2-S13. Request runtime context MUST use typed fields for response-history state and tool transport mappings.
It MUST NOT serialize into URP JSON, provider requests, or JavaScript transform inputs.
JSON deserialization MUST NOT populate trusted context. A JavaScript transform MUST preserve the existing runtime context.
Authentication injection and removal MUST NOT replace history state or tool mappings.

URPV2-S14. A tool transport mapping MUST record its target protocol, wire call type, and original typed tool identity.
The original identity consists of namespace, name, and call type. The map key is the emitted wire tool name.
Mappings MUST apply only while a matching typed tool definition remains in the prepared request.
Deleting, renaming, changing the namespace, or changing the wire call type MUST invalidate that mapping.
Chat, Messages, and Gemini MAY flatten namespaces to collision-free names. Responses MUST retain native namespaces.
Messages custom-tool conversion MUST use the mapping's call types, without protocol-private fields in tool extras.
Restoration MUST preserve original names, namespaces, call IDs, and custom input bytes in non-stream and streaming paths.
Repeated preparation for the same target MUST preserve existing aliases and MUST NOT duplicate promoted tool identities.

URPV2-S15. Reasoning signature association MUST read typed node identity, typed metadata, or the current wire transport envelope.
It MUST NOT read `_monoize_reasoning_envelope_item_id` or restore a deleted signature from that legacy field.
Raw reasoning, summary, and encrypted values remain independent under these changes.

URPV2-S16. Native shape markers and unknown native fields MAY remain protocol-scoped adapter extras.
They MUST NOT acquire runtime authority or replace a typed semantic owner.
Temporary adapter parts are local conversion values, not an alternative canonical request or response.

URPV2-1. The canonical internal request object MUST be:

```text
UrpRequestV2 {
  model: String,
  input: Vec<Node>,
  instructions_format?: InstructionsFormat,
  context: RequestContext,
  stream?: bool,
  temperature?: number,
  top_p?: number,
  max_output_tokens?: integer,
  reasoning?: ReasoningConfig,
  logprobs?: LogprobConfig,
  tools?: Vec<ToolDefinition>,
  tool_choice?: ToolChoice,
  parallel_tool_calls?: bool,
  stop?: StopControl,
  verbosity?: String,
  response_format?: ResponseFormat,
  user?: String,
  ...extra_body
}
```

`context` contains trusted runtime identities, history state, and tool transport mappings. It MUST NOT serialize into canonical JSON.
`instructions_format` records native instruction placement without instruction content.

URPV2-2. The canonical internal response object MUST be:

```text
UrpResponseV2 {
  id: String,
  model: String,
  created_at?: integer,
  output: Vec<Node>,
  finish_reason?: FinishReason,
  outcome?: ResponseOutcome,
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

URPV2-8a. `UrpRequestV2.stop` MUST be a typed `StopControl` with exactly one of these shapes:

- `Single(String)` for a Chat Completions scalar `stop` value;
- `Multiple(Vec<String>)` for a Chat Completions array `stop` value or a Messages `stop_sequences` array.

A Chat request decoder MUST preserve whether the source used the scalar or array shape. A Messages request decoder MUST use `Multiple`. A Chat encoder MUST emit `Single` as a JSON string and `Multiple` as a JSON string array. A Messages encoder MUST emit `Single(s)` as `stop_sequences = [s]` and `Multiple(values)` as `stop_sequences = values`. A Responses encoder MUST omit `stop` because Responses create has no equivalent request control.

URPV2-8b. `UrpRequestV2.verbosity` is the typed OpenAI verbosity string. A Chat request decoder MUST read top-level `verbosity`. A Responses request decoder MUST read `text.verbosity`. Chat and Responses encoders MUST emit the corresponding native field. A Messages encoder MUST omit `verbosity` because Messages create has no equivalent request control.

URPV2-8c. `UrpRequestV2.user` owns the semantic caller identifier. Chat and Responses decoders MUST read top-level `user`; a Messages decoder MUST read `metadata.user_id`. Chat and Responses encoders MUST emit top-level `user`; a Messages encoder MUST emit `metadata.user_id`. A Messages decoder MUST preserve every non-`user_id` member of the source `metadata` object in `extra_body.metadata`. A Messages encoder MUST merge those preserved members into the emitted `metadata` object. If typed `user` and `extra_body.metadata.user_id` collide, typed `user` MUST win.

## 3. Canonical node model

URPV2-9. `Node` MUST be the discriminated union below.

```text
Node =
  | Text {
      type: "text",
      id?: String,
      role: OrdinaryRole,
      content: String,
      phase?: String,
      signature?: JsonValue,
      citations: Vec<Citation>,
      ...extra_body
    }
  | Image {
      type: "image",
      id?: String,
      role: OrdinaryRole,
      source: ImageSource,
      metadata: MediaMetadata,
      ...extra_body
    }
  | Audio {
      type: "audio",
      id?: String,
      role: OrdinaryRole,
      source: AudioSource,
      metadata: MediaMetadata,
      ...extra_body
    }
  | File {
      type: "file",
      id?: String,
      role: OrdinaryRole,
      source: FileSource,
      metadata: MediaMetadata,
      ...extra_body
    }
  | Refusal {
      type: "refusal",
      id?: String,
      content: String,
      ...extra_body
    }
  | Reasoning {
      type: "reasoning",
      id?: String,
      content?: String,
      summary?: String,
      encrypted?: JsonValue,
      source?: String,
      metadata: ReasoningMetadata,
      ...extra_body
    }
  | ToolCall {
      type: "tool_call",
      id?: String,
      tool_type: "function" | "custom",
      call_id: String,
      name: String,
      arguments: String,
      namespace?: String,
      signature?: JsonValue,
      ...extra_body
    }
  | ProviderItem {
      type: "provider_item",
      id?: String,
      origin_protocol: ProviderProtocol,
      role: OrdinaryRole,
      item_type: String,
      body: JsonValue,
      ...extra_body
    }
  | ToolResult {
      type: "tool_result",
      signature?: JsonValue,
      id?: String,
      tool_type: "function" | "custom",
      call_id: String,
      namespace?: String,
      name?: String,
      is_error: bool,
      content: Vec<ToolResultContent>,
      ...extra_body
    }
  | NextDownstreamEnvelopeExtra {
      type: "next_downstream_envelope_extra",
      ...extra_body
    }
```

Refusal, Reasoning, and ToolCall nodes have an implicit assistant role. These variants MUST NOT store a separate `role` field.

URPV2-10. `OrdinaryRole` MUST be one of `system`, `developer`, `user`, or `assistant`.

URPV2-11. `tool` is not an ordinary role in URP v2. Tool execution output is represented only by `ToolResult`.

URPV2-12. `ToolResultContent` MUST be the discriminated union below.

```text
ToolResultContent =
  | Text { type: "text", text: String, ...extra_body }
  | Image { type: "image", source: ImageSource, ...extra_body }
  | File { type: "file", source: FileSource, ...extra_body }
  | ProviderItem {
      type: "provider_item",
      origin_protocol: ProviderProtocol,
      item_type: String,
      body: JsonValue,
      ...extra_body
    }
```

URPV2-13. Sources MUST use the exact typed shapes below.

```text
ImageSource =
  | Url { type: "url", url: String, detail?: String }
  | Base64 { type: "base64", media_type: String, data: String }
  | FileId { type: "file_id", file_id: String, detail?: String }

AudioSource =
  | Url { type: "url", url: String }
  | Base64 { type: "base64", media_type: String, data: String }

FileSource =
  | Url { type: "url", url: String }
  | FileId { type: "file_id", file_id: String }
  | Text { type: "text", text: String }
  | Content { type: "content", content: Vec<JsonValue> }
  | Base64 {
      type: "base64",
      media_type: String,
      data: String
    }
```

URPV2-13a. A file-ID decoder MUST set `MediaMetadata.resource.protocol`. OpenAI file IDs use the Responses/Chat namespace; Anthropic file IDs use Messages.

URPV2-13b. File references MUST satisfy the protocol and resource-scope checks in media-transport.spec.md. Unsupported or ambiguous references MUST produce explicit errors. Encoders MUST NOT infer portability from prefixes.

URPV2-13c. File provenance MUST use typed MediaResource, not `_monoize_file_id_origin`. Cross-family stripping MUST preserve typed provenance. Native wire objects MUST NOT expose it.

URPV2-13d. MediaMetadata MUST own optional filename, detail, document_title, document_context, document_citations, and resource fields. ToolResultContent media MUST carry this metadata.
URPV2-13e. ToolResult nodes and headers MUST carry an optional typed signature. File transport MUST satisfy media-transport.spec.md.

### 3.1 Ordinary node invariants

ORD-1. Every ordinary node MUST carry `role` directly on the node. No ordinary node may be nested under a message wrapper.

ORD-2. `Reasoning`, `ToolCall`, and `Refusal` nodes MUST use `role = "assistant"`.

ORD-3. `Text`, `Image`, `Audio`, `File`, and `ProviderItem` nodes MAY use any `OrdinaryRole`.

ORD-4. `Text.phase` belongs only to `Text` nodes.

ORD-5. `Text.phase`, when present, MUST be treated as an unconstrained string. The decoder MUST preserve unknown values byte-for-byte. The encoder MUST NOT rewrite or drop a non-empty `phase` value solely because the value is not recognized locally.

ORD-6. A decoder MUST emit one ordinary node for each source-order semantic unit that the upstream protocol exposes. The decoder MUST NOT first merge several ordinary units into a logical message envelope.

ORD-7. Ordinary node `extra_body` stores unknown fields that belong to exactly that ordinary node's protocol object.

ORD-8. Typed fields are authoritative, including absence. An extra field or replay snapshot MUST NOT restore a removed typed value.

ORD-9. `ProviderItem.origin_protocol` MUST be one of the Provider protocol names from §1. It records the exact protocol that supplied `ProviderItem.body`.

ORD-10. `ProviderItem` is a same-protocol opaque native carrier. A decoder MUST use it only for one native item, block, or part that the source adapter cannot represent as `Text`, `Image`, `Audio`, `File`, `Refusal`, `Reasoning`, `ToolCall`, or `ToolResult`.

ORD-11. An encoder MUST replay `ProviderItem.body` only when `ProviderItem.origin_protocol` exactly equals the target provider protocol. If the protocols differ, the encoder MUST ignore that `ProviderItem`. It MUST NOT stringify the body, insert it into a prompt, wrap it as `Text`, or disguise it as another typed node.

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

TCL-2. Function ToolCall.arguments MUST contain JSON text. A decoder MUST serialize structured arguments to JSON text. Custom ToolCall.arguments MUST preserve freeform input byte-for-byte, including JSON-looking input. Numeric normalization MUST apply only to complete function arguments. A stateless stream helper MUST NOT rewrite argument fragments.

TCL-3. `ToolCall.tool_type` MUST be `function` for JSON-schema function calls and `custom` for freeform custom-tool calls. Missing `tool_type` in legacy internal data defaults to `function`.

TCL-4. `ToolResult.tool_type` MUST equal the correlated `ToolCall.tool_type`. A decoder that receives an explicitly typed Responses `custom_tool_call_output` MUST set `tool_type = "custom"`. A Chat tool-role result MUST inherit the type of the earlier call with the same `call_id`; if no correlated call is present, it defaults to `function`.

TR-1. `ToolResult` is a distinct top-level node. It MUST NOT carry `role`.

TR-2. `ToolResult.call_id` MUST be non-empty and MUST correlate to the originating `ToolCall.call_id` byte-for-byte.

TR-3. A terminal node sequence MUST NOT contain two distinct `ToolResult` nodes with the same `call_id`.

TR-4. `ToolResult.content` order is canonical. The encoder MUST preserve that order when rendering a protocol that supports multimodal tool results.

TR-5. `ToolResult.is_error` defaults to `false` when absent.

TR-6. `ToolResult.extra_body` stores unknown fields that belong to the protocol object representing that one tool result.

TR-7. If a key exists in both a `ToolResult` typed field and `ToolResult.extra_body`, the typed field value MUST win.

TR-8. `ToolResultContent.extra_body` stores unknown fields that belong to exactly that one content entry inside the tool result.

TR-9. If a key exists in both a `ToolResultContent` typed field and that entry's `extra_body`, the typed field value MUST win.

TR-10. A tool-result content block with no cross-family semantic mapping, including Anthropic `search_result` and `tool_reference`, MUST decode as `ToolResultContent::ProviderItem`. It MAY replay only when its `origin_protocol` equals the target protocol. Cross-family encoding MUST omit that entry rather than stringify its JSON body as user-visible text.

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

1. remove every non-internal member from `extra_body` on every ordinary node;
2. remove every non-internal member from `extra_body` on every `ToolResult` node;
3. remove every non-internal member from `extra_body` on every `ToolResultContent` entry; and
4. remove every `NextDownstreamEnvelopeExtra` control node.

A decoder-created or transform-created member reserved by XTRA-10 is internal semantic provenance, not nested wire passthrough. The runtime MUST retain such a member through XTRA-5 until the target adapter consumes or discards it. The target adapter MUST NOT emit the reserved member on the wire. This retained provenance includes `_monoize_chat_reasoning_detail`, which is required to apply MSG-6 to adjacent Chat reasoning-detail nodes after a Chat-to-Messages family transition.

XTRA-6. After XTRA-5, later transforms or adapters for the target family MAY add new target-family passthrough fields.

XTRA-7. On a same-family encode step, the runtime MUST preserve node `extra_body`, `ToolResultContent.extra_body`, and control nodes.

XTRA-8. The same-family passthrough rules in XTRA-4 through XTRA-7 do not authorize `ProviderItem` replay. `ProviderItem` replay is governed only by exact same-protocol equality under ORD-11.

XTRA-9. Before request-phase transforms run for one upstream attempt, the runtime MUST remove every downstream-origin `ProviderItem` whose `origin_protocol` differs from the selected upstream provider protocol. Later transforms MAY insert a `ProviderItem` only when they set `origin_protocol` to the intended target provider protocol.

XTRA-10. A key whose name starts with `_monoize_` is internal metadata, not wire passthrough. A wire decoder MUST treat that prefix as reserved and MUST NOT copy an incoming `_monoize_` member into an `extra_body`; only decoder or transform logic MAY create internal metadata after parsing semantic wire fields. An envelope reconstruction helper or protocol encoder MUST NOT emit such a key as a provider request field. A same-family encoder MAY consume an internal key to reconstruct the native field represented by its value, then MUST discard the internal key. For Chat `reasoning_details`, `_monoize_chat_reasoning_detail` MUST contain only shape and unknown entry metadata. It MUST NOT contain text, summary, encrypted data, id, or format copies. The encoder MUST derive these values from the current typed node. An opaque same-protocol `ProviderItem.body` or `ProviderControl.data` MUST be cloned at its wire boundary. The clone MUST recursively remove object members whose keys start with `_monoize_`, including members below arrays, while preserving every other member. This sanitization MUST NOT mutate the canonical URP body or control data and MUST NOT apply to arbitrary typed or user payloads.

## 5. Canonical flat streaming events

STR-1. The canonical URP v2 streaming representation MUST be:

```text
UrpStreamEventV2 =
  | ResponseStart {
      id: String,
      model: String,
      usage?: Usage,
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
      outcome?: ResponseOutcome,
      usage?: Usage,
      output: Vec<Node>,
      ...extra_body
    }
  | ProviderControl {
      protocol: String,
      event_name: String,
      data: JsonValue,
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
  | Text {
      id?: String, role: OrdinaryRole, phase?: String,
      signature?: JsonValue, citations: Vec<Citation>
    }
  | Image { id?: String, role: OrdinaryRole, metadata: MediaMetadata }
  | Audio { id?: String, role: OrdinaryRole, metadata: MediaMetadata }
  | File { id?: String, role: OrdinaryRole, metadata: MediaMetadata }
  | Refusal { id?: String }
  | Reasoning { id?: String, metadata: ReasoningMetadata }
  | ToolCall {
      id?: String, tool_type: "function" | "custom", call_id: String,
      name: String, namespace?: String, signature?: JsonValue
    }
  | ProviderItem {
      id?: String, origin_protocol: ProviderProtocol, role: OrdinaryRole,
      item_type: String, body?: JsonValue
    }
  | ToolResult {
      id?: String, tool_type: "function" | "custom", call_id: String,
      namespace?: String, name?: String, signature?: JsonValue
    }
  | NextDownstreamEnvelopeExtra
```

STR-3. `NodeDelta` MUST be the discriminated union below.

```text
NodeDelta =
  | Text { content: String, signature?: JsonValue, citations: Vec<Citation> }
  | Reasoning {
      metadata: ReasoningMetadata,
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

STR-12. `ProviderControl` is same-protocol stream passthrough for one valid provider event that has no typed URP event mapping. It MUST NOT create a node, consume a `node_index`, mutate `ResponseDone.output`, or cross a protocol-family boundary.

STR-13. A same-protocol stream encoder MAY replay `ProviderControl.data` only when `ProviderControl.protocol` equals the target protocol and `event_name` identifies the source wire event. Before replay, the encoder MUST clone `ProviderControl.data` and recursively remove every object member whose key starts with `_monoize_`, including members below arrays. The encoder MUST preserve all other members and MUST NOT mutate canonical `ProviderControl.data`. A mismatched encoder MUST drop the event without converting it to text or success state.

### 5.1 Delta accumulation invariants

SACC-1. For `NodeDelta::Text`, terminal `Text.content` is the ordered concatenation of all `content` fragments for that `node_index`.

SACC-2. For `NodeDelta::ToolCallArguments`, terminal `ToolCall.arguments` is the ordered concatenation of all `arguments` fragments for that `node_index`.

SACC-3. For `NodeDelta::Reasoning.content`, terminal `Reasoning.content` is the ordered concatenation of all non-null `content` fragments for that `node_index`.

SACC-4. For `NodeDelta::Reasoning.summary`, terminal `Reasoning.summary` is the ordered concatenation of all non-null `summary` fragments for that `node_index`.

SACC-4a. When a source protocol emits a later non-empty full-field done snapshot for `Reasoning.summary`, that snapshot replaces the delta concatenation for `NodeDone.node` and `ResponseDone.output`. A full-item done snapshot MAY also replace other non-empty reasoning fields. A non-empty terminal response-object summary is the final authoritative `ResponseDone.output` presentation and replaces any different accumulated summary without creating a terminal conflict. An empty done-snapshot or terminal-response field MUST NOT erase a non-empty accumulated field.

SACC-5. If a source protocol defines streamed `Reasoning.encrypted` values as fragments and each fragment is a string, terminal `Reasoning.encrypted` is the ordered concatenation of those raw string fragments. Anthropic Messages `signature_delta.signature` and the legacy Chat scalar `reasoning_opaque` are fragment surfaces. A Chat `reasoning_details[]` entry is one ordered reasoning-sequence element and is not a fragment of another entry.

SACC-5b. When reasoning envelopes are enabled, Monoize MUST NOT wrap each encrypted fragment independently. It MUST accumulate the raw fragments for one `node_index`, select the non-empty `NodeDone.node.encrypted` value as the authoritative complete value when present, and otherwise use the SACC-5 concatenation. Monoize MUST wrap that complete value exactly once before a response transform or downstream encoder observes it. A downstream encoder MAY split the one wrapped string only to satisfy the configured SSE frame limit. Concatenating all downstream string frames for that encrypted field MUST produce exactly one parseable `mz2.` envelope.

SACC-5c. A full encrypted terminal snapshot is one opaque value. If it differs from prior encrypted fragments or prior full snapshots, the terminal accumulator MUST replace its encrypted field atomically. It MUST NOT derive a prefix or suffix from the opaque value. This rule does not change the text and summary suffix rules in SACC-4a.

SACC-5a. A Responses `response.output_item.added.item.encrypted_content` value is one complete opaque lifecycle snapshot. It is not an incremental encrypted delta. A Responses stream decoder MUST preserve the complete added-event value in item-level `NextDownstreamEnvelopeExtra` state. It MUST NOT copy that value into `NodeDelta`, `NodeDone.node`, or the terminal reasoning accumulator. A same-Responses stream encoder MUST emit that preserved value in the matching downstream `response.output_item.added.item.encrypted_content` field. A later non-empty value from `response.output_item.done.item` is a separate complete snapshot. The decoder MUST store the done-event value as one atomic terminal value. It MUST NOT concatenate, splice, or partially overwrite encrypted lifecycle snapshots. A non-empty `response.completed.response.output[].encrypted_content` value is the final authoritative terminal `Reasoning.encrypted` snapshot. It MUST replace a different done-event snapshot without creating a terminal conflict. If the completed value is absent or empty, a non-empty done-event snapshot remains.

SACC-6. If a provider supplies a non-string `Reasoning.encrypted` payload, the decoder MUST emit that value only in `NodeDone.node` and `ResponseDone.output`, or in exactly one `NodeDelta`. The decoder MUST NOT split a non-string JSON value across several deltas.

SACC-7. For `NodeDelta::Reasoning.source`, the terminal `Reasoning.source` value is the most recent non-empty `source` value seen for that `node_index`.

SACC-8. An empty-string `Reasoning.source` delta MUST be ignored.

SACC-9. `NodeHeader::ProviderItem` MUST carry `origin_protocol`. A downstream stream encoder MUST open and complete a ProviderItem lifecycle only when that origin protocol equals the downstream provider protocol. A mismatched ProviderItem stream node MUST be skipped without producing text or another typed node.

## 6. Encoder-owned logical envelope reconstruction

RECON-1. URP v2 canonical storage is only the flat node sequence. Message grouping, output-item grouping, and block grouping are encoder responsibilities.

RECON-2. An encoder MAY place consecutive ordinary nodes into one consumable envelope only when all conditions below hold:

1. every grouped value is an ordinary node;
2. no `ToolResult` node lies inside the grouped run;
3. no `NextDownstreamEnvelopeExtra` node lies inside the grouped run;
4. grouping preserves original node order exactly; and
5. grouping does not violate a protocol-specific boundary rule in this section.

RECON-3. A `ToolResult` node is always its own top-level semantic unit. An encoder MUST NOT merge a `ToolResult` node into an ordinary-node envelope.

RECON-4. A `ProviderItem` node is never eligible for generic message-compatible grouping. A family-specific encoder MAY replay it as a native item, block, or part only under ORD-11.

RECON-5. A control node is not itself emitted downstream. Its only effect is to modify the next consumable envelope under CTL-5 through CTL-8.

### 6.1 Responses stability and reconstruction

RESP-1. The Responses encoder MUST reconstruct canonical Responses output items from flat nodes.

RESP-2. Each `Reasoning` node MUST encode as one top-level Responses `reasoning` item.

RESP-3. Each `ToolCall(tool_type = "function")` node MUST encode as one top-level Responses `function_call` item. Each `ToolCall(tool_type = "custom")` node MUST encode as one top-level Responses `custom_tool_call` item with freeform `input`.

RESP-3a. `ToolResult(tool_type = "function")` MUST encode as `function_call_output`. `ToolResult(tool_type = "custom")` MUST encode as `custom_tool_call_output`.

RESP-3b. Mixed ToolResult content MUST encode as an ordered array of input_text, input_image, and input_file blocks. It MUST NOT become JSON text or a later user message.

RESP-4. Each maximal run of adjacent ordinary nodes that are not `Reasoning` and not `ToolCall`, and that share the same `role`, MAY encode as one Responses `message` item.

RESP-5. A change in `Text.phase` value inside a Responses `message` run MUST force a new Responses `message` item boundary.

RESP-6. `NextDownstreamEnvelopeExtra` applies to the next Responses output item envelope created under RESP-2 through RESP-4.

RESP-7. Responses streaming output MUST preserve canonical event lifecycle semantics:

1. `response.created` occurs before any output-item lifecycle event;
2. every emitted `response.output_item.done` has exactly one earlier matching `response.output_item.added` with the same `output_index`;
3. `response.content_part.added` and `response.content_part.done` are emitted for each Responses content-bearing item part, including `message` content and `reasoning.content[]` entries of type `reasoning_text`;
4. `response.completed` reflects the same reconstructed `response.output` ordering used by the terminal item lifecycle; and
5. the stream terminates with exactly one plain `data: [DONE]` sentinel.

RESP-8. The Responses encoder MUST preserve externally visible addressing and lifecycle semantics for `response_id`, `item_id`, `output_index`, `content_index`, and item `status`. These fields are encoder-owned output coordinates derived from the flat node order.

RESP-9. Responses external reasoning behavior remains stable:

1. summary text remains distinct from full reasoning text;
2. full reasoning text encodes in `reasoning.content[]` entries with `type = "reasoning_text"`, not in a top-level reasoning-item `text` field;
3. opaque reasoning payload remains typed reasoning data rather than plain text;
4. `source` is preserved when present upstream; and
5. `source` is omitted when upstream omitted it.

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

MSG-6. `Reasoning` nodes MUST reconstruct Anthropic `thinking` blocks. If adjacent Chat reasoning-detail nodes contain non-empty plaintext followed by an encrypted-only payload, a Messages encoder MUST render those two semantic surfaces in one thinking block while preserving the two URP nodes for same-Chat replay. If the stream exposes both thinking text and signature state, `thinking_delta` MUST occur before `signature_delta`, and both MUST occur before that block's `content_block_stop`.

MSG-7. `ToolCall(tool_type = "function")` nodes MUST reconstruct Anthropic `tool_use` blocks. Streamed tool input JSON remains block-scoped and index-scoped. Messages has no specified freeform custom-call lifecycle; its encoder MUST omit `ToolCall(tool_type = "custom")` and `ToolResult(tool_type = "custom")` rather than reinterpret freeform input as JSON tool input.

MSG-8. `ToolResult` nodes MUST reconstruct Anthropic `tool_result` blocks as distinct tool-result protocol objects. They MUST NOT be rewritten as ordinary role-bearing nodes.

MSG-8a. Consecutive `ToolResult` nodes MUST reconstruct as consecutive `tool_result` blocks inside one Anthropic user message envelope. A Messages encoder MUST NOT emit an empty text block solely to preserve an empty `Text.content` value.

### 6.3 OpenRouter-compatible Chat stability and reconstruction

CHAT-1. The Chat Completions encoder MUST preserve OpenRouter-compatible chat behavior. The flat redesign MUST NOT reduce the downstream contract to plain OpenAI Chat Completions.

CHAT-2. In non-stream chat responses, `choices[0].message.content` MUST remain a JSON string. If several text nodes are merged into one downstream assistant message, the encoder MUST concatenate them in source order with `"\n\n"` between adjacent text segments.

CHAT-3. Structured reasoning in chat responses MUST remain in `reasoning_details`. Plaintext assistant reasoning, when emitted as a simple scalar field, remains `message.reasoning` for non-stream and `delta.reasoning` only when the downstream protocol already defines that exact field for plain text.

CHAT-4. Chat `reasoning_details[]` entries MUST preserve the OpenRouter-compatible discriminated union:

1. `{ "type": "reasoning.summary", "summary": ... }`
2. `{ "type": "reasoning.text", "text": ..., "signature"?: ... }`
3. `{ "type": "reasoning.encrypted", "data": ... }`
4. `{ "type": "reasoning.server_tool_call", "tool_name": ..., "arguments": ..., "result": ..., "tool_call_id"?: ... }`

CHAT-4a. Every `reasoning_details[]` entry MAY carry `id`, `format`, and `index`. It MAY also carry future entry-local fields. A decoder MUST preserve those fields on the owning reasoning node, and a same-Chat encoder MUST replay them on the same entry.

CHAT-4b. Source detail order is canonical. A decoder MUST create one reasoning node per detail entry. An encoder MUST preserve repeated detail types and MUST NOT merge, reorder, or deduplicate entries. Scalar `reasoning` is a compatibility view and MUST NOT duplicate matching content or summary details.
Scalar `reasoning_content` is raw content. It MUST deduplicate only matching content, never a matching summary.
If summary and raw content contain identical bytes, both typed meanings MUST remain present.
A simultaneous `reasoning` field MUST NOT suppress a distinct `reasoning_content` value.

CHAT-4c. Without an explicit response transform, non-empty `Reasoning.content` MUST encode as `reasoning.text` and MUST NOT encode as `reasoning.summary`.

CHAT-5. Opaque encrypted reasoning payloads MUST appear only in `reasoning_details[]` entries with `type = "reasoning.encrypted"` and field `data`.

CHAT-6. Streaming chat output remains data-only SSE and terminates with exactly one `[DONE]` sentinel.

CHAT-7. If streamed chat output emits tool-call deltas, terminal `finish_reason` semantics for that downstream stream remain `tool_calls`.

CHAT-7a. A Chat encoder MUST emit `ToolCall(tool_type = "function")` as `{type:"function",function:{name,arguments}}` and `ToolCall(tool_type = "custom")` as `{type:"custom",custom:{name,input}}`. A Chat decoder MUST accept both shapes in request history, non-stream output, and stream deltas. Chat tool-role results inherit the correlated call type so a later Responses encoder can choose `function_call_output` versus `custom_tool_call_output`.

CHAT-7b. Chat tool and legacy function content MUST decode compatible text, image, file, and audio blocks under DC4c and MT41-MT46. Decoder capability MUST NOT depend on the selected target.

CHAT-8. If cumulative usage is available when a successful Chat Completions stream terminates, the encoder MUST emit exactly one usage chunk after the empty-delta finish chunk and immediately before `[DONE]`. The usage chunk MUST use the same `id`, `object`, `created`, and `model` envelope values as the finish chunk, MUST set `choices` to an empty array, and MUST contain the cumulative `usage` object. The finish chunk MUST NOT contain a non-null `usage` object. If cumulative usage is unavailable, the encoder MUST omit the usage chunk.

CHAT-9. SSE comment lines and post-start chunk-shaped error payloads remain representable downstream. The flat URP redesign MUST NOT remove support for those externally visible chat stream forms.

## 7. Validity summary

VALID-1. A valid terminal URP v2 sequence is an ordered `Vec<Node>` with no `Message { role, parts }` wrapper.

VALID-2. A valid terminal sequence MUST NOT end with `NextDownstreamEnvelopeExtra`.

VALID-3. `ToolResult` remains a distinct top-level node variant and MUST NOT be reclassified as an ordinary role-bearing node.

VALID-4. Terminal stream state is authoritative. `ResponseDone.output` is the final flat node sequence.

VALID-5. Decoder complexity is minimized by emitting flat nodes only. Encoder complexity owns all logical envelope reconstruction.

## Typed control mapping

CTRL-1. Request reasoning controls MUST use `ReasoningConfig` fields: `effort`, `summary`, `mode`, `budget_tokens`, and `display`.
CTRL-2. `mode` represents enabled, disabled, or adaptive execution. `budget_tokens` is an unsigned integer. Decoders MUST NOT derive effort from an explicit budget.
CTRL-3. Disabled mode or effort `none` disables thinking. Otherwise an explicit budget takes precedence over an effort-to-budget fallback.
CTRL-4. Messages emits a budget only for enabled thinking. Adaptive thinking uses effort when present.
CTRL-5. Gemini maps thinkingLevel to effort, nonnegative thinkingBudget to budget_tokens, and budget -1 to adaptive mode.
CTRL-6. Gemini includeThoughts maps to summary auto or none. Non-none summary modes request includeThoughts on output.
CTRL-7. Removing a control removes that control only. Other explicit typed controls remain active. An empty reasoning configuration MUST NOT enable thinking.
CTRL-8. Text citations and thought signatures MUST survive unchanged node conversion. A signature MUST stay associated with its original text node.
CTRL-9. Responses source snapshots MUST exclude typed id, model, output, usage, status, and terminal details. Start events use typed usage.
CTRL-10. Session affinity MUST derive instruction content from current instruction nodes. Historical instruction text MUST NOT affect the affinity key.


## 9. Canonical annotations, probabilities, outcomes, and usage components

SEM-1. Text citations MUST use typed citations. URL citations contain a URL, an optional title, and an optional answer range.
Answer ranges use zero-based Unicode scalar offsets with an exclusive end, relative to the owning Text node.
Document citations MUST distinguish source character, page, and block ranges from answer ranges.
Unknown citation shapes MUST retain an exact origin protocol and MAY replay only to that protocol.
Encoders MUST omit a citation when its target protocol cannot represent its source or required range.
Chat message concatenation MUST shift answer ranges by the preceding text and inserted separators.

SEM-2. Text and Refusal nodes and deltas MAY contain typed token logprobs.
Each entry contains token text, optional bytes, a log probability, and ordered alternative tokens.
Request logprob controls MUST have one typed owner for enabled state and alternative count.
Decoders MUST remove recognized logprob fields from passthrough.
Encoders MUST omit token scores when their concatenated bytes do not match the current owning text.
Stream token scores MUST follow their text delta and MUST accumulate into terminal node scores in order.
A protocol without token score support MUST omit scores without changing text.

SEM-3. UrpResponse and ResponseDone MUST carry an optional typed outcome, separate from finish_reason.
Outcome status distinguishes completed, incomplete, failed, cancelled, queued, and in_progress.
An explicit outcome is authoritative. Encoders derive an outcome from finish_reason only when outcome is absent.
Outcome owns structured error details and an optional incomplete reason. Native extras MUST NOT restore absent outcome fields.
Finish reasons additionally distinguish context limits, paused server work, and compaction from unknown termination.
Messages context limits MUST map to incomplete output. A paused or compaction-only response MUST NOT become successful completed output across protocols.
Targets without pause semantics MUST emit incomplete or truncated output rather than claim a completed turn.
A generation failure MAY contain partial output and usage. It MUST use a terminal outcome, not discard its snapshot as a transport error.
Transport errors and malformed protocol events MUST remain Error events.

SEM-4. Usage MAY contain ordered typed iterations. Each iteration records its kind and normalized inclusive token counters.
Usage top-level counters retain their existing primary-generation meaning. Complete accounting MUST sum iterations when present, without adding top-level counters again.
Each iteration normalizes cache buckets by the same rules as ordinary Usage.
Messages wire encoders MUST reconstruct disjoint cache counters per iteration and preserve the primary-generation top-level counters.
Other protocols MUST receive aggregate iteration totals where they have no iteration representation.
Cumulative stream snapshots MUST replace iterations; they MUST NOT append repeated snapshots.

SEM-5. Messages compaction_delta.content MUST replace the active compaction ProviderItem content.
NodeDone and ResponseDone MUST retain that complete value, including stream-to-nonstream conversion.

SEM-6. Reasoning content, summary, and encrypted remain independent typed fields. This change MUST NOT merge these meanings.
