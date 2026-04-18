# Image API Proxy Specification

## 0. Status

- **Subsystem:** Image API to Responses API one-way forwarding proxy.
- **Scope:** Monoize accepts downstream requests in OpenAI Image API format (`/v1/images/generations`, `/v1/images/edits`) and forwards them as non-streaming Responses API requests through the existing URP forwarding pipeline.
- **Dependency:** This spec extends `unified_responses_proxy.spec.md` §2.2 and §5.

## 1. Terminology

- **Image API:** The OpenAI Images API shape (`POST /v1/images/generations`, `POST /v1/images/edits`).
- **Downstream Image Request:** A request to Monoize in Image API format.
- **Sub-request:** One non-streaming URP forwarding request derived from a downstream Image API request. A single downstream Image API request with `n > 1` produces multiple sub-requests.

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

## 3. Request parsing

### 3.1 `POST /v1/images/generations`

Request body MUST be JSON. Monoize MUST parse the following fields:

| Field | Type | Required | Default | Description |
|---|---|---|---|---|
| `prompt` | string | YES | — | Text prompt for image generation. |
| `model` | string | YES | — | Logical model name for routing. |
| `n` | integer | NO | `1` | Number of images to generate. MUST be ≥ 1. |

IG1. All other fields present in the request body (including but not limited to `size`, `quality`, `background`, `output_format`, `output_compression`, `moderation`, `style`, `response_format`, `user`) MUST be preserved as URP `extra_body` fields on the generated URP request. Monoize MUST NOT interpret, validate, or reject these fields.

IG2. `response_format` field: Monoize MUST NOT interpret this field. It is preserved in `extra_body` and subject to the same whitelist filtering as other extra fields (per `unified_responses_proxy.spec.md` §7.7.1). The downstream Image API response always uses `b64_json` format (see §5).

### 3.2 `POST /v1/images/edits`

Request body MUST be `multipart/form-data`. Monoize MUST parse the following fields:

| Field | Type | Required | Description |
|---|---|---|---|
| `prompt` | text field | YES | Text prompt describing the edit. |
| `model` | text field | YES | Logical model name for routing. |
| `image` | file field | YES | Source image to edit. Single file upload. |
| `mask` | file field | NO | Mask image indicating edit region. Single file upload. |
| `n` | text field | NO | Number of images to generate. Default `1`. MUST be ≥ 1 when present. |

IE1. All other text fields present in the multipart body (including but not limited to `size`, `quality`, `background`, `output_format`, `output_compression`, `moderation`, `user`) MUST be preserved as URP `extra_body` fields. String values that are valid JSON numbers or booleans MUST be preserved as their JSON-typed equivalents; all other string values MUST be preserved as JSON strings.

IE2. File fields other than `image` and `mask` MUST be ignored.

IE3. File upload processing:

- For each uploaded file (`image`, `mask`), Monoize MUST read the file bytes and base64-encode them.
- The media type MUST be determined from the `Content-Type` header of the multipart part. If absent, Monoize MUST infer from file extension or default to `application/octet-stream`.
- Maximum individual file size is bounded by the existing 50 MiB body limit (`unified_responses_proxy.spec.md` §C5).

## 4. Request mapping to URP

### 4.1 Generations mapping

For each sub-request derived from `POST /v1/images/generations`:

IM1. `model` → `UrpRequest.model` (used for routing).

IM2. `prompt` → `UrpRequest.input` as one `Node::Text` with `role: User` and the prompt string.

IM3. `stream` MUST be set to `Some(false)`. Image API sub-requests are always non-streaming.

IM4. All remaining fields from the request body → `UrpRequest.extra_body`. The fields `prompt`, `model`, and `n` MUST be excluded from `extra_body`.

IM5. `tools`, `tool_choice`, `temperature`, `top_p`, `max_output_tokens`, `reasoning`, `response_format`, and `user` on the URP request MUST be left as `None`/absent. Monoize MUST NOT inject any `tools` or `tool_choice` values. Users who need specific tool injection (e.g. `image_generation` tool for OpenAI Responses upstream) MUST configure request-phase transforms on the provider or API key.

### 4.2 Edits mapping

For each sub-request derived from `POST /v1/images/edits`:

IM6. `model` → `UrpRequest.model`.

IM7. The `image` file MUST be mapped to one `Node::Image` with `role: User` and `ImageSource::Base64 { media_type, data }`.

IM8. If `mask` is present, the mask file MUST be mapped to a second `Node::Image` with `role: User` and `ImageSource::Base64 { media_type, data }`, after the source image.

IM9. `prompt` MUST be mapped to one `Node::Text` with `role: User`, after the image node(s).

IM10. Node order in `UrpRequest.input` MUST be: `[image, mask?, prompt_text]`.

IM11. `stream` MUST be set to `Some(false)`.

IM12. All remaining text fields → `UrpRequest.extra_body`. The fields `prompt`, `model`, `n`, `image`, and `mask` MUST be excluded from `extra_body`.

IM13. Same as IM5: no `tools`/`tool_choice` injection.

### 4.3 Sub-request fan-out for `n > 1`

IM14. When `n > 1`, Monoize MUST issue `n` independent non-streaming URP forwarding sub-requests concurrently (using `tokio::JoinSet` or equivalent).

IM15. Each sub-request MUST go through the full forwarding pipeline independently: auth transforms, provider routing, upstream call, response transforms, billing, and request logging. Each sub-request is billed as one independent request.

IM16. Partial success policy:

- If all `n` sub-requests fail, Monoize MUST return the error from the last failed sub-request.
- If at least one sub-request succeeds, Monoize MUST return a successful response containing only the successful results. Failed sub-requests MUST be silently excluded from the `data[]` array.

IM17. The order of items in the response `data[]` array is not required to match the order of sub-requests. Results MAY appear in completion order.

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

## 6. Pipeline integration

### 6.1 Transform support

TR1. Image API sub-requests MUST go through the full URP transform pipeline:

- API-key request-phase transforms apply before routing.
- Provider request-phase transforms apply per attempt.
- Provider response-phase transforms apply after upstream response decode.
- API-key response-phase transforms apply after provider response transforms.

TR2. The `assistant_markdown_images_to_output` response transform is the expected mechanism for extracting images from providers that return images embedded in assistant markdown text (e.g. Gemini image models). Users MUST configure this transform on the relevant provider or API key for such providers.

TR3. Monoize MUST NOT automatically enable any transform for Image API requests. All transforms are user-configured.

### 6.2 Routing

RT1. Routing uses the `model` field from the Image API request as the logical model for provider matching, following existing routing rules (`unified_responses_proxy.spec.md` §6, `monoize-upstream-routing.spec.md`).

RT2. The provider type determines which upstream adapter encodes the URP request. The same provider type resolution used for `/v1/responses` applies.

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

CO1. Image API endpoints are non-streaming only. `stream` field in the Image API request body, if present, MUST be ignored. Sub-requests always use `stream = false`.

CO2. Monoize MUST NOT implement `POST /v1/images/variations`. Only generations and edits are supported.

CO3. The 50 MiB body limit from `unified_responses_proxy.spec.md` §C5 applies to Image API endpoints.

CO4. Image API endpoints MUST NOT be listed in `GET /v1/models` output (they are not model endpoints; they are adapters).
