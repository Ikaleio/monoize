# Request Logs Specification

## 0. Status

- **Purpose:** Record and expose per-request metadata for API-key-authenticated and classified internal proxy requests.
- **Scope:** Applies to all forwarding endpoints (responses, chat completions, messages, embeddings) and the dashboard request-logs API.

## 1. Data model

### 1.1 Request log row

A request log row has:

- `id: string` (UUID)
- `request_id: string` (the `x-request-id` header assigned by tower-http `SetRequestIdLayer`)
- `user_id: string` (the historical authenticated user identifier captured when the request was admitted)
- `api_key_id: string?`
- `model: string` (logical model requested by the client)
- `provider_id: string?`
- `upstream_model: string?`
- `channel_id: string?` (the channel that ultimately served the request)
- `is_stream: boolean`
- `input_tokens: integer?`
- `output_tokens: integer?`
- `cache_read_tokens: integer?`
- `cache_creation_tokens: integer?`
- `tool_prompt_tokens: integer?`
- `reasoning_tokens: integer?`
- `accepted_prediction_tokens: integer?`
- `rejected_prediction_tokens: integer?`
- `provider_multiplier: string?` (canonical positive base-10 decimal string)
- `charge_nano_usd: string?` (nano-dollar integer string)
- `status: string` (`"pending"`, `"success"`, `"error"`, or `"client_gone"`)
- `usage_breakdown_json: object?` (normalized per-request usage detail snapshot; persisted as JSON text in DB)
- `billing_breakdown_json: object?` (per-request pricing and charge breakdown snapshot at billing time; persisted as JSON text in DB)
- `error_code: string?` (error code for failed requests, e.g. `upstream_error`)
- `error_message: string?` (error message for failed requests; for upstream-derived failures this is the full internal detail per `upstream-error-sanitization.spec.md` SAN-9 — the truncated raw upstream text, which MAY differ from the downstream client message and MAY contain raw upstream URLs, domains, IPv4 addresses, and `api_key:` values. Read-time disclosure to dashboard viewers follows RL-API14.)
- `error_http_status: integer?` (HTTP status returned to downstream client for failed requests)
- `duration_ms: integer?` (wall-clock time from request start to upstream response)
- `ttfb_ms: integer?` (time from request start to first byte/chunk from upstream; null for non-streaming)
- `request_ip: string?` (the server-generated canonical client IP for the request)
- `reasoning_effort: string?` (the selected reasoning-effort label when present)
- `tried_providers_json: object[]?` (array of failed upstream attempts in chronological order; persisted as JSON text in DB; null when no upstream attempt failed). Each object has:
  - `attempt_number: integer` (>= 1)
  - `provider_id: string`
  - `channel_id: string`
  - `provider_name: string` (Provider display name at attempt time)
  - `channel_name: string` (Channel display name at attempt time)
  - `error: string`
  - `duration_ms: integer?` (wall-clock milliseconds of that failed attempt)
  - `upstream_status: integer?`
  - `upstream_code: string?`
  - `upstream_type: string?`
  - `upstream_param: string?`
  Historical rows MAY omit `provider_name`, `channel_name`, `attempt_number`, and the `upstream_*` fields.
- `request_kind: string?` (classification of log source; null for normal API-key client requests, `"playground"` for dashboard Playground requests, and `"active_probe_connectivity"` for active health-probe model probes)
- `effective_provider_type: string?` (effective upstream type used for the selected attempt; null when no attempt was selected)
- `affinity_hit: boolean?` (true when request routing used an eligible affinity binding; false when affinity was evaluated but no binding was used; null when affinity did not run)
- `affinity_key_hash: string?` (short hash of the affinity cache key; raw affinity key material MUST NOT be stored)
- `affinity_target: string?` (`provider_id/channel_id` for the affinity target when present)
- `session_affinity_value: string?` (the exact `x-session-affinity` header value sent to the upstream when per-channel automatic session affinity produced one, per `channel-management.spec.md` CM-AFF-4; null when disabled or no value was produced)
- `created_at: RFC3339 string`
- `created_at_unix_ms: integer?` (the same creation instant as Unix epoch milliseconds; nullable only for legacy rows whose text timestamp could not be backfilled)

### 1.2 Enriched fields (computed at query time, not stored)

When returning request log rows via the dashboard API, the following fields are JOINed from related tables:

- `username: string?` (from `users.username` via `user_id`; null after that user is deleted)
- `api_key_name: string?` (from `api_keys.name` via `api_key_id`)
- `channel_name: string?` (from `monoize_channels.name` via `channel_id`)
- `provider_name: string?` (from `monoize_providers.name` via `provider_id`)
- For each `tried_providers` hop whose `provider_name` is missing or empty, dashboard list responses MUST set `provider_name` from the current `monoize_providers.name` for that `provider_id` when the row exists.
- For each `tried_providers` hop whose `channel_name` is missing or empty, dashboard list responses MUST set `channel_name` from the current `monoize_channels.name` for that `channel_id` when the row exists.
- A deleted Provider or Channel MUST leave that hop name absent.

## 2. Recording rules

RL1. For every API-key-authenticated proxy request (`user_id` is present), the system MUST create exactly one lifecycle request-log row.

RL1a. The lifecycle row MUST be accumulated in memory during request processing. No database row is written until terminal state. The row MUST be submitted as a single INSERT with all fields (status, usage, billing, provider metadata) populated at terminal state. The INSERT is submitted to a write batcher (see `db-performance-tuning.spec.md` §2 RequestLogBatcher) and is NOT guaranteed to be persisted synchronously.

RL1a-1. The server MUST broadcast an in-memory request-log snapshot with `status = "pending"` to the request-log SSE stream as soon as request processing begins. This SSE-only snapshot MUST NOT create or update any database row.

RL1a-2. When provider/channel metadata for an in-flight request becomes known, the server SHOULD broadcast an updated in-memory `pending` snapshot for the same `request_id`. When the terminal `success`, `client_gone`, or `error` row is later broadcast, clients MUST treat it as replacing any earlier `pending` snapshot with the same `request_id`.

RL1a-3. The server MUST maintain an in-memory map of current SSE-only `pending` snapshots keyed by `request_id`. Creating or updating a `pending` snapshot MUST upsert that key before broadcasting the snapshot. Enqueuing a terminal `success` or `error` row with the same `request_id` MUST remove that key from the map before broadcasting the terminal row. The map is process-local and starts empty after process startup.

RL1a-4. Pending and terminal snapshots MUST include `effective_provider_type`, `affinity_hit`, `affinity_key_hash`, `affinity_target`, and `session_affinity_value` when those values are known.

RL1b. The lifecycle row MUST transition from `"pending"` to exactly one terminal status:

- `"success"` when the downstream client received a normal API response payload (including truncated/cutoff completion cases such as `finish_reason = "length"`),
- `"client_gone"` when the downstream HTTP client disconnected after admission and the upstream attempt completed as a billable success (see RL1h),
- `"error"` only when the request ends with an API error response.

RL1b-1. The only exception to RL1b for an already-delivered normal streaming response is a post-response billing settlement failure. That lifecycle row MUST use `status = "error"` with `error_code = "billing_settlement_failed"`. No additional terminal status value is introduced.

RL1b-1a. A `billing_settlement_failed` terminal row MUST still include the usage snapshot, scalar token fields, and timing fields that were available when settlement was attempted (`input_tokens`, `output_tokens`, related detail counters, `usage_breakdown_json`, `duration_ms`, and `ttfb_ms` when known). It MUST NOT store those fields as null solely because charging failed. `charge_nano_usd` and `billing_breakdown_json` MAY be null on that row.

RL1c. Terminal logging MUST enqueue exactly one new row with all fields populated (including terminal status, usage, billing, and provider metadata) into the request-log write batcher. There is no preceding pending row to update. Enqueue succeeds only after a durable bounded spool file exists. An abrupt process termination after successful enqueue MUST NOT lose the spooled entry.

RL1c-2. The write batcher MUST assign a stable database row ID when an entry is enqueued. If transaction begin, any insert, or commit reports failure, the complete drained batch MUST be returned to the front of the buffer for retry. Retrying the same stable row ID MUST be idempotent so an ambiguous commit outcome cannot create duplicate rows.

RL1c-3. Requeuing a failed batch MUST preserve its original order ahead of entries enqueued while the flush was in progress. A failed flush MUST NOT silently discard any entry.

RL1c-4. If the durable request-log spool cannot accept a terminal row because its byte quota, per-entry quota, or filesystem write failed, terminal-log enqueue MUST return an error to the request-finalization path. The path MUST expose a fail-closed signal and MUST NOT silently report durable billing-log success.

RL1c-0. Enqueuing a terminal row into the request-log write batcher MUST immediately broadcast that terminal row to the request-log SSE stream. SSE visibility of terminal lifecycle transitions MUST NOT wait for the later batch-flush tick, database transaction begin, database commit, or any other write-behind persistence step.

RL1c-1. The `created_at` value persisted for a terminal row MUST equal the wall-clock time at which request processing began, not the later terminal-finalization time and not the later write-batcher flush time.

RL1d. Creating or updating `pending` status MUST NOT trigger any extra billing call. Request billing execution count MUST remain identical to pre-pending behavior (at most once per billable request outcome).

RL1d-1. For a billed request, the terminal request-log row and the corresponding `request_charge` or `api_key_charge` ledger row MUST carry the same non-empty `request_id`. These writes are not required to share one transaction; `request_id` is the reconciliation key when either asynchronous write is absent after an abrupt process failure.

RL1e. When all provider attempts are exhausted (including the case where zero attempts exist), the pending row MUST still transition to `"error"`. The absence of a `last_failed_attempt` MUST NOT prevent finalization.

