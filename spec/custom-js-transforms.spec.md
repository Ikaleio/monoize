# Custom JS Transforms Specification

## 0. Status

- Version: `1.0.0`
- Product name: Monoize
- Scope: administrator-authored JavaScript custom transforms executed in an embedded
  QuickJS sandbox, their persistence, the admin CRUD API, registry exposure and
  visibility, runtime execution inside the URP v2 transform pipeline, resource bounds,
  the admin dashboard management page, and the copyable authoring skill document.
- Dependencies: `spec/urp-transform-system.spec.md` (transform interface, pipeline,
  registry endpoint), `spec/transform-config-ui.spec.md` (registry item consumption),
  `spec/dashboard-ui-layout.spec.md` (navigation and page shell),
  `spec/security-access-control.spec.md` (registry authorization SAC-1 through SAC-4).

## 1. Identity and namespace

CJS-ID-1. Every custom transform id MUST start with the fixed prefix `js:`.

CJS-ID-2. A custom transform id MUST match `^js:[a-z0-9]+(-[a-z0-9]+)*$` and MUST be at
most 64 characters long including the prefix.

CJS-ID-3. The `js:` namespace cannot collide with built-in canonical transform IDs
because `urp-transform-system.spec.md` TF-14 forbids `:` in canonical IDs. The runtime
MUST NOT register a built-in transform whose id starts with `js:`.

CJS-ID-4. Transform id canonicalization (`urp-transform-system.spec.md` TF-15 through
TF-17a) MUST leave every `js:`-prefixed id unchanged.

## 2. Source frontmatter

CJS-FM-1. A custom transform is stored as one JavaScript source string. The source MUST
begin (after optional leading whitespace) with one block comment that carries the
frontmatter. The block comment opener is `/*` or `/**`.

CJS-FM-2. Frontmatter line normalization: each line inside the block comment is stripped
of leading whitespace, then of one optional leading `*`, then of one optional following
space. Empty normalized lines are ignored.

CJS-FM-3. The first non-empty normalized line MUST be exactly `@monoize-transform`.

CJS-FM-4. Every following non-empty normalized line until the comment terminator MUST
match `key: value` where `key` is one of exactly: `id`, `name`, `description`, `author`,
`phase`, `scopes`, `visibility`. `value` is the remainder of the line after the first
`: `, trimmed. A line with any other key MUST reject the save. A duplicated key MUST
reject the save.

CJS-FM-5. Required keys are `id`, `name`, `description`, and `author`. Each required
value MUST be non-empty after trimming. Length bounds: `name` at most 100 characters,
`description` at most 500 characters, `author` at most 100 characters. `id` MUST satisfy
CJS-ID-2.

CJS-FM-6. Optional keys resolve exactly as follows:

1. `phase`: one of `request`, `response`, or `both`; default `both`. `request` maps to
   supported phases `[request]`, `response` to `[response]`, `both` to
   `[request, response]`.
2. `scopes`: a comma-separated list whose entries after trimming are members of
   `{provider, global, api_key}`; duplicates MUST reject the save; default
   `provider, global, api_key`.
3. `visibility`: one of `admin` or `user`; default `admin`.

Any other value for these keys MUST reject the save.

CJS-FM-7. Custom transform `name` and `description` are plain strings. Custom transforms
DO NOT carry localized text objects; there is no per-locale metadata for custom
transforms (contrast with `urp-transform-system.spec.md` TF-8a for built-ins).

Frontmatter example:

```js
/**
 * @monoize-transform
 * id: js:example-rewrite
 * name: Example Rewrite
 * description: Rewrites request fields for demo.
 * author: alice
 * phase: request
 * scopes: provider, global
 * visibility: user
 */
```

## 3. Save-time validation

CJS-VAL-1. Save (create or update) MUST reject a source larger than
`MONOIZE_CUSTOM_JS_SOURCE_MAX_BYTES` bytes (default `262144`).

CJS-VAL-2. Save MUST parse the frontmatter per §2 and reject the source when any §2 rule
fails. The rejection response is HTTP `400` with error code `invalid_custom_transform`
and a human-readable detail message.

CJS-VAL-3. Save MUST evaluate the full source once in a validation sandbox that applies
the same resource bounds as §7 except that `Monoize.fetch` throws
`fetch is not available during validation`. Validation MUST reject the save when:

