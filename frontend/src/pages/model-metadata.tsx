import { useEffect, useMemo, useState } from "react";
import { useTranslation } from "react-i18next";
import {
  RefreshCw,
  Plus,
  Pencil,
  Trash2,
  Database,
  TableProperties,
  SlidersHorizontal,
  ArrowUp,
  ArrowDown,
  Save,
} from "lucide-react";
import { Button } from "@/components/ui/button";
import { Input } from "@/components/ui/input";
import { Label } from "@/components/ui/label";
import { Badge } from "@/components/ui/badge";
import { Skeleton } from "@/components/ui/skeleton";
import { Switch } from "@/components/ui/switch";
import { Textarea } from "@/components/ui/textarea";
import { Tabs, TabsContent, TabsList, TabsTrigger } from "@/components/ui/tabs";
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
  Select,
  SelectContent,
  SelectItem,
  SelectTrigger,
  SelectValue,
} from "@/components/ui/select";
import {
  Dialog,
  DialogContent,
  DialogDescription,
  DialogFooter,
  DialogHeader,
  DialogTitle,
} from "@/components/ui/dialog";
import { ModelBadge } from "@/components/ModelBadge";
import {
  useModelMetadata,
  useBillingRates,
  usePricingProfilePatterns,
  upsertModelMetadataOptimistic,
  deleteModelMetadataOptimistic,
  syncModelMetadata,
  upsertBillingRateOptimistic,
  deleteBillingRateOptimistic,
  syncBillingRatesCatalog,
  updatePricingProfilePatternsOptimistic,
} from "@/lib/swr";
import type {
  BillingRateRecord,
  PricingProfilePattern,
  ModelMetadataRecord,
  UpsertBillingRateInput,
  UpsertModelMetadataInput,
} from "@/lib/api";
import { PageWrapper, motion, transitions } from "@/components/ui/motion";
import { EmptyState } from "@/components/ui/empty-state";
import { PageHeader } from "@/components/ui/page-header";
import { TablePageSkeleton } from "@/components/ui/page-skeleton";
import { DataTableShell, TableToolbarSearch, VirtualTableCell, VirtualTableHeaderCell } from "@/components/ui/data-table-shell";
import { toast } from "sonner";
import { TableVirtuoso } from "react-virtuoso";
import { BillingProfilesTab } from "./model-metadata/BillingProfilesTab";

function nanoToPerMillion(nano?: string | null): string {
  if (!nano) return "-";
  const n = Number(nano);
  if (!Number.isFinite(n)) return "-";
  const perM = n / 1000;
  if (perM === 0) return "$0";
  if (perM < 0.0001) return `$${perM.toFixed(6)}`;
  return `$${perM.toFixed(4)}`;
}

function perMillionToNano(value: string): string | null {
  if (!value.trim()) return null;
  const n = Number(value);
  if (!Number.isFinite(n)) return null;
  return Math.trunc(n * 1000).toString();
}

function nanoToPerMillionInput(nano?: string | null): string {
  if (!nano) return "";
  const n = Number(nano);
  if (!Number.isFinite(n)) return "";
  return (n / 1000).toString();
}

function formatTokens(tokens?: number | null): string {
  if (tokens == null) return "-";
  if (tokens >= 1_000_000) return `${(tokens / 1_000_000).toFixed(1)}M`;
  if (tokens >= 1_000) return `${(tokens / 1_000).toFixed(0)}K`;
  return tokens.toString();
}

function formatRelativeTime(iso: string): string {
  const diff = Date.now() - new Date(iso).getTime();
  const mins = Math.floor(diff / 60_000);
  if (mins < 1) return "just now";
  if (mins < 60) return `${mins}m ago`;
  const hours = Math.floor(mins / 60);
  if (hours < 24) return `${hours}h ago`;
  const days = Math.floor(hours / 24);
  if (days < 30) return `${days}d ago`;
  return new Date(iso).toLocaleDateString();
}

interface EditFormData {
  modelId: string;
  modelsDevProvider: string;
  mode: string;
  inputCostPerM: string;
  outputCostPerM: string;
  cacheReadCostPerM: string;
  cacheWriteCostPerM: string;
  reasoningCostPerM: string;
  maxInputTokens: string;
  maxOutputTokens: string;
  maxTokens: string;
}

const emptyForm: EditFormData = {
  modelId: "",
  modelsDevProvider: "",
  mode: "chat",
  inputCostPerM: "",
  outputCostPerM: "",
  cacheReadCostPerM: "",
  cacheWriteCostPerM: "",
  reasoningCostPerM: "",
  maxInputTokens: "",
  maxOutputTokens: "",
  maxTokens: "",
};

function recordToForm(r: ModelMetadataRecord): EditFormData {
  return {
    modelId: r.model_id,
    modelsDevProvider: r.models_dev_provider ?? "",
    mode: r.mode ?? "chat",
    inputCostPerM: nanoToPerMillionInput(r.input_cost_per_token_nano),
    outputCostPerM: nanoToPerMillionInput(r.output_cost_per_token_nano),
    cacheReadCostPerM: nanoToPerMillionInput(r.cache_read_input_cost_per_token_nano),
    cacheWriteCostPerM: nanoToPerMillionInput(r.cache_creation_input_cost_per_token_nano),
    reasoningCostPerM: nanoToPerMillionInput(r.output_cost_per_reasoning_token_nano),
    maxInputTokens: r.max_input_tokens?.toString() ?? "",
    maxOutputTokens: r.max_output_tokens?.toString() ?? "",
    maxTokens: r.max_tokens?.toString() ?? "",
  };
}

function formToInput(form: EditFormData): UpsertModelMetadataInput {
  return {
    models_dev_provider: form.modelsDevProvider || null,
    mode: form.mode || null,
    input_cost_per_token_nano: perMillionToNano(form.inputCostPerM),
    output_cost_per_token_nano: perMillionToNano(form.outputCostPerM),
    cache_read_input_cost_per_token_nano: perMillionToNano(form.cacheReadCostPerM),
    cache_creation_input_cost_per_token_nano: perMillionToNano(form.cacheWriteCostPerM),
    output_cost_per_reasoning_token_nano: perMillionToNano(form.reasoningCostPerM),
    max_input_tokens: form.maxInputTokens ? Number(form.maxInputTokens) : null,
    max_output_tokens: form.maxOutputTokens ? Number(form.maxOutputTokens) : null,
    max_tokens: form.maxTokens ? Number(form.maxTokens) : null,
  };
}

