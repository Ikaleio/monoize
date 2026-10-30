import { QueryError, type QueryErrorProps } from "@/components/ui/query-error";
import { useTranslation } from "react-i18next";
import { Link } from "react-router-dom";
import { RefreshCw } from "lucide-react";
import {
  Bar,
  BarChart,
  CartesianGrid,
  ReferenceLine,
  XAxis,
  YAxis,
} from "recharts";
import {
  Card,
  CardContent,
  CardDescription,
  CardHeader,
  CardTitle,
} from "@/components/ui/card";
import {
  ChartContainer,
  ChartTooltip,
  type ChartConfig,
} from "@/components/ui/chart";
import { Button } from "@/components/ui/button";
import { Skeleton } from "@/components/ui/skeleton";
import { EmptyState } from "@/components/ui/empty-state";
import { motion, transitions } from "@/components/ui/motion";
import { cn } from "@/lib/utils";
import type { DashboardAnalytics } from "@/lib/api";
import {
  bucketLabelForWindow,
  USAGE_WINDOWS,
  usesTodayMarker,
  type UsageWindow,
} from "@/lib/usage-window";
import { SegmentedControl } from "@/components/ui/segmented-control";
import {
  formatCompactTokens,
  OTHER_SERIES,
  OTHER_SERIES_COLOR,
  type UsageSeries,
} from "./utils";

interface UsageChartPanelProps extends QueryErrorProps {
  failed?: boolean;
  analytics: DashboardAnalytics | undefined;
  series: UsageSeries;
  /** First load with no data for the panel (DH-12a): show skeleton. */
  loading?: boolean;
  /** Fetching another window while previous data is shown (DH-12b): dim only. */
  pending?: boolean;
  window: UsageWindow;
  onWindowChange: (window: UsageWindow) => void;
  onRefresh: () => void;
}

function updatedAtClock(timeTo: string | undefined): string | null {
  if (!timeTo) return null;
  const date = new Date(timeTo);
  return Number.isNaN(date.getTime()) ? null : bucketLabelForWindow("24h", date);
}

