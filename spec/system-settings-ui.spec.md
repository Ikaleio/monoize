# System Settings UI Specification

## 0. Scope

- Product name: Monoize.
- Scope: layout, interaction, and state contract of the admin system settings page at
  `/dashboard/admin-settings` (`frontend/src/pages/settings.tsx` and
  `frontend/src/components/settings/*`).
- Data contract: the page reads `GET /api/dashboard/settings` and writes
  `PUT /api/dashboard/settings`. The existing settings fields retain their types and
  persistence behavior.
- The system settings data contract includes `models_dev_base_url`, a persisted complete
  HTTP(S) URL used by the Model Database Models.dev synchronization control. The field is
  edited in the Site Information category.
- Field-level behavior requirements from `dashboard-ui-layout.spec.md` (ST1-ST7) remain in
  force. Where ST statements describe the container as a "card", this specification
  supersedes the container shape: the container is a category panel per SSU-14.

## 1. Category model

SSU-1. The page MUST partition the editable settings fields into exactly 9 categories with
these stable ids, in this order:

| # | id | title key | fields |
|---|----|-----------|--------|
| 01 | `site` | `settings.siteInformation` | `site_name`, `site_description`, `api_base_url`, `models_dev_base_url` |
| 02 | `access` | `settings.accessControl` | `registration_enabled`, `default_user_role`, `captcha_enabled`, `session_ttl_days`, `api_key_max_per_user` |
| 03 | `codex` | `settings.codexModels` | `codex_model_ids` |
| 04 | `suffix` | `settings.reasoningSuffixMap` | `reasoning_suffix_map` |
| 05 | `redirects` | `settings.globalModelRedirects` | `global_model_redirects` |
| 06 | `transforms` | `settings.globalTransforms` | `global_transforms` |
| 07 | `affinity` | `settings.affinityRouting` | `monoize_affinity_enabled`, `monoize_affinity_failback_mode`, `monoize_affinity_idle_ttl_seconds`, `monoize_affinity_failback_delay_seconds` |
| 08 | `health` | `settings.healthMonitoring` | `monoize_active_probe_enabled`, `monoize_active_probe_interval_seconds`, `monoize_active_probe_success_threshold`, `monoize_active_probe_model`, `monoize_passive_failure_threshold`, `monoize_passive_cooldown_seconds`, `monoize_passive_window_seconds`, `monoize_passive_min_samples`, `monoize_passive_failure_rate_threshold`, `monoize_passive_rate_limit_cooldown_seconds`, `monoize_request_capture_enabled`, `monoize_mask_sensitive_info`, `monoize_request_capture_retention_days`, `monoize_enable_estimated_billing`, `monoize_strip_cross_protocol_nested_extra`, `monoize_request_timeout_ms` |
| 09 | `extra` | `settings.extraFieldsWhitelist` | `monoize_extra_fields_whitelist` (sub-keys `chat_completion`, `responses`, `messages`, `gemini`) |

SSU-2. Every field listed in SSU-1 MUST be editable through exactly one category panel.
No field present in the pre-redesign page may become unreachable.

SSU-3. `pricing_profile_model_patterns` and `updated_at` are not edited on this page.
Save MUST pass them through unchanged from the current draft object.

## 2. Horizontal category rail

SSU-4. Below the page header the page MUST render exactly one horizontal category rail
that lists all 9 categories of SSU-1 in SSU-1 order.

SSU-5. The rail MUST lay its items out in a single horizontal row. The row MUST NOT wrap.
When the row's content width exceeds the available width, the rail MUST scroll
horizontally inside its own scroll container; it MUST NOT create page-level horizontal
overflow.

SSU-6. The rail MUST expose `role="tablist"` with `aria-label` resolved from the i18n key
`settings.categoryRailLabel`. Each category item MUST expose `role="tab"` with a correct
`aria-selected` state. The visible category container MUST expose `role="tabpanel"` and be
associated with its tab (Radix Tabs wiring satisfies this).

SSU-7. With focus on a rail tab, `ArrowRight` MUST move focus and selection to the next
tab and `ArrowLeft` to the previous tab (Radix Tabs roving-tabindex automatic activation).

SSU-8. Each rail item MUST render two text elements: a zero-padded 2-digit index
(`01` through `09`) using the `font-display` family, and the category title resolved from
the SSU-1 title key.

