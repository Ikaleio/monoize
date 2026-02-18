import { useCallback, useMemo, useState } from "react";
import { useTranslation } from "react-i18next";
import { Bar, BarChart, CartesianGrid, XAxis, YAxis } from "recharts";
import { ChevronUp, ChevronDown } from "lucide-react";
import { Card, CardContent, CardHeader, CardTitle } from "@/components/ui/card";
import {
  ChartContainer,
  ChartTooltip,
  ChartTooltipContent,
  type ChartConfig,
} from "@/components/ui/chart";
import { Skeleton } from "@/components/ui/skeleton";
import { useAuth } from "@/hooks/use-auth";
import { type Provider, type RequestLog } from "@/lib/api";
import { useProviders, usePublicSettings, useRequestLogs, useStats } from "@/lib/swr";
import { cn } from "@/lib/utils";
import { PageWrapper, motion, transitions } from "@/components/ui/motion";
import { toast } from "sonner";

type DashboardProvider = Pick<Provider, "id" | "name" | "provider_type" | "models" | "channels">;
type AnalysisTabId = "spendDistribution" | "callDistribution" | "callRank";

interface MetricRow {
  key: string;
  label: string;
  value: string;
}

interface OverviewCardData {
  key: string;
  title: string;
  metrics: MetricRow[];
}

interface ParsedLog {
  ts: number;
  providerLabel: string;
  modelId: string;
  chargeUsd: number;
}

interface StackedBucketRow {
  label: string;
  [modelKey: string]: number | string;
}

interface AnalysisData {
  rows: StackedBucketRow[];
  models: string[];
  total: number;
  valueType: "money" | "count";
  metricTitle: string;
}

const ANALYSIS_TABS: Array<{ id: AnalysisTabId; i18nKey: string; fallback: string }> = [
  { id: "spendDistribution", i18nKey: "dashboard.analysisTabs.spendDistribution", fallback: "Spend Distribution" },
  { id: "callDistribution", i18nKey: "dashboard.analysisTabs.callDistribution", fallback: "Call Distribution" },
  { id: "callRank", i18nKey: "dashboard.analysisTabs.callRank", fallback: "Call Ranking" },
];

function formatNumber(value: number): string {
  return value.toLocaleString("en-US");
}

function formatMoney(value: string | number | undefined): string {
  const parsed = Number(value ?? 0);
  if (!Number.isFinite(parsed)) return "$0.00";
  return `$${parsed.toLocaleString("en-US", { minimumFractionDigits: 2, maximumFractionDigits: 2 })}`;
}

function parseTimestamp(value: string | undefined): number | null {
  if (!value) return null;
  const ts = Date.parse(value);
  return Number.isFinite(ts) ? ts : null;
}

function nanoUsdToUsd(nanoUsd: string | undefined): number {
  if (!nanoUsd) return 0;
  const parsed = Number(nanoUsd);
  if (!Number.isFinite(parsed)) return 0;
  return parsed / 1e9;
}

function formatTimeBucketLabel(ms: number): string {
  const d = new Date(ms);
  const month = String(d.getMonth() + 1).padStart(2, "0");
  const day = String(d.getDate()).padStart(2, "0");
  const hour = String(d.getHours()).padStart(2, "0");
  return `${month}-${day} ${hour}:00`;
}

/**
 * Deterministic color palette for model→color mapping.
 * Uses hsl-based hues to maximise visual distinction; saturation/lightness
 * are tuned for light backgrounds while remaining readable in dark mode.
 */
const MODEL_COLORS = [
  "hsl(45, 93%, 65%)",
  "hsl(330, 70%, 72%)",
  "hsl(24, 85%, 60%)",
  "hsl(160, 55%, 55%)",
  "hsl(270, 60%, 68%)",
  "hsl(200, 70%, 58%)",
  "hsl(0, 70%, 62%)",
  "hsl(95, 50%, 55%)",
  "hsl(290, 50%, 60%)",
  "hsl(180, 50%, 50%)",
  "hsl(50, 80%, 55%)",
  "hsl(210, 55%, 65%)",
  "hsl(350, 65%, 58%)",
  "hsl(130, 45%, 50%)",
  "hsl(15, 75%, 55%)",
  "hsl(240, 50%, 65%)",
] as const;

