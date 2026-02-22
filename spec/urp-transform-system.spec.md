# Monoize URP + Transform System Specification

## 0. Status

- Version: `1.0.0`
- Product name: Monoize
- Internal protocol name: `URP`
- Scope: URP structures, decode/encode behavior, transform execution, and routing integration.

## 1. URP Core Contract

URP-1. Internal request representation MUST be `UrpRequest { model, messages, ... }`.

URP-2. URP MUST be messages-centric: request/response history is represented as an ordered array of `Message`.

URP-3. A `Message` MUST include `role` and `parts`.

URP-4. `Part` MUST support at least: `Text`, `Image`, `Audio`, `File`, `Reasoning`, `ReasoningEncrypted`, `ToolCall`, `ToolResult`, `Refusal`.

URP-5. Every URP struct that maps to JSON MUST support unknown field passthrough using flattened `extra_body`.

URP-6. Encode path MUST flatten `extra_body` entries into the output object.

URP-7. If a key exists in both a named field and `extra_body`, named field value MUST win.

URP-8. Server MUST remain stateless: no conversation persistence and no dependency on `previous_response_id`.

## 2. Decode/Encode Requirements

DEC-1. Downstream requests from `/v1/chat/completions`, `/v1/responses`, `/v1/messages` MUST decode into `UrpRequest`.

DEC-2. Unknown wire fields MUST be preserved into URP `extra_body`.

DEC-3. Tool calls MUST decode to `Part::ToolCall`.

DEC-4. Tool result messages MUST decode to `Role::Tool` + `Part::ToolResult` with sibling content parts.

DEC-5. Reasoning fields from upstream/downstream wire formats MUST decode into `Part::Reasoning` and/or `Part::ReasoningEncrypted`.

ENC-1. Upstream request construction MUST encode from URP only; transforms MUST NOT access raw wire payloads.

ENC-2. URP-to-upstream encoding MUST support provider types: `responses`, `chat_completion`, `messages`, `gemini`, `grok`.

ENC-3. History encoding rule:
- if a message contains `ReasoningEncrypted`, plaintext `Reasoning` in the same message MUST be omitted.
- otherwise `Reasoning` MAY be encoded when supported by target wire format.

ENC-4. Model rewrite MUST apply provider `models[requested].redirect` when present; otherwise use requested model.

## 3. Streaming Representation

STR-1. Internal streaming representation MUST use `UrpStreamEvent`.

STR-2. `UrpStreamEvent` MUST support: `ResponseStart`, `PartStart`, `Delta`, `PartDone`, `ResponseDone`, `Error`.

STR-3. `Delta` MUST be part-indexed and typed via `PartDelta` (text/reasoning/tool args/media/refusal variants).

STR-4. Transform engine MUST be able to process stream events incrementally with per-request mutable state.

STR-5. If a streaming request matches any enabled response-phase transform rule, the runtime MAY execute upstream in non-stream mode, apply response transforms on `UrpResponse`, and emit a synthesized downstream stream. In this mode, downstream still receives protocol-correct streaming events (`SSE` for Chat/Responses/Messages), but event timing is buffered.

## 4. Transform System

TF-1. A transform MUST implement:
- `type_id()`
- `supported_phases()`
- `config_schema()`
- `parse_config()`
- `init_state()`
- `apply()`

TF-2. `TransformRuleConfig` persisted form MUST include: `transform`, `enabled`, `models`, `phase`, `config`.

TF-3. Transform rule execution MUST be ordered; output of rule `i` is input to rule `i+1`.

TF-4. Rules MUST be filtered by:
- `enabled=true`
- matching phase
- model glob match when `models` is present

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
- `auto_cache_user_id`
- `auto_cache_system`
- `auto_cache_tool_use`

### 4.2 `append_empty_user_message`

AEUM-1. Phase: `request` only.

AEUM-2. Config: empty object (no configuration required).

AEUM-3. On apply:
1. Inspect the last element of `req.messages`.
2. If the last message has `role == assistant`, append a new `Message { role: user, parts: [] }` to `req.messages`.
3. If the last message is not `assistant`, or `messages` is empty, no-op.

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
- Billing, logging, response-phase transform matching, and downstream response `model` field MUST use the original requested model name (pre-step-5).

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