1. evaluation throws;
2. after evaluation, the global `transform` is not a function; or
3. after evaluation, the global `configSchema` is defined but is not JSON-serializable
   to a JSON object.

CJS-VAL-4. When the global `configSchema` evaluates to a JSON object, save MUST persist
its JSON serialization as the transform's declarative config schema. When `configSchema`
is undefined, save MUST persist no schema and the registry MUST report the empty object
schema `{"type": "object", "properties": {}}`.

CJS-VAL-5. On update of transform `{id}`, the frontmatter `id` MUST equal the path id;
otherwise the save is rejected. Renaming requires delete plus create.

CJS-VAL-6. Create MUST reject a frontmatter `id` that already exists with HTTP `409`
and error code `custom_transform_exists`.

## 4. Persistence

CJS-DB-1. Custom transforms persist in table `custom_transforms` with exactly these
columns:

| column | type | constraints |
| --- | --- | --- |
| `id` | TEXT | primary key; satisfies CJS-ID-2 |
| `name` | TEXT | NOT NULL; from frontmatter |
| `description` | TEXT | NOT NULL; from frontmatter |
| `author` | TEXT | NOT NULL; from frontmatter |
| `source` | TEXT | NOT NULL; full script including frontmatter |
| `enabled` | BOOLEAN | NOT NULL; default TRUE |
| `visibility` | TEXT | NOT NULL; `admin` or `user` |
| `phases` | TEXT | NOT NULL; JSON array of `request`/`response` |
| `scopes` | TEXT | NOT NULL; JSON array of `provider`/`global`/`api_key` |
| `config_schema` | TEXT | NULL; JSON object serialization per CJS-VAL-4 |
| `created_at` | TEXT | NOT NULL; RFC 3339 UTC |
| `updated_at` | TEXT | NOT NULL; RFC 3339 UTC |

CJS-DB-2. The metadata columns `name`, `description`, `author`, `visibility`, `phases`,
`scopes`, and `config_schema` are derived from the source on every save. The source is
the single source of truth; a save MUST rewrite every derived column.

CJS-DB-3. `enabled` is store-level state toggled through the API and is not part of the
frontmatter.

## 5. Admin API

All four endpoints require an authenticated session whose user role passes the same
admin check as provider management (`require_admin`). Non-admin callers receive HTTP
`403`.

CJS-API-1. `GET /api/dashboard/custom-transforms` returns
`{ "transforms": [CustomTransform, ...] }` ordered by `id` ascending, including disabled
rows. `CustomTransform` is the JSON object
`{ id, name, description, author, source, enabled, visibility, phases, scopes,
config_schema, created_at, updated_at }` where `config_schema` is the stored object or
the CJS-VAL-4 default.

CJS-API-2. `POST /api/dashboard/custom-transforms` accepts
`{ "source": string, "enabled"?: boolean }` (default `enabled = true`), applies §3
validation, inserts the row, reloads the runtime snapshot (§6), and returns the created
`CustomTransform`.

CJS-API-3. `PUT /api/dashboard/custom-transforms/{id}` accepts
`{ "source"?: string, "enabled"?: boolean }`. When `source` is present it re-runs §3
validation (including CJS-VAL-5) and rewrites the derived columns. When `enabled` is
present it updates the flag. At least one field MUST be present; otherwise HTTP `400`.
An unknown `{id}` returns HTTP `404`. On success the endpoint reloads the runtime
snapshot and returns the updated `CustomTransform`.

CJS-API-4. `DELETE /api/dashboard/custom-transforms/{id}` deletes the row, reloads the
runtime snapshot, and returns `{ "success": true }`. An unknown `{id}` returns HTTP
`404`. Deletion MUST NOT rewrite provider, global, or API-key transform chains; rules
referencing the deleted id become no-ops per CJS-RT-3.

CJS-API-5. Every successful mutation (CJS-API-2 through CJS-API-4) MUST bump the shared
configuration epoch used by replica polling.

## 6. Runtime registration and lookup

CJS-RT-1. The process keeps an in-memory custom-transform snapshot: a map from id to the
compiled entry of every **enabled** row of `custom_transforms`. The snapshot is loaded
at startup and atomically replaced after every successful mutation (CJS-API-2 through
CJS-API-4).

