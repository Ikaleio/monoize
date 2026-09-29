# System Settings UI Specification

## 0. Scope

- Product name: Monoize.
- Scope: layout, interaction, and state contract of the admin system settings page at
  `/dashboard/admin-settings` (`frontend/src/pages/settings.tsx` and
  `frontend/src/components/settings/*`).
- Data contract: the page reads `GET /api/dashboard/settings` and writes
  `PUT /api/dashboard/settings`. This specification changes no API field, no field type,
  and no persistence behavior.
- Field-level behavior requirements from `dashboard-ui-layout.spec.md` (ST1-ST7) remain in
  force. Where ST statements describe the container as a "card" or "section", the
  container is the category section defined by SSU-12.
- Visual rules (type roles, tokens, primitives) are defined in `design.md` §3.7.

## 1. Category and entry model

SSU-1. The page MUST partition the editable settings fields into exactly 9 categories with
these stable ids, in this order:

| # | id | title key | fields |
|---|----|-----------|--------|
| 1 | `site` | `settings.siteInformation` | `site_name`, `site_description`, `api_base_url`, `recharge_public_origin` |
| 2 | `access` | `settings.accessControl` | `registration_enabled`, `default_user_role`, `captcha_enabled`, `session_ttl_days`, `api_key_max_per_user` |
| 3 | `codex` | `settings.codexModels` | `codex_model_ids` |
| 4 | `suffix` | `settings.reasoningSuffixMap` | `reasoning_suffix_map` |
| 5 | `redirects` | `settings.globalModelRedirects` | `global_model_redirects` |
| 6 | `transforms` | `settings.globalTransforms` | `global_transforms` |
| 7 | `affinity` | `settings.affinityRouting` | `monoize_affinity_enabled`, `monoize_affinity_failback_mode`, `monoize_affinity_idle_ttl_seconds`, `monoize_affinity_failback_delay_seconds` |
| 8 | `health` | `settings.healthMonitoring` | `monoize_active_probe_enabled`, `monoize_active_probe_interval_seconds`, `monoize_active_probe_success_threshold`, `monoize_active_probe_model`, `monoize_passive_failure_threshold`, `monoize_passive_cooldown_seconds`, `monoize_passive_window_seconds`, `monoize_passive_min_samples`, `monoize_passive_failure_rate_threshold`, `monoize_passive_rate_limit_cooldown_seconds`, `monoize_request_capture_enabled`, `monoize_mask_sensitive_info`, `monoize_request_capture_max_total_bytes`, `monoize_enable_estimated_billing`, `allow_free_when_unpriced`, `allow_free_when_missing_usage`, `monoize_strip_cross_protocol_nested_extra`, `monoize_request_timeout_ms`, `dashboard_performance_group_ids`, `dashboard_performance_model_ids` |
| 9 | `extra` | `settings.extraFieldsWhitelist` | `monoize_extra_fields_whitelist` (sub-keys `chat_completion`, `responses`, `messages`, `gemini`) |

SSU-1a. Each category contains one or more groups. A group is either a *row group*
(setting rows on one `Card`) or an *editor group* (one editor with no `Card`). Groups
render in this order:

| category | group id | title key | kind |
|----------|----------|-----------|------|
| `site` | `site` | none | row |
| `access` | `access.registration` | `settings.registration` | row |
| `access` | `access.session` | `settings.sessionSecurity` | row |
| `codex` | `codex` | none | editor |
| `suffix` | `suffix` | none | editor |
| `redirects` | `redirects` | none | editor |
| `transforms` | `transforms` | none | editor |
| `affinity` | `affinity` | none | row |
| `health` | `health.probe` | `settings.groupActiveProbe` | row |
| `health` | `health.passive` | `settings.groupPassiveBreaker` | row |
| `health` | `health.capture` | `settings.groupRequestCapture` | row |
| `health` | `health.runtime` | `settings.groupRuntimeBehavior` | row |
| `health` | `health.dashboard` | `settings.dashboardPerformanceTitle` | editor |
| `extra` | `extra` | none | row |

SSU-1b. A *setting entry* is the smallest searchable unit. The static entry list in
`frontend/src/components/settings/settings-categories.ts` is the single source of the
label, description, group, and API field names of every entry. Row groups contain one
entry per API field, with these exceptions: `health.dashboard` is one entry
(`dashboard_performance_targets`) for both `dashboard_performance_*` fields, and `extra`
has one entry per whitelist sub-key (`extra_fields_<sub-key>`). Each editor group
contains exactly one entry, named after its API field.

SSU-2. Every field listed in SSU-1 MUST be editable through exactly one setting entry.
No field present in the pre-redesign page may become unreachable.

