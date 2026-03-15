# Monoize URP + Transform System Specification

## 0. Status

- Version: `1.0.0`
- Product name: Monoize
- Internal protocol name: `URP`
- Scope: URP structures, decode/encode behavior, transform execution, and routing integration.

## 1. URP Core Contract

URP-1. Internal request representation MUST be `UrpRequest { model, inputs, ... }`.

URP-2. URP MUST be messages-centric: request/response history is represented as an ordered array of `Message`.

URP-3. A `Message` MUST include `role` and `parts`.

URP-4. `Part` MUST support at least: `Text`, `Image`, `Audio`, `File`, `Reasoning`, `ToolCall`, `ToolResult`, `Refusal`, `ProviderItem`.

URP-5. Every URP struct that maps to JSON MUST support unknown field passthrough using flattened `extra_body`.

URP-6. Encode path MUST flatten `extra_body` entries into the output object.

URP-7. If a key exists in both a named field and `extra_body`, named field value MUST win.

URP-8. Server MUST remain stateless: no conversation persistence and no dependency on `previous_response_id`.

## 2. Decode/Encode Requirements

DEC-1. Downstream requests from `/v1/chat/completions`, `/v1/responses`, `/v1/messages` MUST decode into `UrpRequest`.

DEC-2. Unknown wire fields MUST be preserved into URP `extra_body`.

DEC-3. Tool calls MUST decode to `Part::ToolCall`.

DEC-4. Tool result messages MUST decode to `Role::Tool` + `Item::ToolResult` with sibling content parts.

DEC-5. Reasoning fields from upstream/downstream wire formats MUST decode into `Part::Reasoning`, using `encrypted` when the provider requires opaque passthrough data.

ENC-1. Upstream request construction MUST encode from URP only; transforms MUST NOT access raw wire payloads.

ENC-2. URP-to-upstream encoding MUST support provider types: `responses`, `chat_completion`, `messages`, `gemini`, `grok`. The `grok` provider type MUST reuse the same request and response adapter logic as `responses`.

ENC-3. History encoding rule:
- if a single `Part::Reasoning` carries both opaque reasoning payload (`encrypted`) and plaintext fields (`content` and/or `summary`), adapters MAY omit the plaintext fields only when the target wire format requires opaque reasoning exclusivity for that same reasoning part.
- adapters MUST NOT drop a distinct `Part::Reasoning` solely because another reasoning part in the same message carries `encrypted: Some(_)`.
- otherwise `Part::Reasoning` MAY be encoded when supported by target wire format.

ENC-4. Model rewrite MUST apply provider `models[requested].redirect` when present; otherwise use requested model.

## 3. Streaming Representation

STR-1. Internal streaming representation MUST use `UrpStreamEvent`.

STR-2. `UrpStreamEvent` MUST support: `ResponseStart`, `ItemStart`, `PartStart`, `Delta`, `PartDone`, `ItemDone`, `ResponseDone`, `Error`.

STR-3. `Delta` MUST be part-indexed and typed via `PartDelta` (text/reasoning/tool args/media/refusal/provider-item variants).

STR-3a. `PartDone.part` MUST contain the complete terminal `Part`, `ItemDone.item` MUST contain the complete terminal `Item`, and `ResponseDone.outputs` MUST contain the complete terminal ordered `Vec<Item>`.

STR-3b. `ResponseDone.outputs` is the authoritative final streamed response state for downstream response reconstruction.

STR-3c. Pass-through streaming MUST follow an `upstream SSE -> decoder -> UrpStreamEvent channel -> downstream encoder` architecture. Provider-specific stream decoders emit `UrpStreamEvent`; downstream-specific stream encoders consume `UrpStreamEvent` and produce SSE.

STR-4. Transform engine MUST be able to process stream events incrementally with per-request mutable state.

STR-5. If a streaming request matches any enabled response-phase transform rule, the runtime MAY execute upstream in non-stream mode, apply response transforms on `UrpResponse`, and emit a synthesized downstream stream. In this mode, downstream still receives protocol-correct streaming events (`SSE` for Chat/Responses/Messages), but event timing is buffered.

## 4. Transform System

TF-1. A transform MUST implement:
- `type_id()`
- `supported_phases()`
- `supported_scopes()`
- `config_schema()`
- `parse_config()`
- `init_state()`
- `apply()`

TF-1a. Request-phase and response-phase transform execution MUST support asynchronous work. The runtime MAY await file I/O, network-free background work, or other asynchronous operations performed by `apply()` before continuing to the next rule.

TF-2. `TransformRuleConfig` persisted form MUST include: `transform`, `enabled`, `models`, `phase`, `config`.

