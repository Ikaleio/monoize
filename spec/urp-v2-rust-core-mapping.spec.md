# URP v2 Rust Core Mapping Specification

## 0. Status

- Version: `1.0.0`
- Product name: Monoize
- Internal protocol name: `URP v2`
- Scope: Rust core type mapping, helper invariants, stream-helper invariants, and encoder grouping rules for the flat URP v2 redesign.

## 1. Purpose and precedence

RUSTMAP-1. This file defines the Rust-facing target contract for the flat URP v2 redesign in these implementation surfaces:

1. `src/urp/mod.rs`
2. `src/urp/encode/*`
3. `src/urp/decode/*`
4. `src/urp/greedy.rs`
5. `src/urp/stream_helpers.rs`
6. `src/urp/stream_encode/*`
7. `src/urp/stream_decode/*`

RUSTMAP-2. `spec/urp-v2-flat-structure.spec.md` remains authoritative for canonical URP v2 structure.

RUSTMAP-3. `spec/urp-transform-system.spec.md` remains authoritative for transform-visible behavior.

RUSTMAP-4. This file is authoritative only for the Rust core type layer and for helper-level invariants that the structure and transform specs intentionally leave implementation-local.

RUSTMAP-5. If this file disagrees with `spec/urp-v2-flat-structure.spec.md` about canonical node meaning, stream-event meaning, passthrough ownership, or `ResponseDone.output` authority, `spec/urp-v2-flat-structure.spec.md` wins.

RUSTMAP-6. The Rust core layer MUST expose exactly one canonical conversational representation. The implementation MUST NOT keep a second canonical representation based on grouped `Message { role, parts }` storage.

## 2. Rust core type surface target

### 2.1 Canonical Rust type family

RTYPE-1. The canonical Rust request type in `src/urp/mod.rs` MUST represent the URP v2 request shape `UrpRequestV2 { model, input, ... }` from `spec/urp-v2-flat-structure.spec.md`.

RTYPE-2. The canonical Rust response type in `src/urp/mod.rs` MUST represent the URP v2 response shape `UrpResponseV2 { id, model, output, ... }` from `spec/urp-v2-flat-structure.spec.md`.

RTYPE-3. The Rust item names MAY remain unsuffixed `UrpRequest` and `UrpResponse`. A `V2` suffix is not required in Rust item names. The required property is canonical shape, not suffix spelling.

RTYPE-4. If the Rust item names remain unsuffixed, they MUST still use these exact field names:

```text
UrpRequest {
  model: String,
  input: Vec<Node>,
  stream: Option<bool>,
  temperature: Option<f64>,
  top_p: Option<f64>,
  max_output_tokens: Option<u64>,
  reasoning: Option<ReasoningConfig>,
  tools: Option<Vec<ToolDefinition>>,
  tool_choice: Option<ToolChoice>,
  response_format: Option<ResponseFormat>,
  user: Option<String>,
  extra_body: HashMap<String, JsonValue>
}

UrpResponse {
  id: String,
  model: String,
  output: Vec<Node>,
  finish_reason: Option<FinishReason>,
  usage: Option<Usage>,
  extra_body: HashMap<String, JsonValue>
}
```

RTYPE-5. The Rust core module MUST define exactly one canonical top-level conversational enum:

