# Request Capture Dumps Specification

## 0. Status

- **Purpose:** Persist opt-in per-request diagnostic dumps for API-key-authenticated forwarding requests.
- **Scope:** Applies to API-key-authenticated URP forwarding endpoints `POST /v1/responses`, `POST /v1/chat/completions`, and `POST /v1/messages` (including their `/api` aliases).
- **Storage:** Dumps are filesystem files, not database rows.

## 1. Configuration

RCD-C1. System settings MUST include `monoize_request_capture_enabled: boolean`.

RCD-C2. The default value of `monoize_request_capture_enabled` MUST be `false`.

RCD-C3. System settings MUST include `monoize_request_capture_retention_days: integer`.

RCD-C4. The default value of `monoize_request_capture_retention_days` MUST be `1`.

RCD-C5. If a settings update supplies `monoize_request_capture_retention_days < 1`, the server MUST persist `1`.

RCD-C6. API key rows MUST include `request_capture_mode: "off" | "capture-all" | "capture-only-abnormal"`.

RCD-C7. The default value of `request_capture_mode` for newly created API keys MUST be `"off"`.

RCD-C8. If an existing API key row has no stored `request_capture_mode` value, runtime MUST treat it as `"off"`.

RCD-C9. A forwarding request is capture-eligible iff all conditions are true:

1. the request is authenticated by a dashboard-managed API key;
2. `monoize_request_capture_enabled == true` at request-processing time;
3. the authenticated API key has `request_capture_mode != "off"`.

RCD-C10. If `monoize_request_capture_enabled == false`, no dump file MUST be written even when the authenticated API key has `request_capture_mode != "off"`.

RCD-C11. If `request_capture_mode == "capture-all"`, Monoize MUST persist a dump file for every capture-eligible forwarding request that records at least one upstream attempt.

RCD-C12. If `request_capture_mode == "capture-only-abnormal"`, Monoize MUST persist a dump file only when at least one of the following conditions is true for the request:

1. an upstream call failed and produced an upstream error for any recorded attempt;
2. the final request result contains no upstream `usage` object;
3. the final request result contains an upstream `usage` object whose `input_tokens + output_tokens == 0`.

RCD-C13. For RCD-C12, synthetic usage created only for estimated billing MUST NOT cause condition 2 to become false. The abnormal-only decision MUST be based on upstream-provided usage data before estimated billing fallback.

RCD-C14. Capture resource bounds MUST be process configuration. Defaults are:

- `MONOIZE_REQUEST_CAPTURE_MAX_ATTEMPTS=16`;
- `MONOIZE_REQUEST_CAPTURE_MAX_FRAMES=4096`;
- `MONOIZE_REQUEST_CAPTURE_MAX_FRAME_BYTES=262144`; and
- `MONOIZE_REQUEST_CAPTURE_MAX_SESSION_BYTES=16777216`.

Each positive integer environment value replaces its default, except `MONOIZE_REQUEST_CAPTURE_MAX_SESSION_BYTES` values below `8192`, which resolve to `8192`. Zero or invalid values use the default.

RCD-C15. The captured top-level `request_id` and every identifying string retained in an oversized-attempt placeholder MUST contain at most `256` UTF-8 bytes. Truncation MUST occur on a valid UTF-8 boundary and MUST increase `capture_truncation.omitted_bytes` for the top-level request id.

## 2. Directory and filename

RCD-S1. Dumps MUST be written under a directory named `dumps` inside the Monoize data directory.

RCD-S2. For the default database DSN `sqlite://./data/monoize.db`, the dump directory MUST be `./data/dumps`.

RCD-S3. For a SQLite file DSN, the Monoize data directory is the parent directory of the SQLite database file.

RCD-S4. For a non-file or non-SQLite database DSN, the Monoize data directory MUST fall back to the parent directory of the default database file, `./data`.

RCD-S5. The dump directory MUST be created before the first dump write if it does not exist.

RCD-S6. Each dump filename MUST have this shape:

```text
{request_id_prefix}_{utc_timestamp_ms}.json
```

RCD-S7. `request_id_prefix` MUST be derived from the first eight Unicode scalar values of the Monoize request id when a request id is present.

