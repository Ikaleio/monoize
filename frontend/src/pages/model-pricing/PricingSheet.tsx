import { useEffect, useMemo, useState } from "react";
import { useTranslation } from "react-i18next";
import { ChevronRight, Lock, Trash2, X } from "lucide-react";
import { toast } from "sonner";
import { Badge } from "@/components/ui/badge";
import { Button } from "@/components/ui/button";
import { Input } from "@/components/ui/input";
import { Label } from "@/components/ui/label";
import { Switch } from "@/components/ui/switch";
import { Tabs, TabsList, TabsTrigger } from "@/components/ui/tabs";
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
  Sheet,
  SheetContent,
  SheetDescription,
  SheetHeader,
  SheetTitle,
} from "@/components/ui/sheet";
import type {
  BillingExprTier,
  BillingMode,
  ModelMetadataRecord,
  ModelPriceRecord,
  UpsertModelPriceInput,
} from "@/lib/api";
import {
  deleteModelPriceOptimistic,
  upsertModelMetadataOptimistic,
  upsertModelPriceOptimistic,
  useModelMetadata,
} from "@/lib/swr";
import {
  BILLING_MODES,
  PER_TOKEN_PRICE_FIELDS,
  isValidUsdDecimal,
  type PerTokenPriceField,
  type PricingSheetTarget,
} from "./shared";

interface TierRow {
  lte: string;
  input: string;
  output: string;
  cacheRead: string;
  cacheWrite: string;
  cacheWrite1h: string;
  reasoning: string;
}

interface PriceForm {
  billingMode: BillingMode;
  prices: Record<PerTokenPriceField, string>;
  perRequestUsd: string;
  tiers: TierRow[];
  lockedFields: string[];
  enabled: boolean;
}

interface MetadataForm {
  modelsDevProvider: string;
  mode: string;
  maxTokens: string;
  maxInputTokens: string;
  maxOutputTokens: string;
}

function emptyTier(): TierRow {
  return {
    lte: "",
    input: "",
    output: "",
    cacheRead: "",
    cacheWrite: "",
    cacheWrite1h: "",
    reasoning: "",
  };
}

function tierFromExpr(tier: BillingExprTier): TierRow {
  return {
    lte: tier.when_input_tokens_lte != null ? String(tier.when_input_tokens_lte) : "",
    input: tier.input_usd_per_1m ?? "",
    output: tier.output_usd_per_1m ?? "",
    cacheRead: tier.cache_read_usd_per_1m ?? "",
    cacheWrite: tier.cache_write_usd_per_1m ?? "",
    cacheWrite1h: tier.cache_write_1h_usd_per_1m ?? "",
    reasoning: tier.reasoning_usd_per_1m ?? "",
  };
}

function formFromRecord(record: ModelPriceRecord | null): PriceForm {
  return {
    billingMode: record?.billing_mode ?? "per_token",
    prices: Object.fromEntries(
      PER_TOKEN_PRICE_FIELDS.map((field) => [field, record?.[field] ?? ""])
    ) as Record<PerTokenPriceField, string>,
    perRequestUsd: record?.per_request_usd ?? "",
    tiers: record?.billing_expr?.tiers.map(tierFromExpr) ?? [emptyTier()],
    lockedFields: record?.locked_fields ?? [],
    enabled: record?.enabled ?? true,
  };
}

function metadataFormFromRecord(record: ModelMetadataRecord | undefined): MetadataForm {
  return {
    modelsDevProvider: record?.models_dev_provider ?? "",
    mode: record?.mode ?? "",
    maxTokens: record?.max_tokens?.toString() ?? "",
    maxInputTokens: record?.max_input_tokens?.toString() ?? "",
    maxOutputTokens: record?.max_output_tokens?.toString() ?? "",
  };
}

interface ProviderVariant {
  provider: string;
  input: string;
  output: string;
  cacheRead: string;
  cacheWrite: string;
  reasoning: string;
  mode: string;
  maxTokens: string;
  maxInputTokens: string;
  maxOutputTokens: string;
}

