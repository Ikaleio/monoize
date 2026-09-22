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

IG1. Known image options MUST map to typed `UrpRequest.image_generation` fields. These fields are `size`, `quality`, `background`, `output_format`, `output_compression`, `moderation`, `style`, `response_format`, `partial_images`, and `input_fidelity`. Monoize MUST remove their copies from `extra_body`. Unknown fields MUST remain in `extra_body`. `user` MUST map to `UrpRequest.user` and MUST NOT remain in `extra_body`.

IG2. `response_format` MUST map to `UrpRequest.image_generation.response_format`. Native OpenAI Image attempts MUST preserve this option. Downstream responses MUST preserve the source representation returned by the upstream under §5.

IG3. `stream` MUST be a JSON boolean or null. Missing and null values select `false`. Other types MUST return HTTP 400. The parsed field MUST be excluded from `extra_body`. A missing or null `n` selects `1`.

IG4. When `stream = true`, `n` MUST equal 1. Monoize MUST reject `stream = true` with `n > 1` with HTTP 400.

IG5. `partial_images` MUST map to the typed image options and MUST be an integer from 0 through 3 when non-null. `output_compression` MUST be an integer from 0 through 100 when non-null. Other known options MUST use their declared JSON types. Null options select absence. Invalid option types or ranges MUST return HTTP 400.

### 3.2 `POST /v1/images/edits`

The request body MUST use `multipart/form-data` or JSON. Other content types MUST return HTTP 415. For multipart requests, Monoize MUST parse these fields:

| Field | Type | Required | Description |
|---|---|---|---|
| `prompt` | text field | YES | Text prompt describing the edit. |
| `model` | text field | YES | Logical model name for routing. |
| `image` | file field(s) | YES | Source image(s) to edit. One or more file parts named `image` or `image[]`, in wire order. The first part is the primary image; every additional part is an extra source image (IM10). |
| `mask` | file field | NO | Mask image indicating edit region. Single file upload. |
| `n` | text field | NO | Number of images to generate. Default `1`. MUST be ≥ 1 when present. |
| `stream` | text field | NO | `true` or `false`. Default `false`. When `true`, the downstream response is the SSE stream defined in §5.5. |

IE1. Known image options MUST map to typed image options under IG1 and IG5. Multipart `user` MUST retain its exact text and map to `UrpRequest.user`. Other text values that are JSON numbers or booleans MUST use their JSON types. Remaining unknown text fields MUST remain in `extra_body`.

IE2. File fields other than `image`, `image[]`, and `mask` MUST be ignored.

IE2a. An edit request MUST contain 1 through 16 source images. A multipart request MUST NOT contain more than one mask part. These violations MUST return HTTP 400.

IE2b. JSON edits MUST contain required string fields `prompt` and `model`, and a required `images` array. Each array item MUST contain exactly one non-empty string reference: `image_url` or `file_id`. `image_url` MUST be an HTTP(S) URL or a Base64 data URL. A non-null `mask` MUST use the same reference object shape. Invalid references MUST return HTTP 400. JSON `n`, `stream`, options, and `user` follow generation parsing rules. Unknown top-level JSON fields MUST remain in `extra_body`.

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

IM3a. When the downstream request has `stream = true`, the single sub-request (IG4) MUST initially use `UrpRequest.stream = Some(true)`. Downstream SSE MUST remain active independently of upstream transport. Native upstream SSE and synthetic terminal events from upstream JSON MUST use the same response-phase stream transforms and §5.5 encoding. A request transform MAY select non-streaming upstream transport without changing downstream SSE.

IM4. Unknown remaining request fields map to `UrpRequest.extra_body`. Known fields MUST use their typed mappings. Every Image API sub-request MUST contain `image_generation = Some(options)`, including empty options. The downstream `n` controls fan-out and MUST NOT set the upstream image count.

IM5. `tools`, `tool_choice`, `temperature`, `top_p`, `max_output_tokens`, `reasoning`, and URP structured `response_format` MUST remain absent. `user` uses the typed mapping under IG1. Monoize MUST NOT inject tools at ingress. Users who need Responses image tools MUST configure request transforms.

IM5b. A Responses attempt MUST have a configured `image_generation` tool after request transforms. If absent, Monoize MUST reject the attempt before dispatch. A forced tool choice is optional. Without it, assistant text without images fails §5.1 validation.

IM5a. If a routed upstream provider only surfaces generated image outputs on the streaming Responses event channel and omits them from the terminal non-streaming response body, Monoize MAY internally execute the sub-request as a streaming upstream request, collect the emitted URP stream events into a final `UrpResponse`, and continue response extraction from that collected `UrpResponse`.

### 4.2 Edits mapping

For each sub-request derived from `POST /v1/images/edits`:

IM6. `model` → `UrpRequest.model`.