RCD-S8. Within that derived prefix, any character outside ASCII alphanumeric, `-`, and `_` MUST be replaced with `_` before the filename is joined to the dump directory.

RCD-S9. If a request id is absent or shorter than eight scalar values, `request_id_prefix` MUST use the available request id value after the sanitization in RCD-S8, or `unknown` when absent or when sanitization yields an empty prefix.

RCD-S10. `utc_timestamp_ms` MUST be a UTC timestamp with millisecond precision formatted as `YYYYMMDDTHHMMSSmmmZ`.

RCD-S11. A dump write MUST use a temporary file followed by an atomic rename into the final filename when the operating system supports rename within the dump directory.

RCD-S12. Dump write failure MUST be logged and MUST NOT change the HTTP response returned to the downstream client.

## 3. Dump file schema

RCD-D1. A dump file MUST be UTF-8 JSON.

RCD-D2. A dump file MUST contain at least these top-level fields:

- `version: 2`
- `request_id: string?`
- `created_at: RFC3339 string`
- `api_key_id: string`
- `user_id: string`
- `downstream_protocol: string`
- `is_stream: boolean`
- `attempts: object[]`
- `capture_truncation: object`

RCD-D2a. Dump schema version 2 replaces version 1. Monoize MUST write only version-2 dumps. Version-1 files that already exist on disk are not migrated, are not registered in the capture metadata table (section 5), and are therefore unreachable through the capture detail API; retention cleanup still deletes them per section 4.

RCD-D3. Each `attempts[]` entry MUST contain:

- `attempt_number: integer`
- `provider_id: string`
- `channel_id: string?`
- `provider_type: string`
- `logical_model: string`
- `upstream_model: string`
- `upstream_path: string`
- `raw_input: object`
- `transformed_urp_request: object`
- `upstream_request: object`
- `downstream_response: object?`
- `downstream_sse_frames: string[]?`
- `transform_chain: object[] | null`
- `error: object?`

RCD-D3a. `transform_chain` MUST list the transform rules that apply to the attempt, in application order: provider-scope rules first, then global-scope rules, then API-key-scope rules; within one scope, configured order. A rule applies iff `enabled == true` and either the rule has no `models` patterns or at least one pattern glob-matches the attempt's transform match model. Each entry MUST be:

```json
{ "scope": "provider" | "global" | "api_key", "transform": string, "phase": "request" | "response" }
```

`transform` MUST be the canonical transform id. Rule `config` payloads MUST NOT be recorded.

RCD-D3b. For the `POST /v1/responses/compact` passthrough endpoint, which applies no URP transforms, `transform_chain` MUST be `[]`.

RCD-D3c. An oversized-attempt placeholder (RCD-D13) MUST store `transform_chain: null`.

RCD-D4. `raw_input` MUST be the parsed downstream JSON request body as received by the forwarding handler, before conversion to URP and before request transforms.

RCD-D5. `transformed_urp_request` MUST be the URP request after provider request transforms, global request transforms, API-key request transforms, Monoize context removal, and reasoning-envelope upstream filtering.

RCD-D6. `upstream_request` MUST be the provider-native JSON object sent as the upstream HTTP request body.

RCD-D7. For a non-streaming upstream response, `downstream_response` MUST be the provider raw response JSON object returned by the upstream HTTP response body before Monoize decodes it to URP.

RCD-D8. For a buffered synthetic stream, `downstream_response` MUST be the provider raw response JSON object returned by the upstream HTTP response body before Monoize decodes it to URP.

RCD-D9. For a pass-through streaming response, `downstream_sse_frames` MUST contain the SSE frame data strings emitted to the downstream client in emission order after response transforms and downstream encoding.

RCD-D9a. If downstream SSE frame emission occurs inside asynchronous tasks spawned by the pass-through streaming pipeline, all such tasks MUST record emitted frames into the same per-attempt `downstream_sse_frames` array and MUST share one retained-byte counter, omitted-frame counter, omitted-byte counter, and one immutable limit set. Synthetic terminal error and `[DONE]` frames MUST use the same bounded recorder. Their aggregate capture and metadata MUST obey RCD-D14.

