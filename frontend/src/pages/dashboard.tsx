import { useCallback, useMemo, useState } from "react";
import { useTranslation } from "react-i18next";
import { useAuth } from "@/hooks/use-auth";
import {
  useApiKeys,
  useDashboardAnalytics,
  useDashboardPerformance,
  usePublicSettings,
  useRechargeChannels,
} from "@/lib/swr";
import {
  readStoredUsageWindow,
  storeUsageWindow,
  USAGE_WINDOW_QUERY,
  type UsageWindow,
} from "@/lib/usage-window";
import { PanelErrorBoundary } from "@/components/ui/error-boundary";
import { PageWrapper, motion, transitions } from "@/components/ui/motion";
import { PageHeader } from "@/components/ui/page-header";
import { Skeleton } from "@/components/ui/skeleton";
import { AccountStrip } from "./dashboard/account-strip";
import { GettingStartedCard } from "./dashboard/getting-started";
import { UsageChartPanel } from "./dashboard/usage-chart";
import { RecentUsagePanel } from "./dashboard/recent-usage";
import { ApiInfoPanel } from "./dashboard/api-info-panel";
import { PerformancePanel } from "./dashboard/performance-panel";
import { buildUsageSeries } from "./dashboard/utils";

const ACCOUNT_OVERVIEW_QUERY = USAGE_WINDOW_QUERY["24h"];
const USAGE_REFRESH_INTERVAL_MS = 60000;

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
  const { user, refreshUser } = useAuth();

  const [usageWindow, setUsageWindowState] = useState<UsageWindow>(
    readStoredUsageWindow,
  );
  const setUsageWindow = useCallback((next: UsageWindow) => {
    storeUsageWindow(next);
    setUsageWindowState(next);
  }, []);

  const usageQuery = USAGE_WINDOW_QUERY[usageWindow];
  // keepPreviousData: a window switch keeps showing the previous payload until
  // the new one resolves (DH-12b) instead of flashing a skeleton.
  const {
    data: usageAnalytics,
    error: usageError,
    isLoading: usageLoading,
    isValidating: usageValidating,
    mutate: retryUsage,
  } = useDashboardAnalytics(usageQuery.buckets, usageQuery.rangeHours, {
    keepPreviousData: true,
    refreshInterval: USAGE_REFRESH_INTERVAL_MS,
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
    { refreshInterval: USAGE_REFRESH_INTERVAL_MS },
  );
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
  const { data: apiKeys } = useApiKeys();
  const { data: rechargeChannels } = useRechargeChannels();

  const series = useMemo(
    () => buildUsageSeries(usageAnalytics, usageWindow),
    [usageAnalytics, usageWindow],
  );

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

  const refreshUsage = useCallback(() => {
    void retryUsage();
    void retryAccount();
    void refreshUser();
  }, [retryUsage, retryAccount, refreshUser]);

  const userLoading = !user;
  const canTopUp =
    Boolean(user) &&
    !user?.balance_unlimited &&
    (rechargeChannels?.length ?? 0) > 0;
  const hasAnyRequest =
    usageAnalytics?.has_any_request ?? accountAnalytics?.has_any_request;
  const showGettingStarted =
    apiKeys?.length === 0 || hasAnyRequest === false;

  return (
    <PageWrapper className="flex min-h-0 min-w-0 flex-col gap-4 pb-6">
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
            title={tt("dashboard.greeting", "Hello, {{username}}", {
              username: user?.username ?? "User",
            })}
            description={tt(
              "dashboard.subtitle",
              "Account, usage, API, and platform performance at a glance",
            )}
          />
        )}
      </motion.header>

      {showGettingStarted && user ? (
        <PanelErrorBoundary>
          <GettingStartedCard
            user={user}
            settings={publicSettings}
            hasApiKey={(apiKeys?.length ?? 0) > 0}
            canTopUp={canTopUp}
          />
        </PanelErrorBoundary>
      ) : null}

      <PanelErrorBoundary>
        <AccountStrip
          user={user}
          analytics={accountAnalytics}
          canTopUp={canTopUp}
          failed={Boolean(accountError)}
          onRetry={retryAccount}
          retrying={accountValidating}
          loading={
            userLoading ||
            (accountAnalyticsLoading && accountAnalytics === undefined)
          }
        />
      </PanelErrorBoundary>

      <PanelErrorBoundary>
        <ApiInfoPanel
          settings={publicSettings}
          loading={publicSettingsLoading}
          failed={Boolean(settingsError)}
          onRetry={retrySettings}
          retrying={settingsValidating}
        />
      </PanelErrorBoundary>

      <PanelErrorBoundary>
        <UsageChartPanel
          analytics={usageAnalytics}
          series={series}
          failed={Boolean(usageError)}
          onRetry={retryUsage}
          retrying={usageValidating}
          loading={usageLoading && !usageAnalytics}
          pending={usageLoading && usageAnalytics !== undefined}
          window={usageWindow}
          onWindowChange={setUsageWindow}
          onRefresh={refreshUsage}
        />
      </PanelErrorBoundary>

      <PanelErrorBoundary>
        <RecentUsagePanel
          analytics={usageAnalytics}
          colors={series.colors}
          username={user?.username}
          isAdmin={user?.role === "admin" || user?.role === "super_admin"}
          failed={Boolean(usageError)}
          onRetry={retryUsage}
          retrying={usageValidating}
          loading={usageLoading && !usageAnalytics}
          pending={usageLoading && usageAnalytics !== undefined}
          window={usageWindow}
          onShowThirtyDays={() => setUsageWindow("30d")}
        />
      </PanelErrorBoundary>

      <PanelErrorBoundary>
        <PerformancePanel
          data={performance}
          loading={performanceLoading}
          failed={Boolean(performanceError)}
          onRetry={retryPerformance}
          retrying={performanceValidating}
        />
      </PanelErrorBoundary>
    </PageWrapper>
  );
}
