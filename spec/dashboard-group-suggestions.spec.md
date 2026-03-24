# Dashboard Group Suggestions Specification

## 0. Status

- Purpose: define the read-only dashboard endpoint that suggests existing group labels.
- Scope: `GET /api/dashboard/groups`.

## 1. Endpoint

- Method/Path: `GET /api/dashboard/groups`
- Authorization: any authenticated dashboard session.
- Response body shape: `{ "groups": string[] }`.

## 2. Data sources

DG-1. The response `groups` array MUST be derived from the union of values stored in:

- `monoize_channels.groups`
- `users.allowed_groups`
- `api_keys.allowed_groups`

DG-2. Each stored column value MUST be read as a JSON TEXT array of strings.

DG-3. If a stored value is absent, null-equivalent in the query layer, empty string, whitespace-only string, malformed JSON, or a serialized empty array, the endpoint MUST treat that row as contributing zero labels.

## 3. Canonicalization and ordering

DG-4. Every contributed label MUST be canonicalized by trimming leading/trailing whitespace, lowercasing, removing empty strings after trimming, deduplicating, and sorting ascending.

DG-5. The endpoint MUST return the sorted unique union after canonicalization.

DG-6. The endpoint is read-only. It MUST NOT create, update, delete, cache, or otherwise persist any registry of groups.
