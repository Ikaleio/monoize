# Playground Specification

## 0. Status

- Product name: Monoize.
- Scope: ephemeral chatbot playground accessible at `/dashboard/playground`.
- This revision replaces the previous message-row + BYOK playground design entirely.

## 1. Purpose

The Playground is a session-only chatbot for the local Monoize instance. It supports
streamed chat completions, multimodal user attachments, image generation, and image
editing through the Monoize forwarding endpoints. It is a pure-frontend feature: the
conversation is never persisted to any backend store.

## 2. Session and Persistence Model

PG-STATE1. The conversation (all messages, attachments, and generated images) MUST live
only in browser memory (React state). The frontend MUST NOT send conversation history to
any Monoize dashboard endpoint and MUST NOT write conversation history to `localStorage`,
`sessionStorage`, IndexedDB, or cookies. A page reload starts an empty conversation.

PG-STATE2. Exactly these preference keys MAY be persisted in `localStorage`:

| Key | Type | Meaning |
|---|---|---|
| `playground_group` | string | Selected routing group label; empty/absent means "auto". |
| `playground_chat_model` | string | Selected chat model id. |
| `playground_image_model` | string | Selected image model id. |
| `playground_api_key_id` | string | Id of an explicitly user-selected API key; empty/absent means automatic key resolution. |
| `playground_temperature` | string | Decimal string; empty/absent means "omit from request". |
| `playground_max_tokens` | string | Integer string; empty/absent means "omit from request". |
| `playground_system_prompt` | string | System prompt text; empty means "no system message". |

PG-STATE3. On first mount the page MUST delete the legacy keys `playground_api_key` and
`playground_model` from `localStorage` so no previously pasted API-key secret remains
stored client-side.

## 3. Authentication (no BYOK)

PG-AUTH1. Dashboard data (API keys, groups, marketplace models, session user) MUST be
fetched with the authenticated dashboard session through the existing SWR hooks
(`useApiKeys`, `useDashboardGroups`, `useMarketplaceModels`, `useCurrentUser`).

PG-AUTH2. Forwarding requests (`/api/v1/chat/completions`, `/api/v1/images/generations`,
`/api/v1/images/edits`) MUST authenticate with `Authorization: Bearer <full key>` where
`<full key>` is the `key` field of one of the user's own API keys returned by
`GET /api/dashboard/tokens`.

PG-AUTH3. The page MUST NOT render any free-form secret input for API keys or upstream
provider keys, and MUST NOT store API-key secret values in `localStorage` (only the key
id per PG-STATE2).

PG-AUTH4. An API key is *eligible* iff `enabled == true` and (`expires_at` is absent or
in the future at resolution time).

PG-AUTH5. Key resolution is a pure function of (eligible keys `E` in list order,
persisted `playground_api_key_id` = `k`, selected group `g`, session user allowed
groups `U`):

1. If `g` is "auto":
   - if `k` identifies a member of `E`, resolve to that key;
   - otherwise resolve to `E[0]`, or to "none" when `E` is empty.
2. If `g` is a group label, define:
   - `C1` = keys in `E` with `allowed_groups == [g]`;
   - `C2` = keys in `E` with `g ∈ allowed_groups` and `|allowed_groups| > 1`;
   - `C3` = keys in `E` with `allowed_groups == []` and (`U == []` or `g ∈ U`);
   - if `k` identifies a member of `C1 ∪ C2 ∪ C3`, resolve to that key;
   - otherwise resolve to the first member of `C1`, else of `C2`, else of `C3`,
     else "none".

PG-AUTH6. When resolution yields "none" and `E` is empty, the empty-state hero MUST show
a single-action prompt that creates an API key via
`POST /api/dashboard/tokens` with body `{ "name": "Playground" }`, revalidates the
API-key SWR cache, and resolves to the created key. No other backend mutation is allowed.

PG-AUTH7. When resolution yields "none" and `E` is non-empty (no key covers the selected
group), the composer send action MUST be blocked and an inline hint MUST identify the
selected group as the cause.

PG-AUTH8. Routing-scope semantics MUST be documented in the key-picker UI as follows:
the effective routing groups of a request are derived from the API key and owning user
(`api-key-authentication.spec.md` §4). Selecting group `g` guarantees the request is
authorized for `g` and prefers the narrowest matching key (`C1` before `C2` before `C3`);
only a key with `allowed_groups == [g]` restricts routing to exactly `g`.

