# Dashboard Home Overview Spec

## Scope

This spec defines expected behavior for `GET /dashboard` (route `/dashboard`) in the
user console. It is the single source of truth for the home overview layout, data
sources, motion, and loading contracts. Where this file conflicts with section 5 of
`dashboard-ui-layout.spec.md`, this file takes precedence for `/dashboard` home content;
`dashboard-ui-layout.spec.md` section 5 MUST be kept aligned with this file.

## Page Composition

DH-1. The page MUST render these sections in this vertical order inside the dashboard
content scroll pane:

1. greeting header;
2. account strip (balance + subscription);
3. usage chart panel;
4. recent usage table and API information panel (side-by-side on `lg+`, stacked below);
5. performance panel.

DH-2. The page MUST use a compact Vercel/shadcn card layout. Decorative metric icons
inside metric value rows are forbidden. Section titles MUST use nonlinear typography:
display font (`font-display`) for the greeting title; `text-base font-semibold
leading-none tracking-tight` for card/section titles unless a section specifies
otherwise.

DH-3. Desktop content MAY scroll vertically inside the main content pane (DL7). The page
MUST NOT create page-level horizontal overflow at mobile, tablet, or desktop widths.

## Greeting Header

DH-4. Row A MUST render greeting only (no action controls). Title text MUST come from
i18n key `dashboard.greeting` with `username` interpolation. Subtitle MUST come from
`dashboard.subtitle`.

## Account Strip

DH-5. The account strip MUST contain exactly two compact cards on `md+` (1 column below
`md`):

- Balance card;
- Subscription card.

DH-5a. Balance card metrics MUST be sourced from the authenticated session user
(`GET /api/dashboard/auth/me`), not from admin-only billing-plan endpoints:

- primary value = localized unlimited label when `balance_unlimited` is true; otherwise
  `balance_usd` formatted as USD with 2 fractional digits via `formatUsdDecimal`;
- secondary label MUST identify the value as current balance.

DH-5b. Subscription card MUST be sourced from the same session user object:

- when `billing_plan` is null: localized no-plan label; grant, remaining-quota progress,
  and reset rows MUST be absent;
- when `billing_plan` is non-null: show `billing_plan.name`, remaining quota, and reset
  time;
- remaining quota: when `balance_unlimited` is true, show the unlimited label; otherwise
  show `balance_usd` vs `grant_amount_usd` and a single-row progress bar whose filled
  fraction is `clamp(balance_nano_usd / grant_amount_nano_usd, 0, 1)` with `BigInt`
  arithmetic when `grant_amount_nano_usd` parses as an integer greater than 0;
- reset time: the session user's `next_grant_at` (top-level field on the user object, NOT
  a `billing_plan` property) localized via `toLocaleString()` when present; otherwise a
  localized unavailable label;
- schedule MAY appear as secondary monospace text (`billing_plan.schedule`).

DH-5c. The account strip MUST NOT display `my_api_keys_count`.

## Usage Chart Panel

DH-6. The usage panel MUST occupy the full content width and MUST render:

- title from `dashboard.usage.title` (default English: "Your Usage");
- subtitle from `dashboard.usage.subtitle` (default English: "Cumulative token usage
  for the selected time range");
- a time-window control at the top-right of the card header (DH-6i);
- a horizontal stacked cumulative area chart;
- a vertically scrolling legend below the chart (NOT a multi-column wrapping legend and
  NOT a paginated page-flip legend).

The panel MUST NOT render a "Group By" control.

DH-6a. Chart data MUST come from `GET /api/dashboard/analytics` with query parameters
derived from the selected window `W`:

| `W`   | `range_hours` | `buckets` | bucket width |
| ----- | ------------- | --------- | ------------ |
| `1h`  | 1             | 12        | 5 minutes    |
| `24h` | 24            | 24        | 1 hour       |
| `7d`  | 168           | 7         | 1 day        |
| `30d` | 720           | 30        | 1 day        |

This mapping MUST be defined in exactly one shared frontend module consumed by both the
usage chart and the recent usage panel. The frontend MUST NOT invent synthetic fallback
matrix values.

DH-6b. Each analytics bucket MUST include `tokens_by_model: Record<string, number>` where
each value is the exact integer sum of
`COALESCE(input_tokens,0) + COALESCE(output_tokens,0) + COALESCE(cache_read_tokens,0) +
COALESCE(cache_creation_tokens,0) + COALESCE(reasoning_tokens,0)` for that model in that
bucket. The token aggregate MUST be wrapped in `CAST(... AS BIGINT)` so that PostgreSQL
`SUM(bigint)` (type `NUMERIC`) and SQLite `SUM` (type `INTEGER`) both decode into the
same `i64` column; an uncast `SUM` MUST NOT be used because PostgreSQL decoding then
fails with a NUMERIC/INT8 mismatch. Models with zero total tokens across all buckets MUST
be omitted.

DH-6c. The chart Y-axis MUST represent cumulative tokens: for each model series at bucket
index `i`, the plotted value equals the sum of that model's `tokens_by_model` over
buckets `0..i` inclusive. The X-axis MUST show one label per bucket. Bucket start
instants MUST be computed on the frontend as
`start_i = time_from + i * (time_to - time_from) / bucket_count` from the analytics
response `time_from` / `time_to` fields. The label format depends on the selected window
and uses the browser's local time zone:

- `1h` and `24h`: 24-hour clock time `HH:mm`, both fields zero-padded;
- `7d` and `30d`: English short month name plus day of month (e.g. `Jan 27`).

When `time_from` or `time_to` does not parse as a timestamp, the frontend MUST fall back
to reformatting the backend bucket `label` (`MM-DD HH:00`) as short month plus day.

DH-6d. The chart MUST mark the final bucket (the bucket whose interval contains the
request instant) with a vertical dashed reference line at that bucket's X label. The
line label MUST be:

- localized "Today" (`dashboard.usage.today`) when the selected window is `7d` or `30d`
  (the final day-width bucket overlaps the current local calendar day);
- localized "Now" (`dashboard.usage.now`) when the selected window is `1h` or `24h`.

DH-6e. Hover/focus tooltip MUST show:

- the bucket label per DH-6c, with a header labeled by `dashboard.usage.periodBreakdown`
  (default English: "Period breakdown");
- per-model per-bucket (non-cumulative) token counts for models with nonzero tokens in
  that bucket, sorted descending by bucket tokens, each with percentage of that bucket's
  total;
- bucket total tokens labeled by `dashboard.usage.periodTotal` (default English:
  "Period total");
- cumulative total tokens through that bucket.

Tooltip copy MUST NOT hardcode a "daily" period because bucket width varies with the
selected window. Token display MUST use compact SI-style formatting (e.g. `10M`, `1.2B`)
with at most one fractional digit when abbreviated.

DH-6f. The legend MUST list every model present in the chart series as a vertical list
inside a bounded-height `ScrollArea` (max height approximately 5–6 rows). Each row shows
color swatch + model id in monospace. Legend color for a model MUST equal the chart
series color for that model.

DH-6g. Chart series colors MUST use CSS variables `--chart-1` … `--chart-16` (stable hash
of model id → palette index is allowed).

DH-6h. The chart MUST be rendered with `@/components/ui/chart` and Recharts `AreaChart` /
stacked `Area` elements. Empty analytics MUST show an `EmptyState` instead of an empty
axes frame with fake data.

DH-6i. Time-window control contract (shared by DH-6 and DH-7):

- the control MUST be a segmented button group of exactly four options with the literal
  ASCII labels `1h`, `24h`, `7d`, `30d`, in that order. These labels are canonical
  product tokens and MUST NOT be translated in any locale;
- the group MUST carry a localized `aria-label` from `dashboard.usage.timeRange`
  (default English: "Time range"). Each option MUST be a real `<button>` element with
  `aria-pressed` reflecting its selection state, and MUST be keyboard operable;
- the default selected window MUST be `1h` on every mount of `/dashboard`;
- the selection MUST be held only in React component state. It MUST NOT be written to
  `localStorage`, `sessionStorage`, cookies, or the URL.

## Recent Usage Panel

DH-7. The recent usage panel header MUST render its own time-window control that follows
DH-6i. Its selection state is independent of the usage chart's selection; each panel
holds its own React state, both defaulting to `1h`.

DH-7a. Table rows MUST be per-model aggregates computed from the authenticated user's
own request logs (`GET /api/dashboard/request-logs` with the same user scoping the API
already applies), fetched with these query parameters for the selected window `W`:

- `time_from = now − range_hours(W)` and `time_to = now`, both ISO-8601, where
  `range_hours(W)` uses the DH-6a mapping and `now` is evaluated at fetch time (so each
  SWR revalidation observes logs created after the window was selected);
- `limit = 200`, `offset = 0`. The aggregate is page-limited: when more than 200 log
  rows exist inside the window, only the 200 most recent rows are aggregated.

Columns MUST be:

- Model (monospace model id);
- Tokens (sum of input + output + cache_read + cache_creation + reasoning when present);
- Cache hit rate (`cache_read / input` when input > 0; em dash otherwise; format per
  `user-live-usage.spec.md` LU-11);
- Charge (sum of `billing.charge_nano_usd` as USD via `formatNanoUsd` / `BigInt`).

Rows MUST sort by Tokens descending. Models with zero tokens and zero charge MUST be
omitted. While logs are first loading with no data for the panel, the panel MUST show a
skeleton table (DH-12a). When zero rows match the selected window, the panel MUST show
the localized empty state.

## API Information Panel

DH-8. The API information panel MUST be visible to all authenticated dashboard users
(including non-admin `user` role). It MUST NOT depend on admin-only endpoints.

DH-8a. Data source: `api_base_url` from `GET /api/dashboard/settings/public`.

DH-8b. If `api_base_url` is empty, show an explicit empty state directing the user to
system settings. If non-empty, show:

- the configured API base URL;
- derived endpoint paths: `/v1/chat/completions`, `/v1/responses`, `/v1/models`,
  `/v1/messages`.

DH-8c. Clicking a base URL or endpoint row MUST copy the full absolute URL to the
clipboard and toast a localized copied confirmation.

DH-8d. `GET /api/dashboard/settings/public` MUST continue to read only the setting keys
defined for that endpoint in one set-based database query. It MUST NOT load transforms,
redirects, pricing patterns, suffix maps, performance targets, or other unrelated
settings.

## Performance Panel

DH-9. The performance panel MUST show platform performance for admin-configured targets.

DH-9a. Admin configuration lives in system settings fields:

- `dashboard_performance_group_ids: string[]` (default `[]`);
- `dashboard_performance_model_ids: string[]` (default `[]`).

These fields MUST be editable only through `GET/PUT /api/dashboard/settings` by admin /
super_admin. They MUST appear in the `health` settings category on
`/dashboard/admin-settings` as multi-select controls (groups via the shared unordered
group selector; models via a searchable checkbox list of available Channel model keys,
same availability rules as the Codex model picker).

DH-9b. `GET /api/dashboard/performance` MUST be available to any authenticated dashboard
user and MUST return:

```json
{
  "groups": [
    {
      "id": "group-id",
      "name": "Group Name",
      "bricks": [{ "index": 0, "status": "up" }],
      "avg_ttft_ms": 120.5,
      "avg_tps": 42.1
    }
  ],
  "models": [
    {
      "id": "model-id",
      "bricks": [{ "index": 0, "status": "up" }],
      "avg_ttft_ms": 90.0,
      "avg_tps": 55.2
    }
  ],
  "brick_count": 24,
  "window_hours": 24
}
```

Semantics:

- `brick_count` MUST be 24 and `window_hours` MUST be 24. Brick `index` `0` is the oldest
  hour in `[NOW-24h, NOW)`; index `23` is the newest hour.
- `status` MUST be one of `up`, `degraded`, `down`, `empty`:
  - `empty`: zero finished requests in that hour for the target;
  - `up`: success rate ≥ 0.99 among finished requests in that hour;
  - `degraded`: success rate ≥ 0.95 and < 0.99;
  - `down`: success rate < 0.95.
  A finished request is any request log row whose `status` is not an in-flight value;
  success means `status` is `success` or `client_gone`.
- `avg_ttft_ms` MUST be the arithmetic mean of positive `ttfb_ms` values over the 24h
  window for the target, or `null` when no such samples exist.
- `avg_tps` MUST be the arithmetic mean of per-request TPS values computed with the same
  rules as request-log display TPS (`request-logs.spec.md` FL4a) over the 24h window, or
  `null` when no samples exist.
- Group rows: include every id in `dashboard_performance_group_ids` that still exists in
  the groups registry (unknown ids omitted). Metrics aggregate request logs whose
  `provider_id` belongs to a Provider whose `group_ids` contains that group id. Admin
  aggregation is global; non-admin callers still receive the same configured targets and
  global aggregates (status page semantics).
- Model rows: include every id in `dashboard_performance_model_ids` (order preserved).
  Metrics aggregate request logs whose `model` equals that id. Global aggregates as above.
- When both configured lists are empty, `groups` and `models` MUST be empty arrays and
  the UI MUST show an empty state. Non-admin empty copy MUST NOT instruct the user to
  open admin settings with an admin-only deep link requirement; it MAY say performance
  targets are not configured.

DH-9c. The performance panel UI MUST render, for each returned group and model row:

- name/id;
- a horizontal uptime brick strip (`brick_count` equal-sized squares, color-coded by
  status using success/warning/destructive/muted tokens);
- `avgTTFT` formatted as milliseconds with at most 1 fractional digit, or em dash;
- `avgTPS` formatted with at most 2 fractional digits and a `t/s` suffix, or em dash.

## Analytics Endpoint Extensions

DH-10. `GET /api/dashboard/analytics` keeps the existing authorization, clamping, bucket
math, cost, and call contracts from the previous revision of this spec, and MUST
additionally:

- select and group token sums per model bucket as defined in DH-6b;
- include `tokens_by_model` on every bucket object in the JSON response;
- continue to return `cost_by_model`, `calls_by_model`, `calls_by_provider`, totals, and
  today fields unchanged in meaning.

DH-10a. Cost fields remain signed base-10 integer strings in nano-USD with checked `i128`
aggregation. Token and call counts are integers. The frontend MUST NOT narrow monetary
strings through JavaScript `Number` before exact totals and comparisons are complete.
Chart libraries MAY receive derived bounded display numbers for tokens.

## Motion Contract

DH-11. The page MUST use `framer-motion` with the shared motion helpers
(`components/ui/motion.tsx`) for:

- page entry on the greeting and major panels;
- staggered entry of account strip cards;
- staggered entry of performance rows;
- hover lift on account strip cards only (interactive opt-in; base Card remains static).

All motion MUST respect reduced-motion rules (DS32–DS34).

## Loading Contract

DH-12. Before required dashboard data resolves, the page MUST render skeleton
placeholders that mirror the ready layout: greeting, account strip, usage chart,
recent/API row, and performance panel.

DH-12a. A panel MUST show its skeleton only during the first load of that panel: while
its SWR hook reports `isLoading` AND the hook exposes no data (neither current nor
kept-previous data). A rendered chart or table MUST NOT be swapped back to a skeleton
because SWR is revalidating an already-populated key (`isValidating` MUST NOT unmount
panel content).

DH-12b. The usage-chart analytics hook and the recent-usage request-logs hook MUST use
SWR `keepPreviousData: true`. When the user switches windows and data for a previously
viewed window exists in the SWR cache, the panel MUST keep rendering the previous
window's data until the new payload resolves, and MAY indicate the pending fetch with a
non-layout-shifting treatment (reduced content opacity). The panel MUST NOT show a
skeleton during this transition. A window switch before any data has ever resolved
still shows the skeleton.

DH-12c. The usage chart's Card and its `framer-motion` entry wrapper MUST remain mounted
across revalidations and window switches; page-entry motion plays at most once per page
mount. Recharts `Area` elements MUST set `isAnimationActive={false}` so a background
data refresh cannot replay the grow-from-zero enter animation. The recent-usage panel
follows the same mount and motion rules.

DH-13. Individual panels MAY resolve independently via SWR. A panel with data MUST render
even if a sibling panel is still loading, provided the page-level critical session user
object is available. While the session user is unresolved, the account strip MUST show
skeletons.

## i18n Contract

DH-14. Every user-visible string on the page MUST be wrapped in an i18n translation call.
DH-15. All hardcoded fallback strings passed to the translation helper MUST be in English
(en). Chinese or other non-English fallbacks are forbidden in source code.
DH-16. Corresponding translation keys MUST exist in `locales/en.json`, `locales/zh.json`,
`locales/zh-TW.json`, and `locales/ja.json`.
