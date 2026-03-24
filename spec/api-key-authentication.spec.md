# Forwarding API Key Authentication Specification

## 0. Status

- **Purpose:** Define forwarding API-key authentication for forwarding endpoints.
- **Scope:** Applies to all forwarding endpoints in `spec/unified_responses_proxy.spec.md` §2.2 (including `/api` aliases).

## 1. Token extraction

AK1. Monoize MUST extract the forwarding API key token from one of these HTTP headers:

- `Authorization: Bearer <token>`
- `x-api-key: <token>`

AK2. If neither header is present, or if `Authorization` is present but does not use the `Bearer ` prefix, Monoize MUST return:

- HTTP `401`
- error code `unauthorized`

## 2. Token resolution sources

Monoize MUST use one source for resolving `<token>` to `tenant_id`:

- **Database API keys** stored in the `api_keys` table (dashboard-managed).

## 3. Resolution order

AKP1. Monoize MUST attempt database API key validation first **only** if:

- the token starts with the literal prefix `sk-`, AND
- the token length is at least 12 characters.

AKP2. If AKP1 holds, Monoize MUST:

1. Set `key_prefix = token[0..12]` (first 12 characters).
2. Look up an API key row by `key_prefix`.
3. If no row exists, treat the token as invalid.
4. If a row exists, Monoize MUST validate the token as follows:
   - `enabled` MUST be true.
   - `expires_at` MUST be null or a future timestamp.
   - the stored full key value MUST equal the full token.
   - the referenced user MUST exist and have `enabled` true.
   - if an in-memory cache entry for the same `key_prefix` exists but fails cache-side validation, Monoize MUST invalidate that cache entry and continue with the database validation path in the same request.
5. If validation succeeds:
   - Monoize MUST update `last_used_at` to the current time.
   - Monoize MUST authenticate the request with `tenant_id = user.id`.
   - Monoize MUST attach API key routing policy (`max_multiplier`, `effective_groups`, ordered `transforms`) to the authenticated context.
   - The attached `transforms` value MUST already satisfy the API-key transform safety boundary in `api-token-management.spec.md` §2.4a. Stored disallowed transform rules MUST be discarded before request processing continues.

AKP3. If database validation fails or is skipped, Monoize MUST return:

- HTTP `401`
- error code `unauthorized`

## 4. Effective group resolution

AKG1. The owning user row MUST be read as if it contains `allowed_groups: string[]`. `[]` means the user grants access to all channel groups. Any write path that persists `users.allowed_groups` MUST canonicalize it by trimming each element, lowercasing, removing empty strings after trimming, deduplicating, and sorting ascending.

AKG2. For backward compatibility, if `users.allowed_groups` is absent, null, empty string, or serialized empty array, authentication MUST treat it as `[]`.

AKG3. The authenticated API key row MUST be read as if it contains `allowed_groups: string[]`. `[]` means inherit from the owning user at request-authentication time.

AKG4. The authenticated context MUST represent request-scoped group access as `effective_groups: string[] | null`. `null` means the request is unrestricted by group filtering. A non-null array means the request is restricted to the named groups in that array.

AKG5. Authentication MUST resolve `effective_groups` as follows:

1. If `user.allowed_groups == []` and `api_key.allowed_groups == []`, `effective_groups = null`.
2. If `user.allowed_groups == []` and `api_key.allowed_groups != []`, `effective_groups = api_key.allowed_groups`.
3. If `user.allowed_groups != []` and `api_key.allowed_groups == []`, `effective_groups = user.allowed_groups`.
4. If both arrays are non-empty, `effective_groups = intersection(user.allowed_groups, api_key.allowed_groups)`.

AKG6. When `effective_groups` is non-null, the attached array MUST be canonicalized: lowercase, trimmed, non-empty, deduplicated, sorted ascending.

AKG7. Authentication MUST succeed even when `effective_groups = []`. The downstream routing consequence is that only public channels are group-eligible for that request.

## 5. Error response uniformity

AKE1. Authentication failures MUST NOT reveal whether a token partially matched (e.g. prefix exists but hash mismatch).

## 6. Max multiplier enforcement

AKM1. The effective `max_multiplier` for a request is resolved as follows:

1. Let `ceiling` = API key's stored `max_multiplier` (may be null).
2. Let `requested` = the first defined value from:
   - `max_multiplier` field in the request body `extra`, OR
   - `X-Max-Multiplier` HTTP header parsed as a finite positive float.
3. Resolution:
   - If both `ceiling` and `requested` are present: `effective = min(requested, ceiling)`.
   - If only `ceiling` is present: `effective = ceiling`.
   - If only `requested` is present: `effective = requested`.
   - If neither is present: `effective = null` (no multiplier filtering).

AKM2. Consequence: a per-request `requested` value can only lower the effective multiplier below the API key ceiling, never raise it above.

AKM3. During provider selection, if `effective` is not null, providers whose model entry `multiplier` exceeds `effective` MUST be excluded from the candidate set.

## 7. Model allowlist enforcement

AKL1. If `model_limits_enabled = true` and `model_limits` is non-empty on the authenticated API key, every forwarding request MUST be rejected unless the logical model requested by the client is an exact member of `model_limits`.

AKL2. AKL1 enforcement MUST occur on forwarding endpoints themselves, not only on `/v1/models` listing responses.

AKL3. Requests rejected by AKL1 MUST return HTTP `403` with code `model_not_allowed`.