PG-AUTH9. A compact key picker (inside the composer settings popover) MUST list each
eligible key with name and `key_prefix`, plus an "auto" option. Picking a key writes
`playground_api_key_id`; picking "auto" clears it. The picker MUST NOT display full key
secrets.

## 4. Selectors

PG-SEL1. The composer MUST contain compact popover selectors (shadcn `Popover` +
`Command`) for: routing group, chat model, and — while image mode is active — image
model. Selectors MUST NOT be free-text-only inputs.

PG-SEL2. Group selector:

- Options are "auto" plus group labels from `GET /api/dashboard/groups`.
- If the session user's `allowed_groups` is non-empty, options MUST be restricted to
  that set (intersection with the suggestion list, preserving sorted order).
- Selection persists to `playground_group` (empty string for "auto").

PG-SEL3. Model selectors:

- The option list is `GET /api/dashboard/marketplace/models` (`useMarketplaceModels`).
- Each option row MUST render the model id with its provider icon (same icon resolution
  as `ModelBadge`).
- The selector MUST provide text search over model ids.
- When the search text is non-empty and does not exactly match an option, the list MUST
  include a "use custom id" entry that selects the typed text verbatim (the routable
  model set can exceed the metadata set).
- Chat model persists to `playground_chat_model`; image model persists to
  `playground_image_model`.

PG-SEL4. Image-model classification: a marketplace record is an *image model* iff its
`mode` contains the substring `image` (case-insensitive) or its lowercased `model_id`
contains at least one of:

`dall-e`, `dalle`, `gpt-image`, `flux`, `stable-diffusion`, `sdxl`, `sd3`, `imagen`,
`seedream`, `seededit`, `kolors`, `ideogram`, `recraft`, `cogview`, `qwen-image`,
`hunyuan-image`, `nano-banana`, `janus`, `hidream`.

The image-model selector MUST list image models in a group before all remaining models.
The chat-model selector lists all models unsegmented.

PG-SEL5. While any of the backing SWR hooks (`useApiKeys`, `useDashboardGroups`,
`useMarketplaceModels`) is loading with no cached data, the corresponding selector
trigger MUST render as a skeleton pill instead of an interactive control.

## 5. Chat Execution (AI SDK)

PG-CHAT1. Chat state MUST be managed by `useChat` from `@ai-sdk/react` with a custom
`ChatTransport` implementation (`MonoizeChatTransport`). The transport MUST NOT be the
default HTTP transport.

PG-CHAT2. `MonoizeChatTransport.sendMessages` MUST:

1. Read the current chat model id, resolved API key, system prompt, temperature, and
   max-tokens values at call time (latest selector state applies to every request,
   including regenerations).
2. Reject with an error carrying a translatable reason when the model is empty or the
   resolved key is "none".
3. Build an OpenAI-compatible provider via `createOpenAICompatible` from
   `@ai-sdk/openai-compatible` with `baseURL = <origin>/api/v1` and
   `apiKey = <full key value>`, so the upstream call is
   `POST /api/v1/chat/completions` with `stream: true` against the local Monoize
   instance.
4. Convert UI messages with `convertToModelMessages` after applying PG-CHAT3
   sanitation.
5. Call `streamText` with: the converted messages; `system` set iff the stored system
   prompt is non-empty; `temperature` set iff `playground_temperature` parses as a
   finite number; `maxOutputTokens` set iff `playground_max_tokens` parses as a positive
   integer; and the abort signal from the chat.
6. Return `toUIMessageStream(...)` of the resulting stream with an `onError` mapper
   that maps the failure to human-readable text (upstream error text must reach the
   UI): an `Error` maps to its `message`; a non-Error object maps to its string
   `message` field, else its nested `error.message` string field, else its JSON
   serialization; any other value maps to `String(value)`.

PG-CHAT3. Outgoing-message sanitation: `file` parts of **assistant** messages MUST be
excluded from the converted model messages. If exclusion leaves an assistant message
with no parts, one text part with literal content `[image]` MUST be substituted.
User-message `file` parts MUST be preserved (they encode user image attachments).

