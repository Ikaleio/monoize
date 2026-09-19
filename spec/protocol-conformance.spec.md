# Protocol conformance tests

## Scope

PCT-1. Tests cover Chat Completions, Responses, and Messages conversion boundaries.
They verify native-to-URP-to-native and URP-to-native-to-URP mappings.
They do not prove upstream service acceptance, cryptographic validity, or every possible input combination.
Each documented feature MUST have a supported mapping, explicit omission, or explicit rejection assertion.

PCT-2. Request cases MUST exercise stream=false and stream=true when the feature affects both modes.
Response cases MUST exercise non-stream decoding, native SSE decoding, and synthetic SSE encoding when applicable.
Stream tests MUST inspect terminal state and emitted wire frames, not only HTTP status or initial frames.

PCT-3. Matrices MUST include normal values, absence, empty values, mutation, deletion, and invalid input where applicable.
Failures MUST NOT become successful empty responses. Transport errors and valid failed response snapshots remain distinct.
A first terminal event MUST prevent any later successful terminal event.
Unsupported protocol-native features MUST NOT leak into another protocol's ordinary text or tool surface.

## Reasoning

PCT-4. Raw content, summary, and encrypted payloads MUST use distinct fixture values.
Tests MUST cover raw-only, summary-only, encrypted-only, and combined values.
Deleting one field MUST NOT restore it from another field or native extras.
Protocol projections with one readable thinking field MUST document their representational limit.
Chat and Responses MUST NOT relabel raw content as summary.

PCT-5. Encrypted fixtures are opaque synthetic bytes or JSON values, not real vendor credentials.
Envelope tests MUST cover exact payload recovery, source/model match, mismatch, missing item ID, repeated wrapping, and malformed envelopes.
Stream envelope tests MUST cover string fragments, independent nodes, full snapshots, terminal replacement, and error cleanup.
The signature or ciphertext MUST NOT appear as raw content or summary.

PCT-5a. Wrapping a null, empty-string, empty-array, or empty-object encrypted payload MUST NOT create meaningful reasoning output.
An empty payload MUST remain empty. The wrapper MUST NOT convert absence into a replay token.

## Shared semantic metadata

PCT-6. Citation tests MUST distinguish answer ranges from document source ranges and preserve origin-scoped unknown fields.
Token-score tests MUST bind scores to current text bytes, including UTF-8 fragments, and reject stale scores after text mutation.
Outcome tests MUST assert typed authority over native extras.
Usage iteration tests MUST assert complete accounting and absence of double counting.

## Documentation sources

The source review date is 2026-09-19. Protocol agents maintain concrete test mappings in the coverage report.

- [OpenAI Chat Completions](https://developers.openai.com/api/reference/resources/chat/subresources/completions/methods/create)
- [OpenAI Responses](https://developers.openai.com/api/reference/resources/responses/methods/create)
- [OpenAI streaming events](https://developers.openai.com/api/reference/resources/responses/streaming-events)
- [OpenAI reasoning](https://developers.openai.com/api/docs/guides/reasoning)
- [DeepSeek thinking mode](https://api-docs.deepseek.com/guides/thinking_mode/)
- [OpenRouter reasoning](https://openrouter.ai/docs/guides/best-practices/reasoning-tokens)
- [Claude Messages](https://platform.claude.com/docs/en/api/messages)
- [Claude streaming](https://platform.claude.com/docs/en/build-with-claude/streaming)
- [Claude thinking](https://platform.claude.com/docs/en/build-with-claude/thinking)
- [Claude citations](https://platform.claude.com/docs/en/build-with-claude/citations)
- [Claude compaction](https://platform.claude.com/docs/en/build-with-claude/compaction)