TF-3. Transform rule execution MUST be ordered; output of rule `i` is input to rule `i+1`.

TF-4. Rules MUST be filtered by:
- `enabled=true`
- matching phase
- model glob match against the normalized logical model when `models` is present

TF-5. Transform registry MUST be auto-discovered using `inventory`.

TF-6. Adding a new transform file with inventory submit MUST be sufficient for registration.

TF-7. Built-ins that MUST exist:
- `reasoning_to_think_xml`
- `think_xml_to_reasoning`
- `reasoning_effort_to_budget`
- `reasoning_effort_to_model_suffix`
- `strip_reasoning`
- `system_to_developer_role`
- `merge_consecutive_roles`
- `inject_system_prompt`
- `override_max_tokens`
- `set_field`
- `remove_field`
- `force_stream`
- `append_empty_user_message`
- `split_sse_frames`
- `auto_cache_user_id`
- `auto_cache_system`
- `auto_cache_tool_use`
- `compress_user_message_images`
- `plaintext_reasoning_to_summary`
- `assistant_markdown_images_to_output`
- `assistant_output_images_to_markdown`

TF-8. Every transform registry item returned by `/api/dashboard/transforms/registry` MUST include:

- `type_id: string`
- `supported_phases: Phase[]`
- `supported_scopes: ("provider" | "api_key")[]`
- `config_schema: object`

TF-9. Scope semantics:

- `provider` means the transform MAY be configured in provider transform chains.
- `api_key` means the transform MAY be configured in API-key transform chains.
- A transform MAY support both scopes.
- Dashboard editors MUST hide transforms that do not include the current editor scope.

### 4.2 `append_empty_user_message`

AEUM-1. Phase: `request` only.

AEUM-2. Config MAY contain:
- `content` (string, optional): text content for the padding user message. Defaults to `" "` (a single space).
AEUM-3. On apply:
1. Inspect the last element of `req.inputs`.
2. If the last message has `role == assistant`, append a new `Message { role: user, parts: [Text { content: config.content }] }` to `req.inputs`.
3. If the last message is not `assistant`, or `inputs` is empty, no-op.

### 4.3 `compress_user_message_images`

CUMI-1. Phase: `request` only.

CUMI-2. Config MAY contain:
- `max_edge_px` (integer, optional): maximum allowed width or height of a compressed image. Defaults to `1568`.
- `jpeg_quality` (integer, optional): JPEG quality used when the output image has no alpha channel. Defaults to `80`.
- `skip_if_smaller` (boolean, optional): if `true`, the transform MUST keep the original image when the compressed payload is not smaller than the original payload. Defaults to `true`.

CUMI-3. Input eligibility:
1. The transform MUST inspect only messages with `role == user`.
2. Within those messages, the transform MUST inspect `Part::Image` parts whose `source` is either:
   - `ImageSource::Base64`; or
   - `ImageSource::Url` whose `url` field is a `data:<image-media-type>;base64,<payload>` URL.
3. `ImageSource::Url` parts that refer to non-`data:` URLs MUST be left unchanged.
4. Parts whose media type is not decodable by the image codec stack MUST be left unchanged.

CUMI-4. Compression behavior:
1. The transform MUST base64-decode the input bytes.
2. If either image dimension exceeds `max_edge_px`, the transform MUST resize the image so that `max(width, height) == max_edge_px` and the original aspect ratio is preserved.
3. If the decoded image has no alpha channel, the transform MUST encode the output as JPEG using a JPEG optimization crate and `jpeg_quality`.
4. If the decoded image has an alpha channel, the transform MUST encode the output as PNG and MAY run an additional lossless PNG optimization pass before persistence.
5. If `skip_if_smaller == true` and the transformed byte length is greater than or equal to the original byte length, the transform MUST keep the original part unchanged.
6. On successful replacement:
   - if the original source was `ImageSource::Base64`, the transform MUST update the part to `ImageSource::Base64 { media_type, data }` using the transformed media type and transformed base64 payload;
   - if the original source was a `data:` URL in `ImageSource::Url`, the transform MUST keep the source variant as `ImageSource::Url` and replace `url` with a transformed `data:<media-type>;base64,<payload>` URL.
7. When the original source is `ImageSource::Url` and carries provider-specific fields such as `detail`, the transform MUST preserve those fields unchanged.

CUMI-5. Cache key and persistence:
1. Successful transformed outputs MUST be persisted on local disk.
2. The cache key MUST be a deterministic hash of: transform version identifier, input media type, compression config, and original decoded bytes.
3. Cache lookup MUST occur before recompression.
4. Cache hits MUST return the persisted transformed payload without recomputing the image.
5. Cache entries that cannot be decoded from disk MUST be treated as misses and MAY be deleted eagerly.

