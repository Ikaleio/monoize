# Dashboard UI Layout Specification

## 0. Status

- Product name: Monoize.
- Scope: layout and key interaction requirements under `/dashboard/*`.

## 1. Global Layout

DL1. Desktop (`lg` and above) MUST render:

- left sidebar navigation
- main content area

DL1a. On shell mount, the desktop sidebar MUST restore the state stored by DL1f, or start
expanded when no state is stored. A viewport width change within the desktop range MUST
NOT collapse the sidebar.

DL1b. The expanded desktop sidebar MUST use a `16rem` width. Its brand row MUST place a
collapse button to the right of the Monoize brand link. Activating this button MUST
collapse the sidebar to a `4rem` icon rail.

DL1c. The collapsed desktop sidebar MUST render the 32 px Monoize brand surface as an
expand button instead of a dashboard link. Activating this button MUST expand the
sidebar. Pointer hover or keyboard focus MUST replace the visible Monoize mark with a
lucide expand-sidebar icon while the button remains in that state.

DL1d. The collapse and expand buttons MUST expose localized accessible names. The
mobile sheet behavior in DL4 MUST remain independent of the desktop collapse state.

DL1e. The dashboard shell MUST use the shadcn/ui Radix `SidebarProvider` and
`Sidebar` components with `variant="sidebar"` and `collapsible="icon"`.
`SidebarHeader`, `SidebarContent`, `SidebarGroup`, `SidebarMenu`, and `SidebarFooter`
MUST compose the sidebar. Navigation links MUST use `SidebarMenuButton` with
`isActive` and `aria-current="page"` for the selected route.

DL1f. `SidebarProvider` MUST own desktop and mobile visibility. The mobile breakpoint
MUST equal `1024px`. Every desktop collapse or expand MUST write `collapsed` or
`expanded` to localStorage key `monoize-sidebar`. The shell MUST read that key on mount
and start collapsed only when the stored value is `collapsed`. Navigation within the
shell, including between `/dashboard/*` and `/settings`, MUST preserve desktop state.
Ctrl+B or Meta+B MUST toggle the sidebar for the current viewport.
Width transitions MUST take 200 ms and MUST stop when reduced motion is requested.

DL2. Top header bar MUST NOT be rendered.

DL3. User/account menu MUST be anchored at sidebar bottom.

DL3a. The expanded sidebar account trigger MUST show the authenticated user's prepaid
balance on the second line (localized unlimited label when `balance_unlimited` is true;
otherwise `balance_nano_usd` formatted as USD with 2 fractional digits). The collapsed
sidebar MUST expose the same summary through the account-trigger tooltip. This display
MUST use the session user object and MUST NOT call `GET /api/dashboard/billing-plans`.

DL3b. The account dropdown content MUST be a compact `18rem`-wide (`w-72`) menu with
exactly these sections in order, separated by menu separators:

1. identity header,
2. prepaid-balance block,
3. active-plan block when an active subscription exists,
4. live-usage block,
5. settings item,
6. the theme pill toggle and the language submenu,
7. sign out.

The dropdown MUST use the shared DropdownMenu (Radix) component so keyboard and
dismissal contracts are unchanged, and lucide icons only (no emoji glyphs).

DL3c. The identity header MUST show the Gravatar avatar (same `getGravatarUrl` source as
the trigger, username-initial fallback), the username (truncated), the localized role
label (`roles.{role}`), and the email truncated when present.

DL3d. The prepaid-balance block MUST be sourced from the session user object only and MUST
NOT call `GET /api/dashboard/billing-plans`. It MUST render:

- a balance row: localized unlimited label when `balance_unlimited` is true, otherwise
  `balance_nano_usd` formatted as USD with 2 fractional digits via `formatNanoUsd`.
  The signed nano-USD formatter MUST render negative balances without throwing. The
  sidebar MUST NOT depend on the runtime JSON type of `balance_usd`.

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

DL3f. The actions MUST be exactly: a settings item navigating to `/settings`; the theme
pill toggle with the options light, dark, and system, rendered as menu radio items so they
are arrow-key reachable; a `DropdownMenuSub` language submenu that lists the supported
languages by native name in a radio group (below the `lg` breakpoint, where the menu sits in
the mobile sheet and a side submenu would overflow the viewport, the same radio group MUST
render inline under a "language" label instead); and a sign-out item. The theme options MUST NOT
close the menu when selected. While the current route is `/settings`, the account trigger
MUST set `aria-current="page"`.

DL3g. The mobile sheet sidebar (DL4) MUST render the same account dropdown component,
so DL3b through DL3f hold on both desktop and mobile.

DL4. Mobile (`< lg`) MUST render sidebar via left sheet menu. The menu trigger MUST expose
a localized accessible name. The Sheet content MUST contain a localized accessible title
and description even when both strings are visually hidden.
Selecting a navigation link or the brand link MUST close the mobile sheet.
The mobile sheet MUST use a `16rem` width and retain its visible close button.

DL5. Sidebar main navigation (always visible to authenticated users) MUST include exactly
these items in this order, without a group label:

- `/dashboard`
- `/dashboard/tokens`
- `/dashboard/wallet`
- `/dashboard/logs`
- `/dashboard/playground`
- `/dashboard/marketplace`

The `/dashboard/wallet` page content is governed by
`spec/recharge-system.spec.md` §10.

DL5a. Every navigation label MUST equal the `h1` of its page in every locale, except
`/dashboard`, whose `h1` is the greeting defined by the home-overview spec. The labels
are (zh / en): 仪表盘 / Dashboard, API 密钥 / API Keys, 钱包 / Wallet, 请求日志 / Request
Logs, Playground / Playground, 模型广场 / Model Marketplace, 系统仪表盘 / System
Dashboard, Provider / Providers, 分组 / Groups, 自定义变换 / Custom Transforms, 模型定价 /
Model Pricing, 订阅套餐 / Subscription Plans, 支付管理 / Payments, 用户管理 / Users, 系统设置 /
System Settings. The account page `/settings` is titled 账户设置 / Account Settings. Two
navigation items MUST NOT share a label in any locale.

DL6. Sidebar admin navigation group (visible only when user role is `admin` or `super_admin`) MUST be labeled `nav.admin` and MUST include exactly these items, in this order:

- `/dashboard/admin`
- `/dashboard/providers`
- `/dashboard/models`
- `/dashboard/plans`
- `/dashboard/payments`
- `/dashboard/users`
- `/dashboard/groups`
- `/dashboard/custom-transforms`
- `/dashboard/admin-settings`

The `/dashboard/custom-transforms` page content is governed by
`spec/custom-js-transforms.spec.md` §11 (CJS-UI-1 through CJS-UI-7).
The `/dashboard/payments` page content is governed by
`spec/recharge-system.spec.md` §11.

