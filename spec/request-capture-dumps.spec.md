# Request Capture Dumps Specification

## 0. Status

- **Purpose:** Persist opt-in per-request diagnostic dumps for API-key-authenticated forwarding requests.
- **Scope:** Applies to API-key-authenticated URP forwarding endpoints `POST /v1/responses`, `POST /v1/chat/completions`, `POST /v1/messages`, `POST /v1/images/generations`, and `POST /v1/images/edits` (including their `/api` aliases).
- **Storage:** Dumps are filesystem files, not database rows.

## 1. Configuration

RCD-C1. System settings MUST include `monoize_request_capture_enabled: boolean`.

RCD-C2. The default value of `monoize_request_capture_enabled` MUST be `false`.

RCD-C3. System settings MUST include `monoize_request_capture_max_total_bytes: integer` (the global capture size budget, section 4.3).

RCD-C4. The default value of `monoize_request_capture_max_total_bytes` MUST be `1073741824` (1 GiB). The value `0` means no size budget. If a settings update supplies a value in `[1, 1048575]`, the server MUST persist `1048576` (1 MiB).

RCD-C5. The setting `monoize_request_capture_retention_days` no longer exists. The settings API MUST NOT accept it, the settings store MUST NOT read or write it, and the migration in RCD-M7 deletes its persisted row. Per-key retention (RCD-C5a through RCD-C5c) replaces it.

RCD-C5a. API key rows MUST include `request_capture_retention: "5m" | "1h" | "24h" | "7d"`.

RCD-C5b. The default value of `request_capture_retention` for newly created API keys MUST be `"24h"`. If an existing API key row has an absent or null stored `request_capture_retention` value, runtime MUST treat it as `"24h"`. Any other stored value outside the RCD-C5a set MUST fail the read (`api-token-management.spec.md` TM-STORAGE-7).

RCD-C5c. Retention values map to these durations: `"5m"` = 300 s, `"1h"` = 3600 s, `"24h"` = 86400 s, `"7d"` = 604800 s.

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

RCD-C16. For the Image API endpoints (`image-api-proxy.spec.md`), the capture unit is one fan-out sub-request (IM14), not the downstream HTTP request. For each capture-eligible sub-request (RCD-C9 evaluated once per downstream request), Monoize MUST start one independent capture session whose captured `request_id` is the sub-request id: the downstream request id when `n == 1`, or `{request_id}:img:{i}` with the 0-based sub-request index `i` when `n > 1`, matching the sub-request's request-log row. Each sub-request session applies its RCD-C11/RCD-C12 persistence decision to its own attempts and writes at most one dump file. The assembled downstream Image API response envelope (`image-api-proxy.spec.md` §5.2) is not captured.

## 2. Directory and filename

RCD-S1. Dumps MUST be written under a directory named `dumps` inside the Monoize data directory.

RCD-S2. For the default database DSN `sqlite://./data/monoize.db`, the dump directory MUST be `./data/dumps`.

RCD-S3. For a SQLite file DSN, the Monoize data directory is the parent directory of the SQLite database file.

RCD-S4. For a non-file or non-SQLite database DSN, the Monoize data directory MUST fall back to the parent directory of the default database file, `./data`.

RCD-S5. The dump directory MUST be created before the first dump write if it does not exist.

RCD-S6. Each dump filename MUST have one of these shapes:

```text
{request_id_prefix}_{utc_timestamp_ms}.json.zst       (zstd-compressed dump, RCD-Z1)
{request_id_prefix}_{utc_timestamp_ms}_{k}.json.zst   (RCD-S6a collision disambiguation, k >= 1)
{request_id_prefix}_{utc_timestamp_ms}.json           (uncompressed dump, RCD-Z4 fallback only)
{request_id_prefix}_{utc_timestamp_ms}_{k}.json       (RCD-S6a fallback disambiguation, k >= 1)
```

RCD-S6a. When the undisambiguated RCD-S6 filename already exists in the dump directory, the write MUST select the smallest integer `k >= 1` for which `{request_id_prefix}_{utc_timestamp_ms}_{k}` with the same extension does not exist. Final filename selection MUST be atomic against concurrent dump writes: two concurrent writes MUST NOT resolve to the same final filename, and a final-name file MUST never be observable in a partially written state. Precondition making this rule reachable: RCD-C16 fan-out sub-request sessions share one request-id prefix and can persist within the same millisecond.

