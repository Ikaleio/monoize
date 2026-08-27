import * as React from "react";
import { HardDrive, HeartPulse, Server, Users } from "lucide-react";
import {
  TableVirtuoso,
  type ScrollerProps,
  type TableComponents,
} from "react-virtuoso";
import type {
  AdminOverview,
  AdminOverviewChannelHealth,
  AdminOverviewUserRanking,
} from "@/lib/api";
import { formatNanoUsd } from "@/lib/exact-decimal";
import { cn } from "@/lib/utils";
import { Badge } from "@/components/ui/badge";
import {
  Card,
  CardContent,
  CardDescription,
  CardHeader,
  CardTitle,
} from "@/components/ui/card";
import { EmptyState } from "@/components/ui/empty-state";
import {
  TableBody,
  TableCell,
  TableHead,
  TableHeader,
  TableRow,
} from "@/components/ui/table";

type Translate = (key: string, fallback?: string) => string;

const RANKING_HEADER_HEIGHT = 40;
const RANKING_ROW_HEIGHT = 44;
const RANKING_TABLE_MAX_HEIGHT = 260;
const HEALTH_HEADER_HEIGHT = 40;
const HEALTH_ROW_HEIGHT = 64;
const HEALTH_TABLE_MAX_HEIGHT = 360;

function formatNumber(value: number): string {
  return value.toLocaleString("en-US");
}

function humanizeUptime(seconds: number): string {
  if (!Number.isFinite(seconds) || seconds <= 0) return "-";
  const days = Math.floor(seconds / 86400);
  const hours = Math.floor((seconds % 86400) / 3600);
  const minutes = Math.floor((seconds % 3600) / 60);
  if (days > 0) return `${days}d ${hours}h ${minutes}m`;
  if (hours > 0) return `${hours}h ${minutes}m`;
  return `${minutes}m`;
}

function formatBytes(bytes: number): string {
  if (!Number.isFinite(bytes) || bytes <= 0) return "0 B";
  const units = ["B", "KB", "MB", "GB"];
  let value = bytes;
  let unit = 0;
  while (value >= 1024 && unit < units.length - 1) {
    value /= 1024;
    unit += 1;
  }
  return `${value.toFixed(1)} ${units[unit]}`;
}

function formatTimestamp(unixMs: number | null | undefined): string {
  if (unixMs == null) return "-";
  const date = new Date(unixMs);
  if (Number.isNaN(date.getTime())) return "-";
  return `${date.getFullYear()}-${String(date.getMonth() + 1).padStart(2, "0")}-${String(
    date.getDate(),
  ).padStart(2, "0")} ${String(date.getHours()).padStart(2, "0")}:${String(
    date.getMinutes(),
  ).padStart(2, "0")}:${String(date.getSeconds()).padStart(2, "0")}`;
}

function MetricCell({
  label,
  value,
  className,
}: {
  label: string;
  value: React.ReactNode;
  className?: string;
}) {
  return (
    <div className={cn("min-w-0 border-l-2 border-border pl-3", className)}>
      <dt className="text-sm leading-5 text-muted-foreground">{label}</dt>
      <dd className="mt-1 min-w-0 break-words font-mono text-sm leading-5 text-foreground">
        {value}
      </dd>
    </div>
  );
}

function healthBadge(channel: AdminOverviewChannelHealth, t: Translate) {
  if ((channel.cooldown_until ?? 0) > Date.now()) {
    return <Badge variant="secondary">{t("adminDashboard.coolingDown")}</Badge>;
  }
  if (!channel.enabled) {
    return <Badge variant="secondary">{t("adminDashboard.disabled")}</Badge>;
  }
  return channel.healthy ? (
    <Badge
      variant="secondary"
      className="border-success bg-success/10 text-success"
    >
      {t("adminDashboard.healthy")}
    </Badge>
  ) : (
    <Badge variant="destructive">{t("adminDashboard.unhealthy")}</Badge>
  );
}

const VirtualTableScroller = React.forwardRef<HTMLDivElement, ScrollerProps>(
  ({ children, style, ...props }, ref) => (
    <div
      {...props}
      ref={ref}
      style={style}
      className="overflow-auto overscroll-contain [scrollbar-gutter:stable]"
    >
      {children}
    </div>
  ),
);
VirtualTableScroller.displayName = "VirtualTableScroller";

