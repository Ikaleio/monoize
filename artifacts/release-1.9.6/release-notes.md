Image clients that require base64 can now use `image_resolve_urls` in the response phase. Monoize downloads upstream image URLs and returns `data[].b64_json` through the Images API.

The transform supports non-streaming output and completed stream image nodes, reuses resolved images for terminal stream output, and enforces the download size limit while reading. Existing request-phase behavior remains supported. Configuration examples are available in all four documentation languages.

Validation: 266 existing Rust tests passed, all-target compilation passed, and the documentation production build passed.
