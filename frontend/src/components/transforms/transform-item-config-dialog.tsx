import { useId, useMemo, useRef, useState, type FormEvent } from "react";
import { useTranslation } from "react-i18next";
import { AlertTriangle, Braces } from "lucide-react";
import { toast } from "sonner";
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
import { Button } from "@/components/ui/button";
import { CopyButton } from "@/components/ui/copy-button";
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
  formatTransformValidationError,
  getSchemaObject,
  isRecord,
  serializeDraftConfig,
  validateTransformRule,
  type DraftConfig,
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
  const modelsId = useId();
  const rawConfigId = useId();
  const formRef = useRef<HTMLFormElement>(null);
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
  const [confirmDiscardOpen, setConfirmDiscardOpen] = useState(false);
  // TCU-16: the baseline is taken after TCU-8 initialization so that required
  // fields opening with default drafts do not count as user edits.
  const [initialSnapshot] = useState(() =>
    dirtySnapshot(draftRule.models, draftConfig, rawConfigText)
  );
  const dirty = dirtySnapshot(draftRule.models, draftConfig, rawConfigText) !== initialSnapshot;

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

  const showErrors = (nextErrors: Record<string, string>) => {
    setFieldErrors(nextErrors);
    // TCU-15: focus the first invalid control once the error state has rendered.
    requestAnimationFrame(() => {
      formRef.current?.querySelector<HTMLElement>('[aria-invalid="true"]')?.focus();
    });
  };

  const validateAndSave = (event: FormEvent<HTMLFormElement>) => {
    event.preventDefault();
    // React propagates submit through the portal; hosts such as the Provider editor
    // render this dialog inside their own <form>.
    event.stopPropagation();
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
            nextErrors[error.path] = formatTransformValidationError(error, t);
          }
        }
        showErrors(nextErrors);
        return;
      }
      candidate.config = serialized.config;
    } else if (!isUnknownTransform) {
      try {
        const parsed = JSON.parse(rawConfigText);
        if (!isRecord(parsed)) {
          showErrors({ config: formatTransformValidationError({ code: "configNotObject" }, t) });
          return;
        }
        candidate.config = parsed;
      } catch {
        showErrors({ config: formatTransformValidationError({ code: "invalidJson" }, t) });
        return;
      }
    }

    const validationErrors = validateTransformRule(candidate, registryItem);
    if (validationErrors.length > 0) {
      const nextErrors: Record<string, string> = {};
      for (const item of validationErrors) {
        if (!nextErrors[item.field]) {
          nextErrors[item.field] = formatTransformValidationError(item, t);
        }
      }
      showErrors(nextErrors);
      return;
    }

    onSave(candidate);
    onOpenChange(false);
  };

  const cancel = () => {
    if (dirty) {
      setConfirmDiscardOpen(true);
      return;
    }
    onOpenChange(false);
  };

  const formatRawConfig = () => {
    try {
      setRawConfigText(JSON.stringify(JSON.parse(rawConfigText.trim()), null, 2));
    } catch {
      toast.error(formatTransformValidationError({ code: "invalidJson" }, t));
    }
  };

  const rawConfigValue = isUnknownTransform
    ? JSON.stringify(draftRule.config, null, 2)
    : rawConfigText;

  return (
    <Dialog open={open} onOpenChange={onOpenChange}>
      <DialogContent
        dirty={dirty}
        /* Radix requires an explicit undefined when the dialog has no description. */
        {...(displayDescription ? {} : { "aria-describedby": undefined })}
        className="max-h-[calc(100dvh-2rem)] overflow-hidden p-0 sm:max-h-[calc(100dvh-3rem)] sm:max-w-2xl"
      >
        <form ref={formRef} noValidate onSubmit={validateAndSave} className="flex min-h-0 flex-col p-6">
          <DialogHeader className="shrink-0">
            <DialogTitle>{displayName}</DialogTitle>
            <p className="font-mono text-xs text-muted-foreground">{draftRule.transform}</p>
            {displayDescription && <DialogDescription>{displayDescription}</DialogDescription>}
          </DialogHeader>

          <div className="min-h-0 flex-1 space-y-5 overflow-y-auto py-2 pr-1">
            <div className="space-y-2">
              <Label htmlFor={modelsId}>{t("transforms.modelsFilter")}</Label>
              <ModelsGlobInput
                id={modelsId}
                value={draftRule.models}
                onChange={(models) => setDraftRule((prev) => ({ ...prev, models }))}
              />
            </div>

            <div className="space-y-2">
              {canUseSchemaForm ? (
                <p className="text-sm font-medium">{t("transforms.config")}</p>
              ) : (
                <Label htmlFor={rawConfigId}>{t("transforms.config")}</Label>
              )}
              {isUnknownTransform && (
                <div className="rounded-md border border-warning-border bg-warning-soft p-3 text-xs text-warning-foreground">
                  <div className="flex items-center gap-2">
                    <AlertTriangle className="h-4 w-4" aria-hidden="true" />
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
                        <Braces aria-hidden="true" />
                      </Button>
                    )}
                    <CopyButton
                      className="sm:size-8"
                      label={t("transforms.jsonCopy")}
                      value={rawConfigValue}
                    />
                  </div>
                  <Textarea
                    id={rawConfigId}
                    rows={8}
                    className="font-mono text-xs"
                    value={rawConfigValue}
                    readOnly={isUnknownTransform}
                    aria-invalid={fieldErrors.config ? true : undefined}
                    aria-describedby={fieldErrors.config ? `${rawConfigId}-error` : undefined}
                    onChange={(e) => setRawConfigText(e.target.value)}
                  />
                  {fieldErrors.config && (
                    <p id={`${rawConfigId}-error`} className="text-xs text-error-foreground">
                      {fieldErrors.config}
                    </p>
                  )}
                </div>
              )}
            </div>
          </div>

          <DialogFooter className="shrink-0 pt-4">
            <Button type="button" variant="outline" onClick={cancel}>
              {t("common.cancel")}
            </Button>
            <Button type="submit">{t("common.save")}</Button>
          </DialogFooter>
        </form>
      </DialogContent>

      <AlertDialog open={confirmDiscardOpen} onOpenChange={setConfirmDiscardOpen}>
        <AlertDialogContent>
          <AlertDialogHeader>
            <AlertDialogTitle>{t("transforms.discardTitle")}</AlertDialogTitle>
            <AlertDialogDescription>{t("transforms.discardDescription")}</AlertDialogDescription>
          </AlertDialogHeader>
          <AlertDialogFooter>
            <AlertDialogCancel>{t("transforms.keepEditing")}</AlertDialogCancel>
            <AlertDialogAction onClick={() => onOpenChange(false)}>
              {t("transforms.discard")}
            </AlertDialogAction>
          </AlertDialogFooter>
        </AlertDialogContent>
      </AlertDialog>
    </Dialog>
  );
}

/** Serialized comparison key for TCU-16 dirty detection. */
function dirtySnapshot(
  models: string[] | null | undefined,
  draftConfig: DraftConfig,
  rawConfigText: string
): string {
  return JSON.stringify([models ?? null, draftConfig, rawConfigText]);
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
