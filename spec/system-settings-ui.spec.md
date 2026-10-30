# System Settings UI Specification

## 0. Scope

- Product name: Monoize.
- Scope: layout, interaction, and state contract of the admin system settings page at
  `/dashboard/admin-settings` (`frontend/src/pages/settings.tsx` and
  `frontend/src/components/settings/*`).
- Data contract: the page reads `GET /api/dashboard/settings` and writes
  `PUT /api/dashboard/settings`. The API contract used by this page is defined in §8.
- Field-level behavior requirements from `dashboard-ui-layout.spec.md` (ST1-ST7) remain in
  force. Where ST statements describe the container as a "card" or "section", the
  container is the category section defined by SSU-12.
- Visual rules (type roles, tokens, primitives) are defined in `design.md` §3.7.

## 1. Category and entry model

SSU-1. The page MUST partition the editable settings fields into exactly 8 categories,
organized by administrative intent, with these stable ids, in this order:

| # | id | title key | fields |
|---|----|-----------|--------|
| 1 | `site` | `settings.categorySite` | `site_name`, `site_description`, `api_base_url`, `recharge_public_origin`, `dashboard_performance_group_ids`, `dashboard_performance_model_ids` |
| 2 | `access` | `settings.categoryAccess` | `registration_enabled`, `captcha_enabled`, `session_ttl_days`, `api_key_max_per_user` |
| 3 | `models` | `settings.categoryModels` | `codex_model_ids`, `reasoning_suffix_map`, `global_model_redirects` |
| 4 | `transforms` | `settings.categoryTransforms` | `global_transforms` |
| 5 | `routing` | `settings.categoryRouting` | `monoize_affinity_enabled`, `monoize_affinity_failback_mode`, `monoize_affinity_idle_ttl_seconds`, `monoize_affinity_failback_delay_seconds`, `monoize_active_probe_enabled`, `monoize_active_probe_interval_seconds`, `monoize_active_probe_success_threshold`, `monoize_active_probe_model`, `monoize_passive_failure_threshold`, `monoize_passive_cooldown_seconds`, `monoize_passive_window_seconds`, `monoize_passive_rate_limit_cooldown_seconds`, `monoize_request_timeout_ms`, `monoize_stream_idle_timeout_ms` |
| 6 | `billing` | `settings.categoryBilling` | `monoize_enable_estimated_billing`, `allow_free_when_unpriced`, `allow_free_when_missing_usage` |
| 7 | `privacy` | `settings.categoryPrivacy` | `monoize_request_capture_enabled`, `monoize_request_capture_max_total_bytes`, `monoize_mask_sensitive_info` |
| 8 | `compatibility` | `settings.categoryCompatibility` | `monoize_extra_fields_whitelist` (sub-keys `chat_completion`, `responses`, `messages`, `gemini`), `monoize_strip_cross_protocol_nested_extra` |

SSU-1a. Each category contains one or more groups. A group is either a *row group*
(setting rows on one `Card`) or an *editor group* (one editor with no `Card`). Groups
render in this order:

| category | group id | title key | description key | kind |
|----------|----------|-----------|-----------------|------|
| `site` | `site` | none | none | row |
| `site` | `site.dashboard` | `settings.dashboardPerformanceTitle` | none | editor |
| `access` | `access.registration` | `settings.registration` | none | row |
| `access` | `access.session` | `settings.sessionSecurity` | none | row |
| `models` | `models.codex` | `settings.codexModels` | `settings.codexModelsDescription` | editor |
| `models` | `models.suffix` | `settings.reasoningSuffixMap` | `settings.reasoningSuffixMapDescription` | editor |
| `models` | `models.redirects` | `settings.globalModelRedirects` | `settings.globalModelRedirectsDescription` | editor |
| `transforms` | `transforms` | none | none | editor |
| `routing` | `routing.affinity` | `settings.affinityRouting` | `settings.affinityRoutingDescription` | row |
| `routing` | `routing.probe` | `settings.groupActiveProbe` | none | row |
| `routing` | `routing.passive` | `settings.groupPassiveBreaker` | none | row |
| `routing` | `routing.timeouts` | `settings.groupTimeouts` | none | row |
| `billing` | `billing` | none | none | row |
| `privacy` | `privacy` | none | none | row |
| `compatibility` | `compatibility.extra` | `settings.extraFieldsWhitelist` | `settings.extraFieldsWhitelistDescription` | row |
| `compatibility` | `compatibility.protocol` | `settings.groupProtocolConversion` | none | row |

