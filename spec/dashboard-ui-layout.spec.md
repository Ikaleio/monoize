# Dashboard UI Layout Specification

## 0. Status

- Product name: Monoize.
- Scope: layout and key interaction requirements under `/dashboard/*`.

## 1. Global Layout

DL1. Desktop (`lg` and above) MUST render:

- left sidebar navigation
- main content area

DL1a. The desktop sidebar MUST start expanded at every viewport width in the desktop
range. A viewport width change within the desktop range MUST NOT collapse the sidebar.

DL1b. The expanded desktop sidebar MUST use a `16rem` width. Its brand row MUST place a
collapse button to the right of the Monoize brand link. Activating this button MUST
collapse the sidebar to a `4rem` icon rail.

DL1c. The collapsed desktop sidebar MUST render the 32 px Monoize brand surface as an
expand button instead of a dashboard link. Activating this button MUST expand the
sidebar. Pointer hover or keyboard focus MUST replace the visible Monoize mark with a
lucide expand-sidebar icon while the button remains in that state.

DL1d. The collapse and expand buttons MUST expose localized accessible names. The
mobile sheet behavior in DL4 MUST remain independent of the desktop collapse state.

DL2. Top header bar MUST NOT be rendered.

DL3. User/account menu MUST be anchored at sidebar bottom.

DL3a. The expanded sidebar account trigger MUST show the authenticated user's prepaid
balance on the second line (localized unlimited label when `balance_unlimited` is true;
otherwise `balance_usd` formatted as USD with 2 fractional digits). The collapsed
sidebar MUST expose the same summary through the account-trigger tooltip. This display
MUST use the session user object and MUST NOT call `GET /api/dashboard/billing-plans`.

DL3b. The account dropdown content MUST be a compact `18rem`-wide (`w-72`) menu with
exactly these sections in order, separated by menu separators:

1. identity header,
2. prepaid-balance block,
3. active-plan block when an active subscription exists,
4. live-usage block,
5. actions (settings, theme toggle, sign out).

The dropdown MUST use the shared DropdownMenu (Radix) component so keyboard and
dismissal contracts are unchanged, and lucide icons only (no emoji glyphs).

DL3c. The identity header MUST show the Gravatar avatar (same `getGravatarUrl` source as
the trigger, username-initial fallback), the username (truncated), the localized role
label (`roles.{role}`), and the email truncated when present.

DL3d. The prepaid-balance block MUST be sourced from the session user object only and MUST
NOT call `GET /api/dashboard/billing-plans`. It MUST render:

- a balance row: localized unlimited label when `balance_unlimited` is true, otherwise
  `balance_usd` formatted as USD with 2 fractional digits via `formatUsdDecimal`.

While the session user object has not resolved, the block MUST render skeleton
placeholders instead of empty rows.

DL3d1. The active-plan block MUST load
`GET /api/dashboard/billing-plan-subscription` through the
`useBillingPlanSubscription` SWR hook. It MUST NOT load
`GET /api/dashboard/billing-plans` or the plan marketplace. When the endpoint returns an
active subscription, the block MUST show the plan name and one row for every configured
sliding window in the order 5 hours, 24 hours, 7 days, and 30 days. Each row MUST show:

- the localized window label;
- `used_nano_usd / limit_nano_usd`, formatted as USD with 2 fractional digits;
- a progress indicator whose value is `clamp(used / limit * 100, 0, 100)`;
- the localized next-reset label and the locale-formatted `next_reset_at`, or a localized
  fully-available label when `next_reset_at` is `null`.

The block MUST be absent when the endpoint returns `null`. While no response is cached,
it MUST render skeleton placeholders. When the fetch fails and no cached response exists,
it MUST render a compact localized failure message and retry action. A failure MUST NOT
unmount or break the other dropdown sections. The menu MUST render the plan block on both
desktop and mobile.

DL3e. The live-usage block MUST show the authenticated user's own rolling 60-second
metrics fetched from `GET /api/dashboard/me/live-usage` through the `useLiveUsage` SWR
hook (`user-live-usage.spec.md` LU-9/LU-10; 10-second refresh while the menu is open).
It MUST render three labeled metrics: `rpm`, `tpm` (locale-grouped integers), and
`cache_hit_rate` formatted per LU-11 (percentage with at most 1 fractional digit; em
dash when `null`). This block never shows another user's data, including for admin
viewers. While no data is cached, the block MUST render skeleton placeholders. When the
fetch fails, the block MUST render a compact localized error message with a retry action
that revalidates the SWR key; the failure MUST NOT unmount or break the rest of the menu.

DL3f. The actions section MUST keep exactly: a settings item navigating to `/settings`,
the theme toggle, and a sign-out item, with the same behavior as before the dropdown
expansion.

DL3g. The mobile sheet sidebar (DL4) MUST render the same account dropdown component,
so DL3b through DL3f hold on both desktop and mobile.

DL4. Mobile (`< lg`) MUST render sidebar via left sheet menu. The menu trigger MUST expose
a localized accessible name. The Sheet content MUST contain a localized accessible title
and description even when both strings are visually hidden.

