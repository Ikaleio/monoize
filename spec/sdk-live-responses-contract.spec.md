# SDK Verification Scripts Specification

## 0. Status

- **Purpose:** Define the supported local SDK verification scripts under `sdk-tests/`.
- **Scope:** Applies to `sdk-tests/openai-smoke.ts` and `sdk-tests/openai-agent-tool-smoke.ts`.

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
- one model entry for `gpt-4o-mini` with multiplier `1.0`
- one channel entry with:
  - `name = "sdk-mock-channel"`
  - `base_url = http://127.0.0.1:{MOCK_PORT}`
  - `api_key = MOCK_API_KEY`

SDK19a. `sdk-tests/openai-smoke.ts` MUST set `provider_type = "responses"` in the provider request from SDK19.

SDK19b. `sdk-tests/openai-agent-tool-smoke.ts` MUST set `provider_type = "chat_completion"` in the provider request from SDK19.

SDK20. The runner MUST fail if any request in SDK14-SDK19 returns a non-success HTTP status or omits a required response field.

SDK21. Each runner MUST seed pricing metadata for `gpt-4o-mini` by sending `PUT /api/dashboard/model-metadata/gpt-4o-mini` with non-empty input and output token price fields before issuing any forwarded SDK request.

SDK22. The runner MUST fail if the pricing-metadata request in SDK21 returns a non-success HTTP status.

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
