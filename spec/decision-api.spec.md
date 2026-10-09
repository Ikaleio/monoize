# Decision API Specification

## 0. Status

- **Subsystem:** Decision model forwarding.
- **Scope:** Monoize accepts decision requests in two wire formats, System One and OpenAI Decisions. Monoize routes each request to decision Channels and converts between the two formats when the downstream format differs from the upstream format.
- **Non-goals:** streaming, conditional fallback on low confidence, decision-based model routing for chat requests, and direct upstreams that use neither upstream path in DR-P1.

## 1. Terminology

- **Decision request:** A request that asks typed questions about one shared input and receives one typed answer per question.
- **System One format:** The TypeSafe format. Request fields: `model`, `state`, `questions` (object keyed by question key). Question types: `noul`, `choice`, `score`. Response fields: `model`, `answers` (object keyed by question key), `usage`.
- **OpenAI Decisions format:** The OpenAI format. Request fields: `model`, `input`, `questions` (array). Question types: `predicate`, `choice`, `score`. Response fields: `model`, `answers` (array in question order), `usage`.
- **Decision format:** One of `system_one` and `openai_decisions`.
- **Decision API type:** An effective API type (`monoize-upstream-routing.spec.md` AT-1) whose value is a decision format.
- **Downstream format:** The decision format of the endpoint that the client called.
- **Upstream format:** The decision format equal to the effective API type of one attempt.
- **Same-format attempt:** An attempt whose upstream format equals the downstream format.
- **Cross-format attempt:** An attempt whose upstream format differs from the downstream format.
- **Decision IR:** The typed canonical decision representation in §5. Monoize uses it only for cross-format attempts.

## 2. Endpoints

DR-E1. Monoize MUST implement these forwarding endpoints:

| Endpoint | Downstream format |
|---|---|
| `POST /v1/systemone` | `system_one` |
| `POST /v1/decisions` | `openai_decisions` |

DR-E2. Each endpoint MUST also be served at `/api` + endpoint path with identical semantics.

DR-E3. Both endpoints MUST authenticate with forwarding API keys, apply configured model redirects, enforce model permissions, and apply the pre-forward balance guard exactly as `POST /v1/embeddings` does (`unified_responses_proxy.spec.md` §2.1, §7.9 DE1).

DR-E4. Both endpoints are non-streaming. The response MUST be one JSON body. Monoize MUST NOT open an SSE response.

DR-E5. `max_multiplier` MUST be read from the request body field `max_multiplier` or the max-multiplier request header, with the same precedence and API-key ceiling as `POST /v1/embeddings`. Monoize MUST remove `max_multiplier` from every upstream body.

## 3. Request validation

DR-V1. The request body MUST be a JSON object. Otherwise Monoize MUST return `400 invalid_request`.

DR-V2. `model` MUST be a non-empty string. Otherwise Monoize MUST return `400 invalid_request`.

DR-V3. For `POST /v1/systemone`:

- `state` MUST be present and MUST be a string, an object, or an array.
- `questions` MUST be an object with at least 1 entry.

DR-V4. For `POST /v1/decisions`:

- `input` MUST be present and MUST be a string or an array.
- `questions` MUST be an array with at least 1 element.

DR-V5. A request that violates DR-V3 or DR-V4 MUST return `400 invalid_request` before routing.

DR-V6. Monoize MUST NOT enforce upstream limits such as question count, option count, level count, or token count. The upstream enforces those limits.

DR-V7. Monoize MUST decode the request into the Decision IR (§5) only when at least one routed attempt is a cross-format attempt. A decode failure in that case MUST return `400 invalid_request` with a message that names the offending field path.

## 4. Routing

DR-R1. Routing MUST use the same Provider and Channel model-map matching rules, group eligibility, circuit-breaker filters, weighted ordering, and retry limits as chat requests (`monoize-upstream-routing.spec.md`).

DR-R2. For a decision endpoint, a Channel is a candidate only when its effective API type is a decision API type. Monoize MUST apply this filter before it applies the Provider attempt limit (`max_retries`).

