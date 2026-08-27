import type { DashboardAnalyticsBucket, RequestLog } from "@/lib/api";
import { formatCacheHitRate } from "@/lib/live-usage";
import { formatNanoUsd, isSignedIntegerString } from "@/lib/exact-decimal";

export const CHART_COLORS = Array.from(
  { length: 16 },
  (_, index) => `hsl(var(--chart-${index + 1}))`
);

/** Stable hash → palette index so the same model always gets the same color. */
export function modelToColor(modelId: string): string {
  let hash = 0;
  for (let i = 0; i < modelId.length; i++) {
    hash = ((hash << 5) - hash + modelId.charCodeAt(i)) | 0;
  }
  return CHART_COLORS[((hash % CHART_COLORS.length) + CHART_COLORS.length) % CHART_COLORS.length];
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

export function tokensFromLog(log: RequestLog): number {
  const t = log.tokens;
  return (
    (t.input ?? 0) +
    (t.output ?? 0) +
    (t.cache_read ?? 0) +
    (t.cache_creation ?? 0) +
    (t.reasoning ?? 0)
  );
}

export interface ModelUsageRow {
  model: string;
  tokens: number;
  cacheHitRate: number | null;
  chargeNano: bigint;
}

export function aggregateRecentUsage(logs: RequestLog[]): ModelUsageRow[] {
  const map = new Map<
    string,
    { tokens: number; cacheRead: number; input: number; chargeNano: bigint }
  >();

  for (const log of logs) {
    const model = log.model || "unknown";
    const current = map.get(model) ?? {
      tokens: 0,
      cacheRead: 0,
      input: 0,
      chargeNano: 0n,
    };
    current.tokens += tokensFromLog(log);
    current.cacheRead += log.tokens.cache_read ?? 0;
    current.input += log.tokens.input ?? 0;
    const charge = log.billing?.charge_nano_usd;
    if (charge != null && isSignedIntegerString(charge)) {
      current.chargeNano += BigInt(charge);
    }
    map.set(model, current);
  }

  return [...map.entries()]
    .map(([model, row]) => ({
      model,
      tokens: row.tokens,
      cacheHitRate: row.input > 0 ? row.cacheRead / row.input : null,
      chargeNano: row.chargeNano,
    }))
    .filter((row) => row.tokens > 0 || row.chargeNano !== 0n)
    .sort((a, b) => b.tokens - a.tokens);
}

export function formatCharge(nano: bigint): string {
  return formatNanoUsd(nano.toString(), 4);
}

export function formatCacheHit(rate: number | null): string {
  return formatCacheHitRate(rate);
}

export interface CumulativeSeries {
  models: string[];
  /** Cumulative stacked rows for AreaChart. */
  rows: Array<Record<string, number | string>>;
  /** Per-bucket daily (non-cumulative) totals by model. */
  dailyByBucket: Array<Record<string, number>>;
  dailyTotals: number[];
  cumulativeTotals: number[];
}

export function buildCumulativeTokenSeries(
  buckets: DashboardAnalyticsBucket[]
): CumulativeSeries {
  const totals = new Map<string, number>();
  for (const bucket of buckets) {
    for (const [model, tokens] of Object.entries(bucket.tokens_by_model ?? {})) {
      const n = Number(tokens) || 0;
      if (n > 0) totals.set(model, (totals.get(model) ?? 0) + n);
    }
  }

  const models = [...totals.entries()]
    .filter(([, v]) => v > 0)
    .sort((a, b) => b[1] - a[1])
    .map(([k]) => k);

  const running = new Map<string, number>(models.map((m) => [m, 0]));
  const rows: Array<Record<string, number | string>> = [];
  const dailyByBucket: Array<Record<string, number>> = [];
  const dailyTotals: number[] = [];
  const cumulativeTotals: number[] = [];

  for (const bucket of buckets) {
    const daily: Record<string, number> = {};
    let dayTotal = 0;
    for (const model of models) {
      const day = Number(bucket.tokens_by_model?.[model] ?? 0) || 0;
      daily[model] = day;
      dayTotal += day;
      running.set(model, (running.get(model) ?? 0) + day);
    }
    const row: Record<string, number | string> = {
      label: shortBucketLabel(bucket.label),
      rawLabel: bucket.label,
    };
    let cum = 0;
    for (const model of models) {
      const value = running.get(model) ?? 0;
      row[model] = value;
      cum += value;
    }
    rows.push(row);
    dailyByBucket.push(daily);
    dailyTotals.push(dayTotal);
    cumulativeTotals.push(cum);
  }

  return { models, rows, dailyByBucket, dailyTotals, cumulativeTotals };
}
