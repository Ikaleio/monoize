import { useMemo } from "react";
import { useTranslation } from "react-i18next";
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
import type { RequestLog } from "@/lib/api";
import {
  aggregateRecentUsage,
  formatCacheHit,
  formatCharge,
  formatCompactTokens,
  modelToColor,
} from "./utils";

interface RecentUsagePanelProps {
  logs: RequestLog[];
  loading?: boolean;
}

export function RecentUsagePanel({ logs, loading }: RecentUsagePanelProps) {
  const { t } = useTranslation();
  const rows = useMemo(() => aggregateRecentUsage(logs), [logs]);

  return (
    <motion.div
      initial={{ opacity: 0, y: 18 }}
      animate={{ opacity: 1, y: 0 }}
      transition={{ delay: 0.18, ...transitions.normal }}
      className="h-full min-h-0"
    >
      <Card className="flex h-full min-h-0 flex-col">
        <CardHeader className="flex flex-col gap-1 p-4 pb-2">
          <CardTitle className="text-balance text-base font-semibold leading-none tracking-tight">
            {t("dashboard.recentUsage.title", "Recent Usage")}
          </CardTitle>
          <p className="text-pretty text-xs leading-relaxed text-muted-foreground">
            {t(
              "dashboard.recentUsage.subtitle",
              "Token usage, cache hit rate, and charges by model"
            )}
          </p>
        </CardHeader>
        <CardContent className="min-h-0 flex-1 p-4 pt-2">
          {loading ? (
            <div className="flex flex-col gap-2">
              {Array.from({ length: 5 }).map((_, i) => (
                <Skeleton key={i} className="h-8 w-full" />
              ))}
            </div>
          ) : rows.length === 0 ? (
            <EmptyState
              title={t("dashboard.recentUsage.empty", "No recent usage")}
              description={t(
                "dashboard.recentUsage.emptyDescription",
                "Model usage appears here after you make API requests."
              )}
              className="py-8"
            />
          ) : (
            <div className="max-h-80 overflow-auto rounded-md border">
              <Table>
                <TableHeader>
                  <TableRow>
                    <TableHead>{t("dashboard.recentUsage.model", "Model")}</TableHead>
                    <TableHead className="text-right">
                      {t("dashboard.recentUsage.tokens", "Tokens")}
                    </TableHead>
                    <TableHead className="text-right">
                      {t("dashboard.recentUsage.cacheHit", "Cache hit")}
                    </TableHead>
                    <TableHead className="text-right">
                      {t("dashboard.recentUsage.charge", "Charge")}
                    </TableHead>
                  </TableRow>
                </TableHeader>
                <TableBody>
                  {rows.map((row) => (
                    <TableRow key={row.model}>
                      <TableCell>
                        <div className="flex min-w-0 items-center gap-2">
                          <span
                            className="h-2 w-2 shrink-0 rounded-sm"
                            style={{ backgroundColor: modelToColor(row.model) }}
                          />
                          <span className="truncate font-mono text-xs">{row.model}</span>
                        </div>
                      </TableCell>
                      <TableCell className="text-right font-mono text-xs tabular-nums">
                        {formatCompactTokens(row.tokens)}
                      </TableCell>
                      <TableCell className="text-right font-mono text-xs tabular-nums">
                        {formatCacheHit(row.cacheHitRate)}
                      </TableCell>
                      <TableCell className="text-right font-mono text-xs tabular-nums">
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
