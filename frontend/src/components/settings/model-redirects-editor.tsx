import { useEffect, useId, useState } from "react";
import { ArrowRight, Plus, X } from "lucide-react";
import { useTranslation } from "react-i18next";

import { Button } from "@/components/ui/button";
import {
  Field,
  FieldDescription,
  FieldError,
  FieldGroup,
  FieldLabel,
} from "@/components/ui/field";
import { Input } from "@/components/ui/input";
import type { ModelRedirectRule } from "@/lib/api";
import { isRedirectPatternInvalid } from "./redirect-pattern";

interface ModelRedirectsEditorProps {
  value: ModelRedirectRule[];
  onChange: (value: ModelRedirectRule[]) => void;
  disabled?: boolean;
  /** Shown when no rule exists; states what happens without rules in this scope. */
  emptyText: string;
  /** Row rejected by the server on the last save; focused when set. */
  serverErrorIndex?: number;
}

/** Ordered regex redirect rules (frontend-design-system.spec.md DS64). */
export function ModelRedirectsEditor({
  value,
  onChange,
  disabled = false,
  emptyText,
  serverErrorIndex,
}: ModelRedirectsEditorProps) {
  const { t } = useTranslation();
  const idPrefix = useId();
  // Patterns are validated once blurred. Tracking the text rather than the row index keeps
  // errors correct when rows are removed, and hides them while the pattern is being edited.
  const [blurredPatterns, setBlurredPatterns] = useState<ReadonlySet<string>>(new Set());

  useEffect(() => {
    if (serverErrorIndex === undefined) return;
    document.getElementById(`${idPrefix}-pattern-${serverErrorIndex}`)?.focus();
  }, [serverErrorIndex, idPrefix]);

  const updateRule = (
    index: number,
    field: keyof ModelRedirectRule,
    nextValue: string
  ) => {
    onChange(
      value.map((rule, ruleIndex) =>
        ruleIndex === index ? { ...rule, [field]: nextValue } : rule
      )
    );
  };

  return (
    <FieldGroup className="gap-4">
      {value.length === 0 ? (
        <FieldDescription className="rounded-md border border-dashed p-4">
          {emptyText}
        </FieldDescription>
      ) : (
        value.map((rule, index) => {
          const patternId = `${idPrefix}-pattern-${index}`;
          const replacementId = `${idPrefix}-replacement-${index}`;
          const patternInvalid =
            index === serverErrorIndex ||
            (blurredPatterns.has(rule.pattern) && isRedirectPatternInvalid(rule.pattern));

          return (
            <div
              key={`${idPrefix}-${index}`}
              className="grid min-w-0 gap-3 rounded-md border p-4 sm:grid-cols-[minmax(0,1fr)_auto_minmax(0,1fr)_auto] sm:items-end"
            >
              <Field className="min-w-0">
                <FieldLabel htmlFor={patternId}>
                  {t("settings.modelRedirectPattern")}
                </FieldLabel>
                <Input
                  id={patternId}
                  value={rule.pattern}
                  disabled={disabled}
                  placeholder="claude-.*"
                  autoComplete="off"
                  spellCheck={false}
                  aria-invalid={patternInvalid || undefined}
                  onChange={(event) =>
                    updateRule(index, "pattern", event.target.value)
                  }
                  onBlur={() =>
                    setBlurredPatterns((previous) => new Set(previous).add(rule.pattern))
                  }
                />
              </Field>
              {patternInvalid ? (
                <FieldError className="sm:col-start-1 sm:row-start-2">
                  {t("settings.modelRedirectPatternInvalid")}
                </FieldError>
              ) : null}

              <div className="flex justify-center sm:h-9 sm:items-center">
                <ArrowRight
                  aria-hidden="true"
                  className="size-6 text-muted-foreground rotate-90 sm:rotate-0"
                />
              </div>

              <Field className="min-w-0">
                <FieldLabel htmlFor={replacementId}>
                  {t("settings.modelRedirectReplacement")}
                </FieldLabel>
                <Input
                  id={replacementId}
                  value={rule.replace}
                  disabled={disabled}
                  placeholder="gpt-5.6-sol"
                  autoComplete="off"
                  spellCheck={false}
                  onChange={(event) =>
                    updateRule(index, "replace", event.target.value)
                  }
                />
              </Field>

              <Button
                type="button"
                variant="ghost"
                size="icon"
                disabled={disabled}
                className="justify-self-end sm:mb-0"
                aria-label={t("settings.removeModelRedirect", {
                  index: index + 1,
                })}
                onClick={() =>
                  onChange(value.filter((_, ruleIndex) => ruleIndex !== index))
                }
              >
                <X aria-hidden="true" />
              </Button>
            </div>
          );
        })
      )}

      <div className="flex flex-wrap items-center justify-between gap-3">
        <Button
          type="button"
          variant="outline"
          size="sm"
          disabled={disabled || value.length >= 32}
          onClick={() => onChange([...value, { pattern: "", replace: "" }])}
        >
          <Plus data-icon="inline-start" aria-hidden="true" />
          {t("settings.addModelRedirect")}
        </Button>
        <FieldDescription>
          {t("settings.globalModelRedirectsHelp")}
        </FieldDescription>
      </div>
    </FieldGroup>
  );
}
