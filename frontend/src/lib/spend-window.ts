// Spend window of the admin System Dashboard (admin-dashboard.spec.md AD-2a, ADF-5).
// Kept separate from `usage-window.ts` because the home-dashboard windows also include 1h.

export const SPEND_WINDOWS = ["24h", "7d", "30d"] as const;

export type SpendWindow = (typeof SPEND_WINDOWS)[number];

export const DEFAULT_SPEND_WINDOW: SpendWindow = "24h";
