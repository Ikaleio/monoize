import type { DashboardAnalytics, PublicSystemSettings } from "@/lib/api";
import { formatCacheHitRate } from "@/lib/live-usage";
import { formatNanoUsd, isSignedIntegerString } from "@/lib/exact-decimal";
import {
  bucketLabelForWindow,
  bucketStartDates,
  type UsageWindow,
} from "@/lib/usage-window";

/** Series beyond this rank merge into OTHER_SERIES (DH-6g). */
const MAX_NAMED_SERIES = 8;
export const OTHER_SERIES = "__other__";
export const OTHER_SERIES_COLOR = "hsl(var(--muted-foreground))";

function seriesColor(rank: number): string {
  return rank < MAX_NAMED_SERIES
    ? `hsl(var(--chart-${rank + 1}))`
    : OTHER_SERIES_COLOR;
}

/** DH-8b: the configured base URL, or the current origin when it is empty. */
export function resolveApiBase(settings: PublicSystemSettings | undefined): {
  url: string;
  isFallback: boolean;
} {
  const configured = settings?.api_base_url?.trim() ?? "";
  return configured
    ? { url: configured, isFallback: false }
    : { url: window.location.origin, isFallback: true };
}

/** Compact SI-style token counts (e.g. 10M, 1.2B). */
export function formatCompactTokens(value: number): string {
  if (!Number.isFinite(value) || value === 0) return "0";
  const abs = Math.abs(value);
  const sign = value < 0 ? "-" : "";
  const format = (n: number, suffix: string) => {
    const rounded = Math.round(n * 10) / 10;
    const text = Number.isInteger(rounded) ? String(rounded) : rounded.toFixed(1);
    return `${sign}${text}${suffix}`;
  };
  if (abs >= 1e12) return format(abs / 1e12, "T");
  if (abs >= 1e9) return format(abs / 1e9, "B");
  if (abs >= 1e6) return format(abs / 1e6, "M");
  if (abs >= 1e3) return format(abs / 1e3, "K");
  return `${sign}${Math.round(abs).toLocaleString("en-US")}`;
}

export function shortBucketLabel(label: string): string {
  // Analytics labels are typically "MM-DD HH:00" — keep the date part.
  const datePart = label.split(" ")[0] ?? label;
  const [mm, dd] = datePart.split("-");
  if (!mm || !dd) return label;
  const months = ["Jan", "Feb", "Mar", "Apr", "May", "Jun", "Jul", "Aug", "Sep", "Oct", "Nov", "Dec"];
  const monthIndex = Number(mm) - 1;
  if (monthIndex < 0 || monthIndex > 11) return datePart;
  return `${months[monthIndex]} ${Number(dd)}`;
}

export interface ModelUsageRow {
  model: string;
  tokens: number;
  cacheHitRate: number | null;
  chargeNano: bigint;
}

/** DH-7a: per-model totals from the full server-side aggregation. */
export function aggregateModelUsage(
  analytics: DashboardAnalytics | undefined,
): ModelUsageRow[] {
  const rows = new Map<string, { tokens: number; chargeNano: bigint }>();
  const entry = (model: string) => {
    let row = rows.get(model);
    if (!row) {
      row = { tokens: 0, chargeNano: 0n };
      rows.set(model, row);
    }
    return row;
  };

  for (const bucket of analytics?.buckets ?? []) {
    for (const [model, tokens] of Object.entries(bucket.tokens_by_model ?? {})) {
      entry(model).tokens += Number(tokens) || 0;
    }
    for (const [model, cost] of Object.entries(bucket.cost_by_model ?? {})) {
      if (isSignedIntegerString(cost)) entry(model).chargeNano += BigInt(cost);
    }
  }

  return [...rows.entries()]
    .filter(([, row]) => row.tokens > 0 || row.chargeNano !== 0n)
    .map(([model, row]) => {
      const cache = analytics?.cache_by_model?.[model];
      return {
        model,
        tokens: row.tokens,
        chargeNano: row.chargeNano,
        cacheHitRate:
          cache && cache.input_tokens > 0
            ? cache.cache_read_tokens / cache.input_tokens
            : null,
      };
    })
    .sort((a, b) => b.tokens - a.tokens || a.model.localeCompare(b.model));
}

export function formatCharge(nano: bigint): string {
  return formatNanoUsd(nano.toString(), 4);
}

export function formatCacheHit(rate: number | null): string {
  return formatCacheHitRate(rate);
}

export interface UsageSeries {
  /** Bar series keys in rank order; ends with OTHER_SERIES when models merged. */
  series: string[];
  /** Rank color per real model and per OTHER_SERIES (DH-6g). */
  colors: Record<string, string>;
  /** One row per bucket, keyed by series. */
  rows: Array<Record<string, number | string>>;
  /** Per-bucket tokens per real model; the tooltip expands OTHER_SERIES from this. */
  bucketByModel: Array<Record<string, number>>;
  bucketTotals: number[];
  windowTotal: number;
}

export function buildUsageSeries(
  analytics: DashboardAnalytics | undefined,
  window: UsageWindow,
): UsageSeries {
  const buckets = analytics?.buckets ?? [];
  // DH-6c: labels come from the response time range; the backend `label`
  // field truncates minutes (`%H:00`) so it cannot distinguish 5-minute
  // buckets. Fall back to the backend label when the range does not parse.
  const starts = analytics
    ? bucketStartDates(analytics.time_from, analytics.time_to, buckets.length)
    : null;

  const totals = new Map<string, number>();
  for (const bucket of buckets) {
    for (const [model, tokens] of Object.entries(bucket.tokens_by_model ?? {})) {
      const n = Number(tokens) || 0;
      if (n > 0) totals.set(model, (totals.get(model) ?? 0) + n);
    }
  }

  const models = [...totals.entries()]
    .sort((a, b) => b[1] - a[1] || a[0].localeCompare(b[0]))
    .map(([model]) => model);
  const named = models.slice(0, MAX_NAMED_SERIES);
  const hasOther = models.length > MAX_NAMED_SERIES;
  const colors: Record<string, string> = { [OTHER_SERIES]: OTHER_SERIES_COLOR };
  models.forEach((model, rank) => {
    colors[model] = seriesColor(rank);
  });

  let windowTotal = 0;
  const rows: UsageSeries["rows"] = [];
  const bucketByModel: UsageSeries["bucketByModel"] = [];
  const bucketTotals: number[] = [];

  buckets.forEach((bucket, index) => {
    const perBucket: Record<string, number> = {};
    let bucketTotal = 0;
    for (const model of models) {
      const value = Number(bucket.tokens_by_model?.[model] ?? 0) || 0;
      perBucket[model] = value;
      bucketTotal += value;
    }
    const start = starts?.[index];
    const row: Record<string, number | string> = {
      label: start ? bucketLabelForWindow(window, start) : shortBucketLabel(bucket.label),
      bucketIndex: index,
    };
    let namedTotal = 0;
    for (const model of named) {
      row[model] = perBucket[model] ?? 0;
      namedTotal += perBucket[model] ?? 0;
    }
    if (hasOther) row[OTHER_SERIES] = bucketTotal - namedTotal;
    windowTotal += bucketTotal;
    rows.push(row);
    bucketByModel.push(perBucket);
    bucketTotals.push(bucketTotal);
  });

  return {
    series: hasOther ? [...named, OTHER_SERIES] : named,
    colors,
    rows,
    bucketByModel,
    bucketTotals,
    windowTotal,
  };
}
