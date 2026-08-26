import { useState } from "react";
import { useTranslation } from "react-i18next";
import { CircleDollarSign, SearchX } from "lucide-react";
import { TableVirtuoso } from "react-virtuoso";
import { Button } from "@/components/ui/button";
import { EmptyState } from "@/components/ui/empty-state";
import { TablePageSkeleton } from "@/components/ui/page-skeleton";
import {
  DataTableShell,
  TableToolbarSearch,
  VirtualTableCell,
  VirtualTableHeaderCell,
} from "@/components/ui/data-table-shell";
import { ModelBadge } from "@/components/ModelBadge";
import { useModelPrices, useUnpricedModels } from "@/lib/swr";
import { PricingSheet } from "./PricingSheet";
import type { PricingSheetTarget } from "./shared";

export function UnpricedModelsTab() {
  const { t } = useTranslation();
  const { data: models = [], isLoading } = useUnpricedModels();
  const { data: priceRecords = [] } = useModelPrices();
  const [search, setSearch] = useState("");
  const [sheetTarget, setSheetTarget] = useState<PricingSheetTarget | null>(null);

  const filtered = models.filter((model) =>
    model.toLowerCase().includes(search.toLowerCase())
  );

  const openCreate = (modelId: string) =>
    setSheetTarget({ mode: "create", modelId, record: null });

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
        records={priceRecords}
      />
      <DataTableShell
        toolbar={
          <>
            <div className="flex items-center gap-2 text-base font-semibold">
              <SearchX className="h-5 w-5" />
              {t("modelPricing.tabs.unpricedModels", "Unpriced Models")}
              <span className="text-sm font-normal text-muted-foreground">
                {t("modelPricing.unpricedCount", "{{count}} models", {
                  count: filtered.length,
                })}
              </span>
            </div>
            <TableToolbarSearch
              value={search}
              onChange={(event) => setSearch(event.target.value)}
              placeholder={t("modelPricing.searchPlaceholder", "Search models...")}
            />
          </>
        }
        isEmpty={filtered.length === 0}
        emptyState={
          <EmptyState
            icon={<CircleDollarSign className="h-12 w-12" />}
            title={t("modelPricing.allPriced", "All routable models are priced")}
            description={t(
              "modelPricing.allPricedDesc",
              "Every model available for routing resolves to a price row."
            )}
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
              <tr {...props} className="border-b transition-colors hover:bg-muted/50" />
            ),
            TableBody: (props) => <tbody {...props} className="[&_tr:last-child]:border-0" />,
          }}
          fixedHeaderContent={() => (
            <tr className="border-b bg-background">
              <VirtualTableHeaderCell>
                {t("modelPricing.model", "Model")}
              </VirtualTableHeaderCell>
              <VirtualTableHeaderCell className="w-[140px]">
                {t("common.actions", "Actions")}
              </VirtualTableHeaderCell>
            </tr>
          )}
          itemContent={(_index, model) => (
            <>
              <VirtualTableCell>
                <ModelBadge model={model} showDetails={false} />
              </VirtualTableCell>
              <VirtualTableCell>
                <Button variant="outline" size="sm" onClick={() => openCreate(model)}>
                  <CircleDollarSign className="mr-1.5 h-3.5 w-3.5" />
                  {t("modelPricing.setPrice", "Set price")}
                </Button>
              </VirtualTableCell>
            </>
          )}
        />
      </DataTableShell>
    </>
  );
}
