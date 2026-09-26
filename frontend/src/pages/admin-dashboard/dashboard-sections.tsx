import { useContext, type ReactNode } from "react";
import { useTranslation } from "react-i18next";
import { Virtuoso } from "react-virtuoso";
import type {
  AdminOverview,
  AdminOverviewChannelHealth,
} from "@/lib/api";
import type { SpendWindow } from "@/lib/spend-window";
import { DashboardScrollParentContext } from "@/lib/dashboard-scroll";
import { formatNanoUsd } from "@/lib/exact-decimal";
import { cn } from "@/lib/utils";
import { SpendWindowControl } from "@/pages/admin-dashboard/spend-window-control";
import { Badge } from "@/components/ui/badge";
import {
  Card,
  CardContent,
  CardDescription,
  CardHeader,
  CardTitle,
} from "@/components/ui/card";
import {
  DataList,
  DataListCell,
  DataListHead,
  DataListHeader,
} from "@/components/ui/data-list";
import { virtualDataListComponents } from "@/components/ui/data-list-virtual";
import { EmptyState } from "@/components/ui/empty-state";
import { StatusBadge } from "@/components/ui/status";
import {
  Table,
  TableBody,
  TableCell,
  TableHead,
  TableHeader,
  TableRow,
} from "@/components/ui/table";

type ChannelStatus = "cooling" | "disabled" | "healthy" | "unhealthy";

interface KeyValueRow {
  label: string;
  value: ReactNode;
  /** Technical identifiers render in the monospace font (ADF-3). */
  mono?: boolean;
}

function formatNumber(value: number): string {
  return value.toLocaleString("en-US");
}

function humanizeUptime(seconds: number): string {
  if (!Number.isFinite(seconds) || seconds <= 0) return "—";
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
  if (unixMs == null) return "—";
  const date = new Date(unixMs);
  if (Number.isNaN(date.getTime())) return "—";
  const pad = (value: number) => String(value).padStart(2, "0");
  return `${date.getFullYear()}-${pad(date.getMonth() + 1)}-${pad(date.getDate())} ${pad(
    date.getHours(),
  )}:${pad(date.getMinutes())}:${pad(date.getSeconds())}`;
}

/** ADF-5b: cooldown wins over the enabled flag, which wins over the health flag. */
function channelStatus(channel: AdminOverviewChannelHealth): ChannelStatus {
  if (channel.cooldown_active) return "cooling";
  if (!channel.enabled) return "disabled";
  return channel.healthy ? "healthy" : "unhealthy";
}

function KeyValueRows({ rows }: { rows: KeyValueRow[] }) {
  return (
    <dl className="divide-y">
      {rows.map((row) => (
        <div key={row.label} className="flex items-baseline justify-between gap-4 px-5 py-2.5 text-sm">
          <dt className="shrink-0 text-muted-foreground">{row.label}</dt>
          <dd className={cn("min-w-0 break-all text-end", row.mono ? "font-mono" : "tabular-nums")}>
            {row.value}
          </dd>
        </div>
      ))}
    </dl>
  );
}

function SectionHeader({ title, description, className }: { title: string; description: ReactNode; className?: string }) {
  return (
    <CardHeader className={cn("p-5", className)}>
      <CardTitle>{title}</CardTitle>
      <CardDescription>{description}</CardDescription>
    </CardHeader>
  );
}

export function SystemStatusCard({ data }: { data: AdminOverview }) {
  const { t } = useTranslation();
  return (
    <Card className="overflow-clip">
      <SectionHeader title={t("adminDashboard.systemStatus")} description={t("adminDashboard.systemDescription")} />
      <CardContent className="border-t p-0">
        <KeyValueRows
          rows={[
            { label: t("adminDashboard.nodeRole"), value: data.node.role, mono: true },
            { label: t("adminDashboard.version"), value: data.node.version, mono: true },
            { label: t("adminDashboard.uptime"), value: humanizeUptime(data.node.uptime_seconds) },
            { label: t("adminDashboard.startedAt"), value: formatTimestamp(Date.parse(data.node.started_at)) },
            { label: t("adminDashboard.listen"), value: data.node.listen, mono: true },
            { label: t("adminDashboard.metricsPath"), value: data.node.metrics_path, mono: true },
            {
              label: t("adminDashboard.database"),
              value: `${data.node.database_backend} · ${data.node.database_dsn_redacted}`,
              mono: true,
            },
            { label: t("adminDashboard.upstreamProxy"), value: data.node.upstream_proxy_url || "—", mono: true },
            { label: t("adminDashboard.pendingRequestLogs"), value: formatNumber(data.system.pending_request_logs) },
            { label: t("adminDashboard.sseConnections"), value: formatNumber(data.system.sse_connections) },
            { label: t("adminDashboard.routingRevision"), value: data.system.routing_config_revision, mono: true },
            { label: t("adminDashboard.healthEntries"), value: formatNumber(data.system.channel_health_entries) },
            { label: t("adminDashboard.affinityEntries"), value: formatNumber(data.system.channel_affinity_entries) },
          ]}
        />
      </CardContent>
    </Card>
  );
}

