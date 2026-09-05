# Frontend Typography Specification

## 0. Scope

- Product name: Monoize.
- Scope: global typography rules for the embedded frontend.

## 1. Global Font Injection

FT1. Frontend stylesheet MUST globally load at least one Google Fonts web font that provides Chinese glyph coverage and is sans-serif.

FT2. The global font stack MUST put the injected CJK sans-serif web font before generic fallback families.

## 2. Global Application

FT3. `body` MUST use the CJK sans-serif global font stack.

FT4. Code-oriented elements (`code`, `pre`, `kbd`, `samp`) MUST keep a monospaced stack for readability.

FT5. If the injected web font fails to load, the stack MUST fall back to system sans-serif fonts without breaking rendering.

FT6. The frontend theme MUST expose a display font stack through `--font-display` and Tailwind `font-display`.

FT7. Dashboard page titles rendered by the shared page header MUST use the display font stack.

## 3. Control and Supporting Text Sizes

FT8. Shared Button labels, including the `sm` variant, MUST use at least 0.875rem.
Button size variants MUST NOT reduce the label below that size.

FT9. Dashboard panel descriptions, API endpoint labels, API endpoint values, and
usage-window controls MUST use at least 0.875rem. Supporting prose MUST use a
line height of at least 1.5.

FT10. Chart axis ticks, chart tooltips, and dense tabular metadata MAY use 0.75rem.
This exception MUST NOT apply to ordinary form actions or explanatory prose.
The root font size MUST NOT be reduced to implement density.