CJS-RT-2. Transform resolution during rule execution is exact:

1. canonicalize the rule's transform id (TF-15);
2. when the built-in registry contains the id, use the built-in transform;
3. otherwise, when the id starts with `js:` and the snapshot contains it, use the custom
   transform;
4. otherwise apply CJS-RT-3 or CJS-RT-4.

CJS-RT-3. A rule whose transform id starts with `js:` and does not resolve in step 3 of
CJS-RT-2 (the transform is deleted or disabled) MUST be skipped as a no-op. The request
MUST NOT fail. The runtime MAY log a warning.

CJS-RT-4. A rule whose transform id does not start with `js:` and is not in the built-in
registry MUST keep the existing not-found error behavior.

CJS-RT-5. A custom transform rule executes only when the rule phase (TF-4 condition 2)
is also a member of the transform's declared phases (CJS-FM-6). A rule whose phase is
outside the declared phases MUST be skipped as a no-op.

CJS-RT-6. Custom transforms participate in provider, global, and API-key chains exactly
like built-in transforms: ordered execution (TF-3), eligibility (TF-4), and model glob
matching (TF-4a) are unchanged.

CJS-RT-7. On replicas, the configuration-epoch poll tick MUST also reload the
custom-transform snapshot when the epoch changed.

## 7. Sandbox execution model

CJS-EX-1. Each `apply()` invocation of a custom transform runs the full source in a
fresh QuickJS runtime and context on the blocking thread pool. No JS value survives
between invocations except through `ctx.state` (CJS-JS-5).

CJS-EX-2. The sandbox enforces, per invocation:

1. memory limit `MONOIZE_CUSTOM_JS_MEMORY_LIMIT_BYTES` bytes (default `33554432`);
2. stack limit `MONOIZE_CUSTOM_JS_STACK_LIMIT_BYTES` bytes (default `1048576`);
3. wall-clock budget `MONOIZE_CUSTOM_JS_TIMEOUT_MS` milliseconds (default `10000`),
   enforced through the QuickJS interrupt handler; the budget covers script evaluation,
   the `transform` call, and all host `fetch` time.

CJS-EX-3. Concurrent sandbox invocations process-wide are bounded by a semaphore with
`MONOIZE_CUSTOM_JS_MAX_CONCURRENCY` permits (default `8`).

CJS-EX-4. After evaluating the source, the runtime calls the global function
`transform(ctx)` once. A missing `transform` function is an apply error.

CJS-EX-5. Any of the following surfaces as `TransformError::Apply` for the current
request (HTTP `transform_apply_failed`) and MUST NOT terminate or corrupt the proxy
process: script throw, missing `transform` function, exceeded memory limit, exceeded
wall-clock budget, non-conforming return value (CJS-JS-7, CJS-JS-8), or a result payload
that fails URP v2 deserialization.

CJS-EX-6. The sandbox exposes no filesystem API, no process API, no module loader, no
dynamic code loading from disk, and no timer API. The only host bridges are the `ctx`
argument, `Monoize.fetch`, and the console logging functions (CJS-JS-9, CJS-JS-10).

## 8. Sandbox JS API

CJS-JS-1. The `ctx` argument passed to `transform(ctx)` is one object with exactly these
properties:

| property | type | meaning |
| --- | --- | --- |
| `phase` | `"request"` \| `"response"` | current pipeline phase (TF-4) |
| `kind` | `"request"` \| `"response"` \| `"stream"` | payload surface kind |
| `data` | object | JSON serialization of the payload (CJS-JS-2) |
| `config` | object | the rule's `config` value (TF-2) |
| `state` | object | per-request mutable state (CJS-JS-5) |
| `upstream_provider_type` | string \| null | TF-10 through TF-13 value |

CJS-JS-2. `ctx.data` is the exact serde JSON serialization of the URP v2 value:
`UrpRequest` when `kind = "request"`, `UrpResponse` when `kind = "response"`, and one
canonical `UrpStreamEvent` when `kind = "stream"`. Transform-visible surfaces are the
same surfaces defined by `urp-transform-system.spec.md` SURF-1 through SURF-10.

CJS-JS-3. The runtime writes the result back by deserializing the resulting JSON into
the same URP v2 type. A deserialization failure is an apply error (CJS-EX-5).

