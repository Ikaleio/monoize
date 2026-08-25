<div align="center">

<img src="frontend/public/monoize.svg" width="96" alt="Monoize logo">

# Monoize

**AI APIs look alike. Their contracts differ.**

Monoize is a Rust gateway for OpenAI Responses, Chat Completions, Anthropic Messages, Gemini, embeddings, and image APIs. It converts protocol semantics. It routes one logical model across multiple upstream channels. It handles failures between clients and upstreams.

[English](README.md) · [简体中文](README.zh-CN.md)
</div>

## The problem

An AI API gateway does more than map JSON fields.

Responses, Chat Completions, and Messages use different data models for conversation history, reasoning, tools, usage, errors, and streaming. A converter can return HTTP 200 and still corrupt the conversation. It can drop encrypted reasoning, attach a delta to the wrong content block, duplicate a stream event, or turn a tool result into assistant text.

Routing also requires a state machine. A gateway must retry a failed channel. It must move to the next provider. It must stop retrying after it sends the first response byte to the client. If a gateway switches upstreams after that point, it splices two different generations into one stream.

Clients and upstreams also differ in boundary behavior. Claude Code, OpenRouter-compatible clients, Codex WebSocket clients, DeepSeek tool loops, image providers, and provider SSE implementations make different assumptions.

Inline images add latency. Upload time and upstream image preprocessing increase time to first token. When every retry carries the same base64 payload, this cost multiplies.
## Where common converters fail

Format support does not equal protocol correctness. These public examples were checked on 2026-08-10:

- OpenAI uses `encrypted_content` to preserve reasoning across stateless multi-turn requests. In New API commit [`823e263`](https://github.com/QuantumNous/new-api/commit/823e26304a396854ace30b52b98ec497c2dd9c36), the Responses output DTO [cannot represent that field](https://github.com/QuantumNous/new-api/blob/823e26304a396854ace30b52b98ec497c2dd9c36/relaykit/dto/openai_response.go#L327-L339). The Responses-to-Chat converter [reads only reasoning text](https://github.com/QuantumNous/new-api/blob/823e26304a396854ace30b52b98ec497c2dd9c36/relaykit/relayconvert/internal/oai_responses/to_oai_chat_resp.go#L212-L229). The conversion drops encrypted reasoning. See the [OpenAI reasoning guide](https://developers.openai.com/api/docs/guides/reasoning#preserve-reasoning-without-stored-responses).
- LiteLLM issue [#32357](https://github.com/BerriAI/litellm/issues/32357) reports an Anthropic adapter that emits `message_start` twice and sends `thinking_delta` inside a text block. Anthropic SDKs discard that reasoning because the event violates block lifecycle rules.
- New API issue [#5480](https://github.com/QuantumNous/new-api/issues/5480) documents streaming relay paths that retain complete generated text in memory to count tokens. Proxy memory grows with output length and concurrency.

Monoize addresses these issues in its protocol model, stream state machines, routing rules, and resource bounds.
## What Monoize does

### Semantic protocol conversion

Monoize decodes each supported protocol into URP v2. URP v2 is a flat, typed representation. It separates text, reasoning summaries, raw reasoning, encrypted reasoning, tool calls, tool results, images, files, refusals, usage, and control boundaries into distinct nodes.

The selected upstream adapter encodes these nodes into the target protocol. The response follows the same path in reverse.

This design provides these properties:

- The Responses, Chat Completions, and Messages matrix is tested in streaming and non-streaming modes.
- Encrypted reasoning remains separate from visible reasoning. Optional `mz2` envelopes preserve opaque reasoning across incompatible replay formats.
- Tool-call IDs, parallel calls, multipart tool results, and assistant history keep their roles.
- Responses output items and Messages content blocks maintain balanced lifecycle events.
- Unknown fields within the same protocol family pass through. Monoize strips unsafe nested fields at cross-family boundaries to avoid invalid requests.

See the [protocol test matrix](spec/urp-v2-flat-protocol-test-matrix.spec.md) for test cases.

### Retry before commit

A logical model can match several ordered Providers. Each Provider contains weighted Channels.

Monoize evaluates routes in a bounded waterfall:

1. Select the first matching Provider.
2. Select an eligible Channel by weight and affinity.
3. Retry retryable failures within configured budgets.
4. When the current route is exhausted, advance to the next route.
5. Stop fallback after sending the first response byte.

Network errors, timeouts, `429`, and selected `5xx` responses advance the waterfall. Client errors such as `400`, `401`, `403`, and `422` stop the waterfall. Circuit breakers, passive health checks, active probes, cooldowns, and model affinity exclude unhealthy channels from the path.

Monoize never switches providers in the middle of a visible stream. Transition rules are defined in the [routing specification](spec/monoize-upstream-routing.spec.md).
### Boundary transforms

Core adapters handle standard protocol conversion. Ordered transforms handle behavior specific to a client, provider, model, or API key.

Examples include:

- OpenRouter structured reasoning and trailing usage chunks.
- DeepSeek reasoning replay during tool loops.
- Anthropic thinking blocks and signatures.
- Codex Responses WebSocket sessions and `/v1/responses/compact`.
- Converting data-URL images to provider-native image sources.
- Splitting SSE frames for clients with small line buffers.
- Cleaning up orphaned tool calls and repairing consecutive identical roles.
- Role mapping between `system` and `developer`.
- Prompt-cache breakpoints for system prompts, tools, and OpenAI schemas.
- Removing provider-specific headers, adding model suffixes, and mapping token budgets.

Transforms can run at Provider, global, or API-key scope. Model globs select matching rules. See the [transform specification](spec/urp-transform-system.spec.md).
### Request image compression

`compress_user_message_images` is an opt-in request transform. It resizes and recompresses inline user images before routing them upstream. Supported output formats include JPEG, PNG, WebP, and JPEG XL.

The transform preserves the image node and provider-specific detail hints. It skips unsupported formats and remote URLs. Input bytes, decoded pixels, concurrent encodes, cache entries, and cache bytes have explicit bounds.

The transform reduces request size and image-related TTFT. Cached results avoid duplicate encoding during retries and repeated requests.
### Low forwarding overhead

Monoize reduces proxy overhead:

- Rust and Tokio handle asynchronous I/O without an interpreter on the request path.
- The default stream path decodes and encodes incrementally through bounded channels.
- Usage estimation updates counters as deltas arrive, without buffering the complete response text.
- Rate-limit keys, health state, affinity, API-key caches, request capture, WebSocket history, and image transforms have explicit memory bounds.
- A release build embeds the React dashboard. One process serves the API, the dashboard, and Prometheus metrics.

Some response transforms intentionally use buffered streaming. Replicate also uses that path. The default bridge remains incremental.

This comparison concerns proxy-side CPU, memory, and latency. It does not claim to make an upstream model generate tokens faster. See [stream usage accounting](src/handlers/usage.rs) and [runtime resource bounds](spec/runtime-resource-bounds.spec.md).
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
| `POST` | `/v1/images/edits` | Multipart image edits |

Every forwarding endpoint also has an `/api/v1/...` alias.

### Upstream channel types

| Type | Native upstream contract |
| --- | --- |
| `responses` | OpenAI Responses-compatible |
| `chat_completion` | OpenAI Chat Completions-compatible |
| `messages` | Anthropic Messages-compatible |
| `gemini` | Google Gemini native |
| `openai_image` | OpenAI-compatible image API |
| `replicate` | Replicate predictions |

Providers define routing order, retry budgets, and health policy. Channels hold the actual upstream type, base URL, credential, model mapping, weight, and timeout.

## Request path

```text
Client protocol
    │
    ▼
Decode to typed URP v2
    │
    ▼
Provider waterfall ──► weighted Channel ──► circuit breaker / affinity
    │                                           │
    │                                retry or fail forward
    │                                before the first byte
    ▼
Provider, global, and API-key transforms
    │
    ▼
Upstream protocol encoding
    │
    ▼
Upstream stream ──► URP v2 events ──► downstream protocol events
```

## Quick start

Run Monoize once with Bun:

```bash
bunx monoize
```

Or install it globally:

```bash
bun add --global monoize
monoize
```

The same package works with npm and pnpm:

```bash
npx monoize
# or: pnpm dlx monoize
# global: npm install --global monoize
# global: pnpm add --global monoize
```

The package manager installs only the native binary for the current operating system and CPU. The npm package supports GNU-libc and musl-based Linux distributions, macOS, and Windows on x86-64 and ARM64. Linux packages use static musl executables and do not depend on the host libc or `libstdc++`.

To build from source, install a stable Rust toolchain and [Bun](https://bun.sh/). A release build compiles the frontend and embeds it in the executable.

```bash
cargo build --release
./target/release/monoize
```

Open `http://localhost:8080`. The first registered account becomes `super_admin`, even when public registration is disabled. Then:

1. Create a Provider.
2. Add at least one Channel with its upstream URL and credential.
3. Map a logical model to the Channel.
4. Create an API key.

### Docker

The published image supports Linux x86-64 and ARM64. Run it with a persistent SQLite volume:

```bash
docker run -d \
  --name monoize \
  --restart unless-stopped \
  -p 8080:8080 \
  -v monoize-data:/app/data \
  ghcr.io/ikaleio/monoize:latest
```

To use PostgreSQL or a non-default SQLite location, set `MONOIZE_DATABASE_DSN` with `-e`.

Call the logical model through any supported downstream protocol:

```bash
curl http://localhost:8080/v1/responses \
  -H 'Authorization: Bearer sk-your-monoize-key' \
  -H 'Content-Type: application/json' \
  -d '{
    "model": "your-logical-model",
    "input": "Explain why stream fallback must stop after the first byte.",
    "stream": true
  }'
```

## Configuration

Runtime bootstrap uses environment variables. The database stores Providers, Channels, models, routing policy, transforms, users, and API keys. The dashboard manages them.

| Variable | Default | Purpose |
| --- | --- | --- |
| `MONOIZE_LISTEN` | `0.0.0.0:8080` | HTTP listen address |
| `MONOIZE_DATABASE_DSN` | `sqlite://./data/monoize.db` | SQLite or PostgreSQL DSN |
| `DATABASE_URL` | unset | Fallback DSN when `MONOIZE_DATABASE_DSN` is unset |
| `MONOIZE_METRICS_PATH` | `/metrics` | Prometheus metrics path |
| `MONOIZE_HTTP_BODY_MAX_BYTES` | `52428800` | Forwarding request-body limit |
| `MONOIZE_TRUSTED_PROXY_CIDRS` | `127.0.0.0/8,::1/128` | Trusted reverse-proxy networks; an explicitly empty value disables trust |
| `MONOIZE_UPSTREAM_PROXY_URL` | unset | Node-local outbound HTTP(S) proxy for upstream calls; channels may override per channel via `proxy_url` |
| `MONOIZE_CAP_API_ENDPOINT` | unset | Optional external Cap site endpoint including the site-key path; unset uses Monoize's built-in Cap service |
| `MONOIZE_CAP_SECRET_KEY` | unset | Secret for the external Cap site; configure it together with `MONOIZE_CAP_API_ENDPOINT` |

Dashboard login and registration use Monoize's built-in Cap proof-of-work service by default and require no extra configuration. Administrators can disable human verification under system settings, which removes bot and credential-stuffing protection from these endpoints. To use [Cap Standalone](https://capjs.js.org/guide/) instead, create one site key, set both variables above, and allow the dashboard origin in Cap's CORS configuration. Monoize then verifies each token through the site's `/siteverify` endpoint.

Monoize supports SQLite and PostgreSQL. One Monoize application process is the supported writer for its business tables.

### Primary/replica deployment

Monoize can run as one writable primary plus read-only replicas. All nodes share one PostgreSQL database (`spec/primary-replica-deployment.spec.md`). Replicas serve `/v1/**` traffic only. They do not serve the dashboard. Replicas ship request logs and billing deltas to the primary over an authenticated internal API. Balance checks subtract locally unshipped charges to keep overspend bounded. Failover is manual: to promote a replica, switch its role and restart it.

| Variable | Default | Purpose |
| --- | --- | --- |
| `MONOIZE_NODE_ROLE` | `primary` | `primary` or `replica` |
| `MONOIZE_PRIMARY_INTERNAL_URL` | required on replicas | Base URL of the primary for metering shipment |
| `MONOIZE_REPLICA_TOKEN` | unset | Shared secret: required on replicas; on a primary it enables the ingest endpoint |
| `MONOIZE_REPLICA_ID` | auto-generated and persisted | Fixed replica identity (UUID v4). When unset, an identity is generated once and persisted as `replica-identity` inside the metering spool directory, so the ID survives restarts |
| `MONOIZE_CONFIG_POLL_INTERVAL_SECONDS` | `5` | Replica config-epoch poll interval |
| `MONOIZE_METERING_SHIP_INTERVAL_SECONDS` | `10` | Replica metering shipment interval |
| `MONOIZE_METERING_SHIP_BATCH_MAX_ENTRIES` | `500` | Per-batch entry cap (hard cap 2000) |
| `MONOIZE_REPLICA_METERING_SPOOL_DIR` | `./data/replica-metering-spool` | Durable delta spool directory |

## Operations

The embedded dashboard manages:

- Providers, Channels, health, priority, model mapping, and pricing multipliers.
- API keys, quotas, model restrictions, IP allowlists, transforms, and sub-accounts.
- Users, balances, nano-dollar billing, and an append-only ledger.
- Request logs with TTFB, duration, token usage, cost, errors, and tried routes.
- Model metadata and pricing imported from [Models.dev](https://models.dev).
- Prometheus metrics and live operational views.

Request capture is opt-in and bounded. Credentials and prompt bodies are not part of normal observability logs.

## Limits and non-goals

- Monoize forwards tool definitions and tool calls. It does not execute tools locally.
- Monoize does not provide OpenAI Files, vector stores, or local retrieval.
- Responses object storage and later object retrieval are not implemented.
- Fallback ends after downstream bytes begin. Mid-stream provider switching is intentionally forbidden.
- Cross-family conversion preserves representable semantics. Provider-specific nested fields that have no safe target representation are intentionally removed.
- Image compression is opt-in. It does not fetch arbitrary remote image URLs unless the separate URL-resolution transform is configured.

## Release artifacts

A GitHub Release whose tag equals `v` plus the Cargo package version triggers the [release workflow](.github/workflows/release.yml). The workflow builds native x86-64 and ARM64 binaries for Linux, macOS, and Windows.

Linux and macOS assets use `tar.gz`. Windows assets use `zip`. Every archive includes both READMEs and the license. Every archive has a separate SHA-256 file. The workflow uploads nothing until all six builds and all checksum checks succeed.

A manual workflow run executes the same six-platform preflight. It does not change a GitHub Release. The exact asset contract is defined in the [release artifact specification](spec/release-artifacts.spec.md).

The workflow also builds seven npm tarballs: one TypeScript-derived launcher and six platform packages. A normal Bun, npm, or pnpm installation selects one platform package through `os` and `cpu` metadata. The npm publication job authenticates through npm Trusted Publishing and GitHub Actions OIDC; it does not use a long-lived npm token. The exact npm contract is defined in the [npm CLI distribution specification](spec/npm-cli-distribution.spec.md).

## Development and verification

Run the backend tests:

```bash
cargo test
```

Run the frontend checks:

```bash
cd frontend
bun install
bun run lint
bun run build
```

Run the live three-protocol suite against a configured instance:

```bash
cd sdk-tests
bun run live-protocol-suite.ts <baseURL> <apiKey> <model>
```

The suite checks non-streaming text, streaming text, tool loops, and streaming tool loops through Chat Completions, Responses, and Messages.

Observable behavior is specified under [`spec/`](spec/). Code and specifications change together.

## License

Monoize is licensed under the [MIT License](LICENSE).
