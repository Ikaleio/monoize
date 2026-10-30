# Gemini GenerateContent codec

## Scope and sources

GEM-1. This specification covers `generateContent` and SSE `streamGenerateContent` request, response, and assistant-history conversion through URP.
It does not add public HTTP endpoints or implement Live, file management, caching management, embeddings, or batch scheduling.
The shared URP specifications remain authoritative for node ownership, runtime context, and terminal output.

Source review date: 2026-10-31. Google labels this API "Gemini Generate Content API (Legacy)"; the Interactions API is out of scope.

- [GenerateContent reference](https://ai.google.dev/api/generate-content)
- [Content and Part reference](https://ai.google.dev/api/caching#Content)
- [Function calling](https://ai.google.dev/gemini-api/docs/function-calling)
- [Thought signatures](https://ai.google.dev/gemini-api/docs/generate-content/thought-signatures)
- [Thinking](https://ai.google.dev/gemini-api/docs/generate-content/thinking)
- [Gemini 3 guide](https://ai.google.dev/gemini-api/docs/generate-content/gemini-3)

`unified_responses_proxy.spec.md` §7.5 (PG1–PG8) states the concrete upstream rules referenced below.

## Canonical mapping

GEM-2. Decoders MUST map supported text, reasoning, media, function calls, function results, citations, token scores, usage, and termination to their typed URP owners.
Encoders MUST read current typed values. Deleting a typed value MUST prevent native metadata from restoring it.
Unknown fields, native array boundaries, and protocol-specific extensions MAY remain scoped metadata.
Unknown Parts and server tool Parts MUST remain Gemini ProviderItems and MUST NOT become client function calls or ordinary text.
Response decoding renders `executableCode` and `codeExecutionResult` Parts as assistant text under PG5d1, because no downstream protocol can carry them; Gemini-native request decoding keeps them as ProviderItems.
Thought Parts are thought summaries: they decode to `Reasoning.summary` (PG5a).

GEM-3. Request conversion MUST preserve Content and Part order, instruction placement, function identity, and supported media MIME types.
System and developer text maps to `systemInstruction`. Gemini system instructions MUST reject unsupported media.
Envelope controls and node extras of other protocols MUST NOT add Content members or split Content (PG4e).
Signed Parts MUST retain their individual signatures and order during history replay. Text and media signatures travel through signature carriers (PG5a). Only signatures with Gemini provenance reach the wire (PG5a3); the first unsigned function call of each current-turn step receives the documented dummy signature on Gemini 3+ (PG4f).
Function-result objects and ordered media parts MUST retain their typed content and matching call identity.

GEM-4. Temperature, top-p, output limits, stop sequences, reasoning configuration, response formats, tools, and tool choice MUST map bidirectionally.
Gemini thinkingLevel values MUST decode to lowercase URP effort. Encoding is model-aware under PG4b/PG4b1: Gemini 2.5 families use `thinkingBudget`, Gemini 3+/4 families use only supported `thinkingLevel` values, and `XHIGH`/`MAX` are never sent.
THINKING_LEVEL_UNSPECIFIED MUST decode as absent effort.
`responseLogprobs` and `logprobs` MUST map to `LogprobConfig.enabled` and `LogprobConfig.top_k`.
Encoding MUST reject enabled top-logprob counts above 20. Disabled configuration MUST NOT request token scores.
Native schema conversion MUST visit schema positions only. It MUST NOT alter property names, defaults, examples, enum strings, or constant payloads.
Gemini-native built-in tools MUST use `ToolDefinition.config` and `origin_protocol`. Cross-family built-in tools map under PG4h; custom tools use the function bridge of TCI7f; tool choice follows PG4g.
Gemini-specific controls without a shared semantic mapping MUST retain their native names and remain restricted to Gemini request preparation.
Legacy `text/x.enum` output MUST use typed response-format schema ownership and a native MIME shape marker.
Same-Gemini encoding MUST reconstruct `responseSchema` and `text/x.enum`; schema mutation and deletion MUST remain authoritative.
Gemini-native request decoding MUST accept the documented proto-JSON snake_case field names (for example `system_instruction`, `generation_config`, `inline_data`, `mime_type`, `function_call`, `thought_signature`) at structural positions only; user payloads such as function arguments, function responses, and schemas are never renamed. An OpenAI-compatible media part whose `file_data` is a string is not renamed.

GEM-4a. Optional `UrpRequest.sampling` MUST own top-k, random seed, presence penalty, and frequency penalty when provided.
Gemini camelCase controls and supported Chat or Messages controls MUST map to this shared configuration.
Unsupported target controls MUST be omitted. Absent typed controls MUST NOT be restored from native extras.
`FunctionDefinition.response_schema` MUST own a function result schema; native schema placement MAY remain a shape marker.
`TokenScore.token_id` MAY retain a provider token identifier. Targets without a token identifier field MUST omit it.
`InputDetails.tool_prompt_modality_breakdown` MUST own supplied Gemini tool-use modality counts independently of ordinary prompt modality counts.

GEM-5. A decoded response MUST select candidate index zero when present, otherwise the first candidate, matching the single-output URP contract.
It MUST NOT concatenate independent candidate alternatives.
Streaming MUST retain the candidate index selected in the first candidate-bearing frame and ignore other candidate indices thereafter.
Native error objects MUST fail decoding even when they also contain candidates.
Missing candidates without prompt blocking MUST fail decoding.
An absent or unspecified finish reason MUST remain absent in non-stream decoding.
Finish reasons map under the PG5d table: prompt blocking and filter or policy stops map to ContentFilter; `MAX_TOKENS` and the resumable `CONTINUATION` map to Length (incomplete, never failed); other non-success reasons fail with the native reason as error code.
Typed ResponseOutcome MUST describe completion, incomplete output, or failure consistently with the finish reason.
A failed or cancelled typed outcome MUST NOT encode as successful Gemini completion.
An explicit Completed outcome MUST encode STOP regardless of stale finish metadata.
An explicit Incomplete outcome MUST encode safety or length termination; unrepresentable incomplete reasons MUST use MAX_TOKENS.

GEM-6. Candidate token scores MUST map to ordered Text.logprobs entries when their token bytes match current answer text.
Encoders MUST derive token scores from current text nodes and MUST omit stale scores after text mutation or deletion.
Gemini token strings MUST use authoritative token bytes when those bytes form valid UTF-8.
Unrepresentable split-byte tokens and scores crossing canonical Text-node boundaries MUST be omitted without changing text or partially replacing a score snapshot.
Candidate-level score summaries MUST be recomputed from valid typed scores.
Citations MUST use typed source and answer ranges. Native byte offsets MUST convert correctly for non-ASCII text.
Grounding source links and answer ranges MUST map to typed citations where representable.
Streaming grounding chunks MUST accumulate in arrival order; support indices address that accumulated source list. Streamed support segments resolve under PG5b2.
Stream encoders MUST rebase grounding Part indices and byte ranges when adjacent text Parts merge on the wire.
Provider-specific grounding presentation and retrieval metadata MAY remain scoped metadata without a second answer-text copy.

GEM-7. Usage counters MUST retain existing checked arithmetic and modality accounting.
Typed changes and deletions MUST win over retained native usage metadata.
Streaming usage records are cumulative snapshots and MUST NOT be added together. Metering and decoding use one parser (PG6).
A Gemini-shaped usage encoder MUST emit the documented UsageMetadata counters and emit Monoize extension counters only when non-zero.

## Streaming

GEM-8. Native text fragments are deltas and MUST append exactly once.
Signed Parts and media Parts MUST retain boundaries. Function arguments in GenerateContent Parts are complete JSON objects.
A signed empty-text Part is its own Part; the wire stream does not identify it as a continuation of the preceding Part (PG5b).
Late citations and token scores MUST update canonical output before ResponseDone; they are sent as deltas only to nodes that are still open (PG5b1).
Scores supplied with native text SHOULD accompany its canonical text delta.
A later empty-text score delta MUST contain only scores for previously unscored text and MUST NOT repeat answer text.
Each canonical node MUST have one start and one completion. A closed node MUST NOT receive later deltas.

GEM-9. The encoder MUST emit available ordinary text and reasoning deltas in canonical order before node completion.
It MAY buffer atomic media, signed Parts, and function calls until complete when their wire format requires it.
NodeDone and ResponseDone MUST reconcile emitted prefixes without duplicate content.
Unrepresentable replacement or retraction after emission MUST fail explicitly.
A late signature MUST fail when earlier wire merging would bind it to text from another canonical node.
A signature carrier emits as a standalone `{"text": "", "thoughtSignature": …}` Part, the documented streaming shape, because an emitted Part cannot be amended.
Terminal-only nodes MUST be emitted from authoritative ResponseDone.output.
An error event encodes as `{"error": {"code": <HTTP status>, "message", "status": <canonical status>}}`; a URP error code that is a Google canonical status name selects that status and its HTTP code, and any other code encodes as `INTERNAL` / 500.

GEM-10. A native finish reason or prompt block establishes terminal completion.
EOF and `[DONE]` alone MUST NOT establish completion. Idle silence after terminal completion ends the stream successfully (PG5b1).
Usage and metadata after the finish reason MAY update the terminal snapshot; later content MUST NOT extend the completed answer.
Malformed JSON, transport failure, and native error objects MUST terminate with an error and MUST NOT emit successful ResponseDone.
The encoder MUST emit terminal metadata once and ignore events after termination.
A completed canonical ResponseDone without a finish reason MUST emit STOP in SSE.
Queued and in-progress outcomes MUST NOT terminate an SSE stream as completed output.

## Verification

GEM-11. Run existing Gemini feature, protocol contract, cross-protocol, and streaming checks affected by this change.
Do not add tests without an explicit user request.
Offline verification does not establish live provider acceptance or cryptographic validity of synthetic signatures.
