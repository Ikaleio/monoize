# Auto Cache Transforms Specification

## 0. Status

- Version: `1.3.0`
- Scope: Five request-phase `cache_*` domain transforms that automatically optimize provider prompt caching by injecting Anthropic `cache_control` markers, OpenAI prompt-cache request fields and content breakpoints, and user identity fields.
- Dependency: URP Transform System (see `urp-transform-system.spec.md`, TF-1 through TF-7b; historical IDs map to the canonical `cache_*` IDs through TF-17).

## 1. Shared Definitions

DEF-1. An **Anthropic cache breakpoint** is any `Node` in `req.input` whose `extra_body` contains a key `"cache_control"`.

DEF-2. The **Anthropic cache breakpoint count** of a request is the total number of Anthropic cache breakpoints across all nodes in `req.input`.

DEF-3. The **Anthropic max cache breakpoint limit** is `4`. No transform in this specification SHALL increase the Anthropic cache breakpoint count beyond `4`.

DEF-4. The **Monoize username** is the string value of `req.extra_body["__monoize_username"]`, if present and non-null. The **Monoize API key ID** is the string value of `req.extra_body["__monoize_api_key_id"]`, if present and non-null. These fields are injected by the request handler before transforms run and stripped after transforms complete. Transforms MUST NOT assume their presence.

DEF-5. The canonical Anthropic cache control value is `{"type": "ephemeral"}`.

DEF-6. An **OpenAI upstream request** is a request attempt whose selected upstream provider type is `responses` or `chat_completion`.

DEF-7. An **OpenAI explicit cache breakpoint** is an `extra_body` key named `"prompt_cache_breakpoint"` on either a node in `req.input` or a nested `ToolResultContent` value in a `Node::ToolResult`.

DEF-8. The **OpenAI explicit cache breakpoint count** is the total number of OpenAI explicit cache breakpoints defined by DEF-7.

DEF-9. The **OpenAI explicit cache breakpoint limit** is `4` when `req.extra_body["prompt_cache_options"]["mode"]` is the string `"explicit"`. The limit is `3` otherwise because the default implicit breakpoint consumes one of the four new cache-write slots.

DEF-10. An **OpenAI explicit-breakpoint model** is a model whose identifier contains a `/`- or `:`-delimited segment with a `gpt-<major>[.<minor>]` prefix, where `major > 5` or `major = 5` and `minor >= 6`. Missing `minor` is treated as `0`.

## 2. `cache_user_id`

### 2.1 Registration

ACUID-1. Transform type ID: `"cache_user_id"`.

ACUID-2. Phase: `Request` only.

ACUID-3. Config schema: empty object, no configuration parameters.

### 2.2 Preconditions

ACUID-4. If `req.extra_body["__monoize_username"]` is absent or not a string, the transform is a no-op.

ACUID-5. If no Anthropic cache breakpoint exists anywhere in `req.input` (i.e., Anthropic cache breakpoint count = 0), the transform is a no-op.

### 2.3 Behavior

ACUID-6. **Anthropic user ID injection**: The transform MUST ensure `req.extra_body["metadata"]["user_id"]` exists. If `metadata` does not exist in `extra_body`, it MUST be created as `{"user_id": <username>}`. If `metadata` exists but `user_id` is absent, `user_id` MUST be set to `<username>`. If `user_id` already exists, it MUST NOT be overwritten.

ACUID-7. **OpenAI user field injection**: If `req.user` is `None`, it MUST be set to `<username>`. If `req.user` is already `Some(...)`, it MUST NOT be overwritten.

ACUID-8. The transform MUST NOT modify `req.input`, `req.model`, or any node content.

## 3. `cache_anthropic_system`

### 3.1 Registration

ACS-1. Transform type ID: `"cache_anthropic_system"`.

ACS-2. Phase: `Request` only.

ACS-3. Config schema: empty object, no configuration parameters.

### 3.2 Preconditions

ACS-4. If the Anthropic cache breakpoint count is `>= 4`, the transform is a no-op.

ACS-5. If `req.input` contains no node with `role == System` or `role == Developer`, the transform is a no-op.

ACS-6. If the target node (defined in ACS-7) already contains a `"cache_control"` key in its `extra_body`, the transform is a no-op.

### 3.3 Behavior