RL1f. On server startup, all request-log rows with `status = "pending"` MUST be transitioned to `status = "error"` with `error_code = "server_shutdown"` and `error_message = "interrupted by server restart"`. This cleanup MUST execute before the HTTP listener begins accepting connections.

RL1g. On receipt of SIGINT or SIGTERM, the server MUST initiate graceful shutdown: set the process-local background-shutdown flag, stop accepting new connections, allow in-flight requests to drain, wait for all tracked request-log and active-probe work to finish, flush all write batchers (including the request-log batcher), then transition any remaining `"pending"` rows (legacy) to `"error"` with the same fields as RL1f before process exit.

RL1h. A downstream client disconnect MUST NOT cancel in-flight upstream work. After admission, Monoize MUST keep the forwarding task alive independently of the downstream HTTP connection: it MUST continue dispatching or consuming the upstream request until one of the L2/L2.1 terminal conditions in `user-billing-and-model-metadata.spec.md` holds. Encoded bytes that can no longer be delivered MAY be discarded. If that upstream attempt completes as a billable success, billing MUST execute normally on the accumulated or terminal upstream usage and the request log MUST finalize as `status = "client_gone"` with `error_code = "client_gone"`, `error_message = "client disconnected"`, and `error_http_status = 499`. If the upstream attempt fails as an API error, the request log MUST finalize as `status = "error"` with that upstream error (not as a local 500). `"client_gone"` is a billable terminal status and MUST NOT be treated as a server fault.

RL1i. When a provider attempt is selected (upstream call succeeds or streaming begins), the provider metadata (`provider_id`, `channel_id`, `upstream_model`, `provider_multiplier`) MUST be captured in memory and included in the terminal INSERT. No intermediate database write is performed.

RL1j. For every dashboard-managed API-key request that will generate a terminal request log, Monoize MUST reserve durable request-log spool admission after authentication succeeds and before it dispatches an HTTP request upstream, opens an upstream WebSocket, or commits any upstream request headers. If admission is unavailable, Monoize MUST emit an `ERROR` log with message `request-log spool admission failed`, stage `reserve`, the canonical `request_id`, and the concrete admission error. Monoize MUST then return HTTP `503` with code `request_log_spool_unavailable` and MUST NOT dispatch or partially dispatch the request upstream.

RL1j-1. After duplicate-request-id admission succeeds and before any action listed by RL1j, Monoize MUST arm the reservation with a durable terminal fallback containing the canonical `request_id`, authenticated `user_id` and `api_key_id`, requested model, stream flag, and captured creation time. An arm failure MUST emit an `ERROR` log with message `request-log spool admission failed`, stage `arm`, the canonical `request_id`, and the concrete arm error. It MUST then remove that lifecycle by lifecycle identity, return HTTP `503` with code `request_log_spool_unavailable`, and perform no upstream dispatch. A duplicate-request-id rejection MUST drop only an unarmed probe and MUST NOT create a recoverable fallback row.

RL1j-2. If a forwarding handler reaches request-log admission without a canonical request ID, Monoize MUST emit an `ERROR` log with message `request-log spool admission failed`, stage `request_id`, request ID `<missing>`, and error `request id missing before request-log spool admission`. Monoize MUST then return HTTP `503` with code `request_log_spool_unavailable` and MUST NOT dispatch or partially dispatch the request upstream.

RL1k. Before forwarding code observes `x-request-id`, Monoize MUST derive one canonical value by trimming its leading and trailing ASCII whitespace. Admission lookup, the pending-snapshot map key, pending and terminal snapshot `request_id`, billing reconciliation, and the terminal database row MUST use that canonical value. An absent, invalid, or whitespace-only incoming value MUST be replaced with a generated UUID before forwarding code observes it.

RL1k-1. At most one in-flight dashboard-managed request may own a given non-empty canonical `request_id`. Admission for a second in-flight request with the same canonical `request_id` MUST fail before upstream dispatch with HTTP `409` and code `duplicate_request_id`.

RL1k-2. One request-log lifecycle owns one preflight `RequestLogReservation` and one atomic `terminal_scheduled` state. Exactly one explicit terminal scheduler or guard fallback MAY change `terminal_scheduled` from false to true. Every losing scheduler MUST perform no terminal enqueue. The winning scheduler MUST enqueue with the lifecycle's original reservation; it MUST NOT reserve replacement capacity by looking up an unnormalized key or by using the unreserved terminal API.

RL1k-3. The lifecycle admission and pending snapshot MUST remain present until the winning terminal task has durably enqueued its row. After successful enqueue, `RequestLogBatcher` removes the pending snapshot before the lifecycle removes its admission entry. Admission removal MUST compare lifecycle identity while the map entry is locked. A terminal task from an older lifecycle MUST NOT remove a later lifecycle after canonical key reuse.

RL1k-4. Dropping a request guard while `terminal_scheduled = false` MUST atomically win terminal scheduling and enqueue one terminal row from the latest pending snapshot. Guard drop MUST NOT silently release the reservation or delete the pending snapshot. The fallback classification is:

- If the dropping thread is panicking, the row MUST use `status = "error"`, `error_code = "request_finalization_aborted"`, `error_message = "request ended before terminal log scheduling"`, and `error_http_status = 500`.
- Otherwise the row MUST use `status = "client_gone"`, `error_code = "client_gone"`, `error_message = "client disconnected"`, and `error_http_status = 499`. This path is the last-resort marker when the downstream connection ended before an explicit terminal scheduler ran; it MUST NOT be used when RL1h's detached forwarding task can still complete and bill.

The durable preflight arm written at admission (crash-recovery fallback) MUST keep `status = "error"`, `error_code = "request_finalization_aborted"`, `error_message = "request ended before terminal log scheduling"`, and `error_http_status = 500`, because an abrupt process death is a server interruption, not a client disconnect.

RL2. Requests authenticated only by static config keys MUST NOT generate request logs.

RL3. Terminal log finalization (`pending -> success/error`) MUST be fire-and-forget (spawned asynchronously) and MUST NOT block the response to the client. Admission MUST register the lifecycle with a process-local terminal-task tracker before upstream dispatch. The tracker count MUST reach zero only after every registered lifecycle's terminal task has returned.

RL3b. SIGINT or SIGTERM handling MUST only signal the HTTP server to stop accepting new work. After HTTP graceful drain completes, shutdown MUST wait until the terminal-task tracker count is zero, then flush all write batchers, then transition legacy database rows that still have `status = "pending"`. Shutdown MUST NOT flush or clean legacy pending rows from the signal future before HTTP drain.

RL3c. `AppState` MUST contain one process-local background-shutdown flag initialized to false. SIGINT or SIGTERM handling MUST set the flag to true before HTTP graceful drain begins. The active-probe scheduler MUST register one task with the same tracker used by request-log lifecycles before the scheduler task is spawned. The scheduler MUST check the flag before each scheduler iteration and before each channel probe dispatch. After the flag becomes true, the scheduler MUST dispatch no new probe. A probe already dispatched MUST finish its upstream call, terminal persistence, and health-state update before the scheduler completes its tracker registration. Shutdown MUST wait for that completion before the final batcher flush.

RL3a. *(Removed — pending row creation is no longer performed. See RL1a for the in-memory accumulation pattern.)*

RL4. For non-streaming requests, the log MUST include token usage from the upstream response. `ttfb_ms` MUST be null.

RL5. For streaming requests where response transforms require buffering (synthetic stream), the log MUST include token usage. `ttfb_ms` MUST record the time from `started_at` to the point where the upstream response body is received.

RL6. For pass-through streaming requests, `ttfb_ms` MUST record the time from `started_at` to the point where the first chunk is received from upstream.

RL6a. For pass-through streaming requests where usage cannot be extracted from streamed events, token usage fields MAY be omitted (set to null).

RL6b. For pass-through streaming requests, usage fields (`input_tokens`, `output_tokens`, `cached_tokens`, `reasoning_tokens`, `cache_creation_tokens`, `tool_prompt_tokens`, `accepted_prediction_tokens`, `rejected_prediction_tokens`, `usage_breakdown_json`) MUST be accumulated in memory via `StreamRuntimeMetrics` during streaming. No incremental database updates are performed. The final cumulative usage snapshot is included in the terminal INSERT (see RL1a).

RL6c. *(Removed — no incremental pending updates exist. Usage is written once at terminal state per RL6b.)*

RL6d. For pass-through streaming requests that finalize successfully without a usage snapshot, Monoize MUST emit an observability warning containing the request identifier plus the in-memory terminal-stream diagnostics collected during adaptation. The warning payload MUST include whether a literal upstream `[DONE]` sentinel was observed, the last terminal event classification seen by the adapter, the terminal finish reason when the upstream protocol exposes one, and whether Monoize synthesized its own terminal chunk before closing the downstream stream.

RL6e. For pass-through streaming requests, an upstream in-stream terminal error event is an API error response even when the upstream HTTP status is `200`. This includes OpenAI Responses SSE events named `error` and `response.failed`. Monoize MUST forward the protocol-correct downstream terminal error event, MUST finalize the request log with `status = "error"`, MUST leave `charge_nano_usd` and `billing_breakdown_json` null, and MUST populate `error_code`, `error_message`, and `error_http_status`. For OpenAI Responses `response.failed`, `error_code` MUST equal `response.error.code` when present, `error_message` MUST equal `response.error.message` when present, and `error_http_status` MUST be `400` unless the upstream stream exposes a more specific non-2xx status.

RL6f. *(Removed — the stored visible-output TPS basis (`first_visible_output_ms`, `last_visible_output_ms`, `visible_generation_ms`, `visible_output_tokens`, `tps_mode`) is deleted, including rules RL6f-1 through RL6f-6. Output throughput is now computed at display time exclusively from `output_tokens` (or `usage_breakdown_json.output.total_tokens`), `duration_ms`, `ttfb_ms`, and `is_stream` per FL4a. The request-log write path MUST NOT record per-delta visible-output timestamps. See RL-S12 for the column drop migration.)*