const VirtualTableHead = React.forwardRef<
  HTMLTableSectionElement,
  React.ComponentPropsWithoutRef<"thead">
>(({ children, className, ...props }, ref) => (
  <TableHeader ref={ref} className={cn("bg-card", className)} {...props}>
    {children}
  </TableHeader>
));
VirtualTableHead.displayName = "VirtualTableHead";

const VirtualTableBody = React.forwardRef<
  HTMLTableSectionElement,
  React.ComponentPropsWithoutRef<"tbody">
>(({ children, className, ...props }, ref) => (
  <TableBody ref={ref} className={className} {...props}>
    {children}
  </TableBody>
));
VirtualTableBody.displayName = "VirtualTableBody";

const rankingTableComponents: TableComponents<AdminOverviewUserRanking> = {
  Scroller: VirtualTableScroller,
  Table: ({ children, style }) => (
    <table
      style={style}
      className="w-full min-w-[30rem] table-fixed border-separate border-spacing-0 text-sm"
    >
      {children}
    </table>
  ),
  TableHead: VirtualTableHead,
  TableBody: VirtualTableBody,
};

const healthTableComponents: TableComponents<AdminOverviewChannelHealth> = {
  Scroller: VirtualTableScroller,
  Table: ({ children, style }) => (
    <table
      style={style}
      className="w-full min-w-[48rem] table-fixed border-separate border-spacing-0 text-sm"
    >
      {children}
    </table>
  ),
  TableHead: VirtualTableHead,
  TableBody: VirtualTableBody,
};

export function SystemStatusCard({
  data,
  t,
}: {
  data: AdminOverview;
  t: Translate;
}) {
  return (
    <Card className="order-1 h-fit overflow-hidden">
      <CardHeader className="p-5 pb-4">
        <CardTitle className="flex items-center gap-2 text-base">
          <Server className="size-4 text-primary" aria-hidden="true" />
          {t("adminDashboard.systemStatus", "System Status")}
        </CardTitle>
        <CardDescription className="font-mono text-sm">
          {data.node.role} · v{data.node.version} ·{" "}
          {humanizeUptime(data.node.uptime_seconds)}
        </CardDescription>
      </CardHeader>
      <CardContent className="p-5 pt-0">
        <dl className="grid grid-cols-2 gap-x-4 gap-y-4 sm:grid-cols-3 lg:grid-cols-2 xl:grid-cols-3">
          <MetricCell
            label={t("adminDashboard.nodeRole", "Node role")}
            value={
              <Badge variant="secondary" className="font-mono">
                {data.node.role}
              </Badge>
            }
          />
          <MetricCell
            label={t("adminDashboard.version", "Version")}
            value={data.node.version}
          />
          <MetricCell
            label={t("adminDashboard.uptime", "Uptime")}
            value={humanizeUptime(data.node.uptime_seconds)}
          />
          <MetricCell
            label={t("adminDashboard.startedAt", "Started at")}
            value={formatTimestamp(Date.parse(data.node.started_at))}
          />
          <MetricCell
            label={t("adminDashboard.listen", "Listen")}
            value={data.node.listen}
          />
          <MetricCell
            label={t("adminDashboard.metricsPath", "Metrics path")}
            value={data.node.metrics_path}
          />
          <MetricCell
            label={t("adminDashboard.database", "Database")}
            value={`${data.node.database_backend} · ${data.node.database_dsn_redacted}`}
            className="col-span-2 sm:col-span-3 lg:col-span-2 xl:col-span-3"
          />
          <MetricCell
            label={t("adminDashboard.upstreamProxy", "Egress proxy")}
            value={data.node.upstream_proxy_url || "-"}
          />
          <MetricCell
            label={t(
              "adminDashboard.pendingRequestLogs",
              "Pending request logs",
            )}
            value={formatNumber(data.system.pending_request_logs)}
          />
          <MetricCell
            label={t("adminDashboard.sseConnections", "SSE connections")}
            value={formatNumber(data.system.sse_connections)}
          />
          <MetricCell
            label={t("adminDashboard.routingRevision", "Routing revision")}
            value={data.system.routing_config_revision}
          />
        </dl>
      </CardContent>
    </Card>
  );
}