RCD-S7. `request_id_prefix` MUST be derived from the first eight Unicode scalar values of the Monoize request id when a request id is present.

RCD-S8. Within that derived prefix, any character outside ASCII alphanumeric, `-`, and `_` MUST be replaced with `_` before the filename is joined to the dump directory.

RCD-S9. If a request id is absent or shorter than eight scalar values, `request_id_prefix` MUST use the available request id value after the sanitization in RCD-S8, or `unknown` when absent or when sanitization yields an empty prefix.

RCD-S10. `utc_timestamp_ms` MUST be a UTC timestamp with millisecond precision formatted as `YYYYMMDDTHHMMSSmmmZ`.

RCD-S11. A dump write MUST fully write a temporary file first and then publish it under the final filename with one atomic filesystem operation that fails (rather than overwrites) when the final filename already exists, when the operating system supports such an operation within the dump directory; an `already exists` publication failure re-enters RCD-S6a name selection with the next `k`.

RCD-S12. Dump write failure MUST be logged and MUST NOT change the HTTP response returned to the downstream client.

## 2a. Compression and asynchronous write pipeline

RCD-Z1. New dumps MUST be persisted as one zstd frame (compression level 3) containing the compact UTF-8 JSON byte vector accepted by RCD-D13, under the `.json.zst` filename shape of RCD-S6. The compressed frame therefore begins with the zstd magic number bytes `0x28 0xB5 0x2F 0xFD`.

RCD-Z2. The RCD-D13 attempt-count and session-byte bounds apply to the uncompressed JSON byte vector. The on-disk file MAY be smaller than the bounded payload; it is never larger than the zstd frame of that payload.

RCD-Z3. Write pipeline ordering. When a capture session decides to persist (`persist_with_result`):

1. Attempt bounding, envelope encoding, and the RCD-D13 capacity loop run synchronously in the calling task, exactly as before compression existed.
2. Compression, the temporary-file write, the atomic rename (RCD-S11), the metadata insert (RCD-M3), and the size-budget enforcement pass (RCD-R7) run in one spawned background task.
3. The HTTP response path MUST NOT await step 2. A caller MAY await the returned task handle in tests.

RCD-Z4. If zstd compression fails, the background task MUST log the failure and write the uncompressed JSON byte vector under the `.json` filename shape instead. The metadata row records whichever file name was written.

RCD-Z5. A process shutdown MAY discard background write tasks from step 2 of RCD-Z3 that have not completed. This is a permitted dump loss under RCD-S12 semantics.

RCD-Z6. Read-path format detection MUST use content, not filename: when the first four bytes of a dump file equal the zstd magic number (RCD-Z1), the reader MUST zstd-decompress the file; otherwise the reader MUST treat the file bytes as raw JSON. Legacy uncompressed `.json` dumps written before this section existed therefore stay readable.

RCD-Z7. Decompression MUST enforce an output bound equal to the effective `MONOIZE_REQUEST_CAPTURE_MAX_SESSION_BYTES` value (RCD-C14). A frame whose decompressed size exceeds the bound, or whose bytes are not a valid zstd frame despite the magic prefix, MUST fail the read; the capture detail API maps that failure to `capture_dump_unreadable` (`request-capture-viewer.spec.md` RCV-A9).

RCD-Z8. `size_bytes` in the metadata row (RCD-M1) records the byte length of the file as persisted on disk (the compressed length for RCD-Z1 files, the raw length for RCD-Z4 fallback files). The size budget of section 4.3 therefore measures actual disk usage of registered dumps.

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

RCD-D2a. Dump schema version 2 replaces version 1. Monoize MUST write only version-2 dumps. Version-1 files that already exist on disk are not migrated, are not registered in the capture metadata table (section 5), and are therefore unreachable through the capture detail API; orphan cleanup deletes them per RCD-R5a.

RCD-D2b. `downstream_protocol` MUST be exactly one of: `responses`, `chat_completions`, `anthropic_messages`, `image_generations` (`POST /v1/images/generations` ingress), or `image_edits` (`POST /v1/images/edits` ingress).

