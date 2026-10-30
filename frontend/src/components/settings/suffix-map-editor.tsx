import { useTranslation } from "react-i18next";
import { Plus, X } from "lucide-react";

import { Button } from "@/components/ui/button";
import { FieldError } from "@/components/ui/field";
import { Input } from "@/components/ui/input";
import {
  Select,
  SelectContent,
  SelectItem,
  SelectTrigger,
  SelectValue,
} from "@/components/ui/select";
import { createSuffixRow, duplicateSuffixRowIds, type SuffixRow } from "./suffix-rows";

const EFFORT_VALUES = ["none", "minimum", "low", "medium", "high", "xhigh", "max"] as const;

interface SuffixMapEditorProps {
  rows: SuffixRow[];
  onRowsChange: (rows: SuffixRow[]) => void;
}

/**
 * Row-based editor for the reasoning suffix -> effort map. The page owns the rows so
 * blank and duplicate rows survive category changes (SSU-20c).
 */
export function SuffixMapEditor({ rows, onRowsChange }: SuffixMapEditorProps) {
  const { t } = useTranslation();
  const duplicates = duplicateSuffixRowIds(rows);

  const updateRow = (id: number, updates: Partial<SuffixRow>) => {
    onRowsChange(rows.map((row) => (row.id === id ? { ...row, ...updates } : row)));
  };

  return (
    <div className="flex flex-col gap-4">
      {rows.map((row, index) => {
        const duplicate = duplicates.has(row.id);
        return (
          <div key={row.id} className="flex flex-col gap-1.5">
            <div className="flex items-center gap-2">
              <Input
                value={row.suffix}
                placeholder={t("settings.suffix")}
                aria-label={t("settings.suffixRowLabel", { index: index + 1 })}
                aria-invalid={duplicate || undefined}
                className="flex-1 transition-all"
                autoComplete="off"
                spellCheck={false}
                onChange={(e) => updateRow(row.id, { suffix: e.target.value })}
              />
              <Select value={row.effort} onValueChange={(effort) => updateRow(row.id, { effort })}>
                <SelectTrigger
                  className="w-[140px]"
                  aria-label={t("settings.suffixEffortRowLabel", { index: index + 1 })}
                >
                  <SelectValue />
                </SelectTrigger>
                <SelectContent>
                  {EFFORT_VALUES.map((value) => (
                    <SelectItem key={value} value={value}>
                      {value}
                    </SelectItem>
                  ))}
                </SelectContent>
              </Select>
              <Button
                type="button"
                variant="ghost"
                size="icon"
                className="size-11 touch-manipulation sm:size-9"
                aria-label={t("settings.removeSuffixRow", { index: index + 1 })}
                onClick={() => onRowsChange(rows.filter((item) => item.id !== row.id))}
              >
                <X aria-hidden="true" />
              </Button>
            </div>
            {duplicate ? <FieldError>{t("settings.suffixDuplicate")}</FieldError> : null}
          </div>
        );
      })}
      <div className="flex flex-wrap items-center justify-between gap-3">
        <Button
          type="button"
          variant="outline"
          size="sm"
          onClick={() => onRowsChange([...rows, createSuffixRow()])}
        >
          <Plus data-icon="inline-start" aria-hidden="true" />
          {t("settings.addSuffix")}
        </Button>
        <p className="text-sm text-muted-foreground">{t("settings.effortValues")}</p>
      </div>
    </div>
  );
}
