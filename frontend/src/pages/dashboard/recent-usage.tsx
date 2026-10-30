import { QueryError, type QueryErrorProps } from "@/components/ui/query-error";
import { useMemo } from "react";
import { useTranslation } from "react-i18next";
import { Link } from "react-router-dom";
import { Button } from "@/components/ui/button";
import { Card, CardContent, CardHeader, CardTitle } from "@/components/ui/card";
import {
  Table,
  TableBody,
  TableCell,
  TableHead,
  TableHeader,
  TableRow,
} from "@/components/ui/table";
import { Skeleton } from "@/components/ui/skeleton";
import { EmptyState } from "@/components/ui/empty-state";
import { motion, transitions } from "@/components/ui/motion";
import { cn } from "@/lib/utils";
import type { DashboardAnalytics } from "@/lib/api";
import type { UsageWindow } from "@/lib/usage-window";
import {
  aggregateModelUsage,
  formatCacheHit,
  formatCharge,
  formatCompactTokens,
  OTHER_SERIES_COLOR,
} from "./utils";

interface RecentUsagePanelProps extends QueryErrorProps {
  failed?: boolean;
  analytics: DashboardAnalytics | undefined;
  /** Model colors shared with the chart (DH-6g). */
  colors: Record<string, string>;
  username: string | undefined;
  isAdmin: boolean;
  /** First load with no data for the panel (DH-12a): show skeleton. */
  loading?: boolean;
  /** Fetching another window while previous data is shown (DH-12b): dim only. */
  pending?: boolean;
  window: UsageWindow;
  onShowThirtyDays: () => void;
}

export function RecentUsagePanel({
  analytics,
  colors,
  username,
  isAdmin,
  loading,
  pending,
  window,
  onShowThirtyDays,
  failed,
  onRetry,
  retrying,
}: RecentUsagePanelProps) {
  const { t } = useTranslation();
  const rows = useMemo(() => aggregateModelUsage(analytics), [analytics]);
  const windowName = t(`dashboard.usage.windowName.${window}`);
  const logsParams = new URLSearchParams({ range: window });
  if (isAdmin && username) logsParams.set("username", username);

  return (
    <motion.div
      initial={{ opacity: 0, y: 18 }}
      animate={{ opacity: 1, y: 0 }}
      transition={{ delay: 0.18, ...transitions.normal }}
      className="min-w-0"
    >
      <Card className="flex max-h-[28rem] min-h-0 flex-col">
        <CardHeader className="flex flex-row flex-wrap items-start justify-between gap-3 p-4 pb-2">
          <div className="flex min-w-0 flex-col gap-1">
            <CardTitle className="text-balance text-base font-semibold leading-none tracking-tight">
              {t("dashboard.recentUsage.title", "Recent Usage")}
            </CardTitle>
            <p className="text-pretty text-sm leading-relaxed text-muted-foreground">
              {t("dashboard.recentUsage.subtitle", { window: windowName })}
            </p>
          </div>
          <Button asChild variant="outline" size="sm">
            <Link to={`/dashboard/logs?${logsParams.toString()}`}>
              {t("dashboard.recentUsage.viewLogs", "View logs")}
            </Link>
          </Button>
        </CardHeader>
        <CardContent
          className={cn(
            "flex min-h-0 flex-1 flex-col overflow-hidden p-4 pt-2 transition-opacity",
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
            <div className="flex flex-col gap-2">
              {Array.from({ length: 5 }).map((_, i) => (
                <Skeleton key={i} className="h-8 w-full" />
              ))}
            </div>
          ) : rows.length === 0 ? (
            <EmptyState
              title={t("dashboard.usage.emptyWindow", { window: windowName })}
              variant="inline"
              action={
                window === "30d" ? undefined : (
                  <Button type="button" variant="outline" onClick={onShowThirtyDays}>
                    {t("dashboard.usage.viewThirtyDays")}
                  </Button>
                )
              }
              className="py-8"
            />
          ) : (
            <div className="min-h-0 flex-1 overflow-y-auto overscroll-contain rounded-md border">
              <Table>
                <TableHeader>
                  <TableRow>
                    <TableHead className="w-full">
                      {t("dashboard.recentUsage.model", "Model")}
                    </TableHead>
                    <TableHead className="whitespace-nowrap text-right">
                      {t("dashboard.recentUsage.tokens", "Tokens")}
                    </TableHead>
                    <TableHead className="hidden whitespace-nowrap text-right sm:table-cell">
                      {t("dashboard.recentUsage.cacheHit", "Cache hit")}
                    </TableHead>
                    <TableHead className="whitespace-nowrap text-right">
                      {t("dashboard.recentUsage.charge", "Charge")}
                    </TableHead>
                  </TableRow>
                </TableHeader>
                <TableBody>
                  {rows.map((row) => (
                    <TableRow key={row.model}>
                      {/* max-w-0 with w-full lets the id truncate inside an auto-layout table. */}
                      <TableCell className="w-full max-w-0">
                        <div className="flex min-w-0 items-center gap-2">
                          <span
                            className="h-2 w-2 shrink-0 rounded-sm"
                            style={{
                              backgroundColor:
                                colors[row.model] ?? OTHER_SERIES_COLOR,
                            }}
                          />
                          <span
                            className="truncate font-mono text-xs"
                            title={row.model}
                          >
                            {row.model}
                          </span>
                        </div>
                        <p className="mt-0.5 pl-4 text-xs text-muted-foreground tabular-nums sm:hidden">
                          {t("dashboard.recentUsage.cacheHit", "Cache hit")}{" "}
                          {formatCacheHit(row.cacheHitRate)}
                        </p>
                      </TableCell>
                      <TableCell className="whitespace-nowrap text-right font-mono text-xs tabular-nums">
                        {formatCompactTokens(row.tokens)}
                      </TableCell>
                      <TableCell className="hidden whitespace-nowrap text-right font-mono text-xs tabular-nums sm:table-cell">
                        {formatCacheHit(row.cacheHitRate)}
                      </TableCell>
                      <TableCell className="whitespace-nowrap text-right font-mono text-xs tabular-nums">
                        {formatCharge(row.chargeNano)}
                      </TableCell>
                    </TableRow>
                  ))}
                </TableBody>
              </Table>
            </div>
          )}
        </CardContent>
      </Card>
    </motion.div>
  );
}