interface ProviderVariant {
  provider: string;
  inputCostPerM: string;
  outputCostPerM: string;
  cacheReadCostPerM: string;
  cacheWriteCostPerM: string;
  reasoningCostPerM: string;
  maxInputTokens: string;
  maxOutputTokens: string;
  maxTokens: string;
}

function extractProviderVariants(rawJson: Record<string, unknown>): ProviderVariant[] {
  const providers = rawJson?.providers;
  if (!providers || typeof providers !== "object") return [];
  const result: ProviderVariant[] = [];
  for (const [provider, val] of Object.entries(providers as Record<string, unknown>)) {
    if (!val || typeof val !== "object") continue;
    const obj = val as Record<string, unknown>;
    const cost = obj.cost as Record<string, unknown> | undefined;
    const limit = obj.limit as Record<string, unknown> | undefined;
    const costStr = (v: unknown): string => {
      if (v == null) return "";
      const n = Number(v);
      if (!Number.isFinite(n)) return "";
      return n.toString();
    };
    const toStr = (v: unknown): string => (v != null ? String(v) : "");
    result.push({
      provider,
      inputCostPerM: costStr(cost?.input),
      outputCostPerM: costStr(cost?.output),
      cacheReadCostPerM: costStr(cost?.cache_read),
      cacheWriteCostPerM: costStr(cost?.cache_write),
      reasoningCostPerM: costStr(cost?.reasoning),
      maxInputTokens: toStr(limit?.input),
      maxOutputTokens: toStr(limit?.output),
      maxTokens: toStr(limit?.context),
    });
  }
  return result.sort((a, b) => a.provider.localeCompare(b.provider));
}

function applyVariantToForm(form: EditFormData, variant: ProviderVariant): EditFormData {
  return {
    ...form,
    modelsDevProvider: variant.provider,
    inputCostPerM: variant.inputCostPerM,
    outputCostPerM: variant.outputCostPerM,
    cacheReadCostPerM: variant.cacheReadCostPerM,
    cacheWriteCostPerM: variant.cacheWriteCostPerM,
    reasoningCostPerM: variant.reasoningCostPerM,
    maxInputTokens: variant.maxInputTokens,
    maxOutputTokens: variant.maxOutputTokens,
    maxTokens: variant.maxTokens,
  };
}

