# Frontend Design System Specification

## 0. Scope

- Product name: Monoize.
- Scope: shared visual and interaction rules for the embedded frontend under `frontend/src`.
- Style baseline: Vercel dashboard conventions and shadcn/ui primitives.

## 1. Surface and Color Tokens

DS1. Shared UI components MUST use CSS variables from `frontend/src/index.css` for base colors.

DS2. Tailwind theme colors MUST expose the base tokens used by shadcn/ui:

- `background`
- `foreground`
- `card`
- `popover`
- `primary`
- `secondary`
- `muted`
- `accent`
- `destructive`
- `border`
- `input`
- `ring`

DS3. Tailwind theme colors MUST expose semantic status tokens:

- `success`
- `warning`
- `info`

DS4. Semantic status tokens MUST provide at least these forms:

- base foreground color;
- foreground text color;
- soft background color;
- border color.

DS4b. Text rendered on semantic soft backgrounds MUST have a contrast ratio of at least 4.5:1 in both light and dark themes. This rule applies to `text-success-foreground` on `bg-success-soft`, `text-warning-foreground` on `bg-warning-soft`, and `text-info-foreground` on `bg-info-soft`.

DS4c. Dark-theme semantic foreground tokens MUST be lighter than their matching soft background tokens when the soft token is a dark surface. A dark foreground on a dark semantic soft surface is invalid.

DS4a. Chart series colors MUST be exposed as CSS variables `--chart-1` through `--chart-16` and Tailwind colors `chart.1` through `chart.16`.

DS5. Business UI MUST NOT introduce raw Tailwind status palettes for repeated semantic states when a status token exists. This rule applies to success, warning, info, and destructive states.

DS5a. Dashboard status soft backgrounds and borders SHOULD use lower saturation than their matching foreground tokens when contrast remains at least 4.5:1.

DS5b. The dashboard body background MAY use the product grid texture. Implementations MUST NOT remove the grid texture unless the product specification is updated.

## 2. Cards

DS6. Base `Card` MUST be a static surface by default.

DS7. Base `Card` MUST NOT apply hover shadow or hover transform by default.

DS8. Interactive card affordance MUST be opt-in by using a dedicated interactive wrapper or explicit classes at the call site.

DS8a. Dashboard card layout and floating-shell layout are product decisions. Vercel/shadcn alignment work MUST NOT remove floating card layout or body grid unless a separate specification change requires it.

## 3. Page Headers

DS9. Dashboard page headers that contain title text and actions MUST allow wrapping on narrow viewports.

DS10. A standard page header MUST use these layout properties:

- outer container: `flex flex-wrap items-center justify-between gap-4`;
- title container: `min-w-0`;
- action container: `flex shrink-0 flex-wrap items-center gap-2` when actions exist.

DS11. Page title text MUST be truncatable when horizontal space is insufficient.

## 4. Loading Skeletons

DS12. Dashboard page loading states MUST render inside `PageWrapper`.

DS13. Loading states MUST use shared skeleton components for repeated page shapes.

DS14. A table page loading state MUST include:

- a page header skeleton;
- a toolbar skeleton when the ready state contains a toolbar;
- a content skeleton that matches the primary table/card region.

DS15. A card-grid page loading state MUST include:

- a page header skeleton;
- one or more card skeletons with the same grid columns as the ready state when possible.

## 5. Empty States

DS16. Repeated empty states MUST use a shared `EmptyState` component.

DS17. Empty states MUST support at least these variants:

- `card`, which renders a bordered card surface;
- `inline`, which renders content without an extra card surface.

DS18. Empty states MUST accept an icon, title, description, and optional action.

## 6. Tables

DS19. Repeated table surfaces MUST use shared table shell components when the layout contains a toolbar or empty state.

DS19a. A table shell with `isEmpty = true` and `emptyState` provided MUST render the empty state instead of the table surface.

DS19b. Table toolbar search controls MUST support an inline leading search icon without changing the responsive width requirement in DS22.

DS20. Standard table rows MUST use `hover:bg-muted/50` for hover feedback.

