# Monoize URP v2 Transform System Specification

## 0. Status

- Version: `2.0.0`
- Product name: Monoize
- Internal protocol name: `URP v2`
- Scope: flat URP v2 request and response transform surfaces, flat decode and encode behavior, flat streaming transform behavior, cross-family nested passthrough stripping, and routing integration.

## 1. URP v2 Core Contract

URPTF-1. This specification extends `spec/urp-v2-flat-structure.spec.md`. If the two files disagree about the meaning of a URP v2 request, response, node, control node, or stream event, `spec/urp-v2-flat-structure.spec.md` is authoritative for structure and this file is authoritative for transform execution.

URPTF-2. Internal request representation for transforms MUST be `UrpRequestV2 { model, input, ... }` where `input` is an ordered flat `Vec<Node>`.

URPTF-3. Internal response representation for transforms MUST be `UrpResponseV2 { id, model, output, ... }` where `output` is an ordered flat `Vec<Node>`.

URPTF-4. `Message { role, parts }` is not a URP v2 value and MUST NOT appear in canonical request storage, canonical response storage, transform-visible payload surfaces, or canonical stream terminal state.

URPTF-5. Transform-visible payload surfaces are only these surfaces:
1. typed top-level request and response fields plus top-level `extra_body`;
2. ordinary top-level nodes in `request.input` and `response.output`;
3. top-level `ToolResult` nodes and their nested `ToolResultContent` entries;
4. top-level control nodes;
5. canonical URP v2 stream events and terminal `ResponseDone.output`.

URPTF-6. A transform MUST operate on URP v2 values only. A transform MUST NOT require access to raw downstream wire payloads, raw upstream wire payloads, or decoder-private grouped helper state.

URPTF-7. Server behavior MUST remain stateless. Transform execution MUST NOT depend on persisted conversation state or on `previous_response_id` lookups.

## 2. Decode and encode requirements for transforms

DEC-1. Downstream requests from `/v1/chat/completions`, `/v1/responses`, and `/v1/messages` MUST decode into `UrpRequestV2` before any request-phase transform executes.

DEC-2. Downstream responses and upstream responses MUST decode into `UrpResponseV2` before any response-phase non-stream transform executes.

DEC-3. Stream decoders MUST emit canonical URP v2 stream events before any response-phase stream transform executes.

DEC-4. Unknown wire fields that belong to top-level request or response objects MUST decode into top-level `extra_body`.

DEC-5. Unknown wire fields that belong to one ordinary node, one `ToolResult`, or one `ToolResultContent` entry MUST decode into that exact owner's `extra_body`.

DEC-6. Unknown wire fields that belong to one downstream or upstream envelope rather than to one emitted ordinary node or `ToolResult` MUST decode as `next_downstream_envelope_extra` control nodes under the URP v2 flat structure rules.

DEC-7. Tool calls MUST decode as ordinary `ToolCall` nodes.

DEC-8. Tool execution output MUST decode as top-level `ToolResult` nodes. A decoder MUST NOT represent tool execution output as an ordinary role-bearing node.

DEC-9. Reasoning data from upstream or downstream wire formats MUST decode as ordinary `Reasoning` nodes, using the typed `encrypted` field when the provider requires opaque reasoning passthrough data.

ENC-1. Upstream request construction MUST encode from URP v2 values only.

ENC-2. URP v2 to upstream encoding MUST support provider types `responses`, `chat_completion`, `messages`, `gemini`, and `openai_image`.

ENC-3. If one `Reasoning` node carries both opaque reasoning payload in `encrypted` and plaintext fields in `content` and/or `summary`, an adapter MAY omit the plaintext fields only when the target wire format requires opaque reasoning exclusivity for that same reasoning node.

ENC-4. An adapter MUST NOT drop one `Reasoning` node solely because a different `Reasoning` node in the same flat node sequence carries `encrypted`.

ENC-5. Model rewrite MUST apply provider `models[requested].redirect` when present; otherwise the requested model name remains the upstream model.

ENC-6. Logical downstream envelope reconstruction belongs only to the encoder. A decoder or transform MUST NOT reintroduce canonical grouped-message storage under different terminology.

### 2.1 Cross-family nested passthrough stripping