DL5. Sidebar main navigation (always visible to authenticated users) MUST include exactly:

- `/dashboard`
- `/dashboard/tokens`
- `/dashboard/wallet`
- `/dashboard/logs`
- `/dashboard/playground`

The `/dashboard/wallet` page content is governed by
`spec/recharge-system.spec.md` §10.

DL6. Sidebar admin navigation group (visible only when user role is `admin` or `super_admin`) MUST include exactly:

- `/dashboard/admin`
- `/dashboard/providers`
- `/dashboard/models`
- `/dashboard/custom-transforms`
- `/dashboard/plans`
- `/dashboard/payments`
- `/dashboard/users`
- `/dashboard/groups`
- `/dashboard/admin-settings`

The `/dashboard/custom-transforms` page content is governed by
`spec/custom-js-transforms.spec.md` §11 (CJS-UI-1 through CJS-UI-7).
The `/dashboard/payments` page content is governed by
`spec/recharge-system.spec.md` §11.

DL7. In desktop layout (`lg` and above), `/dashboard/*` pages MUST use single-pane vertical scrolling:

- viewport-level/document-level vertical scroll MUST be disabled for dashboard shell;
- left sidebar pane MUST remain fixed in viewport and MUST NOT move during right-pane content scroll;
- right main content pane MUST be the only vertical scroll container when page content overflows viewport height.

## 2. Providers Page

PL1. `/providers` page MUST be provider-centric.

PL2. Provider list MUST display, at minimum:

- provider name
- enabled state
- model count
- channel count
- routing priority index

PL3. Provider list MUST support drag-and-drop reordering and persist order through `/api/dashboard/providers/reorder`.

PL4. Provider detail/editor MUST display:

- provider-level fields: `name`, `enabled`, `max_retries`, and `group_ids` edited through the shared unordered multi-select group selector (GS rules, §11); freeform group text entry MUST NOT be offered
- channel master list: name, type, base URL, weight, enabled, model count, and runtime health
- selected Channel detail editor with per-model controls for: logical model, redirect target, multiplier, and delete
- channel runtime health indicator: healthy/probing/unhealthy

PL4.0. Provider detail/editor MUST NOT render or maintain a Provider-level model selector or Provider-level model editor.

PL4.1. Provider detail/editor MUST place the provider `enabled` switch in the top title row, right-aligned from the provider editor title.

PL4.2. At mobile widths, the Provider editor title row MUST reserve a dedicated right-side touch area for the dialog close button. The Provider `enabled` switch and the close button hit areas MUST NOT overlap, and each control MUST remain independently clickable.

PL5. API keys/secrets for channels MUST never be shown after save (write-only behavior).

PL6. Provider detail/editor MUST include an upstream transform editor bound to provider `transforms`.

PL7. Provider upstream transform editor MUST render exactly two independent compact chains:

- request-phase chain (`phase = request`)
- response-phase chain (`phase = response`)

PL8. Each provider transform chain MUST support:

- append transform from transform registry filtered by supported phase
- drag-and-drop reordering within the same phase chain
- per-item delete
- per-item enabled toggle
- per-item config button that opens a config dialog

PL8a. At viewport widths below `640px`, each transform chain header MUST stack its title and add controls vertically. The transform selector MUST shrink to the available width, and the add action MUST remain visible without creating horizontal overflow.

PL8b. The add-transform selector options and every rendered chain item MUST display the transform's localized registry `name` resolved per `transform-config-ui.spec.md` TCU-2, with the canonical `type_id` rendered as secondary monospace text. A chain item whose `type_id` is absent from the registry MUST fall back to displaying the raw `type_id`.

PL9. Provider transform config dialog MUST:

- edit `models` glob filters as string list (`*` and `?` supported)
- edit transform `config` using schema-driven fields from `/api/dashboard/transforms/registry`
- block save when schema validation fails

PL9b. Config field rendering, typing, and null/omission semantics are defined by `transform-config-ui.spec.md` TCU-3 through TCU-9. A config schema property that has neither `type` nor `enum` is a JSON-valued field and MUST render as the typed value editor defined by TCU-7; it MUST NOT render as a bare JSON text input.

PL9c. When the transform config dialog opens, field initialization MUST follow `transform-config-ui.spec.md` TCU-8. The dialog MUST NOT initialize an absent field as the text `null`.

PL9d. Saving the dialog MUST produce config values per `transform-config-ui.spec.md` TCU-9. Empty optional fields MUST be omitted from the saved config and MUST NOT fail save as invalid JSON.

PL10. If a provider transform item type is not present in transform registry, editor MUST:

- keep item visible with unknown marker
- allow reorder/delete/toggle-enabled
- render `config` as read-only JSON
- preserve unknown item fields on save unless user deletes the item

PL11. In provider editor channel table, `base_url` input MUST enforce the following blur behavior:

- When input loses focus and value ends with `/v1` (or `/v1/`), UI MUST open a confirmation dialog.
- Opening this confirmation dialog MUST NOT throw runtime exceptions, and provider editor controls MUST remain interactive.
- Dialog MUST offer two explicit actions:
  - remove trailing `/v1` (recommended);
  - keep trailing `/v1`.
- If user chooses remove, input value MUST be replaced with value without trailing `/v1`.
- If user chooses keep, UI MUST preserve the entered value and MUST allow save without further automatic normalization.

PL12. Provider list card header MUST place provider metadata and controls in a compact single-row layout on desktop:

- metadata block MUST include `priority` and `max_retries` aligned near action buttons;
- provider enable switch MUST be colocated in the header action zone;
- edit/delete/reorder controls MUST remain available without expanding card height.

PL12a. Provider list card header metadata badges MUST render through a collapsed badge collection when the number of metadata badges is greater than 3.

- The header badge preview MUST render no more than 3 badges before a `+N` overflow badge.
- The preview row MUST NOT wrap.
- The complete popover list MUST include channel type, enabled state, one model-attention badge when any model has a breaker or pricing warning, and one badge per provider `group_ids` entry showing the group name resolved from `GET /api/dashboard/groups` (raw id fallback for unknown ids).
- The model-attention badge count MUST deduplicate logical model IDs. It MUST use destructive styling when any model has `availability_status = unavailable`, and warning styling otherwise.

PL13. The selected Channel model section MUST include an explicit "Fetch upstream" action that opens a model-diff selection dialog before insertion.

- Dialog MUST fetch upstream model list from `POST /api/dashboard/fetch-channel-models` with the current Channel `provider_type` and `base_url`.
- If the current Channel is an existing saved Channel and the API key input is empty, Dialog MUST pass `provider_id` and `channel_id` instead of requiring API key entry.
- If the current Channel is new or has no saved `channel_id`, Dialog MUST require a non-empty API key before opening the picker.
- If the API key input is non-empty, Dialog MUST pass that value so unsaved key edits are used for the fetch request.
- Dialog MUST place the "Fetch Models" action in the Supported Models action row immediately before "Select All".
- Dialog MUST split entries into `new` and `existing` tabs.
- Dialog MUST initialize selection from the keys of the current Channel `models` object.
- Dialog MUST allow selecting fetched models for the current Channel.
- While the dialog remains open, a successful fetched model list MUST remain visible and MUST NOT be cleared by unrelated parent rerenders.
- Dialog model list container MUST have a bounded positive height with internal scrolling so fetched rows are visible immediately after load.
- Dialog model list items MUST render as compact stacked badges (wrapping rows), not forced single-column rows.
- Confirming selection MUST set only the current Channel `models` object.
- Newly selected model IDs MUST receive default `{ redirect: null, multiplier: "1" }` entries.
- Existing Channel model entries MUST preserve their redirect and multiplier values when the same logical model remains selected.
- Removing a selected model MUST remove only that Channel model entry and MUST NOT mutate any sibling Channel.

PL14. Channel model badges (Provider overview and model-diff dialog) MUST display provider logo using model metadata (`models_dev_provider`) when available, with graceful fallback icon behavior when unavailable.

PL14.1. Model-badge icon resolution MUST be deterministic for GLM series:

- Normalize provider ID by lowercasing and removing whitespace, `_`, and `-`.
- If lowercase `model` contains `glm`, the badge MUST render the GLM-series icon (this rule has higher priority than provider-based mapping).
- If normalized provider is `glm` or `chatglm`, the badge MUST render the GLM-series icon.

PL15. In the selected Channel model section, each model row MUST be rendered as a compact clickable model tag.

- Tag text format MUST be `<(provider-logo) model-id [multiplier, target]>`.
- Bracket details (`[multiplier, target]`) MUST use muted/gray text to indicate secondary information.
- The model section MUST render all model tags in a wrapping `flex` collection with a bounded vertical scroll area.
- The model section MUST NOT render logical-model, redirect, multiplier, or delete controls inline before a model tag is clicked.
- Clicking a model tag MUST open an edit dialog for that row.
- Edit dialog MUST allow updating at least: `model`, `redirect`, `multiplier`.
- Edit dialog MUST include delete action for the selected model row.
- Clicking "Add Model" MUST open a draft model dialog without appending a row immediately.
- A new model row MUST be appended only when user confirms via dialog save action.
- Closing/canceling the add-model dialog without saving MUST NOT create an empty model row.
- Editing an existing model row MUST operate on a draft copy. Closing/canceling the edit dialog without saving MUST leave the underlying row unchanged.
- The provider editor UI MUST reject duplicate logical model names before save or final submit. It MUST NOT silently overwrite an earlier model row when two rows use the same trimmed model name.

PL16. Channel model tag bracket details in provider card/editor MUST follow omission rules:

- multiplier fragment MUST be omitted when multiplier equals `1x`;
- redirect fragment MUST be omitted when redirect target equals the model itself (or is empty);
- bracket section MUST be omitted entirely when both fragments are omitted.

