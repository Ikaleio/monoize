# Image API Proxy Specification

## 0. Status

- **Subsystem:** Image API to Responses API one-way forwarding proxy.
- **Scope:** Monoize accepts downstream requests in OpenAI Image API format (`/v1/images/generations`, `/v1/images/edits`) and returns Image API responses through the existing URP forwarding pipeline. When the downstream request sets `stream = true`, the response is an SSE stream of OpenAI image stream events (§5.5). Otherwise the response is one non-streaming JSON body, and internal upstream transport MAY be non-streaming or streaming when required to recover provider-native image outputs.
- **Dependency:** This spec extends `unified_responses_proxy.spec.md` §2.2 and §5.

## 1. Terminology

- **Image API:** The OpenAI Images API shape (`POST /v1/images/generations`, `POST /v1/images/edits`).
- **Downstream Image Request:** A request to Monoize in Image API format.
- **Sub-request:** One URP forwarding request derived from a downstream Image API request. A single downstream Image API request with `n > 1` produces multiple sub-requests.
- **Image stream event family:** `image_generation` for `/v1/images/generations`, `image_edit` for `/v1/images/edits`. The family selects the downstream SSE event names in §5.5.

## 2. Endpoints

### 2.1 New forwarding endpoints

Monoize MUST implement:

- `POST /v1/images/generations` — text-to-image generation.
- `POST /v1/images/edits` — image editing with prompt and source image(s).

IA-AP1. For every endpoint above, Monoize MUST also accept the same request at `/api` + endpoint path (e.g. `/api/v1/images/generations`), with identical semantics. This follows `unified_responses_proxy.spec.md` §2.2 alias rule AP1.

### 2.2 Authentication and guards

IA-A1. Both endpoints MUST require forwarding API-key authentication per `unified_responses_proxy.spec.md` §2.1.

IA-A2. Both endpoints MUST enforce balance guard per `unified_responses_proxy.spec.md` §2.1.1.

IA-A3. Both endpoints MUST enforce quota guard.

IA-A4. Both endpoints MUST enforce model allowlist per API key `model_limits`.

IA-A5. Both endpoints MUST apply API-key and global model redirects according
to `api-key-model-redirects.spec.md` before IA-A4.

## 3. Request parsing

### 3.1 `POST /v1/images/generations`

Request body MUST be JSON. Monoize MUST parse the following fields:

| Field | Type | Required | Default | Description |
|---|---|---|---|---|
| `prompt` | string | YES | — | Text prompt for image generation. |
| `model` | string | YES | — | Logical model name for routing. |
| `n` | integer | NO | `1` | Number of images to generate. MUST be ≥ 1. |
| `stream` | boolean | NO | `false` | When `true`, the downstream response is the SSE stream defined in §5.5. |

IG1. All other fields present in the request body (including but not limited to `size`, `quality`, `background`, `output_format`, `output_compression`, `moderation`, `style`, `response_format`, `partial_images`, `user`) MUST be preserved as URP `extra_body` fields on the generated URP request. Monoize MUST NOT interpret, validate, or reject these fields.

IG2. `response_format` field: Monoize MUST NOT interpret this field. It is preserved in `extra_body` and subject to the same whitelist filtering as other extra fields (per `unified_responses_proxy.spec.md` §7.7.1). The downstream non-streaming Image API response always uses `b64_json` format (see §5).

IG3. `stream` field: if present and not a JSON boolean, Monoize MUST reject the request with HTTP 400. The parsed value MUST be excluded from `extra_body`.

IG4. When `stream = true`, `n` MUST equal 1. Monoize MUST reject `stream = true` with `n > 1` with HTTP 400.

IG5. `partial_images` field: Monoize preserves it in `extra_body` per IG1. For attempts whose effective upstream type is `openai_image`, the field passes the OIU-E6 whitelist and is forwarded to the upstream request body.

### 3.2 `POST /v1/images/edits`