DL6a. Every DL6 route MUST be a child of the `RequireAdmin` layout route in
`frontend/src/App.tsx`. For a user whose role is neither `admin` nor `super_admin`,
`RequireAdmin` MUST render the shared no-permission state: the page title from the route
handle as `h1`, `guards.forbiddenTitle`, `guards.forbiddenDescription`, and a
`guards.backToDashboard` link to `/dashboard`. The page component MUST NOT mount, so it
MUST NOT send requests or render admin actions.

DL6b. A path that matches no route MUST render `NotFoundPage` inside the dashboard shell
for a signed-in user, with `errors.notFoundTitle` as `h1` and a link to `/dashboard`. It
MUST NOT redirect. `/` MUST redirect to `/dashboard`.

DL6c. When no user is signed in, the shell MUST redirect to `/login` with `replace` and
navigation state `{ from: location }`, where `location` is the requested location
including search and hash. The login page returns to that location after sign-in
(`dashboard-session-authentication.spec.md`).

DL6d. The app MUST use a React Router data router (`createBrowserRouter` and
`RouterProvider` in `frontend/src/main.tsx`). Each page route MUST declare
`handle.titleKey`, the i18n key of its navigation label (DL5a).

DL7. In desktop layout (`lg` and above), `/dashboard/*` pages MUST use single-pane vertical scrolling:

- viewport-level/document-level vertical scroll MUST be disabled for dashboard shell;
- left sidebar pane MUST remain fixed in viewport and MUST NOT move during right-pane content scroll;
- right main content pane MUST be the only vertical scroll container when page content overflows viewport height.

DL7a. The right main pane MUST receive a definite remaining viewport height (`flex-1` and `min-height: 0` inside the `h-dvh` shell). Fill-height pages such as request logs MAY consume that height with an internal scroll region. Other pages MAY grow and scroll inside the same main pane.

DL7c. The right main pane MUST span the full width between the sidebar and the right viewport edge, so its vertical scrollbar renders at the right viewport edge. The horizontal page padding (`1.5rem`, `2rem` at `lg` and above) MUST be inside the main pane. Page content MUST render inside a child of the main pane that is centered and limited to `max-w-6xl` (72rem). That child MUST pass the definite height of DL7a to fill-height pages.

DL7b. The shell MUST expose the right main pane element to pages through
`DashboardScrollParentContext` from `frontend/src/lib/dashboard-scroll.ts`, provided by `frontend/src/pages/layout.tsx`. The context value
MUST be `null` until the element mounts. A virtualized page list MUST use this element
as its scroll parent and MUST NOT create a second vertical scroll container.

DL8. Site identity. The expanded sidebar brand link MUST show the two-line lockup "Monoize"
over "Console". `document.title` MUST be `{page title} · {site name}`. The site name is the
trimmed public setting `site_name`, or `Monoize` while it loads or when it is empty. The page
title is the translation of the deepest matched route's `handle.titleKey`. `document.title`
MUST update on every route change and language change. The `/login` route uses `auth.signIn`.
`document.documentElement.lang` MUST equal the active i18n language on startup and after
every language change.

DL9. Render errors. Inside the shell, the route `errorElement` MUST be
`RouteErrorBoundary`, so a page render error replaces only the content area and the
sidebar and account menu stay usable. It MUST show `errors.pageTitle`,
`errors.pageDescription`, a reload button, and a `guards.backToDashboard` link. The root
route MUST also have `RouteErrorBoundary` as `errorElement`, so an error outside the shell
never leaves a blank document.

## 2. Providers Page

PL1. `/providers` page MUST be provider-centric.

PL2. Provider list MUST display, at minimum:

- provider name
- enabled state
- model count
- channel count
- routing position as a 1-based ordinal ("第 1 位") and the retry policy in words ("尝试全部 Channel" when `max_retries = -1`, otherwise "最多 N 次" with N = `max_retries + 1`). The raw 0-based `priority` and the value `-1` MUST NOT be displayed.

PL2a. The page description MUST state the routing model: Providers are tried in list order; within a Provider a Channel is chosen at random by weight and failures fall back. When any Provider has a non-empty `group_ids`, it MUST add that group priority applies before this order.

PL3. Provider list MUST support reordering by drag-and-drop AND by move-up/move-down buttons, and persist order through `/api/dashboard/providers/reorder`.

- Only the grip handle is draggable. The handle is a focusable button with an accessible name. Text inside the card (e.g. Base URL) MUST remain selectable.
- Reordering MUST update the list optimistically. A second move issued before the first request completes MUST start from the optimistic order. A failed request MUST show a localized error toast and revalidate the list from the server.

PL4. Provider detail/editor MUST display:

- provider-level fields: `name`, `enabled`, `max_retries`, and `group_ids` edited through the shared unordered multi-select group selector (GS rules, §11); freeform group text entry MUST NOT be offered
- channel master list rows: name, type, base URL, model count, `权重 N`, and the effective status badge (PL18b)
- selected Channel detail editor with per-model controls for: logical model, redirect target, multiplier, and delete
- channel runtime health indicator: healthy/probing/unhealthy

PL4.0. Provider detail/editor MUST NOT render or maintain a Provider-level model selector or Provider-level model editor.

PL4.1. Provider detail/editor MUST place the provider `enabled` switch in the top title row, right-aligned from the provider editor title. Because the switch edits the draft, its visible label (shown at `sm` and wider) MUST read "保存后启用" when on and "保存后禁用" when off. When the editor opens, initial focus MUST go to the Provider name input in create mode and to the dialog title in edit mode; it MUST NOT land on the switch.

PL4.3. Create mode MUST open on the `provider` section. Edit mode MUST open on the `channels` section. When the draft has exactly one Channel, the mobile layout (`< lg`) MUST open that Channel's detail pane directly.

PL4.4. The routing section MUST show, read-only, "本 Provider 当前排第 N 位，可在列表中调整" (create mode: "新 Provider 会排在列表末尾").

PL4.5. `max_retries` MUST be edited as a two-option control: "尝试全部可用 Channel" (stores `-1`, default) or "指定次数" with a positive integer input (stores `N - 1`, N ≥ 1). All numeric fields of the editor MUST use the shared `NumberField`: typing MUST NOT be rewritten while the field has focus, and invalid values MUST show an inline field error instead of being coerced.

PL4.6. The request-timeout override hint MUST read "全局默认：N ms · 修改", where "修改" links to `/dashboard/admin-settings?setting=monoize_request_timeout_ms`.

PL4.7. The editor MUST expose Provider-level `active_probe_enabled_override`, `active_probe_interval_seconds_override`, `active_probe_success_threshold_override` and `active_probe_model_override` in the routing section under "健康检查". Empty values serialize as `null` (inherit global).

PL4.2. At mobile widths, the Provider editor title row MUST reserve a dedicated right-side touch area for the dialog close button. The Provider `enabled` switch and the close button hit areas MUST NOT overlap, and each control MUST remain independently clickable.

