# Groups Registry Specification

## 0. Status

- Purpose: define first-class routing groups (registry table, invariants, admin CRUD API,
  reference model, deletion cascade, and the one-time migration from the legacy opaque
  string-label model).
- Scope: `monoize_groups` table, `/api/dashboard/groups*` endpoints, group-id reference
  columns on `users`, `api_keys`, `monoize_providers`, `billing_plans`.
- Terminology: the Chinese UI term 「渠道」 maps to **providers** (`monoize_providers`) in
  Monoize's architecture; routing group eligibility lives on providers, not on
  `monoize_channels` rows. Channels do not store groups.
- Related specs: `api-key-authentication.spec.md` §4 (effective-group resolution),
  `database-provider-routing.spec.md` §3 (eligibility and ordering),
  `api-token-management.spec.md` §1.2 (API-key group fields),
  `user-billing-and-model-metadata.spec.md` (user group field),
  `billing-plan-subscriptions.spec.md` (plan group ceiling),
  `channel-management.spec.md` (provider group field),
  `dashboard-ui-layout.spec.md` §11 (group management UI and shared selector).

## 1. Data model

Table `monoize_groups`:

| Column            | Type    | Constraints                                                             |
|-------------------|---------|-------------------------------------------------------------------------|
| `id`              | TEXT    | PRIMARY KEY, UUID v4 string, immutable                                  |
| `name`            | TEXT    | NOT NULL; display name; 1..64 chars after trimming; unique after `lower(trim(name))` |
| `description`     | TEXT    | NOT NULL, default `''`; 0..256 chars after trimming                     |
| `is_default`      | INTEGER | NOT NULL, `0` or `1`, default `0`                                       |
| `user_selectable` | INTEGER | NOT NULL, `0` or `1`, default `0`                                       |
| `sort_order`      | INTEGER | NOT NULL, default `0`                                                   |
| `billing_ratio`   | TEXT    | NOT NULL, default `'1'`; decimal string `>= 0` per `model-pricing.spec.md` MP-U1 |
| `created_at`      | TEXT    | NOT NULL, RFC 3339 UTC                                                  |
| `updated_at`      | TEXT    | NOT NULL, RFC 3339 UTC                                                  |

GR-D1. Group identity is `id`. All references from other tables store `id` values, never
names. Renaming a group MUST NOT change any reference.

GR-D2. Exactly one row MUST have `is_default = 1` at all times after migration
`m20260825_000042_groups_registry` has run. This row is called the **default group**.

GR-D3. The default group is renameable: `name` and `description` edits are allowed like any
other group. `is_default` is not editable through the API.

GR-D4. `user_selectable = 1` means a non-admin user may attach this group to their own API
keys (see `api-token-management.spec.md` TM-GRP-5). It has no other runtime meaning.

GR-D5. `sort_order` defines the system default ordering. The canonical registry order is
`sort_order ASC, created_at ASC, id ASC`. Every list read MUST return this order.

GR-D8. `billing_ratio` multiplies the final charge of every request whose billing group
is this group (`model-pricing.spec.md` §5). The value MUST be a decimal string that
satisfies `model-pricing.spec.md` MP-U1 and `>= 0`. `0` makes requests billed through
this group free. A write path MUST parse, compare, persist, and return the value
without converting it through `f32` or `f64`. Read responses return the canonical
decimal form without exponent notation or trailing fractional zeroes. The column is
added by migration `m20260826_000048_model_prices` with default `'1'` for every
existing row.

### 1.1 Reference columns

| Table               | Column           | Type    | Meaning                                                        |
|---------------------|------------------|---------|----------------------------------------------------------------|
| `users`             | `group_id`       | TEXT NOT NULL, default `''` | The user's single group (single-select)    |
| `api_keys`          | `use_user_group` | INTEGER NOT NULL, `0`/`1`, default `1` | Key inherits the owner's group |
| `api_keys`          | `group_ids`      | TEXT NOT NULL, default `'[]'` | Ordered JSON array of group ids          |
| `monoize_providers` | `group_ids`      | TEXT NOT NULL | JSON array of group ids the provider serves; length >= 1 |
| `billing_plans`     | `group_ids`      | TEXT NOT NULL, default `'[]'` | JSON array of group ids; `[]` = unrestricted ceiling |

GR-D6. Referential integrity is enforced by write paths (validation on every mutation) and
by the deletion cascade (§4), not by SQL foreign keys.

GR-D7. The legacy columns `users.allowed_groups`, `api_keys.allowed_groups`,
`api_keys.token_group`, `monoize_providers.groups`, and `billing_plans.allowed_groups` are
removed by migration `m20260825_000042_groups_registry`. No API surface, entity, or store
may read or write them after that migration.

### 1.2 Group-id list canonicalization

GR-C1. Every write path that accepts a group-id list MUST canonicalize it by trimming each
element, removing empty strings after trimming, and deduplicating while **preserving first
occurrence order**. Group-id lists MUST NOT be lowercased or sorted; ids are opaque and
order is significant for API keys.