// models.dev variant prices are USD-per-1M strings inside metadata raw_json
// (model-metadata-dashboard.spec.md UI3); kept as strings end to end.
function extractVariants(rawJson: Record<string, unknown> | undefined): ProviderVariant[] {
  const providers = rawJson?.providers;
  if (!providers || typeof providers !== "object") return [];
  const priceStr = (value: unknown): string =>
    typeof value === "string" && isValidUsdDecimal(value) ? value : "";
  const integerStr = (value: unknown): string => {
    if (typeof value === "number" && Number.isSafeInteger(value) && value >= 0) {
      return String(value);
    }
    return typeof value === "string" && /^\d+$/.test(value) ? value : "";
  };
  return Object.entries(providers as Record<string, unknown>)
    .flatMap(([provider, value]) => {
      if (!value || typeof value !== "object") return [];
      const cost = (value as Record<string, unknown>).cost as
        | Record<string, unknown>
        | undefined;
      const limit = (value as Record<string, unknown>).limit as
        | Record<string, unknown>
        | undefined;
      const family = (value as Record<string, unknown>).family;
      return [
        {
          provider,
          input: priceStr(cost?.input),
          output: priceStr(cost?.output),
          cacheRead: priceStr(cost?.cache_read),
          cacheWrite: priceStr(cost?.cache_write),
          reasoning: priceStr(cost?.reasoning),
          mode:
            typeof family === "string" && family.toLowerCase().includes("embed")
              ? "embedding"
              : "chat",
          maxTokens: integerStr(limit?.context),
          maxInputTokens: integerStr(limit?.input),
          maxOutputTokens: integerStr(limit?.output),
        },
      ];
    })
    .sort((a, b) => a.provider.localeCompare(b.provider));
}

const PRICE_FIELD_LABEL_KEYS: Record<PerTokenPriceField, [string, string]> = {
  input_usd_per_1m: ["modelPricing.fieldInput", "Input"],
  output_usd_per_1m: ["modelPricing.fieldOutput", "Output"],
  cache_read_usd_per_1m: ["modelPricing.fieldCacheRead", "Cache read"],
  cache_write_usd_per_1m: ["modelPricing.fieldCacheWrite", "Cache write 5m"],
  cache_write_1h_usd_per_1m: ["modelPricing.fieldCacheWrite1h", "Cache write 1h"],
  reasoning_usd_per_1m: ["modelPricing.fieldReasoning", "Reasoning"],
};

