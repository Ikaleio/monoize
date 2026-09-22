<div align="center">

<img src="frontend/public/monoize.svg" width="96" alt="Monoize logo">

# Monoize

**AI APIs look alike. Their contracts differ.**

Monoize is a Rust gateway for AI APIs. It converts semantics between OpenAI Responses, Chat Completions, and Anthropic Messages. It routes one logical model across multiple upstream Channels. It serves the management dashboard from the same process.

[English](README.md) · [简体中文](README.zh-CN.md)

</div>

<div align="center">
  <img src="docs/public/images/en/dashboard.webp" width="880" alt="Monoize dashboard preview">
</div>

## Why Monoize

An AI API gateway does more than map JSON fields.

Responses, Chat Completions, and Messages use different data models for conversation history, reasoning, tools, usage, and streaming. A field-level converter can return HTTP 200 and still corrupt the conversation:

1. **Lost reasoning context.** Responses carries reasoning state across stateless requests in `encrypted_content`. A converter that cannot represent this field drops it silently in multi-turn conversations.
2. **Broken stream lifecycle.** Each protocol defines its own open and close rules for content blocks. A reasoning delta inside a text block, or a duplicated start event, makes downstream SDKs discard data.
3. **Spliced streams on failover.** A gateway must retry failed upstreams. After it sends the first response byte, switching upstreams splices two different generations into one stream.

Monoize addresses these problems with a typed protocol model, stream state machines, and a bounded routing waterfall.

## Core design

### 1. URP v2 protocol model

Monoize decodes each supported protocol into URP v2. URP v2 is a flat, typed representation. It separates text, reasoning summaries, raw reasoning, encrypted reasoning, tool calls, tool results, images, files, refusals, usage, and control boundaries into distinct nodes. The upstream adapter encodes these nodes into the target protocol. The response follows the same path in reverse.

- Encrypted reasoning remains separate from visible reasoning. Optional `mz2` envelopes preserve opaque reasoning across incompatible replay formats.
- Tool-call IDs, parallel calls, multipart tool results, and assistant history keep their roles.
- Responses output items and Messages content blocks keep balanced lifecycle events.
- Unknown fields within one protocol family pass through. Cross-family conversion strips nested fields the target cannot represent.

### 2. Retry before the first byte

A logical model can match several ordered Providers. Each Provider contains weighted Channels.

1. Select the first matching Provider.
2. Select a healthy Channel by weight and Channel affinity.
3. Retry retryable failures within configured budgets.
4. When the current route is exhausted, advance to the next route.
5. Stop fallback after sending the first response byte.

Network errors, timeouts, `429`, and selected `5xx` responses advance the waterfall. `400`, `401`, `403`, and `422` stop it. Circuit breakers, passive health checks, active probes, and cooldowns exclude unhealthy Channels from the path. Monoize never switches Providers in the middle of a visible stream. See the [routing specification](spec/monoize-upstream-routing.spec.md).

### 3. Low forwarding overhead

- Rust and Tokio handle asynchronous I/O without an interpreter on the request path.
- The default stream path decodes and encodes incrementally through bounded channels.
- Usage counters update as deltas arrive, without buffering the complete response text.

Some response transforms rebuild the full response and use a buffered synthetic stream. Replicate also uses that path. The default bridge remains incremental. This comparison concerns proxy-side CPU, memory, and latency. It does not claim to make an upstream model generate tokens faster.

## Capabilities

**Protocol conversion.** Streaming and non-streaming conversion among Responses, Chat Completions, and Messages. Gemini, OpenAI image APIs, and Replicate connect as upstreams.

**Routing.** Ordered Provider fallback, weighted Channels, circuit breakers and active probes, Channel affinity, and per-API-key model redirects.

**Boundary transforms**, attached at global, Provider, or API-key scope and matched by model glob:

- OpenRouter structured reasoning and trailing usage chunks.
- DeepSeek reasoning replay during tool loops.
- Anthropic thinking blocks and signatures.
- Codex Responses WebSocket sessions and `/v1/responses/compact`.
- Prompt-cache breakpoints for system prompts, tools, and history.
- `compress_user_message_images`: recompress inline user images to JPEG, PNG, WebP, or JPEG XL to reduce TTFT.
- Custom JavaScript transforms that rewrite requests and responses at runtime.
- SSE frame splitting, orphaned tool-call cleanup, consecutive-role merging, and `system`/`developer` role mapping.

**Operations.**