export function ReplicaStatusCard({
  data,
  t,
}: {
  data: AdminOverview;
  t: Translate;
}) {
  const replicas = data.replica.replicas ?? [];

  return (
    <Card className="order-4 h-fit overflow-hidden">
      <CardHeader className="p-5 pb-4">
        <CardTitle className="flex items-center gap-2 text-base">
          <HardDrive className="size-4 text-primary" aria-hidden="true" />
          {t("adminDashboard.replicaStatus", "Replica Status")}
        </CardTitle>
        <CardDescription className="font-mono text-sm">
          {data.node.role} · {formatNumber(replicas.length)}
        </CardDescription>
      </CardHeader>
      <CardContent className="p-5 pt-0">
        {data.node.role === "replica" ? (
          <dl className="grid grid-cols-2 gap-4">
            <MetricCell
              label={t(
                "adminDashboard.spoolPendingCount",
                "Spool pending files",
              )}
              value={formatNumber(data.replica.spool_pending_count)}
            />
            <MetricCell
              label={t(
                "adminDashboard.spoolPendingBytes",
                "Spool pending bytes",
              )}
              value={formatBytes(data.replica.spool_pending_bytes)}
            />
          </dl>
        ) : (
          <div className="flex flex-col gap-3">
            <div className="flex items-center justify-between gap-3">
              <span className="text-sm text-muted-foreground">
                {t("adminDashboard.ingestEnabled", "Replica ingest")}
              </span>
              {data.replica.ingest_enabled ? (
                <Badge
                  variant="secondary"
                  className="border-success bg-success/10 text-success"
                >
                  {t("adminDashboard.enabled", "Enabled")}
                </Badge>
              ) : (
                <Badge variant="secondary">
                  {t("adminDashboard.disabled", "Disabled")}
                </Badge>
              )}
            </div>

            <dl className="grid grid-cols-2 gap-3">
              <MetricCell
                label={t(
                  "adminDashboard.spoolPendingCount",
                  "Spool pending files",
                )}
                value={formatNumber(data.replica.spool_pending_count)}
              />
              <MetricCell
                label={t(
                  "adminDashboard.spoolPendingBytes",
                  "Spool pending bytes",
                )}
                value={formatBytes(data.replica.spool_pending_bytes)}
              />
            </dl>

            {!data.replica.ingest_enabled && (
              <p className="text-sm leading-6 text-muted-foreground">
                {t(
                  "adminDashboard.noReplicaToken",
                  "No replica token configured; there is nothing to monitor.",
                )}
              </p>
            )}
            {data.replica.ingest_enabled && replicas.length === 0 && (
              <p className="text-sm leading-6 text-muted-foreground">
                {t(
                  "adminDashboard.noReplicasYet",
                  "No replica has heartbeated yet.",
                )}
              </p>
            )}

            {replicas.length > 0 && (
              <div className="max-h-80 divide-y overflow-y-auto overscroll-contain">
                {replicas.map((replica) => (
                  <section
                    key={replica.id}
                    className="flex flex-col gap-3 py-3 first:pt-0 last:pb-0"
                  >
                    <div className="flex items-center justify-between gap-3">
                      <span className="min-w-0 truncate font-mono text-sm">
                        {replica.hostname || replica.id} · {replica.listen}
                      </span>
                      <Badge variant={replica.stale ? "secondary" : "outline"}>
                        {replica.stale
                          ? t("adminDashboard.stale", "Stale")
                          : t("adminDashboard.live", "Live")}
                      </Badge>
                    </div>
                    <dl className="grid grid-cols-2 gap-3">
                      <MetricCell
                        label={t("adminDashboard.version", "Version")}
                        value={replica.version}
                      />
                      <MetricCell
                        label={t("adminDashboard.uptime", "Uptime")}
                        value={humanizeUptime(replica.uptime_seconds)}
                      />
                      <MetricCell
                        label={t("adminDashboard.lastSeen", "Last seen")}
                        value={formatTimestamp(
                          Date.parse(replica.last_seen_at),
                        )}
                        className="col-span-2"
                      />
                      <MetricCell
                        label={t(
                          "adminDashboard.spoolPendingCount",
                          "Spool pending files",
                        )}
                        value={formatNumber(replica.spool_pending_count)}
                      />
                      <MetricCell
                        label={t(
                          "adminDashboard.spoolPendingBytes",
                          "Spool pending bytes",
                        )}
                        value={formatBytes(replica.spool_pending_bytes)}
                      />
                    </dl>
                  </section>
                ))}
              </div>
            )}
          </div>
        )}
      </CardContent>
    </Card>
  );
}