export function ModelMetadataPage() {
  const { t } = useTranslation();
  const { data: records = [], isLoading } = useModelMetadata();
  const [search, setSearch] = useState("");
  const [syncing, setSyncing] = useState(false);
  const [saving, setSaving] = useState(false);
  const [editRecord, setEditRecord] = useState<ModelMetadataRecord | null>(null);
  const [createOpen, setCreateOpen] = useState(false);
  const [form, setForm] = useState<EditFormData>(emptyForm);
  const [deleteTargetId, setDeleteTargetId] = useState<string | null>(null);

  const filtered = records.filter((r) =>
    r.model_id.toLowerCase().includes(search.toLowerCase())
  );

  const providerVariants = useMemo(() => {
    if (!editRecord?.raw_json) return [];
    return extractProviderVariants(editRecord.raw_json);
  }, [editRecord?.raw_json]);

  const handleSync = async () => {
    setSyncing(true);
    try {
      const result = await syncModelMetadata((error) =>
        toast.error(t("modelMetadata.syncFailed"), { description: error.message })
      );
      toast.success(
        t("modelMetadata.syncSuccess", {
          upserted: result.upserted,
          skipped: result.skipped,
        })
      );
    } catch {
      return
    } finally {
      setSyncing(false);
    }
  };

  const handleSave = async (isCreate: boolean) => {
    const modelId = isCreate ? form.modelId.trim() : editRecord?.model_id;
    if (!modelId) return;
    setSaving(true);
    try {
      await upsertModelMetadataOptimistic(
        modelId,
        formToInput(form),
        records,
        (error) =>
          toast.error(t("modelMetadata.saveFailed"), { description: error.message })
      );
      toast.success(t("modelMetadata.saveSuccess"));
      setEditRecord(null);
      setCreateOpen(false);
      setForm(emptyForm);
    } catch {
      return
    } finally {
      setSaving(false);
    }
  };

  const handleDelete = async (modelId: string) => {
    setDeleteTargetId(modelId);
  };

  const confirmDelete = async () => {
    if (!deleteTargetId) return;
    try {
      await deleteModelMetadataOptimistic(deleteTargetId, records, (error) =>
        toast.error(t("modelMetadata.deleteFailed"), { description: error.message })
      );
      toast.success(t("modelMetadata.deleteSuccess"));
      setEditRecord(null);
    } catch {
      return
    } finally {
      setDeleteTargetId(null);
    }
  };

  const openEdit = (record: ModelMetadataRecord) => {
    setEditRecord(record);
    setForm(recordToForm(record));
  };

  const openCreate = () => {
    setCreateOpen(true);
    setForm(emptyForm);
  };

  const editDialog = (isCreate: boolean, open: boolean, onOpenChange: (v: boolean) => void) => (
    <Dialog open={open} onOpenChange={onOpenChange}>
      <DialogContent className="max-h-[calc(100dvh-2rem)] overflow-hidden p-0 sm:max-h-[calc(100dvh-3rem)] sm:max-w-lg">
        <div className="flex min-h-0 flex-col p-6">
        <DialogHeader className="shrink-0">
          <DialogTitle>
            {isCreate ? t("modelMetadata.createModel") : t("modelMetadata.editModel")}
          </DialogTitle>
          <DialogDescription>
            {isCreate
              ? t("modelMetadata.description")
              : `${form.modelId}`}
          </DialogDescription>
        </DialogHeader>
        <div className="min-h-0 flex-1 space-y-4 overflow-y-auto py-2 pr-1">
          {isCreate && (
            <div className="space-y-2">
              <Label>{t("modelMetadata.modelId")}</Label>
              <Input
                value={form.modelId}
                onChange={(e) => setForm({ ...form, modelId: e.target.value })}
                placeholder={t("modelMetadata.modelIdPlaceholder")}
              />
            </div>
          )}
          <div className="space-y-2">
            <Label>{t("modelMetadata.provider")}</Label>
            <Input
              value={form.modelsDevProvider}
              onChange={(e) => setForm({ ...form, modelsDevProvider: e.target.value })}
              placeholder="e.g., openai"
            />
          </div>
          {!isCreate && providerVariants.length > 0 && (
            <div className="space-y-2">
              <Label>{t("modelMetadata.providerSource")}</Label>
              <Select
                value={form.modelsDevProvider}
                onValueChange={(provider) => {
                  const variant = providerVariants.find((v) => v.provider === provider);
                  if (variant) {
                    setForm(applyVariantToForm(form, variant));
                  }
                }}
              >
                <SelectTrigger>
                  <SelectValue placeholder="Select provider source" />
                </SelectTrigger>
                <SelectContent>
                  {providerVariants.map((variant) => (
                    <SelectItem key={variant.provider} value={variant.provider}>
                      <span className="font-medium">{variant.provider}</span>
                      {variant.inputCostPerM && (
                        <span className="text-xs text-muted-foreground ml-2">
                          In: ${variant.inputCostPerM}
                        </span>
                      )}
                      {variant.outputCostPerM && (
                        <span className="text-xs text-muted-foreground ml-1">
                          Out: ${variant.outputCostPerM}
                        </span>
                      )}
                    </SelectItem>
                  ))}
                </SelectContent>
              </Select>
            </div>
          )}
          <div className="space-y-2">
            <Label>{t("modelMetadata.mode")}</Label>
            <Input
              value={form.mode}
              onChange={(e) => setForm({ ...form, mode: e.target.value })}
              placeholder="chat"
            />
          </div>
          <div className="space-y-1">
            <Label className="text-muted-foreground text-xs">{t("modelMetadata.pricing")}</Label>
            <div className="grid grid-cols-2 gap-3">
              <div className="space-y-1">
                <Label className="text-xs">{t("modelMetadata.inputCost")}</Label>
                <Input
                  type="number"
                  step="any"
                  value={form.inputCostPerM}
                  onChange={(e) => setForm({ ...form, inputCostPerM: e.target.value })}
                  placeholder="0"
                />
              </div>
              <div className="space-y-1">
                <Label className="text-xs">{t("modelMetadata.outputCost")}</Label>
                <Input
                  type="number"
                  step="any"
                  value={form.outputCostPerM}
                  onChange={(e) => setForm({ ...form, outputCostPerM: e.target.value })}
                  placeholder="0"
                />
              </div>
              <div className="space-y-1">
                <Label className="text-xs">{t("modelMetadata.cacheReadCost")}</Label>
                <Input
                  type="number"
                  step="any"
                  value={form.cacheReadCostPerM}
                  onChange={(e) => setForm({ ...form, cacheReadCostPerM: e.target.value })}
                  placeholder="0"
                />
              </div>
              <div className="space-y-1">
                <Label className="text-xs">{t("modelMetadata.cacheWriteCost")}</Label>
                <Input
                  type="number"
                  step="any"
                  value={form.cacheWriteCostPerM}
                  onChange={(e) => setForm({ ...form, cacheWriteCostPerM: e.target.value })}
                  placeholder="0"
                />
              </div>
              <div className="space-y-1">
                <Label className="text-xs">{t("modelMetadata.reasoningCost")}</Label>
                <Input
                  type="number"
                  step="any"
                  value={form.reasoningCostPerM}
                  onChange={(e) => setForm({ ...form, reasoningCostPerM: e.target.value })}
                  placeholder="0"
                />
              </div>
            </div>
          </div>
          <div className="space-y-1">
            <Label className="text-muted-foreground text-xs">{t("modelMetadata.limits")}</Label>
            <div className="grid grid-cols-3 gap-3">
              <div className="space-y-1">
                <Label className="text-xs">{t("modelMetadata.context")}</Label>
                <Input
                  type="number"
                  value={form.maxTokens}
                  onChange={(e) => setForm({ ...form, maxTokens: e.target.value })}
                  placeholder="128000"
                />
              </div>
              <div className="space-y-1">
                <Label className="text-xs">{t("modelMetadata.maxInput")}</Label>
                <Input
                  type="number"
                  value={form.maxInputTokens}
                  onChange={(e) => setForm({ ...form, maxInputTokens: e.target.value })}
                  placeholder="128000"
                />
              </div>
              <div className="space-y-1">
                <Label className="text-xs">{t("modelMetadata.maxOutput")}</Label>
                <Input
                  type="number"
                  value={form.maxOutputTokens}
                  onChange={(e) => setForm({ ...form, maxOutputTokens: e.target.value })}
                  placeholder="16384"
                />
              </div>
            </div>
          </div>
        </div>
        <DialogFooter className="shrink-0 pt-4 sm:justify-between">
          {!isCreate && editRecord && (
            <Button
              variant="destructive"
              size="sm"
              onClick={() => handleDelete(editRecord.model_id)}
            >
              <Trash2 className="mr-1 h-3.5 w-3.5" />
              {t("modelMetadata.deleteModel")}
            </Button>
          )}
          <div className="flex gap-2 ml-auto">
            <Button variant="outline" onClick={() => onOpenChange(false)}>
              {t("common.cancel")}
            </Button>
            <Button
              onClick={() => handleSave(isCreate)}
              disabled={saving || (isCreate && !form.modelId.trim())}
            >
              {saving ? t("common.saving") : t("common.save")}
            </Button>
          </div>
        </DialogFooter>
        </div>
      </DialogContent>
    </Dialog>
  );

  return (
    <PageWrapper className="space-y-6">
      <motion.div
        initial={{ opacity: 0, y: -10 }}
        animate={{ opacity: 1, y: 0 }}
        transition={transitions.normal}
      >
        <PageHeader title={t("modelMetadata.title")} description={t("modelMetadata.description")} />
      </motion.div>

      {editDialog(false, !!editRecord, (open) => !open && setEditRecord(null))}
      {editDialog(true, createOpen, setCreateOpen)}

      <AlertDialog open={!!deleteTargetId} onOpenChange={(open) => { if (!open) setDeleteTargetId(null); }}>
        <AlertDialogContent>
          <AlertDialogHeader>
            <AlertDialogTitle>{t("modelMetadata.deleteModel")}</AlertDialogTitle>
            <AlertDialogDescription>{t("modelMetadata.deleteConfirm")}</AlertDialogDescription>
          </AlertDialogHeader>
          <AlertDialogFooter>
            <AlertDialogCancel>{t("common.cancel")}</AlertDialogCancel>
            <AlertDialogAction className="bg-destructive text-destructive-foreground hover:bg-destructive/90" onClick={confirmDelete}>
              {t("common.delete")}
            </AlertDialogAction>
          </AlertDialogFooter>
        </AlertDialogContent>
      </AlertDialog>

      <Tabs defaultValue="models" className="space-y-4">
        <TabsList className="max-w-full justify-start overflow-x-auto">
          <TabsTrigger value="models">
            <Database className="mr-2 h-4 w-4" />
            {t("modelMetadata.tabs.modelDatabase", "Model Database")}
          </TabsTrigger>
          <TabsTrigger value="billing-profiles">
            <SlidersHorizontal className="mr-2 h-4 w-4" />
            {t("modelMetadata.tabs.billingProfiles", "Billing Profiles")}
          </TabsTrigger>
          <TabsTrigger value="advanced-rates">
            <TableProperties className="mr-2 h-4 w-4" />
            {t("modelMetadata.tabs.advancedRates", "Advanced Rates")}
          </TabsTrigger>
        </TabsList>

        <TabsContent value="models" className="mt-0">
          <motion.div
            initial={{ opacity: 0, y: 20 }}
            animate={{ opacity: 1, y: 0 }}
            transition={{ delay: 0.1, ...transitions.normal }}
          >
            {isLoading ? (
              <TablePageSkeleton showToolbar />
            ) : (
              <DataTableShell
                toolbar={(
                  <>
                    <div className="flex items-center gap-2 text-base font-semibold">
                      <Database className="h-5 w-5" />
                      {t("modelMetadata.title")}
                    </div>
                    <TableToolbarSearch
                      value={search}
                      onChange={(e) => setSearch(e.target.value)}
                      placeholder={t("modelMetadata.searchPlaceholder")}
                    />
                    <div className="ml-auto flex items-center gap-2">
                      <Button variant="outline" onClick={handleSync} disabled={syncing}>
                        <RefreshCw className={`mr-2 h-4 w-4 ${syncing ? "animate-spin" : ""}`} />
                        {syncing ? t("modelMetadata.syncing") : t("modelMetadata.syncModelsDev")}
                      </Button>
                      <Button onClick={openCreate}>
                        <Plus className="mr-2 h-4 w-4" />
                        {t("modelMetadata.addModel")}
                      </Button>
                    </div>
                  </>
                )}
                isEmpty={filtered.length === 0}
                emptyState={(
                  <EmptyState
                    icon={<Database className="h-12 w-12" />}
                    title={t("modelMetadata.noModels")}
                    description={t("modelMetadata.noModelsDesc")}
                  />
                )}
              >
              <TableVirtuoso
                style={{ height: "calc(100dvh - 280px)", minHeight: 400 }}
                data={filtered}
                components={{
                  Table: (props) => (
                    <table
                      {...props}
                      className="w-full caption-bottom text-sm"
                    />
                  ),
                  TableHead: (props) => (
                    <thead {...props} className="[&_tr]:border-b" />
                  ),
                  TableRow: (props) => (
                    <tr
                      {...props}
                      className="border-b transition-colors hover:bg-muted/50 cursor-pointer"
                    />
                  ),
                  TableBody: (props) => (
                    <tbody {...props} className="[&_tr:last-child]:border-0" />
                  ),
                }}
                fixedHeaderContent={() => (
                  <tr className="border-b bg-background">
                    <VirtualTableHeaderCell className="min-w-[200px]">
                      {t("modelMetadata.modelId")}
                    </VirtualTableHeaderCell>
                    <VirtualTableHeaderCell>
                      {t("modelMetadata.inputCost")}
                    </VirtualTableHeaderCell>
                    <VirtualTableHeaderCell>
                      {t("modelMetadata.outputCost")}
                    </VirtualTableHeaderCell>
                    <VirtualTableHeaderCell>
                      {t("modelMetadata.context")}
                    </VirtualTableHeaderCell>
                    <VirtualTableHeaderCell>
                      {t("modelMetadata.source")}
                    </VirtualTableHeaderCell>
                    <VirtualTableHeaderCell>
                      {t("modelMetadata.updated")}
                    </VirtualTableHeaderCell>
                    <VirtualTableHeaderCell className="w-[80px]">
                      {t("common.actions")}
                    </VirtualTableHeaderCell>
                  </tr>
                )}
                itemContent={(_index, record) => (
                  <>
                    <VirtualTableCell
                      onClick={() => openEdit(record)}
                    >
                      <ModelBadge
                        model={record.model_id}
                        provider={record.models_dev_provider}
                        showDetails={false}
                      />
                    </VirtualTableCell>
                    <VirtualTableCell
                      className="font-mono text-xs"
                      onClick={() => openEdit(record)}
                    >
                      {nanoToPerMillion(record.input_cost_per_token_nano)}
                    </VirtualTableCell>
                    <VirtualTableCell
                      className="font-mono text-xs"
                      onClick={() => openEdit(record)}
                    >
                      {nanoToPerMillion(record.output_cost_per_token_nano)}
                    </VirtualTableCell>
                    <VirtualTableCell
                      className="font-mono text-xs"
                      onClick={() => openEdit(record)}
                    >
                      {formatTokens(record.max_tokens)}
                    </VirtualTableCell>
                    <VirtualTableCell
                      onClick={() => openEdit(record)}
                    >
                      <Badge
                        variant={record.source === "manual" ? "default" : "secondary"}
                        className="text-xs"
                      >
                        {record.source === "manual"
                          ? t("modelMetadata.manual")
                          : t("modelMetadata.modelsDev")}
                      </Badge>
                    </VirtualTableCell>
                    <VirtualTableCell
                      className="text-xs text-muted-foreground"
                      onClick={() => openEdit(record)}
                    >
                      {formatRelativeTime(record.updated_at)}
                    </VirtualTableCell>
                    <VirtualTableCell>
                      <div className="flex items-center gap-1">
                        <Button
                          variant="ghost"
                          size="icon"
                          onClick={(e) => {
                            e.stopPropagation();
                            openEdit(record);
                          }}
                        >
                          <Pencil className="h-4 w-4" />
                        </Button>
                        <Button
                          variant="ghost"
                          size="icon"
                          className="text-destructive hover:text-destructive"
                          onClick={(e) => {
                            e.stopPropagation();
                            handleDelete(record.model_id);
                          }}
                        >
                          <Trash2 className="h-4 w-4" />
                        </Button>
                      </div>
                    </VirtualTableCell>
                  </>
                )}
              />
              </DataTableShell>
            )}
          </motion.div>
        </TabsContent>
        <TabsContent value="billing-profiles" className="mt-0">
          <BillingProfilesTab />
        </TabsContent>
        <TabsContent value="advanced-rates" className="mt-0">
          <BillingRatesTab />
        </TabsContent>
      </Tabs>
    </PageWrapper>
  );
}

