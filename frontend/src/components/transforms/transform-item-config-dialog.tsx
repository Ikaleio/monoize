import { useMemo, useState } from "react";
import { useTranslation } from "react-i18next";
import { AlertTriangle } from "lucide-react";
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
import { ModelsGlobInput } from "./models-glob-input";
import { SchemaFormFields } from "./schema-form-fields";
import { getSchemaObject, validateTransformRule } from "./transform-schema";

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
  const { t } = useTranslation();
  const initialRule = useMemo(() => buildInitialRule(rule), [rule]);
  const [draftRule, setDraftRule] = useState<TransformRuleConfig>(initialRule);
  const [fieldErrors, setFieldErrors] = useState<Record<string, string>>({});
  const [rawJsonInputs, setRawJsonInputs] = useState<Record<string, string>>(
    buildRawJsonInputs(initialRule, registryItem)
  );
  const [rawConfigText, setRawConfigText] = useState(JSON.stringify(initialRule.config, null, 2));

  const schema = registryItem ? getSchemaObject(registryItem.config_schema) : null;
  const canUseSchemaForm = Boolean(registryItem && schema?.type === "object");
  const isUnknownTransform = !registryItem;

  const updateConfigField = (key: string, value: unknown) => {
    setDraftRule((prev) => {
      const nextConfig = { ...prev.config };
      if (value === undefined) {
        delete nextConfig[key];
      } else {
        nextConfig[key] = value;
      }
      return { ...prev, config: nextConfig };
    });
    setFieldErrors((prev) => {
      const next = { ...prev };
      delete next[key];
      return next;
    });
  };

  const commitRawJsonInput = (key: string): boolean => {
    const raw = rawJsonInputs[key];
    if (raw === undefined) {
      return true;
    }
    try {
      const parsed = JSON.parse(raw);
      updateConfigField(key, parsed);
      return true;
    } catch {
      setFieldErrors((prev) => ({
        ...prev,
        [key]: t("transforms.validationInvalidJson"),
      }));
      return false;
    }
  };

  const validateAndSave = () => {
    const candidate: TransformRuleConfig = {
      ...draftRule,
      config: { ...draftRule.config },
    };

    for (const [key, raw] of Object.entries(rawJsonInputs)) {
      try {
        candidate.config[key] = JSON.parse(raw);
      } catch {
        setFieldErrors((prev) => ({
          ...prev,
          [key]: t("transforms.validationInvalidJson"),
        }));
        return;
      }
    }

    if (!canUseSchemaForm && !isUnknownTransform) {
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

  return (
    <Dialog open={open} onOpenChange={onOpenChange}>
      <DialogContent className="max-w-2xl max-h-[85vh] overflow-y-auto">
        <DialogHeader>
          <DialogTitle className="font-mono">{draftRule.transform}</DialogTitle>
          <DialogDescription>
            {t("transforms.configureRule", { phase: draftRule.phase })}
          </DialogDescription>
        </DialogHeader>

        <div className="space-y-5 py-2">
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
              <div className="rounded-md border border-amber-500/40 bg-amber-500/10 p-3 text-xs text-amber-700 dark:text-amber-300">
                <div className="flex items-center gap-2">
                  <AlertTriangle className="h-4 w-4" />
                  <span>{t("transforms.unknownRuleReadOnly")}</span>
                </div>
              </div>
            )}

            {canUseSchemaForm ? (
              <SchemaFormFields
                schema={schema}
                config={draftRule.config}
                errors={fieldErrors}
                rawJsonInputs={rawJsonInputs}
                onFieldChange={updateConfigField}
                onRawJsonInputChange={(key, value) =>
                  setRawJsonInputs((prev) => ({ ...prev, [key]: value }))
                }
                onRawJsonInputCommit={commitRawJsonInput}
              />
            ) : (
              <div className="space-y-2">
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

        <DialogFooter>
          <Button variant="outline" onClick={() => onOpenChange(false)}>
            {t("common.cancel")}
          </Button>
          <Button onClick={validateAndSave}>{t("common.save")}</Button>
        </DialogFooter>
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

function buildRawJsonInputs(
  rule: TransformRuleConfig,
  registryItem?: TransformRegistryItem
): Record<string, string> {
  const schema = registryItem ? getSchemaObject(registryItem.config_schema) : null;
  const properties = schema?.properties ?? {};
  const rawByKey: Record<string, string> = {};
  for (const [key, property] of Object.entries(properties)) {
    if (property?.type || Array.isArray(property?.enum)) {
      continue;
    }
    rawByKey[key] = JSON.stringify(
      Object.prototype.hasOwnProperty.call(rule.config, key) ? rule.config[key] : null,
      null,
      2
    );
  }
  return rawByKey;
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
