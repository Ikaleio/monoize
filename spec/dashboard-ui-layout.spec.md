# Dashboard UI Layout Specification

## 0. Status

- Product name: Monoize.
- Scope: layout and key interaction requirements under `/dashboard/*`.

## 1. Global Layout

DL1. Desktop (`lg` and above) MUST render:

- left sidebar navigation
- main content area

DL2. Top header bar MUST NOT be rendered.

DL3. User/account menu MUST be anchored at sidebar bottom.

DL4. Mobile (`< lg`) MUST render sidebar via left sheet menu.

DL5. Sidebar main navigation (always visible to authenticated users) MUST include exactly:

- `/dashboard`
- `/dashboard/tokens`
- `/dashboard/logs`
- `/dashboard/playground`

DL6. Sidebar admin navigation group (visible only when user role is `admin` or `super_admin`) MUST include exactly:

- `/dashboard/providers`
- `/dashboard/models`
- `/dashboard/users`
- `/dashboard/admin-settings`

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

- provider-level fields: `name`, `enabled`, `max_retries`
- channel master list: name, type, base URL, weight, enabled, model count, and runtime health
- selected Channel detail editor with per-model controls for: logical model, redirect target, multiplier, and delete
- channel runtime health indicator: healthy/probing/unhealthy

PL4.0. Provider detail/editor MUST NOT render or maintain a Provider-level model selector or Provider-level model editor.

PL4.1. Provider detail/editor MUST place the provider `enabled` switch in the top title row, right-aligned from the provider editor title.

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

PL9. Provider transform config dialog MUST:

- edit `models` glob filters as string list (`*` and `?` supported)
- edit transform `config` using schema-driven fields from `/api/dashboard/transforms/registry`
- block save when schema validation fails

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
- The complete popover list MUST include channel type, enabled state, unpriced warning, and each provider group badge represented by the provider.

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
- Newly selected model IDs MUST receive default `{ redirect: null, multiplier: 1 }` entries.
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

PL19. Model badge lists on the Providers page MUST use a collapsed badge collection.

- Expanded provider-card model lists MUST render no more than 3 preview model badges before a `+N` overflow badge.
- Provider edit dialog model lists MUST render direct clickable model tags when the model count is at most 3.
- Provider edit dialog model lists MUST render a collapsed badge collection when the model count is greater than 3.
- The collapsed preview row MUST NOT wrap.
- Long model badge text in the collapsed preview MUST truncate with an ellipsis.
- Hovering, focusing, or clicking the collapsed preview MUST open a small popover containing the complete model badge list.
- Complete model badges in the popover MUST expose the same unpriced highlighting as the preview badges.
- Complete model badges in the provider edit dialog popover MUST keep edit and delete controls available for each model row.
- Complete model badge popovers SHOULD use a width up to `min(44rem, viewport width minus 2rem)` before horizontal scrolling is required.
- If a complete model badge popover contains more than 8 items, it MUST show at most 8 items per page and render previous/next page controls.
- Page controls MUST clamp at the first and last page and MUST NOT close the popover when clicked.
- The provider overview and provider edit dialog model list containers MUST keep symmetric top/bottom inner spacing so the badge block appears visually centered and not top- or bottom-heavy.

PL20. Provider edit dialog channel list MUST use virtualized rendering (`react-virtuoso`) with embedded scrolling.

- Channel list MUST render through `Virtuoso`.
- Container MUST have bounded height and provide an internal vertical scrollbar.

PL21. Unpriced Channel model entries on the Providers page MUST be visually highlighted at model-badge level.

- Unpriced check target MUST be `redirect` model when `redirect` is non-empty; otherwise the logical model key.
- A model is treated as unpriced when pricing metadata does not provide both input and output token prices for that target model.
- A pricing value of `0` MUST be treated as present pricing metadata, not as missing metadata.
- Unpriced model badges MUST use a yellow warning style distinct from normal model badges.

PL21a. `GET /api/dashboard/providers` MAY aggregate `unpriced_model_ids` across Channels for the Provider card. The count MUST deduplicate logical model IDs, while Channel detail highlighting MUST evaluate the selected Channel entry redirect independently.

PL22. In the provider unsaved-changes confirmation dialog ("Save Changes?"), the "Discard" action MUST use destructive red hover styling.

PL23. Provider channel edit dialog MUST expose channel-level passive breaker override fields with empty value meaning "inherit global setting":