PL17. Provider edit dialog initialization MUST be resilient to fast-open timing.

- On open in edit mode, UI MUST fetch fresh provider detail (`GET /api/dashboard/providers/{id}`) using SWR.
- Until detail hydration is ready, UI MUST render skeleton placeholders instead of empty editable controls.
- If detail fetch fails, UI MAY fallback to list-sourced provider snapshot instead of requiring close/reopen.

PL18. In expanded provider card overview, channel runtime list row spacing MUST be deterministic.

- Each rendered channel row MUST use a minimum row height of `40px`.
- Virtual list container height MUST be computed as `min(channel_count * 40, 190)`.
- The row height constant used by the virtual list and the row element style MUST be the same value to prevent visible trailing blank space.
- The runtime health label, Channel name, and Channel Base URL MUST share one visual vertical center at every viewport width. The row action group MUST remain vertically centered independently.

PL18a. The Provider card header title, status badges, and visible metadata MUST share one visual vertical center. The title MUST NOT use a vertical translation.

PL19. Model badge lists on the Providers page MUST use a wrapping stacked-badge layout and MUST NOT hide model badges behind a `+N` overflow badge or popover.

- Expanded provider-card model lists MUST render every model badge in a bounded, vertically scrollable, wrapping container.
- The selected Channel model editor MUST render every clickable model tag directly and MUST NOT collapse tags behind an overflow control.
- Provider overview model badges and Channel model rows MUST preserve unpriced highlighting.
- Model-list containers MUST keep symmetric top/bottom inner spacing so the badge block appears visually centered and not top- or bottom-heavy.
- A detail overlay attached to an individual visible model badge does not violate this rule. The model badge itself MUST remain directly visible in the stacked list.

PL19a. The expanded provider-card model list, the selected Channel model editor, and the model picker dialog result area MUST render their badge collections through one shared stacked model list container component (`StackedModelList`), so that border, inner padding, wrap behavior, and item gap are identical on all three surfaces. The bounded height of the inner scroll region MAY be overridden per surface (the model picker uses a taller viewport-relative height); border, padding, and wrap behavior MUST come from the shared component.

PL20. Provider edit dialog channel list MUST use virtualized rendering (`react-virtuoso`) with embedded scrolling.

- Channel list MUST render through `Virtuoso`.
- Container MUST have bounded height and provide an internal vertical scrollbar.

PL21. Provider overview model entries MUST combine availability and pricing warnings at model-badge level without combining their meanings.

- Unpriced check target MUST be `redirect` model when `redirect` is non-empty; otherwise the logical model key.
- A model is treated as unpriced when pricing metadata does not provide both input and output token prices for that target model.
- A pricing value of `0` MUST be treated as present pricing metadata, not as missing metadata.
- An unavailable model badge MUST use destructive red styling.
- A partially unavailable or unpriced model badge MUST use yellow warning styling.
- When both conditions apply, availability severity MUST determine the badge color.
- The badge detail overlay MUST render separate availability and pricing sections.

PL21a. `GET /api/dashboard/providers` MAY aggregate `unpriced_model_ids` across Channels for the Provider card. The count MUST deduplicate logical model IDs, while Channel detail highlighting MUST evaluate the selected Channel entry redirect independently.

PL22. In the provider unsaved-changes confirmation dialog ("Save Changes?"), the "Discard" action MUST use destructive red hover styling.

PL23. Provider channel edit dialog MUST expose channel-level passive breaker override fields with empty value meaning "inherit global setting":

- `passive_failure_threshold_override`
- `passive_cooldown_seconds_override`
- `passive_window_seconds_override`
- `passive_rate_limit_cooldown_seconds_override`

PL23b. Provider channel edit dialog MUST expose these Channel affinity overrides. The inherit option or an empty numeric value MUST serialize as null:

- `affinity_enabled_override`
- `affinity_idle_ttl_seconds_override`
- `affinity_failback_mode_override`
- `affinity_failback_delay_seconds_override`

PL23c. The Channel editor MUST describe `"sticky"` as retaining an eligible bound Channel and `"prefer_higher_priority"` as retrying an earlier eligible Provider after the configured delay.

PL23a. Provider channel edit dialog MUST operate on a draft copy of the selected channel row.

- Clicking "Add Channel" MUST open a draft channel dialog without appending a row immediately.
- A new channel row MUST be appended only when user confirms via dialog save action.
- Closing/canceling the add-channel dialog without saving MUST NOT create an empty channel row.
- Editing an existing channel row MUST NOT mutate the underlying list row until user confirms via dialog save action.
- Closing/canceling the existing-channel dialog without saving MUST leave the underlying row unchanged.

PL24. While the provider editor dialog is open, interaction with a child dialog that belongs to the provider editor MUST NOT be treated as an outside click of the provider editor dialog. This rule applies to at least:

- the unsaved-changes confirmation dialog;
- the trailing `/v1` confirmation dialog;
- the model picker dialog;
- the model edit dialog;
- the channel edit dialog.

