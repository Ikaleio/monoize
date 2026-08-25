# Monoize Design System

This document describes the Monoize visual language for designers, developers, and coding agents. The normative rules live in `spec/frontend-design-system.spec.md`. When this document and the spec disagree, the spec wins. Token values live in `frontend/src/index.css`; the docs site mirrors them in `docs/app/global.css`.

## Color

Use semantic tokens. Do not use raw Tailwind palette colors for repeated semantic states.

### Base tokens

| Token | Light | Dark | Use |
| --- | --- | --- | --- |
| `background` | `hsl(0 0% 98%)` | `hsl(0 0% 4%)` | Page background |
| `foreground` | `hsl(0 0% 9%)` | `hsl(0 0% 93%)` | Body text |
| `card` | `hsl(0 0% 100%)` | `hsl(0 0% 7%)` | Card surfaces |
| `border` | `hsl(0 0% 90%)` | `hsl(0 0% 15%)` | Borders, grid texture |
| `primary` | `hsl(217 91% 53%)` | `hsl(217 91% 60%)` | Primary actions, links, focus rings |
| `muted` | `hsl(0 0% 96%)` | `hsl(0 0% 10%)` | Subdued surfaces |

The palette is neutral gray plus one blue. Do not add new hues for chrome. Semantic status colors are the only other hues.

### Semantic status tokens

Each status has four forms: base, `-foreground` (text), `-soft` (background), and `-border`.

| Status | Light base | Dark base |
| --- | --- | --- |
| `success` | `hsl(142 76% 28%)` | `hsl(142 69% 58%)` |
| `warning` | `hsl(32 95% 36%)` | `hsl(45 93% 68%)` |
| `info` | `hsl(199 89% 32%)` | `hsl(199 89% 68%)` |
| `destructive` | `hsl(0 84% 60%)` | dark-tuned variant |

Rules:

- Text on a `-soft` background must reach a contrast ratio of at least 4.5:1 in both themes.
- Write status UI with the tokens (`bg-success-soft text-success-foreground border-success-border`). Never write `bg-green-100 text-green-800`.

### Charts

Chart series colors come from `--chart-1` through `--chart-16`. Use them in order.

## Typography

Monoize uses three font stacks. Set them through the CSS variables; do not hard-code font names in components.

| Variable | Stack | Use |
| --- | --- | --- |
| `--font-display` | `Noto Serif SC`, `Source Han Serif SC`, serif | Page titles, brand headings |
| `--font-sans-cjk` | `Noto Sans SC`, `PingFang SC`, `Microsoft YaHei`, `Segoe UI`, sans-serif | Body text |
| `--font-code` | `Sarasa Mono SC`, `SF Mono`, `Menlo`, `Consolas`, monospace | Code, IDs, model names |

Rules:

- Page titles use `font-display`.
- Card titles render as `text-base font-semibold leading-none tracking-tight`.
- Table cells in one table share one font size. Do not mix arbitrary smaller sizes between sibling cells.
- Badge text uses `font-medium` and never wraps.

## Surfaces

- The dashboard body uses the product grid texture: 1px `border`-colored lines on a 32px grid, masked toward the top. Do not remove it.
- Cards are static surfaces. No hover shadow, no hover transform, unless the call site opts in.
- Badge-shaped containers use `rounded-md`. `rounded-full` is reserved for status dots, avatars, switch parts, and loading indicators.

## Page structure

### Page headers

- Outer container: `flex flex-wrap items-center justify-between gap-4`.
- Title container: `min-w-0`; the title truncates when space is insufficient.
- Action container: `flex shrink-0 flex-wrap items-center gap-2`.

### Loading states

- Every page renders a skeleton while data loads. Use the shared skeleton components.
- A table page skeleton contains a header skeleton, a toolbar skeleton, and a content skeleton that matches the ready layout.
- A card-grid skeleton repeats card skeletons with the same grid columns as the ready state.

### Empty states

- Use the shared `EmptyState` component with an icon, a title, a description, and an optional action.
- Variants: `card` (bordered surface) and `inline` (no extra surface).

## Dialogs

- Destructive confirmation uses shadcn `AlertDialog`. Never use browser `confirm()`.
- Long dialogs bound their height to the viewport, scroll the body internally, and keep header and footer visible.

## Motion

- All shared motion helpers respect the reduced-motion preference. With reduced motion, animate opacity only or not at all.
- Buttons get hover/press scale only through the shared `AnimatedButton` wrapper: hover `1.02`, tap `0.98`.
- Non-interactive rows and decorative elements never get hover transforms. Hover feedback on such rows is a color change only.

## Icons

- Icons come from `lucide-react` or the `@lobehub/icons` provider set.
- Never use emoji as icons.
- The only hand-authored inline SVG is the Monoize brand mark.

## Internationalization

- All user-visible copy goes through the i18n system. The frontend supports `en`, `zh`, `zh-TW`, and `ja`.
- Keep product nouns in canonical English in every locale: Provider, Channel, transform type IDs, environment variables, endpoint paths.

## Documentation site

The docs site under `docs/` reuses this design language:

- The Fumadocs theme maps the tokens above onto `--color-fd-*` variables in `docs/app/global.css`.
- The landing page uses shadcn/ui primitives, `lucide-react` icons, and the grid texture.
- Documentation prose follows Simplified Technical English. See `spec/docs-site.spec.md`.
