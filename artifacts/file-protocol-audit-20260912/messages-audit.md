# Anthropic Messages file and media audit

Audited on 2026-09-12. This report records current code behavior against current official documentation. Implementation and specifications were not changed.

The audit found one valid native input that is lost, two target-format validation gaps, and an unsupported response representation. Metadata and resource portability have additional limits.

Evidence includes source tracing and 11 executions of the common offline `probe` harness. No request was sent to Anthropic. Short binary payloads below test codec structure, not file validity. The existing media streaming regression passed. New failing cases have static stream-path evidence; no permanent tests were added.

Read specifications: `spec/urp-v2-flat-structure.spec.md` URPV2-S1–S11 and URPV2-13a–13c; `spec/unified_responses_proxy.spec.md` PM2b–PM2d; `spec/urp-v2-rust-core-mapping.spec.md` NH-11.

## Documented contract

| Surface | Official contract | Source |
|---|---|---|
| Image input | JPEG, PNG, GIF and WebP. Native sources can use raw base64, a URL, or a Files reference. | [Vision](https://platform.claude.com/docs/en/build-with-claude/vision) |
| Document input | PDF uses base64, URL, or Files reference. Plain text uses a text source or a compatible Files reference. DOCX/XLSX require conversion or code execution. | [PDF support](https://platform.claude.com/docs/en/build-with-claude/pdf-support) |
| Custom document | `source.content` accepts a string or an array containing text/image blocks. | [Messages API](https://platform.claude.com/docs/en/api/http/messages), [official generated source type](https://raw.githubusercontent.com/anthropics/anthropic-sdk-python/main/src/anthropic/types/content_block_source_param.py) |
| Document metadata | `title`, `context`, `citations`, and `cache_control` belong to the document block. `filename` does not belong to a base64 document source. | [DocumentBlockParam](https://raw.githubusercontent.com/anthropics/anthropic-sdk-python/main/src/anthropic/types/document_block_param.py), [Base64PDFSourceParam](https://raw.githubusercontent.com/anthropics/anthropic-sdk-python/main/src/anthropic/types/base64_pdf_source_param.py) |
| Tool result | Nested result content supports text, image, document, and search-result blocks. | [Handle tool calls](https://platform.claude.com/docs/en/agents-and-tools/tool-use/handle-tool-calls) |
| Citations | PDF, plain-text, and custom-content documents have different location units. Document title/context reach the model but are not citable source content. | [Citations](https://platform.claude.com/docs/en/build-with-claude/citations) |

## M1 — Valid string custom documents disappear

**Classification: code defect and incomplete specification. Priority: high.**

Native fixture:

```json
{"model":"claude-test","max_tokens":100,"messages":[{"role":"user","content":[{"type":"document","source":{"type":"content","content":"alpha"},"title":"Doc","citations":{"enabled":true}}]}]}
```

Observed probe result: canonical `input` is empty. Every encoded target has empty input/messages. The document text, title, and citation configuration disappear together.

[decode/mod.rs:613](/Users/ikaleio/Projects/monoize/src/urp/decode/mod.rs:613) requires `content.as_array()`. [decode/anthropic.rs:521](/Users/ikaleio/Projects/monoize/src/urp/decode/anthropic.rs:521) drops a recognized document when that parser returns `None`.

The ordinary response extension has the same behavior at [decode/anthropic.rs:709](/Users/ikaleio/Projects/monoize/src/urp/decode/anthropic.rs:709). Live block-start decoding delegates there at [stream_decode/anthropic.rs:1294](/Users/ikaleio/Projects/monoize/src/urp/stream_decode/anthropic.rs:1294).

The same document inside `tool_result.content` survives as a Messages-scoped `ProviderItem`, because [decode/anthropic.rs:897](/Users/ikaleio/Projects/monoize/src/urp/decode/anthropic.rs:897) falls through to opaque preservation. The probe confirmed this inconsistent treatment. Array custom documents survive as typed `File` nodes.

PM2c.1 currently limits this source to a block array. The official string alternative requires a specification correction before implementation.

## M2 — Cross-protocol files generate invalid document source types

**Classification: missing target adaptation and validation; specification too broad. Priority: high.**

The official base64 document source permits only `application/pdf`. Plain text has a separate `source.type="text"` representation. [Official base64 source](https://raw.githubusercontent.com/anthropics/anthropic-sdk-python/main/src/anthropic/types/base64_pdf_source_param.py).

Reproducible inputs:

```json
{"source":"gemini","request":{"contents":[{"role":"user","parts":[{"inlineData":{"mimeType":"application/json","data":"eyJ4IjoxfQ=="}}]}]}}
{"source":"chat","request":{"model":"chat-test","messages":[{"role":"user","content":[{"type":"file","file":{"filename":"note.pdf","file_data":"JVBERi0xLjcK"}}]}]}}
```

The probe emitted `document.source.type="base64"` with `media_type="application/json"` and `"application/octet-stream"`, respectively. Neither MIME is valid for Anthropic's base64 document source. JSON is a documented Gemini Blob MIME. [Gemini Blob](https://ai.google.dev/api/generate-content#Blob).

[decode/gemini.rs:793](/Users/ikaleio/Projects/monoize/src/urp/decode/gemini.rs:793) creates a typed file with the supplied MIME. [decode/mod.rs:573](/Users/ikaleio/Projects/monoize/src/urp/decode/mod.rs:573) assigns the octet-stream default to raw Chat file bytes. It does not infer MIME from `filename` or bytes.

[messages_part2.inc.rs:185](/Users/ikaleio/Projects/monoize/src/urp/encode/anthropic/messages_part2.inc.rs:185) copies any file MIME into a base64 document. [thinking_validation.inc.rs:3](/Users/ikaleio/Projects/monoize/src/urp/encode/anthropic/thinking_validation.inc.rs:3) performs thinking validation, not media validation.

The same encoder handles ordinary request files, tool-result files, and the response extension. A JSON/text file could use the documented text source after decoding its bytes. Arbitrary binary formats need a defined rejection/conversion policy. Automatically selecting code execution would change tool semantics.

URL files have a related limit: [messages_part2.inc.rs:181](/Users/ikaleio/Projects/monoize/src/urp/encode/anthropic/messages_part2.inc.rs:181) always selects the PDF URL source. A non-PDF Gemini `fileData` URL is not made compatible by changing its field name. No fetch/content conversion occurs here.

PM2c.1 says arbitrary base64 media types can be encoded. It needs the official PDF/text distinction.

## M3 — Unsupported image MIME values pass through

**Classification: unsupported target format emitted without adaptation. Priority: medium.**

```json
{"source":"gemini","request":{"contents":[{"role":"user","parts":[{"inlineData":{"mimeType":"image/heic","data":"AAAA"}}]}]}}
```

The probe produced an Anthropic image with `source.media_type="image/heic"`. Replace `AAAA` with actual HEIC bytes for a provider acceptance test.

Gemini lists HEIC/HEIF as supported. Anthropic lists only JPEG/PNG/GIF/WebP. [Gemini Blob](https://ai.google.dev/api/generate-content#Blob), [Anthropic Vision](https://platform.claude.com/docs/en/build-with-claude/vision).

[messages_part2.inc.rs:150](/Users/ikaleio/Projects/monoize/src/urp/encode/anthropic/messages_part2.inc.rs:150) copies any typed base64 image MIME. Its data-URL helper at [line 166](/Users/ikaleio/Projects/monoize/src/urp/encode/anthropic/messages_part2.inc.rs:166) accepts every `image/*` MIME too.

This needs transcoding or an explicit unsupported-format policy. A MIME rename alone cannot convert the image bytes. Size, dimension, and PDF-page limits are also left to upstream validation; this audit did not exercise those resource limits.

## M4 — Response media uses input-only block shapes

**Classification: target capability gap and response specification/test mismatch. Priority: medium.**

The official response `ContentBlock` union does not include top-level `ImageBlock` or `DocumentBlock`. It includes text, reasoning, tools, tool results, and container uploads. The distinction is visible in [Messages API](https://platform.claude.com/docs/en/api/http/messages) and explicit in the [official generated response union](https://raw.githubusercontent.com/anthropics/anthropic-sdk-python/main/src/anthropic/types/content_block.py).

An assistant URP Image produces this response block in the probe:

```json
{"type":"image","source":{"type":"base64","media_type":"image/png","data":"AAAA"}}
```

[messages_part1.inc.rs:227](/Users/ikaleio/Projects/monoize/src/urp/encode/anthropic/messages_part1.inc.rs:227) and [line 239](/Users/ikaleio/Projects/monoize/src/urp/encode/anthropic/messages_part1.inc.rs:239) emit image/document input shapes as assistant response content.

The live encoder calls the same helper at [stream_encode/anthropic.rs:520](/Users/ikaleio/Projects/monoize/src/urp/stream_encode/anthropic.rs:520). Synthetic streaming uses that mapping at [line 1422](/Users/ikaleio/Projects/monoize/src/urp/stream_encode/anthropic.rs:1422).

[messages_feature_tests.rs:244](/Users/ikaleio/Projects/monoize/src/urp/messages_feature_tests.rs:244) tests these input-shaped blocks as responses. Those tests establish extension roundtrip behavior, not official response validity. Native generated-file references inside server-tool results/container uploads remain separately scoped opaque items. A legal downstream representation for arbitrary cross-protocol generated media requires a product decision.

## Verified preservation and bounded limitations

| Case | Current result |
|---|---|
| Native PDF base64/URL/file reference | Typed file source; same-Messages re-encoding preserves supported source syntax. |
| Native plain text | `FileSource::Text`; encoder emits `source.type="text"`, `media_type="text/plain"`. |
| Native custom content array | Array remains in typed `FileSource::Content`, including nested image/text blocks. |
| Native title/context/citations/cache | Stored in file extras and merged back at [messages_part1.inc.rs:34](/Users/ikaleio/Projects/monoize/src/urp/encode/anthropic/messages_part1.inc.rs:34). Probe confirmed preservation. |
| Native tool-result image/document | Uses the same source encoder and preserves metadata. [messages_part1.inc.rs:280](/Users/ikaleio/Projects/monoize/src/urp/encode/anthropic/messages_part1.inc.rs:280). |
| Chat/Responses PDF data URL in file data | Shared parser strips the data-URL prefix and records `application/pdf`; probe produced correct raw base64. [decode/mod.rs:544](/Users/ikaleio/Projects/monoize/src/urp/decode/mod.rs:544). |
| Chat image data URL | Messages converts it to raw base64 plus MIME. Probe confirmed no nested data URL. [messages_part2.inc.rs:138](/Users/ikaleio/Projects/monoize/src/urp/encode/anthropic/messages_part2.inc.rs:138). |
| Base64 filename | Typed filename is omitted on Messages wire. This is correct: no source filename field exists. It is not automatically converted to document title. |
| File identity | Only `messages` origin references are emitted. OpenAI-origin or missing-origin references are omitted, without file transfer. [messages_part2.inc.rs:210](/Users/ikaleio/Projects/monoize/src/urp/encode/anthropic/messages_part2.inc.rs:210). |

Document metadata is not represented by dedicated fields in [MediaMetadata](/Users/ikaleio/Projects/monoize/src/urp/mod.rs:761). When cross-protocol stripping is enabled, title/context/document citation settings are removed with ordinary extras. The probe with `strip:true` confirmed this. Actual handlers apply stripping conditionally at [nonstream.rs:282](/Users/ikaleio/Projects/monoize/src/handlers/nonstream.rs:282) and [streaming.rs:297](/Users/ikaleio/Projects/monoize/src/handlers/streaming.rs:297). This is a portability limit, not same-Messages metadata loss. Text response citations use their typed citation field separately.

The file-origin marker records protocol family, not the actual Anthropic workspace. Official Files documentation identifies the workspace as the file boundary. Files uploaded by clients cannot generally be downloaded through the Files API; generated outputs can. A future file migration layer cannot assume every source reference is downloadable. [Files API](https://platform.claude.com/docs/en/build-with-claude/files).

The codec has no file resolution or re-upload stage. Gemini file URIs become URP URLs, then Messages URL sources. Their target accessibility is not established. These limits prevent a claim of lossless conversion for all file types or provider pairs.

## Streaming evidence boundary

`cargo test --lib messages_documents_images_files_bidirectional` passed: **1 passed, 0 failed**. See [execution log](/Users/ikaleio/Projects/monoize/artifacts/file-protocol-audit-20260912/messages-existing-stream-check.log).

The existing helper constructs `message_start`, block-start/delta/stop events, and terminal events. It executes native stream decode, live re-encode, synthetic stream encode, and terminal comparison. Its fixtures cover PNG/PDF, file references, plain-text document metadata, and custom-content arrays. This proves the existing extension behavior described in M4, not a valid native media response capability.

For M1, use this content-block event between a standard `message_start` and block/message terminal events:

```json
{"type":"content_block_start","index":0,"content_block":{"type":"document","source":{"type":"content","content":"alpha"},"title":"Doc"}}
```

This new failing stream fixture was not executed. The decoder calls the already-probed non-stream parser at [stream_decode/anthropic.rs:1294](/Users/ikaleio/Projects/monoize/src/urp/stream_decode/anthropic.rs:1294), then requires its first output node. The empty parser result therefore produces no active block. The three Messages stream entrypoints are crate-private, so the external offline probe cannot call them directly.

For M2–M4, live and synthetic image/file payloads both use [stream_encode/anthropic.rs:520](/Users/ikaleio/Projects/monoize/src/urp/stream_encode/anthropic.rs:520). That path calls the same assistant media encoder used by the executed non-stream probe. Thus the stream format findings are statically established through shared code, not separately measured with new stream fixtures.
