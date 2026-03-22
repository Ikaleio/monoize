# sdk-tests

To install dependencies:

```bash
bun install
```

To run:

```bash
bun run openai-smoke.ts
```

`openai-smoke.ts` is the supported automated SDK smoke test in this directory. It boots the bundled mock server when needed, starts a temporary Monoize instance, and validates the OpenAI-compatible path end to end.