RL7. The `duration_ms` field MUST measure wall-clock time from the start of request processing (after auth) to the point where the upstream response is received.

RL8. The `request_id` field MUST be populated from the `x-request-id` header set by the tower-http middleware.

RL9. The `request_ip` field MUST equal the canonical client IP generated by the server's client-IP middleware. When `MONOIZE_TRUSTED_PROXY_CIDRS` is absent, the middleware MUST trust only `127.0.0.0/8` and `::1/128`. When the variable is present, including with an empty value, the middleware MUST trust exactly its configured entries. The middleware MUST use the socket peer IP unless that peer matches the effective trusted-proxy list. Only for a trusted proxy peer may the middleware parse `Forwarded`, then `X-Forwarded-For`, then `X-Real-IP`, according to the canonical client-IP rules. A forwarding header supplied by an untrusted peer MUST NOT affect `request_ip`.

RL10. The `channel_id` field MUST record the ID of the channel that ultimately served the request.

RL11. For non-streaming requests and synthetic-stream requests (where usage is available and billing is executed), `charge_nano_usd` in `request_logs` MUST equal the computed request charge persisted by the billing subsystem for the same request.

RL12. For pass-through streaming requests where usage is unavailable and billing is skipped, `charge_nano_usd` MAY be null.

RL13. For pass-through streaming requests where usage is extracted from streamed events, the log MUST persist extracted usage fields and `charge_nano_usd` MUST equal the computed request charge persisted by the billing subsystem for that request.

RL14. For failed requests (`status = "error"`):

- `charge_nano_usd` MUST be null.
- `billing_breakdown_json` MUST be null.
- `error_code` and `error_message` MUST be populated when available.
- `error_http_status` MUST store the HTTP status returned to the client.

RL15. For successful requests where usage exists, `usage_breakdown_json` MUST persist a request-time snapshot of usage details. The snapshot MUST include `input.total_tokens` and `output.total_tokens`, and SHOULD include subtype token counts when present (for example: cached, cache creation/read, reasoning, audio, image, text).

RL15b. If normalized usage contains an authoritative cached-input modality split, `usage_breakdown_json.input` SHOULD include the corresponding `cached_text_tokens`, `cached_image_tokens`, `cached_audio_tokens`, `cached_video_tokens`, or `cached_document_tokens` fields when present.

RL15a. `usage_breakdown_json.input.total_tokens` MUST be the aggregate/inclusive prompt token total as defined in `user-billing-and-model-metadata.spec.md` § 5 C3 — i.e. it MUST include cache-read tokens and cache-creation tokens. `usage_breakdown_json.input.uncached_tokens` MUST equal `input.total_tokens - cached_tokens - cache_creation_tokens` clamped at zero (the base-rate billable bucket). These fields MUST be computed uniformly across all upstream provider types, because upstream usage is normalized at decode time per C3-ii of the billing spec. Provider-type branching in usage-breakdown construction MUST NOT exist.

RL16. For successful requests where billing is executed, `billing_breakdown_json` MUST persist the version-3 settlement snapshot defined by `model-pricing.spec.md` §8 (MP-B1). New settlements MUST NOT write any other version.

RL16a. Version-2 snapshots persisted before the model-prices cutover remain readable in stored request logs (MP-B6). A stored version-2 snapshot contains:

- `token_line_items: array`
- `tool_line_items: array`
- `unpriced_tool_classes: array`
- `applied_tier_index: integer | null`
- `service_tier: string | null`
- `group_billing_ratio: string`
- `channel_multiplier: string`
- `base_charge_nano: string`
- `final_charge_nano: string`

RL16b. Each stored version-2 token line item contains `usage_class`, `unit`, `unit_price_nano`, `quantity`, `charge_nano`, and any selected dimension fields among `context_tier`, `service_tier`, `modality`, and `cache_ttl`.

RL16c. The request-log viewer MUST render line items, tiers, multipliers, and charges for both version 2 and version 3 snapshots. For version 3 it MUST additionally render `free_reason` markers, non-empty `unpriced_tool_classes`, and `group_billing_ratio` when it differs from `"1"`.

RL17. When a request triggers waterfall fail-forward, `tried_providers_json` MUST record each failed upstream attempt. This rule applies to every upstream error class. Each persisted entry MUST contain `attempt_number`, `provider_id`, `channel_id`, `provider_name`, `channel_name`, and `error`. `error` MUST equal the attempt's unmasked, truncated internal detail per `upstream-error-sanitization.spec.md` SAN-5/SAN-10; the attempt's client-facing `client_error` text MUST NOT be persisted. It MUST also persist `duration_ms`, `upstream_status`, `upstream_code`, `upstream_type`, and `upstream_param` when those values exist on the failed attempt. `duration_ms` MUST equal the wall-clock milliseconds from the start of that upstream attempt to the failure. `provider_name` and `channel_name` MUST equal the Provider and Channel display names at attempt time. The array MUST be ordered chronologically. When no upstream attempt failed, the field MUST be null.

RL17a. `GET /api/dashboard/request-logs` MUST return `tried_providers` as that JSON array, or null. For each hop, if `provider_name` is missing or empty, the handler MUST set it from the current `monoize_providers` row for `provider_id` when that row exists. If `channel_name` is missing or empty, the handler MUST set it from the current `monoize_channels` row for `channel_id` when that row exists. In-memory SSE snapshots already contain write-time names and MUST NOT require this fill.

RL18. Successful active probe connectivity tests that can incur upstream token cost MUST be persisted as request logs with `request_kind = "active_probe_connectivity"`. Failed active probe connectivity tests MUST NOT be persisted as request logs.

RL18a. When a successful active probe returns both prompt and completion token counts, its charge calculation MUST read the `model_prices` row selected by pricing-key normalization (`model-pricing.spec.md` §3) of the redirected upstream model, falling back to the logical model key. It MUST NOT read token prices from `model_metadata_records`. A disabled or incomplete row is exactly a missing row (MP-R2/MP-R4).

RL18b. A priced probe MUST settle through the `model-pricing.spec.md` §6 settlement with `group_billing_ratio = 1`, the selected Channel model entry `multiplier`, an empty `tool_prices` object, and no requested tool classes. All arithmetic MUST use checked exact decimal and integer operations (MP-C1).

RL18c. A successful active probe with missing usage, no matching price row, or a settlement error MUST still persist its connectivity log with `charge_nano_usd = null` and `billing_breakdown_json = null`. It MUST NOT substitute zero for a missing or failed calculation. A successful calculated probe snapshot MUST be a version-3 breakdown (MP-B1) whose `pricing_model_key` is the RL18a key.

RL18d. Monoize MUST resolve the active-probe system user ID once during process startup and reuse that ID for every probe log. The system user MUST have unlimited balance before startup completes. If the user cannot be read, created, or changed to unlimited balance, startup MUST fail with `active_probe_user_init_failed`. One scheduler tick MUST NOT execute a system-user query per Provider, Channel, or probe.

RL18e. Before each active probe that can produce a successful request log dispatches any upstream bytes, the scheduler MUST reserve durable request-log spool capacity. If reservation fails, the scheduler MUST skip that probe, MUST NOT dispatch it upstream, and MUST NOT update its channel health state.

RL18f. Before dispatch, an active probe MUST arm its reservation with a terminal fallback whose `request_kind = "active_probe_connectivity"` and `error_code = "active_probe_interrupted"`. A completed failed active probe MUST explicitly cancel that armed reservation and MUST NOT persist a request log. A successful active probe MUST enqueue its terminal row with that exact reservation and MUST await successful durable spool enqueue before the scheduler proceeds. It MUST NOT acquire a replacement reservation after upstream dispatch. An abrupt process exit after arming and before either outcome operation MUST leave the fallback recoverable.

RL18g. When the process-local background-shutdown flag becomes true, the active-probe scheduler MUST stop before the next probe dispatch. A probe already dispatched MUST complete the RL18f transition before the scheduler exits. The final shutdown flush MUST occur only after the scheduler's shared task-tracker registration has completed.

RL19. For active probe logs, `api_key_id` MUST be null and the UI token column label MUST be rendered as a localized "Model Probe" string.

RL20. Dashboard Playground forwarding requests MUST use the normal request-log lifecycle,
MUST persist `request_kind = "playground"`, and MUST persist `api_key_id = null`. The UI
token column label for these rows MUST be rendered as a localized "Playground" string.
Neither internal-source label may be exposed through the API-key management endpoints.

## 3. Dashboard endpoint

### 3.1 List request logs

