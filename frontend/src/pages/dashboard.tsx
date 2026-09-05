import { useCallback, useState } from "react";
import { useTranslation } from "react-i18next";
import { useAuth } from "@/hooks/use-auth";
import {
  useDashboardAnalytics,
  useDashboardPerformance,
  usePublicSettings,
  useWindowedRequestLogs,
} from "@/lib/swr";
import {
  DEFAULT_USAGE_WINDOW,
  USAGE_WINDOW_QUERY,
  type UsageWindow,
} from "@/lib/usage-window";
import { PageWrapper, motion, transitions } from "@/components/ui/motion";
import { PageHeader } from "@/components/ui/page-header";
import { Skeleton } from "@/components/ui/skeleton";
import { AccountStrip } from "./dashboard/account-strip";
import { UsageChartPanel } from "./dashboard/usage-chart";
import { RecentUsagePanel } from "./dashboard/recent-usage";
import { ApiInfoPanel } from "./dashboard/api-info-panel";
import { PerformancePanel } from "./dashboard/performance-panel";

const ACCOUNT_OVERVIEW_QUERY = USAGE_WINDOW_QUERY["24h"];

function GreetingSkeleton() {
  return (
    <div className="flex flex-col gap-2">
      <Skeleton className="h-9 w-64" />
      <Skeleton className="h-4 w-80" />
    </div>
  );
}

export function DashboardPage() {
  const { t } = useTranslation();
  const { user } = useAuth();

  // Windows live in React state only (DH-6i): default 1h on every mount,
  // independent selections for the chart and the recent-usage table.
  const [chartWindow, setChartWindow] =
    useState<UsageWindow>(DEFAULT_USAGE_WINDOW);
  const [recentWindow, setRecentWindow] =
    useState<UsageWindow>(DEFAULT_USAGE_WINDOW);

  const chartQuery = USAGE_WINDOW_QUERY[chartWindow];
  // keepPreviousData: a window switch keeps showing the previous chart until
  // the new payload resolves (DH-12b) instead of flashing a skeleton.
  const {
    data: usageAnalytics,
    error: usageError,
    isLoading: usageLoading,
    isValidating: usageValidating,
    mutate: retryUsage,
  } = useDashboardAnalytics(chartQuery.buckets, chartQuery.rangeHours, {
    keepPreviousData: true,
  });
  const {
    data: accountAnalytics,
    error: accountError,
    isLoading: accountAnalyticsLoading,
    isValidating: accountValidating,
    mutate: retryAccount,
  } = useDashboardAnalytics(
    ACCOUNT_OVERVIEW_QUERY.buckets,
    ACCOUNT_OVERVIEW_QUERY.rangeHours,
  );
  const {
    data: requestLogsResponse,
    error: logsError,
    isLoading: logsLoading,
    isValidating: logsValidating,
    mutate: retryLogs,
  } = useWindowedRequestLogs(recentWindow, 200);
  const {
    data: publicSettings,
    error: settingsError,
    isLoading: publicSettingsLoading,
    isValidating: settingsValidating,
    mutate: retrySettings,
  } = usePublicSettings();
  const {
    data: performance,
    error: performanceError,
    isLoading: performanceLoading,
    isValidating: performanceValidating,
    mutate: retryPerformance,
  } = useDashboardPerformance();

  const tt = useCallback(
    (
      key: string,
      fallback: string,
      options?: Record<string, unknown>,
    ): string => {
      const translated = t(key, {
        ...(options ?? {}),
        defaultValue: fallback,
      } as never);
      return typeof translated === "string" ? translated : fallback;
    },
    [t],
  );

  const logs = requestLogsResponse?.data;
  const userLoading = !user;

  return (
    <PageWrapper className="flex min-h-0 flex-col gap-4 pb-6">
      <motion.header
        initial={{ opacity: 0, y: -12 }}
        animate={{ opacity: 1, y: 0 }}
        transition={transitions.normal}
        className="shrink-0"
      >
        {userLoading ? (
          <GreetingSkeleton />
        ) : (
          <PageHeader
            title={tt("dashboard.greeting", "Good afternoon, {{username}}", {
              username: user?.username ?? "User",
            })}
            description={tt(
              "dashboard.subtitle",
              "Account, usage, API, and platform performance at a glance",
            )}
          />
        )}
      </motion.header>

      <AccountStrip
        user={user}
        analytics={accountAnalytics}
        failed={Boolean(accountError)}
        onRetry={retryAccount}
        retrying={accountValidating}
        loading={
          userLoading ||
          (accountAnalyticsLoading && accountAnalytics === undefined)
        }
      />
      <UsageChartPanel
        analytics={usageAnalytics}
        failed={Boolean(usageError)}
        onRetry={retryUsage}
        retrying={usageValidating}
        loading={usageLoading && !usageAnalytics}
        pending={usageLoading && usageAnalytics !== undefined}
        window={chartWindow}
        onWindowChange={setChartWindow}
      />

      <section className="grid min-h-0 items-stretch gap-4 lg:grid-cols-3">
        <div className="min-h-0 lg:col-span-2">
          <RecentUsagePanel
            logs={logs}
            failed={Boolean(logsError)}
            onRetry={retryLogs}
            retrying={logsValidating}
            loading={logsLoading && !requestLogsResponse}
            pending={logsLoading && requestLogsResponse !== undefined}
            window={recentWindow}
            onWindowChange={setRecentWindow}
          />
        </div>
        <div className="min-h-0">
          <ApiInfoPanel
            settings={publicSettings}
            loading={publicSettingsLoading}
            failed={Boolean(settingsError)}
            onRetry={retrySettings}
            retrying={settingsValidating}
          />
        </div>
      </section>

      <PerformancePanel
        data={performance}
        loading={performanceLoading}
        failed={Boolean(performanceError)}
        onRetry={retryPerformance}
        retrying={performanceValidating}
      />
    </PageWrapper>
  );
}