export function PricingSheet({
  target,
  onOpenChange,
  records,
}: {
  target: PricingSheetTarget | null;
  onOpenChange: (open: boolean) => void;
  records: ModelPriceRecord[];
}) {
  const { t } = useTranslation();
  const { data: metadataRecords = [] } = useModelMetadata();
  const [modelId, setModelId] = useState("");
  const [form, setForm] = useState<PriceForm>(() => formFromRecord(null));
  const [metaForm, setMetaForm] = useState<MetadataForm>(() =>
    metadataFormFromRecord(undefined)
  );
  const [metaDirty, setMetaDirty] = useState(false);
  const [locksDirty, setLocksDirty] = useState(false);
  const [saving, setSaving] = useState(false);
  const [deleteOpen, setDeleteOpen] = useState(false);

  const metadataRecord = useMemo(
    () => metadataRecords.find((record) => record.model_id === modelId.trim()),
    [metadataRecords, modelId]
  );
  const variants = useMemo(
    () => extractVariants(metadataRecord?.raw_json),
    [metadataRecord?.raw_json]
  );

  useEffect(() => {
    if (!target) return;
    setModelId(target.modelId);
    setForm(formFromRecord(target.record));
    setMetaDirty(false);
    setLocksDirty(false);
  }, [target]);

  useEffect(() => {
    if (!metaDirty) setMetaForm(metadataFormFromRecord(metadataRecord));
  }, [metadataRecord, metaDirty]);

  const isCreate = target?.mode === "create";

  const setPrice = (field: PerTokenPriceField, value: string) =>
    setForm((previous) => ({
      ...previous,
      prices: { ...previous.prices, [field]: value },
    }));

  const applyVariant = (variant: ProviderVariant) => {
    setForm((previous) => ({
      ...previous,
      prices: {
        ...previous.prices,
        input_usd_per_1m: variant.input,
        output_usd_per_1m: variant.output,
        cache_read_usd_per_1m: variant.cacheRead,
        cache_write_usd_per_1m: variant.cacheWrite,
        reasoning_usd_per_1m: variant.reasoning,
      },
    }));
    setMetaForm({
      modelsDevProvider: variant.provider,
      mode: variant.mode,
      maxTokens: variant.maxTokens,
      maxInputTokens: variant.maxInputTokens,
      maxOutputTokens: variant.maxOutputTokens,
    });
    setMetaDirty(true);
  };

  const removeLock = (field: string) => {
    setForm((previous) => ({
      ...previous,
      lockedFields: previous.lockedFields.filter((item) => item !== field),
    }));
    setLocksDirty(true);
  };

  const validate = (): string | null => {
    if (!modelId.trim()) return t("modelPricing.errorModelIdRequired", "Model ID is required");
    const invalidDecimal = t(
      "modelPricing.errorInvalidDecimal",
      "Prices must be non-negative decimals with at most 9 fractional digits"
    );
    if (form.billingMode === "per_token") {
      for (const field of PER_TOKEN_PRICE_FIELDS) {
        const value = form.prices[field].trim();
        if (value && !isValidUsdDecimal(value)) return invalidDecimal;
      }
    } else if (form.billingMode === "per_request") {
      const value = form.perRequestUsd.trim();
      if (!value || !isValidUsdDecimal(value)) return invalidDecimal;
    } else {
      if (form.tiers.length === 0) {
        return t("modelPricing.errorTierRequired", "Tiered pricing requires at least one tier");
      }
      for (const [index, tier] of form.tiers.entries()) {
        if (!tier.input.trim() || !isValidUsdDecimal(tier.input.trim())) {
          return t("modelPricing.errorTierInput", "Tier {{index}} requires a valid input price", {
            index: index + 1,
          });
        }
        for (const value of [tier.output, tier.cacheRead, tier.cacheWrite, tier.cacheWrite1h, tier.reasoning]) {
          if (value.trim() && !isValidUsdDecimal(value.trim())) return invalidDecimal;
        }
        const isLast = index === form.tiers.length - 1;
        if (!isLast && (!tier.lte.trim() || !/^\d+$/.test(tier.lte.trim()))) {
          return t(
            "modelPricing.errorTierThreshold",
            "Every tier except the last requires an integer token threshold"
          );
        }
      }
    }
    return null;
  };

  const buildInput = (): UpsertModelPriceInput => {
    const input: UpsertModelPriceInput = {
      billing_mode: form.billingMode,
      enabled: form.enabled,
    };
    if (form.billingMode === "per_token") {
      for (const field of PER_TOKEN_PRICE_FIELDS) {
        const value = form.prices[field].trim();
        input[field] = value ? value : null;
      }
    } else if (form.billingMode === "per_request") {
      input.per_request_usd = form.perRequestUsd.trim();
    } else {
      input.billing_expr = {
        tiers: form.tiers.map((tier, index) => ({
          when_input_tokens_lte:
            index === form.tiers.length - 1 || !tier.lte.trim()
              ? null
              : Number.parseInt(tier.lte.trim(), 10),
          input_usd_per_1m: tier.input.trim(),
          output_usd_per_1m: tier.output.trim() || null,
          cache_read_usd_per_1m: tier.cacheRead.trim() || null,
          cache_write_usd_per_1m: tier.cacheWrite.trim() || null,
          cache_write_1h_usd_per_1m: tier.cacheWrite1h.trim() || null,
          reasoning_usd_per_1m: tier.reasoning.trim() || null,
        })),
      };
    }
    // MP-Y18: an explicit locked_fields replaces the stored set; only send it
    // when the operator edited locks, so normal saves keep MP-Y17 semantics.
    if (locksDirty) input.locked_fields = form.lockedFields;
    return input;
  };

  const save = async () => {
    const invalid = validate();
    if (invalid) {
      toast.error(invalid);
      return;
    }
    setSaving(true);
    const id = modelId.trim();
    try {
      await upsertModelPriceOptimistic(id, buildInput(), records, (error) =>
        toast.error(t("modelPricing.saveFailed", "Failed to save model price"), {
          description: error.message,
        })
      );
      if (metaDirty) {
        await upsertModelMetadataOptimistic(
          id,
          {
            models_dev_provider: metaForm.modelsDevProvider.trim() || null,
            mode: metaForm.mode.trim() || null,
            max_tokens: metaForm.maxTokens.trim() ? Number(metaForm.maxTokens) : null,
            max_input_tokens: metaForm.maxInputTokens.trim()
              ? Number(metaForm.maxInputTokens)
              : null,
            max_output_tokens: metaForm.maxOutputTokens.trim()
              ? Number(metaForm.maxOutputTokens)
              : null,
          },
          metadataRecords,
          (error) =>
            toast.error(t("modelPricing.metadataSaveFailed", "Failed to save metadata"), {
              description: error.message,
            })
        );
      }
      toast.success(t("modelPricing.saveSuccess", "Model price saved"));
      onOpenChange(false);
    } catch {
      return;
    } finally {
      setSaving(false);
    }
  };

  const confirmDelete = async () => {
    if (!target?.record) return;
    try {
      await deleteModelPriceOptimistic(target.record.model_id, records, (error) =>
        toast.error(t("modelPricing.deleteFailed", "Failed to delete model price"), {
          description: error.message,
        })
      );
      toast.success(t("modelPricing.deleteSuccess", "Model price deleted"));
      onOpenChange(false);
    } catch {
      return;
    } finally {
      setDeleteOpen(false);
    }
  };

  return (
    <>
      <Sheet open={!!target} onOpenChange={onOpenChange}>
        <SheetContent side="right" className="w-full p-0 sm:max-w-xl">
          <div className="flex h-full flex-col">
            <SheetHeader className="shrink-0 border-b px-6 py-4">
              <SheetTitle className="font-mono text-base">
                {isCreate
                  ? t("modelPricing.sheetCreateTitle", "New model price")
                  : modelId}
              </SheetTitle>
              <SheetDescription>
                {t(
                  "modelPricing.sheetDescription",
                  "Prices are USD per 1M tokens. Values are exact decimal strings."
                )}
              </SheetDescription>
            </SheetHeader>

            <div className="min-h-0 flex-1 space-y-5 overflow-y-auto px-6 py-4">
              {isCreate ? (
                <div className="space-y-2">
                  <Label htmlFor="price-model-id">
                    {t("modelPricing.modelId", "Model ID")}
                  </Label>
                  <Input
                    id="price-model-id"
                    value={modelId}
                    onChange={(event) => setModelId(event.target.value)}
                    placeholder="gpt-4o"
                    className="font-mono"
                    disabled={!!target?.modelId}
                  />
                </div>
              ) : null}

              <div className="flex items-center justify-between gap-4 rounded-lg border p-3">
                <div className="flex items-center gap-2">
                  <Label htmlFor="price-enabled">
                    {t("modelPricing.enabled", "Enabled")}
                  </Label>
                  {target?.record ? (
                    <Badge
                      variant={target.record.source === "manual" ? "default" : "secondary"}
                      className="text-xs"
                    >
                      {target.record.source}
                    </Badge>
                  ) : null}
                </div>
                <Switch
                  id="price-enabled"
                  checked={form.enabled}
                  onCheckedChange={(enabled) =>
                    setForm((previous) => ({ ...previous, enabled }))
                  }
                />
              </div>

              {variants.length > 0 ? (
                <div className="space-y-2">
                  <Label>{t("modelPricing.variantSource", "models.dev variant")}</Label>
                  <Select
                    onValueChange={(provider) => {
                      const variant = variants.find((item) => item.provider === provider);
                      if (variant) applyVariant(variant);
                    }}
                  >
                    <SelectTrigger>
                      <SelectValue
                        placeholder={t(
                          "modelPricing.variantPlaceholder",
                          "Apply prices from a provider variant"
                        )}
                      />
                    </SelectTrigger>
                    <SelectContent>
                      {variants.map((variant) => (
                        <SelectItem key={variant.provider} value={variant.provider}>
                          <span className="font-medium">{variant.provider}</span>
                          {variant.input ? (
                            <span className="ml-2 text-xs text-muted-foreground">
                              In ${variant.input}
                            </span>
                          ) : null}
                          {variant.output ? (
                            <span className="ml-1 text-xs text-muted-foreground">
                              Out ${variant.output}
                            </span>
                          ) : null}
                        </SelectItem>
                      ))}
                    </SelectContent>
                  </Select>
                </div>
              ) : null}

              <Tabs
                value={form.billingMode}
                onValueChange={(value) =>
                  setForm((previous) => ({
                    ...previous,
                    billingMode: value as BillingMode,
                  }))
                }
              >
                <TabsList className="w-full">
                  {BILLING_MODES.map((mode) => (
                    <TabsTrigger key={mode} value={mode} className="flex-1">
                      {mode === "per_token"
                        ? t("modelPricing.modePerToken", "Per token")
                        : mode === "per_request"
                          ? t("modelPricing.modePerRequest", "Per request")
                          : t("modelPricing.modeTiered", "Tiered")}
                    </TabsTrigger>
                  ))}
                </TabsList>
              </Tabs>

              {form.billingMode === "per_token" ? (
                <div className="grid grid-cols-2 gap-3">
                  {PER_TOKEN_PRICE_FIELDS.map((field) => (
                    <div key={field} className="space-y-1">
                      <Label className="flex items-center gap-1 text-xs">
                        {t(...PRICE_FIELD_LABEL_KEYS[field])}
                        {form.lockedFields.includes(field) ? (
                          <Lock className="h-3 w-3 text-muted-foreground" />
                        ) : null}
                      </Label>
                      <Input
                        inputMode="decimal"
                        value={form.prices[field]}
                        onChange={(event) => setPrice(field, event.target.value)}
                        placeholder="0"
                        className="font-mono"
                      />
                    </div>
                  ))}
                  <p className="col-span-2 text-xs text-muted-foreground">
                    {t(
                      "modelPricing.perTokenHint",
                      "Empty fields clear the stored price. Missing cache prices fall back per spec."
                    )}
                  </p>
                </div>
              ) : form.billingMode === "per_request" ? (
                <div className="space-y-1">
                  <Label className="text-xs">
                    {t("modelPricing.fieldPerRequest", "USD per request")}
                  </Label>
                  <Input
                    inputMode="decimal"
                    value={form.perRequestUsd}
                    onChange={(event) =>
                      setForm((previous) => ({
                        ...previous,
                        perRequestUsd: event.target.value,
                      }))
                    }
                    placeholder="0.02"
                    className="font-mono"
                  />
                </div>
              ) : (
                <div className="space-y-3">
                  {form.tiers.map((tier, index) => {
                    const isLast = index === form.tiers.length - 1;
                    const updateTier = (patch: Partial<TierRow>) =>
                      setForm((previous) => ({
                        ...previous,
                        tiers: previous.tiers.map((item, itemIndex) =>
                          itemIndex === index ? { ...item, ...patch } : item
                        ),
                      }));
                    return (
                      <div key={index} className="space-y-2 rounded-lg border p-3">
                        <div className="flex items-center justify-between">
                          <span className="text-xs font-medium text-muted-foreground">
                            {t("modelPricing.tierLabel", "Tier {{index}}", { index: index + 1 })}
                          </span>
                          <Button
                            variant="ghost"
                            size="icon"
                            className="size-8 text-destructive hover:text-destructive"
                            aria-label={t("modelPricing.removeTier", "Remove tier")}
                            disabled={form.tiers.length === 1}
                            onClick={() =>
                              setForm((previous) => ({
                                ...previous,
                                tiers: previous.tiers.filter((_, i) => i !== index),
                              }))
                            }
                          >
                            <Trash2 className="h-4 w-4" />
                          </Button>
                        </div>
                        <div className="grid grid-cols-2 gap-2">
                          <div className="space-y-1">
                            <Label className="text-xs">
                              {isLast
                                ? t("modelPricing.tierUnbounded", "Input tokens ≤ (unbounded)")
                                : t("modelPricing.tierThreshold", "Input tokens ≤")}
                            </Label>
                            <Input
                              inputMode="numeric"
                              value={tier.lte}
                              disabled={isLast}
                              placeholder={isLast ? "∞" : "200000"}
                              onChange={(event) => updateTier({ lte: event.target.value })}
                              className="font-mono"
                            />
                          </div>
                          <div className="space-y-1">
                            <Label className="text-xs">
                              {t("modelPricing.fieldInput", "Input")}
                            </Label>
                            <Input
                              inputMode="decimal"
                              value={tier.input}
                              onChange={(event) => updateTier({ input: event.target.value })}
                              className="font-mono"
                            />
                          </div>
                          <div className="space-y-1">
                            <Label className="text-xs">
                              {t("modelPricing.fieldOutput", "Output")}
                            </Label>
                            <Input
                              inputMode="decimal"
                              value={tier.output}
                              onChange={(event) => updateTier({ output: event.target.value })}
                              className="font-mono"
                            />
                          </div>
                          <div className="space-y-1">
                            <Label className="text-xs">
                              {t("modelPricing.fieldCacheRead", "Cache read")}
                            </Label>
                            <Input
                              inputMode="decimal"
                              value={tier.cacheRead}
                              onChange={(event) => updateTier({ cacheRead: event.target.value })}
                              className="font-mono"
                            />
                          </div>
                          <div className="space-y-1">
                            <Label className="text-xs">
                              {t("modelPricing.fieldCacheWrite", "Cache write 5m")}
                            </Label>
                            <Input
                              inputMode="decimal"
                              value={tier.cacheWrite}
                              onChange={(event) => updateTier({ cacheWrite: event.target.value })}
                              className="font-mono"
                            />
                          </div>
                          <div className="space-y-1">
                            <Label className="text-xs">
                              {t("modelPricing.fieldReasoning", "Reasoning")}
                            </Label>
                            <Input
                              inputMode="decimal"
                              value={tier.reasoning}
                              onChange={(event) => updateTier({ reasoning: event.target.value })}
                              className="font-mono"
                            />
                          </div>
                        </div>
                      </div>
                    );
                  })}
                  <Button
                    variant="outline"
                    size="sm"
                    onClick={() =>
                      setForm((previous) => ({
                        ...previous,
                        tiers: [...previous.tiers, emptyTier()],
                      }))
                    }
                  >
                    {t("modelPricing.addTier", "Add tier")}
                  </Button>
                </div>
              )}

              {form.lockedFields.length > 0 ? (
                <div className="space-y-2">
                  <Label className="text-xs text-muted-foreground">
                    {t("modelPricing.lockedFields", "Locked fields (sync will not overwrite)")}
                  </Label>
                  <div className="flex flex-wrap gap-1.5">
                    {form.lockedFields.map((field) => (
                      <Badge key={field} variant="outline" className="gap-1 font-mono text-xs">
                        <Lock className="h-3 w-3" />
                        {field}
                        <button
                          type="button"
                          aria-label={t("modelPricing.removeLock", "Remove lock")}
                          onClick={() => removeLock(field)}
                          className="ml-0.5 rounded-sm opacity-70 hover:opacity-100"
                        >
                          <X className="h-3 w-3" />
                        </button>
                      </Badge>
                    ))}
                  </div>
                </div>
              ) : null}

              <details className="group rounded-lg border">
                <summary className="flex cursor-pointer list-none items-center justify-between gap-3 p-3">
                  <span className="text-sm font-medium">
                    {t("modelPricing.metadataSection", "Metadata (mode, token limits)")}
                  </span>
                  <ChevronRight className="h-4 w-4 transition-transform group-open:rotate-90" />
                </summary>
                <div className="grid grid-cols-2 gap-3 border-t p-3">
                  <div className="space-y-1">
                    <Label className="text-xs">
                      {t("modelPricing.metaProvider", "models.dev provider")}
                    </Label>
                    <Input
                      value={metaForm.modelsDevProvider}
                      placeholder="openai"
                      onChange={(event) => {
                        setMetaForm((previous) => ({
                          ...previous,
                          modelsDevProvider: event.target.value,
                        }));
                        setMetaDirty(true);
                      }}
                    />
                  </div>
                  <div className="space-y-1">
                    <Label className="text-xs">{t("modelPricing.metaMode", "Mode")}</Label>
                    <Input
                      value={metaForm.mode}
                      placeholder="chat"
                      onChange={(event) => {
                        setMetaForm((previous) => ({ ...previous, mode: event.target.value }));
                        setMetaDirty(true);
                      }}
                    />
                  </div>
                  <div className="space-y-1">
                    <Label className="text-xs">{t("modelPricing.metaContext", "Context")}</Label>
                    <Input
                      type="number"
                      value={metaForm.maxTokens}
                      placeholder="128000"
                      onChange={(event) => {
                        setMetaForm((previous) => ({ ...previous, maxTokens: event.target.value }));
                        setMetaDirty(true);
                      }}
                    />
                  </div>
                  <div className="space-y-1">
                    <Label className="text-xs">{t("modelPricing.metaMaxInput", "Max input")}</Label>
                    <Input
                      type="number"
                      value={metaForm.maxInputTokens}
                      placeholder="128000"
                      onChange={(event) => {
                        setMetaForm((previous) => ({
                          ...previous,
                          maxInputTokens: event.target.value,
                        }));
                        setMetaDirty(true);
                      }}
                    />
                  </div>
                  <div className="space-y-1">
                    <Label className="text-xs">
                      {t("modelPricing.metaMaxOutput", "Max output")}
                    </Label>
                    <Input
                      type="number"
                      value={metaForm.maxOutputTokens}
                      placeholder="16384"
                      onChange={(event) => {
                        setMetaForm((previous) => ({
                          ...previous,
                          maxOutputTokens: event.target.value,
                        }));
                        setMetaDirty(true);
                      }}
                    />
                  </div>
                </div>
              </details>
            </div>

            <div className="flex shrink-0 items-center justify-between gap-2 border-t px-6 py-4">
              {!isCreate && target?.record ? (
                <Button
                  variant="destructive"
                  size="sm"
                  onClick={() => setDeleteOpen(true)}
                >
                  <Trash2 className="mr-1 h-3.5 w-3.5" />
                  {t("common.delete", "Delete")}
                </Button>
              ) : (
                <span />
              )}
              <div className="flex items-center gap-2">
                <Button variant="outline" onClick={() => onOpenChange(false)}>
                  {t("common.cancel", "Cancel")}
                </Button>
                <Button onClick={() => void save()} disabled={saving}>
                  {saving ? t("common.saving", "Saving...") : t("common.save", "Save")}
                </Button>
              </div>
            </div>
          </div>
        </SheetContent>
      </Sheet>

      <AlertDialog open={deleteOpen} onOpenChange={setDeleteOpen}>
        <AlertDialogContent>
          <AlertDialogHeader>
            <AlertDialogTitle>
              {t("modelPricing.deleteTitle", "Delete model price")}
            </AlertDialogTitle>
            <AlertDialogDescription>
              {t(
                "modelPricing.deleteConfirm",
                "Requests for this model will fail closed unless free settlement is allowed."
              )}
            </AlertDialogDescription>
          </AlertDialogHeader>
          <AlertDialogFooter>
            <AlertDialogCancel>{t("common.cancel", "Cancel")}</AlertDialogCancel>
            <AlertDialogAction
              className="bg-destructive text-destructive-foreground hover:bg-destructive/90"
              onClick={() => void confirmDelete()}
            >
              {t("common.delete", "Delete")}
            </AlertDialogAction>
          </AlertDialogFooter>
        </AlertDialogContent>
      </AlertDialog>
    </>
  );
}