interface BillingRateFormData {
  id: string;
  source: string;
  pricingProfile: string;
  modelPattern: string;
  providerType: string;
  rateKind: string;
  usageClass: string;
  unit: string;
  unitPriceNanoUsd: string;
  contextTier: string;
  serviceTier: string;
  modality: string;
  cacheTtl: string;
  matchJson: string;
  priority: string;
  enabled: boolean;
  rawJson: string;
}

const emptyBillingRateForm: BillingRateFormData = {
  id: "",
  source: "manual",
  pricingProfile: "",
  modelPattern: "",
  providerType: "",
  rateKind: "token",
  usageClass: "",
  unit: "token",
  unitPriceNanoUsd: "",
  contextTier: "",
  serviceTier: "",
  modality: "",
  cacheTtl: "",
  matchJson: "{}",
  priority: "0",
  enabled: true,
  rawJson: "{}",
};

function nullableText(value: string): string | null {
  const trimmed = value.trim();
  return trimmed ? trimmed : null;
}

function jsonObjectText(value: Record<string, unknown>): string {
  return JSON.stringify(value ?? {}, null, 2);
}

function billingRateToForm(rate: BillingRateRecord): BillingRateFormData {
  return {
    id: rate.id,
    source: rate.source === "manual" ? rate.source : "manual",
    pricingProfile: rate.pricing_profile,
    modelPattern: rate.model_pattern ?? "",
    providerType: rate.provider_type ?? "",
    rateKind: rate.rate_kind,
    usageClass: rate.usage_class,
    unit: rate.unit,
    unitPriceNanoUsd: rate.unit_price_nano_usd,
    contextTier: rate.context_tier ?? "",
    serviceTier: rate.service_tier ?? "",
    modality: rate.modality ?? "",
    cacheTtl: rate.cache_ttl ?? "",
    matchJson: jsonObjectText(rate.match_json),
    priority: rate.priority.toString(),
    enabled: rate.enabled,
    rawJson: jsonObjectText(rate.raw_json),
  };
}

