import { useEffect, useMemo, useState } from "react";
import { useTranslation } from "react-i18next";
import { Eye, Play, RefreshCw, Save } from "lucide-react";
import { toast } from "sonner";
import { Badge } from "@/components/ui/badge";
import { Button } from "@/components/ui/button";
import { Card } from "@/components/ui/card";
import { Input } from "@/components/ui/input";
import { Label } from "@/components/ui/label";
import { Skeleton } from "@/components/ui/skeleton";
import { StatusBadge } from "@/components/ui/status";
import {
  Dialog,
  DialogContent,
  DialogDescription,
  DialogHeader,
  DialogTitle,
} from "@/components/ui/dialog";
import {
  VirtualTableCell,
  VirtualTableHeaderCell,
} from "@/components/ui/data-table-shell";
import { api } from "@/lib/api";
import type {
  PriceSyncPreview,
  PriceSyncRun,
  PriceSyncSource,
  SystemSettings,
} from "@/lib/api";
import {
  applyPriceSync,
  updateSettingsOptimistic,
  usePriceSyncRuns,
  useSettings,
} from "@/lib/swr";

const SOURCES: Array<{
  id: PriceSyncSource;
  name: string;
  descriptionKey: [string, string];
}> = [
  {
    id: "models_dev",
    name: "models.dev",
    descriptionKey: [
      "modelPricing.sync.modelsDevDesc",
      "Official catalog. Prefers the official provider variant, else the highest non-zero input price.",
    ],
  },
  {
    id: "openrouter",
    name: "OpenRouter",
    descriptionKey: [
      "modelPricing.sync.openrouterDesc",
      "OpenRouter public model list. Fills models absent from models.dev.",
    ],
  },
  {
    id: "new_api",
    name: "new-api",
    descriptionKey: [
      "modelPricing.sync.newApiDesc",
      "A reachable new-api instance. Ratios convert to USD at $2 per unit ratio.",
    ],
  },
];

function runStatusBadge(run: PriceSyncRun | undefined, t: (key: string, fallback: string) => string) {
  if (!run) {
    return <Badge variant="outline">{t("modelPricing.sync.neverRan", "Never ran")}</Badge>;
  }
  if (run.status === "success") {
    return <StatusBadge variant="success">{t("modelPricing.sync.success", "Success")}</StatusBadge>;
  }
  if (run.status === "running") {
    return <StatusBadge variant="info">{t("modelPricing.sync.running", "Running")}</StatusBadge>;
  }
  return <StatusBadge variant="destructive">{t("modelPricing.sync.failed", "Failed")}</StatusBadge>;
}

