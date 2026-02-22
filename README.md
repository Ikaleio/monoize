<div align="center">

# Monoize

**One endpoint. Every model.**

A high-performance AI gateway that unifies OpenAI, Anthropic, Google Gemini, xAI Grok, and DeepSeek behind a single API — with built-in routing, billing, transforms, and a full admin dashboard.

Written in Rust. Embeds its own frontend. Ships as a single binary.

</div>

---

## Why Monoize?

Most AI proxy solutions are Node/Python wrappers that glue APIs together. Monoize is different:

- **Protocol-native translation** — Internally converts every request to a unified representation (URP-Proto), then encodes to each provider's native wire format. No lossy pass-through.
- **Single binary deployment** — The React dashboard compiles into the Rust binary at build time. No separate frontend server, no static file hosting.
- **Streaming-first** — SSE streaming works across all provider types with automatic format adaptation, including mid-stream tool calls and reasoning tokens.
- **Transform pipeline** — Rewrite, inject, strip, or reshape requests and responses at the API-key or provider level — without touching upstream configs.

## Features

### API Gateway

- **Multi-format ingress** — Accept requests via OpenAI Chat Completions (`/v1/chat/completions`), OpenAI Responses (`/v1/responses`), Anthropic Messages (`/v1/messages`), or Embeddings (`/v1/embeddings`)
- **Provider-native egress** — Route to `responses`, `chat_completion`, `messages`, `gemini`, or `grok` upstream types with full format conversion
- **Waterfall routing** — Ordered provider evaluation with weighted channel selection, automatic fail-forward, and configurable retry policies
- **Health checks** — Passive failure tracking with cooldown + active probing to recover unhealthy channels
- **Unknown field preservation** — Forward provider-specific parameters (e.g. `logprobs`, `top_k`) without explicit support
- **Reasoning normalization** — Translate reasoning effort hints (`none`/`minimum`/`low`/`medium`/`high`/`xhigh`) across OpenAI, Anthropic, and native formats

### Transform System

A pluggable, ordered pipeline that runs on every request:

| Transform | Description |
|-----------|-------------|
| `inject_system_prompt` | Prepend or append system instructions |
| `reasoning_to_think_xml` | Convert structured reasoning to `<think>` XML |
| `think_xml_to_reasoning` | Parse `<think>` XML back to structured reasoning |
| `reasoning_effort_to_budget` | Map effort levels to token budgets |
| `strip_reasoning` | Remove reasoning from responses |
| `system_to_developer_role` | Rewrite `system` role to `developer` |
| `merge_consecutive_roles` | Collapse adjacent same-role messages |
| `override_max_tokens` | Force a max output token limit |
| `set_field` / `remove_field` | Arbitrary JSON field manipulation |
| `force_stream` | Force streaming mode on all requests |

Transforms can be scoped per API key or per provider, filtered by model glob, and applied to request or response phase.

### Dashboard

A full admin console served from the same binary:

- **Home** — Real-time overview with usage charts (consumption distribution, trends, call rankings)
- **Providers** — Visual provider/channel management with drag-to-reorder priority, per-model redirect & multiplier config, and live health indicators
- **API Keys** — Per-user token management with expiry, quotas, model restrictions, IP whitelists, max multiplier ceilings, and per-key transform rules
- **Request Logs** — Virtualized, filterable log viewer with timing metrics (duration, TTFB), token counts, cost tracking, and IP display
- **Model Metadata** — Browse and sync pricing data from [Models.dev](https://models.dev)
- **Playground** — Interactive chat completions tester with streaming output
- **Users** — User management with role-based access and prepaid nano-dollar balance system
- **Settings** — System configuration (API base URL, etc.)
- **i18n** — Multi-language UI

### Billing

- Nano-dollar precision (1 USD = 1,000,000,000 nano-USD) — no floating-point drift
- Per-request charge calculation using model metadata pricing with multiplier support
- Cached token and reasoning token aware billing
- Balance guard on every forwarded request
- Append-only ledger for full audit trail

## Quick Start

### Prerequisites

- [Rust](https://rustup.rs/) (2024 edition)
- [Bun](https://bun.sh/) (for frontend builds)

### Development

```bash
# Start the frontend dev server (with API proxy to backend)
cd frontend && bun install && bun dev

# In another terminal, start the backend
cargo run
```

The dashboard will be available at `http://localhost:5173` (Vite dev server), proxying API calls to the Rust backend on port `8080`.

### Production

```bash
# Release build embeds the frontend into the binary
cargo build --release

# Run — that's it, no nginx/caddy needed
./target/release/monoize
```

The single binary serves both the API and the dashboard on `0.0.0.0:8080`.

## Configuration

Monoize is configured via environment variables — no config files.

| Variable | Default | Description |
|----------|---------|-------------|
| `MONOIZE_LISTEN` | `0.0.0.0:8080` | Server listen address |
| `MONOIZE_DATABASE_DSN` | `sqlite://./data/monoize.db` | Database connection string |
| `DATABASE_URL` | *(fallback for above)* | Alternative DSN variable |
| `MONOIZE_METRICS_PATH` | `/metrics` | Prometheus metrics endpoint |

## API Endpoints

### Forwarding (proxy)

All forwarding endpoints require `Authorization: Bearer sk-...` using a dashboard-managed API key.

| Method | Path | Description |
|--------|------|-------------|
| `POST` | `/v1/responses` | OpenAI Responses API |
| `POST` | `/v1/chat/completions` | OpenAI Chat Completions |
| `POST` | `/v1/messages` | Anthropic Messages |
| `POST` | `/v1/embeddings` | Embeddings (pass-through) |

All endpoints are also available under `/api/v1/...`.

### Dashboard

The dashboard API lives under `/api/dashboard/*` and uses session-based authentication.

## Architecture

```
                         ┌─────────────────────────────────┐
                         │           Downstream            │
                         │  (your app / SDK / curl / etc)  │
                         └──────────────┬──────────────────┘
                                        │
                    ┌───────────────────────────────────────────┐
                    │  /v1/responses  │  /v1/chat/completions  │
                    │  /v1/messages   │  /v1/embeddings        │
                    └───────────────────┬───────────────────────┘
                                        │
                    ┌───────────────────────────────────────────┐
                    │             Auth + Balance Guard          │
                    ├───────────────────────────────────────────┤
                    │          Decode to URP-Proto              │
                    ├───────────────────────────────────────────┤
                    │      API-Key Request Transforms           │
                    ├───────────────────────────────────────────┤
                    │     Waterfall Router (fail-forward)       │
                    │   ┌─────────┐ ┌─────────┐ ┌─────────┐   │
                    │   │Provider1│→│Provider2│→│Provider3│   │
                    │   └─────────┘ └─────────┘ └─────────┘   │
                    ├───────────────────────────────────────────┤
                    │     Provider Request Transforms           │
                    ├───────────────────────────────────────────┤
                    │       Encode to Provider Format           │
                    └───────────────────┬───────────────────────┘
                                        │
              ┌─────────────┬───────────┼───────────┬──────────────┐
              │             │           │           │              │
         ┌────┴────┐  ┌────┴────┐ ┌────┴────┐ ┌────┴────┐  ┌─────┴────┐
         │ OpenAI  │  │Anthropic│ │ Gemini  │ │  Grok   │  │DeepSeek  │
         │responses│  │messages │ │ native  │ │ native  │  │  chat    │
         └─────────┘  └─────────┘ └─────────┘ └─────────┘  └──────────┘
```

## Tech Stack

| Layer | Technology |
|-------|-----------|
| Runtime | Rust, Tokio, Axum |
| Database | SQLite (via SQLx) |
| Auth | JWT + Argon2 |
| Metrics | Prometheus (`metrics` crate) |
| Frontend | React 19, TypeScript, Vite |
| Styling | Tailwind CSS, shadcn/ui |
| Animation | Framer Motion |
| Data Fetching | SWR |
| Charts | Recharts |
| Virtualization | react-virtuoso |
| i18n | i18next |

## License

See [LICENSE](LICENSE) for details.