```text
Node =
  | Text {
      role: OrdinaryRole,
      content: String,
      phase: Option<String>,
      extra_body: HashMap<String, JsonValue>
    }
  | Image {
      role: OrdinaryRole,
      source: ImageSource,
      extra_body: HashMap<String, JsonValue>
    }
  | Audio {
      role: OrdinaryRole,
      source: AudioSource,
      extra_body: HashMap<String, JsonValue>
    }
  | File {
      role: OrdinaryRole,
      source: FileSource,
      extra_body: HashMap<String, JsonValue>
    }
  | Refusal {
      role: OrdinaryRole::Assistant,
      content: String,
      extra_body: HashMap<String, JsonValue>
    }
  | Reasoning {
      role: OrdinaryRole::Assistant,
      content: Option<String>,
      summary: Option<String>,
      encrypted: Option<JsonValue>,
      source: Option<String>,
      extra_body: HashMap<String, JsonValue>
    }
  | ToolCall {
      role: OrdinaryRole::Assistant,
      call_id: String,
      name: String,
      arguments: String,
      extra_body: HashMap<String, JsonValue>
    }
  | ProviderItem {
      role: OrdinaryRole,
      item_type: String,
      body: JsonValue,
      extra_body: HashMap<String, JsonValue>
    }
  | ToolResult {
      call_id: String,
      is_error: bool,
      content: Vec<ToolResultContent>,
      extra_body: HashMap<String, JsonValue>
    }
  | NextDownstreamEnvelopeExtra {
      extra_body: HashMap<String, JsonValue>
    }
```

RTYPE-6. `OrdinaryRole` in the Rust core layer MUST contain exactly `System`, `Developer`, `User`, and `Assistant`.

RTYPE-7. The Rust core role enum MUST NOT contain `Tool`.

RTYPE-8. The Rust core layer MUST define exactly one canonical nested tool-result-content enum:

```text
ToolResultContent =
  | Text {
      text: String,
      extra_body: HashMap<String, JsonValue>
    }
  | Image {
      source: ImageSource,
      extra_body: HashMap<String, JsonValue>
    }
  | File {
      source: FileSource,
      extra_body: HashMap<String, JsonValue>
    }
```

RTYPE-9. The Rust core layer MUST define exactly one canonical streaming enum family:

```text
UrpStreamEvent =
  | ResponseStart { id: String, model: String, extra_body: HashMap<String, JsonValue> }
  | NodeStart { node_index: u32, header: NodeHeader, extra_body: HashMap<String, JsonValue> }
  | NodeDelta { node_index: u32, delta: NodeDelta, usage: Option<Usage>, extra_body: HashMap<String, JsonValue> }
  | NodeDone { node_index: u32, node: Node, usage: Option<Usage>, extra_body: HashMap<String, JsonValue> }
  | ResponseDone { finish_reason: Option<FinishReason>, usage: Option<Usage>, output: Vec<Node>, extra_body: HashMap<String, JsonValue> }
  | Error { code: Option<String>, message: String, extra_body: HashMap<String, JsonValue> }
```

RTYPE-10. `NodeHeader` and `NodeDelta` MUST mirror the canonical flat event contract in `spec/urp-v2-flat-structure.spec.md`. The Rust core layer MUST NOT retain `ItemHeader`, `PartHeader`, or `PartDelta` as canonical stream types.

RTYPE-11. The Rust core layer MUST NOT retain `Item`, `Part`, `ItemHeader`, `PartHeader`, `PartDelta`, `UrpRequest.inputs`, `UrpResponse.outputs`, `UrpStreamEvent::ItemStart`, `UrpStreamEvent::PartStart`, `UrpStreamEvent::PartDone`, or `UrpStreamEvent::ItemDone` as canonical URP representations.

### 2.2 Exact current-to-target concept mapping

MAP-1. The current grouped request field `UrpRequest.inputs: Vec<Item>` maps to `UrpRequest.input: Vec<Node>`.

MAP-2. The current grouped response field `UrpResponse.outputs: Vec<Item>` maps to `UrpResponse.output: Vec<Node>`.

MAP-3. The current grouped variant `Item::Message { role, parts, extra_body }` does not survive in canonical storage. It maps to zero or more flat `Node` values in source order.

MAP-4. The current grouped variant `Item::ToolResult { call_id, is_error, content, extra_body }` maps one-to-one to flat `Node::ToolResult { call_id, is_error, content, extra_body }`.

MAP-5. The current part variants map to flat node variants as follows:

| Current grouped concept | Flat canonical target |
| --- | --- |
| `Part::Text` | `Node::Text` with copied `role` |
| `Part::Image` | `Node::Image` with copied `role` |
| `Part::Audio` | `Node::Audio` with copied `role` |
| `Part::File` | `Node::File` with copied `role` |
| `Part::Refusal` | `Node::Refusal` with `role = OrdinaryRole::Assistant` |
| `Part::Reasoning` | `Node::Reasoning` with `role = OrdinaryRole::Assistant` |
| `Part::ToolCall` | `Node::ToolCall` with `role = OrdinaryRole::Assistant` |
| `Part::ProviderItem` | `Node::ProviderItem` with copied `role` |

MAP-6. The current grouped role `Role::Tool` has no flat canonical target. Any prior semantic use of `Role::Tool` MUST be represented as top-level `Node::ToolResult` instead.

MAP-7. Unknown fields on the current grouped `Message` envelope do not map to a surviving canonical message wrapper. They MUST map either:

1. to the owning emitted flat node's `extra_body`, when the field belongs to exactly one emitted node; or
2. to a preceding `Node::NextDownstreamEnvelopeExtra`, when the field belongs to the envelope rather than to exactly one emitted node.

MAP-8. The current stream lifecycle `{ ItemStart, PartStart, Delta, PartDone, ItemDone, ResponseDone.outputs }` maps to flat node lifecycle `{ NodeStart, NodeDelta, NodeDone, ResponseDone.output }`.

MAP-9. `ResponseDone.output` is the only authoritative terminal flat stream state. There is no Rust helper contract in which `ResponseDone` authority depends on first rebuilding grouped `Item::Message` values.

## 3. Node-family helper invariants

### 3.1 Ordinary role-bearing nodes

NH-1. An ordinary node is one of `Text`, `Image`, `Audio`, `File`, `Refusal`, `Reasoning`, `ToolCall`, or `ProviderItem`.

NH-2. Every ordinary node MUST carry `role` on the node itself.

NH-3. No helper in `src/urp/encode/*`, `src/urp/decode/*`, or `src/urp/greedy.rs` may require a canonical parent message wrapper in order to determine an ordinary node's role, ordering, phase, reasoning fields, tool-call fields, or passthrough fields.

NH-4. Ordinary-node helper code MUST treat source order in `Vec<Node>` as canonical. It MUST NOT infer a hidden grouped-message boundary solely because adjacent nodes have the same role.

NH-5. `Refusal`, `Reasoning`, and `ToolCall` MUST always behave as assistant-role ordinary nodes. Helpers MUST NOT accept any other role for those variants.

NH-6. `Text.phase` is node-local metadata. A helper MAY read it only from `Node::Text` and MUST NOT synthesize a message-level phase cache that becomes canonical state.

### 3.2 Top-level `ToolResult`

NH-7. `ToolResult` is a top-level node family, not an ordinary role-bearing node family.

NH-8. Shared helper code for ordinary-role grouping, role rewriting, and ordinary-node merging MUST treat `ToolResult` as outside that behavior.

NH-9. A helper MUST never require or synthesize `role = tool` in order to process a `ToolResult` node.

NH-10. A helper that correlates a tool result to a tool call MUST use `call_id` only. It MUST NOT depend on grouped adjacency inside a synthetic message wrapper.

NH-11. `ToolResult.content` is canonical ordered typed content. Shared helper code MUST preserve that content order byte-for-byte.

### 3.3 `next_downstream_envelope_extra`

NH-12. `NextDownstreamEnvelopeExtra` is the only control node family.

NH-13. Shared helper code MUST treat `NextDownstreamEnvelopeExtra` as a boundary marker plus one-use envelope-local unknown-field map.

NH-14. `NextDownstreamEnvelopeExtra` is not user-visible content, is not an ordinary node, and is not a `ToolResult`.

NH-15. When a helper scan encounters `NextDownstreamEnvelopeExtra`, it MUST flush any currently buffered consumable envelope run before storing the control node for later application.

NH-16. Stored control-node state applies only to the next downstream envelope opened after the control node.