PG-CHAT4. Send/stop contract: while `status` is `submitted` or `streaming`, the primary
composer action MUST be a stop control invoking `stop()`. Stopping keeps all partial
assistant output as a normal message and MUST NOT surface an error.

PG-CHAT5. When `useChat` reports an `error`, an inline dismissible banner MUST appear
between the message list and the composer showing the error message, with a retry action
that calls `regenerate()` and a dismiss action that calls `clearError()`. No toast is
shown for chat request errors.

PG-CHAT6. User attachments: the composer accepts image files (`image/*`). In chat mode,
send MUST call `sendMessage({ text, files })` so attachments become user-message `file`
parts (data URLs) that convert to image parts for the upstream request.

## 6. Message Operations

PG-MSG1. Every message exposes hover/focus actions. Minimum set: copy (all roles with
text), edit (user and assistant), delete (all roles). Assistant messages additionally
expose regenerate, and each assistant image exposes download and edit-image actions.
On coarse-pointer devices the actions MUST be reachable without hover (always visible)
with touch targets per `frontend-design-system.spec.md` DS49.

PG-MSG2. Edit is inline: the message body is replaced by a textarea initialized with the
concatenated text parts, with confirm and cancel actions. Preconditions: `status` is
`ready` or `error`.

PG-MSG3. Confirming a **user** message edit MUST call
`sendMessage({ text: <edited>, messageId })`, which replaces that message, removes all
later messages, and requests a new assistant response. Attachments of the edited message
are not preserved (the edited message consists of the edited text only).

PG-MSG4. Confirming an **assistant** message edit MUST replace the message's text parts
with a single text part containing the edited text via `setMessages`, in place, without
issuing any request.

PG-MSG5. Delete MUST remove exactly the targeted message via `setMessages` filtering,
without issuing any request. The optimistic update is the operation itself (client-only
state); no rollback path exists.

PG-MSG6. Regenerate on an assistant message MUST call `regenerate({ messageId })`,
which removes that assistant message and everything after it, then requests a new
response using the current selector state.

## 7. Image Generation and Editing

PG-IMG1. The composer has a chat/image mode toggle. Mode is session state (not
persisted). While image mode is active the image-model selector is visible and the send
action executes an image request instead of a chat request.

PG-IMG2. Image send with no attachment MUST call
`POST /api/v1/images/generations` with JSON body
`{ "model": <image model>, "prompt": <composer text>, "n": 1 }`.

PG-IMG3. Image send with at least one attachment MUST call
`POST /api/v1/images/edits` as `multipart/form-data` with fields `model`, `prompt`,
`n = 1`, and `image` = the first attachment file. Additional attachments beyond the
first are ignored for the upstream call (the endpoint accepts a single source image).

PG-IMG4. On image send the frontend MUST synchronously append a user message (prompt
text plus attachment file parts) to the chat state, and render a pending assistant
placeholder with an animated loading treatment until the request settles.

PG-IMG5. On success, the placeholder MUST be replaced by an assistant message whose
parts are, in order: one text part with `revised_prompt` when present, then one `file`
part per `data[]` entry — `url` used verbatim when present, otherwise
`data:image/png;base64,<b64_json>`.

PG-IMG6. On failure, the placeholder MUST be replaced by an inline error state with a
retry action that re-issues the same request. The user message remains in the
conversation.

PG-IMG7. Image requests MUST be abortable through the same stop control (an
`AbortController` scoped to the in-flight image request). Aborting removes the pending
placeholder and keeps the user message; no error is shown.

PG-IMG8. The edit-image action on a generated (or attached) image MUST switch the
composer to image mode and stage that image as the composer attachment, so the next send
follows PG-IMG3. If fetching the image bytes for staging fails, an error toast is shown
and the composer state is unchanged.

PG-IMG9. Generated images participate in later chat requests only through PG-CHAT3
(assistant file parts are stripped); the image bytes are never re-uploaded in chat mode.

## 8. Composer

PG-CMP1. The composer is a single bordered surface containing, top to bottom: the
attachment preview row (when attachments exist), the auto-growing textarea (1 to 8 lines),
and a control row with the selectors (PG-SEL1), the attach action, the mode toggle, the
settings popover trigger, and the send/stop action.

