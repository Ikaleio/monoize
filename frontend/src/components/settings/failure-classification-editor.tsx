import { ArrowDown, ArrowUp, Plus, X } from "lucide-react";
import { useTranslation } from "react-i18next";

import { Button } from "@/components/ui/button";
import { Field, FieldDescription, FieldError, FieldGroup, FieldLabel } from "@/components/ui/field";
import { Input } from "@/components/ui/input";
import { Select, SelectContent, SelectItem, SelectTrigger, SelectValue } from "@/components/ui/select";
import {
  CLASSIFICATION_RULE_LIMIT,
  FAILURE_CLASSES,
  classificationInputId,
  createClassificationRow,
  type ClassificationRow,
  type ClassificationRowErrors,
} from "./failure-classification-rows";
import { useSettingsRowState } from "./settings-context";

interface FailureClassificationEditorProps {
  rows: ClassificationRow[];
  errors?: ClassificationRowErrors[];
  disabled: boolean;
  onChange: (rows: ClassificationRow[]) => void;
}

export function FailureClassificationEditor({ rows, errors, disabled, onChange }: FailureClassificationEditorProps) {
  const { t } = useTranslation();
  const { fieldError } = useSettingsRowState();
  const serverRule = fieldError?.classificationRule;
  const update = (index: number, patch: Partial<ClassificationRow>) => {
    onChange(rows.map((row, i) => (i === index ? { ...row, ...patch } : row)));
  };
  const move = (index: number, direction: -1 | 1) => {
    const adjacent = index + direction;
    if (adjacent < 0 || adjacent >= rows.length) return;
    const next = [...rows];
    [next[index], next[adjacent]] = [next[adjacent], next[index]];
    onChange(next);
  };

  return (
    <FieldGroup className="gap-4">
      {rows.length === 0 && (
        <FieldDescription className="rounded-md border border-dashed p-4">
          {t("settings.failureClassificationEmpty")}
        </FieldDescription>
      )}
      {rows.map((rule, index) => {
        const rowLabel = t("settings.failureClassificationRow", { index: index + 1 });
        const rowError = serverRule?.index === index ? fieldError?.message : undefined;
        const rowErrorField = rowError ? serverRule?.field : undefined;
        const statusId = classificationInputId(index, "status_codes");
        const keywordId = classificationInputId(index, "keyword");
        const classId = classificationInputId(index, "class");
        const statusError = errors?.[index]?.status_codes
          ? t("settings.failureClassificationStatusInvalid")
          : rowErrorField === "status_codes" ? rowError : undefined;
        const keywordError = errors?.[index]?.keyword
          ? t("settings.failureClassificationKeywordInvalid")
          : rowErrorField === "keyword" ? rowError : undefined;
        const classError = errors?.[index]?.class
          ? t("settings.failureClassificationClassInvalid")
          : rowErrorField === "class" ? rowError : undefined;

        return (
          <fieldset key={rule.id} className="min-w-0 space-y-3 rounded-md border px-4 pb-4">
            <legend className="px-1 text-sm font-medium">{rowLabel}</legend>
            <div className="grid min-w-0 gap-3 [@container(min-width:40rem)]:grid-cols-[minmax(0,1fr)_minmax(0,1.5fr)_minmax(0,1fr)]">
              <Field className="min-w-0 gap-2" data-invalid={Boolean(statusError)}>
                <FieldLabel htmlFor={statusId}>{t("settings.failureClassificationStatusCodes")}</FieldLabel>
                <Input
                  id={statusId}
                  value={rule.status_codes}
                  placeholder={t("settings.failureClassificationAnyStatus")}
                  disabled={disabled}
                  autoComplete="off"
                  aria-invalid={statusError ? true : undefined}
                  aria-describedby={statusError ? `${statusId}-error` : undefined}
                  onChange={(event) => update(index, { status_codes: event.target.value })}
                />
                {statusError && <FieldError id={`${statusId}-error`}>{statusError}</FieldError>}
              </Field>
              <Field className="min-w-0 gap-2" data-invalid={Boolean(keywordError)}>
                <FieldLabel htmlFor={keywordId}>{t("settings.failureClassificationKeyword")}</FieldLabel>
                <Input
                  id={keywordId}
                  value={rule.keyword}
                  disabled={disabled}
                  autoComplete="off"
                  aria-invalid={keywordError ? true : undefined}
                  aria-describedby={keywordError ? `${keywordId}-error` : undefined}
                  onChange={(event) => update(index, { keyword: event.target.value })}
                />
                {keywordError && <FieldError id={`${keywordId}-error`}>{keywordError}</FieldError>}
              </Field>
              <Field className="min-w-0 gap-2" data-invalid={Boolean(classError)}>
                <FieldLabel htmlFor={classId}>{t("settings.failureClassificationClass")}</FieldLabel>
                <Select
                  value={rule.class}
                  disabled={disabled}
                  onValueChange={(value) => {
                    const failureClass = FAILURE_CLASSES.find((option) => option === value);
                    if (failureClass) update(index, { class: failureClass });
                  }}
                >
                  <SelectTrigger
                    id={classId}
                    aria-invalid={classError ? true : undefined}
                    aria-describedby={classError ? `${classId}-error` : undefined}
                  >
                    <SelectValue />
                  </SelectTrigger>
                  <SelectContent>
                    {FAILURE_CLASSES.map((value) => (
                      <SelectItem key={value} value={value}>
                        {t(`settings.failureClassificationClasses.${value}`)}
                      </SelectItem>
                    ))}
                  </SelectContent>
                </Select>
                {classError && <FieldError id={`${classId}-error`}>{classError}</FieldError>}
              </Field>
            </div>
            {rowError && !rowErrorField && <FieldError>{rowError}</FieldError>}
            <div className="flex justify-end gap-1">
              <Button type="button" size="icon" variant="ghost" disabled={disabled || index === 0} aria-label={t("settings.moveClassificationRuleUp", { index: index + 1 })} onClick={() => move(index, -1)}>
                <ArrowUp aria-hidden="true" />
              </Button>
              <Button type="button" size="icon" variant="ghost" disabled={disabled || index === rows.length - 1} aria-label={t("settings.moveClassificationRuleDown", { index: index + 1 })} onClick={() => move(index, 1)}>
                <ArrowDown aria-hidden="true" />
              </Button>
              <Button type="button" size="icon" variant="ghost" disabled={disabled} aria-label={t("settings.removeClassificationRule", { index: index + 1 })} onClick={() => onChange(rows.filter((_, i) => i !== index))}>
                <X aria-hidden="true" />
              </Button>
            </div>
          </fieldset>
        );
      })}
      {rows.length > CLASSIFICATION_RULE_LIMIT && (
        <FieldError>{t("settings.failureClassificationRuleLimit", { count: CLASSIFICATION_RULE_LIMIT })}</FieldError>
      )}
      <Button type="button" variant="outline" size="sm" className="self-start" disabled={disabled || rows.length >= CLASSIFICATION_RULE_LIMIT} onClick={() => onChange([...rows, createClassificationRow()])}>
        <Plus data-icon="inline-start" aria-hidden="true" />
        {t("settings.addClassificationRule")}
      </Button>
    </FieldGroup>
  );
}