function parseJsonObject(value: string, field: string): Record<string, unknown> {
  const parsed = JSON.parse(value || "{}");
  if (!parsed || typeof parsed !== "object" || Array.isArray(parsed)) {
    throw new Error(`${field} must be a JSON object`);
  }
  return parsed as Record<string, unknown>;
}

function formToBillingRateInput(form: BillingRateFormData): UpsertBillingRateInput {
  if (!form.id.trim()) throw new Error("id is required");
  if (!form.pricingProfile.trim()) throw new Error("pricing profile is required");
  if (!form.usageClass.trim()) throw new Error("usage class is required");
  if (!form.unit.trim()) throw new Error("unit is required");
  if (!/^-?\d+$/.test(form.unitPriceNanoUsd.trim())) {
    throw new Error("unit price must be an integer nano-USD string");
  }
  const priority = Number(form.priority || "0");
  if (!Number.isInteger(priority)) throw new Error("priority must be an integer");
  return {
    source: form.source.trim() || "manual",
    pricing_profile: form.pricingProfile.trim(),
    model_pattern: nullableText(form.modelPattern),
    provider_type: nullableText(form.providerType),
    rate_kind: form.rateKind.trim() || "token",
    usage_class: form.usageClass.trim(),
    unit: form.unit.trim(),
    unit_price_nano_usd: form.unitPriceNanoUsd.trim(),
    context_tier: nullableText(form.contextTier),
    service_tier: nullableText(form.serviceTier),
    modality: nullableText(form.modality),
    cache_ttl: nullableText(form.cacheTtl),
    match_json: parseJsonObject(form.matchJson, "match_json"),
    priority,
    enabled: form.enabled,
    raw_json: parseJsonObject(form.rawJson, "raw_json"),
  };
}

function rateMatchesSearch(rate: BillingRateRecord, search: string): boolean {
  const q = search.trim().toLowerCase();
  if (!q) return true;
  return [
    rate.id,
    rate.source,
    rate.pricing_profile,
    rate.model_pattern,
    rate.provider_type,
    rate.rate_kind,
    rate.usage_class,
    rate.unit,
    rate.context_tier,
    rate.service_tier,
    rate.modality,
    rate.cache_ttl,
  ]
    .filter(Boolean)
    .some((value) => String(value).toLowerCase().includes(q));
}

