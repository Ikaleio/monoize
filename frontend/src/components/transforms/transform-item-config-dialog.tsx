import { useMemo, useState } from "react";
import { useTranslation } from "react-i18next";
import { AlertTriangle, Braces, Copy } from "lucide-react";
import { toast } from "sonner";
import { Button } from "@/components/ui/button";
import {
  Dialog,
  DialogContent,
  DialogDescription,
  DialogFooter,
  DialogHeader,
  DialogTitle,
} from "@/components/ui/dialog";
import { Label } from "@/components/ui/label";
import { Textarea } from "@/components/ui/textarea";
import type { TransformRegistryItem, TransformRuleConfig } from "@/lib/api";
import { resolveLocalizedText } from "./localized-text";
import { ModelsGlobInput } from "./models-glob-input";
import { SchemaFormFields } from "./schema-form-fields";
import {
  buildDraftConfig,
  getSchemaObject,
  serializeDraftConfig,
  validateTransformRule,
  type DraftValue,
} from "./transform-schema";

type TransformItemConfigDialogProps = {
  open: boolean;
  rule: TransformRuleConfig | null;
  registryItem?: TransformRegistryItem;
  onOpenChange: (open: boolean) => void;
  onSave: (nextRule: TransformRuleConfig) => void;
};

export function TransformItemConfigDialog({
  open,
  rule,
  registryItem,
  onOpenChange,
  onSave,
}: TransformItemConfigDialogProps) {
  if (!rule) {
    return null;
  }

  const stateKey = [
    rule.phase,
    rule.transform,
    String(rule.enabled),
    JSON.stringify(rule.models ?? null),
    JSON.stringify(rule.config ?? {}),
  ].join("|");

  return (
    <TransformItemConfigDialogInner
      key={stateKey}
      open={open}
      rule={rule}
      registryItem={registryItem}
      onOpenChange={onOpenChange}
      onSave={onSave}
    />
  );
}

