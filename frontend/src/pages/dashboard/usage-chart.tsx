import { useMemo } from "react";
import { useTranslation } from "react-i18next";
import {
  Area,
  AreaChart,
  CartesianGrid,
  ReferenceLine,
  XAxis,
  YAxis,
} from "recharts";
import { Card, CardContent, CardDescription, CardHeader, CardTitle } from "@/components/ui/card";
import {
  ChartContainer,
  ChartTooltip,
  type ChartConfig,
} from "@/components/ui/chart";
import { ScrollArea } from "@/components/ui/scroll-area";
import { Skeleton } from "@/components/ui/skeleton";
import { EmptyState } from "@/components/ui/empty-state";
import { motion, transitions } from "@/components/ui/motion";
import { cn } from "@/lib/utils";
import type { DashboardAnalytics } from "@/lib/api";
import { usesTodayMarker, type UsageWindow } from "@/lib/usage-window";
import { TimeWindowControl } from "./time-window-control";
import {
  buildCumulativeTokenSeries,
  formatCompactTokens,
  modelToColor,
} from "./utils";

interface UsageChartPanelProps {
  analytics: DashboardAnalytics | undefined;
  /** First load with no data for the panel (DH-12a): show skeleton. */
  loading?: boolean;
  /** Fetching another window while previous data is shown (DH-12b): dim only. */
  pending?: boolean;
  window: UsageWindow;
  onWindowChange: (window: UsageWindow) => void;
}

