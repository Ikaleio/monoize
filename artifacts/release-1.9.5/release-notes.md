Images API responses now preserve upstream-reported quality, size, background, output format, and image model. Previously, Monoize discarded these fields while decoding or assembling the response.

The fix covers non-streaming Images responses, completed image stream events, and Responses image-generation items. Metadata remains typed through URP conversion. Missing upstream values stay absent, and conflicting values across multiple images are omitted from the shared response envelope.

Base64 Images responses also retain JPEG and WebP MIME types when the upstream specifies the output format.

Validation: existing Rust tests and the documentation build. No new tests were added.