CJS-JS-4. Return contract for `kind = "request"` and `kind = "response"`:

1. return `undefined` → the mutated `ctx.data` becomes the payload;
2. return an object → the returned object becomes the payload;
3. return anything else (including `null` and arrays) → apply error.

CJS-JS-5. `ctx.state` starts as `{}` at the first invocation for a request and is
persisted as JSON between invocations of the same rule within the same request,
including across stream events. Mutations to `ctx.state` (or to the `state` property of
a returned besides-data carrier — not supported; only `ctx.state` persists) survive to
the next invocation. State is discarded when the request ends.

CJS-JS-6. Return contract for `kind = "stream"`:

1. return `undefined` → the mutated `ctx.data` is emitted as the single event;
2. return an object → the returned object is emitted as the single event;
3. return `null` → the current event is dropped (zero events emitted);
4. return an array of objects → each element is emitted as one event, in array order;
5. any other return value → apply error.

CJS-JS-7. Under CJS-JS-6 rules 1 through 4, every emitted object MUST deserialize as one
canonical `UrpStreamEvent`; otherwise apply error.

CJS-JS-8. A custom stream transform MUST preserve a valid canonical node lifecycle
(`urp-transform-system.spec.md` STR-10). The runtime does not validate lifecycle
invariants beyond deserialization; violating them is an authoring defect.

CJS-JS-9. Host network bridge: `Monoize.fetch(url, options?)` performs one blocking HTTP
request through the shared runtime HTTP client (`TransformRuntimeContext.http_client`)
and returns `{ status: number, headers: object, body: string }`:

1. `options.method` string, default `"GET"`;
2. `options.headers` object of string values, default `{}`;
3. `options.body` string, default absent;
4. `options.timeout_ms` integer; the effective per-call timeout is
   `min(options.timeout_ms, remaining invocation budget)`; default is the remaining
   invocation budget;
5. response `headers` uses lowercase header names; repeated headers join with `", "`;
6. a response body longer than `MONOIZE_CUSTOM_JS_FETCH_MAX_BYTES` bytes (default
   `8388608`) throws;
7. more than `MONOIZE_CUSTOM_JS_FETCH_MAX_CALLS` calls in one invocation (default `16`)
   throw;
8. network errors, timeouts, and non-UTF-8 bodies throw a JS exception whose message
   names the failure. HTTP error statuses do not throw; the caller inspects `status`.

CJS-JS-10. Host logging bridge: `console.log`, `console.info`, `console.warn`, and
`console.error` are provided and forward their arguments (JSON-stringified,
space-joined) to the server log at info, info, warn, and error levels under a
custom-transform log target that includes the transform id. No other `console` member is
provided.

## 9. Registry exposure and visibility

CJS-REG-1. `GET /api/dashboard/transforms/registry` is authorized by
`security-access-control.spec.md` SAC-1 through SAC-4. After that check succeeds, the
response is the built-in items (TF-8) plus one item per **enabled** custom transform,
regardless of `visibility`. Non-admin attachment of custom `js:` rules is gated by
CJS-AKV-2, not by this endpoint.

CJS-REG-2. A custom registry item carries the TF-8 fields plus two marker fields:

1. `type_id`: the custom id;
2. `name`: `{ "en": name, "zh": name }` — the plain string mirrored into both required
   locale keys (CJS-FM-7);
3. `description`: `{ "en": description, "zh": description }` — mirrored likewise;
4. `supported_phases`: the declared phases (CJS-FM-6);
5. `supported_scopes`: exactly the declared scopes (CJS-FM-6); the registry endpoint
   MUST NOT force-append `global` to custom items;
6. `config_schema`: the stored schema or the CJS-VAL-4 default;
7. `custom`: JSON `true`;
8. `visibility`: `"admin"` or `"user"`.

CJS-REG-3. Built-in registry items remain unchanged and carry neither `custom` nor
`visibility`.

CJS-REG-4. Disabled custom transforms appear in no caller's registry response.

## 10. API-key chain validation

CJS-AKV-1. For admin callers, API-key transform sanitize/validate accepts every
`js:`-prefixed rule unchanged (consistent with the existing admin bypass).

