# Auto Cache Transforms Specification

## 0. Status

- Version: `1.0.0`
- Scope: Three request-phase transforms that automatically optimize Anthropic prompt caching by injecting `cache_control` markers and user identity fields.
- Dependency: URP Transform System (see `urp-transform-system.spec.md`, TF-1 through TF-6).

## 1. Shared Definitions

DEF-1. A **cache breakpoint** is any `Part` in `req.messages` whose `extra_body` contains a key `"cache_control"`.

DEF-2. The **cache breakpoint count** of a request is the total number of cache breakpoints across all messages and all parts.

DEF-3. The **max cache breakpoint limit** is `4`. No transform in this specification SHALL increase the cache breakpoint count beyond `4`.

DEF-4. The **Monoize username** is the string value of `req.extra_body["__monoize_username"]`, if present and non-null. This field is injected by the request handler before transforms run and stripped after transforms complete. Transforms MUST NOT assume its presence; absence means no-op for username-dependent logic.

DEF-5. The canonical cache control value is `{"type": "ephemeral"}`.

## 2. `auto_cache_user_id`

### 2.1 Registration

ACUID-1. Transform type ID: `"auto_cache_user_id"`.

ACUID-2. Phase: `Request` only.

ACUID-3. Config schema: empty object, no configuration parameters.

### 2.2 Preconditions

ACUID-4. If `req.extra_body["__monoize_username"]` is absent or not a string, the transform is a no-op.

ACUID-5. If no cache breakpoint exists anywhere in `req.messages` (i.e., cache breakpoint count = 0), the transform is a no-op.

### 2.3 Behavior

ACUID-6. **Anthropic user ID injection**: The transform MUST ensure `req.extra_body["metadata"]["user_id"]` exists. If `metadata` does not exist in `extra_body`, it MUST be created as `{"user_id": <username>}`. If `metadata` exists but `user_id` is absent, `user_id` MUST be set to `<username>`. If `user_id` already exists, it MUST NOT be overwritten.

ACUID-7. **OpenAI user field injection**: If `req.user` is `None`, it MUST be set to `<username>`. If `req.user` is already `Some(...)`, it MUST NOT be overwritten.

ACUID-8. The transform MUST NOT modify `req.messages`, `req.model`, or any `Part` content.

## 3. `auto_cache_system`

### 3.1 Registration

ACS-1. Transform type ID: `"auto_cache_system"`.

ACS-2. Phase: `Request` only.

ACS-3. Config schema: empty object, no configuration parameters.

### 3.2 Preconditions

ACS-4. If the cache breakpoint count is `>= 4`, the transform is a no-op.

ACS-5. If `req.messages` contains no message with `role == System` or `role == Developer`, the transform is a no-op.

ACS-6. If any part of the target system message (defined in ACS-7) already contains a `"cache_control"` key in its `extra_body`, the transform is a no-op.

### 3.3 Behavior

ACS-7. The **target message** is the last message in `req.messages` whose role is `System` or `Developer` (searched via reverse-position scan).

ACS-8. The transform MUST insert `"cache_control": {"type": "ephemeral"}` into the `extra_body` of the last `Part` of the target message.

ACS-9. After insertion, the cache breakpoint count increases by exactly `1`.

ACS-10. The transform MUST NOT modify any other message, any part content (text, image data, etc.), `req.model`, or `req.user`.

## 4. `auto_cache_tool_use`

### 4.1 Registration

ACTU-1. Transform type ID: `"auto_cache_tool_use"`.

ACTU-2. Phase: `Request` only.

ACTU-3. Config schema: empty object, no configuration parameters.

### 4.2 Preconditions

ACTU-4. Let `last_msg` = the last element of `req.messages`. If `last_msg.role != Tool` AND `last_msg` contains no `Part::ToolResult`, the transform is a no-op. (The request is not a tool-result submission.)

ACTU-5. If the cache breakpoint count is `>= 4`, the transform is a no-op.

### 4.3 Target Resolution

ACTU-6. Starting from `last_msg` and scanning backwards through `req.messages`:
1. Skip contiguous trailing messages that are either `role == Tool` or contain any `Part::ToolResult`.
2. The first non-skipped message MUST be `role == Assistant` with at least one `Part::ToolCall`. If this condition is not met, the transform is a no-op.
3. Let `assistant_idx` = the index of this Assistant message.

ACTU-7. Scan backwards from `assistant_idx - 1` to find the first message with `role == User`. Let `user_idx` = that index. If no such message exists, the transform is a no-op.

ACTU-8. If any part of `req.messages[user_idx]` already contains a `"cache_control"` key in its `extra_body`, the transform is a no-op.

### 4.4 Behavior

ACTU-9. The transform MUST insert `"cache_control": {"type": "ephemeral"}` into the `extra_body` of the last `Part` of `req.messages[user_idx]`.

ACTU-10. After insertion, the cache breakpoint count increases by exactly `1`.

ACTU-11. The transform MUST NOT modify any other message, any part content, `req.model`, or `req.user`.

## 5. Context Injection Lifecycle

CTX-1. Before API-key request-phase transforms execute, the request handler MUST inject `req.extra_body["__monoize_username"]` from `auth.username` when `auth.username` is `Some(...)`.

CTX-2. After all request-phase transforms complete (both API-key and provider scopes), the request handler MUST remove `req.extra_body["__monoize_username"]` to prevent leaking internal fields to upstream providers.

CTX-3. `auth.username` is populated from `User.username` during API key authentication. If authentication does not resolve to a user record, `auth.username` is `None`.

## 6. Transform Ordering Guidance

ORD-1. `auto_cache_system` SHOULD be ordered before `auto_cache_tool_use` in the transform rule list, so that system prompt caching takes priority when approaching the 4-breakpoint limit.

ORD-2. `auto_cache_user_id` has no ordering dependency relative to the other two transforms; it does not consume cache breakpoints.

## 7. Invariants

INV-1. No transform in this specification shall produce a request with cache breakpoint count exceeding `4`.

INV-2. No transform in this specification shall overwrite an existing `cache_control`, `metadata.user_id`, or `req.user` value.

INV-3. All three transforms are idempotent: applying the same transform twice to the same request produces the same result as applying it once.