NH-17. Consecutive `NextDownstreamEnvelopeExtra` nodes before the same downstream envelope MUST merge in source order. Later keys win.

NH-18. If end-of-sequence or end-of-stream occurs while control-node state remains unmatched, helpers MUST discard that unmatched control-node state without emitting an empty envelope and without emitting an error solely because the control node was unmatched.

### 3.4 Passthrough ownership boundaries

NH-19. Top-level request and response `extra_body` own only top-level unknown fields.

NH-20. Ordinary-node `extra_body` owns only unknown fields that belong to exactly that one flat node.

NH-21. `ToolResult.extra_body` owns only unknown fields that belong to exactly that one top-level tool-result object.

NH-22. `ToolResultContent.extra_body` owns only unknown fields that belong to exactly that one nested tool-result content entry.

NH-23. `NextDownstreamEnvelopeExtra.extra_body` owns only unknown fields that belong to the next downstream envelope as a whole rather than to exactly one emitted flat node.

NH-24. Shared decode helpers MUST assign unknown fields to exactly one ownership layer from NH-19 through NH-23. The same unknown field MUST NOT be duplicated across ownership layers.

NH-25. Shared encode helpers MUST consume unknown fields from the ownership layer that matches the target wire object being emitted. A helper MUST NOT read node-local passthrough from `NextDownstreamEnvelopeExtra`, and MUST NOT read envelope-local passthrough from an ordinary node or `ToolResult`.

NH-26. The nested-passthrough stripping helper that replaces current grouped `strip_nested_extra_body` behavior MUST operate over `Vec<Node>` and MUST do exactly these actions on the target node sequence:

1. clear `extra_body` on each ordinary node;
2. clear `extra_body` on each `ToolResult` node;
3. clear `extra_body` on each `ToolResultContent` entry; and
4. remove each `NextDownstreamEnvelopeExtra` node.

NH-27. The stripping helper in NH-26 MUST NOT remove or mutate top-level request or response `extra_body`.

## 4. Deterministic encoder grouping invariants

### 4.1 General grouping contract

EGR-1. A grouping helper MUST operate over flat `Vec<Node>` input only.

EGR-2. A grouping helper MUST scan left to right and produce a deterministic sequence of downstream consumable envelopes.

EGR-3. A grouping helper MUST NOT define groupability as "same role" alone.

EGR-4. A maximal consumable envelope run is the longest consecutive node subsequence beginning at position `i` for which all conditions below hold:

1. every member of the run is eligible for the same target-family envelope kind;
2. no member of the run is `ToolResult`;
3. no member of the run is `NextDownstreamEnvelopeExtra`;
4. target-family boundary predicates do not require a flush between any adjacent pair in the run;
5. emitting the run as one downstream envelope preserves flat node order exactly; and
6. emitting the run as one downstream envelope does not collapse a node kind that the target protocol exposes as a distinct top-level semantic unit.

EGR-5. Any node kind that a target family exposes as a distinct top-level semantic unit MUST force its own top-level envelope in the grouping helper for that family.

EGR-6. At minimum, `ToolResult` always forces its own top-level envelope in every protocol family.

EGR-7. A control node always forces a flush before later envelope creation. The control node itself does not create an envelope.

EGR-8. Grouping helpers MUST treat node-kind classification and protocol-family classification as first-class inputs. They MUST NOT infer protocol-correct grouping from one generic `role + phase zone` heuristic alone.

### 4.2 Generic node classes for shared helper code

EGR-9. Shared encoder helper code MAY use these generic helper classes, or an equivalent classification with the same semantics:

1. `MessageCompatibleOrdinary`: `Text`, `Image`, `Audio`, `File`, `Refusal`
2. `StandaloneOrdinary`: `Reasoning`, `ToolCall`, `ProviderItem`
3. `StandaloneToolResult`: `ToolResult`
4. `ControlBoundary`: `NextDownstreamEnvelopeExtra`