PL5. API keys/secrets for channels MUST never be shown after save (write-only behavior).

PL6. Provider detail/editor MUST include an upstream transform editor bound to provider `transforms`.

PL7. Provider upstream transform editor MUST render exactly two independent compact chains:

- request-phase chain (`phase = request`)
- response-phase chain (`phase = response`)

PL8. Each provider transform chain MUST support:

- append transform from transform registry filtered by supported phase
- reordering within the same phase chain by BOTH drag-and-drop and per-item move-up/move-down buttons (`transform-config-ui.spec.md` governs the details)
- per-item delete
- per-item enabled toggle
- per-item config button that opens a config dialog

PL8a. At viewport widths below `640px`, each transform chain header MUST stack its title and the add combobox vertically; the combobox trigger MUST shrink to the available width without creating horizontal overflow (`transform-config-ui.spec.md` TCU-10a).

PL8b. The add-transform selector options and every rendered chain item MUST display the transform's localized registry `name` resolved per `transform-config-ui.spec.md` TCU-2, with the canonical `type_id` rendered as secondary monospace text. A chain item whose `type_id` is absent from the registry MUST fall back to displaying the raw `type_id`.

PL9. Provider transform config dialog MUST:

- edit `models` glob filters as string list (`*` and `?` supported)
- edit transform `config` using schema-driven fields from `/api/dashboard/transforms/registry`
- block save when schema validation fails

PL9b. Config field rendering, typing, and null/omission semantics are defined by `transform-config-ui.spec.md` TCU-3 through TCU-9. A config schema property that has neither `type` nor `enum` is a JSON-valued field and MUST render as the typed value editor defined by TCU-7; it MUST NOT render as a bare JSON text input.

PL9c. When the transform config dialog opens, field initialization MUST follow `transform-config-ui.spec.md` TCU-8. The dialog MUST NOT initialize an absent field as the text `null`.

PL9d. Saving the dialog MUST produce config values per `transform-config-ui.spec.md` TCU-9. Empty optional fields MUST be omitted from the saved config and MUST NOT fail save as invalid JSON.

PL10. If a provider transform item type is not present in transform registry, editor MUST:

- keep item visible with unknown marker per `transform-config-ui.spec.md` TCU-12a (a `js:` id renders the warning badge "已停用或已删除的自定义变换"; other ids render the destructive unknown badge)
- allow reorder/delete/toggle-enabled
- render `config` as read-only JSON
- preserve unknown item fields on save unless user deletes the item

PL11. In the selected Channel detail, when the `base_url` value ends with `/v1` (or `/v1/`), the UI MUST show an inline warning under the field: "末尾的 /v1 通常会由适配器自动追加，可能导致路径重复。" with a "移除 /v1" action that replaces the value with the value without the trailing `/v1`. The warning MUST NOT open a dialog, MUST NOT block save, and disappears when the value no longer ends with `/v1`.

PL12. Provider list card header MUST place provider metadata and controls in a compact single-row layout on desktop:

- metadata block MUST include `priority` and `max_retries` aligned near action buttons;
- provider enable switch MUST be colocated in the header action zone;
- edit/delete/reorder controls MUST remain available without expanding card height;
- the expand/collapse control MUST be a real `<button>` covering the chevron and the Provider icon tile, with `aria-expanded` and `aria-controls` pointing at the expanded region, so keyboard users can expand a card. The Provider name MUST remain clickable to expand the card.

PL12c. The list Provider enable switch applies immediately (no confirmation dialog). Disabling MUST show a toast "已禁用 {name}，新请求将不再路由到它" with an "撤销" action that re-enables it; enabling MUST show a toast "已启用 {name}" with an "撤销" action. A failed toggle MUST revert the switch and show a localized error toast. A disabled Provider card MUST show a secondary "已禁用" badge next to its name.

PL12d. Deleting a Provider MUST show a success toast. When the Provider's `live_usage.rpm > 0`, the delete confirmation MUST add "最近 1 分钟有 N 个请求".

PL12e. Deep link: `/dashboard/providers?provider=<provider_id>&channel=<channel_id>` MUST expand that Provider card, scroll it into view, and highlight the Channel row (when `channel` is given and present) for 2 seconds. Unknown ids are ignored. The parameters are removed from the URL (`replace`) after they are applied once.

PL12b. Provider headers MUST show RPM and TPM as two left-aligned rows immediately before the enable switch on desktop. Every card MUST reserve the same metric-column width. Labels and values MUST share consistent starting positions across cards. Both labels and values MUST use secondary typography: `font-mono`, `text-sm`, normal weight, and `text-muted-foreground`. Values MUST use tabular numerals. Do not add a card or badge. On narrow screens, place metrics below Provider identity without horizontal overflow or loss of actions. Initial list loading MUST use the existing page skeleton. Missing usage MUST render an em dash, not zero. Failed refreshes MUST retain cached values and preserve the existing list error notice.

PL12a. Provider list card header metadata badges MUST render through a collapsed badge collection when the number of metadata badges is greater than 3.

- The header badge preview MUST render no more than 3 badges before a `+N` overflow badge.
- The preview row MUST NOT wrap.
- The complete popover list MUST include channel type, enabled state, one model-attention badge when any model has a breaker or pricing warning, and one badge per provider `group_ids` entry showing the group name resolved from `GET /api/dashboard/groups` (raw id fallback for unknown ids).
- The model-attention badge count MUST deduplicate logical model IDs. It MUST use destructive styling when any model has `availability_status = unavailable`, and warning styling otherwise.

PL13. The selected Channel model section MUST include an explicit "Fetch upstream" action that opens a model-diff selection dialog before insertion.