export function ReplicaStatusCard({ data }: { data: AdminOverview }) {
  const { t } = useTranslation();
  const replicas = data.replica.replicas ?? [];
  const isReplica = data.node.role === "replica";
  const spoolRows: KeyValueRow[] = [
    { label: t("adminDashboard.spoolPendingCount"), value: formatNumber(data.replica.spool_pending_count) },
    { label: t("adminDashboard.spoolPendingBytes"), value: formatBytes(data.replica.spool_pending_bytes) },
  ];
  const note = isReplica
    ? null
    : !data.replica.ingest_enabled
      ? t("adminDashboard.noReplicaToken")
      : replicas.length === 0
        ? t("adminDashboard.noReplicasYet")
        : null;

  return (
    <Card className="overflow-clip">
      <SectionHeader title={t("adminDashboard.replicaStatus")} description={t("adminDashboard.replicaDescription")} />
      <CardContent className="border-t p-0">
        <KeyValueRows
          rows={[
            { label: t("adminDashboard.nodeRole"), value: data.node.role, mono: true },
            ...(isReplica
              ? []
              : [
                  {
                    label: t("adminDashboard.ingestEnabled"),
                    value: data.replica.ingest_enabled ? (
                      <StatusBadge variant="success">{t("adminDashboard.enabled")}</StatusBadge>
                    ) : (
                      <span className="text-muted-foreground">{t("adminDashboard.disabled")}</span>
                    ),
                  },
                ]),
            ...spoolRows,
          ]}
        />
        {note ? <p className="border-t px-5 py-3 text-sm leading-6 text-muted-foreground">{note}</p> : null}
        {replicas.map((replica) => (
          <section key={replica.id} aria-label={replica.hostname || replica.id} className="border-t">
            <div className="flex items-center justify-between gap-3 px-5 pt-3">
              <span className="min-w-0 truncate font-mono text-sm font-medium">
                {replica.hostname || replica.id} · {replica.listen}
              </span>
              <StatusBadge variant={replica.stale ? "warning" : "success"}>
                {replica.stale ? t("adminDashboard.stale") : t("adminDashboard.live")}
              </StatusBadge>
            </div>
            <KeyValueRows
              rows={[
                { label: t("adminDashboard.version"), value: replica.version, mono: true },
                { label: t("adminDashboard.uptime"), value: humanizeUptime(replica.uptime_seconds) },
                { label: t("adminDashboard.lastSeen"), value: formatTimestamp(Date.parse(replica.last_seen_at)) },
                { label: t("adminDashboard.spoolPendingCount"), value: formatNumber(replica.spool_pending_count) },
                { label: t("adminDashboard.spoolPendingBytes"), value: formatBytes(replica.spool_pending_bytes) },
              ]}
            />
          </section>
        ))}
      </CardContent>
    </Card>
  );
}

export function UsageRankingCard({ data, className }: { data: AdminOverview; className?: string }) {
  const { t } = useTranslation();
  const rows = data.users_ranking;
  const title = t("adminDashboard.usageRanking");

  return (
    <Card className={cn("overflow-clip", className)}>
      <SectionHeader title={title} description={t("adminDashboard.rankingDescription")} />
      <CardContent className="border-t p-0">
        {rows.length === 0 ? (
          <EmptyState variant="inline" title={t("adminDashboard.noUsage")} className="py-8" />
        ) : (
          <Table aria-label={title} className="table-fixed">
            <TableHeader>
              <TableRow className="hover:bg-transparent">
                <TableHead className="w-12 pl-5">#</TableHead>
                <TableHead>{t("adminDashboard.username")}</TableHead>
                <TableHead className="w-24 whitespace-nowrap text-right">{t("adminDashboard.calls")}</TableHead>
                <TableHead className="w-36 whitespace-nowrap pr-5 text-right">{t("adminDashboard.cost")}</TableHead>
              </TableRow>
            </TableHeader>
            <TableBody>
              {rows.map((row, index) => (
                <TableRow key={row.user_id}>
                  <TableCell className="pl-5 tabular-nums text-muted-foreground">{index + 1}</TableCell>
                  <TableCell>
                    <span className={cn("block truncate", !row.username && "font-mono")} title={row.username || row.user_id}>
                      {row.username || row.user_id}
                    </span>
                  </TableCell>
                  <TableCell className="text-right tabular-nums">{formatNumber(row.call_count)}</TableCell>
                  <TableCell className="pr-5 text-right tabular-nums">{formatNanoUsd(row.cost_nano_usd, 6)}</TableCell>
                </TableRow>
              ))}
            </TableBody>
          </Table>
        )}
      </CardContent>
    </Card>
  );
}