function TransformItemConfigDialogInner({
  open,
  rule,
  registryItem,
  onOpenChange,
  onSave,
}: {
  open: boolean;
  rule: TransformRuleConfig;
  registryItem?: TransformRegistryItem;
  onOpenChange: (open: boolean) => void;
  onSave: (nextRule: TransformRuleConfig) => void;
}) {
  const { t, i18n } = useTranslation();
  const initialRule = useMemo(() => buildInitialRule(rule), [rule]);

  const schema = registryItem ? getSchemaObject(registryItem.config_schema) : null;
  const canUseSchemaForm = Boolean(registryItem && schema?.type === "object");
  const isUnknownTransform = !registryItem;

  const [draftRule, setDraftRule] = useState<TransformRuleConfig>(initialRule);
  const [draftConfig, setDraftConfig] = useState(() =>
    buildDraftConfig(canUseSchemaForm ? schema : null, initialRule.config)
  );
  const [fieldErrors, setFieldErrors] = useState<Record<string, string>>({});
  const [rawConfigText, setRawConfigText] = useState(
    JSON.stringify(initialRule.config, null, 2)
  );

  const displayName = registryItem
    ? resolveLocalizedText(registryItem.name, i18n.language, registryItem.type_id)
    : rule.transform;
  const displayDescription = registryItem
    ? resolveLocalizedText(registryItem.description, i18n.language, "")
    : "";

  const updateDraftField = (key: string, draft: DraftValue) => {
    setDraftConfig((prev) => ({
      ...prev,
      drafts: { ...prev.drafts, [key]: draft },
    }));
    setFieldErrors((prev) => {
      const next: Record<string, string> = {};
      for (const [path, message] of Object.entries(prev)) {
        if (path !== key && !path.startsWith(`${key}.`)) {
          next[path] = message;
        }
      }
      return next;
    });
  };

  const validateAndSave = () => {
    const candidate: TransformRuleConfig = {
      ...draftRule,
      config: { ...draftRule.config },
    };

    if (canUseSchemaForm) {
      const serialized = serializeDraftConfig(schema, draftConfig);
      if (serialized.errors.length > 0) {
        const nextErrors: Record<string, string> = {};
        for (const error of serialized.errors) {
          if (!nextErrors[error.path]) {
            nextErrors[error.path] = error.message;
          }
        }
        setFieldErrors(nextErrors);
        return;
      }
      candidate.config = serialized.config;
    } else if (!isUnknownTransform) {
      try {
        const parsed = JSON.parse(rawConfigText);
        if (!isRecord(parsed)) {
          setFieldErrors({ config: t("transforms.validationConfigObject") });
          return;
        }
        candidate.config = parsed;
      } catch {
        setFieldErrors({ config: t("transforms.validationInvalidJson") });
        return;
      }
    }

    const validationErrors = validateTransformRule(candidate, registryItem);
    if (validationErrors.length > 0) {
      const nextErrors: Record<string, string> = {};
      for (const item of validationErrors) {
        if (!nextErrors[item.field]) {
          nextErrors[item.field] = item.message;
        }
      }
      setFieldErrors(nextErrors);
      return;
    }

    onSave(candidate);
    onOpenChange(false);
  };

  const copyConfigJson = async (text: string) => {
    try {
      await navigator.clipboard.writeText(text);
      toast.success(t("transforms.jsonCopied"));
    } catch {
      toast.error(t("transforms.jsonCopyFailed"));
    }
  };

  const formatRawConfig = () => {
    try {
      setRawConfigText(JSON.stringify(JSON.parse(rawConfigText.trim()), null, 2));
    } catch {
      toast.error(t("transforms.validationInvalidJson"));
    }
  };

  return (
    <Dialog open={open} onOpenChange={onOpenChange}>
      <DialogContent className="max-h-[calc(100dvh-2rem)] overflow-hidden p-0 sm:max-h-[calc(100dvh-3rem)] sm:max-w-2xl">
        <div className="flex min-h-0 flex-col p-6">
        <DialogHeader className="shrink-0">
          <DialogTitle>{displayName}</DialogTitle>
          <p className="font-mono text-xs text-muted-foreground">{draftRule.transform}</p>
          <DialogDescription>
            {displayDescription ? `${displayDescription} ` : ""}
            {t("transforms.configureRule", { phase: draftRule.phase })}
          </DialogDescription>
        </DialogHeader>

        <div className="min-h-0 flex-1 space-y-5 overflow-y-auto py-2 pr-1">
          <div className="space-y-2">
            <Label>{t("transforms.modelsFilter")}</Label>
            <ModelsGlobInput
              value={draftRule.models}
              onChange={(models) => setDraftRule((prev) => ({ ...prev, models }))}
            />
          </div>

          <div className="space-y-2">
            <Label>{t("transforms.config")}</Label>
            {isUnknownTransform && (
              <div className="rounded-md border border-warning-border bg-warning-soft p-3 text-xs text-warning-foreground">
                <div className="flex items-center gap-2">
                  <AlertTriangle className="h-4 w-4" />
                  <span>{t("transforms.unknownRuleReadOnly")}</span>
                </div>
              </div>
            )}

            {canUseSchemaForm ? (
              <SchemaFormFields
                schema={schema}
                draftConfig={draftConfig}
                errors={fieldErrors}
                onDraftChange={updateDraftField}
              />
            ) : (
              <div className="space-y-2">
                <div className="flex items-center gap-1">
                  {!isUnknownTransform && (
                    <Button
                      type="button"
                      variant="ghost"
                      size="icon"
                      className="size-11 touch-manipulation sm:size-8"
                      aria-label={t("transforms.jsonFormat")}
                      onClick={formatRawConfig}
                    >
                      <Braces className="h-3.5 w-3.5" />
                    </Button>
                  )}
                  <Button
                    type="button"
                    variant="ghost"
                    size="icon"
                    className="size-11 touch-manipulation sm:size-8"
                    aria-label={t("transforms.jsonCopy")}
                    onClick={() =>
                      copyConfigJson(
                        isUnknownTransform
                          ? JSON.stringify(draftRule.config, null, 2)
                          : rawConfigText
                      )
                    }
                  >
                    <Copy className="h-3.5 w-3.5" />
                  </Button>
                </div>
                <Textarea
                  rows={8}
                  className="font-mono text-xs"
                  value={isUnknownTransform ? JSON.stringify(draftRule.config, null, 2) : rawConfigText}
                  readOnly={isUnknownTransform}
                  onChange={(e) => setRawConfigText(e.target.value)}
                />
                {fieldErrors.config && (
                  <p className="text-xs text-destructive">{fieldErrors.config}</p>
                )}
              </div>
            )}
          </div>
        </div>

        <DialogFooter className="shrink-0 pt-4">
          <Button variant="outline" onClick={() => onOpenChange(false)}>
            {t("common.cancel")}
          </Button>
          <Button onClick={validateAndSave}>{t("common.save")}</Button>
        </DialogFooter>
        </div>
      </DialogContent>
    </Dialog>
  );
}

function buildInitialRule(rule: TransformRuleConfig): TransformRuleConfig {
  const safeConfig = isRecord(rule.config) ? rule.config : {};
  return {
    ...rule,
    models: normalizeModels(rule.models),
    config: { ...safeConfig },
  };
}

function normalizeModels(models: string[] | null | undefined): string[] | null {
  if (!models || models.length === 0) {
    return null;
  }
  return models;
}

function isRecord(value: unknown): value is Record<string, unknown> {
  return typeof value === "object" && value !== null && !Array.isArray(value);
}