- Dialog MUST fetch upstream model list from `POST /api/dashboard/fetch-channel-models` with the current Channel `provider_type` and `base_url`.
- Dialog MUST show the returned `compact_scheme`: `same_model` means remote compact uses the conversation model; `openai_compact_sibling` means remote compact uses `{model}-openai-compact` siblings present in the fetched list.
- If the current Channel is an existing saved Channel and the API key input is empty, Dialog MUST pass `provider_id` and `channel_id` instead of requiring API key entry.
- If the current Channel is new or has no saved `channel_id`, Dialog MUST require a non-empty API key before opening the picker.
- If the API key input is non-empty, Dialog MUST pass that value so unsaved key edits are used for the fetch request.
- Dialog MUST place the "Fetch upstream" action in the Supported Models action row.
- Dialog MUST split entries into `new` and `existing` tabs. Both tabs list model IDs sorted ascending.
- Dialog MUST offer "全选当前结果" and "清除当前结果" next to the search box; they check or uncheck exactly the models in the current tab that match the search.
- The confirm button MUST read "应用（+A / −R）" where A is the number of checked models not in the Channel and R the number of Channel models unchecked. With no change it is disabled and the footer states "未做任何更改".
- The `existing` tab MUST state that unchecking a model removes it from the Channel.
- A fetch failure MUST render an error state in the list area ("无法从上游获取模型", localized error, retry action) and MUST NOT render an empty-state text. The empty states are distinct: "上游没有新模型" (fetched list has no new model), "没有已有模型" (existing tab empty), "没有匹配的模型" (search filtered everything out).
- When any listed model is unpriced, a legend line MUST explain the warning style and link to `/dashboard/models?tab=unpriced`.
- The dialog MUST be a column flex layout: header and footer fixed, the list `min-h-0 flex-1` with internal scrolling, so footer buttons are never clipped at 390px width.
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
- Edit dialog MUST allow updating at least: `model`, `redirect`, `multiplier`. The `multiplier` field MUST show help stating its formula and target: final charge = model price × this Channel multiplier × group multiplier (applied to the balance charge); `0` makes requests on this Channel free.
- Edit dialog MUST include delete action for the selected model row.
- Clicking "Add Model" MUST open a draft model dialog without appending a row immediately. In add mode the model field is a multi-line input that accepts several model IDs separated by newlines or commas; save appends one row per new trimmed, de-duplicated ID with the dialog's redirect (only when exactly one ID is entered) and multiplier. IDs already in the Channel are reported as an inline error and nothing is appended. Edit mode keeps a single-line field.
- A new model row MUST be appended only when user confirms via dialog save action.
- Closing/canceling the add-model dialog without saving MUST NOT create an empty model row.
- Editing an existing model row MUST operate on a draft copy. Closing/canceling the edit dialog without saving MUST leave the underlying row unchanged.
- The provider editor UI MUST reject duplicate logical model names before save or final submit. It MUST NOT silently overwrite an earlier model row when two rows use the same trimmed model name.
- The model section MUST offer a "选择" mode: tags become checkable, with a filter box, "全选" (of filtered tags), and bulk actions "删除所选" and "设置倍率" (applies one valid multiplier to all selected rows). Leaving the mode clears the selection.
- The model edit dialog MUST pass `dirty` to the shared dialog guard so Esc, outside click and close ask before discarding changes.

PL16. Channel model tag bracket details in provider card/editor MUST follow omission rules:

- multiplier fragment MUST be omitted when multiplier equals `1x`;
- redirect fragment MUST be omitted when redirect target equals the model itself (or is empty);
- bracket section MUST be omitted entirely when both fragments are omitted.

PL17. Provider edit dialog initialization MUST be resilient to fast-open timing.

- On open in edit mode, UI MUST fetch fresh provider detail (`GET /api/dashboard/providers/{id}`) using SWR.
- Until detail hydration is ready, UI MUST render skeleton placeholders instead of empty editable controls.
- If detail fetch fails, UI MAY fallback to list-sourced provider snapshot instead of requiring close/reopen.
- The form MUST be hydrated exactly once per open (keyed by Provider id and the open event). After hydration, no revalidation (reconnect, focus, interval, mutate) may modify the form draft, the dirty state, the selected section, or the selected Channel. The detail hook MUST disable focus/reconnect/stale revalidation and polling while the editor is open.
- Hydrated Channel models are listed in ascending model ID order.
- Runtime health shown in the editor is read-only and MAY come from the latest list data; it is not part of the draft.

PL18. In expanded provider card overview, channel runtime list row spacing MUST be deterministic.

- Each rendered channel row MUST use a minimum row height of `40px`.
- Virtual list container height MUST be computed as `min(channel_count * 40, 190)`.
- The row height constant used by the virtual list and the row element style MUST be the same value to prevent visible trailing blank space.
- The runtime health label, Channel name, and Channel Base URL MUST share one visual vertical center at every viewport width. The row action group MUST remain vertically centered independently.
- Weight text MUST read "权重 W（P%）" where P is W divided by the sum of weights of enabled Channels with weight > 0 in that Provider, rounded to an integer; P is omitted for a disabled or zero-weight Channel. Model count reads "N 个模型".
- Each row MUST include a Channel enable switch with an accessible name containing the Channel name. Toggling it saves immediately through `PUT /api/dashboard/providers/{provider_id}/channels/{channel_id}/enabled` (CM-CH-EN-1), updates optimistically, and on failure reverts and shows a localized error toast.
- The detail-test button MUST have the visible text "选择模型测试" (or an accessible name with the same text when icon-only) and the quick test button MUST state the model it will use in its tooltip ("使用 {model} 测试"), where the model is resolved like the server (Channel `active_probe_model_override` → Provider `active_probe_model_override` → global `monoize_active_probe_model` → smallest Channel model ID).

PL18b. A Channel shows exactly one effective status badge, chosen by priority: "已禁用" (secondary, `enabled = false`) > "不接收流量" (secondary, `weight = 0`) > "不健康" (destructive) > "探测中" (warning) > "健康" (success). Disabled and zero-weight Channels MUST NOT show health. The same rule applies to the list rows and the editor master list; unsaved Channels in the editor show "未保存".

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
- The Virtuoso root MUST have a non-zero definite height on the first paint. A parent of that root MUST NOT use `overflow: auto` as the vertical scroll container.
- The Channel list pane MUST remain a column flex container at every viewport where the list is visible. Visibility classes MUST NOT replace `display: flex` with `display: block`.
- Each draft Channel MUST render as a selectable row. A non-empty draft MUST NOT show only the list header.

PL21. Provider overview model entries MUST combine availability and pricing warnings at model-badge level without combining their meanings.

- Unpriced check target MUST be `redirect` model when `redirect` is non-empty; otherwise the logical model key.
- A model is treated as unpriced when pricing metadata does not provide both input and output token prices for that target model.
- A pricing value of `0` MUST be treated as present pricing metadata, not as missing metadata.
- An unavailable model badge MUST use destructive red styling.
- A partially unavailable or unpriced model badge MUST use yellow warning styling.
- When both conditions apply, availability severity MUST determine the badge color.
- The badge detail overlay MUST render separate availability and pricing sections.

PL21a. `GET /api/dashboard/providers` MAY aggregate `unpriced_model_ids` across Channels for the Provider card. The count MUST deduplicate logical model IDs, while Channel detail highlighting MUST evaluate the selected Channel entry redirect independently.

PL22. Closing the Provider editor with unsaved changes MUST open a confirmation dialog titled "放弃未保存的修改？" with three actions: "继续编辑" (closes only the confirmation and returns focus to the element focused before it opened), "放弃" (destructive red hover styling; closes the editor without saving) and "保存并关闭" (runs the same validation and save as the footer Save; on success closes the editor, on validation failure closes the confirmation and navigates to the invalid field per PL26). The footer status text MUST read "有未保存的更改" when dirty, "尚未保存" for a clean create draft, and "无改动" for a clean edit draft.

