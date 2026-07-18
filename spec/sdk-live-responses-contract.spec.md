# SDK Verification Scripts Specification

## 0. Status

- **Purpose:** Define the supported SDK verification scripts under `sdk-tests/`.
- **Scope:** Applies to `sdk-tests/openai-smoke.ts`, `sdk-tests/openai-agent-tool-smoke.ts`, and `sdk-tests/live-protocol-suite.ts`.

## 1. Runtime environment

SDK1. Each runner MUST derive its working directories relative to its own file location.

SDK2. Each runner MUST select the mock upstream port from `MOCK_PORT` when present; otherwise it MUST default to `3901`.

SDK3. Each runner MUST select the Monoize listen port from `MONOIZE_PORT` when present; otherwise it MUST default to `8085`.

SDK4. Each runner MUST construct `MONOIZE_DATABASE_DSN` as a SQLite database file under `sdk-tests/.tmp/` whose filename is unique per selected Monoize port.

SDK5. Each runner MUST set `MONOIZE_LISTEN` to `127.0.0.1:{MONOIZE_PORT}` for the Monoize child process.

SDK6. Each runner MUST set `MOCK_API_KEY` in the environment used for the Monoize child process and dashboard bootstrap/provider-creation flow, defaulting to `mock-key` when the variable is absent in the parent environment.

SDK7. Each runner MUST NOT require repository-committed credentials, fixtures, or snapshots.

## 2. Process orchestration

SDK8. Before starting a mock child process, each runner MUST probe `GET http://127.0.0.1:{MOCK_PORT}/health`.

SDK9. If the health probe in SDK8 returns an HTTP success status, the runner MUST reuse the existing mock server and MUST NOT start another mock child process.

SDK10. If the health probe in SDK8 does not succeed, the runner MUST start the mock server by executing `bun run server.ts` in the `mock/` directory and MUST wait until the same health endpoint responds successfully.

SDK11. Each runner MUST start Monoize by executing `cargo run --quiet` in the repository root.

SDK12. After starting Monoize, each runner MUST wait for `GET http://127.0.0.1:{MONOIZE_PORT}/metrics` to return an HTTP success status before sending dashboard setup requests.

SDK13. If the Monoize child process exits before SDK12 completes, the runner MUST fail.

## 3. Dashboard bootstrap

SDK14. Each runner MUST register a dashboard user by sending `POST /api/dashboard/auth/register` with a username derived from the Monoize port and a fixed password.

SDK15. The registration response in SDK14 MUST contain both:

- `token: string`
- `user.id: string`

SDK16. After registration, each runner MUST send `PUT /api/dashboard/users/{user_id}` with `balance_unlimited = true` using the returned bearer token.

SDK17. Each runner MUST create a forwarding API key by sending `POST /api/dashboard/tokens` with body `{ "name": "sdk-forward-key" }` using the returned bearer token.

SDK18. The token-creation response in SDK17 MUST contain `key: string`.

SDK19. Each runner MUST create exactly one provider by sending `POST /api/dashboard/providers` with:

- `name = "sdk-mock-provider"`
- one Channel model entry for `gpt-4o-mini` with multiplier `1.0`
- one channel entry with:
  - `name = "sdk-mock-channel"`
  - `provider_type` set by SDK19a or SDK19b
  - `base_url = http://127.0.0.1:{MOCK_PORT}`
  - `api_key = MOCK_API_KEY`
  - `models = { "gpt-4o-mini": { "redirect": null, "multiplier": 1.0 } }`

SDK19a. `sdk-tests/openai-smoke.ts` MUST set the channel `provider_type = "responses"` in the provider request from SDK19.

SDK19b. `sdk-tests/openai-agent-tool-smoke.ts` MUST set the channel `provider_type = "chat_completion"` in the provider request from SDK19.

SDK20. The runner MUST fail if any request in SDK14-SDK19 returns a non-success HTTP status or omits a required response field.

SDK21. Each runner MUST seed pricing metadata for `gpt-4o-mini` by sending `PUT /api/dashboard/model-metadata/gpt-4o-mini` with non-empty input and output token price fields before issuing any forwarded SDK request.

SDK21a. After SDK21, each runner MUST create exactly two manual billing-rate rows by sending authenticated `PUT` requests to:

- `/api/dashboard/billing-rates/sdk-gpt-4o-mini-input`
- `/api/dashboard/billing-rates/sdk-gpt-4o-mini-output`

Each request body MUST contain:

