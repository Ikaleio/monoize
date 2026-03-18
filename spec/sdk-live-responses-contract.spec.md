## Scope

This specification applies to the live SDK contract runner `sdk-tests/ai-sdk-responses-sse-contract.ts`.

## Environment and secrecy

SLRC1. The runner MUST obtain the downstream base URL from environment variable `AI_SDK_RESPONSES_BASE_URL`.

SLRC2. The runner MUST obtain the downstream API key from environment variable `AI_SDK_RESPONSES_API_KEY`.

SLRC3. The runner MUST obtain the logical model from environment variable `AI_SDK_RESPONSES_MODEL` when set; otherwise it MUST default to `gpt-5.4`.

SLRC4. The runner MUST NOT require secrets to be committed into repository files, fixtures, snapshots, or README examples.

SLRC5. Any runner-emitted success or failure text that mentions the configured base URL or API key MUST replace them with fixed redacted placeholders rather than host, path, prefix, or suffix fragments.

## Request construction

SLRC6. The AI SDK reachability probe and the direct `POST /v1/responses` stream request MUST both use the same logical model value selected by SLRC3.

SLRC7. The direct stream request MUST send `stream: true` and MUST include one function tool named `get_weather`.

## Stream validation

SLRC8. The runner MUST require downstream SSE events `response.created` and `response.in_progress` before reporting success.

SLRC9. For every observed `response.output_text.done` event, the runner MUST validate presence of `output_index`, `content_index`, `item_id`, `text`, and `logprobs`.

SLRC10. For every observed `response.function_call_arguments.done` event, the runner MUST validate presence of `output_index`, `item_id`, `name`, `call_id`, and `arguments`.

SLRC11. The runner MUST require at least one terminal content completion event from this set:

- `response.output_text.done`
- `response.function_call_arguments.done`

SLRC12. The runner MUST reject any stream in which a child `.done` event for a given `output_index` arrives after that same output item's `response.output_item.done`.

SLRC13. The runner MUST allow valid tool-only streams that emit `response.function_call_arguments.done` but do not emit `response.output_text.done`.
