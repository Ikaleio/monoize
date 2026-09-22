# Playground Specification

## 0. Status

- Product name: Monoize.
- Scope: ephemeral chatbot playground accessible at `/dashboard/playground`.
- The Playground supports an internal session credential and explicit user API keys.

## 1. Purpose

The Playground is a session-only chatbot for the local Monoize instance. It supports
streamed chat through the OpenAI Responses API, reasoning display, multimodal user
attachments, image generation, and image editing through the Monoize forwarding
endpoints. Conversation state is a frontend-only feature and is never persisted to any
backend store. Forwarding authentication is supplied by either the authenticated
dashboard session or a user-selected API key.

## 2. Session and Persistence Model

PG-STATE1. The conversation (all messages, attachments, and generated images) MUST live
only in browser memory (React state). The frontend MUST NOT send conversation history to
any Monoize dashboard endpoint and MUST NOT write conversation history to `localStorage`,
`sessionStorage`, IndexedDB, or cookies. A page reload starts an empty conversation.

PG-STATE2. Exactly these preference keys MAY be persisted in `localStorage`:

| Key | Type | Meaning |
|---|---|---|
| `playground_group` | string | Selected routing group id; empty/absent means "auto". |
| `playground_chat_model` | string | Selected chat model id. |
| `playground_image_model` | string | Selected image model id. |
| `playground_image_size` | string | Selected image size as `<width>x<height>`. Empty/absent means `auto`. |
| `playground_image_quality` | string | Selected image quality: `default`, `low`, `medium`, `high`, `xhigh`, or `max`. Empty/absent means `default`. |
| `playground_api_key_id` | string | Explicitly selected API-key id; empty/absent means the built-in Playground credential. |
| `playground_temperature` | string | Decimal string; empty/absent means "omit from request". |
| `playground_max_tokens` | string | Integer string; empty/absent means "omit from request". |
| `playground_system_prompt` | string | System prompt text; empty means "no system message". |

PG-STATE3. On first mount the page MUST delete the legacy keys `playground_api_key` and
`playground_model` from `localStorage`. The Playground MUST NOT persist a full API-key
secret. Persisting the selected API-key id per PG-STATE2 is permitted.

## 3. Authentication

PG-AUTH1. Dashboard API keys, groups, marketplace models, and the session user MUST be
fetched with the authenticated dashboard session through `useApiKeys`,
`useDashboardGroups`, `useMarketplaceModels`, and `useCurrentUser`. The Playground MUST
NOT create or mutate API keys.

PG-AUTH2. Credential selection has exactly two modes:

1. If `playground_api_key_id` is empty, the effective credential is the built-in
   Playground credential.
2. If `playground_api_key_id = k`, the effective credential is the user's API key whose
   id equals `k`. The frontend MUST NOT replace `k` with another API key automatically.
3. If `k` does not identify a time-eligible API key after the API-key response loads, the
   frontend MUST clear `playground_api_key_id` and return to the built-in credential.

PG-AUTH3. A built-in forwarding request (`/api/v1/chat/completions`,
`/api/v1/images/generations`, or `/api/v1/images/edits`) MUST omit `Authorization` and
`x-api-key`, include browser credentials, and send
`x-monoize-internal-source: playground`. When a non-auto routing group is selected, the
request MUST also send `x-monoize-playground-group: <group-id>`.

PG-AUTH4. The forwarding server MUST accept `x-monoize-internal-source: playground` only
when no API-key credential is present and the `monoize_session` HttpOnly cookie identifies
an enabled dashboard user. The resulting authentication object MUST have `api_key_id =
null`, `api_key_name = null`, and internal source `playground`. No API-key row, token,
secret, prefix, or identifier is created for this authentication mode.

PG-AUTH5. An explicit API-key forwarding request MUST authenticate with
`Authorization: Bearer <full-key-value>`, where the value comes from the selected
dashboard API-key record. It MUST omit `x-monoize-internal-source` and
`x-monoize-playground-group`; the normal API-key authentication, billing, routing,
transforms, redirects, allowlists, multiplier ceiling, and request-capture rules apply.

