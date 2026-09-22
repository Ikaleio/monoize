# OpenAI Image Upstream Type Specification

## 0. Status

- **Subsystem:** OpenAI Image upstream channel type.
- **Scope:** Monoize accepts downstream requests via any supported ingress endpoint (`/v1/responses`, `/v1/chat/completions`, `/v1/messages`, `/v1/images/generations`, `/v1/images/edits`) and forwards them through an attempt whose effective upstream type is `openai_image`. Text-only requests are sent to `POST /v1/images/generations` as JSON. Requests with user image inputs use `POST /v1/images/edits`. Inline Base64 inputs use multipart. URL or file-reference inputs use JSON.
- **Dependency:** This spec extends `unified_responses_proxy.spec.md` §7 (Adapters) and `monoize-upstream-routing.spec.md` §2.3 (Provider).

## 1. Terminology

- **OpenAI Image upstream:** An upstream attempt whose effective upstream type is `openai_image`.
- **Upstream Image Generation Request:** The `POST /v1/images/generations` request Monoize sends to the upstream provider for text-only image generation.
- **Upstream Image Edit Request:** The `POST /v1/images/edits` request Monoize sends to the upstream provider when the URP request contains at least one user-role `Node::Image`.
- **Upstream Image Response:** The JSON response from the upstream provider containing `data[].b64_json` or `data[].url` image fields.

## 2. Provider Type Registration

OIU-1. `openai_image` MUST be a valid `provider_type` value for Channel configuration. It MUST be accepted in Channel create/update payloads and stored on the Channel row.

OIU-2. `openai_image` MUST be a valid value in `api_type_overrides[].api_type`, allowing per-model override to this upstream type.

OIU-3. `openai_image` MUST appear in the frontend Channel type selector alongside existing types.

## 3. Request Encoding

### 3.1 URP to Upstream Image Generation Request

OIU-E1. When `provider_type` resolves to `openai_image` and `UrpRequest.input` contains zero user-role `Node::Image` items, Monoize MUST encode the URP request as a `POST /v1/images/generations` JSON body.

OIU-E2. The upstream request body MUST include:
- `model`: from `UrpRequest.model` (after redirect).
- `prompt`: concatenation of all `Node::Text.content` from user-role nodes in `UrpRequest.input`, joined by newline.

OIU-E3. All key-value pairs from `UrpRequest.extra_body` that pass whitelist filtering MUST be merged into the upstream request body as top-level fields. Adapter-generated keys (`model`, `prompt`, `stream`, `images`, `image`, and `mask`) take precedence over extra fields.

OIU-E3a. When `UrpRequest.image_generation` is present, its typed fields MUST supply the image options. The encoder MUST ignore same-named extra fields, including when a typed field is absent. `UrpRequest.user` MUST supply `user`; an absent typed user MUST remove any extra-body user value.

OIU-E3b. Every native Images encoding path MUST validate typed option ranges and media references after request transforms. Invalid data URLs, Base64 data, MIME types, and incompatible file provenance MUST fail before upstream dispatch.

OIU-E4. If `UrpRequest.stream == Some(true)`, Monoize MUST include top-level JSON field `stream: true` in the upstream image request body. If `UrpRequest.stream != Some(true)`, Monoize MUST NOT include a `stream` field in the upstream image request body.

OIU-E5. URP fields `tools`, `tool_choice`, `temperature`, `top_p`, `max_output_tokens`, `reasoning`, and the text `response_format` MUST be ignored. The typed image `response_format` option MUST remain independent of the text response format.

### 3.2 URP to Upstream Image Edit Request

OIU-E5a. When the request contains user-role image nodes, Monoize MUST send the request to `POST /v1/images/edits`. If every image source is Base64, the body MUST use `multipart/form-data`. If any source is a URL or file reference, the body MUST use JSON.

OIU-E5b. The upstream edit multipart body MUST include text field `model` from `UrpRequest.model` after redirect.

