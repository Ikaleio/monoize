# Frontend Design System Specification

## 0. Scope

- Product name: Monoize.
- Scope: shared visual and interaction rules for the embedded frontend under `frontend/src`.
- Style baseline: Vercel dashboard conventions and shadcn/ui primitives.

## 1. Surface and Color Tokens

DS1. Shared UI components MUST use CSS variables from `frontend/src/index.css` for base colors.

DS1a. The Vite entry document's `html` element MUST apply the Tailwind
`bg-background` class so browser overscroll and pre-rendered document areas use
the active theme background token.

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

DS4b. Text rendered on semantic soft backgrounds MUST have a contrast ratio of at least 4.5:1 in both light and dark themes. This rule applies to `text-success-foreground` on `bg-success-soft`, `text-warning-foreground` on `bg-warning-soft`, and `text-info-foreground` on `bg-info-soft`. `StatusBadge variant="destructive"` MUST render with `border-error-border bg-error-soft text-error-foreground`. The `destructive` token is a solid-button background. It MUST NOT color text or icons in a data row.

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

DS19c. The table shell surface MUST clip its content with `overflow: clip` so that it does not establish a scroll container. A sticky descendant MUST remain sticky relative to the dashboard main pane.

DS19b. Table toolbar search controls MUST support an inline leading search icon without changing the responsive width requirement in DS22.

DS20. Standard table rows MUST use `hover:bg-muted/50` for hover feedback.

DS21. Standard table cells MUST use deterministic horizontal and vertical padding.

DS22. Search inputs in table toolbars MUST use responsive width: full width below `sm`, bounded width at `sm` and above.

DS22a. Shared virtual table header cells SHOULD use `h-9 px-3 text-xs font-medium text-muted-foreground`.

DS22b. Shared virtual table body cells SHOULD use `px-3 py-2 align-middle` unless the table is explicitly high-density.

### 6.1 Responsive Data Lists

DS22c. `components/ui/data-list.tsx` MUST export `DataList`, `DataListHeader`, `DataListHead`, `DataListBody`, `DataListRow`, `DataListCell`, and `DataListActions`. `DataList` MUST accept a `columns` string. The header and every row MUST use that string as their `grid-template-columns` value in wide mode. Because the header and each row are independent grids, every track in `columns` MUST be either a fixed length or `minmax(0, <flex>)`. Content-sized tracks (`auto`, `min-content`, `max-content`, `fit-content()`) MUST NOT appear. In wide mode, the header cell and every body cell of one column MUST share the same left edge (±1 CSS pixel).

DS22d. `DataList` MUST set `container-type: inline-size` on its root. Let `w` be the root content width.

- If `w < 36rem`, every cell MUST occupy its own line.
- If `36rem <= w < 56rem`, primary cells and action groups MUST span the full row. Other cells MUST flow into two equal columns.
- Below wide mode, a primary cell MUST render before every other cell of its row, and the action group MUST render after every other cell.
- If `w >= 56rem` (wide mode), cells MUST align to `columns`, and the header MUST be visible.

DS22e. Below wide mode, the header MUST NOT render. A `DataListCell` with a `label` MUST render the label before its value on the same line, with the value at the inline end. In wide mode, the label MUST be visually hidden and remain available to assistive technology. The header MUST be hidden from assistive technology in every mode.

DS22f. `DataListHead` and `DataListCell` MUST accept `align = "start" | "end"`. In wide mode, a header cell and the body cells of the same column MUST use the same text alignment.

DS22g. Header labels MUST use `text-xs font-medium text-muted-foreground`. Row cells MUST use `text-sm`. Rows MUST use `px-4 py-3`, `divide-y` separators, and `hover:bg-muted/50`. Call sites MUST NOT override row padding or cell font size.

DS22h. `DataListBody` and `DataListRow` MUST forward refs and unknown props to `ul` and `li` elements, so that a virtualization library can supply them as list and item components. `DataListRow` MUST support `asChild` composition.

DS22i. `DataListActions` MUST end-align its controls in every mode. Below wide mode, it MUST occupy the last line of the row.

DS22j. A `DataList` MUST NOT establish a horizontal or vertical scroll container. Content that exceeds its column MUST truncate or wrap inside the cell.

DS22k. `components/ui/data-list-virtual.tsx` MUST export `virtualDataListComponents`, a `Virtuoso` `components` object whose `List` renders `DataListBody` and whose `Item` renders `DataListRow`. `Item` MUST forward only `style`, `data-index`, `data-item-index`, `data-known-size`, and `children` to the row element. `List` MUST set the list's accessible name from `context.label`. Every virtualized `DataList` MUST use this object instead of page-local adapters.

## 7. Dialogs and Confirmation

DS23. Destructive confirmation UI MUST use shadcn `AlertDialog` primitives.

DS24. Browser-native `confirm()` MUST NOT be used for dashboard destructive actions.