Clicking an action button inside any such child dialog MUST execute only that child dialog action and MUST NOT open another unsaved-changes confirmation dialog through the parent provider editor outside-click handler.

PL25. Provider editor MUST use an explicit workbench information architecture.

- Desktop (`lg` and above) MUST render a Provider section rail, Channel master list, and selected Channel detail pane simultaneously.
- Mobile (`< lg`) MUST render one pane at a time. Selecting a Channel MUST open a full-width Channel editor with an explicit back action.
- Mobile save/cancel actions MUST remain reachable in a sticky bottom action bar.
- Primary connection and model controls MUST appear before breaker, probe, retry, transform, and protocol override controls.
- Advanced groups MUST be collapsed by default and MUST display a summary when closed.

## 3. Playground Page

ST0. The page-level layout of `/dashboard/admin-settings` (horizontal category rail,
category panels, skeleton, and motion) is governed by `spec/system-settings-ui.spec.md`.
ST1-ST7 below define field-level behavior; where an ST statement calls the container a
"card", the container is the corresponding category panel/section defined there.

ST1. `/dashboard/admin-settings` MUST include a "Health Monitoring" section for Monoize active probe settings.

ST2. Health Monitoring section MUST expose at least these editable fields bound to `GET/PUT /api/dashboard/settings`:

- `monoize_active_probe_enabled` (boolean switch)
- `monoize_active_probe_interval_seconds` (integer >= 1)
- `monoize_active_probe_success_threshold` (integer >= 1)
- `monoize_active_probe_model` (optional string, empty means null)
- `monoize_passive_failure_threshold` (integer >= 1)
- `monoize_passive_cooldown_seconds` (integer >= 1)
- `monoize_passive_window_seconds` (integer >= 1)
- `monoize_passive_min_samples` (integer >= 1)
- `monoize_passive_failure_rate_threshold` (number in `[0.01, 1.0]`)
- `monoize_passive_rate_limit_cooldown_seconds` (integer >= 1)
- `monoize_enable_estimated_billing` (boolean)
- `monoize_strip_cross_protocol_nested_extra` (boolean)
- `monoize_request_capture_enabled` (boolean switch, default off)
- `monoize_request_capture_max_total_bytes` (integer >= 0, default 1073741824; MiB-denominated input per `system-settings-ui.spec.md` SSU-2a)
- `monoize_mask_sensitive_info` (boolean switch, default on; when on, client-facing and non-admin request-log error text apply `MASK` per `upstream-error-sanitization.spec.md`; when off, that masking is disabled)

ST2a. `/dashboard/admin-settings` MUST include a "Routing Affinity" section bound to `GET/PUT /api/dashboard/settings`. The section MUST expose:

- `monoize_affinity_enabled` (boolean switch, default `true`)
- `monoize_affinity_idle_ttl_seconds` (integer >= 1, default `1800`)
- `monoize_affinity_failback_mode` (exactly `"sticky"` or `"prefer_higher_priority"`, default `"sticky"`)
- `monoize_affinity_failback_delay_seconds` (integer >= 0, default `300`)

ST2b. The Routing Affinity section MUST state that Channel overrides replace global values and that `"prefer_higher_priority"` returns to normal Provider order only after its delay and only when an earlier Provider is eligible.

ST3. Settings UI MUST perform optimistic update and persist via existing settings save flow; persisted values MUST be reflected after reload.

ST4. `/dashboard/admin-settings` MUST include a global transform editor bound to `GET/PUT /api/dashboard/settings` field `global_transforms`.

ST5. The global transform editor MUST follow the same interaction contract as PL7, PL8, PL9, and PL10, but its option list MUST be filtered to transforms whose registry metadata includes `global` in `supported_scopes`.

ST6. `/dashboard/admin-settings` MUST include a "Codex Model Picker" section bound to `GET/PUT /api/dashboard/settings` field `codex_model_ids`.

ST6a. The section MUST load Provider data through the existing SWR Provider hook. Its available model set MUST be the sorted union of Channel model keys where the Provider is enabled, the Channel is enabled, and Channel weight is greater than zero.

ST6b. The section MUST provide a text search input and one controlled checkbox for each matching model. Changing a checkbox MUST update the local optimistic settings draft. The existing settings save action MUST persist the resulting ordered array. The section MUST NOT provide a bulk "select all" action.

ST6c. A configured model absent from the available model set MUST remain visible and removable with an unavailable label. Provider loading MUST render skeleton rows. Provider failure MUST render an error state with a retry action. Search with no matches MUST render an explicit empty state.

ST6d. The section MUST state that standard OpenAI `data` continues to include every available model and that `codex_model_ids` controls only the extended Codex `models` catalog.

ST6e. At viewport widths below `640px`, the settings category rail and the active category panel MUST shrink to the available content width without creating page-level horizontal overflow; the rail scrolls inside its own container per `system-settings-ui.spec.md` SSU-5.