OIU-E5c. The upstream edit multipart body MUST include text field `prompt` equal to the concatenation of all user-role `Node::Text.content` values in `UrpRequest.input`, joined by newline, excluding empty text after trim.

OIU-E5d. For multipart edits, the encoder MUST decode each Base64 image into a file part. A node with `MediaMetadata.image_mask = true` MUST use field name `mask`. Other nodes MUST use `image` for one source image, or `image[]` for multiple source images. The file part content type MUST equal the source media type. The encoder MUST NOT infer a mask from node identity or position.

OIU-E5e. Multipart text fields MUST use the same option selection and typed precedence as generation requests. JSON string values MUST be written without quotes. JSON number, boolean, object, array, and null values MUST be written as their JSON serialization.

OIU-E5f. If `UrpRequest.stream == Some(true)`, Monoize MUST include text field `stream` with value `true` in the upstream edit multipart body. If `UrpRequest.stream != Some(true)`, Monoize MUST NOT include a `stream` field in the upstream edit multipart body.

OIU-E5g. When a request capture session is active for the attempt, the sent multipart body MUST be recorded as the attempt's `upstream_request` in the multipart capture representation of `request-capture-dumps.spec.md` RCD-D6a/RCD-D16, with part order, field names, file names, part content types, and part bytes equal to the sent form.

OIU-E5h. JSON edits MUST place source images in an `images` array, in source order. Each entry MUST contain `image_url` for a URL or `file_id` for a file reference. Base64 sources in the same request MUST become `image_url` data URLs. A mask MUST use a separate `mask` object with the same reference representation. JSON edits MUST use the same model, prompt, options, user, and stream rules as generation requests. Request capture MUST record the sent JSON body.

OIU-E5i. The encoder MUST require 1 through 16 source images and at most one mask. Multipart encoding MUST reject URL and file-reference sources instead of uploading their text as files.

### 3.3 Extra Body Whitelist

OIU-E6. The default extra body whitelist for `openai_image` MUST be: `size`, `quality`, `style`, `response_format`, `n`, `background`, `output_format`, `output_compression`, `moderation`, `user`, `partial_images`, `input_fidelity`.

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
- If `b64_json` is a string with non-whitespace content, create an assistant `Node::Image` with `ImageSource::Base64`. Preserve its value and use the MIME type from OIU-D8.
- Otherwise, if `url` is a string with non-whitespace content, create an assistant `Node::Image` with `ImageSource::Url`. Preserve its value and set `detail` to `None`.
- Otherwise, skip the entry. If no image nodes remain, return an error instead of a successful response.

OIU-D3. Each `data[]` entry's `revised_prompt` MUST map to that image's typed `MediaMetadata.image_generation.revised_prompt`. An empty string MUST remain present. The decoder MUST NOT create a duplicate text node or associate one entry's prompt with another image.

OIU-D4. All extracted assistant nodes MUST be placed directly into `UrpResponse.output` in source order.

OIU-D5. The decoded `UrpResponse` MUST have:
- `id`: the string value of `created` from the upstream response, or a generated ID if absent.
- `model`: the requested model name.
- `output`: containing the assembled assistant nodes.
- `finish_reason`: `Some(FinishReason::Stop)`.
- `usage`: parsed from upstream `usage` object if present, otherwise `None`.

OIU-D6. If the upstream response contains a top-level `usage` object, Monoize MUST parse it into URP `Usage` using the same field mapping as the existing image API response handler.

OIU-D7. For `gpt-image-2`, decoded usage MUST preserve text input tokens, image input tokens, cached input tokens, and image output tokens as structured URP usage fields when upstream provides them.

OIU-D8. Decoders MUST copy upstream-reported `quality`, `size`, `background`, `output_format`, and `model` strings into each generated image's typed `MediaMetadata.image_generation`. Non-streaming Images responses use top-level fields. Completed Images SSE events and Responses `image_generation_call` items use their own fields. These recognized fields MUST NOT remain duplicate semantic values in adapter extras. Base64 image MIME MUST follow an explicit `output_format` (`png`, `jpeg`, or `webp`), subject to the Images `media_type` precedence in OIU-D10. Absent format retains the existing PNG fallback without inventing response metadata.