ACS-7. The **target node** is the last node in `req.input` whose role is `System` or `Developer` (searched via reverse-position scan).

ACS-8. The transform MUST insert `"cache_control": {"type": "ephemeral"}` into the `extra_body` of the target node.

ACS-9. After insertion, the Anthropic cache breakpoint count increases by exactly `1`.

ACS-10. The transform MUST NOT modify any other node, any node content (text, image data, etc.), `req.model`, or `req.user`.

## 4. `cache_anthropic_tool_use`

### 4.1 Registration

ACTU-1. Transform type ID: `"cache_anthropic_tool_use"`.

ACTU-2. Phase: `Request` only.

ACTU-3. Config schema: empty object, no configuration parameters.

### 4.2 Preconditions

ACTU-4. Let `last_node` = the last element of `req.input`. If `last_node` is not `Node::ToolResult`, the transform is a no-op. (The request is not a tool-result submission.)

ACTU-5. If the Anthropic cache breakpoint count is `>= 4`, the transform is a no-op.

### 4.3 Target Resolution

ACTU-6. Starting from `last_node` and scanning backwards through `req.input`:
1. Skip contiguous trailing `Node::ToolResult` entries.
2. The first non-skipped node MUST be `Node::ToolCall` with assistant role. If this condition is not met, the transform is a no-op.
3. Let `tool_call_idx` = the index of this assistant tool-call node.

ACTU-7. Scan backwards from `tool_call_idx - 1` to find the first node with `role == User`. Let `user_idx` = that index. If no such node exists, the transform is a no-op.

ACTU-8. If `req.input[user_idx]` already contains a `"cache_control"` key in its `extra_body`, the transform is a no-op.

### 4.4 Behavior

ACTU-9. The transform MUST insert `"cache_control": {"type": "ephemeral"}` into the `extra_body` of `req.input[user_idx]`.

ACTU-10. After insertion, the Anthropic cache breakpoint count increases by exactly `1`.

ACTU-11. The transform MUST NOT modify any other node, any node content, `req.model`, or `req.user`.

## 5. Context Injection Lifecycle

CTX-1. Before request-phase transforms execute, the request handler MUST inject `req.extra_body["__monoize_username"]` from `auth.username` when `auth.username` is `Some(...)`.

CTX-2. Before request-phase transforms execute, the request handler MUST inject `req.extra_body["__monoize_api_key_id"]` from `auth.api_key_id` when `auth.api_key_id` is `Some(...)`.

CTX-3. After all request-phase transforms complete (provider, global, and API-key scopes), the request handler MUST remove `req.extra_body["__monoize_username"]` and `req.extra_body["__monoize_api_key_id"]` to prevent leaking internal fields to upstream providers.

CTX-4. `auth.username` is populated from `User.username` during API key authentication. If authentication does not resolve to a user record, `auth.username` is `None`.

## 6. `cache_openai_prompt`

### 6.1 Registration

ACOP-1. Transform type ID: `"cache_openai_prompt"`.

ACOP-2. Phase: `Request` only.

ACOP-3. Supported scopes are `Provider`, `Global`, and `ApiKey`.

ACOP-4. Config schema:
- `retention`: optional string, allowed values are `"24h"` and `"in_memory"`, default `"24h"`;
- `key_prefix`: optional string, default `"mzpc"`;
- `key_mode`: optional string, allowed values are `"prefix"` and `"identity"`, default `"prefix"`;
- `include_user_in_key`: optional boolean, default `false`;
- `include_full_input_in_key`: optional boolean, default `false`.

### 6.2 Preconditions

ACOP-5. If the selected upstream provider type is not `responses` and is not `chat_completion`, the transform is a no-op.

ACOP-6. If `req.extra_body["prompt_cache_key"]` exists, the transform MUST NOT overwrite it.

ACOP-7. If `req.extra_body["prompt_cache_retention"]` exists, the transform MUST NOT overwrite it.

### 6.3 Cache key material

ACOP-8. The transform MUST build a JSON object named `key_material`.

ACOP-9. If `key_mode = "prefix"`, `key_material["model"]` MUST equal `req.model` at transform execution time.

ACOP-10. If `key_mode = "prefix"`, `key_material["prefix_nodes"]` MUST be an array containing every leading node from `req.input` whose role is `System` or `Developer`, in original order. The scan MUST stop at the first node that is not a `System` or `Developer` node.