SSU-9. Exactly one category is active at any time. The initial active category on page
load MUST be `site`. Active-category state is client-side view state only; it MUST NOT be
persisted to the backend or to browser storage.

SSU-10. When a category becomes active while its rail item is partially or fully outside
the rail's visible scroll area, the rail MUST scroll that item into view.

SSU-11. The active rail item MUST show a primary-colored indicator that moves between
items through the shared layout animation helper (`SharedTabIndicator`). Motion MUST
respect DS32-DS34 reduced-motion rules.

## 3. Category panel

SSU-12. Exactly one category panel MUST be mounted and visible at a time, containing only
the fields assigned to the active category by SSU-1.

SSU-13. Each panel MUST begin with a header band containing: the 2-digit index numeral,
the category title in the `font-display` family at `text-3xl` or larger, and the category
description in `text-muted-foreground`. At `lg` and above the header band MUST use a
two-column asymmetric grid where the title column is wider than the description column.

SSU-14. Panels MUST NOT wrap their content in the shared `Card` component. Panel chrome is
limited to the SSU-13 header band plus hairline (`border` token) separators between field
subgroups.

SSU-15. Field-cluster density requirements:

- Sibling numeric inputs inside one subgroup MUST be arranged in a grid of at least 2
  columns at `sm` and above.
- Boolean fields MUST render as horizontal rows (label and description left, switch
  right), and sibling switch rows inside one subgroup MUST be arranged in a grid of at
  least 2 columns at `lg` and above when the subgroup contains more than one switch.
- Below `sm`, all clusters MUST collapse to one column.

SSU-16. Switching the active category MUST NOT discard unsaved edits: draft state lives in
page-level state (SSU-19) and MUST survive panel unmount/remount.

SSU-17. On category change the incoming panel MAY animate opacity and a horizontal offset
of at most 8px, exactly once per change. Under reduced motion the panel MUST animate
opacity only or render without animation.

## 4. State, save, and validation (behavior preserved)

SSU-18. Data fetching MUST use the existing SWR hooks `useSettings`, `useProviders`, and
`useTransformRegistry`. The page MUST NOT fetch inside `useEffect`.

SSU-19. Draft-state contract (unchanged from pre-redesign):

- the page keeps a `localSettings` draft; the rendered value is
  `localSettings ?? settings`;
- any field edit replaces `localSettings` with the merged draft;
- the save action is enabled if and only if `localSettings` is non-null and no save is in
  flight.

SSU-20. Save behavior MUST be byte-identical to the pre-redesign contract:

1. Validate `global_transforms` with `findFirstInvalidTransformRule` against the
   global-scope subset of the transform registry; on failure show the
   `transforms.validationRuleInvalid` toast and do not call the API.
2. Drop `global_model_redirects` entries whose `pattern` or `replace` trims to empty.
3. Persist through `updateSettingsOptimistic` (optimistic SWR update), then clear
   `localSettings`, show the saved label for 2000 ms, and revalidate.
4. On error show a toast with the error message or `settings.failedSave`.

SSU-21. While `useSettings` is loading, the page MUST render a settings skeleton inside
`PageWrapper` consisting of: a page-header skeleton, a rail skeleton with at least 4 chip
placeholders in one horizontal row, and a panel skeleton with a title placeholder and at
least a 2-column field grid placeholder.

SSU-22. When settings resolve to no data, the page MUST render the
`settings.failedLoad` message.

SSU-23. All user-visible strings MUST resolve through `react-i18next`. Keys introduced for
the rail/panel chrome (`settings.categoryRailLabel`, `settings.accessControl`,
`settings.accessControlDescription`, `settings.groupActiveProbe`,
`settings.groupPassiveBreaker`, `settings.groupRequestCapture`,
`settings.groupRuntimeBehavior`) MUST exist in all four locales (`en`, `zh`, `zh-TW`,
`ja`).

SSU-24. At viewport widths of 320px and above, the page MUST NOT create page-level
horizontal overflow. The only horizontal scroll container on the page is the rail's
internal scroll area (SSU-5). This restates ST6e for the redesigned layout.

SSU-25. This specification supersedes the previous stacked-card composition of the
settings page, including the per-card stagger entrance animation. Page-level motion for
this page is defined by SSU-11 and SSU-17 plus the existing page-header entrance.