PL23. The selected Channel detail MUST expose channel-level passive breaker override fields with empty value meaning "inherit global setting":

- `passive_failure_threshold_override`
- `passive_cooldown_seconds_override`
- `passive_window_seconds_override`
- `passive_rate_limit_cooldown_seconds_override`
- `active_probe_enabled_override`, `active_probe_model_override`, `active_probe_interval_seconds_override`, `active_probe_success_threshold_override`

PL23b. The selected Channel detail MUST expose these Channel affinity overrides. The inherit option or an empty numeric value MUST serialize as null:

- `affinity_enabled_override`
- `affinity_idle_ttl_seconds_override`
- `affinity_failback_mode_override`
- `affinity_failback_delay_seconds_override`

PL23c. The Channel editor MUST describe `"sticky"` as retaining an eligible bound Channel and `"prefer_higher_priority"` as retrying an earlier eligible Provider after the configured delay.

PL23d. The Channel editor MUST expose `websocket_supported` as unknown / supported / not supported in the Connection section immediately after Channel `provider_type`. It MUST NOT place that control under routing affinity. When `provider_type` is `responses`, it MUST show a Detect WebSocket button that calls `POST /api/dashboard/probe-channel-websocket` with the current draft `provider_type`, `base_url`, optional draft API key, and stored `provider_id`/`channel_id` when present. A successful `supported` value MUST update only the draft field. Cancel MUST discard that draft value. Handshake failure and dashboard transport failure MUST write `false` into the draft field. The Detect action MUST treat a dashboard HTTP `502` as `supported=false` and MUST NOT leave the editor in an uncaught error state.

PL23e. When Channel `provider_type` is `chat_completion`, the Channel editor MUST show the `chat_dialect` Select in the Connection section immediately after `provider_type` (`channel-management.spec.md` CP-FE-7b). For every other type, the editor MUST NOT show it. The Provider Protocol override editor applies the same rule per row (`channel-management.spec.md` CP-FE-7c).

PL23a. Channel edits in the workbench modify the Provider draft directly; no Channel edit dialog exists. "Add Channel" appends an empty draft row and selects it. No Channel change is persisted until the Provider is saved. "Duplicate Channel" copies the selected Channel's draft fields except `id`, `api_key` and runtime health; the copy shows the "未保存" badge and requires a new API key (PL26).

PL24. While the provider editor dialog is open, interaction with a child dialog that belongs to the provider editor MUST NOT be treated as an outside click of the provider editor dialog. This rule applies to at least:

- the unsaved-changes confirmation dialog;
- the model picker dialog;
- the model edit dialog;
- the transform config dialog;
- the Channel test dialog.

Clicking an action button inside any such child dialog MUST execute only that child dialog action and MUST NOT open another unsaved-changes confirmation dialog through the parent provider editor outside-click handler.

PL25. Provider editor MUST use an explicit workbench information architecture.

- Desktop (`lg` and above) MUST render a Provider section rail, Channel master list, and selected Channel detail pane simultaneously.
- Mobile (`< lg`) MUST render one pane at a time. Selecting a Channel MUST open a full-width Channel editor with an explicit back action.
- Mobile save/cancel actions MUST remain reachable in a sticky bottom action bar.
- Primary connection and model controls MUST appear before breaker, probe, retry, transform, and protocol override controls.
- Channel advanced groups, in order after Supported Models: "路由亲和" (affinity overrides, `session_affinity_auto`, `proxy_url`, `extra_headers`), "健康检查与熔断" (PL23 fields). They MUST be collapsed by default and, when closed, MUST display a summary generated from the current values: "继承全局设置" when no field in the group is set, otherwise "N 项覆盖：" followed by up to two set fields as "label：value" (e.g. "冷却时间（秒）：120") joined by " · ", and "…" when more are set.
- The Channel proxy control MUST accept the URL formats in CP-INV-14 and explain how to supply credentials. It MUST mask the URL by default and provide a show/hide control. A collapsed summary MUST display at most the proxy scheme as the field value. Fetch Models and Detect WebSocket MUST submit the draft `proxy_url`, including `null` for an empty field.
- `extra_fields_whitelist` MUST be edited in the Provider basics section through the shared `ListField` (comma separated).
- The Channel weight field MUST use `NumberField` (integer, min 0) with the hint "同一 Provider 内按权重分配流量；0 表示不接收请求". When the value is 0 it MUST additionally show "权重为 0 时此 Channel 不接收请求".
- The Channel detail header MUST contain the duplicate, delete, and enable-switch controls in that order; the switch has the visible label "已启用"/"已禁用".
- The Channel detail header MUST include a "测试" button that opens the Channel test dialog (CP-FE-9) for the saved Channel. It is disabled with a tooltip "请先保存更改" while the draft is dirty or the Channel is unsaved.

PL26. Provider editor validation runs on Save and on "保存并关闭". The first error MUST switch to its section (and Channel, for Channel fields), focus the invalid control, render the message under the field with `FieldError` and set `aria-invalid` on it. "First" means on-screen order: Provider fields, then each Channel in list order with its fields top to bottom (name, Base URL, API key, models). While a numeric field holds invalid text, switching section, switching or adding/duplicating a Channel, and the mobile "back to Channel list" action MUST NOT happen; the editor focuses the invalid field instead, so the last valid value is never saved silently. The section rail/tabs and the Channel master list rows MUST show an error dot for sections/Channels containing errors until the next validation. Errors are cleared per field when that field changes. Validation rules: Provider name required; at least one Channel; at least one Channel has a model; each Channel needs a name and Base URL; a Channel without `id` needs an API key ("新 Channel 需要填写 API Key"); weight is an integer ≥ 0; numeric overrides are positive integers (failback delay ≥ 0); `extra_headers` is a JSON object; transform rules are valid (error names rule and field per `transform-config-ui.spec.md` and reveals the rule). Required fields (Provider name, Channel name, Base URL, API Key of a new Channel) MUST display a "必填" marker. Every field label MUST be bound to its control (`htmlFor`/`id`). Server errors MUST be shown through the shared localized API error mapping; when the error carries `param`, the editor maps it to the field when possible. Raw server messages MUST NOT be shown.

PL27. Terminology on the Providers page follows the dashboard glossary: "Provider" and "Channel" are never translated (no 服务提供者/提供者/通道/渠道). The page title is "Provider", the create button "添加 Provider", the test dialog title "Channel 连通性测试". UI text MUST NOT contain `null`, `W:`, `5M`-style abbreviations, or phase identifiers such as `request`.

## 3. Playground Page

ST0. The page-level layout of `/dashboard/admin-settings` (category navigation, search,
category sections, save bar, skeleton, and motion) is governed by
`spec/system-settings-ui.spec.md`. ST1-ST7 below define field-level behavior; where an ST
statement calls the container a "card" or "section", the container is the corresponding
category section defined there.