OIU-D9. Non-streaming and streaming Responses encoders MUST project generated-image metadata from the current typed fields. Native replay extras MUST NOT restore deleted metadata. The response envelope's text-model identifier MUST NOT be used as the generated image's model.

## 5. Downstream Rendering

### 5.1 Responses API downstream (`/v1/responses`)

OIU-R1. When the downstream protocol is Responses, the URP response MUST be encoded using the standard Responses encoder. Assistant image nodes appear as native `output_image` items in the response.

### 5.2 Chat Completions / Messages downstream

OIU-R2. When the downstream protocol is Chat Completions or Anthropic Messages, Monoize MUST automatically convert assistant `Node::Image` outputs to inline markdown base64 images appended to assistant text content before encoding the downstream response.

OIU-R3. The markdown format MUST be: `![image](data:{media_type};base64,{data})` for base64 images, and `![image]({url})` for URL images.

OIU-R3a. When converting an image to markdown, the renderer MUST place its nonempty typed `revised_prompt` before that image. This projection MUST consume the image node and MUST NOT retain a second semantic copy in URP.

OIU-R4. This automatic conversion MUST occur after response-phase transforms have been applied, so user-configured transforms can still operate on the raw `Node::Image` data.

## 6. Streaming Behavior

OIU-S1. The `openai_image` upstream type supports upstream SSE streaming when `UrpRequest.stream == Some(true)`.

OIU-S1a. Event name resolution: for each upstream SSE frame, the decoder MUST use the SSE `event` field name. When the SSE `event` field is absent, empty, or the default value `message`, the decoder MUST fall back to the JSON `type` field of the frame data.

OIU-S2. When decoding upstream image SSE, Monoize MUST accept the partial-image events `image_generation.partial_image`, `image_edit.partial_image`, and `response.image_generation.partial_image`. For each partial-image event whose payload contains non-empty `b64_json` or `result`, the decoder MUST emit one canonical `NodeDelta` stream event with `delta = Image` whose source is `Base64` (media type derived from payload `output_format`, defaulting to `image/png`). The `NodeDelta` event `extra_body` MUST contain `provider_event_type` set to the resolved upstream event name, `partial_image_index` copied from the payload when present, and all remaining non-internal payload fields. A partial-image event whose payload contains no image data MUST be ignored. Neither case is a stream error.

OIU-S2a. Partial-image events MUST NOT contribute nodes to the terminal `ResponseDone.output`.

OIU-S2b. All partial-image `NodeDelta` events and the completed image node of the same generation MUST share one node index, allocated when the first event of that generation is decoded.

OIU-S3. When decoding upstream image SSE, Monoize MUST accept the completed events `image_generation.completed`, `image_edit.completed`, and `response.image_generation.completed`. If the payload contains non-empty `b64_json` or `result`, the decoder MUST emit one assistant `Image` node with `Image.source = Base64`. The media type MUST be derived from `output_format`, defaulting to `image/png`.

OIU-S3a. If a completed event payload contains a `usage` object, the decoder MUST parse it into URP `Usage` using the same field mapping as OIU-D6 and attach it to the terminal `ResponseDone`. When multiple completed events carry `usage`, the last parsed `usage` wins.

OIU-S4. When upstream image SSE reaches a terminal successful event, Monoize MUST emit one canonical `ResponseDone` whose `output` contains all completed image nodes collected from the stream.

OIU-S5. When the downstream request is streaming, Monoize MAY pass upstream image SSE through the canonical URP streaming pipeline and encode downstream SSE from canonical URP events.

OIU-S5a. When the downstream protocol is Responses streaming, partial-image `NodeDelta` events decoded per OIU-S2 MUST be rendered as `response.image_generation_call.partial_image` frames whose payload carries the base64 data as `partial_image_b64`, following the existing Responses image-tool encoding conventions.

