import { useMemo, useState } from "react";
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
import {
  Select,
  SelectContent,
  SelectItem,
  SelectTrigger,
  SelectValue,
} from "@/components/ui/select";
import { ScrollArea } from "@/components/ui/scroll-area";
import { Skeleton } from "@/components/ui/skeleton";
import { EmptyState } from "@/components/ui/empty-state";
import { motion, transitions } from "@/components/ui/motion";
import type { DashboardAnalytics } from "@/lib/api";
import {
  buildCumulativeTokenSeries,
  formatCompactTokens,
  modelToColor,
} from "./utils";

interface UsageChartPanelProps {
  analytics: DashboardAnalytics | undefined;
  loading?: boolean;
}

export function UsageChartPanel({ analytics, loading }: UsageChartPanelProps) {
  const { t } = useTranslation();
  const [groupBy, setGroupBy] = useState<"model">("model");

  const series = useMemo(
    () => buildCumulativeTokenSeries(analytics?.buckets ?? []),
    [analytics?.buckets]
  );

  const chartConfig = useMemo<ChartConfig>(() => {
    const cfg: ChartConfig = {};
    for (const model of series.models) {
      cfg[model] = { label: model, color: modelToColor(model) };
    }
    return cfg;
  }, [series.models]);

  const todayLabel =
    series.rows.length > 0 ? String(series.rows[series.rows.length - 1]?.label ?? "") : "";

  if (loading) {
    return (
      <Card>
        <CardHeader className="flex flex-col gap-2 p-4 pb-2">
          <Skeleton className="h-5 w-36" />
          <Skeleton className="h-4 w-64" />
        </CardHeader>
        <CardContent className="p-4 pt-2">
          <Skeleton className="h-72 w-full rounded-lg" />
        </CardContent>
      </Card>
    );
  }

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
                "Your usage per day across this billing period"
              )}
            </CardDescription>
          </div>
          <Select value={groupBy} onValueChange={(v) => setGroupBy(v as "model")}>
            <SelectTrigger className="h-8 w-40 text-xs" aria-label={t("dashboard.usage.groupBy", "Group By")}>
              <SelectValue placeholder={t("dashboard.usage.groupBy", "Group By")} />
            </SelectTrigger>
            <SelectContent>
              <SelectItem value="model">
                {t("dashboard.usage.groupByModel", "Group By: Model")}
              </SelectItem>
            </SelectContent>
          </Select>
        </CardHeader>

        <CardContent className="flex flex-col gap-3 p-4 pt-2">
          {series.rows.length === 0 || series.models.length === 0 ? (
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
                  margin={{ top: 12, right: 8, left: 0, bottom: 0 }}
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
                      const daily = idx >= 0 ? series.dailyByBucket[idx] ?? {} : {};
                      const dailyTotal = idx >= 0 ? series.dailyTotals[idx] ?? 0 : 0;
                      const cumulativeTotal =
                        idx >= 0 ? series.cumulativeTotals[idx] ?? 0 : 0;
                      const entries = series.models
                        .map((model) => ({
                          model,
                          daily: daily[model] ?? 0,
                          color: modelToColor(model),
                        }))
                        .filter((e) => e.daily > 0)
                        .sort((a, b) => b.daily - a.daily);

                      return (
                        <div className="flex min-w-56 flex-col gap-2 rounded-lg border bg-background px-3 py-2.5 text-xs shadow-md">
                          <div className="flex items-baseline justify-between gap-3 border-b pb-2">
                            <span className="font-medium">{String(label)}</span>
                            <span className="text-muted-foreground">
                              {t("dashboard.usage.dailyBreakdown", "Daily breakdown")}
                            </span>
                          </div>
                          <ul className="flex flex-col gap-1.5">
                            {entries.map((entry) => {
                              const pct =
                                dailyTotal > 0
                                  ? ((entry.daily / dailyTotal) * 100).toFixed(1)
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
                                    {formatCompactTokens(entry.daily)}{" "}
                                    <span className="text-foreground/70">({pct}%)</span>
                                  </span>
                                </li>
                              );
                            })}
                          </ul>
                          <div className="flex flex-col gap-1 border-t pt-2 text-muted-foreground">
                            <div className="flex justify-between gap-3">
                              <span>{t("dashboard.usage.dailyTotal", "Daily total")}</span>
                              <span className="font-medium tabular-nums text-foreground">
                                {formatCompactTokens(dailyTotal)}
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
                  {todayLabel ? (
                    <ReferenceLine
                      x={todayLabel}
                      stroke="hsl(var(--muted-foreground))"
                      strokeDasharray="4 4"
                      label={{
                        value: t("dashboard.usage.today", "Today"),
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
                      isAnimationActive
                      animationDuration={700}
                      animationEasing="ease-out"
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
