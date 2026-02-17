# Frontend Popup Motion Specification

## 0. Status

- Product name: Monoize.
- Scope: shared popup primitives in `frontend/src/components/ui`.

## 1. Components in Scope

PM1. The following shared popup components MUST implement motion behavior:

- `Dialog` (`dialog.tsx`)
- `AlertDialog` (`alert-dialog.tsx`)
- `Sheet` (`sheet.tsx`)

PM2. Any UI popup that is composed from PM1 components MUST inherit the same motion behavior without per-page overrides.

## 2. Motion Contract

PM3. Every popup in PM1 MUST animate on both open and close.

PM4. Motion easing MUST be non-linear and aligned with the project motion style tokens:

- entering transitions MUST use `easeOutExpo` (`cubic-bezier(0.16, 1, 0.3, 1)`),
- exiting transitions MUST use `easeInOutQuart` (`cubic-bezier(0.76, 0, 0.24, 1)`).

PM5. Overlay animation contract:

- open: opacity `0 -> 1` in `0.22s` using PM4 entering easing,
- close: opacity `1 -> 0` in `0.16s` using PM4 exiting easing.

PM6. Centered popup panel contract (`Dialog`, `AlertDialog` content):

- open: opacity `0 -> 1`, scale `0.96 -> 1`, y-offset `12px -> 0px` in `0.24s` with PM4 entering easing,
- close: opacity `1 -> 0`, scale `1 -> 0.96`, y-offset `0px -> 12px` in `0.18s` with PM4 exiting easing.

PM7. Sheet panel contract (direction-aware):

- open MUST slide from its configured side into resting position in `0.26s` with PM4 entering easing,
- close MUST slide back to its configured side in `0.20s` with PM4 exiting easing.

PM8. Popup motion implementation MUST be centralized in shared popup primitives (`dialog.tsx`, `alert-dialog.tsx`, `sheet.tsx`) and MUST use `framer-motion` with `AnimatePresence` for enter/exit lifecycle control.

PM9. Page-level callers MUST NOT need custom animation code to get PM3-PM7 behavior.

PM10. Shared popup primitives MUST keep Radix accessibility and interaction contracts intact:

- focus trapping,
- escape-key close,
- outside-click behavior,
- close button behavior.

PM11. Shared popup primitives MUST preserve DOM mount during exit animation and unmount only after exit transition completes.

PM12. Shared popup primitives MUST apply a secondary content-layer motion without requiring changes in page-level children:

- popup shell (`Dialog` / `AlertDialog` / `Sheet`) animates per PM5-PM7,
- inner content container animates opacity `0 -> 1` and y-offset `8px -> 0px` on open,
- inner content container exit animates opacity `1 -> 0` and y-offset `0px -> 4px`,
- inner content motion MUST use non-linear easing from PM4 and duration between `0.16s` and `0.22s`.