SSU-2a. `monoize_request_capture_max_total_bytes` is edited through one integer input
denominated in MiB: the displayed value is `round(bytes / 1048576)`, and an input value
`v >= 0` writes `v * 1048576` to the draft. Input `0` writes `0` (no size budget,
`request-capture-dumps.spec.md` RCD-C4). The field description MUST state that `0`
disables the budget.

SSU-3. `tool_prices`, `price_sync_auto_enabled`, `price_sync_new_api_base_url`,
`price_sync_new_api_token`, and `updated_at` are not edited on this page. Save MUST pass
them through unchanged from the current draft object. `tool_prices` and the price-sync
settings are edited on the `/dashboard/models` page (`model-pricing.spec.md` §11).

SSU-3a. `allow_free_when_unpriced` and `allow_free_when_missing_usage` render as two
switch rows in the `health.runtime` group. Each description MUST state the fail-closed
default (`false`) and the effect defined by `model-pricing.spec.md` §7.

SSU-3b. `recharge_public_origin` renders as one text input in the `site` group.
Its value constraint, default, and rejection behavior are defined by
`recharge-system.spec.md` RC-G1; the field description MUST state that order
creation requires a non-empty value.

## 2. Page layout

SSU-4. The page MUST render, in DOM order: `PageHeader` (title `settings.title`,
description `settings.description`, no actions), then the settings body.

SSU-5. The settings body MUST be an inline-size container. Let `W` be its width.

- `W >= 56rem`: two columns. The left column is `13rem` wide and contains the search
  input above the category navigation. The left column is sticky at the top of the
  dashboard `<main>` scroll container. The right column holds the content and the save
  bar.
- `W < 56rem`: one column, in this order: search input, category navigation, content,
  save bar. The category navigation is one horizontal row that does not wrap. When the
  row is wider than `W`, it scrolls horizontally inside its own container.

SSU-6. The category navigation MUST be a `nav` element with `aria-label` resolved from
`settings.categoryRailLabel`. It contains one `button` per category in SSU-1 order. Each
button shows the category title. The button of the active category in browse mode
(SSU-9) MUST carry `aria-current="true"`; no other button carries `aria-current`.

SSU-7. The active navigation button MUST show a `bg-accent` indicator that moves between
buttons through the shared layout animation helper (`SharedTabIndicator`). Motion MUST
respect DS32-DS34 reduced-motion rules.

SSU-8. When the navigation is one horizontal row (SSU-5), the row MUST scroll a target
button into view when that button is partially or fully outside the visible scroll area.
The target is the active category's button in browse mode, and the button of the first
category (SSU-1 order) with at least one match in search mode.

SSU-24. At viewport widths of 320px and above, the page MUST NOT create page-level
horizontal overflow. The only horizontal scroll container on the page is the navigation
row of SSU-5 (`W < 56rem`).

## 3. Browse mode

SSU-9. The page is in *browse mode* if and only if the normalized search query (SSU-15)
is empty. In browse mode exactly one category is active and only its category section is
rendered. The initial active category on page load MUST be `site`. Active-category state
is client-side view state only; it MUST NOT be persisted to the backend or to browser
storage.

SSU-10. Activating a navigation button in browse mode MUST make its category active.
If the top edge of the content column is then above the top edge of `<main>`, the page
MUST scroll `<main>` so that the content column top is visible.

SSU-11. On category change in browse mode, the incoming section MAY animate opacity and a
vertical offset of at most 8px, exactly once per change. Under reduced motion the section
MUST animate opacity only or render without animation.

## 4. Category section, groups, and rows

SSU-12. A category section MUST contain, in order: an `h2` with the category title, one
paragraph with the category description, and the category's visible groups (SSU-1a
order). The section element MUST have DOM id `settings-category-<category id>`.

SSU-13. A group with a title renders the title as an `h3` above the group body. A row
group body is one `Card` whose rows are separated by `divide-y`. An editor group body is
the editor without a `Card`.

SSU-14. A setting row MUST have DOM id `setting-<entry id>` and contain the entry label
(a `label` element bound to the control when the control is a native input, a `Switch`,
or a `Select` trigger), the entry description when the entry has one, and the control.
Let `R` be the row group body width.

- Switch rows: label and description on the left, `Switch` on the right, at every `R`.
- Other rows with `R >= 40rem`: two columns `minmax(0,1fr) 20rem`; label and
  description in the left column, control in the right column.
- Other rows with `R < 40rem`: label, description, and control stacked in one column.

SSU-14a. The `extra` group ends with one paragraph resolved from
`settings.extraFieldsWhitelistHelp`, rendered below the `Card`.

## 5. Search

SSU-15. The search input MUST be the first element of the settings body, use
`type="search"`, and have an accessible name and placeholder resolved from
`settings.searchPlaceholder`. The *normalized query* is the input value trimmed and
lowercased with `toLocaleLowerCase()`. Its *tokens* are the normalized query split on
runs of whitespace.

