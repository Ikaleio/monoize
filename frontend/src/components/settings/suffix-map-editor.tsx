import { useCallback, useEffect, useRef, useState } from "react";
import { useTranslation } from "react-i18next";
import { Plus, X } from "lucide-react";

import { Button } from "@/components/ui/button";
import { Input } from "@/components/ui/input";
import {
  Select,
  SelectContent,
  SelectItem,
  SelectTrigger,
  SelectValue,
} from "@/components/ui/select";

const EFFORT_VALUES = ["none", "minimum", "low", "medium", "high", "xhigh", "max"] as const;

interface SuffixRow {
  id: number;
  suffix: string;
  effort: string;
}

let suffixRowId = 0;

function mapToRows(map: Record<string, string> | undefined): SuffixRow[] {
  return Object.entries(map ?? {}).map(([suffix, effort]) => ({
    id: ++suffixRowId,
    suffix,
    effort,
  }));
}

function rowsToMap(rows: SuffixRow[]): Record<string, string> {
  const map: Record<string, string> = {};
  for (const row of rows) {
    if (row.suffix) map[row.suffix] = row.effort;
  }
  return map;
}

interface SuffixMapEditorProps {
  value: Record<string, string> | undefined;
  onChange: (map: Record<string, string>) => void;
}

/**
 * Row-based editor for the reasoning suffix -> effort map. Suffix edits commit
 * on blur; effort and row removal commit immediately.
 */
export function SuffixMapEditor({ value, onChange }: SuffixMapEditorProps) {
  const { t } = useTranslation();
  const [rows, setRows] = useState<SuffixRow[]>(() => mapToRows(value));
  const prevValueRef = useRef(value);

  useEffect(() => {
    if (prevValueRef.current !== value) {
      prevValueRef.current = value;
      setRows(mapToRows(value));
    }
  }, [value]);

  const commit = useCallback(
    (updated: SuffixRow[]) => {
      setRows(updated);
      onChange(rowsToMap(updated));
    },
    [onChange]
  );

  return (
    <div className="flex flex-col gap-4">
      {rows.map((row, idx) => (
        <div key={row.id} className="flex items-center gap-2">
          <Input
            defaultValue={row.suffix}
            placeholder={t("settings.suffix")}
            className="flex-1 transition-all"
            onBlur={(e) => {
              const updated = rows.map((r, i) =>
                i === idx ? { ...r, suffix: e.target.value } : r
              );
              commit(updated);
            }}
          />
          <Select
            value={row.effort}
            onValueChange={(val) => {
              const updated = rows.map((r, i) =>
                i === idx ? { ...r, effort: val } : r
              );
              commit(updated);
            }}
          >
            <SelectTrigger className="w-[140px]">
              <SelectValue />
            </SelectTrigger>
            <SelectContent>
              {EFFORT_VALUES.map((v) => (
                <SelectItem key={v} value={v}>
                  {v}
                </SelectItem>
              ))}
            </SelectContent>
          </Select>
          <Button
            variant="ghost"
            size="icon"
            className="size-11 touch-manipulation sm:size-9"
            aria-label={t("common.delete")}
            onClick={() => commit(rows.filter((_, i) => i !== idx))}
          >
            <X className="h-4 w-4" />
          </Button>
        </div>
      ))}
      <div className="flex flex-wrap items-center justify-between gap-3">
        <Button
          variant="outline"
          size="sm"
          onClick={() => {
            setRows([...rows, { id: ++suffixRowId, suffix: "", effort: "high" }]);
          }}
        >
          <Plus className="mr-2 h-4 w-4" />
          {t("settings.addSuffix")}
        </Button>
        <p className="text-sm text-muted-foreground">{t("settings.effortValues")}</p>
      </div>
    </div>
  );
}
