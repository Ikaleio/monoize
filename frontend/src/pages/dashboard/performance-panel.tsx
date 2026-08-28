import { useTranslation } from "react-i18next";
import { useReducedMotion } from "framer-motion";
import { Card, CardContent, CardHeader, CardTitle } from "@/components/ui/card";
import { Skeleton } from "@/components/ui/skeleton";
import { EmptyState } from "@/components/ui/empty-state";
import {
  Table,
  TableBody,
  TableCell,
  TableHead,
  TableHeader,
  TableRow,
} from "@/components/ui/table";
import {
  Tooltip,
  TooltipContent,
  TooltipProvider,
  TooltipTrigger,
} from "@/components/ui/tooltip";
import { motion, springs, transitions } from "@/components/ui/motion";
import type {
  DashboardPerformance,
  DashboardPerformanceBrick,
  DashboardPerformanceBrickStatus,
} from "@/lib/api";
import { formatPerformanceBrickRange } from "@/lib/performance-time";
import { cn } from "@/lib/utils";

interface PerformancePanelProps {
  data: DashboardPerformance | undefined;
  loading?: boolean;
}

const MotionTableRow = motion.create(TableRow);

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
  brickCount,
  timeFrom,
  timeTo,
}: {
  bricks: DashboardPerformanceBrick[];
  label: string;
  brickCount: number;
  timeFrom: string;
  timeTo: string;
}) {
  const { t, i18n } = useTranslation();
  const ordered = [...bricks].sort((a, b) => a.index - b.index);

  return (
    <TooltipProvider delayDuration={200}>
      <div
        className="flex min-w-0 flex-1 items-center gap-0.5"
        role="img"
        aria-label={t(
          "dashboard.performance.uptimeAria",
          "Uptime for {{name}}",
          {
            name: label,
          },
        )}
      >
        {ordered.map((brick) => {
          const timeRange = formatPerformanceBrickRange(
            brick.index,
            brickCount,
            timeFrom,
            timeTo,
            i18n.language,
          );
          return (
            <Tooltip key={brick.index}>
              <TooltipTrigger asChild>
                <span
                  className={cn(
                    "h-3 min-w-1.5 flex-1 rounded-sm",
                    brickClass(brick.status),
                  )}
                />
              </TooltipTrigger>
              <TooltipContent side="top" className="text-xs">
                {t(
                  `dashboard.performance.status.${brick.status}`,
                  brick.status,
                )}
                {timeRange ? ` · ${timeRange}` : null}
              </TooltipContent>
            </Tooltip>
          );
        })}
      </div>
    </TooltipProvider>
  );
}

function PerfRow({
  name,
  bricks,
  avgTtft,
  avgTps,
  index,
  brickCount,
  timeFrom,
  timeTo,
}: {
  name: string;
  bricks: DashboardPerformanceBrick[];
  avgTtft: number | null;
  avgTps: number | null;
  index: number;
  brickCount: number;
  timeFrom: string;
  timeTo: string;
}) {
  const reduceMotion = useReducedMotion();
  return (
    <MotionTableRow
      initial={reduceMotion ? { opacity: 0 } : { opacity: 0, y: 10 }}
      animate={{ opacity: 1, y: 0 }}
      transition={
        reduceMotion
          ? { duration: 0 }
          : { delay: 0.04 * index, ...springs.smooth }
      }
    >
      <TableCell className="w-64 text-left">
        <span
          className="block truncate font-mono text-xs font-medium"
          title={name}
        >
          {name}
        </span>
      </TableCell>
      <TableCell className="text-left">
        <UptimeBricks
          bricks={bricks}
          label={name}
          brickCount={brickCount}
          timeFrom={timeFrom}
          timeTo={timeTo}
        />
      </TableCell>
      <TableCell className="w-32 text-right font-mono text-xs tabular-nums">
        {formatTtft(avgTtft)}
      </TableCell>
      <TableCell className="w-32 text-right font-mono text-xs tabular-nums">
        {formatTps(avgTps)}
      </TableCell>
    </MotionTableRow>
  );
}

export function PerformancePanel({ data, loading }: PerformancePanelProps) {
  const { t } = useTranslation();
  const groups = data?.groups ?? [];
  const models = data?.models ?? [];
  const brickCount = data?.brick_count ?? 0;
  const timeFrom = data?.time_from ?? "";
  const timeTo = data?.time_to ?? "";
  const empty = !loading && groups.length === 0 && models.length === 0;

  return (
    <motion.div
      initial={{ opacity: 0, y: 18 }}
      animate={{ opacity: 1, y: 0 }}
      transition={{ delay: 0.26, ...transitions.normal }}
    >
      <Card>
        <CardHeader className="flex flex-col gap-1 p-4 pb-2">
          <CardTitle className="text-balance text-base font-semibold leading-none tracking-tight">
            {t("dashboard.performance.title", "Performance")}
          </CardTitle>
          <p className="text-pretty text-xs leading-relaxed text-muted-foreground">
            {t(
              "dashboard.performance.subtitle",
              "Last 24 hours · uptime bricks, avgTTFT, and avgTPS",
            )}
          </p>
        </CardHeader>
        <CardContent className="flex flex-col gap-2 p-4 pt-2">
          {loading ? (
            <div className="flex flex-col gap-2">
              {Array.from({ length: 3 }).map((_, i) => (
                <Skeleton key={i} className="h-12 w-full" />
              ))}
            </div>
          ) : empty ? (
            <EmptyState
              title={t(
                "dashboard.performance.empty",
                "Performance targets are not configured",
              )}
              description={t(
                "dashboard.performance.emptyDescription",
                "An administrator can select groups and models in system settings.",
              )}
              className="py-8"
            />
          ) : (
            <div className="overflow-hidden rounded-lg border">
              <Table className="min-w-[48rem] table-fixed">
                <TableHeader>
                  <TableRow className="hover:bg-transparent">
                    <TableHead className="w-64 text-left">
                      {t("dashboard.performance.target", "Target")}
                    </TableHead>
                    <TableHead className="text-left">
                      {t(
                        "dashboard.performance.availability",
                        "Availability (24h)",
                      )}
                    </TableHead>
                    <TableHead className="w-32 text-right">
                      {t("dashboard.performance.avgTtft", "avgTTFT")}
                    </TableHead>
                    <TableHead className="w-32 text-right">
                      {t("dashboard.performance.avgTps", "avgTPS")}
                    </TableHead>
                  </TableRow>
                </TableHeader>
                <TableBody>
                  {groups.map((group, index) => (
                    <PerfRow
                      key={`group-${group.id}`}
                      name={group.name || group.id}
                      bricks={group.bricks}
                      avgTtft={group.avg_ttft_ms}
                      avgTps={group.avg_tps}
                      index={index}
                      brickCount={brickCount}
                      timeFrom={timeFrom}
                      timeTo={timeTo}
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
                      brickCount={brickCount}
                      timeFrom={timeFrom}
                      timeTo={timeTo}
                    />
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