ACOP-11. If `key_mode = "prefix"`, `key_material["tools"]` MUST equal `req.tools` when `req.tools` is present. It MUST be absent when `req.tools` is absent.

ACOP-12. If `key_mode = "prefix"`, `key_material["response_format"]` MUST equal `req.response_format` when `req.response_format` is present. It MUST be absent when `req.response_format` is absent.

ACOP-13. If `key_mode = "prefix"` and `include_user_in_key = true`, `key_material["user"]` MUST equal `req.user` when `req.user` is present. If `req.user` is absent, it MUST equal `req.extra_body["__monoize_username"]` when that value is a string. If both are absent, `key_material["user"]` MUST be absent.

ACOP-14. If `key_mode = "prefix"` and `include_full_input_in_key = true`, `key_material["input"]` MUST equal `req.input` and `key_material["prefix_nodes"]` MUST be absent.

ACOP-14a. If `key_mode = "identity"`, `key_material` MUST contain exactly:
1. `username`, equal to `req.extra_body["__monoize_username"]` when that value is a string, otherwise JSON null; and
2. `api_key_id`, equal to `req.extra_body["__monoize_api_key_id"]` when that value is a string, otherwise JSON null.

ACOP-14b. If `key_mode = "identity"`, `key_material` MUST NOT include `req.model`, `req.input`, `req.tools`, `req.response_format`, `req.user`, or any node content.

### 6.4 Behavior

ACOP-15. If `req.extra_body["prompt_cache_key"]` is absent, the transform MUST serialize `key_material` using deterministic JSON object key ordering, compute xxHash3 128-bit over the serialized bytes, format the digest as 32 lowercase hexadecimal characters, and set `req.extra_body["prompt_cache_key"]` to `<key_prefix>_<digest32>`.

ACOP-16. If `req.extra_body["prompt_cache_retention"]` is absent, the transform MUST set it to the configured `retention`.

ACOP-17. The transform MUST NOT modify `req.input`, node content, `req.tools`, `req.response_format`, `req.model`, or `req.user`.

ACOP-18. The transform is idempotent.

ACOP-19. The transform does not guarantee an OpenAI cache hit. OpenAI prompt caching requires upstream eligibility, a minimum prompt size, and exact prefix compatibility as defined by OpenAI.

## 7. `cache_openai_tool_use`

### 7.1 Registration

ACOTU-1. Transform type ID: `"cache_openai_tool_use"`.

ACOTU-2. Phase: `Request` only.

ACOTU-3. Supported scopes are `Provider`, `Global`, and `ApiKey`.

ACOTU-4. Config schema: empty object, no configuration parameters.

### 7.2 Preconditions

ACOTU-5. If the selected upstream provider type is not `responses`, the transform is a no-op.

ACOTU-6. If `req.model` is not an OpenAI explicit-breakpoint model as defined by DEF-10, the transform is a no-op.

ACOTU-7. Let `last_node` be the last element of `req.input`. If `last_node` is not `Node::ToolResult`, the transform is a no-op.

ACOTU-8. If the OpenAI explicit cache breakpoint count is greater than or equal to the limit defined by DEF-9, the transform is a no-op.

### 7.3 Target Resolution

ACOTU-9. Starting from `last_node`, scan backwards through the contiguous trailing `Node::ToolResult` entries. The first node before this trailing run MUST be `Node::ToolCall`. If this condition is not met, the transform is a no-op.

ACOTU-10. A **tool-result run** is a maximal contiguous sequence of one or more `Node::ToolResult` entries whose immediately preceding node is `Node::ToolCall`. Scan all tool-result runs in reverse request order. Within each run, scan its result nodes and their `content` entries in reverse order. The first content entry in each run that satisfies one of the following conditions is that run's candidate content block:
1. every `ToolResultContent::Text`;
2. `ToolResultContent::Image` with `ImageSource::Url` or `ImageSource::Base64`;
3. `ToolResultContent::Image` with `ImageSource::FileId` whose `extra_body["_monoize_file_id_origin"]` equals `"openai"`;
4. `ToolResultContent::File` with `FileSource::Url` or `FileSource::Base64`; or
5. `ToolResultContent::File` with `FileSource::FileId` whose `extra_body["_monoize_file_id_origin"]` equals `"openai"`.