PG-AUTH6. An API key is *time-eligible* iff `enabled == true` and `expires_at` is absent,
invalid, or in the future at evaluation time. A time-eligible key is *model-compatible*
with selected model id `m` iff `m` is empty, `model_limits_enabled == false`,
`model_limits == []`, or `m ∈ model_limits`. A time-eligible key *covers* an explicit
selected group `g` iff one condition is true:

1. `use_user_group == false` and `g ∈ group_ids`.
2. (`use_user_group == true` or `group_ids == []`) and `g` equals the session user's
   current group id.

PG-AUTH7. When an explicit API key is not model-compatible or does not cover the selected
non-auto group, sending MUST be disabled and an inline translated reason MUST identify
the incompatible model or group. The selected key id MUST remain unchanged. The frontend
MUST NOT silently fall back to the built-in credential or another API key.

PG-AUTH8. A built-in auto-group request omits `x-monoize-playground-group`.
The effective routing group list MUST equal every group that PG-AUTH9 permits and that
remains after the billing-plan group ceiling.
The session user's current group MUST be first when that group remains.
Every other group MUST follow in registry order (`groups-registry.spec.md` GR-D5).
A built-in explicit-group request MUST use only the selected group after the same ceiling.
If the ceiling removes the selected group, Monoize MUST return HTTP `403` before upstream
dispatch.
If the auto-group list is empty, Monoize MUST return HTTP `403` before upstream dispatch.

PG-AUTH9. For a built-in request, an explicit group is permitted iff the group exists and
at least one condition is true: the session user is an administrator, the group has
`user_selectable = true`, or the group id equals the session user's current group id. A
non-permitted group MUST return HTTP `403` before upstream dispatch. A group removed by a
non-empty enabled billing-plan group ceiling MUST return HTTP `403` before upstream
dispatch.

PG-AUTH10. Playground internal authentication MUST bill the session user's main balance.
It MUST NOT enable API-key sub-account billing, model allowlists, key transforms, key model
redirects, key IP allowlists, key multiplier ceilings, or key request capture.

PG-AUTH11. Built-in Playground requests MUST be admitted to the normal request-log
lifecycle with `request_kind = "playground"`. This classification is metadata; it is not
an API key and MUST NOT be accepted as an API-key credential. Requests authenticated by a
real API key MUST use the normal API-key request-log identity and MUST NOT use
`request_kind = "playground"`.

PG-AUTH12. The composer toolbar MUST render the credential picker as an independent
shadcn `DropdownMenu`. The picker MUST NOT be nested inside the settings popover. Its
trigger MUST identify the selected credential. Its first option is the translated
built-in Playground credential and represents `playground_api_key_id = ""`. It MUST then
list every time-eligible API key in API response order with its name and masked
`key_prefix`. The built-in option is selected by default. The picker MUST NOT show full
API-key values and MUST NOT contain an automatic-key-resolution option. The dropdown
content MAY scroll vertically, but an option list inside the content MUST NOT create a
second scrolling surface.

## 4. Selectors

PG-SEL1. The composer MUST contain compact popover selectors (shadcn `Popover` +
`Command`) for the routing group and the model for the active mode. Chat mode MUST show
only the chat-model selector. Image mode MUST show only the image-model selector.
Selectors MUST NOT be free-text-only inputs.

PG-SEL2. Group selector:

- Options are "auto" plus every `Group` from `GET /api/dashboard/groups` that satisfies
  PG-AUTH9 and the enabled billing-plan group ceiling, in response order.
- Each option MUST use `Group.id` as its value and render `Group.name` as its label.
- Selection persists the group id to `playground_group` (empty string for "auto").
- If a non-empty persisted id does not match a returned `Group.id`, the page MUST clear
  it to "auto" after the group response loads.

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

PG-SEL5. While either composer-selector backing hook (`useDashboardGroups` or
`useMarketplaceModels`) is loading with no cached data, the corresponding selector trigger
MUST render as a skeleton pill instead of an interactive control. While `useApiKeys` is
loading with no cached data, the credential picker MUST keep the built-in option available
and render a skeleton in place of API-key rows.

