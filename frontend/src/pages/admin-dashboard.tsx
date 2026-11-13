import { useState } from "react";
import { useTranslation } from "react-i18next";
import { RefreshCw } from "lucide-react";
import { useSearchParamState } from "@/hooks/use-search-param-state";
import { useAdminOverview } from "@/lib/swr";
import { DEFAULT_SPEND_WINDOW, SPEND_WINDOWS, type SpendWindow } from "@/lib/spend-window";
import { cn } from "@/lib/utils";
import { Button } from "@/components/ui/button";
import { Card } from "@/components/ui/card";
import { PageHeader } from "@/components/ui/page-header";
import { PageWrapper } from "@/components/ui/motion";
import { PageHeaderSkeleton } from "@/components/ui/page-skeleton";
import { QueryError } from "@/components/ui/query-error";
import { Skeleton } from "@/components/ui/skeleton";
import {
  ChannelHealthCard,
  ReplicaStatusCard,
  SystemStatusCard,
  UsageRankingCard,
} from "@/pages/admin-dashboard/dashboard-sections";

function SectionSkeleton({ rows, className }: { rows: number; className?: string }) {
  return (
    <Card className={className}>
      <div className="flex flex-col gap-2 p-5">
        <Skeleton className="h-5 w-40" />
        <Skeleton className="h-4 w-64 max-w-full" />
      </div>
      <div className="flex flex-col gap-3 border-t p-5">
        {Array.from({ length: rows }, (_, index) => (
          <Skeleton key={index} className="h-8 w-full" />
        ))}
      </div>
    </Card>
  );
}

export function AdminDashboardPage() {
  const { t } = useTranslation();
  const [spendWindow, setSpendWindow] = useSearchParamState<SpendWindow>(
    "window",
    DEFAULT_SPEND_WINDOW,
    SPEND_WINDOWS
  );
  const [refreshing, setRefreshing] = useState(false);
  const { data, error, isLoading, isValidating, mutate } = useAdminOverview(spendWindow);

  if (isLoading && !data) {
    return (
      <PageWrapper>
        <div className="space-y-6" aria-busy="true">
          <PageHeaderSkeleton actions={1} />
          <SectionSkeleton rows={5} />
          <div className="grid grid-cols-1 gap-6 lg:grid-cols-12 lg:items-start">
            <SectionSkeleton rows={4} className="lg:col-span-7" />
            <SectionSkeleton rows={6} className="lg:col-span-5" />
          </div>
        </div>
      </PageWrapper>
    );
  }

  // The 10-second poll also sets isValidating, so the button tracks only its own request.
  const refresh = async () => {
    setRefreshing(true);
    try {
      await mutate();
    } finally {
      setRefreshing(false);
    }
  };

  const pending = data?.spend?.window !== spendWindow;

  return (
    <PageWrapper className="space-y-6">
      <PageHeader
        title={t("adminDashboard.title")}
        description={t("adminDashboard.subtitle")}
        actions={
          <>
            <span className="text-sm text-muted-foreground">{t("adminDashboard.autoRefresh")}</span>
            <Button variant="outline" disabled={refreshing} onClick={() => void refresh()}>
              <RefreshCw className={cn(refreshing && "motion-safe:animate-spin")} aria-hidden="true" />
              {t("adminDashboard.refresh")}
            </Button>
          </>
        }
      />

      {error ? <QueryError onRetry={mutate} retrying={isValidating} stale={data !== undefined} /> : null}

      {data ? (
        <>
          <ChannelHealthCard
            data={data}
            spendWindow={spendWindow}
            onSpendWindowChange={setSpendWindow}
            pending={pending}
          />
          <div className="grid grid-cols-1 gap-6 lg:grid-cols-12 lg:items-start">
            <UsageRankingCard data={data} pending={pending} className="lg:col-span-7" />
            <div className="flex flex-col gap-6 lg:col-span-5">
              <SystemStatusCard data={data} />
              <ReplicaStatusCard data={data} />
            </div>
          </div>
        </>
      ) : null}
    </PageWrapper>
  );
}