XSTRIP-1. Protocol family names and cross-family hop semantics are defined by `spec/urp-v2-flat-structure.spec.md`.

XSTRIP-2. Cross-family nested passthrough stripping applies only to nested passthrough state. Top-level request and response `extra_body` are not nested and MUST remain intact across all hops.

XSTRIP-3. When the downstream protocol family differs from the upstream provider type, and cross-family stripping is enabled for that provider attempt, the runtime MUST perform stripping after downstream request decoding and before provider request-phase transforms execute.

XSTRIP-4. The stripping pass in XSTRIP-3 MUST do all of the following on `UrpRequestV2.input`:
1. clear `extra_body` on every ordinary node;
2. clear `extra_body` on every top-level `ToolResult` node;
3. clear `extra_body` on every nested `ToolResultContent` entry; and
4. remove every `next_downstream_envelope_extra` control node.

XSTRIP-5. After XSTRIP-4, later provider request-phase transforms MAY add new target-family nested passthrough state.

XSTRIP-6. On a same-family hop, the runtime MUST preserve node-local `extra_body`, `ToolResult.extra_body`, `ToolResultContent.extra_body`, and control nodes.

XSTRIP-7. Cross-family stripping enablement MUST be controlled by these settings in descending precedence:
1. provider-level override `strip_cross_protocol_nested_extra` when present;
2. global setting `monoize_strip_cross_protocol_nested_extra` otherwise.

XSTRIP-8. Resolution semantics for XSTRIP-7 are exact:
1. provider override `Some(true)` means strip on every cross-family hop for that provider attempt;
2. provider override `Some(false)` means never strip for that provider attempt; and
3. provider override `None` means inherit the global setting.

## 3. Canonical streaming representation for transforms

STR-1. Response-phase stream transforms MUST operate on canonical URP v2 stream events only.

STR-2. The transform-visible canonical event set is: `ResponseStart`, `NodeStart`, `NodeDelta`, `NodeDone`, `ResponseDone`, and `Error`.

STR-3. `NodeDone.node` MUST contain the complete terminal node for that `node_index`.

STR-4. `ResponseDone.output` MUST contain the complete terminal ordered flat `Vec<Node>`.

STR-5. `ResponseDone.output` is the authoritative final streamed response state for downstream stream reconstruction, synthetic terminal event synthesis, and all post-stream transform reasoning.

STR-6. Pass-through streaming MUST use the architecture `upstream SSE -> decoder -> URP v2 stream event channel -> transform pipeline -> downstream encoder`.

STR-7. If an upstream streaming protocol emits opaque reasoning payload incrementally, the stream decoder MUST emit ordered `NodeDelta` events that preserve that opaque payload before terminal `NodeDone` and `ResponseDone` reconstruction. A downstream stream encoder or transform MUST NOT rely on terminal reconstruction alone to surface opaque reasoning data during pass-through streaming.

STR-8. The transform engine MUST support incremental stream processing with per-request mutable transform state.

STR-9. If a streaming request matches at least one enabled response-phase transform rule that requires whole-response mutation rather than incremental URP v2 stream rewriting, or if the selected downstream streaming protocol cannot faithfully represent that transform's incremental output, the runtime MAY execute the upstream attempt in non-stream mode, apply response transforms to `UrpResponseV2`, and emit synthesized downstream streaming events. Buffered synthetic timing is allowed in that mode.

STR-10. A stream transform MAY rewrite `NodeStart`, `NodeDelta`, `NodeDone`, and `ResponseDone`, but it MUST preserve a valid canonical node lifecycle unless the runtime switches to the buffered synthetic path in STR-9.

## 4. Transform system

TF-1. A transform MUST implement these conceptual interface members:
- `type_id()`
- `supported_phases()`
- `supported_scopes()`
- `config_schema()`
- `parse_config()`
- `init_state()`
- `apply()`

TF-1a. Request-phase and response-phase transform execution MUST support asynchronous work. The runtime MAY await file I/O, local computation, or other asynchronous operations performed by `apply()` before continuing to the next matching rule.

TF-2. Persisted `TransformRuleConfig` MUST include `transform`, `enabled`, `models`, `phase`, and `config`.