PG-SEL6. Image mode MUST show image settings inside the shared composer settings popover.
The toolbar MUST NOT contain a separate image-size trigger.
Image settings MUST remain hidden in chat mode.
The image-size header MUST contain a dropdown showing `auto` for an empty size, or the selected width and height.
The dropdown MUST offer `auto` first, followed by these grouped presets:

| Group | Sizes in pixels, in menu order |
|---|---|
| Square | `512x512`, `1024x1024`, `2048x2048`, `4096x4096` |
| Landscape | `1024x768`, `1280x720`, `1536x1024`, `1792x1024`, `1920x1080`, `2560x1440`, `3840x2160` |
| Portrait | `768x1024`, `720x1280`, `1024x1536`, `1024x1792`, `1080x1920`, `1440x2560`, `2160x3840` |

Each preset MUST show its width, height, and aspect ratio.
Selecting a preset MUST update both dimensions, sliders, numeric inputs, and the header without closing the settings popover.
Selecting `auto` MUST clear the stored size and display 1024 in both dimension controls.
Custom dimensions MUST remain selectable through separate width and height controls. Each dimension control MUST
contain a shadcn `Slider` and a numeric `Input`. Each dimension MUST accept every integer
from 256 through 4096 pixels. Each slider MUST use a 64-pixel step. Changing either
dimension MUST store `<width>x<height>` in `playground_image_size`. The `auto` action MUST
store an empty value. An invalid persisted value MUST resolve to `auto` and MUST NOT reach
an image request.
During one pointer drag, each slider MUST continue updating until release, including after the first change from `auto`.
Changing a dimension MUST NOT replace its slider or input DOM node.
Numeric inputs MUST retain focus while typing and accept integers that are not multiples of 64.
Non-numeric or empty input MUST restore the current dimension on blur.
On blur, finite numeric input MUST round to the nearest integer and clamp to 256–4096.

PG-SEL7. Image settings MUST include one horizontal Quality row below the dimensions.
The row MUST contain its label and one shadcn `Select` trigger showing the selected quality.
The options MUST appear in a dropdown, not a grid of buttons.
Its options MUST be `default`, `low`, `medium`, `high`, `xhigh`, and `max`, in that order.
The initial selection MUST be `default`.
Changes MUST persist to `playground_image_quality` and apply without reopening the popover.
An invalid persisted quality MUST resolve to `default`.
Quality MUST remain independent of size and MUST remain hidden in chat mode.
Focusing or blurring an unchanged dimension input MUST NOT replace the `auto` size.
The popover MUST stay within the viewport and scroll when its content exceeds the available height.

## 5. Chat Execution (AI SDK)

PG-CHAT1. Chat state MUST be managed by `useChat` from `@ai-sdk/react` with a custom
`ChatTransport` implementation (`MonoizeChatTransport`). The transport MUST NOT be the
default HTTP transport.

PG-CHAT2. `MonoizeChatTransport.sendMessages` MUST:

1. Read the current chat model id, selected credential, selected group id, system prompt,
   temperature, and max-tokens values at call time (latest selector state applies to every
   request, including regenerations).
2. Reject with an error carrying a translatable reason when the model is empty.
3. Build an OpenAI provider via `createOpenAI` from `@ai-sdk/openai` with
   `name = "monoize"` and `baseURL = <origin>/api/v1`. Built-in mode MUST pass the
   constant non-secret placeholder `monoize-playground-session-placeholder` as the SDK
   `apiKey`. The built-in fetch implementation MUST remove `Authorization` and
   `x-api-key` after the provider constructs the request. It MUST then add the PG-AUTH3
   headers and set `credentials = "include"`. The placeholder MUST NOT reach the HTTP
   request. Explicit API-key mode MUST use `apiKey = <full-key-value>` and omit the
   internal headers. In both modes the transport MUST select the Responses model
   `provider.responses(<model id>)`, so the upstream call is `POST /api/v1/responses`
   with `stream: true` against the local Monoize instance.
4. Convert UI messages with `convertToModelMessages` after applying PG-CHAT3
   sanitation.