DS25. Long-form dialogs MUST keep header and footer reachable inside the visible viewport.

DS26. Long-form dialogs MUST place overflow on an internal body container when content exceeds viewport height.

DS26a. Long-form dialog content MUST set viewport-bounded max height and `overflow-hidden` on the outer content element.

DS26b. Long-form dialog footers MUST be `shrink-0` so actions remain reachable while the dialog body scrolls.

DS26c. A long-form dialog MUST keep at least 24 CSS pixels between its action
footer controls and the bottom border of the dialog at every viewport height.

DS27. Dialog action footers MUST use shadcn button variants.

## 8. Form Controls

DS28. Text inputs and textareas MUST use focus ring feedback from shadcn primitives.

DS29. Text inputs and textareas MUST NOT scale on focus.

DS30. Field validation errors MUST render inline when the error is tied to a specific field.

DS31. Operation failures that are not tied to a specific field MUST render as toast or alert feedback.

## 9. Inline alignment

DS31a. A horizontal group that contains one-line text, an icon, a badge, or another compact element MUST align the rendered boxes by vertical center.

DS31b. If text in a horizontal group can wrap, each adjacent icon or compact action MUST align with the first rendered text line. It MUST NOT center against the complete multi-line text block.

DS31c. Interactive controls that share one horizontal row MUST use the same rendered height at that viewport. A multi-line editor MAY keep adjacent actions top-aligned.

DS31d. Ordinary card-header text MUST NOT use a vertical translation to compensate for sibling geometry. The header layout MUST establish alignment through flex or grid alignment.

## 10. Motion

DS32. Shared motion helpers MUST respect the user's reduced motion preference.

DS33. When reduced motion is enabled, shared motion helpers MUST NOT animate x-offset, y-offset, scale, or rotation.

DS34. When reduced motion is enabled, shared motion helpers MAY animate opacity or render without animation.

DS34a. Existing dashboard motion effects are part of the product interaction model. Vercel/shadcn visual alignment work MUST preserve existing motion timing, transforms, and layout animations unless a separate motion specification change requires modification. DS34b and DS34c are such motion specification changes.

DS34b. Non-interactive form rows (a label/description block paired with a control such as a switch, input, or select) MUST NOT apply hover-triggered offset or scale transforms. Hover feedback on such rows is limited to color changes.

DS34c. Hover/press scale affordance on action buttons MUST be applied through the shared `AnimatedButton` wrapper from `components/ui/motion.tsx`:

- hover scale MUST be `1.02`;
- tap scale MUST be `0.98`;
- the wrapper MUST respect reduced motion per DS32-DS34;
- the wrapper MUST forward all props it does not consume (event handlers, ARIA and data attributes) to its underlying element, so that `asChild` composition (for example Radix `DialogTrigger asChild`) keeps its injected open/close handlers functional.

Pages MUST NOT wrap buttons in ad-hoc `motion.div` elements with `whileHover`/`whileTap` scale handlers.

DS34d. Hover-triggered scale or offset transforms MUST NOT be applied to non-interactive decorative elements (for example, a static brand mark that is not a link or button).

## 11. Internationalized Copy

DS35. User-visible copy in reusable components MUST be provided through the frontend i18n system.

DS36. Reusable components MUST NOT hard-code English labels when equivalent translated namespaces exist.

## 11. Typography

DS37. Tailwind MUST expose a `font-display` family backed by `--font-display`.

DS38. Standard page titles rendered through `PageHeader` MUST use `font-display`.

DS39. Base dashboard card titles SHOULD render as `text-base font-semibold leading-none tracking-tight`.

DS40. Badge text SHOULD use `font-medium` by default.

DS40a. Base badge elements MUST render as a single line. Badge text and icon content MUST NOT wrap inside the badge.

DS40b. A read-only collection containing more badges than its configured preview count MUST render as a collapsed badge collection by default.

- The preview MUST render no more than the configured preview count.
- The preview MUST render a `+N` badge when hidden badges exist.
- The preview row MUST NOT wrap.

DS40c. In a collapsed badge collection preview, badge text that exceeds the available preview width MUST be truncated with an ellipsis.

DS40d. Hovering or focusing a collapsed badge collection MUST open a small portal-backed popover that contains the complete badge list.

- The popover list MUST contain every badge represented by the collection.
- Badge entries in the popover MUST NOT wrap.
- If the complete list exceeds the popover bounds, the popover content MUST scroll instead of wrapping badge text.
- The popover surface MUST use a translucent `popover` token background with backdrop blur and MUST remain portal-backed.
- On fine-pointer devices, clicking the collection trigger MUST NOT pin the popover open. The popover MUST close after the pointer leaves both the trigger and the popover content.
- Focus caused by a fine-pointer trigger click MUST NOT reopen the popover after pointer-leave close logic runs.
- A trigger activation whose PointerEvent reports `mouse` or `pen` MUST be treated as fine-pointer activation even when viewport media queries report coarse or no-hover capability.
- On touch pointers, coarse-pointer devices, or devices that report no hover capability, tapping the collection trigger MAY toggle the popover open or closed.