RCD-D2c. Dumps for Image API sub-requests MUST record `is_stream: false`, because the downstream Image API contract is non-streaming (`image-api-proxy.spec.md` CO1) regardless of the internal upstream transport.

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
- `reconstructed_urp_response: object?`
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

RCD-D4a. For `POST /v1/images/edits`, whose downstream body is `multipart/form-data`, `raw_input` MUST instead be the RCD-D16 multipart capture object built from the fields the ingress parser consumed (`image-api-proxy.spec.md` §3.2), in wire order: every text field with its exact wire text (before JSON-type coercion), and every accepted file field (`image`, `image[]`, `mask`) with its raw part bytes. File fields the parser ignores (IE2) MUST NOT appear.

RCD-D4b. When `n > 1` produces multiple Image API sub-request sessions (RCD-C16), every sub-request dump records the same `raw_input`: the one downstream body.

RCD-D5. `transformed_urp_request` MUST be the URP request after provider request transforms, global request transforms, API-key request transforms, Monoize context removal, and reasoning-envelope upstream filtering.

RCD-D6. `upstream_request` MUST be the provider-native JSON object sent as the upstream HTTP request body.

RCD-D6a. When the upstream HTTP request body is `multipart/form-data` (an `openai_image` attempt with user-role image input, `openai-image-upstream.spec.md` OIU-E5a), `upstream_request` MUST instead be the RCD-D16 multipart capture object built from exactly the parts of the sent form, in part order, with the same field names, file names, part content types, and part bytes.

RCD-D7. For a non-streaming upstream response, `downstream_response` MUST be the provider raw response JSON object returned by the upstream HTTP response body before Monoize decodes it to URP.

RCD-D8. For a buffered synthetic stream, `downstream_response` MUST be the provider raw response JSON object returned by the upstream HTTP response body before Monoize decodes it to URP.

RCD-D9. For a pass-through streaming response, `downstream_sse_frames` MUST contain the SSE frame data strings emitted to the downstream client in emission order after response transforms and downstream encoding.

RCD-D9a. If downstream SSE frame emission occurs inside asynchronous tasks spawned by the pass-through streaming pipeline, all such tasks MUST record emitted frames into the same per-attempt `downstream_sse_frames` array and MUST share one retained-byte counter, omitted-frame counter, omitted-byte counter, and one immutable limit set. Synthetic terminal error and `[DONE]` frames MUST use the same bounded recorder. Their aggregate capture and metadata MUST obey RCD-D14.

RCD-D10. For a pass-through streaming response, `downstream_response` MUST be null or absent.

RCD-D10a. For a pass-through streaming response whose transformed URP stream (post response transforms, pre downstream encoding) emitted a terminal `response_done` event, `reconstructed_urp_response` MUST be that event serialized as one object with the shape:

```json
{ "finish_reason": string?, "usage": object?, "output": Node[], ...extra_body }
```

This is the URP parser's non-stream reconstruction of the streamed result. It reflects the stream after response transforms, so it matches what the downstream client semantically received.

RCD-D10b. `reconstructed_urp_response` MUST be null or absent when: the attempt recorded a non-null `downstream_response` (RCD-D7 non-streaming and RCD-D8 buffered synthetic-stream attempts, where `downstream_response` already holds the provider payload), the stream terminated without a `response_done` event, or the attempt is an oversized placeholder (RCD-D13).

RCD-D10c. A stream-collected attempt is an attempt whose downstream result is non-streaming but whose upstream transport was SSE collected inside Monoize: the non-stream collection path for `responses` and `openai_image` upstream attempts whose transformed request has `stream == true`, and the Image API sub-request path whose attempts all target `responses` providers (`image-api-proxy.spec.md` IM15). For a stream-collected attempt whose decoded URP stream (before response-phase transforms) emitted a terminal `response_done` event, `reconstructed_urp_response` MUST be that event serialized in the RCD-D10a shape. It therefore reflects the upstream artifact before response transforms, unlike RCD-D10a, because a stream-collected attempt has no `downstream_response` to fill that role. For a stream-collected attempt, `downstream_response` and `downstream_sse_frames` MUST be null or absent: upstream SSE frames are not downstream emissions and are not captured.