5. Call `streamText` with: the converted messages; `system` set iff the stored system
   prompt is non-empty; `temperature` set iff `playground_temperature` parses as a
   finite number; `maxOutputTokens` set iff `playground_max_tokens` parses as a positive
   integer; the abort signal from the chat; and
   `providerOptions.openai = { reasoningSummary: "auto", store: false }`.
6. Return `toUIMessageStream(...)` of the resulting stream with an `onError` mapper
   that maps the failure to human-readable text (upstream error text must reach the
   UI): an `Error` maps to its `message`; a non-Error object maps to its string
   `message` field, else its nested `error.message` string field, else its JSON
   serialization; any other value maps to `String(value)`.

PG-CHAT2a. Request-field mapping (performed by the AI SDK Responses model; listed here
as the observable request contract):

- The model id maps to the `model` field.
- The system prompt maps to one `input[]` message item with role `system`, or role
  `developer` when the AI SDK classifies the model id as a reasoning model (model id
  matching `^o<digits>` followed by `-` or end, or `^gpt-<major>` with `major >= 5` and
  no `chat` variant suffix).
- `temperature` maps to the top-level `temperature` field. For a model id classified as
  a reasoning model, the AI SDK omits `temperature` and records a warning instead of
  sending it.
- Max tokens maps to the top-level `max_output_tokens` field.
- `providerOptions.openai.reasoningSummary = "auto"` maps to
  `reasoning: { "summary": "auto" }` iff the model id is classified as a reasoning
  model; otherwise no `reasoning` object is sent.
- `providerOptions.openai.store = false` maps to `store: false` on every request. For a
  model id classified as a reasoning model, the AI SDK additionally sends
  `include: ["reasoning.encrypted_content"]`.
- User-message image attachments map to `input_image` content parts (data URLs).

PG-CHAT3. Outgoing-message sanitation: `file` parts and `reasoning` parts of
**assistant** messages MUST be excluded from the converted model messages (generated
images cannot be replayed as assistant content, and reasoning is not replayed because
Monoize may route each request to a different upstream). If exclusion leaves an
assistant message with no parts: when at least one `file` part was removed, one text
part with literal content `[image]` MUST be substituted; otherwise the assistant
message MUST be removed from the outgoing conversation. User-message `file` parts MUST
be preserved (they encode user image attachments).

PG-CHAT4. The Dashboard document Content Security Policy MUST set `img-src * data:`.
The policy MUST allow Playground images from every network origin and from data URLs.

PG-CHAT7. Raw-reasoning SSE adapter. `@ai-sdk/openai@4` parses the reasoning-summary
event family (`response.reasoning_summary_part.added`,
`response.reasoning_summary_text.delta`, `response.reasoning_summary_part.done`) but
drops the raw-reasoning events `response.reasoning_text.delta` and
`response.reasoning_text.done` that Monoize emits for `Reasoning.content`
(`unified_responses_proxy.spec.md` STR3d). The transport MUST wrap the provider `fetch`
with an adapter obeying exactly these rules:

1. Responses whose `Content-Type` does not contain `text/event-stream` pass through
   unchanged.
2. The event-stream body is split into SSE frames at blank-line boundaries. A frame
   whose `data:` payload is not a JSON object with a string `type` field passes through
   byte-identical.
3. A frame with `type = "response.reasoning_text.delta"` is rewritten to
   `type = "response.reasoning_summary_text.delta"` with
   `summary_index = 1000 + <content_index>` (`content_index` defaults to `0` when
   absent) and unchanged `item_id`, `output_index`, and `delta`.
4. A frame with `type = "response.reasoning_text.done"` is rewritten to
   `type = "response.reasoning_summary_part.done"` with
   `summary_index = 1000 + <content_index>` and unchanged `item_id` and
   `output_index`.
4a. Before the first rewritten frame (rule 3 or rule 4) for a given
   (`item_id`, `content_index`) pair, the adapter MUST inject one synthetic
   `response.reasoning_summary_part.added` frame with the same `item_id`,
   `output_index`, and `summary_index`.
5. All other frames pass through byte-identical. The adapter MUST NOT reorder frames.
6. Rewritten frames use the new event type in both the `event:` line and the `data:`
   JSON `type` field.