SSU-1b. A *setting entry* is the smallest searchable unit. The static entry list in
`frontend/src/components/settings/settings-categories.ts` is the single source of the
label, description, search keywords, numeric constraints, group, and API field names of
every entry. Row groups contain one entry per API field, with this exception:
`compatibility.extra` has one entry per whitelist sub-key (`extra_fields_<sub-key>`). Each
editor group contains exactly one entry, named after its API field, except
`site.dashboard`, whose entry `dashboard_performance_targets` covers both
`dashboard_performance_*` fields. Entry ids are a public contract (SSU-9b, other pages
link to them); a category or group change MUST NOT rename an entry id.

SSU-2. Every field listed in SSU-1 MUST be editable through exactly one setting entry.
`default_user_role`, `monoize_passive_min_samples`, and
`monoize_passive_failure_rate_threshold` are removed from the page, the API, and storage
(§8) because no runtime code reads them.

SSU-2a. `monoize_request_capture_max_total_bytes` is edited through one `NumberField`
denominated in MiB: the displayed value is `round(bytes / 1048576)`, and an input value
`v >= 0` writes `v * 1048576` to the draft. Input `0` writes `0` (no size budget,
`request-capture-dumps.spec.md` RCD-C4). The field description MUST state that `0`
disables the budget.

SSU-2b. Every numeric setting MUST use the shared `NumberField`
(`frontend-design-system.spec.md`) with the unit suffix and minimum below. The control
MUST NOT rewrite the text while the user types. The value is parsed on blur. An
out-of-range or non-integer value renders an inline `FieldError`, does not reach the draft,
and blocks saving (SSU-21). The row description MUST state the default and the minimum.

| field | unit key | min | default |
|-------|----------|-----|---------|
| `session_ttl_days` | `settings.unitDays` | 1 | 7 |
| `api_key_max_per_user` | none | 1 | 1000 |
| `monoize_affinity_idle_ttl_seconds` | `settings.unitSeconds` | 1 | 1800 |
| `monoize_affinity_failback_delay_seconds` | `settings.unitSeconds` | 0 | 300 |
| `monoize_active_probe_interval_seconds` | `settings.unitSeconds` | 1 | 30 |
| `monoize_active_probe_success_threshold` | `settings.unitTimes` | 1 | 1 |
| `monoize_passive_failure_threshold` | `settings.unitTimes` | 1 | 3 |
| `monoize_passive_cooldown_seconds` | `settings.unitSeconds` | 1 | 60 |
| `monoize_passive_window_seconds` | `settings.unitSeconds` | 1 | 30 |
| `monoize_passive_rate_limit_cooldown_seconds` | `settings.unitSeconds` | 1 | 15 |
| `monoize_request_timeout_ms` | `settings.unitMilliseconds` | 1 | 30000 |
| `monoize_stream_idle_timeout_ms` | `settings.unitMilliseconds` | 1 | 120000 |
| `monoize_request_capture_max_total_bytes` | `settings.unitMib` | 0 | 1024 |

SSU-2c. Each `monoize_extra_fields_whitelist` sub-key MUST use the shared `ListField` with
comma separation. Typing keeps the raw text; blur splits, trims, and deduplicates it. An
empty list removes the sub-key from the draft.

SSU-2d. The `site_name` row description MUST state that the value appears as the sidebar
brand, the browser tab title, and the login page title. The `site_description` row
description MUST state that the value appears as the login page subtitle.

SSU-3. `tool_prices`, `price_sync_auto_enabled`, `price_sync_new_api_base_url`,
`price_sync_new_api_token`, and `updated_at` are not edited on this page. Save MUST NOT
send them unless the draft differs from the server value (SSU-22 step 4 sends only changed
keys), so a stale draft never overwrites values saved on `/dashboard/models`. `tool_prices`
and the price-sync settings are edited on the `/dashboard/models` page
(`model-pricing.spec.md` §11).

SSU-3a. `allow_free_when_unpriced` and `allow_free_when_missing_usage` render as two
switch rows in the `billing` group. Each description MUST state the fail-closed
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
button shows the category title, plus a destructive status dot with screen-reader text
`settings.categoryHasError` while the category contains an error (SSU-20d, SSU-21b, SSU-22). The button of the active category in browse mode
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
rendered. The active category MUST live in the URL query parameter `category`, written with
`replace`. The parameter is absent for `site`, the default. A missing or unknown value
selects `site`. Active-category state MUST NOT be persisted to the backend or to browser
storage.

SSU-9a. A category change MUST NOT trigger the unsaved-changes guard (SSU-20a), because the
pathname does not change.

