import { useTranslation } from "react-i18next";
import { Card, CardContent, CardHeader, CardTitle } from "@/components/ui/card";
import { Skeleton } from "@/components/ui/skeleton";
import { EmptyState } from "@/components/ui/empty-state";
import {
  Tooltip,
  TooltipContent,
  TooltipTrigger,
} from "@/components/ui/tooltip";
import { motion, transitions } from "@/components/ui/motion";
import type {
  DashboardPerformance,
  DashboardPerformanceBrick,
  DashboardPerformanceBrickStatus,
} from "@/lib/api";
import { cn } from "@/lib/utils";

interface PerformancePanelProps {
  data: DashboardPerformance | undefined;
  loading?: boolean;
}

function brickClass(status: DashboardPerformanceBrickStatus): string {
  switch (status) {
    case "up":
      return "bg-success";
    case "degraded":
      return "bg-warning";
    case "down":
      return "bg-destructive";
    case "empty":
    default:
      return "bg-muted";
  }
}

function formatTtft(ms: number | null | undefined): string {
  if (ms == null || !Number.isFinite(ms)) return "—";
  const rounded = Math.round(ms * 10) / 10;
  return `${rounded.toFixed(1).replace(/\.0$/, "")} ms`;
}

function formatTps(tps: number | null | undefined): string {
  if (tps == null || !Number.isFinite(tps)) return "—";
  return `${tps.toFixed(2)} t/s`;
}

function UptimeBricks({
  bricks,
  label,
}: {
  bricks: DashboardPerformanceBrick[];
  label: string;
}) {
  const { t } = useTranslation();
  const ordered = [...bricks].sort((a, b) => a.index - b.index);

  return (
    <div
      className="flex min-w-0 flex-1 items-center gap-0.5"
      role="img"
      aria-label={t("dashboard.performance.uptimeAria", "Uptime for {{name}}", {
        name: label,
      })}
    >
      {ordered.map((brick) => (
        <Tooltip key={brick.index}>
          <TooltipTrigger asChild>
            <span
              className={cn(
                "h-3 min-w-[6px] flex-1 rounded-[2px]",
                brickClass(brick.status)
              )}
            />
          </TooltipTrigger>
          <TooltipContent side="top" className="text-xs">
            {t(`dashboard.performance.status.${brick.status}`, brick.status)} · h
            {brick.index + 1}
          </TooltipContent>
        </Tooltip>
      ))}
    </div>
  );
}

function PerfRow({
  name,
  bricks,
  avgTtft,
  avgTps,
  index,
}: {
  name: string;
  bricks: DashboardPerformanceBrick[];
  avgTtft: number | null;
  avgTps: number | null;
  index: number;
}) {
  const { t } = useTranslation();
  return (
    <motion.div
      initial={{ opacity: 0, y: 10 }}
      animate={{ opacity: 1, y: 0 }}
      transition={{ delay: 0.04 * index, ...transitions.normal }}
      className="grid gap-2 rounded-lg border bg-muted/15 px-3 py-2.5 sm:grid-cols-[minmax(0,1.2fr)_minmax(0,2fr)_auto_auto] sm:items-center"
    >
      <p className="truncate font-mono text-xs font-medium" title={name}>
        {name}
      </p>
      <UptimeBricks bricks={bricks} label={name} />
      <div className="flex items-baseline justify-between gap-4 sm:block sm:text-right">
        <span className="text-[10px] uppercase tracking-wide text-muted-foreground sm:hidden">
          {t("dashboard.performance.avgTtft", "avgTTFT")}
        </span>
        <span className="font-mono text-xs tabular-nums">{formatTtft(avgTtft)}</span>
      </div>
      <div className="flex items-baseline justify-between gap-4 sm:block sm:text-right">
        <span className="text-[10px] uppercase tracking-wide text-muted-foreground sm:hidden">
          {t("dashboard.performance.avgTps", "avgTPS")}
        </span>
        <span className="font-mono text-xs tabular-nums">{formatTps(avgTps)}</span>
      </div>
    </motion.div>
  );
}

export function PerformancePanel({ data, loading }: PerformancePanelProps) {
  const { t } = useTranslation();
  const groups = data?.groups ?? [];
  const models = data?.models ?? [];
  const empty = !loading && groups.length === 0 && models.length === 0;

  return (
    <motion.div
      initial={{ opacity: 0, y: 18 }}
      animate={{ opacity: 1, y: 0 }}
      transition={{ delay: 0.26, ...transitions.normal }}
    >
      <Card>
        <CardHeader className="p-4 pb-2">
          <div className="flex flex-wrap items-end justify-between gap-2">
            <div>
              <CardTitle className="text-base font-semibold leading-none tracking-tight">
                {t("dashboard.performance.title", "Performance")}
              </CardTitle>
              <p className="pt-1 text-xs text-muted-foreground">
                {t(
                  "dashboard.performance.subtitle",
                  "Last 24 hours · uptime bricks, avgTTFT, and avgTPS"
                )}
              </p>
            </div>
            <div className="hidden gap-4 text-[10px] uppercase tracking-wide text-muted-foreground sm:flex">
              <span className="w-16 text-right">
                {t("dashboard.performance.avgTtft", "avgTTFT")}
              </span>
              <span className="w-16 text-right">
                {t("dashboard.performance.avgTps", "avgTPS")}
              </span>
            </div>
          </div>
        </CardHeader>
        <CardContent className="space-y-2 p-4 pt-2">
          {loading ? (
            <div className="space-y-2">
              {Array.from({ length: 3 }).map((_, i) => (
                <Skeleton key={i} className="h-12 w-full" />
              ))}
            </div>
          ) : empty ? (
            <EmptyState
              title={t(
                "dashboard.performance.empty",
                "Performance targets are not configured"
              )}
              description={t(
                "dashboard.performance.emptyDescription",
                "An administrator can select groups and models in system settings."
              )}
              className="py-8"
            />
          ) : (
            <div className="space-y-2">
              {groups.map((group, index) => (
                <PerfRow
                  key={`group-${group.id}`}
                  name={group.name || group.id}
                  bricks={group.bricks}
                  avgTtft={group.avg_ttft_ms}
                  avgTps={group.avg_tps}
                  index={index}
                />
              ))}
              {models.map((model, index) => (
                <PerfRow
                  key={`model-${model.id}`}
                  name={model.id}
                  bricks={model.bricks}
                  avgTtft={model.avg_ttft_ms}
                  avgTps={model.avg_tps}
                  index={groups.length + index}
                />
              ))}
            </div>
          )}
        </CardContent>
      </Card>
    </motion.div>
  );
}