PG-CHAT8. Reasoning-part classification: every reasoning UI part produced by the AI SDK
Responses model has id `<item_id>:<summary_index>`. The transport module MUST export a
classifier that maps a reasoning part to kind `content` iff its id parses to
`summary_index >= 1000` (the PG-CHAT7 base), and to kind `summary` otherwise (including
parts with no id). The UI MUST use only this classifier to distinguish raw reasoning
from reasoning summaries.

PG-CHAT4. Send/stop contract: while `status` is `submitted` or `streaming`, the primary
composer action MUST be a stop control invoking `stop()`. Stopping keeps all partial
assistant output as a normal message and MUST NOT surface an error.

PG-CHAT5. When `useChat` reports an `error`, an inline dismissible banner MUST appear
between the message list and the composer showing the error message, with a retry action
that follows PG-MSG6 and a dismiss action that calls `clearError()`. The retry action
MUST be disabled while a chat or image request is pending. No toast is shown for chat
request errors.

PG-CHAT6. User attachments: chat mode accepts images and ordinary files. Send MUST call
`sendMessage({ text, files })` so attachments become user-message `file` parts with their
original media type, file name, and data URL. Image mode accepts image files only because
its edit endpoint requires an image source.

## 6. Message Operations

PG-MSG1. Every message exposes hover/focus actions. Minimum set: copy (all roles with
text), edit (user and assistant), delete (all roles). Assistant messages additionally
expose regenerate, and each assistant image exposes download and edit-image actions.
For an assistant message with an image, all actions MUST render in one non-wrapping
toolbar below the image. The UI MUST NOT render a separate second message-action row.
On coarse-pointer devices the actions MUST be reachable without hover (always visible)
with touch targets per `frontend-design-system.spec.md` DS49.

PG-MSG2. Edit is inline: the message body is replaced by a textarea initialized with the
concatenated text parts, with confirm and cancel actions. Preconditions: `status` is
`ready` or `error`.

PG-MSG3. Confirming a **user** message edit MUST replace that message and remove all
later messages. The active composer mode determines the new request:

1. Chat mode MUST call `sendMessage({ text: <edited>, messageId })`.
   If the original message contains `file` parts, the call MUST also pass those parts
   through `files`.
2. Image mode MUST submit the edited text through PG-IMG2 or PG-IMG3, using all
   image file parts from that user message in their original order.
   Other file parts MUST NOT enter the image request.
   The request MUST use the current image model, size, group, and credential.
   It MUST NOT call the text transport or append another user message.

Both modes MUST preserve the original user message id and file parts. Each file MUST
preserve its media type, file name, URL, provider reference, and provider metadata.

PG-MSG4. Confirming an **assistant** message edit MUST replace the message's text parts
with a single text part containing the edited text via `setMessages`, in place, without
issuing any request.

PG-MSG5. Delete MUST remove exactly the targeted message via `setMessages` filtering,
without issuing any request. The optimistic update is the operation itself (client-only
state); no rollback path exists.

PG-MSG6. Regenerate on an assistant message that was not created by PG-IMG5 MUST call
`regenerate({ messageId })` in chat mode. This removes that assistant message and all
later messages, then requests a new text response with the current selector state.
In image mode, it MUST use the text and all image attachments from the preceding user
message through PG-IMG2 or PG-IMG3. It MUST use the current image model, size, group,
and credential. It MUST remove the targeted assistant message and all later messages
without appending another user message or calling the text transport.

The chat-error retry action follows the same mode selection for the last message.
If no assistant message exists for the failed request, it MUST use the last user message.
Starting an image request MUST clear the previous chat error. Missing image models,
empty prompts, or incompatible credentials MUST prevent dispatch and leave messages
unchanged. Message operations MUST NOT start requests while a request is pending.

## 7. Image Generation and Editing

PG-IMG1. The composer has a chat/image mode toggle. Mode is session state (not
persisted). While image mode is active the image-model selector is visible, the
chat-model selector is hidden, and the send action executes an image request instead of
a chat request.