ST7. `/dashboard/admin-settings` MUST include a "Global Model Redirects" section
bound to `GET/PUT /api/dashboard/settings` field `global_model_redirects`.
The section MUST follow `spec/api-key-model-redirects.spec.md` FR-8 through FR-13.

PG-L1. `/playground` page MUST be accessible from the main navigation sidebar (below Token Management).

PG-L2. The playground page layout follows `playground.spec.md` §9: a full-height chat
shell inside the dashboard content pane with its own internal scroll container and
`framer-motion` animations. It intentionally has no `PageHeader`/`text-3xl` heading
block.

## 4. Token Management Page

AK1. API key create and edit dialogs MUST include a downstream transform editor bound to API key `transforms`.

AK2. API key downstream transform editor MUST follow the same interaction contract as PL7, PL8, PL9, and PL10.

AK3. API key transform edits MUST be scoped to the edited key only and MUST NOT mutate other keys.

AK3a. API key transform editor option list MUST be filtered by transform scope metadata returned from `/api/dashboard/transforms/registry`.

- The editor MUST show only transforms whose `supported_scopes` includes `api_key`.
- The editor MUST continue filtering by `supported_phases` within the API-key-scoped subset.
- Transforms not available to API keys MUST be hidden from the add-transform selector instead of being shown and rejected after selection.
- The transform-editor UI MUST fetch the registry only when the session user role is
  `admin` or `super_admin`; for role `user` the client MUST NOT issue the registry
  request at all (the endpoint is admin-only per SAC-1/SAC-3), and the transform editor
  MUST render as unavailable (no registry options) instead of polling and failing with
  HTTP 403.

AK3b. Backend API key persistence and validation MUST accept every transform whose registry metadata advertises `supported_scopes` including `api_key`, including `reasoning_inject_content_field` for response-phase rules.

AK4. API key create and edit dialogs MUST include a `request_capture_mode` tri-state control.

AK5. The `request_capture_mode` control MUST default to `"off"` when creating an API key.

AK6. The API key list MUST display a visible indicator for keys whose `request_capture_mode != "off"`.

AK7. The `request_capture_mode` control label or help text MUST state that the system-wide capture switch must also be enabled before dumps are written.

AK8. The `request_capture_mode` control MUST expose exactly these three options:

- `"off"`
- `"capture-all"`
- `"capture-only-abnormal"`

AK9. The `"capture-only-abnormal"` option help text MUST explain that abnormal means upstream error, missing usage information, or usage total equal to zero.

AK10. API key restriction indicators in `/dashboard/tokens` MUST render as a non-wrapping collapsed badge preview when at least one restriction badge is present.

- The restriction preview MUST render no more than 2 badges before a `+N` overflow badge.
- Restriction badges MUST NOT wrap.
- The restriction preview MUST NOT render long help text inside the table cell.
- The complete popover list MUST include model-limit, IP whitelist, max-multiplier, and request-capture badges when those restrictions are active.

AK11. In the `/dashboard/tokens` list table, the API key name and its group badge collection MUST render in a single non-wrapping inline row inside the name cell.