DR-R3. For every non-decision forwarding endpoint (`/v1/responses`, `/v1/responses/compact`, `/v1/chat/completions`, `/v1/messages`, `/v1/embeddings`, `/v1/images/generations`, `/v1/images/edits`, and their aliases), a Channel is a candidate only when its effective API type is not a decision API type. Monoize MUST apply this filter before it applies the Provider attempt limit.

DR-R4. Both decision endpoints MUST accept attempts of both decision API types.

DR-R5. Before the balance guard, Monoize MUST compute the upstream body for each decision format that occurs in the routed attempts (§6). Monoize MUST remove each attempt whose upstream body computation failed. If no attempt remains and at least one attempt was removed by this rule, Monoize MUST return `400 unsupported_decision_input` with the first conversion error message.

DR-R6. Monoize MUST compute each upstream body once per decision format per request and MUST reuse it for every attempt of that format, replacing only `model`.

DR-P1. The upstream path MUST be selected by the effective API type:

| Effective API type | Upstream request |
|---|---|
| `system_one` | `POST {base_url}/v1/systemone` |
| `openai_decisions` | `POST {base_url}/v1/decisions` |

DR-R7. Retry, same-Channel retry, passive failure recording, circuit breaking, Channel affinity, and exhausted-error construction MUST follow the behavior of `POST /v1/embeddings`. Because no response byte is sent before the upstream response completes, every attempt failure MAY move to the next attempt.

## 5. Decision IR

DR-IR1. A Decision IR request contains:

- `input`: one of
  - `Text(string)`;
  - `Structured(object | array)`: a System One `state` that is an object or an array;
  - `Parts(Part[])`: OpenAI message content parts in message order, where `Part` is `Text(string)` or `Image { image_url: string, detail: string? }`.
- `questions`: an ordered array of questions. Each question contains:
  - `name: string?`;
  - `instructions: JSON value` (System One accepts string, object, or array; OpenAI accepts string);
  - `kind`: one of
    - `Binary { when_true: JSON value?, when_false: JSON value? }`;
    - `Choice { options: { value: string | boolean, description: JSON value? }[] }`;
    - `Score { levels: { label: JSON value, description: string? }[] }`.

DR-IR2. A Decision IR answer set contains one answer per question in question order. Each answer is one of:

- `Binary { probability: number }`;
- `Choice { index: integer, probabilities: number[], confidence: number? }`, where `index` and `probabilities` refer to the question's `options` in order;
- `Score { score: number, probabilities: number[], confidence: number? }`, where `probabilities` refer to the question's `levels` in order;
- `Refusal`.

DR-IR3. Decision IR MUST NOT carry top-level request fields other than `input` and `questions`. A cross-format upstream body MUST NOT contain downstream fields that §6 does not map. This rule drops, for example, OpenAI `safety_identifier` and OpenRouter `provider`, `user`, and `session_id` on cross-format attempts.

### 5.1 Decoding a System One request

DR-D1. `state`: a string decodes to `Text`. An object or an array decodes to `Structured`.

DR-D2. Each `questions` entry decodes to one question in the order of the decoded JSON object. The entry key becomes `name`. The entry MUST be an object with string `type` and with `instructions` present. Otherwise decode fails.

DR-D3. `noul` decodes to `Binary`. When `criteria` is present, it MUST be an object. `criteria.true` becomes `when_true`. `criteria.false` becomes `when_false`.

DR-D4. `choice` decodes to `Choice`. `criteria` MUST be an object. Each entry becomes an option with string `value` equal to the entry key. A non-null entry value becomes `description`.

DR-D5. `score` decodes to `Score`. `criteria` MUST be an array. Each element becomes a level with `label` equal to the element and no `description`.

DR-D6. Any other `type` value MUST fail decode.

### 5.2 Decoding an OpenAI Decisions request

DR-D10. `input`: a string decodes to `Text`. An array decodes to `Parts`. Each array element MUST be an object. When the element has `type`, it MUST be `"message"`. When the element has `role`, it MUST be `"user"`. Its `content` MUST be a string or an array of parts. A string content becomes one `Text` part. An `input_text` part becomes `Text(text)`. An `input_image` part becomes `Image { image_url, detail }`. Any other part type MUST fail decode.

DR-D11. Each `questions` element MUST be an object with string `type` and string `instructions`. A present `name` MUST be a string.