PG-CMP2. Enter submits and Shift+Enter inserts a newline on fine-pointer devices. On
coarse-pointer devices Enter inserts a newline and only the send button submits.

PG-CMP3. Send is enabled iff: a model for the active mode is selected, key resolution
does not yield "none", `status` is `ready`/`error`, no image request is pending, and the
trimmed text is non-empty (chat mode also allows empty text with ≥ 1 attachment).

PG-CMP4. The settings popover contains: system prompt (multiline), temperature (number,
range 0–2, step 0.1, clearable), max tokens (positive integer, clearable), and the key
picker (PG-AUTH9). Each field persists per PG-STATE2 on change.

PG-CMP5. A "new chat" action MUST be visible whenever the conversation is non-empty; it
clears the chat state, any pending image job, and composer attachments. It MUST NOT
clear persisted preferences.

## 9. Layout

PG-L1. The page renders inside the standard dashboard shell (sidebar navigation entry
retained). The playground content root MUST be a full-height flex column sized so the
page itself never scrolls: height `calc(100dvh - 5.5rem)` below `lg` and
`calc(100dvh - 3rem)` at `lg` and above (the dashboard main pane paddings).

PG-L2. Empty conversation renders a hero: centered greeting text (display font) with a
one-line muted hint stating that the chat is ephemeral, and the composer centered
beneath it, with no card wrapper. Non-empty conversation renders the scrollable message
list (the only scroll container) with the composer docked at the bottom and a "new
chat" action above the list (PG-CMP5). Both states share one composer element.

PG-L3. Message column max width MUST be `48rem` (`max-w-3xl`) centered. User messages
render as right-aligned bubbles on the `muted` surface token with `rounded-2xl` corners;
assistant messages render full-width on the page surface without a bubble. No purple or
violet styling is introduced; all colors come from existing theme tokens.

PG-L4. While streaming or waiting, the list MUST follow the newest content
(auto-scroll), and auto-scroll MUST pause when the user has scrolled up more than
`80px` from the bottom, resuming when they return to the bottom.

## 10. Rendering

PG-RD1. Assistant text parts MUST render through the `streamdown` package's
`Streamdown` component (streaming-safe markdown with incomplete-block handling). User
text parts render as plain text preserving whitespace.

PG-RD2. Assistant reasoning parts MUST render as a collapsed, expandable muted section
labeled through i18n, separate from the answer text.

PG-RD3. `file` parts with an `image` media type render as rounded images constrained to
the message column (max height `24rem`), with the PG-MSG1 image actions.

## 11. Motion

PG-MO1. All animations use `framer-motion` with the shared spring presets from
`components/ui/motion.tsx`; reduced-motion behavior follows
`frontend-design-system.spec.md` DS32–DS34 (no x/y/scale animation when reduced motion
is on).

PG-MO2. Message entry animates opacity `0 → 1`, y `12px → 0`, scale `0.98 → 1` with a
spring (stiffness 300–500, damping 24–35). Message removal animates opacity `1 → 0` and
scale `1 → 0.96` inside `AnimatePresence` with `mode="popLayout"`, and surviving
siblings reflow via `layout` animation.

PG-MO3. The composer is a `layout`-animated element shared between the hero and docked
positions (PG-L2); the hero-to-docked transition MUST animate with a spring rather than
jumping.

PG-MO4. The chat/image mode toggle MUST animate its active indicator with a shared
`layoutId` spring. The send/stop icon swap animates scale/opacity.

PG-MO5. The pending assistant state renders an animated indicator (pulsing dot or
shimmer). All indicator animation must be opacity-only under reduced motion.

## 12. Internationalization

PG-I18N1. All user-visible copy uses i18n keys under the `playground` namespace, present
in `en.json`, `zh.json`, `zh-TW.json`, and `ja.json`.

## 13. Constraints

PG-C1. The Playground performs no backend mutation except PG-AUTH6 key creation.

PG-C2. The Playground MUST NOT implement its own SSE parser for chat; streaming is
handled by the AI SDK provider/`streamText` pipeline (PG-CHAT2).

PG-C3. The page MUST be split into multiple components under
`frontend/src/components/playground/`; the route file composes them.