ST1. `/dashboard/admin-settings` MUST include active probe, passive breaker, timeout, billing, request capture, and protocol settings in the categories defined by `system-settings-ui.spec.md` SSU-1.

ST2. Those categories MUST expose at least these editable fields bound to `GET/PUT /api/dashboard/settings`:

- `monoize_active_probe_enabled` (boolean switch)
- `monoize_active_probe_interval_seconds` (integer >= 1)
- `monoize_active_probe_success_threshold` (integer >= 1)
- `monoize_active_probe_model` (optional string, empty means null)
- `monoize_passive_failure_threshold` (integer >= 1)
- `monoize_passive_cooldown_seconds` (integer >= 1)
- `monoize_passive_window_seconds` (integer >= 1)
- `monoize_passive_rate_limit_cooldown_seconds` (integer >= 1)
- `monoize_request_timeout_ms` (integer >= 1)
- `monoize_stream_idle_timeout_ms` (integer >= 1)
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

ST6e. At viewport widths below `640px`, the settings category navigation and the rendered category sections MUST shrink to the available content width without creating page-level horizontal overflow; the navigation row scrolls inside its own container per `system-settings-ui.spec.md` SSU-5.

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

AK11. In the `/dashboard/tokens` list, the API key name and its group badge collection MUST render in a single non-wrapping inline row inside the name cell.