TF-3. Transform rule execution MUST be ordered. The output of rule `i` MUST be the input to rule `i + 1` within the same phase and scope chain.

TF-4. A rule is eligible to execute only when all conditions below hold:
1. `enabled = true`;
2. the rule phase equals the current phase; and
3. if `models` is present, at least one model glob matches the normalized logical model.

TF-5. Transform registry discovery MUST be automatic through `inventory`.

TF-6. Adding a new transform file with a valid inventory submission MUST be sufficient for registration.

TF-7. Built-ins that MUST exist are:
- `reasoning_to_think_xml`
- `think_xml_to_reasoning`
- `reasoning_effort_to_budget`
- `reasoning_effort_to_model_suffix`
- `strip_reasoning`
- `strip_input_reasoning`
- `system_to_developer_role`
- `developer_to_system_role`
- `merge_consecutive_roles`
- `inject_system_prompt`
- `override_max_tokens`
- `set_field`
- `remove_field`
- `force_stream`
- `append_empty_user_message`
- `enable_openai_image_generation_tool`
- `split_sse_frames`
- `auto_cache_user_id`
- `auto_cache_system`
- `auto_cache_tool_use`
- `compress_user_message_images`
- `resolve_image_urls`
- `plaintext_reasoning_to_summary`
- `reasoning_summary_to_raw_cot`
- `reasoning_content_delta`
- `assistant_markdown_images_to_output`
- `assistant_output_images_to_markdown`
- `strip_orphaned_tool_use`

TF-8. Every transform registry item returned by `/api/dashboard/transforms/registry` MUST include `type_id`, `supported_phases`, `supported_scopes`, and `config_schema`.

TF-9. Scope semantics are exact:
1. `provider` means the transform MAY be configured in provider transform chains;
2. `global` means the transform MAY be configured in the system settings global transform chain;
3. `api_key` means the transform MAY be configured in API-key transform chains;
4. a transform MAY support more than one scope; and
5. dashboard editors MUST hide transforms that do not support the current editor scope.

### 4.1 Transform-visible request and response surfaces

SURF-1. Request-phase transforms MAY read and write typed top-level request fields and top-level request `extra_body`.

SURF-2. Request-phase transforms MAY read and write ordinary nodes in `request.input`.

SURF-3. Request-phase transforms MAY read and write top-level `ToolResult` nodes and their nested `ToolResultContent` entries in `request.input`.

SURF-4. Request-phase transforms MAY read, remove, preserve, or insert control nodes only when this specification defines that behavior explicitly for the named transform. Otherwise control nodes are opaque sequence elements and MUST remain byte-for-byte unchanged.

SURF-5. Response-phase transforms MAY read and write typed top-level response fields and top-level response `extra_body`.

SURF-6. Response-phase transforms MAY read and write ordinary nodes in `response.output`.

SURF-7. Response-phase transforms MAY read and write top-level `ToolResult` nodes and nested `ToolResultContent` entries in `response.output` when the transform's target surface includes those nodes explicitly.

SURF-8. Response-phase stream transforms MAY read and write canonical stream events and terminal `ResponseDone.output`.

SURF-9. Unless a transform section below says otherwise, a transform MUST treat `ToolResult` as outside ordinary role-based rewrite and merge semantics.

SURF-10. Unless a transform section below says otherwise, a transform MUST treat `next_downstream_envelope_extra` as an opaque boundary marker rather than as user-visible content.

### 4.2 Role and sequence transforms on ordinary nodes

ROLE-1. `system_to_developer_role` is request-phase only.

ROLE-2. `system_to_developer_role` MUST rewrite `role = system` to `role = developer` on ordinary nodes in `request.input`.

ROLE-3. `system_to_developer_role` MUST NOT modify `ToolResult` nodes, `ToolResultContent` entries, or control nodes.

ROLE-4. `developer_to_system_role` is request-phase only.

ROLE-5. `developer_to_system_role` MUST rewrite `role = developer` to `role = system` on ordinary nodes in `request.input`.

ROLE-6. `developer_to_system_role` MUST NOT modify `ToolResult` nodes, `ToolResultContent` entries, or control nodes.

ROLE-7. `merge_consecutive_roles` is request-phase only.