DR-D12. `predicate` decodes to `Binary` with no criteria.

DR-D13. `choice` decodes to `Choice`. `choices` MUST be an array. Each element MUST have `value` of type string or boolean. A present `description` becomes `description`.

DR-D14. `score` decodes to `Score`. `levels` MUST be an array. Each element MUST have `label`. A present string `description` becomes `description`.

DR-D15. Any other `type` value MUST fail decode.

## 6. Upstream body construction

DR-B1. For a same-format attempt, the upstream body MUST be the downstream body with `model` replaced by the attempt `upstream_model` and `max_multiplier` removed. All other fields MUST be forwarded unchanged.

DR-B2. For a cross-format attempt, the upstream body MUST be encoded from the Decision IR by §6.1 or §6.2, with `model` equal to the attempt `upstream_model`.

DR-B3. Text conversion: a JSON value converts to text as itself when it is a string, and as its compact JSON serialization otherwise.

### 6.1 Encoding a System One upstream body

DR-B10. `state`:

- `Text(s)` encodes to `s`.
- `Parts` that contain only `Text` parts encode to the part texts joined by `"\n\n"`.
- `Parts` that contain an `Image` part MUST fail with message `image input requires an openai_decisions channel`.

DR-B11. Question keys: a question with `name` uses `name`. A question without `name` at zero-based position `i` uses `q{i}`; if that key is already used, Monoize MUST append `_` until the key is unused. Two questions with the same `name` MUST fail with message `duplicate question name: {name}`.

DR-B12. `Binary` encodes to `{ "type": "noul", "instructions": instructions }`. When `when_true` or `when_false` is present, `criteria` MUST contain the present values under `true` and `false`.

DR-B13. `Choice` encodes to `{ "type": "choice", "instructions": instructions, "criteria": object }`. Each option contributes key `value` (boolean `true` and `false` convert to `"true"` and `"false"`) and value `description` or `null`. Two options with the same key MUST fail with message `duplicate choice value: {key}`.

DR-B14. `Score` encodes to `{ "type": "score", "instructions": instructions, "criteria": array }`. Each level contributes `text(label)` when `description` is absent, and `text(label) + ": " + description` otherwise.

### 6.2 Encoding an OpenAI Decisions upstream body

DR-B20. `input`:

- `Text(s)` encodes to `s`.
- `Structured(v)` encodes to `text(v)` (DR-B3).
- `Parts` encode to one user message `{ "role": "user", "content": parts }`, where `Text(t)` becomes `{ "type": "input_text", "text": t }` and `Image` becomes `{ "type": "input_image", "image_url": image_url }` plus `detail` when present.

DR-B21. Each question encodes to an object with `type`, `instructions = text(instructions)`, and `name` when the question has `name`.

DR-B22. `Binary` encodes to `type = "predicate"`. When `when_true` or `when_false` is present, `instructions` MUST be `text(instructions)` followed by `"\n\nAnswer true when: " + text(when_true)` when `when_true` is present and `"\n\nAnswer false when: " + text(when_false)` when `when_false` is present.

DR-B23. `Choice` encodes to `type = "choice"` and `choices`. Each option becomes `{ "value": value }` plus `description = text(description)` when present.

DR-B24. `Score` encodes to `type = "score"` and `levels`. Each level becomes `{ "label": text(label) }` plus `description` when present.

## 7. Response handling

DR-S1. Monoize MUST parse billing usage from the raw upstream response before conversion. `usage.input_tokens` and `usage.output_tokens` are required. `usage.input_tokens_details.cached_tokens` and `usage.input_tokens_details.cache_write_tokens` are optional cache counts.

DR-S2. Missing usage MUST follow the missing-usage rules of `model-pricing.spec.md` (MP-F3), exactly as for `POST /v1/embeddings`.

DR-S3. Billing MUST use the reported token counts and the effective model price row. Decision upstreams that report non-zero `output_tokens` bill output tokens at the configured output price. An operator who prices output at zero gets zero output charge.

DR-S4. For a same-format attempt, the downstream body MUST equal the upstream body with top-level `model` replaced by the logical model.

DR-S5. For a cross-format attempt, Monoize MUST decode the upstream answers into a Decision IR answer set (§7.1) and encode the downstream body (§7.2). The downstream `model` MUST be the logical model.

