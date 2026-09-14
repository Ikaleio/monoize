# v1.9.6 deployment and image response verification

- Commit: `400ce6cfabb38a662426c8647e7bb6a27f5c6ba6`.
- Release: https://github.com/Ikaleio/monoize/releases/tag/v1.9.6
- CI: https://github.com/Ikaleio/monoize/actions/runs/34711072545
- Server: `Ikaleio-TYO`, PM2 `monoize`, PID `735291`.
- Build, installed executable, and running executable SHA-256: `0a344b91512ae021cdbf1b5f599eeffdf919cafcad10637565298037a0cf1a28`.
- Local and public home endpoints: HTTP 200. Unauthenticated models endpoint: HTTP 401. Deployment rollback watchdog cancelled after verification.
- Existing Rust tests: 266 passed. Documentation production build passed in all four languages.

The API key named `Test` has one response-phase `image_resolve_urls` rule, scoped to `gpt-image-2.5-sunburst`, with a 30-second timeout and a 20 MiB download limit. Its prior transform list was empty. The targeted database update checked the prior value and incremented the configuration epoch in one transaction. The subsequent deployment restarted the service and cleared the authentication cache.

The original generation request included `quality: high` and `size: 1024x1536`, with no `response_format` override. It returned HTTP 200 in 23,279 ms. The sole image item contained `b64_json`, with no URL. The decoded PNG was 2,542,266 bytes and 1024 × 1536 pixels. Request ID: `caa3a1dd-9052-4b43-a1cd-0b1a6776d2b7`.

The response model was `gpt-image-2.5-sunburst`. The upstream omitted `quality`, so the downstream response omitted it as required by the metadata preservation behavior. OpenWebUI display was not directly tested.

The prior v1.9.5 metadata fix is included. Its release workflow completed successfully and npm latest was independently observed at 1.9.5 before the v1.9.6 publication finished.

The captured raw upstream response for the successful verification request still contained `data[0].url` and `bytes`. Its top-level fields were `account`, `created`, `data`, `model`, and `usage`. This confirms that Monoize performed the URL-to-base64 conversion and that `quality` was absent from the upstream response.

The v1.9.6 GitHub Release is public and its source-built deployment is verified. Its cross-platform release artifact workflow is still running; v1.9.6 npm publication has not yet been verified.
