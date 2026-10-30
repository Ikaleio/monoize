import { useEffect, useId, useMemo, useState } from "react";
import { useTranslation } from "react-i18next";
import type { TFunction } from "i18next";
import { Link } from "react-router-dom";
import { CloudDownload, Eye, RefreshCw, Save } from "lucide-react";
import { toast } from "sonner";
import { Badge } from "@/components/ui/badge";
import { Button, buttonVariants } from "@/components/ui/button";
import {
  Card,
  CardContent,
  CardDescription,
  CardHeader,
  CardTitle,
} from "@/components/ui/card";
import {
  Field,
  FieldContent,
  FieldDescription,
  FieldTitle,
} from "@/components/ui/field";
import { Input } from "@/components/ui/input";
import { Label } from "@/components/ui/label";
import { QueryError } from "@/components/ui/query-error";
import { SegmentedControl } from "@/components/ui/segmented-control";
import { Skeleton } from "@/components/ui/skeleton";
import { StatusBadge } from "@/components/ui/status";
import { Switch } from "@/components/ui/switch";
import {
  Dialog,
  DialogContent,
  DialogDescription,
  DialogFooter,
  DialogHeader,
  DialogTitle,
} from "@/components/ui/dialog";
import {
  AlertDialog,
  AlertDialogAction,
  AlertDialogCancel,
  AlertDialogContent,
  AlertDialogDescription,
  AlertDialogFooter,
  AlertDialogHeader,
  AlertDialogTitle,
} from "@/components/ui/alert-dialog";
import {
  VirtualTableCell,
  VirtualTableHeaderCell,
} from "@/components/ui/data-table-shell";
import { api, DashboardApiError } from "@/lib/api";
import { toastApiError } from "@/lib/api-error";
import { formatDateTime } from "@/lib/format-time";
import type {
  BillingExpr,
  PriceSyncChange,
  PriceSyncPreview,
  PriceSyncRun,
  PriceSyncSource,
} from "@/lib/api";
import {
  applyPriceSync,
  patchPricingSettingsOptimistic,
  syncModelMetadata,
  usePriceSyncRuns,
  useSettings,
} from "@/lib/swr";
import { formatUsdPerM } from "./shared";

const SOURCES: Array<{
  id: PriceSyncSource;
  name: string;
  descriptionKey: string;
}> = [
  { id: "models_dev", name: "models.dev", descriptionKey: "modelPricing.sync.modelsDevDesc" },
  { id: "openrouter", name: "OpenRouter", descriptionKey: "modelPricing.sync.openrouterDesc" },
  { id: "new_api", name: "new-api", descriptionKey: "modelPricing.sync.newApiDesc" },
];

const SOURCE_NAMES: Record<PriceSyncSource, string> = {
  models_dev: "models.dev",
  openrouter: "OpenRouter",
  new_api: "new-api",
};

const PREVIEW_FILTERS = ["changes", "deletes", "skips"] as const;
type PreviewFilter = (typeof PREVIEW_FILTERS)[number];

const FIELD_LABEL_KEYS: Record<string, string> = {
  billing_mode: "modelPricing.mode",
  input_usd_per_1m: "modelPricing.fieldInput",
  output_usd_per_1m: "modelPricing.fieldOutput",
  cache_read_usd_per_1m: "modelPricing.fieldCacheRead",
  cache_write_usd_per_1m: "modelPricing.fieldCacheWrite",
  cache_write_1h_usd_per_1m: "modelPricing.fieldCacheWrite1h",
  reasoning_usd_per_1m: "modelPricing.fieldReasoning",
  per_request_usd: "modelPricing.modePerRequest",
  billing_expr: "modelPricing.modeTiered",
};

const MODE_LABEL_KEYS: Record<string, string> = {
  per_token: "modelPricing.modePerToken",
  per_request: "modelPricing.modePerRequest",
  tiered_expr: "modelPricing.modeTiered",
};