export function UpstreamSyncTab() {
  const { t } = useTranslation();
  const { data: runs = [], isLoading: runsLoading, mutate: refreshRuns } = usePriceSyncRuns();
  const { data: settings } = useSettings();
  const [preview, setPreview] = useState<PriceSyncPreview | null>(null);
  const [busySource, setBusySource] = useState<string | null>(null);
  const [baseUrl, setBaseUrl] = useState("");
  const [token, setToken] = useState("");
  const [tokenTouched, setTokenTouched] = useState(false);
  const [connectionDirty, setConnectionDirty] = useState(false);
  const [savingConnection, setSavingConnection] = useState(false);

  useEffect(() => {
    if (!connectionDirty && settings) {
      setBaseUrl(settings.price_sync_new_api_base_url ?? "");
    }
  }, [settings, connectionDirty]);

  const latestRunBySource = useMemo(() => {
    const map = new Map<string, PriceSyncRun>();
    for (const run of runs) {
      if (!map.has(run.source)) map.set(run.source, run);
    }
    return map;
  }, [runs]);

  const tokenConfigured = settings?.price_sync_new_api_token === "__set__";

  const handlePreview = async (source: PriceSyncSource) => {
    setBusySource(`preview:${source}`);
    try {
      setPreview(await api.previewPriceSync(source));
    } catch (error) {
      toast.error(t("modelPricing.sync.previewFailed", "Preview failed"), {
        description: error instanceof Error ? error.message : String(error),
      });
    } finally {
      setBusySource(null);
    }
  };

  const handleApply = async (source: PriceSyncSource) => {
    setBusySource(`apply:${source}`);
    try {
      const run = await applyPriceSync(source, (error) =>
        toast.error(t("modelPricing.sync.applyFailed", "Sync failed"), {
          description: error.message,
        })
      );
      toast.success(
        t("modelPricing.sync.applySuccess", "Synced: {{inserted}} inserted, {{updated}} updated", {
          inserted: run.inserted,
          updated: run.updated,
        })
      );
    } catch {
      return;
    } finally {
      setBusySource(null);
    }
  };

  const saveConnection = async () => {
    if (!settings) return;
    setSavingConnection(true);
    // MP-Y2a: "__set__" keeps the stored token; "" clears it; any other
    // string replaces it. An untouched field therefore keeps the token.
    const tokenValue = tokenTouched ? token : tokenConfigured ? "__set__" : "";
    const next: SystemSettings = {
      ...settings,
      price_sync_new_api_base_url: baseUrl.trim(),
      price_sync_new_api_token: tokenValue,
    };
    try {
      await updateSettingsOptimistic(next, (error) =>
        toast.error(t("modelPricing.sync.connectionFailed", "Failed to save connection"), {
          description: error.message,
        })
      );
      setConnectionDirty(false);
      setToken("");
      setTokenTouched(false);
      toast.success(t("modelPricing.sync.connectionSaved", "new-api connection saved"));
    } catch {
      return;
    } finally {
      setSavingConnection(false);
    }
  };

  return (
    <div className="space-y-4">
      <div className="grid gap-4 lg:grid-cols-3">
        {SOURCES.map((source) => {
          const lastRun = latestRunBySource.get(source.id);
          return (
            <Card key={source.id} className="flex flex-col gap-3 p-4">
              <div className="flex items-center justify-between gap-2">
                <h3 className="text-base font-semibold leading-none tracking-tight">
                  {source.name}
                </h3>
                {runStatusBadge(lastRun, (key, fallback) => t(key, fallback))}
              </div>
              <p className="text-sm text-muted-foreground">
                {t(...source.descriptionKey)}
              </p>
              {lastRun ? (
                <p className="font-mono text-xs text-muted-foreground">
                  {new Date(lastRun.started_at).toLocaleString()} · +{lastRun.inserted} ~
                  {lastRun.updated} −{lastRun.deleted}
                </p>
              ) : null}

              {source.id === "new_api" ? (
                <div className="space-y-2 rounded-lg border p-3">
                  <div className="space-y-1">
                    <Label className="text-xs">
                      {t("modelPricing.sync.baseUrl", "Base URL")}
                    </Label>
                    <Input
                      value={baseUrl}
                      onChange={(event) => {
                        setBaseUrl(event.target.value);
                        setConnectionDirty(true);
                      }}
                      placeholder="https://new-api.example.com"
                      className="font-mono text-xs"
                    />
                  </div>
                  <div className="space-y-1">
                    <Label className="text-xs">
                      {t("modelPricing.sync.token", "Access token")}
                    </Label>
                    <Input
                      type="password"
                      autoComplete="new-password"
                      value={token}
                      onChange={(event) => {
                        setToken(event.target.value);
                        setTokenTouched(true);
                        setConnectionDirty(true);
                      }}
                      placeholder={
                        tokenConfigured
                          ? t("modelPricing.sync.tokenSet", "Configured — leave blank to keep")
                          : t("modelPricing.sync.tokenUnset", "Not configured")
                      }
                      className="font-mono text-xs"
                    />
                  </div>
                  <Button
                    variant="outline"
                    size="sm"
                    className="w-full"
                    disabled={!connectionDirty || savingConnection}
                    onClick={() => void saveConnection()}
                  >
                    <Save className="mr-1.5 h-3.5 w-3.5" />
                    {savingConnection
                      ? t("common.saving", "Saving...")
                      : t("modelPricing.sync.saveConnection", "Save connection")}
                  </Button>
                </div>
              ) : null}

              <div className="mt-auto flex items-center gap-2 pt-1">
                <Button
                  variant="outline"
                  size="sm"
                  className="flex-1"
                  disabled={busySource === `preview:${source.id}`}
                  onClick={() => void handlePreview(source.id)}
                >
                  <Eye className="mr-1.5 h-3.5 w-3.5" />
                  {t("modelPricing.sync.preview", "Preview")}
                </Button>
                <Button
                  size="sm"
                  className="flex-1"
                  disabled={busySource === `apply:${source.id}`}
                  onClick={() => void handleApply(source.id)}
                >
                  <Play className="mr-1.5 h-3.5 w-3.5" />
                  {t("modelPricing.sync.apply", "Apply")}
                </Button>
              </div>
            </Card>
          );
        })}
      </div>

      <div className="space-y-3">
        <div className="flex items-center justify-between gap-3">
          <h3 className="text-base font-semibold leading-none tracking-tight">
            {t("modelPricing.sync.recentRuns", "Recent sync runs")}
          </h3>
          <Button variant="ghost" size="sm" onClick={() => void refreshRuns()}>
            <RefreshCw className="mr-1.5 h-3.5 w-3.5" />
            {t("common.refresh", "Refresh")}
          </Button>
        </div>
        {runsLoading ? (
          <Skeleton className="h-40 w-full" />
        ) : runs.length === 0 ? (
          <Card className="px-6 py-8 text-center text-sm text-muted-foreground">
            {t("modelPricing.sync.noRuns", "No sync runs recorded yet.")}
          </Card>
        ) : (
          <Card className="overflow-x-auto">
            <table className="w-full caption-bottom text-sm">
              <thead className="[&_tr]:border-b">
                <tr className="border-b">
                  <VirtualTableHeaderCell>
                    {t("modelPricing.sync.source", "Source")}
                  </VirtualTableHeaderCell>
                  <VirtualTableHeaderCell>
                    {t("modelPricing.sync.statusCol", "Status")}
                  </VirtualTableHeaderCell>
                  <VirtualTableHeaderCell>
                    {t("modelPricing.sync.startedAt", "Started")}
                  </VirtualTableHeaderCell>
                  <VirtualTableHeaderCell>
                    {t("modelPricing.sync.counts", "Inserted / Updated / Skipped / Deleted")}
                  </VirtualTableHeaderCell>
                  <VirtualTableHeaderCell>
                    {t("modelPricing.sync.error", "Error")}
                  </VirtualTableHeaderCell>
                </tr>
              </thead>
              <tbody className="[&_tr:last-child]:border-0">
                {runs.map((run) => (
                  <tr key={run.id} className="border-b">
                    <VirtualTableCell className="font-mono text-xs">{run.source}</VirtualTableCell>
                    <VirtualTableCell>
                      {runStatusBadge(run, (key, fallback) => t(key, fallback))}
                    </VirtualTableCell>
                    <VirtualTableCell className="font-mono text-xs">
                      {new Date(run.started_at).toLocaleString()}
                    </VirtualTableCell>
                    <VirtualTableCell className="font-mono text-xs">
                      {run.inserted} / {run.updated} / {run.skipped} / {run.deleted}
                    </VirtualTableCell>
                    <VirtualTableCell className="max-w-[240px] truncate text-xs text-destructive">
                      {run.error ?? "—"}
                    </VirtualTableCell>
                  </tr>
                ))}
              </tbody>
            </table>
          </Card>
        )}
      </div>

      <Dialog open={!!preview} onOpenChange={(open) => !open && setPreview(null)}>
        <DialogContent className="max-h-[calc(100dvh-4rem)] overflow-hidden sm:max-w-2xl">
          <DialogHeader>
            <DialogTitle>
              {t("modelPricing.sync.previewTitle", "Sync preview: {{source}}", {
                source: preview?.source ?? "",
              })}
            </DialogTitle>
            <DialogDescription>
              {t(
                "modelPricing.sync.previewSummary",
                "{{insert}} insert · {{update}} update · {{skip}} skip · {{del}} delete",
                {
                  insert: preview?.insert ?? 0,
                  update: preview?.update ?? 0,
                  skip: preview?.skip ?? 0,
                  del: preview?.delete ?? 0,
                }
              )}
              {preview?.truncated
                ? ` ${t("modelPricing.sync.previewTruncated", "(list truncated)")}`
                : ""}
            </DialogDescription>
          </DialogHeader>
          <div className="max-h-[50dvh] overflow-y-auto rounded-lg border">
            <table className="w-full caption-bottom text-sm">
              <thead className="sticky top-0 bg-background [&_tr]:border-b">
                <tr className="border-b">
                  <VirtualTableHeaderCell>
                    {t("modelPricing.model", "Model")}
                  </VirtualTableHeaderCell>
                  <VirtualTableHeaderCell>
                    {t("modelPricing.sync.changeKind", "Change")}
                  </VirtualTableHeaderCell>
                  <VirtualTableHeaderCell>
                    {t("modelPricing.sync.changedFields", "Fields")}
                  </VirtualTableHeaderCell>
                </tr>
              </thead>
              <tbody className="[&_tr:last-child]:border-0">
                {(preview?.changes ?? []).map((change) => (
                  <tr key={`${change.kind}:${change.model_id}`} className="border-b">
                    <VirtualTableCell className="font-mono text-xs">
                      {change.model_id}
                    </VirtualTableCell>
                    <VirtualTableCell>
                      <Badge
                        variant={change.kind === "delete" ? "destructive" : "secondary"}
                        className="text-xs"
                      >
                        {change.kind}
                      </Badge>
                    </VirtualTableCell>
                    <VirtualTableCell className="font-mono text-xs text-muted-foreground">
                      {change.fields?.join(", ") ?? "—"}
                    </VirtualTableCell>
                  </tr>
                ))}
              </tbody>
            </table>
          </div>
        </DialogContent>
      </Dialog>
    </div>
  );
}