SSU-9b. Loading the page with `?setting=<entry id>` MUST select the category containing the
entry, scroll `#setting-<entry id>` into view, and highlight the row for 2 seconds. The
`setting` parameter is then removed with `replace` and `category` is written per SSU-9. An
unknown entry id is removed without any other effect. A highlight is a visible ring on the
row; under reduced motion the scroll is instant.

SSU-10. Activating a navigation button in browse mode MUST make its category active.
If the top edge of the content column is then above the top edge of `<main>`, the page
MUST scroll `<main>` so that the content column top is visible.

SSU-11. On category change in browse mode, the incoming section MAY animate opacity and a
vertical offset of at most 8px, exactly once per change. Under reduced motion the section
MUST animate opacity only or render without animation.

## 4. Category section, groups, and rows

SSU-12. A category section MUST contain, in order: an `h2` with the category title, one
paragraph with the category description, the warning of SSU-12a when it applies, and the
category's visible groups (SSU-1a order). The section element MUST have DOM id
`settings-category-<category id>`.

SSU-12a. When the rendered draft has `registration_enabled = true` and
`captcha_enabled = false`, the `access` section MUST render `StatusAlert variant="warning"`
stating that open registration without human verification allows automated sign-ups.

SSU-13. A group with a title renders the title as an `h3` above the group body, followed by
its description paragraph when SSU-1a lists one. A row
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

SSU-14a. The `compatibility.extra` group ends with one paragraph resolved from
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
listed for the entry, the translated search keywords of the entry (`keywordsKey`), the
translated group title, the translated category title, and the entry's API field names
(including whitelist sub-keys). For an editor group entry, the help text includes the
group description. The category description is not part of the search text. Matching MUST
run on the client without network requests and MUST re-run when the UI language changes.

SSU-16a. `keywordsKey` lists synonyms that do not appear in the label or description, in the
UI language plus common English terms. In `zh`, the query `验证码` MUST match
`captcha_enabled` and the query `超时` MUST match `monoize_request_timeout_ms` and
`monoize_stream_idle_timeout_ms`.

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
- any field edit replaces `localSettings` with the merged draft, and sets it to `null` when
  the merged draft deep-equals the server value;
- the draft survives category changes, mode changes, and section unmount/remount;
- the reasoning-suffix rows are draft state kept as an ordered array, so blank and duplicate
  rows survive section unmount/remount.

SSU-20a. *Dirty* means at least one setting entry differs from the server value by deep
equality after normalization. Normalization treats a redirect row with a blank `pattern` or
`replace` as absent, an empty whitelist sub-key as absent, an empty
`monoize_active_probe_model` as `null`, and object key order as irrelevant. A suffix-row
count that differs from the server map's size also makes `reasoning_suffix_map` differ, so
adding a blank row is dirty immediately. Editing a value and restoring it, or focusing and
leaving a field, MUST NOT make the page dirty.

SSU-20b. While dirty, leaving the page MUST ask for confirmation: `useUnsavedChangesGuard`
blocks in-app navigation to another pathname with a confirm dialog and registers
`beforeunload`.

SSU-20c. The reasoning-suffix editor MUST add a row with an immediate draft change. Choosing
an effort on a row with an empty suffix MUST keep the row. Rows whose trimmed suffix repeats
an earlier row MUST show the inline error `settings.suffixDuplicate` with `aria-invalid`, and
block saving. Each suffix input has `aria-label` `settings.suffixRowLabel` and each effort
select has `aria-label` `settings.suffixEffortRowLabel`, both containing the 1-based row
number. Blank rows are dropped on save.

SSU-20d. `ModelRedirectsEditor` MUST validate a pattern on blur and render
`settings.modelRedirectPatternInvalid` as an inline `FieldError` with `aria-invalid` below
the row while the rendered pattern is the blurred one. Validation approximates the Rust
`regex` crate on the client: inline flags `(?i)` and named groups `(?P<n>…)` are accepted;
lookaround and backreferences are rejected; other syntax follows `new RegExp`. Blur-time
errors do not block saving, because the backend is authoritative (SSU-22). The nav button of
the `models` category shows the error dot while any rendered pattern is invalid.

SSU-21. The save bar MUST render if and only if the page is dirty or a save is in flight. It
is sticky at the bottom edge of `<main>` inside the content column and contains: the text
`settings.unsavedChangesCount` (plural, the number of differing entries), a "view changes"
button (`settings.viewChanges`), a `Discard` button (`settings.discardChanges`,
`variant="outline"`), and the save button (default variant, label `common.saveChanges`, or
`common.saving` while a save is in flight). Discard and save are disabled while a save is in
flight. The save button is the only default-variant button on the page. The bar enters and
leaves with opacity and a vertical offset of at most 8px; under reduced motion it animates
opacity only.