function ChannelStatusCell({ channel, status }: { channel: AdminOverviewChannelHealth; status: ChannelStatus }) {
  const { t } = useTranslation();
  const models = channel.unhealthy_models ?? [];
  return (
    <>
      {status === "cooling" ? (
        <StatusBadge variant="warning">{t("adminDashboard.coolingDown")}</StatusBadge>
      ) : status === "disabled" ? (
        <Badge variant="secondary">{t("adminDashboard.disabled")}</Badge>
      ) : status === "healthy" ? (
        <StatusBadge variant="success">{t("adminDashboard.healthy")}</StatusBadge>
      ) : (
        <StatusBadge variant="destructive">{t("adminDashboard.unhealthy")}</StatusBadge>
      )}
      {models.length > 0 ? (
        <span className="mt-1 block truncate font-mono text-error-foreground" title={models.join(", ")}>
          {models.join(", ")}
        </span>
      ) : null}
    </>
  );
}

export function ChannelHealthCard({
  data,
  spendWindow,
  onSpendWindowChange,
  pending,
}: {
  data: AdminOverview;
  spendWindow: SpendWindow;
  onSpendWindowChange: (window: SpendWindow) => void;
  pending: boolean;
}) {
  const { t } = useTranslation();
  const scrollParent = useContext(DashboardScrollParentContext);
  const rows = data.channel_health;
  const unhealthy = rows.filter((channel) => {
    const status = channelStatus(channel);
    return status === "cooling" || status === "unhealthy";
  }).length;

  return (
    <Card className="overflow-clip">
      <CardHeader className="gap-3 space-y-0 p-5">
        <div className="flex flex-wrap items-start justify-between gap-3">
          <div className="flex min-w-0 flex-col gap-1.5">
            <CardTitle>{t("adminDashboard.channelHealth")}</CardTitle>
            <CardDescription className="tabular-nums">
              {t("adminDashboard.channelSummary", { count: rows.length, unhealthy })}
            </CardDescription>
          </div>
          <SpendWindowControl value={spendWindow} onChange={onSpendWindowChange} />
        </div>
        <p className="flex flex-wrap items-baseline gap-x-4 gap-y-1 text-sm text-muted-foreground">
          <span>
            {t("adminDashboard.spend")}{" "}
            <span className="font-medium tabular-nums text-foreground">
              {formatNanoUsd(data.spend?.cost_nano_usd ?? "0", 2)}
            </span>
          </span>
          <span>
            {t("adminDashboard.calls")}{" "}
            <span className="font-medium tabular-nums text-foreground">{formatNumber(data.spend?.calls ?? 0)}</span>
          </span>
          <span>{t("adminDashboard.spendNote")}</span>
        </p>
      </CardHeader>
      <CardContent className={cn("border-t p-0 transition-opacity", pending && "opacity-60")}>
        {rows.length === 0 ? (
          <EmptyState variant="inline" title={t("adminDashboard.noChannels")} className="py-8" />
        ) : (
          <DataList columns="minmax(0,1fr) 4.5rem 5rem 10rem 8rem 10.5rem">
            <DataListHeader>
              <DataListHead>{t("adminDashboard.channel")}</DataListHead>
              <DataListHead align="end">{t("adminDashboard.weight")}</DataListHead>
              <DataListHead>{t("adminDashboard.affinity")}</DataListHead>
              <DataListHead>{t("adminDashboard.status")}</DataListHead>
              <DataListHead align="end">{t("adminDashboard.spend")}</DataListHead>
              <DataListHead align="end">{t("adminDashboard.lastProbe")}</DataListHead>
            </DataListHeader>
            {scrollParent && (
              <Virtuoso
                customScrollParent={scrollParent}
                data={rows}
                context={{ label: t("adminDashboard.channelHealth") }}
                computeItemKey={(_index, channel) => channel.channel_id}
                components={virtualDataListComponents}
                itemContent={(_index, channel) => (
                  <>
                    <DataListCell primary>
                      <span className="block truncate font-medium" title={channel.channel_name}>
                        {channel.channel_name}
                      </span>
                      <span className="block truncate text-muted-foreground" title={channel.provider_name}>
                        {channel.provider_name}
                      </span>
                    </DataListCell>
                    <DataListCell label={t("adminDashboard.weight")} align="end">
                      <span className="tabular-nums">{channel.weight}</span>
                    </DataListCell>
                    <DataListCell label={t("adminDashboard.affinity")}>
                      {channel.session_affinity_auto ? "auto" : <span className="text-muted-foreground">—</span>}
                    </DataListCell>
                    <DataListCell label={t("adminDashboard.status")}>
                      <ChannelStatusCell channel={channel} status={channelStatus(channel)} />
                    </DataListCell>
                    <DataListCell label={t("adminDashboard.spend")} align="end">
                      <span className="block tabular-nums">
                        {formatNanoUsd(channel.window_cost_nano_usd ?? "0", 2)}
                      </span>
                      <span className="block tabular-nums text-muted-foreground">
                        {t("adminDashboard.callsCount", { count: channel.window_calls ?? 0 })}
                      </span>
                    </DataListCell>
                    <DataListCell label={t("adminDashboard.lastProbe")} align="end">
                      <span className="tabular-nums text-muted-foreground">
                        {formatTimestamp(channel.last_probe_at)}
                      </span>
                    </DataListCell>
                  </>
                )}
              />
            )}
          </DataList>
        )}
      </CardContent>
    </Card>
  );
}