PG-IMG2. Image send with no reference image selected by PG-IMG3a MUST call
`POST /api/v1/images/generations` with JSON body
`{ "model": <image model>, "prompt": <composer text>, "n": 1, "stream": true }` and the credentials and
headers selected by PG-AUTH2 through PG-AUTH5. If the selected image size is explicit,
the body MUST also contain `"size": <selected image size>`. If the selected image size is
`auto`, the body MUST omit `size`.

PG-IMG3. Image send with at least one reference image selected by PG-IMG3a MUST call
`POST /api/v1/images/edits` as `multipart/form-data`.
The form MUST contain `model`, `prompt`, `n = 1`, and `stream = true`.
The form MUST contain one `image` file field for each selected reference image, in order.
If the selected image size is explicit, the form MUST also contain `size`.
If the selected image size is `auto`, the form MUST omit `size`.
The request MUST use the credentials and headers selected by PG-AUTH2 through PG-AUTH5.

PG-IMG3a. Explicit composer attachments MUST supply the reference images when present.
Otherwise, select all image file parts from the most recent conversation message that contains image file parts.
Preserve their order, URLs, media types, and filenames.
If no message contains image file parts, select no reference images.
The selected references MUST enter the new user message and retained request input.
Thus, a text-only follow-up after image generation MUST use the latest generated images through PG-IMG3.
New chat MUST clear this reference context with the conversation.

PG-IMG3b. A selected quality other than `default` MUST enter generation JSON or edit multipart as the exact `quality` string.
The `default` selection MUST omit `quality` from both request formats.
Retained image requests MUST retain their quality for regeneration and error retry.
Editing a user prompt MUST use the currently selected quality.

PG-IMG3c. Loading an image reference from a base64 data URL MUST decode its bytes locally without a network request.
Image editing, image staging, regeneration, and follow-up sends MUST work with `connect-src 'self'`.
The decoded bytes and declared media type MUST remain unchanged.
HTTP image URLs MUST use the existing fetch path and report failed loads.

PG-IMG4. On image send the frontend MUST synchronously append a user message (prompt
text plus attachment file parts) to the chat state, and render a pending assistant
placeholder with an animated loading treatment until the request settles.

PG-IMG5. On success, the placeholder MUST be replaced by an assistant message whose
parts are, in order: one text part with `revised_prompt` when present, then one `file`
part per `data[]` entry — `url` used verbatim when present, otherwise
`data:image/png;base64,<b64_json>`. The frontend MUST retain the request input in memory,
keyed by the generated assistant message id, until the conversation is cleared.

PG-IMG5a. Image requests MUST consume the SSE response defined by `image-api-proxy.spec.md` section 5.5.
The parser MUST support chunk boundaries, UTF-8 decoding, LF or CRLF delimiters, and multiline `data` fields.
Ignore comment heartbeats, unknown events, and partial-image events; retain the pending placeholder until completion.
Collect images from `image_generation.completed` and `image_edit.completed` events in received order.
An `error` event MUST enter PG-IMG6, including when HTTP status is 200.
Success requires at least one completed image and the `[DONE]` sentinel.
EOF before `[DONE]`, malformed data, or a completed event without image data MUST enter PG-IMG6.

PG-IMG6. On failure, the placeholder MUST be replaced by an inline error state with a
retry action that re-issues the same request. The user message remains in the
conversation.

PG-IMG6a. Regenerate on an assistant message created by PG-IMG5 MUST remove that
assistant message and all later messages. It MUST then re-issue the retained image request
through `/api/v1/images/generations` or `/api/v1/images/edits`, according to whether the
retained request has at least one attachment. It MUST NOT call the text-generation
transport or append a second user message.

PG-IMG6b. While image mode is active, regenerate on an assistant message MUST NOT call
the text-generation transport.
If the retained image request is absent, rebuild the image request from the nearest
preceding user message and the current image model, size, group, and credential.
The rebuild MUST NOT append a second user message.

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
attachment preview row (when attachments exist), the auto-growing textarea,
and a control row with the selectors (PG-SEL1), the attach action, the mode toggle, the
settings popover trigger, and the send/stop action.

