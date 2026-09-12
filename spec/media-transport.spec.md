# Media Transport

This specification governs typed media conversion for Chat Completions, Responses, Messages, and Gemini GenerateContent.
Its requirements supersede historical rules that silently omit unsupported typed media.

## Canonical representation

MT1. Image data URLs MUST decode to Base64 sources. Sources MUST contain raw Base64, not a nested data URL.
MT2. File data URLs MUST supply their MIME type. Raw file bytes MUST use recognizable byte signatures or filename extensions when MIME is absent.
MT3. An unknown MIME MUST NOT be emitted as a fabricated supported format. Target validation MUST reject an unknown required MIME.
MT4. MediaMetadata MUST own filename, PDF/image detail not represented by the source, document title, document context, and document citation configuration.
MT5. FileSource::Base64 MUST NOT retain a second filename. Typed metadata deletion MUST suppress native metadata replay.
MT6. ToolResultContent Image and File MUST carry MediaMetadata. ToolResult nodes and headers MUST carry an optional typed signature.
MT7. Messages custom document strings MUST normalize to an ordered one-element text content array. Native string shape MAY remain metadata without duplicating text.
MT8. File reference provenance MUST use typed MediaResource metadata. It MUST identify the source protocol and optionally its provider, channel, and credential scope.
MT9. Resource metadata MUST NOT appear as native wire fields. Protocol-family compatibility alone MUST NOT authorize a bound resource on another credential scope.

## Request preparation

MT10. Checked encoders MUST prepare and validate a complete request before producing its wire object. Preparation MUST preserve the caller's request.
MT11. Messages text and compound documents MUST expand into ordered text and supported media for other protocols.
MT12. Document title and context MUST remain model-visible when the target has no document metadata carrier. Citation configuration MUST remain typed in URP.
MT13. Messages MUST encode PDF bytes as a base64 PDF source. UTF-8 text-like bytes MUST become a text source.
MT14. Unsupported Messages binary files, unsupported image MIME types, and unsupported audio inputs MUST produce explicit errors.
MT14a. A Messages URL document MUST have PDF evidence from typed MIME, the source document contract, or its URL extension. Other file URLs MUST fail.
MT14b. Image preparation MUST validate MIME syntax and use its type and subtype. Parameters MUST NOT enter a target's fixed image MIME field.
MT15. When encoding a Chat request, file URLs and unsupported media roles MUST produce explicit errors. Chat tool-result media MUST fail at target encoding.
MT15a. Chat tool and function result decoders MUST accept compatible text, image, and file content. They MUST preserve content order and typed metadata.
MT16. Responses assistant history MAY use stable easy-input messages with input media. Output-message content MUST contain only supported output types.
MT17. Gemini FunctionResponsePart MUST use the GenerateContent schema. URL media requiring unsupported nested fileData MUST produce an explicit error.
MT18. Public HTTP(S) resources MAY use a supported URL carrier. Provider-private file URIs and GCS references MUST retain their source restrictions.
MT19. An unsupported source MUST NOT become an empty input or disappear from a mixed message.
MT20. Invalid or unsupported media MUST produce a descriptive request error before the upstream request is sent.

## Resource routing

MT21. Bound resources MUST match the selected provider, channel, and credential scope. Retries MUST NOT weaken this check.
MT22. An unbound native file reference MAY use an unambiguous matching source route. The request MUST reject ambiguous source routes.
MT23. The first eligible source route MUST bind unbound references before retries. Later attempts MUST retain that binding.
MT24. Cross-provider transfer requires available file bytes or an independently accessible URL. The codec MUST NOT invent file IDs or download credentials.
MT25. When bytes are unavailable, incompatible resource routes MUST return an explicit error. No automatic file upload, arbitrary binary conversion, or tool execution is implied.
MT25a. Managed response history MUST retain bound input and output resource scopes. Replaying history MUST NOT rebind an existing reference to another scope.
MT25b. Native decoding MUST place private URL and compound document provenance in typed metadata. Compound references MUST inherit their document's bound scope.
MT25c. Deleting typed provenance MUST make an existing private reference invalid. Preparation and binding MUST NOT reconstruct deleted provenance from native content.

## Responses and streams