SSU-16. An entry matches a non-empty normalized query if and only if every token is a
substring of the entry's lowercased search text. The search text is the concatenation of:
the translated entry label, the translated entry description, any translated help text
listed for the entry, the translated group title, the translated category title, and the
entry's API field names (including whitelist sub-keys). For an editor group entry, the
help text includes the category description. For a row group entry, the category
description is not part of the search text. Matching MUST run on the client
without network requests and MUST re-run when the UI language changes.

SSU-17. The page is in *search mode* if and only if the normalized query is non-empty.
Entering search mode from browse mode applies the SSU-10 scroll rule. In search mode:

1. The content column renders one category section (SSU-12) for every category with at
   least one matching entry, in SSU-1 order. Inside each section only groups with at
   least one matching entry render, and inside each group only matching entries render.
2. Above the sections, one status line with `aria-live="polite"` states the number of
   matching entries through `settings.searchResultCount` (plural forms).
3. If no entry matches, the content column renders `EmptyState` with title
   `settings.searchNoMatchTitle` and description `settings.searchNoMatchDescription`
   containing the raw query, in place of the sections.
4. Every navigation button shows its category's match count. Buttons whose category has
   zero matches are `disabled`. No button carries `aria-current`.
5. Activating a navigation button makes that category active for the next browse mode
   and scrolls its section (`settings-category-<id>`) to the top of `<main>`. The query
   is unchanged.
6. Matching rows keep their controls. Edits in search mode update the same draft as
   edits in browse mode.

SSU-18. Pressing `Escape` in the search input MUST clear the query. Clearing the query
returns to browse mode with the last active category and applies the SSU-10 scroll rule.

## 6. Draft, save bar, and validation

SSU-19. Data fetching MUST use the existing SWR hooks `useSettings`, `useProviders`, and
`useTransformRegistry`. The page MUST NOT fetch inside `useEffect`.

SSU-20. Draft-state contract:

- the page keeps a `localSettings` draft; the rendered value is
  `localSettings ?? settings`;
- any field edit replaces `localSettings` with the merged draft;
- the draft survives category changes, mode changes, and section unmount/remount.

SSU-21. The save bar MUST render if and only if `localSettings` is non-null or a save is
in flight. It is sticky at the bottom edge of `<main>` inside the content column and
contains: the text `settings.unsavedChanges`, a `Discard` button
(`settings.discardChanges`, `variant="outline"`), and the save button (default variant,
label `common.saveChanges`, or `common.saving` while a save is in flight). Both buttons
are disabled while a save is in flight. The save button is the only default-variant
button on the page. The bar enters and leaves with opacity and a vertical offset of at
most 8px; under reduced motion it animates opacity only.

SSU-21a. `Discard` sets `localSettings` to `null` without an API call. The rendered
values return to the last SWR value.

SSU-22. Save behavior:

1. Validate `global_transforms` with `findFirstInvalidTransformRule` against the
   global-scope subset of the transform registry; on failure show the
   `transforms.validationRuleInvalid` toast and do not call the API.
2. Drop `global_model_redirects` entries whose `pattern` or `replace` trims to empty.
3. Persist through `updateSettingsOptimistic` (optimistic SWR update), then clear
   `localSettings`, show a success toast `settings.saved`, and revalidate.
4. On error show a toast with the error message or `settings.failedSave`. The draft is
   kept.

## 7. Loading, failure, and copy

SSU-23. While `useSettings` is loading, the page MUST render a skeleton inside
`PageWrapper` with the SSU-5 body geometry: a page-header skeleton, a left column with a
search placeholder and at least 6 navigation placeholders (`W >= 56rem`) or one
horizontal row of at least 4 navigation placeholders (`W < 56rem`), and a content
placeholder with a title and a `Card` of at least 4 row placeholders.

SSU-23a. When `useSettings` resolves with an error and no data, the page MUST render
`PageHeader` and `QueryError` whose retry revalidates `useSettings`. The raw error
message MUST NOT be shown.

SSU-23b. All user-visible strings MUST resolve through `react-i18next`. Keys used by the
page chrome (`settings.categoryRailLabel`, `settings.searchPlaceholder`,
`settings.searchResultCount`, `settings.searchNoMatchTitle`,
`settings.searchNoMatchDescription`, `settings.unsavedChanges`,
`settings.discardChanges`, `settings.saved`, and every group title of SSU-1a) MUST exist
in all four locales (`en`, `zh`, `zh-TW`, `ja`).

SSU-25. This specification supersedes the numbered horizontal category rail, the oversized
serif category header band, and the stacked-card composition of earlier revisions.