- `source = "manual"`
- `pricing_profile = "openai"`
- `model_pattern = "gpt-4o-mini"`
- `rate_kind = "token"`
- `unit = "token"`
- `unit_price_nano_usd = "1"`
- `priority = 100`
- `enabled = true`

The input row MUST contain `usage_class = "input_uncached"`. The output row MUST contain `usage_class = "output"`. Both rows MUST omit `provider_type`, `context_tier`, `service_tier`, `modality`, and `cache_ttl` so the rows apply to both SDK19a and SDK19b.

SDK22. The runner MUST fail if the pricing-metadata request in SDK21 or either billing-rate request in SDK21a returns a non-success HTTP status.

## 4. OpenAI SDK smoke assertion

SDK23. After the bootstrap steps complete, `sdk-tests/openai-smoke.ts` MUST construct an OpenAI SDK client with:

- `apiKey =` the key produced by SDK17
- `baseURL = http://127.0.0.1:{MONOIZE_PORT}/v1`

SDK24. `sdk-tests/openai-smoke.ts` MUST send exactly one `responses.create` request with:

- `model = "gpt-4o-mini"`
- `input = "hello from sdk"`

SDK25. `sdk-tests/openai-smoke.ts` MUST require the returned `response.output` field to be an array with length greater than zero.

SDK26. If SDK25 succeeds, `sdk-tests/openai-smoke.ts` MUST write `OpenAI SDK smoke test passed.` to stdout.

## 5. AI SDK multi-step tool assertion

SDK27. `sdk-tests/openai-agent-tool-smoke.ts` MUST reuse the runtime environment, process orchestration, dashboard bootstrap, and provider-creation flow defined by SDK1-SDK22.

SDK28. After the bootstrap steps complete, `sdk-tests/openai-agent-tool-smoke.ts` MUST construct an AI SDK OpenAI-compatible provider with:

- `baseURL = http://127.0.0.1:{MONOIZE_PORT}/v1`
- `apiKey =` the forwarding key produced by SDK17

SDK29. `sdk-tests/openai-agent-tool-smoke.ts` MUST call `generateText` with:

- model `gpt-4o-mini`
- exactly two tools named `weather` and `websearch`
- `stopWhen = stepCountIs(n)` for some integer `n >= 3`

SDK30. The `weather` tool in SDK29 MUST accept an object containing `city: string` and MUST return a deterministic payload containing the city string.

SDK31. The `websearch` tool in SDK29 MUST accept an object containing `query: string` and MUST return a deterministic payload containing the query string.

SDK32. The prompt in SDK29 MUST require a tool sequence that causes both tools in SDK29 to execute before the final assistant answer is produced.

SDK33. `sdk-tests/openai-agent-tool-smoke.ts` MUST require `result.steps` to show all of the following:

- at least two steps containing one or more tool calls
- at least two steps containing one or more tool results
- at least three total steps

SDK34. `sdk-tests/openai-agent-tool-smoke.ts` MUST fail if SDK33 is not satisfied.

SDK35. `sdk-tests/openai-agent-tool-smoke.ts` MUST require the final generated text to contain both the weather-tool payload and the websearch-tool payload.

SDK36. If SDK33 and SDK35 succeed, `sdk-tests/openai-agent-tool-smoke.ts` MUST write a stdout line beginning with `PASS openai-agent-tool-smoke`.

SDK37. If any assertion in SDK27-SDK36 fails, `sdk-tests/openai-agent-tool-smoke.ts` MUST write a stdout or stderr line beginning with `FAIL openai-agent-tool-smoke` and MUST exit non-zero.

## 6. Cleanup

SDK38. On process completion, each runner MUST terminate any Monoize child process it started and MUST wait for that child process to exit.

SDK39. If a runner started a mock child process under SDK10, it MUST terminate that child process and MUST wait for that child process to exit.

SDK40. On process completion, each runner MUST attempt to delete the SQLite database file selected by SDK4.

SDK41. Failure to delete the temporary database file in SDK40 MAY be ignored.

## 7. Live AI SDK protocol suite

SDK42. `sdk-tests/live-protocol-suite.ts` MUST be a Bun CLI.

SDK43. `sdk-tests/live-protocol-suite.ts` MUST accept exactly three positional arguments:

- `baseURL`
- `apiKey`
- `model`

SDK44. The command form MUST be:

```bash
bun run live-protocol-suite.ts <baseURL> <apiKey> <model>
```

SDK45. If the argument count is not exactly three, or if `--help` is present, `sdk-tests/live-protocol-suite.ts` MUST print usage text and exit non-zero except for `--help`, which MUST exit zero.

