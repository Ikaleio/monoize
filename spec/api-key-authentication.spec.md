# Forwarding API Key Authentication Specification

## 0. Status

- **Purpose:** Define bearer-token authentication for forwarding endpoints.
- **Scope:** Applies to all forwarding endpoints in `spec/unified_responses_proxy.spec.md` §2.2 (including `/api` aliases).

## 1. Token extraction

AK1. Monoize MUST extract the bearer token from the HTTP header:

- `Authorization: Bearer <token>`

AK2. If the header is missing or does not use the `Bearer ` prefix, Monoize MUST return:

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
   - the Argon2 hash `key_hash` MUST verify against the full token.
   - the referenced user MUST exist and have `enabled` true.
5. If validation succeeds:
   - Monoize MUST update `last_used_at` to the current time.
   - Monoize MUST authenticate the request with `tenant_id = user.id`.
   - Monoize MUST attach API key routing policy (`max_multiplier`, ordered `transforms`) to the authenticated context.

AKP3. If database validation fails or is skipped, Monoize MUST return:

- HTTP `401`
- error code `unauthorized`

## 4. Error response uniformity

AKE1. Authentication failures MUST NOT reveal whether a token partially matched (e.g. prefix exists but hash mismatch).

## 5. Max multiplier enforcement

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