Request body MUST be `multipart/form-data`. Monoize MUST parse the following fields:

| Field | Type | Required | Description |
|---|---|---|---|
| `prompt` | text field | YES | Text prompt describing the edit. |
| `model` | text field | YES | Logical model name for routing. |
| `image` | file field(s) | YES | Source image(s) to edit. One or more file parts named `image` or `image[]`, in wire order. The first part is the primary image; every additional part is an extra source image (IM10). |
| `mask` | file field | NO | Mask image indicating edit region. Single file upload. |
| `n` | text field | NO | Number of images to generate. Default `1`. MUST be ≥ 1 when present. |
| `stream` | text field | NO | `true` or `false`. Default `false`. When `true`, the downstream response is the SSE stream defined in §5.5. |

IE1. All other text fields present in the multipart body (including but not limited to `size`, `quality`, `background`, `output_format`, `output_compression`, `moderation`, `input_fidelity`, `partial_images`, `user`) MUST be preserved as URP `extra_body` fields. String values that are valid JSON numbers or booleans MUST be preserved as their JSON-typed equivalents; all other string values MUST be preserved as JSON strings.

IE2. File fields other than `image`, `image[]`, and `mask` MUST be ignored.

IE4. `stream` text field: any value other than exactly `true` or `false` MUST be rejected with HTTP 400. The parsed value MUST be excluded from `extra_body`. When `stream = true`, `n` MUST equal 1 (same rule as IG4).

IE3. File upload processing:

- For each uploaded file (`image`, `mask`), Monoize MUST read the file bytes and base64-encode them.
- The media type MUST be determined from the `Content-Type` header of the multipart part. If absent, Monoize MUST infer from file extension or default to `application/octet-stream`.
- Maximum individual file size is bounded by the configured HTTP body limit (`unified_responses_proxy.spec.md` §C5), whose default is 50 MiB.

## 4. Request mapping to URP

### 4.1 Generations mapping

For each sub-request derived from `POST /v1/images/generations`:

IM1. `model` → `UrpRequest.model` (used for routing).

IM2. `prompt` → `UrpRequest.input` as one `Node::Text` with `role: User` and the prompt string.

IM3. When the downstream request has `stream` absent or `false`, the downstream contract is non-streaming. Monoize MAY use either `stream: Some(false)` or `stream: Some(true)` on the internal upstream URP request, provided the final downstream response remains a single non-streaming Image API JSON response. If a request-phase transform sets `stream = true`, Monoize MUST collect the upstream stream internally and MUST NOT return downstream SSE for a non-streaming downstream request.

IM3a. When the downstream request has `stream = true`, the single sub-request (IG4) MUST be built with `UrpRequest.stream = Some(true)` and MUST execute through the URP streaming pipeline: upstream SSE decode, response-phase stream transforms, then the §5.5 downstream encoding. Request-phase transforms MUST NOT downgrade the sub-request to non-streaming output; `stream` is forced to `Some(true)` on the attempt request after request-phase transforms.

IM4. All remaining fields from the request body → `UrpRequest.extra_body`. The fields `prompt`, `model`, `n`, and `stream` MUST be excluded from `extra_body`.

IM5. `tools`, `tool_choice`, `temperature`, `top_p`, `max_output_tokens`, `reasoning`, `response_format`, and `user` on the URP request MUST be left as `None`/absent. Monoize MUST NOT inject any `tools` or `tool_choice` values at Image API request mapping time. Users who need specific tool injection (e.g. `image_generation` tool for OpenAI Responses upstream) MUST configure request-phase transforms on the provider or API key.

IM5b. If the selected upstream attempt has effective upstream type `responses`, the Images API compatibility path SHOULD use a request-phase transform that inserts a Responses `image_generation` tool and forces a specific `tool_choice` for that tool. Without a forced tool choice, a text-capable Responses model MAY return only assistant text, which produces no Image API data item under §5.1.