MT26. Checked response encoders MUST reject media without a legal target response representation. Existing native image-generation items remain supported.
MT27. Messages responses MUST NOT emit top-level input-only image, document, file, or audio blocks.
MT28. Chat responses MUST NOT emit media arrays as message.content. Native Chat audio envelopes remain supported.
MT29. Responses tool-result media MUST use input_text, input_image, and input_file in non-stream, live-stream, and synthetic-stream output.
MT30. A live encoding failure MUST emit the target error event and return an error. It MUST NOT emit a successful terminal response.
MT30a. An encoder MUST mark its returned error after it successfully sends a terminal error sequence. This internal marker MUST NOT appear on the wire.
MT30b. The SSE wrapper MUST log marked failures and skip billing. It MUST NOT send more frames after a marked failure.
MT30c. For an unmarked adapter or transport failure, the SSE wrapper MUST emit exactly one target error sequence.
MT31. A synthetic encoding failure MUST be detected before successful content events. It MUST return an error after emitting the target error event.
MT31a. Managed response history MUST NOT retain an output rejected by the downstream media encoder. History decoration MUST NOT imply successful retention.
MT32. Chat audio decoding SHOULD use the request audio format when available. Unknown format MUST remain unknown until supported evidence determines it.
MT33. Gemini MUST classify its documented audio MIME aliases as Audio. Media-specific replay metadata MUST be removed after an incompatible source change.
MT34. Gemini function-response parent signatures and unknown parent Part metadata MUST retain their original ownership.
MT34a. Gemini MAY preserve an existing video/* MIME for a documented YouTube URL. Inline media MUST use a supported fixed MIME.

## Verification

MT35. Tests MUST assert official wire shapes, canonical values, and typed mutation/deletion behavior.
MT36. Request fixtures MUST cover both stream flags. Response fixtures MUST exercise non-stream, live SSE, and synthetic SSE where the endpoint supports them.
MT37. Failure fixtures MUST verify explicit errors, absence of invalid media blocks, and absence of successful stream terminal events.
MT38. File bytes, MIME, data URL parsing, compound documents, nested tool results, and resource scope MUST have independent assertions.
MT39. Offline codec checks MUST NOT be reported as provider acceptance or successful document extraction.
MT40. Routing and stream context MUST be built directly from typed URP. Context creation MUST NOT encode media through an unrelated provider protocol.

## Compatible content decoding

MT41. Decoders MUST map recognized compatible content to typed URP, including content outside the source protocol's standard response union. Target encoders own output capability checks.
MT42. A supported content position MAY contain a string, one block object, or an ordered block array. Decoders MUST preserve recognized contents in each shape.
MT43. Chat tool-result JSON without a recognized content type MUST remain JSON text. Recognized malformed media MUST produce an explicit error.
MT44. Content validation MUST inspect content positions only. Tool arguments, custom tool input, schemas, and unknown item payloads MUST NOT trigger media validation.
MT45. Known audio content MUST become typed Audio nodes. Nested tool-result audio MUST use typed File content with its exact MIME and source.
MT46. Stream content headers and completion events MUST use the same parser and field ownership as non-stream content. Native fields MUST NOT duplicate typed payloads.
MT47. Compound document preparation MUST preserve native text-block fields. Expansion into ordinary Text nodes MUST move citations into typed citations.
MT48. Audio data URLs MUST become Base64 sources. Missing image or audio MIME MUST use available format or byte evidence, not a fabricated default.
MT49. A Gemini candidate citation on a frame without Text MUST attach to the existing candidate Text. A media-only terminal frame MUST NOT discard it.
MT50. Media classification MUST ignore MIME type and subtype case. Parameter handling MUST preserve meaningful audio parameters and enforce target MIME fields.
MT51. Request hygiene MAY remove unanswered ToolCall nodes. It MUST NOT remove ToolResult content merely because the corresponding call is absent locally.
MT52. Messages response encoders MUST reject client ToolResult nodes in every output mode. Compatible decoding MUST retain those results for capable targets.
MT53. Gemini encoders MUST reject Custom ToolCall and ToolResult nodes until a typed custom-tool bridge exists. They MUST NOT silently discard their input or result content.
MT54. Compatible image and file URL blocks MUST retain explicit MIME in typed metadata. Explicit MIME MUST take precedence over the Messages PDF URL default.
MT55. Normalizing an image URL to Base64 MUST move image detail into typed metadata for every accepted URL shape.
MT56. A live citation update MUST reach its typed Text node. A target that cannot update an already closed native text block MUST report an error instead of silently losing the citation.

## Protocol sources

The following official sources were checked on 2026-09-12:

- [OpenAI file inputs](https://developers.openai.com/api/docs/guides/file-inputs)
- [OpenAI generated protocol types](https://github.com/openai/openai-python/tree/main/src/openai/types)
- [Anthropic Messages types](https://github.com/anthropics/anthropic-sdk-python/tree/main/src/anthropic/types)
- [Gemini GenerateContent](https://ai.google.dev/api/generate-content)
- [GenerateContent audio](https://ai.google.dev/gemini-api/docs/generate-content/audio)
- [GenerateContent video](https://ai.google.dev/gemini-api/docs/generate-content/video-understanding)