function BillingRatesTab() {
  const { t } = useTranslation();
  const { data: rates = [], isLoading } = useBillingRates();
  const [search, setSearch] = useState("");
  const [syncing, setSyncing] = useState(false);
  const [saving, setSaving] = useState(false);
  const [editRate, setEditRate] = useState<BillingRateRecord | null>(null);
  const [createOpen, setCreateOpen] = useState(false);
  const [deleteTargetId, setDeleteTargetId] = useState<string | null>(null);
  const [form, setForm] = useState<BillingRateFormData>(emptyBillingRateForm);

  const filtered = rates.filter((rate) => rateMatchesSearch(rate, search));

  const openCreate = () => {
    setForm(emptyBillingRateForm);
    setCreateOpen(true);
  };

  const openEdit = (rate: BillingRateRecord) => {
    setForm(billingRateToForm(rate));
    setEditRate(rate);
  };

  const handleSync = async () => {
    setSyncing(true);
    try {
      const result = await syncBillingRatesCatalog((error) =>
        toast.error(t("modelMetadata.billingRates.syncFailed", "Catalog sync failed"), {
          description: error.message,
        })
      );
      toast.success(
        t("modelMetadata.billingRates.syncSuccess", "Catalog synced", {
          upserted: result.upserted,
          skipped: result.skipped,
          deleted: result.deleted,
        })
      );
    } catch {
      return;
    } finally {
      setSyncing(false);
    }
  };

  const handleSave = async (isCreate: boolean) => {
    const id = isCreate ? form.id.trim() : editRate?.id;
    if (!id) return;
    let input: UpsertBillingRateInput;
    try {
      input = formToBillingRateInput({ ...form, id });
    } catch (error) {
      toast.error(t("modelMetadata.billingRates.invalidForm", "Invalid rate"), {
        description: error instanceof Error ? error.message : String(error),
      });
      return;
    }
    setSaving(true);
    try {
      await upsertBillingRateOptimistic(id, input, rates, (error) =>
        toast.error(t("modelMetadata.billingRates.saveFailed", "Save failed"), {
          description: error.message,
        })
      );
      toast.success(t("modelMetadata.billingRates.saveSuccess", "Billing rate saved"));
      setCreateOpen(false);
      setEditRate(null);
      setForm(emptyBillingRateForm);
    } catch {
      return;
    } finally {
      setSaving(false);
    }
  };

  const confirmDelete = async () => {
    if (!deleteTargetId) return;
    try {
      await deleteBillingRateOptimistic(deleteTargetId, rates, (error) =>
        toast.error(t("modelMetadata.billingRates.deleteFailed", "Delete failed"), {
          description: error.message,
        })
      );
      toast.success(t("modelMetadata.billingRates.deleteSuccess", "Billing rate deleted"));
      setEditRate(null);
    } catch {
      return;
    } finally {
      setDeleteTargetId(null);
    }
  };

  const rateDialog = (isCreate: boolean, open: boolean, onOpenChange: (v: boolean) => void) => (
    <Dialog open={open} onOpenChange={onOpenChange}>
      <DialogContent className="max-h-[calc(100dvh-2rem)] overflow-hidden p-0 sm:max-w-3xl">
        <div className="flex min-h-0 flex-col p-6">
          <DialogHeader className="shrink-0">
            <DialogTitle>
              {isCreate
                ? t("modelMetadata.billingRates.addRate", "Add Billing Rate")
                : t("modelMetadata.billingRates.editRate", "Edit Billing Rate")}
            </DialogTitle>
            <DialogDescription>{form.id || t("modelMetadata.billingRates.newRate", "New rate")}</DialogDescription>
          </DialogHeader>
          <div className="min-h-0 flex-1 space-y-4 overflow-y-auto py-2 pr-1">
            <div className="grid gap-3 md:grid-cols-3">
              <div className="space-y-1">
                <Label className="text-xs">ID</Label>
                <Input
                  value={form.id}
                  disabled={!isCreate}
                  onChange={(e) => setForm({ ...form, id: e.target.value })}
                  placeholder="openai:gpt-image-2:output:image"
                />
              </div>
              <div className="space-y-1">
                <Label className="text-xs">{t("modelMetadata.billingRates.source", "Source")}</Label>
                <Input value={form.source} onChange={(e) => setForm({ ...form, source: e.target.value })} />
              </div>
              <div className="space-y-1">
                <Label className="text-xs">{t("modelMetadata.billingRates.profile", "Profile")}</Label>
                <Input value={form.pricingProfile} onChange={(e) => setForm({ ...form, pricingProfile: e.target.value })} />
              </div>
              <div className="space-y-1">
                <Label className="text-xs">{t("modelMetadata.billingRates.modelPattern", "Model Pattern")}</Label>
                <Input value={form.modelPattern} onChange={(e) => setForm({ ...form, modelPattern: e.target.value })} placeholder="gpt-image-2" />
              </div>
              <div className="space-y-1">
                <Label className="text-xs">{t("modelMetadata.billingRates.providerType", "Provider Type")}</Label>
                <Input value={form.providerType} onChange={(e) => setForm({ ...form, providerType: e.target.value })} placeholder="responses" />
              </div>
              <div className="space-y-1">
                <Label className="text-xs">{t("modelMetadata.billingRates.rateKind", "Kind")}</Label>
                <Select value={form.rateKind} onValueChange={(value) => setForm({ ...form, rateKind: value })}>
                  <SelectTrigger><SelectValue /></SelectTrigger>
                  <SelectContent>
                    <SelectItem value="token">token</SelectItem>
                    <SelectItem value="meter">meter</SelectItem>
                  </SelectContent>
                </Select>
              </div>
              <div className="space-y-1">
                <Label className="text-xs">{t("modelMetadata.billingRates.usageClass", "Usage Class")}</Label>
                <Input value={form.usageClass} onChange={(e) => setForm({ ...form, usageClass: e.target.value })} placeholder="input_uncached" />
              </div>
              <div className="space-y-1">
                <Label className="text-xs">{t("modelMetadata.billingRates.unit", "Unit")}</Label>
                <Input value={form.unit} onChange={(e) => setForm({ ...form, unit: e.target.value })} placeholder="token" />
              </div>
              <div className="space-y-1">
                <Label className="text-xs">{t("modelMetadata.billingRates.unitPrice", "Nano-USD / Unit")}</Label>
                <Input value={form.unitPriceNanoUsd} onChange={(e) => setForm({ ...form, unitPriceNanoUsd: e.target.value })} placeholder="1000" />
              </div>
              <div className="space-y-1">
                <Label className="text-xs">{t("modelMetadata.billingRates.contextTier", "Context Tier")}</Label>
                <Input value={form.contextTier} onChange={(e) => setForm({ ...form, contextTier: e.target.value })} placeholder="short" />
              </div>
              <div className="space-y-1">
                <Label className="text-xs">{t("modelMetadata.billingRates.serviceTier", "Service Tier")}</Label>
                <Input value={form.serviceTier} onChange={(e) => setForm({ ...form, serviceTier: e.target.value })} />
              </div>
              <div className="space-y-1">
                <Label className="text-xs">{t("modelMetadata.billingRates.modality", "Modality")}</Label>
                <Input value={form.modality} onChange={(e) => setForm({ ...form, modality: e.target.value })} placeholder="image" />
              </div>
              <div className="space-y-1">
                <Label className="text-xs">{t("modelMetadata.billingRates.cacheTtl", "Cache TTL")}</Label>
                <Input value={form.cacheTtl} onChange={(e) => setForm({ ...form, cacheTtl: e.target.value })} placeholder="5m" />
              </div>
              <div className="space-y-1">
                <Label className="text-xs">{t("modelMetadata.billingRates.priority", "Priority")}</Label>
                <Input value={form.priority} onChange={(e) => setForm({ ...form, priority: e.target.value })} />
              </div>
              <div className="flex items-center gap-2 pt-6">
                <Switch checked={form.enabled} onCheckedChange={(enabled) => setForm({ ...form, enabled })} />
                <Label className="text-xs">{t("modelMetadata.billingRates.enabled", "Enabled")}</Label>
              </div>
            </div>
            <div className="grid gap-3 md:grid-cols-2">
              <div className="space-y-1">
                <Label className="text-xs">match_json</Label>
                <Textarea className="min-h-[140px] font-mono text-xs" value={form.matchJson} onChange={(e) => setForm({ ...form, matchJson: e.target.value })} />
              </div>
              <div className="space-y-1">
                <Label className="text-xs">raw_json</Label>
                <Textarea className="min-h-[140px] font-mono text-xs" value={form.rawJson} onChange={(e) => setForm({ ...form, rawJson: e.target.value })} />
              </div>
            </div>
          </div>
          <DialogFooter className="shrink-0 pt-4 sm:justify-between">
            {!isCreate && editRate && (
              <Button variant="destructive" size="sm" onClick={() => setDeleteTargetId(editRate.id)}>
                <Trash2 className="mr-1 h-3.5 w-3.5" />
                {t("common.delete")}
              </Button>
            )}
            <div className="ml-auto flex gap-2">
              <Button variant="outline" onClick={() => onOpenChange(false)}>{t("common.cancel")}</Button>
              <Button onClick={() => handleSave(isCreate)} disabled={saving || !form.id.trim()}>
                {saving ? t("common.saving") : t("common.save")}
              </Button>
            </div>
          </DialogFooter>
        </div>
      </DialogContent>
    </Dialog>
  );

  if (isLoading) {
    return <TablePageSkeleton showToolbar />;
  }

  return (
    <>
      {rateDialog(false, !!editRate, (open) => !open && setEditRate(null))}
      {rateDialog(true, createOpen, setCreateOpen)}
      <AlertDialog open={!!deleteTargetId} onOpenChange={(open) => { if (!open) setDeleteTargetId(null); }}>
        <AlertDialogContent>
          <AlertDialogHeader>
            <AlertDialogTitle>{t("modelMetadata.billingRates.deleteRate", "Delete Billing Rate")}</AlertDialogTitle>
            <AlertDialogDescription>{t("modelMetadata.billingRates.deleteConfirm", "This billing rate will be removed.")}</AlertDialogDescription>
          </AlertDialogHeader>
          <AlertDialogFooter>
            <AlertDialogCancel>{t("common.cancel")}</AlertDialogCancel>
            <AlertDialogAction className="bg-destructive text-destructive-foreground hover:bg-destructive/90" onClick={confirmDelete}>
              {t("common.delete")}
            </AlertDialogAction>
          </AlertDialogFooter>
        </AlertDialogContent>
      </AlertDialog>
      <DataTableShell
        toolbar={(
          <>
            <div className="flex items-center gap-2 text-base font-semibold">
              <TableProperties className="h-5 w-5" />
              {t("modelMetadata.tabs.billingRates", "Billing Rates")}
            </div>
            <TableToolbarSearch
              value={search}
              onChange={(e) => setSearch(e.target.value)}
              placeholder={t("modelMetadata.billingRates.search", "Search rates")}
            />
            <div className="ml-auto flex items-center gap-2">
              <Button variant="outline" onClick={handleSync} disabled={syncing}>
                <RefreshCw className={`mr-2 h-4 w-4 ${syncing ? "animate-spin" : ""}`} />
                {syncing
                  ? t("modelMetadata.billingRates.syncing", "Syncing")
                  : t("modelMetadata.billingRates.syncCatalog", "Sync Catalog")}
              </Button>
              <Button onClick={openCreate}>
                <Plus className="mr-2 h-4 w-4" />
                {t("modelMetadata.billingRates.addRate", "Add Billing Rate")}
              </Button>
            </div>
          </>
        )}
        isEmpty={filtered.length === 0}
        emptyState={(
          <EmptyState
            icon={<TableProperties className="h-12 w-12" />}
            title={t("modelMetadata.billingRates.noRates", "No billing rates")}
            description={t("modelMetadata.billingRates.noRatesDesc", "No rates match the current filter.")}
          />
        )}
      >
        <TableVirtuoso
          style={{ height: "calc(100dvh - 320px)", minHeight: 400 }}
          data={filtered}
          components={{
            Table: (props) => <table {...props} className="w-full caption-bottom text-sm" />,
            TableHead: (props) => <thead {...props} className="[&_tr]:border-b" />,
            TableRow: (props) => <tr {...props} className="border-b transition-colors hover:bg-muted/50" />,
            TableBody: (props) => <tbody {...props} className="[&_tr:last-child]:border-0" />,
          }}
          fixedHeaderContent={() => (
            <tr className="border-b bg-background">
              <VirtualTableHeaderCell className="min-w-[240px]">ID</VirtualTableHeaderCell>
              <VirtualTableHeaderCell>{t("modelMetadata.billingRates.profile", "Profile")}</VirtualTableHeaderCell>
              <VirtualTableHeaderCell>{t("modelMetadata.billingRates.match", "Match")}</VirtualTableHeaderCell>
              <VirtualTableHeaderCell>{t("modelMetadata.billingRates.class", "Class")}</VirtualTableHeaderCell>
              <VirtualTableHeaderCell>{t("modelMetadata.billingRates.price", "Price")}</VirtualTableHeaderCell>
              <VirtualTableHeaderCell>{t("modelMetadata.billingRates.dimensions", "Dimensions")}</VirtualTableHeaderCell>
              <VirtualTableHeaderCell>{t("modelMetadata.source")}</VirtualTableHeaderCell>
              <VirtualTableHeaderCell className="w-[80px]">{t("common.actions")}</VirtualTableHeaderCell>
            </tr>
          )}
          itemContent={(_index, rate) => (
            <>
              <VirtualTableCell className="font-mono text-xs" onClick={() => openEdit(rate)}>{rate.id}</VirtualTableCell>
              <VirtualTableCell className="font-mono text-xs" onClick={() => openEdit(rate)}>{rate.pricing_profile}</VirtualTableCell>
              <VirtualTableCell className="font-mono text-xs" onClick={() => openEdit(rate)}>
                {[rate.model_pattern, rate.provider_type].filter(Boolean).join(" / ") || "-"}
              </VirtualTableCell>
              <VirtualTableCell className="font-mono text-xs" onClick={() => openEdit(rate)}>
                {rate.rate_kind}:{rate.usage_class}
              </VirtualTableCell>
              <VirtualTableCell className="font-mono text-xs" onClick={() => openEdit(rate)}>
                {rate.unit_price_nano_usd} / {rate.unit}
              </VirtualTableCell>
              <VirtualTableCell className="font-mono text-xs" onClick={() => openEdit(rate)}>
                {[rate.context_tier, rate.service_tier, rate.modality, rate.cache_ttl].filter(Boolean).join(" / ") || "-"}
              </VirtualTableCell>
              <VirtualTableCell onClick={() => openEdit(rate)}>
                <div className="flex items-center gap-2">
                  <Badge variant={rate.source === "manual" ? "default" : "secondary"} className="text-xs">{rate.source}</Badge>
                  {!rate.enabled && <Badge variant="outline" className="text-xs">off</Badge>}
                </div>
              </VirtualTableCell>
              <VirtualTableCell>
                <div className="flex items-center gap-1">
                  <Button variant="ghost" size="icon" onClick={() => openEdit(rate)}>
                    <Pencil className="h-4 w-4" />
                  </Button>
                  <Button variant="ghost" size="icon" className="text-destructive hover:text-destructive" onClick={() => setDeleteTargetId(rate.id)}>
                    <Trash2 className="h-4 w-4" />
                  </Button>
                </div>
              </VirtualTableCell>
            </>
          )}
        />
      </DataTableShell>
    </>
  );
}

