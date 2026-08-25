# Dashboard Authentication CAPTCHA Specification

## 0. Scope

This specification defines Cap challenge generation, configuration, and verification for the public dashboard login and registration endpoints.

## 1. Configuration

CAP-C1. `captcha_enabled` MUST be stored in `system_settings` as the exact text `true` or `false`. A missing row MUST resolve to `true`. The authenticated system settings API MUST return and update this value.

CAP-C2. If `captcha_enabled` is `true` and both `MONOIZE_CAP_API_ENDPOINT` and `MONOIZE_CAP_SECRET_KEY` are absent or empty, Monoize MUST use its built-in Cap service. The browser endpoint MUST be `/api/dashboard/captcha/`. This mode MUST require no operator-supplied secret, site key, sidecar process, or external network request.

CAP-C3. If both external Cap variables are non-empty, Monoize MUST use external mode. `MONOIZE_CAP_API_ENDPOINT` MUST contain the public Cap site endpoint, including the site key path. It MUST be an absolute `http` or `https` URL with a host. Monoize MUST normalize its path to end with `/`. It MUST NOT contain credentials, a query string, or a fragment. `MONOIZE_CAP_SECRET_KEY` MUST contain the corresponding site secret and MUST NOT be returned through an API or log.

CAP-C4. If exactly one external Cap variable is non-empty, or the endpoint is invalid, startup MUST fail with code `cap_config_invalid`.

CAP-C5. `GET /api/dashboard/settings/public` MUST return `captcha_enabled`. When it is `true`, `cap_api_endpoint` MUST equal `/api/dashboard/captcha/` in built-in mode and the normalized external endpoint in external mode. When it is `false`, `cap_api_endpoint` MUST be `null`.

CAP-C6. Updating `captcha_enabled` through the authenticated system settings API MUST NOT create, rotate, or delete any dashboard session. The session that authorizes the update MUST remain valid after the update succeeds.

## 2. Authentication request contract

CAP-A1. `POST /api/dashboard/auth/login` and `POST /api/dashboard/auth/register` MUST accept an optional `captcha_token: string` in the JSON request body.

CAP-A2. Each authentication request MUST read only the `captcha_enabled` settings row before CAPTCHA and account processing. A missing row MUST resolve to `true`. A malformed row or database error MUST return HTTP `500` without querying or mutating user or session state.

CAP-A3. If `captcha_enabled` is `true`, a missing token, an empty token after trimming, or a token longer than 4096 bytes MUST return HTTP `400` with code `captcha_required`. The handler MUST NOT query or mutate user or session state after this rejection.

CAP-A4. If `captcha_enabled` is `false`, the handler MUST ignore `captcha_token` and continue without a Cap request. Monoize MUST NOT derive an authentication admission decision from the client IP address.

## 3. Server verification

CAP-V1. In external mode, Monoize MUST send one JSON request to `<cap_api_endpoint>siteverify` after CAP-A2 and CAP-A3 and before username, password, account, registration-state, or session processing:

```json
{
  "secret": "<MONOIZE_CAP_SECRET_KEY>",
  "response": "<captcha_token>"
}
```

Monoize MUST NOT include a client IP field or a forwarded client IP header in this request.

CAP-V2. The verification request timeout MUST be 5 seconds. Its response body MUST be limited to 4096 bytes. Monoize MUST NOT follow redirects from the verification endpoint.

CAP-V3. A `2xx` verification response with JSON field `success: true` MUST authorize the handler to continue. A `2xx` response without boolean `success: true` MUST return HTTP `400` with code `captcha_invalid`.

CAP-V4. An external Cap transport error, timeout, non-`2xx` response, over-limit response body, or invalid response JSON MUST return HTTP `503` with code `captcha_unavailable`.

CAP-V5. The handler MUST verify each request token exactly once. It MUST NOT retry verification. Cap tokens are single-use; after any post-verification authentication error, the client MUST obtain a new token before another submission.

## 4. Built-in Cap service

CAP-B1. `POST /api/dashboard/captcha/challenge` MUST be available only when `captcha_enabled` is `true` and built-in mode is active. It MUST return a Cap format-1 challenge with `c = 50`, `s = 32`, `d = 3`, an opaque random token with at least 128 bits of entropy, and an expiry 10 minutes after issuance.