ROLE-8. `merge_consecutive_roles` MUST operate on a derived contiguous-run view of adjacent ordinary nodes in `request.input`. It MUST NOT introduce grouped canonical storage.

ROLE-9. Within one maximal run of adjacent ordinary nodes, `merge_consecutive_roles` MAY merge neighboring ordinary nodes only when all conditions below hold:
1. both nodes are ordinary nodes;
2. both nodes carry the same ordinary `role`;
3. neither node is `Reasoning` or `ToolCall` if the downstream encoder treats those node kinds as distinct top-level semantic units;
4. no `ToolResult` node lies between them; and
5. no control node lies between them.

ROLE-10. If `merge_consecutive_roles` merges neighboring ordinary nodes, it MUST preserve node order and MUST preserve all surviving typed fields. If conflicting nested passthrough keys survive on merged ordinary-node state, the earlier surviving node's typed fields remain authoritative and merge policy for residual passthrough keys MUST be deterministic.

ROLE-11. `merge_consecutive_roles` MUST NOT merge `ToolResult` into ordinary nodes and MUST NOT cross a control-node boundary.

### 4.3 `append_empty_user_message`

AEUM-1. Phase: request only.

AEUM-2. Config MAY contain `content` as a string. Default value is one single-space string.

AEUM-3. The transform MUST inspect the final element of `request.input`.

AEUM-4. If the final element is an ordinary node with `role = assistant`, the transform MUST append one ordinary `Text` node with `role = user` and `content = config.content`.

AEUM-5. If `request.input` is empty, or if the final element is not an ordinary assistant node, the transform MUST be a no-op.

AEUM-6. `append_empty_user_message` MUST NOT append `ToolResult` nodes, MUST NOT append control nodes, and MUST NOT inspect derived grouped-message wrappers.

### 4.4 `inject_system_prompt`

ISP-1. Phase: request only.

ISP-2. Config MUST contain `content: string` and `position: "prepend" | "append"`.

ISP-3. `inject_system_prompt` targets only ordinary `Text` nodes with `role = system` in `request.input`.

ISP-4. If `position = prepend`, the transform MUST locate the first ordinary `Text` node with `role = system` and append the configured text to that node's `content` as an additional system text segment under the encoder's later grouping rules. If no such node exists, the transform MUST insert one new ordinary `Text` node with `role = system` at the beginning of `request.input`.

ISP-5. If `position = append`, the transform MUST locate the last ordinary `Text` node with `role = system` and append the configured text to that node's `content` as an additional system text segment under the encoder's later grouping rules. If no such node exists, the transform MUST append one new ordinary `Text` node with `role = system` to the end of `request.input`.

ISP-6. `inject_system_prompt` MUST NOT rewrite `ToolResult` nodes, `ToolResultContent`, or control nodes.

ISP-7. If a control node lies at the target insertion boundary, the inserted system text node MUST be placed as an ordinary sequence element without consuming or modifying the control node.

### 4.5 `strip_orphaned_tool_use`

SOTU-1. Phase: request only.

SOTU-2. `strip_orphaned_tool_use` MUST collect the set of `call_id` values from top-level `ToolResult` nodes in `request.input`.

SOTU-3. The transform MUST remove every ordinary `ToolCall` node in `request.input` whose `call_id` does not appear in the collected `ToolResult` set.

SOTU-4. `strip_orphaned_tool_use` MUST NOT remove `ToolResult` nodes.

SOTU-5. `strip_orphaned_tool_use` MUST preserve all non-`ToolCall` ordinary nodes unchanged.

SOTU-6. `strip_orphaned_tool_use` MUST preserve control nodes unchanged.

### 4.6 Image transforms on request ordinary nodes

EOIGT-1. `enable_openai_image_generation_tool` is request-phase only.

EOIGT-2. Config MAY contain:
- `output_format` as one of `png`, `webp`, or `jpeg`; default `png`;
- `action` as a string; optional; and
- `extra` as an object whose entries are copied verbatim into the inserted tool descriptor's `extra_body`.

EOIGT-3. The transform MUST inspect only top-level `request.tools`.

EOIGT-4. If `request.tools` is absent, the transform MUST create it as a one-element array containing one tool descriptor with `type = "image_generation"`.