- Embedded React dashboard: Providers, Channels, model mapping, pricing, users, API keys, and sub-accounts.
- Nano-dollar billing, multipliers, and an append-only ledger. Price sync from [models.dev](https://models.dev), [OpenRouter](https://openrouter.ai), and new-api.
- Request logs with TTFB, duration, tokens, cost, errors, and tried routes.
- Request Capture: per-request event timelines, opt-in and bounded.
- Built-in Cap proof-of-work human verification with no external Captcha service.
- Prometheus `/metrics`.

## Request path

```text
Client protocol (Responses / Chat Completions / Messages)
    │
    ▼
Decode to URP v2
    │
    ▼
Provider waterfall ──► weighted Channel ──► circuit breaker / affinity
    │                                           │
    │                             retry or advance before the first byte
    ▼
Transforms (global / Provider / API key)
    │
    ▼
Encode to upstream protocol
    │
    ▼
Upstream stream ──► URP v2 events ──► downstream protocol events
```

## Quick start

### npm / Bun

```bash
bunx monoize
# or: npx monoize
```

Global install:

```bash
bun add --global monoize
monoize
```

The package manager installs only the native binary for the current OS and CPU. Supported targets: Linux x86-64 and ARM64 (glibc and musl), Windows x86-64.

### Docker

```bash
docker run -d \
  --name monoize \
  --restart unless-stopped \
  -p 8080:8080 \
  -v monoize-data:/app/data \
  ghcr.io/ikaleio/monoize:latest
```

`docker-compose.yml`:

```yaml
services:
  monoize:
    image: ghcr.io/ikaleio/monoize:latest
    restart: unless-stopped
    ports:
      - "8080:8080"
    volumes:
      - ./data:/app/data
    # For PostgreSQL:
    # environment:
    #   - MONOIZE_DATABASE_DSN=postgres://user:pass@host/monoize
```

### Build from source

Requires a stable Rust toolchain and [Bun](https://bun.sh/). A release build compiles the frontend and embeds it in the executable.

```bash
cargo build --release
./target/release/monoize
```

### First configuration

Open `http://localhost:8080`. The first registered account becomes `super_admin`, even when public registration is disabled.

1. Create a Provider.
2. Add at least one Channel with its upstream URL and credential.
3. Map a logical model to the Channel.
4. Create an API key.

```bash
curl http://localhost:8080/v1/chat/completions \
  -H 'Authorization: Bearer sk-your-monoize-key' \
  -H 'Content-Type: application/json' \
  -d '{
    "model": "your-logical-model",
    "messages": [{"role": "user", "content": "Hello"}],
    "stream": true
  }'
```

## Supported surface

### Downstream endpoints

| Method | Endpoint | Contract |
| --- | --- | --- |
| `GET` | `/v1/models` | OpenAI-compatible model list |
| `POST` | `/v1/responses` | OpenAI Responses, streaming or non-streaming |
| `GET` | `/v1/responses` | OpenAI Responses WebSocket transport |
| `POST` | `/v1/responses/compact` | Responses compaction |
| `POST` | `/v1/chat/completions` | OpenAI Chat Completions |
| `POST` | `/v1/messages` | Anthropic Messages |
| `POST` | `/v1/embeddings` | Embeddings |
| `POST` | `/v1/images/generations` | Image generation |
| `POST` | `/v1/images/edits` | Image edits with multipart uploads or JSON references |

Every forwarding endpoint also has an `/api/v1/...` alias.

### Upstream Channel types

| Type | Native upstream contract |
| --- | --- |
| `responses` | OpenAI Responses-compatible |
| `chat_completion` | OpenAI Chat Completions-compatible |
| `messages` | Anthropic Messages-compatible |
| `gemini` | Google Gemini native |
| `openai_image` | OpenAI-compatible image API |
| `replicate` | Replicate predictions |

## Configuration

Runtime bootstrap uses environment variables. The database stores Providers, Channels, models, routing, transforms, users, and API keys. The dashboard manages them.

| Variable | Default | Purpose |
| --- | --- | --- |
| `MONOIZE_LISTEN` | `0.0.0.0:8080` | HTTP listen address |
| `MONOIZE_DATABASE_DSN` | `sqlite://./data/monoize.db` | SQLite or PostgreSQL DSN |
| `MONOIZE_METRICS_PATH` | `/metrics` | Prometheus metrics path |
| `MONOIZE_HTTP_BODY_MAX_BYTES` | `52428800` | Forwarding request-body limit |
| `MONOIZE_TRUSTED_PROXY_CIDRS` | `127.0.0.0/8,::1/128` | Trusted reverse-proxy networks; an explicitly empty value disables trust |
| `MONOIZE_UPSTREAM_PROXY_URL` | unset | Node-local outbound HTTP(S) proxy; Channels may override via `proxy_url` |
| `MONOIZE_CAP_API_ENDPOINT` | unset | External Cap site endpoint; unset uses the built-in Cap service |
| `MONOIZE_CAP_SECRET_KEY` | unset | Secret for the external Cap site; set together with the endpoint |

### Primary/replica deployment

Monoize can run as one writable primary plus read-only replicas that share one PostgreSQL database. Replicas serve `/v1/**` traffic only and do not serve the dashboard. Replicas ship request logs and billing deltas to the primary over an authenticated internal API. Balance checks subtract locally unshipped charges to bound overspend. Failover is manual: switch the role and restart. See the [primary/replica specification](spec/primary-replica-deployment.spec.md).

| Variable | Default | Purpose |
| --- | --- | --- |
| `MONOIZE_NODE_ROLE` | `primary` | `primary` or `replica` |
| `MONOIZE_PRIMARY_INTERNAL_URL` | required on replicas | Internal base URL of the primary |
| `MONOIZE_REPLICA_TOKEN` | unset | Shared secret; required on replicas, enables the ingest endpoint on the primary |
| `MONOIZE_REPLICA_ID` | generated and persisted | Fixed replica identity (UUID v4) |
| `MONOIZE_CONFIG_POLL_INTERVAL_SECONDS` | `5` | Replica config poll interval |
| `MONOIZE_METERING_SHIP_INTERVAL_SECONDS` | `10` | Replica metering shipment interval |
| `MONOIZE_REPLICA_METERING_SPOOL_DIR` | `./data/replica-metering-spool` | Durable metering spool directory |

## Limits and non-goals

- Monoize forwards tool definitions and tool calls. It does not execute tools locally.
- No OpenAI Files, vector stores, or local retrieval.
- No Responses object storage or later retrieval by ID.
- Fallback ends after downstream bytes begin. Mid-stream Provider switching is forbidden.
- Cross-family conversion preserves representable semantics. Provider-specific nested fields without a safe target representation are removed.
- Image compression is opt-in. Remote image URLs are not fetched unless the separate URL-resolution transform is configured.

## Specifications and documentation

Observable behavior is specified under [`spec/`](spec/). Code and specifications change together. Full documentation lives under [`docs/`](docs/).

## License

Monoize is licensed under the [MIT License](LICENSE).
