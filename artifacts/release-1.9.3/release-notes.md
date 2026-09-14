## Protocol and media fixes

- Use typed URP fields for protocol features, tool namespaces, signatures, and media metadata across Chat Completions, Responses, Messages, and Gemini.
- Preserve compatible tool-result images and files, single-object content, repeated media, and mixed-content order through decoding and streaming.
- Keep MIME, filenames, image detail, document metadata, and citations. Distinguish inline Base64, public URLs, and provider-bound file references.
- Bind private file references to their Provider, Channel, and credential scope. Reject incompatible routes instead of inventing transferable file IDs.
- Preserve signed tool-call bindings and envelope-control targets through live streams and client history replay.
- Return explicit errors for unsupported media. Prevent duplicate terminal errors, incorrect successful completion, and retention of rejected response history.

## Validation

`cargo test --locked --all-targets`: 266 tests passed. The suite covers request stream flags, non-stream responses, live SSE conversion, synthetic streams, and production request preparation.

These checks use offline protocol fixtures. They do not establish acceptance by every upstream Provider or validate file extraction quality.