DS40d-1. A collapsed badge collection MUST call its `onOpenChange` callback only when the collection popover open state changes. Parent rerenders that do not change the popover open state MUST NOT emit an open-change callback.

DS40e. Editable chip inputs and selectable option lists MAY render all badges directly when hiding entries would remove direct edit, delete, or selection controls.

DS40f. Badge-shaped containers MUST use the `rounded-md` corner radius. This rule applies to:

- the base `Badge` component;
- `StatusBadge`;
- `ModelBadge`;
- overflow-count (`+N`) badges in collapsed badge collections;
- compact timing/state badges inside tables;
- group-suggestion chip buttons and other badge-shaped filter chips;
- clickable wrappers whose visible surface is a badge (for example, a ghost button wrapping a model badge);
- skeleton placeholders that stand in for any of the above.

Badge-shaped containers MUST NOT use `rounded-full`.

DS40g. Fully circular (`rounded-full`) shapes are reserved for non-badge elements: status dots, avatars, switch tracks and thumbs, segmented-control indicators, and loading/ping indicators.

DS40h. Table body cells within one table MUST share the table's base font size. Adjacent sibling cells MUST NOT mix arbitrary smaller sizes (for example `text-[10px]` or `text-[11px]`) with the base size. Secondary detail text nested inside a badge (for example the `ModelBadge` bracket details) MAY be one visual step smaller than the badge label.

## 12. Sidebar and Navigation

DS41. Sidebar active navigation items SHOULD use a low-emphasis surface (`accent` or `muted`) rather than a solid primary background.

DS42. Sidebar active navigation icons MAY use primary color as a low-area active indicator.

DS43. Sidebar brand marks SHOULD use a neutral bordered surface rather than a solid primary chip.

DS43a. In-app Monoize brand marks MUST render without an opaque dark or brand-colored plate inside the SVG. The M body MUST inherit `currentColor`. The red, orange, cyan, and celeste beam shapes MAY use fixed brand colors. Browser favicon assets MAY keep an opaque dark plate.

DS43b. The dashboard sidebar header MUST render the Monoize brand mark inside a 32 px by 32 px surface. The logo SVG MUST fill that surface without wrapper padding. The SVG view box provides the mark's internal clear space. This rule MUST apply in both expanded and collapsed sidebar states and MUST NOT change the login-page brand mark.

DS44. Sidebar mobile sheet content SHOULD match the desktop sidebar surface and border treatment.

DS45. Sidebar motion and floating-card layout MUST be preserved unless a separate product specification changes them.

## 13. Selected Filters

DS46. Selected filter presets SHOULD use `accent` surface and `accent-foreground` text instead of solid primary background.

## 14. Iconography

DS50. Icons MUST come from `lucide-react` or the existing `@lobehub/icons` provider icon set.

DS51. Emoji characters MUST NOT be used as icons or icon substitutes in dashboard UI.

DS52. The only permitted hand-authored inline SVG is the Monoize brand mark governed by DS43a.

## 15. Tooltip and Touch Actions

DS47. On coarse-pointer devices, shared tooltip triggers that do not contain an interactive element MUST open the tooltip on tap and close it on outside tap.

DS48. On coarse-pointer devices, shared tooltip triggers that wrap a native interactive element (`button`, `a`, `input`, `textarea`, `select`, or `[role="button"]`) MUST preserve that element's native click activation. The wrapped element's primary action MUST run on the first tap.

DS49. Icon-only dashboard action buttons intended for touch use MUST expose an accessible label and MUST provide a hit target of at least `44px` by `44px` below the `sm` breakpoint.

## 16. Accessible Feedback and Controls

DS53. Error notices MUST use the `error-foreground`, `error-soft`, and `error-border`
semantic tokens. Error text on its soft surface MUST have contrast of at least 4.5:1
in both themes. The destructive button palette MUST remain independent.

DS54. Shared data-load failure notices MUST expose an alert and a localized retry
button. While retry is pending, the button MUST be disabled. A notice for cached data
MUST state that the refresh failed and previous data remains visible.

DS55. Shared icon buttons, Dialog close buttons, and Sheet close buttons MUST have
at least a 44 by 44 CSS pixel target below `sm`. Close buttons MUST have a localized
accessible name. Their target MUST NOT overlap the popup title text.

DS56. Every editable price-sheet field MUST have a unique id and an associated label.
Repeated tier ids MUST include the component instance id, tier index, and field name.
Input association MUST remain correct after a tier is removed.

DS57. The application MUST apply the user's reduced-motion preference to direct
Framer Motion elements as well as shared helpers. CSS button styles MUST NOT add
scale transforms outside the reduced-motion-aware AnimatedButton wrapper.