OIU-S6. When the downstream request is non-streaming but the selected attempt has `UrpRequest.stream == Some(true)` after request-phase transforms, Monoize MUST call the upstream image endpoint with streaming enabled, collect canonical URP stream events into one `UrpResponse`, apply response transforms, and return a normal non-streaming downstream response.

OIU-S7. Streaming edits MUST use the same transport selection as non-streaming edits under OIU-E5a. This rule applies to downstream streaming, internal stream collection, and the Image API streaming path. Multipart edits MUST include text field `stream=true`. JSON edits MUST include boolean `stream=true`. The upstream response MUST be decoded as SSE.

## 7. Routing Integration

OIU-RT1. The upstream path for text-only `openai_image` requests MUST be `/v1/images/generations`.

OIU-RT1a. The upstream path for `openai_image` requests containing one or more user-role `Node::Image` items MUST be `/v1/images/edits`.

OIU-RT2. `openai_image` MUST NOT require any extra request headers beyond standard auth.

OIU-RT3. Channel test (probe) for `openai_image` providers is not meaningful for image generation models. Monoize MUST use a `POST /v1/images/generations` probe with `{ "model": <probe_model>, "prompt": "test", "size": "1024x1024" }` and treat a 2xx response as success.

## 8. Dashboard Integration

OIU-UI1. The provider type selector in the dashboard MUST include `openai_image` with label `OpenAI Image` and path `/v1/images/generations`.

OIU-UI2. The provider type MUST use the OpenAI icon in the UI.

## 8a. Billing Integration

OIU-B1. `gpt-image-2` billing MUST use the `model_prices` row selected by
`model-pricing.spec.md` §3 and settle through the §6 settlement function. Token prices
are not modality-specific: input tokens, cached input tokens, and image output tokens
bill at the row's `input_usd_per_1m`, `cache_read_usd_per_1m`, and `output_usd_per_1m`.

OIU-B2. By default, `gpt-image-2` MUST charge only upstream usage quantities:

- text/image input tokens;
- cached input tokens;
- image output tokens.

OIU-B2a. If OpenAI image usage provides `input_tokens_details.cached_tokens`, Monoize MUST normalize it to `Usage.input_details.cache_read_tokens`.

OIU-B2b. If OpenAI image usage provides `cached_tokens_details`, `cache_read_tokens_details`, or `cached_input_tokens_details` with `text_tokens` or `image_tokens`, Monoize MUST normalize it to `Usage.input_details.cache_read_modality_breakdown`.

OIU-B2c. Modality breakdowns are preserved in `usage_breakdown_json` for display; they
do not select prices (OIU-B1).

OIU-B3. Monoize MUST NOT add a fixed per-output-image fee. Image requests bill only the
usage classes priced by the selected `model_prices` row.

OIU-B4. Monoize MUST NOT infer image duration, render time, or output count from local runtime timing for billing.

## 9. Constraints

OIU-C1. `openai_image` is a concrete upstream type, not virtual. Providers with this type MUST have `base_url` and `auth`.

OIU-C2. `openai_image` MUST appear in the `stream_upstream_to_urp_events` streaming decoder dispatch and MUST NOT return `provider_type_not_supported` for streaming image generation attempts.

OIU-C3. The encoder MUST forward `image_generation.n` when present. If typed image options are absent, an allowed `extra_body.n` MAY supply this value. This upstream adapter MUST NOT perform fan-out. The upstream provider handles `n` natively; Image API ingress fan-out remains a separate operation.

## 10. Shared Images decoding

OIU-D10. The shared Images decoder MUST honor nonblank per-image `media_type` before the existing `output_format` inference.
This rule applies to non-streaming responses and partial or completed SSE image events.
The decoder MUST map `media_type` to the typed image source and MUST NOT retain a second copy in image extras.

OIU-S8. The shared stream decoder MUST decode nested `error.message` and `error.code`, with the existing top-level error shape as fallback.
An error MUST terminate decoding without a successful response terminal event.
A stream that ends without a completed image MUST fail with `upstream_stream_missing_terminal`.
