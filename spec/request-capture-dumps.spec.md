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

RCD-C6. API key rows MUST include `request_capture_enabled: boolean`.

RCD-C7. The default value of `request_capture_enabled` for newly created API keys MUST be `false`.

RCD-C8. If an existing API key row has no stored `request_capture_enabled` value, runtime MUST treat it as `false`.

RCD-C9. A forwarding request is capture-eligible iff all conditions are true:

1. the request is authenticated by a dashboard-managed API key;
2. `monoize_request_capture_enabled == true` at request-processing time;
3. the authenticated API key has `request_capture_enabled == true`.

RCD-C10. If `monoize_request_capture_enabled == false`, no dump file MUST be written even when the authenticated API key has `request_capture_enabled == true`.

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

- `version: 1`
- `request_id: string?`
- `created_at: RFC3339 string`
- `api_key_id: string`
- `user_id: string`
- `downstream_protocol: string`
- `is_stream: boolean`
- `attempts: object[]`

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
- `error: object?`

RCD-D4. `raw_input` MUST be the parsed downstream JSON request body as received by the forwarding handler, before conversion to URP and before request transforms.

RCD-D5. `transformed_urp_request` MUST be the URP request after provider request transforms, global request transforms, API-key request transforms, Monoize context removal, and reasoning-envelope upstream filtering.

RCD-D6. `upstream_request` MUST be the provider-native JSON object sent as the upstream HTTP request body.

RCD-D7. For a non-streaming upstream response, `downstream_response` MUST be the provider raw response JSON object returned by the upstream HTTP response body before Monoize decodes it to URP.

RCD-D8. For a buffered synthetic stream, `downstream_response` MUST be the provider raw response JSON object returned by the upstream HTTP response body before Monoize decodes it to URP.

RCD-D9. For a pass-through streaming response, `downstream_sse_frames` MUST contain the SSE frame data strings emitted to the downstream client in emission order after response transforms and downstream encoding.

RCD-D9a. If downstream SSE frame emission occurs inside asynchronous tasks spawned by the pass-through streaming pipeline, all such tasks MUST record emitted frames into the same per-attempt `downstream_sse_frames` array.

RCD-D10. For a pass-through streaming response, `downstream_response` MUST be null or absent.

RCD-D11. If an upstream call fails before a response body is available, the attempt entry MUST include `error` with at least `message` and `code` when available.

RCD-D12. Capture MUST NOT redact prompt text, tool arguments, image payloads, or provider response bodies because the feature is explicitly a raw diagnostic dump. Operators MUST keep the feature disabled unless they accept that sensitive payloads are persisted.

## 4. Retention cleanup

RCD-R1. On startup, Monoize SHOULD delete dump files whose modification time is older than `monoize_request_capture_retention_days` days relative to cleanup execution time.

RCD-R2. While running, Monoize MUST periodically delete dump files whose modification time is older than `monoize_request_capture_retention_days` days relative to cleanup execution time.

RCD-R3. The default periodic cleanup interval MUST be 1 hour.

RCD-R4. Cleanup failure MUST be logged and MUST NOT stop process startup or request handling.

RCD-R5. Cleanup MUST only delete regular files directly under the dump directory. It MUST NOT recurse into subdirectories.