- `passive_failure_threshold_override`
- `passive_cooldown_seconds_override`
- `passive_window_seconds_override`
- `passive_rate_limit_cooldown_seconds_override`

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
- `monoize_request_capture_retention_days` (integer >= 1, default 1)

ST3. Settings UI MUST perform optimistic update and persist via existing settings save flow; persisted values MUST be reflected after reload.

ST4. `/dashboard/admin-settings` MUST include a global transform editor bound to `GET/PUT /api/dashboard/settings` field `global_transforms`.

ST5. The global transform editor MUST follow the same interaction contract as PL7, PL8, PL9, and PL10, but its option list MUST be filtered to transforms whose registry metadata includes `global` in `supported_scopes`.

PG-L1. `/playground` page MUST be accessible from the main navigation sidebar (below Token Management).

PG-L2. The page MUST follow standard dashboard layout patterns: `PageWrapper`, `text-3xl` heading, motion animations.

## 4. Token Management Page

AK1. API key create and edit dialogs MUST include a downstream transform editor bound to API key `transforms`.

AK2. API key downstream transform editor MUST follow the same interaction contract as PL7, PL8, PL9, and PL10.

AK3. API key transform edits MUST be scoped to the edited key only and MUST NOT mutate other keys.

AK3a. API key transform editor option list MUST be filtered by transform scope metadata returned from `/api/dashboard/transforms/registry`.

- The editor MUST show only transforms whose `supported_scopes` includes `api_key`.
- The editor MUST continue filtering by `supported_phases` within the API-key-scoped subset.
- Transforms not available to API keys MUST be hidden from the add-transform selector instead of being shown and rejected after selection.

AK3b. Backend API key persistence and validation MUST accept every transform whose registry metadata advertises `supported_scopes` including `api_key`, including `reasoning_content_delta` for response-phase rules.

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

AK11. In the `/dashboard/tokens` list table, the API key name and allowed-group badge collection MUST render in a single non-wrapping inline row inside the name cell.

- The allowed-group badge collection MUST remain adjacent to the API key name and MUST NOT move below the name.
- If the inline row exceeds the available viewport width, the table container MUST handle overflow through horizontal scrolling.

## 5. Dashboard Home Page

DH1. `/dashboard` MUST render a dark themed overview shell containing exactly 3 visual rows:

- row A: greeting/title block only (no action controls);
- row B: 4 overview cards in desktop (`xl` and above), 2 columns in tablet (`md` to `< xl`), and 1 column in mobile (`< md`);
- row C: analysis area where the left panel takes 2 columns and the right panel takes 1 column on desktop; both stack vertically on mobile.

DH2. Each overview card in row B MUST contain:

- two metric rows (`label + value`);
- compact metric rows with no embedded chart and no decorative metric icons.
- card section title MUST be one typographic step smaller than row C section title.
- card header/content vertical padding MUST be compact to avoid excessive top whitespace.

DH3. The left analysis panel in row C MUST contain:

- a title row with section name;
- a tab strip with exactly 4 tab labels (`消耗分布`, `消耗趋势`, `调用次数分布`, `调用次数排行`);
- an analysis chart rendered through `@/components/ui/chart` using Recharts `BarChart`;
- analysis data MUST be computed from real request logs (`GET /api/dashboard/request-logs`) and MUST NOT use synthetic placeholder values.
- title and tab strip MUST be on the same row, with tab strip right-aligned.
- tab separators (`/`) MUST be visually separate from clickable tab label and MUST NOT be included in active underline.
- chart heading MUST be rendered as an `h2` element and MUST update with active tab label.
- chart heading and total summary text MUST share one horizontal row.
- in `调用次数排行` tab, category key MUST be provider-level key (provider name or provider id), not channel-level key.

DH3a. Dashboard home analysis queries MUST cover the complete latest 24-hour window:

- frontend MUST send `buckets=8` and `range_hours=24` to `GET /api/dashboard/analytics`;
- backend MUST compute `time_to = NOW()` and `time_from = time_to - 24h` for that analytics response;
- chart buckets MUST be generated from that same `[time_from, time_to)` window.

DH4. The right panel in row C MUST be an API information panel:

- when no provider data exists, it MUST show an explicit empty state (`暂无API信息`) and muted helper text;
- when provider data exists, it MUST show at least 1 provider summary row and 1 server/runtime summary row.

