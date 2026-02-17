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
- data source: `api_base_url` field from `GET /api/dashboard/settings`;
- if `api_base_url` is empty, show explicit empty state text directing user to system settings;
- if `api_base_url` is non-empty, show:
  - the configured API base URL;
  - derived endpoint paths: `/v1/chat/completions`, `/v1/responses`, `/v1/models`.

## Data Source Contract

DH-6. Provider-derived metrics shown on dashboard home MUST use `GET /api/dashboard/providers` data when available.

DH-7. The page MUST NOT throw runtime exceptions when optional config fields are missing from `GET /api/dashboard/settings`.

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