EOIGT-5. If `request.tools` already contains at least one tool descriptor whose `type` is `image_generation`, the transform MUST leave `request.tools` unchanged.

EOIGT-6. When the transform inserts a tool descriptor, it MUST set:
1. `type = "image_generation"`;
2. `extra_body.size = request.extra_body.size` when `request.extra_body` contains `size`;
3. `extra_body.quality = request.extra_body.quality` when `request.extra_body` contains `quality`;
4. all `extra` object entries into `extra_body` afterward, preserving their JSON values verbatim;
5. `extra_body.output_format = <configured output_format>`; and
6. `extra_body.action = <configured action>` only when `action` is configured.

EOIGT-6a. If `extra` contains keys `size` or `quality`, the transform MUST preserve the `extra` values in the inserted tool descriptor and MUST NOT overwrite them with same-named values from `request.extra_body`.

EOIGT-6b. If `extra` contains keys `output_format` or `action`, the transform MUST still apply EOIGT-6.5 and EOIGT-6.6 afterward so that the explicit typed config fields take precedence over colliding `extra` entries.

EOIGT-7. The transform MUST preserve all pre-existing tool descriptors in source order and MUST append the inserted `image_generation` tool after them.

EOIGT-8. The transform MUST NOT modify `request.input`, `request.tool_choice`, or any response-phase payload surface.

CUMI-1. `compress_user_message_images` is request-phase only.

CUMI-2. Config MAY contain:
- `max_edge_px` (integer, default `1568`)
- `jpeg_quality` (integer, default `80`)
- `skip_if_smaller` (boolean, default `true`)

CUMI-3. The transform MUST inspect only ordinary `Image` nodes with `role = user`.

CUMI-4. Eligible image sources are:
1. `Image.source = Base64`; or
2. `Image.source = Url` whose `url` is a `data:<image-media-type>;base64,<payload>` URL.

CUMI-5. Non-`data:` URL sources MUST remain unchanged.

CUMI-6. If the media type is not decodable by the image codec stack, the node MUST remain unchanged.

CUMI-7. On successful replacement:
1. `Base64` sources MUST remain `Base64` with updated `media_type` and `data`;
2. `data:` URL sources MUST remain `Url` with updated `url`; and
3. provider-specific typed fields such as image detail hints MUST remain unchanged.

CUMI-8. The cache key, persistence, eviction, and failure-isolation rules from the previous transform specification remain normative, but they apply to eligible ordinary `Image` nodes rather than to nested message parts.

RIU-1. `resolve_image_urls` is request-phase only.

RIU-2. Config MAY contain:
- `timeout_seconds` (integer, default `30`)
- `max_bytes` (integer, default `20971520`)
- `roles` (string array of ordinary roles, optional)

RIU-3. The transform MUST inspect only ordinary `Image` nodes whose `role` is in the configured role set, or all ordinary roles when `roles` is absent.

RIU-4. The transform MUST inspect only `Image.source = Url` whose `url` does not start with `data:`.

RIU-5. On successful fetch, the transform MUST replace the source with `Image.source = Base64 { media_type, data }` using standard base64 encoding.

RIU-6. Multiple eligible image fetches within one request MUST be concurrent.

RIU-7. A failed fetch for one image node MUST NOT block other eligible image nodes and MUST leave the failed node unchanged.

### 4.7 Reasoning transforms on flat nodes and stream state

PRTS-1. `plaintext_reasoning_to_summary` is response-phase only.

PRTS-2. Config MUST be an empty object.

PRTS-3. On non-stream responses, the transform MUST inspect only ordinary `Reasoning` nodes.

PRTS-4. If a `Reasoning` node carries non-empty plaintext `content`, the transform MUST move that value into `summary` and clear `content`.

PRTS-5. PRTS-4 applies whether or not the same `Reasoning` node also carries `encrypted`.

PRTS-6. If a `Reasoning` node already has `summary`, the moved plaintext `content` value MUST replace the previous `summary`.

PRTS-7. The transform MUST preserve `encrypted`, `source`, and node-local `extra_body`.

PRTS-8. Empty plaintext content MUST NOT create a non-empty summary.