/** Stable hash → palette index so the same model always gets the same color. */
function modelToColor(modelId: string): string {
  let hash = 0;
  for (let i = 0; i < modelId.length; i++) {
    hash = ((hash << 5) - hash + modelId.charCodeAt(i)) | 0;
  }
  return MODEL_COLORS[((hash % MODEL_COLORS.length) + MODEL_COLORS.length) % MODEL_COLORS.length];
}

function OverviewCard({
  card,
  index,
}: {
  card: OverviewCardData;
  index: number;
}) {
  return (
    <motion.div
      initial={{ opacity: 0, y: 22, scale: 0.98 }}
      animate={{ opacity: 1, y: 0, scale: 1 }}
      transition={{ delay: 0.08 * index, ...transitions.normal }}
      whileHover={{ y: -3, transition: { duration: 0.18 } }}
      className="h-full"
    >
      <Card className="h-full">
        <CardHeader className="p-4 pb-2">
          <CardTitle className="text-lg">{card.title}</CardTitle>
        </CardHeader>
        <CardContent className="space-y-2.5 p-4 pt-0">
          {card.metrics.map((metric) => {
            return (
              <div
                key={metric.key}
                className="rounded-lg border bg-muted/25 px-3 py-2"
              >
                <p className="truncate text-xs text-muted-foreground">{metric.label}</p>
                <p className="truncate text-xl font-semibold leading-tight">{metric.value}</p>
              </div>
            );
          })}
        </CardContent>
      </Card>
    </motion.div>
  );
}

const LEGEND_PAGE_SIZE = 4;

function PaginatedChartLegend({
  items,
}: {
  items: Array<{ key: string; label: string; color: string }>;
}) {
  const [page, setPage] = useState(0);
  const totalPages = Math.ceil(items.length / LEGEND_PAGE_SIZE);
  const visible = items.slice(page * LEGEND_PAGE_SIZE, (page + 1) * LEGEND_PAGE_SIZE);

  return (
    <div className="flex items-center justify-center gap-3 pt-3">
      <div className="flex flex-wrap items-center justify-center gap-x-4 gap-y-1.5">
        {visible.map((item) => (
          <div key={item.key} className="flex items-center gap-1.5">
            <div
              className="h-2.5 w-2.5 shrink-0 rounded-[2px]"
              style={{ backgroundColor: item.color }}
            />
            <span className="text-xs text-muted-foreground">{item.label}</span>
          </div>
        ))}
      </div>
      {totalPages > 1 && (
        <div className="flex flex-col items-center gap-0.5 text-muted-foreground">
          <button
            disabled={page === 0}
            onClick={() => setPage((p) => Math.max(0, p - 1))}
            className="disabled:opacity-30"
          >
            <ChevronUp className="h-3.5 w-3.5" />
          </button>
          <span className="text-[10px] tabular-nums leading-none">
            {page + 1}/{totalPages}
          </span>
          <button
            disabled={page >= totalPages - 1}
            onClick={() => setPage((p) => Math.min(totalPages - 1, p + 1))}
            className="disabled:opacity-30"
          >
            <ChevronDown className="h-3.5 w-3.5" />
          </button>
        </div>
      )}
    </div>
  );
}

function DashboardSkeleton() {
  return (
    <div className="space-y-5">
      <div className="space-y-2">
        <Skeleton className="h-8 w-56" />
        <Skeleton className="h-4 w-72" />
      </div>
      <div className="grid gap-4 md:grid-cols-2 xl:grid-cols-4">
        {Array.from({ length: 4 }).map((_, index) => (
          <Skeleton key={index} className="h-[170px] rounded-xl" />
        ))}
      </div>
      <div className="grid gap-4 lg:grid-cols-3">
        <Skeleton className="h-[360px] rounded-xl lg:col-span-2" />
        <Skeleton className="h-[360px] rounded-xl" />
      </div>
    </div>
  );
}

