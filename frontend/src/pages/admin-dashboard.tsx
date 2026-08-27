import { useTranslation } from "react-i18next";
import {
  Activity,
  AlertTriangle,
  CopyPlus,
  Network,
  RefreshCw,
} from "lucide-react";
import { useAuth } from "@/hooks/use-auth";
import { useAdminOverview } from "@/lib/swr";
import { Button } from "@/components/ui/button";
import { EmptyState } from "@/components/ui/empty-state";
import { PageHeader } from "@/components/ui/page-header";
import { PageWrapper, motion, transitions } from "@/components/ui/motion";
import { CardsPageSkeleton } from "@/components/ui/page-skeleton";
import { Separator } from "@/components/ui/separator";
import { cn } from "@/lib/utils";
import {
  ChannelHealthCard,
  ReplicaStatusCard,
  SystemStatusCard,
  UsageRankingCard,
} from "@/pages/admin-dashboard/dashboard-sections";

function formatNumber(value: number): string {
  return value.toLocaleString("en-US");
}

export function AdminDashboardPage() {
  const { t } = useTranslation();
  const { user } = useAuth();
  const isAdmin = user?.role === "super_admin" || user?.role === "admin";
  const { data, error, isLoading, mutate } = useAdminOverview({
    isPaused: () => !isAdmin,
  });

  const tt = (key: string, fallback?: string): string => {
    const translated = t(key, { defaultValue: fallback ?? key } as never);
    return typeof translated === "string" ? translated : (fallback ?? key);
  };

  if (!isAdmin) {
    return (
      <PageWrapper className="h-full min-h-0 overflow-hidden">
        <EmptyState
          title={tt(
            "adminDashboard.unauthorized",
            "Administrator access required",
          )}
          description={tt(
            "adminDashboard.unauthorizedDescription",
            "This page is only available to administrators.",
          )}
          className="h-full py-0"
        />
      </PageWrapper>
    );
  }

  if (isLoading && !data) {
    return (
      <PageWrapper className="h-full min-h-0 overflow-hidden space-y-4">
        <CardsPageSkeleton />
      </PageWrapper>
    );
  }

  if (error && !data) {
    return (
      <PageWrapper className="h-full min-h-0 overflow-hidden">
        <EmptyState
          variant="card"
          icon={<AlertTriangle className="h-8 w-8 text-destructive" />}
          title={tt(
            "adminDashboard.loadFailed",
            "Failed to load system overview",
          )}
          description={
            <span className="font-mono text-xs break-all">
              {error instanceof Error
                ? error.message
                : tt("common.error", "Error")}
            </span>
          }
          className="h-full py-0"
        />
        <div className="mt-3 flex justify-center">
          <Button variant="outline" onClick={() => void mutate()}>
            <RefreshCw data-icon />
            {tt("adminDashboard.retry", "Retry")}
          </Button>
        </div>
      </PageWrapper>
    );
  }

  if (!data) return null;

  return (
    <PageWrapper className="flex h-full min-h-0 flex-col gap-4 overflow-hidden">
      <motion.header
        initial={{ opacity: 0, y: -12 }}
        animate={{ opacity: 1, y: 0 }}
        transition={transitions.normal}
        className="shrink-0"
      >
        <PageHeader
          title={tt("adminDashboard.title", "System Dashboard")}
          description={tt(
            "adminDashboard.subtitle",
            "System status, user usage ranking, model/channel health and replica status",
          )}
        />
      </motion.header>

      <main className="min-h-0 flex-1 overflow-y-auto pr-1">
        <div className="grid grid-cols-1 gap-4 lg:grid-cols-12">
          <div className="contents lg:col-span-5 lg:flex lg:flex-col lg:gap-4">
            <SystemStatusCard data={data} t={tt} />
            <ReplicaStatusCard data={data} t={tt} />
          </div>
          <div className="contents lg:col-span-7 lg:flex lg:flex-col lg:gap-4">
            <UsageRankingCard data={data} t={tt} />
            <ChannelHealthCard data={data} t={tt} />
          </div>
        </div>
      </main>

      <footer className="flex shrink-0 flex-col gap-3">
        <Separator />
        <div className="flex flex-wrap items-center gap-x-3 gap-y-2 text-sm text-muted-foreground">
          <Activity className="size-4" aria-hidden="true" />
          <span>
            {tt("adminDashboard.healthEntries", "Tracked health entries")}:{" "}
            {formatNumber(data.system.channel_health_entries)}
          </span>
          <span aria-hidden="true">·</span>
          <span>
            {tt("adminDashboard.affinityEntries", "Affinity bindings")}:{" "}
            {formatNumber(data.system.channel_affinity_entries)}
          </span>
          <Button variant="ghost" size="sm" onClick={() => void mutate()}>
            <CopyPlus data-icon="inline-start" />
            {tt("adminDashboard.refresh", "Refresh")}
          </Button>
          <span className={cn("ml-auto hidden items-center gap-2 sm:flex")}>
            <Network className="size-4" aria-hidden="true" />
            {tt("adminDashboard.autoRefresh", "Auto refresh 10s")}
          </span>
        </div>
      </footer>
    </PageWrapper>
  );
}