SSU-21b. While any `NumberField` is invalid or any suffix is duplicated, the save button MUST
be disabled, its `title` and a visible line in the bar MUST state
`settings.saveBlockedInvalid`, and the nav button of each affected category shows the error
dot. An invalid `NumberField` that unmounts discards its raw text and stops blocking.

SSU-21c. "View changes" opens a `Popover` listing every differing entry in SSU-1b order as
"label: old → new". Booleans render as `settings.valueOn`/`settings.valueOff`, numbers with
their SSU-2b unit, strings verbatim, lists and maps as `settings.valueItemCount`, and empty
values as `—`. Activating an item closes the popover and jumps to the entry (SSU-9b scroll
and highlight), leaving search mode if the entry is not in the results.

SSU-21a. `Discard` sets `localSettings` and the suffix rows to `null` without an API call,
clears inline errors and raw `NumberField` text, and returns the rendered values to the last
SWR value.

SSU-22. Save behavior:

1. Validate `global_transforms` with `findFirstInvalidTransformRule` against the
   global-scope subset of the transform registry; on failure show the
   `formatTransformRuleError` text as a toast, jump to `global_transforms` and reveal the
   failing rule, render the same text as an inline error on that entry, and do not call the
   API.
2. If the draft makes a change listed in SSU-22a, open the confirm dialog of SSU-22a. The
   remaining steps run only after confirmation.
3. Drop `global_model_redirects` entries whose `pattern` or `replace` trims to empty.
4. Build a partial body with exactly the top-level keys whose value (after step 3) differs
   from the server snapshot by deep equality, excluding `updated_at`, and send it with
   `PUT /api/dashboard/settings` through `patchSettingsOptimistic` (optimistic SWR merge).
   An empty body sends no request. Then clear `localSettings`, show a success toast
   `settings.saved`, and revalidate.
5. On an error with a `param` that resolves to an entry (§8), the draft is kept, the page jumps
   to the entry and focuses its control, the entry renders an inline `FieldError`, and the
   toast is localized: `settings.invalidRedirectRule` with the 1-based row number of the
   draft for redirect rules, `settings.invalidSettingValue` with the entry label otherwise.
   The inline error clears on the next draft edit.
6. On any other error the toast is the shared localized API error (`toastApiError`) titled
   `settings.failedSave`. The raw server message MUST NOT be shown. The draft is kept.

SSU-22a. High-risk changes: `registration_enabled` to `true`, `captcha_enabled` to `false`,
`monoize_mask_sensitive_info` to `false`, `allow_free_when_unpriced` to `true`,
`allow_free_when_missing_usage` to `true`, and `monoize_request_capture_enabled` to `true`.
A change counts only when the server value differs. The `AlertDialog` lists each matching
entry with its label and risk text and offers "back" and a destructive "save anyway".

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
`settings.searchNoMatchDescription`, `settings.unsavedChangesCount`,
`settings.discardChanges`, `settings.saved`, and every group title of SSU-1a) MUST exist
in all four locales (`en`, `zh`, `zh-TW`, `ja`).

SSU-23c. Provider and Channel stay in English in every locale. Internal setting keys MUST NOT
appear as labels, except the whitelist sub-keys, which are protocol type ids.

SSU-25. This specification supersedes the numbered horizontal category rail, the oversized
serif category header band, and the stacked-card composition of earlier revisions.

## 8. API contract

SSU-26. `GET` and `PUT /api/dashboard/settings` MUST NOT contain `default_user_role`,
`monoize_passive_min_samples`, or `monoize_passive_failure_rate_threshold`. `PUT` ignores
these keys when a client still sends them. A migration deletes their `system_settings` rows,
and startup MUST NOT reseed them.

SSU-27. Invalid `PUT /api/dashboard/settings` values MUST return HTTP 400 with code
`invalid_request` and `param` naming the offending field:

| condition | `param` |
|-----------|---------|
| `session_ttl_days <= 0` | `session_ttl_days` |
| `api_key_max_per_user <= 0` | `api_key_max_per_user` |
| `global_model_redirects` has more than 32 rules | `global_model_redirects` |
| rule `i` of `global_model_redirects` is invalid (0-based, in the request body) | `global_model_redirects[i].pattern`, or `global_model_redirects[i].replace` when only `replace` is blank |
| `tool_prices` is invalid | `tool_prices` |
| `recharge_public_origin` is invalid | `recharge_public_origin` |

SSU-28. The page resolves `param` to an entry by exact entry id or API field name.
`global_model_redirects[i].*` resolves to `global_model_redirects` and row `i` of the rules
sent, mapped back to the draft row (SSU-22 step 3 removes blank rows before sending).