PRTS-9. On streams, the transform MAY annotate `NodeDelta` event `extra_body` to mark summary-oriented reasoning deltas, but terminal correctness is defined by `NodeDone.node` and `ResponseDone.output` after applying PRTS-4 through PRTS-8 to `Reasoning` nodes.

RSRC-1. `reasoning_summary_to_raw_cot` is response-phase only.

RSRC-2. Config MUST be an empty object.

RSRC-3. On non-stream responses, the transform MUST inspect only ordinary `Reasoning` nodes.

RSRC-4. If a `Reasoning` node carries non-empty `summary`, the transform MUST mark that node for OpenWebUI-compatible raw chain-of-thought emission by setting node-local `extra_body.openwebui_reasoning_content = true`.

RSRC-5. The transform MUST NOT modify `content`, `summary`, or `encrypted`.

RSRC-6. On streams, the transform MAY annotate reasoning `NodeDelta` event `extra_body` for downstream encoders, but terminal correctness is defined by marking the final `Reasoning` nodes in `NodeDone.node` and `ResponseDone.output`.

RSRC-7. When a downstream Chat Completions encoder sees `openwebui_reasoning_content = true` on a reasoning summary node, it MUST emit that summary through OpenWebUI-compatible raw-CoT fields for non-streaming and streaming encodings.

RCD-1. `reasoning_content_delta` is response-phase only.

RCD-2. Config MUST be an empty object.

RCD-3. For each ordinary `Reasoning` node or reasoning `NodeDelta`, the transform MUST resolve a plaintext `reasoning_content` value as follows:
1. use non-empty `content` if present;
2. otherwise use non-empty `summary` if present; and
3. otherwise resolve no value.

RCD-4. `encrypted` MUST NOT contribute to the resolved value.

RCD-5. If a resolved value exists on a terminal `Reasoning` node, the transform MUST set node-local `extra_body.inject_reasoning_content` to that string.

RCD-6. If a resolved value exists on a reasoning `NodeDelta`, the transform MAY set event-local `extra_body.inject_reasoning_content` to that string.

RCD-7. If a reasoning node or delta carries only encrypted reasoning and no plaintext `content` or `summary`, the transform MUST inject nothing.

RCD-8. The transform MUST be independent of `reasoning_summary_to_raw_cot`. Both transforms MAY be enabled simultaneously.

RCD-9. When a downstream Chat Completions encoder sees non-empty `inject_reasoning_content`, it MUST emit the additional OpenRouter-compatible or DeepSeek-compatible downstream reasoning-content field without removing normal reasoning fields.

### 4.8 Response image transforms on flat ordinary nodes and stream state

AMIO-1. `assistant_markdown_images_to_output` is response-phase only.

AMIO-2. Config MUST be an empty object.

AMIO-3. The transform MUST inspect only ordinary assistant `Text` nodes.

AMIO-4. The transform MUST recognize Markdown image syntax `![alt](url)` inside those text-node contents.

AMIO-5. Recognized ordinary URLs MUST become ordinary assistant `Image` nodes with `Image.source = Url { url, detail: None }`.

AMIO-6. Recognized `data:image/...;base64,...` URLs MUST become ordinary assistant `Image` nodes with `Image.source = Base64 { media_type, data }`.

AMIO-7. Non-image `data:` URLs and malformed Markdown image blocks MUST remain inside the text content unchanged.

AMIO-8. Extracted image nodes MUST be inserted immediately after the originating ordinary assistant text node, preserving original order.

AMIO-9. If removing Markdown image blocks leaves a text node empty, the transform MAY remove that text node.

AMIO-10. On streams where the downstream protocol can faithfully represent extracted image nodes incrementally, the transform MUST preserve pass-through timing by buffering only the ambiguous Markdown suffix required to disambiguate a candidate image block, emitting safe text deltas as soon as possible, and emitting image-node lifecycle events in source order once one full Markdown image block is recognized.

AMIO-11. Under the incremental path in AMIO-10, the transform MUST update terminal `NodeDone.node` and `ResponseDone.output` so the authoritative final flat node state contains the cleaned text nodes and inserted image nodes.

AMIO-12. If the selected downstream protocol cannot faithfully represent the incremental rewritten node lifecycle, the runtime MUST use the buffered synthetic stream path.

AOIM-1. `assistant_output_images_to_markdown` is response-phase only.