export function UsageRankingCard({
  data,
  t,
}: {
  data: AdminOverview;
  t: Translate;
}) {
  const rows = data.users_ranking;
  const tableHeight = Math.min(
    RANKING_HEADER_HEIGHT + rows.length * RANKING_ROW_HEIGHT,
    RANKING_TABLE_MAX_HEIGHT,
  );

  return (
    <Card className="order-2 h-fit overflow-hidden">
      <CardHeader className="p-5 pb-4">
        <CardTitle className="flex items-center justify-between gap-3 text-base">
          <span className="flex items-center gap-2">
            <Users className="size-4 text-primary" aria-hidden="true" />
            {t("adminDashboard.usageRanking", "User Usage Ranking (24h)")}
          </span>
          <Badge variant="outline" className="font-mono">
            {formatNumber(rows.length)} / 20
          </Badge>
        </CardTitle>
        <CardDescription>24h</CardDescription>
      </CardHeader>
      <CardContent className="p-0">
        {rows.length === 0 ? (
          <EmptyState
            title={t("adminDashboard.noUsage", "No usage in the last 24 hours")}
            className="px-5 py-6"
          />
        ) : (
          <TableVirtuoso
            aria-label={t(
              "adminDashboard.usageRanking",
              "User Usage Ranking (24h)",
            )}
            components={rankingTableComponents}
            computeItemKey={(_, row) => row.user_id}
            data={rows}
            fixedHeaderContent={() => (
              <TableRow className="h-10 bg-card hover:bg-card">
                <TableHead className="h-10 w-14 px-5">#</TableHead>
                <TableHead className="h-10 px-2">
                  {t("adminDashboard.username", "User")}
                </TableHead>
                <TableHead className="h-10 w-28 px-2 text-right">
                  {t("adminDashboard.calls", "Calls")}
                </TableHead>
                <TableHead className="h-10 w-36 px-5 text-right">
                  {t("adminDashboard.cost", "Cost")}
                </TableHead>
              </TableRow>
            )}
            fixedItemHeight={RANKING_ROW_HEIGHT}
            itemContent={(index, row) => (
              <>
                <TableCell className="h-11 border-b px-5 py-0 font-mono text-muted-foreground">
                  {index + 1}
                </TableCell>
                <TableCell className="h-11 border-b px-2 py-0">
                  <span
                    className={cn(
                      "block truncate",
                      !row.username && "font-mono",
                    )}
                  >
                    {row.username || row.user_id}
                  </span>
                </TableCell>
                <TableCell className="h-11 border-b px-2 py-0 text-right font-mono">
                  {formatNumber(row.call_count)}
                </TableCell>
                <TableCell className="h-11 border-b px-5 py-0 text-right font-mono">
                  {formatNanoUsd(row.cost_nano_usd, 6)}
                </TableCell>
              </>
            )}
            style={{ height: tableHeight }}
          />
        )}
      </CardContent>
    </Card>
  );
}

