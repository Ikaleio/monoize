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
