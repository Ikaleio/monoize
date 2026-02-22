# Dashboard Home Overview Spec

## Scope

This spec defines expected behavior for `GET /dashboard` frontend page in the admin console.

## Rendering Contract

DH-1. The page MUST render three visual rows in this order:
- row A: greeting only (no action controls);
- row B: overview cards;
- row C: analysis panel and API information panel.

DH-2. Row B layout MUST be responsive:
- `< md`: 1 column;
- `md` to `< xl`: 2 columns;
- `>= xl`: 4 columns.

DH-3. Each overview card MUST contain:
- exactly two metric rows (`label`, `value`);
- compact metric rows with no embedded chart and no decorative metric icons.
- overview card section title typography MUST be one size smaller than row C section title typography.
- overview card internal top spacing MUST be compact (reduced header/content padding) to avoid excessive top whitespace.

DH-4. Row C left panel MUST contain:
- one title row;
- exactly four analysis tabs (`消耗分布`, `消耗趋势`, `调用次数分布`, `调用次数排行`);
- one analysis chart rendered with `@/components/ui/chart` + Recharts `BarChart`.
- analysis values MUST be computed from real request log rows (`GET /api/dashboard/request-logs`) and MUST NOT use synthetic fallback matrix generation.
- title row MUST render without decorative section icon.
- tab strip MUST be rendered on the same horizontal row as the section title and right-aligned.
- visual separator `/` between tabs MUST NOT be part of active-tab underline.
- chart heading MUST be a level-2 heading that follows active tab label.
- chart heading and `总计` text MUST be rendered in the same horizontal row.
- for `调用次数排行` tab, ranking key MUST use provider dimension (`providers[]`) rather than channel dimension.

DH-5. Row C right panel MUST contain downstream API information:
- data source: `api_base_url` field from `GET /api/dashboard/settings/public`;
- if `api_base_url` is empty, show explicit empty state text directing user to system settings;
- if `api_base_url` is non-empty, show:
  - the configured API base URL;
  - derived endpoint paths: `/v1/chat/completions`, `/v1/responses`, `/v1/models`.

DH-5a. Row C right panel (API information) MUST be visible to all authenticated dashboard users (including non-admin `user` role). It MUST NOT depend on admin-only endpoints.

## Data Source Contract

DH-6. Provider-derived metrics shown on dashboard home MUST use `GET /api/dashboard/providers` data when available.

DH-7. The page MUST NOT throw runtime exceptions when optional config fields are missing from `GET /api/dashboard/settings`.

DH-8. Row C analysis charts MUST be driven by the server-side analytics endpoint `GET /api/dashboard/analytics`.

### Analytics Endpoint Contract

- **Endpoint:** `GET /api/dashboard/analytics`
- **Authorization:** Any authenticated dashboard user.
- **Query parameters:**
  - `buckets: integer` (default 8, clamped to [1, 48])
  - `range_hours: integer` (default 24, clamped to [1, 720])
- **Behavior:**
  - The server computes `time_from = NOW() - range_hours` and `time_to = NOW()`.
  - For admin users: aggregates across ALL users' request logs.
  - For non-admin users: aggregates only the requesting user's logs.
  - Bucket boundaries: `bucket_width = range_hours / buckets`. Each bucket `i` covers `[time_from + i * bucket_width, time_from + (i+1) * bucket_width)`.
  - Per bucket, the server groups by `model` and by `provider_id`, computing:
    - `cost_nano_usd: SUM(charge_nano_usd)` — total cost per model per bucket.
    - `call_count: COUNT(*)` — total calls per model (or provider) per bucket.
  - Only models/providers with nonzero totals across all buckets are included.
- **Response:**

```json
{
  "buckets": [
    {
      "label": "MM-DD HH:00",
      "cost_by_model": { "model-a": 12345, "model-b": 678 },
      "calls_by_model": { "model-a": 5, "model-b": 2 },
      "calls_by_provider": { "provider-x": 4, "provider-y": 3 }
    }
  ],
  "time_from": "ISO 8601 string",
  "time_to": "ISO 8601 string",
  "total_cost_nano_usd": 13023,
  "total_calls": 7,
  "today_cost_nano_usd": 8000,
  "today_calls": 4
}
```

- `cost_by_model` values are integers in nano-USD.
- `calls_by_provider` keys use the human-readable provider name (from `monoize_providers.name`) when available, falling back to `provider_id`.
- Models/providers with zero total cost or zero total calls across ALL buckets MUST be omitted from the response entirely.

## Motion Contract

DH-9. The page MUST use `framer-motion` for:
- page entry transition on header and row C panels;
- staggered entry of row B cards;
- hover lift on row B cards.

## Loading Contract

DH-10. Before required dashboard data resolves, the page MUST render skeleton placeholders for row A, row B, and row C.

DH-11. `/dashboard` row C analysis section MUST render chart visualization, not tabular list.

DH-12. Row C layout MUST be container-responsive:
- the analysis panel MUST fit within viewport width without horizontal overflow at mobile, tablet, and desktop widths;
- the chart MUST resize with its card container.

DH-13. In desktop layout, row C left analysis card and right API info card MUST be vertically aligned to equal row height.

DH-14. In desktop layout, `/dashboard` MUST avoid page-level vertical overflow for normal data volumes:
- row C cards MUST adapt to remaining viewport height;
- overflow content inside row C cards MUST scroll within the card, not expand page height.

## i18n Contract

DH-15. Every user-visible string in the dashboard page MUST be wrapped in an i18n translation call.
DH-16. All hardcoded fallback strings passed to the translation helper (`tt()` / `t()`) MUST be in English (en). Chinese or other non-English fallbacks are forbidden in source code.
DH-17. Corresponding translation keys MUST exist in both `locales/en.json` and `locales/zh.json`.
