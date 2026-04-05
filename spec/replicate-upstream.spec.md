# Replicate Upstream Proxy Specification

## 1. Scope

Monoize proxies **all** Replicate prediction API endpoints upstream. The downstream interface exposed by Monoize is the **Replicate HTTP API itself** — no protocol translation occurs. This is a raw-proxy provider type; there is no URP encode/decode adapter.

## 2. Provider Type

A new `MonoizeProviderType` variant `Replicate` is added. Its `config::ProviderType` counterpart is `Replicate`.

### 2.1 Channel Configuration

Channels for a Replicate provider store:
- `base_url`: The Replicate API base (default `https://api.replicate.com`).
- `api_key`: A Replicate bearer token (format `r8_*`).

Authentication is always `Bearer` — no Header or Query auth.

### 2.2 Model Key Convention

The `model` key in the provider model map encodes the Replicate model identifier. Formats:
- `{owner}/{model}` — routes to `POST /v1/models/{owner}/{model}/predictions`.
- `{owner}/{model}:{version_id}` — routes to `POST /v1/predictions` with `version` = `{owner}/{model}:{version_id}`.
- `deployment:{owner}/{name}` — routes to `POST /v1/deployments/{owner}/{name}/predictions`.

The `redirect` field in `MonoizeModelEntry` may rewrite the model key before routing.

## 3. Exposed Endpoints

All endpoints require `Authorization: Bearer sk-...` (Monoize API key).

### 3.1 Create Prediction

```
POST /v1/replicate/predictions
```

Request body:
```json
{
  "model": "<logical_model>",
  "input": { ... },
  "webhook": "...",
  "webhook_events_filter": ["start", "output", "logs", "completed"],
  "stream": true|false
}
```

Behaviour:
1. Authenticate tenant via Monoize API key.
2. Extract `model` from body. Apply model redirects from API key config.
3. Enforce model allowlist.
4. Route to an eligible Replicate provider/channel via the standard waterfall routing algorithm (§ database-provider-routing.spec.md).
5. Resolve upstream model from `redirect` in model entry.
6. Determine upstream endpoint:
   - If upstream model matches `deployment:{owner}/{name}` → `POST /v1/deployments/{owner}/{name}/predictions`.
   - If upstream model contains `:` (i.e. `{owner}/{model}:{version}`) → `POST /v1/predictions` with `"version": "{owner}/{model}:{version}"`.
   - Otherwise (`{owner}/{model}`) → `POST /v1/models/{owner}/{model}/predictions`.
7. Construct upstream request body:
   - Copy all fields from the downstream body except `model` and `max_multiplier`.
   - For version-based routing, add `"version"` field.
8. Forward request to upstream with `Authorization: Bearer {channel.api_key}`.
9. Relay upstream headers `Prefer` and `Cancel-After` if present in the downstream request.
10. Return upstream response verbatim (status code, JSON body).

### 3.2 Get Prediction

```
GET /v1/replicate/predictions/{prediction_id}
```

Behaviour:
1. Authenticate tenant.
2. Select the first enabled Replicate provider/channel.
3. Forward `GET /v1/predictions/{prediction_id}` upstream.
4. Return upstream response verbatim.

### 3.3 List Predictions

```
GET /v1/replicate/predictions
```

Query parameters forwarded verbatim: `created_after`, `created_before`, `source`.

Behaviour:
1. Authenticate tenant.
2. Select the first enabled Replicate provider/channel.
3. Forward `GET /v1/predictions` upstream with query string preserved.
4. Return upstream response verbatim.

### 3.4 Cancel Prediction

```
POST /v1/replicate/predictions/{prediction_id}/cancel
```

Behaviour:
1. Authenticate tenant.
2. Select the first enabled Replicate provider/channel.
3. Forward `POST /v1/predictions/{prediction_id}/cancel` upstream (empty body).
4. Return upstream response verbatim.

## 4. Routing

Create-prediction follows the full waterfall routing:
- Provider eligibility: enabled, model exists, multiplier ceiling, group eligibility.
- Channel eligibility: enabled, weight > 0, healthy.
- Weighted shuffle across channels.
- Retry on 429 / 5xx / network error.
- Stop on 400 / 401 / 403 / 422.

Get / List / Cancel use a simplified routing: pick the first enabled Replicate provider with any healthy channel. These operations are not model-scoped since prediction IDs are global.

## 5. Billing

Create-prediction logs a request log entry with:
- `model`: the logical model.
- `upstream_model`: the resolved upstream model.
- `provider_id`, `channel_id`: from the attempt.
- `status`: success / error.
- No token-based billing — Replicate bills by compute time, not tokens.
- `charge_nano_usd` = `None` (not computed by Monoize).

Get / List / Cancel operations are not billed.

## 6. Streaming

Replicate SSE streaming is **not** proxied through URP events. If a prediction model supports streaming, the `urls.stream` field in the create-prediction response contains the SSE URL. The client consumes the SSE stream directly from Replicate (or via a later Monoize streaming proxy endpoint if needed).

The `stream_upstream_to_urp_events` dispatcher returns `provider_type_not_supported` for `Replicate`.

## 7. Active Probing

Replicate providers are **excluded** from the active health probe loop. The probe system skips providers where `provider_type == Replicate`. Health assessment relies on passive failure tracking only.

## 8. URP Integration

`Replicate` is explicitly excluded from URP encode/decode dispatch:
- `encode_request_for_provider` returns `provider_type_not_supported` for `Replicate`.
- `decode_response_from_provider` returns `provider_type_not_supported` for `Replicate`.

This enforces that Replicate traffic flows only through the dedicated handler, never through the generic URP pipeline.

## 9. Transform Support

Provider-level and API-key-level transforms do **not** apply to Replicate requests — the raw proxy preserves the body exactly as submitted by the client.

## 10. Unknown Field Preservation

All fields in the request body (except `model` and `max_multiplier`, which are Monoize-specific routing fields) are forwarded upstream unchanged.
