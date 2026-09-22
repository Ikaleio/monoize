# OpenRouter Image Upstream Specification

## 1. Configuration and routing

ORI-1. `openrouter_image` MUST be a Channel type and an API type override value.
The corresponding Rust variants MUST be `OpenrouterImage` in each provider protocol enum.
Existing Channels MUST retain their configured types.

ORI-2. Every attempt MUST send JSON to `POST {base}/v1/images`, including requests with reference images.
A base URL ending in `/v1` MUST NOT produce a second `/v1` segment.
The standard OpenRouter base URL is `https://openrouter.ai/api`.
Authentication, proxies, extra headers, timeouts, retries, and captures MUST use the existing Channel pipeline.

## 2. Request encoding

ORI-3. The wire `model` MUST equal the redirected URP model.
The wire `prompt` MUST join nonblank user text nodes with one newline, in input order.
The wire `stream` MUST be `true` only when the typed request enables streaming.

ORI-4. Each user image node MUST produce one `input_references` item, in input order.
Each item MUST have `type: "image_url"` and `image_url.url`.
URL sources MUST retain their URL.
Base64 sources MUST become `data:{media_type};base64,{data}` URLs.
When no user image exists, the encoder MUST omit `input_references`.
Extra fields MUST NOT override `model`, `prompt`, `stream`, or `input_references`, including their absence.

ORI-5. A user image with typed `metadata.image_mask = true` MUST fail encoding with `unsupported_media` before an upstream call.
The encoder MUST reject file-ID image sources with the same error code.
Routing preflight MAY first reject foreign private references with its existing `incompatible_file_reference` error.
It MUST NOT drop masks or send them as reference images.
Model-specific reference limits MUST be enforced by the upstream service.

ORI-6. The default extra-field allowlist MUST contain `n`, `resolution`, `aspect_ratio`, `size`, `quality`, `output_format`, `background`, `output_compression`, `seed`, `user`, and `provider`.
Configured allowlist extensions MUST retain their existing semantics.
Typed `user` MUST take precedence over the extra-field value, including absence.
Typed image options MUST supply `n`, `size`, `quality`, `background`, `output_format`, and `output_compression` when present.
Their absence within a present typed options object MUST suppress stale extra-field copies.
Other typed OpenAI image options MUST NOT be sent as OpenRouter top-level fields.
Provider routing and passthrough options MUST remain under `provider`.
The encoder MUST validate typed image options and media sources after request transforms.
Media preparation MUST target the `openrouter_image` protocol and reject foreign private references.

## 3. Responses and streaming

ORI-7. Non-streaming responses MUST use the existing Images response decoder and empty-image validation.
For Base64 images, each nonblank `data[].media_type` MUST set the typed image source media type.
When absent, the decoder MUST infer the media type from `output_format`, then default to `image/png`.
The decoder MUST preserve the existing nonblank Base64 preference and URL fallback.
Usage MUST map `prompt_tokens` and `completion_tokens` to canonical input and output tokens.
Billing MUST use the existing model pricing policy.

ORI-8. Streaming responses MUST accept data-only SSE frames with JSON `type` values `image_generation.partial_image`, `image_generation.completed`, and `error`.
The shared Images decoder MUST retain partial-image ordering and completed images.
It MUST honor `media_type` in partial and completed payloads.
Nested `error.message` and `error.code` MUST be decoded.
An error MUST NOT be followed by a successful response terminal event.
A stream without a completed image MUST fail with `upstream_stream_missing_terminal`.
Image API downstream streaming MUST use the transport negotiation rules in `image-api-proxy.spec.md` IS9 through IS12.
Other model and operation restrictions MUST remain upstream errors.

ORI-9. A non-streaming downstream request whose transformed request enables streaming MUST collect the upstream SSE response.
It MUST still return one non-streaming downstream response.
All existing image ingress endpoints and downstream image rendering rules MUST support the new type.

## 4. Dashboard and discovery

ORI-10. The Channel type selector MUST show `OpenRouter Image` with path `/v1/images`.
Fetch models MUST call `GET {base}/v1/images/models` with the Channel bearer credential.
The result MUST use the existing model-list selection and mapping flow.

ORI-11. A non-streaming Channel probe MUST call `POST {base}/v1/images` with the resolved model, `prompt: "test"`, and `n: 1`.
Streaming liveness tests MUST be rejected, consistent with the existing Image Channel test UI.
Periodic discovery health checks MUST use `/v1/images/models`.

## 5. Documentation

ORI-12. The introduction and Provider configuration pages MUST document this type in all four supported locales.
The configuration instructions MUST explain the base URL, upstream model slug, unified image endpoint, reference images, and unsupported masks.
They MUST explain that Monoize serves streaming Image API edits through non-streaming OpenRouter requests and downstream SSE heartbeats.

## References

- [OpenRouter Image Generation](https://openrouter.ai/docs/guides/overview/multimodal/image-generation)
- [OpenRouter Image Models](https://openrouter.ai/docs/api/api-reference/images/list-image-models)