RCD-D10. For a pass-through streaming response, `downstream_response` MUST be null or absent.

RCD-D11. If an upstream call fails before a response body is available, the attempt entry MUST include `error` with at least `message` and `code` when available.

RCD-D12. Capture MUST NOT redact prompt text, tool arguments, image payloads, or provider response bodies because the feature is explicitly a raw diagnostic dump. Operators MUST keep the feature disabled unless they accept that sensitive payloads are persisted.

RCD-D13. One session MUST retain no more than the configured attempt count and no more than the effective serialized session byte count. Capacity validation and filesystem writing MUST use the same compact UTF-8 JSON byte vector; the writer MUST NOT re-encode the accepted payload. An attempt that cannot fit MAY be replaced by bounded identifying fields plus explicit `capture_truncation` metadata. A capture-all request that recorded an attempt MUST retain at least one full or placeholder attempt and MUST persist the bounded envelope. Further attempts MUST be counted as omitted rather than appended.

RCD-D14. One captured downstream SSE frame MUST retain no more than the configured frame byte count. One session MUST retain no more than the configured frame count and total session byte count. Truncation MUST occur on a valid UTF-8 boundary.

RCD-D15. The top-level `capture_truncation` object MUST include `truncated`, `omitted_attempts`, `omitted_frames`, `omitted_bytes`, and `retained_bytes`. Each attempt whose frame list was truncated MUST include `downstream_sse_frames_truncation` with the corresponding omitted counts. An oversized-attempt placeholder that removes retained frames MUST preserve prior frame-omission counts, add the removed retained-frame count and byte count, and report zero retained frames and bytes. A non-truncated dump MUST include zero counts.

## 4. Retention cleanup

RCD-R1. On startup, Monoize SHOULD delete dump files whose modification time is older than `monoize_request_capture_retention_days` days relative to cleanup execution time.

RCD-R2. While running, Monoize MUST periodically delete dump files whose modification time is older than `monoize_request_capture_retention_days` days relative to cleanup execution time.

RCD-R3. The default periodic cleanup interval MUST be 1 hour.

RCD-R4. Cleanup failure MUST be logged and MUST NOT stop process startup or request handling.

RCD-R5. Cleanup MUST only delete regular files directly under the dump directory. It MUST NOT recurse into subdirectories.

RCD-R6. Each cleanup pass MUST also delete every `request_capture_records` row (section 5) whose `created_at_unix_ms` is older than the same retention cutoff. Row deletion failure MUST be logged and MUST NOT stop file cleanup or request handling.

## 5. Capture metadata records

RCD-M1. Capture metadata MUST be stored in table `request_capture_records` with columns:

- `file_name: TEXT PRIMARY KEY` (the dump filename from RCD-S6, without directory components)
- `request_id: TEXT NOT NULL` (the canonical Monoize request id recorded in the dump, after RCD-C15 truncation)
- `user_id: TEXT NOT NULL`
- `api_key_id: TEXT NOT NULL`
- `created_at: TEXT NOT NULL` (RFC3339, equal to the dump `created_at`)
- `created_at_unix_ms: BIGINT NOT NULL` (same instant as Unix epoch milliseconds)
- `size_bytes: BIGINT NOT NULL` (the byte length of the persisted dump file)

RCD-M2. The table MUST have an index on `(user_id, request_id)` and an index on `(created_at_unix_ms)`.

RCD-M3. Immediately after a dump file write succeeds (RCD-S11), and only when the capture session has a request id, Monoize MUST insert one `request_capture_records` row describing that file. Insert conflicts on `file_name` MUST upsert. A session without a request id MUST write no metadata row.

RCD-M4. Metadata insert failure MUST be logged and MUST NOT delete the dump file and MUST NOT change the HTTP response returned to the downstream client.

RCD-M5. `request_capture_records` rows have no foreign keys. Deleting a user, an API key, or a request-log row MUST NOT delete capture metadata rows; only retention cleanup (RCD-R6) and stale-record cleanup (`request-capture-viewer.spec.md` RCV-A8) delete them.