IM7. Each multipart source file MUST map to a user `Node::Image` with a Base64 source. JSON HTTP(S) references MUST map to URL sources. JSON data URLs MUST map to Base64 sources. JSON file IDs MUST map to FileId sources with unbound `MediaResource` provenance for `ProviderProtocol::OpenaiImage`.

IM8. An optional mask MUST map to the final user image node. Its typed `MediaMetadata.image_mask` MUST be true. Node IDs MUST NOT encode mask semantics. Mask source representations follow IM7.

IM9. `prompt` MUST be mapped to one `Node::Text` with `role: User`, before the image node(s).

IM10. Node order in `UrpRequest.input` MUST be: `[prompt_text, image, extra_image*, mask?]`.

IM11. When the downstream request has `stream` absent or `false`, IM3 applies to edit sub-requests unchanged. When the downstream request has `stream = true`, IM3a applies to the single edit sub-request.

IM12. Remaining edit fields follow IM4. `prompt`, `model`, `n`, `stream`, `image`, `image[]`, `images`, and `mask` MUST NOT remain in `extra_body`.

IM13. Same as IM5: no `tools`/`tool_choice` injection.

### 4.3 Sub-request fan-out for `n > 1`

IM14. When `n > 1`, Monoize MUST issue `n` independent non-streaming URP forwarding sub-requests concurrently (using `tokio::JoinSet` or equivalent).

IM15. Each sub-request MUST go through the full forwarding pipeline independently: auth transforms, provider routing, upstream call, response transforms, billing, and request logging. Each sub-request is billed as one independent request.

IM16. Partial success policy:

- If all `n` sub-requests fail, Monoize MUST return the error from the last failed sub-request.
- If at least one sub-request succeeds, Monoize MUST return a successful response containing only the successful results. Failed sub-requests MUST be silently excluded from the `data[]` array.

IM17. The order of items in the response `data[]` array is not required to match the order of sub-requests. Results MAY appear in completion order.

IM18. Request capture for Image API sub-requests follows `request-capture-dumps.spec.md` RCD-C16 (one capture session per sub-request), RCD-D4a (multipart `raw_input` for multipart edits, original JSON for JSON edits), RCD-D2c (`is_stream` equals the parsed downstream `stream` flag), and RCD-D10c (stream-collected reconstruction for the IM3 internal-stream path and the IM3a streaming path).

## 5. Response mapping

### 5.1 Image extraction from URP response

IR1. For each successful sub-request, Monoize MUST scan the URP response `output` for assistant `Node::Image` nodes.

IR2. For each `Node::Image` found:

- `ImageSource::Base64 { data, .. }` → use `data` as `b64_json`.
- `ImageSource::Url { url, .. }` → use `url` as `url` field in the response data item. If the downstream request did not specify `response_format: "url"`, Monoize MUST still include the URL as-is (no download/re-encoding).

IR3. If a sub-request succeeds but produces zero assistant `Node::Image` nodes, Monoize MUST scan for assistant `Node::Text` nodes and attempt to extract text content. If the URP response contains no extractable image, that sub-request MUST be treated as failed for the purpose of IM16.

IR3a. Image extraction MUST skip Base64 sources whose data is empty or whitespace-only, and URL sources whose URL is empty or whitespace-only. These nodes MUST NOT satisfy IR3 validation.

IR4. Each image's typed `image_generation.revised_prompt` MUST supply its `revised_prompt`, including an empty string. If absent, concatenated assistant text MAY supply the fallback. If both sources are absent, Monoize MUST omit `revised_prompt`. Identical images MUST remain separate data items.

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

IR8a. Generated image metadata MUST use typed `MediaMetadata.image_generation` fields: `quality`, `size`, `background`, `output_format`, and `model`. Each field is an optional upstream-reported string. The non-streaming response MUST emit a field at the top level only when every returned image reports the same non-absent value. Missing or conflicting values MUST be omitted. Request parameters, routing aliases, and MIME defaults MUST NOT supply missing response metadata.

IR8b. Completed downstream image SSE events MUST emit the corresponding image's typed generation metadata. Decoding a streaming upstream for a non-streaming downstream MUST preserve the same metadata as direct non-streaming decoding. Changes or deletions to typed metadata MUST override adapter extras.

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

IS1. When `stream = true`, Monoize MUST validate the request, authenticate, enforce model permissions, and check candidate-route balance before opening SSE. Preflight failures MUST use §5.4 JSON errors. Only then MAY Monoize return HTTP 200 with `Content-Type: text/event-stream` and execute IM3a.

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
- `revised_prompt` when available under IR4, including an empty string;
- upstream-reported generation fields from typed image metadata, with output format reconciled against the current source;
- `created_at`: the Unix timestamp (seconds) at emission;
- on the last completed frame only: `usage` in the IR10 shape, present iff the sub-request produced URP `Usage`.