- When the inline size available to the name and its group badges is less than 12rem, the group badges MUST NOT render, so the name keeps that space. The edit dialog still shows the groups.
- A key with `use_user_group = true` MUST NOT render group badges (it follows the owner's group).
- A key with `use_user_group = false` MUST render its `group_ids` in stored order as name badges resolved from `GET /api/dashboard/groups`; an id without a matching registry row MUST fall back to the raw id.
- The group badge collection MUST remain adjacent to the API key name and MUST NOT move below the name.
- If the inline row exceeds the cell width, the name MUST truncate with an ellipsis and expose the full name through its `title` attribute. The list MUST NOT scroll horizontally.

## 5. Dashboard Home Page

The normative home-overview contract lives in `spec/dashboard-home-overview.spec.md`.
This section restates the layout binding for `/dashboard` and MUST stay aligned with that
file.

DH1. `/dashboard` MUST render these sections in order: greeting header; conditional
getting-started card; account overview; full-width usage chart; recent-usage table + API
information; performance panel. Details: `dashboard-home-overview.spec.md` DH-1 through DH-9e.

DH2. The flat five-cell account overview, its fixed 24-hour Analytics data, and its
conditional active-model/subscription cell MUST follow
`dashboard-home-overview.spec.md` DH-5 through DH-5e.

DH3. The usage chart MUST be a stacked cumulative token `AreaChart` driven by
`GET /api/dashboard/analytics` with `buckets` and `range_hours` derived from the
selected time window (`1h`, `24h`, `7d`, `30d`; default `24h`, persisted) per the
`dashboard-home-overview.spec.md` DH-6a mapping, including `tokens_by_model`, a
fully visible Today/Now marker, and a width-adaptive paginated horizontal legend whose
controls appear on the right of the same row
(`dashboard-home-overview.spec.md` DH-6 through DH-6k).

DH4. Recent usage and API information panels MUST follow
`dashboard-home-overview.spec.md` DH-7 through DH-8d, including server-side aggregation,
bounded table scrolling, and the Messages-before-Models endpoint order.

DH5. The performance panel and admin-configurable targets MUST follow
`dashboard-home-overview.spec.md` DH-9 through DH-9c. The ready state MUST use the
specified four-column table alignment and shared nonlinear row-entry motion.

DH6. Loading and motion contracts MUST follow
`dashboard-home-overview.spec.md` DH-11 through DH-13, including the
no-skeleton-on-revalidate and no-animation-replay rules (DH-12a through DH-12c).

DH7. `/dashboard` MUST NOT throw runtime errors when optional config fields are absent
from settings or analytics payloads.

## 6. Users Page

UP1. In the `/dashboard/users` list, the role badge (`user.role`) MUST be rendered as a single-line badge. Badge text and icon MUST NOT wrap into multiple lines.

UP2. The users list MUST use `DataList` inside `DataTableShell` (`frontend-design-system.spec.md` DS68). In wide mode, every column, including the actions column, MUST be visible without horizontal scrolling at a list width of 56rem or more. Below wide mode, rows MUST stack as defined by DS22d.

UP3. The users list MUST NOT establish its own horizontal or vertical scroll container and MUST NOT set a viewport-derived height.

UP4. The users list body MUST be virtualized with `Virtuoso`, `virtualDataListComponents`, and the dashboard main pane as `customScrollParent`. The list header MUST stay sticky relative to the main pane in wide mode.

UP5. In the `/dashboard/users` list table, the username text and the user's group badge MUST render in a single non-wrapping inline row inside the user cell.

- If horizontal space is insufficient, the username text MAY truncate.
- The group badge MUST display the group **name** resolved from `GET /api/dashboard/groups`; an id without a matching registry row MUST fall back to the raw id.
- The group badge MUST remain single-line and MUST NOT move below the username.

UP12. The user create and edit dialogs MUST select the user's group through the shared
single-select group selector (GS rules, §11). They MUST NOT offer freeform group text
entry.

UP13. Subscription management MUST use the separate per-row dialog defined by
`billing-plan-subscriptions.spec.md` BP-UI6 through BP-UI8. The user edit dialog MUST
contain only deferred-submit fields (applied by its "Save" button); it MUST NOT contain
any control that takes effect before "Save".

UP6. The users table wide-mode columns MUST be, in order: user (UP5), role, prepaid balance, UTC-calendar-day spend, UTC-calendar-day call count, plan, created, last login, status, actions, with the grid track list `minmax(0,9.25fr) minmax(0,6.5fr) minmax(0,5fr) minmax(0,4.25fr) minmax(0,3.75fr) minmax(0,4.5fr) minmax(0,5.75fr) minmax(0,5.75fr) 4rem 12.25rem`. The plan cell MUST show `active_plan_name` on line 1 and the localized expiry (`formatDate(active_plan_expires_at)`) on line 2, or `—` when the user has no active plan. The actions cell MUST show inline ghost icon buttons, in order: view request logs (UP11), view API keys (UP15), plan (BP-UI6, editable rows only), edit (editable rows only), and delete (deletable rows only).

UP14. The users toolbar left side MUST contain a `TableToolbarSearch` that filters the loaded list by case-insensitive substring of username or email. The query MUST be stored in the URL as `?q=` (replace navigation; removed when empty) and initialized from it. The count and the UP10 totals MUST reflect the filtered rows. When the query matches nothing, the list MUST show an inline no-match empty state that names the query.

UP15. Each user row MUST include a read-only "View API keys" action that opens a side sheet listing the target user's keys from `GET /api/dashboard/users/{user_id}/api-keys` (`api-token-management.spec.md` §2.1a). Each item MUST show name, `key_prefix`, enabled/expired state, and the sub-account balance (`$` + `formatUsdDecimal(value, 2)`) or "uses account balance". The sheet MUST show a skeleton while loading, `QueryError` on failure, and an empty state when the user has no keys. It MUST NOT offer any mutation.

UP16. The status switch on the signed-in admin's own row MUST be disabled with the reason "You cannot disable the account you are signed in with" exposed as `title` and accessible description. Disabling another user MUST first open a confirm dialog stating that the user's N API keys stop working immediately (N from UP15's endpoint when loaded, otherwise a count-free sentence). Re-enabling MUST NOT require confirmation. The server rejects self-disable with `400 cannot_disable_self` (`user-billing-and-model-metadata.spec.md` A9).

UP17. The edit dialog's balance control MUST offer "Set" and "Add" modes, each using a USD decimal input with inline validation. "Add" MUST submit only `balance_delta_nano_usd` (exact `BigInt` conversion, may be negative) and preview "current → current + amount". "Set" MUST submit `balance_nano_usd` with `expected_balance_nano_usd` equal to the balance shown when the dialog opened, and only when the value changed. A note input ("Note (optional, visible to the user)", max 200 characters) MUST be submitted as `balance_note` whenever a balance change is submitted. On `409 balance_changed` the dialog MUST stay open, revalidate the users list, update the displayed current balance, and show "Balance changed (now $X). Review and retry." inline at the balance field.

UP18. The user create and edit dialogs MUST be `<form>` elements with a `type="submit"` primary button, MUST map server errors whose `param` is `username`, `password`, `email`, or `balance` to inline field errors, and MUST use toasts only for errors without a field. They MUST pass `dirty` to `DialogContent` when the draft differs from its initial value.

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

US1. `/settings` MUST render exactly these cards in order: profile, security, preferences.
It MUST NOT render a billing card, a balance value, or plan data.

US2. The profile card MUST show the avatar, the read-only username, the user's group when
`group_id` is set, and the email form. It MUST NOT call `GET /api/dashboard/billing-plans`.

US3. The Email field label icon and label text MUST form one inline group whose rendered boxes share one visual vertical center.

US4. The profile card MUST reflect a later `auth/me` refresh without a page close or reopen.

US5. The email form and the password form MUST each be one `<form>` with `noValidate` and a
`type="submit"` primary button, so Enter in a single-line input submits. Submit buttons MUST
be disabled only while their request is pending.

US6. The email value is trimmed before validation. An empty value clears the email. A
non-empty value MUST have length at most 254, contain no whitespace, contain exactly one
`@` with a non-empty local part, and have a domain that contains a dot and no empty label.
The client MUST reject an invalid value inline with
`userSettings.errors.invalidEmail` and MUST NOT send a request. `PUT
/api/dashboard/auth/me` MUST apply the same rule, store the trimmed value, store null for an
empty value, and reject an invalid value with HTTP `400`, code `invalid_email`, and
`param: "email"`, without changing the row.

US7. The password form MUST validate on submit: current password non-empty, new password at
least 8 characters, confirmation equal to the new password. Each failure MUST render as a
`FieldError` under its field, set `aria-invalid`, and focus the first invalid field.
`PUT /api/dashboard/auth/password` MUST set `param: "current_password"` on
`invalid_current_password` and `param: "new_password"` on `invalid_password`. The client
MUST use `apiErrorParam` to render these errors below the named field, using localized
text. Other failures MUST use `toastApiError`, so raw server messages are never displayed.

US8. The language and theme controls MUST be the outline-button variants of the shared
dropdown components that the login page also uses (CAP-U15).

## 7. Token Management Page (UI)

AK4. The API key list body in `/dashboard/tokens` MUST use virtualized rendering via `react-virtuoso` (`Virtuoso`) instead of rendering all rows as plain DOM rows.

- The list MUST render through the `DataList` primitives (`frontend-design-system.spec.md` §6.1). `Virtuoso` MUST receive `DataListBody` as its list component and `DataListRow` as its item component.
- `Virtuoso` MUST receive the DL7b main pane element as `customScrollParent`. The list height MUST equal the height of its rendered rows. The page MUST NOT set a fixed viewport-relative list height.
- In wide mode, the header row MUST be sticky at the top of the main pane.
- The select-all checkbox MUST render in the toolbar with a localized visible label in every mode. Per-row checkboxes MUST remain in each row. The header row MUST NOT contain interactive controls.

AK-UI-LIST-1. `/dashboard/tokens` MUST render, in order: the page header, a toolbar, and the list surface. The page MUST NOT render a section heading or a key-count paragraph between the header and the toolbar.

- The toolbar MUST contain the select-all checkbox and a search input at the inline start, and a localized key count at the inline end. The count MUST equal the number of keys that match the current search query.
- The list MUST use `DataList dense` (`frontend-design-system.spec.md` DS22g). Every wide-mode cell MUST render exactly one line of text, so each wide-mode row is 48 CSS px high (±1) when it contains no wrapped badge.
- In wide mode, the list columns MUST be, in order: name, key, restrictions, expiry, sub-account balance, status, and actions. The row checkbox MUST render at the inline start of the name cell, so that it shares the primary line below wide mode.
- The name cell MUST render the AK11 name row. The key cell MUST render the displayed key prefix (AK-UI-SEARCH-1) in `font-mono` followed by `…` and its copy action.
- The sub-account balance column MUST render only when at least one loaded key has `sub_account_enabled = true`. Its header MUST read "Independent Balance" (localized). A key with `sub_account_enabled = false` MUST render `—` in that column. The page MUST NOT render "Uses account balance" or an equivalent sentence in any row.
- The actions column width MUST follow `design.md` §4.3 for the largest number of row actions among the loaded keys: 2 (edit, delete), 3 when any key has a sub-account, and 4 when any key has a sub-account and the viewer is an admin.
- A key without `expires_at` MUST render the localized text "Never expires". A key whose `expires_at` is earlier than the current time MUST render a warning status badge equivalent to "Expired" in the expiry cell.
- A disabled or expired key MUST render its name in `text-muted-foreground`. The row opacity MUST remain 1.

AK-UI-LIST-2. Below wide mode, each key row MUST render: (1) a primary line with the row checkbox, the AK11 name row, and the enable switch at the inline end; (2) one wrapping metadata group with the key prefix and its copy action, the restriction badges (nothing when the key has none), the expiry text, and, for a key with `sub_account_enabled = true`, the localized "Independent Balance" label followed by the balance. The row actions MUST end-align on the first metadata line when that leaves the metadata group at least 14rem of inline size; otherwise they MUST wrap to one end-aligned last line. In both cases the actions are the last content of the row, as DS22i requires. The wide-mode key, restrictions, expiry, balance, status, and `DataListActions` cells MUST NOT render below wide mode, and the metadata group MUST NOT render in wide mode. This is the only `/dashboard/tokens` exception to DS22d.

AK-UI-SEARCH-1. The token search MUST filter the loaded key list on the client. A key matches when its `name` or its displayed key prefix contains the trimmed query, compared case-insensitively. An empty query matches every key.

- When the query changes, the selection MUST drop every key that no longer matches.
- The select-all checkbox MUST be checked exactly when at least one key matches and every matching key is selected. Activating it MUST select or deselect only matching keys.
- When keys exist and none match, the list surface MUST render an inline empty state that names the query. It MUST NOT render the no-keys empty state.

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

PLN-UI1. The data and mutation contract of `/dashboard/plans` is defined by
`billing-plan-subscriptions.spec.md` §9 (BP-UI1, BP-UI2). No plan reset action or reset
endpoint exists.

PLN-UI2. The page MUST render, in order: the page header with the create action, a
toolbar whose inline end shows a localized plan count, and the list surface. The list MUST
use the `DataList` primitives (`frontend-design-system.spec.md` §6.1). In wide mode, the
columns MUST be, in order: name with description, sliding-window limits, eligible groups,
prices, multiplier, listed state, and actions.

- The name cell MUST render the plan name on its first line and the description on a
  second muted line.
- The limits cell MUST render one line per configured window, labeled `5h`, `24h`, `7d`,
  or `30d`. Each amount MUST render as `$` followed by the exact decimal value with
  trailing fractional zeros removed. The value MUST NOT be rounded.
- The prices cell MUST render one line per price as "$price / N days" in the active
  locale. A plan without prices MUST render `—`.
- The multiplier cell MUST be end-aligned and render `{multiplier}×`.
- A listed plan MUST render `StatusBadge variant="success"`; an unlisted plan MUST render
  muted plain text.
- The action group MUST contain edit and delete buttons whose accessible names include
  the plan name.
- Header and body cells MUST use start alignment, except the multiplier column and the
  action group, which MUST use end alignment.

PLN-UI3. Plan create and edit dialogs MUST select `group_ids` through the shared
unordered multi-select group selector (GS rules, §11) instead of freeform text. The plan
list MUST render `group_ids` as group-name badges; an empty array renders the localized
unrestricted label.

PLN-UI4. Every plan form input MUST have an associated label. Each price row MUST expose
labeled price and duration inputs and a remove button with an accessible name.

PLN-UI5. The loading state MUST render inside `PageWrapper`. An empty plan list MUST
render the shared empty state with the create action.

## 11. Groups Management Page and Shared Group Selector

### 11.1 Groups page

GP1. `/dashboard/groups` is an admin page (nav per DL6). It MUST list every registry row
from `GET /api/dashboard/groups` in the returned order and MUST render a skeleton
placeholder while the list is loading.

GP2. Each row MUST display: its 1-based position in the list, name, description (muted,
truncated with title attribute), a default-group badge when `is_default` is true, the
`billing_ratio` as `<ratio>×` (for example `1×`, `1.5×`), and a user-selectable switch.
The raw `sort_order` value MUST NOT be displayed.

GP2a. The page MUST render, in order: the page header with the create action, a toolbar
whose inline end shows a localized group count, and the list surface. The list MUST use
the `DataList` primitives (`frontend-design-system.spec.md` §6.1). In wide mode, the
columns MUST be, in order: position (位置), name with description, billing multiplier
(`align="end"`, `tabular-nums`), user-selectable, and actions.

- The position cell MUST contain the drag handle and `index + 1`.
- The name cell MUST render the name and default badge on its first line and the
  description on its second line. An empty description MUST render as `—`.
- The action group MUST contain, in order: move up, move down, edit, and delete. Move
  and drag controls MUST name the group in their accessible names.

GP3. The page MUST offer create, edit, and delete actions bound to
`POST /api/dashboard/groups`, `PUT /api/dashboard/groups/{id}`, and
`DELETE /api/dashboard/groups/{id}`. Create/edit dialogs MUST expose exactly: name
(required), description, billing multiplier (required, decimal per MP-U1, `>= 0`), and
the user-selectable switch. The dialogs MUST NOT expose `sort_order`; create omits it so
the server appends the group (`groups-registry.spec.md` GR-A3a), and reordering happens
only through GP6. The create dialog MUST default `user_selectable` to `false` and
`billing_ratio` to `1`.

GP3a. The billing multiplier field MUST carry help text stating the formula
"final charge = model price × Channel multiplier × this group's multiplier", that it
applies to the balance charge, and that `0` makes the group free. The user-selectable
help text MUST ask the admin to confirm that the group's Providers may serve every user.

GP3b. Both dialogs MUST wrap their fields in a `<form>` whose primary button is
`type="submit"`, MUST pass `dirty` to `DialogContent` when any field differs from its
initial value, and MUST show validation errors under the offending field with
`aria-invalid` and focus on the first invalid field. A `group_name_exists` or
`invalid_group_name` server error MUST render as the name field's error; other server
errors MUST use the shared localized error toast (`apiErrors.<code>`), never the raw
server message.

GP4. The delete action for the default group MUST be disabled. Deleting any other group
MUST open a confirmation dialog that names the group and states the cascade consequences:
member users move to the default group, API keys and Providers drop the group, and the
delete is rejected when a billing plan would be left without an eligible group (GR-X4).
Before confirming, the dialog MUST check the loaded billing plans; when any plan's
`group_ids` equals exactly `[group.id]`, the dialog MUST list those plan names, link to
`/dashboard/plans`, and disable the confirm button.

GP5. Every mutation MUST apply an SWR optimistic update to the groups cache and roll back
on error with a localized error toast (`apiErrors.<code>`, never the raw server message).
After a successful create or update the
groups cache MUST be revalidated (badges resolve names from the groups cache, so no other
cache is stale). After a successful delete the groups cache MUST be revalidated together
with the users, tokens, providers, billing-plans, and current-user caches, because the
server-side deletion cascade rewrites group references in those entities.

GP6. Each group row MUST provide move-up and move-down buttons in its action group.
The first row's move-up button and the last row's move-down button MUST be disabled. Native
HTML drag-and-drop reordering MUST be available through a drag handle when the browser
matches `(pointer: fine)`; coarse-pointer users MUST retain the move buttons. A completed
move or drop MUST call `POST /api/dashboard/groups/reorder` with every group id in the new
visual order. While that mutation is pending, every group reorder control MUST be disabled.
The groups SWR cache MUST immediately show the new order with `sort_order` values equal to
the zero-based visual indices, MUST revalidate after success, and MUST restore the prior
cache snapshot and show a localized error toast after failure.

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