DR-S6. A cross-format answer decode failure MUST be an attempt failure with HTTP status `502` and code `upstream_decode_error`. Monoize MUST record it like other non-retryable attempt failures and MUST continue with the next attempt. Monoize MUST NOT charge for that attempt.

### 7.1 Decoding upstream answers

DR-A1. System One answers: `answers` MUST be an object. The answer for question key `k` (DR-B11) MUST be present. `noul` reads `noul`. `choice` reads `choice`, which MUST equal an option key, plus `probabilities` (object keyed by option key) and optional `confidence`. `score` reads `score`, `probabilities` (object keyed by decimal level index), and optional `confidence`. An answer object with `type = "refusal"` decodes to `Refusal`.

DR-A2. OpenAI answers: `answers` MUST be an array with one element per question in question order. `predicate` reads `probability`. `choice` reads `choice`, which MUST equal an option value (string or boolean), plus `probabilities` (array of `{ value, probability }`) and optional `confidence`. `score` reads `score`, `probabilities` (array of `{ value, probability }`, where `value` is the level index), and optional `confidence`. `refusal` decodes to `Refusal`.

DR-A3. A probability entry absent for an option or level decodes to `0`. An answer type that does not match its question kind MUST fail decode.

### 7.2 Encoding downstream answers

DR-A10. System One downstream: `answers` is an object keyed by the question key of DR-B11 applied to the downstream request.

- `Binary` → `{ "type": "noul", "noul": probability }`.
- `Choice` → `{ "type": "choice", "choice": key, "probabilities": { key: p }, "confidence": c }`.
- `Score` → `{ "type": "score", "score": s, "legend": { "i": text(label_i) }, "probabilities": { "i": p }, "confidence": c }`.
- `Refusal` → `{ "type": "refusal" }`.
- `confidence` MUST be omitted when absent.

DR-A11. OpenAI downstream: `answers` is an array in question order. Each element contains `name` when the question has `name`.

- `Binary` → `{ "type": "predicate", "probability": probability }`.
- `Choice` → `{ "type": "choice", "choice": value, "probabilities": [{ "value": value_j, "probability": p_j }], "confidence": c }`, where each value keeps its original string or boolean type.
- `Score` → `{ "type": "score", "score": s, "probabilities": [{ "value": j, "label": text(label_j), "probability": p_j }], "confidence": c }`.
- `Refusal` → `{ "type": "refusal" }`.
- `confidence` MUST be omitted when absent.

DR-A12. Downstream `usage`:

- System One downstream: `{ "input_tokens": in, "output_tokens": out }`.
- OpenAI downstream: `{ "input_tokens": in, "input_tokens_details": { "cached_tokens": cached, "cache_write_tokens": write }, "output_tokens": out, "output_tokens_details": { "reasoning_tokens": 0 }, "total_tokens": in + out }`, with absent cache counts equal to `0`.

## 8. Logging and observability

DR-L1. Each request MUST produce one request log entry through the existing request logging pipeline, with `is_stream = false`, the logical model, the selected Channel, the parsed usage, and the charge.

DR-L2. Upstream response model mismatch detection MUST follow `POST /v1/embeddings`.

## 9. Channel management

DR-C1. `system_one` and `openai_decisions` MUST be valid Channel `provider_type` values and valid `api_type_overrides[].api_type` values.

DR-C2. The Channel test probe for `system_one` MUST send `POST {base}/v1/systemone` with one `noul` question. The probe for `openai_decisions` MUST send `POST {base}/v1/decisions` with one `predicate` question. Both probes MUST be non-streaming.

DR-C3. Fetching Channel models for both types MUST call `GET {base}/v1/models` with bearer authentication. For `openai_decisions`, model ids are `data[].id`. For `system_one`, model ids are `data[].id`; when that list is empty, model ids are `models[].name` without prefix stripping. Duplicates MUST be removed and ids MUST be sorted ascending.

DR-C4. A database migration MUST rename the Channel `provider_type` value `systemone` to `system_one`. It MUST also rename every Provider `api_type_overrides[].api_type` value `systemone` to `system_one`. Request log rows MUST keep their recorded `effective_provider_type` value.
