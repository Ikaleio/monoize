# Skill: monoize-custom-transform-design

Display title: Monoize Custom Transform Design / Monoize 自定义变换设计

Use this skill when you write a custom JavaScript transform for Monoize.
A custom transform runs in an embedded QuickJS sandbox inside the URP v2 transform pipeline.

## 1. Script structure

Write one JavaScript source file with three parts, in this order:

1. One frontmatter block comment. It must be the first statement of the file.
2. Optional: one global `configSchema` constant.
3. One global function named `transform`.

Example:

```js
/**
 * @monoize-transform
 * id: js:example-rewrite
 * name: Example Rewrite
 * description: Rewrites request fields for demo.
 * author: alice
 * phase: request
 * scopes: provider, global
 * visibility: user
 */
const configSchema = {
  type: "object",
  properties: {
    target: { type: "string", title: "Target model" }
  }
};
function transform(ctx) {
  if (ctx.config.target) {
    ctx.data.model = ctx.config.target;
  }
}
```

## 2. Frontmatter format

Start the file with one block comment (`/*` or `/**`).
Put one directive line and one `key: value` pair per line inside the comment.
The first non-empty line must be exactly `@monoize-transform`.

Required keys:

| Key | Rule |
| --- | --- |
| `id` | Must match `^js:[a-z0-9]+(-[a-z0-9]+)*$`. Maximum 64 characters. |
| `name` | Non-empty. Maximum 100 characters. Plain string, no localization. |
| `description` | Non-empty. Maximum 500 characters. Plain string, no localization. |
| `author` | Non-empty. Maximum 100 characters. |

Optional keys:

| Key | Allowed values | Default |
| --- | --- | --- |
| `phase` | `request`, `response`, `both` | `both` |
| `scopes` | Comma-separated subset of `provider`, `global`, `api_key` | `provider, global, api_key` |
| `visibility` | `admin`, `user` | `admin` |

Do not repeat a key. Do not add other keys. The save is rejected on any violation.

## 3. The transform function

Define one global function `transform(ctx)`.
Monoize evaluates the full source and then calls `transform(ctx)` once per invocation.
No JavaScript value survives between invocations. Use `ctx.state` for per-request state.

The `ctx` object has exactly these properties:

| Property | Type | Meaning |
| --- | --- | --- |
| `phase` | `"request"` or `"response"` | Current pipeline phase. |
| `kind` | `"request"`, `"response"`, or `"stream"` | Payload surface kind. |
| `data` | object | JSON form of the URP v2 payload. Mutable. |
| `config` | object | The rule's `config` value from the transform chain. |
| `state` | object | Per-request mutable state. Persists across invocations. |
| `upstream_provider_type` | string or null | Upstream provider type, when known. |

## 4. Payload shapes

`ctx.data` is the exact JSON serialization of a URP v2 value:

- `kind = "request"`: a `UrpRequest` object. Key fields: `model`, `input` (array of nodes), `stream`, `temperature`, `top_p`, `max_output_tokens`, `reasoning`, `tools`, `tool_choice`, `stop`, `response_format`, `user`. Unknown fields pass through at the top level (`extra_body` is flattened).
- `kind = "response"`: a `UrpResponse` object. Key fields: `model`, `output` (array of nodes), `stop_reason`, `usage`.
- `kind = "stream"`: one canonical `UrpStreamEvent` object with an `event` field. The `event` values are `response_start`, `node_start`, `node_delta`, `node_done`, `response_done`, `provider_control`, and `error`.

## 5. Return contracts

For `kind = "request"` and `kind = "response"`:

1. Return `undefined`: the mutated `ctx.data` becomes the payload.
2. Return an object: the returned object becomes the payload.
3. Any other return value causes an apply error.

For `kind = "stream"`:

1. Return `undefined`: the mutated `ctx.data` is emitted as one event.
2. Return an object: the returned object is emitted as one event.
3. Return `null`: the current event is dropped.
4. Return an array of objects: each element is emitted as one event, in order.
5. Any other return value causes an apply error.

Every emitted object must deserialize as one canonical URP v2 value.
Keep the stream node lifecycle valid: `node_start` before `node_delta`, `node_delta` before `node_done`.

## 6. Per-request state

`ctx.state` starts as `{}` for each request.
Mutations to `ctx.state` persist between invocations of the same rule in the same request.
State persists across stream events. State is discarded when the request ends.

Example: count stream text deltas.

```js
function transform(ctx) {
  if (ctx.kind !== "stream") return;
  ctx.state.deltas = (ctx.state.deltas || 0) + 1;
}
```

## 7. Network bridge

Call `Monoize.fetch(url, options?)` for HTTP requests. The call blocks and returns
`{ status, headers, body }`.

- `options.method`: string. Default `"GET"`.
- `options.headers`: object with string values. Default `{}`.
- `options.body`: string. Default absent.
- `options.timeout_ms`: integer. Capped by the remaining invocation budget.
- `headers` in the result uses lowercase names. Repeated headers join with `", "`.
- HTTP error statuses do not throw. Inspect `status`.
- Network errors, timeouts, and non-UTF-8 bodies throw an exception.
- The response body is limited to 8 MiB by default.
- One invocation can make at most 16 fetch calls by default.

Example:

```js
function transform(ctx) {
  const res = Monoize.fetch("https://api.example.com/lookup", {
    method: "POST",
    headers: { "content-type": "application/json" },
    body: JSON.stringify({ model: ctx.data.model })
  });
  if (res.status === 200) {
    ctx.data.model = JSON.parse(res.body).model;
  }
}
```

## 8. Logging bridge

Use `console.log`, `console.info`, `console.warn`, and `console.error`.
Arguments are JSON-stringified, space-joined, and written to the server log.
The log line includes the transform id. No other `console` member exists.

## 9. Declarative config

Declare an optional global `configSchema` constant with a JSON Schema object.
The dashboard renders a config form from this schema in the transform chain editor.
The rule's saved config arrives as `ctx.config` at run time.
When you omit `configSchema`, the rule has an empty config object.

## 10. Security constraints

- The sandbox has no filesystem API. Do not attempt file access.
- The sandbox has no process, module loader, timer, or dynamic import API.
- Network access is only available through `Monoize.fetch`.
- Each invocation runs with a memory limit (default 32 MiB), a stack limit (default 1 MiB), and a wall-clock budget (default 10 s).
- A script throw, a limit violation, or an invalid return value fails the current request with a transform apply error. The proxy process continues.

## 11. Design guidelines

- Choose the narrowest `phase`. Declare `request` or `response` instead of `both` when one phase is sufficient.
- Choose the narrowest `scopes`. Omit `api_key` unless end users need the transform on their keys.
- Set `visibility: user` only when non-admin users may attach the transform to an API-key chain. The default `admin` rejects non-admin attachment.
- Keep `transform` fast. The wall-clock budget includes all fetch time.
- Validate `ctx.config` values before use. Fail fast with a thrown `Error` and a clear message.
- Do not store secrets in the script source. Administrators can read every script.
- For stream transforms, handle only the event types you target and return `undefined` for the rest.
- Test with a disabled rule first. A disabled or deleted custom transform is skipped as a no-op and never fails a request.