DS21. Standard table cells MUST use deterministic horizontal and vertical padding.

DS22. Search inputs in table toolbars MUST use responsive width: full width below `sm`, bounded width at `sm` and above.

DS22a. Shared virtual table header cells SHOULD use `h-9 px-3 text-xs font-medium text-muted-foreground`.

DS22b. Shared virtual table body cells SHOULD use `px-3 py-2 align-middle` unless the table is explicitly high-density.

## 7. Dialogs and Confirmation

DS23. Destructive confirmation UI MUST use shadcn `AlertDialog` primitives.

DS24. Browser-native `confirm()` MUST NOT be used for dashboard destructive actions.

DS25. Long-form dialogs MUST keep header and footer reachable inside the visible viewport.

DS26. Long-form dialogs MUST place overflow on an internal body container when content exceeds viewport height.

DS26a. Long-form dialog content MUST set viewport-bounded max height and `overflow-hidden` on the outer content element.

DS26b. Long-form dialog footers MUST be `shrink-0` so actions remain reachable while the dialog body scrolls.

DS27. Dialog action footers MUST use shadcn button variants.

## 8. Form Controls

DS28. Text inputs and textareas MUST use focus ring feedback from shadcn primitives.

DS29. Text inputs and textareas MUST NOT scale on focus.

DS30. Field validation errors MUST render inline when the error is tied to a specific field.

DS31. Operation failures that are not tied to a specific field MUST render as toast or alert feedback.

## 9. Motion

DS32. Shared motion helpers MUST respect the user's reduced motion preference.

DS33. When reduced motion is enabled, shared motion helpers MUST NOT animate x-offset, y-offset, scale, or rotation.

DS34. When reduced motion is enabled, shared motion helpers MAY animate opacity or render without animation.

DS34a. Existing dashboard motion effects are part of the product interaction model. Vercel/shadcn visual alignment work MUST preserve existing motion timing, transforms, and layout animations unless a separate motion specification change requires modification.

## 10. Internationalized Copy

DS35. User-visible copy in reusable components MUST be provided through the frontend i18n system.

DS36. Reusable components MUST NOT hard-code English labels when equivalent translated namespaces exist.

## 11. Typography

DS37. Tailwind MUST expose a `font-display` family backed by `--font-display`.

DS38. Standard page titles rendered through `PageHeader` MUST use `font-display`.

DS39. Base dashboard card titles SHOULD render as `text-base font-semibold leading-none tracking-tight`.

DS40. Badge text SHOULD use `font-medium` by default.

## 12. Sidebar and Navigation

DS41. Sidebar active navigation items SHOULD use a low-emphasis surface (`accent` or `muted`) rather than a solid primary background.

DS42. Sidebar active navigation icons MAY use primary color as a low-area active indicator.

DS43. Sidebar brand marks SHOULD use a neutral bordered surface rather than a solid primary chip.

DS43a. In-app Monoize brand marks MUST render without an opaque dark or brand-colored plate inside the SVG. The M body MUST inherit `currentColor`. The red, orange, cyan, and celeste beam shapes MAY use fixed brand colors. Browser favicon assets MAY keep an opaque dark plate.

DS44. Sidebar mobile sheet content SHOULD match the desktop sidebar surface and border treatment.

DS45. Sidebar motion and floating-card layout MUST be preserved unless a separate product specification changes them.

## 13. Selected Filters

DS46. Selected filter presets SHOULD use `accent` surface and `accent-foreground` text instead of solid primary background.

## 14. Tooltip and Touch Actions

DS47. On coarse-pointer devices, shared tooltip triggers that do not contain an interactive element MUST open the tooltip on tap and close it on outside tap.

DS48. On coarse-pointer devices, shared tooltip triggers that wrap a native interactive element (`button`, `a`, `input`, `textarea`, `select`, or `[role="button"]`) MUST preserve that element's native click activation. The wrapped element's primary action MUST run on the first tap.

DS49. Icon-only dashboard action buttons intended for touch use MUST expose an accessible label and MUST provide a hit target of at least `44px` by `44px` below the `sm` breakpoint.
