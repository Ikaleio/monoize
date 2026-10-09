# System One API Specification

## 0. Status

- **Purpose:** Accept a TypeSafe System One request at `POST /v1/systemone`, route it only to a Channel whose effective type is `systemone`, bill `usage.input_tokens` and `usage.output_tokens`, and return the upstream JSON unchanged.
- **Provider type:** `systemone`.
- **Non-goals:** Request capture, streaming, the TypeSafe SDK `GET /typesafe/v1/models` shape, and `Retry-After` passthrough.

## 1. Request

SO-REQ-1. The request body MUST be a JSON object. A body that is not a JSON object MUST return HTTP 400 with code `invalid_request`. Monoize MUST NOT call upstream.

SO-REQ-2. `model` MUST be a string that is non-empty after trim. Monoize MUST use that trimmed string as the logical model. Any other `model` value MUST return HTTP 400 with code `invalid_request`. Monoize MUST NOT call upstream.

SO-REQ-3. The object MUST contain the key `state`. The value MAY be any JSON value, including `null`. A missing `state` key MUST return HTTP 400 with code `invalid_request`. Monoize MUST NOT call upstream.

SO-REQ-4. `questions` MUST be a non-empty object. Each value MUST be an object. Each value's `type` MUST be a non-empty string. Monoize MUST NOT restrict `type` to a fixed set. A violation MUST return HTTP 400 with code `invalid_request`. Monoize MUST NOT call upstream.

SO-REQ-5. When `stream` is boolean `true`, Monoize MUST return HTTP 400 with code `invalid_request`. Monoize MUST NOT call upstream. Any other `stream` value is not this rejection.

## 2. Order

SO-ORDER-1. Monoize MUST process a System One request in this order:

1. Authenticate the caller.
2. Read `model` under SO-REQ-2.
3. Apply model redirects to that logical model.
4. Call `ensure_model_allowed` on the redirected logical model.
5. Validate `state`, `questions`, and `stream` under SO-REQ-3, SO-REQ-4, and SO-REQ-5.
6. Resolve `max_multiplier` from the body string `max_multiplier`, else from the same header rule as embeddings.
7. Build candidate attempts.
8. Run the balance check.
9. Write the pending request log.

SO-ORDER-2. A failure at a step MUST stop the later steps. Validation failures in steps 2 and 5 MUST NOT call upstream.

## 3. Routing

SO-ROUTE-1. Monoize MUST build attempts with `required_provider_type = systemone`. It MUST keep an attempt only when the effective provider type is `systemone`.

SO-ROUTE-2. When no attempt remains, Monoize MUST return the same exhausted-route result as embeddings: HTTP 502 and code `upstream_error`.

SO-EXCL-1. Every forwarding endpoint other than `POST /v1/systemone` and its `/api` alias MUST drop attempts whose effective provider type is `systemone`. This rule includes the case where the caller does not require a provider type.

SO-EXCL-2. `POST /v1/systemone` MUST NOT select an attempt whose effective provider type is not `systemone`.

## 4. Upstream request

SO-UP-1. Monoize MUST send `POST join_url(base_url, "/v1/systemone")`. A base URL that ends in `/v1` MUST NOT produce a repeated `/v1` segment.

SO-UP-2. Monoize MUST send `Authorization: Bearer <channel api key>`. It MUST also send the Channel `extra_headers` and the CM-AFF session headers under the same rules as `POST /v1/responses/compact`.

SO-UP-3. The upstream body MUST be a copy of the downstream body with exactly three changes:

1. Replace `model` with the attempt `upstream_model`.
2. Remove `max_multiplier`.
3. Remove every key whose name starts with `_monoize_`.

Every other field MUST pass through unchanged.

## 5. Response and billing

SO-RESP-1. When upstream returns HTTP 2xx with a JSON body, Monoize MUST return that JSON unchanged. Monoize MUST NOT replace `model` with the logical model name.

SO-RESP-2. When the upstream `model` string differs from the sent `upstream_model` under case-insensitive comparison, Monoize MUST store the upstream value in the request log field `upstream_response_model`. A missing or matching upstream `model` MUST leave that field null.

SO-BILL-1. Normalized usage MUST set `input_tokens = usage.input_tokens` and `output_tokens = usage.output_tokens`. Both values MUST be non-negative integers. The normalized usage MUST NOT set cache details or reasoning details.

SO-BILL-2. When either token field is missing or is not a non-negative integer, normalized usage is absent. Monoize MUST then apply `model-pricing.spec.md` MP-F3.

SO-BILL-3. Output tokens MUST use the model output price. Operators SHOULD set that output price to `0` when the upstream does not charge for output tokens.

SO-ERR-1. Upstream errors, same-channel retry, provider fallback, circuit-breaker updates, and error sanitization MUST follow the embeddings rules.

## 6. Probe and model list

SO-PROBE-1. A liveness probe for effective type `systemone` MUST be non-streaming `POST join_url(base, "/v1/systemone")` with this JSON body:

```json
{
  "model": "<probe model>",
  "state": "Monoize liveness probe.",
  "questions": {
    "probe": {
      "type": "noul",
      "instructions": "Is this text a liveness probe?"
    }
  }
}
```

HTTP 2xx MUST count as success.

SO-PROBE-2. A dashboard liveness test with `stream = true` for effective type `systemone` MUST return HTTP 400. The active-probe scheduler MUST keep sending a non-streaming probe.

SO-MODELS-1. A `systemone` model-list request MUST be `GET {base}/v1/models` with `Authorization: Bearer`. The URL join MUST NOT repeat a trailing `/v1`.

SO-MODELS-2. The parser MUST read `data[].id` first. When that list is empty, it MUST read `models[].name`. It MUST remove duplicate ids and return the ids sorted ascending. It MUST NOT strip a `models/` prefix from `models[].name`.

SO-MODELS-3. `POST /api/dashboard/fetch-channel-models` with `provider_type = systemone` MUST use SO-MODELS-1 and SO-MODELS-2. The Provider model-list helper MUST use that parser only when the selected Channel type is `systemone`. Every other Channel type MUST keep its current parser.