export function UsageChartPanel({
  analytics,
  series,
  loading,
  pending,
  window,
  onWindowChange,
  onRefresh,
  failed,
  onRetry,
  retrying,
}: UsageChartPanelProps) {
  const { t } = useTranslation();
  const otherLabel = t("dashboard.usage.other", "Other");
  const seriesLabel = (key: string) => (key === OTHER_SERIES ? otherLabel : key);

  const chartConfig: ChartConfig = {};
  for (const key of series.series) {
    chartConfig[key] = {
      label: key === OTHER_SERIES ? otherLabel : key,
      color: series.colors[key],
    };
  }

  const markerLabel =
    series.rows.length > 0
      ? String(series.rows[series.rows.length - 1]?.label ?? "")
      : "";
  const markerText = usesTodayMarker(window)
    ? t("dashboard.usage.today", "Today")
    : t("dashboard.usage.now", "Now");
  const updatedAt = updatedAtClock(analytics?.time_to);

  return (
    <motion.div
      initial={{ opacity: 0, y: 18 }}
      animate={{ opacity: 1, y: 0 }}
      transition={{ delay: 0.12, ...transitions.normal }}
      className="min-w-0"
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
                "Tokens per period for the current account",
              )}
            </CardDescription>
            {analytics ? (
              <p className="mt-1 flex items-baseline gap-1.5">
                <span className="font-display text-xl font-semibold tabular-nums">
                  {formatCompactTokens(series.windowTotal)}
                </span>
                <span className="text-sm text-muted-foreground">
                  {t("dashboard.usage.windowTotal", {
                    window: t(`dashboard.usage.windowName.${window}`),
                  })}
                </span>
              </p>
            ) : null}
          </div>
          <div className="flex flex-wrap items-center gap-2">
            {updatedAt ? (
              <span className="text-xs text-muted-foreground tabular-nums">
                {t("dashboard.usage.updatedAt", { time: updatedAt })}
              </span>
            ) : null}
            <Button
              type="button"
              variant="ghost"
              size="icon"
              disabled={retrying}
              aria-label={t("dashboard.refreshAction")}
              title={t("dashboard.refreshAction")}
              onClick={onRefresh}
            >
              <RefreshCw
                className={cn(
                  retrying && "animate-spin motion-reduce:animate-none",
                )}
                aria-hidden="true"
              />
            </Button>
            <SegmentedControl
              value={window}
              options={USAGE_WINDOWS}
              onChange={onWindowChange}
              ariaLabel={t("dashboard.usage.timeRange", "Time range")}
            />
          </div>
        </CardHeader>

        <CardContent
          className={cn(
            "flex flex-col gap-3 p-4 pt-2 transition-opacity",
            pending && "opacity-60",
          )}
        >
          {failed && (
            <QueryError
              onRetry={onRetry}
              retrying={retrying}
              stale={analytics !== undefined}
            />
          )}
          {failed && analytics === undefined ? null : loading &&
            analytics === undefined ? (
            <Skeleton className="h-72 w-full rounded-lg" />
          ) : series.rows.length === 0 || series.series.length === 0 ? (
            analytics?.has_any_request === false ? (
              <EmptyState
                title={t("dashboard.noAnalysisData", "No requests yet")}
                description={t(
                  "dashboard.noAnalysisDataDescription",
                  "Create an API key and send a request. Statistics then appear here.",
                )}
                variant="inline"
                action={
                  <Button asChild variant="outline">
                    <Link to="/dashboard/tokens">
                      {t("dashboard.usage.createApiKey", "Create API key")}
                    </Link>
                  </Button>
                }
                className="min-h-60 py-8"
              />
            ) : (
              <EmptyState
                title={t("dashboard.usage.emptyWindow", {
                  window: t(`dashboard.usage.windowName.${window}`),
                })}
                variant="inline"
                action={
                  window === "30d" ? undefined : (
                    <Button
                      type="button"
                      variant="outline"
                      onClick={() => onWindowChange("30d")}
                    >
                      {t("dashboard.usage.viewThirtyDays")}
                    </Button>
                  )
                }
                className="min-h-60 py-8"
              />
            )
          ) : (
            <>
              <ChartContainer
                config={chartConfig}
                className="h-72 w-full !aspect-auto sm:h-80"
              >
                <BarChart
                  data={series.rows}
                  // Keep the marker label inside the plot bounds so localized
                  // glyphs cannot be clipped by the chart container (DH-6d).
                  margin={{ top: 24, right: 20, left: 0, bottom: 0 }}
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
                      value: t("dashboard.usage.tokensAxis", "Tokens per period"),
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
                      const idx = Number(payload[0]?.payload?.bucketIndex ?? -1);
                      const perBucket =
                        idx >= 0 ? (series.bucketByModel[idx] ?? {}) : {};
                      const bucketTotal =
                        idx >= 0 ? (series.bucketTotals[idx] ?? 0) : 0;
                      // Lists real models, so the merged "Other" bar expands here (DH-6e).
                      const entries = Object.entries(perBucket)
                        .map(([model, tokens]) => ({
                          model,
                          tokens,
                          color: series.colors[model] ?? OTHER_SERIES_COLOR,
                        }))
                        .filter((e) => e.tokens > 0)
                        .sort((a, b) => b.tokens - a.tokens);

                      return (
                        <div className="flex min-w-56 flex-col gap-2 rounded-lg border bg-background px-3 py-2.5 text-xs shadow-md">
                          <div className="flex items-baseline justify-between gap-3 border-b pb-2">
                            <span className="font-medium">{String(label)}</span>
                            <span className="text-muted-foreground">
                              {t(
                                "dashboard.usage.periodBreakdown",
                                "Period breakdown",
                              )}
                            </span>
                          </div>
                          <ul className="flex flex-col gap-1.5">
                            {entries.map((entry) => {
                              const pct =
                                bucketTotal > 0
                                  ? (
                                      (entry.tokens / bucketTotal) *
                                      100
                                    ).toFixed(1)
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
                                    <span className="text-foreground/70">
                                      ({pct}%)
                                    </span>
                                  </span>
                                </li>
                              );
                            })}
                          </ul>
                          <div className="flex justify-between gap-3 border-t pt-2 text-muted-foreground">
                            <span>
                              {t("dashboard.usage.periodTotal", "Period total")}
                            </span>
                            <span className="font-medium tabular-nums text-foreground">
                              {formatCompactTokens(bucketTotal)}
                            </span>
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
                        offset: 8,
                        fill: "hsl(var(--muted-foreground))",
                        fontSize: 11,
                      }}
                    />
                  ) : null}
                  {series.series.map((key) => (
                    <Bar
                      key={key}
                      dataKey={key}
                      stackId="tokens"
                      fill={series.colors[key]}
                      maxBarSize={40}
                      // DH-12c: background revalidation must not replay the
                      // grow-from-zero enter animation.
                      isAnimationActive={false}
                    />
                  ))}
                </BarChart>
              </ChartContainer>

              <div className="min-w-0 border-t pt-3">
                <p className="sr-only">
                  {t("dashboard.usage.legend", "Model legend")}
                </p>
                <ul className="grid grid-cols-2 gap-x-4 gap-y-2 sm:grid-cols-3 lg:grid-cols-5">
                  {series.series.map((key) => (
                    <li key={key} className="flex min-w-0 items-center gap-2">
                      <span
                        className="size-2.5 shrink-0 rounded-sm"
                        style={{ backgroundColor: series.colors[key] }}
                      />
                      <span
                        className="truncate font-mono text-xs text-muted-foreground"
                        title={seriesLabel(key)}
                      >
                        {seriesLabel(key)}
                      </span>
                    </li>
                  ))}
                </ul>
              </div>
            </>
          )}
        </CardContent>
      </Card>
    </motion.div>
  );
}