export function PricingProfilesTab() {
  const { t } = useTranslation();
  const { data: patterns = [], isLoading } = usePricingProfilePatterns();
  const [draft, setDraft] = useState<PricingProfilePattern[]>([]);
  const [dirty, setDirty] = useState(false);
  const [saving, setSaving] = useState(false);

  useEffect(() => {
    if (!dirty) {
      setDraft(patterns.map((p) => ({ ...p })));
    }
  }, [dirty, patterns]);

  const updateDraft = (next: PricingProfilePattern[]) => {
    setDraft(next);
    setDirty(true);
  };

  const invalid = draft.some((p) => !p.pattern.trim() || !p.pricing_profile.trim());

  const handleSave = async () => {
    if (invalid) {
      toast.error(t("modelMetadata.pricingProfiles.invalid", "Patterns and profiles are required"));
      return;
    }
    setSaving(true);
    try {
      await updatePricingProfilePatternsOptimistic(
        draft.map((p) => ({ pattern: p.pattern.trim(), pricing_profile: p.pricing_profile.trim() })),
        patterns,
        (error) => toast.error(t("modelMetadata.pricingProfiles.saveFailed", "Save failed"), { description: error.message })
      );
      setDirty(false);
      toast.success(t("modelMetadata.pricingProfiles.saveSuccess", "Pricing profiles saved"));
    } catch {
      return;
    } finally {
      setSaving(false);
    }
  };

  if (isLoading && draft.length === 0) {
    return (
      <div className="space-y-3">
        <Skeleton className="h-10 w-full" />
        <Skeleton className="h-64 w-full" />
      </div>
    );
  }

  return (
    <DataTableShell
      toolbar={(
        <>
          <div className="flex items-center gap-2 text-base font-semibold">
            <SlidersHorizontal className="h-5 w-5" />
            {t("modelMetadata.tabs.pricingProfiles", "Pricing Profiles")}
          </div>
          <div className="ml-auto flex items-center gap-2">
            <Button variant="outline" onClick={() => updateDraft([...draft, { pattern: "", pricing_profile: "" }])}>
              <Plus className="mr-2 h-4 w-4" />
              {t("modelMetadata.pricingProfiles.addPattern", "Add Pattern")}
            </Button>
            <Button onClick={handleSave} disabled={saving || invalid || !dirty}>
              <Save className="mr-2 h-4 w-4" />
              {saving ? t("common.saving") : t("common.save")}
            </Button>
          </div>
        </>
      )}
      isEmpty={draft.length === 0}
      emptyState={(
        <EmptyState
          icon={<SlidersHorizontal className="h-12 w-12" />}
          title={t("modelMetadata.pricingProfiles.noPatterns", "No pricing profiles")}
          description={t("modelMetadata.pricingProfiles.noPatternsDesc", "Add ordered glob patterns to choose billing profiles.")}
        />
      )}
    >
      <div className="overflow-x-auto">
        <table className="w-full caption-bottom text-sm">
          <thead className="[&_tr]:border-b">
            <tr className="border-b">
              <VirtualTableHeaderCell className="w-[72px]">{t("modelMetadata.pricingProfiles.order", "Order")}</VirtualTableHeaderCell>
              <VirtualTableHeaderCell>{t("modelMetadata.pricingProfiles.pattern", "Pattern")}</VirtualTableHeaderCell>
              <VirtualTableHeaderCell>{t("modelMetadata.pricingProfiles.profile", "Profile")}</VirtualTableHeaderCell>
              <VirtualTableHeaderCell className="w-[120px]">{t("common.actions")}</VirtualTableHeaderCell>
            </tr>
          </thead>
          <tbody className="[&_tr:last-child]:border-0">
            {draft.map((pattern, index) => (
              <tr key={index} className="border-b">
                <VirtualTableCell className="font-mono text-xs">{index + 1}</VirtualTableCell>
                <VirtualTableCell>
                  <Input
                    value={pattern.pattern}
                    onChange={(e) => updateDraft(draft.map((p, i) => i === index ? { ...p, pattern: e.target.value } : p))}
                    placeholder="gpt-*"
                  />
                </VirtualTableCell>
                <VirtualTableCell>
                  <Input
                    value={pattern.pricing_profile}
                    onChange={(e) => updateDraft(draft.map((p, i) => i === index ? { ...p, pricing_profile: e.target.value } : p))}
                    placeholder="openai"
                  />
                </VirtualTableCell>
                <VirtualTableCell>
                  <div className="flex items-center gap-1">
                    <Button
                      variant="ghost"
                      size="icon"
                      disabled={index === 0}
                      onClick={() => {
                        const next = [...draft];
                        [next[index - 1], next[index]] = [next[index], next[index - 1]];
                        updateDraft(next);
                      }}
                    >
                      <ArrowUp className="h-4 w-4" />
                    </Button>
                    <Button
                      variant="ghost"
                      size="icon"
                      disabled={index === draft.length - 1}
                      onClick={() => {
                        const next = [...draft];
                        [next[index + 1], next[index]] = [next[index], next[index + 1]];
                        updateDraft(next);
                      }}
                    >
                      <ArrowDown className="h-4 w-4" />
                    </Button>
                    <Button
                      variant="ghost"
                      size="icon"
                      className="text-destructive hover:text-destructive"
                      onClick={() => updateDraft(draft.filter((_p, i) => i !== index))}
                    >
                      <Trash2 className="h-4 w-4" />
                    </Button>
                  </div>
                </VirtualTableCell>
              </tr>
            ))}
          </tbody>
        </table>
      </div>
    </DataTableShell>
  );
}
