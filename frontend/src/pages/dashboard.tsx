import { useCallback } from "react";
import { useTranslation } from "react-i18next";
import { useAuth } from "@/hooks/use-auth";
import {
  useDashboardAnalytics,
  useDashboardPerformance,
  usePublicSettings,
  useRequestLogs,
} from "@/lib/swr";
import { PageWrapper, motion, transitions } from "@/components/ui/motion";
import { PageHeader } from "@/components/ui/page-header";
import { Skeleton } from "@/components/ui/skeleton";
import { AccountStrip } from "./dashboard/account-strip";
import { UsageChartPanel } from "./dashboard/usage-chart";
import { RecentUsagePanel } from "./dashboard/recent-usage";
import { ApiInfoPanel } from "./dashboard/api-info-panel";
import { PerformancePanel } from "./dashboard/performance-panel";

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

  const { data: usageAnalytics, isLoading: usageLoading } = useDashboardAnalytics(7, 168);
  const { data: requestLogsResponse, isLoading: logsLoading } = useRequestLogs(200, 0);
  const { data: publicSettings, isLoading: publicSettingsLoading } = usePublicSettings();
  const { data: performance, isLoading: performanceLoading } = useDashboardPerformance();

  const tt = useCallback(
    (key: string, fallback: string, options?: Record<string, unknown>): string => {
      const translated = t(key, { ...(options ?? {}), defaultValue: fallback } as never);
      return typeof translated === "string" ? translated : fallback;
    },
    [t]
  );

  const logs = requestLogsResponse?.data ?? [];
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
              "Account, usage, API, and platform performance at a glance"
            )}
          />
        )}
      </motion.header>

      <AccountStrip user={user} loading={userLoading} />
      <UsageChartPanel analytics={usageAnalytics} loading={usageLoading} />

      <section className="grid min-h-0 items-stretch gap-4 lg:grid-cols-3">
        <div className="min-h-0 lg:col-span-2">
          <RecentUsagePanel logs={logs} loading={logsLoading} />
        </div>
        <div className="min-h-0">
          <ApiInfoPanel settings={publicSettings} loading={publicSettingsLoading} />
        </div>
      </section>

      <PerformancePanel data={performance} loading={performanceLoading} />
    </PageWrapper>
  );
}
