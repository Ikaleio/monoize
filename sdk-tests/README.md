# sdk-tests

To install dependencies:

```bash
bun install
```

To run:

```bash
bun run openai-smoke.ts
bun run openai-agent-tool-smoke.ts
```

`openai-smoke.ts` is the baseline OpenAI SDK smoke test in this directory. It boots the bundled mock server when needed, starts a temporary Monoize instance, and validates the OpenAI-compatible path end to end.

`openai-agent-tool-smoke.ts` is a local-only AI SDK verification script. It reuses the same Monoize bootstrap flow, targets Monoize through `/v1` with a dashboard-created forwarding key, executes a real multi-step tool loop with `generateText(...)`, and prints a clear `PASS` or `FAIL` outcome based on whether tool calls and tool results appear in `result.steps`.

These scripts are verification helpers for local development. They are not deploy gates.
