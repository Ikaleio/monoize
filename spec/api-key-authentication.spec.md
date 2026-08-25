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

1. Reject a token longer than 512 bytes as invalid before querying it.
2. Look up the API-key row by complete-token equality through the indexed plaintext `api_keys.key` column.
3. Read the API-key row and its owning user in the same database query.
4. Use the complete token as the in-memory authentication-cache identity. The first 12 characters MUST NOT identify a cache entry.
5. If no row exists, treat the token as invalid.
6. If a row exists, Monoize MUST validate the token as follows:
   - `enabled` MUST be true.
   - `expires_at` MUST be null or a future timestamp.
   - the stored full key value MUST equal the full token.
   - the referenced user MUST exist and have `enabled` true.
   - if an in-memory cache entry for the same complete token exists but fails cache-side validation, Monoize MUST invalidate that cache entry and continue with the database validation path in the same request.
   - if cache invalidation occurs after the database read but before cache publication, Monoize MUST discard that result and repeat database validation.
7. If validation succeeds:
   - Monoize MUST update `last_used_at` to the current time.
   - Monoize MUST authenticate the request with `tenant_id = user.id`.
   - Monoize MUST attach API key routing policy and runtime guards (`max_multiplier`, `effective_groups`, ordered `transforms`, `reasoning_envelope_enabled`) to the authenticated context.
   - The attached `transforms` value MUST already satisfy the API-key transform safety boundary in `api-token-management.spec.md` §2.4a. Stored disallowed transform rules MUST be discarded before request processing continues.

AKP3. If database validation fails or is skipped, Monoize MUST return:

- HTTP `401`
- error code `unauthorized`

## 4. Effective group resolution

Groups are first-class registry rows (`groups-registry.spec.md`). All group values in this
section are group **ids** (UUID strings), never names.

AKG1. The owning user row MUST be read as if it contains `group_id: string` (the user's
single group id). Authentication MUST NOT join the `monoize_groups` table; a dangling
`group_id` simply matches no provider downstream.

AKG2. The authenticated API key row MUST be read as if it contains
`use_user_group: boolean` and `group_ids: string[]` (ordered). Persisted `use_user_group`
MUST be integer `0` or `1`; any other value or incompatible type MUST fail authentication
with an internal storage error.

AKG2a. A stored `api_keys.group_ids` value that is absent, null, empty string, or a
serialized empty array decodes as `[]`. Every other value MUST decode as a JSON array of
strings; malformed JSON, a non-array JSON value, a non-string element, or an incompatible
database type MUST fail authentication with an internal storage error. It MUST NOT be
converted to `[]`.

AKG3. The billing-plan layer is read as `plan_group_ids: string[] | absent` from the
enabled plan referenced by the user (`billing-plan-subscriptions.spec.md` BP-R2: a
disabled or missing plan contributes no restriction). Storage decoding follows AKG2a.

AKG4. The authenticated context MUST represent request-scoped group access as
`effective_groups: string[] | null` where the array is an **ordered** list of group ids.
`null` is reserved for system-originated internal traffic (request-capture replay,
probes); API-key authentication MUST always produce a non-null array.

AKG5. Authentication MUST resolve `effective_groups` as follows:

1. `base = [user.group_id]` if `api_key.use_user_group` is true OR `api_key.group_ids == []`;
   otherwise `base = api_key.group_ids` with order preserved.
2. If `plan_group_ids` is present and non-empty, `effective_groups` = the elements of
   `base` that are members of `plan_group_ids`, in `base` order. Otherwise
   `effective_groups = base`.

AKG6. The attached array MUST be deduplicated preserving first occurrence order. Elements
MUST NOT be lowercased, sorted, or otherwise rewritten; group ids are opaque and their
order defines routing preference (`database-provider-routing.spec.md` R-GRP-2).

AKG7. Authentication MUST succeed even when `effective_groups = []` (the plan ceiling
excluded every base group). The downstream routing consequence is that zero providers are
group-eligible for that request.

## 5. Error response uniformity

AKE1. Authentication failures MUST NOT reveal whether a token partially matched an existing prefix.

AKE2. The number of database rows decoded for one invalid token MUST be bounded independently of the total API-key count.

AKE3. Authentication MUST decode the selected API-key row and owning user before publishing an authentication-cache entry. The following persisted API-key fields are required authorization-policy values:

- `model_limits`, `ip_whitelist`, `transforms`, and `model_redirects` MUST decode from JSON arrays of their declared element types;
- `enabled`, `sub_account_enabled`, `model_limits_enabled`, and `reasoning_envelope_enabled` MUST decode from integer `0` or integer `1` only;
- the owning user's `enabled` value MUST decode from integer `0` or integer `1` only;
- `request_capture_mode` MUST be absent, null, or one of `"off"`, `"capture-all"`, and `"capture-only-abnormal"`; absent or null means `"off"` as defined by `request-capture-dumps.spec.md`;
- every decoded non-empty `ip_whitelist` entry MUST be a valid IP address or CIDR network;
- every decoded `model_redirects` entry MUST satisfy the stored-rule invariants applied by API-key create and update operations.

AKE4. If any required value in AKE3 has malformed JSON, an incompatible database type, an out-of-domain integer value, an unsupported enum value, or invalid element semantics, API-key validation MUST return an internal storage error. It MUST NOT authenticate the request, publish or retain an authentication-cache entry from that database read, or record the key as used. Empty/default policy values MUST NOT be substituted for the invalid value.

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
