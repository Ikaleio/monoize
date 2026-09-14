# Protocol codec audit

Date: 2026-09-12

The audit covers Chat Completions, Responses, Messages, and Gemini request, response, and SSE codecs.
`git pull --rebase --autostash` reported `Already up to date`.
The working-tree baseline is commit `6914e8e`.
Existing changes outside this audit were preserved.

## Canonical ownership

| Value | Canonical owner | Removed duplicate or corrected behavior |
| --- | --- | --- |
| Tool namespace | `ToolCall.namespace`, `ToolResult.namespace`, `ToolDefinition.namespace` | Encoders no longer recover deleted namespaces from native extras. |
| Namespace children | `ToolDefinition.tools` | Promotion processes typed definitions directly and preserves their native configuration. |
| Native tool configuration | `ToolDefinition.config` and `origin_protocol` | Native configuration is stored once and emitted only for its protocol. |
| Function-call signature | `ToolCall.signature` | Gemini no longer creates a duplicate canonical reasoning node for a signed call. |
| Media information | `MediaMetadata` | Signatures, MIME types, audio references, transcripts, and expiry values have typed owners. |
| Tool-result function name | `ToolResult.name` | Gemini result names no longer depend on an internal name copy. |
| Legacy Chat result identity | `ToolResult.id` and `ToolResult.name` | Native fields and legacy markers cannot restore a deleted identity. |
| Initial provider item | `NodeHeader::ProviderItem.body` | Messages stream decoding no longer stores a second initial body in extras. |
| Native content identity | `ProviderItem.id` and `item_type` | Wire clones use current typed identity without changing canonical opaque bodies or adding synthetic native fields. |
| Citations, refusal, phase | Existing typed node fields | Streaming and non-streaming paths preserve the same semantic fields. |
| Response and image envelopes | Current typed values plus shape markers | Native snapshots no longer retain duplicate text, image bytes, usage, or response controls. |

Gemini call signatures require wire transport through clients that accept only reasoning envelopes or signature sigils.
Downstream encoders construct this transport from the typed signature without mutating canonical storage.
Request decoding consumes the bound transport before transforms and restores the typed call signature.
Provider and model filtering can remove the signature while retaining the call.

Affinity fingerprints now include typed namespaces, signatures, citations, reasoning metadata, and media metadata.
The legacy bridge preserves these values and prevents stale phase metadata from overriding typed deletion.
Function arguments retain JSON normalization. Custom-tool input remains byte-exact, including input that resembles JSON.

## Feature coverage

The fixture helpers exercise native decoding, native encoding, and a second decode with canonical assertions.
Response fixtures include non-streaming, native SSE, and synthetic SSE paths where the protocol exposes them.
Request fixtures exercise both streaming flags; request bodies themselves are JSON, not SSE.

| Protocol | Covered special features |
| --- | --- |
| Chat Completions | Function calls, custom tools, legacy functions and results, scalar reasoning, reasoning details, encrypted reasoning, refusal, citations, assistant phase, generated and fragmented audio, image/file/audio content, multimodal requests, structured controls, usage, errors, terminal reasons, bounded-frame citation emission. |
| Responses | Namespace definitions and selectors, function/custom calls and results, current and preview Computer Use, screenshot results, web search, file search, code interpreter, MCP calls/listing/approval, shell and local shell, apply-patch calls/results, programmatic calling, tool search, additional tools, deferred tool configuration, compaction, image generation and partial images, image/file content, reasoning summaries/content/encryption, refusal, citations, structured controls, usage, incomplete/cancelled/failed responses, errors, terminal output reconstruction. |
| Messages | Client tool calls/results, Computer Use and toolset members, browser/computer toolset configuration, web search/fetch, code execution, bash, text editor, tool search, MCP, thinking/redacted thinking/signatures, citations, cache controls, phase, images/documents/file identifiers, multimodal results, parallel controls, usage, stop reasons, errors, authoritative buffered completion. |
| Gemini | Signed thoughts/text/calls/media, Computer Use actions, function results and multimodal results, inline and URI media, video metadata, executable code/results, server tools/results, search/maps/URL/file-search/MCP configuration, schemas, allowed-function selection, structured generation controls, grounding/citations/safety, blocked prompts, usage details, terminal reconciliation. |
| Shared integration | Namespace alias collisions and restoration, native tool filtering, canonical affinity hashes, legacy bridge preservation, encrypted-signature transport, and typed mutation/deletion. |