function formatChangeValue(field: string, value: unknown, t: TFunction): string {
  if (value == null) return "—";
  if (field === "billing_mode" && typeof value === "string") {
    return MODE_LABEL_KEYS[value] ? t(MODE_LABEL_KEYS[value]) : value;
  }
  if (field === "billing_expr" && typeof value === "object") {
    return t("modelPricing.sync.exprTiers", { count: (value as BillingExpr).tiers?.length ?? 0 });
  }
  if (typeof value === "string") return formatUsdPerM(value);
  return JSON.stringify(value);
}

/** MP-A6 diff text: `field: old → new` for updates, `field: value` otherwise. */
function ChangeDetail({ change }: { change: PriceSyncChange }) {
  const { t } = useTranslation();
  const values = change.kind === "delete" ? change.before : change.after;
  const fields = change.kind === "update" ? (change.fields ?? []) : Object.keys(values ?? {});
  if (fields.length === 0) return <>—</>;
  return (
    <ul className="space-y-0.5">
      {fields.map((field) => {
        const label = FIELD_LABEL_KEYS[field] ? t(FIELD_LABEL_KEYS[field]) : field;
        return (
          <li key={field}>
            {label}: {change.kind === "update" ? (
              <>
                {formatChangeValue(field, change.before?.[field], t)}
                {" → "}
                {formatChangeValue(field, change.after?.[field], t)}
              </>
            ) : (
              formatChangeValue(field, values?.[field], t)
            )}
          </li>
        );
      })}
    </ul>
  );
}

function runStatusBadge(run: PriceSyncRun | undefined, t: TFunction) {
  if (!run) {
    return <Badge variant="outline">{t("modelPricing.sync.neverRan")}</Badge>;
  }
  if (run.status === "success") {
    return <StatusBadge variant="success">{t("modelPricing.sync.success")}</StatusBadge>;
  }
  if (run.status === "running") {
    return <StatusBadge variant="info">{t("modelPricing.sync.running")}</StatusBadge>;
  }
  return <StatusBadge variant="destructive">{t("modelPricing.sync.failed")}</StatusBadge>;
}

