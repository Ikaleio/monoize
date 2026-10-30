import { useEffect, useId, useMemo, useState } from "react";
import { useTranslation } from "react-i18next";
import { ChevronRight, Lock, Trash2, X } from "lucide-react";
import { toast } from "sonner";
import { Badge } from "@/components/ui/badge";
import { Button } from "@/components/ui/button";
import { Input } from "@/components/ui/input";
import { Label } from "@/components/ui/label";
import { Switch } from "@/components/ui/switch";
import { FieldError } from "@/components/ui/field";
import { Tabs, TabsList, TabsTrigger } from "@/components/ui/tabs";
import {
  Dialog,
  DialogContent,
  DialogDescription,
  DialogFooter,
  DialogHeader,
  DialogTitle,
} from "@/components/ui/dialog";
import {
  Select,
  SelectContent,
  SelectItem,
  SelectTrigger,
  SelectValue,
} from "@/components/ui/select";
import type {
  BillingExprTier,
  BillingMode,
  ModelMetadataRecord,
  ModelPriceRecord,
  UpsertModelPriceInput,
} from "@/lib/api";
import { toastApiError } from "@/lib/api-error";
import {
  upsertModelMetadataOptimistic,
  upsertModelPriceOptimistic,
  useModelMetadata,
} from "@/lib/swr";
import {
  BILLING_MODES,
  PER_TOKEN_PRICE_FIELDS,
  isValidUsdDecimal,
  type PerTokenPriceField,
  type PricingEditorTarget,
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
  serviceTiers: Record<string, TierRow[]>;
  lockedFields: string[];
  enabled: boolean;
}

interface MetadataForm {
  mode: string;
  maxTokens: string;
  maxInputTokens: string;
  maxOutputTokens: string;
}

/** Field errors keyed by the invalid control's element id. */
type FieldErrors = Record<string, string>;

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
    lte:
      tier.when_input_tokens_lte != null
        ? String(tier.when_input_tokens_lte)
        : "",
    input: tier.input_usd_per_1m ?? "",
    output: tier.output_usd_per_1m ?? "",
    cacheRead: tier.cache_read_usd_per_1m ?? "",
    cacheWrite: tier.cache_write_usd_per_1m ?? "",
    cacheWrite1h: tier.cache_write_1h_usd_per_1m ?? "",
    reasoning: tier.reasoning_usd_per_1m ?? "",
  };
}

function serviceTiersFromRecord(
  record: ModelPriceRecord | null,
): Record<string, TierRow[]> {
  const tables = record?.billing_expr?.service_tiers;
  if (!tables) return {};
  return Object.fromEntries(
    Object.entries(tables).map(([key, table]) => [
      key,
      table.tiers.map(tierFromExpr),
    ]),
  );
}

function rowsToExprTiers(tiers: TierRow[]): BillingExprTier[] {
  return tiers.map((tier, index) => ({
    when_input_tokens_lte:
      index === tiers.length - 1 || !tier.lte.trim()
        ? null
        : Number.parseInt(tier.lte.trim(), 10),
    input_usd_per_1m: tier.input.trim(),
    output_usd_per_1m: tier.output.trim() || null,
    cache_read_usd_per_1m: tier.cacheRead.trim() || null,
    cache_write_usd_per_1m: tier.cacheWrite.trim() || null,
    cache_write_1h_usd_per_1m: tier.cacheWrite1h.trim() || null,
    reasoning_usd_per_1m: tier.reasoning.trim() || null,
  }));
}

function copyThresholds(source: TierRow[]): TierRow[] {
  const rows = source.length > 0 ? source : [emptyTier()];
  return rows.map((tier) => ({
    ...emptyTier(),
    lte: tier.lte,
  }));
}