AOIM-2. Config MAY contain `template: string`.

AOIM-3. Raw placeholders are `{{src}}`, `{{url}}`, `{{media_type}}`, and `{{data}}`. Percent-encoded placeholders are `{{src_urlencoded}}`, `{{url_urlencoded}}`, `{{media_type_urlencoded}}`, and `{{data_urlencoded}}`.

AOIM-4. Placeholder resolution MUST follow these exact rules:
1. raw placeholders expand to literal values;
2. percent-encoded placeholders expand to percent-encoded UTF-8 bytes of the raw value;
3. for `Image.source = Url`, `src` and `url` both resolve to the source URL while `media_type` and `data` resolve to empty strings; and
4. for `Image.source = Base64`, `src` resolves to `data:{media_type};base64,{data}`, `url` resolves to the empty string, and `media_type` and `data` resolve to the underlying raw fields.

AOIM-5. If `template` is absent, the transform MUST render `![image]({url})` for URL-backed image nodes and `![image](data:{media_type};base64,{data})` for base64-backed image nodes.

AOIM-6. The transform MUST inspect only ordinary assistant `Image` nodes.

AOIM-7. The transform MUST append the rendered Markdown strings to assistant text output in source order.

AOIM-8. If an assistant text node already exists later in the same encoder-owned ordinary-node run, the rendered Markdown MUST append to the final such text node. Otherwise the transform MUST create one new trailing ordinary assistant `Text` node.

AOIM-9. The transform MUST NOT remove or rewrite the original image nodes.

AOIM-10. On pass-through streams, the transform MUST preserve pass-through timing and MAY apply only to terminal stream state by updating `NodeDone.node` and `ResponseDone.output`.

AOIM-11. If a request is already on the buffered synthetic path because of another matching response transform, the final transformed `UrpResponseV2` MUST produce downstream text deltas that include the appended Markdown.

AOIM-12. `assistant_output_images_to_markdown` alone MUST NOT force an otherwise pass-through stream onto the buffered synthetic path.

### 4.9 `split_sse_frames`

SSF-1. Phase: response only.

SSF-2. Config MAY contain `max_frame_length` as an integer. Default value is `131072`.

SSF-3. If a streaming request matches at least one enabled `split_sse_frames` response rule, the runtime MUST execute the request through the buffered synthetic stream path.

SSF-4. The transform affects only downstream SSE emitted by Monoize.

SSF-5. Non-stream requests remain semantically unchanged.

SSF-6. The transform MUST preserve downstream protocol correctness for Responses, Chat Completions, and Anthropic Messages SSE output.

SSF-7. The transform MUST split oversized string-bearing delta payloads into multiple smaller downstream SSE events of the same downstream protocol event kind, in original order, such that downstream concatenation reconstructs the original logical content.

SSF-8. Eligible split targets include text deltas, reasoning deltas, opaque reasoning signature or encrypted deltas, and tool-argument deltas.

SSF-9. The runtime MUST NOT split inside a serialized JSON string literal by inserting raw SSE line breaks.

SSF-10. If a Responses synthetic stream snapshot event would exceed `max_frame_length` only because it duplicates content already emitted in prior delta events, the runtime MAY replace large duplicated text-bearing snapshot fields with protocol-valid empty values.

SSF-11. Sanitization under SSF-10 MUST preserve reconstructability from the emitted delta sequence and terminal events.

SSF-12. If `max_frame_length` is too small to encode even the minimal wrapper for one required downstream event, the runtime MAY emit that minimal unsplit event rather than fail the entire request.

SSF-13. The transform MUST preserve event order and MUST NOT change usage, finish reason, `call_id`, node role, node phase, or other typed metadata except for the duplicated snapshot text fields allowed by SSF-10.

### 4.10 `reasoning_effort_to_model_suffix`

REMS-1. Phase: request only.

REMS-2. Config MUST contain `rules`, a non-empty ordered array of objects with `pattern` and `suffix`.

REMS-3. The literal substring `{effort}` inside `suffix` MUST expand to the resolved effort value.

REMS-4. On apply:
1. read `request.reasoning.effort`;
2. if the effort is absent or not one of `none`, `minimum`, `low`, `medium`, `high`, `xhigh`, or `max`, the transform MUST no-op;
3. otherwise iterate `rules` in order;
4. for the first matching rule, append the expanded suffix to `request.model`; and
5. stop after the first match.