CAP-B2. Built-in challenge salts and targets MUST use the Cap format-1 FNV-1a and xorshift derivation from the opaque challenge token. `POST /api/dashboard/captcha/redeem` MUST accept `{ token, solutions }`, require exactly 50 non-negative integer solutions, and verify each SHA-256 proof against its derived hexadecimal prefix.

CAP-B3. A challenge token MUST be redeemed at most once. The server MUST remove it atomically only after every proof is valid and before returning success. An expired, unknown, or concurrently redeemed challenge MUST return JSON `success: false`.

CAP-B4. A successful redemption MUST return a random authentication token and an expiry 20 minutes after redemption. The server MUST store only a SHA-256 lookup key for that token. Authentication MUST consume the lookup key atomically. An expired, unknown, or reused authentication token MUST return `captcha_invalid`.

CAP-B5. The built-in challenge store and authentication-token store MUST each contain at most 10,000 entries. Before rejecting an insertion at capacity, the server MUST remove expired entries. A capacity rejection MUST return HTTP `503` and MUST NOT evict an unexpired entry.

CAP-B6. Built-in state is process-local. A process restart MAY invalidate unredeemed challenges and unconsumed authentication tokens. Built-in endpoints MUST return HTTP `404` when external mode is active and HTTP `403` when `captcha_enabled` is `false`.

## 5. Dashboard client

CAP-U1. The login page MUST load the Cap widget from the pinned frontend package and MUST set `data-cap-api-endpoint` to the public `cap_api_endpoint` value.

CAP-U2. The frontend MUST bundle the pinned Cap WASM solver and its pako decompression fallback as same-origin build assets. The widget MUST use those assets through `CAP_CUSTOM_WASM_URL` and `CAP_PAKO_URL`; it MUST NOT depend on a runtime CDN request for either asset.

CAP-U3. While public settings are loading, the login form MUST show a skeleton in the widget position and MUST disable submission. If `captcha_enabled` is `false`, the page MUST omit the widget and allow submission without a token. If `captcha_enabled` is `true` and `cap_api_endpoint` is `null`, the page MUST show a configuration error and disable submission.

CAP-U4. When `captcha_enabled` is `true`, the client MUST enable submission only after the widget emits a non-empty token and MUST send that token as `captcha_token`. When it is `false`, the client MUST send an empty token and MUST NOT gate submission on a token.

CAP-U5. After a failed login or registration request, the client MUST clear the stored token and reset the widget. Switching between login and registration MUST also clear the token and reset the widget.

CAP-U6. Widget solve and widget error messages MUST use the active dashboard locale. Supported dashboard locales MUST remain English, Simplified Chinese, Traditional Chinese, and Japanese.

CAP-U7. The system settings page MUST expose `captcha_enabled` as a switch in the session and security section. Its default state MUST be enabled. Its description MUST state that disabling it removes bot and credential-stuffing protection from dashboard login and registration.

CAP-U8. The rendered widget control MUST occupy 100% of the login form width and match the submit button width. It MUST have a height of 48 CSS pixels and an 8 CSS pixel border radius. Its background, border, text, checkbox, spinner, and focus colors MUST derive from the dashboard semantic color variables. Changing the dashboard between light and dark themes MUST update these colors without reloading or recreating the widget.

## 6. Content Security Policy

CAP-S1. The response Content Security Policy MUST allow `connect-src` only from `'self'` and the external Cap endpoint origin when external mode is configured. Built-in mode MUST keep `connect-src 'self'`.

CAP-S2. The policy MUST allow Cap solver workers from same-origin URLs and `blob:` URLs.

CAP-S3. Each HTTP response MUST receive a fresh script nonce. Embedded SPA entry responses MUST expose that nonce through a non-script metadata element. The dashboard client MUST set `CAP_SCRIPT_NONCE` to that value before loading the Cap widget. The policy MUST NOT add `'unsafe-inline'` to `script-src`.

CAP-S4. The policy MUST include `'wasm-unsafe-eval'` in `script-src` so the Cap solver can compile its bundled WASM module. The policy MUST NOT include `'unsafe-eval'`.