CUMI-6. Cache directory and eviction:
1. Cache files MUST be stored under a dedicated image-transform cache directory.
2. Each cache file MUST correspond to exactly one hash key.
3. The runtime MUST run a periodic cleanup task that deletes cache files whose last-modified time is older than the cache TTL.
4. Cache reads MUST treat expired files as misses even if the periodic cleanup task has not removed them yet.

CUMI-7. Failure handling:
1. Invalid base64 image payloads MUST leave the original part unchanged.
2. Unsupported or undecodable image payloads MUST leave the original part unchanged.
3. Cache write failures or cache cleanup failures MUST NOT mutate the request incorrectly; if compression succeeds but cache persistence fails, the runtime MAY still use the in-memory transformed payload for the current request.

### 4.4 `split_sse_frames`

SSF-1. Phase: `response` only.

SSF-2. Config MAY contain:
- `max_frame_length` (integer, optional): maximum allowed byte length of any emitted downstream SSE frame payload produced by this transform. Defaults to `131072`.

SSF-3. Default rationale:
1. The default value `131072` MUST match the effective default aiohttp `StreamReader.readline()` high-water limit for SSE body lines when aiohttp client settings are left at defaults.
2. The runtime MUST NOT use aiohttp HTTP-header parser limits such as `8190` as the default for this transform, because those limits apply to header parsing rather than SSE body payload lines.

SSF-4. Activation and scope:
1. If a streaming request matches at least one enabled `split_sse_frames` response-phase rule, the runtime MUST execute the request through the existing response-transform synthetic-stream path defined in PIPE-1a.
2. The transform MUST affect only downstream SSE emitted by Monoize.
3. Non-stream requests MUST remain semantically unchanged.

SSF-5. Splitting behavior:
1. The runtime MUST preserve downstream protocol correctness for Responses, Chat Completions, and Anthropic Messages SSE output.
2. The runtime MUST split oversized string-bearing delta payloads into multiple smaller downstream SSE events of the same protocol/event kind, in original order, such that concatenating the payload fragments according to the downstream protocol reconstructs the original content.
3. Eligible split targets include text deltas, reasoning deltas, reasoning signature deltas, and tool-argument deltas.
4. The runtime MUST NOT split inside a JSON string literal by inserting raw SSE line breaks into serialized JSON text.

SSF-6. Snapshot event handling:
1. If a Responses synthetic stream event contains a full snapshot object whose serialized payload would exceed `max_frame_length` because it duplicates content already emitted in delta events, the runtime MAY replace large text-bearing fields in that snapshot with protocol-valid empty values.
2. When such sanitization occurs, the downstream stream MUST remain reconstructable from the emitted delta events and terminal events.
3. Lifecycle events that are already within `max_frame_length` MUST be emitted unchanged.

SSF-7. Failure handling:
1. If `max_frame_length` is too small to encode even the minimal protocol wrapper for a required event, the runtime MAY emit the minimal unsplit event for that wrapper rather than fail the entire request.
2. The transform MUST preserve event order.
3. The transform MUST NOT change `usage`, `finish_reason`, `call_id`, `name`, `phase`, or role metadata except for emptying duplicated large snapshot strings under SSF-6.

### 4.5 `plaintext_reasoning_to_summary`

PRTS-1. Phase: `response` only.

PRTS-2. Config MUST be an empty object.

PRTS-3. Response behavior:
1. The transform MUST inspect only `Part::Reasoning` parts.
2. If a reasoning part carries `encrypted != None`, the transform MUST leave that part unchanged.
3. If a reasoning part carries plaintext reasoning in `content` and `encrypted == None`, the transform MUST move the plaintext value into `summary` and clear `content`.
4. If a reasoning part already has `summary`, the transform MUST replace `summary` with the plaintext `content` value when rule PRTS-3.3 applies.
5. Empty plaintext content MUST NOT create a non-empty summary.

PRTS-4. Streaming behavior:
1. For `UrpStreamEvent::Delta` reasoning deltas that correspond to non-encrypted reasoning, the transform MUST mark the delta as summary reasoning by setting `extra_body.reasoning_delta_type = "summary"`.
2. If a reasoning delta carries an encrypted signature in stream `extra_body.signature`, the transform MUST treat that reasoning part as encrypted and MUST NOT mark subsequent deltas for that part as summary deltas.
3. For `PartDone.part` and `ResponseDone.outputs`, the transform MUST apply the same plaintext-to-summary rewrite defined by PRTS-3.

