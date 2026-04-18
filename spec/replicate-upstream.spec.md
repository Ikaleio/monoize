# Replicate Upstream Provider Specification

## 1. Scope

Monoize supports Replicate as an **upstream provider type**, fully integrated into the URP pipeline. Downstream clients send requests via standard Monoize endpoints (`/v1/chat/completions`, `/v1/responses`, `/v1/messages`), and the URP encode/decode layer translates between the downstream format and Replicate's prediction API. This follows the same pattern as the Gemini upstream provider.

## 2. Provider Type

`MonoizeProviderType::Replicate` and `config::ProviderType::Replicate` are added.

### 2.1 Channel Configuration

- `base_url`: Replicate API base, typically `https://api.replicate.com`.
- `api_key`: Bearer token (format `r8_*`).

Authentication: always `Bearer`.

### 2.2 Model Key Convention

The model key stored in the provider's model map determines the upstream Replicate endpoint:

| Model key format | Upstream path |
|---|---|
| `{owner}/{model}` | `POST /v1/models/{owner}/{model}/predictions` |
| `{owner}/{model}:{version_id}` | `POST /v1/predictions` (with `version` in body) |
| `deployment:{owner}/{name}` | `POST /v1/deployments/{owner}/{name}/predictions` |

`MonoizeModelEntry.redirect` may rewrite the model key before path resolution.

## 3. URP Integration

### 3.1 Request Encoding (URP → Replicate)

`urp::encode::replicate::encode_request` converts a `UrpRequest` into a Replicate prediction request body:

| URP field | Replicate field |
|---|---|
| System/Developer text nodes → concatenated text | `input.system_prompt` |
| User text nodes → concatenated text | `input.prompt` |
| User image nodes → first URL/data-URL | `input.image` |
| User file nodes → first URL/data-URL | `input.file` |
| User audio nodes → first URL/data-URL | `input.audio` |
| `max_output_tokens` | `input.max_tokens` and `input.max_new_tokens` |
| `temperature` | `input.temperature` |
| `top_p` | `input.top_p` |
| `stream` (only if true) | `stream: true` |
| `extra_body` fields | merged into top-level body |

For version-based routing (`model` contains `:`), a `version` field is added to the top-level body.

Fields `model` and `max_multiplier` are stripped from the upstream body.

### 3.2 Response Decoding (Replicate → URP)

`urp::decode::replicate::decode_response` converts a Replicate prediction response into a `UrpResponse`:

| Replicate field | URP field |
|---|---|
| `id` | `UrpResponse.id` |
| `model` | `UrpResponse.model` |
| `status` == `"succeeded"` | `finish_reason = Stop` |
| `status` == `"failed"` / `"canceled"` / `"aborted"` | `finish_reason = Other` |
| `output` (string) | `Node::Text` or `Node::Image` (if URL pointing to media) |
| `output` (array of strings) | Concatenated `Node::Text` or multiple `Node::Image` (if all are media URLs) |
| `output` (other) | JSON-serialized `Node::Text` |
| `error` (non-empty, no output) | `Node::Refusal` |
| `metrics.input_token_count` | `Usage.input_tokens` |
| `metrics.output_token_count` | `Usage.output_tokens` |

URLs pointing to `replicate.delivery` or with common image/video/audio extensions are decoded as `Node::Image`.

### 3.3 Stream Decoding (Replicate SSE → URP Events)

`stream_decode::replicate::stream_replicate_to_urp_events` converts Replicate SSE events:

| Replicate SSE event | URP event |
|---|---|
| `event: output`, `data: <text>` | `NodeDelta { Text }` |
| `event: error`, `data: <message>` | `Error` |
| `event: done` | `ResponseDone` with `finish_reason = Stop` |

In practice, streaming uses the **buffered fallback** path: the handler calls the non-stream endpoint with `Prefer: wait=60`, decodes the completed prediction response, and emits a synthetic SSE stream from the URP response. This avoids the two-step Replicate streaming flow (POST → follow `urls.stream`).

## 4. Upstream Path Resolution

`upstream_path_for_model(ProviderType::Replicate, model, stream)` resolves based on the model key:

- `deployment:{owner}/{name}` → `/v1/deployments/{owner}/{name}/predictions`
- `{owner}/{model}:{version}` → `/v1/predictions`
- `{owner}/{model}` → `/v1/models/{owner}/{model}/predictions`

The `stream` parameter is unused; the path is the same regardless.

## 5. Extra Headers

`provider_extra_headers(ProviderType::Replicate)` returns `[("prefer", "wait=60")]`. This makes Replicate block the HTTP response until the prediction completes (up to 60 seconds), enabling synchronous request/response flow through the URP pipeline.

## 6. Routing

Standard waterfall routing applies (§ database-provider-routing.spec.md):
- Provider eligibility: enabled, model exists, multiplier ceiling, group eligibility.
- Channel eligibility: enabled, weight > 0, healthy.
- Weighted channel shuffle.
- Retry on 429 / 5xx / network error; stop on 400 / 401 / 403 / 422.

## 7. Active Probing

Replicate providers are **excluded** from the active health probe loop. `probe_channel_completion` skips providers where `provider_type == Replicate`. Health assessment relies on passive failure tracking only.

## 8. Transform Support

Provider-level and API-key-level transforms apply normally through the URP pipeline, same as any other provider type.

## 9. Billing

Token-based billing applies if `metrics.input_token_count` and `metrics.output_token_count` are present in the Replicate response. Otherwise, `charge_nano_usd` is `None`.