export function ChannelHealthCard({
  data,
  t,
}: {
  data: AdminOverview;
  t: Translate;
}) {
  const rows = data.channel_health;
  const tableHeight = Math.min(
    HEALTH_HEADER_HEIGHT + rows.length * HEALTH_ROW_HEIGHT,
    HEALTH_TABLE_MAX_HEIGHT,
  );

  return (
    <Card className="order-3 h-fit overflow-hidden">
      <CardHeader className="p-5 pb-4">
        <CardTitle className="flex items-center justify-between gap-3 text-base">
          <span className="flex items-center gap-2">
            <HeartPulse className="size-4 text-primary" aria-hidden="true" />
            {t("adminDashboard.channelHealth", "Model / Channel Health")}
          </span>
          <Badge variant="outline" className="font-mono">
            {formatNumber(rows.length)}
          </Badge>
        </CardTitle>
        <CardDescription className="flex flex-wrap items-center gap-x-2 gap-y-1 font-mono text-sm">
          <span>
            {t("adminDashboard.todaySpend", "Today's spend")}:{" "}
            {formatNanoUsd(data.today?.cost_nano_usd ?? "0", 2)}
          </span>
          <span aria-hidden="true">·</span>
          <span>
            {t("adminDashboard.todayCalls", "Today's calls")}:{" "}
            {formatNumber(data.today?.calls ?? 0)}
          </span>
        </CardDescription>
      </CardHeader>
      <CardContent className="p-0">
        {rows.length === 0 ? (
          <EmptyState
            title={t("adminDashboard.noChannels", "No channels configured")}
            className="px-5 py-6"
          />
        ) : (
          <TableVirtuoso
            aria-label={t(
              "adminDashboard.channelHealth",
              "Model / Channel Health",
            )}
            components={healthTableComponents}
            computeItemKey={(_, channel) => channel.channel_id}
            data={rows}
            fixedHeaderContent={() => (
              <TableRow className="h-10 bg-card hover:bg-card">
                <TableHead className="h-10 w-52 px-5">
                  {t("adminDashboard.channel", "Channel")}
                </TableHead>
                <TableHead className="h-10 w-20 px-2">
                  {t("adminDashboard.weight", "Weight")}
                </TableHead>
                <TableHead className="h-10 w-24 px-2">
                  {t("adminDashboard.affinity", "Affinity")}
                </TableHead>
                <TableHead className="h-10 w-32 px-2">
                  {t("adminDashboard.status", "Status")}
                </TableHead>
                <TableHead className="h-10 w-32 px-2 text-right">
                  {t("adminDashboard.todaySpend", "Today")}
                </TableHead>
                <TableHead className="h-10 w-44 px-5 text-right">
                  {t("adminDashboard.lastProbe", "Last probe")}
                </TableHead>
              </TableRow>
            )}
            fixedItemHeight={HEALTH_ROW_HEIGHT}
            itemContent={(_, channel) => (
              <>
                <TableCell className="h-16 border-b px-5 py-0">
                  <span className="block truncate font-medium">
                    {channel.channel_name}
                  </span>
                  <span className="block truncate text-muted-foreground">
                    {channel.provider_name}
                  </span>
                </TableCell>
                <TableCell className="h-16 border-b px-2 py-0 font-mono">
                  {channel.weight}
                </TableCell>
                <TableCell className="h-16 border-b px-2 py-0">
                  {channel.session_affinity_auto ? (
                    <Badge variant="secondary">auto</Badge>
                  ) : (
                    <span className="text-muted-foreground">-</span>
                  )}
                </TableCell>
                <TableCell className="h-16 border-b px-2 py-0">
                  <div className="flex min-w-0 flex-col items-start gap-1">
                    {healthBadge(channel, t)}
                    {(channel.unhealthy_models ?? []).length > 0 && (
                      <span className="block max-w-full truncate font-mono text-sm text-destructive">
                        {(channel.unhealthy_models ?? []).join(", ")}
                      </span>
                    )}
                  </div>
                </TableCell>
                <TableCell className="h-16 border-b px-2 py-0 text-right font-mono">
                  <div>
                    {formatNanoUsd(channel.today_cost_nano_usd ?? "0", 2)}
                  </div>
                  <div className="text-sm text-muted-foreground">
                    {formatNumber(channel.today_calls ?? 0)}{" "}
                    {t("adminDashboard.calls", "Calls")}
                  </div>
                </TableCell>
                <TableCell className="h-16 border-b px-5 py-0 text-right font-mono text-muted-foreground">
                  {formatTimestamp(channel.last_probe_at)}
                </TableCell>
              </>
            )}
            style={{ height: tableHeight }}
          />
        )}
      </CardContent>
    </Card>
  );
}
