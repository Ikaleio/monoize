// Shared spend-window model for the admin System Dashboard channel-health
// card (admin-dashboard.spec.md AD-2 / ADF-5). Kept separate from
// `usage-window.ts` because the home-dashboard windows are 1h/24h/7d/30d.

export type SpendWindow = "24h" | "3d" | "7d" | "14d" | "30d";

export const SPEND_WINDOWS: SpendWindow[] = ["24h", "3d", "7d", "14d", "30d"];

export const DEFAULT_SPEND_WINDOW: SpendWindow = "24h";

export const SPEND_WINDOW_HOURS: Record<SpendWindow, number> = {
  "24h": 24,
  "3d": 72,
  "7d": 168,
  "14d": 336,
  "30d": 720,
};