export function UsageChartPanel({
  analytics,
  loading,
  pending,
  window,
  onWindowChange,
}: UsageChartPanelProps) {
  const { t } = useTranslation();

  const series = useMemo(
    () => buildCumulativeTokenSeries(analytics, window),
    [analytics, window]
  );

  const chartConfig = useMemo<ChartConfig>(() => {
    const cfg: ChartConfig = {};
    for (const model of series.models) {
      cfg[model] = { label: model, color: modelToColor(model) };
    }
    return cfg;
  }, [series.models]);

  const markerLabel =
    series.rows.length > 0 ? String(series.rows[series.rows.length - 1]?.label ?? "") : "";
  const markerText = usesTodayMarker(window)
    ? t("dashboard.usage.today", "Today")
    : t("dashboard.usage.now", "Now");

  return (
    <motion.div
      initial={{ opacity: 0, y: 18 }}
      animate={{ opacity: 1, y: 0 }}
      transition={{ delay: 0.12, ...transitions.normal }}
    >
      <Card>
        <CardHeader className="flex flex-row flex-wrap items-start justify-between gap-3 p-4 pb-2">
          <div className="flex min-w-0 flex-col gap-1">
            <CardTitle className="text-balance font-display text-2xl font-semibold tracking-tight">
              {t("dashboard.usage.title", "Your Usage")}
            </CardTitle>
            <CardDescription className="text-pretty leading-relaxed">
              {t(
                "dashboard.usage.subtitle",
                "Cumulative token usage for the selected time range"
              )}
            </CardDescription>
          </div>
          <TimeWindowControl value={window} onChange={onWindowChange} />
        </CardHeader>

        <CardContent
          className={cn(
            "flex flex-col gap-3 p-4 pt-2 transition-opacity",
            pending && "opacity-60"
          )}
        >
          {loading ? (
            <Skeleton className="h-72 w-full rounded-lg" />
          ) : series.rows.length === 0 || series.models.length === 0 ? (
            <EmptyState
              title={t("dashboard.noAnalysisData", "No request log data available")}
              description={t(
                "dashboard.noAnalysisDataDescription",
                "Statistics will appear automatically after requests are made."
              )}
              className="min-h-60 py-8"
            />
          ) : (
            <>
              <ChartContainer
                config={chartConfig}
                className="h-72 w-full !aspect-auto sm:h-80"
              >
                <AreaChart
                  data={series.rows}
                  // Right margin leaves room for the Now/Today marker label,
                  // which is centered on the final bucket's reference line.
                  margin={{ top: 12, right: 20, left: 0, bottom: 0 }}
                >
                  <CartesianGrid vertical={false} strokeDasharray="3 3" />
                  <XAxis
                    dataKey="label"
                    tickLine={false}
                    axisLine={false}
                    tickMargin={8}
                    minTickGap={20}
                  />
                  <YAxis
                    tickLine={false}
                    axisLine={false}
                    width={52}
                    tickFormatter={(v) => formatCompactTokens(Number(v))}
                    label={{
                      value: t("dashboard.usage.cumulativeTokens", "Cumulative Tokens"),
                      angle: -90,
                      position: "insideLeft",
                      offset: 8,
                      style: {
                        textAnchor: "middle",
                        fill: "hsl(var(--muted-foreground))",
                        fontSize: 11,
                      },
                    }}
                  />
                  <ChartTooltip
                    content={({ active, payload, label }) => {
                      if (!active || !payload?.length) return null;
                      const idx = series.rows.findIndex((row) => row.label === label);
                      const perBucket = idx >= 0 ? series.bucketByModel[idx] ?? {} : {};
                      const bucketTotal = idx >= 0 ? series.bucketTotals[idx] ?? 0 : 0;
                      const cumulativeTotal =
                        idx >= 0 ? series.cumulativeTotals[idx] ?? 0 : 0;
                      const entries = series.models
                        .map((model) => ({
                          model,
                          tokens: perBucket[model] ?? 0,
                          color: modelToColor(model),
                        }))
                        .filter((e) => e.tokens > 0)
                        .sort((a, b) => b.tokens - a.tokens);

                      return (
                        <div className="flex min-w-56 flex-col gap-2 rounded-lg border bg-background px-3 py-2.5 text-xs shadow-md">
                          <div className="flex items-baseline justify-between gap-3 border-b pb-2">
                            <span className="font-medium">{String(label)}</span>
                            <span className="text-muted-foreground">
                              {t("dashboard.usage.periodBreakdown", "Period breakdown")}
                            </span>
                          </div>
                          <ul className="flex flex-col gap-1.5">
                            {entries.map((entry) => {
                              const pct =
                                bucketTotal > 0
                                  ? ((entry.tokens / bucketTotal) * 100).toFixed(1)
                                  : "0";
                              return (
                                <li
                                  key={entry.model}
                                  className="flex items-center justify-between gap-3"
                                >
                                  <div className="flex min-w-0 items-center gap-2">
                                    <span
                                      className="h-2 w-2 shrink-0 rounded-sm"
                                      style={{ backgroundColor: entry.color }}
                                    />
                                    <span className="truncate font-mono text-[11px]">
                                      {entry.model}
                                    </span>
                                  </div>
                                  <span className="shrink-0 tabular-nums text-muted-foreground">
                                    {formatCompactTokens(entry.tokens)}{" "}
                                    <span className="text-foreground/70">({pct}%)</span>
                                  </span>
                                </li>
                              );
                            })}
                          </ul>
                          <div className="flex flex-col gap-1 border-t pt-2 text-muted-foreground">
                            <div className="flex justify-between gap-3">
                              <span>{t("dashboard.usage.periodTotal", "Period total")}</span>
                              <span className="font-medium tabular-nums text-foreground">
                                {formatCompactTokens(bucketTotal)}
                              </span>
                            </div>
                            <div className="flex justify-between gap-3">
                              <span>
                                {t("dashboard.usage.cumulativeTotal", "Cumulative total")}
                              </span>
                              <span className="font-medium tabular-nums text-foreground">
                                {formatCompactTokens(cumulativeTotal)}
                              </span>
                            </div>
                          </div>
                        </div>
                      );
                    }}
                  />
                  {markerLabel ? (
                    <ReferenceLine
                      x={markerLabel}
                      stroke="hsl(var(--muted-foreground))"
                      strokeDasharray="4 4"
                      label={{
                        value: markerText,
                        position: "top",
                        fill: "hsl(var(--muted-foreground))",
                        fontSize: 11,
                      }}
                    />
                  ) : null}
                  {series.models.map((model) => (
                    <Area
                      key={model}
                      type="monotone"
                      dataKey={model}
                      stackId="tokens"
                      stroke={modelToColor(model)}
                      fill={modelToColor(model)}
                      fillOpacity={0.55}
                      strokeWidth={1.5}
                      // DH-12c: background revalidation must not replay the
                      // grow-from-zero enter animation.
                      isAnimationActive={false}
                    />
                  ))}
                </AreaChart>
              </ChartContainer>

              <div className="flex flex-col gap-2">
                <p className="sr-only">
                  {t("dashboard.usage.legend", "Model legend")}
                </p>
                <ScrollArea className="h-32 rounded-md border bg-muted/20">
                  <ul className="flex flex-col gap-1.5 p-3 pr-4">
                    {series.models.map((model) => (
                      <li key={model} className="flex items-center gap-2">
                        <span
                          className="h-2.5 w-2.5 shrink-0 rounded-sm"
                          style={{ backgroundColor: modelToColor(model) }}
                        />
                        <span className="truncate font-mono text-xs text-muted-foreground">
                          {model}
                        </span>
                      </li>
                    ))}
                  </ul>
                </ScrollArea>
              </div>
            </>
          )}
        </CardContent>
      </Card>
    </motion.div>
  );
}