GR-C2. A canonicalized group-id list longer than 32 entries MUST be rejected with HTTP `400`
and code `invalid_request`.

GR-C3. Every element of a canonicalized group-id list MUST equal the `id` of an existing
`monoize_groups` row at write time; otherwise the mutation MUST be rejected with HTTP `400`
and code `invalid_request`.

GR-C4. Stored `group_ids` values MUST decode as JSON arrays of strings. Absent, null, empty
string, or serialized empty array decode as `[]`. Any other malformed JSON, non-array value,
or non-string element MUST fail the read with a storage error; it MUST NOT decode as `[]`.

## 2. Dashboard API

### 2.1 List groups

- Endpoint: `GET /api/dashboard/groups`
- Authorization: any authenticated dashboard session.
- Response: `{ "groups": Group[] }` where `Group` is:

```json
{
  "id": "3e9c…",
  "name": "default",
  "description": "",
  "is_default": true,
  "user_selectable": true,
  "sort_order": 0,
  "billing_ratio": "1",
  "created_at": "2026-08-25T00:00:00Z",
  "updated_at": "2026-08-25T00:00:00Z"
}
```

GR-A1. The list MUST contain every registry row in the canonical order of GR-D5 for every
authenticated caller, admin or not. Group names and descriptions are not confidential; the
legacy suggestions endpoint already exposed all labels to any authenticated session.

GR-A2. The endpoint is read-only and MUST NOT create or modify rows.

### 2.2 Create group

- Endpoint: `POST /api/dashboard/groups`
- Authorization: admin (`role` is `admin` or `super_admin`).
- Request body: `{ "name": string, "description"?: string, "user_selectable"?: boolean, "sort_order"?: integer, "billing_ratio"?: string }`
  with defaults `description = ""`, `user_selectable = false`, `sort_order = 0`,
  `billing_ratio = "1"`.
- Response: `201` + created `Group` object with server-generated UUID v4 `id` and
  `is_default = false`.
- A `billing_ratio` that violates GR-D8 MUST be rejected with HTTP `400` code
  `invalid_request`.

GR-A3. `name` MUST be trimmed; the trimmed value MUST be 1..64 characters, else HTTP `400`
code `invalid_group_name`. `description` MUST be trimmed; the trimmed value MUST be at most
256 characters, else HTTP `400` code `invalid_group_description`.

GR-A4. If another row exists whose `lower(trim(name))` equals the new name's
`lower(trim(name))`, the request MUST be rejected with HTTP `409` code `group_name_exists`.

### 2.3 Update group

- Endpoint: `PUT /api/dashboard/groups/{group_id}`
- Authorization: admin.
- Request body: partial; each of `name`, `description`, `user_selectable`, `sort_order`,
  `billing_ratio` is optional and, when present, replaces the stored value. Omitted
  fields are unchanged.
- Response: `200` + updated `Group` object.
- Errors: `404 not_found` for an unknown id; GR-A3/GR-A4 apply to present fields (name
  uniqueness compares against every other row); a `billing_ratio` violating GR-D8 is
  rejected with HTTP `400` code `invalid_request`.

GR-A5. `is_default` MUST NOT be changeable through this endpoint; a request body containing
`is_default` MUST be treated as if the field were absent.

GR-A6. A successful update MUST set `updated_at` to the current time and MUST invalidate
the process-local API-key authentication cache (group semantics are embedded in cached
authentication results only as ids, but name changes must not serve stale reads elsewhere;
invalidation cost is accepted).

### 2.4 Delete group

- Endpoint: `DELETE /api/dashboard/groups/{group_id}`
- Authorization: admin.
- Response: `{ "success": true }`.
- Errors: `404 not_found` for an unknown id; HTTP `400` code `cannot_delete_default_group`
  when the target row has `is_default = 1`; HTTP `409` code `group_required_by_plan` when deletion would leave a billing plan without an eligible group.

GR-A7. Because the default group cannot be deleted, the registry always contains at least
one row and GR-D2 cannot be violated by deletion.

### 2.5 Reorder groups

- Endpoint: `POST /api/dashboard/groups/reorder`
- Authorization: admin.
- Request body: `{ "group_ids": string[] }`.
- Response: `{ "success": true }`.

GR-A8. `group_ids` MUST contain every current group id exactly once. A duplicate id, an
unknown id, a missing current id, or more than 199 ids MUST return HTTP `400` with code
`invalid_request`. Validation MUST complete before any `sort_order` value changes.

GR-A9. For a valid request, the group at zero-based array index `i` MUST receive
`sort_order = i`. All `sort_order` and `updated_at` writes MUST execute atomically in one
database transaction, and every row MUST receive the same `updated_at` value. A successful
reorder MUST invalidate the process-local API-key authentication cache.

## 3. Deletion cascade

Deleting a non-default group `X` MUST apply all of the following in one database
transaction:

GR-X1. Every `users` row with `group_id = X.id` is set to the default group's id.

GR-X2. Every `api_keys` row whose `group_ids` array contains `X.id` has that element
removed, preserving the order of the remaining elements. If the resulting array is empty
and the row has `use_user_group = 0`, the row is additionally set to `use_user_group = 1`.