REMS-5. The transform MUST NOT modify `request.reasoning`.

## 5. Routing and transform pipeline

PIPE-1. Non-stream and stream requests MUST execute in this order:
1. decode the downstream wire payload into URP v2;
2. resolve model suffix;
3. route to provider and channel using waterfall plus fail-forward;
4. set `request.model` to the selected upstream model name;
5. if required, perform cross-family nested passthrough stripping under XSTRIP-3 through XSTRIP-8;
6. apply provider request-phase transforms;
7. apply global request-phase transforms configured in system settings;
8. apply API-key request-phase transforms;
9. encode URP v2 to the upstream wire payload using the selected upstream model name;
10. decode the upstream response or stream into URP v2;
11. apply provider response-phase transforms;
12. apply global response-phase transforms configured in system settings;
13. apply API-key response-phase transforms; and
14. encode URP v2 to the downstream wire response using the original requested logical model name.

PIPE-1a. For streaming requests that satisfy STR-9, the runtime MAY call the upstream non-stream endpoint for that attempt, decode to `UrpResponseV2`, apply response transforms, and emit synthesized downstream stream events. The postcondition is that transformed content remains visible on the stream path even when upstream native streaming is bypassed.

PIPE-1b. Model identity split is exact:
1. the upstream model name sent to the provider is `request.model` after provider request-phase transforms; and
2. billing, logging, and downstream response `model` field MUST use the original requested logical model name.

PIPE-1c. Transform rule model matching MUST use the normalized logical model rather than temporary redirected upstream model names.

PIPE-2. API-key policy MUST support a default `max_multiplier` routing constraint and ordered transform rules.

PIPE-3. Provider configuration MUST support ordered transform rules.

PIPE-3a. System settings MUST support ordered global transform rules. The default global transform rule list MUST be empty.

PIPE-4. If request max multiplier is absent, the router MUST use the API-key max multiplier when configured.

## 6. Externally stable downstream safety constraints

SAFE-1. The transform system rewrite to flat URP v2 MUST preserve externally observable Responses safety constraints.

SAFE-2. For `/v1/responses`, downstream encoders and transforms MUST preserve observable response lifecycle, output-item lifecycle, content-part lifecycle, output ordering, addressing coordinates, item status transitions, and terminal `response.completed` ordering even though canonical internal storage is flat.

SAFE-3. `ResponseDone.output` is the only authoritative terminal flat state used to reconstruct final Responses output items.

SAFE-4. The transform system rewrite to flat URP v2 MUST preserve Anthropic Messages safety constraints.

SAFE-5. For `/v1/messages`, downstream encoders and transforms MUST preserve the exact event lifecycle `message_start -> content_block_* -> message_delta -> message_stop`, preserve block index semantics as final content positions, preserve cumulative usage semantics, and keep `tool_result` distinct from ordinary role-bearing content.

SAFE-6. The transform system rewrite to flat URP v2 MUST preserve OpenRouter-compatible Chat safety constraints.

SAFE-7. For `/v1/chat/completions`, downstream encoders and transforms MUST preserve OpenRouter-compatible reasoning behavior, including `reasoning_details`, plain-text reasoning fields when those exact downstream fields already exist, final usage chunk semantics, SSE comment compatibility, and chunk-shaped streaming error compatibility.

SAFE-8. Control nodes MUST NOT be emitted downstream as visible content. Their only normative downstream effect is envelope-level passthrough application by the next downstream encoder-owned consumable envelope.

## 7. Validity summary

VALID-1. A valid transform-visible URP v2 request or response uses flat top-level node sequences, not grouped message wrappers.

VALID-2. `ToolResult` remains a distinct top-level node type and MUST NOT be reclassified as an ordinary role-bearing node.

VALID-3. Control-node behavior is explicit only where stated in this specification. Otherwise control nodes are opaque sequence elements.

VALID-4. Response stream terminal state is authoritative. `ResponseDone.output` is the final flat node sequence.

VALID-5. If faithful incremental stream rewriting is not possible, buffered synthetic streaming remains allowed.
