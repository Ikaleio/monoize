# Differential Smoke Tool Specification

## 1. Command and inputs

DS1. Run the maintained tool with Bun from any working directory:

```sh
bun run scripts/differential-smoke/run.ts --base <bin> --new <bin> [--only <regex>] [--out <directory>] [--rebootstrap] [--expected <json>]
```

Resolve relative binary, output, and expected-file paths against the caller's working directory.
Resolve repository assets from the tool's own location, not a machine-specific path.
Reject missing `--base`/`--new`, unknown arguments, invalid regular expressions, and an empty selected case set with exit code `2` before starting servers.
A binary that cannot be spawned is a setup failure (DS11).

DS2. Store the seed database at `artifacts/smoke/seed/monoize.db` and credentials at adjacent `meta.json`.
Bootstrap missing seed files with the base binary.
`--rebootstrap` MUST replace that seed directory and rebuild it with the base binary.
Create one administrator, one forwarding API key, and three mock channels.
Each channel MUST expose plain and reasoning model names for one upstream protocol.
Disable CAPTCHA, automatic price synchronization, and active probes in the seed.
Allow unpriced requests and missing usage so protocol errors remain observable.
Never print the seed API key or administrator password.

DS3. Copy the same stopped, checkpointed seed into separate base and new run databases.
Change only their channel base URLs to the current loopback mock address.
Each binary MUST use a separate work directory, spool directory, cache directory, and port.
Do not build either binary or read the concurrently edited Rust implementation.

DS4. By default, store run outputs in a new directory below ignored `artifacts/smoke/out/`
whose name is an ISO timestamp with colons and periods replaced by hyphens.
`--out` MUST select the run directory instead.

DS5. `--only` MUST select case names by regular expression.
`--expected` MUST load a JSON array of `{ "case": <regex>, "where": <regex>, "note"?: <string> }` objects.
The default file MUST be `scripts/differential-smoke/expected-diffs.json`.
Reject missing or malformed expected files, entries without string `case` and `where`, and invalid regular expressions with exit code `2`.
A matching case-and-path pair MUST mark that difference as expected without hiding it.

## 2. Case matrix

DS6. Without `--only`, execute exactly 45 cases:

| Cases                   | Count | Requests                                                                                                |
| ----------------------- | ----: | ------------------------------------------------------------------------------------------------------- |
| Protocol matrix         |    36 | Three downstream protocols × three upstream protocols × plain or reasoning model × stream or non-stream |
| Tool loops              |     3 | Each downstream protocol over Chat upstream; weather call, websearch call, final answer                 |
| Responses multi-turn    |     1 | Two streamed reasoning turns; replay completed output items into turn two                               |
| Chat multi-turn         |     1 | Two non-stream turns over Messages upstream; replay the first assistant message                         |
| Messages multi-turn     |     1 | Two non-stream turns over reasoning Chat upstream; replay first response content                        |
| Messages token counting |     3 | `/v1/messages/count_tokens` over each upstream protocol                                                 |

The downstream protocols are Responses, Chat Completions, and Messages.
The upstream protocols are `responses`, `chat_completion`, and `messages`.
Use the repository mock defined by `mock-upstream.spec.md`.
Run each case against both binaries with identical request input.
Keep responses from all tool-loop and multi-turn steps.
Capture HTTP status, content type, parsed body or ordered SSE frames, and transport errors.
Use a 90-second request timeout.

## 3. Normalization and comparison

DS7. Preserve raw captures before normalization.
For each case and side, replace numeric values under these keys with `<ts>`:
`created`, `created_at`, `completed_at`, `started_at`, and `duration`.
Normalize generated response, message, chat, reasoning, function-call, tool-call, request, and UUID identifiers.
Assign placeholders in first-seen order per identifier prefix across all steps in that case.
Normalize repeated occurrences consistently, including identifiers inside strings.
Keep `rs_mock_1`, `msg_mock_1`, `call_weather_1`, and `call_websearch_1` unchanged.

DS8. Parse SSE `event`, `id`, and multiline `data` fields.
Ignore comment-only frames, but preserve their count as `droppedComments` for comparison.
Preserve non-JSON data, including `[DONE]`.
Do not normalize text, usage, status codes, stop reasons, frame boundaries, or object key order.

DS9. Align SSE frames by longest common subsequence of structural event signatures.
The signature MUST include the event or payload type and available delta, finish, item, and block types.
Compare aligned frames recursively.
Report added or removed frames separately.
Compare arrays in order and report changes to common object-key order.
Each difference MUST name its step, frame indexes or body location, and field path.

## 4. Reports and exit status

DS10. Write `run.json`, `report.json`, and `report.md` inside the run directory.
Write side-specific `raw/`, `normalized/`, `db/`, and `work/` directories.
Write mock logs under `logs/` and binary logs under each side's `work/` directory.
The run metadata MUST identify binaries, seed, ports, and child process IDs without credentials.
The case report MUST separate comparison results from protocol outcomes.
Report per-step HTTP status, success or error reason, and captured usage for each side.
A successful SSE outcome MUST contain its protocol terminal and no error frame.
HTTP 200 alone MUST NOT classify an SSE stream as successful.
Report the full usage objects in JSON; Markdown MAY shorten their rendering.

DS11. Exit `0` when every observed difference matches DS5 or no differences exist.
Equivalent error responses MAY therefore produce exit `0`; the outcome columns MUST expose those errors.
Exit `1` when at least one difference is unexpected.
Exit `2` for invalid input, setup failure, or harness failure.

## 5. Child environment and cleanup

DS12. Remove uppercase and lowercase HTTP, HTTPS, and ALL proxy variables from the
mock's and both binaries' environments.
Remove `NODE_USE_ENV_PROXY`; set `NO_PROXY` and `no_proxy` to `*`.
The tool does not sandbox network access; the seed's channels point only at the loopback mock (DS2, DS3).

DS13. Stop and await all child processes after success, differences, or setup failure.
On SIGINT or SIGTERM, stop children before exiting with `128 + signal number`.
Send SIGTERM first, then SIGKILL after five seconds when needed.
Close parent log descriptors after spawning children.
Do not leave the bootstrap server, mock server, or either binary running.
