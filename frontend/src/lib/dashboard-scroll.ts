import { createContext } from "react";

/**
 * The dashboard main pane: the single vertical scroll container for every
 * `/dashboard/*` page (dashboard-ui-layout.spec.md DL7b). `null` until mounted.
 * Virtualized lists pass it to `Virtuoso` as `customScrollParent`.
 */
export const DashboardScrollParentContext = createContext<HTMLElement | null>(null);
