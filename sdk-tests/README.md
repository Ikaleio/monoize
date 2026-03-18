# sdk-tests

To install dependencies:

```bash
bun install
```

To run:

```bash
bun run openai-smoke.ts
```

Responses SSE contract test (requires env vars, secrets stay in env only):

```bash
AI_SDK_RESPONSES_BASE_URL="$AI_SDK_RESPONSES_BASE_URL" \
AI_SDK_RESPONSES_API_KEY="$AI_SDK_RESPONSES_API_KEY" \
AI_SDK_RESPONSES_MODEL="gpt-5.4" \
bun run ai-sdk-responses-sse-contract.ts
```

Chat Completion (local API via `CHAT_BASE_URL`):

```bash
bun run openai-chat-smoke.ts
```

This project was created using `bun init` in bun v1.3.5. [Bun](https://bun.com) is a fast all-in-one JavaScript runtime.