DH5. `/dashboard` loading state MUST show skeleton placeholders for row A, row B (4 cards), and row C (left and right panels) before stats/config data is ready.

DH6. `/dashboard` motion contract MUST use `framer-motion` and include:

- page entry fade/slide for row A and row C panels;
- staggered card entry for row B;
- hover lift effect for overview cards.

DH7. `/dashboard` MUST be resilient to config schema variance from `GET /api/dashboard/config`:

- UI MUST NOT throw runtime errors when optional keys (including `providers` and `model_registry`) are absent.
- Provider summary data for row B/row C MUST be sourced from `GET /api/dashboard/providers` when available.
- If `config.routing.providers_count` exists, it MAY be used as a fallback aggregate count.

DH8. `/dashboard` row C analysis panel MUST be responsive without horizontal overflow:

- analysis chart container MUST adapt to available width instead of enforcing a fixed minimum width.
- chart area MUST resize with card size.

DH9. In desktop layout, row C left analysis card and right API info card MUST have equal stretched row height.

DH10. In desktop layout, `/dashboard` MUST avoid page-level vertical overflow for normal data volumes:

- row C cards MUST consume remaining page space and keep equal height;
- overflowing content in row C panels MUST scroll within panel containers.

## 6. Users Page

UP1. In `/dashboard/users` list table, the role badge (`user.role`) MUST be rendered as a single-line badge. Badge text and icon MUST NOT wrap into multiple lines.

UP2. The role badge container in `/dashboard/users` table MUST enforce a fixed maximum height equal to one badge row and MUST use horizontal overflow (`overflow-x: auto`, `overflow-y: hidden`) when space is insufficient on narrow viewports.

UP3. The users table in `/dashboard/users` MUST allow horizontal scrolling on narrow viewports so role badges remain single-line instead of wrapping.

UP4. The users table body in `/dashboard/users` MUST use virtualized rendering via `react-virtuoso` (`TableVirtuoso`) instead of rendering all rows as plain DOM rows.

- Table header MUST be rendered via `fixedHeaderContent` (sticky header).
- Table body rows MUST be rendered via `itemContent` callback.
- Virtualized table container height MUST be `calc(100vh - 280px)` with a minimum height of `400px`.

UP5. In the `/dashboard/users` list table, the username text and allowed-group badge collection MUST render in a single non-wrapping inline row inside the user cell.

- If horizontal space is insufficient, the username text MAY truncate.
- The allowed-group badge collection MUST remain single-line and MUST NOT move below the username.

## 7. Token Management Page (UI)

AK4. The API keys table body in `/dashboard/tokens` MUST use virtualized rendering via `react-virtuoso` (`TableVirtuoso`) instead of rendering all rows as plain DOM rows.

- Table header MUST be rendered via `fixedHeaderContent` (sticky header).
- Table body rows MUST be rendered via `itemContent` callback.
- Virtualized table container height MUST be `calc(100vh - 280px)` with a minimum height of `400px`.
- Select-all checkbox MUST remain in the fixed header; per-row checkboxes MUST remain in `itemContent`.

AK5. API key create and edit dialogs in `/dashboard/tokens` MUST include both the existing legacy `group` text input and a distinct `allowed_groups` chip input.

AK6. The `allowed_groups` chip input MUST follow the same interaction contract as the provider/user group editors:

- freeform text entry;
- `Enter`, comma, and blur commit pending draft labels;
- selected labels render as removable chips;
- suggestion buttons are sourced from `GET /api/dashboard/groups`;
- suggestion buttons for labels already selected in the current draft MUST be hidden.

AK6a. Typed commits, chip removals, and suggestion clicks in the API-key `allowed_groups` editor MUST apply against the latest in-session chip draft. A typed commit MUST NOT resurrect labels the user has already removed from the current draft.

AK7. The API key `allowed_groups` helper text MUST explain that an empty array means the key inherits the owning user's `allowed_groups`. If the authenticated dashboard user payload exposes `allowed_groups`, the dialog MUST render that value as a non-authoritative hint only. The frontend MUST NOT block save with client-side subset validation.

AK8. If `POST /api/dashboard/tokens` or `PUT /api/dashboard/tokens/{key_id}` returns a validation error for `allowed_groups` subset rules, the frontend MUST surface the server-provided message in a toast and MUST keep the dialog open with the current draft state intact.
