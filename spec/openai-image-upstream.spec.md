# OpenAI Image Upstream Type Specification

## 0. Status

- **Subsystem:** OpenAI Image upstream provider type.
- **Scope:** Monoize accepts downstream requests via any supported ingress endpoint (`/v1/responses`, `/v1/chat/completions`, `/v1/messages`, `/v1/images/generations`, `/v1/images/edits`) and forwards them to the upstream provider configured with `provider_type = "openai_image"`. Text-only requests are sent to `POST /v1/images/generations` as JSON. Requests that contain user image inputs are sent to `POST /v1/images/edits` as `multipart/form-data`.
- **Dependency:** This spec extends `unified_responses_proxy.spec.md` §7 (Adapters) and `monoize-upstream-routing.spec.md` §2.3 (Provider).

## 1. Terminology

- **OpenAI Image upstream:** A provider whose `provider_type` is `openai_image`.
- **Upstream Image Generation Request:** The `POST /v1/images/generations` request Monoize sends to the upstream provider for text-only image generation.
- **Upstream Image Edit Request:** The `POST /v1/images/edits` request Monoize sends to the upstream provider when the URP request contains at least one user-role `Node::Image`.
- **Upstream Image Response:** The JSON response from the upstream provider containing `data[].b64_json` or `data[].url` image fields.

## 2. Provider Type Registration

OIU-1. `openai_image` MUST be a valid `provider_type` value for provider configuration. It MUST be accepted in provider create/update payloads and stored in the database.

OIU-2. `openai_image` MUST be a valid value in `api_type_overrides[].api_type`, allowing per-model override to this upstream type.

OIU-3. `openai_image` MUST appear in the frontend provider type selector alongside existing types.

## 3. Request Encoding

### 3.1 URP to Upstream Image Generation Request

OIU-E1. When `provider_type` resolves to `openai_image` and `UrpRequest.input` contains zero user-role `Node::Image` items, Monoize MUST encode the URP request as a `POST /v1/images/generations` JSON body.

OIU-E2. The upstream request body MUST include:
- `model`: from `UrpRequest.model` (after redirect).
- `prompt`: concatenation of all `Node::Text.content` from user-role nodes in `UrpRequest.input`, joined by newline.

OIU-E3. All key-value pairs from `UrpRequest.extra_body` that pass whitelist filtering MUST be merged into the upstream request body as top-level fields. Adapter-generated keys (`model`, `prompt`) take precedence over `extra_body` keys.

OIU-E4. If `UrpRequest.stream == Some(true)`, Monoize MUST include top-level JSON field `stream: true` in the upstream image request body. If `UrpRequest.stream != Some(true)`, Monoize MUST NOT include a `stream` field in the upstream image request body.

OIU-E5. URP fields `tools`, `tool_choice`, `temperature`, `top_p`, `max_output_tokens`, `reasoning`, `response_format`, and `user` MUST be ignored and MUST NOT appear in the upstream image request body.

### 3.2 URP to Upstream Image Edit Request

OIU-E5a. When `provider_type` resolves to `openai_image` and `UrpRequest.input` contains one or more user-role `Node::Image` items, Monoize MUST send the upstream request as `multipart/form-data` to `POST /v1/images/edits`.

OIU-E5b. The upstream edit multipart body MUST include text field `model` from `UrpRequest.model` after redirect.

OIU-E5c. The upstream edit multipart body MUST include text field `prompt` equal to the concatenation of all user-role `Node::Text.content` values in `UrpRequest.input`, joined by newline, excluding empty text after trim.

OIU-E5d. Each user-role `Node::Image` whose source is `ImageSource::Base64 { media_type, data }` MUST be decoded from base64 and included as a file field. The field name MUST be `image` unless the node has `id = "__monoize_image_api_mask"`; a node with that id MUST use field name `mask`. Monoize MUST NOT infer a mask from image position alone. The file part content type MUST equal `media_type`.

OIU-E5e. All key-value pairs from `UrpRequest.extra_body` that pass whitelist filtering MUST be included in the upstream edit multipart body as text fields, except `model`, `prompt`, and `stream`. JSON string values MUST be written without quotes. JSON number, boolean, object, array, and null values MUST be written as their JSON serialization.

OIU-E5f. If `UrpRequest.stream == Some(true)`, Monoize MUST include text field `stream` with value `true` in the upstream edit multipart body. If `UrpRequest.stream != Some(true)`, Monoize MUST NOT include a `stream` field in the upstream edit multipart body.

### 3.3 Extra Body Whitelist

OIU-E6. The default extra body whitelist for `openai_image` MUST be: `size`, `quality`, `style`, `response_format`, `n`, `background`, `output_format`, `output_compression`, `moderation`, `user`.

## 4. Response Decoding

### 4.1 Upstream Image Response to URP

OIU-D1. Monoize MUST parse the upstream response as the OpenAI Image API response shape:

```json
{
  "created": <unix_timestamp>,
  "data": [
    { "b64_json": "<base64_data>", "revised_prompt": "..." }
  ]
}
```

OIU-D2. For each entry in `data[]`:
- If `b64_json` is present: create a `Node::Image` with `role: Assistant` and `ImageSource::Base64 { media_type: "image/png", data: <b64_json> }`.
- If `url` is present (and `b64_json` is absent): create a `Node::Image` with `role: Assistant` and `ImageSource::Url { url: <url>, detail: None }`.

OIU-D3. If `revised_prompt` is present in any `data[]` entry, Monoize MUST create a `Node::Text` with `role: Assistant` and the `revised_prompt` content, placed before image nodes in source order.

OIU-D4. All extracted assistant nodes MUST be placed directly into `UrpResponse.output` in source order.

OIU-D5. The decoded `UrpResponse` MUST have:
- `id`: the string value of `created` from the upstream response, or a generated ID if absent.
- `model`: the requested model name.
- `output`: containing the assembled assistant nodes.
- `finish_reason`: `Some(FinishReason::Stop)`.
- `usage`: parsed from upstream `usage` object if present, otherwise `None`.

OIU-D6. If the upstream response contains a top-level `usage` object, Monoize MUST parse it into URP `Usage` using the same field mapping as the existing image API response handler.

## 5. Downstream Rendering

### 5.1 Responses API downstream (`/v1/responses`)

OIU-R1. When the downstream protocol is Responses, the URP response MUST be encoded using the standard Responses encoder. Assistant image nodes appear as native `output_image` items in the response.

### 5.2 Chat Completions / Messages downstream

OIU-R2. When the downstream protocol is Chat Completions or Anthropic Messages, Monoize MUST automatically convert assistant `Node::Image` outputs to inline markdown base64 images appended to assistant text content before encoding the downstream response.

OIU-R3. The markdown format MUST be: `![image](data:{media_type};base64,{data})` for base64 images, and `![image]({url})` for URL images.

OIU-R4. This automatic conversion MUST occur after response-phase transforms have been applied, so user-configured transforms can still operate on the raw `Node::Image` data.

## 6. Streaming Behavior

OIU-S1. The `openai_image` upstream type supports upstream SSE streaming when `UrpRequest.stream == Some(true)`.

OIU-S2. When decoding upstream image SSE, Monoize MUST accept event `image_generation.partial_image`. The decoder MAY ignore the event for canonical URP node emission. Ignoring that event MUST NOT be treated as a stream error.

OIU-S3. When decoding upstream image SSE, Monoize MUST accept event `image_generation.completed`. If the payload contains non-empty `b64_json` or `result`, the decoder MUST emit one assistant `Image` node with `Image.source = Base64`. The media type MUST be derived from `output_format`, defaulting to `image/png`.

OIU-S4. When upstream image SSE reaches a terminal successful event, Monoize MUST emit one canonical `ResponseDone` whose `output` contains all completed image nodes collected from the stream.

OIU-S5. When the downstream request is streaming, Monoize MAY pass upstream image SSE through the canonical URP streaming pipeline and encode downstream SSE from canonical URP events.

OIU-S6. When the downstream request is non-streaming but the selected attempt has `UrpRequest.stream == Some(true)` after request-phase transforms, Monoize MUST call the upstream image endpoint with streaming enabled, collect canonical URP stream events into one `UrpResponse`, apply response transforms, and return a normal non-streaming downstream response.

## 7. Routing Integration

OIU-RT1. The upstream path for text-only `openai_image` requests MUST be `/v1/images/generations`.

OIU-RT1a. The upstream path for `openai_image` requests containing one or more user-role `Node::Image` items MUST be `/v1/images/edits`.

OIU-RT2. `openai_image` MUST NOT require any extra request headers beyond standard auth.

OIU-RT3. Channel test (probe) for `openai_image` providers is not meaningful for image generation models. Monoize MUST use a `POST /v1/images/generations` probe with `{ "model": <probe_model>, "prompt": "test", "size": "1024x1024" }` and treat a 2xx response as success.

## 8. Dashboard Integration

OIU-UI1. The provider type selector in the dashboard MUST include `openai_image` with label `OpenAI Image` and path `/v1/images/generations`.

OIU-UI2. The provider type MUST use the OpenAI icon in the UI.

## 9. Constraints

OIU-C1. `openai_image` is a concrete upstream type, not virtual. Providers with this type MUST have `base_url` and `auth`.

OIU-C2. `openai_image` MUST appear in the `stream_upstream_to_urp_events` streaming decoder dispatch and MUST NOT return `provider_type_not_supported` for streaming image generation attempts.

OIU-C3. `n` field handling: if `n` is present in `extra_body`, it is forwarded to the upstream as-is. Monoize does NOT perform fan-out for this upstream type (unlike the Image API proxy endpoints which do their own fan-out). The upstream provider handles `n` natively.