IM5a. If a routed upstream provider only surfaces generated image outputs on the streaming Responses event channel and omits them from the terminal non-streaming response body, Monoize MAY internally execute the sub-request as a streaming upstream request, collect the emitted URP stream events into a final `UrpResponse`, and continue response extraction from that collected `UrpResponse`.

### 4.2 Edits mapping

For each sub-request derived from `POST /v1/images/edits`:

IM6. `model` → `UrpRequest.model`.

IM7. The `image` file MUST be mapped to one `Node::Image` with `role: User` and `ImageSource::Base64 { media_type, data }`.

IM8. If `mask` is present, the mask file MUST be mapped to a second `Node::Image` with `role: User` and `ImageSource::Base64 { media_type, data }`, after the source image.

IM9. `prompt` MUST be mapped to one `Node::Text` with `role: User`, before the image node(s).

IM10. Node order in `UrpRequest.input` MUST be: `[prompt_text, image, extra_image*, mask?]`.

IM11. When the downstream request has `stream` absent or `false`, IM3 applies to edit sub-requests unchanged. When the downstream request has `stream = true`, IM3a applies to the single edit sub-request.

IM12. All remaining text fields → `UrpRequest.extra_body`. The fields `prompt`, `model`, `n`, `stream`, `image`, and `mask` MUST be excluded from `extra_body`.

IM13. Same as IM5: no `tools`/`tool_choice` injection.

### 4.3 Sub-request fan-out for `n > 1`

IM14. When `n > 1`, Monoize MUST issue `n` independent non-streaming URP forwarding sub-requests concurrently (using `tokio::JoinSet` or equivalent).

IM15. Each sub-request MUST go through the full forwarding pipeline independently: auth transforms, provider routing, upstream call, response transforms, billing, and request logging. Each sub-request is billed as one independent request.

IM16. Partial success policy:

- If all `n` sub-requests fail, Monoize MUST return the error from the last failed sub-request.
- If at least one sub-request succeeds, Monoize MUST return a successful response containing only the successful results. Failed sub-requests MUST be silently excluded from the `data[]` array.

IM17. The order of items in the response `data[]` array is not required to match the order of sub-requests. Results MAY appear in completion order.

IM18. Request capture for Image API sub-requests follows `request-capture-dumps.spec.md` RCD-C16 (one capture session per sub-request), RCD-D4a (multipart `raw_input` for edits), RCD-D2c (`is_stream` equals the parsed downstream `stream` flag), and RCD-D10c (stream-collected reconstruction for the IM3 internal-stream path and the IM3a streaming path).

## 5. Response mapping

### 5.1 Image extraction from URP response

IR1. For each successful sub-request, Monoize MUST scan the URP response `output` for assistant `Node::Image` nodes.

IR2. For each `Node::Image` found:

- `ImageSource::Base64 { data, .. }` → use `data` as `b64_json`.
- `ImageSource::Url { url, .. }` → use `url` as `url` field in the response data item. If the downstream request did not specify `response_format: "url"`, Monoize MUST still include the URL as-is (no download/re-encoding).

IR3. If a sub-request succeeds but produces zero assistant `Node::Image` nodes, Monoize MUST scan for assistant `Node::Text` nodes and attempt to extract text content. If the URP response contains no extractable image, that sub-request MUST be treated as failed for the purpose of IM16.

IR4. `revised_prompt`: If the URP response contains assistant `Node::Text` nodes alongside assistant `Node::Image` nodes, the concatenated text content of those text nodes MUST be used as `revised_prompt` for the corresponding `data[]` entry. If no assistant text nodes exist alongside images, `revised_prompt` MUST be omitted.

### 5.2 Response envelope

IR5. The downstream Image API response MUST have the following shape:

```json
{
  "created": <unix_timestamp_seconds>,
  "data": [
    {
      "b64_json": "<base64_image_data>",
      "revised_prompt": "<optional_text>"
    }
  ]
}
```