EGR-10. `MessageCompatibleOrdinary` means only that the node is eligible for family-specific envelope grouping. It does not mean the node is always grouped with adjacent nodes.

EGR-11. `StandaloneOrdinary` means the generic grouping helper MUST start a fresh family-specific branch for that node kind instead of merging it into the current `MessageCompatibleOrdinary` run.

EGR-12. `ProviderItem` MUST be treated as `StandaloneOrdinary` by shared generic grouping helpers. A family-specific adapter MAY consume a provider item through a dedicated branch, but generic same-role message grouping MUST NOT absorb it.

### 4.3 Family-specific grouping rules

EGR-13. For target family `responses`, the grouping helper MUST obey all rules below:

1. `Reasoning` is a distinct top-level Responses item and therefore always forces its own envelope.
2. `ToolCall` is a distinct top-level Responses `function_call` item and therefore always forces its own envelope.
3. `ToolResult` is a distinct top-level Responses `function_call_output` item and therefore always forces its own envelope.
4. A Responses `message` item run may contain only `MessageCompatibleOrdinary` nodes.
5. All nodes in one Responses `message` item run MUST share the same `role`.
6. A change in `Text.phase` between two adjacent text nodes in a candidate run MUST force a new Responses `message` item.
7. A non-text node between two text nodes does not erase phase boundaries. If later text resumes with a different `phase`, that resumed text MUST begin a new Responses `message` item.

EGR-14. For target family `chat_completion`, the grouping helper MUST obey all rules below:

1. `ToolResult` always forces its own top-level envelope.
2. `ToolCall` never belongs to a pure same-role message-content heuristic. The helper MUST route tool-call nodes through a chat-tool-call branch that preserves tool-call index order and argument-fragment assembly.
3. `Reasoning` never becomes grouped canonical message content. The helper MUST route reasoning nodes through chat reasoning fields or `reasoning_details` construction.
4. Non-stream message-content grouping may merge adjacent `MessageCompatibleOrdinary` nodes of the same `role` only when doing so does not interleave content emission with tool-call emission in a way forbidden by downstream chat semantics.
5. In streaming chat encoding, a content-emission run and a tool-call-emission run are always distinct downstream lifecycle segments even when both belong to the same eventual assistant message object.

EGR-15. For target family `messages`, the grouping helper MUST obey all rules below:

1. `ToolResult` always forces its own top-level tool-result block container.
2. `ToolCall` always forces its own tool-use block branch.
3. `Reasoning` always forces its own thinking block branch.
4. `MessageCompatibleOrdinary` nodes may join one Anthropic message-envelope reconstruction only when their flat order can be represented as a strict non-interleaving sequence of content blocks.
5. The helper MUST preserve final Anthropic `content[]` block index order exactly. It MUST never reorder nodes in order to coalesce blocks.

### 4.4 Control-node application during grouping

EGR-16. When a grouping helper has buffered one or more `NextDownstreamEnvelopeExtra` nodes, the merged control-node map MUST be attached to the very next downstream envelope object that is actually emitted.

EGR-17. If a key exists both in the buffered control-node map and in a typed field produced by the target-family adapter for that downstream envelope, the typed field value MUST win.

EGR-18. Once the buffered control-node map has been applied to one emitted downstream envelope, it is consumed. It MUST NOT leak to later envelopes.

EGR-19. If a control node is followed by a `ToolResult`, the buffered control-node map applies to that `ToolResult` envelope, not to the next ordinary-node envelope after it.

EGR-20. If a control node appears between two otherwise groupable ordinary nodes, the control node forces a flush and therefore the two ordinary nodes belong to different downstream envelopes.

## 5. Stream-helper invariants

### 5.1 Canonical node lifecycle

SHELP-1. Stream decoders and stream transforms MUST treat `node_index` as the canonical stream identity for one flat node lifecycle.

SHELP-2. `node_index` values are URP-local coordinates assigned in first-seen flat-node order starting at `0`.

