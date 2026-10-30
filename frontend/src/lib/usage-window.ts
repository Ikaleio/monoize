// Shared time-window model for the /dashboard usage chart and recent-usage
// panel (dashboard-home-overview.spec.md DH-6a/DH-6c/DH-6i/DH-7a).

export type UsageWindow = "1h" | "24h" | "7d" | "30d";

export const USAGE_WINDOWS: UsageWindow[] = ["1h", "24h", "7d", "30d"];

export const DEFAULT_USAGE_WINDOW: UsageWindow = "24h";

const USAGE_WINDOW_STORAGE_KEY = "monoize-home-window";

export function readStoredUsageWindow(): UsageWindow {
  try {
    const stored = window.localStorage.getItem(USAGE_WINDOW_STORAGE_KEY);
    return USAGE_WINDOWS.find((option) => option === stored) ?? DEFAULT_USAGE_WINDOW;
  } catch {
    return DEFAULT_USAGE_WINDOW;
  }
}

export function storeUsageWindow(value: UsageWindow): void {
  try {
    window.localStorage.setItem(USAGE_WINDOW_STORAGE_KEY, value);
  } catch {
    // Storage may be unavailable; the selection still applies to this mount.
  }
}

export interface UsageWindowQuery {
  rangeHours: number;
  buckets: number;
}

// DH-6a: 1h → 12 five-minute buckets, 24h → 24 one-hour buckets,
// 7d → 7 day buckets, 30d → 30 day buckets (720h is the API range_hours max).
export const USAGE_WINDOW_QUERY: Record<UsageWindow, UsageWindowQuery> = {
  "1h": { rangeHours: 1, buckets: 12 },
  "24h": { rangeHours: 24, buckets: 24 },
  "7d": { rangeHours: 168, buckets: 7 },
  "30d": { rangeHours: 720, buckets: 30 },
};

/**
 * Bucket start instants reconstructed from the analytics response range
 * (DH-6c): start_i = time_from + i * (time_to - time_from) / count.
 * Returns null when the range does not parse or is degenerate.
 */
export function bucketStartDates(
  timeFrom: string,
  timeTo: string,
  count: number
): Date[] | null {
  const from = Date.parse(timeFrom);
  const to = Date.parse(timeTo);
  if (!Number.isFinite(from) || !Number.isFinite(to) || to <= from || count <= 0) {
    return null;
  }
  const width = (to - from) / count;
  return Array.from({ length: count }, (_, i) => new Date(from + i * width));
}

const SHORT_MONTHS = [
  "Jan", "Feb", "Mar", "Apr", "May", "Jun",
  "Jul", "Aug", "Sep", "Oct", "Nov", "Dec",
];

/**
 * X-axis label for a bucket start in the user's local time zone (DH-6c):
 * clock time for sub-day buckets, short date for day-width buckets.
 */
export function bucketLabelForWindow(window: UsageWindow, start: Date): string {
  if (window === "1h" || window === "24h") {
    const hh = String(start.getHours()).padStart(2, "0");
    const mm = String(start.getMinutes()).padStart(2, "0");
    return `${hh}:${mm}`;
  }
  return `${SHORT_MONTHS[start.getMonth()]} ${start.getDate()}`;
}

/** DH-6d: day-width windows mark "Today"; sub-day windows mark "Now". */
export function usesTodayMarker(window: UsageWindow): boolean {
  return window === "7d" || window === "30d";
}