IS5. Termination: after the last completed frame, or after the IS6 error frame, Monoize MUST emit `data: [DONE]` and close the stream.

IS6. Error frames: if the sub-request fails before any completed frame was emitted (routing exhaustion, upstream error, zero extracted images per IR3, billing rejection), Monoize MUST emit exactly one frame with SSE event name `error` and JSON data `{"type": "error", "error": {"message": <string>, "type": <string>, "code": <string>}}`, followed by IS5 termination. The HTTP status remains 200.

IS7. Attempt failover for the streaming sub-request is allowed only until the first partial or completed frame has been emitted downstream. After any image frame has been emitted, a failing attempt terminates the stream per IS6 without retry.

IS8. The streaming sub-request is billed and request-logged as one request with `is_stream = true`, using the same billing pipeline as §6.3. `request_kind` follows RL2.

IS9. Streaming sub-requests SHOULD request upstream SSE. OpenRouter Image attempts with user image inputs MUST instead request non-streaming upstream transport. OpenAI Image generations use JSON. All-inline edits use multipart. Edits containing URL or FileId references use JSON under `openai-image-upstream.spec.md` OIU-S7.

IS10. After successful preflight, Monoize MUST immediately send an SSE `heartbeat` comment. It MUST send another comment after 15 seconds without a data event. Responses MUST set `Cache-Control: no-cache, no-transform` and `X-Accel-Buffering: no`. Heartbeats MUST NOT count as image frames for IS7.

IS11. An upstream HTTP 400 or 422 that explicitly rejects streaming MAY trigger exactly one non-streaming resend of that attempt before any image frame. Recognized signals are `streaming_not_supported`, `unsupported_streaming`, an unsupported or unknown `stream` parameter, or an explicit message that streaming is unsupported. The resend MUST remove `stream` and `partial_images` from the wire request. It MUST preserve the model, prompt, images, other options, authentication, and transforms. This one transport negotiation is independent of configured retries. Each physical request MUST have its own attempt number and capture entry. Negotiation MUST NOT mark the Channel unhealthy. Other HTTP failures, network failures, empty outputs, and interrupted streams MUST NOT trigger this negotiation.

IS12. A successful upstream `application/json` response MUST be decoded as a normal provider response, even when streaming was requested. The decoder MUST produce one typed terminal response for response transforms, validation, billing, and completion frames. It MUST NOT manufacture partial images. Synthetic streams MUST retain upstream usage, metadata, image multiplicity, and error handling. Downstream logs MUST retain `is_stream = true`, and billing MUST occur once.

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

RT3. OpenAI Image edit attempts MUST target `POST /v1/images/edits`. All-inline inputs use multipart. Inputs containing URL or FileId references use JSON, including data URLs for any accompanying inline images. The optional mask follows the same encoding choice.

RT4. If an Image API generation sub-request routes to an attempt with effective upstream type `openai_image`, and the mapped URP request contains no user-role image nodes, Monoize MUST keep the existing JSON upstream encoding and upstream path `POST /v1/images/generations`.

### 6.3 Billing

BL1. Each sub-request is billed independently through the existing billing pipeline.

BL2. For `n = 3`, the user is billed for 3 separate forwarding requests.

### 6.4 Request logging

RL1. Each sub-request MUST produce its own request log entry through the existing request logging pipeline.

RL2. API-key Image API requests MUST record `request_kind = "image_generation"` for generations and `"image_edit"` for edits. A pre-existing internal source, including Playground, MUST retain its classification.

## 7. Observability

OB1. Monoize MUST log the downstream Image API request shape at INFO level before fan-out, including:

- logical model;
- `n` value;
- endpoint type (generations or edits);
- for edits: source image count, inline byte size estimate, reference count, and whether a mask is present. Logs MUST NOT include prompts, image content, URLs, file IDs, or credentials.

OB2. Each sub-request's upstream call observability follows existing FP4b/FP4c requirements.

## 8. Constraints

CO1. The downstream response transport is selected only by the parsed `stream` field: absent or `false` yields one JSON response (§5.1–§5.4); `true` yields the §5.5 SSE stream. Internal upstream transport is independent of the downstream contract (IM3, IM3a).

CO2. Monoize MUST NOT implement `POST /v1/images/variations`. Only generations and edits are supported.

CO3. The configured HTTP body limit from `unified_responses_proxy.spec.md` §C5 applies to Image API endpoints.

CO4. Image API endpoints MUST NOT be listed in `GET /v1/models` output (they are not model endpoints; they are adapters).

RT5. An `openrouter_image` attempt MUST encode generation and edit sub-requests as JSON to `POST /v1/images`.
It MUST map source images to `input_references` and reject masks according to `openrouter-image-upstream.spec.md`.
The same rule applies to IS9 streaming attempts.