PG-CMP1a. The textarea MUST reserve at least one complete line plus its vertical padding, including when empty.
Its height MUST grow with explicit line breaks and wrapped text, up to 200 CSS pixels.
It MUST scroll vertically only when its content exceeds that maximum.
The textarea MUST recalculate its height before paint after text changes.
It MUST recalculate after its width changes or fonts finish loading.
Deleting text MUST reduce the height without clipping the remaining line or placeholder.
These requirements MUST hold in Safari and Chromium in both composer modes.

PG-CMP2. Enter submits and Shift+Enter inserts a newline on fine-pointer devices. On
coarse-pointer devices Enter inserts a newline and only the send button submits.

PG-CMP3. Send is enabled iff: a model for the active mode is selected, the selected
credential satisfies PG-AUTH2 and PG-AUTH7, `status` is `ready`/`error`, no image request
is pending, and the trimmed text is non-empty (chat mode also allows empty text with ≥ 1
attachment).

PG-CMP4. The composer MUST contain one settings trigger shared by both modes.
In chat mode, the popover MUST contain only these fields:

- System prompt: multiline text.
- Temperature: a clearable number from 0 through 2, with step 0.1.
- Max tokens: a clearable positive integer.

In image mode, it MUST contain only the image-size and Quality controls from PG-SEL6 and PG-SEL7.
Changing mode MUST select the corresponding settings content and preserve both modes' preferences.
The credential picker MUST remain outside this popover, as PG-AUTH12 requires.
Each field persists per PG-STATE2 on change. The
popover MUST align its end edge to the trigger, prefer opening above the trigger, keep at
least 16 CSS pixels from every viewport edge, and use internal vertical scrolling when its
content exceeds the collision-computed available height.

PG-CMP5. A "new chat" action MUST be visible whenever the conversation is non-empty; it
clears the chat state, any pending image job, and composer attachments. It MUST NOT
clear persisted preferences.

PG-CMP6. The Playground root MUST accept attachment files from all three input paths:
the file-picker action, a file drag-and-drop anywhere inside the page, and a clipboard
paste event whose clipboard contains one or more files. A clipboard paste without files
MUST preserve normal textarea text paste behavior. All three paths MUST apply PG-CHAT6's
mode restriction. Switching from chat mode to image mode MUST remove staged non-image
files. Each staged image MUST render an image thumbnail; each staged non-image file MUST
render a file icon and its truncated file name. Every staged attachment MUST expose the
same remove action.

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

PG-RD2. Assistant reasoning parts with non-empty trimmed text MUST render as up to two
collapsible muted sections above the answer text, one per PG-CHAT8 kind present in the
message: kind `content` uses label `playground.reasoning`; kind `summary` uses label
`playground.reasoningSummary`. Sections render in the order in which the first part of
each kind appears in `message.parts`. Within a section, part texts are joined with one
blank line and rendered as plain text preserving whitespace.

PG-RD2a. Reasoning parts whose trimmed text is empty MUST be excluded; a kind with no
remaining parts MUST NOT render a section (no empty panel).

PG-RD2b. Section expansion state: a section is *auto-expanded* while the message is the
actively streaming assistant response and at least one of the section's parts has
`state == "streaming"`; it auto-collapses when that condition stops holding. A manual
toggle by the user overrides automatic control for the remaining lifetime of the
rendered message component.

PG-RD2c. While a section is streaming (PG-RD2b condition holds), its header MUST show an
animated activity indicator whose animation is opacity-only under reduced motion.

PG-RD3. `file` parts with an `image` media type render as rounded images constrained to
the message column (max height `24rem`), with the PG-MSG1 image actions.

PG-RD4. A user-message `file` part with a non-image media type MUST render as a compact
downloadable file row containing a file icon and the original file name. It MUST NOT be
passed to an image element.

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

PG-C1. The Playground performs no dashboard configuration mutation.

PG-C2. The Playground MUST NOT implement its own SSE-to-UI-message decoding for chat;
stream decoding into UI message parts is handled by the AI SDK Responses
provider/`streamText` pipeline (PG-CHAT2). The only permitted SSE processing is the
frame-level event rewrite defined in PG-CHAT7.

PG-C3. The page MUST be split into multiple components under
`frontend/src/components/playground/`; the route file composes them.