RCD-D11. If an upstream call fails before a response body is available, the attempt entry MUST include `error` with at least `message` and `code` when available.

RCD-D12. Capture MUST NOT redact prompt text, tool arguments, image payloads, or provider response bodies because the feature is explicitly a raw diagnostic dump. Operators MUST keep the feature disabled unless they accept that sensitive payloads are persisted.

RCD-D13. One session MUST retain no more than the configured attempt count and no more than the effective serialized session byte count. Capacity validation and filesystem writing MUST use the same compact UTF-8 JSON byte vector; the writer MUST NOT re-encode the accepted payload. An attempt that cannot fit MAY be replaced by bounded identifying fields plus explicit `capture_truncation` metadata. A capture-all request that recorded an attempt MUST retain at least one full or placeholder attempt and MUST persist the bounded envelope. Further attempts MUST be counted as omitted rather than appended.

RCD-D14. One captured downstream SSE frame MUST retain no more than the configured frame byte count. One session MUST retain no more than the configured frame count and total session byte count. Truncation MUST occur on a valid UTF-8 boundary.

RCD-D15. The top-level `capture_truncation` object MUST include `truncated`, `omitted_attempts`, `omitted_frames`, `omitted_bytes`, and `retained_bytes`. Each attempt whose frame list was truncated MUST include `downstream_sse_frames_truncation` with the corresponding omitted counts. An oversized-attempt placeholder that removes retained frames MUST preserve prior frame-omission counts, add the removed retained-frame count and byte count, and report zero retained frames and bytes. A non-truncated dump MUST include zero counts.

RCD-D16. A multipart capture object is the JSON representation of one `multipart/form-data` body. It MUST have exactly this shape:

```json
{
  "content_type": "multipart/form-data",
  "parts": [ MultipartTextPart | MultipartFilePart, ... ]
}
```

`parts[]` holds one entry per form part, in part order (wire order for RCD-D4a, sent order for RCD-D6a). A text part MUST be:

```json
{ "name": string, "text": string }
```

where `text` is the exact part text. A file part MUST be:

```json
{
  "name": string,
  "filename": string | null,
  "part_content_type": string | null,
  "byte_length": integer,
  "data_base64": string
}
```

where `filename` is the part's filename parameter (`null` when the part carried none), `part_content_type` is the part's `Content-Type` value (`null` when the part carried none), `data_base64` is the RFC 4648 standard (padded, non-URL-safe) base64 encoding of the exact part bytes, and `byte_length` is the decoded byte count. A part is a file part iff it contains `data_base64`.

RCD-D17. Multimodal payload retention. Image, audio, and document payloads (base64 strings, `data:` URLs, remote URLs, and RCD-D16 file parts) contained in `raw_input`, `transformed_urp_request`, `upstream_request`, `downstream_response`, or `reconstructed_urp_response` MUST be retained exactly as they appear in the source body: capture MUST NOT truncate, recompress, resample, or replace a multimodal payload with a reference (this restates RCD-D12 for multimodal content). Within those JSON fields, the only mechanism that removes multimodal content is the RCD-D13 whole-attempt bound: an attempt either retains its bodies verbatim or is replaced by the oversized-attempt placeholder. Within captured SSE frames, the RCD-D14 per-frame byte bound applies to the frame string as an opaque whole and MAY cut through an embedded payload; the omission is accounted in `downstream_sse_frames_truncation`.

## 4. Retention cleanup and size rotation

### 4.1 Per-key TTL

RCD-R1. Every metadata row (section 5) stores `expires_at_unix_ms = created_at_unix_ms + retention_seconds * 1000`, where `retention_seconds` is the RCD-C5c duration of the authenticated API key's `request_capture_retention` value resolved at request-authentication time. Changing a key's retention affects only captures persisted after the change.

RCD-R2. A cleanup pass runs at process startup and then periodically. The default periodic interval MUST be 1 hour. Each pass executes, in order: the TTL step (RCD-R3), the orphan step (RCD-R5a), and the size-budget step (RCD-R7).

RCD-R3. TTL step: the pass MUST select every metadata row with `expires_at_unix_ms <= now_ms`, delete each row's dump file (a missing file is not an error), and then delete those rows. File names MUST be processed in batches of at most 400 per SQL statement.