`ToolResultContent::File` with `FileSource::Text` or `FileSource::Content` and every `ToolResultContent::ProviderItem` are not eligible. A run with no eligible content block produces no candidate. If no run produces a candidate, the transform is a no-op.

ACOTU-11. Let `remaining` equal the limit defined by DEF-9 minus the current OpenAI explicit cache breakpoint count. Starting with the newest candidate and proceeding towards older candidates, select each candidate whose `extra_body` does not already contain `"prompt_cache_breakpoint"` until `remaining` candidates have been selected. A candidate that already contains the key is preserved and skipped.

### 7.4 Behavior

ACOTU-12. The transform MUST insert `"prompt_cache_breakpoint": {"mode": "explicit"}` into the `extra_body` of every selected candidate content block.

ACOTU-13. After insertion, the OpenAI explicit cache breakpoint count increases by exactly the number of selected candidates and does not exceed the limit defined by DEF-9.

ACOTU-14. The transform MUST NOT set `"prompt_cache_breakpoint"` on the outer `Node::ToolResult.extra_body`.

ACOTU-15. For a Responses request, the encoder MUST emit every selected candidate as an `input_text`, `input_image`, or `input_file` block inside the corresponding `function_call_output.output` or `custom_tool_call_output.output` array. Each emitted content block MUST contain the inserted `"prompt_cache_breakpoint"` value.

ACOTU-16. The transform MUST NOT create or modify `req.extra_body["prompt_cache_options"]`. An existing request-wide mode and TTL MUST remain unchanged.

ACOTU-17. The transform MUST NOT modify any node content, `req.model`, `req.tools`, `req.response_format`, or `req.user`.

ACOTU-18. The transform is idempotent.

## 8. Transform Ordering Guidance

ORD-1. `cache_anthropic_system` SHOULD be ordered before `cache_anthropic_tool_use` in the transform rule list, so that system prompt caching takes priority when approaching the 4-breakpoint limit.

ORD-2. `cache_user_id` has no ordering dependency relative to the other transforms; it does not consume cache breakpoints.

ORD-3. The per-attempt cross-protocol strip of nested `extra_body` (see provider setting `strip_cross_protocol_nested_extra`) MUST run BEFORE any request-phase transform (provider, global, and API-key scopes) within the same attempt. This guarantees that `cache_control` markers produced by `cache_anthropic_system` / `cache_anthropic_tool_use` on part-level `extra_body` survive into the encoded upstream request, even when the downstream and upstream protocol families differ (e.g. downstream OpenAI Responses → upstream Anthropic Messages).

ORD-4. As a consequence of ORD-3, request-phase transforms may be invoked more than once per request across multiple attempts. Each invocation operates on an independent clone of the originally-decoded URP request, so `cache_*` idempotency (INV-4) is sufficient to keep behavior deterministic; non-idempotent transforms MUST likewise produce the same result when applied once to a fresh clone, which is the only pattern exercised here.

ORD-5. `cache_openai_prompt` SHOULD run after transforms that modify the stable prompt prefix, tool definitions, or response format. This ensures the generated `prompt_cache_key` reflects the upstream request shape after those mutations.

ORD-6. `prompt_strip_anthropic_billing_header` SHOULD run before `cache_openai_prompt` when both transforms are enabled. This ensures the generated `prompt_cache_key` and the OpenAI upstream prompt omit Claude Code's per-request billing marker.

ORD-7. `cache_openai_tool_use` SHOULD run before `cache_openai_prompt` when `cache_openai_prompt.include_full_input_in_key = true`. This ensures the generated key material includes the inserted content breakpoint.

## 9. Invariants

INV-1. No transform in this specification shall produce a request whose Anthropic cache breakpoint count exceeds `4`.

INV-2. No transform in this specification shall produce a request whose OpenAI explicit cache breakpoint count exceeds the limit defined by DEF-9.

INV-3. No transform in this specification shall overwrite an existing `cache_control`, `prompt_cache_breakpoint`, `metadata.user_id`, `req.user`, `prompt_cache_key`, or `prompt_cache_retention` value.

INV-4. All five transforms are idempotent: applying the same transform twice to the same request produces the same result as applying it once.