export function UpstreamSyncTab() {
  const { t } = useTranslation();
  const connectionId = useId();
  const {
    data: runsData,
    error: runsError,
    isLoading: runsLoading,
    isValidating: runsValidating,
    mutate: refreshRuns,
  } = usePriceSyncRuns();
  const runs = useMemo(() => runsData ?? [], [runsData]);
  const {
    data: settings,
    error: settingsError,
    isValidating: settingsValidating,
    mutate: refreshSettings,
  } = useSettings();
  const [preview, setPreview] = useState<PriceSyncPreview | null>(null);
  const [previewFilter, setPreviewFilter] = useState<PreviewFilter>("changes");
  const [confirmDeleteOpen, setConfirmDeleteOpen] = useState(false);
  const [previewingSource, setPreviewingSource] = useState<PriceSyncSource | null>(null);
  const [applying, setApplying] = useState(false);
  const [metadataSyncing, setMetadataSyncing] = useState(false);
  const [baseUrl, setBaseUrl] = useState("");
  const [token, setToken] = useState("");
  const [tokenTouched, setTokenTouched] = useState(false);
  const [connectionDirty, setConnectionDirty] = useState(false);
  const [savingConnection, setSavingConnection] = useState(false);
  const [savingAutoSync, setSavingAutoSync] = useState(false);

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
  // MP-Y2: new-api is disabled until a base URL is saved.
  const newApiConfigured = !!settings?.price_sync_new_api_base_url?.trim();

  const changeCount = preview ? preview.insert + preview.update + preview.delete : 0;
  const visibleChanges = useMemo(
    () =>
      (preview?.changes ?? []).filter((change) =>
        previewFilter === "changes"
          ? change.kind === "insert" || change.kind === "update"
          : previewFilter === "deletes"
            ? change.kind === "delete"
            : change.kind === "skip",
      ),
    [preview, previewFilter],
  );
  // MP-A6 lists only rows skipped for ownership; rows whose values are unchanged
  // count toward `skip` but have no entry, so the difference must be explained.
  const unlistedSkips = preview
    ? preview.skip - preview.changes.filter((change) => change.kind === "skip").length
    : 0;

  const handlePreview = async (source: PriceSyncSource) => {
    setPreviewingSource(source);
    try {
      const result = await api.previewPriceSync(source);
      setPreviewFilter(result.insert + result.update > 0 || result.delete === 0 ? "changes" : "deletes");
      setPreview(result);
    } catch (error) {
      toastApiError(error, t, t("modelPricing.sync.previewFailed"));
    } finally {
      setPreviewingSource(null);
    }
  };

  // MP-UI6: apply is reachable only from the preview of the same source and
  // applies exactly that preview snapshot (MP-A7).
  const handleApply = async () => {
    if (!preview) return;
    setApplying(true);
    try {
      const run = await applyPriceSync(preview.source, preview.preview_id, (error) =>
        toastApiError(error, t, t("modelPricing.sync.applyFailed")),
      );
      toast.success(
        t("modelPricing.sync.applySuccess", {
          inserted: run.inserted,
          updated: run.updated,
          deleted: run.deleted,
        }),
      );
      setConfirmDeleteOpen(false);
      setPreview(null);
    } catch (error) {
      setConfirmDeleteOpen(false);
      // Preview ids are single-use (MP-A7): a stale or expired preview can never
      // be applied again, so keeping its dialog open would only offer a dead action.
      if (error instanceof DashboardApiError && error.code.startsWith("price_sync_preview_")) {
        setPreview(null);
      }
    } finally {
      setApplying(false);
    }
  };

  const handleMetadataSync = async () => {
    setMetadataSyncing(true);
    try {
      const result = await syncModelMetadata((error) =>
        toastApiError(error, t, t("modelPricing.sync.metadataFailed")),
      );
      toast.success(t("modelPricing.sync.metadataSuccess", { upserted: result.upserted }));
    } catch {
      return;
    } finally {
      setMetadataSyncing(false);
    }
  };

  const saveConnection = async () => {
    if (!settings) return;
    setSavingConnection(true);
    try {
      // MP-Y2a / MP-A8: send only the connection keys; an untouched token
      // field is omitted so the stored token is kept.
      await patchPricingSettingsOptimistic(
        {
          price_sync_new_api_base_url: baseUrl.trim(),
          ...(tokenTouched ? { price_sync_new_api_token: token } : {}),
        },
        (error) => toastApiError(error, t, t("modelPricing.sync.connectionFailed")),
      );
      setConnectionDirty(false);
      setToken("");
      setTokenTouched(false);
      toast.success(t("modelPricing.sync.connectionSaved"));
    } catch {
      return;
    } finally {
      setSavingConnection(false);
    }
  };

  const setAutoSyncEnabled = async (enabled: boolean) => {
    if (!settings) return;
    setSavingAutoSync(true);
    try {
      await patchPricingSettingsOptimistic({ price_sync_auto_enabled: enabled }, (error) =>
        toastApiError(error, t, t("modelPricing.sync.autoSaveFailed")),
      );
    } catch {
      return;
    } finally {
      setSavingAutoSync(false);
    }
  };

  const runSummary = (run: PriceSyncRun) =>
    t("modelPricing.sync.runSummary", {
      inserted: run.inserted,
      updated: run.updated,
      deleted: run.deleted,
    });

  return (
    <div className="flex flex-col gap-4">
      {settingsError ? (
        <QueryError
          onRetry={() => refreshSettings()}
          retrying={settingsValidating}
          stale={settings !== undefined}
        />
      ) : null}
      {settings === undefined ? (
        settingsError ? null : <Skeleton className="h-32 w-full" />
      ) : (
        <Card>
          <CardHeader>
            <CardTitle>{t("modelPricing.sync.autoTitle")}</CardTitle>
            <CardDescription>
              {t("modelPricing.sync.autoDescription")}{" "}
              <Link
                to="/dashboard/admin-settings?category=billing"
                className="text-primary underline-offset-4 hover:underline"
              >
                {t("modelPricing.sync.billingSettingsLink")}
              </Link>
            </CardDescription>
          </CardHeader>
          <CardContent>
            <Field orientation="horizontal" data-disabled={savingAutoSync || undefined}>
              <FieldContent>
                <FieldTitle>
                  {settings.price_sync_auto_enabled
                    ? t("modelPricing.sync.autoEnabled")
                    : t("modelPricing.sync.autoDisabled")}
                </FieldTitle>
                <FieldDescription>{t("modelPricing.sync.autoToggleDescription")}</FieldDescription>
              </FieldContent>
              <Switch
                aria-label={t("modelPricing.sync.autoTitle")}
                checked={settings.price_sync_auto_enabled}
                disabled={savingAutoSync}
                onCheckedChange={(checked) => void setAutoSyncEnabled(checked)}
              />
            </Field>
          </CardContent>
        </Card>
      )}
      <div className="grid gap-4 lg:grid-cols-3">
        {SOURCES.map((source) => {
          const lastRun = latestRunBySource.get(source.id);
          const previewBlocked = source.id === "new_api" && !newApiConfigured;
          return (
            <Card key={source.id} className="flex flex-col gap-3 p-4">
              <div className="flex items-center justify-between gap-2">
                <h3 className="text-base font-semibold leading-none tracking-tight">{source.name}</h3>
                {runStatusBadge(lastRun, t)}
              </div>
              <p className="text-sm text-muted-foreground">{t(source.descriptionKey)}</p>
              {lastRun ? (
                <p className="font-mono text-xs text-muted-foreground">
                  {formatDateTime(lastRun.started_at)} · {runSummary(lastRun)}
                </p>
              ) : null}

              {source.id === "new_api" ? (
                <div className="space-y-2 rounded-lg border p-3">
                  <div className="space-y-1">
                    <Label htmlFor={`${connectionId}-base-url`} className="text-xs">
                      {t("modelPricing.sync.baseUrl")}
                    </Label>
                    <Input
                      id={`${connectionId}-base-url`}
                      type="url"
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
                    <Label htmlFor={`${connectionId}-token`} className="text-xs">
                      {t("modelPricing.sync.token")}
                    </Label>
                    <Input
                      id={`${connectionId}-token`}
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
                          ? t("modelPricing.sync.tokenSet")
                          : t("modelPricing.sync.tokenUnset")
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
                    <Save data-icon="inline-start" aria-hidden="true" />
                    {savingConnection ? t("common.saving") : t("modelPricing.sync.saveConnection")}
                  </Button>
                </div>
              ) : null}

              <div className="mt-auto flex items-center gap-2 pt-1">
                <Button
                  variant="outline"
                  size="sm"
                  className="flex-1"
                  disabled={previewingSource !== null || previewBlocked}
                  aria-describedby={previewBlocked ? `${connectionId}-preview-blocked` : undefined}
                  onClick={() => void handlePreview(source.id)}
                >
                  <Eye data-icon="inline-start" aria-hidden="true" />
                  {previewingSource === source.id
                    ? t("modelPricing.sync.previewing")
                    : t("modelPricing.sync.preview")}
                </Button>
              </div>
              {previewBlocked ? (
                <p id={`${connectionId}-preview-blocked`} className="text-xs text-muted-foreground">
                  {t("modelPricing.sync.newApiUnconfigured")}
                </p>
              ) : null}
              {source.id === "models_dev" ? (
                <Button
                  variant="ghost"
                  size="sm"
                  disabled={metadataSyncing}
                  onClick={() => void handleMetadataSync()}
                >
                  <CloudDownload
                    className={`mr-1.5 h-3.5 w-3.5 ${metadataSyncing ? "animate-pulse" : ""}`}
                    aria-hidden="true"
                  />
                  {metadataSyncing
                    ? t("modelPricing.sync.metadataSyncing")
                    : t("modelPricing.sync.metadataSync")}
                </Button>
              ) : null}
            </Card>
          );
        })}
      </div>

      <div className="space-y-3">
        <div className="flex items-center justify-between gap-3">
          <h3 className="text-base font-semibold leading-none tracking-tight">
            {t("modelPricing.sync.recentRuns")}
          </h3>
          <Button variant="ghost" size="sm" onClick={() => void refreshRuns()}>
            <RefreshCw data-icon="inline-start" aria-hidden="true" />
            {t("common.refresh")}
          </Button>
        </div>
        {runsError ? (
          <QueryError
            onRetry={() => refreshRuns()}
            retrying={runsValidating}
            stale={runsData !== undefined}
          />
        ) : null}
        {runsLoading ? (
          <Skeleton className="h-40 w-full" />
        ) : runsData === undefined ? null : runs.length === 0 ? (
          <Card className="px-6 py-8 text-center text-sm text-muted-foreground">
            {t("modelPricing.sync.noRuns")}
          </Card>
        ) : (
          <Card className="overflow-x-auto">
            <table className="w-full caption-bottom text-sm">
              <thead className="[&_tr]:border-b">
                <tr className="border-b">
                  <VirtualTableHeaderCell>{t("modelPricing.sync.source")}</VirtualTableHeaderCell>
                  <VirtualTableHeaderCell>{t("modelPricing.sync.statusCol")}</VirtualTableHeaderCell>
                  <VirtualTableHeaderCell>{t("modelPricing.sync.startedAt")}</VirtualTableHeaderCell>
                  <VirtualTableHeaderCell>{t("modelPricing.sync.counts")}</VirtualTableHeaderCell>
                  <VirtualTableHeaderCell>{t("modelPricing.sync.error")}</VirtualTableHeaderCell>
                </tr>
              </thead>
              <tbody className="[&_tr:last-child]:border-0">
                {runs.map((run) => (
                  <tr key={run.id} className="border-b">
                    <VirtualTableCell className="font-mono text-xs">
                      {SOURCE_NAMES[run.source as PriceSyncSource] ?? run.source}
                    </VirtualTableCell>
                    <VirtualTableCell>{runStatusBadge(run, t)}</VirtualTableCell>
                    <VirtualTableCell className="font-mono text-xs">
                      {formatDateTime(run.started_at)}
                    </VirtualTableCell>
                    <VirtualTableCell className="font-mono text-xs">
                      {t("modelPricing.sync.runSummaryWithSkip", {
                        inserted: run.inserted,
                        updated: run.updated,
                        skipped: run.skipped,
                        deleted: run.deleted,
                      })}
                    </VirtualTableCell>
                    <VirtualTableCell
                      className="max-w-[240px] truncate text-xs text-error-foreground"
                      title={run.error ?? undefined}
                    >
                      {run.error ?? "—"}
                    </VirtualTableCell>
                  </tr>
                ))}
              </tbody>
            </table>
          </Card>
        )}
      </div>

      <Dialog open={!!preview} onOpenChange={(open) => !open && !applying && setPreview(null)}>
        <DialogContent className="flex max-h-[calc(100dvh-4rem)] flex-col overflow-hidden sm:max-w-2xl">
          <DialogHeader className="shrink-0">
            <DialogTitle>
              {t("modelPricing.sync.previewTitle", {
                source: preview ? SOURCE_NAMES[preview.source] : "",
              })}
            </DialogTitle>
            <DialogDescription>
              {t("modelPricing.sync.previewSummary", {
                insert: preview?.insert ?? 0,
                update: preview?.update ?? 0,
                skip: preview?.skip ?? 0,
                del: preview?.delete ?? 0,
              })}
              {preview?.truncated ? ` ${t("modelPricing.sync.previewTruncated")}` : ""}
            </DialogDescription>
          </DialogHeader>
          <SegmentedControl
            value={previewFilter}
            options={PREVIEW_FILTERS}
            onChange={setPreviewFilter}
            ariaLabel={t("modelPricing.sync.previewFilter")}
            label={(option) =>
              option === "changes"
                ? t("modelPricing.sync.filterChanges", {
                    count: (preview?.insert ?? 0) + (preview?.update ?? 0),
                  })
                : option === "deletes"
                  ? t("modelPricing.sync.filterDeletes", { count: preview?.delete ?? 0 })
                  : t("modelPricing.sync.filterSkips", { count: preview?.skip ?? 0 })
            }
          />
          {previewFilter === "skips" && unlistedSkips > 0 && visibleChanges.length > 0 && !preview?.truncated ? (
            <p className="shrink-0 text-sm text-muted-foreground">
              {t("modelPricing.sync.skipsUnlisted", { count: unlistedSkips })}
            </p>
          ) : null}
          <div className="max-h-[50dvh] min-h-0 overflow-y-auto rounded-lg border">
            {visibleChanges.length === 0 ? (
              <p className="px-6 py-8 text-center text-sm text-muted-foreground">
                {previewFilter === "skips" && unlistedSkips > 0
                  ? t("modelPricing.sync.skipsAllUnchanged")
                  : t("modelPricing.sync.previewEmpty")}
              </p>
            ) : (
              <table className="w-full caption-bottom text-sm">
                <thead className="sticky top-0 bg-background [&_tr]:border-b">
                  <tr className="border-b">
                    <VirtualTableHeaderCell>{t("modelPricing.model")}</VirtualTableHeaderCell>
                    <VirtualTableHeaderCell>{t("modelPricing.sync.changeKind")}</VirtualTableHeaderCell>
                    <VirtualTableHeaderCell>{t("modelPricing.sync.changedFields")}</VirtualTableHeaderCell>
                  </tr>
                </thead>
                <tbody className="[&_tr:last-child]:border-0">
                  {visibleChanges.map((change) => (
                    <tr key={`${change.kind}:${change.model_id}`} className="border-b">
                      <VirtualTableCell className="font-mono text-xs">{change.model_id}</VirtualTableCell>
                      <VirtualTableCell>
                        <Badge
                          variant={change.kind === "delete" ? "destructive" : "secondary"}
                          className="text-xs"
                        >
                          {t(`modelPricing.sync.kind.${change.kind}`)}
                        </Badge>
                      </VirtualTableCell>
                      <VirtualTableCell className="font-mono text-xs text-muted-foreground">
                        <ChangeDetail change={change} />
                      </VirtualTableCell>
                    </tr>
                  ))}
                </tbody>
              </table>
            )}
          </div>
          <DialogFooter className="shrink-0">
            <Button variant="outline" disabled={applying} onClick={() => setPreview(null)}>
              {t("common.cancel")}
            </Button>
            <Button
              variant={preview && preview.delete > 0 ? "destructive" : "default"}
              disabled={applying || changeCount === 0}
              onClick={() => {
                if (preview && preview.delete > 0) setConfirmDeleteOpen(true);
                else void handleApply();
              }}
            >
              {applying
                ? t("modelPricing.sync.applying")
                : preview && preview.delete > 0
                  ? t("modelPricing.sync.applyWithDeletes", {
                      count: changeCount,
                      deletes: preview.delete,
                    })
                  : t("modelPricing.sync.applyChanges", { count: changeCount })}
            </Button>
          </DialogFooter>
        </DialogContent>
      </Dialog>

      <AlertDialog
        open={confirmDeleteOpen}
        onOpenChange={(open) => !applying && setConfirmDeleteOpen(open)}
      >
        <AlertDialogContent>
          <AlertDialogHeader>
            <AlertDialogTitle>{t("modelPricing.sync.confirmDeleteTitle")}</AlertDialogTitle>
            <AlertDialogDescription>
              {t("modelPricing.sync.confirmDeleteDescription", {
                count: preview?.delete ?? 0,
                source: preview ? SOURCE_NAMES[preview.source] : "",
              })}
            </AlertDialogDescription>
          </AlertDialogHeader>
          <AlertDialogFooter>
            <AlertDialogCancel disabled={applying}>{t("common.cancel")}</AlertDialogCancel>
            <AlertDialogAction
              className={buttonVariants({ variant: "destructive" })}
              disabled={applying}
              onClick={(event) => {
                event.preventDefault();
                void handleApply();
              }}
            >
              {applying
                ? t("modelPricing.sync.applying")
                : t("modelPricing.sync.confirmDeleteAction", { count: preview?.delete ?? 0 })}
            </AlertDialogAction>
          </AlertDialogFooter>
        </AlertDialogContent>
      </AlertDialog>
    </div>
  );
}