SHELP-3. A helper MUST NOT assume that `node_index` equals any downstream or upstream wire coordinate such as Responses `output_index`, Responses `content_index`, Anthropic block index, or chat tool-call array index.

SHELP-4. For one `node_index`, the canonical lifecycle is exactly: one `NodeStart`, zero or more `NodeDelta`, one `NodeDone`.

SHELP-5. `ToolResult` and `NextDownstreamEnvelopeExtra` lifecycles contain zero `NodeDelta` events.

SHELP-6. `NodeDone.node` MUST be complete terminal state for that one flat node, including all typed fields and that node's final local `extra_body`.

SHELP-7. `NodeDone.node` completeness MUST NOT depend on first rebuilding a grouped message object or on scanning sibling nodes in the same downstream envelope.

### 5.2 `ResponseDone.output` authority

SHELP-8. `ResponseDone.output` MUST contain the complete terminal ordered flat node sequence.

SHELP-9. `ResponseDone.output` is authoritative over any helper-maintained partial node map, buffered message accumulator, merged-item cache, or downstream-envelope reconstruction state.

SHELP-10. If helper-maintained incremental state disagrees with `ResponseDone.output`, helpers MUST discard the incremental reconstruction and trust `ResponseDone.output`.

SHELP-11. Completed-only downstream reconstruction, duplicate suppression, terminal finish synthesis, and post-stream transforms MUST derive final semantic state from `ResponseDone.output`.

SHELP-12. No stream helper may require a helper named `merged_output_items`, or any equivalent grouped-message reconstruction cache, as canonical authority.

### 5.3 Separation between canonical lifecycle and downstream wire lifecycle

SHELP-13. Canonical flat node lifecycle and downstream wire lifecycle are separate layers.

SHELP-14. The canonical layer owns only `ResponseStart`, `NodeStart`, `NodeDelta`, `NodeDone`, `ResponseDone`, and `Error`.

SHELP-15. The downstream encoder layer owns all wire-format lifecycle coordinates such as Responses item identifiers, Responses content-part identifiers, chat chunk sequencing, Anthropic block indices, and Anthropic message envelope events.

SHELP-16. One downstream wire envelope MAY correspond to several canonical nodes, and one canonical node MAY produce several downstream wire events. This does not change canonical node order.

SHELP-17. Stream helpers MUST NOT attempt to mirror downstream wire envelopes as canonical state. They MUST reconstruct wire envelopes from canonical node events and `ResponseDone.output` only.

SHELP-18. A stream transform MAY rewrite canonical node events, but it MUST preserve a valid canonical node lifecycle and a valid terminal `ResponseDone.output` unless the runtime switches to a buffered synthetic path under the transform spec.

## 6. Concrete target for Tasks 6 through 9

TASK-1. Task 6 target: `src/urp/mod.rs` MUST expose the flat Rust core type family from RTYPE-1 through RTYPE-11 and MUST remove grouped `Item` and `Part` concepts from canonical URP storage.

TASK-2. Task 7 target: shared encode helpers in `src/urp/encode/*` and `src/urp/greedy.rs` MUST implement grouping and extraction logic against flat `Node` sequences under EGR-1 through EGR-20 rather than against grouped `parts` inside `Message`.

TASK-3. Task 8 target: shared decode helpers in `src/urp/decode/*` MUST emit flat `Node` values directly, assign unknown fields by NH-19 through NH-25, and emit `NextDownstreamEnvelopeExtra` only for envelope-local passthrough.

TASK-4. Task 9 target: stream helpers in `src/urp/stream_decode/*`, `src/urp/stream_encode/*`, and `src/urp/stream_helpers.rs` MUST use canonical node lifecycle events from RTYPE-9 through RTYPE-10 plus SHELP-1 through SHELP-18, with `ResponseDone.output` as final authority.

TASK-5. Tasks 6 through 9 are not complete if any helper still depends on a hidden canonical grouped-message assumption, including any helper whose core inputs are grouped `parts`, grouped `items`, or cached merged message wrappers.
