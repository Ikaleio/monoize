import { useState } from "react";
import { useTranslation } from "react-i18next";
import { CircleDollarSign, Lock, Pencil, Plus, Trash2 } from "lucide-react";
import { TableVirtuoso } from "react-virtuoso";
import { Badge } from "@/components/ui/badge";
import { Button } from "@/components/ui/button";
import { StatusBadge } from "@/components/ui/status";
import { EmptyState } from "@/components/ui/empty-state";
import { TablePageSkeleton } from "@/components/ui/page-skeleton";
import {
  DataTableShell,
  TableToolbarSearch,
  VirtualTableCell,
  VirtualTableHeaderCell,
} from "@/components/ui/data-table-shell";
import { ModelBadge } from "@/components/ModelBadge";
import { useModelPrices } from "@/lib/swr";
import type { ModelPriceRecord } from "@/lib/api";
import { PricingSheet } from "./PricingSheet";
import { formatRelativeTime, formatUsdPerM, type PricingSheetTarget } from "./shared";

function modeLabel(record: ModelPriceRecord): string {
  if (record.billing_mode === "per_request") return "per_request";
  if (record.billing_mode === "tiered_expr") return "tiered";
  return "per_token";
}

export function ModelPricingTab() {
  const { t } = useTranslation();
  const { data: records = [], isLoading } = useModelPrices();
  const [search, setSearch] = useState("");
  const [sheetTarget, setSheetTarget] = useState<PricingSheetTarget | null>(null);

  const filtered = records.filter((record) =>
    record.model_id.toLowerCase().includes(search.toLowerCase())
  );

  const openEdit = (record: ModelPriceRecord) =>
    setSheetTarget({ mode: "edit", modelId: record.model_id, record });

  const openCreate = () => setSheetTarget({ mode: "create", modelId: "", record: null });

  if (isLoading) {
    return <TablePageSkeleton showToolbar />;
  }

  return (
    <>
      <PricingSheet
        target={sheetTarget}
        onOpenChange={(open) => {
          if (!open) setSheetTarget(null);
        }}
        records={records}
      />
      <DataTableShell
        toolbar={
          <>
            <div className="flex items-center gap-2 text-base font-semibold">
              <CircleDollarSign className="h-5 w-5" />
              {t("modelPricing.tabs.modelPricing", "Model Pricing")}
            </div>
            <TableToolbarSearch
              value={search}
              onChange={(event) => setSearch(event.target.value)}
              placeholder={t("modelPricing.searchPlaceholder", "Search models...")}
            />
            <div className="ml-auto flex items-center gap-2">
              <Button onClick={openCreate}>
                <Plus className="mr-2 h-4 w-4" />
                {t("modelPricing.addPrice", "Add Price")}
              </Button>
            </div>
          </>
        }
        isEmpty={filtered.length === 0}
        emptyState={
          <EmptyState
            icon={<CircleDollarSign className="h-12 w-12" />}
            title={t("modelPricing.noPrices", "No model prices yet")}
            description={t(
              "modelPricing.noPricesDesc",
              "Sync from an upstream source or add prices manually."
            )}
            action={
              <Button onClick={openCreate}>
                <Plus className="mr-2 h-4 w-4" />
                {t("modelPricing.addPrice", "Add Price")}
              </Button>
            }
          />
        }
      >
        <TableVirtuoso
          style={{ height: "calc(100dvh - 320px)", minHeight: 400 }}
          data={filtered}
          components={{
            Table: (props) => <table {...props} className="w-full caption-bottom text-sm" />,
            TableHead: (props) => <thead {...props} className="[&_tr]:border-b" />,
            TableRow: (props) => (
              <tr
                {...props}
                className="cursor-pointer border-b transition-colors hover:bg-muted/50"
              />
            ),
            TableBody: (props) => <tbody {...props} className="[&_tr:last-child]:border-0" />,
          }}
          fixedHeaderContent={() => (
            <tr className="border-b bg-background">
              <VirtualTableHeaderCell className="min-w-[220px]">
                {t("modelPricing.model", "Model")}
              </VirtualTableHeaderCell>
              <VirtualTableHeaderCell>
                {t("modelPricing.mode", "Mode")}
              </VirtualTableHeaderCell>
              <VirtualTableHeaderCell>
                {t("modelPricing.inputPrice", "Input $/1M")}
              </VirtualTableHeaderCell>
              <VirtualTableHeaderCell>
                {t("modelPricing.outputPrice", "Output $/1M")}
              </VirtualTableHeaderCell>
              <VirtualTableHeaderCell>
                {t("modelPricing.source", "Source")}
              </VirtualTableHeaderCell>
              <VirtualTableHeaderCell>
                {t("modelPricing.status", "Status")}
              </VirtualTableHeaderCell>
              <VirtualTableHeaderCell>
                {t("modelPricing.updated", "Updated")}
              </VirtualTableHeaderCell>
              <VirtualTableHeaderCell className="w-[80px]">
                {t("common.actions", "Actions")}
              </VirtualTableHeaderCell>
            </tr>
          )}
          itemContent={(_index, record) => (
            <>
              <VirtualTableCell onClick={() => openEdit(record)}>
                <ModelBadge model={record.model_id} showDetails={false} />
              </VirtualTableCell>
              <VirtualTableCell className="font-mono text-xs" onClick={() => openEdit(record)}>
                {modeLabel(record)}
              </VirtualTableCell>
              <VirtualTableCell className="font-mono text-xs" onClick={() => openEdit(record)}>
                {record.billing_mode === "per_request"
                  ? formatUsdPerM(record.per_request_usd)
                  : formatUsdPerM(record.input_usd_per_1m)}
              </VirtualTableCell>
              <VirtualTableCell className="font-mono text-xs" onClick={() => openEdit(record)}>
                {record.billing_mode === "per_request"
                  ? "—"
                  : formatUsdPerM(record.output_usd_per_1m)}
              </VirtualTableCell>
              <VirtualTableCell onClick={() => openEdit(record)}>
                <Badge
                  variant={record.source === "manual" ? "default" : "secondary"}
                  className="text-xs"
                >
                  {record.source}
                </Badge>
              </VirtualTableCell>
              <VirtualTableCell onClick={() => openEdit(record)}>
                <div className="flex items-center gap-1.5">
                  {record.enabled ? (
                    <StatusBadge variant="success">
                      {t("modelPricing.enabledBadge", "On")}
                    </StatusBadge>
                  ) : (
                    <Badge variant="outline" className="text-xs">
                      {t("modelPricing.disabledBadge", "Off")}
                    </Badge>
                  )}
                  {record.locked_fields.length > 0 ? (
                    <Badge variant="outline" className="gap-1 text-xs">
                      <Lock className="h-3 w-3" />
                      {record.locked_fields.length}
                    </Badge>
                  ) : null}
                </div>
              </VirtualTableCell>
              <VirtualTableCell
                className="text-xs text-muted-foreground"
                onClick={() => openEdit(record)}
              >
                {formatRelativeTime(record.updated_at)}
              </VirtualTableCell>
              <VirtualTableCell>
                <div className="flex items-center gap-1">
                  <Button
                    variant="ghost"
                    size="icon"
                    className="size-11 touch-manipulation sm:size-9"
                    aria-label={t("common.edit", "Edit")}
                    onClick={(event) => {
                      event.stopPropagation();
                      openEdit(record);
                    }}
                  >
                    <Pencil className="h-4 w-4" />
                  </Button>
                  <Button
                    variant="ghost"
                    size="icon"
                    className="size-11 touch-manipulation text-destructive hover:text-destructive sm:size-9"
                    aria-label={t("common.delete", "Delete")}
                    onClick={(event) => {
                      event.stopPropagation();
                      setSheetTarget({ mode: "edit", modelId: record.model_id, record });
                    }}
                  >
                    <Trash2 className="h-4 w-4" />
                  </Button>
                </div>
              </VirtualTableCell>
            </>
          )}
        />
      </DataTableShell>
    </>
  );
}