RCD-R4. Cleanup failure MUST be logged and MUST NOT stop process startup or request handling. Row-deletion failure MUST NOT stop file deletion and vice versa.

RCD-R5. Cleanup MUST only delete regular files directly under the dump directory. It MUST NOT recurse into subdirectories.

### 4.2 Orphan files

RCD-R5a. Orphan step: the pass MUST delete every regular file directly under the dump directory that satisfies both conditions:

1. its file name has no `request_capture_records` row, and
2. its modification time is older than 86400 s relative to pass execution time.

This bounds dumps that never receive a metadata row (sessions without a request id per RCD-M3, pre-metadata version-1 files, and abandoned temporary files) to a fixed 24-hour horizon. Row-existence checks MUST use set-based queries with at most 400 file names per query.

### 4.3 Global size budget

RCD-R6. The global size budget is `monoize_request_capture_max_total_bytes` (RCD-C3/RCD-C4) read at pass execution time. A budget of `0` disables the size-budget step. The budget measures `SUM(size_bytes)` over all `request_capture_records` rows; unregistered files are bounded by RCD-R5a instead.

RCD-R7. Size-budget step: while the metadata size total exceeds a non-zero budget, Monoize MUST delete registered dumps strictly oldest-first, ordered by `(created_at_unix_ms ASC, file_name ASC)`, deleting each victim's dump file and then its metadata row, until the total is at most the budget. Victim selection MUST use batches of at most 400 rows per query.

RCD-R8. In addition to RCD-R2 passes, the background write task of RCD-Z3 MUST run one RCD-R7 size-budget step after its metadata insert succeeds. The budget can therefore be exceeded only transiently, between a dump write and the completion of that step.

## 5. Capture metadata records

RCD-M1. Capture metadata MUST be stored in table `request_capture_records` with columns:

- `file_name: TEXT PRIMARY KEY` (the dump filename from RCD-S6, without directory components)
- `request_id: TEXT NOT NULL` (the canonical Monoize request id recorded in the dump, after RCD-C15 truncation)
- `user_id: TEXT NOT NULL`
- `api_key_id: TEXT NOT NULL`
- `created_at: TEXT NOT NULL` (RFC3339, equal to the dump `created_at`)
- `created_at_unix_ms: BIGINT NOT NULL` (same instant as Unix epoch milliseconds)
- `size_bytes: BIGINT NOT NULL` (the on-disk byte length of the persisted dump file, RCD-Z8)
- `expires_at_unix_ms: BIGINT NOT NULL` (per-key TTL deadline, RCD-R1)

RCD-M2. The table MUST have an index on `(user_id, request_id)`, an index on `(created_at_unix_ms)`, and an index on `(expires_at_unix_ms)`.

RCD-M3. Immediately after a dump file write succeeds (RCD-S11), and only when the capture session has a request id, Monoize MUST insert one `request_capture_records` row describing that file. Insert conflicts on `file_name` MUST upsert. A session without a request id MUST write no metadata row.

RCD-M4. Metadata insert failure MUST be logged and MUST NOT delete the dump file and MUST NOT change the HTTP response returned to the downstream client.

RCD-M5. `request_capture_records` rows have no foreign keys. Deleting a user, an API key, or a request-log row MUST NOT delete capture metadata rows; only TTL cleanup (RCD-R3), size rotation (RCD-R7/RCD-R8), and stale-record cleanup (`request-capture-viewer.spec.md` RCV-A8) delete them.

## 6. Migration

RCD-M6. One schema migration MUST:

1. add `api_keys.request_capture_retention TEXT NOT NULL DEFAULT '24h'` (existing keys therefore migrate to the `"24h"` default),
2. add `request_capture_records.expires_at_unix_ms BIGINT NOT NULL DEFAULT 0`,
3. backfill `expires_at_unix_ms = created_at_unix_ms + 86400000` for rows where it is `0` (existing captures inherit the 24-hour default), and
4. create the `expires_at_unix_ms` index of RCD-M2.

RCD-M7. The same migration MUST delete the `system_settings` row with `key = 'monoize_request_capture_retention_days'`. No compatibility alias for that setting remains.