IR6. `created` MUST be the Unix timestamp (seconds) at the time the response is assembled.

IR7. `data` MUST be a JSON array. Each element corresponds to one extracted image across all successful sub-requests.

IR8. If `n = 1` and the single sub-request produces multiple assistant `Node::Image` outputs, all images MUST appear as separate entries in `data[]`.

IR9. When a `Node::Image` has `ImageSource::Url`, the data item MUST use field `url` instead of `b64_json`:

```json
{
  "url": "<image_url>",
  "revised_prompt": "<optional_text>"
}
```

### 5.3 Usage forwarding

IR10. If any successful sub-request carries URP `Usage`, the response MUST include a top-level `usage` object aggregated across all successful sub-requests:

```json
{
  "usage": {
    "input_tokens": <sum>,
    "output_tokens": <sum>,
    "total_tokens": <sum>,
    "input_tokens_details": {
      "text_tokens": <sum>,
      "image_tokens": <sum>
    },
    "output_tokens_details": {
      "image_tokens": <sum>,
      "text_tokens": <sum>
    }
  }
}
```

IR11. Token fields MUST be summed across all successful sub-requests. If a detail field is absent from a sub-request's usage, it contributes 0 to the sum.

### 5.4 Error responses

IR12. When all sub-requests fail or the request itself is invalid, Monoize MUST return a JSON error response using the standard Monoize error shape:

```json
{
  "error": {
    "message": "<description>",
    "type": "<error_type>",
    "code": "<error_code>"
  }
}
```

IR13. HTTP status codes follow existing Monoize conventions:

- `400` for invalid request body (missing prompt, invalid n, etc.).
- `401` for authentication failure.
- `402` for insufficient balance.
- `403` for model not allowed.
- `429` for quota exceeded.
- `502` for upstream errors (all sub-requests failed).

### 5.5 Streaming response mapping (`stream = true`)

IS1. When the downstream request has `stream = true` and passes §3 validation, authentication (IA-A1..IA-A3), and the model allowlist (IA-A4), Monoize MUST respond with HTTP 200 and `Content-Type: text/event-stream`, then execute the single sub-request per IM3a. Failures detected before the SSE response starts (parse errors, auth, allowlist) use the §5.4 JSON error responses.

IS2. Downstream SSE event names use the endpoint's image stream event family (§1): `image_generation.partial_image` / `image_generation.completed` for generations, `image_edit.partial_image` / `image_edit.completed` for edits. Every frame MUST carry the event name in both the SSE `event:` line and the `type` field of the JSON data.

IS3. Partial frames: for each canonical `NodeDelta` stream event with `delta = Image` and a `Base64` source received from the sub-request after response-phase stream transforms, Monoize MUST emit one `<family>.partial_image` frame whose JSON data contains:

- `type`: the event name;
- `b64_json`: the base64 image data;
- `partial_image_index`: the event's `partial_image_index` extra field when it is a non-negative integer, else the 0-based count of partial frames already emitted for the sub-request;
- `created_at`: the event's `created_at` extra field when it is an integer, else the Unix timestamp (seconds) at emission;
- each of `output_format`, `size`, `quality`, and `background` copied from the event's extra fields when present.

IS4. Completed frames: when the sub-request reaches its terminal URP response (after response-phase transforms) and that response passes IR3 validation, Monoize MUST emit one `<family>.completed` frame per extracted image (IR2), in extraction order. Each completed frame's JSON data contains:

- `type`: the event name;
- `b64_json` for a `Base64` source, or `url` for a `Url` source;
- `created_at`: the Unix timestamp (seconds) at emission;
- on the last completed frame only: `usage` in the IR10 shape, present iff the sub-request produced URP `Usage`.

IS5. Termination: after the last completed frame, or after the IS6 error frame, Monoize MUST emit `data: [DONE]` and close the stream.