### 4.6 `assistant_markdown_images_to_output`

AMIO-1. Phase: `response` only.

AMIO-2. Config MUST be an empty object.

AMIO-3. Extraction target:
1. The transform MUST inspect only assistant `Item::Message` items.
2. Within those items, the transform MUST inspect only `Part::Text` parts.
3. The transform MUST recognize Markdown image syntax of the form `![alt](url)`.
4. The transform MUST accept both ordinary URLs and `data:image/<subtype>;base64,<payload>` URLs.

AMIO-4. Extraction behavior:
1. Each recognized Markdown image block MUST be removed from the originating text content.
2. Each recognized block MUST be converted into a `Part::Image` output part.
3. Ordinary URLs MUST become `ImageSource::Url { url, detail: None }`.
4. `data:image/...;base64,...` URLs MUST become `ImageSource::Base64 { media_type, data }`.
5. Non-image `data:` URLs and malformed Markdown image blocks MUST remain in text unchanged.
6. Extracted image parts MUST be inserted immediately after the originating text part, preserving original order.
7. If removing Markdown image blocks leaves a text part empty, the transform MAY remove that empty text part.

AMIO-5. Streaming behavior:
1. For streaming requests executed through the response-transform fallback path defined in PIPE-1a, the transformed final `UrpResponse` MUST emit downstream text deltas for cleaned assistant text and MUST emit downstream image parts for extracted Markdown image blocks.
2. For direct `UrpStreamEvent` application, the transform MUST update at least `ItemDone.item` and `ResponseDone.outputs` so the authoritative streamed response state contains the cleaned text and the new image parts.

### 4.7 `assistant_output_images_to_markdown`

AOIM-1. Phase: `response` only.

AOIM-2. Config MAY contain:
- `template` (string, optional): string template used for each image appended to assistant text. Supported placeholders are `{{src}}`, `{{url}}`, `{{media_type}}`, and `{{data}}`.

AOIM-3. Default formatting:
1. If `template` is absent and the image source is `ImageSource::Url`, the transform MUST render `![image]({url})`.
2. If `template` is absent and the image source is `ImageSource::Base64`, the transform MUST render `![image](data:{media_type};base64,{data})`.

AOIM-4. Response behavior:
1. The transform MUST inspect only assistant `Item::Message` items.
2. For each `Part::Image` in such a message, the transform MUST format one Markdown image string according to AOIM-2 or AOIM-3.
3. The transform MUST append the formatted Markdown strings to the end of assistant text output.
4. If the assistant message already contains at least one `Part::Text`, the transform MUST append the formatted Markdown strings to the final text part in that message.
5. If the assistant message contains no text part, the transform MUST create a new trailing `Part::Text` containing the formatted Markdown strings.
6. The transform MUST NOT remove or rewrite the original `Part::Image` parts.

AOIM-5. Streaming behavior:
1. For streaming requests executed through the synthetic-stream response-transform path, the transformed final `UrpResponse` MUST produce downstream text deltas that include the appended Markdown image strings.
2. For direct `UrpStreamEvent` application, the transform MUST update at least `ItemDone.item` and `ResponseDone.outputs` so the authoritative final streamed response state includes the appended Markdown image strings.

### 4.1 `reasoning_effort_to_model_suffix`

REMS-1. Phase: `request` only.

REMS-2. Config MUST contain `rules`: a non-empty ordered array of objects, each with:
- `pattern`: model glob pattern (same syntax as TF-4 model matching).
- `suffix`: string template. The literal substring `{effort}` MUST be replaced with the resolved effort value (`low`, `medium`, or `high`).

REMS-3. On apply:
1. Read `req.reasoning.effort`. If absent or not one of `low`/`medium`/`high`, skip (no-op).
2. Iterate `rules` in order. For the first rule whose `pattern` matches `req.model`, expand `{effort}` in `suffix` and append the result to `req.model`.
3. First match wins; remaining rules are not evaluated.

REMS-4. The transform MUST NOT modify `req.reasoning`. Effort-stripping is a separate concern handled by other transforms or the upstream adapter.

## 5. Routing + Transform Pipeline

PIPE-1. Non-stream and stream request lifecycle MUST execute as:

1. Decode downstream wire payload to URP.
2. Apply API-key request-phase transforms.
3. Resolve model suffix (§5.1).
4. Route to provider/channel using waterfall + fail-forward.
5. Set `urp.model` to the resolved upstream model (via `redirect` or requested model).
6. Apply provider request-phase transforms (transforms MAY modify `urp.model`).
7. Encode URP to upstream wire payload using `urp.model` as the upstream model name, and send.
8. Decode upstream response/stream to URP.
9. Apply provider response-phase transforms.
10. Apply API-key response-phase transforms.
11. Encode URP to downstream wire response using the original requested model name.