function formFromRecord(record: ModelPriceRecord | null): PriceForm {
  return {
    billingMode: record?.billing_mode ?? "per_token",
    prices: Object.fromEntries(
      PER_TOKEN_PRICE_FIELDS.map((field) => [field, record?.[field] ?? ""]),
    ) as Record<PerTokenPriceField, string>,
    perRequestUsd: record?.per_request_usd ?? "",
    tiers: record?.billing_expr?.tiers.map(tierFromExpr) ?? [emptyTier()],
    serviceTiers: serviceTiersFromRecord(record),
    lockedFields: record?.locked_fields ?? [],
    enabled: record?.enabled ?? true,
  };
}

function metadataFormFromRecord(
  record: ModelMetadataRecord | undefined,
): MetadataForm {
  return {
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
  contextLimit: string;
  outputLimit: string;
}

// models.dev variant prices are USD-per-1M strings inside metadata raw_json
// (model-metadata-dashboard.spec.md UI3); kept as strings end to end.
function extractVariants(
  rawJson: Record<string, unknown> | undefined,
): ProviderVariant[] {
  const providers = rawJson?.providers;
  if (!providers || typeof providers !== "object") return [];
  const priceStr = (value: unknown): string =>
    typeof value === "string" && isValidUsdDecimal(value) ? value : "";
  const limitStr = (value: unknown): string =>
    typeof value === "number" && Number.isSafeInteger(value) && value > 0
      ? String(value)
      : "";
  return Object.entries(providers as Record<string, unknown>)
    .flatMap(([provider, value]) => {
      if (!value || typeof value !== "object") return [];
      const entry = value as Record<string, unknown>;
      const cost = entry.cost as Record<string, unknown> | undefined;
      const limit = entry.limit as Record<string, unknown> | undefined;
      return [
        {
          provider,
          input: priceStr(cost?.input),
          output: priceStr(cost?.output),
          cacheRead: priceStr(cost?.cache_read),
          cacheWrite: priceStr(cost?.cache_write),
          reasoning: priceStr(cost?.reasoning),
          contextLimit: limitStr(limit?.context),
          outputLimit: limitStr(limit?.output),
        },
      ];
    })
    .sort((a, b) => a.provider.localeCompare(b.provider));
}

const PRICE_FIELD_LABEL_KEYS: Record<PerTokenPriceField, string> = {
  input_usd_per_1m: "modelPricing.fieldInput",
  output_usd_per_1m: "modelPricing.fieldOutput",
  cache_read_usd_per_1m: "modelPricing.fieldCacheRead",
  cache_write_usd_per_1m: "modelPricing.fieldCacheWrite",
  cache_write_1h_usd_per_1m: "modelPricing.fieldCacheWrite1h",
  reasoning_usd_per_1m: "modelPricing.fieldReasoning",
};

const TIER_PRICE_FIELDS = [
  ["input", "modelPricing.fieldInput"],
  ["output", "modelPricing.fieldOutput"],
  ["cacheRead", "modelPricing.fieldCacheRead"],
  ["cacheWrite", "modelPricing.fieldCacheWrite"],
  ["cacheWrite1h", "modelPricing.fieldCacheWrite1h"],
  ["reasoning", "modelPricing.fieldReasoning"],
] as const satisfies ReadonlyArray<readonly [keyof TierRow, string]>;

function RequiredMark() {
  return (
    <span aria-hidden="true" className="text-error-foreground">
      *
    </span>
  );
}

/** Text input with an optional inline error bound through aria-describedby. */
function PriceInput({
  id,
  value,
  error,
  required,
  disabled,
  placeholder,
  inputMode = "decimal",
  mono = true,
  onChange,
}: {
  id: string;
  value: string;
  error?: string;
  required?: boolean;
  disabled?: boolean;
  placeholder?: string;
  inputMode?: "decimal" | "numeric";
  mono?: boolean;
  onChange: (value: string) => void;
}) {
  return (
    <>
      <Input
        inputMode={inputMode}
        id={id}
        value={value}
        disabled={disabled}
        placeholder={placeholder}
        aria-required={required || undefined}
        aria-invalid={error ? true : undefined}
        aria-describedby={error ? `${id}-error` : undefined}
        onChange={(event) => onChange(event.target.value)}
        className={mono ? "font-mono" : undefined}
      />
      {error ? <FieldError id={`${id}-error`}>{error}</FieldError> : null}
    </>
  );
}

function TiersEditor({
  formId,
  idPrefix,
  labelPrefix,
  tiers,
  errors,
  onFieldEdit,
  onChange,
}: {
  formId: string;
  idPrefix: string;
  labelPrefix?: string;
  tiers: TierRow[];
  errors: FieldErrors;
  onFieldEdit: (id: string) => void;
  onChange: (tiers: TierRow[]) => void;
}) {
  const { t } = useTranslation();
  const tierName = (index: number) => {
    const label = t("modelPricing.tierLabel", "Tier {{index}}", {
      index: index + 1,
    });
    return labelPrefix ? `${labelPrefix} · ${label}` : label;
  };
  return (
    <div className="space-y-3">
      {tiers.map((tier, index) => {
        const isLast = index === tiers.length - 1;
        const fieldId = (field: keyof TierRow) => `${formId}-${idPrefix}-${index}-${field}`;
        const updateTier = (field: keyof TierRow, value: string) => {
          onFieldEdit(fieldId(field));
          onChange(
            tiers.map((item, itemIndex) =>
              itemIndex === index ? { ...item, [field]: value } : item,
            ),
          );
        };
        return (
          <div key={index} className="space-y-2 rounded-lg border p-3">
            <div className="flex items-center justify-between">
              <span className="text-sm font-medium text-muted-foreground">
                {tierName(index)}
              </span>
              <Button
                type="button"
                variant="ghost"
                size="icon"
                className="size-8 text-error-foreground hover:text-error-foreground"
                aria-label={`${t("modelPricing.removeTier", "Remove tier")}: ${tierName(index)}`}
                disabled={tiers.length === 1}
                onClick={() =>
                  onChange(tiers.filter((_, itemIndex) => itemIndex !== index))
                }
              >
                <Trash2 className="text-error-foreground" />
              </Button>
            </div>
            <div className="grid grid-cols-2 gap-2">
              <div className="space-y-1">
                <Label htmlFor={fieldId("lte")}>
                  <span className="sr-only">{tierName(index)} · </span>
                  {isLast
                    ? t("modelPricing.tierUnbounded", "Input tokens ≤ (unbounded)")
                    : t("modelPricing.tierThreshold", "Input tokens ≤")}
                  {isLast ? null : <RequiredMark />}
                </Label>
                <PriceInput
                  id={fieldId("lte")}
                  inputMode="numeric"
                  value={tier.lte}
                  disabled={isLast}
                  required={!isLast}
                  placeholder={isLast ? "∞" : "272000"}
                  error={errors[fieldId("lte")]}
                  onChange={(value) => updateTier("lte", value)}
                />
              </div>
              {TIER_PRICE_FIELDS.map(([field, labelKey]) => (
                <div key={field} className="space-y-1">
                  <Label htmlFor={fieldId(field)}>
                    <span className="sr-only">{tierName(index)} · </span>
                    {t(labelKey)}
                    {field === "input" ? <RequiredMark /> : null}
                  </Label>
                  <PriceInput
                    id={fieldId(field)}
                    value={tier[field]}
                    required={field === "input"}
                    error={errors[fieldId(field)]}
                    onChange={(value) => updateTier(field, value)}
                  />
                </div>
              ))}
            </div>
          </div>
        );
      })}
      <Button
        type="button"
        variant="outline"
        size="sm"
        onClick={() => onChange([...tiers, emptyTier()])}
      >
        {t("modelPricing.addTier", "Add tier")}
      </Button>
    </div>
  );
}

/** Create or edit one model price (model-pricing.spec.md MP-UI3, frontend-design-system.spec.md DS67). */
export function PricingEditorDialog({
  target,
  onOpenChange,
  onTargetChange,
  records,
}: {
  target: PricingEditorTarget | null;
  onOpenChange: (open: boolean) => void;
  /** Switches the open dialog to another target ("Edit instead"). */
  onTargetChange: (target: PricingEditorTarget) => void;
  records: ModelPriceRecord[];
}) {
  const { t } = useTranslation();
  const formId = useId();
  const { data: metadataRecords = [] } = useModelMetadata();
  const [modelId, setModelId] = useState("");
  const [form, setForm] = useState<PriceForm>(() => formFromRecord(null));
  const [initialForm, setInitialForm] = useState("");
  const [metaForm, setMetaForm] = useState<MetadataForm>(() =>
    metadataFormFromRecord(undefined),
  );
  const [metaDirty, setMetaDirty] = useState(false);
  const [locksDirty, setLocksDirty] = useState(false);
  const [saving, setSaving] = useState(false);
  const [errors, setErrors] = useState<FieldErrors>({});
  const [appliedVariant, setAppliedVariant] = useState<string | null>(null);

  const metadataRecord = useMemo(
    () => metadataRecords.find((record) => record.model_id === modelId.trim()),
    [metadataRecords, modelId],
  );
  const variants = useMemo(
    () => extractVariants(metadataRecord?.raw_json),
    [metadataRecord?.raw_json],
  );

  useEffect(() => {
    if (!target) return;
    const initial = formFromRecord(target.record);
    setModelId(target.modelId);
    setForm(initial);
    setInitialForm(JSON.stringify(initial));
    setMetaDirty(false);
    setLocksDirty(false);
    setErrors({});
    setAppliedVariant(null);
  }, [target]);

  useEffect(() => {
    if (!metaDirty) setMetaForm(metadataFormFromRecord(metadataRecord));
  }, [metadataRecord, metaDirty]);

  const isCreate = target?.mode === "create";
  // CAT-8: create mode must never silently overwrite an existing row.
  const existingRecord = isCreate
    ? records.find((record) => record.model_id === modelId.trim())
    : undefined;
  const dirty =
    target !== null &&
    (JSON.stringify(form) !== initialForm ||
      metaDirty ||
      locksDirty ||
      modelId.trim() !== target.modelId);

  const id = (suffix: string) => `${formId}-${suffix}`;
  const clearError = (fieldId: string) =>
    setErrors((previous) => {
      if (!(fieldId in previous)) return previous;
      const next = { ...previous };
      delete next[fieldId];
      return next;
    });

  const setPrice = (field: PerTokenPriceField, value: string) => {
    clearError(id(field));
    setForm((previous) => ({
      ...previous,
      prices: { ...previous.prices, [field]: value },
    }));
  };

  // model-metadata-dashboard.spec.md UI3: a variant fills the visible price
  // fields of the current mode (per-token columns, or tier 1 in tiered mode)
  // and the context / max-output limits.
  const applyVariant = (variant: ProviderVariant) => {
    setForm((previous) => {
      if (previous.billingMode === "tiered_expr") {
        const [first = emptyTier(), ...rest] = previous.tiers;
        return {
          ...previous,
          tiers: [
            {
              ...first,
              input: variant.input,
              output: variant.output,
              cacheRead: variant.cacheRead,
              cacheWrite: variant.cacheWrite,
              reasoning: variant.reasoning,
            },
            ...rest,
          ],
        };
      }
      return {
        ...previous,
        prices: {
          ...previous.prices,
          input_usd_per_1m: variant.input,
          output_usd_per_1m: variant.output,
          cache_read_usd_per_1m: variant.cacheRead,
          cache_write_usd_per_1m: variant.cacheWrite,
          reasoning_usd_per_1m: variant.reasoning,
        },
      };
    });
    if (variant.contextLimit || variant.outputLimit) {
      setMetaForm((previous) => ({
        ...previous,
        maxTokens: variant.contextLimit || previous.maxTokens,
        maxOutputTokens: variant.outputLimit || previous.maxOutputTokens,
      }));
      setMetaDirty(true);
    }
    setErrors({});
    setAppliedVariant(variant.provider);
  };

  const removeLock = (field: string) => {
    setForm((previous) => ({
      ...previous,
      lockedFields: previous.lockedFields.filter((item) => item !== field),
    }));
    setLocksDirty(true);
  };

  /** Errors in DOM order, so the first key is the first invalid control. */
  const validate = (): FieldErrors => {
    const next: FieldErrors = {};
    const invalidDecimal = t("modelPricing.errorInvalidDecimal");
    const required = t("modelPricing.errorRequired");
    const checkPrice = (fieldId: string, value: string, isRequired: boolean) => {
      const trimmed = value.trim();
      if (!trimmed) {
        if (isRequired) next[fieldId] = required;
      } else if (!isValidUsdDecimal(trimmed)) {
        next[fieldId] = invalidDecimal;
      }
    };
    if (isCreate && !modelId.trim()) next[id("model-id")] = t("modelPricing.errorModelIdRequired");
    if (form.billingMode === "per_token") {
      for (const field of PER_TOKEN_PRICE_FIELDS) {
        checkPrice(id(field), form.prices[field], field === "input_usd_per_1m");
      }
    } else if (form.billingMode === "per_request") {
      checkPrice(id("per-request"), form.perRequestUsd, true);
    } else {
      const tables: Array<[string, TierRow[]]> = [
        ["tier", form.tiers],
        ...Object.entries(form.serviceTiers).map(
          ([key, tiers]): [string, TierRow[]] => [`${key}-tier`, tiers],
        ),
      ];
      for (const [prefix, tiers] of tables) {
        tiers.forEach((tier, index) => {
          const isLast = index === tiers.length - 1;
          if (!isLast && !/^\d+$/.test(tier.lte.trim())) {
            next[id(`${prefix}-${index}-lte`)] = t("modelPricing.errorTierThreshold");
          }
          for (const [field] of TIER_PRICE_FIELDS) {
            checkPrice(id(`${prefix}-${index}-${field}`), tier[field], field === "input");
          }
        });
      }
    }
    for (const [key, value] of [
      ["meta-maxTokens", metaForm.maxTokens],
      ["meta-maxInputTokens", metaForm.maxInputTokens],
      ["meta-maxOutputTokens", metaForm.maxOutputTokens],
    ] as const) {
      if (value.trim() && !/^[1-9]\d*$/.test(value.trim())) {
        next[id(key)] = t("modelPricing.errorTokenLimit");
      }
    }
    return next;
  };

  const save = async () => {
    if (saving || existingRecord) return;
    const nextErrors = validate();
    setErrors(nextErrors);
    const firstInvalid = Object.keys(nextErrors)[0];
    if (firstInvalid) {
      const element = document.getElementById(firstInvalid);
      element?.closest("details")?.setAttribute("open", "");
      element?.focus();
      element?.scrollIntoView({ block: "center" });
      return;
    }
    setSaving(true);
    const modelKey = modelId.trim();
    try {
      await upsertModelPriceOptimistic(modelKey, buildInput(), records, (error) =>
        toastApiError(error, t, t("modelPricing.saveFailed")),
      );
      if (metaDirty) {
        await upsertModelMetadataOptimistic(
          modelKey,
          {
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
          (error) => toastApiError(error, t, t("modelPricing.metadataSaveFailed")),
        );
      }
      toast.success(t("modelPricing.saveSuccess"));
      onOpenChange(false);
    } catch {
      return;
    } finally {
      setSaving(false);
    }
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
      const service_tiers = Object.fromEntries(
        Object.entries(form.serviceTiers)
          .filter(([, tiers]) => tiers.length > 0)
          .map(([key, tiers]) => [key, { tiers: rowsToExprTiers(tiers) }]),
      );
      input.billing_expr = {
        tiers: rowsToExprTiers(form.tiers),
        ...(Object.keys(service_tiers).length > 0 ? { service_tiers } : {}),
      };
    }
    // MP-Y18: an explicit locked_fields replaces the stored set; only send it
    // when the operator edited locks, so normal saves keep MP-Y17 semantics.
    if (locksDirty) input.locked_fields = form.lockedFields;
    return input;
  };

  const metaField = (key: keyof Omit<MetadataForm, "mode">, labelKey: string, placeholder: string) => {
    const fieldId = id(`meta-${key}`);
    return (
      <div className="space-y-1">
        <Label htmlFor={fieldId}>{t(labelKey)}</Label>
        <PriceInput
          id={fieldId}
          inputMode="numeric"
          mono={false}
          value={metaForm[key]}
          placeholder={placeholder}
          error={errors[fieldId]}
          onChange={(value) => {
            clearError(fieldId);
            setMetaForm((previous) => ({ ...previous, [key]: value }));
            setMetaDirty(true);
          }}
        />
      </div>
    );
  };

  return (
    <Dialog open={!!target} onOpenChange={onOpenChange}>
      <DialogContent
        dirty={dirty}
        className="flex max-h-[calc(100dvh-2rem)] flex-col overflow-hidden p-0 sm:max-h-[calc(100dvh-3rem)] sm:max-w-2xl"
      >
        <DialogHeader className="shrink-0 border-b px-6 py-4">
          <DialogTitle className="font-mono text-base">
            {isCreate ? t("modelPricing.sheetCreateTitle") : modelId}
          </DialogTitle>
          <DialogDescription>
            {form.billingMode === "per_request"
              ? t("modelPricing.sheetDescriptionPerRequest")
              : t("modelPricing.sheetDescription")}
          </DialogDescription>
        </DialogHeader>

        <form
          noValidate
          className="flex min-h-0 flex-1 flex-col"
          onSubmit={(event) => {
            event.preventDefault();
            void save();
          }}
        >
          <div className="min-h-0 flex-1 space-y-5 overflow-y-auto px-6 py-4">
            {isCreate ? (
              <div className="space-y-2">
                <Label htmlFor={id("model-id")}>
                  {t("modelPricing.modelId")}
                  <RequiredMark />
                </Label>
                <Input
                  id={id("model-id")}
                  value={modelId}
                  onChange={(event) => {
                    clearError(id("model-id"));
                    setModelId(event.target.value);
                  }}
                  placeholder="gpt-4o"
                  className="font-mono"
                  disabled={!!target?.modelId}
                  aria-required
                  aria-invalid={errors[id("model-id")] || existingRecord ? true : undefined}
                  aria-describedby={
                    errors[id("model-id")] || existingRecord ? id("model-id-error") : undefined
                  }
                />
                {existingRecord ? (
                  <div
                    id={id("model-id-error")}
                    role="alert"
                    className="flex flex-wrap items-center gap-2 text-sm font-normal text-destructive"
                  >
                    {t("modelPricing.errorModelExists")}
                    <Button
                      type="button"
                      variant="outline"
                      size="sm"
                      onClick={() =>
                        onTargetChange({
                          mode: "edit",
                          modelId: existingRecord.model_id,
                          record: existingRecord,
                        })
                      }
                    >
                      {t("modelPricing.editInstead")}
                    </Button>
                  </div>
                ) : errors[id("model-id")] ? (
                  <FieldError id={id("model-id-error")}>{errors[id("model-id")]}</FieldError>
                ) : null}
              </div>
            ) : null}

            {existingRecord ? null : (
              <>
                <div className="flex items-center justify-between gap-4 rounded-lg border p-3">
                  <div className="flex items-center gap-2">
                    <Label htmlFor={id("enabled")}>{t("modelPricing.enabled")}</Label>
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
                    id={id("enabled")}
                    checked={form.enabled}
                    onCheckedChange={(enabled) =>
                      setForm((previous) => ({ ...previous, enabled }))
                    }
                  />
                </div>

                {variants.length > 0 && form.billingMode !== "per_request" ? (
                  <div className="space-y-2">
                    <Label htmlFor={id("variant")}>{t("modelPricing.variantSource")}</Label>
                    <Select
                      value={appliedVariant ?? ""}
                      onValueChange={(provider) => {
                        const variant = variants.find((item) => item.provider === provider);
                        if (variant) applyVariant(variant);
                      }}
                    >
                      <SelectTrigger id={id("variant")} aria-describedby={id("variant-hint")}>
                        <SelectValue placeholder={t("modelPricing.variantPlaceholder")} />
                      </SelectTrigger>
                      <SelectContent>
                        {variants.map((variant) => (
                          <SelectItem key={variant.provider} value={variant.provider}>
                            <span className="font-medium">{variant.provider}</span>
                            {variant.input ? (
                              <span className="ml-2 text-xs text-muted-foreground">
                                {t("modelPricing.variantInput", { price: variant.input })}
                              </span>
                            ) : null}
                            {variant.output ? (
                              <span className="ml-1 text-xs text-muted-foreground">
                                {t("modelPricing.variantOutput", { price: variant.output })}
                              </span>
                            ) : null}
                          </SelectItem>
                        ))}
                      </SelectContent>
                    </Select>
                    <p id={id("variant-hint")} className="text-xs text-muted-foreground">
                      {appliedVariant
                        ? t(
                            form.billingMode === "tiered_expr"
                              ? "modelPricing.variantAppliedTier"
                              : "modelPricing.variantApplied",
                            { provider: appliedVariant },
                          )
                        : form.billingMode === "tiered_expr"
                          ? t("modelPricing.variantHintTier")
                          : t("modelPricing.variantHint")}
                    </p>
                  </div>
                ) : null}

                <Tabs
                  value={form.billingMode}
                  onValueChange={(value) => {
                    setErrors({});
                    setAppliedVariant(null);
                    setForm((previous) => ({
                      ...previous,
                      billingMode: value as BillingMode,
                    }));
                  }}
                >
                  <TabsList className="w-full">
                    {BILLING_MODES.map((mode) => (
                      <TabsTrigger key={mode} value={mode} className="flex-1">
                        {mode === "per_token"
                          ? t("modelPricing.modePerToken")
                          : mode === "per_request"
                            ? t("modelPricing.modePerRequest")
                            : t("modelPricing.modeTiered")}
                      </TabsTrigger>
                    ))}
                  </TabsList>
                </Tabs>

                {form.billingMode === "per_token" ? (
                  <div className="grid grid-cols-2 gap-3">
                    {PER_TOKEN_PRICE_FIELDS.map((field) => (
                      <div key={field} className="space-y-1">
                        <Label htmlFor={id(field)} className="flex items-center gap-1">
                          {t(PRICE_FIELD_LABEL_KEYS[field])}
                          {field === "input_usd_per_1m" ? <RequiredMark /> : null}
                          {form.lockedFields.includes(field) ? (
                            <Lock className="h-3 w-3 text-muted-foreground" aria-hidden="true" />
                          ) : null}
                        </Label>
                        <PriceInput
                          id={id(field)}
                          value={form.prices[field]}
                          required={field === "input_usd_per_1m"}
                          placeholder="0"
                          error={errors[id(field)]}
                          onChange={(value) => setPrice(field, value)}
                        />
                      </div>
                    ))}
                    <p className="col-span-2 text-xs text-muted-foreground">
                      {t("modelPricing.perTokenHint")}
                    </p>
                  </div>
                ) : form.billingMode === "per_request" ? (
                  <div className="space-y-1">
                    <Label htmlFor={id("per-request")}>
                      {t("modelPricing.fieldPerRequest")}
                      <RequiredMark />
                    </Label>
                    <PriceInput
                      id={id("per-request")}
                      value={form.perRequestUsd}
                      required
                      placeholder="0.02"
                      error={errors[id("per-request")]}
                      onChange={(value) => {
                        clearError(id("per-request"));
                        setForm((previous) => ({ ...previous, perRequestUsd: value }));
                      }}
                    />
                  </div>
                ) : (
                  <div className="space-y-4">
                    <TiersEditor
                      formId={formId}
                      idPrefix="tier"
                      tiers={form.tiers}
                      errors={errors}
                      onFieldEdit={clearError}
                      onChange={(tiers) => setForm((previous) => ({ ...previous, tiers }))}
                    />
                    <div className="space-y-3 rounded-lg border p-3">
                      <div className="flex items-center justify-between gap-3">
                        <div>
                          <p className="text-sm font-medium">{t("modelPricing.serviceTierFast")}</p>
                          <p className="text-xs text-muted-foreground">
                            {t("modelPricing.serviceTierFastHint")}
                          </p>
                        </div>
                        {form.serviceTiers.fast ? (
                          <Button
                            type="button"
                            variant="ghost"
                            size="sm"
                            onClick={() =>
                              setForm((previous) => {
                                const next = { ...previous.serviceTiers };
                                delete next.fast;
                                return { ...previous, serviceTiers: next };
                              })
                            }
                          >
                            {t("modelPricing.removeFastPrices")}
                          </Button>
                        ) : (
                          <Button
                            type="button"
                            variant="outline"
                            size="sm"
                            onClick={() =>
                              setForm((previous) => ({
                                ...previous,
                                serviceTiers: {
                                  ...previous.serviceTiers,
                                  fast: copyThresholds(previous.tiers),
                                },
                              }))
                            }
                          >
                            {t("modelPricing.addFastPrices")}
                          </Button>
                        )}
                      </div>
                      {form.serviceTiers.fast ? (
                        <TiersEditor
                          formId={formId}
                          idPrefix="fast-tier"
                          labelPrefix={t("modelPricing.serviceTierFast")}
                          tiers={form.serviceTiers.fast}
                          errors={errors}
                          onFieldEdit={clearError}
                          onChange={(tiers) =>
                            setForm((previous) => ({
                              ...previous,
                              serviceTiers: { ...previous.serviceTiers, fast: tiers },
                            }))
                          }
                        />
                      ) : null}
                    </div>
                  </div>
                )}

                {form.lockedFields.length > 0 ? (
                  <div className="space-y-2">
                    <p className="text-xs font-medium leading-none text-muted-foreground">{t("modelPricing.lockedFields")}</p>
                    <div className="flex flex-wrap gap-1.5">
                      {form.lockedFields.map((field) => (
                        <Badge key={field} variant="outline" className="gap-1 font-mono text-xs">
                          <Lock className="h-3 w-3" aria-hidden="true" />
                          {field}
                          <button
                            type="button"
                            aria-label={`${t("modelPricing.removeLock")}: ${field}`}
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
                    <span className="text-sm font-medium">{t("modelPricing.metadataSection")}</span>
                    <ChevronRight className="h-4 w-4 transition-transform group-open:rotate-90" />
                  </summary>
                  <div className="grid grid-cols-2 gap-3 border-t p-3">
                    <div className="space-y-1">
                      <Label htmlFor={id("meta-mode")}>{t("modelPricing.metaMode")}</Label>
                      <Input
                        id={id("meta-mode")}
                        value={metaForm.mode}
                        placeholder="chat"
                        onChange={(event) => {
                          setMetaForm((previous) => ({ ...previous, mode: event.target.value }));
                          setMetaDirty(true);
                        }}
                      />
                    </div>
                    {metaField("maxTokens", "modelPricing.metaContext", "128000")}
                    {metaField("maxInputTokens", "modelPricing.metaMaxInput", "128000")}
                    {metaField("maxOutputTokens", "modelPricing.metaMaxOutput", "16384")}
                  </div>
                </details>
              </>
            )}
          </div>

          <DialogFooter className="shrink-0 border-t px-6 pb-6 pt-4">
            <Button type="button" variant="outline" onClick={() => onOpenChange(false)}>
              {t("common.cancel")}
            </Button>
            <Button type="submit" disabled={saving || !!existingRecord}>
              {saving ? t("common.saving") : t("common.save")}
            </Button>
          </DialogFooter>
        </form>
      </DialogContent>
    </Dialog>
  );
}
