# sdk-tests

To install dependencies:

```bash
bun install
```

To run:

```bash
bun run openai-smoke.ts
bun run openai-agent-tool-smoke.ts
bun run live-protocol-suite.ts <baseURL> <apiKey> <model>
```

`openai-smoke.ts` is the baseline OpenAI SDK smoke test in this directory. It boots the bundled mock server when needed, starts a temporary Monoize instance, and validates the OpenAI-compatible path end to end.

`openai-agent-tool-smoke.ts` is a local-only AI SDK verification script. It reuses the same Monoize bootstrap flow, targets Monoize through `/v1` with a dashboard-created forwarding key, executes a real multi-step tool loop with `generateText(...)`, and prints a clear `PASS` or `FAIL` outcome based on whether tool calls and tool results appear in `result.steps`.

`live-protocol-suite.ts` is a live AI SDK protocol suite for an existing Monoize-compatible endpoint. It accepts a base URL, API key, and model, then runs basic text, streaming text, and tool-loop checks through Chat Completions, Responses, and Anthropic Messages providers. It does not print the API key.

These scripts are verification helpers for local development. They are not deploy gates.