SDK46. `sdk-tests/live-protocol-suite.ts` MUST NOT print the API key or write the API key to a file.

SDK47. `sdk-tests/live-protocol-suite.ts` MUST normalize `baseURL` to an API base URL ending in `/v1`.

SDK48. The normalization in SDK47 MUST accept all of the following inputs:

- `https://example.invalid/v1`
- `https://example.invalid/v1/responses`
- `https://example.invalid/v1/chat/completions`
- `https://example.invalid/v1/messages`

SDK49. `sdk-tests/live-protocol-suite.ts` MUST derive the Responses endpoint as `{normalized_base_url}/responses`.

SDK50. For Chat Completions, `sdk-tests/live-protocol-suite.ts` MUST use `createOpenAICompatible` from `@ai-sdk/openai-compatible` and MUST create the model with `chatModel(model)`.

SDK51. For Responses, `sdk-tests/live-protocol-suite.ts` MUST use `createOpenResponses` from `@ai-sdk/open-responses` and MUST pass the endpoint from SDK49 as the provider `url`.

SDK52. For Messages, `sdk-tests/live-protocol-suite.ts` MUST use `createAnthropic` from `@ai-sdk/anthropic` and MUST create the model with `messages(model)`.

SDK53. `sdk-tests/live-protocol-suite.ts` MUST run one non-streaming text generation check for each protocol in SDK50-SDK52.

SDK54. Each non-streaming text generation check MUST call `generateText` and MUST require the final text to contain the check-specific sentinel string.

SDK55. `sdk-tests/live-protocol-suite.ts` MUST run one streaming text generation check for each protocol in SDK50-SDK52.

SDK56. Each streaming text generation check MUST call `streamText`, MUST consume `fullStream`, MUST require at least one text delta event, MUST require one finish event, and MUST require the aggregated text to contain the check-specific sentinel string.

SDK57. `sdk-tests/live-protocol-suite.ts` MUST run one tool-loop check for each protocol in SDK50-SDK52.

SDK58. Each tool-loop check MUST call `generateText` with a deterministic tool named `lookupWeather`.

SDK59. The `lookupWeather` tool MUST accept an object containing `city: string` and MUST return a deterministic payload containing the city string and the check-specific sentinel string.

SDK60. Each tool-loop check MUST use `stopWhen = stepCountIs(n)` for some integer `n >= 3`.

SDK61. Each tool-loop check MUST require `result.steps` to contain at least one tool call and at least one tool result whose output contains the complete deterministic tool payload from SDK59.

SDK62. Each tool-loop check MUST require the final generated text to contain the check-specific sentinel string from the deterministic tool payload in SDK59.

SDK63. The Responses non-streaming text generation check MUST require the provider response body to contain an `output` array.

SDK64. The Responses non-streaming tool-loop check MUST require the provider response body from at least one step to contain a `function_call` item in its `output` array.

SDK65. Each check MUST write exactly one stdout line beginning with `PASS {check_name}` when it succeeds.

SDK66. Each failed check MUST write one stderr line beginning with `FAIL {check_name}` and MUST include the failure message without including the API key.

SDK67. If any check fails, `sdk-tests/live-protocol-suite.ts` MUST print a final failed summary and exit non-zero.

SDK68. If all checks pass, `sdk-tests/live-protocol-suite.ts` MUST print a final passed summary and exit zero.

SDK69. `sdk-tests/live-protocol-suite.ts` MUST run one streaming tool-loop check for each protocol in SDK50-SDK52.

SDK70. Each streaming tool-loop check MUST call `streamText` with the same deterministic `lookupWeather` contract as SDK58-SDK60.

SDK71. Each streaming tool-loop check MUST consume `fullStream`, MUST require at least one `tool-call` event, MUST require at least one `tool-result` event whose output contains the complete deterministic tool payload from SDK59, MUST require one finish event, and MUST require the aggregated text to contain the check-specific sentinel string from that payload.

SDK72. The Responses streaming tool-loop check MUST enable raw chunk inclusion and MUST require at least one raw Responses event whose item type is `function_call`.

SDK73. Each tool-loop check MUST use `prepareStep` to force the named `lookupWeather` tool only when `stepNumber = 0`. For every later step, `prepareStep` MUST set `toolChoice = "auto"` so the model can emit the final assistant text after receiving the tool result. The runner MUST NOT apply a forced tool choice to every step.

SDK74. Every `generateText` and `streamText` call in the live protocol suite MUST set a timeout of 120000 milliseconds. A request that exceeds this timeout MUST fail its check instead of waiting without a bound.
