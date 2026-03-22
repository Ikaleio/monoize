# SDK OpenAI Smoke Test Specification

## 0. Status

- **Purpose:** Define the supported automated SDK smoke test under `sdk-tests/`.
- **Scope:** Applies to `sdk-tests/openai-smoke.ts` only.

## 1. Runtime environment

SDK1. The runner MUST derive its working directories relative to its own file location.

SDK2. The runner MUST select the mock upstream port from `MOCK_PORT` when present; otherwise it MUST default to `3901`.

SDK3. The runner MUST select the Monoize listen port from `MONOIZE_PORT` when present; otherwise it MUST default to `8085`.

SDK4. The runner MUST construct `MONOIZE_DATABASE_DSN` as a SQLite database file under `sdk-tests/.tmp/` whose filename is unique per selected Monoize port.

SDK5. The runner MUST set `MONOIZE_LISTEN` to `127.0.0.1:{MONOIZE_PORT}` for the Monoize child process.

SDK6. The runner MUST set `MOCK_API_KEY` in the environment used for the Monoize child process and dashboard bootstrap/provider-creation flow, defaulting to `mock-key` when the variable is absent in the parent environment.

SDK7. The runner MUST NOT require repository-committed credentials, fixtures, or snapshots.

## 2. Process orchestration

SDK8. Before starting a mock child process, the runner MUST probe `GET http://127.0.0.1:{MOCK_PORT}/health`.

SDK9. If the health probe in SDK8 returns an HTTP success status, the runner MUST reuse the existing mock server and MUST NOT start another mock child process.

SDK10. If the health probe in SDK8 does not succeed, the runner MUST start the mock server by executing `bun run server.ts` in the `mock/` directory and MUST wait until the same health endpoint responds successfully.

SDK11. The runner MUST start Monoize by executing `cargo run --quiet` in the repository root.

SDK12. After starting Monoize, the runner MUST wait for `GET http://127.0.0.1:{MONOIZE_PORT}/metrics` to return an HTTP success status before sending dashboard setup requests.

SDK13. If the Monoize child process exits before SDK12 completes, the runner MUST fail.

## 3. Dashboard bootstrap

SDK14. The runner MUST register a dashboard user by sending `POST /api/dashboard/auth/register` with a username derived from the Monoize port and a fixed password.

SDK15. The runner MUST require the registration response to contain both:

- `token: string`
- `user.id: string`

SDK16. After registration, the runner MUST send `PUT /api/dashboard/users/{user_id}` with `balance_unlimited = true` using the returned bearer token.

SDK17. The runner MUST create a forwarding API key by sending `POST /api/dashboard/tokens` with body `{ "name": "sdk-forward-key" }` using the returned bearer token.

SDK18. The token-creation response in SDK17 MUST contain `key: string`.

SDK19. The runner MUST create exactly one provider by sending `POST /api/dashboard/providers` with:

- `name = "sdk-mock-provider"`
- `provider_type = "responses"`
- one model entry for `gpt-4o-mini` with multiplier `1.0`
- one channel entry with:
  - `name = "sdk-mock-channel"`
  - `base_url = http://127.0.0.1:{MOCK_PORT}`
  - `api_key = MOCK_API_KEY`

SDK20. The runner MUST fail if any request in SDK14–SDK19 returns a non-success HTTP status or omits a required response field.

## 4. Smoke assertion

SDK21. After the bootstrap steps complete, the runner MUST construct an OpenAI SDK client with:

- `apiKey =` the key produced by SDK17
- `baseURL = http://127.0.0.1:{MONOIZE_PORT}/v1`

SDK22. The runner MUST send exactly one `responses.create` request with:

- `model = "gpt-4o-mini"`
- `input = "hello from sdk"`

SDK23. The runner MUST require the returned `response.output` field to be an array with length greater than zero.

SDK24. If SDK23 succeeds, the runner MUST write `OpenAI SDK smoke test passed.` to stdout.

## 5. Cleanup

SDK25. On process completion, the runner MUST terminate any Monoize child process it started and MUST wait for that child process to exit.

SDK26. If the runner started a mock child process under SDK10, it MUST terminate that child process and MUST wait for that child process to exit.

SDK27. On process completion, the runner MUST attempt to delete the SQLite database file selected by SDK4.

SDK28. Failure to delete the temporary database file in SDK27 MAY be ignored.
