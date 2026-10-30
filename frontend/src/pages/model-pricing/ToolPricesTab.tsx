import { useEffect, useState, type ComponentProps, type ReactElement } from "react";
import { useTranslation } from "react-i18next";
import { Hammer, Plus, Trash2 } from "lucide-react";
import { toast } from "sonner";
import { Button } from "@/components/ui/button";
import { Input } from "@/components/ui/input";
import { FieldError } from "@/components/ui/field";
import { SettingsSaveBar } from "@/components/settings/settings-save-bar";
import { useUnsavedChangesGuard } from "@/hooks/use-unsaved-changes-guard";
import { toastApiError } from "@/lib/api-error";
import { EmptyState } from "@/components/ui/empty-state";
import { QueryError } from "@/components/ui/query-error";
import { TablePageSkeleton } from "@/components/ui/page-skeleton";
import {
  DataTableShell,
  VirtualTableCell,
  VirtualTableHeaderCell,
} from "@/components/ui/data-table-shell";
import {
  Select,
  SelectContent,
  SelectItem,
  SelectTrigger,
  SelectValue,
} from "@/components/ui/select";
import type { ToolPriceEntry, ToolPriceUnit } from "@/lib/api";
import { patchPricingSettingsOptimistic, useSettings } from "@/lib/swr";
import { isValidUsdDecimal } from "./shared";

interface ToolPriceRow {
  usageClass: string;
  usd: string;
  per: ToolPriceUnit;
  minimumUnits: string;
}

const UNITS: ToolPriceUnit[] = ["1k_calls", "minute", "session"];

function rowsFromSettings(toolPrices: Record<string, ToolPriceEntry>): ToolPriceRow[] {
  return Object.entries(toolPrices)
    .map(([usageClass, entry]): ToolPriceRow => {
      if (typeof entry === "number" || typeof entry === "string") {
        return { usageClass, usd: String(entry), per: "1k_calls", minimumUnits: "" };
      }
      return {
        usageClass,
        usd: String(entry.usd),
        per: entry.per,
        minimumUnits: entry.minimum_units != null ? String(entry.minimum_units) : "",
      };
    })
    .sort((a, b) => a.usageClass.localeCompare(b.usageClass));
}

function rowsToToolPrices(rows: ToolPriceRow[]): Record<string, ToolPriceEntry> {
  const result: Record<string, ToolPriceEntry> = {};
  for (const row of rows) {
    const usd = row.usd.trim();
    // Plain string keeps the new-api-compatible shorthand for 1k_calls rows
    // without a minimum (model-pricing.spec.md tool_prices schema).
    if (row.per === "1k_calls") {
      result[row.usageClass.trim()] = usd;
    } else {
      result[row.usageClass.trim()] = {
        usd,
        per: row.per,
        ...(row.minimumUnits.trim()
          ? { minimum_units: Number.parseInt(row.minimumUnits.trim(), 10) }
          : {}),
      };
    }
  }
  return result;
}