Test sources:

- `src/urp/chat_feature_tests.rs`
- `src/urp/responses_feature_tests.rs`
- `src/urp/messages_feature_tests.rs`
- `src/urp/gemini_feature_tests.rs`
- `src/urp/tool_signature.rs`
- `src/urp/decode/mod.rs`
- `src/urp/internal_legacy_bridge.rs`
- `src/transforms/reasoning_strip_encrypted.rs`
- `src/handlers/tool_namespace_tests.rs`
- `src/handlers/helpers.rs`

## Internal fields retained

Native shape markers identify legacy function syntax, reasoning surfaces, envelope placement, image-generation items, and Gemini JSON-schema syntax.
Unknown provider fields remain at their original owning layer.
File-identifier provenance prevents an OpenAI file capability from being sent as an Anthropic Files API identifier, or conversely.
Namespace alias metadata records the name mapping needed at a target protocol boundary.
These fields do not retain a second canonical text, instruction, signature, or request-control value.

Provider-specific server tool actions remain `ProviderItem` values scoped by `origin_protocol`.
Their bodies contain native semantics without a shared URP equivalent.
They are not represented as client-executed function calls.
Internal `_monoize_` keys remain excluded from wire objects.

## Verification

The final integrated run passed **169 tests**, with zero failures, ignored tests, or filtered tests.
`git diff --check` passed.

| Test group | Passed |
| --- | ---: |
| Chat Completions features | 26 |
| Responses features | 43 |
| Messages features | 24 |
| Gemini features | 55 |
| Signature transport, stripping, namespace integration, affinity, filtering, and canonical bridge | 21 |
| Total | 169 |

`all-tests.log` contains the final full-suite output.
`test-inventory.txt` lists all 169 passing test names.
The total includes existing tests and new regression tests.

Commands:

```sh
cargo test --all-targets -- --nocapture
git diff --check
```

The tests use local wire fixtures with the production codecs, including real SSE parsing and SSE serialization.
They do not contact a model provider or execute a provider-hosted tool.
They therefore establish codec behavior, not live upstream acceptance or tool execution.
Compatibility fixtures for assistant media do not establish that every official upstream accepts those history shapes.
Gemini now has a reusable stream encoder; this change does not add a downstream HTTP endpoint.
Its encoder emits completed Parts and rejects terminal changes that would require retracting an emitted Part.

## Specifications and reference schemas

The corresponding requirements are maintained in:

- `spec/urp-v2-flat-structure.spec.md`
- `spec/urp-v2-rust-core-mapping.spec.md`
- `spec/unified_responses_proxy.spec.md`

Primary references consulted during the audit:

- [OpenAI Chat Completions reference](https://developers.openai.com/api/reference/resources/chat/subresources/completions)
- [OpenAI Responses reference](https://developers.openai.com/api/reference/cli/resources/beta/subresources/responses)
- [OpenAI Computer Use guide](https://developers.openai.com/api/docs/guides/tools-computer-use)
- [OpenAI programmatic tool calling guide](https://developers.openai.com/api/docs/guides/tools-programmatic-tool-calling)
- [OpenAI tool search guide](https://developers.openai.com/api/docs/guides/tools-tool-search)
- [Anthropic Computer Use guide](https://platform.claude.com/docs/en/agents-and-tools/tool-use/computer-use-tool)
- [Gemini GenerateContent reference](https://ai.google.dev/api/generate-content)