- **Endpoint:** `GET /api/dashboard/request-logs`
- **Authorization:** Any authenticated dashboard user.
- **Query parameters:**
  - `limit: integer` (default 50, clamped to [1, 200])
  - `offset: integer` (default 0, clamped to >= 0)
  - `model: string?` (filter by model name; supports comma-separated list for multi-model OR matching, e.g. `"gpt-4o, gpt-5"`. Each entry is trimmed and matched as a case-insensitive literal substring. `%`, `_`, and `\` in an entry are ordinary characters, not LIKE syntax.)
  - `status: string?` (filter by status, exact match: `"pending"`, `"success"`, `"error"`, or `"client_gone"`)
  - `api_key_id: string?` (filter by specific API key ID)
  - `username: string?` (filter by username, exact match via JOIN on `users.username`; only effective when the caller has admin role — non-admin callers ignore this parameter)
  - `search: string?` (case-insensitive literal-substring search across model, upstream_model, request_id, request_ip; `%`, `_`, and `\` are ordinary characters)
  - `time_from: string?` (ISO 8601 / RFC 3339 timestamp; inclusive lower bound on `created_at`)
  - `time_to: string?` (ISO 8601 / RFC 3339 timestamp; exclusive upper bound on `created_at`)
- **Response:**

```json
{
  "data": EnrichedRequestLogRow[],
  "total": integer,
  "total_charge_nano_usd": string,
  "limit": integer,
  "offset": integer
}
```

Where `EnrichedRequestLogRow` = `RequestLogRow` + `username` + `api_key_name` + `channel_name` + `provider_name` + `has_capture`.

RL-API14. Every `EnrichedRequestLogRow` MUST include `has_capture: boolean` computed per `request-capture-viewer.spec.md` RCV-L1/RCV-L2: `true` iff at least one `request_capture_records` row matches the row's `(request_id, user_id)`, computed with an indexed `EXISTS` subquery. The list path MUST NOT open, stat, or read dump files. SSE-delivered rows carry `has_capture: false` (RCV-L3).

RL-API6. `total_charge_nano_usd` MUST equal the SUM of `charge_nano_usd` across all rows matching the active filters (not just the current page). Rows with null `charge_nano_usd` MUST be treated as 0. The value MUST be a string representation of a non-negative integer (nano-dollar).

RL-API1. When the authenticated user has role `super_admin` or `admin`, the endpoint MUST return logs for ALL users. Otherwise, it MUST return only logs belonging to the current authenticated user.

RL-API2. Results MUST be ordered newest first using the complete ordering in RL-API10.

RL-API3. `total` MUST reflect the count of logs matching all active filters, not the page size.

RL-API4. Filter parameters are combined with AND logic.

RL-API5. For admin users applying `username` filter, rows with `request_kind = "active_probe_connectivity"` MUST remain included regardless of username value.

RL-API7. A present `time_from` or `time_to` value MUST parse as RFC 3339. If either value is malformed, the endpoint MUST return HTTP `400`, code `invalid_time_filter`, and `param` equal to the malformed parameter name. The endpoint MUST perform no request-log database query for that request. When both values are present, `time_from` MUST be earlier than `time_to`; otherwise the endpoint MUST return the same HTTP `400` error with `param = "time_from"`.

RL-API8. All filter predicates MUST use a contiguous placeholder sequence. An omitted or rejected filter MUST NOT reserve a placeholder index.

RL-API9. `model` and `search` matching MUST have the same ASCII-case-insensitive literal-substring semantics on SQLite and PostgreSQL. Matching MUST fold only ASCII `A` through `Z` to `a` through `z`; every non-ASCII code point MUST remain unchanged and case-sensitive. The implementation MUST ASCII-fold the bound search text, escape `\`, `%`, and `_` before binding a LIKE pattern, and declare `\` as the LIKE escape character. SQLite MUST fold stored text with `LOWER`. PostgreSQL MUST fold stored text with an ASCII-only expression such as `translate(value, 'ABCDEFGHIJKLMNOPQRSTUVWXYZ', 'abcdefghijklmnopqrstuvwxyz')`; PostgreSQL `LOWER` is not sufficient because its non-ASCII behavior depends on database collation.

RL-API10. The page rows, `total`, and `total_charge_nano_usd` returned by one request MUST be computed from one database snapshot. PostgreSQL reads MUST use at least repeatable-read isolation. The final row order MUST be `created_at_unix_ms DESC` with nulls last, then `created_at DESC`, then `id DESC`; `id` is the unique pagination tie-breaker.

RL-API11. Exact charge totals MUST be aggregated in the database into bounded exact integer components or an exact decimal value. The server MUST NOT transfer one `charge_nano_usd` value per matching row solely to calculate a list-page total. Rust MUST reconstruct or parse the database aggregate with checked `i128` arithmetic. A syntactically canonical charge outside the signed `i128` domain, or a total outside that domain, MUST return an explicit internal storage error. Non-canonical stored charge text is ignored.

RL-API12. The effective maximum number of non-empty comma-separated `model` terms MUST be configured by `MONOIZE_REQUEST_LOG_MODEL_FILTER_MAX_TERMS`. The default and hard maximum MUST both be `32`. A trimmed base-10 integer in `[1, 32]` selects that value. An unset, empty, malformed, zero, negative, or greater-than-32 value MUST resolve to `32`. Empty comma-separated entries are discarded; every remaining entry, including a duplicate, counts as one term because it creates one SQL predicate and one bind value.

RL-API13. If a request supplies more `model` terms than the effective RL-API12 limit, `GET /api/dashboard/request-logs` MUST return HTTP `400`, code `request_log_model_filter_too_many_terms`, and `param = "model"` before executing any database query, including authentication or session lookup. Both admin and non-admin paths MUST apply the same check. The `UserStore` list methods and the SQL filter builder MUST independently reject an over-limit model filter so non-HTTP callers cannot construct an unbounded OR expression or bind list.

RL-API14. Error-detail disclosure is role-dependent (`upstream-error-sanitization.spec.md` section 8):

- When the caller's role is `admin` or `super_admin`, `GET /api/dashboard/request-logs` and `GET /api/dashboard/request-logs/stream` MUST return `error.message` and every `tried_providers[].error` exactly as stored (full raw detail, bounded only by write-time truncation).
- For any other caller, both endpoints MUST replace `error.message` with `MASK(stored text)` and each `tried_providers[].error` with `MASK(stored text)` before serialization, where `MASK` is defined by `upstream-error-sanitization.spec.md` SAN-D1. The stored row MUST NOT be modified.

### 3.2 Admin-visible vs user-visible fields

The API returns the same enriched schema for all users. The frontend controls column visibility:

- **Admin-only columns:** `username`, `channel` (display text uses `provider_name` when available, otherwise falls back to `provider_id`; tooltip shows channel name and upstream model context)
- **All users see:** `created_at`, `request_id`, `model` (with ModelBadge), `api_key_name`, `duration_ms`/`ttfb_ms`/`is_stream` (merged badge group), `input_tokens`, `output_tokens`, `charge_nano_usd`, `status`, `request_ip`, and error tooltip details (`error_code`, `error_message`, `error_http_status`) when `status = "error"`.
- For non-admin callers, the `error_message` and `tried_providers[].error` values inside the returned rows are the read-time-masked forms defined by RL-API14; admin callers receive the stored full detail. The frontend renders whichever text the API returned and performs no additional masking.

## 4. Storage

RL-S1. Request logs MUST be stored in table `request_logs`.

RL-S2. The table MUST have a persisted sortable time column `created_at_unix_ms` storing the request creation instant as Unix epoch milliseconds.

RL-S2a. The table MUST have indexes on the persisted sortable time column:
- a composite index on `(user_id, created_at_unix_ms DESC)` for per-user pagination and time-range filtering,
- an index on `(created_at_unix_ms DESC)` for global pagination, analytics range scans, and retention cleanup.
- a partial compatibility index on `(created_at)` where `created_at_unix_ms IS NULL`, so the explicit legacy-null range branch does not force a full-table scan.

RL-S2a-1. The table MUST also have a partial index `idx_request_logs_status_pending` on `(status)` where `status = 'pending'`, so the RL1f/RL3b legacy pending sweeps do not force a full-table scan. Because terminal-only inserts (RL1a) never write `status = 'pending'`, this index contains at most legacy rows.

RL-S2b. Request-log reads MUST use `created_at_unix_ms` as the primary ordering and range-filter column on both SQLite and PostgreSQL. A range predicate MUST compare `created_at_unix_ms` directly, without wrapping that indexed column in `COALESCE`, a cast, or a date function. A separate `created_at_unix_ms IS NULL AND created_at ...` branch MUST retain compatibility for legacy null rows. The legacy branch compares normalized UTC RFC 3339 text and MUST NOT change the direct indexed predicate for non-null rows.

RL-S2c. `created_at_unix_ms` MUST be backfilled for pre-existing rows during migration using the canonical `created_at` value when that value is parseable. Rows whose legacy `created_at` cannot be parsed may retain `created_at_unix_ms = NULL`.

RL-S2d. Request-log writes MUST populate both canonical time representations from the same captured request terminalization instant:
- `created_at` stores RFC3339 text,
- `created_at_unix_ms` stores the same instant as Unix epoch milliseconds.

RL-S2e. Request-log charge aggregation MUST preserve the full signed `i128` nano-dollar domain on both backends. The implementation MUST aggregate syntactically valid canonical `charge_nano_usd` text through exact database decimal arithmetic or bounded decimal limbs, then parse or reconstruct the result with checked `i128` arithmetic. It MUST NOT cast a complete canonical charge through SQLite `INTEGER`/`BIGINT`, PostgreSQL `BIGINT`, Rust `i64`, `REAL`, `DOUBLE PRECISION`, or `f64`. A canonical input outside the signed `i128` domain and aggregate overflow MUST return an explicit internal storage error.

RL-S2f. Request-log scalar token and timing columns decoded into Rust `i64` MUST use SQLite `INTEGER` and PostgreSQL `BIGINT`. Conversion from an in-memory `u64` MUST be checked. A value above `i64::MAX` MUST NOT wrap to a negative number. When the full unsigned value is also present in canonical usage JSON, the unrepresentable scalar field MUST be stored as null and the batcher MUST emit a warning without dropping the terminal row.

RL-S2g. A request-log row read MUST decode every selected database column explicitly. A database type mismatch, malformed persisted JSON, or malformed persisted multiplier MUST return an internal storage error for the read. The decoder MUST NOT replace a failed decode with null, zero, an empty string, or an empty object.

RL-S3. `request_logs.user_id` MUST store the exact authenticated user identifier captured when the request was admitted. `request_logs.user_id` MUST NOT have a foreign key to `users`. Deleting a `users` row MUST NOT delete or modify any request-log row.

RL-S3a. A terminal row from the durable request-log spool MUST remain insertable when its `user_id` no longer exists in `users`, including when the user was deleted after request admission and when a later process recovers an older spool file. A missing current user MUST NOT make request-log batch flush retry permanently.

RL-S3b. Migration `m20260809_000031_request_logs_without_user_fk` requires its input `request_logs` table to contain `id`, `user_id`, `model`, `is_stream`, `status`, and `created_at`. Every other canonical column in section 1.1 MAY be absent. The input MAY also contain non-canonical legacy columns. The output table on SQLite and PostgreSQL MUST contain exactly 42 columns — the canonical columns in section 1.1 except `session_affinity_value` (added later per RL-S4), plus the five visible-TPS columns (`first_visible_output_ms`, `last_visible_output_ms`, `visible_generation_ms`, `visible_output_tokens`, `tps_mode`) that a later migration removes per RL-S12 — and MUST contain no foreign key from `user_id` to `users`.

RL-S3b-1. For each canonical source column that exists, migration `m20260809_000031_request_logs_without_user_fk` MUST copy its value without conversion except for the three token fallback rules in RL-S3b-2. For each absent nullable canonical source column, the migration MUST create that column with the backend type defined by RL-S2f and store null for every existing row.

RL-S3b-2. The migration MUST derive the three canonical token columns per row as follows:

- `input_tokens = COALESCE(input_tokens, prompt_tokens)` when both columns exist; it equals the existing column when only one exists; it is null when neither exists.
- `output_tokens = COALESCE(output_tokens, completion_tokens)` under the same existence rule.
- `cache_read_tokens = COALESCE(cache_read_tokens, cached_tokens)` under the same existence rule.

RL-S3b-3. PostgreSQL migration MUST drop both possible user foreign-key constraint names, `request_logs_user_id_fkey` and `fk_request_logs_user_id`, with `IF EXISTS`. On both backends, the migration MUST replace every ordinary request-log index with exactly these four indexes while preserving any index owned by a table constraint:

- `idx_request_logs_user_created_at` on `(user_id, created_at_unix_ms DESC)`
- `idx_request_logs_created_at` on `(created_at_unix_ms DESC)`
- `idx_request_logs_model` on `(model)`
- `idx_request_logs_legacy_created_at` on `(created_at)` where `created_at_unix_ms IS NULL`

RL-S3c. On SQLite and PostgreSQL, every column outside the 42-column set defined in RL-S3b, including legacy `prompt_tokens`, `completion_tokens`, and `cached_tokens`, MUST be absent after migration `m20260809_000031_request_logs_without_user_fk`. These columns are not canonical storage and MUST NOT be retained as compatibility aliases. When a canonical token value and its legacy counterpart are both non-null and differ, the canonical value MUST win under RL-S3b-2.

RL-S3d. Every schema inspection, data update, table rebuild, constraint change, column change, and index change performed by migration `m20260809_000031_request_logs_without_user_fk` MUST execute in one database transaction per backend. If a required RL-S3b source column is absent or any statement fails, the original table, rows, constraints, and indexes MUST remain unchanged. Running the up migration twice against a successful output MUST leave the same rows, 42-column schema, four ordinary indexes, and no user foreign key. The down migration MUST be a no-op because an intervening request-log row may contain a deleted `user_id` that cannot satisfy a restored foreign key.

RL-S4. Outside the SQLite rebuild defined by RL-S3b, new columns (`request_id`, `channel_id`, `ttfb_ms`, `request_ip`, `usage_breakdown_json`, `billing_breakdown_json`, `error_code`, `error_message`, `error_http_status`, `tried_providers_json`, `session_affinity_value`) MUST be added via `ALTER TABLE ADD COLUMN` statements in migration logic. The RL-S3b SQLite rebuild MAY define an absent nullable canonical column directly on its replacement table. All such columns are nullable for existing rows.

RL-S6. Migration `m20260809_000031_request_logs_without_user_fk` MUST converge legacy `prompt_tokens`/`completion_tokens`/`cached_tokens` and canonical `input_tokens`/`output_tokens`/`cache_read_tokens` according to RL-S3b-2, then remove the three legacy columns. New usage detail columns (`cache_creation_tokens`, `tool_prompt_tokens`, `accepted_prediction_tokens`, `rejected_prediction_tokens`) MUST be nullable for migrated rows whose source schema did not contain those columns.

RL-S5. Outside the SQLite rebuild defined by RL-S3b, `request_kind` MUST be added as a nullable `TEXT` column via `ALTER TABLE ADD COLUMN`. The RL-S3b SQLite rebuild MAY define an absent `request_kind` column directly on its replacement table. Its value MUST be null for a source row that had no `request_kind` column.

RL-S7. If a PostgreSQL database still contains legacy shadow columns (`created_at_ts`, `is_stream_bool`, `charge_nano_usd_decimal`) from an older Monoize version, startup migration MUST drop those columns and their associated indexes without touching the canonical columns (`created_at`, `is_stream`, `charge_nano_usd`).

RL-S8. While SQLite and PostgreSQL both remain supported, request-log writes MUST target only the canonical columns present in RL-S1. The application MUST NOT create, backfill, or write PostgreSQL-only shadow columns.

RL-S9. Request-log retention MUST delete rows whose `created_at_unix_ms` is older than 90 days relative to cleanup execution time.

RL-S9a. Retention cleanup MUST delete expired rows in bounded batches. One batch is one autocommitted DELETE statement that targets at most 5000 expired rows selected by `id` through a subquery on the RL-S2a time index. Cleanup MUST repeat batches until one batch deletes fewer rows than the bound, then report the total number of deleted rows. Because each batch commits independently, a failure between batches keeps earlier batches deleted, and the writer (the SQLite write lock, PostgreSQL row locks) is released between batches so concurrent request-log flushes can interleave.

RL-S10. Expired-row cleanup defined in RL-S9 SHOULD execute once during startup before the HTTP listener begins accepting traffic. If that cleanup attempt fails, startup MAY continue and the failure MUST be logged.

RL-S11. Expired-row cleanup defined in RL-S9 MUST also execute periodically in a background task while the process is running. The default cleanup interval MUST be 1 hour.

RL-S12. Migration `m20260824_000040_drop_request_log_visible_tps` MUST drop columns `first_visible_output_ms`, `last_visible_output_ms`, `visible_generation_ms`, `visible_output_tokens`, and `tps_mode` from `request_logs` on SQLite and PostgreSQL. Each drop MUST be a no-op when that column is already absent, so running the up migration twice succeeds and leaves the same schema. The migration MUST NOT modify any other column, row, or index. The down migration MUST be a no-op because dropped visible-TPS values cannot be reconstructed.

RL-S13. Migration `m20260826_000047_request_logs_pending_status_index` MUST create the RL-S2a-1 partial index `idx_request_logs_status_pending` on `request_logs (status)` with predicate `status = 'pending'` on SQLite and PostgreSQL, using `IF NOT EXISTS` so running the up migration twice succeeds. The down migration MUST drop the index with `IF EXISTS`.

## 5. Frontend display

### 5.1 Format

FL1. The logs page MUST use a compact list format (dense table rows) with horizontal scrolling for overflow.

FL2. The `created_at` field MUST be displayed as a localized timestamp in format `YYYY-MM-DD HH:mm:ss` using the browser's local timezone.

FL2a. The `created_at` value displayed for a request row MUST remain stable across in-memory `pending` snapshots and the later terminal `success`, `client_gone`, or `error` row for the same `request_id`. Transitioning from `pending` to terminal state MUST NOT cause the displayed timestamp to jump forward.

FL3. The model column MUST use the `ModelBadge` component (same as Provider page).

FL4. The `duration_ms`, `ttfb_ms`, and `is_stream` fields MUST be merged into a single cell as a non-collapsed inline badge row.

- The row MUST render the duration badge when `duration_ms` is present.
- The row MUST render the TTFB badge when `ttfb_ms` is present.
- The row MUST render exactly one stream-mode badge (`流` or `非流`).
- When `tried_providers` is non-empty, the row MUST also render a hop-count badge whose text is the localized retry-hop count for `tried_providers.length`. This hop-count badge is not a timing-value overflow control.
- The row MUST NOT render a `+N` overflow badge for timing values.
- Timing badges MUST NOT wrap.
- The timing cell MAY use horizontal overflow when viewport space is insufficient; it MUST NOT hide badges behind a collapsed popover.

FL4b. The frontend MUST treat request-log timing values as numeric-compatible inputs. For badge rendering and tooltip math, it MUST accept canonical fields `duration_ms` and `ttfb_ms`, and it MUST also accept the compatibility aliases `durationMs`, `elapsed_ms`, or `latency_ms` (total duration) and `ttfbMs`, `first_token_ms`, or `firstTokenMs` (TTFB) when those aliases are present. String values that parse to finite numbers MUST be rendered identically to numeric values.
FL4c. Backend request-log API responses MUST include compatibility aliases for timing fields (`durationMs`, `elapsed_ms`, `latency_ms`, `ttfbMs`, `first_token_ms`, `firstTokenMs`) with values equal to canonical `duration_ms` / `ttfb_ms`, so updated frontend builds do not rely on client-side fallback only.

FL4a. Hovering, focusing, or activating the timing badge row MUST show a tooltip containing a duration detail row with the total duration and a TTFB detail row when TTFB is present. The tooltip MUST include exactly one "TPS" (output tokens per second) metric when both the TPS numerator and generation window defined by FL4a-1 and FL4a-2 are greater than zero. Activation MUST work on touch devices; activating outside the tooltip or pressing Escape MUST close it.

FL4a-1. The TPS numerator MUST be the total output token count: `usage_breakdown_json.output.total_tokens` takes precedence over scalar `output_tokens`. Reasoning tokens MUST NOT be subtracted. There is no other numerator source; when neither value is present and positive, no TPS row is rendered.

FL4a-2. The TPS generation window follows the new-api `RecordRelaySample` model. For a streaming row (`is_stream` truthy) where both `duration_ms` and `ttfb_ms` are present and `duration_ms > ttfb_ms`, the window MUST be `duration_ms - ttfb_ms` (generation time excludes time-to-first-token). Otherwise the window MUST be a positive `duration_ms` value (total latency). No other timing field participates.

FL4a-3. The UI MUST compute `TPS = numerator / (generation_window_ms / 1000)` and display it with two decimal places and the unit `t/s`, without any approximation prefix. TPS MUST NOT impose a minimum token count or minimum generation-window duration.

FL4a-4. When the numerator or generation window is absent or not greater than zero, the tooltip MUST omit the TPS row. It MUST NOT render an insufficient-sample state, a basis/mode label, or a separate generation-window row.

FL4a-5. *(Removed — the "Visible window TPS" metric and its generation-window tooltip row are deleted together with the stored visible-TPS basis fields; see RL6f and RL-S12.)*

FL4d. When `tried_providers` is non-empty, the timing tooltip MUST list those hops in stored order after the duration/TTFB/TPS rows. Each hop row MUST show the FL9a.3 label, `duration_ms` when present (same duration format as the duration badge), `upstream_status` when present, and `error`. The list MUST use the same served-terminal rule as FL9b.

FL4e. Every text badge rendered by the logs page MUST use the shared rounded-rectangle
shape (`rounded-md`). A logs-page text badge MUST NOT override that shape with
`rounded-full`. Circular status indicators are not text badges and MUST remain circular.

FL5. The `api_key_name` column header MUST be "Token" (referring to the API key name, not the literal token value).

FL6. The multiplier column from the old layout MUST be removed (multiplier is already shown inside ModelBadge on the Provider page).

FL7. The top of the page MUST include a search bar and filter controls:
  - **Model filter**: text input accepting comma-separated model names (e.g. `gpt-4o, gpt-5`); applied on Enter or blur.
  - **Status filter**: dropdown with options `All`, `Pending`, `Success`, `Error`.
  - **Token filter**: dropdown listing all of the user's API keys by name; selecting one filters by `api_key_id`.
  - **Username filter** (admin only): text input. The default value is empty, which MUST omit `username` from `GET /api/dashboard/request-logs` and therefore list every user's logs. Applied on Enter or blur. Non-admin users do not see this control.
  - **Time range filter**: dropdown with preset options `All Time`, `Last 1 Hour`, `Last 24 Hours`, `Last 7 Days`, `Last 30 Days`, `Today`, `Yesterday`, `This Month`, `Last Month`. Selecting a preset computes `time_from` / `time_to` as ISO 8601 strings in the browser's local timezone and sends them as query parameters to the API.

FL7a. The filter-control area MUST display the total charge sum for the current filter conditions. The displayed value MUST come from the most recent REST response field `total_charge_nano_usd` for the active filters. It MUST include every matching persisted row and MUST be independent of request `limit`, request `offset`, and the number of rows currently loaded by infinite scrolling. The frontend MUST NOT compute this value by summing `loadedLogs` or the current response page. The value MUST be formatted as regular USD currency with 6 fractional digits (e.g. `$1.234567`). The label MUST use the i18n key `requestLogs.totalCost`. The element MUST be displayed in the summary area (top-right) alongside the existing "Showing X-Y of Z" text.

FL7b. `/dashboard/logs` MUST read the optional `username` query parameter. For an admin viewer, a non-empty `username` value MUST initialize the username filter to that exact string. An absent or empty `username` query parameter MUST initialize the filter to empty (all users). Non-admin viewers MUST ignore the query parameter.

FL7c. The free-text search input MUST debounce fetches. A keystroke MUST NOT issue a request-log fetch directly; the client applies the current search text to the active filter set only after 300 ms have elapsed without a further keystroke. Clearing the input follows the same 300 ms rule. Local (client-side) filtering of SSE-delivered rows per FL53 MAY use the debounced value.

FL8. Column order (left to right): merged `created_at/request_id`, merged `model/[channel]`, merged `[username/]api_key_name`, `duration/ttfb/stream` (merged badges), `input_tokens` (input), `output_tokens` (output), `charge_nano_usd` (cost), `request_ip`.

FL8a. The merged `created_at/request_id` cell MUST use exactly two non-wrapping visible lines in one column. The first line MUST render `created_at` per FL2. The second line MUST render the first 8 characters of `request_id` followed immediately by the FL36 status indicator. If `request_id` is absent, the second line MUST render `-`. When `has_capture == true`, the second line MUST also render the right-aligned capture-viewer button defined by `request-capture-viewer.spec.md` RCV-F2. The request tooltip behavior defined by FL28 and FL29 MUST remain attached to the second line.

FL8b. Every request-log body row MUST have a fixed height of 44 pixels. The merged leading cells, the admin-visible merged User/Token cell, and the Input cell MUST reserve their two-line layout inside that height. Text and badges inside those lines MUST NOT wrap. Content-oriented columns MAY expand the table width, and the table viewport MUST provide horizontal scrolling when the expanded table exceeds the viewport.

FL9. The merged `model/[channel]` cell MUST use exactly two non-wrapping layout lines in one column:

- The first line MUST render the ModelBadge defined by FL3.
- For an admin viewer, the second line MUST render the first non-empty value among `channel_name`, `channel_id`, and `-`.
- For a non-admin viewer, the second line MUST remain visually empty and MUST NOT expose Provider or Channel information.
- When `affinity_hit` is true for an admin viewer, the second line MUST include a localized sticky-session badge immediately after the Channel display value. The badge MUST NOT appear when `affinity_hit` is false or null.
- Retry-chain hops MUST NOT create a third visible line. Their full path remains available through FL9b and their count remains visible through FL4.
- On hover, focus, or activate, the tooltip MUST show the content defined by FL9b. Activation MUST work on touch devices; activating outside the tooltip or pressing Escape MUST close it.

FL9a. Compact retry-chain hops:

1. Walk `tried_providers` in stored order. For each entry, hop identity is `(provider_id, channel_id)`. Skip the entry if that identity is already in the hop list.
2. If the row has a terminal Provider or Channel id, form terminal identity `(provider.id ?? "", channel.id ?? "")`. If that identity is not already in the hop list, append one hop for the terminal Provider/Channel.
3. A hop label MUST be the first non-empty value among hop `channel_name`, hop `provider_name`, hop `channel_id`, hop `provider_id`. For the terminal hop those fields are `channel.name`, `provider.name`, `channel.id`, `provider.id`.
4. Empty-label hops MUST be omitted.
5. A hop list of length less than 2 is not a retry chain. The visible merged `model/[channel]` cell remains the fixed two-line layout from FL9 regardless of hop count.

FL9b. Channel tooltip:

- If `tried_providers` is non-empty, the tooltip MUST first render a localized retry-chain heading, then one row per stored `tried_providers` entry in chronological order. Each row MUST show the hop label from FL9a.3, `duration_ms` when present, `upstream_status` when present, and `error`.
- After those failed-attempt rows, if a terminal hop identity exists and either (a) it differs from the last `tried_providers` identity, or (b) row `status` is `success` or `client_gone` and the last `tried_providers` identity equals the terminal identity, the tooltip MUST append one terminal row with the terminal hop label and a localized served marker. When `status` is `error` and the last `tried_providers` identity already equals the terminal identity, the tooltip MUST NOT append a duplicate terminal row.
- The tooltip MUST then show `provider_name` (or `provider_id` as fallback) when available, `channel_name` (or `channel_id` as fallback) when available, `affinity_target` when present, `session_affinity_value` when present, and upstream model when it differs from the requested model. `affinity_hit = false` MUST NOT render a miss label or any other standalone affinity-status row.
- When `affinity_hit = true`, the localized affinity-hit badge MUST appear immediately after the Channel value on the same tooltip row (for example, `Channel: input-im [Sticky hit]`). The tooltip MUST NOT render a separate affinity-hit row.
- The visible affinity-target value MUST NOT render the stored `provider_id/channel_id` string. The frontend MUST resolve that exact identity to human-readable Provider and Channel names from the current admin Provider list, the terminal request-log names, or the matching `tried_providers` name snapshot, in that order. When both names are available, it MUST render `<provider_name>/<channel_name>` (for example, `codex/input-im`). When the target cannot be resolved to a non-empty human-readable name, the affinity-target row MUST be omitted.

FL10. The request logs table body MUST use virtualized rendering via `react-virtuoso` (`TableVirtuoso`) instead of rendering all loaded rows as plain DOM rows.

FL11. The request logs page MUST remove explicit previous/next pagination buttons. Additional rows MUST be loaded by scroll-to-end (infinite loading).

FL12. Infinite loading MUST fetch in backend-paginated chunks using `limit=100` and `offset = loaded_row_count` semantics, and MUST stop requesting when `loaded_row_count >= total`.

FL13. The virtualized table viewport MUST occupy the remaining page height below the header + filter controls (using a flexible layout) so the first screen shows as many rows as possible.

FL14. The filter-control area second row MUST include an IP visibility toggle button at the far right:

- The button MUST be a square icon button using an eye/eye-off glyph.
- Initial state MUST be "hidden".
- When hidden, request IP cell text MUST remain present but rendered with a Gaussian blur effect.
- When shown, the blur MUST be removed immediately.

FL15. The table MUST use compact column spacing:

- Header and body cells MUST use reduced horizontal/vertical padding suitable for dense log browsing.
- Columns MUST use content-oriented widths (instead of evenly stretched wide columns) to avoid large unused horizontal gaps between adjacent fields.

FL16. The Input column MUST use a minimum width of 8 rem so its two non-wrapping lines fit common values. The Output column MUST use a minimum width of 6 rem. Its visible token count MUST use the same font size, font weight, and tabular-number treatment as the first line of the Input column. The width removed by merging the admin User and Token columns MUST be allocated to the Output column instead of becoming unused inter-column space.

FL17. The `duration/ttfb/stream` merged column MUST use compact inline-badge spacing and width so that token-count columns remain visually closer to it (reduced horizontal gap).

FL18. The merged `created_at/request_id` leading column MUST use a compact width and reduced horizontal padding.

FL19. The first visible merged `created_at/request_id` column MUST keep a small left inset from the table edge to avoid text touching the border.

FL20. The status indicator MUST be rendered directly adjacent to the request ID text on the second line of the merged `created_at/request_id` cell (near-zero gap), and columns to the right SHOULD use reduced left padding to keep the layout left-compacted.

FL21. For an admin viewer, `username` and `api_key_name` MUST share one fixed two-line, non-wrapping column with a minimum width of 6 rem. The first line MUST show `username` as the primary value. The second line MUST show the Token display value from FL30 in smaller muted text. Each line MUST truncate independently when its content exceeds the available width. For a non-admin viewer, the same column MUST show only the Token display value and MUST NOT repeat the viewer's username.

FL22. The merged `duration/ttfb/stream` column MUST remain narrowly sized with minimal horizontal cell padding and a compact inline badge row, and MUST NOT reserve excess blank width when values are short.

FL23. The merged `model/[channel]` column MUST use the model width rules in FL34. Its admin-only Channel line MUST remain non-wrapping and MAY expand the table width with long content.

FL24. On desktop dashboard layouts, content MAY expand the logs table beyond the page content width. The table viewport MUST then provide horizontal scrolling. The `request_ip` column MUST use narrow width with truncated text display.

FL25. The `charge_nano_usd` (Cost) column displayed value MUST use regular USD currency formatting with exactly 6 fractional digits (for example: `$0.000123`), and MUST NOT use threshold shorthand (for example: `<$0.0001`). Formatting MUST operate on the integer string with `BigInt`; it MUST round to the nearest micro-dollar with ties away from zero and MUST NOT pass through JavaScript `Number`, `parseFloat`, or `Intl.NumberFormat`.

FL25a. The Cost column MUST NOT truncate visible cell text. The table layout MUST allow this column to expand with content when needed (while preserving horizontal overflow/scroll behavior for narrow viewports).

FL26. Hovering, focusing, or activating the `charge_nano_usd` (Cost) cell MUST show billing breakdown details sourced from `billing_breakdown_json`, including per-class expression `unit_price × quantity` and subtotal for token and meter line items, plus multiplier and base charge. Activation MUST work on touch devices; activating outside the tooltip or pressing Escape MUST close it. The tooltip MUST show context tier and service tier when present, and MUST include cache TTL and modality labels on line items when present. Canonical usage-class values, units, modalities, cache TTL values, context tiers, and service tiers defined by Monoize MUST render through locale keys. An unknown custom pricing-profile value MUST remain visible as its original string. The "final cost" line MUST NOT be rendered; the total cost line at the bottom already displays the definitive charge.

FL26a. In the cost breakdown tooltip, any per-class line item whose computed charge is zero (i.e. `charge_nano = "0"` or quantity is `0`) MUST be hidden from the rendered tooltip. The backend MUST continue to include all fields in `billing_breakdown_json` regardless of value; this is a frontend-only rendering filter.

FL26b. *(Removed — "final cost" line is unconditionally removed from the tooltip. See FL26.)*

FL26c. If the cost breakdown tooltip would contain no visible line items (all per-class charges are zero per FL26a, no base charge, no multiplier, and billing snapshot is present), the Cost cell MUST render as plain text without a tooltip wrapper. The displayed cost value remains unchanged.

FL26d. Legacy snapshot exception to FL26c: if an existing historical `billing_breakdown_json.exemption_reason = "admin_unpriced_model"`, the Cost cell MUST still render a tooltip. That tooltip MUST include a localized note stating that an admin-used unpriced model was exempted from billing, so a visible `$0.000000` charge is not mistaken for missing data. New metered-billing requests MUST NOT create this exemption.

FL26e. The cost breakdown tooltip width MUST NOT exceed the mobile viewport width minus 24 pixels. On viewports narrower than the `sm` breakpoint, each line-item price expression MUST render below its localized label. On wider viewports, the label and expression MAY render in two columns.

FL27. Hovering the `input_tokens` (Input) and `output_tokens` (Output) cells MUST show usage breakdown details sourced from `usage_breakdown_json`, including subtype token counts when available (for example: text, cached, cache creation/read, image, audio, reasoning).

FL27a. In the request-logs table, the visible Input and Output token cell values MUST prefer `usage_breakdown_json.input.total_tokens` and `usage_breakdown_json.output.total_tokens` when those fields are present. If those fields are absent, the UI MUST fall back to scalar columns `input_tokens` and `output_tokens`.

FL27b. If neither the usage-breakdown totals nor the scalar token columns are available for a row, the UI MUST render a localized unavailable placeholder (`-`) in the visible Input and Output cells instead of `0`. This placeholder state represents "usage unavailable", not zero token consumption.

FL27c. When FL27b applies, hovering the Input or Output cell MUST show the standard token-detail tooltip shell with a localized unavailable message rather than an empty numeric breakdown.

FL27d. The visible Input cell MUST always render the total input count selected by FL27a; it MUST NOT substitute the uncached-input count. When the cached-input count selected from `usage_breakdown_json.input.cached_tokens` or scalar `cache_read_tokens` is positive and total input is positive, the cell MUST use exactly two non-wrapping lines and the second line MUST render `<formatted cached count> / <integer percentage>%`, where `integer percentage = round(cached input / total input * 100)`. For example, total input `32000` and cached input `16000` render as first line `32,000` and second line `16,000 / 50%`. The second line MUST use smaller muted text and MUST NOT use a success/status color. When cached input is absent or not positive, or total input is absent or not positive, the cell MUST render only the total-input line and vertically center that line within the 44-pixel body row. It MUST NOT reserve an empty second line or render `0`, `-`, or a percentage. The complete visible cell remains the FL27 hover surface.

FL28. For rows with `status = "error"` or `status = "client_gone"`, hovering the request-id/status indicator MUST show error details from `error_code`, `error_message`, and `error_http_status` when present.

FL29. When `tried_providers` is non-empty, the request-id tooltip MUST additionally display the localized retry-chain heading and the same chronological attempt rows as FL9b (hop label, optional `upstream_status`, `error`), separated from the main error details by a visual divider. The hop label MUST follow FL9a.3. The tooltip MUST NOT use raw `provider_id/channel_id` as the label when a name exists.

FL30. For rows where `request_kind = "active_probe_connectivity"` and `api_key_name` is null, the Token column MUST display a localized i18n label meaning "Connectivity Test".

FL31. The rightmost `request_ip` column MUST keep a trailing right inset equal to the leading left inset of the merged `created_at/request_id` column, so IP text does not visually touch the table's right boundary.

FL32. Tooltip overlays for request-log table detail cells (request-id, model, token, channel, duration, input/output, cost) MUST render in a portal layer attached to `document.body` so overlay width/position is not constrained by table/cell/container layout width or overflow clipping.

FL33. On coarse-pointer devices (touch-first), those tooltip overlays MUST open on tap and close on outside tap, while preserving hover behavior on fine-pointer devices.

FL34. The merged `model/[channel]` column MUST use a minimum width of 13.5 rem as its baseline and MUST be allowed to expand with content when long model identifiers or admin-visible Channel identifiers are present.

FL35. In the logs table, model badge text in the merged `model/[channel]` column MUST NOT be forcibly truncated. On narrow viewports, overflow MUST be handled by the table/container horizontal scrolling behavior rather than wrapping or clipping model badge text.

FL36. In the request-id status indicator, status-color mapping MUST be:

- `pending`: blue lamp,
- `success`: green lamp,
- `client_gone`: warning/amber lamp,
- `error`: red lamp.

Hovering a `client_gone` row MUST show `error_code`, `error_message`, and `error_http_status` the same way FL28 shows those fields for `error` rows. The status filter MUST include a `client_gone` option.

FL37. The logs page MUST auto-refresh the newest page periodically so that terminal rows and aggregate totals refresh without manual reload. While an SSE connection is active, in-progress requests SHOULD first appear as SSE-delivered `pending` rows and later transition to terminal state by replacement. *(See FL49: when SSE is connected, SSE is the primary real-time mechanism; polling becomes fallback only.)*

FL37a. When SWR revalidation replaces `loadedLogs` with server-fetched data (initial load, focus revalidation, reconnect revalidation, resync, or polling), the frontend MUST preserve any SSE-delivered `pending` rows that are not yet represented in the server response. Specifically: rows with `status = "pending"` whose `id` is absent from the server data AND whose `request_id` (when non-null) is absent from the server data MUST be re-prepended to the merged result. This prevents SSE-only pending items (which are never persisted to the database per RL1a-1) from being silently dropped by SWR cache replacement.

FL38. While any tooltip-detail overlay in the request-logs table is open (request-id, model, token, channel, duration, input/output, cost):

- The periodic auto-refresh poll defined in FL37 MUST be paused (`isPaused` returns `true`).
- Any data updates that arrive from in-flight requests (started before the tooltip opened) or from SWR revalidation triggers (e.g. `revalidateOnFocus` when the browser tab regains focus) MUST be buffered and MUST NOT cause the table row list to re-render.
- When all tooltip overlays close, buffered data MUST be flushed and applied to the visible table immediately, and periodic polling MUST resume at the normal interval.
- This guarantee MUST hold on both fine-pointer (desktop hover) and coarse-pointer (mobile tap) devices.

FL39. The time-range filter popover MUST contain three vertical sections in this order: preset row, manual datetime inputs, single-month calendar.

FL40. The preset row MUST be horizontally scrollable when content overflows and MUST reserve scrollbar gutter space to avoid layout jump while scrolling.

FL41. The popover content width MUST equal the rendered calendar width for the currently displayed month. The datetime input rows MUST NOT expand popover width beyond calendar width.

FL42. The manual datetime inputs MUST be stacked in two rows (`from` then `to`) and accept second-precision format `yyyy-MM-dd HH:mm:ss`.

FL43. Time-range selection MUST be bidirectionally synchronized:

- selecting a preset MUST update manual inputs and calendar selection,
- selecting calendar range or committing manual inputs MUST activate the matching fixed preset (`today`, `yesterday`, `this_month`, `last_month`) when and only when the selected range matches that preset, otherwise no preset is active.

FL44. Active preset buttons (including `All Time`) MUST use a high-contrast foreground/background pair so text remains legible in both light and dark themes.

## 6. SSE Real-Time Updates

### 6.1 SSE endpoint contract

FL45. The server MUST expose a streaming endpoint at `GET /api/dashboard/request-logs/stream`.

- Content-Type of the response MUST be `text/event-stream`.
- Authentication MUST use the dashboard session validated by the same `get_current_user()` mechanism as all other dashboard endpoints. The dashboard browser MUST send the `monoize_session` cookie and MUST NOT add an `Authorization` header, as specified by `dashboard-session-authentication.spec.md` DSA2 and DSA5.
- If the token is missing or invalid, the server MUST respond with HTTP 401 before entering streaming mode.

### 6.2 SSE event types

FL46. The endpoint MUST emit exactly two event types:

1. **`log_batch`**: Carries an array of one or more complete `RequestLog` objects (same shape as items in the REST response `data[]` from section 3.1).
   Wire format:
   ```
   event: log_batch
   data: [{RequestLog}, ...]

   ```
   Each object in the array MUST contain all fields defined in the `RequestLog` interface (section 1.1 + enriched fields from 1.2). The array MUST contain at least one element per emission.
   For terminal `success` / `error` rows, the server MUST enqueue the SSE-visible event at terminalization time, before the later write-batcher flush persists the row to the database. Batch persistence MAY still occur asynchronously, but SSE delivery latency MUST be bounded by request finalization rather than by the write-batcher interval.

2. **`resync`**: Signals the client to discard SSE-delivered incremental state and perform a full SWR refetch.
   Wire format:
   ```
   event: resync
   data: {}

   ```
   The server MUST emit `resync` when the internal broadcast channel enters a `Lagged` state (i.e., a slow consumer missed messages). The client MUST respond by calling `mutate()` on the request-logs SWR key to trigger a full REST refetch.

### 6.3 Permission model

FL47. SSE event visibility MUST obey the same permission rules as the REST endpoint (RL-API1):

- If the authenticated user has role `super_admin` or `admin`, the server MUST push ALL newly created log entries.
- Otherwise, the server MUST push only log entries where `user_id` matches the authenticated user's ID, and each pushed row MUST carry the read-time-masked `error.message` and `tried_providers[].error` values defined by RL-API14.
- The server MUST NOT accept filter query parameters on the SSE endpoint (see FL53). Client-side code MUST filter SSE-delivered logs locally against the active UI filter state before displaying them.

### 6.4 Connection lifecycle

FL48. The SSE connection lifecycle MUST follow these phases:

1. **Connect**: The client MUST open the SSE stream using a `fetch()`-based reader with `credentials: "include"` and without an `Authorization` header.
    The frontend integration layer for this stream MUST be implemented through SWR subscription state, so stream lifecycle and event delivery are owned by the SWR data layer rather than a page-local ad hoc subscription mechanism.
    The client MUST begin attempting this SSE connection as soon as the logs page mounts; it MUST NOT gate connection startup on completion of the initial REST page fetch.
    After authentication succeeds, the server MUST emit an initial SSE frame without waiting for a future request-log broadcast. If the current pending-snapshot map defined in RL1a-3 contains one or more entries visible to the authenticated user, the initial frame MUST be a `log_batch` event containing those entries ordered by `created_at DESC`. Otherwise, the initial frame MUST be an SSE comment frame. This initial frame exists so the client can mark the stream connected promptly and so in-flight `pending` requests that began before stream establishment become visible.
2. **Receive**: On each `log_batch` event, the client MUST merge the received `RequestLog` objects into the existing table data array (newest first). If an incoming row has the same `request_id` as an existing row, the incoming row MUST be processed before active UI filters are applied. If the incoming row matches active UI filters, it MUST replace the existing row instead of creating a duplicate. If the incoming row does not match active UI filters, the existing row with that `request_id` MUST be removed from the visible table. This replacement/removal rule is required for the `pending -> success/error` SSE-only lifecycle defined in RL1a-1 and RL1a-2, including the case where the active filter is `status = pending` and a terminal row arrives.
3. **Disconnect**: On network error, HTTP error, or stream close, the client MUST fall back to SWR polling (see FL50).
4. **Reconnect**: The client MUST automatically attempt reconnection using exponential backoff: initial delay 1s, doubling on each consecutive failure (1s, 2s, 4s, 8s, 16s), capped at 30s maximum delay. On successful reconnection, the backoff counter MUST reset to 1s.
5. **Visibility recovery**: When `document.visibilityState` transitions to `visible`, the client MUST treat the current stream as suspect, close it if open, immediately perform a full REST refetch, and immediately open a new SSE stream attempt. This transition MUST NOT wait for the normal exponential-backoff timer.

### 6.5 SSE as primary real-time mechanism

FL49. When an SSE connection is active and receiving events, SSE replaces the periodic SWR polling defined in FL37 as the primary real-time data delivery mechanism. The SWR auto-refresh interval defined in FL37 MUST be paused while SSE is connected. Polling MUST resume only when SSE is disconnected (see FL50).

### 6.6 Polling fallback on SSE disconnect

FL50. When the SSE connection is lost (network failure, server restart, or stream termination), the client MUST immediately activate SWR polling at an interval of approximately 3 seconds. This polling MUST continue until the SSE connection is re-established, at which point polling MUST be paused again per FL49.

### 6.7 Aggregate values remain REST-derived

FL51. The aggregate fields `total` and `total_charge_nano_usd` (as defined in section 3.1 response schema) MUST NOT be delivered via SSE events. These values MUST remain REST-derived and MUST refresh on the initial page fetch, on explicit manual refresh, on SSE-triggered resync/reconnect revalidation, and during the polling fallback defined in FL50. While SSE is connected, the client MUST NOT keep a periodic `request-logs` polling loop alive solely to refresh these aggregate values.

### 6.8 Name-cache enrichment model

FL52. REST request-log responses MUST compute enriched fields only by query-time joins against the related tables listed in section 1.2. The REST path MUST NOT consult a full-table in-memory name cache or replace JOIN results with cache values.

FL52a. Each in-memory request-log event MUST carry the request-time name snapshots available for `provider_name`, `channel_name`, `username`, and `api_key_name`. SSE delivery MUST convert the event directly to `RequestLogRow` from those snapshots. It MUST NOT perform database JOINs or consult full-table ID-to-name caches at delivery time. A name that was unavailable in the event snapshot MUST be null, and the client MUST render the raw ID as fallback display text where applicable.

FL52b. Monoize MUST NOT maintain full-table user, API-key, Provider, or Channel name caches for request-log enrichment. Related-row deletion therefore requires no request-log name-cache invalidation.

### 6.9 No server-side filtering on SSE

FL53. The SSE endpoint `GET /api/dashboard/request-logs/stream` MUST NOT accept any filter query parameters (`model`, `status`, `api_key_id`, `username`, `search`, `time_from`, `time_to`). The server pushes all user-visible logs (per FL47 permission rules). The client MUST apply active UI filters locally to determine which SSE-delivered rows to display.

### 6.10 Keep-alive

FL54. The server MUST emit SSE comment lines (lines beginning with `:`) at regular intervals of approximately 15 seconds when no data events have been sent. This prevents intermediate proxies and load balancers from closing idle connections due to inactivity timeouts.

FL54a. The client MUST treat the stream as stale if no SSE bytes (data events or comment frames) are observed for at least 45 seconds while the document is visible. A stale stream MUST be closed and reconnected using the reconnect rules in FL48.

### 6.11 Concurrent connection policy

FL55. The endpoint `GET /api/dashboard/request-logs/stream` MUST enforce a per-user concurrent SSE connection cap. The default cap is 5 and `MONOIZE_REQUEST_LOG_SSE_MAX_CONNECTIONS_PER_USER` MAY set any positive integer cap. When a user already has the configured number of active SSE connections open on this endpoint, any additional connection attempt by that user MUST be rejected with HTTP 429 Too Many Requests. The active connection count MUST be tracked per authenticated user ID and MUST be decremented atomically when a connection closes (via a Drop guard or equivalent RAII mechanism). A zero counter MUST be removed without an ABA race by comparing the shared counter identity while the map entry is locked. The counter map MUST contain only users with active or concurrently-opening SSE streams.

### 6.12 Tooltip-pause interaction with SSE

FL56. While any tooltip-detail overlay in the request-logs table is open (as defined in FL38), SSE-delivered `log_batch` data MUST be buffered in memory and MUST NOT cause the table row list to re-render. When all tooltip overlays close, all buffered SSE data MUST be flushed: buffered rows MUST be prepended to the table data array and the table MUST re-render with the combined dataset. This behavior is analogous to the polling-pause guarantee in FL38 and MUST hold on both fine-pointer and coarse-pointer devices.

FL57. The frontend tooltip-pause bookkeeping used by FL38 and FL56 MUST be resilient to virtualization-driven row unmounts and tooltip component remounts. If a tooltip-owning row leaves the DOM before a matching close callback fires, the page MUST still eventually resume live updates without requiring a manual refresh or page reload. Implementations MUST therefore track tooltip-open state by stable tooltip identity (or an equivalent leak-free ownership model), rather than relying on a process-wide integer counter that can remain permanently positive after an unbalanced open/close sequence.