export function ToolPricesTab({ onDirtyChange }: { onDirtyChange: (dirty: boolean) => void }) {
  const { t } = useTranslation();
  const { data: settings, error, isLoading, isValidating, mutate } = useSettings();
  const [rows, setRows] = useState<ToolPriceRow[]>([]);
  const [dirty, setDirty] = useState(false);
  const [saving, setSaving] = useState(false);
  // Errors keyed by `${rowIndex}:${column}`.
  const [errors, setErrors] = useState<Record<string, string>>({});
  // MP-UI5a: in-app route changes and tab closes ask before dropping the draft.
  const leaveGuard = useUnsavedChangesGuard(dirty);

  useEffect(() => {
    if (!dirty && settings) setRows(rowsFromSettings(settings.tool_prices ?? {}));
  }, [settings, dirty]);

  useEffect(() => onDirtyChange(dirty), [dirty, onDirtyChange]);

  const updateRows = (next: ToolPriceRow[]) => {
    setRows(next);
    setDirty(true);
    setErrors({});
  };

  const discard = () => {
    setDirty(false);
    setErrors({});
    if (settings) setRows(rowsFromSettings(settings.tool_prices ?? {}));
  };

  const validate = (): Record<string, string> => {
    const next: Record<string, string> = {};
    const seen = new Set<string>();
    rows.forEach((row, index) => {
      const usageClass = row.usageClass.trim();
      if (!usageClass) {
        next[`${index}:class`] = t("modelPricing.toolPrices.errorClass");
      } else if (seen.has(usageClass)) {
        next[`${index}:class`] = t("modelPricing.toolPrices.errorDuplicate", { class: usageClass });
      }
      seen.add(usageClass);
      if (!isValidUsdDecimal(row.usd.trim())) {
        next[`${index}:usd`] = t("modelPricing.toolPrices.errorPrice");
      }
      if (
        row.minimumUnits.trim() &&
        (!/^\d+$/.test(row.minimumUnits.trim()) ||
          Number.parseInt(row.minimumUnits.trim(), 10) < 1)
      ) {
        next[`${index}:minimum`] = t("modelPricing.toolPrices.errorMinimum");
      }
    });
    return next;
  };

  const save = async () => {
    if (!settings) return;
    const nextErrors = validate();
    setErrors(nextErrors);
    const firstInvalid = Object.keys(nextErrors)[0];
    if (firstInvalid) {
      document.getElementById(`tool-price-${firstInvalid.replace(":", "-")}`)?.focus();
      return;
    }
    setSaving(true);
    try {
      await patchPricingSettingsOptimistic({ tool_prices: rowsToToolPrices(rows) }, (saveError) =>
        toastApiError(saveError, t, t("modelPricing.toolPrices.saveFailed")),
      );
      setDirty(false);
      toast.success(t("modelPricing.toolPrices.saveSuccess"));
    } catch {
      return;
    } finally {
      setSaving(false);
    }
  };

  if (settings === undefined) {
    return isLoading ? (
      <TablePageSkeleton showToolbar />
    ) : (
      <QueryError onRetry={() => mutate()} retrying={isValidating} />
    );
  }

  const cellInput = (
    index: number,
    column: "class" | "usd" | "minimum",
    label: string,
    input: ReactElement<ComponentProps<typeof Input>>,
  ) => {
    const key = `${index}:${column}`;
    const inputId = `tool-price-${index}-${column}`;
    return (
      <>
        <Input
          {...input.props}
          id={inputId}
          aria-label={`${label} ${index + 1}`}
          aria-invalid={errors[key] ? true : undefined}
          aria-describedby={errors[key] ? `${inputId}-error` : undefined}
        />
        {errors[key] ? (
          <FieldError id={`${inputId}-error`} className="mt-1">
            {errors[key]}
          </FieldError>
        ) : null}
      </>
    );
  };

  const emptyRow: ToolPriceRow = { usageClass: "", usd: "", per: "1k_calls", minimumUnits: "" };

  return (
    <div className="space-y-6">
      {leaveGuard}
      {error ? <QueryError onRetry={() => mutate()} retrying={isValidating} stale={true} /> : null}
      <DataTableShell
        toolbar={
          <>
            <div className="flex items-center gap-2 text-base font-semibold">
              <Hammer className="h-5 w-5" aria-hidden="true" />
              {t("modelPricing.tabs.toolPrices")}
            </div>
            <Button variant="outline" className="ml-auto" onClick={() => updateRows([...rows, emptyRow])}>
              <Plus data-icon="inline-start" aria-hidden="true" />
              {t("modelPricing.toolPrices.addRow")}
            </Button>
          </>
        }
        isEmpty={rows.length === 0}
        emptyState={
          <EmptyState
            icon={<Hammer className="h-12 w-12" aria-hidden="true" />}
            title={t("modelPricing.toolPrices.empty")}
            description={t("modelPricing.toolPrices.emptyDesc")}
            action={
              <Button onClick={() => updateRows([emptyRow])}>
                <Plus data-icon="inline-start" aria-hidden="true" />
                {t("modelPricing.toolPrices.addRow")}
              </Button>
            }
          />
        }
      >
        <div className="overflow-x-auto">
          <table className="w-full caption-bottom text-sm">
            <thead className="[&_tr]:border-b">
              <tr className="border-b">
                <VirtualTableHeaderCell className="min-w-[240px]">
                  {t("modelPricing.toolPrices.usageClass")}
                </VirtualTableHeaderCell>
                <VirtualTableHeaderCell className="w-[160px]">
                  {t("modelPricing.toolPrices.price")}
                </VirtualTableHeaderCell>
                <VirtualTableHeaderCell className="w-[160px]">
                  {t("modelPricing.toolPrices.unit")}
                </VirtualTableHeaderCell>
                <VirtualTableHeaderCell className="w-[150px]">
                  {t("modelPricing.toolPrices.minimumUnits")}
                </VirtualTableHeaderCell>
                <VirtualTableHeaderCell className="w-[70px]">
                  {t("common.actions")}
                </VirtualTableHeaderCell>
              </tr>
            </thead>
            <tbody className="[&_tr:last-child]:border-0">
              {rows.map((row, index) => {
                const updateRow = (patch: Partial<ToolPriceRow>) =>
                  updateRows(rows.map((item, i) => (i === index ? { ...item, ...patch } : item)));
                const minimumEnabled = row.per !== "1k_calls";
                return (
                  <tr key={index} className="border-b align-top">
                    <VirtualTableCell>
                      {cellInput(
                        index,
                        "class",
                        t("modelPricing.toolPrices.usageClass"),
                        <Input
                          value={row.usageClass}
                          onChange={(event) => updateRow({ usageClass: event.target.value })}
                          placeholder="web_search"
                          className="font-mono"
                        />,
                      )}
                    </VirtualTableCell>
                    <VirtualTableCell>
                      {cellInput(
                        index,
                        "usd",
                        t("modelPricing.toolPrices.price"),
                        <Input
                          inputMode="decimal"
                          value={row.usd}
                          onChange={(event) => updateRow({ usd: event.target.value })}
                          placeholder="0.01"
                          className="font-mono"
                        />,
                      )}
                    </VirtualTableCell>
                    <VirtualTableCell>
                      <Select
                        value={row.per}
                        onValueChange={(per) =>
                          updateRow({
                            per: per as ToolPriceUnit,
                            ...(per === "1k_calls" ? { minimumUnits: "" } : {}),
                          })
                        }
                      >
                        <SelectTrigger aria-label={`${t("modelPricing.toolPrices.unit")} ${index + 1}`}>
                          <SelectValue />
                        </SelectTrigger>
                        <SelectContent>
                          {UNITS.map((unit) => (
                            <SelectItem key={unit} value={unit}>
                              {unit === "1k_calls"
                                ? t("modelPricing.toolPrices.unitCalls")
                                : unit === "minute"
                                  ? t("modelPricing.toolPrices.unitMinute")
                                  : t("modelPricing.toolPrices.unitSession")}
                            </SelectItem>
                          ))}
                        </SelectContent>
                      </Select>
                    </VirtualTableCell>
                    <VirtualTableCell>
                      {cellInput(
                        index,
                        "minimum",
                        t("modelPricing.toolPrices.minimumUnits"),
                        <Input
                          inputMode="numeric"
                          value={row.minimumUnits}
                          disabled={!minimumEnabled}
                          onChange={(event) => updateRow({ minimumUnits: event.target.value })}
                          placeholder={minimumEnabled ? "1" : "—"}
                          className="font-mono"
                        />,
                      )}
                    </VirtualTableCell>
                    <VirtualTableCell>
                      <Button
                        variant="ghost"
                        size="icon"
                        className="size-11 touch-manipulation sm:size-9"
                        aria-label={t("common.deleteItem", {
                          name: row.usageClass.trim() || `${t("modelPricing.toolPrices.usageClass")} ${index + 1}`,
                        })}
                        onClick={() => updateRows(rows.filter((_, i) => i !== index))}
                      >
                        <Trash2 className="text-error-foreground" />
                      </Button>
                    </VirtualTableCell>
                  </tr>
                );
              })}
            </tbody>
          </table>
        </div>
      </DataTableShell>
      <SettingsSaveBar open={dirty || saving} saving={saving} onDiscard={discard} onSave={() => void save()} />
    </div>
  );
}
