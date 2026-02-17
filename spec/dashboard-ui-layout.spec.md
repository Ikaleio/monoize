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

- provider-level fields: `name`, `provider_type`, `enabled`, `max_retries`
- compact model editor list with per-row controls for: downstream model, redirect target, multiplier, delete
- channel table: name, base URL, weight, enabled
- channel runtime health indicator: healthy/probing/unhealthy

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

PL13. Provider editor model section MUST include an explicit "Fetch Models" action that opens a model-diff selection dialog before insertion.

- Dialog MUST fetch upstream model list from `POST /api/dashboard/providers/{provider_id}/fetch-models`.
- Dialog MUST split entries into `new` and `existing` tabs.
- Dialog MUST allow selecting only `new` models for insertion.
- Confirming selection MUST append selected models with default `{ redirect: "", multiplier: "1" }` while preserving existing rows.

PL14. Provider model badges (overview and model-diff dialog) MUST display provider logo using model metadata (`models_dev_provider`) when available, with graceful fallback icon behavior when unavailable.

PL15. In provider editor model section, each model row MUST be rendered as a compact clickable model tag.

- Tag text format MUST be `<(provider-logo) model-id [multiplier, target]>`.
- Bracket details (`[multiplier, target]`) MUST use muted/gray text to indicate secondary information.
- Clicking a model tag MUST open an edit dialog for that row.
- Edit dialog MUST allow updating at least: `model`, `redirect`, `multiplier`.
- Edit dialog MUST include delete action for the selected model row.
- Clicking "Add Model" MUST open a draft model dialog without appending a row immediately.
- A new model row MUST be appended only when user confirms via dialog save action.
- Closing/canceling the add-model dialog without saving MUST NOT create an empty model row.

PL16. Model tag bracket details in provider card/editor MUST follow omission rules:

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

PL19. Model lists on the Providers page MUST use virtualized rendering (`react-virtuoso`) with embedded scrolling.

- Expanded provider-card model list MUST render through `Virtuoso`.
- Provider edit dialog model list MUST render through `Virtuoso`.
- Both containers MUST have bounded height and provide an internal vertical scrollbar.
- Virtualized model list presentation MUST remain compact stacked badges (multiple model badges per rendered row when width allows), not a forced single-column one-badge-per-row list.
- In both provider overview and provider edit dialog, model list container MUST keep symmetric top/bottom inner spacing so badge block appears visually centered and not top- or bottom-heavy.

PL20. Provider edit dialog channel list MUST use virtualized rendering (`react-virtuoso`) with embedded scrolling.

- Channel list MUST render through `Virtuoso`.
- Container MUST have bounded height and provide an internal vertical scrollbar.

PL21. Unpriced models on the Providers page MUST be visually highlighted at model-badge level.

- Unpriced check target MUST be `redirect` model when `redirect` is non-empty; otherwise the logical model key.
- A model is treated as unpriced when pricing metadata does not provide both input and output token prices for that target model.
- Unpriced model badges MUST use a yellow warning style distinct from normal model badges.

PL22. In the provider unsaved-changes confirmation dialog ("Save Changes?"), the "Discard" action MUST use destructive red hover styling.

## 3. Playground Page

PG-L1. `/playground` page MUST be accessible from the main navigation sidebar (below Token Management).

PG-L2. The page MUST follow standard dashboard layout patterns: `PageWrapper`, `text-3xl` heading, motion animations.

## 4. Token Management Page

AK1. API key create and edit dialogs MUST include a downstream transform editor bound to API key `transforms`.

AK2. API key downstream transform editor MUST follow the same interaction contract as PL7, PL8, PL9, and PL10.

AK3. API key transform edits MUST be scoped to the edited key only and MUST NOT mutate other keys.

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
