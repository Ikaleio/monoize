import { useMemo, useState } from "react";
import { useTranslation } from "react-i18next";
import {
  RefreshCw,
  Plus,
  Pencil,
  Trash2,
  Search,
  Database,
} from "lucide-react";
import { Button } from "@/components/ui/button";
import { Card, CardContent, CardHeader, CardTitle } from "@/components/ui/card";
import { Input } from "@/components/ui/input";
import { Label } from "@/components/ui/label";
import { Badge } from "@/components/ui/badge";
import { Skeleton } from "@/components/ui/skeleton";
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
  upsertModelMetadataOptimistic,
  deleteModelMetadataOptimistic,
  syncModelMetadata,
} from "@/lib/swr";
import type { ModelMetadataRecord, UpsertModelMetadataInput } from "@/lib/api";
import { PageWrapper, motion, transitions } from "@/components/ui/motion";
import { toast } from "sonner";
import { TableVirtuoso } from "react-virtuoso";

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
    } finally {
      setSaving(false);
    }
  };

  const handleDelete = async (modelId: string) => {
    if (!confirm(t("modelMetadata.deleteConfirm"))) return;
    try {
      await deleteModelMetadataOptimistic(modelId, records, (error) =>
        toast.error(t("modelMetadata.deleteFailed"), { description: error.message })
      );
      toast.success(t("modelMetadata.deleteSuccess"));
      setEditRecord(null);
    } catch {
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

  if (isLoading) {
    return (
      <div className="space-y-6">
        <Skeleton className="h-8 w-48" />
        <Skeleton className="h-12 w-full" />
        <Skeleton className="h-64 w-full" />
      </div>
    );
  }

  const editDialog = (isCreate: boolean, open: boolean, onOpenChange: (v: boolean) => void) => (
    <Dialog open={open} onOpenChange={onOpenChange}>
      <DialogContent className="max-w-lg">
        <DialogHeader>
          <DialogTitle>
            {isCreate ? t("modelMetadata.createModel") : t("modelMetadata.editModel")}
          </DialogTitle>
          <DialogDescription>
            {isCreate
              ? t("modelMetadata.description")
              : `${form.modelId}`}
          </DialogDescription>
        </DialogHeader>
        <div className="space-y-4 py-2 max-h-[60vh] overflow-y-auto">
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
        <DialogFooter className="flex justify-between">
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
      </DialogContent>
    </Dialog>
  );

  return (
    <PageWrapper className="space-y-6">
      <motion.div
        initial={{ opacity: 0, y: -10 }}
        animate={{ opacity: 1, y: 0 }}
        transition={transitions.normal}
        className="flex items-center justify-between"
      >
        <div>
          <h1 className="text-3xl font-bold tracking-tight">{t("modelMetadata.title")}</h1>
          <p className="text-muted-foreground">{t("modelMetadata.description")}</p>
        </div>
        <div className="flex items-center gap-2">
          <motion.div whileHover={{ scale: 1.02 }} whileTap={{ scale: 0.98 }}>
            <Button variant="outline" onClick={handleSync} disabled={syncing}>
              <RefreshCw className={`mr-2 h-4 w-4 ${syncing ? "animate-spin" : ""}`} />
              {syncing ? t("modelMetadata.syncing") : t("modelMetadata.syncModelsDev")}
            </Button>
          </motion.div>
          <motion.div whileHover={{ scale: 1.02 }} whileTap={{ scale: 0.98 }}>
            <Button onClick={openCreate}>
              <Plus className="mr-2 h-4 w-4" />
              {t("modelMetadata.addModel")}
            </Button>
          </motion.div>
        </div>
      </motion.div>

      {editDialog(false, !!editRecord, (open) => !open && setEditRecord(null))}
      {editDialog(true, createOpen, setCreateOpen)}

      <motion.div
        initial={{ opacity: 0, y: 20 }}
        animate={{ opacity: 1, y: 0 }}
        transition={{ delay: 0.1, ...transitions.normal }}
      >
        <Card>
          <CardHeader className="pb-3">
            <div className="flex items-center justify-between">
              <CardTitle className="flex items-center gap-2">
                <Database className="h-5 w-5" />
                {t("modelMetadata.title")}
              </CardTitle>
              <div className="relative w-64">
                <Search className="absolute left-2.5 top-2.5 h-4 w-4 text-muted-foreground" />
                <Input
                  value={search}
                  onChange={(e) => setSearch(e.target.value)}
                  placeholder={t("modelMetadata.searchPlaceholder")}
                  className="pl-9"
                />
              </div>
            </div>
          </CardHeader>
          <CardContent>
            {filtered.length === 0 ? (
              <div className="flex flex-col items-center justify-center py-12 text-center">
                <Database className="h-12 w-12 text-muted-foreground/30 mb-4" />
                <p className="text-lg font-medium text-muted-foreground">
                  {t("modelMetadata.noModels")}
                </p>
                <p className="text-sm text-muted-foreground/70 mt-1">
                  {t("modelMetadata.noModelsDesc")}
                </p>
              </div>
            ) : (
              <TableVirtuoso
                style={{ height: "calc(100vh - 280px)", minHeight: 400 }}
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
                    <th className="h-10 px-4 text-left align-middle font-medium text-muted-foreground min-w-[200px]">
                      {t("modelMetadata.modelId")}
                    </th>
                    <th className="h-10 px-4 text-left align-middle font-medium text-muted-foreground">
                      {t("modelMetadata.inputCost")}
                    </th>
                    <th className="h-10 px-4 text-left align-middle font-medium text-muted-foreground">
                      {t("modelMetadata.outputCost")}
                    </th>
                    <th className="h-10 px-4 text-left align-middle font-medium text-muted-foreground">
                      {t("modelMetadata.context")}
                    </th>
                    <th className="h-10 px-4 text-left align-middle font-medium text-muted-foreground">
                      {t("modelMetadata.source")}
                    </th>
                    <th className="h-10 px-4 text-left align-middle font-medium text-muted-foreground">
                      {t("modelMetadata.updated")}
                    </th>
                    <th className="h-10 px-4 text-left align-middle font-medium text-muted-foreground w-[80px]">
                      {t("common.actions")}
                    </th>
                  </tr>
                )}
                itemContent={(_index, record) => (
                  <>
                    <td
                      className="p-4 align-middle"
                      onClick={() => openEdit(record)}
                    >
                      <ModelBadge
                        model={record.model_id}
                        provider={record.models_dev_provider}
                        showDetails={false}
                      />
                    </td>
                    <td
                      className="p-4 align-middle font-mono text-xs"
                      onClick={() => openEdit(record)}
                    >
                      {nanoToPerMillion(record.input_cost_per_token_nano)}
                    </td>
                    <td
                      className="p-4 align-middle font-mono text-xs"
                      onClick={() => openEdit(record)}
                    >
                      {nanoToPerMillion(record.output_cost_per_token_nano)}
                    </td>
                    <td
                      className="p-4 align-middle font-mono text-xs"
                      onClick={() => openEdit(record)}
                    >
                      {formatTokens(record.max_tokens)}
                    </td>
                    <td
                      className="p-4 align-middle"
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
                    </td>
                    <td
                      className="p-4 align-middle text-xs text-muted-foreground"
                      onClick={() => openEdit(record)}
                    >
                      {formatRelativeTime(record.updated_at)}
                    </td>
                    <td className="p-4 align-middle">
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
                    </td>
                  </>
                )}
              />
            )}
          </CardContent>
        </Card>
      </motion.div>
    </PageWrapper>
  );
}