CJS-AKV-2. For non-admin callers, a `js:`-prefixed rule is allowed exactly when the id
resolves in the enabled snapshot to a custom transform where all of the following hold:

1. `visibility = user`;
2. `api_key` is in the declared scopes; and
3. the rule phase is in the declared phases.

CJS-AKV-3. A non-admin rule that fails CJS-AKV-2 is rejected by validation with the
existing `transform '<id>' is not allowed for API keys` error and filtered by
sanitization, matching the built-in allowlist behavior.

## 11. Admin UI — Custom Transforms page

CJS-UI-1. Route `/dashboard/custom-transforms` renders the Custom Transforms admin page.
The admin sidebar navigation includes it (see `dashboard-ui-layout.spec.md` DL6). The
page is admin-only in navigation; the underlying API enforces authorization
independently (§5).

CJS-UI-2. Data flow: the page fetches `GET /api/dashboard/custom-transforms` through one
shared SWR hook (`useCustomTransforms`). While loading it renders a grid of skeleton
cards (at least 3). Mutations follow the SWR optimistic pattern:

1. toggling `enabled` applies the flipped value to the cache before the request and
   rolls back on error;
2. delete removes the card from the cache before the request and rolls back on error;
3. create/update submit then revalidate;
4. every successful mutation also revalidates the transform-registry SWR key so chain
   editors observe the change without close/reopen.

CJS-UI-3. Layout: one card per custom transform in a responsive grid (1 column below
`md`, 2 columns from `md`, 3 columns from `xl`). Each card displays: the plain `name`,
the id as secondary monospace text, the `description`, the `author`, badge chips for
each declared phase and scope, a visibility badge, an `enabled` switch, an edit action,
and a delete action behind a confirmation dialog. There is no table view.

CJS-UI-4. Motion: card entrance animates opacity `0 → 1` and y-offset `12px → 0` with
the project `easeOutExpo` token and a per-card stagger of `0.05s`
(`frontend-popup-motion.spec.md` PM4 easing vocabulary). The editor opens through the
shared Dialog primitive and therefore inherits PM3 through PM7.

CJS-UI-5. Editor dialog: creating or editing opens a dialog containing a JavaScript
code editor with syntax highlighting (CodeMirror), plus exactly these toolbar actions:

1. copy — copies the current editor buffer to the clipboard;
2. format — reformats the buffer with Prettier (babel parser) in the browser and
   replaces the buffer on success; a parse failure shows an error toast and leaves the
   buffer unchanged;
3. save — submits the buffer as `source`.

A server-side validation failure keeps the dialog open with the buffer intact and shows
the server detail message. The create action pre-fills the buffer with a commented
template containing a valid frontmatter block and a `transform` function skeleton.

CJS-UI-6. The page header contains a copy-skill action that copies the canonical
authoring skill document (§12) to the clipboard and confirms with a toast.

CJS-UI-7. The empty state (zero custom transforms, load complete) renders an explicit
localized empty message plus the create action; it MUST NOT render a bare blank grid.

## 12. Copyable authoring skill

CJS-SKILL-1. The canonical skill document is the repository file
`frontend/src/skills/monoize-custom-transform-design.skill.md`. The copy-skill action
(CJS-UI-6) copies this file's content verbatim (imported as a raw asset at build time).

CJS-SKILL-2. The skill document is written in Simplified Technical English and MUST
document at least: the frontmatter format (§2), the `transform(ctx)` contract and every
`ctx` property (§8), the return contracts for all three kinds, `ctx.state` semantics,
`Monoize.fetch` and `console` bridges, the declarative `configSchema` mechanism, the
resource bounds and security constraints (§7), and design guidelines for phases, scopes,
and visibility.

CJS-SKILL-3. The skill name is `monoize-custom-transform-design` (English) with the
display title 「Monoize 自定义变换设计」/ "Monoize Custom Transform Design".

## 13. Validity summary

CJS-VALID-1. A stored custom transform always has: a valid `js:` id, parseable
frontmatter, a source that evaluated without throwing at save time, and derived metadata
columns consistent with the source.

CJS-VALID-2. A disabled or deleted custom transform never executes, never appears in any
registry response, and never fails a request that still references it.

CJS-VALID-3. The JS sandbox never reads or writes the local filesystem and reaches the
network only through `Monoize.fetch`.
