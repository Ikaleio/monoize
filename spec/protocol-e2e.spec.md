# Protocol End-to-End Test Specification

## 1. Scope

E2E-1. `bun e2e/cli.ts list` MUST list six harnesses: `codex-responses-ws-v2`,
`claude-messages`, `opencode-responses`, `opencode-chat`, `pi-responses`, and `pi-chat`.
`run` MUST execute real clients against an isolated Monoize and one real upstream.
Supported upstream types are `responses`, `chat_completion`, and `messages`.
HTTP Codex and WebSocket v1 are excluded.

E2E-2. The chain MUST be client, downstream recorder, Monoize, upstream recorder,
and the configured endpoint. Recorders MUST NOT repair or translate protocol bodies.
The upstream recorder MUST replace only the destination URL and transport headers.
It MUST preserve the configured endpoint path and query. Redirects MUST NOT be followed.

## 2. CLI

E2E-3. `--harness` accepts repeated comma-separated selections, deduplicated in order.
The default is `all`. Unknown selections and options MUST fail before container creation.
`--base-url`, `--api-key`, `--model`, and `--upstream-type` override `BASE_URL`,
`API_KEY`, `MODEL`, and `UPSTREAM_TYPE`. Empty required values MUST fail.
The URL MUST be absolute HTTP(S), without embedded credentials or a fragment.
Known suffixes `/responses`, `/chat/completions`, and `/messages` infer the type.
Unknown suffixes require an explicit type. A known suffix conflicting with that type MUST fail.

E2E-4. `--jobs` defaults to 1. Positive values enable bounded parallel execution.
Each item has `--timeout` seconds, default 1800, from item initialization to completion.
`--max-requests`, default 100, counts downstream generated requests, including retries.
WebSocket `generate=false` does not count. Exceeding either limit MUST fail the item.
A failed item MUST NOT prevent queued items. The runner MUST NOT rerun an item.
After recording a generation HTTP error or upstream terminal error event, stop the item
in the next status poll. Do not spend further client retries on an already failed item.

E2E-5. `--task` and `--task-file` are mutually exclusive. Absence selects the built-in
architecture task. `--repo` defaults to `https://github.com/Ikaleio/Monoize`.
`--ref` defaults to `HEAD`. Resolve a branch or tag to a commit once before execution;
an explicit 40-character commit is used directly. Every item MUST use that commit.
`--monoize-image` selects an existing image; otherwise build current workspace source.
`--output` MUST be inside the project and MUST identify a new directory.
The default is `e2e/.runs/<timestamp>-<random-id>`.

E2E-6. Exit codes are 0 for all items passing, 1 for item failures, 2 for invalid
arguments or common setup failure, and 130 for interruption. Partial results MUST be
written after each item and on interruption. Build time is outside item timeouts.

## 3. Isolation and initialization

E2E-7. Each item MUST have independent containers, network, SQLite database, credentials,
and working directory. No client container MAY mount host credentials, the host project,
or the Docker socket. Clients MUST run as non-root users with noninteractive tool execution.
Only temporary Monoize credentials MAY be given to clients. Host orchestration uses Docker CLI.
All persistent local artifacts MUST be inside the project. Cleanup MUST remove item containers,
networks, secret files, and volumes on completion, failure, or interruption.

E2E-8. A dedicated Dockerfile MUST build current source, including uncommitted source
changes, with the release profile. The production Dockerfile MUST remain unchanged.
Base image digests, CLI versions, and SDK versions MUST be pinned. The report MUST record
source commit, working-tree state digest, image IDs/digests, and installed client versions.
After image resolution, containers MUST use immutable image IDs throughout the run.
Build contexts MUST exclude credentials, databases, dependencies, and generated outputs.
Builds MUST forward configured HTTP_PROXY, HTTPS_PROXY, and NO_PROXY environment values
as Docker build arguments. Lowercase aliases are accepted. For loopback proxy hosts, builds MUST use the corresponding daemon proxy when configured,
otherwise map the host to host.docker.internal. These settings MUST NOT become runtime image settings.

E2E-9. Initialize the empty database with Monoize migrations, stop Monoize, disable CAPTCHA,
automatic price sync, and active probes in that database, and restart it. Register the first
administrator through the Dashboard API. Through that API, set unlimited test balance,
zero test model prices, one Provider, one Channel, and one capture-enabled API key.
Disable Provider retries, Channel retries, active probes, and additional routing.
Use `e2e-model` as the only downstream model and redirect it to the configured model.
Constrain auxiliary model settings to this alias and disable optional background client work.
Claude MUST disable optional adaptive thinking and set its thinking budget to zero.
OpenCode Responses MUST use stateless requests (`store=false`). Disable its title and summary agents.

## 4. Evidence and validation

E2E-10. Record both HTTP directions, SSE events, WebSocket upgrades and text messages,
client structured output, Monoize logs and request captures, and task results.
Decode `.json.zst` captures before validation. Non-generation auxiliary HTTP errors remain
in evidence but MUST NOT count as model generation errors.
Correlation MUST include recorder request IDs, response IDs, tool IDs, and available
Monoize request IDs. Missing required evidence MUST fail validation.
Sensitive header values, known credentials, and URL query values MUST be redacted before
host persistence. A shared per-item 128 MiB evidence budget MUST fail closed on exhaustion.
Capture parsing and individual messages MUST also be bounded by that budget.

E2E-11. Protocol success requires the selected downstream transport, configured upstream
shape and model, successful stream termination, and a complete tool cycle. Match an emitted
tool ID to a successful local execution, subsequent request tool result, and subsequent
successful generation. A WebSocket v2 item additionally requires the v2 beta header and
an actual `previous_response_id` continuation containing a tool result. Any HTTP Responses
generation in that item MUST fail it. Warmups MUST NOT satisfy generation or tool coverage.
Errors, unfinished streams, unmatched tools, unexpected models, and capture truncation MUST fail.

E2E-12. The built-in task MUST ask the client to clone into `/work/repo`, check out the
resolved commit, read at least three source files, and write `/work/architecture.md`.
The document MUST contain `Entry points`, `Modules`, `Request flow`, and `Sources` headings.
Sources MUST contain at least three backtick-quoted repository-relative source file paths.
Task success requires matching origin and commit, successful clone tool evidence, evidence
of reading the cited source files, a nonempty document with those headings and existing paths,
and normal client exit. Path checks MUST reject traversal and paths escaping the checkout.
These checks do not establish semantic correctness of the architecture explanation.

E2E-13. Custom tasks MUST require normal exit and a nonempty final response. Their report
MUST mark built-in repository checks as not applicable. They MUST still satisfy E2E-11.
Failures MUST distinguish setup, timeout, request limit, evidence limit, upstream error,
protocol mismatch, tool failure, task failure, and interruption.

E2E-14. Write a terminal summary and versioned `report.json` with separate protocol and task
results. A test item passes iff both pass and no infrastructure/evidence failure exists.
Do not report an unexecuted real-upstream combination as verified.

## 5. Verification and documentation

E2E-15. Offline tests MUST cover CLI precedence, endpoint preservation, fragmented SSE,
WebSocket continuation and fallback, tool correlation, errors, incomplete streams, limits,
redaction, scheduling, and cleanup. Controlled fixtures MUST NOT count as real-client coverage.
Real acceptance requires all six clients across three compatible HTTP upstream types.
Unavailable credentials or infrastructure MUST be reported as unverified, not passing.

E2E-16. Document usage and acceptance limits in all four documentation locales. This feature
MUST NOT deploy production services, schedule paid runs, or change production URL semantics.