- A key with `use_user_group = true` MUST NOT render group badges (it follows the owner's group).
- A key with `use_user_group = false` MUST render its `group_ids` in stored order as name badges resolved from `GET /api/dashboard/groups`; an id without a matching registry row MUST fall back to the raw id.
- The group badge collection MUST remain adjacent to the API key name and MUST NOT move below the name.
- If the inline row exceeds the available viewport width, the table container MUST handle overflow through horizontal scrolling.

## 5. Dashboard Home Page

The normative home-overview contract lives in `spec/dashboard-home-overview.spec.md`.
This section restates the layout binding for `/dashboard` and MUST stay aligned with that
file.

DH1. `/dashboard` MUST render these sections in order: greeting header; account overview;
full-width usage chart; recent-usage table + API information; performance panel. Details:
`dashboard-home-overview.spec.md` DH-1 through DH-9c.

DH2. The flat five-cell account overview, its fixed 24-hour Analytics data, and its
conditional active-model/subscription cell MUST follow
`dashboard-home-overview.spec.md` DH-5 through DH-5e.

DH3. The usage chart MUST be a stacked cumulative token `AreaChart` driven by
`GET /api/dashboard/analytics` with `buckets` and `range_hours` derived from the
selected time window (`1h`, `24h`, `7d`, `30d`; default `1h`) per the
`dashboard-home-overview.spec.md` DH-6a mapping, including `tokens_by_model`, a
fully visible Today/Now marker, and a width-adaptive paginated horizontal legend whose
controls appear on the right of the same row
(`dashboard-home-overview.spec.md` DH-6 through DH-6i).

DH4. Recent usage and API information panels MUST follow
`dashboard-home-overview.spec.md` DH-7 through DH-8d, including bounded table scrolling
and the Messages-before-Models endpoint order.

DH5. The performance panel and admin-configurable targets MUST follow
`dashboard-home-overview.spec.md` DH-9 through DH-9c. The ready state MUST use the
specified four-column table alignment and shared nonlinear row-entry motion.

DH6. Loading and motion contracts MUST follow
`dashboard-home-overview.spec.md` DH-11 through DH-13, including the
no-skeleton-on-revalidate and no-animation-replay rules (DH-12a through DH-12c).

DH7. `/dashboard` MUST NOT throw runtime errors when optional config fields are absent
from settings or analytics payloads.

## 6. Users Page

UP1. In `/dashboard/users` list table, the role badge (`user.role`) MUST be rendered as a single-line badge. Badge text and icon MUST NOT wrap into multiple lines.

UP2. The role badge container in `/dashboard/users` table MUST enforce a fixed maximum height equal to one badge row and MUST use horizontal overflow (`overflow-x: auto`, `overflow-y: hidden`) when space is insufficient on narrow viewports.

UP3. The users table in `/dashboard/users` MUST allow horizontal scrolling on narrow viewports so role badges remain single-line instead of wrapping.

UP4. The users table body in `/dashboard/users` MUST use virtualized rendering via `react-virtuoso` (`TableVirtuoso`) instead of rendering all rows as plain DOM rows.

- Table header MUST be rendered via `fixedHeaderContent` (sticky header).
- Table body rows MUST be rendered via `itemContent` callback.
- Virtualized table container height MUST be `calc(100vh - 280px)` with a minimum height of `400px`.

UP5. In the `/dashboard/users` list table, the username text and the user's group badge MUST render in a single non-wrapping inline row inside the user cell.

- If horizontal space is insufficient, the username text MAY truncate.
- The group badge MUST display the group **name** resolved from `GET /api/dashboard/groups`; an id without a matching registry row MUST fall back to the raw id.
- The group badge MUST remain single-line and MUST NOT move below the username.

UP12. The user create and edit dialogs MUST select the user's group through the shared
single-select group selector (GS rules, §11). They MUST NOT offer freeform group text
entry.

UP13. The user edit dialog MUST provide the active-subscription management surface defined
by `billing-plan-subscriptions.spec.md` BP-UI6 through BP-UI8. Subscription assignment
MUST remain separate from prepaid-balance editing. The users table MUST continue to omit a
plan-assignment column.

UP6. The users table MUST include columns for UTC-calendar-day spend and UTC-calendar-day call count, in addition to the existing user, role, prepaid-balance, and status columns. It MUST NOT include a plan-assignment column.

UP8. The today-spend cell MUST display `today_cost_nano_usd` as USD with 2 fractional digits
using exact integer formatting (`BigInt`). A missing value MUST display as `$0.00`.

UP9. The today-calls cell MUST display `today_calls` as a locale integer. A missing value MUST
display as `0`.

UP10. The users-page toolbar MUST display the UTC-calendar-day totals across the listed users:
the sum of `today_cost_nano_usd` formatted as USD with 2 fractional digits, and the sum of
`today_calls`. Both sums MUST be computed from the list payload with `BigInt` / integer
arithmetic. The page MUST NOT fetch a second usage endpoint for those totals.

UP11. Each user row MUST include an action that navigates to
`/dashboard/logs?username={username}` with the row's exact username. That destination MUST
initialize the request-log username filter as defined by `spec/request-logs.spec.md` FL7b.

## 8. User Settings Page

US1. `/settings` MUST render a read-only prepaid-balance card sourced from the authenticated user
object. The card MUST NOT call `GET /api/dashboard/billing-plans`.

US2. The billing card MUST show current prepaid balance, or the localized unlimited label when `balance_unlimited` is true. It MUST NOT show plan data.

US3. The Email field label icon and label text MUST form one inline group whose rendered boxes share one visual vertical center.

US4. The billing card MUST use the same skeleton/loading contract as the rest of `/settings`
when the user object has not yet resolved. It MUST NOT require a page close/reopen to
reflect a later `auth/me` refresh.

## 7. Token Management Page (UI)

AK4. The API keys table body in `/dashboard/tokens` MUST use virtualized rendering via `react-virtuoso` (`TableVirtuoso`) instead of rendering all rows as plain DOM rows.

- Table header MUST be rendered via `fixedHeaderContent` (sticky header).
- Table body rows MUST be rendered via `itemContent` callback.
- Virtualized table container height MUST be `calc(100vh - 280px)` with a minimum height of `400px`.
- Select-all checkbox MUST remain in the fixed header; per-row checkboxes MUST remain in `itemContent`.

AK-UI-BATCH-1. When one or more API keys are selected, the page header MUST show a destructive batch-delete action with a localized label equivalent to "Delete selected". The frontend MUST render the translated label and MUST NOT render an untranslated locale-key identifier.

AK5. API key create and edit dialogs in `/dashboard/tokens` MUST include a group section
containing exactly:

- a "use the owner's user group" switch bound to `use_user_group` (default on for create);
- when the switch is off, the shared ordered multi-select group selector (GS rules, §11)
  bound to `group_ids`.

The dialogs MUST NOT offer freeform group text entry and MUST NOT render a legacy `group`
text input.

AK6. When `use_user_group` is on, the `group_ids` selector MUST be hidden or disabled and
the mutation payload MUST send `use_user_group: true`. When it is off, the mutation payload
MUST send `use_user_group: false` and the ordered `group_ids` array exactly as displayed.

AK6a. Selection, removal, and reordering in the API-key group selector MUST apply against
the latest in-session draft. A reorder MUST NOT resurrect groups the user removed from the
current draft.

AK7. The group section helper text MUST explain that group order is the routing preference
order (earlier groups are tried first) and that the owner-group switch inherits the user's
single group. Non-admin callers MUST only be offered options with `user_selectable = true`
plus their own current group; admin callers MUST be offered every group.

AK8. If `POST /api/dashboard/tokens` or `PUT /api/dashboard/tokens/{key_id}` returns a
group validation error, the frontend MUST surface the server-provided message in a toast
and MUST keep the dialog open with the current draft state intact.

## 9. Billing Plans Page

BP-UI1. Each plan row on `/dashboard/plans` MUST include a Reset action in addition to
edit and delete.

BP-UI2. Activating Reset MUST open a confirmation dialog that names the plan. Confirming
MUST call `POST /api/dashboard/billing-plans/{plan_id}/reset`. Cancel MUST call nothing.

BP-UI3. After a successful reset, the users-list SWR cache and the session user cache
MUST be revalidated so `/dashboard/users` and the sidebar balance reflect the new
balances without a page close/reopen. On failure, the dialog MUST remain available and
the UI MUST surface the server error.

BP-UI4. Plan create and edit dialogs MUST select `group_ids` through the shared
unordered multi-select group selector (GS rules, §11) instead of freeform text. The plan
list MUST render `group_ids` as group-name badges; an empty array renders the localized
unrestricted label.

BP-UI5. Every header cell and data cell in the plan table MUST center its content on the
horizontal and vertical axes. When a cell contains multiple text lines, the complete text
block MUST remain centered within the cell.

## 11. Groups Management Page and Shared Group Selector

### 11.1 Groups page

GP1. `/dashboard/groups` is an admin page (nav per DL6). It MUST list every registry row
from `GET /api/dashboard/groups` in the returned order and MUST render a skeleton
placeholder while the list is loading.

GP2. Each row MUST display: name, description (muted, truncated with title attribute),
a default-group badge when `is_default` is true, a user-selectable indicator, and
`sort_order`.

GP3. The page MUST offer create, edit, and delete actions bound to
`POST /api/dashboard/groups`, `PUT /api/dashboard/groups/{id}`, and
`DELETE /api/dashboard/groups/{id}`. Create/edit dialogs MUST expose exactly: name,
description, user-selectable switch, and sort-order number input.

GP4. The delete action for the default group MUST be disabled. Deleting any other group
MUST open a confirmation dialog that names the group and states the cascade consequences
(members move to the default group; keys/providers/plans drop the group).

GP5. Every mutation MUST apply an SWR optimistic update to the groups cache and roll back
on error with a toast showing the server message. After a successful create or update the
groups cache MUST be revalidated (badges resolve names from the groups cache, so no other
cache is stale). After a successful delete the groups cache MUST be revalidated together
with the users, tokens, providers, billing-plans, and current-user caches, because the
server-side deletion cascade rewrites group references in those entities.

GP6. Each group row MUST provide move-up and move-down buttons in the sort-order cell.
The first row's move-up button and the last row's move-down button MUST be disabled. Native
HTML drag-and-drop reordering MUST be available through a drag handle when the browser
matches `(pointer: fine)`; coarse-pointer users MUST retain the move buttons. A completed
move or drop MUST call `POST /api/dashboard/groups/reorder` with every group id in the new
visual order. While that mutation is pending, every group reorder control MUST be disabled.
The groups SWR cache MUST immediately show the new order with `sort_order` values equal to
the zero-based visual indices, MUST revalidate after success, and MUST restore the prior
cache snapshot and show the server error after failure.

### 11.2 Shared group selector (GS)

GS1. One shared selector component MUST serve users (single mode), API keys (ordered
multi mode), providers (unordered multi mode), and billing plans (unordered multi mode).

GS2. Options MUST come from the SWR cache of `GET /api/dashboard/groups`. While the list
is loading, the selector MUST render a skeleton control, not an empty option list.

GS3. Every option row MUST display the group name and, when non-empty, its description.
The default group option MUST carry a default badge.

GS4. Single mode MUST behave as an exclusive select and produce exactly one group id.

GS5. Multi mode MUST render selected groups as removable rows and unselected options as
add buttons. In ordered multi mode each selected row MUST show its 1-based position and
support pointer drag-and-drop reordering (framer-motion `Reorder`), and the emitted array
order MUST equal the visual row order. In unordered multi mode the emitted order is the
selection order and no position or drag affordance is shown.

GS6. When a selected id has no matching registry row (deleted concurrently), the chip MUST
render the raw id and remain removable.

GS7. The selector MUST NOT perform freeform text creation of groups. Group creation happens
only on `/dashboard/groups`.