GR-X3. Every `monoize_providers` row whose `group_ids` array contains `X.id` has that
element removed. If the resulting array is empty, it is replaced by `[default_group_id]`.

GR-X4. Every `billing_plans` row whose `group_ids` array contains `X.id` has that element
removed. If any result would be empty, the transaction MUST fail with HTTP `409` code `group_required_by_plan` and delete nothing.

GR-X5. Rows whose stored `group_ids` value fails GR-C4 decoding MUST abort the transaction
with a storage error; the cascade MUST NOT silently repair or skip corrupt rows.

GR-X6. After commit, the process MUST invalidate the API-key authentication cache and bump
the routing config revision so in-flight affinity bindings re-validate against the new
provider group sets.

## 4. Migration `m20260825_000042_groups_registry`

The migration MUST run identically on SQLite and PostgreSQL and MUST be a pure schema/data
migration (no network, no settings reads).

GM-1. Create `monoize_groups` per §1 with a unique index on `lower(name)`
(`uq_monoize_groups_name_lower`).

GM-2. Insert the default group: fresh UUID v4 `id`, `name = 'default'`, `description = ''`,
`is_default = 1`, `user_selectable = 1`, `sort_order = 0`, timestamps = migration time.

GM-3. Collect legacy labels from all four legacy columns (`monoize_providers.groups`,
`users.allowed_groups`, `api_keys.allowed_groups`, `billing_plans.allowed_groups`). Each
stored value is decoded as a JSON string array; absent, null, empty, whitespace-only,
malformed JSON, or non-string-array values contribute zero labels (the migration MUST NOT
fail on corrupt legacy values). Labels are canonicalized exactly like the legacy runtime:
trim, lowercase, drop empties, deduplicate.

GM-4. For every distinct canonical label except `default`, insert one group row: fresh UUID
v4 `id`, `name = label`, `description = ''`, `is_default = 0`, `user_selectable = 0`, and
`sort_order` = 1-based position of the label in ascending label order. The label `default`
maps to the GM-2 row instead of creating a new row. The resulting label→id mapping is used
by GM-5..GM-8.

GM-5. `users`: add `group_id` TEXT. For each row, decode legacy `allowed_groups` per GM-3:
an empty label set maps to the default group id; a non-empty set maps to the id of its
**alphabetically first** label (legacy arrays were stored canonically sorted, so this is the
stored first element). Set the column NOT NULL semantics via backfill of every row, then
drop `allowed_groups`.

GM-6. `api_keys`: add `use_user_group` INTEGER NOT NULL DEFAULT 1 and `group_ids` TEXT NOT
NULL DEFAULT `'[]'`. For each row, decode legacy `allowed_groups` per GM-3: an empty label
set maps to `use_user_group = 1, group_ids = '[]'` (legacy empty meant "inherit from
user"); a non-empty set maps to `use_user_group = 0` and `group_ids` = JSON array of mapped
ids in ascending label order. Then drop `allowed_groups` and the legacy unused
`token_group` column.

GM-7. `monoize_providers`: add `group_ids` TEXT NOT NULL DEFAULT `'[]'`. For each row,
decode legacy `groups` per GM-3: an empty label set (legacy "public" provider) maps to
`[default_group_id]`; a non-empty set maps to the mapped ids in ascending label order. Then
drop `groups`. After this step every provider row has at least one group id.

GM-8. `billing_plans`: add `group_ids` TEXT NOT NULL DEFAULT `'[]'`. For each row, decode
legacy `allowed_groups` per GM-3: the mapped ids in ascending label order (empty stays
`[]`). Then drop `allowed_groups`.

GM-9. `down()` MUST reverse the schema: recreate the legacy TEXT label columns, backfill
them with the JSON arrays of the referenced groups' `lower(trim(name))` values (users:
single-element array; api_keys with `use_user_group = 1`: `[]`; providers whose only group
is the default group: `[]`), drop the new columns, and drop `monoize_groups`.

GM-10. Running `up()` on an empty database (fresh install) MUST produce exactly one group
(the default group) and no other rows.

## 5. Post-migration invariants

GR-I1. Every `users.group_id` references an existing group (write validation + cascade).

GR-I2. Every `monoize_providers.group_ids` is non-empty. The former "public provider"
concept no longer exists; the default group plays that role. Provider create/update
requests whose canonicalized `group_ids` is empty MUST be stored as `[default_group_id]`.

GR-I3. `api_keys.use_user_group = 1` means the key's own `group_ids` is ignored at
authentication time. `use_user_group = 0` requires a non-empty stored `group_ids`
(write-path validation); if a stored row nevertheless has `use_user_group = 0` and
`group_ids = []`, authentication MUST resolve it exactly like `use_user_group = 1`.

GR-I4. There is no longer an "unrestricted" caller tier derived from empty group lists:
every API-key request resolves to a concrete ordered group-id list per
`api-key-authentication.spec.md` §4. A `null` group filter exists only for
system-originated internal traffic (request-capture replay, probes) and is never produced
by API-key authentication.