export function DashboardPage() {
  const { t } = useTranslation();
  const { user } = useAuth();
  const [activeTab, setActiveTab] = useState<AnalysisTabId>("spendDistribution");

  const isAdmin = user?.role === "super_admin" || user?.role === "admin";
  const { data: stats, isLoading: statsLoading } = useStats();
  const { data: providerRows, isLoading: providersLoading } = useProviders({
    isPaused: () => !isAdmin,
    revalidateOnMount: isAdmin,
  });
  const { data: requestLogsResponse, isLoading: logsLoading } = useRequestLogs(400, 0);
  const { data: publicSettings, isLoading: publicSettingsLoading } = usePublicSettings();

  const providers = useMemo<DashboardProvider[]>(
    () =>
      (providerRows ?? []).map((provider) => ({
        id: provider.id,
        name: provider.name,
        provider_type: provider.provider_type,
        models: provider.models,
        channels: provider.channels,
      })),
    [providerRows]
  );

  const providerNameById = useMemo(() => {
    const map = new Map<string, string>();
    for (const provider of providers) {
      const id = provider.id?.trim();
      const name = provider.name?.trim();
      if (!id || !name) continue;
      if (!map.has(id)) {
        map.set(id, name);
      }
    }
    return map;
  }, [providers]);

  const rawLogs = requestLogsResponse?.data ?? [];
  const totalRequests = requestLogsResponse?.total ?? 0;

  const parsedLogs = useMemo<ParsedLog[]>(() => {
    return rawLogs
      .map((log: RequestLog) => {
        const ts = parseTimestamp(log.created_at);
        if (ts == null) return null;
        const providerId = log.provider_id?.trim() || "unknown";
        const providerName = providerNameById.get(providerId);
        return {
          ts,
          providerLabel: providerName || providerId,
          modelId: log.model?.trim() || "unknown",
          chargeUsd: nanoUsdToUsd(log.charge_nano_usd),
        };
      })
      .filter((row): row is ParsedLog => row !== null);
  }, [providerNameById, rawLogs]);

  const todayStart = useMemo(() => {
    const d = new Date();
    d.setHours(0, 0, 0, 0);
    return d.getTime();
  }, []);

  const logDerivedStats = useMemo(() => {
    let todayRequests = 0;
    let totalSpend = 0;
    let todaySpend = 0;
    let successCount = 0;
    let durationSum = 0;
    let durationCount = 0;
    let tokenSum = 0;

    for (const log of rawLogs) {
      const ts = parseTimestamp(log.created_at);
      const charge = nanoUsdToUsd(log.charge_nano_usd);
      totalSpend += charge;

      if (ts != null && ts >= todayStart) {
        todayRequests++;
        todaySpend += charge;
      }

      if (log.status === "success") successCount++;

      if (log.duration_ms != null && log.duration_ms > 0) {
        durationSum += log.duration_ms;
        durationCount++;
      }

      tokenSum += (log.prompt_tokens ?? 0) + (log.completion_tokens ?? 0);
    }

    const successRate = rawLogs.length > 0
      ? Math.round((successCount / rawLogs.length) * 100)
      : 0;
    const avgLatency = durationCount > 0
      ? Math.round(durationSum / durationCount)
      : 0;

    return { todayRequests, totalSpend, todaySpend, successRate, avgLatency, tokenSum };
  }, [rawLogs, todayStart]);

  const loading = statsLoading || logsLoading || publicSettingsLoading || (isAdmin && providersLoading);

  const tt = useCallback(
    (key: string, fallback: string, options?: Record<string, unknown>): string => {
      const translated = t(key, { ...(options ?? {}), defaultValue: fallback } as never);
      return typeof translated === "string" ? translated : fallback;
    },
    [t]
  );

  const overviewCards = useMemo<OverviewCardData[]>(
    () => [
      {
        key: "account",
        title: tt("dashboard.cards.accountData", "Account Data"),
        metrics: [
          {
            key: "balance",
            label: tt("dashboard.cards.currentBalance", "Current Balance"),
            value: formatMoney(user?.balance_usd),
          },
          {
            key: "myKeys",
            label: tt("dashboard.cards.myApiKeys", "My API Keys"),
            value: formatNumber(stats?.my_api_keys_count ?? 0),
          },
        ],
      },
      {
        key: "requests",
        title: tt("dashboard.cards.requestOverview", "Request Overview"),
        metrics: [
          {
            key: "totalRequests",
            label: tt("dashboard.cards.totalRequests", "Total Requests"),
            value: formatNumber(totalRequests),
          },
          {
            key: "todayRequests",
            label: tt("dashboard.cards.todayRequests", "Today's Requests"),
            value: formatNumber(logDerivedStats.todayRequests),
          },
        ],
      },
      {
        key: "cost",
        title: tt("dashboard.cards.costOverview", "Cost Overview"),
        metrics: [
          {
            key: "totalSpend",
            label: tt("dashboard.cards.totalSpend", "Total Spend"),
            value: formatMoney(logDerivedStats.totalSpend),
          },
          {
            key: "todaySpend",
            label: tt("dashboard.cards.todaySpend", "Today's Spend"),
            value: formatMoney(logDerivedStats.todaySpend),
          },
        ],
      },
      {
        key: "perf",
        title: tt("dashboard.cards.performance", "Performance Metrics"),
        metrics: [
          {
            key: "avgLatency",
            label: tt("dashboard.cards.avgLatency", "Average Latency"),
            value: `${formatNumber(logDerivedStats.avgLatency)} ms`,
          },
          {
            key: "successRate",
            label: tt("dashboard.cards.successRate", "Success Rate"),
            value: `${logDerivedStats.successRate}%`,
          },
        ],
      },
    ],
    [
      logDerivedStats,
      totalRequests,
      stats?.my_api_keys_count,
      user?.balance_usd,
      tt,
    ]
  );

  const analysisData = useMemo<AnalysisData>(() => {
    const base: AnalysisData = {
      rows: [],
      models: [],
      total: 0,
      valueType: (activeTab === "spendDistribution" ? "money" : "count"),
      metricTitle:
        activeTab === "callRank"
          ? tt("dashboard.calls", "Calls")
          : tt("dashboard.value", "Value"),
    };

    if (parsedLogs.length === 0) return base;

    const bucketCount = 8;
    const maxTs = Math.max(...parsedLogs.map((l) => l.ts));
    const endMs = Math.max(Date.now(), maxTs);
    const rangeMs = 24 * 60 * 60 * 1000;
    const startMs = endMs - rangeMs;
    const bucketWidth = rangeMs / bucketCount;

    const getValue = (log: ParsedLog): number => {
      if (activeTab === "spendDistribution") return log.chargeUsd;
      return 1;
    };

    const getGroupKey = (log: ParsedLog): string => {
      if (activeTab === "callRank") return log.providerLabel;
      return log.modelId;
    };

    const modelTotals = new Map<string, number>();
    const bucketData: Array<Map<string, number>> = Array.from({ length: bucketCount }, () => new Map());

    for (const log of parsedLogs) {
      if (log.ts < startMs || log.ts > endMs) continue;
      const idx = Math.min(bucketCount - 1, Math.max(0, Math.floor((log.ts - startMs) / bucketWidth)));
      const key = getGroupKey(log);
      const val = getValue(log);
      bucketData[idx].set(key, (bucketData[idx].get(key) ?? 0) + val);
      modelTotals.set(key, (modelTotals.get(key) ?? 0) + val);
    }

    const models = [...modelTotals.entries()]
      .sort((a, b) => b[1] - a[1])
      .map(([k]) => k);

    const rows: StackedBucketRow[] = bucketData.map((bucket, i) => {
      const row: StackedBucketRow = { label: formatTimeBucketLabel(startMs + i * bucketWidth) };
      for (const model of models) {
        const raw = bucket.get(model) ?? 0;
        row[model] = base.valueType === "money" ? Number(raw.toFixed(4)) : raw;
      }
      return row;
    });

    const total = [...modelTotals.values()].reduce((s, v) => s + v, 0);
    return { ...base, rows, models, total };
  }, [activeTab, parsedLogs, tt]);

  const analysisTotalDisplay =
    analysisData.valueType === "money"
      ? formatMoney(analysisData.total)
      : formatNumber(Math.round(analysisData.total));

  const activeTabMeta = ANALYSIS_TABS.find((tab) => tab.id === activeTab) ?? ANALYSIS_TABS[0];
  const analysisHeading = tt(activeTabMeta.i18nKey, activeTabMeta.fallback);

  const analysisChartConfig = useMemo<ChartConfig>(() => {
    const cfg: ChartConfig = {};
    for (const model of analysisData.models) {
      cfg[model] = {
        label: model,
        color: modelToColor(model),
      };
    }
    return cfg;
  }, [analysisData.models]);

  const legendItems = useMemo(
    () =>
      analysisData.models.map((model) => ({
        key: model,
        label: model,
        color: modelToColor(model),
      })),
    [analysisData.models]
  );

  const formatAnalysisValue = (value: number): string =>
    analysisData.valueType === "money" ? formatMoney(value) : formatNumber(Math.round(value));

  if (loading) {
    return (
      <PageWrapper className="h-full min-h-0 overflow-hidden space-y-4">
        <DashboardSkeleton />
      </PageWrapper>
    );
  }

  return (
    <PageWrapper className="flex h-full min-h-0 flex-col gap-4 overflow-hidden">
      <motion.header
        initial={{ opacity: 0, y: -12 }}
        animate={{ opacity: 1, y: 0 }}
        transition={transitions.normal}
        className="shrink-0"
      >
        <div className="space-y-1">
          <h1 className="text-3xl font-bold tracking-tight">
            {tt("dashboard.greeting", "👋 Good afternoon, {{username}}", { username: user?.username ?? "User" })}
          </h1>
          <p className="text-sm text-muted-foreground">{tt("dashboard.subtitle", "Realtime overview of account status, usage and routing data")}</p>
        </div>
      </motion.header>

      <section className="shrink-0 grid gap-3 md:grid-cols-2 xl:grid-cols-4">
        {overviewCards.map((card, index) => (
          <OverviewCard key={card.key} card={card} index={index} />
        ))}
      </section>

      <section className="grid min-h-0 flex-1 items-stretch gap-4 lg:grid-cols-3">
        <motion.div
          initial={{ opacity: 0, y: 18 }}
          animate={{ opacity: 1, y: 0 }}
          transition={{ delay: 0.15, ...transitions.normal }}
          className="min-h-0 h-full lg:col-span-2"
        >
          <Card className="flex h-full min-h-0 flex-col">
            <CardHeader className="border-b">
              <div className="flex items-center gap-3">
                <CardTitle className="shrink-0 text-xl">{tt("dashboard.analysisTitle", "Model Data")}</CardTitle>
                <div className="ml-auto flex min-w-0 items-center justify-end gap-1.5 whitespace-nowrap">
                {ANALYSIS_TABS.map((tab, index) => {
                  const active = activeTab === tab.id;
                  return (
                    <div key={tab.id} className="flex items-center gap-2">
                      {index > 0 && <span className="text-muted-foreground/40">/</span>}
                      <button
                        onClick={() => setActiveTab(tab.id)}
                        className={cn(
                          "relative shrink-0 px-1 py-1 text-xs sm:text-sm transition-colors",
                          active ? "font-medium text-foreground" : "text-muted-foreground hover:text-foreground"
                        )}
                      >
                        <span>{tt(tab.i18nKey, tab.fallback)}</span>
                        {active && (
                          <motion.span
                            layoutId="dashboard-analysis-tab"
                            className="absolute inset-x-0 -bottom-1 h-0.5 rounded-full bg-primary"
                            transition={{ type: "spring", stiffness: 420, damping: 36 }}
                          />
                        )}
                      </button>
                    </div>
                  );
                })}
                </div>
              </div>
            </CardHeader>

            <CardContent className="flex min-h-0 flex-1 flex-col space-y-3 pt-4">
              <div className="flex items-center justify-between gap-3">
                <h2 className="min-w-0 truncate text-lg font-semibold tracking-tight">
                  {analysisHeading}
                </h2>
                <p className="shrink-0 whitespace-nowrap text-sm text-muted-foreground">
                  {tt("dashboard.chartTotal", "Total: {{total}}", { total: analysisTotalDisplay })}
                </p>
              </div>

              {analysisData.rows.length > 0 ? (
                <div className="flex-1 min-h-0 flex flex-col rounded-lg border bg-muted/20 p-2 sm:p-3">
                  <div className="flex-1 min-h-0">
                    <ChartContainer config={analysisChartConfig} className="h-full min-h-[170px] w-full !aspect-auto">
                      <BarChart data={analysisData.rows} margin={{ top: 8, right: 8, left: 0, bottom: 0 }}>
                        <CartesianGrid vertical={false} />
                        <XAxis
                          dataKey="label"
                          tickLine={false}
                          axisLine={false}
                          tickMargin={8}
                          minTickGap={16}
                        />
                        <YAxis tickLine={false} axisLine={false} width={48} />
                        <ChartTooltip
                          content={
                            <ChartTooltipContent
                              labelFormatter={(label) => String(label)}
                              formatter={(value, name) => (
                                <>
                                  <div
                                    className="h-2.5 w-2.5 shrink-0 rounded-[2px]"
                                    style={{ backgroundColor: modelToColor(String(name)) }}
                                  />
                                  <div className="flex flex-1 items-center justify-between gap-2 leading-none">
                                    <span className="text-muted-foreground">{String(name)}</span>
                                    <span className="font-mono font-medium tabular-nums text-foreground">
                                      {formatAnalysisValue(Number(value))}
                                    </span>
                                  </div>
                                </>
                              )}
                            />
                          }
                        />
                        {analysisData.models.map((model) => (
                          <Bar
                            key={model}
                            dataKey={model}
                            stackId="a"
                            fill={modelToColor(model)}
                            radius={0}
                            isAnimationActive
                            animationDuration={450}
                          />
                        ))}
                      </BarChart>
                    </ChartContainer>
                  </div>
                  <PaginatedChartLegend items={legendItems} />
                </div>
              ) : (
                <div className="flex-1 min-h-0 rounded-lg border bg-muted/20 p-6 sm:p-8">
                  <div className="flex h-full min-h-[170px] flex-col items-center justify-center text-center">
                    <p className="text-base font-medium">
                      {tt("dashboard.noAnalysisData", "No request log data available")}
                    </p>
                    <p className="mt-2 text-sm text-muted-foreground">
                      {tt("dashboard.noAnalysisDataDescription", "Statistics will appear automatically after requests are made.")}
                    </p>
                  </div>
                </div>
              )}
            </CardContent>
          </Card>
        </motion.div>

        <motion.div
          initial={{ opacity: 0, y: 18 }}
          animate={{ opacity: 1, y: 0 }}
          transition={{ delay: 0.24, ...transitions.normal }}
          className="min-h-0 h-full"
        >
          <Card className="flex h-full min-h-0 flex-col">
            <CardHeader className="border-b">
              <CardTitle className="text-xl">{tt("dashboard.apiInformation", "API Information")}</CardTitle>
            </CardHeader>
            <CardContent className="flex flex-1 min-h-0 flex-col pt-4">
              {!publicSettings?.api_base_url ? (
                <div className="flex flex-1 flex-col items-center justify-center text-center">
                  <p className="mt-4 text-xl font-semibold">{tt("dashboard.noApiInfo", "No API Information")}</p>
                  <p className="mt-2 max-w-xs text-sm leading-6 text-muted-foreground">
                    {tt("dashboard.noApiInfoDescription", "Please configure the API base URL in system settings.")}
                  </p>
                </div>
              ) : (
                <div className="flex-1 min-h-0 space-y-2 overflow-auto">
                  <motion.button
                    type="button"
                    initial={{ opacity: 0, x: 12 }}
                    animate={{ opacity: 1, x: 0 }}
                    transition={transitions.normal}
                    className="w-full rounded-lg border bg-muted/30 p-2.5 text-left transition-colors hover:bg-muted/50 active:bg-muted/70"
                    onClick={() => {
                      navigator.clipboard.writeText(publicSettings.api_base_url);
                      toast.success(tt("common.copied", "Copied"));
                    }}
                  >
                    <p className="text-xs text-muted-foreground">{tt("dashboard.apiBaseUrl", "API Base URL")}</p>
                    <p className="mt-0.5 truncate font-mono text-xs font-semibold">{publicSettings.api_base_url}</p>
                  </motion.button>

                  {[
                    { label: "Chat Completions", path: "/v1/chat/completions" },
                    { label: "Responses", path: "/v1/responses" },
                    { label: "Models", path: "/v1/models" },
                  ].map((endpoint, index) => {
                    const fullUrl = `${publicSettings.api_base_url.replace(/\/+$/, "")}${endpoint.path}`;
                    return (
                      <motion.button
                        key={endpoint.path}
                        type="button"
                        initial={{ opacity: 0, x: 12 }}
                        animate={{ opacity: 1, x: 0 }}
                        transition={{ delay: 0.06 * (index + 1), ...transitions.normal }}
                        className="w-full rounded-lg border bg-muted/30 p-2.5 text-left transition-colors hover:bg-muted/50 active:bg-muted/70"
                        onClick={() => {
                          navigator.clipboard.writeText(fullUrl);
                          toast.success(tt("common.copied", "Copied"));
                        }}
                      >
                        <p className="text-xs text-muted-foreground">{endpoint.label}</p>
                        <p className="mt-0.5 font-mono text-xs text-muted-foreground">{endpoint.path}</p>
                      </motion.button>
                    );
                  })}
                </div>
              )}
            </CardContent>
          </Card>
        </motion.div>
      </section>
    </PageWrapper>
  );
}
