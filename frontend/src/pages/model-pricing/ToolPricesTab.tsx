import { useEffect, useState } from "react";
import { useTranslation } from "react-i18next";
import { Hammer, Plus, Save, Trash2 } from "lucide-react";
import { toast } from "sonner";
import { Button } from "@/components/ui/button";
import { Input } from "@/components/ui/input";
import { EmptyState } from "@/components/ui/empty-state";
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
import type { SystemSettings, ToolPriceEntry, ToolPriceUnit } from "@/lib/api";
import { updateSettingsOptimistic, useSettings } from "@/lib/swr";
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

export function ToolPricesTab() {
  const { t } = useTranslation();
  const { data: settings, isLoading } = useSettings();
  const [rows, setRows] = useState<ToolPriceRow[]>([]);
  const [dirty, setDirty] = useState(false);
  const [saving, setSaving] = useState(false);

  useEffect(() => {
    if (!dirty && settings) setRows(rowsFromSettings(settings.tool_prices ?? {}));
  }, [settings, dirty]);

  const updateRows = (next: ToolPriceRow[]) => {
    setRows(next);
    setDirty(true);
  };

  const validate = (): string | null => {
    const classes = new Set<string>();
    for (const row of rows) {
      const usageClass = row.usageClass.trim();
      if (!usageClass) {
        return t("modelPricing.toolPrices.errorClass", "Usage class must not be empty");
      }
      if (classes.has(usageClass)) {
        return t("modelPricing.toolPrices.errorDuplicate", "Duplicate usage class: {{class}}", {
          class: usageClass,
        });
      }
      classes.add(usageClass);
      if (!isValidUsdDecimal(row.usd.trim())) {
        return t(
          "modelPricing.toolPrices.errorPrice",
          "Prices must be non-negative decimals with at most 9 fractional digits"
        );
      }
      if (
        row.minimumUnits.trim() &&
        (!/^\d+$/.test(row.minimumUnits.trim()) ||
          Number.parseInt(row.minimumUnits.trim(), 10) < 1)
      ) {
        return t(
          "modelPricing.toolPrices.errorMinimum",
          "Minimum units must be an integer of at least 1"
        );
      }
    }
    return null;
  };

  const save = async () => {
    if (!settings) return;
    const invalid = validate();
    if (invalid) {
      toast.error(invalid);
      return;
    }
    setSaving(true);
    const next: SystemSettings = { ...settings, tool_prices: rowsToToolPrices(rows) };
    try {
      await updateSettingsOptimistic(next, (error) =>
        toast.error(t("modelPricing.toolPrices.saveFailed", "Failed to save tool prices"), {
          description: error.message,
        })
      );
      setDirty(false);
      toast.success(t("modelPricing.toolPrices.saveSuccess", "Tool prices saved"));
    } catch {
      return;
    } finally {
      setSaving(false);
    }
  };

  if (isLoading && rows.length === 0) {
    return <TablePageSkeleton showToolbar />;
  }

  return (
    <DataTableShell
      toolbar={
        <>
          <div className="flex items-center gap-2 text-base font-semibold">
            <Hammer className="h-5 w-5" />
            {t("modelPricing.tabs.toolPrices", "Tool Prices")}
          </div>
          <div className="ml-auto flex items-center gap-2">
            <Button
              variant="outline"
              onClick={() =>
                updateRows([...rows, { usageClass: "", usd: "", per: "1k_calls", minimumUnits: "" }])
              }
            >
              <Plus className="mr-2 h-4 w-4" />
              {t("modelPricing.toolPrices.addRow", "Add class")}
            </Button>
            <Button onClick={() => void save()} disabled={saving || !dirty}>
              <Save className="mr-2 h-4 w-4" />
              {saving ? t("common.saving", "Saving...") : t("common.save", "Save")}
            </Button>
          </div>
        </>
      }
      isEmpty={rows.length === 0}
      emptyState={
        <EmptyState
          icon={<Hammer className="h-12 w-12" />}
          title={t("modelPricing.toolPrices.empty", "No tool prices")}
          description={t(
            "modelPricing.toolPrices.emptyDesc",
            "Server-native tool usage classes without a price settle at zero cost."
          )}
          action={
            <Button
              onClick={() =>
                updateRows([{ usageClass: "", usd: "", per: "1k_calls", minimumUnits: "" }])
              }
            >
              <Plus className="mr-2 h-4 w-4" />
              {t("modelPricing.toolPrices.addRow", "Add class")}
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
                {t("modelPricing.toolPrices.usageClass", "Usage class")}
              </VirtualTableHeaderCell>
              <VirtualTableHeaderCell className="w-[160px]">
                {t("modelPricing.toolPrices.price", "USD price")}
              </VirtualTableHeaderCell>
              <VirtualTableHeaderCell className="w-[160px]">
                {t("modelPricing.toolPrices.unit", "Per")}
              </VirtualTableHeaderCell>
              <VirtualTableHeaderCell className="w-[150px]">
                {t("modelPricing.toolPrices.minimumUnits", "Minimum units")}
              </VirtualTableHeaderCell>
              <VirtualTableHeaderCell className="w-[70px]">
                {t("common.actions", "Actions")}
              </VirtualTableHeaderCell>
            </tr>
          </thead>
          <tbody className="[&_tr:last-child]:border-0">
            {rows.map((row, index) => {
              const updateRow = (patch: Partial<ToolPriceRow>) =>
                updateRows(rows.map((item, i) => (i === index ? { ...item, ...patch } : item)));
              const minimumEnabled = row.per !== "1k_calls";
              return (
                <tr key={index} className="border-b">
                  <VirtualTableCell>
                    <Input
                      value={row.usageClass}
                      onChange={(event) => updateRow({ usageClass: event.target.value })}
                      placeholder="web_search"
                      className="font-mono"
                    />
                  </VirtualTableCell>
                  <VirtualTableCell>
                    <Input
                      inputMode="decimal"
                      value={row.usd}
                      onChange={(event) => updateRow({ usd: event.target.value })}
                      placeholder="0.01"
                      className="font-mono"
                    />
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
                      <SelectTrigger>
                        <SelectValue />
                      </SelectTrigger>
                      <SelectContent>
                        {UNITS.map((unit) => (
                          <SelectItem key={unit} value={unit}>
                            {unit === "1k_calls"
                              ? t("modelPricing.toolPrices.unitCalls", "1K calls")
                              : unit === "minute"
                                ? t("modelPricing.toolPrices.unitMinute", "Minute")
                                : t("modelPricing.toolPrices.unitSession", "Session")}
                          </SelectItem>
                        ))}
                      </SelectContent>
                    </Select>
                  </VirtualTableCell>
                  <VirtualTableCell>
                    <Input
                      inputMode="numeric"
                      value={row.minimumUnits}
                      disabled={!minimumEnabled}
                      onChange={(event) => updateRow({ minimumUnits: event.target.value })}
                      placeholder={minimumEnabled ? "1" : "—"}
                      className="font-mono"
                    />
                  </VirtualTableCell>
                  <VirtualTableCell>
                    <Button
                      variant="ghost"
                      size="icon"
                      className="size-11 touch-manipulation text-destructive hover:text-destructive sm:size-9"
                      aria-label={t("common.delete", "Delete")}
                      onClick={() => updateRows(rows.filter((_, i) => i !== index))}
                    >
                      <Trash2 className="h-4 w-4" />
                    </Button>
                  </VirtualTableCell>
                </tr>
              );
            })}
          </tbody>
        </table>
      </div>
    </DataTableShell>
  );
}