PIPE-1b. Model identity split:
- The upstream model name sent to the provider is `urp.model` after step 6 (post-transform).
- Billing, logging, and downstream response `model` field MUST use the original requested model name (pre-step-5).

PIPE-1c. Transform rule model matching:
- Define the normalized logical model as follows:
  1. Start from the original requested model name from downstream.
  2. If that requested model matches a configured reasoning suffix under §5.1 and stripping the suffix yields an existing provider model, use the stripped base model.
  3. Otherwise use the original requested model name unchanged.
- API-key request-phase transform rules with `models` filters MUST match against the normalized logical model.
- Provider request-phase transform rules with `models` filters MUST match against the normalized logical model, even though `urp.model` may already hold the redirected upstream model at execution time.
- Response-phase transform rules with `models` filters MUST match against the normalized logical model.

### 5.1 Model Suffix Resolution

MSUF-1. After API-key request transforms and before routing, the runtime MUST attempt model suffix resolution.

MSUF-2. If the requested model exists in at least one enabled provider's model table, suffix resolution is a no-op.

MSUF-3. If the requested model does NOT exist in any provider, the runtime MUST attempt to strip a known suffix from the model name:
- Built-in effort suffixes: `-none`, `-minimum`, `-low`, `-medium`, `-high`, `-xhigh`, `-max` (where `-max` maps to `xhigh`).
- System settings `reasoning_suffix_map`: user-defined suffix→effort mappings (e.g. `{"-thinking": "high"}`).

MSUF-4. Settings suffixes MUST take priority over built-in suffixes. Among settings suffixes, longer suffixes MUST be tried before shorter ones.

MSUF-5. On successful suffix match (base model exists in at least one provider):
- `urp.model` MUST be rewritten to the base model (suffix stripped).
- `urp.reasoning.effort` MUST be set to the effort value mapped by the suffix.

MSUF-6. If no suffix match yields an existing base model, `urp` MUST remain unchanged.

PIPE-1a. Stream transform fallback:
- precondition: request is streaming, and at least one enabled response-phase transform rule matches.
- behavior: runtime calls upstream non-stream endpoint for the selected attempt, decodes to `UrpResponse`, applies response transforms, then emits synthesized downstream stream events from transformed response.
- postcondition: transformed content is visible on stream path even when upstream native stream is bypassed.

PIPE-2. API key policy MUST support:
- `max_multiplier` default routing constraint
- ordered transform rules

PIPE-3. Provider config MUST support ordered transform rules.

PIPE-4. If request max multiplier is absent, router MUST use API key max multiplier when configured.

## 6. Routing and Health

ROUT-1. Provider list MUST be evaluated in configured order.

ROUT-2. Provider filter conditions:
- `provider.enabled = true`
- requested model exists in provider model table
- multiplier constraint satisfied

ROUT-3. Channel candidate conditions:
- `channel.enabled = true`
- `channel.weight > 0`
- runtime health is healthy (or probing-eligible after cooldown)

ROUT-4. Intra-provider retry behavior:
- `max_retries = -1` => try all eligible channels
- else try `min(max_retries + 1, eligible_count)`

ROUT-5. Retryable failures (`429`, `5xx`, timeout/network) MUST advance to next channel.

ROUT-6. Non-retryable failures (`400`,`401`,`403`,`422`) MUST return immediately.

ROUT-7. Provider exhaustion MUST fail-forward to next provider.

ROUT-8. Full exhaustion MUST return `502`.

HEALTH-1. Passive health check defaults:
- failure threshold = `3`
- cooldown seconds = `60`
- window seconds = `30`
- minimum samples = `20`
- failure-rate threshold = `0.6`
- 429 cooldown seconds = `15`

HEALTH-1a. Passive breaker effective parameters MUST be channel-scoped: channel override first, global fallback.

HEALTH-1b. Channel becomes unhealthy when either condition is met:
- consecutive transient retryable failures (`5xx`/timeout/network) reach failure threshold; or
- windowed failure rate reaches threshold with sample count meeting minimum.

HEALTH-1c. If unhealthy is triggered by retryable `429`, the 429 cooldown seconds MUST be used.

HEALTH-2. Active probe defaults:
- enabled = `true`
- interval seconds = `30`
- probe model = `null` (fallback to provider first model)
- success threshold = `1`

HEALTH-3. Runtime health states MUST include `healthy`, `unhealthy`, `probing`.
