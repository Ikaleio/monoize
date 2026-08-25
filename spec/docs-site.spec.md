# Documentation Site Specification

## 0. Scope

- Product name: Monoize.
- Scope: the user-facing static documentation site under `docs/`.
- Audience: operators and API consumers of Monoize. The site documents setup, usage, and troubleshooting. It does not document Rust internals or the URP wire format.

## 1. Technology

DOC-1. The documentation site MUST live in the `docs/` directory as a package separate from `frontend/`.

DOC-2. The site MUST use Fumadocs with the Next.js static-export template (`output: 'export'`).

DOC-3. The package manager for `docs/` MUST be Bun. `cd docs && bun install && bun run build` MUST exit with code `0` and write the static site to `docs/out`.

DOC-4. The build output MUST be deployable to Vercel and Cloudflare Pages as a static site without further configuration changes. The repository MUST contain `docs/vercel.json` and `docs/public/_redirects` so both hosts redirect `/` to `/en`.

DOC-5. Content pages MUST be MDX files under `docs/content/docs`.

DOC-6. The Vercel Git build MUST continue when the current commit changes at least one path under `docs/`. The build MUST be ignored when the current commit changes no path under `docs/`.

## 2. Locales

DOC-10. The site MUST support exactly these locales: `en` (default), `zh`, `zh-TW`, `ja`. They match the frontend locale set in `frontend/src/locales`.

DOC-11. Every route MUST be prefixed with its locale (`/en/...`, `/zh/...`, `/zh-TW/...`, `/ja/...`). The locale prefix MUST NOT be hidden for the default locale.

DOC-12. Localized page files MUST use the Fumadocs suffix convention: `page.mdx` for `en`, `page.zh.mdx`, `page.zh-TW.mdx`, and `page.ja.mdx`.

DOC-13. Every page listed in the main navigation tree MUST exist in all four locales. A missing localized file is a defect, not an allowed fallback.

DOC-14. The root URL `/` MUST redirect to a locale root. The static export MUST contain a client-side redirect page, and the host-level redirect files from DOC-4 MUST target `/en`.

DOC-15. The UI chrome (search dialog, table of contents, pagination, theme switcher, language switcher) MUST render translated strings for `zh`, `zh-TW`, and `ja`.

## 3. Content structure

DOC-20. The navigation tree MUST contain exactly these top-level entries in this order:

1. Introduction (`index.mdx`)
2. Quick Start (`quick-start.mdx`)
3. Configuration (`configuration.mdx`)
4. Dashboard (`dashboard/`: overview, providers and channels, models, API keys)
5. Request Logs (`request-logs.mdx`)
6. Routing and Reliability (`routing.mdx`)
7. API Endpoints (`endpoints.mdx`)
8. Transforms (`transforms/`)
9. Troubleshooting (`troubleshooting.mdx`)

DOC-21. The Transforms section MUST contain one overview page plus one page per built-in transform. The set of transform pages MUST equal the canonical transform ID list in `spec/urp-transform-system.spec.md` TF-7 (33 transforms). Each transform page filename MUST equal its canonical `type_id` plus the locale suffix.

DOC-22. Each transform page MUST state: the transform `type_id`, the phase(s), the supported scopes, every config property with its type and default, at least one JSON config example, and at least one situation in which an operator should enable the transform.

DOC-23. Content MUST describe observable behavior only. Statements about defaults, limits, environment variables, endpoints, and transform behavior MUST agree with the specs under `spec/` and the implementation under `src/`.

## 4. Writing style

DOC-30. All prose MUST follow Simplified Technical English conventions:

1. use imperative mood for instructions;
2. use active voice;
3. one instruction per sentence;
4. keep sentences short (target at most 25 words);
5. use one term per concept (for example, always "Provider", never a synonym).

DOC-31. Marketing vocabulary is forbidden. Banned words include: "seamless", "powerful", "revolutionary", "blazing", "effortless", "world-class".

DOC-32. Translations MUST be written as native technical prose for each locale. Word-for-word translationese is a defect.

DOC-33. Product nouns (Provider, Channel, transform `type_id` values, environment variable names, endpoint paths) MUST remain in their canonical English form in all locales.

## 5. Math rendering

DOC-40. The MDX pipeline MUST enable `remark-math` and `rehype-katex`, and the site MUST load the KaTeX stylesheet.

DOC-41. The Routing and Reliability page MUST contain at least one KaTeX-rendered formula (weighted channel selection) in every locale, and the formula MUST render as KaTeX HTML output (`.katex` class present) in the exported site.

## 6. Visual identity

DOC-50. The site theme MUST reuse the Monoize color tokens from `frontend/src/index.css`: primary `hsl(217 91% 53%)` in light mode and `hsl(217 91% 60%)` in dark mode, neutral background/card/border values from the same file.

DOC-51. The site MUST use at most two font families for text: `Noto Serif SC` for display headings and `Noto Sans SC` (with the frontend CJK fallback stack) for body text. Code MUST use the frontend mono stack.

DOC-52. Custom components on the landing page MUST use shadcn/ui (new-york style) primitives and semantic design tokens. Raw Tailwind palette colors are forbidden for repeated semantic states.

DOC-53. Icons MUST come from `lucide-react`. Emoji characters MUST NOT be used as icons.

## 7. Screenshots

DOC-60. Screenshots of the Monoize dashboard MUST be stored as WebP files under `docs/public/images`.

DOC-61. Two screenshot sets MUST exist: `docs/public/images/zh/` captured with the frontend locale set to Simplified Chinese, and `docs/public/images/en/` captured with the frontend locale set to English.

DOC-62. Pages in the `zh` locale MUST reference the `zh` screenshot set. Pages in `en`, `zh-TW`, and `ja` MUST reference the `en` screenshot set.

DOC-63. When a UI change alters a documented flow, the affected screenshots MUST be recaptured in both sets in the same change.

## 8. Maintenance invariants

DOC-70. When a change alters observable user-facing behavior that the site documents, the same change MUST update the affected pages in all four locales.

DOC-71. When a transform is added to or removed from TF-7, the same change MUST add or remove the matching transform pages in all four locales and update the transforms overview page.