IS6. Error frames: if the sub-request fails before any completed frame was emitted (routing exhaustion, upstream error, zero extracted images per IR3, billing rejection), Monoize MUST emit exactly one frame with SSE event name `error` and JSON data `{"type": "error", "error": {"message": <string>, "type": <string>, "code": <string>}}`, followed by IS5 termination. The HTTP status remains 200.

IS7. Attempt failover for the streaming sub-request is allowed only until the first partial or completed frame has been emitted downstream. After any image frame has been emitted, a failing attempt terminates the stream per IS6 without retry.

IS8. The streaming sub-request is billed and request-logged as one request with `is_stream = true`, using the same billing pipeline as §6.3. `request_kind` follows RL2.

IS9. Upstream transport for the streaming sub-request always uses upstream SSE (`UrpRequest.stream = Some(true)`), for every effective upstream type. For `openai_image` attempts the upstream request follows `openai-image-upstream.spec.md` OIU-E4 (generations JSON) or OIU-S7 (edits multipart).

## 6. Pipeline integration

### 6.1 Transform support

TR1. Image API sub-requests MUST go through the full URP transform pipeline:

- API-key request-phase transforms apply before routing.
- Provider request-phase transforms apply per attempt.
- Provider response-phase transforms apply after upstream response decode.
- API-key response-phase transforms apply after provider response transforms.

TR2. The `image_markdown_to_output` response transform is the expected mechanism for extracting images from providers that return images embedded in assistant markdown text (e.g. Gemini image models). Users MUST configure this transform on the relevant provider or API key for such providers.

TR3. Monoize MUST NOT automatically enable any transform for Image API requests. All transforms are user-configured.

### 6.2 Routing

RT1. Routing uses the `model` field from the Image API request as the logical model for provider matching, following existing routing rules (`unified_responses_proxy.spec.md` §6, `monoize-upstream-routing.spec.md`).

RT2. The provider type determines which upstream adapter encodes the URP request. The same provider type resolution used for `/v1/responses` applies.

RT3. If an Image API edit sub-request routes to an attempt with effective upstream type `openai_image`, Monoize MUST forward the source image node(s) and mask node, if present, as `multipart/form-data` to upstream `POST /v1/images/edits`. Monoize MUST NOT encode that upstream call as JSON and MUST NOT send it to upstream `POST /v1/images/generations`.

RT4. If an Image API generation sub-request routes to an attempt with effective upstream type `openai_image`, and the mapped URP request contains no user-role image nodes, Monoize MUST keep the existing JSON upstream encoding and upstream path `POST /v1/images/generations`.

### 6.3 Billing

BL1. Each sub-request is billed independently through the existing billing pipeline.

BL2. For `n = 3`, the user is billed for 3 separate forwarding requests.

### 6.4 Request logging

RL1. Each sub-request MUST produce its own request log entry through the existing request logging pipeline.

RL2. The `request_kind` field for Image API request logs MUST be `"image_generation"` for generations and `"image_edit"` for edits.

## 7. Observability

OB1. Monoize MUST log the downstream Image API request shape at INFO level before fan-out, including:

- logical model;
- `n` value;
- endpoint type (generations or edits);
- for edits: source image byte size estimate and whether mask is present.

OB2. Each sub-request's upstream call observability follows existing FP4b/FP4c requirements.

## 8. Constraints

CO1. The downstream response transport is selected only by the parsed `stream` field: absent or `false` yields one JSON response (§5.1–§5.4); `true` yields the §5.5 SSE stream. Internal upstream transport is independent of the downstream contract (IM3, IM3a).

CO2. Monoize MUST NOT implement `POST /v1/images/variations`. Only generations and edits are supported.

CO3. The configured HTTP body limit from `unified_responses_proxy.spec.md` §C5 applies to Image API endpoints.

CO4. Image API endpoints MUST NOT be listed in `GET /v1/models` output (they are not model endpoints; they are adapters).
